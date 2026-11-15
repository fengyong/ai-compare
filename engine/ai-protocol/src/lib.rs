//! # ai-protocol（M23）：协议契约层
//!
//! 引擎（`ai-engine` 进程）与 UI / 测试驱动之间唯一的耦合面。对齐 docs/03 §2.3：
//!
//! - 传输帧：NDJSON（每行一条 JSON-RPC 2.0 消息，UTF-8，`\n` 分隔）；
//! - 方法命名空间：`app/`、`dir/`、`text/`、`session/` 等（§2.3.3）；
//! - 错误：JSON-RPC 标准错误码 + 领域错误码（§附录 B）；
//! - 取消：`$/cancel` 通知（LSP 风格）。
//!
//! 本 crate 是最底层契约：不依赖任何业务 crate；所有参数/结果类型可 serde
//! 序列化，并通过 `ts-rs` 导出 TypeScript 定义到 `ui/src/protocol/generated/`
//! （运行 `cargo test -p ai-protocol` 即触发生成）。
//!
//! 协议演进规则：**只加不改不删** + `initialize` 版本协商（semver 协议版本）。

pub mod error;
pub mod safe_int;
#[cfg(feature = "ts")]
pub mod ts_export;
pub mod types;

pub use error::{error_object, error_object_with_data, RpcErrorObject};
pub use types::*;

/// 协议版本（`initialize` 握手协商用；破坏性变更递增主版本）
pub const PROTOCOL_VERSION: &str = "1.0";

/// JSON-RPC 版本常量
pub const JSONRPC_VERSION: &str = "2.0";

// ---------------------------------------------------------------------------
// 方法名常量（§2.3.3 命名空间；只增不改）
// ---------------------------------------------------------------------------

