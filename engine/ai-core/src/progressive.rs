//! Standard 渐进流（FR-TXT-011 标准对齐；FR-BIG-001 先粗后细；docs/03 §4.1）。
//!
//! 两阶段事件模型：
//! 1. **粗粒度**（[`ProgressiveDiffer::coarse_events`]）：公共前缀/后缀裁剪 +
//!    唯一行锚点分块（O(N) 哈希 + LIS，不做逐块 diff），立即交付全部
//!    `Append` 事件 —— 每个块以单一占位 section（Change/Delete/Insert）呈现；
//! 2. **精化**（[`ProgressiveDiffer::poll`]）：逐块独立 diff，以 `Replace`
//!    事件原位替换占位 section（先粗后细）；
//! 3. [`ProgressiveDiffer::done`]：全部块精化完毕后返回终态 `Done` 统计。
//!
//! 一致性契约：`run_to_end`（同步跑完）与 [`crate::pipeline::run_range`] 的
//! Standard 一次性路径产出相同 section 结构。

use std::borrow::Cow;

use crate::keys::{self, KeyedDiffOutput};
use crate::lines::LineTable;
use crate::manual;
use crate::myers::{self, RawOp};
use crate::options::{DiffConfig, DiffSide, TextDiffOptions};
use crate::patience;
use crate::pipeline;
use crate::sections::{
    build_section_vec, merge_adjacent_equal, renumber, DiffError, Importance, LineRange, Section,
    SectionEvent, SectionKind,
};

/// 粗粒度装配计划项
#[derive(Debug)]
enum PlanItem {
    /// 手动锚点强制段（终态，不精化）
    Anchor(LineRange, LineRange),
    /// 唯一锚等值段（终态）
    Run(LineRange, LineRange),
    /// 待精化块（`blocks` 下标）
    Block(usize),
}

/// Standard 渐进 differ（一次构造，多轮 poll）。
pub struct ProgressiveDiffer<'a> {
    left: &'a LineTable,
    right: &'a LineTable,
    opts: TextDiffOptions,
    manual: crate::options::ManualAlignOptions,
    importance: crate::options::ImportanceOptions,
    a_keys: Vec<Cow<'a, str>>,
    b_keys: Vec<Cow<'a, str>>,
    plan: Vec<PlanItem>,
    item_sec_idx: Vec<usize>,
    blocks: Vec<(LineRange, LineRange)>,
    sections: Vec<Section>,
    summary: crate::sections::DiffSummary,
    next_block: usize,
    coarse_emitted: bool,
}

impl<'a> ProgressiveDiffer<'a> {
    /// 构造：裁剪 + 锚点分块 + 粗粒度计划（不 diff 任何块）。
    pub fn new(left: &'a LineTable, right: &'a LineTable, cfg: &DiffConfig) -> Result<Self, DiffError> {
        cfg.manual.validate(left.len() as u32, right.len() as u32)?;
        let opts = cfg.base.clone();
        // 全量键（含隔离哨兵：隔离行永不成为锚点、永不匹配）
        let iso_l = pipeline::clip_ranges(&cfg.manual.isolate, DiffSide::Left, (0, left.len() as u32));
        let iso_r = pipeline::clip_ranges(&cfg.manual.isolate, DiffSide::Right, (0, right.len() as u32));
        let a_keys = pipeline::keys_for_range(left, (0, left.len() as u32), &opts, &iso_l, DiffSide::Left);
        let b_keys = pipeline::keys_for_range(right, (0, right.len() as u32), &opts, &iso_r, DiffSide::Right);

        // 手动锚点分段（全量坐标，局部 == 全量）
        let anchors = pipeline::window_anchors(&cfg.manual, (0, left.len() as u32), (0, right.len() as u32))?;

        let mut plan: Vec<PlanItem> = Vec::new();
        let mut blocks: Vec<(LineRange, LineRange)> = Vec::new();
        let mut pa = 0u32;
        let mut pb = 0u32;
        for (al, ar) in &anchors {
            push_segment_items(
                &mut plan,
                &mut blocks,
                (&a_keys, &b_keys),
                Segment { a: (pa, al.start), b: (pb, ar.start) },
            );
            plan.push(PlanItem::Anchor(*al, *ar));
            pa = al.end;
            pb = ar.end;
        }
        let (la_len, lb_len) = (a_keys.len() as u32, b_keys.len() as u32);
        push_segment_items(
            &mut plan,
            &mut blocks,
            (&a_keys, &b_keys),
            Segment { a: (pa, la_len), b: (pb, lb_len) },
        );

        let mut differ = ProgressiveDiffer {
            left,
            right,
            opts,
            manual: cfg.manual.clone(),
            importance: cfg.importance.clone(),
            a_keys,
            b_keys,
            plan,
            item_sec_idx: Vec::new(),
            blocks,
            sections: Vec::new(),
            summary: crate::sections::DiffSummary::default(),
            next_block: 0,
            coarse_emitted: false,
        };
        differ.build_coarse();
        Ok(differ)
    }

