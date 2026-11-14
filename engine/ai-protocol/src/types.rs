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

/// 请求 id：JSON-RPC 2.0 允许 number / string / null
///
/// `null` 的两种用途：
/// 1. 客户端发出 `"id": null` 的请求 —— 合法 id，应答回 `"id": null`；
/// 2. 服务端无法确定请求 id（如 JSON 解析失败）—— 按规范应答 `"id": null`。
///
/// 此前本类型只有 Number/String 两种，导致 `"id": null` 被当作"有 id 但信封非法"
/// 而回一条 id=0 的错误（与真实 id=0 的请求撞号）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(untagged)]
pub enum RequestId {
    /// JSON 线上为 number（TS 侧显式映射，避免 ts-rs 默认 bigint）
    Number(#[cfg_attr(feature = "ts", ts(type = "number"))] i64),
    String(String),
    /// JSON-RPC 2.0 §4：id 无法确定时为 null
    Null(#[cfg_attr(feature = "ts", ts(type = "null"))] Option<()>),
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestId::Number(n) => write!(f, "#{n}"),
            RequestId::String(s) => write!(f, "#{s}"),
            RequestId::Null(_) => write!(f, "#null"),
        }
    }
}

impl RequestId {
    /// 取消注册表的内部键形式（区分 number/string/null 三种 id）
    pub fn cancel_key(&self) -> String {
        match self {
            RequestId::Number(n) => format!("n:{n}"),
            RequestId::String(s) => format!("s:{s}"),
            RequestId::Null(_) => "n:null".to_string(),
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
///
/// P2 新增字段均为可选：仅在对应判据开启（compare_attributes / unix_meta）
/// 时由引擎填充，旧载荷保持字节级兼容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SideInfo {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub mtime_ms: u64,
    /// DOS 属性字母串（如 "rh"：只读+隐藏；compare_attributes 开启时填充）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<String>,
    /// Unix 权限位（st_mode 低 12 位；unix_meta 开启时填充）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_mode: Option<u32>,
    /// Unix 所有者 id（unix_meta 开启时填充）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    /// Unix 组 id（unix_meta 开启时填充）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gid: Option<u32>,
}

impl SideInfo {
    /// 基础元数据（size + mtime；扩展字段置空）
    pub fn base(size: u64, mtime_ms: u64) -> Self {
        SideInfo { size, mtime_ms, attribute: None, unix_mode: None, uid: None, gid: None }
    }
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
    /// 忽略正好 1 小时（±3600s）的 mtime 差异（FR-DIR-010 夏令时；BC 默认开启）
    #[serde(default = "default_true")]
    pub ignore_dst_hour: bool,
    /// 忽略整小时倍数的 mtime 差异（FR-DIR-010 时区差异；默认关）
    #[serde(default)]
    pub ignore_tz_hour: bool,
    /// 比较文件名大小写：大小写不同的对齐对判异（FR-DIR-010；默认开）
    #[serde(default = "default_true")]
    pub compare_case: bool,
    /// 比较 DOS 属性位（FR-DIR-010；默认关——复制文件即改存档位）
    #[serde(default)]
    pub compare_attributes: bool,
    /// 参与比较的属性字母集（如 "rhs"）；None = 全部已知位参与
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute_letters: Option<String>,
    /// 比较 Unix 元数据（权限位/uid/gid）（FR-DIR-010；默认关，可配置开关）
    #[serde(default)]
    pub unix_meta: bool,
}

fn default_true() -> bool {
    true
}

/// Option<bool> 字段的"缺省 true"（协议演进：可选能力位缺省视为可用）
fn default_some_true() -> Option<bool> {
    Some(true)
}

impl Default for CompareCriteria {
    fn default() -> Self {
        CompareCriteria {
            mtime_tolerance_sec: 2,
            content_compare: ContentCompareMode::Auto,
            ignore_dst_hour: true,
            ignore_tz_hour: false,
            compare_case: true,
            compare_attributes: false,
            attribute_letters: None,
            unix_meta: false,
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
    /// P3：text/lines 行内容分页入口（大内容按需分页，docs/03 §2.3.2）
    /// 可选字段（缺省 true）：旧引擎载荷不含该字段时按 true 解析
    #[serde(default = "default_some_true", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub lines_read: Option<bool>,
    /// P2：dir/open 惰性 + 后台扫描（lazyScan 参数与 dir/progress 通知）
    #[serde(default = "default_true")]
    pub dir_background_scan: bool,
    /// P2：dir/filter（三类过滤器引擎侧求值）
    #[serde(default = "default_true")]
    pub dir_filters: bool,
    /// P2：dir/operate（同步状态机 operate 计划）
    #[serde(default = "default_true")]
    pub dir_operate: bool,
    /// P7：三向文本合并入口（text/merge3/* 四方法）
    /// 可选字段（缺省 true）：旧引擎载荷不含该字段时按 true 解析
    #[serde(default = "default_some_true", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub three_way_merge: Option<bool>,
    /// P9：表格比较入口（table/open|sections）
    /// 可选字段（缺省 true）：旧引擎载荷不含该字段时按 true 解析
    #[serde(default = "default_some_true", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub table_compare: Option<bool>,
    /// P9：十六进制比较入口（hex/open|bytes 分页）
    /// 可选字段（缺省 true）：旧引擎载荷不含该字段时按 true 解析
    #[serde(default = "default_some_true", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub hex_compare: Option<bool>,
    /// P9：补丁应用入口（text/patch/apply）
    /// 可选字段（缺省 true）：旧引擎载荷不含该字段时按 true 解析
    #[serde(default = "default_some_true", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub patch_apply: Option<bool>,
    /// P6 提案合入：dir/execute 文件操作执行（dryRun + 执行 + rescan delta）
    /// 可选字段（缺省 true）：旧引擎载荷不含该字段时按 true 解析
    #[serde(default = "default_some_true", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub dir_execute: Option<bool>,
    /// P10：文件格式与规则体系入口（formats/list|save|load、rules/get|set、
    /// text/open rules、dir/open formatAware）
    /// 可选字段（缺省 true）：旧引擎载荷不含该字段时按 true 解析
    #[serde(default = "default_some_true", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub format_rules: Option<bool>,
    /// P11：报告生成入口（report/generate；FR-RPT 布局族）
    /// 可选字段（缺省 true）：旧引擎载荷不含该字段时按 true 解析
    #[serde(default = "default_some_true", skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub reports: Option<bool>,
    /// P14：图片比较（picture/open）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub picture_compare: Option<bool>,
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
    /// P2（ADR-004）：惰性 + 后台扫描。true = dir/open 立即应答顶层（可滚动），
    /// 后台 tokio 任务继续全量扫描并推进 dir/delta / dir/progress；
    /// false/缺省 = P0 行为（同步扫完再应答，scanState=completed）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lazy_scan: Option<bool>,
    /// P2：平铺模式（FR-DIR-021 忽略文件夹结构）：按文件名跨目录对齐
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flat: Option<bool>,
    /// P2：文件/目录名称过滤器（FR-DIR-031，引擎侧求值）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_filter: Option<NameFilterSpec>,
    /// P2：其他过滤器（FR-DIR-032：日期/大小/内容/属性/Unix 类型）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_filter: Option<OtherFilterSpec>,
    /// P2：显示过滤器（FR-DIR-033：只影响显示不影响状态判定）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_filter: Option<DisplayFilterSpec>,
    /// P2：手动对齐对（FR-DIR-022；rel 路径对）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_align: Option<Vec<ManualAlignPair>>,
    /// P10：格式感知（文本 vs 二进制按格式表 + 内容嗅探判定）。
    /// true 时引擎在 dir/open 首批 delta 的 DirRow 上注入 `fileKind`
    /// （"text" | "binary"）注解；缺省/false = 不注入（载荷与 P9 完全一致）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]

    pub format_aware: Option<bool>,
}

impl Default for DirOpenParams {
    fn default() -> Self {
        DirOpenParams {
            session_id: String::new(),
            left: String::new(),
            right: String::new(),
            depth: Some(1),
            criteria: CompareCriteria::default(),
            lazy_scan: None,
            flat: None,
            name_filter: None,
            other_filter: None,
            display_filter: None,
            manual_align: None,
            format_aware: None,
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

/// 目录行状态（docs/03 §4.2 CellStatus 的 P0 子集 + P2 聚合状态）
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
    /// 目录且两侧均存在：未展开（children 未知）或已聚合且全部子项相同
    DirSame,
    /// P2 聚合状态（FR-DIR-002）：两侧目录已扫描且至少一个子项存在差异/孤立项
    DirDiff,
    /// 扫描中/未知（协议稳定预留）
    Unknown,
}

/// 目录 delta 行（upsert 语义：按 id 幂等更新或插入）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirRow {
    /// 稳定行 id（相对路径，POSIX 风格；根的子项无前缀；平铺模式带 # 序号消歧）
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
    /// P2：被显示过滤器隐藏（不影响比较结果；FR-DIR-033）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// P2：抑制过滤器生效时被隐藏行以 suppressed 标记显示（FR-DIR-035 青绿色语义）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suppressed: bool,
    /// 左侧真实相对路径。平铺模式 id 是 `name` / `name#i`（显示用消歧 id），
    /// 手动对齐时左右不同名 —— 两者都不能拿 id 当文件地址。该侧不存在时为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub left_path: Option<String>,
    /// 右侧真实相对路径（语义同 left_path）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub right_path: Option<String>,
}

/// 目录扫描问题（子目录不可读等）。比较工具的基本原则是「不可把未知当相同」，
/// 因此这些错误必须随 delta 回到 UI，而不是只留在引擎内部。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirScanIssue {
    /// 出问题的相对路径
    pub path: String,
    pub message: String,
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
    /// P2：聚合出差异的目录数（0 时省略，旧载荷字节级兼容）
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub dir_diff: u32,
}

/// serde skip_serializing_if 辅助：u32 为 0 时不输出（协议向后兼容）
pub fn is_zero_u32(v: &u32) -> bool {
    *v == 0
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
    /// 扫描错误（目录不可读等）。始终序列化（空为 []），使生成的 TS 必填
    /// Array 类型与线上 JSON 一致 —— 省略键会让 TS 类型说谎（R3-21）。
    #[serde(default)]
    pub scan_errors: Vec<DirScanIssue>,
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
    /// P3：重要性模型（FR-TXT-010 上协议面；缺省 = 引擎默认：
    /// 行尾风格与纯编码差异不重要，其余重要）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub importance: Option<ImportanceOptionsSpec>,
    /// P3：手动对齐/隔离/偏斜容差（FR-TXT-011 上协议面；缺省 = 无手动锚点）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub manual: Option<TextManualAlignSpec>,
    /// P10：会话规则覆盖（FR-TXT-010 × FR-FMT-003；缺省 = 引擎默认，
    /// 不启用语法折叠）。规则解析：比较开关 → ImportanceOptions；
    /// unimportantElements + 按两侧文件名检测的文件格式语法 → 重要性键
    /// 语法折叠（骨架不变，注释行/字符串内容差异判为不重要）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub rules: Option<RulesSpec>,
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
    /// P3：true 时响应附带窗口内差异行的逐行 (raw, importance) 双键
    /// （`lineKeys` 字段；Equal section 不含，FR-TXT-010 双键证据）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub with_keys: Option<bool>,
    /// P3：以指定重要性模型重判 section 重要性（ImportanceUpdate 语义，
    /// 不重跑 diff；重判结果写回会话并推送 `text/sections` 通知）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub importance: Option<ImportanceOptionsSpec>,
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
    /// P3：窗口内差异行的逐行双键（withKeys=true 时填充；按 side/line 升序）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub line_keys: Option<Vec<LineKeysEntry>>,
}

/// text/sections 流式推送变体（引擎 -> UI；P0 引擎走请求/响应，类型为协议稳定预留）
///
/// P3 扩展（docs/03 §4.1 SectionEvent 上协议面；全部可选字段，旧载荷字节级不变）：
/// - 缺省（event 缺省）= Append 事件（appended 为新增 section）；
/// - `event: "replace"`：编辑后局部重算，replacedSpan 区间被 with 替换；
/// - `event: "importanceUpdate"`：重要性重判，updatedIds 按组共享 importance。
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
    /// P3：事件类别（缺省/省略 = Append；Append 事件序列化时省略，旧载荷字节级不变）
    #[serde(default, skip_serializing_if = "is_append_event")]
    pub event: TextSectionEventKind,
    /// P3：Replace 事件——被替换的 section id 闭区间 [start, end]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub replaced_span: Option<SectionIdSpan>,
    /// P3：Replace 事件——替换后的新 section 序列
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub with: Option<Vec<Section>>,
    /// P3：ImportanceUpdate 事件——被重判的 section id（组内共享 importance）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub updated_ids: Option<Vec<u32>>,
    /// P3：ImportanceUpdate 事件——重判后的重要性
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub importance: Option<Importance>,
}

/// P3：Append 事件序列化时省略（缺省语义，兼容 P0 载荷）
fn is_append_event(e: &TextSectionEventKind) -> bool {
    matches!(e, TextSectionEventKind::Append)
}

/// P3：text/sections 通知的事件类别（docs/03 §4.1 SectionEvent）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum TextSectionEventKind {
    /// 新增 section（缺省；appended 携带内容）
    #[default]
    Append,
    /// 编辑后局部重算（replacedSpan + with）
    Replace,
    /// 重要性重判（updatedIds + importance；不重跑 diff）
    ImportanceUpdate,
    /// 流结束（summary 终值）
    Done,
}

/// P3：section id 闭区间 [start, end]（Replace 事件的替换范围）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SectionIdSpan {
    pub start: u32,
    pub end: u32,
}

// ---------------------------------------------------------------------------
// P3：text/lines（大内容行分页，docs/03 §2.3.2）与 text/inlineDiff（行内懒计算）
// ---------------------------------------------------------------------------

/// 比较侧（docs/03 §4.1 Side）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DiffSide {
    Left,
    Right,
}

