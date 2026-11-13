import type {
  DiffSummary,
  DirDelta,
  DirEntryStatus,
  DirOpenParams,
  DirOpenResult,
  DirRow,
  DirStats,
  InitializeResult,
  Section,
  SectionKind,
  SessionCloseParams,
  ShutdownResult,
  SideInfo,
  TextLinesParams,
  TextLinesResult,
  TextOpenParams,
  TextOpenResult,
  TextInlineDiffParams,
  TextInlineDiffResult,
  TextSectionsParams,
  TextSectionsResult,
} from '../protocol/types';
import type { InlineSpanDto } from '../protocol/generated/InlineSpanDto';
import { wordDiff } from '../lib/inlineDiffCore';
import { hashString, mulberry32 } from '../lib/env';

/**
 * 内置演示引擎（纯 TypeScript 实现的协议子集）。
 *
 * 用途：
 * - 浏览器 `pnpm dev` 时无需引擎二进制即可体验完整闭环（目录扫描 → 双击 → 文本 diff）；
 * - Tauri 内引擎缺失时的优雅降级（见 ipc/engineProcess.ts）。
 *
 * 边界（重要）：这只影响 UI 开发体验，不参与任何测试断言与 golden 口径——
 * 协议正确性以引擎线（ai-engine）+ 协议录制回放测试为准（docs/04 §4.1 L3）。
 *
 * 方法集与真实 ai-engine 的 P0 子集严格对齐（handlers.rs）：initialize /
 * dir/open（应答后按批推 dir/delta）/ text/open / text/sections /
 * session/close / shutdown；错误码同 error.rs（-32601 方法不存在、411 会话已存在、
 * 410 会话不存在）。mock 扩展（引擎入口开放前的同形先行）：
 * `text/lines`（P3 生成类型 TextLinesParams/Result）与
 * `text/inlineDiff`（P3 生成类型 TextInlineDiffParams/Result：按 section
 * 懒计算，绝对行号 + InlineSpanDto 成对区间）。能力位 inlineDiff=true、linesRead=true。
 */

interface MockDirSession {
  id: string;
  rows: DirRow[];
  stats: DirStats;
}

interface MockTextSession {
  id: string;
  leftLines: string[];
  rightLines: string[];
  sections: Section[];
  summary: DiffSummary;
}

const MAX_BATCH_ROWS = 512;

const TEXT_EXTS = ['c', 'h', 'txt', 'md', 'json', 'ts', 'rs', 'xml', 'yml'];
const BIN_EXTS = ['png', 'so', 'dll', 'bin', 'dat'];

const TOP_DIRS = ['src', 'docs', 'config', 'tests', 'assets', 'scripts'];
const FILE_STEMS = [
  'main', 'config', 'utils', 'handler', 'service', 'model', 'index', 'app',
  'parser', 'render', 'store', 'types', 'api', 'core', 'helper', 'view',
];

/** JSON-RPC 错误码（与 ai-protocol error::codes 对齐） */
const CODE = {
  METHOD_NOT_FOUND: -32601,
  INVALID_PARAMS: -32602,
  SESSION_NOT_FOUND: 410,
  SESSION_EXISTS: 411,
} as const;

export class MockEngine {
  private sink?: (line: string) => void;
  private readonly dirSessions = new Map<string, MockDirSession>();
  private readonly textSessions = new Map<string, MockTextSession>();
  private disposed = false;

  attach(sink: (line: string) => void): void {
    this.sink = sink;
  }

  /** 收到一条来自客户端的 NDJSON 行 */
  receive(line: string): void {
    if (this.disposed) return;
    let msg: { id?: number | string; method?: string; params?: unknown };
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    const { id, method, params } = msg;
    if (!method) return;
    Promise.resolve()
      .then(() => this.dispatch(method, params))
      .then((outcome) => {
        // 与真实引擎一致（handlers.rs Handled）：先应答，再依序发通知
        if (id !== undefined && id !== null) {
          this.emit({ jsonrpc: '2.0', id, result: outcome.result });
        }
        for (const delta of outcome.notifications) {
          this.emit({ jsonrpc: '2.0', method: 'dir/delta', params: delta });
        }
      })
      .catch((err: { code?: number; message?: string }) => {
        if (id !== undefined && id !== null) {
          this.emit({
            jsonrpc: '2.0',
            id,
            error: {
              code: typeof err?.code === 'number' ? err.code : -32000,
              message: err instanceof Error ? err.message : String(err?.message ?? err),
            },
          });
        }
      });
  }

