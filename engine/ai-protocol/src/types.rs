//! 协议方法参数 / 结果 / 通知载荷类型（serde 即协议载荷；TS 由 ts-rs 生成）。
//!
//! 字段命名一律 camelCase（`rename_all`）；所有类型通过 `#[ts(export)]`
//! 导出到 `ui/src/protocol/generated/`（协议演进：只加不改不删）。

use serde::{Deserialize, Serialize};

// ts-rs 导出目标目录：crate 内 generated/（ts-rs 对 `..` 路径的解析不可靠，
// 故先导出到 crate 内，再由 [`crate::ts_export::export_all`] 同步到
// 仓库根 ui/src/protocol/generated/；见 lib.rs 的 export_ts_bindings 测试）。

// ---------------------------------------------------------------------------
// JSON-RPC 封装（信封层；params 在边界处为 Value，分发后转强类型）
// ---------------------------------------------------------------------------

/// 请求 id：JSON-RPC 允许 number / string（不允许 null 的服务端场景约定）
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(untagged)]
pub enum RequestId {
    /// JSON 线上为 number（TS 侧显式映射，避免 ts-rs 默认 bigint）
    Number(#[cfg_attr(feature = "ts", ts(type = "number"))] i64),
    String(String),
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestId::Number(n) => write!(f, "#{n}"),
            RequestId::String(s) => write!(f, "#{s}"),
        }
    }
}

impl RequestId {
    /// 取消注册表的内部键形式（区分 number/string 两种 id）
    pub fn cancel_key(&self) -> String {
        match self {
            RequestId::Number(n) => format!("n:{n}"),
            RequestId::String(s) => format!("s:{s}"),
        }
    }
}

/// UI -> 引擎 请求（带 id，引擎必须应答）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// UI -> 引擎 通知（无需应答）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// 引擎 -> UI 应答（result 与 error 二选一，由构造方保证）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: RequestId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::RpcErrorObject>,
}

impl RpcResponse {
    pub fn success(id: RequestId, result: serde_json::Value) -> Self {
        RpcResponse {
            jsonrpc: crate::JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: RequestId, error: crate::RpcErrorObject) -> Self {
        RpcResponse {
            jsonrpc: crate::JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// 引擎 -> UI 通知
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcServerNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
}

impl RpcServerNotification {
    pub fn new(method: &str, params: impl Serialize) -> Self {
        RpcServerNotification {
            jsonrpc: crate::JSONRPC_VERSION.to_string(),
            method: method.to_string(),
            params: serde_json::to_value(params).expect("通知载荷可序列化"),
        }
    }
}

// ---------------------------------------------------------------------------
// 共享基础类型
// ---------------------------------------------------------------------------

/// 条目类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

/// 单侧条目元数据（size/mtime；mtime 为 epoch 毫秒）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SideInfo {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub mtime_ms: u64,
}

/// 目录比较判据（P0：size + mtime 秒容差 + 内容比较模式；P2 扩展 DST/属性等）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct CompareCriteria {
    /// mtime 秒容差：|sec(l) - sec(r)| <= tolerance 判同（BC 惯例默认 2s）
    pub mtime_tolerance_sec: u32,
    /// 快速测试不一致时是否做内容比较
    pub content_compare: ContentCompareMode,
}

impl Default for CompareCriteria {
    fn default() -> Self {
        CompareCriteria {
            mtime_tolerance_sec: 2,
            content_compare: ContentCompareMode::Auto,
        }
    }
}

/// 内容比较模式（docs/03 §3.1.3：快速相同跳过 / 覆盖快速结果）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ContentCompareMode {
    /// 只做快速测试（size/mtime），不做内容比较
    Never,
    /// 快速判异（仅 mtime）时用内容比较复核；size 不同直接判异
    Auto,
    /// 全部做内容比较（分块哈希）
    Always,
}

/// 扫描状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ScanState {
    /// 后台扫描进行中（P0 为同步扫描，恒为 Completed，字段留作协议稳定）
    Running,
    Completed,
}

/// 行区间 [start, end)（半开区间，0 起）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

impl LineRange {
    pub fn new(start: u32, end: u32) -> Self {
        LineRange { start, end }
    }
    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// 字符区间 [start, end)（Unicode scalar 索引；行内差异标注用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct CharRange {
    pub start: u32,
    pub end: u32,
}

// ---------------------------------------------------------------------------
// initialize / shutdown（app/ 命名空间）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    /// 客户端支持的协议版本（引擎据此决定兼容性）
    pub protocol_version: String,
}

/// 引擎能力集（UI 据此隐藏不可用视图；ADR-009 许可门控同层）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct EngineCapabilities {
    pub dir_compare: bool,
    pub text_compare: bool,
    /// 行内差异懒计算入口（P0 协议定义类型，RPC 入口 P1 开放）
    pub inline_diff: bool,
    /// 支持 `$/cancel`
    pub cancel: bool,
    /// dir/delta 分批推送
    pub delta_batching: bool,
    /// 分批的单批最大行数
    pub max_delta_batch_rows: u32,
}

