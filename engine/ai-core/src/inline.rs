//! 行内（词级/字符级）差异：懒计算入口 + 长度上限/超时降级（docs/03 §3.1.1、§6.2）。
//!
//! 词级分词：空白与非空白交替成 token（词 + 间隔词），diff 在 token 序列上做
//! Myers，结果映射回字符区间（Unicode scalar 索引，半开）。
//! 超长行（> [`InlineOptions::max_line_chars`]）降级为 `truncated = true`。

use std::time::Instant;

use crate::lines::LineTable;
use crate::myers::{self, RawOp};
use crate::options::{InlineLevel, InlineOptions, TextDiffOptions};
use crate::sections::{InlineState, Section, SectionKind};

/// 字符区间 [start, end)（Unicode scalar 索引）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharSpan {
    pub start: u32,
    pub end: u32,
}

/// 行内区间标注类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlineSpanKind {
    Equal,
    Replace,
    Insert,
    Delete,
}

/// 单行行内区间标注：两侧区间成对出现（None 表示该侧不存在）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineSpan {
    pub left: Option<CharSpan>,
    pub right: Option<CharSpan>,
    pub kind: InlineSpanKind,
}

/// 单行行内 diff 结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineLineResult {
    pub spans: Vec<InlineSpan>,
    /// 超长行/超时降级：spans 为空且调用方应按整行差异渲染（FR-BIG 策略）
    pub truncated: bool,
}

/// Change section 内的一对行及其行内差异
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineLineDiff {
    pub left_line: u32,
    pub right_line: u32,
    pub result: InlineLineResult,
}

/// token：原文上的字符区间（词或空白串）
#[derive(Debug, Clone, Copy)]
struct TokenSpan {
    start: u32,
    end: u32,
}

/// 分词：Word = 空白串/非空白串交替；Char = 逐字符
fn tokenize(text: &str, level: InlineLevel) -> Vec<TokenSpan> {
    match level {
        InlineLevel::Char => (0..text.chars().count() as u32)
            .map(|i| TokenSpan { start: i, end: i + 1 })
            .collect(),
        InlineLevel::Word => {
            let mut tokens = Vec::new();
            let mut chars = text.chars().peekable();
            let mut idx = 0u32;
            while chars.peek().is_some() {
                let is_ws = chars.peek().is_some_and(|c| c.is_whitespace());
                let start = idx;
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() == is_ws {
                        chars.next();
                        idx += 1;
                    } else {
                        break;
                    }
                }
                tokens.push(TokenSpan { start, end: idx });
            }
            tokens
        }
    }
}

fn char_span(tokens: &[TokenSpan], from: usize, len: usize) -> CharSpan {
    CharSpan {
        start: tokens[from].start,
        end: tokens[from + len - 1].end,
    }
}

/// 单行行内 diff（词级或字符级）
pub fn inline_diff_line(left: &str, right: &str, opts: &InlineOptions) -> InlineLineResult {
    let lchars = left.chars().count();
    let rchars = right.chars().count();
    if lchars == 0 && rchars == 0 {
        return InlineLineResult { spans: Vec::new(), truncated: false };
    }
    if lchars > opts.max_line_chars || rchars > opts.max_line_chars {
        return InlineLineResult { spans: Vec::new(), truncated: true };
    }

    let ltoks = tokenize(left, opts.level);
    let rtoks = tokenize(right, opts.level);
    let ltexts: Vec<&str> = ltoks.iter().map(|t| &left[byte_range(left, *t)]).collect();
    let rtexts: Vec<&str> = rtoks.iter().map(|t| &right[byte_range(right, *t)]).collect();

    let started = Instant::now();
    let ops = myers::diff_seq(&ltexts, &rtexts);

    // 超时粗检（兜底）：计算量已被 max_line_chars 上限约束（<= 1001 token），
    // 此处在重计算完成后检查，超限则整体降级为 truncated（与 DMP deadline 语义一致）。
    if opts.timeout_ms > 0 && started.elapsed().as_millis() as u64 > opts.timeout_ms {
        return InlineLineResult { spans: Vec::new(), truncated: true };
    }

    InlineLineResult { spans: spans_from_ops(&ops, &ltoks, &rtoks), truncated: false }
}

