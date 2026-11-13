/**
 * ai-compare 协议类型（手写版）
 *
 * ⚠️ 对齐基准：docs/03 架构设计文档 §2.3（JSON-RPC over stdio / NDJSON）与 §4.1/§4.2（核心数据结构）。
 *
 * ⚠️ 注意：引擎线（M23 ai-protocol）会通过 ts-rs 把 Rust 类型自动生成为同构 TS 类型，
 *    输出到 `ui/src/protocol/generated/`。**将来以 generated 为准**，本文件仅作为：
 *    1. 引擎线尚未交付时的 UI 编码依据（P0 Walking Skeleton 阶段）；
 *    2. 生成类型缺失时的兜底。
 *    引擎线合入后，应在 `protocol/index.ts` 处切换 import 来源并对齐字段命名（serde 默认 camelCase 约定）。
 *
 * 协议要点（docs/03 §2.3.2）：
 * - 所有请求携带 sessionId（引擎分配句柄）
 * - 会话模型带单调递增 rev，UI 按 delta 追平，失序时 state/resync
 * - 取消：$/cancel + 任务 CancellationToken
 * - 大内容：text 行数据按需分页 text/lines；行内 diff text/inlineDiff 懒计算
 */

/* ============================================================
 * 基础与握手（app/ 命名空间）
 * ============================================================ */

/** 比较侧（docs/03 §4.1 Side） */
export type Side = 'left' | 'right';

/** UI 侧标识，initialize 握手时上报 */
export interface ClientInfo {
  name: string;
  version: string;
}

export interface InitializeParams {
  clientInfo: ClientInfo;
  /** 协议版本，semver；引擎可据此拒绝不兼容客户端 */
  protocolVersion: string;
}

/** 会话类型（docs/03 §3.4.2 ViewPlugin.id；P0 仅实现前两类） */
export type SessionKind =
  | 'folder-compare'
  | 'text-compare'
  | 'text-merge'
  | 'folder-merge'
  | 'folder-sync'
  | 'table-compare'
  | 'hex-compare'
  | 'picture-compare'
  | 'media-compare'
  | 'version-compare'
  | 'registry-compare'
  | 'patch-compare'
  | 'text-edit';

export interface EngineCapabilities {
  /** 引擎启用的会话/视图类型（许可门控后），UI 据此隐藏不可用视图 */
  viewKinds: SessionKind[];
  /** 其余特性开关（如 record/replay、编辑、过滤器），P0 仅作展示 */
  features: string[];
}

export interface InitializeResult {
  protocolVersion: string;
  engineInfo: { name: string; version: string };
  capabilities: EngineCapabilities;
}

/** shutdown 应答（引擎约定进程随后退出） */
export type ShutdownResult = null;

/* ============================================================
 * 目录比较（dir/ 命名空间）—— docs/03 §4.2
 * ============================================================ */

export interface DirOpenParams {
  left: string;
  right: string;
  /** 不传则由引擎分配；传入用于会话恢复场景 */
  sessionId?: string;
  /** P0 仅有快速测试参数占位；P2 扩展（秒容差/DST/大小写/内容比较方法等） */
  criteria?: DirCriteria;
}

export interface DirCriteria {
  /** 时间戳秒容差 */
  timestampToleranceSec?: number;
  /** 是否执行内容比较（P0：二进制逐字节判等） */
  compareContents?: boolean;
}

export type ScanState = 'idle' | 'running' | 'done' | 'cancelled' | 'error';

export interface DirOpenResult {
  sessionId: string;
  /** 当前已可见的行数（后台扫描渐进增加，随 dir/delta 推送） */
  rowCount: number;
  scanState: ScanState;
  rev: number;
}

/** 节点类型（docs/03 §4.2 NodeKind） */
export type NodeKind = 'file' | 'dir' | 'archive' | 'symlink' | 'special';

/**
 * 单元格比较状态（docs/03 §4.2 CellStatus 的 P0 子集）
 * 颜色语义对齐 docs/01 附录 C.1：
 *   same → 黑（暗色主题下为正常前景色）/ contentDiff|newer* → 红 /
 *   orphan* → 紫 / older* → 灰 / unknown|scanning → 灰或黄 / error → 红 X
 */
export type DirCellStatus =
  | 'unknown'
  | 'scanning'
  | 'unscanned'
  | 'same'
  | 'contentDiff'
  | 'ruleDiff'
  | 'insignificantDiff'
  | 'newerLeft'
  | 'newerRight'
  | 'olderLeft'
  | 'olderRight'
  | 'orphanLeft'
  | 'orphanRight'
  | 'error';

/** 各侧元数据（StatInfo 的 UI 投影子集） */
export interface FileStatLite {
  size?: number;
  /** Unix 毫秒时间戳 */
  mtimeMs?: number;
  /** DOS 属性字母或 Unix mode（P0 简化，仅展示） */
  attrs?: string;
}

/** 目录视图一行（docs/03 §4.2 DirNode 的可见行投影） */
export interface DirRow {
  /** 稳定行标识（NodeId），不随排序/插入抖动（docs/03 §6.3） */
  rowId: number;
  name: string;
  relPath: string;
  kind: NodeKind;
  /** 树深度（0 = 基目录顶层），用于缩进 */
  depth: number;
  status: DirCellStatus;
  left?: FileStatLite;
  right?: FileStatLite;
  /** 子会话回传的状态覆盖（docs/03 §5.1 差异回传，引擎侧写回） */
  fileOverride?: DirCellStatus | null;
}