pub mod method {
    /// 握手：能力协商 + 协议版本
    pub const INITIALIZE: &str = "initialize";
    /// 退出前关闭（LSP 语义：收到 shutdown 后引擎退出）
    pub const SHUTDOWN: &str = "shutdown";
    /// 打开目录比较会话
    pub const DIR_OPEN: &str = "dir/open";
    /// 目录增量事件推送（引擎 -> UI 通知）
    pub const DIR_DELTA: &str = "dir/delta";
    /// 展开目录子树（P2，ADR-004 前台优先）
    pub const DIR_EXPAND: &str = "dir/expand";
    /// 设置/更新三类过滤器（P2）
    pub const DIR_FILTER: &str = "dir/filter";
    /// 手动对齐 / 解除对齐（P2）
    pub const DIR_ALIGN: &str = "dir/align";
    /// 生成同步操作计划（P2 数据层，不执行）
    pub const DIR_OPERATE: &str = "dir/operate";
    /// 手动内容比较（R3-16，FR-DIR-012：操作 > 比较内容，CRC/二进制/基于规则）
    pub const DIR_COMPARE_CONTENT: &str = "dir/compareContent";
    /// 后台扫描进度通知（引擎 -> UI，P2）
    pub const DIR_PROGRESS: &str = "dir/progress";
    /// 打开文本比较会话
    pub const TEXT_OPEN: &str = "text/open";
    /// 窗口化取 diff section
    pub const TEXT_SECTIONS: &str = "text/sections";
    /// text/sections 的流式推送变体（引擎 -> UI 通知，P0 定义类型、暂不推送）
    pub const TEXT_SECTIONS_NOTIFY: &str = "text/sections";
    /// 行内容按需分页（P3，docs/03 §2.3.2 大内容约定）
    pub const TEXT_LINES: &str = "text/lines";
    /// 行内差异懒计算（P3，docs/03 §2.3.2 text/inlineDiff {sectionId}）
    pub const TEXT_INLINE_DIFF: &str = "text/inlineDiff";
    /// 编辑增量重 diff（R3-18，FR-TXT-020：引擎权威 sections + 重要性）
    pub const TEXT_EDIT: &str = "text/edit";
    /// 编辑内容落盘（R3-04，FR-TXT-020：保存 / 另存为 / 自动备份）
    pub const TEXT_SAVE: &str = "text/save";
    /// 三向合并：打开会话（P7，docs/03 §4.5 MergeModel）
    pub const TEXT_MERGE3_OPEN: &str = "text/merge3/open";
    /// 三向合并：块清单分批取回（P7）
    pub const TEXT_MERGE3_SECTIONS: &str = "text/merge3/sections";
    /// 三向合并：块决策提交（幂等，P7）
    pub const TEXT_MERGE3_DECIDE: &str = "text/merge3/decide";
    /// 三向合并：合成输出（含未决冲突清单，P7）
    pub const TEXT_MERGE3_OUTPUT: &str = "text/merge3/output";
    /// 三向合并：输出落盘到"合并到"目标（R3-20，FR-TMRG-006）
    pub const TEXT_MERGE3_SAVE: &str = "text/merge3/save";
    /// 表格比较：打开会话（P9，FR-TBL 引擎侧）
    pub const TABLE_OPEN: &str = "table/open";
    /// 表格比较：diff 行清单分批取回（P9）
    pub const TABLE_SECTIONS: &str = "table/sections";
    /// 十六进制比较：打开会话（P9，FR-HEX 引擎侧）
    pub const HEX_OPEN: &str = "hex/open";
    pub const PICTURE_OPEN: &str = "picture/open";
    /// 十六进制比较：十六进制行分页取数（P9，docs/03 §2.3.2 大内容约定）
    pub const HEX_BYTES: &str = "hex/bytes";
    /// 补丁应用（P9，FR-PATCH-011：输入补丁文本 + 目标规格 -> 结果报告）
    pub const TEXT_PATCH_APPLY: &str = "text/patch/apply";
    /// 文件操作执行（P6 提案合入：dryRun + 执行 + rescan delta）
    pub const DIR_EXECUTE: &str = "dir/execute";
    /// 文件夹三向合并：扫描三树 + diff3 分类（R3-07，FR-DMRG-002）
    pub const DIR_MERGE_SCAN: &str = "dir/mergeScan";
    /// 文件夹三向合并：决策 -> 计划预览（R3-07，FR-DMRG-003）
    pub const DIR_MERGE_PLAN: &str = "dir/mergePlan";
    /// 文件夹三向合并：执行计划 -> 部分成功报告（R3-07，FR-DMRG-004）
    pub const DIR_MERGE_EXECUTE: &str = "dir/mergeExecute";
    /// 文件格式：全量列表（P10，内置 + 会话期自定义，按生效顺序）
    pub const FORMATS_LIST: &str = "formats/list";
    /// 文件格式：按 id 覆盖/追加（P10；引擎会话期态，持久化由 UI 负责）
    pub const FORMATS_SAVE: &str = "formats/save";
    /// 报告生成（P11，FR-RPT：会话快照 + 布局参数 -> 报告字符串或分块流）
    pub const REPORT_GENERATE: &str = "report/generate";
    /// 文件格式：从文件加载格式集（.bcpack / JSON，P10）
    pub const FORMATS_LOAD: &str = "formats/load";
    /// 规则：读取三层 + 生效解析（P10，FR-SES-006 作用域）
    pub const RULES_GET: &str = "rules/get";
    /// 规则：写入一层（P10，global/kind/session 三目标）
    pub const RULES_SET: &str = "rules/set";
    /// 关闭会话（释放引擎侧模型）
    pub const SESSION_CLOSE: &str = "session/close";
    /// 取消请求（UI -> 引擎通知，LSP 风格）
    pub const CANCEL: &str = "$/cancel";
}

/// TS 再生成入口：`cargo test -p ai-protocol --features ts -- --ignored regenerate_ui_bindings`。
#[cfg(feature = "ts")]
pub use ts_export::export_all_to_ui;