  /** 清理（传输关闭时） */
  dispose(): void {
    this.disposed = true;
  }

  // ------------------------------------------------------------------

  private emit(msg: unknown): void {
    if (this.disposed) return;
    this.sink?.(JSON.stringify(msg));
  }

  private dispatch(method: string, params: unknown): { result: unknown; notifications: DirDelta[] } {
    switch (method) {
      case 'initialize':
        return { result: this.initialize(), notifications: [] };
      case 'dir/open':
        return this.dirOpen(params as DirOpenParams);
      case 'text/open':
        return { result: this.textOpen(params as TextOpenParams), notifications: [] };
      case 'text/sections':
        return { result: this.textSections(params as TextSectionsParams), notifications: [] };
      case 'text/lines':
        return { result: this.textLines(params as TextLinesParams), notifications: [] };
      case 'text/inlineDiff':
        return { result: this.textInlineDiff(params as TextInlineDiffParams), notifications: [] };
      case 'session/close':
        return { result: this.sessionClose(params as SessionCloseParams), notifications: [] };
      case 'shutdown': {
        const result: ShutdownResult = null;
        this.dispose();
        return { result, notifications: [] };
      }
      default:
        throw new MethodNotFound(method);
    }
  }

  // ---------------- 握手 ----------------

  private initialize(): InitializeResult {
    return {
      protocolVersion: '1.0',
      engine: { name: 'mock-engine', version: '0.1.0' },
      capabilities: {
        dirCompare: true,
        textCompare: true,
        // mock 实现了 text/inlineDiff（P3 生成协议同形演示面）
        inlineDiff: true,
        linesRead: true,
        cancel: true,
        deltaBatching: true,
        maxDeltaBatchRows: MAX_BATCH_ROWS,
        dirBackgroundScan: false,
        dirFilters: false,
        dirOperate: false,
      },
      license: { proUnlocked: true },
    };
  }

  // ---------------- 目录比较 ----------------

  private dirOpen(p: DirOpenParams): { result: DirOpenResult; notifications: DirDelta[] } {
    if (!p || typeof p.sessionId !== 'string' || p.sessionId === '') {
      throw new InvalidParams('sessionId 不能为空');
    }
    if (this.dirSessions.has(p.sessionId) || this.textSessions.has(p.sessionId)) {
      throw new RpcErr(CODE.SESSION_EXISTS, `会话已存在: ${p.sessionId}`);
    }
    const rows = buildDirModel(p.left, p.right);
    const stats = computeStats(rows);
    this.dirSessions.set(p.sessionId, { id: p.sessionId, rows, stats });

    // 与真实引擎一致的输出顺序：先应答，再按批推 dir/delta（batch.rs）
    const batchCount = Math.max(1, Math.ceil(rows.length / MAX_BATCH_ROWS));
    const deltas: DirDelta[] = [];
    for (let i = 0; i < batchCount; i++) {
      const upserts = rows.slice(i * MAX_BATCH_ROWS, (i + 1) * MAX_BATCH_ROWS);
      deltas.push({
        sessionId: p.sessionId,
        // 线上语义为 JSON number（ts-rs 将 u64 映射为 bigint，JSON.parse 实际产出 number）
        rev: (i + 1) as unknown as bigint,
        upserts,
        removals: [],
        stats,
        progress: { scanned: Math.min((i + 1) * MAX_BATCH_ROWS, rows.length), total: rows.length },
        batch: { index: i, hasMore: i + 1 < batchCount },
      });
    }
    return {
      result: {
        sessionId: p.sessionId,
        rowCount: rows.length,
        rev: batchCount as unknown as bigint,
        scanState: 'completed',
      },
      notifications: deltas,
    };
  }

  // ---------------- 文本比较 ----------------

  private textOpen(p: TextOpenParams): TextOpenResult {
    if (!p || typeof p.sessionId !== 'string' || p.sessionId === '') {
      throw new InvalidParams('sessionId 不能为空');
    }
    if (this.dirSessions.has(p.sessionId) || this.textSessions.has(p.sessionId)) {
      throw new RpcErr(CODE.SESSION_EXISTS, `会话已存在: ${p.sessionId}`);
    }
    const seed = baseName(p.left ?? '') || baseName(p.right ?? '') || 'file';
    const gen = genTextPair(seed);
    const summary = summarize(gen.sections);
    this.textSessions.set(p.sessionId, {
      id: p.sessionId,
      leftLines: gen.leftLines,
      rightLines: gen.rightLines,
      sections: gen.sections,
      summary,
    });
    return {
      sessionId: p.sessionId,
      leftLineCount: gen.leftLines.length,
      rightLineCount: gen.rightLines.length,
      sectionCount: gen.sections.length,
      summary,
    };
  }

