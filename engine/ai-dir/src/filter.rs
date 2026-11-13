//! 过滤器引擎侧求值（FR-DIR-030~036 / FR-FLT-001~002，docs/01 §4.4 全表）。
//!
//! 三类过滤：
//! 1. **名称过滤器**（[`NameFilter`]）：文件/目录掩码，影响比较与操作范围
//!    （FR-DIR-031）。掩码语义全集见 [`MaskRule`]：`?`/`*`/`[az]`/`[a-z]`/
//!    `[!az]`/`[[]`、`*.*` 历史特例、末尾句点（`?`/`*` 不匹配 `.`）、
//!    相对掩码（`p\f` 锚定、`.\f` 基文件夹、`...\f` 任意深度）、`p\` 目录、
//!    `-` 排除、分号多模式；包含文件的父目录链自动包含（FR-FLT-002）。
//! 2. **其他过滤器**（[`OtherFilter`]）：日期/大小/内容/属性/Unix 类型等
//!    非名称条件（FR-DIR-032）。
//! 3. **显示过滤器**（协议 `DisplayFilterSpec`）：只影响显示不影响状态判定，
//!    由 [`crate::session::DirModel`] 在 delta 输出层应用（FR-DIR-033）。
//!
//! 大小写：BC 名称过滤器默认不敏感（`case_sensitive` 可配）。

use ai_protocol::{NameFilterSpec, OtherFilterSpec};
use ai_vfs::dos_attr;

// ---------------------------------------------------------------------------
// 通配符模式（FR-FLT-001）
// ---------------------------------------------------------------------------

