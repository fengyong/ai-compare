//! JSON 安全整数约束（R3-21 数字契约）
//!
//! 协议把 Rust `u64`/`i64` 字段定为**线上 JSON number**（见 `types.rs` 各字段
//! 的 `#[ts(type = "number")]`）。JS 只有 IEEE-754 双精度，`Number` 能精确
//! 表示的整数上限是 \(2^{53}-1\)。超过该值的整数若仍以 JSON number 传输，
//! `JSON.parse` 会**静默截断精度**且不会报错 —— 这是最危险的失败模式。
//!
//! 因此协议约定：
//! - 引擎侧所有 `u64`/`i64` 字段的实际量级必须 < `MAX_SAFE_INTEGER`
//!   （rev/批次计数、epoch 毫秒、文件字节数、像素数、缓冲区偏移等均满足；
//!   实测上限远低于 2^53）；
//! - **入站**请求里的整数在此统一校验，越界回 `-32602 Invalid params`；
//! - 未来若确需传输 > 2^53 的值，必须改用字符串等无损表示并递增协议主版本，
//!   **不得**静默 `Number()` 截断。

use serde_json::Value;

/// JS `Number.MAX_SAFE_INTEGER` = \(2^{53}-1\) = 9_007_199_254_740_991
pub const MAX_SAFE_INTEGER: u64 = (1u64 << 53) - 1;

/// 该无符号整数能否由 JS `number` 无损表示
pub fn is_safe_integer(v: u64) -> bool {
    v <= MAX_SAFE_INTEGER
}

/// 该有符号整数能否由 JS `number` 无损表示
pub fn is_safe_signed(v: i64) -> bool {
    v.unsigned_abs() <= MAX_SAFE_INTEGER
}

/// 递归查找首个超出 JS 安全整数范围的 number，返回其 JSON 路径（便于报错定位）。
///
/// 覆盖三种 serde_json 数值形态：
/// - `u64` / `i64`：直接按安全范围判定；
/// - `f64`：JSON 里写成 `9007199254740992.0` 这类"看起来是浮点"的值，
///   若其整数部分超过上限同样会丢精度，一并拒绝；普通小数（如相似度
///   百分比）不受影响。
pub fn find_unsafe_integer(value: &Value) -> Option<String> {
    fn walk(v: &Value, path: &str, out: &mut Option<String>) {
        if out.is_some() {
            return;
        }
        match v {
            Value::Number(n) => {
                let unsafe_num = if let Some(u) = n.as_u64() {
                    !is_safe_integer(u)
                } else if let Some(i) = n.as_i64() {
                    !is_safe_signed(i)
                } else if let Some(f) = n.as_f64() {
                    f.is_finite() && f.fract() == 0.0 && f.abs() > MAX_SAFE_INTEGER as f64
                } else {
                    false
                };
                if unsafe_num {
                    *out = Some(path.to_string());
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    walk(item, &format!("{path}[{i}]"), out);
                    if out.is_some() {
                        return;
                    }
                }
            }
            Value::Object(map) => {
                for (k, item) in map {
                    walk(item, &format!("{path}.{k}"), out);
                    if out.is_some() {
                        return;
                    }
                }
            }
            _ => {}
        }
    }
    let mut found = None;
    walk(value, "$", &mut found);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn boundary_values() {
        assert_eq!(MAX_SAFE_INTEGER, 9_007_199_254_740_991);
        assert!(is_safe_integer(MAX_SAFE_INTEGER));
        assert!(!is_safe_integer(MAX_SAFE_INTEGER + 1));
        assert!(is_safe_signed(-(MAX_SAFE_INTEGER as i64)));
        assert!(!is_safe_signed(-(MAX_SAFE_INTEGER as i64) - 1));
    }

    #[test]
    fn detects_unsafe_integers_at_any_depth() {
        assert!(find_unsafe_integer(&json!({"offset": MAX_SAFE_INTEGER})).is_none());
        assert_eq!(
            find_unsafe_integer(&json!({"params": {"ops": [{"offset": 9007199254740992u64}]}})),
            Some("$.params.ops[0].offset".to_string())
        );
        assert_eq!(
            find_unsafe_integer(&json!(9007199254740992u64)),
            Some("$".to_string())
        );
        // 普通小数（如相似度百分比）不受影响
        assert!(find_unsafe_integer(&json!({"similarityPct": 91.19})).is_none());
        // 大整数写成浮点形态同样要拒绝
        assert!(find_unsafe_integer(&json!(9007199254740992.0)).is_some());
    }
}