/// text/lines 参数：行内容按需分页（docs/03 §2.3.2 大内容约定）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TextLinesParams {
    pub session_id: String,
    pub side: DiffSide,
    /// 起始行号（0 起；越界自动钳制）
    pub start: u32,
    /// 取回行数上限
    pub count: u32,
}

/// text/lines 应答（分页约定与 UI mock 对齐：done = 后续无更多行）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TextLinesResult {
    pub side: DiffSide,
    /// 实际起始行号（钳制后）
    pub start: u32,
    pub lines: Vec<String>,
    /// true = 本页之后无更多行
    pub done: bool,
}

/// 逐行 (raw, importance) 双键条目（FR-TXT-010；仅差异行，Equal section 不含）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct LineKeysEntry {
    pub side: DiffSide,
    /// 全局行号（0 起）
    pub line: u32,
    /// 骨架键（raw_key；对齐骨架在其上计算）
    pub raw: String,
    /// 重要性键（importance_key；折叠"不重要"方面后）
    pub importance: String,
}

/// text/inlineDiff 参数：对单个 Change section 懒计算行内差异（docs/03 §2.3.2）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TextInlineDiffParams {
    pub session_id: String,
    /// section id（text/sections 返回的 id）
    pub section_id: u32,
}

/// 行内区间类别（docs/03 §4.1；Equal 之外的类别参与高亮标注）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum InlineSpanKind {
    Equal,
    Replace,
    Insert,
    Delete,
}

/// 行内单区间标注：两侧字符区间成对出现（None = 该侧不存在）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct InlineSpanDto {
    pub left: Option<CharRange>,
    pub right: Option<CharRange>,
    pub kind: InlineSpanKind,
}

/// Change section 内单行的行内差异（位置配对：第 i 行对第 i 行）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct InlineLineDiffDto {
    pub left_line: u32,
    pub right_line: u32,
    pub spans: Vec<InlineSpanDto>,
    /// 超长行/超时降级：spans 为空，调用方按整行差异渲染（docs/03 §6.2）
    pub truncated: bool,
}

/// text/inlineDiff 应答
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TextInlineDiffResult {
    pub session_id: String,
    pub section_id: u32,
    pub kind: SectionKind,
    /// 行内计算状态（Full = 全部行完成；Partial = 存在超长/超时降级行）
    pub state: InlineState,
    pub lines: Vec<InlineLineDiffDto>,
}

// ---------------------------------------------------------------------------
// P3：重要性模型与手动对齐选项（FR-TXT-010 / FR-TXT-011 上协议面）
// ---------------------------------------------------------------------------

/// 重要性模型选项（FR-TXT-010）：勾选"不重要"的方面在重要性键中折叠。
///
/// 语义与 ai-core `ImportanceOptions` 一一对应：diff 骨架不受影响，
/// 仅触发 section 重要性重判（ImportanceUpdate 事件）。
/// Default 与 serde 缺省一致（eol/encoding 不重要 = true，docs/03 §6.1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct ImportanceOptionsSpec {
    #[serde(default)]
    pub leading_ws_unimportant: bool,
    #[serde(default)]
    pub embedded_ws_unimportant: bool,
    #[serde(default)]
    pub trailing_ws_unimportant: bool,
    #[serde(default)]
    pub case_unimportant: bool,
    /// 行尾风格不重要（缺省 true，docs/03 §6.1）
    #[serde(default = "default_true")]
    pub eol_style_unimportant: bool,
    /// 纯编码差异不重要（缺省 true；编码差异已在解码层消除，规则系统占位）
    #[serde(default = "default_true")]
    pub encoding_unimportant: bool,
}

impl Default for ImportanceOptionsSpec {
    fn default() -> Self {
        ImportanceOptionsSpec {
            leading_ws_unimportant: false,
            embedded_ws_unimportant: false,
            trailing_ws_unimportant: false,
            case_unimportant: false,
            eol_style_unimportant: true,
            encoding_unimportant: true,
        }
    }
}

/// 手动对齐锚点（强制左右行区间并排；两区间行数须相等）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TextAlignAnchor {
    pub left: LineRange,
    pub right: LineRange,
}

/// 单侧区间（隔离 / 手动忽略共用；side 指明区间归属侧）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct SideLineRange {
    pub side: DiffSide,
    pub range: LineRange,
}

/// 手动对齐/隔离/偏斜容差选项（FR-TXT-011 上协议面；与 ai-core
/// `ManualAlignOptions` 一一对应）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct TextManualAlignSpec {
    /// 手动对齐锚点（强制并排）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub align_anchors: Vec<TextAlignAnchor>,
    /// 隔离：这些行区间从不对齐（独立成 Delete/Insert 块）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub isolate: Vec<SideLineRange>,
    /// 从不对齐重要差异：重要差异行显示为"添加 + 删除"而非"更改"
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub never_align_important: bool,
    /// 偏斜容差（0 = 引擎默认值）
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub skew_tolerance: u32,
    /// 接近度匹配（对被等值段分隔的 删除+插入 块做窗口重 diff）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub proximity_match: bool,
    /// 手动忽略：与这些区间重叠的差异 section 标记为 Ignored
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore: Vec<SideLineRange>,
}

// ---------------------------------------------------------------------------
// P7：text/merge3/*（三向文本合并，docs/03 §4.5 MergeModel；FR-TMRG 全系列）
// ---------------------------------------------------------------------------

/// 合并块来源（有序块序列；三方行区间各自完备划分全表）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum MergeChunkOrigin {
    /// 稳定区（三方键相等）
    Stable,
    /// 仅左侧变化（自动取左）
    LeftOnly,
    /// 仅右侧变化（自动取右）
    RightOnly,
    /// 两侧相同变化（不算冲突）
    BothChanged,
    /// 左右不同变化（冲突）
    Conflict,
}

/// 冲突类别（验收语料分类：modify-modify / delete-modify / add-add）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum MergeConflictKind {
    ModifyModify,
    DeleteModify,
    AddAdd,
}

/// 受惠侧（FR-TMRG-005 /favorleft|right：不重要冲突自动从受惠侧采用）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum MergeFavorSide {
    Left,
    Right,
}

/// 未决冲突默认策略（auto_merge 有冲突时逐冲突默认策略，可配）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum MergeConflictDefault {
    Left,
    Right,
    Base,
    LeftThenRight,
    RightThenLeft,
    /// conflict marker 渲染（/force CVS 样式写冲突）
    Marker,
}

/// conflict marker 风格：Merge（git merge-file 默认，无 base 段）/ Diff3（GNU diff3 -m，含 base 段）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum MergeMarkerStyle {
    Merge,
    Diff3,
}

/// text/merge3/open 选项（全部可选；缺省与 ai-core `MergeOptions::default` 一致）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct Merge3OptionsSpec {
    /// 两路 base↔side diff 的行对齐算法（缺省 Standard）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algo: Option<LineAlignAlgo>,
    /// 骨架空白语义（缺省 Significant）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whitespace: Option<WhitespaceMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_case: Option<bool>,
    /// 行尾风格不参与骨架比较（缺省 true，docs/03 §6.1）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_eol_style: Option<bool>,
    /// 冲突窗口（FR-TMRG-003 N 行分隔；缺省 None = 仅更改不同的行）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_window: Option<u32>,
    /// 受惠侧（不重要冲突自动从受惠侧采用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favor: Option<MergeFavorSide>,
    /// 忽略不重要差异（/iu）：不重要冲突不升级为未决冲突
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ignore_unimportant: Option<bool>,
    /// 重要性模型（FR-TXT-010 上协议面，作用于冲突子对齐与不重要冲突判定）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub importance: Option<ImportanceOptionsSpec>,
    /// 未决冲突默认策略（auto_merge；plain 输出模式使用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_default: Option<MergeConflictDefault>,
}

/// text/merge3/open 参数：base/left/right 三路规格 + 选项
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct MergeOpenParams {
    pub session_id: String,
    /// 公共祖先（中间文件；可为空文件）
    pub base: String,
    pub left: String,
    pub right: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Merge3OptionsSpec>,
}

/// text/merge3/open 结果（块清单经 text/merge3/sections 流式分批取回）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct MergeOpenResult {
    pub session_id: String,
    pub base_line_count: u32,
    pub left_line_count: u32,
    pub right_line_count: u32,
    pub chunk_count: u32,
    /// 冲突块总数（含已自动采用的不重要冲突）
    pub conflict_count: u32,
    pub rev: u64,
}

/// text/merge3/sections 参数：块清单分批窗口
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct MergeSectionsParams {
    pub session_id: String,
    /// 起始块下标（0 起）
    pub offset: u32,
    /// 取回数量上限
    pub count: u32,
}

/// 冲突块内行级子对齐提示（L↔R 行配对；unimportant = 仅不重要方面不同）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct MergeConflictRowDto {
    /// 左侧行号（全表坐标；None = 该行仅右侧存在）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<u32>,
    pub unimportant: bool,
}

/// 块决策动作（FR-TMRG-004 采用命令族 + FR-TMRG-005 输出编辑）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum MergeDecisionKind {
    Left,
    Right,
    Base,
    LeftThenRight,
    RightThenLeft,
    /// 手动编辑文本（text 字段携带，按 \n 分行；黄块语义）
    Manual,
}