/// 通配符匹配一段名字。支持：
/// - `?` 单字符、`*` 任意字符序列（迭代回溯，无递归爆炸）；
/// - `[az]` 集合、`[a-z]` 范围、`[!az]` 补集、`[[]` 字面 `[`；
/// - `*.*` 历史特例：匹配任何文件名（含无扩展名）；
/// - `no_dot`（掩码末尾句点触发）：`?`/`*` 不匹配 `.`，且要求 name 无扩展名。
pub fn wildcard_match(pattern: &str, name: &str, case_sensitive: bool) -> bool {
    if pattern == "*.*" {
        // 历史原因：匹配任何文件名（包括无扩展名）
        return true;
    }
    let no_dot = pattern.ends_with('.');
    let pattern = if no_dot { &pattern[..pattern.len() - 1] } else { pattern };
    if no_dot && name.contains('.') {
        return false;
    }
    if !case_sensitive {
        // 大小写折叠按字符进行（ASCII 与常见 Unicode 简单折叠足够）
        let p: Vec<char> = pattern.chars().flat_map(|c| c.to_lowercase()).collect();
        let n: Vec<char> = name.chars().flat_map(|c| c.to_lowercase()).collect();
        return wildcard_match_chars(&p, &n, no_dot);
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    wildcard_match_chars(&p, &n, no_dot)
}

fn wildcard_match_chars(pat: &[char], name: &[char], no_dot: bool) -> bool {
    let (mut pi, mut ni) = (0usize, 0usize);
    let mut star_pi: Option<usize> = None;
    let mut star_ni = 0usize;
    while ni < name.len() {
        if pi < pat.len() && pat[pi] == '*' {
            star_pi = Some(pi);
            star_ni = ni;
            pi += 1;
            continue;
        }
        if pi < pat.len() {
            if let Some(width) = match_one(&pat[pi..], name[ni], no_dot) {
                pi += width;
                ni += 1;
                continue;
            }
        }
        match star_pi {
            Some(sp) => {
                // 回溯：'*' 多吞一个字符（no_dot 时不能吞 '.'）
                if no_dot && name[star_ni] == '.' {
                    return false;
                }
                pi = sp + 1;
                star_ni += 1;
                ni = star_ni;
            }
            None => return false,
        }
    }
    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}

/// 尝试让 pattern[0..] 匹配单个字符 ch；返回消耗的 pattern 宽度。
/// `[...]` 类按完整类体解析宽度；无闭合 `]` 时 `[` 作字面字符。
fn match_one(pat: &[char], ch: char, no_dot: bool) -> Option<usize> {
    match pat[0] {
        '?' => {
            if no_dot && ch == '.' {
                None
            } else {
                Some(1)
            }
        }
        '[' => {
            // `[[]` -> 字面 '['
            if pat.len() >= 3 && pat[1] == '[' && pat[2] == ']' {
                return (ch == '[').then_some(3);
            }
            let mut i = 1usize;
            let negated = pat.get(i) == Some(&'!');
            if negated {
                i += 1;
            }
            let mut first = true;
            let mut hit = false;
            while i < pat.len() {
                let c = pat[i];
                if c == ']' && !first {
                    // 类体结束（宽度含 ']'）
                    return if hit != negated { Some(i + 1) } else { None };
                }
                first = false;
                // 范围 a-z（'-' 后不是 ']' 时为范围）
                if i + 2 < pat.len() && pat[i + 1] == '-' && pat[i + 2] != ']' {
                    if c <= ch && ch <= pat[i + 2] {
                        hit = true;
                    }
                    i += 3;
                } else {
                    if c == ch {
                        hit = true;
                    }
                    i += 1;
                }
            }
            // 无闭合 ']'：'[' 按字面处理
            (ch == '[').then_some(1)
        }
        c => (c == ch).then_some(1),
    }
}

// ---------------------------------------------------------------------------
// 结构化掩码规则（FR-FLT-002 简单与相对掩码）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskTarget {
    File,
    Dir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaskScope {
    /// 单段名：任意深度的 basename 匹配（`*.c`、`f`、`p\`）
    Name,
    /// 锚定基文件夹的精确深度路径（`p\f`、`.\f`）
    Anchored(Vec<String>),
    /// 任意深度后缀路径（`...\f`、`...\p\f`）
    AnyDepth(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskRule {
    pub exclude: bool,
    pub target: MaskTarget,
    pub scope: MaskScope,
    /// Name 作用域的掩码文本（Anchored/AnyDepth 的段文本存于 scope 内）
    pub pattern: String,
}

impl MaskRule {
    /// 解析一条掩码（分号段内；工具栏快捷语法：前导 `-` 排除、尾 `\` 目录）
    pub fn parse(raw: &str) -> MaskRule {
        let mut s = raw.trim().to_string();
        let exclude = s.starts_with('-');
        if exclude {
            s = s[1..].to_string();
        }
        // 尾部分隔符 -> 目录
        let mut target = MaskTarget::File;
        while s.ends_with('/') || s.ends_with('\\') {
            target = MaskTarget::Dir;
            s.pop();
        }
        let s = s.replace('\\', "/");
        // 相对语义前缀
        let mut scope = MaskScope::Name;
        let mut rest = s.as_str();
        let name_part = rest.to_string();
        if let Some(r) = rest.strip_prefix(".../") {
            scope = MaskScope::AnyDepth(split_segments(r));
            rest = "";
        } else if let Some(r) = rest.strip_prefix("./") {
            scope = MaskScope::Anchored(split_segments(r));
            rest = "";
        } else if rest.contains('/') {
            scope = MaskScope::Anchored(split_segments(rest));
            rest = "";
        }
        if !rest.is_empty() {
            scope = MaskScope::Name;
        }
        let pattern = match &scope {
            MaskScope::Name => name_part,
            _ => String::new(),
        };
        MaskRule { exclude, target, scope, pattern }
    }

    /// 按目标与路径段匹配（segments 为基文件夹下的相对段序列）
    pub fn matches(&self, target: MaskTarget, segments: &[&str], case_sensitive: bool) -> bool {
        if self.target != target {
            return false;
        }
        let basename = segments.last().copied().unwrap_or("");
        match &self.scope {
            MaskScope::Name => wildcard_match(&self.pattern, basename, case_sensitive),
            MaskScope::Anchored(segs) => {
                segs.len() == segments.len()
                    && segs.iter().zip(segments.iter()).all(|(p, s)| wildcard_match(p, s, case_sensitive))
            }
            MaskScope::AnyDepth(segs) => {
                segments.len() >= segs.len()
                    && segs
                        .iter()
                        .zip(segments[segments.len() - segs.len()..].iter())
                        .all(|(p, s)| wildcard_match(p, s, case_sensitive))
            }
        }
    }
}

fn split_segments(s: &str) -> Vec<String> {
    s.split('/').filter(|seg| !seg.is_empty()).map(|seg| seg.to_string()).collect()
}

// ---------------------------------------------------------------------------
// NameFilter：文件名掩码 / 目录掩码（FR-DIR-031）
// ---------------------------------------------------------------------------

/// 名称过滤器（四列表 + 工具栏快捷语法；掩码按分号展开为规则序列）
#[derive(Debug, Clone, Default)]
pub struct NameFilter {
    rules: Vec<MaskRule>,
    case_sensitive: bool,
}

impl NameFilter {
    pub fn from_spec(spec: &NameFilterSpec) -> Self {
        let case_sensitive = spec.case_sensitive.unwrap_or(false);
        let mut rules = Vec::new();
        let push_list = |list: &[String], exclude: bool, target: MaskTarget, rules: &mut Vec<MaskRule>| {
            for item in list {
                // 每项可为分号分隔的多模式（FR-DIR-031 工具栏语法）
                for seg in item.split(';') {
                    let seg = seg.trim();
                    if seg.is_empty() {
                        continue;
                    }
                    let mut rule = MaskRule::parse(seg);
                    // 列表类型强制 target（会话四列表语义）
                    rule.target = target;
                    rule.exclude = exclude || rule.exclude;
                    rules.push(rule);
                }
            }
        };
        push_list(&spec.include_files, false, MaskTarget::File, &mut rules);
        push_list(&spec.include_dirs, false, MaskTarget::Dir, &mut rules);
        push_list(&spec.exclude_files, true, MaskTarget::File, &mut rules);
        push_list(&spec.exclude_dirs, true, MaskTarget::Dir, &mut rules);
        NameFilter { rules, case_sensitive }
    }

    /// 从工具栏快捷语法构造（如 `*.c;*.h;-*.md;-p\`）：前导 `-` 为排除
    pub fn parse_shortcut(list: &str, case_sensitive: bool) -> Self {
        let mut rules = Vec::new();
        for seg in list.split(';') {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            rules.push(MaskRule::parse(seg));
        }
        NameFilter { rules, case_sensitive }
    }

    fn rules_for(&self, exclude: bool, target: MaskTarget) -> impl Iterator<Item = &MaskRule> {
        self.rules.iter().filter(move |r| r.exclude == exclude && r.target == target)
    }

    /// 文件是否纳入比较范围（FR-FLT-002：排除优先；包含文件的父目录链自动包含）
    pub fn file_allowed(&self, segments: &[&str]) -> bool {
        // 1. 处于被排除的目录子树内
        for depth in 1..=segments.len() {
            let prefix = &segments[..depth];
            if self.rules_for(true, MaskTarget::Dir).any(|r| r.matches(MaskTarget::Dir, prefix, self.case_sensitive)) {
                return false;
            }
        }
        // 2. 文件自身被排除（Name 按basename；Anchored/AnyDepth 按全路径）
        if self.rules_for(true, MaskTarget::File).any(|r| r.matches(MaskTarget::File, segments, self.case_sensitive)) {
            return false;
        }
        // 3. 包含判定
        let include_files = self.rules_for(false, MaskTarget::File).count();
        let include_dirs = self.rules_for(false, MaskTarget::Dir).count();
        if include_files == 0 && include_dirs == 0 {
            return true;
        }
        // 自身命中文件 include，或任一祖先命中目录 include
        self.rules_for(false, MaskTarget::File).any(|r| r.matches(MaskTarget::File, segments, self.case_sensitive))
            || (1..segments.len()).any(|depth| {
                let prefix = &segments[..depth];
                self.rules_for(false, MaskTarget::Dir)
                    .any(|r| r.matches(MaskTarget::Dir, prefix, self.case_sensitive))
            })
    }

    /// 目录子树是否被显式排除（扫描剪枝：true = 整棵跳过，不 list_dir）
    pub fn dir_pruned(&self, segments: &[&str]) -> bool {
        for depth in 1..=segments.len() {
            let prefix = &segments[..depth];
            if self.rules_for(true, MaskTarget::Dir).any(|r| r.matches(MaskTarget::Dir, prefix, self.case_sensitive)) {
                return true;
            }
        }
        // 文件排除掩码也可能精确命中该目录路径（Anchored 全路径排除）
        self.rules_for(true, MaskTarget::File)
            .any(|r| r.matches(MaskTarget::File, segments, self.case_sensitive))
    }

    /// 目录行是否由名称过滤器直接可见（无 include 约束时恒可见；
    /// 有 include 时须自身/祖先命中——父链自动包含，FR-FLT-002）
    pub fn dir_visible(&self, segments: &[&str]) -> bool {
        if self.dir_pruned(segments) {
            return false;
        }
        let include_files = self.rules_for(false, MaskTarget::File).count();
        let include_dirs = self.rules_for(false, MaskTarget::Dir).count();
        if include_files == 0 && include_dirs == 0 {
            return true;
        }
        self.rules_for(false, MaskTarget::Dir).any(|r| r.matches(MaskTarget::Dir, segments, self.case_sensitive))
            || (1..segments.len()).any(|depth| {
                let prefix = &segments[..depth];
                self.rules_for(false, MaskTarget::Dir)
                    .any(|r| r.matches(MaskTarget::Dir, prefix, self.case_sensitive))
            })
    }

    /// 调试用：规则视图（不属于稳定 API）
    #[doc(hidden)]
    pub fn rules_debug(&self) -> &[MaskRule] {
        &self.rules
    }

    pub fn has_includes(&self) -> bool {
        self.rules.iter().any(|r| !r.exclude)
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

// ---------------------------------------------------------------------------
// OtherFilter：非名称条件（FR-DIR-032）
// ---------------------------------------------------------------------------

/// 其他过滤器（文件须满足全部出现的条件；目录由子内容聚合）
#[derive(Debug, Clone, Default)]
pub struct OtherFilter {
    spec: OtherFilterSpec,
}

/// 内容匹配的读取上限（contains 过滤；BC 无明确上限，1MB 足够内容 sniff）
const CONTENT_SCAN_LIMIT: usize = 1024 * 1024;

impl OtherFilter {
    pub fn from_spec(spec: OtherFilterSpec) -> Self {
        OtherFilter { spec }
    }

    pub fn spec(&self) -> &OtherFilterSpec {
        &self.spec
    }

    /// 文件是否通过全部条件。stat 为该文件元数据；内容由 vfs 按需读取。
    pub async fn allows(
        &self,
        vfs: &dyn ai_vfs::VfsProvider,
        path: &ai_vfs::VfsPath,
        stat: &ai_vfs::StatInfo,
    ) -> bool {
        let s = &self.spec;
        // 受保护 OS 文件（默认 true）：Windows 页面文件等 + Unix 近似（root 专属无读权）
        if s.exclude_protected_os_files.unwrap_or(true) {
            if matches!(path.file_name(), Some(n) if matches!(n.to_ascii_lowercase().as_str(), "pagefile.sys" | "hiberfil.sys" | "swapfile.sys")) {
                return false;
            }
            if stat.uid == Some(0) {
                if let Some(mode) = stat.unix_mode {
                    if mode & 0o004 == 0 {
                        return false;
                    }
                }
            }
        }
        if let Some(min) = s.min_mtime_ms {
            if stat.mtime_ms < min {
                return false;
            }
        }
        if let Some(max) = s.max_mtime_ms {
            if stat.mtime_ms > max {
                return false;
            }
        }
        if let Some(min) = s.min_size {
            if stat.size < min {
                return false;
            }
        }
        if let Some(max) = s.max_size {
            if stat.size > max {
                return false;
            }
        }
        // DOS 属性位
        if let Some(letters) = &s.attrs_include {
            let mask = dos_attr::from_letters(letters);
            if !letters.is_empty() && stat.dos_attrs & mask != mask {
                return false;
            }
        }
        if let Some(letters) = &s.attrs_exclude {
            let mask = dos_attr::from_letters(letters);
            if mask != 0 && stat.dos_attrs & mask != 0 {
                return false;
            }
        }
        // Unix 类型字母（b/c/l/p/r/s；d 目录不在 BC 集合内）
        if let Some(letters) = &s.unix_types_include {
            let t = unix_type_of(stat);
            if !letters.chars().any(|c| c.eq_ignore_ascii_case(&t)) {
                return false;
            }
        }
        if let Some(letters) = &s.unix_types_exclude {
            let t = unix_type_of(stat);
            if letters.chars().any(|c| c.eq_ignore_ascii_case(&t)) {
                return false;
            }
        }
        // 内容包含/不包含（按需读取前 1MB）
        if s.contains.is_some() || s.not_contains.is_some() {
            // 读取失败按"不包含"处理（不因 IO 中断扫描）
            let bytes = vfs.read_file_bytes(path).await.unwrap_or_default();
            let bytes = &bytes[..bytes.len().min(CONTENT_SCAN_LIMIT)];
            let has = |needle: &str| contains_bytes(bytes, needle.as_bytes());
            if let Some(needle) = &s.contains {
                if !has(needle) {
                    return false;
                }
            }
            if let Some(needle) = &s.not_contains {
                if has(needle) {
                    return false;
                }
            }
        }
        true
    }

    pub fn is_empty(&self) -> bool {
        let s = &self.spec;
        s.min_mtime_ms.is_none()
            && s.max_mtime_ms.is_none()
            && s.min_size.is_none()
            && s.max_size.is_none()
            && s.contains.is_none()
            && s.not_contains.is_none()
            && s.attrs_include.is_none()
            && s.attrs_exclude.is_none()
            && s.unix_types_include.is_none()
            && s.unix_types_exclude.is_none()
    }
}

fn unix_type_of(stat: &ai_vfs::StatInfo) -> char {
    ai_vfs::unix_type_letter(stat.kind, stat.unix_mode)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 通配符语义（FR-FLT-001 全表） ----

    #[test]
    fn question_and_star() {
        let cs = true;
        assert!(wildcard_match("?", "a", cs));
        assert!(!wildcard_match("?", "ab", cs));
        assert!(wildcard_match("*", "anything", cs));
        assert!(wildcard_match("*", "", cs));
        assert!(wildcard_match("a*c", "abc", cs));
        assert!(wildcard_match("a*c", "ac", cs));
        assert!(!wildcard_match("a*c", "abxc", cs) || wildcard_match("a*c", "abxc", cs)); // * 吞多字符
        assert!(wildcard_match("a*c", "abxc", cs));
        assert!(!wildcard_match("abc", "abd", cs));
        // 大小写不敏感（默认）
        assert!(wildcard_match("ABC", "abc", false));
        assert!(!wildcard_match("ABC", "abc", true));
    }

    #[test]
    fn char_classes() {
        let cs = true;
        assert!(wildcard_match("[az]", "a", cs));
        assert!(wildcard_match("[az]", "z", cs));
        assert!(!wildcard_match("[az]", "b", cs));
        assert!(wildcard_match("[a-z]", "m", cs));
        assert!(!wildcard_match("[a-z]", "M", cs));
        assert!(!wildcard_match("[!az]", "a", cs));
        assert!(wildcard_match("[!az]", "b", cs));
        // [[] 字面 '['
        assert!(wildcard_match("[[]", "[", cs));
        assert!(!wildcard_match("[[]", "a", cs));
        // 嵌入式类
        assert!(wildcard_match("x[0-9]y", "x5y", cs));
        assert!(!wildcard_match("x[0-9]y", "xay", cs));
    }

    #[test]
    fn star_dot_star_matches_everything() {
        let cs = true;
        assert!(wildcard_match("*.*", "readme", cs), "历史特例：*.* 匹配无扩展名");
        assert!(wildcard_match("*.*", "a.b", cs));
        assert!(wildcard_match("*.*", ".gitignore", cs));
    }

    #[test]
    fn trailing_period_matches_extension_less_only() {
        let cs = true;
        assert!(wildcard_match("*.", "readme", cs));
        assert!(!wildcard_match("*.", "readme.txt", cs));
        assert!(wildcard_match("filename.", "filename", cs));
        assert!(!wildcard_match("filename.", "filename.txt", cs));
        // no_dot 下 ?/* 不吃 '.'
        assert!(wildcard_match("?.", "a", cs));
        assert!(!wildcard_match("?.", "a.", cs));
        assert!(!wildcard_match("?.", ".a", cs));
    }

    // ---- 相对掩码与名称过滤（FR-FLT-002 / FR-DIR-031） ----

    fn parse_list(s: &str) -> Vec<MaskRule> {
        s.split(';').filter(|x| !x.trim().is_empty()).map(MaskRule::parse).collect()
    }

    fn rule_matches(rule: &MaskRule, is_dir: bool, path: &str, cs: bool) -> bool {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        rule.matches(if is_dir { MaskTarget::Dir } else { MaskTarget::File }, &segs, cs)
    }

    #[test]
    fn simple_and_folder_masks() {
        let rules = parse_list("f;p\\;-g");
        // f -> 文件 f（任意深度名匹配）
        assert!(rule_matches(&rules[0], false, "f", true));
        assert!(rule_matches(&rules[0], false, "d/f", true));
        assert!(!rule_matches(&rules[0], true, "f", true), "f 不匹配目录");
        // p\ -> 目录 p（任意深度）
        assert!(rule_matches(&rules[1], true, "p", true));
        assert!(rule_matches(&rules[1], true, "a/p", true));
        assert!(!rule_matches(&rules[1], false, "p", true));
        // -g 排除
        assert!(rules[2].exclude);
    }

    #[test]
    fn relative_masks() {
        let rules = parse_list("p\\f;.\\f;...\\f;...\\src\\*.c");
        // p\f：f 在（基下）p 内 —— 锚定 2 段
        assert!(rule_matches(&rules[0], false, "p/f", true));
        assert!(!rule_matches(&rules[0], false, "a/p/f", true), "p\\f 锚定基文件夹");
        assert!(!rule_matches(&rules[0], false, "f", true));
        // .\f：基文件夹内一层
        assert!(rule_matches(&rules[1], false, "f", true));
        assert!(!rule_matches(&rules[1], false, "sub/f", true));
        // ...\f：任意层级下的 f
        assert!(rule_matches(&rules[2], false, "f", true));
        assert!(rule_matches(&rules[2], false, "a/b/f", true));
        // ...\src\*.c：任意深度 src 内 .c
        assert!(rule_matches(&rules[3], false, "src/x.c", true));
        assert!(rule_matches(&rules[3], false, "a/src/y.c", true));
        assert!(!rule_matches(&rules[3], false, "src/x.h", true));
    }

    fn spec(files_include: &[&str], files_exclude: &[&str], dirs_include: &[&str], dirs_exclude: &[&str]) -> NameFilter {
        NameFilter::from_spec(&NameFilterSpec {
            include_files: files_include.iter().map(|s| s.to_string()).collect(),
            exclude_files: files_exclude.iter().map(|s| s.to_string()).collect(),
            include_dirs: dirs_include.iter().map(|s| s.to_string()).collect(),
            exclude_dirs: dirs_exclude.iter().map(|s| s.to_string()).collect(),
            case_sensitive: None,
        })
    }

    fn segs(path: &str) -> Vec<&str> {
        path.split('/').filter(|s| !s.is_empty()).collect()
    }

    #[test]
    fn name_filter_default_all_and_exclusion() {
        // 无 include：全放行；排除项移除
        let f = spec(&[], &["*.md", "-dummy"], &[], &[".git\\"]);
        assert!(f.file_allowed(&segs("a.c")));
        assert!(!f.file_allowed(&segs("README.md")));
        assert!(!f.file_allowed(&segs("src/notes.md")));
        assert!(!f.file_allowed(&segs("x/README.md")), "-*.md 单段列出即整体排除语义（列表按行）");
    }

    #[test]
    fn name_filter_include_requires_match_and_parent_chain_auto_included() {
        let f = spec(&["*.c"], &[], &[], &[]);
        assert!(f.file_allowed(&segs("a.c")));
        assert!(f.file_allowed(&segs("deep/nested/a.c")));
        assert!(!f.file_allowed(&segs("a.h")));
        // 目录可见性：仅 include 文件掩码时目录行不由名称过滤器直接可见
        // —— 会话层结合"后代存在被包含文件"判定（FR-DIR-034，见 session::dir_has_included_descendant）
        assert!(!f.dir_visible(&segs("src")));
        assert!(!f.dir_pruned(&segs("src")), "不被剪枝：子内容可能命中 include");
    }

    #[test]
    fn name_filter_include_dir_pulls_whole_subtree() {
        let f = spec(&[], &[], &["src\\"], &[]);
        assert!(f.dir_visible(&segs("src")));
        assert!(f.dir_visible(&segs("src/inner")), "包含文件夹 => 其内子目录可见（内容全包含）");
        assert!(f.file_allowed(&segs("src/a.txt")), "包含的文件夹内文件自动包含");
        assert!(f.file_allowed(&segs("src/inner/a.txt")));
        assert!(!f.dir_visible(&segs("docs")));
        assert!(!f.file_allowed(&segs("docs/a.txt")));
    }

    #[test]
    fn name_filter_exclude_dir_prunes_subtree() {
        let f = spec(&[], &[], &[], &[".git\\", "build/tmp\\"]);
        assert!(f.dir_pruned(&segs(".git")));
        assert!(f.dir_pruned(&segs("a/.git")), ".git 目录名任意深度剪枝");
        assert!(f.dir_pruned(&segs("a/.git/objects")));
        assert!(f.dir_pruned(&segs("build/tmp")));
        assert!(!f.dir_pruned(&segs("build")));
        assert!(!f.dir_pruned(&segs("src")));
        // 排除子树内文件一律不可见
        assert!(!f.file_allowed(&segs(".git/config")));
    }

    #[test]
    fn name_filter_exclude_wins_over_include() {
        let f = spec(&["*.c"], &["generated_*"], &[], &[]);
        assert!(f.file_allowed(&segs("main.c")));
        assert!(!f.file_allowed(&segs("generated_main.c")), "排除优先于包含");
    }

    #[test]
    fn name_filter_shortcut_toolbar_syntax() {
        let f = NameFilter::parse_shortcut("*.c;*.h;-*.md;-p\\", false);
        assert!(f.file_allowed(&segs("a.c")));
        assert!(f.file_allowed(&segs("a.h")));
        assert!(!f.file_allowed(&segs("a.md")));
        assert!(f.dir_pruned(&segs("p")));
    }

    // ---- OtherFilter（FR-DIR-032） ----

    #[tokio::test]
    async fn other_filter_conditions() {
        use ai_vfs::{LocalFs, VfsPath, StatInfo, EntryKind};
        use ai_vfs::VfsProvider as _;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hello needle world").unwrap();
        let fs = LocalFs::new();
        let root = VfsPath::parse(dir.path().to_str().unwrap()).unwrap();
        let p = root.join("a.txt");
        let st = fs.stat(&p).await.unwrap();

        // 大小条件
        let of = OtherFilter::from_spec(OtherFilterSpec { min_size: Some(1), max_size: Some(1000), ..Default::default() });
        assert!(of.allows(&fs, &p, &st).await);
        let of = OtherFilter::from_spec(OtherFilterSpec { max_size: Some(5), ..Default::default() });
        assert!(!of.allows(&fs, &p, &st).await);

        // 内容条件
        let of = OtherFilter::from_spec(OtherFilterSpec { contains: Some("needle".into()), ..Default::default() });
        assert!(of.allows(&fs, &p, &st).await);
        let of = OtherFilter::from_spec(OtherFilterSpec { not_contains: Some("needle".into()), ..Default::default() });
        assert!(!of.allows(&fs, &p, &st).await);

        // Unix 类型
        let of = OtherFilter::from_spec(OtherFilterSpec { unix_types_include: Some("r".into()), ..Default::default() });
        assert!(of.allows(&fs, &p, &st).await);
        let of = OtherFilter::from_spec(OtherFilterSpec { unix_types_exclude: Some("r".into()), ..Default::default() });
        assert!(!of.allows(&fs, &p, &st).await);

        // DOS 属性（readonly 位）
        let ro = StatInfo { dos_attrs: dos_attr::READONLY, ..st };
        let of = OtherFilter::from_spec(OtherFilterSpec { attrs_include: Some("r".into()), ..Default::default() });
        assert!(of.allows(&fs, &p, &ro).await);
        let of = OtherFilter::from_spec(OtherFilterSpec { attrs_exclude: Some("r".into()), ..Default::default() });
        assert!(!of.allows(&fs, &p, &ro).await);
        assert_eq!(EntryKind::File, EntryKind::File);
    }
}
