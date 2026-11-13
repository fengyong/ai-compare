import type {
  DiffSummary,
  DirDelta,
  DirEntryStatus,
  DirExecuteOp,
  DirExecuteResult,
  DirFilterParams,
  DirFilterResult,
  DirOpenParams,
  DirOpenResult,
  DirOperateParams,
  DirOperateResult,
  DirRow,
  DirStats,
  DisplayFilterSpec,
  InitializeResult,
  NameFilterSpec,
  PlannedOperation,
  Section,
  SectionKind,
  SessionCloseParams,
  ShutdownResult,
  SideInfo,
  SyncPreset,
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
import { applyDisplayToRows, fileAllowedBySpec, SHOW_ALL_DISPLAY_FILTER, wildcardMatch } from '../views/folder-compare/dirFilters';
import { parentOf } from '../views/folder-compare/dirTree';

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
 * - `text/lines`、`text/inlineDiff`（P3 生成类型，P3 已合入引擎）；
 * - `dir/expand`、`dir/filter`、`dir/operate`（P2 生成类型，真实引擎已开放；
 *   mock 同形实现供浏览器演示树展开/过滤器/同步计划）；
 * - `dir/execute`（P6 提案协议，protocol/types.ts 第 3 节；真实引擎待
 *   协议变更合入，mock 同形实现操作落盘到演示模型）。
 * 能力位 inlineDiff=true、linesRead=true、dirFilters=true、dirOperate=true。
 */

/**
 * mock 目录会话：树行（名称过滤后）+ 显示状态 + 平铺派生。
 * dir/expand/filter/execute 的落盘对象。
 */
interface MockDirSession {
  id: string;
  /** 名称过滤后保留的树形行（hidden/suppressed 为显示标记） */
  treeRows: DirRow[];
  nameFilter: NameFilterSpec | null;
  displayFilter: DisplayFilterSpec;
  suppress: boolean;
  flat: boolean;
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
  /** dir/filter 差分基线（会话 id -> 上次发出的行 id 集） */
  private readonly lastIds = new Map<string, string[]>();
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
      case 'dir/expand':
        return this.dirExpand(params as { sessionId: string; path: string; depth: number | null });
      case 'dir/filter':
        return this.dirFilter(params as DirFilterParams);
      case 'dir/operate':
        return { result: this.dirOperate(params as DirOperateParams), notifications: [] };
      case 'dir/execute':
        return this.dirExecute(params as { sessionId: string; ops: DirExecuteOp[]; dryRun?: boolean | null });
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
        // P6：dir/filter / dir/operate mock 同形实现（演示树展开/过滤器/同步计划）
        dirFilters: true,
        dirOperate: true,
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
    const session: MockDirSession = {
      id: p.sessionId,
      treeRows: rows,
      nameFilter: p.nameFilter ?? null,
      displayFilter: p.displayFilter ?? SHOW_ALL_DISPLAY_FILTER,
      suppress: false,
      flat: p.flat === true,
    };
    applyNameFilter(session);
    refreshDisplayFlags(session);
    this.dirSessions.set(p.sessionId, session);
    this.lastIds.set(p.sessionId, currentRows(session).map((r) => r.id));
    return {
      result: this.openResultFor(p.sessionId, session),
      notifications: this.deltaBatchesFor(p.sessionId, session),
    };
  }

  /** 应答体（行数与扫描态；P2 语义：mock 全量即时完成） */
  private openResultFor(sessionId: string, session: MockDirSession): DirOpenResult {
    const rows = currentRows(session);
    return {
      sessionId,
      rowCount: rows.length,
      rev: 1 as unknown as bigint,
      scanState: 'completed',
    };
  }

  /** dir/delta 批次（应答后推送；P6 起含过滤后的可见行与标记） */
  private deltaBatchesFor(sessionId: string, session: MockDirSession): DirDelta[] {
    const rows = currentRows(session);
    const stats = computeStats(rows);
    const batchCount = Math.max(1, Math.ceil(rows.length / MAX_BATCH_ROWS));
    const deltas: DirDelta[] = [];
    for (let i = 0; i < batchCount; i++) {
      const upserts = rows.slice(i * MAX_BATCH_ROWS, (i + 1) * MAX_BATCH_ROWS);
      deltas.push({
        sessionId,
        // 线上语义为 JSON number（ts-rs 将 u64 映射为 bigint，JSON.parse 实际产出 number）
        rev: (i + 1) as unknown as bigint,
        upserts,
        removals: [],
        stats,
        progress: { scanned: Math.min((i + 1) * MAX_BATCH_ROWS, rows.length), total: rows.length },
        batch: { index: i, hasMore: i + 1 < batchCount },
      });
    }
    return deltas;
  }

  // ---------------- P6：dir/expand（树展开） ----------------

  /**
   * dir/expand 同形实现：返回直接子项数并重推这些子项（upsert 幂等，
   * UI 缓存按 id 合并）。真实引擎为惰性扫描出口；mock 全量预建后按需补推。
   */
  private dirExpand(p: { sessionId: string; path: string; depth: number | null }): { result: unknown; notifications: DirDelta[] } {
    const s = this.requireDir(p.sessionId);
    const depth = Math.max(1, p.depth ?? 1);
    const direct = s.treeRows.filter((r) => parentOf(r.id) === p.path && !r.hidden);
    const stats = computeStats(currentRows(s));
    const rev = 2 as unknown as bigint;
    // depth>1 时同形补推更深层（mock 全量已在缓存，upsert 幂等无害）
    const all = new Map<string, DirRow>();
    const collect = (parent: string, level: number): void => {
      for (const row of s.treeRows) {
        if (parentOf(row.id) === parent) {
          all.set(row.id, row);
          if (row.kind === 'dir' && level + 1 < depth) collect(row.id, level + 1);
        }
      }
    };
    collect(p.path, 1);
    const upserts = [...all.values()];
    const delta: DirDelta = {
      sessionId: s.id,
      rev,
      upserts,
      removals: [],
      stats,
      progress: { scanned: stats.total, total: stats.total },
      batch: { index: 0, hasMore: false },
    };
    return {
      result: {
        sessionId: s.id,
        rev,
        newRows: direct.length,
        stats,
        progress: delta.progress,
      },
      notifications: [delta],
    };
  }

  // ---------------- P6：dir/filter（三类过滤器 + 平铺） ----------------

  /**
   * dir/filter 同形实现：名称过滤（重算保留行）/ 显示过滤 + 抑制（仅标记）/
   * 平铺切换（重派生行集），全量重推 delta（removals 携带被移除 id）。
   */
  private dirFilter(p: DirFilterParams): { result: DirFilterResult; notifications: DirDelta[] } {
    const s = this.requireDir(p.sessionId);
    let changed = false;
    if (p.nameFilter) {
      s.nameFilter = p.nameFilter;
      applyNameFilter(s);
      changed = true;
    }
    if (p.displayFilter) {
      s.displayFilter = p.displayFilter;
      changed = true;
    }
    if (p.suppress != null) {
      s.suppress = p.suppress;
      changed = true;
    }
    if (p.flat != null && p.flat !== s.flat) {
      s.flat = p.flat;
      changed = true;
    }
    if (!changed) {
      const rows = currentRows(s);
      return {
        result: {
          sessionId: s.id,
          rev: 3 as unknown as bigint,
          visibleRows: rows.filter((r) => !r.hidden).length,
          hiddenRows: rows.filter((r) => r.hidden).length,
          stats: computeStats(rows),
        },
        notifications: [],
      };
    }
    refreshDisplayFlags(s);
    // 全量切换语义：旧 id 集合与新 id 集合差分（树↔平铺/名称过滤后的行集变化）
    const before = new Set(this.lastIds.get(s.id) ?? []);
    const rows = currentRows(s);
    const after = new Set(rows.map((r) => r.id));
    const removals = [...before].filter((id) => !after.has(id));
    this.lastIds.set(s.id, [...after]);
    const stats = computeStats(rows);
    const delta: DirDelta = {
      sessionId: s.id,
      rev: 4 as unknown as bigint,
      upserts: rows,
      removals,
      stats,
      progress: { scanned: stats.total, total: stats.total },
      batch: { index: 0, hasMore: false },
    };
    return {
      result: {
        sessionId: s.id,
        rev: 4 as unknown as bigint,
        visibleRows: rows.filter((r) => !r.hidden).length,
        hiddenRows: rows.filter((r) => r.hidden).length,
        stats,
      },
      notifications: [delta],
    };
  }

  // ---------------- P6：dir/operate（同步计划，不执行） ----------------

  /**
   * dir/operate 同形实现（与引擎 sync.rs plan_operations 同口径的 5 预设；
   * FR-DIR-068：hidden 行不进入计划）。
   */
  private dirOperate(p: DirOperateParams): DirOperateResult {
    const s = this.requireDir(p.sessionId);
    const visible = currentRows(s).filter((r) => !r.hidden);
    const plan = mockPlanOperations(visible, p.preset ?? null, p.paths ?? null);
    return { sessionId: s.id, plan, total: plan.length };
  }

  // ---------------- P6 提案：dir/execute（操作执行，同形模拟） ----------------

  /**
   * dir/execute 同形模拟（提案协议见 protocol/types.ts 第 3 节）：
   * 把操作落盘到演示模型（copy→对侧补齐/move→删源/delete→移除侧/
   * rename→改 id 与 name（含子树前缀）/touch→对时/mkdir→新目录行），
   * 差异经 dir/delta 回推；dryRun 只校验不落盘。护栏与真实执行层同形：
   * 空/越界路径拒绝、源缺失失败、逐项收集部分成功。
   */
  private dirExecute(p: { sessionId: string; ops: DirExecuteOp[]; dryRun?: boolean | null }): { result: DirExecuteResult; notifications: DirDelta[] } {
    const s = this.requireDir(p.sessionId);
    const dryRun = p.dryRun === true;
    const succeeded: DirExecuteResult['succeeded'] = [];
    const failed: DirExecuteResult['failed'] = [];
    const skipped: DirExecuteResult['skipped'] = [];
    const before = new Map(s.treeRows.map((r) => [r.id, r] as const));
    const touched = new Set<string>();
    const removed = new Set<string>();

    for (const op of p.ops ?? []) {
      const guard = sanitizeOpPath(op);
      if (guard) {
        failed.push({ op, status: 'failed', detail: guard });
        continue;
      }
      const row = before.get(op.path);
      try {
        switch (op.kind) {
          case 'copy':
          case 'move': {
            if (!row || !row[op.side]) {
              failed.push({ op, status: 'failed', detail: `源不存在（${op.side}）: ${op.path}` });
              break;
            }
            const targetSide = op.targetSide ?? (op.side === 'left' ? 'right' : 'left');
            applyCopyToRow(row, targetSide);
            if (op.kind === 'move') {
              row[op.side] = null;
              recomputeRowStatus(row);
              if (!row.left && !row.right) removed.add(row.id);
            }
            touched.add(row.id);
            succeeded.push({ op, status: 'succeeded', detail: null });
            break;
          }
          case 'delete': {
            if (!row || !row[op.side]) {
              failed.push({ op, status: 'failed', detail: `目标不存在（${op.side}）: ${op.path}` });
              break;
            }
            row[op.side] = null;
            recomputeRowStatus(row);
            if (!row.left && !row.right) {
              removed.add(row.id);
            } else {
              touched.add(row.id);
            }
            succeeded.push({ op, status: 'succeeded', detail: null });
            break;
          }
          case 'rename': {
            if (!row) {
              failed.push({ op, status: 'failed', detail: `源不存在（${op.side}）: ${op.path}` });
              break;
            }
            if (!op.newName || op.newName.includes('/') || op.newName === '.' || op.newName === '..') {
              failed.push({ op, status: 'failed', detail: `新名称非法: ${op.newName ?? ''}` });
              break;
            }
            if (before.has(`${parentOf(op.path)}/${op.newName}`) && parentOf(op.path) + '/' + op.newName !== op.path) {
              failed.push({ op, status: 'failed', detail: `目标名已存在: ${op.newName}` });
              break;
            }
            const newId = parentOf(op.path) ? `${parentOf(op.path)}/${op.newName}` : op.newName!;
            // 子树前缀重写
            for (const other of s.treeRows) {
              if (other.id === op.path) continue;
              if (other.id.startsWith(`${op.path}/`)) {
                other.id = newId + other.id.slice(op.path.length);
                touched.add(other.id);
              }
            }
            row.id = newId;
            row.name = op.newName;
            touched.add(row.id);
            succeeded.push({ op, status: 'succeeded', detail: null });
            break;
          }
          case 'touch': {
            if (!row || !row[op.side]) {
              failed.push({ op, status: 'failed', detail: `目标不存在（${op.side}）: ${op.path}` });
              break;
            }
            const otherSide = op.side === 'left' ? 'right' : 'left';
            const mtime = op.mtimeMs && op.mtimeMs > 0 ? op.mtimeMs : row[otherSide]?.mtimeMs ?? row[op.side]!.mtimeMs;
            row[op.side]!.mtimeMs = mtime;
            recomputeRowStatus(row);
            touched.add(row.id);
            succeeded.push({ op, status: 'succeeded', detail: null });
            break;
          }
          case 'mkdir': {
            if (row) {
              failed.push({ op, status: 'failed', detail: `目标已存在（${op.side}）: ${op.path}` });
              break;
            }
            const created: DirRow = {
              id: op.path,
              name: op.path.split('/').pop() ?? op.path,
              kind: 'dir',
              left: op.side === 'left' ? mockSideInfo() : null,
              right: op.side === 'right' ? mockSideInfo() : null,
              status: op.side === 'left' ? 'leftOnly' : 'rightOnly',
              hasChildren: false,
              hidden: false,
              suppressed: false,
            };
            s.treeRows.push(created);
            touched.add(created.id);
            succeeded.push({ op, status: 'succeeded', detail: null });
            break;
          }
        }
      } catch (err) {
        failed.push({ op, status: 'failed', detail: err instanceof Error ? err.message : String(err) });
      }
    }

    if (!dryRun && (touched.size > 0 || removed.size > 0)) {
      s.treeRows = s.treeRows.filter((r) => !removed.has(r.id));
      refreshDisplayFlags(s);
    }
    const rows = currentRows(s);
    const stats = computeStats(rows);
    let notifications: DirDelta[] = [];
    if (!dryRun && (touched.size > 0 || removed.size > 0)) {
      const upserts = rows.filter((r) => touched.has(r.id));
      notifications = [
        {
          sessionId: s.id,
          rev: 5 as unknown as bigint,
          upserts,
          removals: [...removed],
          stats,
          progress: { scanned: stats.total, total: stats.total },
          batch: { index: 0, hasMore: false },
        },
      ];
    }
    return {
      result: {
        sessionId: s.id,
        total: (p.ops ?? []).length,
        succeeded: dryRun ? succeeded.map((r) => ({ ...r, detail: `DRY: ${r.op.kind} ${r.op.path}` })) : succeeded,
        failed,
        skipped,
        dryRun,
        cancelled: false,
      },
      notifications,
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
    this.lastIds.delete(p.sessionId);
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

  private requireDir(id: string): MockDirSession {
    const s = this.dirSessions.get(id);
    if (!s) {
      if (this.textSessions.has(id)) throw new InvalidParams(`会话不是目录会话: ${id}`);
      throw new RpcErr(CODE.SESSION_NOT_FOUND, `会话不存在: ${id}`);
    }
    return s;
  }
}

// ==================================================================
// mock 目录会话数据面（P6：过滤/平铺/操作落盘的辅助纯函数）
// ==================================================================

/** 当前表示（平铺 = 派生行；树 = 名称过滤后的树行） */
function currentRows(s: MockDirSession): DirRow[] {
  return s.flat ? deriveFlatRows(s) : s.treeRows;
}

/** 名称过滤（FR-DIR-031 子集：include/exclude 文件掩码 + 排除目录掩码） */
function applyNameFilter(s: MockDirSession): void {
  const spec = s.nameFilter;
  if (!spec || (spec.includeFiles.length === 0 && spec.excludeFiles.length === 0 && spec.excludeDirs.length === 0 && spec.includeDirs.length === 0)) {
    return;
  }
  s.treeRows = s.treeRows.filter((row) => {
    const segments = row.id.split('/').filter((p) => p.length > 0);
    if (row.kind === 'dir') {
      // 目录仅按排除掩码裁剪（include 目录语义由引擎全集承担；mock 近似保留）
      if (spec.excludeDirs.length > 0 && segments.length > 0) {
        for (const mask of spec.excludeDirs) {
          if (wildcard(mask, segments[segments.length - 1])) return false;
        }
      }
      return true;
    }
    return fileAllowedBySpec(segments, spec);
  });
}

/** 显示标记重算（FR-DIR-033/035：hidden 与 suppressed 互斥，抑制模式青绿显示） */
function refreshDisplayFlags(s: MockDirSession): void {
  s.treeRows = applyDisplayToRows(s.treeRows, s.displayFilter, s.suppress);
}

/** 平铺派生（FR-DIR-021 近似：跨目录按名对齐文件；目录行不出现） */
function deriveFlatRows(s: MockDirSession): DirRow[] {
  type Occ = { rel: string; name: string; info: SideInfo | null };
  const groups = new Map<string, { ls: Occ[]; rs: Occ[] }>();
  for (const row of s.treeRows) {
    if (row.kind !== 'file' || row.hidden) continue;
    const key = row.name.toLowerCase();
    const g = groups.get(key) ?? { ls: [], rs: [] };
    if (row.left) g.ls.push({ rel: row.id, name: row.name, info: row.left });
    if (row.right) g.rs.push({ rel: row.id, name: row.name, info: row.right });
    groups.set(key, g);
  }
  const out: DirRow[] = [];
  for (const [key, g] of groups) {
    const n = Math.max(g.ls.length, g.rs.length);
    for (let i = 0; i < n; i++) {
      const l = g.ls[i];
      const r = g.rs[i];
      const name = l?.name ?? r?.name ?? key;
      const id = n === 1 ? key : `${key}#${i}`;
      const status: DirEntryStatus = l && r ? statusForPair(l.info!, r.info!) : l ? 'leftOnly' : 'rightOnly';
      out.push({
        id,
        name,
        kind: 'file',
        left: l?.info ?? null,
        right: r?.info ?? null,
        status,
        hasChildren: false,
        hidden: false,
        suppressed: false,
      });
    }
  }
  return out;
}

function statusForPair(l: SideInfo, r: SideInfo): DirEntryStatus {
  return l.size === r.size && l.mtimeMs === r.mtimeMs ? 'same' : 'contentDiff';
}

/** 操作路径护栏（与引擎 sanitize_rel 同形） */
function sanitizeOpPath(op: DirExecuteOp): string | null {
  const path = op.path;
  if (!path) return '拒绝空路径（根目录不可作为操作目标）';
  if (path.startsWith('/') || path.includes('\\')) return `拒绝绝对/反斜杠路径: ${path}`;
  for (const seg of path.split('/')) {
    if (!seg || seg === '.' || seg === '..') return `拒绝相对引用段: ${path}`;
  }
  return null;
}

/** 复制/移动落盘：目标侧补齐源信息（演示模型内即"文件已复制"） */
function applyCopyToRow(row: DirRow, targetSide: 'left' | 'right'): void {
  const sourceSide = targetSide === 'left' ? 'right' : 'left';
  const src = row[sourceSide];
  if (!src) return;
  row[targetSide] = { ...src };
  recomputeRowStatus(row);
}

/** 简化状态重算（mock 演示口径：size+mtime 相等 = same，否则 contentDiff；单侧 = 孤立） */
function recomputeRowStatus(row: DirRow): void {
  if (row.kind === 'dir') {
    row.status = row.left && row.right ? 'dirSame' : row.left ? 'leftOnly' : row.right ? 'rightOnly' : 'unknown';
    return;
  }
  if (row.left && row.right) {
    row.status = statusForPair(row.left, row.right);
  } else if (row.left) {
    row.status = 'leftOnly';
  } else if (row.right) {
    row.status = 'rightOnly';
  } else {
    row.status = 'unknown';
  }
}

function mockSideInfo(): SideInfo {
  return { size: 0, mtimeMs: Date.now(), attribute: null, unixMode: 0, uid: 0, gid: 0 };
}

/**
 * mock 同步计划（与引擎 sync.rs plan_operations 同口径的 5 预设子集；
 * 目录聚合状态 DirDiff 参与 copy 语义、mirroring 删除另一侧孤立项）。
 */
function mockPlanOperations(rows: DirRow[], preset: SyncPreset | null, paths: string[] | null): PlannedOperation[] {
  if (!preset) return [];
  const sel = paths && paths.length > 0 ? new Set(paths) : null;
  const plan: PlannedOperation[] = [];
  const isDiff = (st: DirEntryStatus) => st === 'contentDiff' || st === 'quickDiff' || st === 'dirDiff';
  for (const row of rows) {
    if (sel && !sel.has(row.id)) continue;
    const reason = `preset:${preset}`;
    const push = (op: PlannedOperation['op']) => plan.push({ op, path: row.id, reason });
    const mt = (info: SideInfo | null) => info?.mtimeMs ?? 0;
    switch (row.status) {
      case 'leftOnly':
        if (preset === 'mirrorLeftToRight' || preset === 'updateLeftToRight') push('copyLeftToRight');
        else if (preset === 'mirrorRightToLeft') push('deleteLeft');
        break;
      case 'rightOnly':
        if (preset === 'mirrorRightToLeft' || preset === 'updateRightToLeft') push('copyRightToLeft');
        else if (preset === 'mirrorLeftToRight') push('deleteRight');
        break;
      default:
        if (isDiff(row.status)) {
          const leftOlder = mt(row.left) < mt(row.right);
          if (preset === 'mirrorLeftToRight') push('copyLeftToRight');
          else if (preset === 'updateLeftToRight') {
            if (!leftOlder) push('copyLeftToRight');
          } else if (preset === 'mirrorRightToLeft') push('copyRightToLeft');
          else if (preset === 'updateRightToLeft') {
            if (mt(row.right) >= mt(row.left)) push('copyRightToLeft');
          } else if (preset === 'updateBoth') {
            if (mt(row.left) > mt(row.right)) push('copyLeftToRight');
            else if (mt(row.right) > mt(row.left)) push('copyRightToLeft');
            else push('copyLeftToRight');
          }
        }
        break;
    }
  }
  return plan;
}

/** 本文件内通配符别名（dirFilters.wildcardMatch） */
function wildcard(mask: string, name: string): boolean {
  return wildcardMatch(mask, name, true);
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