  private textSections(p: TextSectionsParams): TextSectionsResult {
    const s = this.requireText(p.sessionId);
    const total = s.sections.length;
    const start = Math.max(0, Math.min(p.offset, total));
    const end = Math.min(start + Math.max(0, p.count), total);
    return {
      sessionId: s.id,
      offset: start,
      total,
      hasMore: end < total,
      sections: s.sections.slice(start, end),
    };
  }

  /** mock 扩展（引擎 P1 开放）：行内容分页 */
  private textLines(p: TextLinesParams): TextLinesResult {
    const s = this.requireText(p.sessionId);
    const all = p.side === 'left' ? s.leftLines : s.rightLines;
    const start = Math.max(0, Math.min(p.start, all.length));
    const end = Math.min(start + Math.max(0, p.count), all.length);
    return { side: p.side, start, lines: all.slice(start, end), done: end >= all.length };
  }

  /**
   * text/inlineDiff（P3 生成协议同形实现）：按 section 懒计算行内差异。
   * 复用客户端 wordDiff 词级 LCS，产出 InlineSpanDto（绝对行号 + 成对区间）；
   * 超长行（>1000 字符，与引擎 DEFAULT_INLINE_MAX_LINE_CHARS 对齐）返回
   * spans 空 + truncated=true（Partial 状态语义）。
   */
  private textInlineDiff(p: TextInlineDiffParams): TextInlineDiffResult {
    const s = this.requireText(p.sessionId);
    const section = s.sections.find((sec) => sec.id === p.sectionId);
    if (!section || !section.left || !section.right) {
      throw new InvalidParams(`section 不存在或非双侧替换段: ${p.sectionId}`);
    }
    const lineCount = Math.min(section.left.end - section.left.start, section.right.end - section.right.start);
    const lines = [];
    let anyTruncated = false;
    for (let k = 0; k < lineCount; k++) {
      const li = section.left.start + k;
      const ri = section.right.start + k;
      const l = s.leftLines[li] ?? '';
      const r = s.rightLines[ri] ?? '';
      if (l.length > 1000 || r.length > 1000) {
        lines.push({ leftLine: li, rightLine: ri, spans: [], truncated: true });
        anyTruncated = true;
        continue;
      }
      const pairs = wordDiff(l, r);
      const spans: InlineSpanDto[] = [];
      for (const pair of pairs) {
        if (pair.equal) continue;
        spans.push({
          left: pair.left ?? null,
          right: pair.right ?? null,
          kind: pair.left && pair.right ? 'replace' : pair.right ? 'insert' : 'delete',
        });
      }
      lines.push({ leftLine: li, rightLine: ri, spans, truncated: false });
    }
    return {
      sessionId: s.id,
      sectionId: p.sectionId,
      kind: section.kind,
      state: anyTruncated ? 'partial' : 'full',
      lines,
    };
  }

  private sessionClose(p: SessionCloseParams): null {
    this.dirSessions.delete(p.sessionId);
    this.textSessions.delete(p.sessionId);
    return null;
  }

  private requireText(id: string): MockTextSession {
    const s = this.textSessions.get(id);
    if (!s) {
      if (this.dirSessions.has(id)) throw new InvalidParams(`会话不是文本会话: ${id}`);
      throw new RpcErr(CODE.SESSION_NOT_FOUND, `会话不存在: ${id}`);
    }
    return s;
  }
}

class RpcErr extends Error {
  constructor(readonly code: number, message: string) {
    super(message);
    this.name = 'RpcErr';
  }
}

class MethodNotFound extends RpcErr {
  constructor(method: string) {
    super(CODE.METHOD_NOT_FOUND, `未知方法: ${method}`);
  }
}

class InvalidParams extends RpcErr {
  constructor(message: string) {
    super(CODE.INVALID_PARAMS, message);
  }
}

// ---------------- 目录模型生成（确定性） ----------------

