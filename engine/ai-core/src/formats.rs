//! 文件格式注册表（P10 / FR-FMT-001~003 引擎侧；docs/03 §4.4 schema）。
//!
//! 职责（L2 纯函数规则：只面对"文件名 + 字节前缀"的纯数据，无 IO）：
//! - 格式定义模型：掩码 / 视图类型 / 语法元素五类（FR-FMT-003 文本、分隔、
//!   列表、列、行-页标题）/ 行权重 / 杂项（制表位、行独立、列数据）；
//! - 掩码语义：FR-FLT-001 通配符（`*`/`?`/`[...]`/`[!...]`，`*.*` 匹配任意
//!   文件名，掩码末尾句点 `*.` 仅匹配无扩展名文件），大小写不敏感；
//! - 内容嗅探（轻量）：shebang 解释器识别、魔数二进制识别、BOM 容错（CRLF
//!   与 UTF-8 BOM 不影响判定）；
//! - 格式列表：顺序重要（自上而下首个掩码匹配且启用的格式生效，FR-FMT-001）、
//!   遮蔽检测（被更高项截获的格式，UI 红显）、禁用项跳过、空掩码不匹配；
//! - 内置语法包：C/C++/C#/Java/JS-TS/Python/Go/Rust/SQL/Shell/XML/JSON/
//!   INI/TOML/YAML/Perl/Ruby/Lua ≥ 12 语言（注释 + 字符串为主）；
//! - 外部格式导入导出：BC 风格格式集（JSON 载体，`.bcpack`/`*.json` 均可）
//!   [`FormatPack`]，可加载/保存/合并；
//! - **会话规则应用钩子 `line_classify`**：把格式携带的重要性规则（"注释行 /
//!   字符串内容不算差异"）映射为行文本的"不重要掩码"——重要性键阶段先把
//!   不重要语法元素的匹配跨度折叠为占位符，再走 P1 空白/大小写折叠
//!   （见 [`crate::rules::RulesOptions`] 与 `keys::importance_key`）。

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// 数据模型（serde 形态 = 持久化与格式集载体；docs/03 §4.4）
// ---------------------------------------------------------------------------

/// 视图类型（FR-FMT-001：文件格式决定可视化的视图）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FormatViewKind {
    #[default]
    Text,
    Table,
    Hex,
    Picture,
    External,
}

/// 语法项（FR-FMT-003 五类；内部 tag = `kind`，serde camelCase）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SyntaxItem {
    /// 文本匹配串（可正则、可区分大小写）
    Text {
        #[serde(default)]
        pattern: String,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_sensitive: bool,
    },
    /// 分隔类别：起始/结束分隔符（或起始 + 到行尾）；可指定转义字符
    Delimited {
        #[serde(default)]
        start: String,
        /// 结束分隔符；与 `to_eol` 二选一（都缺省 = 到行尾）
        #[serde(default)]
        end: Option<String>,
        /// 转义字符（如 `\` 允许内嵌结束分隔符）
        #[serde(default)]
        escape: Option<String>,
        /// true = 起始匹配后直到行尾
        #[serde(default)]
        to_eol: bool,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_sensitive: bool,
    },
    /// 列表类别：逐行 token 列表（可正则、可区分大小写）
    List {
        #[serde(default)]
        tokens: Vec<String>,
        #[serde(default)]
        regex: bool,
        #[serde(default)]
        case_sensitive: bool,
    },
    /// 列类别：起始列-结束列（1 起；to = None 表示到行尾）
    Column { from: u32, #[serde(default)] to: Option<u32> },
    /// 行类别：页面标题（匹配每页顶部标题，通常换页字符；"或第 1 行"处理首页）
    RowTitle {
        #[serde(default)]
        pattern: String,
        /// 标题行数（含首行）
        #[serde(default = "default_title_lines")]
        lines: u32,
        #[serde(default)]
        or_first_line: bool,
    },
}

fn default_title_lines() -> u32 {
    1
}

impl SyntaxItem {
    /// 本项在 `text` 的 `pos`（字符下标）处能匹配的跨度（相对 pos 的字符长度）。
    /// None = 该位置不匹配。RowTitle/Column 为整行/位置判定，不经此扫描。
    pub fn match_len_at(&self, text: &str, pos: usize) -> Option<usize> {
        match self {
            SyntaxItem::Text { pattern, regex, case_sensitive } => {
                if *regex {
                    regex_match_len(pattern, &text[pos..], *case_sensitive, false)
                } else {
                    literal_match_len(pattern, &text[pos..], *case_sensitive)
                }
            }
            SyntaxItem::Delimited { start, end, escape, to_eol, regex, case_sensitive } => {
                let start_len = if *regex {
                    regex_match_len(start, &text[pos..], *case_sensitive, false)?
                } else {
                    literal_match_len(start, &text[pos..], *case_sensitive)?
                };
                if *to_eol {
                    return Some(text.chars().count() - pos);
                }
                let end = end.as_deref().unwrap_or("");
                if end.is_empty() {
                    return Some(text.chars().count() - pos);
                }
                // 扫描结束分隔符（处理转义）
                let chars: Vec<char> = text.chars().collect();
                let esc = escape.as_deref().and_then(|e| e.chars().next());
                let mut i = pos + start_len;
                while i < chars.len() {
                    if let Some(e) = esc {
                        if chars[i] == e {
                            i += 2;
                            continue;
                        }
                    }
                    let rest: String = chars[i..].iter().collect();
                    let end_len = if end == "\n" {
                        None
                    } else if *regex {
                        regex_match_len(end, &rest, *case_sensitive, false)
                    } else {
                        literal_match_len(end, &rest, *case_sensitive)
                    };
                    if let Some(l) = end_len {
                        return Some(i - pos + l);
                    }
                    i += 1;
                }
                // 未闭合：剩余整段视为跨度（注释跨行由逐行模型自然截断）
                Some(chars.len() - pos)
            }
            SyntaxItem::List { tokens, regex, case_sensitive } => {
                let rest = &text[pos..];
                tokens
                    .iter()
                    .filter_map(|t| {
                        if *regex {
                            regex_match_len(t, rest, *case_sensitive, false)
                        } else {
                            literal_match_len(t, rest, *case_sensitive)
                        }
                    })
                    .max()
            }
            SyntaxItem::Column { .. } => None, // 列类别为位置掩码，不走扫描匹配
            SyntaxItem::RowTitle { .. } => None, // 页标题为整行判定，不走扫描匹配
        }
    }
}

