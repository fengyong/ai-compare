import type {
  DirCellStatus,
  DirDeltaParams,
  DirOpenChildParams,
  DirOpenChildResult,
  DirOpenParams,
  DirOpenResult,
  DirRow,
  DirRowsParams,
  DirRowsResult,
  DirStats,
  FileRef,
  NodeKind,
  ScanProgress,
  ScanState,
  Section,
  SessionCloseParams,
  ShutdownResult,
  TextCloseParams,
  TextCloseResult,
  TextLinesParams,
  TextLinesResult,
  TextOpenParams,
  TextOpenResult,
  TextSectionsParams,
  TextSectionsResult,
} from '../protocol/types';
import { hashString, mulberry32 } from '../lib/env';

/**
 * 内置演示引擎（纯 TypeScript 实现的协议子集）。
 *
 * 用途：
 * - 浏览器 `pnpm dev` 时无需引擎二进制即可体验完整闭环（目录扫描渐进着色 → 双击 → 文本 diff）；
 * - Tauri 内引擎缺失时的优雅降级（见 ipc/engineProcess.ts）。
 *
 * 边界（重要）：这只影响 UI 开发体验，不参与任何测试断言与 golden 口径——
 * 协议正确性以引擎线（ai-engine）+ 协议录制回放测试为准（docs/04 §4.1 L3）。
 *
 * 实现的 P0 方法集：initialize / dir/open / dir/delta(通知) / dir/rows /
 * dir/openChild / text/open / text/sections / text/lines / text/close /
 * session/close / shutdown。dir/delta 模拟引擎的"后台扫描 + 批量上推"（ADR-004）。
 */

interface MockDirSession {
  id: string;
  left: string;
  right: string;
  rows: DirRow[];
  /** 已"扫描"出的行数（模拟后台渐进扫描） */
  revealed: number;
  rev: number;
  stats: DirStats;
  scanState: ScanState;
  progress: ScanProgress | null;
  pumpTimer: ReturnType<typeof setTimeout> | null;
}

interface MockTextSession {
  id: string;
  leftLines: string[];
  rightLines: string[];
  sections: Section[];
  summary: { sections: number; diffSections: number; important: number; unimportant: number };
  leftMeta: { file: FileRef; lineCount: number; lineEnding: 'lf' | 'crlf' | 'cr' | 'mixed'; encoding: string };
  rightMeta: { file: FileRef; lineCount: number; lineEnding: 'lf' | 'crlf' | 'cr' | 'mixed'; encoding: string };
  parent: { sessionId: string; rowId: number } | null;
}

const INITIAL_REVEAL = 40;
const REVEAL_STEP = 20;
const REVEAL_INTERVAL_MS = 30;

const TOP_DIRS = ['src', 'docs', 'config', 'tests', 'assets', 'scripts'];
const FILE_STEMS = [
  'main', 'config', 'utils', 'handler', 'service', 'model', 'index', 'app',
  'parser', 'render', 'store', 'types', 'api', 'core', 'helper', 'view',
];
const TEXT_EXTS = ['c', 'h', 'txt', 'md', 'json', 'ts', 'rs', 'xml', 'yml'];
const BIN_EXTS = ['png', 'so', 'dll', 'bin', 'dat'];

export class MockEngine {
  private sink?: (line: string) => void;
  private seq = 1;
  private readonly dirSessions = new Map<string, MockDirSession>();
  private readonly textSessions = new Map<string, MockTextSession>();
  private readonly timers = new Set<ReturnType<typeof setTimeout>>();
  private shuttingDown = false;

  attach(sink: (line: string) => void): void {
    this.sink = sink;
  }

  /** 收到一条来自客户端的 NDJSON 行 */
  receive(line: string): void {
    let msg: { id?: number; method?: string; params?: unknown };
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    const { id, method, params } = msg;
    if (!method) return;
    Promise.resolve(this.dispatch(method, params))
      .then((result) => {
        if (id !== undefined && id !== null) {
          this.emit({ jsonrpc: '2.0', id, result });
        }
      })
      .catch((err: unknown) => {
        if (id !== undefined && id !== null) {
          this.emit({
            jsonrpc: '2.0',
            id,
            error: { code: -32000, message: err instanceof Error ? err.message : String(err) },
          });
        }
      });
  }