/// 许可能力（ADR-009：开源/自用默认全能力；字段为后续门控留位）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct LicenseCapabilities {
    /// Pro 门控特性（三向合并/对齐覆盖/SCC/SFTP/注册表）当前可用
    pub pro_unlocked: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub engine: EngineInfo,
    pub capabilities: EngineCapabilities,
    pub license: LicenseCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub name: String,
    pub version: String,
}

/// shutdown 请求不携带参数；结果为 null。

// ---------------------------------------------------------------------------
// dir/open（目录比较）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct DirOpenParams {
    /// 客户端分配的会话句柄（引擎按其索引模型）
    pub session_id: String,
    /// 左基文件夹（本地路径；远程 profile 为 P1+ 扩展）
    pub left: String,
    pub right: String,
    /// 扫描深度：None = 全递归；1 = 仅顶层（P0 默认）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    pub criteria: CompareCriteria,
}

impl Default for DirOpenParams {
    fn default() -> Self {
        DirOpenParams {
            session_id: String::new(),
            left: String::new(),
            right: String::new(),
            depth: Some(1),
            criteria: CompareCriteria::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirOpenResult {
    pub session_id: String,
    /// 会话内可见行总数（等于全部 delta 的 upserts 总数）
    pub row_count: u32,
    /// 最新 rev（delta 批次推完后与此一致）
    pub rev: u64,
    pub scan_state: ScanState,
}

/// 目录行状态（docs/03 §4.2 CellStatus 的 P0 子集）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DirEntryStatus {
    /// 快速测试或内容比较判定相同
    Same,
    /// size 相同但 mtime 超出容差、内容比较被禁用（未证实是否相同）
    QuickDiff,
    /// 内容比较判定不同（含 size 不同这一确定性判异）
    ContentDiff,
    /// 仅左侧存在（orphan）
    LeftOnly,
    /// 仅右侧存在（orphan）
    RightOnly,
    /// 目录且两侧均存在（P0 占位：未做子树聚合，P2 由聚合状态替换）
    DirSame,
    /// 扫描中/未知（协议稳定预留）
    Unknown,
}

/// 目录 delta 行（upsert 语义：按 id 幂等更新或插入）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirRow {
    /// 稳定行 id（相对路径，POSIX 风格；根的子项无前缀）
    pub id: String,
    pub name: String,
    pub kind: EntryKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<SideInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<SideInfo>,
    pub status: DirEntryStatus,
    /// 有子节点的目录为 true（展开由后续 dir/expand 提供，P0 预留）
    pub has_children: bool,
}

/// 各状态计数（状态栏 / 信息对话框数据源）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirStats {
    pub total: u32,
    pub same: u32,
    pub quick_diff: u32,
    pub content_diff: u32,
    pub left_only: u32,
    pub right_only: u32,
    pub dir_same: u32,
}

/// 扫描进度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub scanned: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
}

/// delta 分批信息（10 万项目录的序列化节流，docs/03 ADR-003/R9）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct BatchInfo {
    /// 0 起批次序号（同会话内单调递增）
    pub index: u32,
    /// 是否还有后续批次
    pub has_more: bool,
}

/// dir/delta 通知载荷（引擎 -> UI；批量、带 rev，docs/03 §4.2）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirDelta {
    pub session_id: String,
    /// 会话模型版本号（单调递增；每批次 +1）
    pub rev: u64,
    pub upserts: Vec<DirRow>,
    /// 被移除的行 id（P0 全量扫描不产生，字段为增量刷新预留）
    pub removals: Vec<String>,
    pub stats: DirStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ScanProgress>,
    pub batch: BatchInfo,
}

// ---------------------------------------------------------------------------
// text/open、text/sections（文本比较）
// ---------------------------------------------------------------------------

/// 行对齐算法（docs/03 §3.1.1；P0 仅 Standard（Myers O(ND) 一次性），其余为 P1）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum LineAlignAlgo {
    /// 标准对齐（P0 实现为 Myers O(ND) 一次性全量；P1 升级为分块渐进）
    Standard,
    Myers,
    Patience,
    Histogram,
}

/// 空白重要性（FR-TXT-010 空白三类的 P0 载荷；判定逻辑已实现，默认 Significant）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum WhitespaceMode {
    /// 空白完全有效（默认）
    Significant,
    /// 忽略空白数量（连续空白视作一个）
    IgnoreAmount,
    /// 忽略所有空白
    IgnoreAll,
}

/// 文本 diff 选项（P0 固定默认值；类型先行，P1 直接复用）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct TextDiffOptions {
    pub algo: LineAlignAlgo,
    pub whitespace: WhitespaceMode,
    pub ignore_case: bool,
    /// true = 比较忽略行尾风格（CRLF/LF/CR 视作等价，docs/03 §6.1 默认）
    pub ignore_eol_style: bool,
}