/// 语法元素（元素名 + 若干语法项；列表顺序即优先级）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrammarElement {
    pub name: String,
    /// 元素级"此元素区分大小写"（FR-FMT-003；作为项级缺省的兜底）
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub items: Vec<SyntaxItem>,
}

impl GrammarElement {
    pub fn new(name: &str, items: Vec<SyntaxItem>) -> Self {
        GrammarElement { name: name.to_string(), case_sensitive: false, items }
    }
}

/// 行权重：低权重行不优先做对齐锚点（FR-FMT-002 语法选项卡）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineWeight {
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub weight: i32,
}

/// 单个文本格式定义（格式列表有序，首匹配生效）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileFormat {
    pub id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 文件名掩码（FR-FLT-001 语义；空 = 不匹配任何文件，仅能手动选择）
    #[serde(default)]
    pub masks: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub view_kind: FormatViewKind,
    /// 语法元素列表（顺序即优先级）
    #[serde(default)]
    pub grammar: Vec<GrammarElement>,
    #[serde(default)]
    pub weights: Vec<LineWeight>,
    /// 制表位（缺省 4，取自容器 Default；字段级 default 会退化为 0，勿加）
    #[serde(default = "default_tab_stops")]
    pub tab_stops: u32,
    /// 行是独立的（每行独立记录，连续差异不作为部分）
    #[serde(default)]
    pub independent_lines: bool,
    /// 基于列的数据（逐列比较）
    #[serde(default)]
    pub column_data: bool,
    /// 编码覆盖（None = 自动检测；代码页覆盖属 ENC-001，模型先行）
    #[serde(default)]
    pub encoding: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_tab_stops() -> u32 {
    4
}

impl Default for FileFormat {
    fn default() -> Self {
        FileFormat {
            id: String::new(),
            enabled: true,
            masks: Vec::new(),
            description: String::new(),
            view_kind: FormatViewKind::Text,
            grammar: Vec::new(),
            weights: Vec::new(),
            tab_stops: 4,
            independent_lines: false,
            column_data: false,
            encoding: None,
        }
    }
}

impl FileFormat {
    /// 便捷构造（内置表与测试用）
    pub fn text(id: &str, description: &str, masks: &[&str], grammar: Vec<GrammarElement>) -> Self {
        FileFormat {
            id: id.to_string(),
            description: description.to_string(),
            masks: masks.iter().map(|m| m.to_string()).collect(),
            grammar,
            ..FileFormat::default()
        }
    }
}

// ---------------------------------------------------------------------------
// 掩码语义（FR-FLT-001 通配符；ai-core 侧轻量实现，与 ai-dir NameFilter 同语义）
// ---------------------------------------------------------------------------

/// 文件名掩码匹配（BC 语义）：
/// - `?` 匹配任意单字符；`*` 匹配零或任意数量字符（含 `.`）；
/// - `[az]`/`[a-z]` 集合/范围；`[!az]` 补集；`[[]` 匹配 `[`；
/// - `*.*` 因历史原因匹配任何文件名（包括无扩展名）；
/// - 掩码以 `.` 结尾（如 `*.`）时 `?`/`*` 不匹配句点 → 仅无扩展名文件；
/// - 大小写不敏感。
pub fn mask_matches(mask: &str, name: &str) -> bool {
    let mask = mask.trim();
    let name_l = name.to_lowercase();
    let mask_l = mask.to_lowercase();
    if mask_l == "*.*" || mask_l == "*" {
        return true;
    }
    if mask_l.ends_with('.') {
        // 仅无扩展名文件；通配符不吞句点
        if name_l.contains('.') {
            return false;
        }
        return glob_match(&mask_l[..mask_l.len() - 1], &name_l, true);
    }
    glob_match(&mask_l, &name_l, false)
}