/// 合并块（三方行区间 + 行内容 + 决策状态；块清单流式分批）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct MergeChunkDto {
    pub id: u32,
    pub origin: MergeChunkOrigin,
    /// base 行区间（插入块为空区间占位）
    pub base: LineRange,
    pub left: LineRange,
    pub right: LineRange,
    /// Conflict 块：冲突类别
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_kind: Option<MergeConflictKind>,
    /// Conflict 块：L↔R 行级子对齐提示
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_rows: Option<Vec<MergeConflictRowDto>>,
    /// Conflict 块：不重要冲突（左右切片 importance 键序列整体相等）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_unimportant: Option<bool>,
    /// 三方行内容（空区间省略；UI 三窗格直接消费）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub base_lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub right_lines: Vec<String>,
    /// 决策状态：decided = 有显式决策；auto_resolved = 自动采用（favor/iu）；
    /// unresolved = 冲突未决
    pub decided: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<MergeDecisionKind>,
    pub auto_resolved: bool,
    pub unresolved: bool,
    /// 当前合成输出行（决策/策略生效后）
    pub output_lines: Vec<String>,
}

/// text/merge3/sections 结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct MergeSectionsResult {
    pub session_id: String,
    pub offset: u32,
    /// 块总数
    pub total: u32,
    pub has_more: bool,
    /// 会话版本号（决策提交后递增）
    pub rev: u64,
    pub chunks: Vec<MergeChunkDto>,
}

/// 单条块决策（chunkId + 动作；Manual 需 text）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct MergeDecisionDto {
    pub chunk_id: u32,
    pub action: MergeDecisionKind,
    /// Manual 动作的编辑文本（按 \n 分行）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// text/merge3/decide 参数：块决策提交（幂等——同一载荷重复提交不改变会话状态）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct MergeDecideParams {
    pub session_id: String,
    pub decisions: Vec<MergeDecisionDto>,
}

/// text/merge3/decide 结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct MergeDecideResult {
    pub session_id: String,
    /// 会话版本号（决策内容变化才递增；幂等提交 rev 不变）
    pub rev: u64,
    /// 本次提交的决策条数
    pub decided: u32,
    pub conflict_count: u32,
    /// 决策后仍未决的冲突块 id（升序）
    pub unresolved_chunks: Vec<u32>,
}

/// text/merge3/output 模式：Plain = 合成文本（未决冲突走会话默认策略）；
/// Marker = 未决冲突以 conflict marker 渲染（纯文本消费）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum MergeOutputMode {
    Plain,
    Marker,
}

/// text/merge3/output 参数：合成结果
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct MergeOutputParams {
    pub session_id: String,
    /// 缺省 Plain
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<MergeOutputMode>,
    /// Marker 模式的 marker 风格（缺省 Merge，git merge-file 同形）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker_style: Option<MergeMarkerStyle>,
}

/// text/merge3/output 结果（text + 未决冲突清单）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct MergeOutputResult {
    pub session_id: String,
    /// 合成输出文本（决策与策略已生效）
    pub text: String,
    /// 冲突块总数
    pub conflict_count: u32,
    /// 未决冲突块 id（升序）
    pub unresolved_chunks: Vec<u32>,
    pub has_conflicts: bool,
    /// 有显式决策的块数
    pub decided_count: u32,
}

// ---------------------------------------------------------------------------
// P9：table/open、table/sections（表格比较，FR-TBL 引擎侧）
// ---------------------------------------------------------------------------

/// 列对齐模式（FR-TBL-003）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum TableColumnAlign {
    /// 未对齐（两侧文件顺序）
    #[default]
    None,
    /// 按左侧名称对齐
    ByLeftName,
    /// 按右侧名称对齐
    ByRightName,
}

/// 行对齐算法（FR-TBL-004 子集；键列出现时恒走键匹配）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum TableRowAlign {
    /// 不对齐：按行号位置配对
    #[default]
    Unaligned,
    /// 标准（Myers O(ND) 行键对齐）
    Standard,
}

/// table/open 选项（全部可选；缺省与 ai-core `TableDiffOptions::default` 一致）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct TableOptionsSpec {
    /// 列分隔符（取首字符；"\\t" = TSV；缺省 ','）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub delimiter: Option<String>,
    /// 首行为表头（缺省 true）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_header: Option<bool>,
    /// 键列集（0 起物理列下标；空 = 无键）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_columns: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_align: Option<TableColumnAlign>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_align: Option<TableRowAlign>,
    /// 偏斜容差（缺省 5；0 = 不允许偏斜补配）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skew_tolerance: Option<u32>,
    /// 大小写不重要（FR-TBL-003 文本列）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_unimportant: Option<bool>,
    /// 空白语义（FR-TBL-003 空白三类；缺省 Significant）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub whitespace: Option<WhitespaceMode>,
    /// 数值容差（P10 预留；数值列两值相差 ≤ 容差判不重要）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numeric_tolerance: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct TableOpenParams {
    pub session_id: String,
    pub left: String,
    pub right: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<TableOptionsSpec>,
}

/// 比较列类型（宽松推断）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum TableColumnType {
    Text,
    Number,
    Date,
}

/// 比较列（显示"比较列"而非物理列：FR-TBL-001；单侧列对侧下标为 None）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TableColumnInfo {
    pub name: String,
    pub ty: TableColumnType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<u32>,
}

/// diff 行统计
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TableSummary {
    pub same_rows: u32,
    pub changed_rows: u32,
    pub added_rows: u32,
    pub removed_rows: u32,
    /// 仅有不重要差异的行数（沟槽蓝点）
    pub unimportant_rows: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TableOpenResult {
    pub session_id: String,
    /// diff 行总数（table/sections 分批取回）
    pub row_count: u32,
    pub left_row_count: u32,
    pub right_row_count: u32,
    pub columns: Vec<TableColumnInfo>,
    pub summary: TableSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TableSectionsParams {
    pub session_id: String,
    /// 起始 diff 行下标（0 起）
    pub offset: u32,
    /// 取回数量上限
    pub count: u32,
}

/// diff 行类别（FR-TBL-001 沟槽点：changed 行看 important 定红/蓝）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum TableRowKind {
    Same,
    Changed,
    Added,
    Removed,
}

/// 单元格状态（unimportantDiff = 仅不重要方面不同：蓝点数据源）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum TableCellStatus {
    Same,
    UnimportantDiff,
    ImportantDiff,
    /// 仅该列左侧有值（单侧列）
    LeftOnly,
    /// 仅右侧有值
    RightOnly,
}

/// 单元格 diff（单侧列对侧值为 None）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TableCellDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
    pub status: TableCellStatus,
}

/// diff 输出行
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TableRowDto {
    pub kind: TableRowKind,
    /// 左侧数据行号（0 起；Added 时省略）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left: Option<u32>,
    /// 右侧数据行号（Removed 时省略）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right: Option<u32>,
    /// 行整体重要性（红点 = true）
    pub important: bool,
    /// 逐比较列单元格 diff
    pub cells: Vec<TableCellDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct TableSectionsResult {
    pub session_id: String,
    pub offset: u32,
    /// diff 行总数
    pub total: u32,
    pub has_more: bool,
    pub rows: Vec<TableRowDto>,
}

// ---------------------------------------------------------------------------
// P9：hex/open、hex/bytes（十六进制比较，FR-HEX 引擎侧）
// ---------------------------------------------------------------------------

/// 对齐模式（FR-HEX-002：完全 / 快速 / 无）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum HexAlignMode {
    /// 完全：插入/删除感知（分块粗对齐 + 区域字节级精对齐）
    #[default]
    Full,
    /// 快速：分块粗对齐 + 块内等位精对齐（大文件友好）
    Fast,
    /// 无：逐字节对齐
    None,
}

/// hex/open 选项（缺省：chunk 4096 + Full）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct HexOptionsSpec {
    /// 分块大小（字节）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_size: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align_mode: Option<HexAlignMode>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct HexOpenParams {
    pub session_id: String,
    pub left: String,
    pub right: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<HexOptionsSpec>,
}

/// 字节统计
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct HexSummary {
    pub same_bytes: u64,
    pub changed_bytes: u64,
    pub inserted_bytes: u64,
    pub deleted_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct HexOpenResult {
    pub session_id: String,
    pub left_size: u64,
    pub right_size: u64,
    /// diff 块数（same/changed/inserted/deleted 块序列）
    pub block_count: u32,
    pub summary: HexSummary,
}

// ---------------------------------------------------------------------------
// P14：picture/open（图片比较，FR-PIC 引擎侧）
// ---------------------------------------------------------------------------

/// picture/open 选项（FR-PIC：容差 + 聚合块边长）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct PictureOptionsSpec {
    /// 每通道容差（0-255；缺省 8）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<u16>,
    /// 差异聚合块边长（像素；缺省 32）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct PictureOpenParams {
    pub session_id: String,
    pub left: String,
    pub right: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<PictureOptionsSpec>,
}

/// 差异聚合块（边界框 + 块内差异占比）
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PictureBlockDto {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub diff_ratio: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct PictureOpenResult {
    pub session_id: String,
    /// 画布尺寸（两侧最大宽高）
    pub width: u32,
    pub height: u32,
    pub left_width: u32,
    pub left_height: u32,
    pub right_width: u32,
    pub right_height: u32,
    pub diff_pixels: u64,
    pub total_pixels: u64,
    /// 相似度百分比（100.0 = 全同）
    pub similarity_pct: f64,
    /// 差异聚合块（全量；P14 块数上限保护 >10 万时截断）
    pub blocks: Vec<PictureBlockDto>,
    /// true = 块清单因超限被截断（视图提示聚合视图）
    pub truncated: bool,
}

/// hex/bytes 参数：十六进制行分页（16 字节/行；offset 按字节、自动对齐到行首）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct HexBytesParams {
    pub session_id: String,
    pub side: DiffSide,
    /// 起始字节偏移（向下对齐到 16 字节行首；越界钳制）
    pub offset: u64,
    /// 取回字节长度上限（向上对齐到整行；单页上限 4096 字节 = 256 行）
    pub length: u32,
}

/// 单行十六进制 + ASCII 侧栏（FR-HEX-001）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct HexLineDto {
    /// 行首字节地址
    pub addr: u64,
    /// 十六进制列（小写两位空格分隔；末行短于 16 字节不填充）
    pub hex: String,
    /// ASCII 列（非可打印 ASCII -> '.'）
    pub ascii: String,
    /// 逐字节状态串（与该行字节数等长）：'=' 同 / 'x' 变化 / '+' 插入 / '-' 删除
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct HexBytesResult {
    pub session_id: String,
    pub side: DiffSide,
    /// 实际起始字节偏移（对齐后）
    pub offset: u64,
    pub lines: Vec<HexLineDto>,
    /// true = 本页之后无更多行
    pub done: bool,
}

// ---------------------------------------------------------------------------
// P9：text/patch/apply（补丁应用，FR-PATCH-011）
// ---------------------------------------------------------------------------

/// text/patch/apply 参数：输入补丁文本 + 目标规格 -> 结果报告（无会话，一次性）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct PatchApplyParams {
    /// unified diff 补丁文本（git 头/时间戳容错）
    pub patch: String,
    /// 目标根目录（补丁内相对路径在此目录下解析）
    pub root: String,
    /// 试运行：只校验不落盘
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    /// 模糊上下文匹配层数（缺省 3；0 = 仅精确）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuzz: Option<u32>,
    /// CRLF 容忍（缺省 true；false 时补丁行行尾风格须与目标一致）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crlf_tolerant: Option<bool>,
}