impl Default for TextDiffOptions {
    fn default() -> Self {
        TextDiffOptions {
            algo: LineAlignAlgo::Standard,
            whitespace: WhitespaceMode::Significant,
            ignore_case: false,
            ignore_eol_style: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct TextOpenParams {
    pub session_id: String,
    pub left: String,
    pub right: String,
    /// 缺省用引擎默认选项（docs/03 §6.1：忽略行尾风格）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<TextDiffOptions>,
}

/// diff 统计（docs/03 §4.1 DiffSummary 的协议形态）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DiffSummary {
    pub section_count: u32,
    pub equal_sections: u32,
    pub insert_sections: u32,
    pub delete_sections: u32,
    pub change_sections: u32,
    /// 等价行数（两侧）
    pub equal_lines: u32,
    /// 右侧新增行数
    pub inserted_lines: u32,
    /// 左侧删除行数
    pub deleted_lines: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TextOpenResult {
    pub session_id: String,
    pub left_line_count: u32,
    pub right_line_count: u32,
    pub section_count: u32,
    pub summary: DiffSummary,
}

/// section 类别（docs/03 §4.1 SectionKind 的双文件比较子集；Conflict 为三向合并专用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum SectionKind {
    Equal,
    /// 仅右侧有内容（新增）
    Insert,
    /// 仅左侧有内容（删除）
    Delete,
    /// 左右均有但不同（替换）
    Change,
}

/// 重要性（FR-TXT-010；P0 全部 Important）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum Importance {
    Important,
    Unimportant,
    Ignored,
}

/// 行内差异懒计算状态（docs/03 §4.1 InlineState）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum InlineState {
    NotComputed,
    Partial,
    Full,
}

/// diff section（不可变；UI 侧概览条/导航/编辑器三处共享消费）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub id: u32,
    pub kind: SectionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<LineRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<LineRange>,
    pub important: Importance,
    pub inline: InlineState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TextSectionsParams {
    pub session_id: String,
    /// 起始 section 下标（0 起）
    pub offset: u32,
    /// 取回数量上限
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TextSectionsResult {
    pub session_id: String,
    pub offset: u32,
    /// section 总数
    pub total: u32,
    pub has_more: bool,
    pub sections: Vec<Section>,
}

/// text/sections 流式推送变体（引擎 -> UI；P0 引擎走请求/响应，类型为协议稳定预留）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TextSectionsNotification {
    pub session_id: String,
    pub rev: u64,
    /// 本次追加的 section（Append 语义，docs/03 §4.1 SectionEvent）
    pub appended: Vec<Section>,
    pub done: bool,
    pub summary: DiffSummary,
}

// ---------------------------------------------------------------------------
// session/close、$/cancel
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SessionCloseParams {
    pub session_id: String,
}

/// 取消通知载荷（LSP 风格：按请求 id 取消）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct CancelParams {
    pub id: RequestId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_id_untagged_roundtrip() {
        let n: RequestId = serde_json::from_str("42").unwrap();
        assert_eq!(n, RequestId::Number(42));
        let s: RequestId = serde_json::from_str("\"abc\"").unwrap();
        assert_eq!(s, RequestId::String("abc".into()));
        assert_eq!(serde_json::to_string(&n).unwrap(), "42");
        assert_eq!(n.cancel_key(), "n:42");
        assert_eq!(s.cancel_key(), "s:abc");
    }

    #[test]
    fn rpc_response_omits_none_side() {
        let r = RpcResponse::success(RequestId::Number(1), serde_json::json!({"ok": true}));
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("error").is_none());
        let e = RpcResponse::failure(RequestId::Number(1), crate::error_object(410, "gone"));
        let v = serde_json::to_value(&e).unwrap();
        assert!(v.get("result").is_none());
        assert_eq!(v["error"]["code"], 410);
    }

    #[test]
    fn dir_row_skips_absent_sides() {
        let row = DirRow {
            id: "a.txt".into(),
            name: "a.txt".into(),
            kind: EntryKind::File,
            left: None,
            right: Some(SideInfo { size: 3, mtime_ms: 1 }),
            status: DirEntryStatus::RightOnly,
            has_children: false,
        };
        let v = serde_json::to_value(&row).unwrap();
        assert!(v.get("left").is_none());
        assert_eq!(v["status"], "rightOnly");
    }

    #[test]
    fn compare_criteria_defaults_via_serde() {
        let v: CompareCriteria = serde_json::from_str("{}").unwrap();
        assert_eq!(v, CompareCriteria::default());
        assert_eq!(v.mtime_tolerance_sec, 2);
    }

    #[test]
    fn text_diff_options_default_ignores_eol() {
        let v: TextDiffOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(v, TextDiffOptions::default());
        assert!(v.ignore_eol_style);
    }
}
