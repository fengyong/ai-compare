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
export type { CancelParams } from './generated/CancelParams';
export type { CharRange } from './generated/CharRange';
export type { ClientInfo } from './generated/ClientInfo';
export type { CompareCriteria } from './generated/CompareCriteria';
export type { ContentCompareMode } from './generated/ContentCompareMode';
export type { DiffSummary } from './generated/DiffSummary';
export type { DirDelta } from './generated/DirDelta';
export type { DirEntryStatus } from './generated/DirEntryStatus';
export type { DirOpenParams } from './generated/DirOpenParams';
export type { DirOpenResult } from './generated/DirOpenResult';
export type { DirRow } from './generated/DirRow';
export type { DiffSide } from './generated/DiffSide';
export type { DirStats } from './generated/DirStats';
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
export type { RequestId } from './generated/RequestId';
export type { RpcErrorObject } from './generated/RpcErrorObject';
export type { ScanProgress } from './generated/ScanProgress';
export type { ScanState } from './generated/ScanState';
export type { Section } from './generated/Section';
export type { SectionKind } from './generated/SectionKind';
export type { SessionCloseParams } from './generated/SessionCloseParams';
export type { SideInfo } from './generated/SideInfo';
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
