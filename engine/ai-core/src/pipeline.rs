//! diff 装配管线（docs/04 P1 交付物 ①③ 的共享骨架）。
//!
//! 流程：比较键（含隔离哨兵）→ 手动锚点分段 → 逐段算法 diff（+
//! 接近度匹配）→ section 装配 → 重要性重判 → 从不对齐拆分 → 相邻 Equal 合并。
//!
//! - [`run_range`]：在行表的任意窗口上运行（全量坐标进出，供增量重算复用）；
//! - [`run_full`]：全文件一次性入口（Standard 经 [`crate::progressive`] 渐进实现）。

use std::borrow::Cow;

use crate::keys::{self, KeyedDiffOutput};
use crate::lines::LineTable;
use crate::manual;
use crate::myers::{self, RawOp};
use crate::options::{DiffConfig, DiffSide, LineAlignAlgo, ManualAlignOptions, TextDiffOptions};
use crate::patience;
use crate::sections::{
    build_section_vec, merge_adjacent_equal, renumber, DiffError, Importance, LineRange, Section,
    SectionKind,
};

/// 一个算法在键序列上产出编辑脚本。
pub(crate) fn algo_ops(
    algo: LineAlignAlgo,
    a_keys: &[Cow<'_, str>],
    b_keys: &[Cow<'_, str>],
) -> Vec<RawOp> {
    match algo {
        LineAlignAlgo::Myers => myers::diff_seq(a_keys, b_keys),
        LineAlignAlgo::Patience => patience::diff_seq(a_keys, b_keys),
        LineAlignAlgo::Histogram => {
            let ar: Vec<&str> = a_keys.iter().map(|c| c.as_ref()).collect();
            let br: Vec<&str> = b_keys.iter().map(|c| c.as_ref()).collect();
            crate::histogram::diff_keys(&ar, &br)
        }
        // Standard（一次性形态）：唯一行锚点分块 + 逐块 Myers（FR-TXT-011
        // "比较每个文件的连续较小部分"；渐进交付形态见 progressive 模块）
        LineAlignAlgo::Standard => blocked_standard_ops(a_keys, b_keys),
    }
}

/// Standard 一次性：唯一行锚点（patience LIS）分块，块间 Myers。
pub(crate) fn blocked_standard_ops(a_keys: &[Cow<'_, str>], b_keys: &[Cow<'_, str>]) -> Vec<RawOp> {
    let anchors = match patience::unique_anchor_lis(a_keys, b_keys) {
        None => return myers::diff_seq(a_keys, b_keys),
        Some(pairs) => pairs,
    };
    let mut ops: Vec<RawOp> = Vec::new();
    let (mut pa, mut pb) = (0u32, 0u32);
    for (i, j) in &anchors {
        if *i > pa || *j > pb {
            ops.extend(myers::diff_seq(
                &a_keys[pa as usize..*i as usize],
                &b_keys[pb as usize..*j as usize],
            ));
        }
        ops.push(RawOp::Equal(1));
        pa = i + 1;
        pb = j + 1;
    }
    if pa < a_keys.len() as u32 || pb < b_keys.len() as u32 {
        ops.extend(myers::diff_seq(
            &a_keys[pa as usize..],
            &b_keys[pb as usize..],
        ));
    }
    myers::coalesce_ops(ops)
}

/// 计算一侧的键序列（骨架键，窗口局部 0 基），并把隔离区间替换为逐行唯一哨兵
/// （哨兵携带全量行号 + 侧标签，保证永不匹配 —— FR-TXT-011「隔离」）。
/// `isolate_local` 为窗口局部坐标。
pub(crate) fn keys_for_range<'a>(
    table: &'a LineTable,
    range: (u32, u32),
    opts: &TextDiffOptions,
    isolate_local: &[LineRange],
    side: DiffSide,
) -> Vec<Cow<'a, str>> {
    let mut keys: Vec<Cow<'a, str>> = (range.0..range.1)
        .map(|i| table.line(i as usize).compare_key(opts))
        .collect();
    for r in isolate_local {
        for i in r.start..r.end {
            keys[i as usize] =
                Cow::Owned(format!("\u{0}ISOL\u{0}{}\u{0}{}", side_tag(side), range.0 + i));
        }
    }
    keys
}