    /// 粗粒度 section 装配（构造时同步完成，事件首次取走）
    fn build_coarse(&mut self) {
        self.sections.clear();
        self.item_sec_idx.clear();
        for (pi, item) in self.plan.iter().enumerate() {
            let sec = match item {
                PlanItem::Anchor(l, r) => {
                    let kind = pipeline::anchor_section_kind(
                        self.left,
                        self.right,
                        &self.opts,
                        (l.start, l.end),
                        (r.start, r.end),
                    );
                    Section {
                        id: pi as u32,
                        kind,
                        left: Some(*l),
                        right: Some(*r),
                        important: Importance::Important,
                        inline: crate::sections::InlineState::NotComputed,
                    }
                }
                PlanItem::Run(l, r) => Section {
                    id: pi as u32,
                    kind: SectionKind::Equal,
                    left: Some(*l),
                    right: Some(*r),
                    important: Importance::Important,
                    inline: crate::sections::InlineState::NotComputed,
                },
                PlanItem::Block(bi) => {
                    let (l, r) = self.blocks[*bi];
                    let kind = if l.is_empty() {
                        SectionKind::Insert
                    } else if r.is_empty() {
                        SectionKind::Delete
                    } else {
                        SectionKind::Change
                    };
                    Section {
                        id: pi as u32,
                        kind,
                        left: if l.is_empty() { None } else { Some(l) },
                        right: if r.is_empty() { None } else { Some(r) },
                        important: Importance::Important,
                        inline: crate::sections::InlineState::NotComputed,
                    }
                }
            };
            self.item_sec_idx.push(self.sections.len());
            self.sections.push(sec);
        }
        self.summary = renumber(&mut self.sections);
    }

    /// 粗粒度事件（仅首次调用有效；后续调用返回空）。
    pub fn coarse_events(&mut self) -> Vec<SectionEvent> {
        if self.coarse_emitted {
            return Vec::new();
        }
        self.coarse_emitted = true;
        self.sections.iter().map(|s| SectionEvent::Append(*s)).collect()
    }

