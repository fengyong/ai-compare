//! 重要性规范化双键（docs/03 §4.1：raw_key / importance_key；FR-TXT-010）。
//!
//! 设计要点（docs/03 §4.1 原文）："重要性在规范化键阶段已留下双键：
//! raw_key / importance_key"。diff 骨架始终在 **raw 键**上计算；**importance 键**
//! 在 raw 语义基础上再折叠"不重要"方面（空白三类/大小写/行尾）。因此：
//!
//! - 切换重要性规则（或显示层"忽略不重要差异"）**不重跑 diff、不改变行对齐骨架**，
//!   只按 importance 键重判 section 重要性（`SectionEvent::ImportanceUpdate`）；
//! - 孤儿行（Insert/Delete section）始终很重要（FR-TXT-010：额外空行或仅含
//!   不重要文本的行视为重要差异）。

use std::borrow::Cow;

use crate::lines::{LineEntry, LineTable};
use crate::options::{ImportanceOptions, TextDiffOptions};
use crate::sections::{DiffOutput, Importance, Section, SectionKind};

/// 一行的双键：raw（骨架键）+ importance（重要性键）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineKeys {
    pub raw: String,
    pub importance: String,
}

/// raw 键（骨架键）：P0 `LineEntry::compare_key` 语义——
/// 空白模式/大小写按骨架选项折叠，行尾风格按 `ignore_eol_style` 决定是否纳入。
pub fn raw_key<'a>(line: &'a LineEntry, opts: &TextDiffOptions) -> Cow<'a, str> {
    line.compare_key(opts)
}

/// importance 键：在 raw 文本上再折叠"不重要"方面（FR-TXT-010）。
///
/// - P10 语法折叠（`opts.rules`）：不重要语法元素（注释/字符串等）的匹配
///   跨度先折叠为占位符（FR-FMT-003 × FR-TXT-010；`line_index` 供页标题
///   "或第 1 行"判定）；
/// - 行首/嵌入/行尾空白各自可折叠；
/// - 大小写折叠（Unicode 简单小写）；
/// - 行尾风格标记仅在"骨架比较行尾 且 行尾重要"时纳入（与 raw 键一致地携带）。
pub fn importance_key<'a>(
    line: &'a LineEntry,
    opts: &TextDiffOptions,
    imp: &ImportanceOptions,
    line_index: Option<usize>,
) -> Cow<'a, str> {
    let text = line.text.as_str();
    let needs_ws = imp.leading_ws_unimportant || imp.embedded_ws_unimportant || imp.trailing_ws_unimportant;
    let needs_case = imp.case_unimportant;
    let needs_eol = !opts.ignore_eol_style && !imp.eol_style_unimportant && line.ending != crate::lines::LineEnding::Lf;
    let needs_grammar = opts.rules.has_folding();
    if !needs_ws && !needs_case && !needs_eol && !needs_grammar {
        return Cow::Borrowed(text);
    }
    // P10 语法折叠先行（产出 owned 文本），再走空白/大小写折叠
    let mut owned = if needs_grammar {
        opts.rules.fold_line(text, line_index)
    } else {
        String::from(text)
    };
    if needs_ws {
        owned = fold_whitespace(&owned, imp);
    }
    if needs_case {
        owned = owned.to_lowercase();
    }
    if needs_eol {
        owned.push('\u{0}');
        owned.push_str(eol_tag(line.ending));
    }
    Cow::Owned(owned)
}

