/**
 * ai-compare 协议类型：统一出口。
 *
 * **对齐基准已切换为引擎线生成类型**（`generated/`，ts-rs 产物，
 * `cargo test -p ai-protocol` 刷新；docs/03 §2.3 / §4.1 / §4.2）。
 * 本文件只做两件事：
 * 1. re-export `generated/` 全部类型（引擎 ⇄ UI 的唯一权威口径）；
 * 2. 定义少量 UI 侧本地辅助类型（引擎 P0 尚未开放 RPC 入口、但 UI 需要
 *    形态先行的部分，均逐个标注来源与开放计划）。
 *
 * 协议要点（docs/03 §2.3.2）：
 * - 所有请求携带 sessionId（**客户端分配**的会话句柄，引擎按其索引模型）
 * - 会话模型带单调递增 rev，UI 按 delta 追平，失序时 state/resync
 * - 取消：$/cancel + 任务 CancellationToken
 * - 大内容：text 行数据按需分页 text/lines（引擎 P1 开放；见下方本地类型）
 */

/* ============================================================
 * 1. 引擎生成类型（权威口径，勿手改；刷新方式 cargo test -p ai-protocol）
 * ============================================================ */

export type { BatchInfo } from './generated/BatchInfo';
export type { PictureBlockDto } from './generated/PictureBlockDto';
export type { PictureOpenParams } from './generated/PictureOpenParams';
export type { PictureOptionsSpec } from './generated/PictureOptionsSpec';
export type { PictureOpenResult } from './generated/PictureOpenResult';
export type { TableOpenParams } from './generated/TableOpenParams';
export type { HexOpenParams } from './generated/HexOpenParams';
export type { HexOptionsSpec } from './generated/HexOptionsSpec';
export type { HexAlignMode } from './generated/HexAlignMode';
export type { HexOpenResult } from './generated/HexOpenResult';
export type { HexSummary } from './generated/HexSummary';
export type { HexBytesParams } from './generated/HexBytesParams';
export type { HexBytesResult } from './generated/HexBytesResult';
export type { TableOptionsSpec } from './generated/TableOptionsSpec';
export type { TableOpenResult } from './generated/TableOpenResult';
export type { TableSectionsParams } from './generated/TableSectionsParams';
export type { TableSectionsResult } from './generated/TableSectionsResult';
export type { TableRowDto } from './generated/TableRowDto';
export type { TableRowKind } from './generated/TableRowKind';
export type { TableCellDto } from './generated/TableCellDto';
export type { TableCellStatus } from './generated/TableCellStatus';
export type { TableColumnInfo } from './generated/TableColumnInfo';
export type { TableColumnType } from './generated/TableColumnType';
export type { TableColumnAlign } from './generated/TableColumnAlign';
export type { TableSummary } from './generated/TableSummary';
export type { TableRowAlign } from './generated/TableRowAlign';