/** 增量上推的单行变更：index 为显示行号（排序+显示过滤后的窗口序） */
export interface RowUpsert {
  index: number;
  row: DirRow;
}

export interface DirStats {
  total: number;
  same: number;
  diff: number;
  insignificant: number;
  orphanLeft: number;
  orphanRight: number;
  error: number;
  unknown: number;
}

export interface ScanProgress {
  scanned: number;
  total: number | null;
  message?: string;
}

/** 引擎 → UI 通知：目录增量（docs/03 §2.3.2 示例与 §4.2 DirDelta） */
export interface DirDeltaParams {
  sessionId: string;
  rev: number;
  upserts: RowUpsert[];
  removals: number[];
  stats?: DirStats;
  progress?: ScanProgress;
  scanState?: ScanState;
  /** 通知后当前可见行总数（便于 UI 缓存定位；未给出时按 upserts 推断） */
  total?: number;
}

/** UI → 引擎：窗口化取行（排序+显示过滤在引擎侧求值，docs/03 §6.3） */
export interface DirRowsParams {
  sessionId: string;
  offset: number;
  count: number;
}

export interface DirRowsResult {
  rev: number;
  /** 本次窗口实际可见行数上限（后台扫描中会增长） */
  total: number;
  scanState: ScanState;
  rows: DirRow[];
}

/** UI → 引擎：解析双击行的打开方式（docs/03 §3.5 比较类型插件三元组） */
export interface DirOpenChildParams {
  sessionId: string;
  rowId: number;
}

export interface FileRef {
  /** 完整路径（VFS 路径，如 zip 内路径）；演示/占位场景可缺省 */
  path?: string;
  name: string;
  encoding?: string;
}

export interface DirOpenChildResult {
  kind: SessionKind;
  /** 建议的视图类型：text | hex | ...（由 FormatRegistry 探测，docs/03 §3.1.5） */
  viewKind: 'text' | 'hex' | 'table' | 'picture' | 'media' | 'version' | 'registry' | 'external';
  left: FileRef;
  right: FileRef;
}

/* ============================================================
 * 文本比较（text/ 命名空间）—— docs/03 §4.1
 * ============================================================ */

export interface TextOpenParams {
  left: FileRef;
  right: FileRef;
  /** 父目录会话回传链路（docs/03 §5.1）：关闭时按 reportStatus 写回 fileOverride */
  parent?: { sessionId: string; rowId: number };
}

export type LineEndingKind = 'lf' | 'crlf' | 'cr' | 'mixed';

export interface TextSideMeta {
  file: FileRef;
  lineCount: number;
  lineEnding: LineEndingKind;
  encoding: string;
}

export interface TextOpenResult {
  sessionId: string;
  left: TextSideMeta;
  right: TextSideMeta;
  rev: number;
}

export type SectionKind = 'equal' | 'insert' | 'delete' | 'change' | 'conflict';

/** 重要性（docs/03 §4.1 Importance；P0 全部为 important） */
export type Importance = 'important' | 'unimportant' | 'ignored';

/** 行号区间，0 基，左闭右开 [start, end) */
export interface LineRange {
  start: number;
  end: number;
}

/** 行内 diff 懒计算状态（docs/03 §4.1 InlineState） */
export type InlineState = 'none' | 'partial' | 'full';

/** diff 部分（section），不可变，可被概览条/导航/编辑器共享消费 */
export interface Section {
  id: number;
  kind: SectionKind;
  left: LineRange | null;
  right: LineRange | null;
  importance: Importance;
  inline: InlineState;
}

export interface DiffSummary {
  sections: number;
  diffSections: number;
  important: number;
  unimportant: number;
}

/** UI → 引擎：拉取 section 列表（P0 一次性全量；后续支持 afterId 流式分页） */
export interface TextSectionsParams {
  sessionId: string;
  /** 流式拉取断点：只返回 id > afterId 的 section（引擎线交付后启用） */
  afterId?: number;
}

export interface TextSectionsResult {
  sessionId: string;
  sections: Section[];
  done: boolean;
  summary: DiffSummary;
}

/** UI → 引擎：窗口化取行内容（大文件不分页打开会 OOM，docs/03 §2.3.2 大内容） */
export interface TextLinesParams {
  sessionId: string;
  side: Side;
  start: number;
  count: number;
}

export interface TextLinesResult {
  side: Side;
  start: number;
  lines: string[];
  done: boolean;
}

/** 字符区间，0 基，左闭右开 */
export interface CharRange {
  start: number;
  end: number;
}

/** 行内差异配对（docs/03 §4.1 InlinePair）：某侧缺省表示该侧整段为插入/删除 */
export interface InlinePair {
  left?: CharRange;
  right?: CharRange;
  equal: boolean;
}

export interface TextInlineDiffParams {
  sessionId: string;
  sectionId: number;
}

export interface TextInlineDiffResult {
  sectionId: number;
  pairs: InlinePair[];
  /** 超长行/超时降级（docs/03 §6.2），UI 回退为整行高亮 */
  truncated: boolean;
}

/** UI → 引擎：关闭文本子会话；reportStatus 为 true 时引擎把判定写回父目录行 */
export interface TextCloseParams {
  sessionId: string;
  reportStatus?: boolean;
}

export interface TextCloseResult {
  /** 写回父行的状态（未关联父会话时为 null） */
  reportedStatus: DirCellStatus | null;
}

/** UI → 引擎：关闭任意会话（docs/04 §3.2 P0 方法集 session/close） */
export interface SessionCloseParams {
  sessionId: string;
}

export type SessionCloseResult = null;