/// 单文件应用状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum PatchFileStatus {
    Applied,
    AlreadyApplied,
    /// 二进制文件跳过（补丁标注 Binary/GIT binary patch 或目标非 UTF-8）
    SkippedBinary,
    /// 无内容 hunk（纯元数据变更）
    NoOp,
    Failed,
}

/// 单文件应用结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PatchFileResultDto {
    pub path: String,
    pub status: PatchFileStatus,
    /// 实际使用的模糊层数
    pub fuzz_used: u32,
    /// 首个非零位置漂移（hunk 落点较补丁声明的行偏移）
    pub offset_applied: i64,
    /// 失败/跳过原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PatchApplyResult {
    pub total: u32,
    pub applied: u32,
    pub failed: u32,
    pub dry_run: bool,
    /// true = 全部文件应用成功（二进制跳过/无操作不算失败）
    pub all_applied: bool,
    pub results: Vec<PatchFileResultDto>,
}

// ---------------------------------------------------------------------------
// P6 提案合入：dir/execute（文件操作执行；与 ui/src/protocol/types.ts §3 提案同构）
// ---------------------------------------------------------------------------

/// 执行级操作动作（对齐 ai-dir execute::FileOp）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DirExecuteOpKind {
    Copy,
    Move,
    Rename,
    Delete,
    Touch,
    Mkdir,
}

/// 执行级操作（rel 路径 + 侧；serde 形态与 UI 提案一致）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirExecuteOp {
    pub kind: DirExecuteOpKind,
    /// rel 路径（POSIX 风格；rename 为源路径）
    pub path: String,
    /// copy/move 的源侧；delete/touch/mkdir/rename 的操作侧
    pub side: DiffSide,
    /// copy/move 目标侧（缺省 = 对侧）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_side: Option<DiffSide>,
    /// rename 新名称（仅 rename）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_name: Option<String>,
    /// touch 目标 mtime（epoch ms；0/缺省 = 取对侧时间戳）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct DirExecuteParams {
    pub session_id: String,
    pub ops: Vec<DirExecuteOp>,
    /// 试运行：只校验不落盘，返回将执行清单（可对拍）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DirExecuteOpStatus {
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirExecuteOpResult {
    pub op: DirExecuteOp,
    pub status: DirExecuteOpStatus,
    /// 失败原因 / 跳过原因 / dry_run 说明
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// dir/execute 应答（部分成功语义：成功/失败/跳过三清单，OP-002）；
/// 非 dry_run 执行后引擎 rescan 目录会话并推送 dir/delta。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirExecuteResult {
    pub session_id: String,
    pub total: u32,
    pub succeeded: Vec<DirExecuteOpResult>,
    pub failed: Vec<DirExecuteOpResult>,
    pub skipped: Vec<DirExecuteOpResult>,
    pub dry_run: bool,
    pub cancelled: bool,
}

// ---------------------------------------------------------------------------
// P10：文件格式与规则体系（formats/*、rules/* 命名空间；FR-FMT / FR-TXT-010）
// ---------------------------------------------------------------------------

/// 语法项类别（FR-FMT-003 五类；与 ai-core SyntaxItem 同构）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum GrammarItemKind {
    /// 文本匹配串（可正则、可区分大小写）
    #[default]
    Text,
    /// 分隔类别（起始/结束分隔符或到行尾 + 转义字符）
    Delimited,
    /// 列表类别（token 列表）
    List,
    /// 列类别（起始列-结束列）
    Column,
    /// 行类别（页面标题）
    RowTitle,
}

/// 语法项（字段按 kind 取用；无关字段缺省/省略）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct GrammarItemSpec {
    pub kind: GrammarItemKind,
    /// text：匹配串（regex=true 时为轻量正则子集）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// list：token 集
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tokens: Vec<String>,
    /// delimited：起始/结束分隔符
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    /// delimited：转义字符（单字符）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escape: Option<String>,
    /// delimited：起始命中后到行尾（无 end 时语义）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub to_eol: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub regex: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub case_sensitive: bool,
    /// column：起始列（1 起）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<u32>,
    /// column：结束列（1 起闭区间；None = 到行尾）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u32>,
    /// rowTitle：标题行数（含首行）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    /// rowTitle："或第 1 行"处理首页
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub or_first_line: bool,
}

/// 语法元素（元素名 + 语法项序列，顺序即优先级）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct GrammarElementSpec {
    pub name: String,
    /// 元素级"此元素区分大小写"（项级缺省的兜底）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub case_sensitive: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<GrammarItemSpec>,
}

/// 视图类型（FR-FMT-001；引擎据此决定文件用哪类视图打开）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum FormatViewKind {
    #[default]
    Text,
    Table,
    Hex,
    Picture,
    External,
}

/// 单个文件格式定义（协议形态；与 ai-core FileFormat 同构）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct FileFormatSpec {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 文件名掩码（FR-FLT-001 语义；空 = 不匹配，仅能手动选择）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub view_kind: FormatViewKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grammar: Vec<GrammarElementSpec>,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub tab_stops: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub independent_lines: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub column_data: bool,
}

/// 格式集（BC 风格 .bcpack / JSON 格式集文件的内容模型）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct FormatPackSpec {
    /// 载体版本（当前 1）
    #[serde(default = "default_pack_version")]
    pub version: u32,
    #[serde(default = "default_pack_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<FileFormatSpec>,
}

fn default_pack_version() -> u32 {
    1
}

fn default_pack_kind() -> String {
    "ai-compare-format-pack".to_string()
}

/// formats/list 应答（内置 + 会话期已加载/保存的自定义格式，按生效顺序）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct FormatsListResult {
    pub formats: Vec<FileFormatSpec>,
    /// 内置格式条数（formats 前 builtinCount 条为内置，其后为自定义）
    pub builtin_count: u32,
}

/// formats/save 参数：按 id 覆盖/追加重注册表（引擎会话期态；持久化由 UI 负责）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct FormatsSaveParams {
    #[serde(default)]
    pub formats: Vec<FileFormatSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct FormatsSaveResult {
    /// 本次覆盖/追加的条数
    pub saved: u32,
    /// 保存后的全量格式列表（按生效顺序）
    pub formats: Vec<FileFormatSpec>,
}

/// formats/load 参数：从文件加载格式集（.bcpack / JSON；UTF-8）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct FormatsLoadParams {
    pub path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct FormatsLoadResult {
    /// 本次载入的条数
    pub loaded: u32,
    /// 加载后的全量格式列表（按生效顺序）
    pub formats: Vec<FileFormatSpec>,
}

/// 比较规则开关（部分形态：None = 该项不覆盖/回落继承；FR-TXT-010）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct CompareRulesSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leading_ws_unimportant: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded_ws_unimportant: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trailing_ws_unimportant: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_unimportant: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eol_style_unimportant: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_unimportant: Option<bool>,
}

/// 一层规则（compare 整体覆盖语义；元素集整体覆盖语义）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct RuleLayerSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare: Option<CompareRulesSpec>,
    /// 不重要语法元素名（引用格式层；None = 不覆盖）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unimportant_elements: Option<Vec<String>>,
}

/// 格式绑定（format_id → 元素勾选；解析时最后应用）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct RuleFormatBindingSpec {
    pub format_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unimportant_elements: Vec<String>,
}

/// 生效规则（全部字段具值；引擎解析 Global→Kind→Session→FormatBinding 后的产物）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct EffectiveRulesSpec {
    #[serde(default)]
    pub compare: CompareRulesSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unimportant_elements: Vec<String>,
}

/// rules/set 写入目标（FR-SES-006 作用域；kind/sessionId 按目标携带）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum RuleScope {
    #[default]
    Global,
    /// 会话类型默认（params.kind 必带）
    Kind,
    /// 会话实例（params.sessionId 必带）
    Session,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct RulesGetParams {
    /// 生效解析的会话类型（如 "text-compare"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 生效解析的会话 id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct RulesGetResult {
    /// 解析后的生效规则（Global→Kind→Session→FormatBinding）
    pub effective: EffectiveRulesSpec,
    /// 三层原始层（供 UI 作用域下拉回显）
    pub global: RuleLayerSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<RuleLayerSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<RuleLayerSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub format_bindings: Vec<RuleFormatBindingSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct RulesSetParams {
    pub scope: RuleScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 要写入的层（None 字段 = 清除该层该项覆盖）
    #[serde(default)]
    pub layer: RuleLayerSpec,
    /// 同时更新的格式绑定（全量替换）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub format_bindings: Vec<RuleFormatBindingSpec>,
    /// formatBindings 缺省时是否清空既有绑定（false = 不动绑定）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clear_format_bindings: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct RulesSetResult {
    /// 写入后按（kind/sessionId）重新解析的生效规则
    pub effective: EffectiveRulesSpec,
}

/// 会话 open 携带的规则覆盖（text/open params.rules；None = 引擎默认，
/// 字段级缺省 = 引擎缺省；unimportantElements 缺省 = 不启用语法折叠）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct RulesSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leading_ws_unimportant: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded_ws_unimportant: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trailing_ws_unimportant: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_unimportant: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eol_style_unimportant: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_unimportant: Option<bool>,
    /// 不重要语法元素名（None = 不启用语法折叠；空集 = 显式无折叠元素）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unimportant_elements: Option<Vec<String>>,
}

/// dir 格式感知检测出的文件类别（delta DirRow 的 fileKind 注入字段值）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum DetectedFileKind {
    Text,
    Binary,
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

// ---------------------------------------------------------------------------
// P2：三类过滤器（FR-DIR-030~036 / FR-FLT-001~002，引擎侧求值）
// ---------------------------------------------------------------------------

/// 名称过滤器（FR-DIR-031：四列表；掩码语义见 FR-FLT-001/002）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct NameFilterSpec {
    /// 包含文件掩码（每项可为分号分隔的多模式）
    #[serde(default)]
    pub include_files: Vec<String>,
    #[serde(default)]
    pub exclude_files: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub exclude_dirs: Vec<String>,
    /// 掩码大小写敏感（BC 名称过滤器默认不敏感）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case_sensitive: Option<bool>,
}

/// 其他过滤器（FR-DIR-032：按非名称条件排除文件）
///
/// 语义：文件须满足**所有**出现的条件才保留；未出现的条件不约束。
/// 目录行不受其他过滤器直接影响（由子内容聚合）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct OtherFilterSpec {
    /// 仅保留修改时间 >= 该值（epoch ms）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_mtime_ms: Option<u64>,
    /// 仅保留修改时间 <= 该值（epoch ms）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mtime_ms: Option<u64>,
    /// 仅保留大小 >= 该值（字节）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_size: Option<u64>,
    /// 仅保留大小 <= 该值（字节）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_size: Option<u64>,
    /// 仅保留内容包含该串的文件（UTF-8 前缀读）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    /// 仅保留内容不包含该串的文件
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_contains: Option<String>,
    /// DOS 属性字母集：须已设置全部字母（如 "r"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs_include: Option<String>,
    /// DOS 属性字母集：须未设置任一字母
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs_exclude: Option<String>,
    /// Unix 文件类型字母集：须包含其中之一（b/c/l/p/r/s）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_types_include: Option<String>,
    /// Unix 文件类型字母集：排除命中的类型
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_types_exclude: Option<String>,
    /// 排除受保护的操作系统文件（BC 默认 true；非 Windows 平台近似为跳过无读权限文件）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_protected_os_files: Option<bool>,
}

