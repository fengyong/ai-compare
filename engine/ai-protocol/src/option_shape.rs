//! R3-21 / D1A：协议字段的「线上形状 ⇔ 生成 TS 形状」契约。
//!
//! 一个字段在线上有三种合法形状，必须严格区分（把三者混为一谈就会产生
//! 「TS 说谎」——类型声称的东西与真实 NDJSON 不一致，调用方写出必然崩溃的代码）：
//!
//! | 形状 | serde 侧 | 线上 | 生成的 TS |
//! | --- | --- | --- | --- |
//! | **键可省略** | `#[serde(default, skip_serializing_if = "...")]` | 零值/空值时**键不出现** | `field?: T` |
//! | **键恒定、值可空** | `Option<T>` 无 skip | 键总在，可能是 `null` | `field: T \| null` |
//! | **键恒定、值非空** | 普通类型、无 skip | 键总在 | `field: T` |
//!
//! 第一类的省略条件两类：`Option::is_none`（None）与零值谓词
//! （`std::ops::Not::not` / `Vec::is_empty` / `String::is_empty` / `is_zero_u32` …）。
//!
//! ## 为什么升级 ts-rs 10 → 12（D1A 决策记录）
//!
//! ts-rs 10 的 `#[ts(optional)]` **只允许**用在 `Option<T>` 上
//! （`ts-rs-macros-10.1.0/src/types/named.rs` 里对非 Option 类型直接
//! `syn::Error`「`optional` can only be used on an Option<T> type」），因此非 Option
//! 的零值省略字段只能用 `#[ts(type = "T | undefined")]` 表达——那是
//! **「键必须存在、值可以是 undefined」**，不是真正的可选键（`{}` 不可赋值），
//! 而且 override 会丢掉被引用类型的 import 依赖（`Vec<X>` 会把 X 变成孤立引用）。
//!
//! ts-rs 12 改为**从 serde 属性直接推导**可选性：
//! `#[serde(default)] + skip_serializing(_if)` ⇒ `field?: T`
//! （`ts-rs-macros-12.0.1/src/optional.rs`：`let is_optional = attr.maybe_omitted && attr.has_default;`）。
//! 于是「可选键」不再需要逐字段注解，也就不会漏标——这正是 D1A 要的
//! 「单一事实源」：形状由 serde 属性唯一决定，生成器负责翻译。
//!
//! 代价与权衡：
//! - `#[serde(default)]` 是推导前提：只 skip 不 default 的字段在 ts-rs 12 里仍是
//!   必填键。这种组合本身是协议不对称（序列化可省略、反序列化却要求该键），
//!   本模块第 2 道锁直接禁止；
//! - ts-rs 12 把 `HashMap<K, V>` 渲染成 `{ [key in string]: V }`（v10 是
//!   `{ [key in string]?: V }`）。唯一受影响的是 `DirMergePlanParams.decisions`，
//!   其读取侧用 `??` 兜底，且索引签名缺键语义由 `noUncheckedIndexedAccess` 决定
//!   （本项目未开启），故无运行时差异；未采用已废弃的 `with_v11_hashmap` 兼容开关。
//!
//! ## 三道锁
//!
//! 1. `option_optionality_matches_serde_skip`：源码级——`Option<T>` 的
//!    `skip_serializing_if` ⇔ `ts(optional)`（双向）。
//! 2. `generated_ts_field_shapes_match_serde_attributes`：**生成物级**——把
//!    types.rs / error.rs 的每个字段与 `ui/src/protocol/generated/*.ts` 逐一对账：
//!    `skip` ⇔ 生成物必须有 `?`，非 skip ⇔ 必须没有 `?`；非 Option 的省略字段
//!    必须有 `serde(default)`。这条覆盖「零值省略」的全部字段（D1A 清单），
//!    而不只是 Option。
//! 3. 运行时：`three_state_option_protocol`（Option 三态）与
//!    `zero_value_omitted_fields_round_trip`（非 Option 零值/非零值/显式零值）。

use crate::types::*;

// ---------------------------------------------------------------------------
// 静态检查：解析「导出到 TS 的结构体」字段形状
// ---------------------------------------------------------------------------

