//! ai-core 场景测试（docs/04 P0 测试清单：空文件/单行/全等/全异/移动块/
//! 行内差异/超大行/CRLF 混合/无尾换行/Unicode + 补丁重放等价性质）。

use crate::options::TextDiffOptions;
use crate::sections::{SectionEvent, SectionKind};
use crate::{diff_stream, diff_text, LineTable};

fn diff(a: &str, b: &str) -> crate::DiffOutput {
    diff_text(&LineTable::from_text(a), &LineTable::from_text(b), &TextDiffOptions::default()).unwrap()
}

/// 补丁重放等价：按 section 应用补丁必须精确还原右侧文本（对拍性质的内部形态）
fn assert_patch_roundtrip(a: &str, b: &str) {
    let left = LineTable::from_text(a);
    let right = LineTable::from_text(b);
    let out = diff(a, b);
    let mut rebuilt: Vec<String> = Vec::new();
    for s in &out.sections {
        match s.kind {
            SectionKind::Equal => {
                for i in s.left.unwrap().start..s.left.unwrap().end {
                    rebuilt.push(left.line(i as usize).text.clone());
                }
            }
            SectionKind::Change | SectionKind::Insert => {
                for i in s.right.unwrap().start..s.right.unwrap().end {
                    rebuilt.push(right.line(i as usize).text.clone());
                }
            }
            SectionKind::Delete => {}
        }
    }
    let expected: Vec<String> = right.iter().map(|l| l.text.clone()).collect();
    assert_eq!(rebuilt, expected, "补丁重放应还原右侧文本：{a:?} -> {b:?}");
}

#[test]
fn empty_vs_empty() {
    let out = diff("", "");
    assert!(out.sections.is_empty());
    assert_eq!(out.summary.section_count, 0);
}

#[test]
fn empty_vs_content_is_insert() {
    let out = diff("", "a\nb\n");
    assert_eq!(out.sections.len(), 1);
    assert_eq!(out.sections[0].kind, SectionKind::Insert);
    assert_eq!(out.summary.inserted_lines, 2);

    let out = diff("a\nb\n", "");
    assert_eq!(out.sections.len(), 1);
    assert_eq!(out.sections[0].kind, SectionKind::Delete);
}

#[test]
fn single_line_equal_and_changed() {
    let out = diff("hello\n", "hello\n");
    assert_eq!(out.sections.len(), 1);
    assert_eq!(out.sections[0].kind, SectionKind::Equal);

    let out = diff("hello\n", "hell\n");
    assert_eq!(out.sections.len(), 1);
    assert_eq!(out.sections[0].kind, SectionKind::Change);
}

#[test]
fn all_equal_single_section() {
    let text = "l1\nl2\nl3\nl4\nl5\n";
    let out = diff(text, text);
    assert_eq!(out.sections.len(), 1);
    assert_eq!(out.sections[0].kind, SectionKind::Equal);
    assert_eq!(out.summary.equal_lines, 5);
}

#[test]
fn all_different_becomes_change() {
    let a: String = (0..10).map(|i| format!("left {i}\n")).collect();
    let b: String = (0..10).map(|i| format!("right {i}\n")).collect();
    let out = diff(&a, &b);
    assert_eq!(out.sections.len(), 1);
    assert_eq!(out.sections[0].kind, SectionKind::Change);
    assert_eq!(out.summary.deleted_lines, 10);
    assert_eq!(out.summary.inserted_lines, 10);
}

#[test]
fn moved_block_roundtrip() {
    let a = "1\n2\n3\n4\n5\n6\n7\n8\n";
    let b = "1\n6\n7\n2\n3\n4\n5\n8\n";
    let out = diff(a, b);
    // 段必须连续覆盖两侧行号（性质：区间无缝隙）
    let mut left_expect = 0u32;
    let mut right_expect = 0u32;
    for s in &out.sections {
        if let Some(l) = s.left {
            assert_eq!(l.start, left_expect);
            left_expect = l.end;
        }
        if let Some(r) = s.right {
            assert_eq!(r.start, right_expect);
            right_expect = r.end;
        }
    }
    assert_eq!(left_expect, 8);
    assert_eq!(right_expect, 8);
    assert_patch_roundtrip(a, b);
}

#[test]
fn inline_diff_within_changed_section() {
    let a = "foo bar baz\nsecond\n";
    let b = "foo qux baz\nsecond\n";
    let left = LineTable::from_text(a);
    let right = LineTable::from_text(b);
    let out = diff(a, b);
    let change = out.sections.iter().find(|s| s.kind == SectionKind::Change).expect("应有 Change");
    let inline = crate::inline_diff_section(&left, &right, change, &TextDiffOptions::default());
    assert_eq!(inline.len(), 1);
    let spans = &inline[0].result.spans;
    assert!(spans.iter().any(|s| s.kind == crate::InlineSpanKind::Replace), "应含词级替换: {spans:?}");
}

