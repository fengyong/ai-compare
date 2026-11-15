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

/* P7：三向文本合并（text/merge3/open|sections|decide|output）生成类型 */
export type { Merge3OptionsSpec } from './generated/Merge3OptionsSpec';
export type { MergeChunkDto } from './generated/MergeChunkDto';
export type { MergeChunkOrigin } from './generated/MergeChunkOrigin';
export type { MergeConflictDefault } from './generated/MergeConflictDefault';
export type { MergeConflictKind } from './generated/MergeConflictKind';
export type { MergeConflictRowDto } from './generated/MergeConflictRowDto';
export type { MergeDecideParams } from './generated/MergeDecideParams';
export type { MergeDecideResult } from './generated/MergeDecideResult';
export type { MergeDecisionDto } from './generated/MergeDecisionDto';
export type { MergeDecisionKind } from './generated/MergeDecisionKind';
export type { MergeFavorSide } from './generated/MergeFavorSide';
export type { MergeMarkerStyle } from './generated/MergeMarkerStyle';
export type { MergeOpenParams } from './generated/MergeOpenParams';
export type { MergeOpenResult } from './generated/MergeOpenResult';
export type { MergeOutputMode } from './generated/MergeOutputMode';
export type { MergeOutputParams } from './generated/MergeOutputParams';
export type { MergeOutputResult } from './generated/MergeOutputResult';
export type { MergeSaveParams } from './generated/MergeSaveParams';
export type { MergeSaveResult } from './generated/MergeSaveResult';
export type { MergeSectionsParams } from './generated/MergeSectionsParams';
export type { MergeSectionsResult } from './generated/MergeSectionsResult';

/* P11：报告生成（report/generate）生成类型 */
export type { ReportColorMode } from './generated/ReportColorMode';
export type { ReportDirColumns } from './generated/ReportDirColumns';
export type { ReportDirDisplayFilter } from './generated/ReportDirDisplayFilter';
export type { ReportDisplayFilter } from './generated/ReportDisplayFilter';
export type { ReportGenerateParams } from './generated/ReportGenerateParams';
export type { ReportGenerateResult } from './generated/ReportGenerateResult';
export type { ReportLayout } from './generated/ReportLayout';
export type { ReportPatchStyle } from './generated/ReportPatchStyle';
export type { ReportWrapMode } from './generated/ReportWrapMode';

export type { CancelParams } from './generated/CancelParams';
export type { CharRange } from './generated/CharRange';
export type { ClientInfo } from './generated/ClientInfo';
export type { CompareCriteria } from './generated/CompareCriteria';
export type { ContentCompareMode } from './generated/ContentCompareMode';
export type { ContentCompareMethod } from './generated/ContentCompareMethod';
export type { ContentCompareResult } from './generated/ContentCompareResult';
export type { DiffSummary } from './generated/DiffSummary';
export type { DirCompareContentParams } from './generated/DirCompareContentParams';
export type { DirCompareContentResult } from './generated/DirCompareContentResult';
export type { DirContentCompareRow } from './generated/DirContentCompareRow';
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
export type { DirScanIssue } from './generated/DirScanIssue';
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
export type { TextEditOp } from './generated/TextEditOp';
export type { TextEditParams } from './generated/TextEditParams';
export type { TextEditResult } from './generated/TextEditResult';
export type { TextInlineDiffParams } from './generated/TextInlineDiffParams';
export type { TextInlineDiffResult } from './generated/TextInlineDiffResult';
export type { TextLinesParams } from './generated/TextLinesParams';
export type { TextLinesResult } from './generated/TextLinesResult';
export type { TextOpenParams } from './generated/TextOpenParams';
export type { TextOpenResult } from './generated/TextOpenResult';
export type { TextSaveEntry } from './generated/TextSaveEntry';
export type { TextSaveParams } from './generated/TextSaveParams';
export type { TextSaveResult } from './generated/TextSaveResult';
export type { TextSaveSide } from './generated/TextSaveSide';
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

/* R3-07：文件夹三向合并（dir/mergeScan|mergePlan|mergeExecute）生成类型 */
export type { DirMergeCategory } from './generated/DirMergeCategory';
export type { DirMergeRowState } from './generated/DirMergeRowState';
export type { DirMergeRow } from './generated/DirMergeRow';
export type { DirMergeScanStats } from './generated/DirMergeScanStats';
export type { DirMergeScanParams } from './generated/DirMergeScanParams';
export type { DirMergeScanResult } from './generated/DirMergeScanResult';
export type { DirMergeTarget } from './generated/DirMergeTarget';
export type { DirMergeDecision } from './generated/DirMergeDecision';
export type { DirMergeOpKind } from './generated/DirMergeOpKind';
export type { DirMergePlannedOp } from './generated/DirMergePlannedOp';
export type { DirMergePlanStats } from './generated/DirMergePlanStats';
export type { DirMergePlanParams } from './generated/DirMergePlanParams';
export type { DirMergePlanResult } from './generated/DirMergePlanResult';
export type { DirMergeExecKind } from './generated/DirMergeExecKind';
export type { DirMergeExecStatus } from './generated/DirMergeExecStatus';
export type { DirMergeExecDetail } from './generated/DirMergeExecDetail';
export type { DirMergeExecuteParams } from './generated/DirMergeExecuteParams';
export type { DirMergeExecuteResult } from './generated/DirMergeExecuteResult';

/* ============================================================
 * 2. UI 侧本地辅助类型
 * ============================================================ */

import type { DiffSide } from './generated/DiffSide';
import type { CompareCriteria } from './generated/CompareCriteria';

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
  contentMethod: 'crc',
  ignoreDstHour: true,
  ignoreTzHour: false,
  compareCase: true,
  compareAttributes: false,
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
 * 3. dir/execute 文件操作执行协议（生成类型为唯一来源；D1B）
 * ============================================================
 *
 * 引擎已落地该 RPC：协议类型在 engine/ai-protocol（`ts(export)` 生成），
 * 执行层在 engine/ai-dir/src/execute.rs（dry_run / 护栏 / 部分成功语义），
 * RPC 入口 engine/ai-engine/src/handlers.rs。
 *
 *   dir/execute { sessionId, ops, dryRun? } →
 *     { sessionId, total, succeeded[], failed[], skipped[], dryRun, cancelled }
 *
 * D1B：此处原有 5 个手写「提案镜像」，与生成类型已发生形态漂移
 * （`detail` 被写成必填可空、`targetSide`/`targetPath`/`newName` 被写成
 * `| null`、缺少 `DirExecuteOpKind` 命名），现全部以生成类型为准。
 */

export type { DirExecuteOp } from './generated/DirExecuteOp';
export type { DirExecuteOpKind } from './generated/DirExecuteOpKind';
export type { DirExecuteParams } from './generated/DirExecuteParams';
export type { DirExecuteOpStatus } from './generated/DirExecuteOpStatus';
export type { DirExecuteOpResult } from './generated/DirExecuteOpResult';
export type { DirExecuteResult } from './generated/DirExecuteResult';