fn side_tag(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Left => "L",
        DiffSide::Right => "R",
    }
}

/// 手动锚点 -> 窗口内过滤 + 局部坐标（返回 (左局部区间, 右局部区间)）。
pub(crate) fn window_anchors(
    manual: &ManualAlignOptions,
    lr: (u32, u32),
    rr: (u32, u32),
) -> Result<Vec<(LineRange, LineRange)>, DiffError> {
    let mut out = Vec::new();
    for a in &manual.align_anchors {
        let inside = |r: LineRange, w: (u32, u32)| r.start >= w.0 && r.end <= w.1;
        if inside(a.left, lr) && inside(a.right, rr) {
            out.push((
                LineRange::new(a.left.start - lr.0, a.left.end - lr.0),
                LineRange::new(a.right.start - rr.0, a.right.end - rr.0),
            ));
        }
    }
    // 局部单调性校验
    let mut prev_l = 0u32;
    let mut prev_r = 0u32;
    for (l, r) in &out {
        if l.start < prev_l || r.start < prev_r || l.len() != r.len() {
            return Err(DiffError::InvalidManualAnchor(format!(
                "锚点在窗口内无效：left {l:?} right {r:?}"
            )));
        }
        prev_l = l.end;
        prev_r = r.end;
    }
    Ok(out)
}

/// 隔离/忽略区间 -> 窗口内过滤 + 局部坐标。
pub(crate) fn clip_ranges(
    ranges: &[(DiffSide, LineRange)],
    side: DiffSide,
    window: (u32, u32),
) -> Vec<LineRange> {
    ranges
        .iter()
        .filter(|(s, r)| *s == side && r.start >= window.0 && r.end <= window.1)
        .map(|(_, r)| LineRange::new(r.start - window.0, r.end - window.0))
        .collect()
}

/// 锚点强制段：键逐行相等 → Equal，否则 Change（FR-TXT-011 手动对齐）。
pub(crate) fn anchor_section_kind(
    left: &LineTable,
    right: &LineTable,
    opts: &TextDiffOptions,
    lr: (u32, u32),
    rr: (u32, u32),
) -> SectionKind {
    let n = lr.1 - lr.0;
    let equal = (0..n).all(|k| {
        left.line((lr.0 + k) as usize).compare_key(opts)
            == right.line((rr.0 + k) as usize).compare_key(opts)
    });
    if equal {
        SectionKind::Equal
    } else {
        SectionKind::Change
    }
}

