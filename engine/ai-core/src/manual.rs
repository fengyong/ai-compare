//! 手动对齐语义的实现层（FR-TXT-011，docs/04 P1 交付物 ③）。
//!
//! - [`apply_never_align`]：「从不对齐差异」——重要 Change section 拆分为
//!   Delete + Insert 文本块（须在重要性重判之后调用）；
//! - [`proximity_pass`]：「使用接近度匹配」——对被 ≤ 偏斜容差行等值段分隔的
//!   删除+插入块做窗口重 diff，按相似度尝试对齐剩余不匹配行（有收益才采纳，
//!   因此只改善对齐、不劣化最小性）。
//!
//! 手动对齐锚点（强制并排）与隔离哨兵在 [`crate::pipeline`] 的分段装配中实现。

use std::borrow::Cow;

use crate::myers::{self, RawOp};
use crate::sections::{InlineState, Importance, Section, SectionKind};

/// 「从不对齐差异」：把重要 Change section 拆为 Delete + Insert（FR-TXT-011）。
///
/// 前置条件：sections 已做过重要性重判（Unimportant 的 Change 保持为 Change）。
pub(crate) fn apply_never_align(sections: &mut Vec<Section>) {
    let mut out: Vec<Section> = Vec::with_capacity(sections.len() + 8);
    for sec in sections.drain(..) {
        if sec.kind == SectionKind::Change && sec.important == Importance::Important {
            if let (Some(l), Some(r)) = (sec.left, sec.right) {
                out.push(Section {
                    id: sec.id,
                    kind: SectionKind::Delete,
                    left: Some(l),
                    right: None,
                    important: sec.important,
                    inline: sec.inline,
                });
                out.push(Section {
                    id: sec.id + 1,
                    kind: SectionKind::Insert,
                    left: None,
                    right: Some(r),
                    important: sec.important,
                    inline: InlineState::NotComputed,
                });
                continue;
            }
        }
        out.push(sec);
    }
    *sections = out;
}