fn fold_whitespace(text: &str, imp: &ImportanceOptions) -> String {
    // 无任何折叠时零分配快速路径
    if !imp.leading_ws_unimportant && !imp.trailing_ws_unimportant && !imp.embedded_ws_unimportant {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut start = 0usize;
    let mut end = chars.len();
    if imp.leading_ws_unimportant {
        while start < end && chars[start].is_whitespace() {
            start += 1;
        }
    }
    if imp.trailing_ws_unimportant {
        while end > start && chars[end - 1].is_whitespace() {
            end -= 1;
        }
    }
    let mut out = String::with_capacity(end - start);
    if imp.embedded_ws_unimportant {
        let mut in_ws = false;
        for &c in &chars[start..end] {
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
    } else {
        out.extend(chars[start..end].iter());
    }
    out
}

fn eol_tag(e: crate::lines::LineEnding) -> &'static str {
    match e {
        crate::lines::LineEnding::Lf => "lf",
        crate::lines::LineEnding::CrLf => "crlf",
        crate::lines::LineEnding::Cr => "cr",
        crate::lines::LineEnding::None => "none",
    }
}

/// 两侧全量双键（一次分配；O(行数)）
pub fn compute_keys(
    left: &LineTable,
    right: &LineTable,
    opts: &TextDiffOptions,
    imp: &ImportanceOptions,
) -> (Vec<LineKeys>, Vec<LineKeys>) {
    fn table_keys(table: &LineTable, opts: &TextDiffOptions, imp: &ImportanceOptions) -> Vec<LineKeys> {
        (0..table.len())
            .map(|i| {
                let entry = table.line(i);
                LineKeys {
                    raw: raw_key(entry, opts).into_owned(),
                    importance: importance_key(entry, opts, imp, Some(i)).into_owned(),
                }
            })
            .collect()
    }
    (table_keys(left, opts, imp), table_keys(right, opts, imp))
}

/// 按 importance 键对 section 重判重要性（不重跑 diff —— docs/03 §4.1）。
///
/// 规则（FR-TXT-010）：
/// - Equal：不是差异，恒 [`Importance::Important`]；
/// - Change：左右行数相等且所有配对行 importance 键相同 → [`Importance::Unimportant`]，
///   否则 Important（含行数不等的孤儿情形）；
/// - Insert/Delete：孤儿行始终 Important；
/// - 手动忽略区间（`cfg.manual.ignore`）重叠的 section → [`Importance::Ignored`]（最高优先）。
///
/// 返回每个 section 的重判结果（下标 = section 下标）。
pub fn classify_sections(
    sections: &[Section],
    left: &LineTable,
    right: &LineTable,
    opts: &TextDiffOptions,
    imp: &ImportanceOptions,
    manual: &crate::options::ManualAlignOptions,
) -> Vec<Importance> {
    let mut result = Vec::with_capacity(sections.len());
    for sec in sections {
        let mut classification = match sec.kind {
            SectionKind::Equal => Importance::Important,
            SectionKind::Change => {
                let (Some(l), Some(r)) = (sec.left, sec.right) else {
                    unreachable!("Change section 必有两侧区间");
                };
                let mut unimportant = l.len() == r.len();
                if unimportant {
                    for k in 0..l.len() {
                        let le = left.line((l.start + k) as usize);
                        let re = right.line((r.start + k) as usize);
                        let li = Some((l.start + k) as usize);
                        let ri = Some((r.start + k) as usize);
                        if importance_key(le, opts, imp, li) != importance_key(re, opts, imp, ri) {
                            unimportant = false;
                            break;
                        }
                    }
                }
                if unimportant {
                    Importance::Unimportant
                } else {
                    Importance::Important
                }
            }
            // 孤儿行始终很重要（FR-TXT-010）
            SectionKind::Insert | SectionKind::Delete => Importance::Important,
        };
        // 手动忽略（最高优先级）
        let overlaps_any = |ranges: &[(crate::options::DiffSide, crate::sections::LineRange)], probe: Option<crate::sections::LineRange>| {
            let Some(probe) = probe else { return false };
            ranges.iter().any(|(_, r)| {
                match r.start < probe.end && probe.start < r.end {
                    true => true,
                    false => false,
                }
            })
        };
        if overlaps_any(&manual.ignore, sec.left) || overlaps_any(&manual.ignore, sec.right) {
            classification = Importance::Ignored;
        }
        result.push(classification);
    }
    result
}

/// 把重判结果写回 sections（返回发生变化的 section id 列表，供
/// `SectionEvent::ImportanceUpdate` 使用）。
pub fn apply_classification(sections: &mut [Section], classification: &[Importance]) -> Vec<crate::sections::SectionId> {
    let mut changed = Vec::new();
    for (sec, imp) in sections.iter_mut().zip(classification.iter()) {
        if sec.important != *imp {
            sec.important = *imp;
            changed.push(sec.id);
        }
    }
    changed
}

/// "忽略不重要差异"聚合视图（FR-TXT-010）：把不重要 Change 折叠为 Equal，
/// 并与相邻 Equal 合并；**行覆盖集与原 sections 完全一致（骨架不变）**。
///
/// 重要差异（含孤儿 Delete/Insert、Important/Ignored）原样保留并打断合并。
pub fn aggregate_unimportant(sections: &[Section]) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for sec in sections {
        let foldable = matches!(sec.important, Importance::Unimportant) && sec.kind == SectionKind::Change;
        if foldable {
            // 折叠为 Equal（左右行数相等是 Unimportant 的前提）
            let left = sec.left.expect("Unimportant Change 必有左侧区间");
            let right = sec.right.expect("Unimportant Change 必有右侧区间");
            let as_equal = Section { kind: SectionKind::Equal, left: Some(left), right: Some(right), ..*sec };
            merge_or_push(&mut out, as_equal);
        } else {
            out.push(*sec);
        }
    }
    // 合并相邻 Equal（覆盖连续）
    let mut merged: Vec<Section> = Vec::new();
    for sec in out {
        merge_or_push(&mut merged, sec);
    }
    merged
}