/// 在窗口 [lr, rr)（全量坐标）上运行线性管线，返回**全量坐标**的 sections
/// （ids 从 0 连续编号）。
///
/// `manual_adjusted`：调用方已按编辑位移调整过的手动选项（全量坐标）；
/// 全文件入口直接传 `cfg.manual`。
pub(crate) fn run_range(
    left: &LineTable,
    right: &LineTable,
    cfg: &DiffConfig,
    lr: (u32, u32),
    rr: (u32, u32),
    manual_adjusted: &ManualAlignOptions,
) -> Result<Vec<Section>, DiffError> {
    let opts = &cfg.base;
    // 窗口尺寸（防锚点越界）
    let lw = lr.1 - lr.0;
    let rw = rr.1 - rr.0;

    let anchors = window_anchors(manual_adjusted, lr, rr)?;
    let iso_l = clip_ranges(&manual_adjusted.isolate, DiffSide::Left, lr);
    let iso_r = clip_ranges(&manual_adjusted.isolate, DiffSide::Right, rr);
    let a_keys = keys_for_range(left, lr, opts, &iso_l, DiffSide::Left);
    let b_keys = keys_for_range(right, rr, opts, &iso_r, DiffSide::Right);

    let algo = opts.algo;
    let skew = manual_adjusted.effective_skew();

    let mut sections: Vec<Section> = Vec::new();
    let la_off = lr.0;
    let rb_off = rr.0;

    // 分段装配：锚点前的段 + 锚点强制 section + 尾段
    let emit_segment = |sections: &mut Vec<Section>,
                            la0: u32,
                            la1: u32,
                            rb0: u32,
                            rb1: u32| {
        if la0 == la1 && rb0 == rb1 {
            return;
        }
        let seg_a = &a_keys[la0 as usize..la1 as usize];
        let seg_b = &b_keys[rb0 as usize..rb1 as usize];
        let mut ops = algo_ops(algo, seg_a, seg_b);
        if manual_adjusted.proximity_match {
            ops = manual::proximity_pass(&ops, seg_a, seg_b, skew);
        }
        // 局部区间 -> 全量坐标（窗口偏移 + 段内偏移）
        for mut sec in build_section_vec(&ops) {
            if let Some(l) = sec.left.as_mut() {
                l.start += lr.0 + la0;
                l.end += lr.0 + la0;
            }
            if let Some(r) = sec.right.as_mut() {
                r.start += rr.0 + rb0;
                r.end += rr.0 + rb0;
            }
            sections.push(sec);
        }
    };

    let mut pa = 0u32;
    let mut pb = 0u32;
    for (al, ar) in &anchors {
        emit_segment(&mut sections, pa, al.start, pb, ar.start);
        // 锚点强制并排（FR-TXT-011 手动对齐）
        let kind = anchor_section_kind(
            left,
            right,
            opts,
            (lr.0 + al.start, lr.0 + al.end),
            (rr.0 + ar.start, rr.0 + ar.end),
        );
        sections.push(Section {
            id: 0, // 装配后统一重编号
            kind,
            left: Some(LineRange::new(la_off + al.start, la_off + al.end)),
            right: Some(LineRange::new(rb_off + ar.start, rb_off + ar.end)),
            important: Importance::Important,
            inline: crate::sections::InlineState::NotComputed,
        });
        pa = al.end;
        pb = ar.end;
    }
    emit_segment(&mut sections, pa, lw, pb, rw);

    // 重要性重判（全量坐标 + 全量行表）→ 从不对齐拆分 → 相邻 Equal 合并
    let classification = keys::classify_sections(&sections, left, right, opts, &cfg.importance, manual_adjusted);
    for (sec, imp) in sections.iter_mut().zip(classification) {
        sec.important = imp;
    }
    if manual_adjusted.never_align_important {
        manual::apply_never_align(&mut sections);
    }
    merge_adjacent_equal(&mut sections);
    renumber(&mut sections);
    Ok(sections)
}

