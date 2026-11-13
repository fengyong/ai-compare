//! 行分割与行表（LineTable）。
//!
//! 每行保留：行文本（不含行尾字符）+ 行尾风格。`to_text` 严格还原原文，
//! 因此比较可以忽略行尾风格而不丢失保存时还原所需的信息（docs/03 §6.1）。

use std::borrow::Cow;

use crate::options::TextDiffOptions;

/// 行尾风格
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
    /// 文件末行无换行符
    None,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
            LineEnding::None => "",
        }
    }
}

/// 单行：文本 + 行尾风格
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineEntry {
    pub text: String,
    pub ending: LineEnding,
}

/// 行表：一次文本载入的行数组视图（P0 内存态；mmap LineIndex 为 P1+ 优化）
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LineTable {
    lines: Vec<LineEntry>,
}

impl LineTable {
    /// 从文本构建行表。空文本 -> 0 行。
    pub fn from_text(text: &str) -> Self {
        let mut lines = Vec::new();
        let bytes = text.as_bytes();
        let mut start = 0usize;
        let mut i = 0usize;
        while i < bytes.len() {
            let b = bytes[i];
            if b == b'\n' {
                lines.push(LineEntry {
                    text: text[start..i].to_string(),
                    ending: LineEnding::Lf,
                });
                i += 1;
                start = i;
            } else if b == b'\r' {
                let ending = if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    i += 1;
                    LineEnding::CrLf
                } else {
                    LineEnding::Cr
                };
                lines.push(LineEntry {
                    text: text[start..(i - if ending == LineEnding::CrLf { 1 } else { 0 })].to_string(),
                    ending,
                });
                i += 1;
                start = i;
            } else {
                i += 1;
            }
        }
        if start < text.len() {
            lines.push(LineEntry {
                text: text[start..].to_string(),
                ending: LineEnding::None,
            });
        }
        LineTable { lines }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// 第 i 行（0 起）
    pub fn line(&self, i: usize) -> &LineEntry {
        &self.lines[i]
    }

    /// 行区间文本（半开区间）
    pub fn lines_range(&self, start: usize, end: usize) -> &[LineEntry] {
        &self.lines[start..end]
    }

    pub fn iter(&self) -> std::slice::Iter<'_, LineEntry> {
        self.lines.iter()
    }

    /// 严格还原原文（行文本 + 行尾拼接）
    pub fn to_text(&self) -> String {
        let mut out = String::with_capacity(
            self.lines.iter().map(|l| l.text.len() + l.ending.as_str().len()).sum(),
        );
        for l in &self.lines {
            out.push_str(&l.text);
            out.push_str(l.ending.as_str());
        }
        out
    }

    /// 行区间替换（编辑增量重 diff 的行表落盘）：
    /// `[start, end)` 行被 `new_text` 按行分割后的内容替换。
    ///
    /// 行尾修正：拼接结果若不在文件末尾且新内容末行缺行尾符，则继承被替换
    /// 首行的行尾风格（缺省 LF），保证文件仍可无损还原为规整文本。
    pub fn replace_lines(&mut self, start: usize, end: usize, new_text: &str) {
        let entries = LineTable::from_text(new_text).lines;
        self.replace_lines_entries(start, end, entries);
    }

    /// [`Self::replace_lines`] 的行条目版本（避免重复解析）。
    pub fn replace_lines_entries(&mut self, start: usize, end: usize, mut new_lines: Vec<LineEntry>) {
        assert!(start <= end && end <= self.lines.len(), "replace_lines 区间越界");
        // 行尾修正：中段拼接时末行必须带行尾
        if end < self.lines.len() {
            if let Some(last) = new_lines.last_mut() {
                if last.ending == LineEnding::None {
                    last.ending = self.lines.get(start).map_or(LineEnding::Lf, |old| old.ending);
                    if last.ending == LineEnding::None {
                        last.ending = LineEnding::Lf;
                    }
                }
            }
        }
        self.lines.splice(start..end, new_lines);
    }

    /// 规范化比较键：diff 在键上运算、在原文上渲染（docs/03 §3.1.1）。
    ///
    /// - `ignore_eol_style`（默认 true）：行尾风格不参与比较；
    /// - `ignore_case`：Unicode 简单大小写折叠；
    /// - `whitespace`：三类空白语义（有效 / 忽略数量 / 忽略全部）。
    ///
    /// 默认选项下返回借用键，零分配。
    pub fn compare_key(&self, i: usize, opts: &TextDiffOptions) -> Cow<'_, str> {
        self.line(i).compare_key(opts)
    }
}