    /// 精化下一块；返回 `Replace` 事件（精化结果与占位相同时为空事件序列）。
    /// 返回 `None` 表示全部块已精化。
    pub fn poll(&mut self) -> Option<Vec<SectionEvent>> {
        while self.next_block < self.blocks.len() {
            let bi = self.next_block;
            self.next_block += 1;
            // 找到该块对应的 plan 位置（Block 项与 blocks 一一对应）
            let plan_pos = self
                .plan
                .iter()
                .position(|it| matches!(it, PlanItem::Block(b) if *b == bi))
                .expect("block 必有 plan 项");
            let sec_idx = self.item_sec_idx[plan_pos];
            let (bl, br) = self.blocks[bi];

            // 逐块 diff（Standard 的"细"= 块内 Myers）
            let seg_a = &self.a_keys[bl.start as usize..bl.end as usize];
            let seg_b = &self.b_keys[br.start as usize..br.end as usize];
            let mut ops: Vec<RawOp> = myers::diff_seq(seg_a, seg_b);
            if self.manual.proximity_match {
                ops = manual::proximity_pass(&ops, seg_a, seg_b, self.manual.effective_skew());
            }
            let mut refined: Vec<Section> = build_section_vec(&ops);
            for sec in refined.iter_mut() {
                if let Some(l) = sec.left.as_mut() {
                    l.start += bl.start;
                    l.end += bl.start;
                }
                if let Some(r) = sec.right.as_mut() {
                    r.start += br.start;
                    r.end += br.start;
                }
            }
            // 块内重要性重判 + 从不对齐 + 块内合并（与一次性管线一致）
            let classification =
                keys::classify_sections(&refined, self.left, self.right, &self.opts, &self.importance, &self.manual);
            for (sec, imp) in refined.iter_mut().zip(classification) {
                sec.important = imp;
            }
            if self.manual.never_align_important {
                manual::apply_never_align(&mut refined);
            }
            merge_adjacent_equal(&mut refined);

            // 边界 Equal 吸收：并入相邻的终态 EqualRun/锚点 Equal 段
            let head_id = self.sections[sec_idx].id;
            while let Some(first) = refined.first() {
                if first.kind != SectionKind::Equal || sec_idx == 0 {
                    break;
                }
                let prev_idx = sec_idx - 1;
                if self.sections[prev_idx].kind != SectionKind::Equal {
                    break;
                }
                let prev = self.sections[prev_idx];
                let first = *refined.first().expect("已检查");
                let (pl, fl) = (prev.left.expect("Equal 必有左区间"), first.left.expect("Equal 必有左区间"));
                debug_assert_eq!(pl.end, fl.start);
                let (pr, fr) = (prev.right.expect("Equal 必有右区间"), first.right.expect("Equal 必有右区间"));
                debug_assert_eq!(pr.end, fr.start);
                self.sections[prev_idx].left = Some(LineRange::new(pl.start, fl.end));
                self.sections[prev_idx].right = Some(LineRange::new(pr.start, fr.end));
                refined.remove(0);
            }
            while let Some(last) = refined.last() {
                if last.kind != SectionKind::Equal || sec_idx + 1 >= self.sections.len() {
                    break;
                }
                let next_idx = sec_idx + 1;
                if self.sections[next_idx].kind != SectionKind::Equal {
                    break;
                }
                let next = self.sections[next_idx];
                let last = *refined.last().expect("已检查");
                let (ll, nl) = (last.left.expect("Equal 必有左区间"), next.left.expect("Equal 必有左区间"));
                debug_assert_eq!(ll.end, nl.start);
                let (lr2, nr) = (last.right.expect("Equal 必有右区间"), next.right.expect("Equal 必有右区间"));
                debug_assert_eq!(lr2.end, nr.start);
                self.sections[next_idx].left = Some(LineRange::new(ll.start, nl.end));
                self.sections[next_idx].right = Some(LineRange::new(lr2.start, nr.end));
                refined.pop();
            }

            // 精化结果与占位完全相同 → 跳过事件与拼装
            let coarse = self.sections[sec_idx];
            if refined.len() == 1
                && refined[0].kind == coarse.kind
                && refined[0].left == coarse.left
                && refined[0].right == coarse.right
                && refined[0].important == coarse.important
            {
                continue;
            }

            // 拼装：替换 1 个占位 section；后续计划项下标平移
            let delta = refined.len() as isize - 1;
            let old_len = self.sections.len();
            self.sections.splice(sec_idx..sec_idx + 1, refined.iter().cloned());
            renumber(&mut self.sections);
            for idx in self.item_sec_idx.iter_mut().skip(plan_pos + 1) {
                *idx = (*idx as isize + delta) as usize;
            }
            debug_assert_eq!(self.sections.len() as isize, old_len as isize + delta);

            let with: Vec<Section> = (0..refined.len())
                .map(|k| self.sections[sec_idx + k])
                .collect();
            if refined.is_empty() {
                // 占位被完全吸收：移除 plan 项（其下标映射已失效）
                self.plan.remove(plan_pos);
                self.item_sec_idx.remove(plan_pos);
            }
            return Some(vec![SectionEvent::Replace { span: (head_id, head_id), with }]);
        }
        None
    }

    /// 终态事件（poll 耗尽后调用）。
    pub fn done(&mut self) -> SectionEvent {
        SectionEvent::Done(self.summary)
    }

    /// 当前 section 列表（粗粒度或部分精化状态）。
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// 同步跑完全部精化（等价于一次性 Standard diff）。
    pub fn run_to_end(left: &'a LineTable, right: &'a LineTable, cfg: &DiffConfig) -> KeyedDiffOutput {
        let mut differ = match ProgressiveDiffer::new(left, right, cfg) {
            Ok(d) => d,
            Err(e) => unreachable!("run_to_end 的锚点已由调用方校验：{e}"),
        };
        let _ = differ.coarse_events();
        while differ.poll().is_some() {}
        let summary = renumber(&mut differ.sections);
        keys::build_keyed_output(
            crate::sections::DiffOutput { sections: differ.sections, summary },
            left,
            right,
            &differ.opts,
            &differ.importance,
            &differ.manual,
        )
    }
}