export type { CancelParams } from './generated/CancelParams';
export type { CharRange } from './generated/CharRange';
export type { ClientInfo } from './generated/ClientInfo';
export type { CompareCriteria } from './generated/CompareCriteria';
export type { ContentCompareMode } from './generated/ContentCompareMode';
export type { DiffSummary } from './generated/DiffSummary';
export type { DirDelta } from './generated/DirDelta';
export type { DirEntryStatus } from './generated/DirEntryStatus';
export type { DirExpandParams } from './generated/DirExpandParams';
export type { DirExpandResult } from './generated/DirExpandResult';
export type { DirFilterParams } from './generated/DirFilterParams';
export type { DirFilterResult } from './generated/DirFilterResult';
export type { DirOpenParams } from './generated/DirOpenParams';
export type { DirOpenResult } from './generated/DirOpenResult';
export type { DirOperateParams } from './generated/DirOperateParams';
export type { DirOperateResult } from './generated/DirOperateResult';
export type { DirRow } from './generated/DirRow';
export type { DirStats } from './generated/DirStats';
export type { DiffSide } from './generated/DiffSide';
export type { DisplayFilterSpec } from './generated/DisplayFilterSpec';
export type { EngineCapabilities } from './generated/EngineCapabilities';
export type { EngineInfo } from './generated/EngineInfo';
export type { EntryKind } from './generated/EntryKind';
export type { Importance } from './generated/Importance';
export type { InitializeParams } from './generated/InitializeParams';
export type { InitializeResult } from './generated/InitializeResult';
export type { InlineLineDiffDto } from './generated/InlineLineDiffDto';
export type { InlineSpanDto } from './generated/InlineSpanDto';
export type { InlineSpanKind } from './generated/InlineSpanKind';
export type { InlineState } from './generated/InlineState';
export type { LicenseCapabilities } from './generated/LicenseCapabilities';
export type { LineAlignAlgo } from './generated/LineAlignAlgo';
export type { LineRange } from './generated/LineRange';
export type { NameFilterSpec } from './generated/NameFilterSpec';
export type { OtherFilterSpec } from './generated/OtherFilterSpec';
export type { PlannedOpKind } from './generated/PlannedOpKind';
export type { PlannedOperation } from './generated/PlannedOperation';
export type { RequestId } from './generated/RequestId';
export type { RpcErrorObject } from './generated/RpcErrorObject';
export type { RuleAction } from './generated/RuleAction';
export type { RuleSide } from './generated/RuleSide';
export type { ScanProgress } from './generated/ScanProgress';
export type { ScanState } from './generated/ScanState';
export type { Section } from './generated/Section';
export type { SectionKind } from './generated/SectionKind';
export type { SessionCloseParams } from './generated/SessionCloseParams';
export type { SideInfo } from './generated/SideInfo';
export type { SyncPreset } from './generated/SyncPreset';
export type { SyncRule } from './generated/SyncRule';
export type { TextDiffOptions } from './generated/TextDiffOptions';
export type { TextInlineDiffParams } from './generated/TextInlineDiffParams';
export type { TextInlineDiffResult } from './generated/TextInlineDiffResult';
export type { TextLinesParams } from './generated/TextLinesParams';
export type { TextLinesResult } from './generated/TextLinesResult';
export type { TextOpenParams } from './generated/TextOpenParams';
export type { TextOpenResult } from './generated/TextOpenResult';
export type { TextSectionsNotification } from './generated/TextSectionsNotification';
export type { TextSectionsParams } from './generated/TextSectionsParams';
export type { TextSectionsResult } from './generated/TextSectionsResult';
export type { WhitespaceMode } from './generated/WhitespaceMode';
export type { CompareRulesSpec } from './generated/CompareRulesSpec';
export type { DetectedFileKind } from './generated/DetectedFileKind';
export type { EffectiveRulesSpec } from './generated/EffectiveRulesSpec';
export type { FileFormatSpec } from './generated/FileFormatSpec';
export type { FormatPackSpec } from './generated/FormatPackSpec';
export type { FormatViewKind } from './generated/FormatViewKind';
export type { FormatsListResult } from './generated/FormatsListResult';
export type { FormatsLoadParams } from './generated/FormatsLoadParams';
export type { FormatsLoadResult } from './generated/FormatsLoadResult';
export type { FormatsSaveParams } from './generated/FormatsSaveParams';
export type { FormatsSaveResult } from './generated/FormatsSaveResult';
export type { GrammarElementSpec } from './generated/GrammarElementSpec';
export type { GrammarItemKind } from './generated/GrammarItemKind';
export type { GrammarItemSpec } from './generated/GrammarItemSpec';
export type { RuleFormatBindingSpec } from './generated/RuleFormatBindingSpec';
export type { RuleScope } from './generated/RuleScope';
export type { RulesGetParams } from './generated/RulesGetParams';
export type { RulesGetResult } from './generated/RulesGetResult';
export type { RulesSetParams } from './generated/RulesSetParams';
export type { RulesSetResult } from './generated/RulesSetResult';
export type { RulesSpec } from './generated/RulesSpec';

/* ============================================================
 * 2. UI 侧本地辅助类型
 * ============================================================ */

import type { DiffSide } from './generated/DiffSide';
import type { CompareCriteria } from './generated/CompareCriteria';
import type { EntryKind } from './generated/EntryKind';
import type { SideInfo } from './generated/SideInfo';

/** 比较侧（docs/03 §4.1 Side）；生成类型已有 DiffSide，此为向后兼容别名 */
export type Side = DiffSide;

/** shutdown 请求不携带参数；结果为 null（docs/03 §2.3.3 app/ 命名空间） */
export type ShutdownResult = null;

/** session/close 应答：null（引擎 handlers.rs 对齐） */
export type SessionCloseResult = null;

/**
 * 目录会话默认判据（与引擎 CompareCriteria::default 对齐：2s 容差 + auto 内容复核；
 * P2 扩展字段取引擎默认值：忽略 DST 整小时差、比较大小写、不比较属性/Unix 元数据）
 */