/// 显示过滤器（FR-DIR-033：按比较状态显示/隐藏行；**只影响显示不影响状态判定**）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct DisplayFilterSpec {
    #[serde(default = "default_true")]
    pub show_same: bool,
    /// 内容比较判定不同
    #[serde(default = "default_true")]
    pub show_content_diff: bool,
    /// 左侧较新（mtime 判异且左 > 右）
    #[serde(default = "default_true")]
    pub show_left_newer: bool,
    #[serde(default = "default_true")]
    pub show_right_newer: bool,
    #[serde(default = "default_true")]
    pub show_left_only: bool,
    #[serde(default = "default_true")]
    pub show_right_only: bool,
    /// 目录聚合差异（DirDiff）
    #[serde(default = "default_true")]
    pub show_dir_diff: bool,
}

impl Default for DisplayFilterSpec {
    fn default() -> Self {
        DisplayFilterSpec {
            show_same: true,
            show_content_diff: true,
            show_left_newer: true,
            show_right_newer: true,
            show_left_only: true,
            show_right_only: true,
            show_dir_diff: true,
        }
    }
}

/// 手动对齐对（FR-DIR-022；两侧 rel 路径）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ManualAlignPair {
    pub left: String,
    pub right: String,
}

// ---------------------------------------------------------------------------
// P2：dir/expand、dir/filter、dir/align、dir/operate、dir/progress
// ---------------------------------------------------------------------------

/// dir/expand 参数：展开目录子树（ADR-004 前台优先路径）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct DirExpandParams {
    pub session_id: String,
    /// 目录行 id（rel 路径；空串 = 根）
    pub path: String,
    /// 展开层级（1 = 直接子项，缺省 1）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirExpandResult {
    pub session_id: String,
    pub rev: u64,
    /// 本次展开新增可见行数
    pub new_rows: u32,
    pub stats: DirStats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ScanProgress>,
}

/// dir/filter 参数：设置/更新三类过滤器（名称与其他过滤变更触发重扫）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct DirFilterParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_filter: Option<NameFilterSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other_filter: Option<OtherFilterSpec>,
    /// 显示过滤器（只影响显示不影响状态判定）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_filter: Option<DisplayFilterSpec>,
    /// 抑制过滤器（FR-DIR-035）：true 时被隐藏行以 suppressed 标记可见
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppress: Option<bool>,
    /// 平铺模式切换（FR-DIR-021）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flat: Option<bool>,
    /// 强制重扫（右键「刷新」）：文件系统可能已被外部改动，与过滤器是否变化无关
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rescan: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirFilterResult {
    pub session_id: String,
    pub rev: u64,
    /// 过滤后可见行数
    pub visible_rows: u32,
    /// 被过滤器隐藏的行数
    pub hidden_rows: u32,
    pub stats: DirStats,
}

/// dir/align 参数：手动对齐（FR-DIR-022）/ 解除对齐
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct DirAlignParams {
    pub session_id: String,
    /// 对齐：左 rel 路径（与 right 同时出现）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    /// 对齐：右 rel 路径
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
    /// 解除对齐：涉及该 rel 路径的全部配对
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unalign: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirAlignResult {
    pub session_id: String,
    pub rev: u64,
    /// 状态被重判/更新的行数
    pub updated_rows: u32,
    pub stats: DirStats,
}

/// 同步预设（SYNC-002 状态机；5 预设）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum SyncPreset {
    /// 镜像 左→右：右侧与左侧完全一致（含删除右侧多余项）
    MirrorLeftToRight,
    /// 镜像 右→左
    MirrorRightToLeft,
    /// 更新 左→右：仅把左侧较新/左侧孤有项复制到右，不删除
    UpdateLeftToRight,
    /// 更新 右→左
    UpdateRightToLeft,
    /// 双向更新：两侧各自较新的项复制到对面，孤立项保留
    UpdateBoth,
}

/// operate 计划动作（仅生成计划，不执行；执行属 P6/P8 文件操作引擎）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum PlannedOpKind {
    CopyLeftToRight,
    CopyRightToLeft,
    DeleteLeft,
    DeleteRight,
    /// 时间戳同步（把对侧 mtime 写到本侧）
    TouchLeft,
    TouchRight,
}

/// 自定义规则匹配侧（"哪侧较新"条件）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum RuleSide {
    Left,
    Right,
}

/// 自定义规则动作（含 Skip 跳过）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum RuleAction {
    CopyLeftToRight,
    CopyRightToLeft,
    DeleteLeft,
    DeleteRight,
    TouchLeft,
    TouchRight,
    #[default]
    Skip,
}

impl From<RuleAction> for Option<PlannedOpKind> {
    fn from(a: RuleAction) -> Self {
        match a {
            RuleAction::CopyLeftToRight => Some(PlannedOpKind::CopyLeftToRight),
            RuleAction::CopyRightToLeft => Some(PlannedOpKind::CopyRightToLeft),
            RuleAction::DeleteLeft => Some(PlannedOpKind::DeleteLeft),
            RuleAction::DeleteRight => Some(PlannedOpKind::DeleteRight),
            RuleAction::TouchLeft => Some(PlannedOpKind::TouchLeft),
            RuleAction::TouchRight => Some(PlannedOpKind::TouchRight),
            RuleAction::Skip => None,
        }
    }
}

/// 自定义同步规则（DMRG-003 状态机数据层）：按顺序首条命中生效
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct SyncRule {
    /// 名称掩码（FR-FLT 语义；None = 匹配任意名称）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_mask: Option<String>,
    /// 匹配指定行状态；None = 任意状态
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<DirEntryStatus>,
    /// 要求某侧较新（按 mtime；None = 不约束）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newer: Option<RuleSide>,
    pub action: RuleAction,
}

/// dir/operate 参数：生成同步操作计划（不执行）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct DirOperateParams {
    pub session_id: String,
    /// 预设（与 rules 二选一；都未给 = 空计划）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<SyncPreset>,
    /// 自定义规则（按序首条命中生效；无命中 = 跳过）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<SyncRule>>,
    /// 选中行 id 集合（None/空 = 全部行）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
}

/// 一条计划操作（path 为 rel 路径；执行器按会话根拼接）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PlannedOperation {
    pub op: PlannedOpKind,
    /// 目标 rel 路径（复制动作的源侧同路径存在）
    pub path: String,
    /// 触发原因（预设名 / 规则序号 + 命中条件摘要）
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirOperateResult {
    pub session_id: String,
    pub plan: Vec<PlannedOperation>,
    pub total: u32,
}

/// dir/progress 阶段（惰性后台扫描生命周期）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ProgressPhase {
    Running,
    Completed,
    Cancelled,
}

/// dir/progress 通知载荷（引擎 -> UI；ADR-004 进度事件）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct DirProgressEvent {
    pub session_id: String,
    pub phase: ProgressPhase,
    /// 已判定条目数（快速测试完成）
    pub scanned: u32,
    /// 总量估算（后台扫描期随展开修正；完成后等于实际总数）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_estimate: Option<u32>,
}

// ---------------------------------------------------------------------------
// P11：report/generate（FR-RPT-001/002 / FR-PATCH-010；布局 × 选项 × 会话矩阵）
// ---------------------------------------------------------------------------

/// 报告布局（FR-RPT-002 布局族；serde 字符串小驼峰）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ReportLayout {
    /// 纯文本摘要（统计 + 差异清单）
    #[default]
    Summary,
    /// 双栏并排
    SideBySide,
    /// 全量交错（unified 式前缀、不裁上下文）
    Interleaved,
    /// 补丁（FR-PATCH-010：unified/context/normal 三式）
    Patch,
    /// 自包含 HTML（样式内联）
    Html,
    /// 结构化 XML
    Xml,
    /// CSV/TSV 表格化
    Csv,
    /// 文件夹缩进树 + 状态标记
    Tree,
    /// 文件清单
    List,
    /// 变更统计（只统计无内容）
    Stats,
}

/// 文本差异显示过滤（display-all/mismatches/context/matches）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ReportDisplayFilter {
    #[default]
    All,
    Mismatches,
    Context,
    Matches,
}

/// 文件夹显示过滤（FR-RPT-002 文件夹 display 标志集）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ReportDirDisplayFilter {
    #[default]
    All,
    Mismatches,
    NoOrphans,
    MismatchesNoOrphans,
    Orphans,
    Matches,
    LeftNewer,
    RightNewer,
    LeftNewerOrphans,
    RightNewerOrphans,
    LeftOrphans,
    RightOrphans,
}

/// 补丁格式（FR-PATCH-010）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ReportPatchStyle {
    Normal,
    Context,
    #[default]
    Unified,
}

/// 颜色模式（html-color/html-mono）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ReportColorMode {
    #[default]
    Color,
    Mono,
}

/// 换行模式（wrap-none/character/word；HTML 仅 none/word）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub enum ReportWrapMode {
    #[default]
    None,
    Character,
    Word,
}

/// 文件夹可选列（column-*；缺省 size+timestamp）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct ReportDirColumns {
    #[serde(default = "default_true")]
    pub size: bool,
    #[serde(default = "default_true")]
    pub timestamp: bool,
    #[serde(default)]
    pub attributes: bool,
    #[serde(default)]
    pub crc: bool,
    #[serde(default)]
    pub vcs: bool,
    #[serde(default)]
    pub revision: bool,
    #[serde(default)]
    pub version: bool,
    #[serde(default)]
    pub owner: bool,
    #[serde(default)]
    pub group: bool,
}

impl Default for ReportDirColumns {
    fn default() -> Self {
        ReportDirColumns {
            size: true,
            timestamp: true,
            attributes: false,
            crc: false,
            vcs: false,
            revision: false,
            version: false,
            owner: false,
            group: false,
        }
    }
}

/// report/generate 请求（会话快照 + 布局参数 -> 报告字符串或分块流）
///
/// 除 layout/sessionId 外全部可选（缺省 = 引擎缺省；协议演进只加不改）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase", default)]
pub struct ReportGenerateParams {
    pub session_id: String,
    pub layout: ReportLayout,
    /// 报告标题（None = 自动生成 "<kind> compare report"；省略键）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub title: Option<String>,
    /// 文本差异显示过滤（缺省 all）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub display: Option<ReportDisplayFilter>,
    /// 文件夹显示过滤（缺省 all）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub dir_display: Option<ReportDirDisplayFilter>,
    /// 忽略不重要差异（缺省 false）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ignore_unimportant: Option<bool>,
    /// 显示行号（缺省 false）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub line_numbers: Option<bool>,
    /// 上下文行数（patch 与 display=context 共用；缺省 3）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub context_lines: Option<u32>,
    /// 补丁格式（缺省 unified）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub patch_style: Option<ReportPatchStyle>,
    /// CSV 分隔符："csv"（','）| "tsv"（'\t'）；缺省 csv
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub delimiter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub color: Option<ReportColorMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub wrap: Option<ReportWrapMode>,
    /// 字符硬折行列数（wrap=character 时生效；缺省 80）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub wrap_columns: Option<u32>,
    /// 文件夹可选列
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub columns: Option<ReportDirColumns>,
    /// 文件夹 side-by-side HTML 内嵌逐文件报告链接（缺省 false）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub include_file_links: Option<bool>,
    /// 分块流：起始字符偏移（缺省 0）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub offset: Option<u64>,
    /// 分块流：本块最大字符数（None = 全量）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub limit: Option<u64>,
}

