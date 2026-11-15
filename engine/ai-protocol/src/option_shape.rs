//! R3-21：Option 字段的可选性契约（serde 运行时形状 ⇔ ts-rs 生成的 TS 形状）。
//!
//! 协议里 `Option<T>` 有两种合法形态，必须区分：
//!
//! | 形态 | serde 行为 | TS 形状 | 语义 |
//! | --- | --- | --- | --- |
//! | 运行时可省略 | `skip_serializing_if = "Option::is_none"` | `field?: T` | 键可缺省；缺省 = None |
//! | 恒定输出 | 无 skip | `field: T \| null` | 键总在；值可能是 null |
//!
//! 两者错配会造成「TS 说谎」：
//! - 可省略字段没标 `ts(optional)` → TS 声称字段必存在，线上却会缺键
//!   （`SideInfo.unixMode/uid/gid` 曾是这种形状：TS 写必填 `number`，线上可缺键）；
//! - 恒定输出字段标了 `ts(optional)` → TS 声称可省略，线上永远有键。
//!
//! 本模块提供两道锁：
//! 1. `option_optionality_matches_serde_skip`：源码级静态检查，逐字段核对
//!    `skip_serializing_if` ⇔ `ts(optional)`（含 error.rs 的 `RpcErrorObject`）。
//! 2. `three_state_option_protocol`：运行时三态（省略键 / 显式 null / 有值）语义。
//!
//! 明确不在本专项范围：`Option<T>` 之外的零值省略字段（bool/Vec/String 等的
//! `skip_serializing_if = "is_zero"` 一类）。ts-rs 的 `ts(optional)` 仅支持
//! `Option<T>`，这类字段另走 `ts(type = "T | undefined")` 口径。

use crate::types::*;

// ---------------------------------------------------------------------------
// 静态检查：skip_serializing_if ⇔ ts(optional)
// ---------------------------------------------------------------------------

/// 一个待核对的字段
struct FieldShape {
    /// 结构体名
    owner: String,
    /// 字段名（snake_case）
    name: String,
    /// 是否带 serde(skip_serializing_if)
    skip: bool,
    /// 是否带 ts(optional)
    optional: bool,
}

/// 解析一个 Rust 源文件，抽出「导出到 TS 的结构体」里的 Option 字段形状。
fn parse_option_fields(src: &str) -> Vec<FieldShape> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    // 当前结构体上下文
    let mut struct_name = String::new();
    let mut struct_is_ts = false;
    // 属性缓冲（紧邻字段上方的注释/属性行）
    let mut attr: Vec<String> = Vec::new();

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        // 结构体开始：向上收集属性块
        if let Some(name) = trimmed
            .strip_prefix("pub struct ")
            .and_then(|r| r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next())
        {
            struct_name = name.to_string();
            let mut attrs: Vec<&str> = Vec::new();
            // 从当前行往上找连续的属性/文档注释块
            let mut k = i;
            while k > 0 {
                k -= 1;
                let prev = lines[k].trim();
                if prev.starts_with("#[") || prev.starts_with("///") {
                    attrs.push(prev);
                } else {
                    break;
                }
            }
            struct_is_ts = attrs.iter().any(|a| a.contains("derive(ts_rs::TS)"));
            attr.clear();
            i += 1;
            continue;
        }

        if trimmed == "}" {
            struct_name.clear();
            struct_is_ts = false;
            attr.clear();
            i += 1;
            continue;
        }

        // 属性 / 文档注释
        if trimmed.starts_with("#[") || trimmed.starts_with("///") {
            attr.push(trimmed.to_string());
            i += 1;
            continue;
        }

        // 字段行
        if let Some(rest) = trimmed.strip_prefix("pub ") {
            if let Some((name, ty)) = rest.split_once(':') {
                let name = name.trim();
                let ty = ty.trim();
                if struct_is_ts && ty.starts_with("Option<") {
                    let skip = attr.iter().any(|a| a.contains("skip_serializing_if"));
                    let optional = attr.iter().any(|a| a.contains("ts(optional"));
                    out.push(FieldShape {
                        owner: struct_name.clone(),
                        name: name.to_string(),
                        skip,
                        optional,
                    });
                }
            }
        }

        // 非属性行清空缓冲
        if !trimmed.is_empty() {
            attr.clear();
        }
        i += 1;
    }
    out
}