/// 通配符回溯匹配（`dot_free` = 通配符不匹配 `.`；`?` 任意单字符、
/// `[...]`/`[!...]`/`[a-z]` 字符类）
fn glob_match(pattern: &str, name: &str, dot_free: bool) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0usize, 0usize);
    let (mut star_pi, mut star_ni) = (usize::MAX, 0usize);
    while ni < n.len() {
        if pi < p.len() && p[pi] == '*' {
            star_pi = pi;
            star_ni = ni;
            pi += 1;
            continue;
        }
        let ok = pi < p.len() && {
            let mut next = pi;
            let hit = glob_char(&p, &mut next, n[ni], dot_free);
            if hit {
                pi = next;
            }
            hit
        };
        if ok {
            ni += 1;
        } else if star_pi != usize::MAX {
            if dot_free && n[star_ni] == '.' {
                return false;
            }
            star_ni += 1;
            pi = star_pi + 1;
            ni = star_ni;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// 匹配单个字符原子；命中时把 `*pi` 推进到原子之后
fn glob_char(p: &[char], pi: &mut usize, c: char, dot_free: bool) -> bool {
    let pc = p[*pi];
    match pc {
        '*' => true, // 不应到达（调用方处理星号）
        '?' => {
            if dot_free && c == '.' {
                return false;
            }
            *pi += 1;
            true
        }
        '[' => {
            let mut i = *pi + 1;
            let neg = matches!(p.get(i), Some('^') | Some('!'));
            if neg {
                i += 1;
            }
            let mut matched = false;
            let mut first = true;
            while i < p.len() && (first || p[i] != ']') {
                first = false;
                let lo = p[i];
                if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
                    let hi = p[i + 2];
                    if c >= lo && c <= hi {
                        matched = true;
                    }
                    i += 3;
                } else {
                    if c == lo {
                        matched = true;
                    }
                    i += 1;
                }
            }
            if i >= p.len() {
                return false; // 未闭合类按不匹配
            }
            *pi = i + 1; // 跳过 ']'
            matched != neg
        }
        pc => {
            *pi += 1;
            pc == c // mask/name 均已小写化
        }
    }
}

// ---------------------------------------------------------------------------
// 轻量正则子集（内置语法包的注释模式足够：`//.*`、`#.*`、`--.*` 等）
// 支持原子：字面字符、`.`、`[...]`/`[^...]`；后缀 `*`/`+`；`\` 转义；
// 整体锚定 `$`（full=true 时要求匹配到结尾）。
// ---------------------------------------------------------------------------

fn regex_match_len(pattern: &str, text: &str, case_sensitive: bool, full: bool) -> Option<usize> {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    regex_here(&pat, 0, &txt, 0, case_sensitive, full).map(|(_, end)| end)
}

/// 在 txt[cur..] 处匹配 pattern[pi..]，返回 (pi 终点, txt 终点)（最长匹配）
fn regex_here(
    pat: &[char],
    pi: usize,
    txt: &[char],
    cur: usize,
    cs: bool,
    full: bool,
) -> Option<(usize, usize)> {
    if pi >= pat.len() {
        return if !full || cur == txt.len() { Some((pi, cur)) } else { None };
    }
    // 解析原子
    let (atom, next_pi) = parse_atom(pat, pi)?;
    let quant = pat.get(next_pi).copied();
    let (min, max): (usize, Option<usize>) = match quant {
        Some('*') => (0, None),
        Some('+') => (1, None),
        Some('?') => (0, Some(1)),
        _ => (1, Some(1)),
    };
    let next_pi = if matches!(quant, Some('*') | Some('+') | Some('?')) { next_pi + 1 } else { next_pi };

    // 贪心 + 回溯：原子按次消费（ends[n] = 消费 n 次后的位置），再逐次吐出
    let mut ends: Vec<usize> = vec![cur];
    let mut pos = cur;
    let mut consumed = 0usize;
    loop {
        if let Some(m) = max {
            if consumed >= m {
                break;
            }
        }
        match atom_match(&atom, txt, pos, cs) {
            Some(l) if l > 0 => {
                pos += l;
                consumed += 1;
                ends.push(pos);
            }
            _ => break,
        }
    }
    if ends.len() > min {
        for count in (min..ends.len()).rev() {
            if let Some(r) = regex_here(pat, next_pi, txt, ends[count], cs, full) {
                return Some(r);
            }
        }
    }
    None
}

enum Atom {
    Any,
    Lit(char),
    Class(Vec<(char, char)>, bool),
}

fn parse_atom(pat: &[char], pi: usize) -> Option<(Atom, usize)> {
    let c = pat[pi];
    match c {
        '.' => Some((Atom::Any, pi + 1)),
        '\\' => {
            let c = *pat.get(pi + 1)?;
            Some((Atom::Lit(c), pi + 2))
        }
        '[' => {
            let mut i = pi + 1;
            let neg = matches!(pat.get(i), Some('^'));
            if neg {
                i += 1;
            }
            let mut ranges = Vec::new();
            // `]` 出现在首位为字面
            while i < pat.len() && (ranges.is_empty() || pat[i] != ']') {
                if pat[i] == ']' {
                    break;
                }
                let lo = if pat[i] == '\\' && i + 1 < pat.len() {
                    i += 1;
                    pat[i]
                } else {
                    pat[i]
                };
                if i + 2 < pat.len() && pat[i + 1] == '-' && pat[i + 2] != ']' {
                    ranges.push((lo, pat[i + 2]));
                    i += 3;
                } else {
                    ranges.push((lo, lo));
                    i += 1;
                }
            }
            if i >= pat.len() {
                return None; // 未闭合类
            }
            Some((Atom::Class(ranges, neg), i + 1))
        }
        c => Some((Atom::Lit(c), pi + 1)),
    }
}

fn atom_match(a: &Atom, txt: &[char], pos: usize, cs: bool) -> Option<usize> {
    let c = *txt.get(pos)?;
    let ok = match a {
        Atom::Any => true,
        Atom::Lit(l) => fold_eq(*l, c, cs),
        Atom::Class(ranges, neg) => {
            let inside = ranges.iter().any(|(lo, hi)| fold_eq(*lo, c, cs) && c as u32 <= *hi as u32 && c as u32 >= *lo as u32);
            inside != *neg
        }
    };
    ok.then(|| 1)
}

fn fold_eq(a: char, b: char, cs: bool) -> bool {
    if cs {
        a == b
    } else {
        a.to_lowercase().eq(b.to_lowercase())
    }
}

/// 字面匹配长度（大小写可选敏感；pattern 为空 = 不匹配，避免零长噪音）
fn literal_match_len(pattern: &str, text: &str, case_sensitive: bool) -> Option<usize> {
    if pattern.is_empty() {
        return None;
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    if t.len() < p.len() {
        return None;
    }
    let ok = p.iter().zip(t.iter()).all(|(pc, tc)| fold_eq(*pc, *tc, case_sensitive));
    ok.then_some(p.len())
}

// ---------------------------------------------------------------------------
// 内容嗅探（轻量：shebang / 魔数 / BOM / NUL）
// ---------------------------------------------------------------------------

/// 嗅探结论
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SniffKind {
    Text,
    Binary,
}

/// 常见二进制魔数（首字节前缀）
const MAGIC_PREFIXES: &[&[u8]] = &[
    b"%PDF",
    b"\x89PNG",
    b"GIF8",
    b"MZ",
    b"\x7fELF",
    b"PK\x03\x04",
    b"\xCA\xFE\xBA\xBE",
    b"\xD0\xCF\x11\xE0", // OLE2（doc/xls 旧格式）
    b"\x1f\x8b",         // gzip
    b"Rar!",
    b"\x00\x00\x01\x00", // ICO
    b"fLtyp",
];

/// UTF-8 BOM 剥离（ai-core 本地实现；与 ai-vfs::strip_bom 同语义）
pub fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

/// 字节前缀嗅探：NUL 字节 / 魔数 → 二进制；UTF-16 BOM 按文本（编码层处理）；
/// UTF-8 BOM 剥离后进入 shebang 判断；CRLF 不影响判定。
pub fn sniff_bytes(head: &[u8]) -> SniffKind {
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return SniffKind::Text; // UTF-16 BOM：文本（编码覆盖属 ENC-001）
    }
    let stripped = strip_utf8_bom(head);
    for m in MAGIC_PREFIXES {
        if stripped.starts_with(m) {
            return SniffKind::Binary;
        }
    }
    if stripped.contains(&0u8) {
        return SniffKind::Binary;
    }
    SniffKind::Text
}

/// shebang 解释器名（`#!/usr/bin/env python3` → "python3"→ 归一 "python"）
pub fn sniff_shebang(head: &[u8]) -> Option<String> {
    let stripped = strip_utf8_bom(head);
    if !stripped.starts_with(b"#!") {
        return None;
    }
    let line_end = stripped.iter().position(|&b| b == b'\n').unwrap_or(stripped.len());
    let line = String::from_utf8_lossy(&stripped[2..line_end]);
    let line = line.trim_end_matches('\r').trim();
    let mut it = line.split_whitespace();
    let first = it.next().unwrap_or("");
    // `#!/usr/bin/env X` 形态：取 env 后的解释器
    let interp = if first.ends_with("env") {
        it.next().unwrap_or("")
    } else {
        // `#! /usr/bin/python` 形态：取路径末段
        first.rsplit('/').next().unwrap_or(first)
    };
    let base = interp.rsplit(['\\', '/']).next().unwrap_or(interp);
    let norm = match base {
        "python3" | "python3.x" => "python".to_string(),
        "bash" | "zsh" | "ksh" | "dash" => "sh".to_string(),
        "node" | "nodejs" | "deno" => "node".to_string(),
        other => other.to_string(),
    };
    if norm.is_empty() {
        None
    } else {
        Some(norm)
    }
}

// ---------------------------------------------------------------------------
// 格式列表（顺序敏感 + 遮蔽检测）
// ---------------------------------------------------------------------------

/// 文件类别判定结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileClass<'a> {
    /// 文本（可附命中格式）
    Text { format: Option<&'a FileFormat> },
    /// 二进制（NUL/魔数）
    Binary,
}

/// 可排序的格式注册表（格式列表：自上而下扫描，首个掩码匹配且启用者生效）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FormatRegistry {
    pub formats: Vec<FileFormat>,
}