export const DEFAULT_DIR_CRITERIA: CompareCriteria = {
  mtimeToleranceSec: 2,
  contentCompare: 'auto',
  ignoreDstHour: true,
  ignoreTzHour: false,
  compareCase: true,
  compareAttributes: false,
  attributeLetters: null,
  unixMeta: false,
};

/**
 * text/lines 与 text/inlineDiff 已切换为 P3 生成类型（上方 re-export，
 * 生成文件：TextLinesParams/Result、TextInlineDiffParams/Result、
 * InlineSpanDto/InlineSpanKind/InlineLineDiffDto）。
 *
 * 对齐要点（P3 磁盘现状，2026-11-14）：
 * - text/lines {sessionId, side, start, count} → {side, start, lines, done}；
 * - text/inlineDiff {sessionId, sectionId}（按 section 懒计算）→
 *   {sessionId, sectionId, kind, state, lines:[{leftLine, rightLine, spans, truncated}]}，
 *   leftLine/rightLine 为该侧绝对行号；spans 三类非 equal 参与高亮。
 * - Side 别名继续暴露（== DiffSide），历史 UI/测试代码无需改动。
 */
/** DMP 语义清理开关（UI 侧后处理；引擎 ai-core SemanticCleanup::None/Dmp 语义对齐） */
export type SemanticCleanup = 'none' | 'dmp';

/* ============================================================
 * 3. P6 提案：dir/execute 文件操作执行协议（UI 本地先行）
 * ============================================================
 *
 * ai-dir 引擎侧执行层已落地（engine/ai-dir/src/execute.rs：
 * execute_planned_operations / execute_ops，含 dry_run/护栏/部分成功语义），
 * 但 ai-protocol/ai-engine 归并行 P7 独占，RPC 入口待主会话协调合入。
 * 本地类型与提议的引擎 DTO 同形（见 P6 交付报告"协议/RPC 变更清单"）：
 *
 *   dir/execute { sessionId, ops, dryRun? } →
 *     { sessionId, total, succeeded[], failed[], skipped[], dryRun, cancelled }
 *
 * mock 引擎已同形实现（浏览器可演示）；真实引擎合入后 UI 零改动切换。
 */

/** 执行级操作（对齐 ai-dir execute::FileOp；serde 形态取协议友好展开式） */
export type DirExecuteOp = {
  /** 动作类别（ai-dir FileOp::Copy/Move/Rename/Delete/Touch/Mkdir） */
  kind: 'copy' | 'move' | 'rename' | 'delete' | 'touch' | 'mkdir';
  /** rel 路径（POSIX 风格；rename 为源路径） */
  path: string;
  /** copy/move 的源侧；delete/touch/mkdir/rename 的操作侧 */
  side: 'left' | 'right';
  /** copy/move 目标侧（缺省 = 对侧） */
  targetSide?: 'left' | 'right' | null;
  /** rename 新名称（仅 rename） */
  newName?: string | null;
  /** touch 目标 mtime（epoch ms；0/缺省 = 取对侧时间戳） */
  mtimeMs?: number | null;
};

export type DirExecuteParams = {
  sessionId: string;
  ops: DirExecuteOp[];
  /** 试运行：只校验不落盘，返回将执行清单（可对拍） */
  dryRun?: boolean | null;
};

export type DirExecuteOpStatus = 'succeeded' | 'failed' | 'skipped';

export type DirExecuteOpResult = {
  op: DirExecuteOp;
  status: DirExecuteOpStatus;
  /** 失败原因 / 跳过原因 / dry_run 说明 */
  detail: string | null;
};

/** dir/execute 应答（部分成功语义：成功/失败/跳过三清单，OP-002） */
export type DirExecuteResult = {
  sessionId: string;
  total: number;
  succeeded: DirExecuteOpResult[];
  failed: DirExecuteOpResult[];
  skipped: DirExecuteOpResult[];
  dryRun: boolean;
  cancelled: boolean;
};