impl LineEntry {
    /// 规范化比较键（见 [`LineTable::compare_key`]）
    pub fn compare_key(&self, opts: &TextDiffOptions) -> Cow<'_, str> {
        let needs_eol = !opts.ignore_eol_style && self.ending != LineEnding::Lf;
        let needs_ws = opts.whitespace != crate::options::WhitespaceMode::Significant;
        if !opts.ignore_case && !needs_ws && !needs_eol {
            return Cow::Borrowed(self.text.as_str());
        }
        let ws = apply_whitespace(self.text.as_str(), opts.whitespace);
        let cased = if opts.ignore_case { ws.to_lowercase() } else { ws };
        if needs_eol {
            Cow::Owned(format!("{cased}\u{0}{}", eol_tag(self.ending)))
        } else {
            Cow::Owned(cased)
        }
    }

    /// 简单 FNV-1a 文本哈希（行内快速判等辅助；比较仍以键相等为准）
    pub fn text_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in self.text.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }
}

fn eol_tag(e: LineEnding) -> &'static str {
    match e {
        LineEnding::Lf => "lf",
        LineEnding::CrLf => "crlf",
        LineEnding::Cr => "cr",
        LineEnding::None => "none",
    }
}

/// 空白三类语义（FR-TXT-010）：有效 / 忽略数量（连续空白折叠为一个空格）/
/// 忽略全部（移除所有空白）
fn apply_whitespace(text: &str, mode: crate::options::WhitespaceMode) -> String {
    use crate::options::WhitespaceMode as W;
    match mode {
        W::Significant => text.to_string(),
        W::IgnoreAll => text.chars().filter(|c| !c.is_whitespace()).collect(),
        W::IgnoreAmount => {
            let mut out = String::with_capacity(text.len());
            let mut in_ws = false;
            for c in text.chars() {
                if c.is_whitespace() {
                    if !in_ws {
                        out.push(' ');
                        in_ws = true;
                    }
                } else {
                    out.push(c);
                    in_ws = false;
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> LineTable {
        LineTable::from_text(text)
    }

    #[test]
    fn empty_text_has_zero_lines() {
        assert_eq!(t("").len(), 0);
        assert!(t("").is_empty());
        assert_eq!(t("").to_text(), "");
    }

    #[test]
    fn single_line_without_newline() {
        let lt = t("abc");
        assert_eq!(lt.len(), 1);
        assert_eq!(lt.line(0).text, "abc");
        assert_eq!(lt.line(0).ending, LineEnding::None);
        assert_eq!(lt.to_text(), "abc");
    }

    #[test]
    fn newline_variants_preserved() {
        let lt = t("a\r\nb\nc\rd\n");
        assert_eq!(lt.len(), 4);
        assert_eq!(lt.line(0).ending, LineEnding::CrLf);
        assert_eq!(lt.line(1).ending, LineEnding::Lf);
        assert_eq!(lt.line(2).ending, LineEnding::Cr);
        assert_eq!(lt.line(3).text, "d");
        assert_eq!(lt.line(3).ending, LineEnding::Lf);
        assert_eq!(lt.to_text(), "a\r\nb\nc\rd\n");
    }

    #[test]
    fn lone_newline_is_empty_line() {
        let lt = t("\n");
        assert_eq!(lt.len(), 1);
        assert_eq!(lt.line(0).text, "");
        assert_eq!(lt.line(0).ending, LineEnding::Lf);
    }

    #[test]
    fn bare_cr_at_eof() {
        let lt = t("x\ry");
        assert_eq!(lt.len(), 2);
        assert_eq!(lt.line(0).ending, LineEnding::Cr);
        assert_eq!(lt.line(1).ending, LineEnding::None);
    }

    #[test]
    fn roundtrip_identity() {
        for s in ["", "\n", "a", "a\n", "a\r\nb", "\r\n", "a\n\n\nb\r\r\n", "Ünïcödé\r\n中文\n"] {
            assert_eq!(t(s).to_text(), s, "roundtrip failed for {s:?}");
        }
    }

    #[test]
    fn compare_key_whitespace_modes() {
        let mut opts = crate::options::TextDiffOptions::default();
        let a = LineEntry { text: "a  b".into(), ending: LineEnding::Lf };
        let b = LineEntry { text: "a b".into(), ending: LineEnding::Lf };
        assert_ne!(a.compare_key(&opts), b.compare_key(&opts));
        opts.whitespace = crate::options::WhitespaceMode::IgnoreAmount;
        assert_eq!(a.compare_key(&opts), b.compare_key(&opts));
        opts.whitespace = crate::options::WhitespaceMode::IgnoreAll;
        let c = LineEntry { text: "ab".into(), ending: LineEnding::Lf };
        assert_eq!(a.compare_key(&opts), c.compare_key(&opts));
    }

    #[test]
    fn compare_key_case_and_eol() {
        let mut opts = crate::options::TextDiffOptions::default();
        let lower = LineEntry { text: "hello".into(), ending: LineEnding::Lf };
        let upper = LineEntry { text: "Hello".into(), ending: LineEnding::Lf };
        assert_ne!(lower.compare_key(&opts), upper.compare_key(&opts));
        opts.ignore_case = true;
        assert_eq!(lower.compare_key(&opts), upper.compare_key(&opts));

        let crlf = LineEntry { text: "x".into(), ending: LineEnding::CrLf };
        let lf = LineEntry { text: "x".into(), ending: LineEnding::Lf };
        assert_eq!(crlf.compare_key(&opts), lf.compare_key(&opts), "默认忽略行尾风格");
        opts.ignore_eol_style = false;
        assert_ne!(crlf.compare_key(&opts), lf.compare_key(&opts));
    }

    #[test]
    fn replace_lines_splices_and_roundtrips() {
        let mut lt = t("a\nb\nc\nd\n");
        lt.replace_lines(1, 3, "X1\nX2\nX3\n");
        assert_eq!(lt.to_text(), "a\nX1\nX2\nX3\nd\n");
        assert_eq!(lt.len(), 5);

        // 纯插入（start == end）
        let mut lt = t("a\nb\n");
        lt.replace_lines(1, 1, "mid\n");
        assert_eq!(lt.to_text(), "a\nmid\nb\n");

        // 纯删除（new_text 为空）
        let mut lt = t("a\nb\nc\n");
        lt.replace_lines(1, 2, "");
        assert_eq!(lt.to_text(), "a\nc\n");
    }

    #[test]
    fn replace_lines_fixes_midfile_missing_ending() {
        // 中段拼接时新内容末行缺行尾 -> 继承被替换首行的行尾，保证可无损还原
        let mut lt = t("a\r\nb\r\nc\r\n");
        lt.replace_lines(1, 2, "X"); // 缺行尾
        assert_eq!(lt.to_text(), "a\r\nX\r\nc\r\n");
    }

    #[test]
    #[should_panic]
    fn replace_lines_out_of_bounds_panics() {
        let mut lt = t("a\n");
        lt.replace_lines(0, 5, "x\n");
    }

    #[test]
    fn default_options_borrowed_key_no_alloc() {
        let opts = crate::options::TextDiffOptions::default();
        let lt = t("plain line");
        assert!(matches!(lt.compare_key(0, &opts), Cow::Borrowed(_)));
    }
}