impl FormatRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 内置格式表（≥12 语言；顺序即优先级）
    pub fn builtin() -> Self {
        FormatRegistry { formats: builtin_formats() }
    }

    /// 追加/按 id 覆盖（外部格式导入；返回受影响条数）
    pub fn upsert(&mut self, fmt: FileFormat) -> bool {
        if let Some(slot) = self.formats.iter_mut().find(|f| f.id == fmt.id) {
            *slot = fmt;
            return false; // 覆盖
        }
        self.formats.push(fmt);
        true // 新增
    }

    /// 检测：首个掩码匹配且启用的格式（嗅探仅做兜底语言识别）
    pub fn detect<'a>(&'a self, file_name: &str, head: &[u8]) -> Option<&'a FileFormat> {
        // 1. 掩码优先（FR-FMT-001：列表顺序扫描）
        for f in &self.formats {
            if !f.enabled || f.masks.is_empty() {
                continue;
            }
            if f.masks.iter().any(|m| mask_matches(m, file_name)) {
                return Some(f);
            }
        }
        // 2. shebang 兜底（无掩码命中的脚本）
        if let Some(interp) = sniff_shebang(head) {
            for f in &self.formats {
                if f.enabled && f.id == format!("builtin-{interp}") {
                    return Some(f);
                }
            }
        }
        None
    }

    /// 文本 vs 二进制判定（dir/open 格式表增强的判定核；FR-DIR/SEL-001）
    pub fn classify_file<'a>(&'a self, file_name: &str, head: &[u8]) -> FileClass<'a> {
        match sniff_bytes(head) {
            SniffKind::Binary => FileClass::Binary,
            SniffKind::Text => FileClass::Text { format: self.detect(file_name, head) },
        }
    }

    /// 被更高项遮蔽的格式 id 集（UI 红显；近似：掩码图案与更高项完全相等，
    /// 或更高项为全匹配 `*`/`*.*`）
    pub fn shadowed_ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, f) in self.formats.iter().enumerate() {
            if !f.enabled || f.masks.is_empty() {
                continue;
            }
            let shadowed = self.formats[..i].iter().any(|earlier| {
                earlier.enabled
                    && earlier.masks.iter().any(|m| {
                        let m = m.trim();
                        (m == "*.*" || m == "*") || f.masks.iter().any(|o| o.trim() == m)
                    })
            });
            if shadowed {
                out.push(f.id.clone());
            }
        }
        out
    }
}