/// R3-21：运行时可省略的 Option 字段必须标 `ts(optional)`；恒定输出的 Option
/// 字段必须**不**标 `ts(optional)`。
///
/// 这条检查锁住「TS 生成的形状与真实 NDJSON 一致」，防止再出现
/// `SideInfo.unixMode` 那种「TS 必填、线上可缺键」的撒谎形状。
#[test]
fn option_optionality_matches_serde_skip() {
    let mut checked = 0usize;
    for src in [include_str!("types.rs"), include_str!("error.rs")] {
        for f in parse_option_fields(src) {
            checked += 1;
            if f.skip && !f.optional {
                panic!(
                    "{}.{}: serde 运行时可省略（skip_serializing_if），但 TS 未标 ts(optional) \
                     —— 生成的 TS 会声称该字段必存在而线上会缺键",
                    f.owner, f.name
                );
            }
            if !f.skip && f.optional {
                panic!(
                    "{}.{}: TS 标了 ts(optional)，但 serde 恒定输出该键（无 skip_serializing_if）\
                     —— 生成的 TS 会声称该字段可省略而线上永远有键",
                    f.owner, f.name
                );
            }
        }
    }
    assert!(
        checked > 150,
        "解析到的 Option 字段过少（{checked}），静态检查可能失效"
    );
}

// ---------------------------------------------------------------------------
// 运行时三态：省略键 / 显式 null / 有值
// ---------------------------------------------------------------------------