  /** 清理全部定时器（进程退出/传输关闭时） */
  dispose(): void {
    this.shuttingDown = true;
    for (const t of this.timers) clearTimeout(t);
    this.timers.clear();
    for (const s of this.dirSessions.values()) {
      if (s.pumpTimer) clearTimeout(s.pumpTimer);
    }
  }

  // ------------------------------------------------------------------

  private emit(msg: unknown): void {
    if (this.shuttingDown) return;
    this.sink?.(JSON.stringify(msg));
  }

  private later(fn: () => void, ms: number): void {
    const t = setTimeout(() => {
      this.timers.delete(t);
      fn();
    }, ms);
    this.timers.add(t);
  }

  private dispatch(method: string, params: unknown): unknown {
    switch (method) {
      case 'initialize':
        return {
          protocolVersion: '1.0',
          engineInfo: { name: 'mock-engine', version: '0.1.0' },
          capabilities: {
            viewKinds: ['folder-compare', 'text-compare'],
            features: ['demo-data'],
          },
        };
      case 'dir/open':
        return this.dirOpen(params as DirOpenParams);
      case 'dir/rows':
        return this.dirRows(params as DirRowsParams);
      case 'dir/openChild':
        return this.dirOpenChild(params as DirOpenChildParams);
      case 'text/open':
        return this.textOpen(params as TextOpenParams);
      case 'text/sections':
        return this.textSections(params as TextSectionsParams);
      case 'text/lines':
        return this.textLines(params as TextLinesParams);
      case 'text/close':
        return this.textClose(params as TextCloseParams);
      case 'session/close':
        return this.sessionClose(params as SessionCloseParams);
      case 'shutdown': {
        const result: ShutdownResult = null;
        this.later(() => this.dispose(), 50);
        return result;
      }
      default:
        throw new Error(`方法未实现: ${method}`);
    }
  }

  // ---------------- 目录比较 ----------------

  private dirOpen(p: DirOpenParams): DirOpenResult {
    const sessionId = `d${this.seq++}`;
    const rows = buildDirModel(p.left, p.right);
    const revealed = Math.min(INITIAL_REVEAL, rows.length);
    const session: MockDirSession = {
      id: sessionId,
      left: p.left,
      right: p.right,
      rows,
      revealed,
      rev: 1,
      stats: computeStats(rows, revealed),
      scanState: revealed >= rows.length ? 'done' : 'running',
      progress: { scanned: revealed, total: rows.length },
      pumpTimer: null,
    };
    this.dirSessions.set(sessionId, session);
    if (session.scanState === 'running') {
      this.schedulePump(session);
    }
    return { sessionId, rowCount: revealed, scanState: session.scanState, rev: session.rev };
  }

  /** 模拟 ADR-004 的后台扫描：按批次上推 delta（16ms/500 条合并刷的演示近似） */
  private schedulePump(session: MockDirSession): void {
    session.pumpTimer = setTimeout(() => {
      session.pumpTimer = null;
      if (this.shuttingDown || !this.dirSessions.has(session.id)) return;
      const from = session.revealed;
      session.revealed = Math.min(session.revealed + REVEAL_STEP, session.rows.length);
      session.rev++;
      session.stats = computeStats(session.rows, session.revealed);
      const done = session.revealed >= session.rows.length;
      session.scanState = done ? 'done' : 'running';
      session.progress = { scanned: session.revealed, total: session.rows.length };
      const delta: DirDeltaParams = {
        sessionId: session.id,
        rev: session.rev,
        upserts: session.rows.slice(from, session.revealed).map((row, i) => ({ index: from + i, row })),
        removals: [],
        stats: session.stats,
        progress: session.progress,
        scanState: session.scanState,
        total: session.revealed,
      };
      this.emit({ jsonrpc: '2.0', method: 'dir/delta', params: delta });
      if (!done) this.schedulePump(session);
    }, REVEAL_INTERVAL_MS);
    this.timers.add(session.pumpTimer);
  }

  private dirRows(p: DirRowsParams): DirRowsResult {
    const s = this.requireDir(p.sessionId);
    const start = Math.max(0, Math.min(p.offset, s.revealed));
    const end = Math.min(start + Math.max(0, p.count), s.revealed);
    return {
      rev: s.rev,
      total: s.revealed,
      scanState: s.scanState,
      rows: s.rows.slice(start, end),
    };
  }