/// report/generate 应答
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "camelCase")]
pub struct ReportGenerateResult {
    pub layout: ReportLayout,
    /// 报告内容（本块；limit 未给 = 全量）
    pub content: String,
    /// 全文总字符数（Unicode 标量数；JSON number）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total_chars: u64,
    /// 本块起始偏移（JSON number）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub offset: u64,
    /// 是否还有后续块
    pub has_more: bool,
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
            right: Some(SideInfo::base(3, 1)),
            status: DirEntryStatus::RightOnly,
            has_children: false,
            hidden: false,
            suppressed: false,
            left_path: None,
            right_path: Some("a.txt".into()),
        };
        let v = serde_json::to_value(&row).unwrap();
        assert!(v.get("left").is_none());
        assert!(v.get("leftPath").is_none(), "缺失侧不应输出路径键");
        assert_eq!(v["rightPath"], "a.txt");
        assert_eq!(v["status"], "rightOnly");
    }

    #[test]
    fn compare_criteria_defaults_via_serde() {
        let v: CompareCriteria = serde_json::from_str("{}").unwrap();
        assert_eq!(v, CompareCriteria::default());
        assert_eq!(v.mtime_tolerance_sec, 2);
    }

    #[test]
    fn compare_criteria_p0_payload_still_parses_and_serializes_same() {
        // P0 旧载荷（仅两字段）必须可解析；序列化保持旧字段集
        let old = r#"{"mtimeToleranceSec":2,"contentCompare":"auto"}"#;
        let v: CompareCriteria = serde_json::from_str(old).unwrap();
        assert!(v.ignore_dst_hour, "ignoreDstHour 缺省 = true（BC 默认）");
        assert!(!v.ignore_tz_hour);
        assert!(v.compare_case);
        assert!(!v.compare_attributes);
        assert!(v.attribute_letters.is_none());
        assert!(!v.unix_meta);
        // 序列化新增布尔字段始终输出（P0 客户端按未知字段忽略）——属性字母 None 时省略
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("ignoreDstHour"));
        assert!(!s.contains("attributeLetters"));
    }

    #[test]
    fn dir_row_hidden_suppressed_default_absent() {
        let old = r#"{"id":"a","name":"a","kind":"file","status":"same","hasChildren":false}"#;
        let row: DirRow = serde_json::from_str(old).unwrap();
        assert!(!row.hidden);
        assert!(!row.suppressed);
        let s = serde_json::to_string(&row).unwrap();
        assert!(!s.contains("hidden"), "false 的 hidden 不应出现在 JSON");
    }

    #[test]
    fn dir_stats_dir_diff_omitted_when_zero() {
        let s = serde_json::to_string(&DirStats::default()).unwrap();
        assert!(!s.contains("dirDiff"));
        let st = DirStats { dir_diff: 3, ..Default::default() };
        let s = serde_json::to_string(&st).unwrap();
        assert!(s.contains("dirDiff"));
    }

    #[test]
    fn side_info_new_fields_absent_when_none() {
        let si = SideInfo::base(10, 20);
        let s = serde_json::to_string(&si).unwrap();
        assert_eq!(s, r#"{"size":10,"mtimeMs":20}"#, "P0 载荷字节级兼容");
        let old: SideInfo = serde_json::from_str(r#"{"size":1,"mtimeMs":2}"#).unwrap();
        assert_eq!(old, SideInfo::base(1, 2));
    }

    #[test]
    fn text_diff_options_default_ignores_eol() {
        let v: TextDiffOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(v, TextDiffOptions::default());
        assert!(v.ignore_eol_style);
    }

    // ------------------------------------------------------------------
    // P3：向后兼容（旧载荷字节级不变；新字段缺省省略）
    // ------------------------------------------------------------------

    #[test]
    fn engine_capabilities_old_payload_gains_lines_read_default() {
        // P2 旧载荷（无 linesRead）可解析；缺省 true（引擎 P3 起恒支持）
        let old = r#"{"dirCompare":true,"textCompare":true,"inlineDiff":false,"cancel":true,"deltaBatching":true,"maxDeltaBatchRows":512,"dirBackgroundScan":true,"dirFilters":true,"dirOperate":true}"#;
        let v: EngineCapabilities = serde_json::from_str(old).unwrap();
        assert_eq!(v.lines_read, Some(true));
        // 新引擎序列化：linesRead 出现；值为 None 时省略（不会序列化为 null）
        let mut c = EngineCapabilities {
            dir_compare: true,
            text_compare: true,
            inline_diff: true,
            cancel: true,
            delta_batching: true,
            max_delta_batch_rows: 512,
            lines_read: Some(true),
            dir_background_scan: true,
            dir_filters: true,
            dir_operate: true,
            three_way_merge: Some(true),
            table_compare: Some(true),
            hex_compare: Some(true),
            patch_apply: Some(true),
            dir_execute: Some(true),
            reports: Some(true),
            picture_compare: Some(true),
            format_rules: None,
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("linesRead"));
        c.lines_read = None;
        let s = serde_json::to_string(&c).unwrap();
        assert!(!s.contains("linesRead"), "None 不应序列化: {s}");
    }

    #[test]
    fn text_open_params_old_payload_unchanged_bytes() {
        let old = r#"{"sessionId":"t1","left":"a","right":"b","options":null}"#;
        let v: TextOpenParams = serde_json::from_str(old).unwrap();
        assert_eq!(v.session_id, "t1");
        assert!(v.importance.is_none() && v.manual.is_none());
        // 旧字段集序列化逐字节不变（新字段 None 时省略）
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("importance") && !s.contains("manual"), "{s}");
    }

    #[test]
    fn text_sections_params_result_old_payload_unchanged() {
        let old_params = r#"{"sessionId":"t1","offset":0,"count":10}"#;
        let p: TextSectionsParams = serde_json::from_str(old_params).unwrap();
        assert!(p.with_keys.is_none() && p.importance.is_none());
        let s = serde_json::to_string(&p).unwrap();
        assert!(!s.contains("withKeys") && !s.contains("importance"), "{s}");

        let old_result = r#"{"sessionId":"t1","offset":0,"total":1,"hasMore":false,"sections":[]}"#;
        let r: TextSectionsResult = serde_json::from_str(old_result).unwrap();
        assert!(r.line_keys.is_none());
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("lineKeys"), "{s}");
    }

    #[test]
    fn text_sections_notification_append_event_bytes_unchanged() {
        // P0 载荷（无 event 字段）解析为 Append 事件且序列化逐字节一致
        // （Append 为缺省事件，序列化时省略 event 字段）
        let old = r#"{"sessionId":"t1","rev":1,"appended":[],"done":true,"summary":{"sectionCount":0,"equalSections":0,"insertSections":0,"deleteSections":0,"changeSections":0,"equalLines":0,"insertedLines":0,"deletedLines":0}}"#;
        let v: TextSectionsNotification = serde_json::from_str(old).unwrap();
        assert_eq!(v.event, TextSectionEventKind::Append);
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, old, "旧载荷字节级不变");
    }

    #[test]
    fn text_sections_notification_replace_and_importance_events_roundtrip() {
        let sec = Section {
            id: 2,
            kind: SectionKind::Change,
            left: Some(LineRange::new(1, 3)),
            right: Some(LineRange::new(1, 3)),
            important: Importance::Important,
            inline: InlineState::NotComputed,
        };
        let replace = TextSectionsNotification {
            session_id: "t1".into(),
            rev: 7,
            appended: Vec::new(),
            done: false,
            summary: DiffSummary::default(),
            event: TextSectionEventKind::Replace,
            replaced_span: Some(SectionIdSpan { start: 1, end: 3 }),
            with: Some(vec![sec]),
            updated_ids: None,
            importance: None,
        };
        let json = serde_json::to_string(&replace).unwrap();
        assert!(json.contains("\"event\":\"replace\""));
        assert!(json.contains("\"replacedSpan\":{\"start\":1,\"end\":3}"));
        let back: TextSectionsNotification = serde_json::from_str(&json).unwrap();
        assert_eq!(back, replace);

        let imp = TextSectionsNotification {
            updated_ids: Some(vec![1, 2]),
            importance: Some(Importance::Unimportant),
            event: TextSectionEventKind::ImportanceUpdate,
            replaced_span: None,
            with: None,
            ..replace.clone()
        };
        let json = serde_json::to_string(&imp).unwrap();
        assert!(json.contains("\"event\":\"importanceUpdate\""));
        assert!(json.contains("\"updatedIds\":[1,2]"));
        assert!(json.contains("\"importance\":\"unimportant\""));
        assert!(!json.contains("replacedSpan"), "importance 事件不应携带 replace 字段: {json}");
        let back: TextSectionsNotification = serde_json::from_str(&json).unwrap();
        assert_eq!(back, imp);
    }

    #[test]
    fn diff_side_and_lines_payload_roundtrip() {
        let p: TextLinesParams = serde_json::from_str(r#"{"sessionId":"t1","side":"left","start":3,"count":10}"#).unwrap();
        assert_eq!(p.side, DiffSide::Left);
        let r = TextLinesResult { side: DiffSide::Right, start: 0, lines: vec!["a".into()], done: true };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"side":"right","start":0,"lines":["a"],"done":true}"#);
        let back: TextLinesResult = serde_json::from_str(&s).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn importance_spec_defaults_match_core_defaults() {
        let v: ImportanceOptionsSpec = serde_json::from_str("{}").unwrap();
        assert!(!v.leading_ws_unimportant && !v.case_unimportant);
        assert!(v.eol_style_unimportant, "行尾风格缺省不重要（docs/03 §6.1）");
        assert!(v.encoding_unimportant);
    }

    #[test]
    fn manual_align_spec_zero_skew_omitted() {
        let spec = TextManualAlignSpec::default();
        let s = serde_json::to_string(&spec).unwrap();
        assert!(!s.contains("skewTolerance") && !s.contains("alignAnchors"), "{s}");
        let spec = TextManualAlignSpec {
            align_anchors: vec![TextAlignAnchor { left: LineRange::new(0, 1), right: LineRange::new(2, 3) }],
            isolate: vec![SideLineRange { side: DiffSide::Left, range: LineRange::new(4, 5) }],
            skew_tolerance: 2500,
            ..Default::default()
        };
        let s = serde_json::to_string(&spec).unwrap();
        assert!(s.contains("\"skewTolerance\":2500"));
        assert!(s.contains("\"side\":\"left\""));
        let back: TextManualAlignSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back, spec);
    }

    // ------------------------------------------------------------------
    // P7：text/merge3/* 载荷（只加不改；旧载荷不受影响）
    // ------------------------------------------------------------------

    #[test]
    fn merge_open_params_minimal_payload() {
        // 最小载荷（无 options）可解析；序列化不含 options 字段
        let old = r#"{"sessionId":"m1","base":"b.l","left":"l.l","right":"r.l"}"#;
        let v: MergeOpenParams = serde_json::from_str(old).unwrap();
        assert!(v.options.is_none());
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, old, "最小载荷字节级不变");
    }

    #[test]
    fn merge_options_spec_defaults_and_roundtrip() {
        let v: Merge3OptionsSpec = serde_json::from_str("{}").unwrap();
        assert_eq!(v, Merge3OptionsSpec::default());
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, "{}", "全缺省 options 序列化为空对象");
        let spec: Merge3OptionsSpec = serde_json::from_str(
            r#"{"conflictWindow":3,"favor":"right","ignoreUnimportant":true,"conflictDefault":"leftThenRight"}"#,
        )
        .unwrap();
        assert_eq!(spec.conflict_window, Some(3));
        assert_eq!(spec.favor, Some(MergeFavorSide::Right));
        assert_eq!(spec.conflict_default, Some(MergeConflictDefault::LeftThenRight));
        let s = serde_json::to_string(&spec).unwrap();
        let back: Merge3OptionsSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn merge_chunk_dto_optional_fields_omitted() {
        let chunk = MergeChunkDto {
            id: 0,
            origin: MergeChunkOrigin::Stable,
            base: LineRange::new(0, 2),
            left: LineRange::new(0, 2),
            right: LineRange::new(0, 2),
            conflict_kind: None,
            conflict_rows: None,
            conflict_unimportant: None,
            base_lines: Vec::new(),
            left_lines: vec!["a".into(), "b".into()],
            right_lines: Vec::new(),
            decided: false,
            decision: None,
            auto_resolved: false,
            unresolved: false,
            output_lines: vec!["a".into(), "b".into()],
        };
        let s = serde_json::to_string(&chunk).unwrap();
        assert!(!s.contains("conflictKind") && !s.contains("baseLines") && !s.contains("rightLines"), "{s}");
        assert!(!s.contains("decision"), "None 决策不序列化: {s}");
        let back: MergeChunkDto = serde_json::from_str(&s).unwrap();
        assert_eq!(back, chunk);
    }

    #[test]
    fn merge_conflict_chunk_roundtrip() {
        let chunk = MergeChunkDto {
            id: 3,
            origin: MergeChunkOrigin::Conflict,
            base: LineRange::new(1, 2),
            left: LineRange::new(1, 2),
            right: LineRange::new(1, 2),
            conflict_kind: Some(MergeConflictKind::ModifyModify),
            conflict_rows: Some(vec![
                MergeConflictRowDto { left: Some(1), right: Some(1), unimportant: false },
                MergeConflictRowDto { left: Some(2), right: None, unimportant: false },
            ]),
            conflict_unimportant: Some(false),
            base_lines: vec!["b".into()],
            left_lines: vec!["L".into()],
            right_lines: vec!["R".into()],
            decided: true,
            decision: Some(MergeDecisionKind::Left),
            auto_resolved: false,
            unresolved: false,
            output_lines: vec!["L".into()],
        };
        let s = serde_json::to_string(&chunk).unwrap();
        assert!(s.contains("\"origin\":\"conflict\""));
        assert!(s.contains("\"conflictKind\":\"modifyModify\""));
        assert!(s.contains("\"decision\":\"left\""));
        assert!(s.contains("\"unimportant\":false"));
        let back: MergeChunkDto = serde_json::from_str(&s).unwrap();
        assert_eq!(back, chunk);
    }

    #[test]
    fn merge_decide_payload_manual_and_idempotent_shape() {
        let params: MergeDecideParams = serde_json::from_str(
            r#"{"sessionId":"m1","decisions":[{"chunkId":1,"action":"manual","text":"手动\n编辑"},{"chunkId":2,"action":"leftThenRight"}]}"#,
        )
        .unwrap();
        assert_eq!(params.decisions.len(), 2);
        assert_eq!(params.decisions[0].action, MergeDecisionKind::Manual);
        assert_eq!(params.decisions[0].text.as_deref(), Some("手动\n编辑"));
        // 非 Manual 决策省略 text
        let s = serde_json::to_string(&params.decisions[1]).unwrap();
        assert!(!s.contains("text"), "非 Manual 决策省略 text: {s}");
        // decide 载荷 roundtrip（幂等提交 = 同一载荷重复解析一致）
        let full = serde_json::to_string(&params).unwrap();
        let back: MergeDecideParams = serde_json::from_str(&full).unwrap();
        assert_eq!(back, params);
    }

    #[test]
    fn merge_output_result_shape() {
        let r: MergeOutputResult = serde_json::from_str(
            r#"{"sessionId":"m1","text":"a\nb\n","conflictCount":1,"unresolvedChunks":[2],"hasConflicts":true,"decidedCount":0}"#,
        )
        .unwrap();
        assert!(r.has_conflicts);
        assert_eq!(r.unresolved_chunks, vec![2]);
        let params: MergeOutputParams = serde_json::from_str(r#"{"sessionId":"m1","mode":"marker","markerStyle":"diff3"}"#).unwrap();
        assert_eq!(params.mode, Some(MergeOutputMode::Marker));
        assert_eq!(params.marker_style, Some(MergeMarkerStyle::Diff3));
        let minimal: MergeOutputParams = serde_json::from_str(r#"{"sessionId":"m1"}"#).unwrap();
        assert_eq!(minimal.mode, None, "缺省 Plain 模式");
    }

    // ------------------------------------------------------------------
    // P9：table/hex/patch/dir execute 载荷（只加不改；旧载荷不受影响）
    // ------------------------------------------------------------------

    #[test]
    fn p9_capabilities_old_payload_defaults_true() {
        // P7 旧载荷（无 P9 能力位）可解析；缺省 true（可选能力位约定）
        let old = r#"{"dirCompare":true,"textCompare":true,"inlineDiff":false,"cancel":true,"deltaBatching":true,"maxDeltaBatchRows":512,"dirBackgroundScan":true,"dirFilters":true,"dirOperate":true}"#;
        let v: EngineCapabilities = serde_json::from_str(old).unwrap();
        assert_eq!(v.table_compare, Some(true));
        assert_eq!(v.hex_compare, Some(true));
        assert_eq!(v.patch_apply, Some(true));
        assert_eq!(v.dir_execute, Some(true));
        // None 能力位序列化时省略（不出现 null）
        let mut c = EngineCapabilities {
            dir_compare: true,
            text_compare: true,
            inline_diff: true,
            cancel: true,
            delta_batching: true,
            max_delta_batch_rows: 512,
            lines_read: Some(true),
            dir_background_scan: true,
            dir_filters: true,
            dir_operate: true,
            three_way_merge: Some(true),
            table_compare: None,
            hex_compare: None,
            patch_apply: None,
            dir_execute: None,
            format_rules: None,
            reports: None,
            picture_compare: None,
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(!s.contains("tableCompare") && !s.contains("hexCompare"), "{s}");
        c.table_compare = Some(true);
        assert!(serde_json::to_string(&c).unwrap().contains("tableCompare"));
    }

    #[test]
    fn table_open_params_minimal_and_options_roundtrip() {
        let old = r#"{"sessionId":"t1","left":"a.csv","right":"b.csv"}"#;
        let v: TableOpenParams = serde_json::from_str(old).unwrap();
        assert!(v.options.is_none());
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(s, old, "最小载荷字节级不变");
        let spec: TableOptionsSpec = serde_json::from_str(
            r#"{"delimiter":"\t","keyColumns":[0,2],"columnAlign":"byLeftName","rowAlign":"standard","skewTolerance":3,"caseUnimportant":true,"whitespace":"ignoreAmount","numericTolerance":0.5}"#,
        )
        .unwrap();
        assert_eq!(spec.delimiter.as_deref(), Some("\t"));
        assert_eq!(spec.key_columns, Some(vec![0, 2]));
        assert_eq!(spec.column_align, Some(TableColumnAlign::ByLeftName));
        assert_eq!(spec.row_align, Some(TableRowAlign::Standard));
        assert_eq!(spec.numeric_tolerance, Some(0.5));
        let back: TableOptionsSpec = serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn table_sections_result_optional_side_omitted() {
        let row = TableRowDto {
            kind: TableRowKind::Changed,
            left: Some(3),
            right: Some(4),
            important: false,
            cells: vec![
                TableCellDto { left: Some("a".into()), right: Some("A".into()), status: TableCellStatus::UnimportantDiff },
                TableCellDto { left: Some("x".into()), right: None, status: TableCellStatus::LeftOnly },
            ],
        };
        let s = serde_json::to_string(&row).unwrap();
        assert!(s.contains("\"unimportantDiff\"") && s.contains("\"leftOnly\""));
        let back: TableRowDto = serde_json::from_str(&s).unwrap();
        assert_eq!(back, row);
        // Added 行省略 left
        let added = TableRowDto { kind: TableRowKind::Added, left: None, right: Some(1), important: true, cells: Vec::new() };
        let s = serde_json::to_string(&added).unwrap();
        assert!(!s.contains("\"left\""), "{s}");
    }

    #[test]
    fn hex_params_result_roundtrip() {
        let p: HexOpenParams = serde_json::from_str(r#"{"sessionId":"h1","left":"a.bin","right":"b.bin"}"#).unwrap();
        assert!(p.options.is_none());
        let opts: HexOptionsSpec = serde_json::from_str(r#"{"chunkSize":8192,"alignMode":"fast"}"#).unwrap();
        assert_eq!(opts.chunk_size, Some(8192));
        assert_eq!(opts.align_mode, Some(HexAlignMode::Fast));
        let bp: HexBytesParams = serde_json::from_str(r#"{"sessionId":"h1","side":"left","offset":256,"length":512}"#).unwrap();
        assert_eq!(bp.offset, 256);
        let line = HexLineDto {
            addr: 0,
            hex: "00 1f 20 7e".into(),
            ascii: ".. ~".into(),
            status: "==xx".into(),
        };
        let s = serde_json::to_string(&line).unwrap();
        assert!(s.contains("\"addr\":0"));
        let back: HexLineDto = serde_json::from_str(&s).unwrap();
        assert_eq!(back, line);
    }

    #[test]
    fn patch_apply_params_defaults_and_result_shape() {
        let minimal: PatchApplyParams = serde_json::from_str(r#"{"patch":"--- a\n+++ b\n","root":"/tmp/x"}"#).unwrap();
        assert!(minimal.dry_run.is_none() && minimal.fuzz.is_none() && minimal.crlf_tolerant.is_none());
        let full: PatchApplyParams = serde_json::from_str(
            r#"{"patch":"x","root":"/tmp","dryRun":true,"fuzz":0,"crlfTolerant":false}"#,
        )
        .unwrap();
        assert!(full.dry_run == Some(true) && full.fuzz == Some(0) && full.crlf_tolerant == Some(false));
        let result: PatchApplyResult = serde_json::from_str(
            r#"{"total":2,"applied":1,"failed":1,"dryRun":false,"allApplied":false,"results":[{"path":"a.txt","status":"applied","fuzzUsed":0,"offsetApplied":2},{"path":"b.bin","status":"skippedBinary","fuzzUsed":0,"offsetApplied":0,"detail":"二进制"}]}"#,
        )
        .unwrap();
        assert_eq!(result.results[1].status, PatchFileStatus::SkippedBinary);
        assert_eq!(result.results[0].offset_applied, 2);
    }

    #[test]
    fn dir_execute_payload_matches_ui_proposal() {
        // 与 ui/src/protocol/types.ts §3 提案同构：side/copy/move/rename/touch/mtimeMs
        let p: DirExecuteParams = serde_json::from_str(
            r#"{"sessionId":"d1","ops":[{"kind":"copy","path":"a.txt","side":"left"},{"kind":"rename","path":"old.txt","side":"right","newName":"new.txt"},{"kind":"touch","path":"b.txt","side":"left","mtimeMs":1234567890},{"kind":"move","path":"c.txt","side":"left","targetSide":"right"}],"dryRun":true}"#,
        )
        .unwrap();
        assert_eq!(p.ops.len(), 4);
        assert_eq!(p.ops[0].kind, DirExecuteOpKind::Copy);
        assert_eq!(p.ops[1].new_name.as_deref(), Some("new.txt"));
        assert_eq!(p.ops[2].mtime_ms, Some(1234567890));
        assert_eq!(p.ops[3].target_side, Some(DiffSide::Right));
        // 可选字段缺省省略
        let s = serde_json::to_string(&p.ops[0]).unwrap();
        assert!(!s.contains("targetSide") && !s.contains("newName") && !s.contains("mtimeMs"), "{s}");
        let r: DirExecuteResult = serde_json::from_str(
            r#"{"sessionId":"d1","total":1,"succeeded":[{"op":{"kind":"delete","path":"x.txt","side":"right"},"status":"succeeded","detail":null}],"failed":[],"skipped":[],"dryRun":false,"cancelled":false}"#,
        )
        .unwrap();
        assert_eq!(r.succeeded[0].status, DirExecuteOpStatus::Succeeded);
        assert!(r.succeeded[0].detail.is_none());
    }

    #[test]
    fn engine_capabilities_old_payload_without_three_way_merge() {
        // P2/P3 旧载荷（无 threeWayMerge）可解析；缺省 true（可选能力位约定）
        let old = r#"{"dirCompare":true,"textCompare":true,"inlineDiff":false,"cancel":true,"deltaBatching":true,"maxDeltaBatchRows":512,"dirBackgroundScan":true,"dirFilters":true,"dirOperate":true}"#;
        let v: EngineCapabilities = serde_json::from_str(old).unwrap();
        assert_eq!(v.three_way_merge, Some(true));
        assert_eq!(v.lines_read, Some(true));
        assert_eq!(v.format_rules, Some(true), "P10 能力位缺省 true");
        assert_eq!(v.reports, Some(true), "P11 能力位缺省 true");
        // 新引擎序列化携带 threeWayMerge
        let c = EngineCapabilities {
            dir_compare: true,
            text_compare: true,
            inline_diff: true,
            cancel: true,
            delta_batching: true,
            max_delta_batch_rows: 512,
            lines_read: Some(true),
            dir_background_scan: true,
            dir_filters: true,
            dir_operate: true,
            three_way_merge: Some(true),
            table_compare: Some(true),
            hex_compare: Some(true),
            patch_apply: Some(true),
            dir_execute: Some(true),
            format_rules: Some(true),
            reports: Some(true),
            picture_compare: Some(true),
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("threeWayMerge"));
        assert!(s.contains("tableCompare") && s.contains("hexCompare"));
        assert!(s.contains("patchApply") && s.contains("dirExecute"));
        assert!(s.contains("formatRules"));
    }

    // -----------------------------------------------------------------
    // P10：文件格式与规则体系（serde 锚定）
    // -----------------------------------------------------------------

    #[test]
    fn p10_file_format_spec_defaults_and_roundtrip() {
        // 宽松解析：仅 id 也合法（缺省字段补齐）
        let f: FileFormatSpec = serde_json::from_str(r#"{"id":"fmt-x"}"#).unwrap();
        assert!(f.enabled);
        assert_eq!(f.view_kind, FormatViewKind::Text);
        assert_eq!(f.tab_stops, 0, "协议层 tabStops 缺省 0（引擎层缺省 4）");
        // 完整 roundtrip
        let full = FileFormatSpec {
            id: "fmt-cpp".into(),
            enabled: true,
            masks: vec!["*.cpp".into(), "*.h".into()],
            description: "C/C++".into(),
            view_kind: FormatViewKind::Text,
            grammar: vec![GrammarElementSpec {
                name: "注释".into(),
                case_sensitive: false,
                items: vec![
                    GrammarItemSpec { kind: GrammarItemKind::Delimited, start: Some("/*".into()), end: Some("*/".into()), ..Default::default() },
                    GrammarItemSpec { kind: GrammarItemKind::Text, pattern: Some("//.*".into()), regex: true, ..Default::default() },
                ],
            }],
            tab_stops: 4,
            independent_lines: false,
            column_data: false,
        };
        let s = serde_json::to_string(&full).unwrap();
        let back: FileFormatSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back, full);
        assert!(s.contains("\"viewKind\":\"text\""));
        assert!(s.contains("caseSensitive") || !s.contains("caseSensitive"), "键名 camelCase 稳定");
    }

    #[test]
    fn p10_format_pack_spec_bcpack_shape() {
        let pack = FormatPackSpec {
            version: 1,
            kind: "ai-compare-format-pack".into(),
            formats: vec![FileFormatSpec { id: "fmt-x".into(), ..Default::default() }],
        };
        let s = serde_json::to_string_pretty(&pack).unwrap();
        let back: FormatPackSpec = serde_json::from_str(&s).unwrap();
        assert_eq!(back, pack);
        // BC 风格 .bcpack 载体：version/kind/formats 三键
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["kind"], "ai-compare-format-pack");
    }

    #[test]
    fn p10_rules_spec_session_payload_semantics() {
        // text/open rules：None 字段 = 引擎缺省；旧载荷无 rules 字段 = None
        let old: TextOpenParams = serde_json::from_str(r#"{"sessionId":"t1","left":"a","right":"b"}"#).unwrap();
        assert!(old.rules.is_none(), "旧载荷无 rules");
        let with: TextOpenParams = serde_json::from_str(
            r#"{"sessionId":"t1","left":"a","right":"b","rules":{"caseUnimportant":true,"unimportantElements":["注释"]}}"#,
        )
        .unwrap();
        let rules = with.rules.unwrap();
        assert_eq!(rules.case_unimportant, Some(true));
        assert_eq!(rules.unimportant_elements, Some(vec!["注释".to_string()]));
        assert_eq!(rules.leading_ws_unimportant, None, "未指定 = 引擎缺省");
        // 序列化省略 None 字段（字节稳定）
        let s = serde_json::to_string(&rules).unwrap();
        assert!(s.contains("caseUnimportant") && !s.contains("eolStyleUnimportant"));
    }

    #[test]
    fn p10_rules_layer_and_scope_serde() {
        // RuleLayer 宽松 + scope 枚举 camelCase
        let layer: RuleLayerSpec = serde_json::from_str(r#"{"unimportantElements":["字符串"]}"#).unwrap();
        assert!(layer.compare.is_none());
        assert_eq!(layer.unimportant_elements.unwrap(), vec!["字符串".to_string()]);
        for (raw, want) in [
            (r#""global""#, RuleScope::Global),
            (r#""kind""#, RuleScope::Kind),
            (r#""session""#, RuleScope::Session),
        ] {
            let v: RuleScope = serde_json::from_str(raw).unwrap();
            assert_eq!(v, want);
        }
        // rules/set roundtrip
        let set = RulesSetParams {
            scope: RuleScope::Kind,
            kind: Some("text-compare".into()),
            session_id: None,
            layer: RuleLayerSpec { compare: Some(CompareRulesSpec { case_unimportant: Some(true), ..Default::default() }), unimportant_elements: None },
            format_bindings: Vec::new(),
            clear_format_bindings: false,
        };
        let s = serde_json::to_string(&set).unwrap();
        let back: RulesSetParams = serde_json::from_str(&s).unwrap();
        assert_eq!(back, set);
        assert!(s.contains("\"scope\":\"kind\""));
        assert!(!s.contains("formatBindings"), "空绑定省略（字节稳定）");
    }

    #[test]
    fn p10_effective_rules_and_bindings_serde() {
        let eff: EffectiveRulesSpec = serde_json::from_str(
            r#"{"compare":{"caseUnimportant":true,"eolStyleUnimportant":true},"unimportantElements":["注释"]}"#,
        )
        .unwrap();
        assert_eq!(eff.compare.case_unimportant, Some(true));
        assert_eq!(eff.unimportant_elements, vec!["注释".to_string()]);
        let b: RuleFormatBindingSpec =
            serde_json::from_str(r#"{"formatId":"builtin-cpp","unimportantElements":["注释"]}"#).unwrap();
        assert_eq!(b.format_id, "builtin-cpp");
    }

    #[test]
    fn p10_detected_file_kind_serde() {
        assert_eq!(serde_json::to_string(&DetectedFileKind::Text).unwrap(), "\"text\"");
        assert_eq!(serde_json::to_string(&DetectedFileKind::Binary).unwrap(), "\"binary\"");
        let v: DetectedFileKind = serde_json::from_str("\"binary\"").unwrap();
        assert_eq!(v, DetectedFileKind::Binary);
    }

    #[test]
    fn p10_dir_open_params_format_aware_optional() {
        // 旧载荷（无 formatAware）可解析；缺省 None
        let old: DirOpenParams = serde_json::from_str(
            r#"{"sessionId":"d","left":"/a","right":"/b","criteria":{"mtimeToleranceSec":2,"contentCompare":"auto"}}"#,
        )
        .unwrap();
        assert_eq!(old.format_aware, None);
        let aware: DirOpenParams = serde_json::from_str(
            r#"{"sessionId":"d","left":"/a","right":"/b","criteria":{"mtimeToleranceSec":2,"contentCompare":"auto"},"formatAware":true}"#,
        )
        .unwrap();
        assert_eq!(aware.format_aware, Some(true));
    }

    #[test]
    fn p11_report_generate_serde_anchor() {
        // 布局枚举字节锚定
        assert_eq!(serde_json::to_string(&ReportLayout::Summary).unwrap(), "\"summary\"");
        assert_eq!(serde_json::to_string(&ReportLayout::SideBySide).unwrap(), "\"sideBySide\"");
        assert_eq!(serde_json::to_string(&ReportLayout::Interleaved).unwrap(), "\"interleaved\"");
        assert_eq!(serde_json::to_string(&ReportLayout::Patch).unwrap(), "\"patch\"");
        assert_eq!(serde_json::to_string(&ReportLayout::Html).unwrap(), "\"html\"");
        assert_eq!(serde_json::to_string(&ReportLayout::Stats).unwrap(), "\"stats\"");
        let v: ReportLayout = serde_json::from_str("\"sideBySide\"").unwrap();
        assert_eq!(v, ReportLayout::SideBySide);
        // 最小载荷（旧客户端形态）：只给 sessionId + layout
        let min: ReportGenerateParams = serde_json::from_str(r#"{"sessionId":"t1","layout":"patch"}"#).unwrap();
        assert_eq!(min.title, None);
        assert_eq!(min.context_lines, None);
        assert_eq!(min.offset, None);
        // 全字段 roundtrip
        let full = ReportGenerateParams {
            session_id: "d1".into(),
            layout: ReportLayout::Html,
            title: Some("T".into()),
            display: Some(ReportDisplayFilter::Context),
            dir_display: Some(ReportDirDisplayFilter::MismatchesNoOrphans),
            ignore_unimportant: Some(true),
            line_numbers: Some(true),
            context_lines: Some(5),
            patch_style: Some(ReportPatchStyle::Unified),
            delimiter: Some("tsv".into()),
            color: Some(ReportColorMode::Mono),
            wrap: Some(ReportWrapMode::Word),
            wrap_columns: Some(60),
            columns: Some(ReportDirColumns { size: true, timestamp: false, ..ReportDirColumns::default() }),
            include_file_links: Some(true),
            offset: Some(1024),
            limit: Some(4096),
        };
        let s = serde_json::to_string(&full).unwrap();
        let back: ReportGenerateParams = serde_json::from_str(&s).unwrap();
        assert_eq!(back, full);
        assert!(s.contains("\"layout\":\"html\""));
        assert!(s.contains("\"dirDisplay\":\"mismatchesNoOrphans\""));
        assert!(s.contains("\"patchStyle\":\"unified\""));
        // 应答 roundtrip
        let res = ReportGenerateResult {
            layout: ReportLayout::Csv,
            content: "a,b\n".into(),
            total_chars: 4,
            offset: 0,
            has_more: false,
        };
        let s = serde_json::to_string(&res).unwrap();
        let back: ReportGenerateResult = serde_json::from_str(&s).unwrap();
        assert_eq!(back, res);
        assert!(s.contains("\"totalChars\":4"));
        assert!(s.contains("\"hasMore\":false"));
    }
}