function computeStats(rows: DirRow[]): DirStats {
  const stats: DirStats = {
    total: rows.length,
    same: 0,
    quickDiff: 0,
    contentDiff: 0,
    leftOnly: 0,
    rightOnly: 0,
    dirSame: 0,
    dirDiff: 0,
  };
  for (const row of rows) {
    switch (row.status) {
      case 'same':
        stats.same++;
        break;
      case 'quickDiff':
        stats.quickDiff++;
        break;
      case 'contentDiff':
        stats.contentDiff++;
        break;
      case 'leftOnly':
        stats.leftOnly++;
        break;
      case 'rightOnly':
        stats.rightOnly++;
        break;
      case 'dirSame':
        stats.dirSame++;
        break;
      // 'unknown'：DirStats 无对应桶，演示模型不产生该状态
    }
  }
  return stats;
}

function pickStatus(rand: () => number): DirEntryStatus {
  const r = rand();
  // 演示权重：同 50% / 内容不同 16% / 快速不同 8% / 孤立左右各 8% / 目录 10%
  if (r < 0.5) return 'same';
  if (r < 0.66) return 'contentDiff';
  if (r < 0.74) return 'quickDiff';
  if (r < 0.82) return 'leftOnly';
  if (r < 0.9) return 'rightOnly';
  return 'dirSame';
}

function sideInfo(rand: () => number, missing: boolean): SideInfo | null {
  if (missing) return null;
  return {
    size: Math.floor(rand() * 4_000_000),
    mtimeMs: Date.now() - Math.floor(rand() * 90 * 24 * 3600 * 1000),
    attribute: null,
    unixMode: 0,
    uid: 0,
    gid: 0,
  };
}

function buildDirModel(left: string, right: string): DirRow[] {
  const rand = mulberry32(hashString(`${left}|${right}`));
  const rows: DirRow[] = [];

  const pushRow = (
    name: string,
    id: string,
    kind: DirRow['kind'],
    status: DirEntryStatus,
    hasChildren: boolean
  ) => {
    rows.push({
      id,
      name,
      kind,
      left: sideInfo(rand, status === 'rightOnly'),
      right: sideInfo(rand, status === 'leftOnly'),
      status,
      hasChildren,
      hidden: false,
      suppressed: false,
    });
  };

  // 顶层散文件
  const rootFiles = ['README.md', 'Makefile', 'version.json', 'main.c'];
  for (const name of rootFiles) {
    pushRow(name, name, 'file', name === 'main.c' ? 'contentDiff' : pickStatus(rand), false);
  }

  // 顶层目录 + 子项 + 少量三级
  for (const dir of TOP_DIRS) {
    pushRow(dir, dir, 'dir', 'dirSame', true);
    const childCount = 6 + Math.floor(rand() * 18);
    for (let i = 0; i < childCount; i++) {
      const ext = rand() < 0.6 ? TEXT_EXTS[Math.floor(rand() * TEXT_EXTS.length)] : BIN_EXTS[Math.floor(rand() * BIN_EXTS.length)];
      const name = `${FILE_STEMS[Math.floor(rand() * FILE_STEMS.length)]}${i}.${ext}`;
      const id = `${dir}/${name}`;
      let status = pickStatus(rand);
      // 文本文件更高概率为内容差异，便于演示双击链路
      if (TEXT_EXTS.includes(ext) && status !== 'contentDiff' && rand() < 0.35) status = 'contentDiff';
      if (rand() < 0.08) {
        // 少量二级目录（含孙项）
        pushRow(name, id, 'dir', 'dirSame', true);
        const subCount = 2 + Math.floor(rand() * 4);
        for (let j = 0; j < subCount; j++) {
          const sub = `sub_${j}.${TEXT_EXTS[Math.floor(rand() * TEXT_EXTS.length)]}`;
          pushRow(sub, `${id}/${sub}`, 'file', pickStatus(rand), false);
        }
      } else {
        pushRow(name, id, 'file', status, false);
      }
    }
  }
  return rows;
}

function baseName(p: string): string {
  const i = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\'));
  return i >= 0 ? p.slice(i + 1) : p;
}

// ---------------- 文本对生成（记录式生成，无需跑 LCS） ----------------

interface GenTextPairResult {
  leftLines: string[];
  rightLines: string[];
  sections: Section[];
}