#[test]
fn huge_line_diffs_and_inline_truncates() {
    let big = "x".repeat(2_000_000);
    let a = format!("head\n{big}\ntail\n");
    let b = format!("head\n{big}y\ntail\n");
    let out = diff(&a, &b);
    assert_eq!(out.sections.len(), 3);
    assert_eq!(out.sections[1].kind, SectionKind::Change);

    // 行内 diff 对超大行降级
    let r = crate::inline_diff_line(&big, &format!("{big}y"), &TextDiffOptions::default().inline);
    assert!(r.truncated);
}

#[test]
fn mixed_crlf_ignored_by_default() {
    let a = "a\r\nb\nc\r\n";
    let b = "a\nb\r\nc\n";
    let out = diff(a, b);
    assert_eq!(out.sections.len(), 1);
    assert_eq!(out.sections[0].kind, SectionKind::Equal);

    // 打开行尾比较后应产生差异
    let opts = TextDiffOptions { ignore_eol_style: false, ..TextDiffOptions::default() };
    let out = diff_text(&LineTable::from_text(a), &LineTable::from_text(b), &opts).unwrap();
    assert!(out.summary.change_sections + out.summary.delete_sections + out.summary.insert_sections >= 1);
}

#[test]
fn no_trailing_newline() {
    // 默认忽略行尾风格：b 结尾有无 \n 不构成差异
    let out = diff("a\nb", "a\nb\n");
    assert_eq!(out.sections.len(), 1);
    assert_eq!(out.sections[0].kind, SectionKind::Equal);

    let out = diff("a\nb", "a\nc");
    assert_eq!(out.sections.len(), 2);
    assert_eq!(out.sections[0].kind, SectionKind::Equal);
    assert_eq!(out.sections[1].kind, SectionKind::Change);
    assert_eq!(out.sections[1].left.unwrap().start, 1);
}

#[test]
fn unicode_lines_and_inline() {
    let a = "中文第一行\nemoji 🎉🎉 line\nÜnïcödé\n";
    let b = "中文第二行\nemoji 🎊🎉 line\nÜnïcödé\n";
    let out = diff(a, b);
    // 两行不同但相邻 -> 单个 Change section（含 2+2 行）
    assert_eq!(out.summary.change_sections, 1);
    assert_eq!(out.summary.deleted_lines, 2);
    assert_eq!(out.summary.inserted_lines, 2);
    assert_eq!(out.summary.equal_lines, 1);
    assert_patch_roundtrip(a, b);

    // CJK 词级差异：无空白整行为一个词，Replace 覆盖整词（Unicode scalar 计）
    let r = crate::inline_diff_line("中文第一行", "中文第二行", &TextDiffOptions::default().inline);
    let rep = r.spans.iter().find(|s| s.kind == crate::InlineSpanKind::Replace).unwrap();
    assert_eq!(rep.left, Some(crate::CharSpan { start: 0, end: 5 }));
    assert_eq!(rep.right, Some(crate::CharSpan { start: 0, end: 5 }));
}

#[test]
fn stream_events_append_then_done() {
    let left = LineTable::from_text("1\n2\n3\n");
    let right = LineTable::from_text("1\nX\n3\n");
    let mut events = Vec::new();
    let summary = diff_stream(&left, &right, &TextDiffOptions::default(), &mut |e| events.push(e)).unwrap();
    assert!(matches!(events.last(), Some(SectionEvent::Done(_))));
    assert_eq!(events.len(), 4); // Append(Equal) + Append(Change) + Append(Equal) + Done
    assert!(matches!(&events[0], SectionEvent::Append(s) if s.kind == SectionKind::Equal));
    assert!(matches!(&events[1], SectionEvent::Append(s) if s.kind == SectionKind::Change));
    assert!(matches!(&events[2], SectionEvent::Append(s) if s.kind == SectionKind::Equal));
    assert!(matches!(&events[3], SectionEvent::Done(_)));
    assert_eq!(summary.change_sections, 1);
}

#[test]
fn roundtrip_property_over_variants() {
    let cases = [
        ("", "x"),
        ("a", "b"),
        ("same\nsame\n", "same\nsame\n"),
        ("1\n2\n3\n4\n5\n", "1\n3\n4\n2\n5\n"),
        ("only\nleft\nlines\n", "only\nright\nlines\nhere\n"),
        ("tab\there\n", "tab\there  \n"),
    ];
    for (a, b) in cases {
        assert_patch_roundtrip(a, b);
    }
}
