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
    /// 后台扫描进度通知（引擎 -> UI，P2）
    pub const DIR_PROGRESS: &str = "dir/progress";
    /// 打开文本比较会话
    pub const TEXT_OPEN: &str = "text/open";
    /// 窗口化取 diff section
    pub const TEXT_SECTIONS: &str = "text/sections";
    /// text/sections 的流式推送变体（引擎 -> UI 通知，P0 定义类型、暂不推送）
    pub const TEXT_SECTIONS_NOTIFY: &str = "text/sections";
    /// 关闭会话（释放引擎侧模型）
    pub const SESSION_CLOSE: &str = "session/close";
    /// 取消请求（UI -> 引擎通知，LSP 风格）
    pub const CANCEL: &str = "$/cancel";
}

/// TS 生成入口：`cargo test -p ai-protocol` 触发（见 ts_export 模块内的测试）。
#[cfg(feature = "ts")]
pub use ts_export::export_all_to_ui;