  private dirOpenChild(p: DirOpenChildParams): DirOpenChildResult {
    const s = this.requireDir(p.sessionId);
    const row = s.rows.find((r) => r.rowId === p.rowId);
    if (!row) throw new Error(`行不存在: ${p.rowId}`);
    if (row.kind !== 'file') throw new Error('仅文件行支持打开子会话（目录展开属 P2/P6 范围）');
    const isText = isTextName(row.name);
    return {
      kind: 'text-compare',
      viewKind: isText ? 'text' : 'hex',
      left: { path: joinPath(s.left, row.relPath), name: row.name },
      right: { path: joinPath(s.right, row.relPath), name: row.name },
    };
  }

  // ---------------- 文本比较 ----------------

  private textOpen(p: TextOpenParams): TextOpenResult {
    const sessionId = `t${this.seq++}`;
    const seedName = p.left.name || 'file';
    const gen = genTextPair(seedName);
    const meta = (file: FileRef, lines: string[]) => ({
      file,
      lineCount: lines.length,
      lineEnding: 'lf' as const,
      encoding: 'UTF-8',
    });
    this.textSessions.set(sessionId, {
      id: sessionId,
      leftLines: gen.leftLines,
      rightLines: gen.rightLines,
      sections: gen.sections,
      summary: gen.summary,
      leftMeta: meta(p.left, gen.leftLines),
      rightMeta: meta(p.right, gen.rightLines),
      parent: p.parent ?? null,
    });
    return {
      sessionId,
      left: this.textSessions.get(sessionId)!.leftMeta,
      right: this.textSessions.get(sessionId)!.rightMeta,
      rev: 1,
    };
  }

  private textSections(p: TextSectionsParams): TextSectionsResult {
    const s = this.requireText(p.sessionId);
    const sections = p.afterId !== undefined ? s.sections.filter((x) => x.id > p.afterId!) : s.sections;
    return { sessionId: s.id, sections, done: true, summary: s.summary };
  }

  private textLines(p: TextLinesParams): TextLinesResult {
    const s = this.requireText(p.sessionId);
    const all = p.side === 'left' ? s.leftLines : s.rightLines;
    const start = Math.max(0, Math.min(p.start, all.length));
    const end = Math.min(start + Math.max(0, p.count), all.length);
    return { side: p.side, start, lines: all.slice(start, end), done: end >= all.length };
  }

  /** 关闭文本会话：模拟引擎侧差异回传（docs/03 §5.1 —— 写回父目录行 fileOverride） */
  private textClose(p: TextCloseParams): TextCloseResult {
    const s = this.requireText(p.sessionId);
    this.textSessions.delete(s.id);
    let reported: DirCellStatus | null = null;
    if (p.reportStatus && s.parent) {
      const dirSession = this.dirSessions.get(s.parent.sessionId);
      if (dirSession) {
        reported = s.summary.diffSections > 0 ? 'contentDiff' : 'same';
        const idx = dirSession.rows.findIndex((r) => r.rowId === s.parent!.rowId);
        if (idx >= 0) {
          const row = { ...dirSession.rows[idx], fileOverride: reported };
          dirSession.rows[idx] = row;
          dirSession.rev++;
          dirSession.stats = computeStats(dirSession.rows, dirSession.revealed);
          const delta: DirDeltaParams = {
            sessionId: dirSession.id,
            rev: dirSession.rev,
            upserts: [{ index: idx, row }],
            removals: [],
            stats: dirSession.stats,
            scanState: dirSession.scanState,
            total: dirSession.revealed,
          };
          this.emit({ jsonrpc: '2.0', method: 'dir/delta', params: delta });
        }
      }
    }
    return { reportedStatus: reported };
  }

  private sessionClose(p: SessionCloseParams): null {
    const dir = this.dirSessions.get(p.sessionId);
    if (dir) {
      if (dir.pumpTimer) clearTimeout(dir.pumpTimer);
      this.dirSessions.delete(p.sessionId);
    }
    this.textSessions.delete(p.sessionId);
    return null;
  }

  private requireDir(id: string): MockDirSession {
    const s = this.dirSessions.get(id);
    if (!s) throw new Error(`目录会话不存在: ${id}`);
    return s;
  }