/// R3-21：三态协议语义。
///
/// - **省略键**：解析为 None；再序列化时键不出现（线上不产生 null 噪声）。
/// - **显式 null**：解析为 None（旧 UI 仍会显式发 null，向后兼容必须接受）。
/// - **有值**：解析为 Some，且序列化保留该键。
///
/// 覆盖代表性请求（dir/open、text/open、merge3/open、dir/filter）、
/// 响应（dir/row）与通知（dir/delta）。
#[test]
fn three_state_option_protocol() {
    // ---- 请求：dir/open ----
    let base = r#""sessionId":"d","left":"/a","right":"/b","criteria":{"mtimeToleranceSec":2,"contentCompare":"auto"}"#;
    let omitted: DirOpenParams = serde_json::from_str(&format!("{{{base}}}")).unwrap();
    // DirOpenParams 带结构体级 #[serde(default)]：省略键取 Default::default()。
    // depth 的默认值是 Some(1)（P0 默认顶层），因此「省略」≠「None」——
    // 这一点必须显式钉住，否则调用方会把省略误解成「不限制深度」。
    assert_eq!(omitted.depth, Some(1), "省略 depth = 结构体默认值 Some(1)");
    assert!(omitted.lazy_scan.is_none(), "省略 lazyScan");
    assert!(omitted.flat.is_none(), "省略 flat");
    assert!(omitted.manual_align.is_none(), "省略 manualAlign");
    assert!(omitted.name_filter.is_none(), "省略 nameFilter");

    let nulled: DirOpenParams = serde_json::from_str(&format!(
        r#"{{{base},"depth":null,"lazyScan":null,"flat":null,"manualAlign":null,"nameFilter":null}}"#
    ))
    .unwrap();
    assert!(nulled.depth.is_none(), "显式 null 的 depth");
    assert!(nulled.lazy_scan.is_none(), "显式 null 的 lazyScan");
    assert!(nulled.flat.is_none(), "显式 null 的 flat");
    assert!(nulled.manual_align.is_none(), "显式 null 的 manualAlign");
    assert!(nulled.name_filter.is_none(), "显式 null 的 nameFilter");

    let valued: DirOpenParams = serde_json::from_str(&format!(
        r#"{{{base},"depth":2,"lazyScan":true,"flat":true,"manualAlign":[{{"left":"a.txt","right":"b.txt"}}],"nameFilter":{{"includeFiles":["*.rs"]}}}}"#
    ))
    .unwrap();
    assert_eq!(valued.depth, Some(2));
    assert_eq!(valued.lazy_scan, Some(true));
    assert_eq!(valued.flat, Some(true));
    assert_eq!(valued.manual_align.as_ref().map(|v| v.len()), Some(1));
    assert!(valued.name_filter.is_some());

    // 省略键的再序列化不得产生 null 噪声（None 的字段直接不出现）
    let s = serde_json::to_string(&omitted).unwrap();
    for k in ["lazyScan", "flat", "manualAlign", "nameFilter"] {
        assert!(!s.contains(k), "{k} 不应出现在序列化结果: {s}");
    }
    // depth 是 Some(1)：键保留且不是 null
    assert!(s.contains(r#""depth":1"#), "{s}");
    assert!(!s.contains(r#""depth":null"#), "{s}");

    // ---- 请求：text/open（options 可选） ----
    let t_omitted: TextOpenParams =
        serde_json::from_str(r#"{"sessionId":"t","left":"a","right":"b"}"#).unwrap();
    assert!(t_omitted.options.is_none());
    let t_nulled: TextOpenParams =
        serde_json::from_str(r#"{"sessionId":"t","left":"a","right":"b","options":null}"#).unwrap();
    assert!(t_nulled.options.is_none(), "显式 null 的 options");
    assert!(!serde_json::to_string(&t_omitted)
        .unwrap()
        .contains("options"));

    // ---- 请求：merge3/open（options 可选） ----
    let m_omitted: MergeOpenParams =
        serde_json::from_str(r#"{"sessionId":"m","base":"b","left":"l","right":"r"}"#).unwrap();
    assert!(m_omitted.options.is_none());
    let m_nulled: MergeOpenParams = serde_json::from_str(
        r#"{"sessionId":"m","base":"b","left":"l","right":"r","options":null}"#,
    )
    .unwrap();
    assert!(m_nulled.options.is_none(), "显式 null 的 options");

    // ---- 请求：dir/filter（nameFilter/suppress/flat 等可选） ----
    let f_omitted: DirFilterParams =
        serde_json::from_str(r#"{"sessionId":"d","rescan":true}"#).unwrap();
    assert!(f_omitted.name_filter.is_none() && f_omitted.suppress.is_none());
    let f_nulled: DirFilterParams = serde_json::from_str(
        r#"{"sessionId":"d","rescan":true,"nameFilter":null,"suppress":null}"#,
    )
    .unwrap();
    assert!(f_nulled.name_filter.is_none() && f_nulled.suppress.is_none());

    // ---- 响应：DirRow（左右侧可选） ----
    let r_omitted: DirRow = serde_json::from_str(
        r#"{"id":"a.txt","name":"a.txt","kind":"file","status":"rightOnly","hasChildren":false}"#,
    )
    .unwrap();
    assert!(r_omitted.left.is_none() && r_omitted.right.is_none());
    let r_nulled: DirRow = serde_json::from_str(
        r#"{"id":"a.txt","name":"a.txt","kind":"file","left":null,"right":null,"status":"same","hasChildren":false}"#,
    )
    .unwrap();
    assert!(
        r_nulled.left.is_none() && r_nulled.right.is_none(),
        "显式 null 的两侧"
    );
    let r_valued: DirRow = serde_json::from_str(
        r#"{"id":"a.txt","name":"a.txt","kind":"file","left":{"size":1,"mtimeMs":2},"right":{"size":1,"mtimeMs":2},"status":"same","hasChildren":false}"#,
    )
    .unwrap();
    assert!(r_valued.left.is_some() && r_valued.right.is_some());
    let s = serde_json::to_string(&r_omitted).unwrap();
    assert!(!s.contains("\"left\":") && !s.contains("\"right\":"), "{s}");

    // ---- 通知：DirDelta（progress 可选） ----
    let d_omitted: DirDelta = serde_json::from_str(
        r#"{"sessionId":"d","rev":1,"upserts":[],"removals":[],"stats":{"total":0,"same":0,"quickDiff":0,"contentDiff":0,"leftOnly":0,"rightOnly":0,"dirSame":0,"dirDiff":0,"similar":0},"batch":{"index":0,"hasMore":false},"scanErrors":[]}"#,
    )
    .unwrap();
    assert!(d_omitted.progress.is_none());
    let d_nulled: DirDelta = serde_json::from_str(
        r#"{"sessionId":"d","rev":1,"upserts":[],"removals":[],"stats":{"total":0,"same":0,"quickDiff":0,"contentDiff":0,"leftOnly":0,"rightOnly":0,"dirSame":0,"dirDiff":0,"similar":0},"batch":{"index":0,"hasMore":false},"scanErrors":[],"progress":null}"#,
    )
    .unwrap();
    assert!(d_nulled.progress.is_none(), "显式 null 的 progress");
    let d_valued: DirDelta = serde_json::from_str(
        r#"{"sessionId":"d","rev":1,"upserts":[],"removals":[],"stats":{"total":0,"same":0,"quickDiff":0,"contentDiff":0,"leftOnly":0,"rightOnly":0,"dirSame":0,"dirDiff":0,"similar":0},"batch":{"index":0,"hasMore":false},"scanErrors":[],"progress":{"scanned":1,"total":3}}"#,
    )
    .unwrap();
    assert!(d_valued.progress.is_some());
}