/// token 字符区间 -> 字节区间（切片用）
fn byte_range(text: &str, t: TokenSpan) -> std::ops::Range<usize> {
    let start = text
        .char_indices()
        .nth(t.start as usize)
        .map_or(text.len(), |(b, _)| b);
    let end = text
        .char_indices()
        .nth(t.end as usize)
        .map_or(text.len(), |(b, _)| b);
    start..end
}

fn spans_from_ops(ops: &[RawOp], ltoks: &[TokenSpan], rtoks: &[TokenSpan]) -> Vec<InlineSpan> {
    let mut out = Vec::new();
    let (mut li, mut ri) = (0usize, 0usize);
    let mut i = 0usize;
    while i < ops.len() {
        match ops[i] {
            RawOp::Equal(n) => {
                let n = n as usize;
                out.push(InlineSpan {
                    left: Some(char_span(ltoks, li, n)),
                    right: Some(char_span(rtoks, ri, n)),
                    kind: InlineSpanKind::Equal,
                });
                li += n;
                ri += n;
                i += 1;
            }
            RawOp::Delete(d) => {
                let mut d = d as usize;
                let mut ins = 0usize;
                if let Some(RawOp::Insert(m)) = ops.get(i + 1) {
                    ins = *m as usize;
                    i += 1;
                }
                if d == 0 {
                    d = 0;
                }
                if ins > 0 && d > 0 {
                    out.push(InlineSpan {
                        left: Some(char_span(ltoks, li, d)),
                        right: Some(char_span(rtoks, ri, ins)),
                        kind: InlineSpanKind::Replace,
                    });
                } else if d > 0 {
                    out.push(InlineSpan {
                        left: Some(char_span(ltoks, li, d)),
                        right: None,
                        kind: InlineSpanKind::Delete,
                    });
                } else if ins > 0 {
                    out.push(InlineSpan {
                        left: None,
                        right: Some(char_span(rtoks, ri, ins)),
                        kind: InlineSpanKind::Insert,
                    });
                }
                li += d;
                ri += ins;
                i += 1;
            }
            RawOp::Insert(n) => {
                let n = n as usize;
                out.push(InlineSpan {
                    left: None,
                    right: Some(char_span(rtoks, ri, n)),
                    kind: InlineSpanKind::Insert,
                });
                ri += n;
                i += 1;
            }
        }
    }
    out
}

/// 对 Change section 做行内差异（位置配对：第 i 行对第 i 行，多余行忽略）。
/// 返回逐行结果（UI 懒取，docs/03 §2.3.2 text/inlineDiff 语义）。
pub fn inline_diff_section(
    left: &LineTable,
    right: &LineTable,
    section: &Section,
    opts: &TextDiffOptions,
) -> Vec<InlineLineDiff> {
    if section.kind != SectionKind::Change {
        return Vec::new();
    }
    let (Some(l), Some(r)) = (section.left, section.right) else {
        return Vec::new();
    };
    let n = l.len().min(r.len()) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let li = l.start as usize + i;
        let ri = r.start as usize + i;
        let result = inline_diff_line(&left.line(li).text, &right.line(ri).text, &opts.inline);
        if !result.truncated || !result.spans.is_empty() {
            out.push(InlineLineDiff {
                left_line: li as u32,
                right_line: ri as u32,
                result,
            });
        }
    }
    out
}