fn merge_or_push(out: &mut Vec<Section>, sec: Section) {
    if sec.kind == SectionKind::Equal {
        if let Some(last) = out.last_mut() {
            if last.kind == SectionKind::Equal {
                if let (Some(ll), Some(lrr)) = (last.left, sec.left) {
                    debug_assert_eq!(ll.end, lrr.start, "Equal 区间必须连续");
                    last.left = Some(crate::sections::LineRange::new(ll.start, lrr.end));
                }
                if let (Some(lr), Some(rr)) = (last.right, sec.right) {
                    debug_assert_eq!(lr.end, rr.start, "Equal 区间必须连续");
                    last.right = Some(crate::sections::LineRange::new(lr.start, rr.end));
                }
                return;
            }
        }
    }
    out.push(sec);
}

/// keyed 输出：骨架输出 + 每 section 双键证据（docs/04 P1 交付物 ②：
/// "diff 结果每 section 带 raw 与 importance 两键"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyedDiffOutput {
    /// 骨架输出（section 序列 + 统计；重要性已按规则重判写回）
    pub output: DiffOutput,
    /// 每个差异 section 的双键证据（Equal section 不含）
    pub section_keys: Vec<SectionKeyInfo>,
    /// 不重要 section 计数
    pub unimportant_sections: u32,
}

/// 单个 section 的双键证据：涉及差异的行逐行给出 (raw, importance)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionKeyInfo {
    pub id: crate::sections::SectionId,
    pub importance: Importance,
    /// 差异行键（Change 按左侧行序与右侧配对；Delete 仅左；Insert 仅右）
    pub lines: Vec<LineKeyPair>,
}

/// 一对差异行的双键
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineKeyPair {
    pub left: Option<LineKeys>,
    pub right: Option<LineKeys>,
    /// 本行对是否"仅不重要方面不同"（importance 键相等）
    pub unimportant: bool,
}

/// 组装 keyed 输出（在骨架 sections + 两侧行表 + 双键齐备后调用）
pub(crate) fn build_keyed_output(
    output: DiffOutput,
    left: &LineTable,
    right: &LineTable,
    opts: &TextDiffOptions,
    imp: &ImportanceOptions,
    manual: &crate::options::ManualAlignOptions,
) -> KeyedDiffOutput {
    let classification = classify_sections(&output.sections, left, right, opts, imp, manual);
    let mut sections = output.sections;
    apply_classification(&mut sections, &classification);

    let mut section_keys = Vec::new();
    let mut unimportant_sections = 0u32;
    for sec in &sections {
        if sec.kind == SectionKind::Equal {
            continue;
        }
        if sec.important == Importance::Unimportant {
            unimportant_sections += 1;
        }
        let mut lines = Vec::new();
        match (sec.left, sec.right) {
            (Some(l), Some(r)) => {
                // Change：按位置配对（多余的一侧为孤儿行，unimportant = false）
                let n = l.len().max(r.len());
                for k in 0..n {
                    let lk = if k < l.len() {
                        Some(line_keys(left, (l.start + k) as usize, opts, imp))
                    } else {
                        None
                    };
                    let rk = if k < r.len() {
                        Some(line_keys(right, (r.start + k) as usize, opts, imp))
                    } else {
                        None
                    };
                    let unimportant = matches!((&lk, &rk), (Some(a), Some(b)) if a.importance == b.importance);
                    lines.push(LineKeyPair { left: lk, right: rk, unimportant });
                }
            }
            (Some(l), None) => {
                for i in l.start..l.end {
                    lines.push(LineKeyPair {
                        left: Some(line_keys(left, i as usize, opts, imp)),
                        right: None,
                        unimportant: false,
                    });
                }
            }
            (None, Some(r)) => {
                for i in r.start..r.end {
                    lines.push(LineKeyPair {
                        left: None,
                        right: Some(line_keys(right, i as usize, opts, imp)),
                        unimportant: false,
                    });
                }
            }
            (None, None) => {}
        }
        section_keys.push(SectionKeyInfo { id: sec.id, importance: sec.important, lines });
    }

    KeyedDiffOutput {
        output: DiffOutput { sections, summary: output.summary },
        section_keys,
        unimportant_sections,
    }
}