/// 一个待核对的字段
struct FieldShape {
    /// 结构体名
    owner: String,
    /// 线上/TS 字段名（camelCase；字段级 `serde(rename)` 时取 rename 值）
    ts_name: String,
    /// Rust 字段名（snake_case，报错信息用）
    rust_name: String,
    /// 字段类型原文（如 `Option<String>`、`Vec<DirRow>`）
    ty: String,
    /// 是否带 `serde(skip_serializing_if)`（键可省略）
    skip: bool,
    /// 是否带 `serde(default)`（缺键可反序列化）
    default: bool,
    /// 是否带 `ts(optional)`
    optional: bool,
}

/// 解析一个 Rust 源文件，抽出所有「导出到 TS 的结构体」的字段形状。
///
/// 手写解析（不引入 syn 依赖）：协议文件由 rustfmt 规范化，字段与属性单行呈现，
/// 结构体级 `#[serde(rename_all = "camelCase")]` 是全局约定（由
/// `ts_structs_use_camel_case` 钉住），因此文本扫描足够且无假阴性风险。
fn parse_field_shapes(src: &str) -> Vec<FieldShape> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut owner = String::new();
    let mut is_ts = false;
    let mut camel = false;
    let mut in_struct = false;
    // 紧邻当前行的属性/文档缓冲
    let mut attr: Vec<String> = Vec::new();

    for line in &lines {
        let t = line.trim();

        // 结构体开始：属性缓冲（结构体上方的 derive/serde）在上一轮已被收集，
        // 这里重新向上收集（与既有 R3-21 解析器同构，避免属性被提前清空）。
        if let Some(name) = t
            .strip_prefix("pub struct ")
            .and_then(|r| r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next())
        {
            owner = name.to_string();
            let mut up: Vec<&str> = Vec::new();
            // lines 是 Vec<&str>，用位置回溯更直观
            let idx = lines.iter().position(|l| std::ptr::eq(*l, *line));
            if let Some(i) = idx {
                let mut k = i;
                while k > 0 {
                    k -= 1;
                    let prev = lines[k].trim();
                    if prev.starts_with("#[") || prev.starts_with("///") {
                        up.push(prev);
                    } else {
                        break;
                    }
                }
            }
            is_ts = up.iter().any(|a| a.contains("derive(ts_rs::TS)"));
            camel = up.iter().any(|a| a.contains("rename_all = \"camelCase\""));
            in_struct = true;
            attr.clear();
            continue;
        }

        if t == "}" {
            if in_struct {
                in_struct = false;
                owner.clear();
                is_ts = false;
                camel = false;
            }
            attr.clear();
            continue;
        }

        if t.starts_with("#[") || t.starts_with("///") {
            attr.push(t.to_string());
            continue;
        }

        if in_struct && is_ts {
            if let Some(rest) = t.strip_prefix("pub ") {
                if let Some((name, ty)) = rest.split_once(':') {
                    let rust_name = name.trim().to_string();
                    let ty = ty.trim().trim_end_matches(',').trim().to_string();
                    let renamed = attr
                        .iter()
                        .find_map(|a| serde_rename(a))
                        .unwrap_or_else(|| {
                            if camel {
                                camel_case(&rust_name)
                            } else {
                                rust_name.clone()
                            }
                        });
                    out.push(FieldShape {
                        owner: owner.clone(),
                        ts_name: renamed,
                        rust_name,
                        ty,
                        skip: attr.iter().any(|a| a.contains("skip_serializing_if")),
                        default: attr.iter().any(|a| a.contains("serde(default")),
                        optional: attr.iter().any(|a| a.contains("ts(optional")),
                    });
                }
            }
        }

        if !t.is_empty() {
            attr.clear();
        }
    }
    out
}

/// `#[serde(rename = "x")]` → `x`
fn serde_rename(attr: &str) -> Option<String> {
    let (_, rest) = attr.split_once("rename = \"")?;
    let (name, _) = rest.split_once('"')?;
    // `rename_all` 也会命中 `rename = "`？不会：rename_all 的写法是 rename_all，
    // 上面的 split_once 精确要求 `rename = "` 前缀。
    Some(name.to_string())
}