/// 全文件一次性 diff（KeyedDiffOutput；Standard 走渐进管线同步跑完）。
pub(crate) fn run_full(
    left: &LineTable,
    right: &LineTable,
    cfg: &DiffConfig,
) -> Result<KeyedDiffOutput, DiffError> {
    cfg.manual.validate(left.len() as u32, right.len() as u32)?;
    if cfg.base.algo == LineAlignAlgo::Standard {
        return Ok(crate::progressive::ProgressiveDiffer::run_to_end(left, right, cfg));
    }
    let mut sections = run_range(left, right, cfg, (0, left.len() as u32), (0, right.len() as u32), &cfg.manual)?;
    let summary = renumber(&mut sections);
    Ok(keys::build_keyed_output(
        crate::sections::DiffOutput { sections, summary },
        left,
        right,
        &cfg.base,
        &cfg.importance,
        &cfg.manual,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{ImportanceOptions, TextDiffOptions};

    fn cfg(algo: LineAlignAlgo) -> DiffConfig {
        DiffConfig {
            base: TextDiffOptions { algo, ..TextDiffOptions::default() },
            importance: ImportanceOptions::default(),
            manual: ManualAlignOptions::default(),
        }
    }

    fn lines(v: &[&str]) -> String {
        let mut s = v.join("\n");
        if !s.is_empty() {
            s.push('\n');
        }
        s
    }

    /// 覆盖完整性：sections 无缝覆盖两侧 + Equal 段真等 + 重放还原右侧
    fn assert_sane(left: &LineTable, right: &LineTable, secs: &[Section]) {
        let mut lc = 0u32;
        let mut rc = 0u32;
        let mut rebuilt = Vec::new();
        for s in secs {
            if let Some(l) = s.left {
                assert_eq!(l.start, lc, "左区间必须连续");
                lc = l.end;
            }
            if let Some(r) = s.right {
                assert_eq!(r.start, rc, "右区间必须连续");
                rc = r.end;
            }
            match s.kind {
                SectionKind::Equal => {
                    let l = s.left.expect("Equal 必有左区间");
                    let r = s.right.expect("Equal 必有右区间");
                    assert_eq!(l.len(), r.len());
                    for k in 0..l.len() {
                        assert_eq!(left.line((l.start + k) as usize).text, right.line((r.start + k) as usize).text);
                    }
                    for i in l.start..l.end {
                        rebuilt.push(left.line(i as usize).text.clone());
                    }
                }
                SectionKind::Change | SectionKind::Insert => {
                    let r = s.right.expect("Change/Insert 必有右区间");
                    for i in r.start..r.end {
                        rebuilt.push(right.line(i as usize).text.clone());
                    }
                }
                SectionKind::Delete => {}
            }
        }
        assert_eq!(lc, left.len() as u32);
        assert_eq!(rc, right.len() as u32);
        let expected: Vec<String> = right.iter().map(|l| l.text.clone()).collect();
        assert_eq!(rebuilt, expected, "重放必须还原右侧");
    }

    #[test]
    fn all_algos_agree_on_simple_change() {
        let left = LineTable::from_text(&lines(&["one", "two", "three"]));
        let right = LineTable::from_text(&lines(&["one", "TWO", "three", "four"]));
        for algo in [LineAlignAlgo::Myers, LineAlignAlgo::Patience, LineAlignAlgo::Histogram, LineAlignAlgo::Standard] {
            let out = run_full(&left, &right, &cfg(algo)).unwrap();
            assert_sane(&left, &right, &out.output.sections);
            assert_eq!(out.output.summary.change_sections, 1, "{algo:?} 应为单 Change");
            assert_eq!(out.output.summary.equal_lines, 2, "{algo:?} one/three 对齐");
        }
    }

    #[test]
    fn moved_block_all_algos_replay() {
        let left = LineTable::from_text(&lines(&["1", "2", "3", "4", "5", "6", "7", "8"]));
        let right = LineTable::from_text(&lines(&["1", "6", "7", "2", "3", "4", "5", "8"]));
        for algo in [LineAlignAlgo::Myers, LineAlignAlgo::Patience, LineAlignAlgo::Histogram, LineAlignAlgo::Standard] {
            let out = run_full(&left, &right, &cfg(algo)).unwrap();
            assert_sane(&left, &right, &out.output.sections);
        }
    }

    #[test]
    fn manual_anchor_forces_alignment() {
        // 无锚点时两侧全异；锚点强制 a[1] 与 b[2] 并排
        let left = LineTable::from_text(&lines(&["A", "M1", "B"]));
        let right = LineTable::from_text(&lines(&["X", "Y", "M1", "Z"]));
        let mut c = cfg(LineAlignAlgo::Myers);
        c.manual.align_anchors = vec![crate::options::AlignAnchor {
            left: LineRange::new(1, 2),
            right: LineRange::new(2, 3),
        }];
        let out = run_full(&left, &right, &c).unwrap();
        assert_sane(&left, &right, &out.output.sections);
        // 锚点 section：M1 ↔ M1 强制 Equal
        let anchored = out
            .output
            .sections
            .iter()
            .find(|s| s.kind == SectionKind::Equal && s.left.is_some_and(|l| l.start == 1))
            .expect("锚点应成为 Equal section");
        assert_eq!(anchored.right, Some(LineRange::new(2, 3)));
    }

    #[test]
    fn isolate_ranges_stay_unaligned() {
        // 隔离 a[1]（与 b[1] 内容相同）→ 不许配对，独立成块
        let left = LineTable::from_text(&lines(&["A", "same", "B"]));
        let right = LineTable::from_text(&lines(&["A", "same", "B"]));
        let mut c = cfg(LineAlignAlgo::Myers);
        c.manual.isolate = vec![(DiffSide::Left, LineRange::new(1, 2))];
        let out = run_full(&left, &right, &c).unwrap();
        assert_sane(&left, &right, &out.output.sections);
        let has_change = out.output.sections.iter().any(|s| s.kind != SectionKind::Equal);
        assert!(has_change, "隔离行必须脱离对齐骨架: {:?}", out.output.sections);
    }

    #[test]
    fn invalid_anchor_rejected() {
        let left = LineTable::from_text(&lines(&["A", "B"]));
        let right = LineTable::from_text(&lines(&["A", "B"]));
        let mut c = cfg(LineAlignAlgo::Myers);
        c.manual.align_anchors = vec![crate::options::AlignAnchor {
            left: LineRange::new(0, 2),
            right: LineRange::new(0, 1),
        }];
        assert!(run_full(&left, &right, &c).is_err(), "等长校验应失败");
    }

    #[test]
    fn windowed_range_matches_full_run() {
        let left = LineTable::from_text(&lines(&["h1", "a", "b", "c", "d", "t1"]));
        let right = LineTable::from_text(&lines(&["h1", "a", "X", "Y", "d", "t1"]));
        let c = cfg(LineAlignAlgo::Patience);
        let full = run_range(&left, &right, &c, (0, left.len() as u32), (0, right.len() as u32), &c.manual).unwrap();
        // 窗口取中段 [1..5)：结果应为全量坐标（含偏移）
        let win = run_range(&left, &right, &c, (1, 5), (1, 5), &c.manual).unwrap();
        assert_sane(&left, &right, &full);
        // 窗口结果：连续 + Equal 真等（不要求从 0 起全量覆盖）
        let mut lc = 1u32;
        let mut rc = 1u32;
        for sec in &win {
            if let Some(l) = sec.left {
                assert_eq!(l.start, lc, "窗口左区间必须连续");
                lc = l.end;
                if let Some(r) = sec.right {
                    assert_eq!(r.start, rc, "窗口右区间必须连续");
                    rc = r.end;
                    if sec.kind == SectionKind::Equal {
                        assert_eq!(l.len(), r.len());
                        for k in 0..l.len() {
                            assert_eq!(
                                left.line((l.start + k) as usize).text,
                                right.line((r.start + k) as usize).text,
                                "Equal 段两侧必须一致"
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(lc, 5);
        assert_eq!(rc, 5);
        // 窗口结果必须恰好覆盖 [1..5)
        for s in &win {
            if let Some(l) = s.left {
                assert!(l.start >= 1 && l.end <= 5, "窗口越界: {l:?}");
            }
            if let Some(r) = s.right {
                assert!(r.start >= 1 && r.end <= 5, "窗口越界: {r:?}");
            }
        }
        let lc: u32 = win.iter().filter_map(|s| s.left.map(|l| l.len())).sum();
        let rc: u32 = win.iter().filter_map(|s| s.right.map(|r| r.len())).sum();
        assert_eq!(lc, 4);
        assert_eq!(rc, 4);
    }
}