/// 接近度匹配（FR-TXT-011）：扫描 coalesce 后的 ops，寻找
/// `[Delete, Equal(g), Insert]` / `[Insert, Equal(g), Delete]` 模式（g ≤ skew），
/// 对"块 + 间隙"窗口重跑 Myers；仅当重 diff 减少编辑量（找到了相似行）时采纳。
///
/// `a`/`b` 为窗口所在全量键序列；ops 的游标基于同一坐标系。
pub(crate) fn proximity_pass(
    ops: &[RawOp],
    a: &[Cow<'_, str>],
    b: &[Cow<'_, str>],
    skew: u32,
) -> Vec<RawOp> {
    let mut out: Vec<RawOp> = Vec::with_capacity(ops.len());
    let (mut ia, mut ib) = (0usize, 0usize);
    let mut i = 0usize;
    while i < ops.len() {
        let pattern = if i + 2 < ops.len() {
            match (ops[i], ops[i + 1], ops[i + 2]) {
                (RawOp::Delete(d), RawOp::Equal(g), RawOp::Insert(m)) if g <= skew => {
                    Some((d, g, m))
                }
                (RawOp::Insert(m), RawOp::Equal(g), RawOp::Delete(d)) if g <= skew => {
                    // 统一参数化为 (删除块 d, 间隙 g, 插入块 m)：窗口左 = d+g，右 = g+m
                    Some((d, g, m))
                }
                _ => None,
            }
        } else {
            None
        };
        match pattern {
            Some((d, g, m)) => {
                // 窗口：删块+间隙（一侧）× 间隙+插块（另一侧）
                let a_win = ia..ia + (d + g) as usize;
                let b_win = ib..ib + (g + m) as usize;
                let old_cost = d + m;
                let new_ops = myers::diff_seq(&a[a_win.clone()], &b[b_win.clone()]);
                let new_cost: u32 = new_ops
                    .iter()
                    .map(|o| match o {
                        RawOp::Equal(_) => 0,
                        RawOp::Delete(k) | RawOp::Insert(k) => *k,
                    })
                    .sum();
                if new_cost < old_cost {
                    out.extend(new_ops);
                } else {
                    out.push(ops[i]);
                    out.push(ops[i + 1]);
                    out.push(ops[i + 2]);
                }
                ia += (d + g) as usize;
                ib += (g + m) as usize;
                i += 3;
            }
            None => {
                match ops[i] {
                    RawOp::Equal(k) => {
                        ia += k as usize;
                        ib += k as usize;
                    }
                    RawOp::Delete(k) => ia += k as usize,
                    RawOp::Insert(k) => ib += k as usize,
                }
                out.push(ops[i]);
                i += 1;
            }
        }
    }
    debug_assert_eq!(ia, a.len(), "接近度匹配游标必须覆盖全量 a");
    debug_assert_eq!(ib, b.len(), "接近度匹配游标必须覆盖全量 b");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{ManualAlignOptions, TextDiffOptions};
    use crate::sections::LineRange;

    fn cow<'a>(v: &[&'a str]) -> Vec<Cow<'a, str>> {
        v.iter().map(|s| Cow::Borrowed(*s)).collect()
    }

    #[test]
    fn never_align_splits_important_change() {
        let mut secs = vec![Section {
            id: 0,
            kind: SectionKind::Equal,
            left: Some(LineRange::new(0, 1)),
            right: Some(LineRange::new(0, 1)),
            important: Importance::Important,
            inline: InlineState::NotComputed,
        }];
        secs.push(Section {
            id: 1,
            kind: SectionKind::Change,
            left: Some(LineRange::new(1, 3)),
            right: Some(LineRange::new(1, 3)),
            important: Importance::Important,
            inline: InlineState::NotComputed,
        });
        secs.push(Section {
            id: 2,
            kind: SectionKind::Change,
            left: Some(LineRange::new(3, 4)),
            right: Some(LineRange::new(3, 4)),
            important: Importance::Unimportant,
            inline: InlineState::NotComputed,
        });
        apply_never_align(&mut secs);
        assert_eq!(secs.len(), 4, "Important Change 拆分，Unimportant 保留");
        assert_eq!(secs[1].kind, SectionKind::Delete);
        assert_eq!(secs[1].left, Some(LineRange::new(1, 3)));
        assert_eq!(secs[2].kind, SectionKind::Insert);
        assert_eq!(secs[2].right, Some(LineRange::new(1, 3)));
        assert_eq!(secs[3].kind, SectionKind::Change, "不重要差异保持更改块");
    }

    #[test]
    fn proximity_aligns_similar_blocks_across_gap() {
        // 删除块与插入块共享子序列 [c1,c2]，比间隙(1)长 → 窗口重 diff 有收益：
        // 采纳条件推导：window LCS = max(gap, LCS(blk1,blk2))，收益 ⟺ LCS(blk1,blk2) > gap
        let a = cow(&["P", "c1", "c2", "x", "G", "Q"]);
        let b = cow(&["P", "G", "c1", "c2", "y", "Q"]);
        // 模拟某个非最小基线产出的 [Del(3), Equal(1), Ins(3)] 模式
        let ops = vec![RawOp::Equal(1), RawOp::Delete(3), RawOp::Equal(1), RawOp::Insert(3), RawOp::Equal(1)];
        let passed = proximity_pass(&ops, &a, &b, 2500);
        let changed: u32 = passed
            .iter()
            .filter_map(|o| match o {
                RawOp::Equal(_) => None,
                RawOp::Delete(k) | RawOp::Insert(k) => Some(*k),
            })
            .sum();
        // 窗口重 diff：LCS=[c1,c2] → 编辑量 6 -> 4
        assert_eq!(changed, 4, "接近度匹配应对齐 c1/c2: {passed:?}");
    }

    #[test]
    fn proximity_skips_dissimilar_blocks() {
        let a = cow(&["P", "x1", "x2", "E1", "Q"]);
        let b = cow(&["P", "E1", "y1", "y2", "Q"]);
        let ops = vec![RawOp::Equal(1), RawOp::Delete(2), RawOp::Equal(1), RawOp::Insert(2), RawOp::Equal(1)];
        let passed = proximity_pass(&ops, &a, &b, 2500);
        assert_eq!(passed, ops, "无收益时保持原 ops");
    }

    #[test]
    fn proximity_respects_skew_tolerance() {
        // c1/c2 共享子序列(2) > 间隙(1)：skew ≥ 1 时窗口重 diff 有收益
        let a = cow(&["P", "c1", "c2", "x", "G", "Q"]);
        let b = cow(&["P", "G", "c1", "c2", "y", "Q"]);
        let ops = vec![RawOp::Equal(1), RawOp::Delete(3), RawOp::Equal(1), RawOp::Insert(3), RawOp::Equal(1)];
        // skew=0 < 间隙 1 → 模式不触发，保持原 ops
        let passed = proximity_pass(&ops, &a, &b, 0);
        assert_eq!(passed, ops, "间隙超出偏斜容差时不处理");
        // skew=1 ≥ 间隙 1 → c1/c2 被对齐（编辑量 6 -> 4）
        let passed = proximity_pass(&ops, &a, &b, 1);
        let changed: u32 = passed
            .iter()
            .filter_map(|o| match o {
                RawOp::Equal(_) => None,
                RawOp::Delete(k) | RawOp::Insert(k) => Some(*k),
            })
            .sum();
        assert_eq!(changed, 4, "间隙在容差内时应对齐 c1/c2: {passed:?}");
    }

    #[test]
    fn full_diff_with_proximity_option_smoke() {
        // 接近度匹配作为选项接入完整管线（对本语料为 no-op），保证不破坏正确性
        let left = crate::LineTable::from_text("P\nc1\nc2\nx\nG\nQ\n");
        let right = crate::LineTable::from_text("P\nG\nc1\nc2\ny\nQ\n");
        let manual = ManualAlignOptions { proximity_match: true, ..ManualAlignOptions::default() };
        let cfg = crate::options::DiffConfig {
            base: TextDiffOptions { algo: crate::options::LineAlignAlgo::Patience, ..TextDiffOptions::default() },
            manual,
            ..crate::options::DiffConfig::default()
        };
        let out = crate::diff_config(&left, &right, &cfg).unwrap();
        // 重放正确性：按 section 类别重建右侧（与 assert_sane 同性质）
        let mut rebuilt = String::new();
        for s in &out.output.sections {
            match s.kind {
                crate::sections::SectionKind::Equal => {
                    let l = s.left.expect("Equal 必有左区间");
                    for i in l.start..l.end {
                        rebuilt.push_str(left.line(i as usize).text.as_str());
                        rebuilt.push('\n');
                    }
                }
                crate::sections::SectionKind::Change | crate::sections::SectionKind::Insert => {
                    let r = s.right.expect("Change/Insert 必有右区间");
                    for i in r.start..r.end {
                        rebuilt.push_str(right.line(i as usize).text.as_str());
                        rebuilt.push('\n');
                    }
                }
                crate::sections::SectionKind::Delete => {}
            }
        }
        assert_eq!(rebuilt, right.to_text(), "重放必须还原右侧");
        // c1/c2 均被对齐
        let equal_lines: u32 = out
            .output
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Equal)
            .map(|s| s.left.map_or(0, |r| r.len()))
            .sum();
        assert_eq!(equal_lines, 4, "P/c1/c2/Q 均应对齐: {:?}", out.output.sections);
    }
}
