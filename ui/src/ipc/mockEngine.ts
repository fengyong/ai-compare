import type {
  DiffSummary,
  DirDelta,
  DirEntryStatus,
  DirExecuteOp,
  DirExecuteResult,
  DirFilterParams,
  DirFilterResult,
  DirMergeCategory,
  DirMergeExecuteParams,
  DirMergeExecuteResult,
  DirMergePlanParams,
  DirMergePlanResult,
  DirMergeRow,
  DirMergeRowState,
  DirMergeScanParams,
  DirMergeScanResult,
  DirMergeScanStats,
  DirOpenParams,
  DirOpenResult,
  DirOperateParams,
  DirOperateResult,
  DirRow,
  DirStats,
  DisplayFilterSpec,
  EntryKind,
  InitializeResult,
  MergeDecision,
  MergeOpKind,
  MergePlannedOp,
  MergeTarget,
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
  PictureOpenParams,
  TableOpenParams,
  TableSectionsParams,
  HexOpenParams,
  HexBytesParams,
  LineRange,
  MergeChunkDto,
  MergeChunkOrigin,
  MergeConflictKind,
  MergeConflictRowDto,
  MergeDecideParams,
  MergeDecideResult,
  MergeDecisionDto,
  MergeDecisionKind,
  MergeMarkerStyle,
  MergeOpenParams,
  MergeOpenResult,
  MergeOutputParams,
  MergeOutputResult,
  MergeSectionsParams,
  MergeSectionsResult,
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

/** P8：mock 三路合并会话（扫描行快照 + 计划/执行的落盘对象） */
interface MockMergeSession {
  id: string;
  rows: DirMergeRow[];
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
  private readonly mergeSessions = new Map<string, MockMergeSession>();
  private readonly tableSessions = new Map<string, { seed: number; columns: Array<{ name: string; ty: string; left: number | null; right: number | null }>; rowCount?: number }>();
  private readonly hexSessions = new Map<string, { seed: number; leftSize: number; rightSize: number }>();
  /** P7：三向文本合并会话（块模板 + 显式决策表 + rev） */
  private readonly merge3Sessions = new Map<string, MockMerge3Session>();
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
      case 'dir/mergeScan':
        return { result: this.mergeScanP(params as DirMergeScanParams), notifications: [] };
      case 'dir/mergePlan':
        return { result: this.mergePlanP(params as DirMergePlanParams), notifications: [] };
      case 'dir/mergeExecute':
        return { result: this.mergeExecuteP(params as DirMergeExecuteParams), notifications: [] };;
      case 'text/open':
        return { result: this.textOpen(params as TextOpenParams), notifications: [] };
      case 'text/sections':
        return { result: this.textSections(params as TextSectionsParams), notifications: [] };
      case 'text/lines':
        return { result: this.textLines(params as TextLinesParams), notifications: [] };
      case 'picture/open':
        return { result: this.pictureOpen(params as PictureOpenParams), notifications: [] };
      case 'hex/open':
        return { result: this.hexOpen(params as HexOpenParams), notifications: [] };
      case 'hex/bytes':
        return { result: this.hexBytes(params as HexBytesParams), notifications: [] };
      case 'table/open':
        return { result: this.tableOpen(params as TableOpenParams), notifications: [] };
      case 'table/sections':
        return { result: this.tableSections(params as TableSectionsParams), notifications: [] };
      // P7：三向文本合并四方法（同形演示；与 handlers.rs merge3_* 语义一致）
      case 'text/merge3/open':
        return { result: this.merge3Open(params as MergeOpenParams), notifications: [] };
      case 'text/merge3/sections':
        return { result: this.merge3Sections(params as MergeSectionsParams), notifications: [] };
      case 'text/merge3/decide':
        return { result: this.merge3Decide(params as MergeDecideParams), notifications: [] };
      case 'text/merge3/output':
        return { result: this.merge3Output(params as MergeOutputParams), notifications: [] };
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
        // P7：text/merge3/* 四方法已同形实现
        threeWayMerge: true,
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


  // ---------------- P8：dir/mergeScan / mergePlan / mergeExecute（同形模拟） ----------------

  /**
   * dir/mergeScan 同形实现：确定性生成 base/left/right 三树演示模型并做
   * 三方分类（与引擎 merge3way::scan_merge 的分类语义同口径：三方同 /
   * 单侧改 / 两侧同改 / 冲突 / 删除跟随 / 孤立）。
   */
  private mergeScanP(p: DirMergeScanParams): DirMergeScanResult {
    if (!p || typeof p.sessionId !== 'string' || p.sessionId === '') {
      throw new InvalidParams('sessionId 不能为空');
    }
    if (this.dirSessions.has(p.sessionId)) {
      throw new RpcErr(CODE.SESSION_EXISTS, `会话已存在: ${p.sessionId}`);
    }
    // 重扫描幂等（同 dir/expand upsert 语义）：刷新/StrictMode 双调用安全
    const rows = buildMergeModel(p.base, p.left, p.right);
    this.mergeSessions.set(p.sessionId, { id: p.sessionId, rows });
    return { sessionId: p.sessionId, rows, stats: mergeScanStats(rows) };
  }

  /** dir/mergePlan 同形实现：与引擎 plan_merge 同口径（决策继承 + 默认决策表） */
  private mergePlanP(p: DirMergePlanParams): DirMergePlanResult {
    const m = this.requireMerge(p.sessionId);
    const ops = mockMergePlan(m.rows, p.target, p.decisions ?? {});
    const stats = { total: ops.length, copied: 0, deleted: 0, touched: 0, skipped: 0, failed: 0 };
    for (const op of ops) {
      if (op.kind === 'takeLeft' || op.kind === 'takeRight') stats.copied++;
      else if (op.kind === 'deleteTarget') stats.deleted++;
      else stats.skipped++;
    }
    return { sessionId: m.id, target: p.target, ops, stats };
  }

  /** dir/mergeExecute 同形模拟：对演示模型落盘（覆盖目标侧信息/删除），逐项收集结果 */
  private mergeExecuteP(p: DirMergeExecuteParams): DirMergeExecuteResult {
    const m = this.requireMerge(p.sessionId);
    const byPath = new Map(m.rows.map((r) => [r.relPath, r]));
    const details: DirMergeExecuteResult['details'] = [];
    let copied = 0;
    let deleted = 0;
    let skipped = 0;
    let failed = 0;
    for (const op of p.ops ?? []) {
      const row = byPath.get(op.path);
      if (!row) {
        failed++;
        details.push({ kind: op.kind, path: op.path, reason: op.reason, status: 'failed', detail: '路径不存在于合并模型' });
        continue;
      }
      if (op.kind === 'keep') {
        skipped++;
        details.push({ kind: op.kind, path: op.path, reason: op.reason, status: 'planned', detail: null });
        continue;
      }
      const targetSide: 'left' | 'right' = p.target === 'right' ? 'right' : 'left';
      const guard = sanitizeOpPath({ kind: op.kind === 'deleteTarget' ? 'delete' : 'copy', path: op.path, side: targetSide });
      if (guard) {
        failed++;
        details.push({ kind: op.kind, path: op.path, reason: op.reason, status: 'failed', detail: guard });
        continue;
      }
      if (op.kind === 'deleteTarget') {
        if (p.target === 'output') {
          deleted++;
          details.push({ kind: op.kind, path: op.path, reason: op.reason, status: 'deleted', detail: null });
          continue;
        }
        if (!row[targetSide]) {
          skipped++;
          details.push({ kind: op.kind, path: op.path, reason: op.reason, status: 'skipped', detail: '目标侧不存在' });
          continue;
        }
        row[targetSide] = null;
        recomputeMergeRow(row);
        deleted++;
        details.push({ kind: op.kind, path: op.path, reason: op.reason, status: 'deleted', detail: null });
        continue;
      }
      // takeLeft / takeRight：目标侧被对应源覆盖（output 目标落第三目录，演示模型不持有）
      if (p.target === 'output') {
        copied++;
        details.push({ kind: op.kind, path: op.path, reason: op.reason, status: 'copied', detail: '输出目录（演示模型不持有）' });
        continue;
      }
      const sourceSide: 'left' | 'right' = op.kind === 'takeLeft' ? 'left' : 'right';
      const src = row[sourceSide];
      if (!src) {
        failed++;
        details.push({ kind: op.kind, path: op.path, reason: op.reason, status: 'failed', detail: `源侧不存在（${sourceSide}）` });
        continue;
      }
      row[targetSide] = { ...src };
      recomputeMergeRow(row);
      copied++;
      details.push({ kind: op.kind, path: op.path, reason: op.reason, status: 'copied', detail: null });
    }
    const log: string[] = [
      `合并目标：${p.target === 'left' ? '左侧' : p.target === 'right' ? '右侧' : '输出文件夹'}`,
      `合并完成：${(p.ops ?? []).length} 项 — 复制 ${copied} / 删除 ${deleted} / 跳过 ${skipped} / 失败 ${failed}`,
    ];
    for (const d of details) {
      if (d.status === 'failed') log.push(`失败 ${d.path}: ${d.kind} — ${d.detail ?? ''}`);
    }
    return {
      sessionId: m.id,
      target: p.target,
      total: (p.ops ?? []).length,
      copied,
      deleted,
      touched: 0,
      skipped,
      failed,
      cancelled: false,
      details,
      log,
    };
  }

  private requireMerge(id: string): MockMergeSession {
    const m = this.mergeSessions.get(id);
    if (!m) {
      throw new RpcErr(CODE.SESSION_NOT_FOUND, `会话不存在: ${id}`);
    }
    return m;
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

  /** P14：图片比较演示（确定性伪随机差异块；不产真实像素——浏览器演示走视图 canvas 合成图） */
  private pictureOpen(p: PictureOpenParams): {
    sessionId: string;
    width: number;
    height: number;
    leftWidth: number;
    leftHeight: number;
    rightWidth: number;
    rightHeight: number;
    diffPixels: number;
    totalPixels: number;
    similarityPct: number;
    blocks: Array<{ x: number; y: number; w: number; h: number; diffRatio: number }>;
    truncated: boolean;
  } {
    if (!p || typeof p.sessionId !== 'string' || p.sessionId === '') {
      throw new InvalidParams('sessionId 不能为空');
    }
    if (this.dirSessions.has(p.sessionId) || this.textSessions.has(p.sessionId)) {
      throw new RpcErr(CODE.SESSION_EXISTS, `会话已存在: ${p.sessionId}`);
    }
    const seed = (p.left ?? '').length + (p.right ?? '').length;
    const w = 512 + (seed % 3) * 128;
    const h = 384 + (seed % 2) * 64;
    const block = p.options?.block ?? 32;
    const tolerance = p.options?.tolerance ?? 8;
    const bx = Math.ceil(w / block);
    const by = Math.ceil(h / block);
    const blocks: Array<{ x: number; y: number; w: number; h: number; diffRatio: number }> = [];
    let diffPixels = 0;
    let rnd = (seed * 2654435761) % 4294967296 || 1;
    for (let iy = 0; iy < by; iy++) {
      for (let ix = 0; ix < bx; ix++) {
        rnd = (rnd * 1103515245 + 12345) % 2147483648;
        if (rnd % 10 < 2) {
          const ratio = 0.1 + (rnd % 100) / 250; // 0.1~0.5
          const cnt = Math.round(ratio * block * block);
          diffPixels += cnt;
          blocks.push({ x: ix * block, y: iy * block, w: block, h: block, diffRatio: ratio });
        }
      }
    }
    const total = w * h;
    return {
      sessionId: p.sessionId,
      width: w,
      height: h,
      leftWidth: w,
      leftHeight: h,
      rightWidth: w,
      rightHeight: h,
      diffPixels,
      totalPixels: total,
      similarityPct: Math.round((1 - diffPixels / total) * 10000) / 100,
      blocks,
      truncated: false,
    };
    void tolerance;
  }

  /** P14：表格比较演示（确定性 CSV 演示数据；与引擎 table/open|sections 同形） */
  private tableOpen(p: TableOpenParams): {
    sessionId: string;
    rowCount: number;
    leftRowCount: number;
    rightRowCount: number;
    columns: Array<{ name: string; ty: string; left: number | null; right: number | null }>;
    summary: { sameRows: number; changedRows: number; addedRows: number; removedRows: number; unimportantRows: number };
  } {
    if (!p || typeof p.sessionId !== 'string' || p.sessionId === '') {
      throw new InvalidParams('sessionId 不能为空');
    }
    if (this.dirSessions.has(p.sessionId) || this.textSessions.has(p.sessionId)) {
      throw new RpcErr(CODE.SESSION_EXISTS, `会话已存在: ${p.sessionId}`);
    }
    const seed = (p.left ?? '').length + (p.right ?? '').length;
    const columns = [
      { name: 'ID', ty: 'number', left: 0, right: 0 },
      { name: '名称', ty: 'text', left: 1, right: 1 },
      { name: '数量', ty: 'number', left: 2, right: 2 },
    ];
    this.tableSessions.set(p.sessionId, { seed, columns });
    void columns;
    const rowCount = 40 + (seed % 5) * 8;
    this.tableSessions.set(p.sessionId, { seed, columns, rowCount });
    const leftRowCount = rowCount;
    const rightRowCount = rowCount + (seed % 3);
    return {
      sessionId: p.sessionId,
      rowCount,
      leftRowCount,
      rightRowCount,
      columns: this.tableOpenColumns(p.sessionId),
      summary: {
        sameRows: Math.floor(rowCount * 0.6),
        changedRows: Math.floor(rowCount * 0.15),
        addedRows: Math.floor(rowCount * 0.1),
        removedRows: Math.floor(rowCount * 0.1),
        unimportantRows: Math.floor(rowCount * 0.05),
      },
    };
  }

  private tableOpenColumns(sessionId: string): Array<{ name: string; ty: string; left: number | null; right: number | null }> {
    return this.tableSessions.get(sessionId)?.columns ?? [
      { name: 'ID', ty: 'number', left: 0, right: 0 },
      { name: '名称', ty: 'text', left: 1, right: 1 },
      { name: '数量', ty: 'number', left: 2, right: 2 },
    ];
  }

  private tableSections(p: TableSectionsParams): {
    sessionId: string;
    offset: number;
    rowCount: number;
    hasMore: boolean;
    rows: Array<{
      kind: string;
      left: number | null;
      right: number | null;
      important: boolean;
      cells: Array<{ left: string | null; right: string | null; status: string }>;
    }>;
  } {
    const meta = this.tableSessions.get(p.sessionId);
    if (!meta) {
      throw new RpcErr(CODE.SESSION_NOT_FOUND, `会话不存在: ${p.sessionId}`);
    }
    const seed = meta.seed;
    const rowCount = meta.rowCount ?? 40;
    let rnd = (seed * 40503) % 2147483648 || 1;
    const rows: Array<{
      kind: string;
      left: number | null;
      right: number | null;
      important: boolean;
      cells: Array<{ left: string | null; right: string | null; status: string }>;
    }> = [];
    const start = Math.max(0, Math.min(p.offset, rowCount));
    const end = Math.min(start + Math.max(0, p.count), rowCount);
    for (let i = start; i < end; i++) {
      rnd = (rnd * 1103515245 + 12345) % 2147483648;
      const bucket = rnd % 10;
      const kind = bucket < 6 ? 'same' : bucket < 8 ? 'changed' : bucket < 9 ? 'added' : 'removed';
      const id = String(i + 1);
      const name = `项目-${i + 1}`;
      const qtyL = String(10 + ((rnd >> 3) % 90));
      const qtyR = String(10 + ((rnd >> 5) % 90));
      const important = bucket >= 6;
      rows.push({
        kind,
        left: kind === 'added' ? null : i,
        right: kind === 'removed' ? null : i,
        important,
        cells: [
          { left: id, right: id, status: 'same' },
          { left: name, right: name, status: 'same' },
          {
            left: kind === 'added' ? null : qtyL,
            right: kind === 'removed' ? null : qtyR,
            status:
              kind === 'same'
                ? 'same'
                : kind === 'added'
                  ? 'rightOnly'
                  : kind === 'removed'
                    ? 'leftOnly'
                    : qtyL === qtyR
                      ? 'unimportantDiff'
                      : 'importantDiff',
          },
        ],
      });
    }
    return { sessionId: p.sessionId, offset: start, rowCount, hasMore: end < rowCount, rows };
  }

  /** P14：十六进制比较演示（确定性字节；与引擎 hex/open|bytes 同形） */
  private hexOpen(p: HexOpenParams): {
    sessionId: string;
    leftSize: number;
    rightSize: number;
    blockCount: number;
    summary: { sameBytes: number; changedBytes: number; insertedBytes: number; deletedBytes: number };
  } {
    if (!p || typeof p.sessionId !== 'string' || p.sessionId === '') {
      throw new InvalidParams('sessionId 不能为空');
    }
    if (this.dirSessions.has(p.sessionId) || this.textSessions.has(p.sessionId)) {
      throw new RpcErr(CODE.SESSION_EXISTS, `会话已存在: ${p.sessionId}`);
    }
    const seed = (p.left ?? '').length + (p.right ?? '').length;
    const leftSize = 256 + (seed % 4) * 64;
    const rightSize = leftSize + 16;
    this.hexSessions.set(p.sessionId, { seed, leftSize, rightSize });
    return {
      sessionId: p.sessionId,
      leftSize,
      rightSize,
      blockCount: 2,
      summary: { sameBytes: leftSize - 8, changedBytes: 4, insertedBytes: 0, deletedBytes: 0 },
    };
  }

  private hexBytes(p: HexBytesParams): {
    lines: Array<{ addr: number; hex: string; ascii: string; status: string }>;
    done: boolean;
  } {
    const meta = this.hexSessions.get(p.sessionId);
    if (!meta) {
      throw new RpcErr(CODE.SESSION_NOT_FOUND, `会话不存在: ${p.sessionId}`);
    }
    const total = p.side === 'left' ? meta.leftSize : meta.rightSize;
    const lines: Array<{ addr: number; hex: string; ascii: string; status: string }> = [];
    // 16 字节/行；确定性：每第 9 字节 +1（演示 changed），右尾 16 字节 inserted
    const start = Math.max(0, Math.min(Number(p.offset), total));
    const len = Math.max(0, Math.min(p.length, total - start));
    while (len > 0 && lines.length * 16 < len) {
      const addr = start + lines.length * 16;
      const rowLen = Math.min(16, total - addr);
      if (rowLen <= 0) break;
      const hexParts: string[] = [];
      let ascii = '';
      let status = '';
      for (let k = 0; k < rowLen; k++) {
        const b = (addr + k) % 256;
        const changed = b % 16 === 8 && addr + k >= meta.leftSize - 8 ? false : b % 16 === 8;
        hexParts.push(b.toString(16).padStart(2, '0'));
        ascii += b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : '.';
        status += changed ? 'x' : '=';
      }
      lines.push({ addr, hex: hexParts.join(' '), ascii, status });
    }
    return { lines, done: start + lines.length * 16 >= total };
  }

  // ---------------- P7：三向文本合并（text/merge3/*） ----------------

  /** text/merge3/open：确定性三路演示模型 + 会话（同形于 handlers.rs merge3_open） */
  private merge3Open(p: MergeOpenParams): MockMerge3OpenResult {
    if (!p || typeof p.sessionId !== 'string' || p.sessionId === '') {
      throw new InvalidParams('sessionId 不能为空');
    }
    if (this.hasAnySession(p.sessionId)) {
      throw new RpcErr(CODE.SESSION_EXISTS, `会话已存在: ${p.sessionId}`);
    }
    const chunks = [...MERGE3_TEMPLATE];
    this.merge3Sessions.set(p.sessionId, {
      id: p.sessionId,
      chunks,
      decisions: new Map(),
      rev: 0,
      labels: { base: p.base ?? '', left: p.left ?? '', right: p.right ?? '' },
    });
    return {
      sessionId: p.sessionId,
      baseLineCount: Math.max(...chunks.map((c) => c.base.end)),
      leftLineCount: Math.max(...chunks.map((c) => c.left.end)),
      rightLineCount: Math.max(...chunks.map((c) => c.right.end)),
      chunkCount: chunks.length,
      conflictCount: chunks.filter((c) => c.origin === 'conflict').length,
      rev: 0,
    };
  }

  /** text/merge3/sections：块清单分批窗口（决策状态实时派生） */
  private merge3Sections(p: MergeSectionsParams): MockMerge3SectionsResult {
    const s = this.requireMerge3(p.sessionId);
    const start = Math.max(0, Math.min(Number(p.offset), s.chunks.length));
    const end = Math.min(start + Math.max(0, Number(p.count)), s.chunks.length);
    return {
      sessionId: p.sessionId,
      offset: start,
      total: s.chunks.length,
      hasMore: end < s.chunks.length,
      rev: s.rev,
      chunks: s.chunks.slice(start, end).map((t) => deriveMergeChunk(t, s.decisions)),
    };
  }

  /** text/merge3/decide：整批校验 + 幂等提交（决策内容不变时 rev 不递增） */
  private merge3Decide(p: MergeDecideParams): MockMerge3DecideResult {
    const s = this.requireMerge3(p.sessionId);
    const list = Array.isArray(p.decisions) ? p.decisions : [];
    for (const d of list) {
      if (!Number.isInteger(d.chunkId) || d.chunkId < 0 || d.chunkId >= s.chunks.length) {
        throw new InvalidParams(`块不存在: ${String(d.chunkId)}`);
      }
      if (!MERGE3_ACTIONS.includes(d.action)) {
        throw new InvalidParams(`未知决策动作: ${String(d.action)}`);
      }
      if (d.action === 'manual' && (d.text === null || d.text === undefined)) {
        throw new InvalidParams('manual 决策必须提供 text');
      }
    }
    let changed = false;
    for (const d of list) {
      const next: MergeDecisionDto = { chunkId: d.chunkId, action: d.action, text: d.text ?? null };
      const prev = s.decisions.get(d.chunkId);
      if (!prev || prev.action !== next.action || (prev.text ?? null) !== next.text) {
        s.decisions.set(d.chunkId, next);
        changed = true;
      }
    }
    if (changed) s.rev += 1;
    return {
      sessionId: p.sessionId,
      rev: s.rev,
      decided: list.length,
      conflictCount: s.chunks.filter((c) => c.origin === 'conflict').length,
      unresolvedChunks: merge3UnresolvedIds(s),
    };
  }

  /** text/merge3/output：plain 合成 / marker（Merge 与 Diff3 两种风格） */
  private merge3Output(p: MergeOutputParams): MergeOutputResult {
    const s = this.requireMerge3(p.sessionId);
    const marker = p.mode === 'marker';
    const style: MergeMarkerStyle = p.markerStyle ?? 'merge';
    const lines: string[] = [];
    for (const t of s.chunks) {
      const chunk = deriveMergeChunk(t, s.decisions);
      if (marker && chunk.unresolved) {
        lines.push(`<<<<<<< ${s.labels.left}`);
        lines.push(...chunk.leftLines);
        if (style === 'diff3') {
          lines.push(`||||||| ${s.labels.base}`);
          lines.push(...chunk.baseLines);
        }
        lines.push('=======');
        lines.push(...chunk.rightLines);
        lines.push(`>>>>>>> ${s.labels.right}`);
      } else {
        lines.push(...chunk.outputLines);
      }
    }
    const unresolved = merge3UnresolvedIds(s);
    return {
      sessionId: p.sessionId,
      text: lines.length > 0 ? `${lines.join('\n')}\n` : '',
      conflictCount: s.chunks.filter((c) => c.origin === 'conflict').length,
      unresolvedChunks: unresolved,
      hasConflicts: unresolved.length > 0,
      decidedCount: s.decisions.size,
    };
  }

  private requireMerge3(id: string): MockMerge3Session {
    const s = this.merge3Sessions.get(id);
    if (!s) {
      if (this.hasAnySession(id)) throw new InvalidParams(`会话不是三向合并会话: ${id}`);
      throw new RpcErr(CODE.SESSION_NOT_FOUND, `会话不存在: ${id}`);
    }
    return s;
  }

  /** 会话句柄全局唯一（客户端分配；所有会话表共用命名空间） */
  private hasAnySession(id: string): boolean {
    return (
      this.dirSessions.has(id) ||
      this.textSessions.has(id) ||
      this.mergeSessions.has(id) ||
      this.tableSessions.has(id) ||
      this.hexSessions.has(id) ||
      this.merge3Sessions.has(id)
    );
  }

  private sessionClose(p: SessionCloseParams): null {
    this.dirSessions.delete(p.sessionId);
    this.textSessions.delete(p.sessionId);
    this.mergeSessions.delete(p.sessionId);
    this.merge3Sessions.delete(p.sessionId);
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


// ==================================================================
// P8：mock 三路合并模型（确定性；分类语义与引擎 merge3way 同口径）
// ==================================================================

/**
 * 三方演示树：以 rel 路径为键对齐三侧；形态涵盖全部分类
 * （三方同/单侧改/两侧同改/冲突/删除跟随/孤立/类型冲突/空目录）。
 */
function buildMergeModel(base: string, left: string, right: string): DirMergeRow[] {
  const rand = mulberry32(hashString(`${base}|${left}|${right}`));
  const now = Date.now();
  const day = 24 * 3600 * 1000;
  type Spec = { name: string; kind: 'file' | 'dir'; content?: string };
  // (rel, base形态, left形态, right形态)：null=缺失；'same'=与 base 相同内容
  type Entry = [string, Spec | null, Spec | null, Spec | null];
  const catalog: Entry[] = [
    ['README.md', { name: 'README.md', kind: 'file', content: '# demo\nshared\n' }, { name: 'README.md', kind: 'file', content: '# demo\nshared\n' }, { name: 'README.md', kind: 'file', content: '# demo\nshared\n' }],
    ['version.json', { name: 'version.json', kind: 'file', content: '{"v":1}' }, { name: 'version.json', kind: 'file', content: '{"v":2}' }, { name: 'version.json', kind: 'file', content: '{"v":1}' }],
    ['config.yml', { name: 'config.yml', kind: 'file', content: 'mode: basic\n' }, { name: 'config.yml', kind: 'file', content: 'mode: basic\n' }, { name: 'config.yml', kind: 'file', content: 'mode: pro\n' }],
    ['conflict.c', { name: 'conflict.c', kind: 'file', content: 'int a = 0;\n' }, { name: 'conflict.c', kind: 'file', content: 'int a = 1;\n' }, { name: 'conflict.c', kind: 'file', content: 'int a = 2;\n' }],
    ['both.c', { name: 'both.c', kind: 'file', content: 'int b = 0;\n' }, { name: 'both.c', kind: 'file', content: 'int b = 9;\n' }, { name: 'both.c', kind: 'file', content: 'int b = 9;\n' }],
    ['deleted-r.txt', { name: 'deleted-r.txt', kind: 'file', content: 'kept\n' }, { name: 'deleted-r.txt', kind: 'file', content: 'kept\n' }, null],
    ['deleted-mod.txt', { name: 'deleted-mod.txt', kind: 'file', content: 'v0\n' }, { name: 'deleted-mod.txt', kind: 'file', content: 'v1\n' }, null],
    ['lonly.txt', null, { name: 'lonly.txt', kind: 'file', content: 'left only\n' }, null],
    ['ronly.txt', null, null, { name: 'ronly.txt', kind: 'file', content: 'right only\n' }],
    ['both-new.txt', null, { name: 'both-new.txt', kind: 'file', content: 'identical\n' }, { name: 'both-new.txt', kind: 'file', content: 'identical\n' }],
    ['gone.txt', { name: 'gone.txt', kind: 'file', content: 'deleted both sides\n' }, null, null],
    ['clash', { name: 'clash', kind: 'file', content: 'file\n' }, { name: 'clash', kind: 'dir' }, { name: 'clash', kind: 'file', content: 'file\n' }],
    ['src', { name: 'src', kind: 'dir' }, { name: 'src', kind: 'dir' }, { name: 'src', kind: 'dir' }],
    ['src/main.c', { name: 'main.c', kind: 'file', content: 'void main() {}\n' }, { name: 'src/main.c', kind: 'file', content: 'void main() { /* fixed */ }\n' }, { name: 'src/main.c', kind: 'file', content: 'void main() {}\n' }],
    ['src/util.c', { name: 'src/util.c', kind: 'file', content: 'util v0\n' }, { name: 'src/util.c', kind: 'file', content: 'util v0\n' }, { name: 'src/util.c', kind: 'file', content: 'util v1\n' }],
    ['src/orphan-dir', null, { name: 'orphan-dir', kind: 'dir' }, null],
    ['src/orphan-dir/inner.txt', null, { name: 'inner.txt', kind: 'file', content: 'nested orphan\n' }, null],
    ['src/empty-orphan', null, { name: 'empty-orphan', kind: 'dir' }, null],
  ];
  const mkInfo = (size: number): SideInfo => ({
    size,
    // mtime 保留随机相位（不参与分类判定；演示列表展示用）
    mtimeMs: now - Math.floor(rand() * 30 * day),
    attribute: null,
    unixMode: 0,
    uid: 0,
    gid: 0,
  });
  // size 确定性 = 内容长度（三方同内容 → 同 size；分类判定不引入随机）
  const infoOf = (spec: Spec | null): { info: SideInfo | null; size: number } => {
    if (!spec) return { info: null, size: 0 };
    const size = spec.kind === 'dir' ? 0 : (spec.content?.length ?? 0);
    return { info: mkInfo(size), size };
  };
  const rows: DirMergeRow[] = [];
  for (const [rel, b, l, r] of catalog) {
    const name = rel.split('/').pop() ?? rel;
    // 展示形态：任一侧为目录即按目录行（BC 语义）；file↔dir 同名 = 冲突叶
    const kind: EntryKind = [b, l, r].some((x) => x?.kind === 'dir') ? 'dir' : 'file';
    const bi = infoOf(b);
    const li = infoOf(l);
    const ri = infoOf(r);
    const { category, rowState, conflictLeaf } = classifyMockEntry(kind, b, l, r, bi.size, li.size, ri.size);
    const parent = rel.includes('/') ? rel.slice(0, rel.lastIndexOf('/')) : null;
    rows.push({
      relPath: rel,
      name,
      kind,
      base: bi.info,
      left: li.info,
      right: ri.info,
      category,
      rowState,
      parent,
      depth: rel.split('/').length,
      conflictLeaf,
    });
  }
  // 目录行聚合（自底向上：Conflict > Orphan > Auto > Same）
  aggregateMockRows(rows);
  return rows;
}

/** mock 三方分类（叶级；与 merge3way::classify_file 同口径，演示以内容/size 代替 CRC） */
function classifyMockEntry(
  kind: EntryKind,
  b: { kind?: 'file' | 'dir'; content?: string } | null,
  l: { kind?: 'file' | 'dir'; content?: string } | null,
  r: { kind?: 'file' | 'dir'; content?: string } | null,
  bSize: number,
  lSize: number,
  rSize: number,
): { category: DirMergeCategory; rowState: DirMergeRowState; conflictLeaf: boolean } {
  // 类型冲突：同名 file↔dir（与引擎 walk 的 clash 同口径）
  const anyDir = [b, l, r].some((x) => x?.kind === 'dir');
  const anyFile = [b, l, r].some((x) => x != null && x.kind !== 'dir');
  if (anyDir && anyFile) {
    return { category: 'conflict', rowState: 'conflict', conflictLeaf: false };
  }
  if (kind === 'dir') {
    // 目录初始按存在性；聚合后重算（空目录孤立也覆盖）
    const cat: DirMergeCategory = !b && l && !r ? 'leftOnly' : !b && !l && r ? 'rightOnly' : b && !l && !r ? 'baseOnly' : 'same';
    return { category: cat, rowState: cat === 'same' ? 'same' : 'orphan', conflictLeaf: false };
  }
  const eq = (a: { content?: string } | null, b2: { content?: string } | null, as: number, bs: number): boolean => {
    if (!a && !b2) return true;
    if (!a || !b2) return false;
    if (as !== bs) return false;
    return (a.content ?? '') === (b2.content ?? '');
  };
  const hasB = b != null;
  const hasL = l != null;
  const hasR = r != null;
  let category: DirMergeCategory;
  if (hasB && hasL && hasR) {
    const bl = eq(b, l, bSize, lSize);
    if (bl) {
      const br = eq(b, r, bSize, rSize);
      category = br ? 'same' : 'rightChanged';
    } else {
      const br = eq(b, r, bSize, rSize);
      if (br) category = 'leftChanged';
      else {
        const lr = eq(l, r, lSize, rSize);
        category = lr ? 'bothSame' : 'conflict';
      }
    }
  } else if (hasB && hasL) {
    category = eq(b, l, bSize, lSize) ? 'deletedRight' : 'conflict';
  } else if (hasB && hasR) {
    category = eq(b, r, bSize, rSize) ? 'deletedLeft' : 'conflict';
  } else if (hasL && hasR) {
    category = eq(l, r, lSize, rSize) ? 'bothSame' : 'conflict';
  } else if (hasL) {
    category = 'leftOnly';
  } else if (hasR) {
    category = 'rightOnly';
  } else {
    category = 'baseOnly';
  }
  const rowState: DirMergeRowState =
    category === 'conflict'
      ? 'conflict'
      : category === 'leftOnly' || category === 'rightOnly' || category === 'baseOnly'
        ? 'orphan'
        : category === 'same'
          ? 'same'
          : 'auto';
  return { category, rowState, conflictLeaf: category === 'conflict' };
}

/** mock 目录聚合（Conflict > Orphan > Auto > Same；叠加自身存在性） */
function aggregateMockRows(rows: DirMergeRow[]): void {
  const depthOrder = [...rows].sort((a, b) => b.depth - a.depth);
  const rank = (st: DirMergeRowState): number => (st === 'conflict' ? 0 : st === 'orphan' ? 1 : st === 'auto' ? 2 : 3);
  const worse = (a: DirMergeRowState, b2: DirMergeRowState): DirMergeRowState => (rank(a) <= rank(b2) ? a : b2);
  for (const row of depthOrder) {
    if (row.kind !== 'dir') continue;
    const children = rows.filter((r) => r.parent === row.relPath);
    let state: DirMergeRowState = row.rowState;
    for (const c of children) state = worse(state, c.rowState);
    row.rowState = state;
    const catRank = (c: DirMergeCategory): number =>
      c === 'conflict' ? 0 : c === 'leftOnly' || c === 'rightOnly' || c === 'baseOnly' ? 1 : c === 'same' ? 3 : 2;
    let cat = row.category;
    for (const c of children) {
      if (catRank(c.category) < catRank(cat)) cat = c.category;
    }
    row.category = cat;
    row.conflictLeaf = false;
  }
}

function mergeScanStats(rows: DirMergeRow[]): DirMergeScanStats {
  const stats: DirMergeScanStats = { total: rows.length, files: 0, dirs: 0, same: 0, auto: 0, conflict: 0, orphan: 0 };
  for (const r of rows) {
    if (r.kind === 'dir') stats.dirs++;
    else stats.files++;
    switch (r.rowState) {
      case 'same':
        stats.same++;
        break;
      case 'auto':
        stats.auto++;
        break;
      case 'conflict':
        stats.conflict++;
        break;
      case 'orphan':
        stats.orphan++;
        break;
    }
  }
  return stats;
}

/** mock 执行后的行状态重算（演示口径：目标侧对齐源后 = same；单侧 = 孤立） */
function recomputeMergeRow(row: DirMergeRow): void {
  if (row.kind === 'dir') {
    row.category = row.left && row.right ? 'same' : row.left ? 'leftOnly' : row.right ? 'rightOnly' : 'baseOnly';
    row.rowState = row.category === 'same' ? 'same' : 'orphan';
    return;
  }
  if (row.left && row.right) {
    const sameContent = row.left.size === row.right.size && row.left.mtimeMs === row.right.mtimeMs;
    row.category = sameContent ? 'same' : 'conflict';
  } else if (row.left) {
    row.category = 'leftOnly';
  } else if (row.right) {
    row.category = 'rightOnly';
  } else {
    row.category = 'baseOnly';
  }
  row.rowState =
    row.category === 'conflict'
      ? 'conflict'
      : row.category === 'same'
        ? 'same'
        : row.category === 'leftOnly' || row.category === 'rightOnly' || row.category === 'baseOnly'
          ? 'orphan'
          : 'auto';
  row.conflictLeaf = row.kind === 'file' && row.category === 'conflict';
}

/**
 * mock 合并计划（与引擎 merge3way::plan_merge 同口径）：
 * 有效决策 = 显式决策 > 祖先继承 > Auto；目录决策递归展开；Auto 走默认决策表。
 */
function mockMergePlan(rows: DirMergeRow[], target: MergeTarget, decisions: Record<string, MergeDecision>): MergePlannedOp[] {
  const childrenOf = new Map<string | null, DirMergeRow[]>();
  for (const r of rows) {
    const list = childrenOf.get(r.parent) ?? [];
    list.push(r);
    childrenOf.set(r.parent, list);
  }
  const ops: MergePlannedOp[] = [];

  const effective = (row: DirMergeRow, inherit: MergeDecision | null): MergeDecision => {
    const own = decisions[row.relPath];
    return own ?? inherit ?? 'auto';
  };

  const autoDefault = (row: DirMergeRow, t: MergeTarget): { kind: MergeOpKind; reason: string } => {
    const cat = row.category;
    switch (cat) {
      case 'same':
        return t === 'output' ? { kind: 'takeLeft', reason: '三方一致（输出取左）' } : { kind: 'keep', reason: '三方一致' };
      case 'leftChanged':
        return t === 'left' ? { kind: 'keep', reason: '目标侧即左侧（已含修改）' } : { kind: 'takeLeft', reason: '仅左改 → 取左' };
      case 'rightChanged':
        return t === 'right' ? { kind: 'keep', reason: '目标侧即右侧（已含修改）' } : { kind: 'takeRight', reason: '仅右改 → 取右' };
      case 'bothSame':
        return t === 'output' ? { kind: 'takeLeft', reason: '两侧同改一致（输出取左，等价取右）' } : { kind: 'keep', reason: '两侧同改一致' };
      case 'deletedRight':
        return t === 'right' ? { kind: 'keep', reason: '目标侧已删除' } : t === 'left' ? { kind: 'deleteTarget', reason: '跟随右侧删除' } : { kind: 'keep', reason: '右侧已删除（输出不含该项）' };
      case 'deletedLeft':
        return t === 'left' ? { kind: 'keep', reason: '目标侧已删除' } : t === 'right' ? { kind: 'deleteTarget', reason: '跟随左侧删除' } : { kind: 'keep', reason: '左侧已删除（输出不含该项）' };
      case 'conflict':
        return { kind: 'keep', reason: '冲突需人工处理' };
      case 'leftOnly':
        return t === 'left' ? { kind: 'keep', reason: '目标侧即左侧' } : { kind: 'takeLeft', reason: t === 'output' ? '左孤立并入输出' : '左孤立并入目标' };
      case 'rightOnly':
        return t === 'right' ? { kind: 'keep', reason: '目标侧即右侧' } : { kind: 'takeRight', reason: t === 'output' ? '右孤立并入输出' : '右孤立并入目标' };
      case 'baseOnly':
        return { kind: 'keep', reason: '两侧均已删除' };
    }
  };

  const walk = (row: DirMergeRow, inherit: MergeDecision | null): void => {
    const eff = effective(row, inherit);
    if (row.kind === 'dir' && !row.conflictLeaf && row.category !== 'conflict') {
      // 目录节点（类型冲突叶按文件处理）
      if (eff === 'skip') {
        ops.push({ kind: 'keep', path: row.relPath, reason: '决策:跳过（含子树）' });
        return;
      }
      // 空孤立目录并入
      const kids = childrenOf.get(row.relPath) ?? [];
      const absentOnTarget =
        (target === 'right' && row.category === 'leftOnly') ||
        (target === 'left' && row.category === 'rightOnly') ||
        (target === 'output' && (row.category === 'leftOnly' || row.category === 'rightOnly'));
      if (kids.length === 0 && absentOnTarget) {
        const explicit = eff === 'takeLeft' || eff === 'takeRight' || eff === 'both';
        const kind: MergeOpKind = eff === 'takeRight' || (!explicit && row.category === 'rightOnly') ? 'takeRight' : 'takeLeft';
        ops.push({ kind, path: row.relPath, reason: explicit ? '决策:目录并入（空目录）' : '孤立空目录并入' });
      }
      const passDown: MergeDecision | null = eff === 'auto' ? null : eff;
      for (const c of kids) walk(c, passDown);
      return;
    }
    // 文件叶 / 类型冲突叶
    let kind: MergeOpKind;
    let reason: string;
    switch (eff) {
      case 'skip':
        kind = 'keep';
        reason = '决策:跳过';
        break;
      case 'takeLeft':
        if (!row.left) {
          kind = 'keep';
          reason = '决策:取左（左侧缺失，保持现状）';
        } else if (target === 'left') {
          kind = 'keep';
          reason = '决策:取左（目标侧即左侧）';
        } else {
          kind = 'takeLeft';
          reason = '决策:取左';
        }
        break;
      case 'takeRight':
        if (!row.right) {
          kind = 'keep';
          reason = '决策:取右（右侧缺失，保持现状）';
        } else if (target === 'right') {
          kind = 'keep';
          reason = '决策:取右（目标侧即右侧）';
        } else {
          kind = 'takeRight';
          reason = '决策:取右';
        }
        break;
      case 'both': {
        const d = autoDefault(row, target);
        kind = d.kind;
        reason = `决策:both（单文件降级默认）· ${d.reason}`;
        break;
      }
      default: {
        const d = autoDefault(row, target);
        kind = d.kind;
        reason = d.reason;
      }
    }
    if (kind === 'keep') {
      // 显式决策的 Keep 记录入计划（预览可解释）；Auto 的 Keep 不入（对齐引擎口径）
      if (eff !== 'auto') ops.push({ kind, path: row.relPath, reason });
      return;
    }
    ops.push({ kind, path: row.relPath, reason });
  };

  for (const root of childrenOf.get(null) ?? []) walk(root, null);
  return ops;
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

// ==================================================================
// P7：mock 三向文本合并演示模型（确定性；与 ai-core::merge3 的块序列、
//      handlers.rs text/merge3/* 的决策/合成语义同形）
// ==================================================================
//
// 三方行区间各自完备划分全表（插入块 base 为空区间占位）；决策不落模板，
// 只记 decisions 表，sections/output 每次实时派生 → 幂等天然成立。
// 块来源覆盖 FR-TMRG-002 全部状态：stable / leftOnly / rightOnly /
// bothChanged（相同更改自动合并）/ conflict（含 addAdd、不重要冲突自动采用）。

/** u64 rev 在 NDJSON 上是 number（bigint 无法 JSON.stringify）；仅应答类型收窄 */
type MockMerge3OpenResult = Omit<MergeOpenResult, 'rev'> & { rev: number };
type MockMerge3SectionsResult = Omit<MergeSectionsResult, 'rev'> & { rev: number };
type MockMerge3DecideResult = Omit<MergeDecideResult, 'rev'> & { rev: number };

type Merge3Template = Omit<
  MergeChunkDto,
  'decided' | 'decision' | 'autoResolved' | 'unresolved' | 'outputLines'
> & {
  autoResolved: boolean;
  /** 无显式决策时的输出（FR-TMRG-004：非冲突自动采用；冲突保持中间内容） */
  defaultOutput: string[];
};

interface MockMerge3Session {
  id: string;
  chunks: Merge3Template[];
  decisions: Map<number, MergeDecisionDto>;
  rev: number;
  labels: { base: string; left: string; right: string };
}

const MERGE3_ACTIONS: readonly MergeDecisionKind[] = [
  'left',
  'right',
  'base',
  'leftThenRight',
  'rightThenLeft',
  'manual',
];

const rg = (start: number, end: number): LineRange => ({ start, end });

function merge3Tpl(
  id: number,
  origin: MergeChunkOrigin,
  base: LineRange,
  left: LineRange,
  right: LineRange,
  baseLines: string[],
  leftLines: string[],
  rightLines: string[],
  opts: {
    conflictKind?: MergeConflictKind;
    conflictRows?: MergeConflictRowDto[];
    unimportant?: boolean;
    autoResolved?: boolean;
    defaultOutput?: string[];
  } = {}
): Merge3Template {
  const defaultOutput =
    opts.defaultOutput ??
    (origin === 'leftOnly' || origin === 'bothChanged'
      ? leftLines
      : origin === 'rightOnly'
        ? rightLines
        : baseLines);
  return {
    id,
    origin,
    base,
    left,
    right,
    conflictKind: opts.conflictKind ?? null,
    conflictRows: opts.conflictRows ?? null,
    conflictUnimportant: opts.unimportant ?? null,
    baseLines,
    leftLines,
    rightLines,
    autoResolved: opts.autoResolved ?? false,
    defaultOutput: [...defaultOutput],
  };
}

const SAME_HEAD = ['// ai-compare 三向合并演示', 'export function boot() {'];
const SAME_BODY = ['  const label = "boot";'];
const SAME_TAIL = ['  return run(label, retries, timeout);'];
const SAME_END = ['}'];

/** 10 个块：base 10 行 / left 11 行 / right 11 行；冲突 3（其中 1 个不重要自动采用） */
const MERGE3_TEMPLATE: Merge3Template[] = [
  merge3Tpl(0, 'stable', rg(0, 2), rg(0, 2), rg(0, 2), SAME_HEAD, SAME_HEAD, SAME_HEAD),
  merge3Tpl(
    1, 'leftOnly', rg(2, 3), rg(2, 3), rg(2, 3),
    ['  const retries = 2;'], ['  const retries = 5;'], ['  const retries = 2;']
  ),
  merge3Tpl(2, 'stable', rg(3, 4), rg(3, 4), rg(3, 4), SAME_BODY, SAME_BODY, SAME_BODY),
  merge3Tpl(
    3, 'conflict', rg(4, 5), rg(4, 5), rg(4, 5),
    ['  const mode = "safe";'], ['  const mode = "fast";'], ['  const mode = "strict";'],
    {
      conflictKind: 'modifyModify',
      conflictRows: [{ left: 4, right: 4, unimportant: false }],
      unimportant: false,
    }
  ),
  merge3Tpl(
    4, 'rightOnly', rg(5, 6), rg(5, 6), rg(5, 6),
    ['  const timeout = 30;'], ['  const timeout = 30;'], ['  const timeout = 60;']
  ),
  merge3Tpl(
    5, 'bothChanged', rg(6, 7), rg(6, 7), rg(6, 7),
    ['  const tag = "v0";'], ['  const tag = "v1";'], ['  const tag = "v1";']
  ),
  merge3Tpl(6, 'stable', rg(7, 8), rg(7, 8), rg(7, 8), SAME_TAIL, SAME_TAIL, SAME_TAIL),
  merge3Tpl(
    7, 'conflict', rg(8, 8), rg(8, 9), rg(8, 9),
    [], ['  // 本地新增：灰度开关'], ['  // 远端新增：熔断开关'],
    {
      conflictKind: 'addAdd',
      conflictRows: [{ left: 8, right: 8, unimportant: false }],
      unimportant: false,
    }
  ),
  merge3Tpl(
    8, 'conflict', rg(8, 9), rg(9, 10), rg(9, 10),
    ['  log("done");'], ['  log("done" );'], ['  log("done");'],
    {
      conflictKind: 'modifyModify',
      conflictRows: [{ left: 9, right: 9, unimportant: true }],
      unimportant: true,
      autoResolved: true,
      defaultOutput: ['  log("done" );'],
    }
  ),
  merge3Tpl(9, 'stable', rg(9, 10), rg(10, 11), rg(10, 11), SAME_END, SAME_END, SAME_END),
];

/** 未决冲突块 id（升序）：冲突且无显式决策、也未自动采用 */
function merge3UnresolvedIds(s: MockMerge3Session): number[] {
  return s.chunks
    .filter((t) => t.origin === 'conflict' && !s.decisions.has(t.id) && !t.autoResolved)
    .map((t) => t.id)
    .sort((a, b) => a - b);
}

/** 与引擎 LineTable::from_text 同口径：结尾换行不产生空行，去掉行尾 \r */
function merge3SplitLines(text: string): string[] {
  if (text === '') return [];
  const parts = text.split('\n');
  if (parts[parts.length - 1] === '') parts.pop();
  return parts.map((l) => (l.endsWith('\r') ? l.slice(0, -1) : l));
}

/** 采用命令 → 输出行（FR-TMRG-004 六动作族） */
function merge3OutputForDecision(t: Merge3Template, d: MergeDecisionDto): string[] {
  switch (d.action) {
    case 'left':
      return [...t.leftLines];
    case 'right':
      return [...t.rightLines];
    case 'base':
      return [...t.baseLines];
    case 'leftThenRight':
      return [...t.leftLines, ...t.rightLines];
    case 'rightThenLeft':
      return [...t.rightLines, ...t.leftLines];
    case 'manual':
      return merge3SplitLines(d.text ?? '');
  }
}

/** 模板 + 决策表 → 引擎 MergeChunkDto（sections/output 共用派生口径） */
function deriveMergeChunk(t: Merge3Template, decisions: Map<number, MergeDecisionDto>): MergeChunkDto {
  const d = decisions.get(t.id);
  return {
    id: t.id,
    origin: t.origin,
    base: t.base,
    left: t.left,
    right: t.right,
    conflictKind: t.conflictKind,
    conflictRows: t.conflictRows,
    conflictUnimportant: t.conflictUnimportant,
    baseLines: t.baseLines,
    leftLines: t.leftLines,
    rightLines: t.rightLines,
    decided: d !== undefined,
    decision: d?.action ?? null,
    autoResolved: t.autoResolved,
    unresolved: t.origin === 'conflict' && d === undefined && !t.autoResolved,
    outputLines: d ? merge3OutputForDecision(t, d) : [...t.defaultOutput],
  };
}