/// section 行内状态推进（供引擎在懒计算后标记，docs/03 §4.1 InlineState）
pub fn mark_inline(section: &mut Section, truncated: bool) {
    section.inline = if truncated { InlineState::Partial } else { InlineState::Full };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_line_single_equal_span() {
        let r = inline_diff_line("hello world", "hello world", &InlineOptions::default());
        assert!(!r.truncated);
        assert_eq!(r.spans.len(), 1);
        assert_eq!(r.spans[0].kind, InlineSpanKind::Equal);
        assert_eq!(r.spans[0].left, Some(CharSpan { start: 0, end: 11 }));
    }

    #[test]
    fn middle_word_replaced() {
        let r = inline_diff_line("foo bar baz", "foo qux baz", &InlineOptions::default());
        assert!(!r.truncated);
        let kinds: Vec<InlineSpanKind> = r.spans.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec![InlineSpanKind::Equal, InlineSpanKind::Replace, InlineSpanKind::Equal]);
        // "bar" 在字符 4..7
        let rep = r.spans[1];
        assert_eq!(rep.left, Some(CharSpan { start: 4, end: 7 }));
        assert_eq!(rep.right, Some(CharSpan { start: 4, end: 7 }));
    }

    #[test]
    fn pure_insert_span() {
        // 词级分词（空白分词）：无空白单行整体为一个词；插入以词为单位
        let r = inline_diff_line("one two", "one two three", &InlineOptions::default());
        assert_eq!(r.spans.len(), 2);
        assert_eq!(r.spans[0].kind, InlineSpanKind::Equal);
        assert_eq!(r.spans[1].kind, InlineSpanKind::Insert);
        assert_eq!(r.spans[1].left, None);
        // 插入 span 覆盖 " three"（含前导空格 gap token）
        assert_eq!(r.spans[1].right, Some(CharSpan { start: 7, end: 13 }));

        // 无空白整行替换：整行为一个 Replace 词
        let r = inline_diff_line("abc", "abcdef", &InlineOptions::default());
        assert_eq!(r.spans.len(), 1);
        assert_eq!(r.spans[0].kind, InlineSpanKind::Replace);
        assert_eq!(r.spans[0].left, Some(CharSpan { start: 0, end: 3 }));
        assert_eq!(r.spans[0].right, Some(CharSpan { start: 0, end: 6 }));
    }

    #[test]
    fn unicode_char_indices() {
        // 词级：无空白整行为一个词，Replace 覆盖整词（Unicode scalar 计区间）
        let r = inline_diff_line("a🎉b", "a🎈b", &InlineOptions::default());
        let rep = r.spans.iter().find(|s| s.kind == InlineSpanKind::Replace).expect("应存在 Replace");
        assert_eq!(rep.left, Some(CharSpan { start: 0, end: 3 }));
        assert_eq!(rep.right, Some(CharSpan { start: 0, end: 3 }));

        // 字符级：定位到单个字符（emoji 1 个 scalar）
        let opts = crate::options::InlineOptions {
            level: crate::options::InlineLevel::Char,
            ..InlineOptions::default()
        };
        let r = inline_diff_line("a🎉b", "a🎈b", &opts);
        let rep = r.spans.iter().find(|s| s.kind == InlineSpanKind::Replace).expect("应存在 Replace");
        assert_eq!(rep.left, Some(CharSpan { start: 1, end: 2 }));
        assert_eq!(rep.right, Some(CharSpan { start: 1, end: 2 }));
    }

    #[test]
    fn oversized_line_truncated() {
        let big = "x".repeat(2001);
        let r = inline_diff_line(&big, "y", &InlineOptions::default());
        assert!(r.truncated);
        assert!(r.spans.is_empty());
    }

    #[test]
    fn char_level_option() {
        let opts = InlineOptions { level: InlineLevel::Char, ..InlineOptions::default() };
        let r = inline_diff_line("axc", "abc", &opts);
        let rep = r.spans.iter().find(|s| s.kind == InlineSpanKind::Replace).expect("字符级 Replace");
        assert_eq!(rep.left, Some(CharSpan { start: 1, end: 2 }));
    }

    #[test]
    fn section_positional_pairing() {
        let left = LineTable::from_text("alpha 1\nbeta 2\nkeep\n");
        let right = LineTable::from_text("alpha one\nbeta two\nkeep\n");
        let out = crate::diff_text(&left, &right, &TextDiffOptions::default()).unwrap();
        let change = out.sections.iter().find(|s| s.kind == SectionKind::Change).expect("应有 Change");
        let diffs = inline_diff_section(&left, &right, change, &TextDiffOptions::default());
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].left_line, 0);
        assert_eq!(diffs[0].right_line, 0);
        assert!(diffs[0].result.spans.iter().any(|s| s.kind == InlineSpanKind::Replace));
    }
}