fn line_keys(table: &LineTable, i: usize, opts: &TextDiffOptions, imp: &ImportanceOptions) -> LineKeys {
    let entry = table.line(i);
    LineKeys {
        raw: raw_key(entry, opts).into_owned(),
        importance: importance_key(entry, opts, imp, Some(i)).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{DiffSide, ManualAlignOptions};
    use crate::sections::LineRange;

    fn entry(text: &str) -> LineEntry {
        LineEntry { text: text.into(), ending: crate::lines::LineEnding::Lf }
    }

    #[test]
    fn importance_key_whitespace_folds() {
        let mut imp = ImportanceOptions::default();
        let opts = TextDiffOptions::default();
        let a = entry("  alpha  beta  ");
        let b = entry("alpha beta");
        // 默认：全部重要 → 键即原文，不同
        assert_ne!(importance_key(&a, &opts, &imp, None), importance_key(&b, &opts, &imp, None));

        imp.trailing_ws_unimportant = true;
        assert_ne!(importance_key(&a, &opts, &imp, None), importance_key(&b, &opts, &imp, None), "行首空白仍重要");

        imp.leading_ws_unimportant = true;
        assert_ne!(importance_key(&a, &opts, &imp, None), importance_key(&b, &opts, &imp, None), "嵌入空白仍重要");

        imp.embedded_ws_unimportant = true;
        assert_eq!(importance_key(&a, &opts, &imp, None), importance_key(&b, &opts, &imp, None));

        // 嵌入折叠语义：连续空白 -> 单空格
        let c = entry("alpha   beta");
        assert_eq!(importance_key(&c, &opts, &imp, None), importance_key(&b, &opts, &imp, None));
    }

    #[test]
    fn importance_key_case_fold() {
        let opts = TextDiffOptions::default();
        let mut imp = ImportanceOptions::default();
        let a = entry("Hello");
        let b = entry("hello");
        assert_ne!(importance_key(&a, &opts, &imp, None), importance_key(&b, &opts, &imp, None));
        imp.case_unimportant = true;
        assert_eq!(importance_key(&a, &opts, &imp, None), importance_key(&b, &opts, &imp, None));
    }

    #[test]
    fn importance_key_eol_tag() {
        let mut opts = TextDiffOptions::default();
        let mut imp = ImportanceOptions::default();
        let mut crlf = entry("x");
        crlf.ending = crate::lines::LineEnding::CrLf;
        let lf = entry("x");
        // 默认：骨架忽略行尾 + 行尾不重要 → 无标记
        assert_eq!(importance_key(&crlf, &opts, &imp, None), importance_key(&lf, &opts, &imp, None));
        // 骨架比较行尾（raw 键带标记），但行尾仍不重要 → importance 键无标记
        opts.ignore_eol_style = false;
        assert_ne!(raw_key(&crlf, &opts), raw_key(&lf, &opts));
        assert_eq!(importance_key(&crlf, &opts, &imp, None), importance_key(&lf, &opts, &imp, None));
        // 行尾重要 → importance 键带标记
        imp.eol_style_unimportant = false;
        assert_ne!(importance_key(&crlf, &opts, &imp, None), importance_key(&lf, &opts, &imp, None));
    }

    #[test]
    fn classify_change_unimportant_vs_orphan() {
        let left = LineTable::from_text("Alpha  \nreal\n");
        let right = LineTable::from_text("alpha\nREAL\n");
        let mut opts = TextDiffOptions::default();
        let mut imp = ImportanceOptions::default();
        // 第一对仅大小写+行尾空白不同；第二对大小写不同
        let out = crate::diff_text(&left, &right, &opts).unwrap();
        assert_eq!(out.sections.len(), 1, "骨架：单 Change");
        // 未开启折叠 → Important
        let cls = classify_sections(&out.sections, &left, &right, &opts, &imp, &ManualAlignOptions::default());
        assert_eq!(cls[0], Importance::Important);
        // 开启大小写+行尾空白折叠 → Unimportant
        imp.case_unimportant = true;
        imp.trailing_ws_unimportant = true;
        let cls = classify_sections(&out.sections, &left, &right, &opts, &imp, &ManualAlignOptions::default());
        assert_eq!(cls[0], Importance::Unimportant);
        opts.whitespace = crate::options::WhitespaceMode::IgnoreAll;
        let _ = opts; // 保留骨架选项不变（骨架不受 importance 影响）
        let _ = LineTable::from_text("");
    }

    #[test]
    fn orphan_insert_is_important_even_if_blank() {
        // 额外空行（孤儿）始终重要（FR-TXT-010）
        let left = LineTable::from_text("a\n\n");
        let right = LineTable::from_text("a\n\n\n");
        let opts = TextDiffOptions::default();
        let imp = ImportanceOptions::default();
        let out = crate::diff_text(&left, &right, &opts).unwrap();
        assert_eq!(out.sections.len(), 2, "空行骨架差异：Equal + Insert");
        let cls = classify_sections(&out.sections, &left, &right, &opts, &imp, &ManualAlignOptions::default());
        assert_eq!(cls[1], Importance::Important, "孤儿行始终重要");
    }

    #[test]
    fn manual_ignore_marks_ignored() {
        let left = LineTable::from_text("a\nb\nc\n");
        let right = LineTable::from_text("a\nx\nc\n");
        let opts = TextDiffOptions::default();
        let imp = ImportanceOptions::default();
        let out = crate::diff_text(&left, &right, &opts).unwrap();
        let manual = ManualAlignOptions {
            ignore: vec![(DiffSide::Left, LineRange::new(1, 2))],
            ..ManualAlignOptions::default()
        };
        let cls = classify_sections(&out.sections, &left, &right, &opts, &imp, &manual);
        assert_eq!(cls[1], Importance::Ignored);
    }

    #[test]
    fn aggregate_preserves_line_coverage() {
        let left = LineTable::from_text("h\none\ntail\nx\ntail2\n");
        let right = LineTable::from_text("h\nOne\ntail\nx\ntail2\n");
        let opts = TextDiffOptions::default();
        let imp = ImportanceOptions { case_unimportant: true, ..ImportanceOptions::default() };
        let keyed = crate::diff_config(&left, &right, &crate::options::DiffConfig {
            base: opts.clone(),
            importance: imp,
            manual: ManualAlignOptions::default(),
        })
        .unwrap();
        assert!(keyed.unimportant_sections >= 1, "大小写差异应为不重要");

        let before = &keyed.output.sections;
        let after = aggregate_unimportant(before);
        // 骨架不变：左侧行覆盖集（归一化为不相交区间并集）一致
        let norm = |secs: &[Section]| -> Vec<(u32, u32)> {
            let mut pts: Vec<(u32, u32)> = secs.iter().filter_map(|s| s.left.map(|r| (r.start, r.end))).collect();
            pts.sort_unstable();
            let mut out: Vec<(u32, u32)> = Vec::new();
            for (a, b) in pts {
                match out.last_mut() {
                    Some(last) if last.1 >= a => last.1 = last.1.max(b),
                    _ => out.push((a, b)),
                }
            }
            out
        };
        assert_eq!(norm(before), norm(&after), "聚合不得改变行覆盖骨架");
        assert_eq!(norm(before), vec![(0, 5)], "语料应全量覆盖 5 行");
        // 折叠后无 Unimportant Change 残留
        assert!(after.iter().all(|s| !(s.kind == SectionKind::Change && s.important == Importance::Unimportant)));
    }

    #[test]
    fn keyed_output_carries_both_keys() {
        let left = LineTable::from_text("Alpha\nkeep\n");
        let right = LineTable::from_text("alpha \nkeep\n");
        let imp = ImportanceOptions {
            trailing_ws_unimportant: true,
            case_unimportant: true,
            ..ImportanceOptions::default()
        };
        let keyed = crate::diff_config(&left, &right, &crate::options::DiffConfig {
            base: TextDiffOptions::default(),
            importance: imp,
            manual: ManualAlignOptions::default(),
        })
        .unwrap();
        assert_eq!(keyed.output.sections.len(), 2, "骨架：Change + Equal");
        assert_eq!(keyed.unimportant_sections, 1);
        assert_eq!(keyed.section_keys.len(), 1, "只有差异 section 有键证据");
        let info = &keyed.section_keys[0];
        assert_eq!(info.importance, Importance::Unimportant);
        assert_eq!(info.lines.len(), 1);
        let pair = &info.lines[0];
        assert_ne!(pair.left.as_ref().unwrap().raw, pair.right.as_ref().unwrap().raw, "raw 键不同");
        assert_eq!(pair.left.as_ref().unwrap().importance, pair.right.as_ref().unwrap().importance, "importance 键相同");
        assert!(pair.unimportant);
    }
}