/* ============================================================
 * 4. P8 提案：文件夹三路合并协议（UI 本地先行，mock 同形模拟）
 * ============================================================
 *
 * ai-dir 引擎侧三路合并数据层已落地（engine/ai-dir/src/merge3way.rs：
 * scan_merge / plan_merge / run_folder_merge，含 diff3 分类/决策继承/
 * 输出目标三根执行），但 ai-protocol 归并行 P9 独占，RPC 入口待主会话
 * 协调合入。本地类型与引擎结构同形（camelCase serde 形态见
 * merge3way::DirMergeRow 及 P8 交付报告"协议/RPC 变更清单"）：
 *
 *   dir/mergeScan    { base, left, right } →
 *     { sessionId, rows: DirMergeRow[], stats }
 *   dir/mergePlan    { sessionId, target, decisions } →
 *     { sessionId, target, ops: MergePlannedOp[], stats }
 *   dir/mergeExecute { sessionId, target, ops } →
 *     { sessionId, target, total, copied, deleted, skipped, failed,
 *       cancelled, details: MergeExecDetail[], log: string[] }
 *
 * mock 引擎已同形实现（浏览器可演示）；真实引擎合入后 UI 零改动切换。
 */

/** 叶级 diff3 分类（对齐 engine/ai-dir merge3way::MergeCategory） */
export type DirMergeCategory =
  | 'same'
  | 'leftChanged'
  | 'rightChanged'
  | 'bothSame'
  | 'deletedRight'
  | 'deletedLeft'
  | 'conflict'
  | 'leftOnly'
  | 'rightOnly'
  | 'baseOnly';

/** 着色口径（FR-DMRG-002：绿=可自动 / 红=冲突 / 灰=孤立 / 暗=三方一致） */
export type DirMergeRowState = 'same' | 'auto' | 'conflict' | 'orphan';

/** 三路合并行（对齐 merge3way::DirMergeRow；平铺行 + parent 构树） */
export type DirMergeRow = {
  relPath: string;
  name: string;
  kind: EntryKind;
  base: SideInfo | null;
  left: SideInfo | null;
  right: SideInfo | null;
  category: DirMergeCategory;
  rowState: DirMergeRowState;
  /** 父 rel 路径（根的直接子项为 null） */
  parent: string | null;
  depth: number;
  /** 叶子级内容冲突（双击 → 文本合并视图占位判定） */
  conflictLeaf: boolean;
};

export type DirMergeScanStats = {
  total: number;
  files: number;
  dirs: number;
  same: number;
  auto: number;
  conflict: number;
  orphan: number;
};

export type DirMergeScanParams = {
  base: string;
  left: string;
  right: string;
  /** 客户端分配的合并会话句柄 */
  sessionId: string;
};

export type DirMergeScanResult = {
  sessionId: string;
  rows: DirMergeRow[];
  stats: DirMergeScanStats;
};

/** 合并目标（FR-DMRG-001：左/右/第三输出文件夹） */
export type MergeTarget = 'left' | 'right' | 'output';

/** 行决策（引擎 merge3way::MergeDecision 同形） */
export type MergeDecision = 'auto' | 'takeLeft' | 'takeRight' | 'both' | 'skip';

/** 合并计划动作（引擎 merge3way::MergeOpKind 同形） */
export type MergeOpKind = 'takeLeft' | 'takeRight' | 'deleteTarget' | 'keep';

/** 一条合并计划操作（PlannedOperation 三字段同形） */
export type MergePlannedOp = {
  kind: MergeOpKind;
  path: string;
  reason: string;
};

export type DirMergePlanStats = {
  total: number;
  copied: number;
  deleted: number;
  touched: number;
  skipped: number;
  failed: number;
};

export type DirMergePlanParams = {
  sessionId: string;
  target: MergeTarget;
  /** rel 路径 -> 显式决策（目录决策递归展开到子树） */
  decisions: Record<string, MergeDecision>;
};

export type DirMergePlanResult = {
  sessionId: string;
  target: MergeTarget;
  ops: MergePlannedOp[];
  stats: DirMergePlanStats;
};

/** 单项执行明细（kind 取 MergeOpKind 标签） */
export type MergeExecDetail = {
  kind: MergeOpKind | 'unknown';
  path: string;
  reason: string;
  status: 'copied' | 'deleted' | 'touched' | 'planned' | 'skipped' | 'failed';
  detail: string | null;
};

export type DirMergeExecuteParams = {
  sessionId: string;
  target: MergeTarget;
  ops: MergePlannedOp[];
};

export type DirMergeExecuteResult = {
  sessionId: string;
  target: MergeTarget;
  total: number;
  copied: number;
  deleted: number;
  touched: number;
  skipped: number;
  failed: number;
  cancelled: boolean;
  details: MergeExecDetail[];
  /** 日志面板行（对齐 FR-SYNC/DMRG 报告需求） */
  log: string[];
};