  private requireText(id: string): MockTextSession {
    const s = this.textSessions.get(id);
    if (!s) throw new Error(`文本会话不存在: ${id}`);
    return s;
  }
}

// ---------------- 目录模型生成（确定性） ----------------

function computeStats(rows: DirRow[], upto: number): DirStats {
  const stats: DirStats = {
    total: upto,
    same: 0,
    diff: 0,
    insignificant: 0,
    orphanLeft: 0,
    orphanRight: 0,
    error: 0,
    unknown: 0,
  };
  for (const row of rows.slice(0, upto)) {
    switch (row.status) {
      case 'same':
        stats.same++;
        break;
      case 'contentDiff':
      case 'ruleDiff':
      case 'newerLeft':
      case 'newerRight':
        stats.diff++;
        break;
      case 'insignificantDiff':
        stats.insignificant++;
        break;
      case 'orphanLeft':
        stats.orphanLeft++;
        break;
      case 'orphanRight':
        stats.orphanRight++;
        break;
      case 'error':
        stats.error++;
        break;
      default:
        stats.unknown++;
    }
  }
  return stats;
}

function pickStatus(rand: () => number): DirCellStatus {
  const r = rand();
  // 演示权重：同 52% / 差异 16% / 孤立左右各 8% / 较新 8% / 较旧 5% / 错误 3%
  if (r < 0.52) return 'same';
  if (r < 0.68) return 'contentDiff';
  if (r < 0.76) return 'orphanLeft';
  if (r < 0.84) return 'orphanRight';
  if (r < 0.92) return rand() < 0.5 ? 'newerLeft' : 'newerRight';
  if (r < 0.97) return rand() < 0.5 ? 'olderLeft' : 'olderRight';
  return 'error';
}

function statLite(rand: () => number): { size: number; mtimeMs: number } {
  return {
    size: Math.floor(rand() * 4_000_000),
    mtimeMs: Date.now() - Math.floor(rand() * 90 * 24 * 3600 * 1000),
  };
}

function buildDirModel(left: string, right: string): DirRow[] {
  const rand = mulberry32(hashString(`${left}|${right}`));
  const rows: DirRow[] = [];
  let rowId = 1;

  const pushRow = (name: string, relPath: string, kind: NodeKind, depth: number, status: DirCellStatus) => {
    const s1 = statLite(rand);
    const s2 = statLite(rand);
    const row: DirRow = {
      rowId: rowId++,
      name,
      relPath,
      kind,
      depth,
      status,
    };
    if (status !== 'orphanRight') row.left = { size: s1.size, mtimeMs: s1.mtimeMs };
    if (status !== 'orphanLeft') row.right = { size: s2.size, mtimeMs: s2.mtimeMs };
    rows.push(row);
  };

  // 顶层散文件
  const rootFiles = ['README.md', 'Makefile', 'version.json', 'main.c'];
  for (const name of rootFiles) {
    pushRow(name, name, 'file', 0, name === 'main.c' ? 'contentDiff' : pickStatus(rand));
  }

  // 顶层目录 + 子项 + 少量三级
  for (const dir of TOP_DIRS) {
    pushRow(dir, dir, 'dir', 0, pickStatus(rand) === 'same' ? 'same' : 'contentDiff');
    const childCount = 6 + Math.floor(rand() * 18);
    for (let i = 0; i < childCount; i++) {
      const ext = rand() < 0.6 ? TEXT_EXTS[Math.floor(rand() * TEXT_EXTS.length)] : BIN_EXTS[Math.floor(rand() * BIN_EXTS.length)];
      const name = `${FILE_STEMS[Math.floor(rand() * FILE_STEMS.length)]}${i}.${ext}`;
      const relPath = `${dir}/${name}`;
      // 文本文件更高概率为内容差异，便于演示双击链路
      let status = pickStatus(rand);
      if (TEXT_EXTS.includes(ext) && status !== 'contentDiff' && rand() < 0.35) status = 'contentDiff';
      if (rand() < 0.08) {
        // 少量二级目录（含孙项）
        pushRow(name, relPath, 'dir', 1, status === 'same' ? 'same' : 'contentDiff');
        const subCount = 2 + Math.floor(rand() * 4);
        for (let j = 0; j < subCount; j++) {
          const sub = `sub_${j}.${TEXT_EXTS[Math.floor(rand() * TEXT_EXTS.length)]}`;
          pushRow(sub, `${relPath}/${sub}`, 'file', 2, pickStatus(rand));
        }
      } else {
        pushRow(name, relPath, 'file', 1, status);
      }
    }
  }
  return rows;
}