function contentLine(n: number, ext: string): string {
  const k = 1 + (n % 7);
  switch (ext) {
    case 'json':
    case 'jsonc':
      return `  "key_${n}": ${n * k},`;
    case 'md':
    case 'markdown':
      return n % 5 === 0 ? `## Section ${n}` : `Body text ${n} with \`code_${n}\` and *emph*.`;
    case 'py':
    case 'pyi':
      return `def func_${n}():  # total ${n % 100}\n    return ${n} * ${k}`;
    case 'rs':
      return `    let value_${n}: i32 = ${n} * ${k};`;
    case 'yml':
      return `  key_${n}: ${n * k}`;
    default:
      return `    int value_${n} = ${n} * ${k};  /* total = ${n % 100} */`;
  }
}

/** 行内变化：把行中最后一个数字改写（确定性 rand），保持行结构不变 */
function mutateLine(line: string, rand: () => number): string {
  const m = line.match(/(\d+)(?!\d)/g);
  if (!m || m.length === 0) return line;
  const last = m[m.length - 1];
  const idx = line.lastIndexOf(last);
  return line.slice(0, idx) + String(Math.floor(rand() * 100)) + line.slice(idx + last.length);
}

function genTextPair(seed: string): GenTextPairResult {
  const ext = seed.includes('.') ? seed.split('.').pop()!.toLowerCase() : '';
  const rand = mulberry32(hashString(`text:${seed}`));
  const target = 600 + Math.floor(rand() * 800);
  const leftLines: string[] = [];
  const rightLines: string[] = [];
  const sections: Section[] = [];
  let secId = 0;
  let li = 0;
  let ri = 0;

  const push = (kind: SectionKind, n: number) => {
    const sl = li;
    const sr = ri;
    if (kind === 'equal') {
      for (let k = 0; k < n; k++) {
        const t = contentLine(li, ext);
        leftLines.push(t);
        rightLines.push(t);
        li++;
        ri++;
      }
      sections.push({ id: secId++, kind, left: { start: sl, end: li }, right: { start: sr, end: ri }, important: 'important', inline: 'notComputed' });
    } else if (kind === 'change') {
      for (let k = 0; k < n; k++) {
        const t = contentLine(li, ext);
        leftLines.push(t);
        rightLines.push(mutateLine(t, rand));
        li++;
        ri++;
      }
      sections.push({ id: secId++, kind, left: { start: sl, end: li }, right: { start: sr, end: ri }, important: 'important', inline: 'notComputed' });
    } else if (kind === 'insert') {
      for (let k = 0; k < n; k++) {
        rightLines.push(`+ ${contentLine(ri, ext).trim()}  (右侧新增)`);
        ri++;
      }
      sections.push({ id: secId++, kind, left: null, right: { start: sr, end: ri }, important: 'important', inline: 'notComputed' });
    } else {
      for (let k = 0; k < n; k++) {
        leftLines.push(`- ${contentLine(li, ext).trim()}  (左侧独有)`);
        li++;
      }
      sections.push({ id: secId++, kind, left: { start: sl, end: li }, right: null, important: 'important', inline: 'notComputed' });
    }
  };

  // 文件头（相同）
  push('equal', 4 + Math.floor(rand() * 4));
  while (li + ri < target) {
    const roll = rand();
    if (roll < 0.62) push('equal', 8 + Math.floor(rand() * 40));
    else if (roll < 0.8) push('change', 2 + Math.floor(rand() * 6));
    else if (roll < 0.9) push('insert', 1 + Math.floor(rand() * 5));
    else push('delete', 1 + Math.floor(rand() * 5));
  }
  // 文件尾（相同）
  push('equal', 3 + Math.floor(rand() * 3));

  return { leftLines, rightLines, sections };
}

/** 与引擎 DiffSummary 口径一致：按 section 类别与行数统计 */
function summarize(sections: Section[]): DiffSummary {
  const summary: DiffSummary = {
    sectionCount: sections.length,
    equalSections: 0,
    insertSections: 0,
    deleteSections: 0,
    changeSections: 0,
    equalLines: 0,
    insertedLines: 0,
    deletedLines: 0,
  };
  for (const s of sections) {
    if (s.kind === 'equal') {
      summary.equalSections++;
      summary.equalLines += (s.left?.end ?? 0) - (s.left?.start ?? 0);
    } else if (s.kind === 'insert') {
      summary.insertSections++;
      summary.insertedLines += (s.right?.end ?? 0) - (s.right?.start ?? 0);
    } else if (s.kind === 'delete') {
      summary.deleteSections++;
      summary.deletedLines += (s.left?.end ?? 0) - (s.left?.start ?? 0);
    } else {
      summary.changeSections++;
    }
  }
  return summary;
}