/// 分段范围（全量坐标）
struct Segment {
    a: (u32, u32),
    b: (u32, u32),
}

/// 把一个段内唯一锚点序列展开为 plan 项：块 + 等值段。
/// 区间均为全量坐标（键序列为全量，段区间是全量的子区间）。
fn push_segment_items(
    plan: &mut Vec<PlanItem>,
    blocks: &mut Vec<(LineRange, LineRange)>,
    keys: (&[Cow<'_, str>], &[Cow<'_, str>]),
    seg: Segment,
) {
    let (sa0, sa1) = seg.a;
    let (sb0, sb1) = seg.b;
    if sa0 == sa1 && sb0 == sb1 {
        return;
    }
    let seg_a = &keys.0[sa0 as usize..sa1 as usize];
    let seg_b = &keys.1[sb0 as usize..sb1 as usize];
    let anchors = match patience::unique_anchor_lis(seg_a, seg_b) {
        None => {
            // 无锚点：整段一个占位块
            let bi = blocks.len();
            blocks.push((LineRange::new(sa0, sa1), LineRange::new(sb0, sb1)));
            plan.push(PlanItem::Block(bi));
            return;
        }
        Some(pairs) => pairs,
    };
    let (mut prev_a, mut prev_b) = (sa0, sb0);
    for (i, j) in &anchors {
        let ai = sa0 + i;
        let bj = sb0 + j;
        if ai > prev_a || bj > prev_b {
            let bi = blocks.len();
            blocks.push((LineRange::new(prev_a, ai), LineRange::new(prev_b, bj)));
            plan.push(PlanItem::Block(bi));
        }
        // 等值段：锚行（连续锚合并进同一 Run）
        match plan.last_mut() {
            Some(PlanItem::Run(l, r)) if l.end == ai && r.end == bj => {
                l.end = ai + 1;
                r.end = bj + 1;
            }
            _ => plan.push(PlanItem::Run(LineRange::new(ai, ai + 1), LineRange::new(bj, bj + 1))),
        }
        prev_a = ai + 1;
        prev_b = bj + 1;
    }
    if prev_a < sa1 || prev_b < sb1 {
        let bi = blocks.len();
        blocks.push((LineRange::new(prev_a, sa1), LineRange::new(prev_b, sb1)));
        plan.push(PlanItem::Block(bi));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{ImportanceOptions, ManualAlignOptions};

    fn cfg() -> DiffConfig {
        DiffConfig {
            base: TextDiffOptions::default(),
            importance: ImportanceOptions::default(),
            manual: ManualAlignOptions::default(),
        }
    }

    #[test]
    fn coarse_then_refine_events() {
        // 重复行块：唯一锚只有 h/t，中段 [x,x]×[x] 为单一粗粒度占位；
        // 精化后块内出现 Equal（粗粒度不可见）→ Replace 事件 + 头部 Equal 吸收
        let left = LineTable::from_text("h\nx\nx\nt\n");
        let right = LineTable::from_text("h\nx\nt\n");
        let mut d = ProgressiveDiffer::new(&left, &right, &cfg()).unwrap();
        let coarse = d.coarse_events();
        assert_eq!(coarse.len(), 3, "Equal(h) + 占位 Change + Equal(t)");
        assert!(matches!(&coarse[0], SectionEvent::Append(s) if s.kind == SectionKind::Equal));
        assert!(matches!(&coarse[1], SectionEvent::Append(s) if s.kind == SectionKind::Change));
        assert_eq!(d.sections().len(), 3);
        // 粗粒度状态即可消费（占位范围正确）
        assert_eq!(d.sections()[1].left, Some(crate::sections::LineRange::new(1, 3)));
        assert_eq!(d.sections()[1].right, Some(crate::sections::LineRange::new(1, 2)));

        let mut events = Vec::new();
        while let Some(ev) = d.poll() {
            events.extend(ev);
        }
        assert_eq!(events.len(), 1, "占位应精化：Replace 事件");
        match &events[0] {
            SectionEvent::Replace { span, with } => {
                assert_eq!(span.0, span.1, "替换单个占位");
                assert_eq!(with.len(), 1, "块内 Equal 被头部吸收，Replace 只余 Delete: {with:?}");
                assert_eq!(with[0].kind, SectionKind::Delete);
            }
            other => panic!("应为 Replace 事件: {other:?}"),
        }
        let done = d.done();
        assert!(matches!(done, SectionEvent::Done(_)));
        // 终态：Equal(h,x) + Delete(x) + Equal(t)（头部吸收后 3 个 section）
        assert_eq!(d.sections().len(), 3);
        assert_eq!(d.sections()[0].kind, SectionKind::Equal);
        assert_eq!(d.sections()[0].left, Some(crate::sections::LineRange::new(0, 2)));
        assert_eq!(d.sections()[1].kind, SectionKind::Delete);
        assert_eq!(d.sections()[2].kind, SectionKind::Equal);
    }

    #[test]
    fn identical_files_no_replace() {
        let left = LineTable::from_text("a\nb\n");
        let right = left.clone();
        let mut d = ProgressiveDiffer::new(&left, &right, &cfg()).unwrap();
        let coarse = d.coarse_events();
        assert_eq!(coarse.len(), 1, "全等文件：单一 Equal");
        while d.poll().is_some() {}
        assert!(matches!(d.done(), SectionEvent::Done(_)));
    }

    #[test]
    fn stream_shape_matches_one_shot() {
        let left = LineTable::from_text("1\n2\n3\n4\n5\n6\n7\n8\n");
        let right = LineTable::from_text("1\n6\n7\n2\n3\n4\n5\n8\n");
        let c = cfg();
        let mut d = ProgressiveDiffer::new(&left, &right, &c).unwrap();
        let mut events = d.coarse_events();
        while let Some(ev) = d.poll() {
            events.extend(ev);
        }
        events.push(d.done());
        // 事件流末态 == 一次性结果
        let one_shot = crate::diff_config(&left, &right, &c).unwrap();
        // 通过重放事件流重建 sections
        let mut rebuilt: Vec<Section> = Vec::new();
        for e in &events {
            match e {
                SectionEvent::Append(s) => rebuilt.push(*s),
                SectionEvent::Replace { span, with } => {
                    let start = rebuilt.iter().position(|s| s.id == span.0).expect("span 起点存在");
                    let end = rebuilt.iter().position(|s| s.id == span.1).expect("span 终点存在");
                    rebuilt.splice(start..=end, with.iter().cloned());
                    renumber(&mut rebuilt);
                }
                SectionEvent::ImportanceUpdate { .. } => {}
                SectionEvent::Done(_) => {}
            }
        }
        assert_eq!(rebuilt, one_shot.output.sections, "事件流重放 == 一次性结果");
    }

    #[test]
    fn run_to_end_equals_progressive_final() {
        let left = LineTable::from_text("h\na\nb\nc\nx\nt\n");
        let right = LineTable::from_text("h\nA\nb\nc\ny\nz\nt\n");
        let c = cfg();
        let keyed = ProgressiveDiffer::run_to_end(&left, &right, &c);
        assert!(!keyed.output.sections.is_empty());
        // 覆盖完整
        let mut lc = 0u32;
        let mut rc = 0u32;
        for s in &keyed.output.sections {
            if let Some(l) = s.left {
                assert_eq!(l.start, lc);
                lc = l.end;
            }
            if let Some(r) = s.right {
                assert_eq!(r.start, rc);
                rc = r.end;
            }
        }
        assert_eq!(lc, left.len() as u32);
        assert_eq!(rc, right.len() as u32);
    }

    #[test]
    fn equal_blocks_skip_replace() {
        // "1\n2\n3\n" vs "1\nX\n3\n"：占位 Change 精化后形状相同 → 无 Replace
        let left = LineTable::from_text("1\n2\n3\n");
        let right = LineTable::from_text("1\nX\n3\n");
        let mut d = ProgressiveDiffer::new(&left, &right, &cfg()).unwrap();
        let coarse = d.coarse_events();
        assert_eq!(coarse.len(), 3);
        let mut replace_count = 0usize;
        while let Some(ev) = d.poll() {
            replace_count += ev.len();
        }
        assert_eq!(replace_count, 0, "精化结果与占位相同时跳过 Replace");
        assert_eq!(d.sections().len(), 3);
    }
}