function isTextName(name: string): boolean {
  const i = name.lastIndexOf('.');
  return i >= 0 && TEXT_EXTS.includes(name.slice(i + 1));
}

function joinPath(base: string, rel: string): string {
  return `${base.replace(/[\\/]+$/, '')}/${rel}`;
}

// ---------------- 文本对生成（记录式生成，无需跑 LCS） ----------------

interface GenTextPairResult {
  leftLines: string[];
  rightLines: string[];
  sections: Section[];
  summary: { sections: number; diffSections: number; important: number; unimportant: number };
}

function contentLine(n: number): string {
  return `    int value_${n} = ${n} * ${1 + (n % 7)};  /* total = ${n % 100} */`;
}

function mutateLine(line: string, rand: () => number): string {
  return line
    .replace(/= (\d+) \* (\d+);/, (_m, a: string, b: string) => `= ${a} * ${Number(b) + 1};`)
    .replace(/total = (\d+)( \*\/)?$/, (_m, _old: string, tail: string | undefined) => `total = ${Math.floor(rand() * 100)}${tail ?? ''}`);
}

function genTextPair(seed: string): GenTextPairResult {
  const rand = mulberry32(hashString(`text:${seed}`));
  const target = 600 + Math.floor(rand() * 800);
  const leftLines: string[] = [];
  const rightLines: string[] = [];
  const sections: Section[] = [];
  let secId = 0;
  let li = 0;
  let ri = 0;

  const pushEqual = (n: number) => {
    const sl = li;
    const sr = ri;
    for (let k = 0; k < n; k++) {
      const t = contentLine(li);
      leftLines.push(t);
      rightLines.push(t);
      li++;
      ri++;
    }
    sections.push({ id: secId++, kind: 'equal', left: { start: sl, end: li }, right: { start: sr, end: ri }, importance: 'important', inline: 'none' });
  };
  const pushChange = (n: number) => {
    const sl = li;
    const sr = ri;
    for (let k = 0; k < n; k++) {
      const t = contentLine(li);
      leftLines.push(t);
      rightLines.push(mutateLine(t, rand));
      li++;
      ri++;
    }
    sections.push({ id: secId++, kind: 'change', left: { start: sl, end: li }, right: { start: sr, end: ri }, importance: 'important', inline: 'none' });
  };
  const pushInsert = (n: number) => {
    const sr = ri;
    for (let k = 0; k < n; k++) {
      rightLines.push(`+ ${contentLine(ri).trim()}  (右侧新增)`);
      ri++;
    }
    sections.push({ id: secId++, kind: 'insert', left: null, right: { start: sr, end: ri }, importance: 'important', inline: 'none' });
  };
  const pushDelete = (n: number) => {
    const sl = li;
    for (let k = 0; k < n; k++) {
      leftLines.push(`- ${contentLine(li).trim()}  (左侧独有)`);
      li++;
    }
    sections.push({ id: secId++, kind: 'delete', left: { start: sl, end: li }, right: null, importance: 'important', inline: 'none' });
  };

  // 文件头（相同）
  pushEqual(4 + Math.floor(rand() * 4));
  while (li + ri < target) {
    const roll = rand();
    if (roll < 0.62) pushEqual(8 + Math.floor(rand() * 40));
    else if (roll < 0.8) pushChange(2 + Math.floor(rand() * 6));
    else if (roll < 0.9) pushInsert(1 + Math.floor(rand() * 5));
    else pushDelete(1 + Math.floor(rand() * 5));
  }
  // 文件尾（相同）
  pushEqual(3 + Math.floor(rand() * 3));

  let diffSections = 0;
  for (const s of sections) if (s.kind !== 'equal') diffSections++;
  return {
    leftLines,
    rightLines,
    sections,
    summary: {
      sections: sections.length,
      diffSections,
      important: diffSections,
      unimportant: 0,
    },
  };
}