/// 内置语法包（≥12 语言；注释 + 字符串为主，覆盖 P10 验收语料）
pub fn builtin_formats() -> Vec<FileFormat> {
    let mut v = Vec::new();
    let mut push = |id: &str, desc: &str, masks: &[&str], els: Vec<GrammarElement>| {
        v.push(FileFormat::text(id, desc, masks, els));
    };

    // C 家族（顺序：越具体越靠前）
    let c_els = vec![
        GrammarElement::new(
            "注释",
            vec![
                SyntaxItem::Delimited { start: "/*".into(), end: Some("*/".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                SyntaxItem::Delimited { start: "//".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false },
            ],
        ),
        GrammarElement::new(
            "字符串",
            vec![
                SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                SyntaxItem::Delimited { start: "'".into(), end: Some("'".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
            ],
        ),
    ];
    push("builtin-cpp", "C/C++ 源码", &["*.cpp", "*.cc", "*.cxx", "*.hpp", "*.hh", "*.hxx", "*.h", "*.c"], c_els.clone());
    push("builtin-csharp", "C# 源码", &["*.cs"], c_els.clone());
    push("builtin-java", "Java 源码", &["*.java"], c_els);
    push(
        "builtin-jsts",
        "JavaScript/TypeScript",
        &["*.js", "*.mjs", "*.cjs", "*.jsx", "*.ts", "*.tsx", "*.mts", "*.cts"],
        vec![
            GrammarElement::new(
                "注释",
                vec![
                    SyntaxItem::Delimited { start: "/*".into(), end: Some("*/".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "//".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false },
                ],
            ),
            GrammarElement::new(
                "字符串",
                vec![
                    SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "'".into(), end: Some("'".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "`".into(), end: Some("`".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                ],
            ),
        ],
    );
    push(
        "builtin-python",
        "Python 源码",
        &["*.py", "*.pyw", "*.pyi"],
        vec![
            GrammarElement::new(
                "注释",
                vec![SyntaxItem::Delimited { start: "#".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false }],
            ),
            GrammarElement::new(
                "字符串",
                vec![
                    SyntaxItem::Delimited { start: "\"\"\"".into(), end: Some("\"\"\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "'''".into(), end: Some("'''".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "'".into(), end: Some("'".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                ],
            ),
        ],
    );
    push(
        "builtin-go",
        "Go 源码",
        &["*.go"],
        vec![
            GrammarElement::new(
                "注释",
                vec![
                    SyntaxItem::Delimited { start: "/*".into(), end: Some("*/".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "//".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false },
                ],
            ),
            GrammarElement::new(
                "字符串",
                vec![
                    SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "`".into(), end: Some("`".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                ],
            ),
        ],
    );
    push(
        "builtin-rust",
        "Rust 源码",
        &["*.rs"],
        vec![
            GrammarElement::new(
                "注释",
                vec![
                    SyntaxItem::Delimited { start: "/*".into(), end: Some("*/".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "//".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false },
                ],
            ),
            GrammarElement::new(
                "字符串",
                vec![SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false }],
            ),
        ],
    );
    push(
        "builtin-sql",
        "SQL 脚本",
        &["*.sql"],
        vec![
            GrammarElement::new(
                "注释",
                vec![
                    SyntaxItem::Delimited { start: "--".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "/*".into(), end: Some("*/".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                ],
            ),
            GrammarElement::new(
                "字符串",
                vec![
                    SyntaxItem::Delimited { start: "'".into(), end: Some("'".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                ],
            ),
        ],
    );
    push(
        "builtin-sh",
        "Shell 脚本",
        &["*.sh", "*.bash", "*.zsh"],
        vec![
            GrammarElement::new(
                "注释",
                vec![SyntaxItem::Delimited { start: "#".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false }],
            ),
            GrammarElement::new(
                "字符串",
                vec![
                    SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "'".into(), end: Some("'".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                ],
            ),
        ],
    );
    push(
        "builtin-node",
        "Node 脚本（无扩展名）",
        &[],
        vec![GrammarElement::new(
            "注释",
            vec![
                SyntaxItem::Delimited { start: "/*".into(), end: Some("*/".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                SyntaxItem::Delimited { start: "//".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false },
            ],
        )],
    );
    push(
        "builtin-perl",
        "Perl 源码",
        &["*.pl", "*.pm"],
        vec![
            GrammarElement::new(
                "注释",
                vec![SyntaxItem::Delimited { start: "#".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false }],
            ),
            GrammarElement::new(
                "字符串",
                vec![
                    SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "'".into(), end: Some("'".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                ],
            ),
        ],
    );
    push(
        "builtin-ruby",
        "Ruby 源码",
        &["*.rb"],
        vec![
            GrammarElement::new(
                "注释",
                vec![SyntaxItem::Delimited { start: "#".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false }],
            ),
            GrammarElement::new(
                "字符串",
                vec![
                    SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "'".into(), end: Some("'".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                ],
            ),
        ],
    );
    push(
        "builtin-lua",
        "Lua 源码",
        &["*.lua"],
        vec![
            GrammarElement::new(
                "注释",
                vec![
                    SyntaxItem::Delimited { start: "--[[".into(), end: Some("]]".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "--".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false },
                ],
            ),
            GrammarElement::new(
                "字符串",
                vec![SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false }],
            ),
        ],
    );
    push(
        "builtin-xml",
        "XML 文档",
        &["*.xml", "*.xsl", "*.xslt", "*.svg", "*.xsd"],
        vec![
            GrammarElement::new(
                "注释",
                vec![SyntaxItem::Delimited { start: "<!--".into(), end: Some("-->".into()), escape: None, to_eol: false, regex: false, case_sensitive: false }],
            ),
            GrammarElement::new(
                "字符串",
                vec![
                    SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "'".into(), end: Some("'".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                ],
            ),
        ],
    );
    push(
        "builtin-json",
        "JSON 数据",
        &["*.json", "*.jsonc", "*.json5"],
        vec![
            GrammarElement::new(
                "注释",
                vec![
                    SyntaxItem::Delimited { start: "/*".into(), end: Some("*/".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "//".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false },
                ],
            ),
            GrammarElement::new(
                "字符串",
                vec![SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false }],
            ),
        ],
    );
    push(
        "builtin-toml",
        "TOML 配置",
        &["*.toml"],
        vec![
            GrammarElement::new(
                "注释",
                vec![SyntaxItem::Delimited { start: "#".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false }],
            ),
            GrammarElement::new(
                "字符串",
                vec![
                    SyntaxItem::Delimited { start: "\"\"\"".into(), end: Some("\"\"\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "\"".into(), end: Some("\"".into()), escape: Some("\\".into()), to_eol: false, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "'".into(), end: Some("'".into()), escape: None, to_eol: false, regex: false, case_sensitive: false },
                ],
            ),
        ],
    );
    push(
        "builtin-yaml",
        "YAML 配置",
        &["*.yml", "*.yaml"],
        vec![GrammarElement::new(
            "注释",
            vec![SyntaxItem::Delimited { start: "#".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false }],
        )],
    );
    push(
        "builtin-ini",
        "INI 配置",
        &["*.ini", "*.cfg", "*.conf", "*.properties"],
        vec![
            GrammarElement::new(
                "注释",
                vec![
                    SyntaxItem::Delimited { start: ";".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false },
                    SyntaxItem::Delimited { start: "#".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false },
                ],
            ),
        ],
    );
    // 页标题示例（报表输出比较：换页符标题行不重要）
    push(
        "builtin-report",
        "分页报表文本",
        &["*.prt", "*.listing"],
        vec![GrammarElement::new(
            "页标题",
            vec![SyntaxItem::RowTitle { pattern: "\\f.*".into(), lines: 2, or_first_line: true }],
        )],
    );
    v
}

// ---------------------------------------------------------------------------
// 外部格式导入导出（FR-FMT-005 外部格式；BC 风格格式集，JSON 载体）
// ---------------------------------------------------------------------------

/// 格式集（BC 风格 `.bcpack` / JSON 格式集文件的内容模型）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FormatPack {
    /// 载体版本（当前 1）
    pub version: u32,
    #[serde(default = "default_pack_kind")]
    pub kind: String,
    #[serde(default)]
    pub formats: Vec<FileFormat>,
}

fn default_pack_kind() -> String {
    "ai-compare-format-pack".to_string()
}

impl Default for FormatPack {
    fn default() -> Self {
        FormatPack { version: 1, kind: default_pack_kind(), formats: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatPackError {
    #[error("格式集 JSON 解析失败: {0}")]
    Json(String),
    #[error("格式集版本不支持: {0}")]
    Version(u32),
}

impl FormatPack {
    pub fn from_json(text: &str) -> Result<Self, FormatPackError> {
        let pack: FormatPack =
            serde_json::from_str(text).map_err(|e| FormatPackError::Json(e.to_string()))?;
        if pack.version > 1 {
            return Err(FormatPackError::Version(pack.version));
        }
        Ok(pack)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

impl FormatRegistry {
    /// 导出当前注册表为格式集
    pub fn to_pack(&self) -> FormatPack {
        FormatPack { version: 1, kind: default_pack_kind(), formats: self.formats.clone() }
    }

    /// 合并格式集（按 id 覆盖/追加；返回受影响条数）
    pub fn merge_pack(&mut self, pack: &FormatPack) -> usize {
        pack.formats.iter().filter(|f| self.upsert((*f).clone())).count()
    }
}

// ---------------------------------------------------------------------------
// 会话规则应用钩子：line_classify / unimportant_mask（FR-TXT-010 × FR-FMT-003）
// ---------------------------------------------------------------------------

/// 行分类（P10 行预处理钩子：重要性键阶段的语法折叠输入）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineClass {
    /// 普通行（含代码/正文）
    Plain,
    /// 整行均为不重要内容（整行注释 / 页标题行）——与任意其他
    /// UnimportantText 行在重要性键中等价（"注释行不算差异"）
    UnimportantText,
}

/// 不重要占位符（重要性键中语法折叠跨度的替换字符）
pub const UNIMPORTANT_MARK: char = '\u{2}';

/// 元素级"此元素区分大小写"兜底：项级未显式声明（false）时沿用元素级声明
fn ensure_item_case(item: &SyntaxItem, element_case: bool) -> SyntaxItem {
    let mut cloned = item.clone();
    match &mut cloned {
        SyntaxItem::Text { case_sensitive, .. }
        | SyntaxItem::Delimited { case_sensitive, .. }
        | SyntaxItem::List { case_sensitive, .. } => {
            *case_sensitive = *case_sensitive || element_case;
        }
        _ => {}
    }
    cloned
}

/// 计算"不重要掩码"：把 `unimportant` 元素命中的跨度替换为 [`UNIMPORTANT_MARK`]。
///
/// 语义（FR-TXT-010 重要性列表 × FR-FMT-003 语法元素）：
/// - 嵌入不重要元素（字符串/注释）→ 内容折叠，代码骨架保留
///   （`int a; // x` vs `int a; // y` → `int a;<mark>` 等价）；
/// - 整行注释 → 整行折叠为单个占位（不同注释行彼此等价）；
/// - 列类别按 1 起列号定位；页标题（\f 前缀行，或 orFirstLine 首行）整行折叠；
/// - 孤儿行（Insert/Delete）的重要性由 keys 层保证恒为 Important，不受此影响。
pub fn unimportant_mask(
    text: &str,
    grammar: &[GrammarElement],
    unimportant: &[String],
    line_index: Option<usize>,
) -> String {
    if unimportant.is_empty() || grammar.is_empty() {
        return text.to_string();
    }
    let lower: Vec<String> = unimportant.iter().map(|s| s.to_lowercase()).collect();
    let active: Vec<&GrammarElement> = grammar
        .iter()
        .filter(|e| lower.iter().any(|n| *n == e.name.to_lowercase()))
        .collect();
    if active.is_empty() {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();

    // 页标题整行判定（\f 前缀；orFirstLine 处理首页）
    for e in &active {
        for item in &e.items {
            if let SyntaxItem::RowTitle { pattern, or_first_line, .. } = item {
                let is_title = chars.first() == Some(&'\u{c}')
                    || (*or_first_line && line_index == Some(0));
                if is_title {
                    if !pattern.is_empty() || *or_first_line {
                        return UNIMPORTANT_MARK.to_string();
                    }
                }
            }
        }
    }

    // 列类别的位置掩码
    let mut col_mask = vec![false; chars.len()];
    for e in &active {
        for item in &e.items {
            if let SyntaxItem::Column { from, to } = item {
                let start = from.saturating_sub(1) as usize;
                let end = match to {
                    Some(t) => (*t as usize).min(chars.len()),
                    None => chars.len(),
                };
                for slot in col_mask.iter_mut().take(end).skip(start) {
                    *slot = true;
                }
            }
        }
    }

    // 逐位置扫描：优先取最长的不重要跨度
    let mut out = String::with_capacity(text.len());
    let mut pos = 0usize;
    while pos < chars.len() {
        if col_mask[pos] {
            out.push(UNIMPORTANT_MARK);
            while pos < chars.len() && col_mask[pos] {
                pos += 1;
            }
            continue;
        }
        let mut best: Option<usize> = None;
        for e in &active {
            for item in &e.items {
                // 元素级 case_sensitive 为项级缺省的兜底（FR-FMT-003）
                let effective = ensure_item_case(item, e.case_sensitive);
                if let Some(l) = effective.match_len_at(text, pos) {
                    if l > 0 && best.map_or(true, |b| l > b) {
                        best = Some(l);
                    }
                }
            }
        }
        match best {
            Some(l) => {
                out.push(UNIMPORTANT_MARK);
                pos += l;
            }
            None => {
                out.push(chars[pos]);
                pos += 1;
            }
        }
    }
    out
}

/// 行分类：掩码后仅剩占位符/空白 → [`LineClass::UnimportantText`]。
pub fn line_classify(
    text: &str,
    grammar: &[GrammarElement],
    unimportant: &[String],
    line_index: Option<usize>,
) -> LineClass {
    let masked = unimportant_mask(text, grammar, unimportant, line_index);
    let has_mark = masked.contains(UNIMPORTANT_MARK);
    if has_mark && masked.chars().all(|c| c == UNIMPORTANT_MARK || c.is_whitespace()) {
        LineClass::UnimportantText
    } else {
        LineClass::Plain
    }
}

/// 内置注册表单例（引擎/CLI 共享，避免每次重建）
pub fn builtin_registry() -> &'static FormatRegistry {
    static REG: OnceLock<FormatRegistry> = OnceLock::new();
    REG.get_or_init(|| FormatRegistry::builtin())
}

// ---------------------------------------------------------------------------
// 测试：格式检测矩阵（≥20：掩码边界/嗅探/CRLF/BOM）+ 分类钩子 + 导入导出
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> FormatRegistry {
        FormatRegistry::builtin()
    }

    // ---- 掩码语义（FR-FLT-001） ----

    #[test]
    fn mask01_star_star_matches_any_name() {
        assert!(mask_matches("*.*", "Makefile"));
        assert!(mask_matches("*.*", "a.txt"));
        assert!(mask_matches("*", "anything.tar.gz"));
    }

    #[test]
    fn mask02_trailing_dot_only_extensionless() {
        assert!(mask_matches("*.", "Makefile"));
        assert!(!mask_matches("*.", "a.txt"));
        assert!(mask_matches("readme.", "readme"));
        assert!(!mask_matches("readme.", "readme.txt"));
    }

    #[test]
    fn mask03_question_mark_single_char() {
        assert!(mask_matches("?.c", "a.c"));
        assert!(!mask_matches("?.c", "ab.c"));
        assert!(mask_matches("*.???", "a.tsx"), "点后恰 3 字符");
        assert!(!mask_matches("*.???", "foo.ts"), "点后仅 2 字符不满足 3 个 ?");
    }

    #[test]
    fn mask04_char_class_and_negation() {
        assert!(mask_matches("[abc].txt", "b.txt"));
        assert!(!mask_matches("[abc].txt", "d.txt"));
        assert!(mask_matches("[a-z0-9].log", "7.log"));
        assert!(mask_matches("[!abc].log", "d.log"));
        assert!(!mask_matches("[!abc].log", "a.log"));
    }

    #[test]
    fn mask05_case_insensitive() {
        assert!(mask_matches("*.C", "foo.c"));
        assert!(mask_matches("README", "readme"));
        assert!(mask_matches("*.JPG", "photo.jpg"));
    }

    #[test]
    fn mask06_empty_mask_matches_nothing() {
        assert!(!mask_matches("", "anything"));
        assert!(!mask_matches("   ", "anything"));
    }

    #[test]
    fn mask07_star_crosses_dots() {
        assert!(mask_matches("a*", "a.tar.gz"));
        assert!(mask_matches("*.tar.*", "x.tar.gz"));
        assert!(!mask_matches("a?c", "ac"));
    }

    // ---- 检测矩阵：掩码命中 / 边界 ----

    #[test]
    fn det08_c_by_mask() {
        let r = reg();
        let f = r.detect("main.c", b"int main(){}").expect("C 掩码命中");
        assert_eq!(f.id, "builtin-cpp", "*.c 归 C/C++（首匹配）");
    }

    #[test]
    fn det09_header_mask_hits_cpp_format() {
        let r = reg();
        assert_eq!(r.detect("util.h", b"").unwrap().id, "builtin-cpp");
        assert_eq!(r.detect("util.hpp", b"").unwrap().id, "builtin-cpp");
    }

    #[test]
    fn det10_py_ts_rs_sql_masks() {
        let r = reg();
        assert_eq!(r.detect("app.py", b"").unwrap().id, "builtin-python");
        assert_eq!(r.detect("app.pyi", b"").unwrap().id, "builtin-python");
        assert_eq!(r.detect("index.ts", b"").unwrap().id, "builtin-jsts");
        assert_eq!(r.detect("index.tsx", b"").unwrap().id, "builtin-jsts");
        assert_eq!(r.detect("schema.sql", b"").unwrap().id, "builtin-sql");
        assert_eq!(r.detect("lib.rs", b"").unwrap().id, "builtin-rust");
    }

    #[test]
    fn det11_unknown_name_no_mask_hit() {
        let r = reg();
        assert!(r.detect("data.bin", b"plain").is_none(), "无掩码命中且无 shebang");
    }

    // ---- 检测矩阵：shebang 嗅探 ----

    #[test]
    fn det12_shebang_shell_crlf_tolerated() {
        let r = reg();
        let head = b"#!/bin/bash\r\necho hi\n";
        let f = r.detect("runme", head).expect("shebang 兜底");
        assert_eq!(f.id, "builtin-sh", "CRLF 行尾不破坏 shebang 解析");
    }

    #[test]
    fn det13_shebang_env_python() {
        let r = reg();
        let f = r.detect("tool", b"#!/usr/bin/env python3\nprint(1)\n").unwrap();
        assert_eq!(f.id, "builtin-python");
    }

    #[test]
    fn det14_shebang_utf8_bom_tolerated() {
        let r = reg();
        let mut head = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
        head.extend_from_slice(b"#!/bin/sh\n:");
        assert_eq!(r.detect("x", &head).unwrap().id, "builtin-sh", "UTF-8 BOM 剥离后仍识别 shebang");
    }

    #[test]
    fn det15_mask_wins_over_shebang() {
        let r = reg();
        // .py 掩码命中即便 shebang 指向 sh（掩码优先，FR-FMT-001）
        let f = r.detect("script.py", b"#!/bin/sh\n").unwrap();
        assert_eq!(f.id, "builtin-python");
    }

    // ---- 检测矩阵：二进制嗅探 ----

    #[test]
    fn det16_nul_byte_is_binary() {
        let r = reg();
        assert_eq!(r.classify_file("a.txt", b"hi\0there"), FileClass::Binary);
    }

    #[test]
    fn det17_magic_numbers_are_binary() {
        let r = reg();
        for head in [
            &b"%PDF-1.7"[..],
            &b"\x89PNG\r\n"[..],
            &b"GIF89a"[..],
            &b"PK\x03\x04rest"[..],
            &b"\x7fELFbinary"[..],
            &b"MZwin"[..],
        ] {
            assert_eq!(r.classify_file("x", head), FileClass::Binary, "{head:?} 应判二进制");
        }
    }

    #[test]
    fn det18_utf16_bom_is_text_class() {
        let r = reg();
        assert!(
            matches!(r.classify_file("x", &[0xFF, 0xFE, 0x41, 0x00]), FileClass::Text { .. }),
            "UTF-16 BOM 按文本（编码层负责解码）"
        );
    }

    #[test]
    fn det19_plain_text_with_format() {
        let r = reg();
        let cls = r.classify_file("main.c", b"int a;\n");
        match cls {
            FileClass::Text { format } => assert_eq!(format.unwrap().id, "builtin-cpp"),
            other => panic!("应为文本: {other:?}"),
        }
    }

    #[test]
    fn det20_text_without_format_is_none() {
        let r = reg();
        assert!(matches!(
            r.classify_file("notes.xyz", b"hello world"),
            FileClass::Text { format: None }
        ));
    }

    #[test]
    fn det21_disabled_format_skipped() {
        let mut r = FormatRegistry::new();
        r.upsert(FileFormat::text("x1", "甲", &["*.log"], vec![]));
        r.upsert(FileFormat::text("x2", "乙", &["*.log"], vec![]));
        r.formats[0].enabled = false; // 禁用项跳过（FR-FMT-001）
        assert_eq!(r.detect("a.log", b"").unwrap().id, "x2", "首个掩码匹配且启用者生效");
        // 禁用不参与遮蔽
        assert!(r.shadowed_ids().is_empty());
    }

    #[test]
    fn det22_order_first_match_wins() {
        let mut r = FormatRegistry::new();
        r.upsert(FileFormat::text("a", "甲", &["*.txt"], vec![]));
        r.upsert(FileFormat::text("b", "乙", &["*.txt"], vec![]));
        assert_eq!(r.detect("f.txt", b"").unwrap().id, "a", "列表顺序先者生效");
        assert_eq!(r.shadowed_ids(), vec!["b".to_string()], "后者被前者遮蔽（红显）");
    }

    // ---- 语法匹配与 line_classify 钩子 ----

    fn c_grammar() -> Vec<GrammarElement> {
        reg().detect("a.c", b"").unwrap().grammar.clone()
    }

    #[test]
    fn cls23_line_comment_masked() {
        let g = c_grammar();
        let m = unimportant_mask("int a; // trailing", &g, &["注释".into()], None);
        assert_eq!(m, format!("int a; {UNIMPORTANT_MARK}"));
        assert_eq!(line_classify("// whole comment", &g, &["注释".into()], None), LineClass::UnimportantText);
        assert_eq!(line_classify("int a;", &g, &["注释".into()], None), LineClass::Plain);
    }

    #[test]
    fn cls24_block_comment_and_unclosed() {
        let g = c_grammar();
        assert_eq!(
            line_classify("/* c1 */", &g, &["注释".into()], None),
            LineClass::UnimportantText,
            "整行块注释"
        );
        // 未闭合块注释：整行折叠
        assert_eq!(line_classify("int x; /* open", &g, &["注释".into()], None), LineClass::Plain, "代码前缀仍在");
        assert_eq!(
            unimportant_mask("int x; /* open", &g, &["注释".into()], None),
            format!("int x; {UNIMPORTANT_MARK}")
        );
    }

    #[test]
    fn cls25_string_contents_folded() {
        let g = c_grammar();
        let m = unimportant_mask("printf(\"hello %d\");", &g, &["字符串".into()], None);
        assert_eq!(m, format!("printf({UNIMPORTANT_MARK});"), "字符串内容折叠，骨架保留");
        // 两个仅字符串内容不同的行在掩码后等价
        let a = unimportant_mask("s = \"foo\";", &g, &["字符串".into()], None);
        let b = unimportant_mask("s = \"bar\";", &g, &["字符串".into()], None);
        assert_eq!(a, b);
        // 转义引号不终止字符串
        let m2 = unimportant_mask("s = \"a\\\"b\"; x", &g, &["字符串".into()], None);
        assert_eq!(m2, format!("s = {UNIMPORTANT_MARK}; x"));
    }

    #[test]
    fn cls26_element_names_case_insensitive() {
        let g = vec![GrammarElement::new(
            "Comment",
            vec![SyntaxItem::Delimited { start: "//".into(), end: None, escape: None, to_eol: true, regex: false, case_sensitive: false }],
        )];
        assert_eq!(
            line_classify("// note", &g, &["comment".into()], None),
            LineClass::UnimportantText,
            "元素名匹配大小写不敏感"
        );
    }

    #[test]
    fn cls27_unknown_element_noop() {
        let g = c_grammar();
        let m = unimportant_mask("int a; // x", &g, &["不存在".into()], None);
        assert_eq!(m, "int a; // x");
    }

    #[test]
    fn cls28_column_item_masks_fixed_range() {
        let g = vec![GrammarElement::new(
            "行号列",
            vec![SyntaxItem::Column { from: 1, to: Some(4) }],
        )];
        let m = unimportant_mask("  12 code", &g, &["行号列".into()], None);
        assert_eq!(m, format!("{UNIMPORTANT_MARK} code"), "列 1..4 折叠");
    }

    #[test]
    fn cls29_row_title_formfeed_and_first_line() {
        let g = vec![GrammarElement::new(
            "页标题",
            vec![SyntaxItem::RowTitle { pattern: "\\f.*".into(), lines: 2, or_first_line: true }],
        )];
        assert_eq!(line_classify("\u{c}Page 1 of 9", &g, &["页标题".into()], None), LineClass::UnimportantText);
        assert_eq!(line_classify("Report header", &g, &["页标题".into()], Some(0)), LineClass::UnimportantText, "orFirstLine 首行");
        assert_eq!(line_classify("Report header", &g, &["页标题".into()], Some(3)), LineClass::Plain, "非首行不折叠");
    }

    #[test]
    fn cls30_regex_text_item() {
        let g = vec![GrammarElement::new(
            "指令",
            vec![SyntaxItem::Text { pattern: "#.*".into(), regex: true, case_sensitive: false }],
        )];
        assert_eq!(line_classify("#include <x.h>", &g, &["指令".into()], None), LineClass::UnimportantText);
        assert_eq!(line_classify("y = #x", &g, &["指令".into()], None), LineClass::Plain);
    }

    #[test]
    fn cls31_empty_inputs_noop() {
        assert_eq!(unimportant_mask("abc", &[], &["注释".into()], None), "abc");
        assert_eq!(unimportant_mask("abc", &c_grammar(), &[], None), "abc");
    }

    // ---- 格式集导入导出 roundtrip ----

    #[test]
    fn pack32_roundtrip_json() {
        let r = reg();
        let pack = r.to_pack();
        let json = pack.to_json();
        let back = FormatPack::from_json(&json).expect("roundtrip");
        assert_eq!(back, pack);
        assert!(back.formats.len() >= 12, "内置 ≥12 语言，实际 {}", back.formats.len());
    }

    #[test]
    fn pack33_merge_upsert() {
        let mut r = FormatRegistry::new();
        let pack = FormatPack {
            formats: vec![
                FileFormat::text("custom-x", "自定义 X", &["*.x"], vec![]),
                FileFormat::text("custom-y", "自定义 Y", &["*.y"], vec![]),
            ],
            ..FormatPack::default()
        };
        assert_eq!(r.merge_pack(&pack), 2);
        assert_eq!(r.merge_pack(&pack), 0, "按 id 覆盖：第二次无新增");
        assert_eq!(r.detect("f.x", b"").unwrap().id, "custom-x");
    }

    #[test]
    fn pack34_version_guard() {
        let bad = FormatPack { version: 2, ..FormatPack::default() }.to_json();
        assert!(matches!(FormatPack::from_json(&bad), Err(FormatPackError::Version(2))));
        assert!(FormatPack::from_json("not json").is_err());
    }

    // ---- 遮蔽 / 默认值 / serde 形态 ----

    #[test]
    fn pack35_serde_defaults_lenient() {
        let v: FileFormat = serde_json::from_str(r#"{"id":"x"}"#).unwrap();
        assert!(v.enabled, "enabled 缺省 true");
        assert_eq!(v.view_kind, FormatViewKind::Text);
        assert_eq!(v.tab_stops, 4);
    }

    #[test]
    fn pack36_builtin_ids_unique_and_masks_nonempty() {
        let r = reg();
        let mut ids: Vec<_> = r.formats.iter().map(|f| f.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), r.formats.len(), "内置 id 唯一");
        assert!(r.formats.iter().filter(|f| f.masks.is_empty()).count() <= 1, "仅 shebang 兜底格式允许空掩码");
    }
}
