//! 错误码约定（docs/03 §2.3.2 + 附录 B）
//!
//! - `-32700..-32603`：JSON-RPC 2.0 标准保留段；
//! - `-32800`：请求已取消（沿用 LSP 约定）；
//! - `409/410/...`：领域错误（rev 冲突、会话不存在等），仅出现在 RPC
//!   `error.code`，不改变 CLI 退出码语义（附录 B）；
//! - CLI 退出码表（FR-CLI-004 / P3）由 `aic` 实现层维护：0 成功、1 差异、
//!   2 错误（P0 简化表；11/12/13/14 等细分在 P3 补齐）。

use serde::{Deserialize, Serialize};

/// JSON-RPC 标准错误码
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;

    /// 请求已被 `$/cancel` 取消（LSP 约定值）
    pub const REQUEST_CANCELLED: i64 = -32800;

    /// 会话模型版本冲突（乐观并发失败，需 state/resync；docs/03 §2.3.2）
    pub const REV_CONFLICT: i64 = 409;
    /// 会话句柄不存在
    pub const SESSION_NOT_FOUND: i64 = 410;
    /// 会话 id 已被占用
    pub const SESSION_EXISTS: i64 = 411;
    /// 路径不存在 / 不可访问
    pub const PATH_NOT_FOUND: i64 = 412;
    /// 文件不是合法 UTF-8 文本（P0 仅支持 UTF-8/UTF-8-BOM）
    pub const INVALID_ENCODING: i64 = 413;
}

/// JSON-RPC error 对象
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct RpcErrorObject {
    /// JSON-RPC 错误码（线上为 JSON number；取值远小于 2^53）
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub code: i64,
    pub message: String,
    /// 结构化附加信息（本地化由 UI 完成：引擎只上报 (code, params)，docs/03 §6.7）
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "unknown"))]
    pub data: Option<serde_json::Value>,
}

/// 构造 error 对象的便捷函数
pub fn error_object(code: i64, message: impl Into<String>) -> RpcErrorObject {
    RpcErrorObject {
        code,
        message: message.into(),
        data: None,
    }
}

/// 带附加数据的 error 对象
pub fn error_object_with_data(
    code: i64,
    message: impl Into<String>,
    data: serde_json::Value,
) -> RpcErrorObject {
    RpcErrorObject {
        code,
        message: message.into(),
        data: Some(data),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_object_serializes_without_null_data() {
        let e = error_object(codes::METHOD_NOT_FOUND, "no such method");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], -32601);
        assert!(v.get("data").is_none(), "data 缺省时不应序列化为 null");
    }

    #[test]
    fn error_object_with_data_roundtrip() {
        let e = error_object_with_data(
            codes::SESSION_NOT_FOUND,
            "gone",
            serde_json::json!({"sessionId": "s1"}),
        );
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["data"]["sessionId"], "s1");
        let back: RpcErrorObject = serde_json::from_value(v).unwrap();
        assert_eq!(back, e);
    }
}