/// snake_case → camelCase（协议结构体统一 `rename_all = "camelCase"`）
fn camel_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, part) in name.split('_').enumerate() {
        if i == 0 {
            out.push_str(part);
        } else {
            let mut chars = part.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    out
}

fn is_option(ty: &str) -> bool {
    ty.replace(' ', "").starts_with("Option<")
}

/// 去掉生成文件里的注释/导入行，避免文档注释里的字段名造成假阳性。
fn ts_body(text: &str) -> String {
    text.lines()
        .filter(|l| {
            let t = l.trim();
            !(t.starts_with("//") || t.starts_with("import ") || t.starts_with("/*"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn generated_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/protocol/generated")
}

// ---------------------------------------------------------------------------
// 锁 1：Option 字段 skip ⇔ ts(optional)
// ---------------------------------------------------------------------------

/// R3-21：运行时可省略的 Option 字段必须标 `ts(optional)`；恒定输出的 Option
/// 字段必须**不**标 `ts(optional)`。
///
/// 这条检查锁住「TS 生成的形状与真实 NDJSON 一致」，防止再出现
/// `SideInfo.unixMode` 那种「TS 必填、线上可缺键」的撒谎形状。
#[test]
fn option_optionality_matches_serde_skip() {
    let mut checked = 0usize;
    for src in [include_str!("types.rs"), include_str!("error.rs")] {
        for f in parse_field_shapes(src) {
            if !is_option(&f.ty) {
                continue;
            }
            checked += 1;
            if f.skip && !f.optional {
                panic!(
                    "{}.{}: serde 运行时可省略（skip_serializing_if），但 TS 未标 ts(optional) \
                     —— 生成的 TS 会声称该字段必存在而线上会缺键",
                    f.owner, f.rust_name
                );
            }
            if !f.skip && f.optional {
                panic!(
                    "{}.{}: TS 标了 ts(optional)，但 serde 恒定输出该键（无 skip_serializing_if）\
                     —— 生成的 TS 会声称该字段可省略而线上永远有键",
                    f.owner, f.rust_name
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
// 锁 2：生成物级对账（含非 Option 的零值省略字段）
// ---------------------------------------------------------------------------

/// D1A：把源码字段形状与**实际生成**的 TS 声明逐字段对账。
///
/// - 键可省略（`skip_serializing_if`）⇒ 生成物必须是 `field?: T`，
///   且非 `Option` 时必须有 `#[serde(default)]`（否则自身输出无法被自己反序列化）；
/// - 键恒定（无 skip）⇒ 生成物必须是 `field: T`（且不得出现 `?`）。
///
/// 这条锁覆盖 D1A 清单里的全部「零值省略」字段（bool / 整数 / String / Vec /
/// 枚举），不依赖逐字段人工注解——形状由 serde 属性唯一决定。
#[test]
fn generated_ts_field_shapes_match_serde_attributes() {
    let dir = generated_dir();
    let mut checked = 0usize;
    let mut omitted = 0usize;

    for src in [include_str!("types.rs"), include_str!("error.rs")] {
        for f in parse_field_shapes(src) {
            checked += 1;
            let path = dir.join(format!("{}.ts", f.owner));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
                panic!(
                    "缺少生成绑定 {}（先运行 `cargo test -p ai-protocol --features ts \
                     -- --ignored regenerate_ui_bindings`）：{e}",
                    path.display()
                )
            });
            let body = ts_body(&text);
            let has_optional = body.contains(&format!("{}?:", f.ts_name));
            let has_required = body.contains(&format!("{}:", f.ts_name));

            if f.skip {
                omitted += 1;
                if !has_optional {
                    panic!(
                        "D1A：{}.{}（TS 名 `{}`）线上会省略该键，但生成的 TS 不是可选键 —— \
                         应为 `{}?: ...`。检查 serde 是否同时带 default 与 skip_serializing_if",
                        f.owner, f.rust_name, f.ts_name, f.ts_name
                    );
                }
                if !is_option(&f.ty) && !f.default {
                    panic!(
                        "D1A：{}.{} 序列化时会省略该键，但没有 `serde(default)` —— \
                         反序列化仍要求该键，与自身输出不对称（ts-rs 也无法推导为可选键）",
                        f.owner, f.rust_name
                    );
                }
                if is_option(&f.ty) && !f.optional {
                    // 锁 1 已覆盖，这里再兜一次生成物级证据
                    panic!(
                        "D1A：{}.{} 是 Option 且可省略，生成的 TS 必须来自 ts(optional) \
                         （`{}?: T` 而非 `{}?: T | null`）",
                        f.owner, f.rust_name, f.ts_name, f.ts_name
                    );
                }
            } else {
                if !has_required {
                    panic!(
                        "D1A：{}.{}（TS 名 `{}`）线上必定出现该键，但生成的 TS 里找不到 `{}:`",
                        f.owner, f.rust_name, f.ts_name, f.ts_name
                    );
                }
                if has_optional {
                    panic!(
                        "D1A：{}.{} 线上必定出现该键（无 skip_serializing_if），\
                         但生成的 TS 是可选键 `{}?:`",
                        f.owner, f.rust_name, f.ts_name
                    );
                }
            }
        }
    }

    assert!(checked > 600, "对账字段过少（{checked}），解析器可能失效");
    assert!(
        omitted >= 60,
        "可省略字段过少（{omitted}），D1A 清单可能被误删"
    );
}

// ---------------------------------------------------------------------------
// 锁 3a：Option 三态（省略键 / 显式 null / 有值）
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

// ---------------------------------------------------------------------------
// 锁 3b：非 Option 零值省略字段（D1A）
// ---------------------------------------------------------------------------

/// D1A：零值省略字段的运行时语义（bool / 整数 / String / Vec / 嵌套 Struct）。
///
/// 三条必须同时成立：
/// 1. 零值序列化 ⇒ **键不出现**（旧载荷字节级不变，这是省略的全部意义）；
/// 2. 非零值序列化 ⇒ 键出现且值正确；
/// 3. 缺键反序列化 ⇒ 取 serde 默认值（0/false/空），显式零值同样合法
///    （其他实现/旧夹具可能显式发 `false`/`[]`，不能因此报错）。
///
/// 与 `null` 的区别是 D1A 的核心：这类字段**没有**「显式 null」形态，
/// 发送 null 会被拒绝——调用方不得把「可选键」误解为「nullable」。
#[test]
fn zero_value_omitted_fields_round_trip() {
    // ---- bool：响应 DirRow.hidden / suppressed ----
    let row = DirRow {
        id: "a.txt".into(),
        name: "a.txt".into(),
        kind: EntryKind::File,
        left: None,
        right: None,
        status: DirEntryStatus::Same,
        has_children: false,
        hidden: false,
        suppressed: false,
        left_path: None,
        right_path: None,
    };
    let json = serde_json::to_value(&row).unwrap();
    assert!(json.get("hidden").is_none(), "false 必须省略键: {json}");
    assert!(json.get("suppressed").is_none(), "{json}");
    let mut flagged = row.clone();
    flagged.hidden = true;
    let json = serde_json::to_value(&flagged).unwrap();
    assert_eq!(json["hidden"], true, "非零值必须保留键: {json}");
    // 显式 false 可解析（旧夹具可能显式发零值）
    let back: DirRow = serde_json::from_value(serde_json::json!({
        "id": "a", "name": "a", "kind": "file", "status": "same",
        "hasChildren": false, "hidden": false, "suppressed": false
    }))
    .unwrap();
    assert!(!back.hidden && !back.suppressed);
    // 但 null 不是合法形态（可选键 ≠ nullable）
    let bad: Result<DirRow, _> = serde_json::from_value(serde_json::json!({
        "id": "a", "name": "a", "kind": "file", "status": "same",
        "hasChildren": false, "hidden": null
    }));
    assert!(bad.is_err(), "零值省略字段不接受 null");

    // ---- 整数：DirStats.dirDiff / similar ----
    let stats = DirStats {
        total: 2,
        ..Default::default()
    };
    let json = serde_json::to_value(stats).unwrap();
    assert!(json.get("dirDiff").is_none(), "0 必须省略键: {json}");
    assert!(json.get("similar").is_none(), "{json}");
    assert_eq!(json["total"], 2, "非零值保留: {json}");
    let stats = DirStats {
        total: 2,
        dir_diff: 1,
        similar: 3,
        ..Default::default()
    };
    let json = serde_json::to_value(stats).unwrap();
    assert_eq!(json["dirDiff"], 1, "{json}");
    assert_eq!(json["similar"], 3, "{json}");
    // 省略键（dirDiff/similar）缺省取 0；其余计数是「键恒定」字段，必须显式给出
    // —— 这正是两类形状的区别：DirStats 自身输出总能被自身反序列化。
    let back: DirStats = serde_json::from_value(serde_json::json!({
        "total": 1, "same": 1, "quickDiff": 0, "contentDiff": 0,
        "leftOnly": 0, "rightOnly": 0, "dirSame": 0
    }))
    .unwrap();
    assert_eq!(back.similar, 0, "缺键取默认 0");
    assert_eq!(back.dir_diff, 0, "缺键取默认 0");
    assert!(
        serde_json::from_value::<DirStats>(serde_json::json!({"total": 1})).is_err(),
        "键恒定字段缺失仍应报错（可选键 ≠ 任意缺键）"
    );

    // ---- String / Vec / 嵌套 Struct：FileFormatSpec ----
    let mut fmt: FileFormatSpec =
        serde_json::from_value(serde_json::json!({"id": "x", "masks": ["*.x"]})).unwrap();
    let json = serde_json::to_value(&fmt).unwrap();
    assert_eq!(json["masks"][0], "*.x", "{json}");
    for k in [
        "description",
        "grammar",
        "tabStops",
        "independentLines",
        "columnData",
    ] {
        assert!(json.get(k).is_none(), "{k} 零值必须省略键: {json}");
    }
    // enabled 有 default 但**无** skip ⇒ 键恒定输出（对照项，避免"见 default 就当可选"）
    assert_eq!(json["enabled"], true, "{json}");

    fmt.description = "X 格式".into();
    fmt.tab_stops = 4;
    fmt.independent_lines = true;
    fmt.column_data = true;
    fmt.grammar = serde_json::from_value(serde_json::json!([
        {"name": "注释", "caseSensitive": true, "items": [{"kind": "delimited", "start": "//", "toEol": true}]}
    ]))
    .unwrap();
    let json = serde_json::to_value(&fmt).unwrap();
    assert_eq!(json["description"], "X 格式", "{json}");
    assert_eq!(json["tabStops"], 4, "{json}");
    assert_eq!(json["independentLines"], true, "{json}");
    assert_eq!(json["columnData"], true, "{json}");
    assert_eq!(json["grammar"][0]["name"], "注释", "{json}");
    assert_eq!(json["grammar"][0]["items"][0]["start"], "//", "{json}");
    assert_eq!(
        json["grammar"][0]["items"][0]["toEol"], true,
        "嵌套零值省略字段同样保留非零值: {json}"
    );
    // 显式空数组/空串等价于缺键
    let explicit: FileFormatSpec = serde_json::from_value(serde_json::json!({
        "id": "y", "masks": [], "description": "", "grammar": [], "tabStops": 0,
        "independentLines": false, "columnData": false
    }))
    .unwrap();
    assert!(explicit.masks.is_empty() && explicit.grammar.is_empty());
    let json = serde_json::to_value(&explicit).unwrap();
    assert!(
        json.get("masks").is_none() && json.get("grammar").is_none(),
        "显式空数组回写时同样省略键（同一语义只有一种线上形态）: {json}"
    );

    // Vec<String> / Vec<Struct> 的省略语义已由上面的 masks / grammar 覆盖
    // （MergeChunkDto.baseLines/leftLines/rightLines 同形状，由静态对账锁覆盖）。

    // ---- 请求侧：TextManualAlignSpec（bool / u32 / Vec） ----
    let spec: TextManualAlignSpec = serde_json::from_value(serde_json::json!({})).unwrap();
    let json = serde_json::to_value(&spec).unwrap();
    for k in [
        "alignAnchors",
        "isolate",
        "neverAlignImportant",
        "skewTolerance",
        "proximityMatch",
        "ignore",
    ] {
        assert!(json.get(k).is_none(), "{k} 零值必须省略键: {json}");
    }
    let spec = TextManualAlignSpec {
        never_align_important: true,
        skew_tolerance: 3,
        proximity_match: true,
        ..spec
    };
    let json = serde_json::to_value(&spec).unwrap();
    assert_eq!(json["neverAlignImportant"], true, "{json}");
    assert_eq!(json["skewTolerance"], 3, "{json}");
    assert_eq!(json["proximityMatch"], true, "{json}");
}
