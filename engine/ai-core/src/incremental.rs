//! 编辑增量重 diff（docs/04 P1 交付物 ④；docs/03 §4.1 Replace 事件、§6.2）。
//!
//! [`DiffEngine`] 持有两侧行表 + 当前 sections。单行/区间编辑只重算
//! **受影响窗口**：窗口以最近的 Equal section 边界（对齐锚点）为界，窗口外
//! 对齐关系保持不变。返回事件流：
//!
//! - `Replace { span, with }`：span 为被替换的首尾 section id（重编号前），
//!   `with` 为重算出的新 sections（重编号后的新 id）；span 之后的 section id
//!   按 `with.len() - span 长度` 平移（全列表始终 0 起连续编号）；
//! - 文件尾/空文件插入以 `Append` 表达（语义 = 列表尾部追加）；
//! - `Done(summary)`：新统计。
//!
//! 契约：窗口重算与"全量重 diff"结构等价（由随机编辑 oracle 测试保证）。

use crate::keys;
use crate::lines::{LineEntry, LineTable};
use crate::options::{DiffConfig, DiffSide, ManualAlignOptions};
use crate::pipeline;
use crate::sections::{renumber, DiffError, DiffSummary, Importance, LineRange, Section, SectionEvent, SectionKind};

/// 一次行区间替换编辑：`[start, end)` 行被 `new_text`（按行分割）替换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub side: DiffSide,
    pub start: u32,
    pub end: u32,
    pub new_text: String,
}

/// 增量 diff 会话状态。
pub struct DiffEngine {
    left: LineTable,
    right: LineTable,
    cfg: DiffConfig,
    sections: Vec<Section>,
    summary: DiffSummary,
}

impl DiffEngine {
    /// 构造：一次性全量 diff 初始化（任意算法；Standard 走分块管线）。
    pub fn new(left: LineTable, right: LineTable, cfg: DiffConfig) -> Result<Self, DiffError> {
        let out = pipeline::run_full(&left, &right, &cfg)?;
        let mut sections = out.output.sections;
        let summary = renumber(&mut sections);
        Ok(DiffEngine { left, right, cfg, sections, summary })
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn summary(&self) -> DiffSummary {
        self.summary
    }

    pub fn tables(&self) -> (&LineTable, &LineTable) {
        (&self.left, &self.right)
    }

    pub fn config(&self) -> &DiffConfig {
        &self.cfg
    }

    /// 应用一次编辑：只重算受影响窗口，返回 `[Replace, Done]`（或尾部插入的
    /// `Append` 序列）。
    pub fn apply_edit(&mut self, edit: &TextEdit) -> Result<Vec<SectionEvent>, DiffError> {
        let side_len = match edit.side {
            DiffSide::Left => self.left.len() as u32,
            DiffSide::Right => self.right.len() as u32,
        };
        if edit.start > edit.end || edit.end > side_len {
            return Err(DiffError::InvalidEdit(format!(
                "编辑区间 [{}, {}) 越界（该侧 {side_len} 行）",
                edit.start, edit.end
            )));
        }
        let new_entries = parse_edit_text(&edit.new_text);
        let n_new = new_entries.len() as u32;
        let delta = n_new as isize - (edit.end - edit.start) as isize;
        let (a0, a1) = (edit.start, edit.end);

        // ---- 1. 窗口边界（编辑前坐标，(本侧, 对侧)）----
        let (ws, we) = self.window_bounds(edit.side, a0, a1);

        // ---- 2. 被替换的 section 下标范围（编辑前列表）----
        let replace = self.window_section_range(edit.side, ws, we);

        // ---- 3. 手动选项调整（编辑侧平移/丢弃）----
        let adjusted = self.adjust_manual(edit.side, a0, a1, delta);

        // ---- 4. 行表落盘 + 窗口范围（新坐标）----
        match edit.side {
            DiffSide::Left => self.left.replace_lines_entries(a0 as usize, a1 as usize, new_entries),
            DiffSide::Right => self.right.replace_lines_entries(a0 as usize, a1 as usize, new_entries),
        }
        let (lr, rr) = match edit.side {
            DiffSide::Left => ((ws.0, (we.0 as isize + delta) as u32), (ws.1, we.1)),
            DiffSide::Right => ((ws.1, we.1), (ws.0, (we.0 as isize + delta) as u32)),
        };

        // ---- 5. 窗口重算（全量坐标 sections）----
        let mut window_secs = pipeline::run_range(&self.left, &self.right, &self.cfg, lr, rr, &adjusted)?;
        let win_len = window_secs.len();

        // ---- 6. 拼回全量列表（窗口后的 sections 编辑侧平移 delta + 边界 Equal 吸收）----
        let (start_idx, span) = self.splice_window(replace, &mut window_secs, edit.side, delta)?;

        // ---- 7. 重编号 + 统计 + 事件 ----
        self.summary = renumber(&mut self.sections);
        let with: Vec<Section> = self.sections[start_idx..start_idx + win_len].to_vec();
        let events = match span {
            None => with.into_iter().map(SectionEvent::Append).collect(),
            Some(span) => {
                vec![SectionEvent::Replace { span, with }, SectionEvent::Done(self.summary)]
            }
        };
        Ok(events)
    }

    /// 重要性规则变更：不重跑 diff，只重判（docs/03 §4.1 ImportanceUpdate）。
    /// 返回按重要性分组的事件序列（末尾附 Done）。
    pub fn reclassify_importance(&mut self, imp: crate::options::ImportanceOptions) -> Vec<SectionEvent> {
        self.cfg.importance = imp;
        let classification = keys::classify_sections(
            &self.sections,
            &self.left,
            &self.right,
            &self.cfg.base,
            &self.cfg.importance,
            &self.cfg.manual,
        );
        let changed = keys::apply_classification(&mut self.sections, &classification);
        let mut events = Vec::new();
        for imp in [Importance::Unimportant, Importance::Ignored, Importance::Important] {
            let ids: Vec<u32> = changed
                .iter()
                .filter(|id| self.sections.get(**id as usize).is_some_and(|s| s.important == imp))
                .copied()
                .collect();
            if !ids.is_empty() {
                events.push(SectionEvent::ImportanceUpdate { ids, important: imp });
            }
        }
        self.summary = renumber(&mut self.sections);
        events.push(SectionEvent::Done(self.summary));
        events
    }

    /// 窗口边界（编辑前坐标）：ws = 最大的 Equal 边界 <= a0（无则 (0,0)）；
    /// we = 最小的 Equal 边界 >= a1（无则文件尾）；纯插入恰落在边界上时
    /// 向后扩一段（保证窗口非空，插入位置参与相邻差异区重评估）。
    fn window_bounds(&self, side: DiffSide, a0: u32, a1: u32) -> ((u32, u32), (u32, u32)) {
        let mut boundaries: Vec<(u32, u32)> = Vec::new();
        for sec in &self.sections {
            if sec.kind == SectionKind::Equal {
                if let (Some(l), Some(r)) = (sec.left, sec.right) {
                    let (mine, other) = match side {
                        DiffSide::Left => (l, r),
                        DiffSide::Right => (r, l),
                    };
                    boundaries.push((mine.start, other.start));
                    boundaries.push((mine.end, other.end));
                }
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();

        let ws = boundaries.iter().rev().find(|(l, _)| *l <= a0).copied().unwrap_or((0, 0));
        let mut we = boundaries
            .iter()
            .find(|(l, _)| *l >= a1)
            .copied()
            .unwrap_or_else(|| self.side_lens(side));
        if we.0 <= ws.0 {
            we = boundaries
                .iter()
                .find(|(l, _)| *l > ws.0)
                .copied()
                .unwrap_or_else(|| self.side_lens(side));
        }
        (ws, we)
    }

    /// (本侧行数, 对侧行数)
    fn side_lens(&self, side: DiffSide) -> (u32, u32) {
        match side {
            DiffSide::Left => (self.left.len() as u32, self.right.len() as u32),
            DiffSide::Right => (self.right.len() as u32, self.left.len() as u32),
        }
    }

    /// 窗口覆盖的 section 下标范围（编辑前列表；按本侧区间与游标判定）。
    /// 空窗口（文件尾插入）返回空 Range。
    ///
    /// 零宽 section（本侧 Insert）除游标落点外，还要求**对侧区间**完整落在
    /// 窗口对侧范围内 —— 边界并列时（如 (105,105)/(105,107)），对侧越界的
    /// Insert 不属于窗口内容，留在窗外才能保持两侧覆盖连续。
    fn window_section_range(
        &self,
        side: DiffSide,
        ws: (u32, u32),
        we: (u32, u32),
    ) -> std::ops::Range<usize> {
        let mut first: Option<usize> = None;
        let mut last = 0usize;
        let mut my_cursor = 0u32;
        for (i, sec) in self.sections.iter().enumerate() {
            let my_interval: Option<(u32, u32)> = match side {
                DiffSide::Left => sec.left.map(|l| (l.start, l.end)),
                DiffSide::Right => sec.right.map(|r| (r.start, r.end)),
            };
            let other_interval: Option<(u32, u32)> = match side {
                DiffSide::Left => sec.right.map(|r| (r.start, r.end)),
                DiffSide::Right => sec.left.map(|l| (l.start, l.end)),
            };
            let in_window = match my_interval {
                Some((s, e)) => e > ws.0 && s < we.0,
                None => {
                    let at_boundary = ws.0 <= my_cursor && my_cursor < we.0;
                    let other_inside = match other_interval {
                        Some((os, oe)) => os >= ws.1 && oe <= we.1,
                        None => true,
                    };
                    at_boundary && other_inside
                }
            };
            if in_window {
                first.get_or_insert(i);
                last = i;
            }
            if let Some((_, e)) = my_interval {
                my_cursor = e;
            }
        }
        match first {
            Some(f) => f..last + 1,
            None => 0..0,
        }
    }

    /// 手动选项调整：编辑侧的锚点/隔离/忽略区间跨越 [a0,a1) 时丢弃，
    /// 位于其后的按 delta 平移（全量坐标）。
    fn adjust_manual(&self, side: DiffSide, a0: u32, a1: u32, delta: isize) -> ManualAlignOptions {
        let mut m = self.cfg.manual.clone();
        let shift_range = |r: &mut LineRange| {
            if r.end <= a0 {
                // 编辑之前：坐标不变
            } else if r.start >= a1 {
                r.start = (r.start as isize + delta) as u32;
                r.end = (r.end as isize + delta) as u32;
            } else {
                *r = LineRange::new(0, 0); // 跨越编辑区间 -> 丢弃
            }
        };
        for a in m.align_anchors.iter_mut() {
            match side {
                DiffSide::Left => shift_range(&mut a.left),
                DiffSide::Right => shift_range(&mut a.right),
            }
        }
        m.align_anchors.retain(|a| !a.left.is_empty() && !a.right.is_empty());
        let filter_side = |(s, r): &(DiffSide, LineRange)| -> Option<(DiffSide, LineRange)> {
            let mut r = *r;
            if *s == side {
                shift_range(&mut r);
                if r.is_empty() {
                    return None;
                }
            }
            Some((*s, r))
        };
        m.isolate = m.isolate.iter().filter_map(filter_side).collect();
        m.ignore = m.ignore.iter().filter_map(filter_side).collect();
        m
    }

    /// 把窗口 sections 拼回全量列表。
    ///
    /// 1. 窗口**之后**的 sections 在编辑侧坐标平移 delta（其内容整体位移）；
    /// 2. 边界吸收：窗口边缘 section 与相邻既有 section 同类且两侧连续时
    ///    就地合并（Equal/Change/Delete/Insert 皆可；重要性须一致）——
    ///    相邻 Change 合并使增量结果与全量重 diff 的块结构一致；
    /// 3. 以收缩后的窗口替换原区间。
    ///
    /// 返回 (with 起点下标, 被替换 span 的 id；None = 尾部追加)。
    /// `with` 区域包含被吸收的相邻 section（合并结果），与 span 严格对应。
    fn splice_window(
        &mut self,
        replace: std::ops::Range<usize>,
        window: &mut Vec<Section>,
        side: DiffSide,
        delta: isize,
    ) -> Result<(usize, Option<(u32, u32)>), DiffError> {
        if replace.is_empty() {
            // 窗口为空仅发生在文件尾插入：Append 语义
            let at = self.sections.len();
            self.sections.append(window);
            return Ok((at, None));
        }
        // 窗口之后的 sections：编辑侧坐标整体平移 delta（对侧不变）
        for sec in &mut self.sections[replace.end..] {
            let r = match side {
                DiffSide::Left => sec.left.as_mut(),
                DiffSide::Right => sec.right.as_mut(),
            };
            if let Some(r) = r {
                r.start = (r.start as isize + delta) as u32;
                r.end = (r.end as isize + delta) as u32;
            }
        }
        let mut with_start = replace.start;
        let mut with_end = replace.end; // 独占下标（span 覆盖 [with_start, with_end)）
        // 头部吸收
        while let Some(first) = window.first() {
            if with_start == 0 {
                break;
            }
            let prev_idx = with_start - 1;
            if !sections_mergeable(&self.sections[prev_idx], first) {
                break;
            }
            let merged = merge_two(&self.sections[prev_idx], first);
            self.sections[prev_idx] = merged;
            window.remove(0);
            with_start -= 1;
        }
        // 尾部吸收
        while let Some(last) = window.last() {
            if with_end >= self.sections.len() {
                break;
            }
            let next_idx = with_end;
            if !sections_mergeable(&self.sections[next_idx], last) {
                break;
            }
            let merged = merge_two(&self.sections[next_idx], last);
            self.sections[next_idx] = merged;
            window.pop();
            with_end += 1;
        }
        let old_first = self.sections[with_start].id;
        let old_last = self.sections[with_end - 1].id;
        let spliced = std::mem::take(window);
        self.sections.splice(replace.start..replace.end, spliced);
        // with 区域：吸收后的相邻 section + 窗口剩余（位置已随 splice 确定）
        Ok((with_start, Some((old_first, old_last))))
    }
}

/// 两个相邻 section 是否可合并：同类、重要性一致、两侧区间连续（或同为 None）。
fn sections_mergeable(prev: &Section, next: &Section) -> bool {
    if prev.kind != next.kind || prev.important != next.important {
        return false;
    }
    let left_ok = match (prev.left, next.left) {
        (Some(a), Some(b)) => a.end == b.start,
        (None, None) => true,
        _ => false,
    };
    let right_ok = match (prev.right, next.right) {
        (Some(a), Some(b)) => a.end == b.start,
        (None, None) => true,
        _ => false,
    };
    left_ok && right_ok
}

/// 合并 next 到 prev（前提：[`sections_mergeable`] 为真）。
fn merge_two(prev: &Section, next: &Section) -> Section {
    let mut merged = *prev;
    if let (Some(a), Some(b)) = (prev.left, next.left) {
        debug_assert_eq!(a.end, b.start, "合并前左区间必须连续");
        merged.left = Some(LineRange::new(a.start, b.end));
    }
    if let (Some(a), Some(b)) = (prev.right, next.right) {
        debug_assert_eq!(a.end, b.start, "合并前右区间必须连续");
        merged.right = Some(LineRange::new(a.start, b.end));
    }
    merged
}

/// 编辑文本 -> 行条目（与 [`LineTable::from_text`] 同一分割规则）。
fn parse_edit_text(text: &str) -> Vec<LineEntry> {
    LineTable::from_text(text).iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{ImportanceOptions, LineAlignAlgo, TextDiffOptions};

    fn cfg_algo(algo: LineAlignAlgo) -> DiffConfig {
        DiffConfig {
            base: TextDiffOptions { algo, ..TextDiffOptions::default() },
            importance: ImportanceOptions::default(),
            manual: ManualAlignOptions::default(),
        }
    }

    fn engine(a: &str, b: &str) -> DiffEngine {
        DiffEngine::new(LineTable::from_text(a), LineTable::from_text(b), DiffConfig::default()).unwrap()
    }

    /// sections 无缝覆盖 + Equal 真等 + 重放还原右侧全文
    fn assert_sane(left: &LineTable, right: &LineTable, secs: &[Section]) {
        let mut lc = 0u32;
        let mut rc = 0u32;
        let mut rebuilt = String::new();
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
                        assert_eq!(
                            left.line((l.start + k) as usize).text,
                            right.line((r.start + k) as usize).text,
                            "Equal 段两侧必须一致"
                        );
                        let e = left.line((l.start + k) as usize);
                        rebuilt.push_str(&e.text);
                        rebuilt.push_str(e.ending.as_str());
                    }
                }
                SectionKind::Change | SectionKind::Insert => {
                    let r = s.right.expect("Change/Insert 必有右区间");
                    for i in r.start..r.end {
                        let e = right.line(i as usize);
                        rebuilt.push_str(&e.text);
                        rebuilt.push_str(e.ending.as_str());
                    }
                }
                SectionKind::Delete => {}
            }
        }
        assert_eq!(lc, left.len() as u32, "左侧必须无缝覆盖");
        assert_eq!(rc, right.len() as u32, "右侧必须无缝覆盖");
        assert_eq!(rebuilt, right.to_text(), "重放必须还原右侧全文");
    }

    fn assert_ids_contiguous(secs: &[Section]) {
        for (i, s) in secs.iter().enumerate() {
            assert_eq!(s.id, i as u32, "ids 必须 0 起连续");
        }
    }

    fn shape(eng: &DiffEngine) -> Vec<(SectionKind, Option<LineRange>, Option<LineRange>)> {
        eng.sections().iter().map(|s| (s.kind, s.left, s.right)).collect()
    }

    #[test]
    fn single_char_edit_touches_only_local_window() {
        // 大文件：每 10 行一改 -> Change/Equal 交替；编辑一个 Equal 行
        let mut a = String::new();
        let mut b = String::new();
        for i in 0..1000 {
            a.push_str(&format!("line {i}\n"));
            if i % 10 == 0 {
                b.push_str(&format!("line {i} changed\n"));
            } else {
                b.push_str(&format!("line {i}\n"));
            }
        }
        let mut eng = engine(&a, &b);
        let before: Vec<Section> = eng.sections().to_vec();
        assert_eq!(before.len(), 200, "100 Change + 100 Equal");
        assert_ids_contiguous(&before);
        assert_eq!(before[0].kind, SectionKind::Change, "行 0 已改 -> 首 Change");
        assert_eq!(before[101].kind, SectionKind::Equal, "行 501..509 为 Equal");

        // 行 501（Equal 段内）只改一个字符
        let edit = TextEdit { side: DiffSide::Left, start: 501, end: 502, new_text: "line 501!".into() };
        let events = eng.apply_edit(&edit).unwrap();
        assert_eq!(events.len(), 2, "Replace + Done");
        let (span, with) = match &events[0] {
            SectionEvent::Replace { span, with } => (*span, with.clone()),
            other => panic!("首事件应为 Replace: {other:?}"),
        };
        // 事件范围断言：行 501 所在 Equal section（id=101）重算为 Change+Equal，
        // 其 Change 与相邻 Change(500) 同类合并（与全量重 diff 块结构一致）
        assert_eq!(span, (100, 101), "span = 相邻 Change 合并 + 所在 Equal: {span:?}");
        assert_eq!(with.len(), 2, "with = 合并后 Change(500..502) + Equal(502..510)");
        assert_eq!(with[0].kind, SectionKind::Change);
        assert_eq!(with[0].left, Some(LineRange::new(500, 502)));
        assert_eq!(with[1].kind, SectionKind::Equal);
        assert_eq!(with[1].left, Some(LineRange::new(502, 510)));
        // span 外的 sections（除 id 平移）完全一致
        let after: Vec<Section> = eng.sections().to_vec();
        assert_ids_contiguous(&after);
        let shift = with.len() as isize - (span.1 - span.0 + 1) as isize; // = 0（1→2 替换 2→2）
        for i in (span.1 as usize + 1)..after.len() {
            let old = &before[(i as isize - shift) as usize];
            assert_eq!((old.kind, old.left, old.right), (after[i].kind, after[i].left, after[i].right), "span 外不得变化（下标 {i}）");
        }
        // span 之前的 sections 完全一致
        for i in 0..span.0 as usize {
            assert_eq!((before[i].kind, before[i].left, before[i].right), (after[i].kind, after[i].left, after[i].right), "span 前不得变化（下标 {i}）");
        }
        // 与全量重 diff 结构等价
        let fresh = engine(eng.tables().0.to_text().as_str(), eng.tables().1.to_text().as_str());
        assert_eq!(shape(&fresh),
        shape(&eng), "增量结果必须与全量重 diff 等价");
        assert_sane(eng.tables().0, eng.tables().1, eng.sections());
        assert!(matches!(events[1], SectionEvent::Done(_)));
    }

    #[test]
    fn edit_inside_change_section() {
        let mut eng = engine("alpha\nold line\nomega\n", "alpha\nold line\nomega\n");
        let edit = TextEdit { side: DiffSide::Right, start: 1, end: 2, new_text: "new line\n".into() };
        let events = eng.apply_edit(&edit).unwrap();
        assert!(matches!(events[0], SectionEvent::Replace { .. }));
        assert!(matches!(events[1], SectionEvent::Done(_)));
        assert_sane(eng.tables().0, eng.tables().1, eng.sections());
        // Change section 统计口径：deleted 与 inserted 都按行计入（P0 语义）
        assert_eq!(eng.summary().inserted_lines, 1);
        assert_eq!(eng.summary().deleted_lines, 1);
        assert_eq!(eng.summary().change_sections, 1);
    }

    #[test]
    fn pure_insert_at_aligned_boundary() {
        let mut eng = engine("a\nb\nc\n", "a\nb\nc\n");
        let edit = TextEdit { side: DiffSide::Right, start: 1, end: 1, new_text: "x\n".into() };
        let events = eng.apply_edit(&edit).unwrap();
        match &events[0] {
            SectionEvent::Replace { span, with } => {
                assert_eq!(span.0, span.1);
                assert!(!with.is_empty());
            }
            other => panic!("应为 Replace: {other:?}"),
        }
        assert_sane(eng.tables().0, eng.tables().1, eng.sections());
        let fresh = engine("a\nb\nc\n", "a\nx\nb\nc\n");
        assert_eq!(shape(&fresh),
        shape(&eng), "纯插入与全量重 diff 等价");
    }

    #[test]
    fn delete_range_and_multi_line_edit() {
        let mut eng = engine("a\nb\nc\nd\ne\n", "a\nb\nc\nd\ne\n");
        let edit = TextEdit { side: DiffSide::Left, start: 1, end: 3, new_text: "B1\nB2\nB3\n".into() };
        eng.apply_edit(&edit).unwrap();
        assert_sane(eng.tables().0, eng.tables().1, eng.sections());
        let fresh = engine(eng.tables().0.to_text().as_str(), eng.tables().1.to_text().as_str());
        assert_eq!(shape(&fresh),
        shape(&eng), "多行编辑与全量重 diff 等价");
    }

    #[test]
    fn append_at_file_end_uses_append_events() {
        let mut eng = engine("a\nb\n", "a\nb\n");
        let edit = TextEdit { side: DiffSide::Right, start: 2, end: 2, new_text: "c\nd\n".into() };
        let events = eng.apply_edit(&edit).unwrap();
        assert!(
            events.iter().all(|e| matches!(e, SectionEvent::Append(_))),
            "文件尾插入用 Append: {events:?}"
        );
        assert_sane(eng.tables().0, eng.tables().1, eng.sections());
        assert_eq!(eng.summary().inserted_lines, 2);
    }

    #[test]
    fn empty_file_first_insert() {
        let mut eng = engine("", "");
        let edit = TextEdit { side: DiffSide::Left, start: 0, end: 0, new_text: "hello\n".into() };
        let events = eng.apply_edit(&edit).unwrap();
        assert!(matches!(events[0], SectionEvent::Append(_)));
        assert_sane(eng.tables().0, eng.tables().1, eng.sections());
    }

    #[test]
    fn out_of_bounds_edit_rejected() {
        let mut eng = engine("a\n", "a\n");
        let edit = TextEdit { side: DiffSide::Left, start: 0, end: 5, new_text: String::new() };
        assert!(matches!(eng.apply_edit(&edit), Err(DiffError::InvalidEdit(_))));
    }

    #[test]
    fn random_edits_stay_valid() {
        // 随机编辑序列：增量结果必须始终是合法 diff（无缝覆盖、Equal 真等、
        // 重放还原右侧、ids 连续、统计一致）。与"全量重 diff"在歧义区域允许
        // 不同（但同样合法）的对齐 —— 增量语义以窗口边界（既有对齐锚点）为准。
        let mut state = 0x853C49E6748FEA9Bu64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let base: String = (0..120).map(|i| format!("shared {i}\n")).collect();
        let mut eng = DiffEngine::new(
            LineTable::from_text(&base),
            LineTable::from_text(&base),
            cfg_algo(LineAlignAlgo::Patience),
        )
        .unwrap();
        for round in 0..40 {
            let side = if next() % 2 == 0 { DiffSide::Left } else { DiffSide::Right };
            let n = match side {
                DiffSide::Left => eng.tables().0.len() as u32,
                DiffSide::Right => eng.tables().1.len() as u32,
            };
            let s = (next() % (u64::from(n) + 1)) as u32;
            let e = s.saturating_add((next() % 3) as u32).min(n);
            let mut new_text = String::new();
            for _ in 0..next() % 3 {
                new_text.push_str(&format!("edit {}\n", next() % 97));
            }
            let edit = TextEdit { side, start: s, end: e, new_text };
            let events = eng.apply_edit(&edit).unwrap();
            assert!(!events.is_empty(), "round {round}: 必须产生事件");
            // 事件引用的 with sections 与引擎状态一致（Replace 场景）
            if let [SectionEvent::Replace { span, with }, SectionEvent::Done(_)] = &events[..] {
                let start_pos = eng
                    .sections()
                    .iter()
                    .position(|x| x.id == span.0)
                    .expect("span 起点存在");
                assert_eq!(&eng.sections()[start_pos..start_pos + with.len()], with.as_slice(), "with 必须与全量列表一致");
            }
            assert_ids_contiguous(eng.sections());
            assert_sane(eng.tables().0, eng.tables().1, eng.sections());
            // 统计与 sections 一致
            let mut s2 = crate::sections::DiffSummary::default();
            for sec in eng.sections() {
                s2.section_count += 1;
                match sec.kind {
                    SectionKind::Equal => { s2.equal_sections += 1; s2.equal_lines += sec.left.map_or(0, |r| r.len()); }
                    SectionKind::Insert => { s2.insert_sections += 1; s2.inserted_lines += sec.right.map_or(0, |r| r.len()); }
                    SectionKind::Delete => { s2.delete_sections += 1; s2.deleted_lines += sec.left.map_or(0, |r| r.len()); }
                    SectionKind::Change => { s2.change_sections += 1; s2.deleted_lines += sec.left.map_or(0, |r| r.len()); s2.inserted_lines += sec.right.map_or(0, |r| r.len()); }
                }
            }
            assert_eq!(s2, eng.summary(), "round {round}: 统计必须与 sections 一致");
        }
    }

    #[test]
    fn reclassify_emits_importance_update() {
        let mut eng = engine("Alpha\nkeep\n", "alpha \nkeep\n");
        assert_eq!(eng.sections()[0].important, Importance::Important);
        let imp = ImportanceOptions {
            case_unimportant: true,
            trailing_ws_unimportant: true,
            ..ImportanceOptions::default()
        };
        let events = eng.reclassify_importance(imp);
        match &events[0] {
            SectionEvent::ImportanceUpdate { ids, important } => {
                assert_eq!(ids, &vec![0]);
                assert_eq!(*important, Importance::Unimportant);
            }
            other => panic!("应为 ImportanceUpdate: {other:?}"),
        }
        assert_eq!(eng.sections()[0].important, Importance::Unimportant);
        assert!(matches!(events.last(), Some(SectionEvent::Done(_))));
    }

    #[test]
    fn summary_tracks_edits() {
        let mut eng = engine("a\nb\n", "a\nb\n");
        assert_eq!(eng.summary().change_sections, 0);
        eng.apply_edit(&TextEdit { side: DiffSide::Left, start: 1, end: 2, new_text: "B\n".into() }).unwrap();
        assert_eq!(eng.summary().change_sections, 1);
        assert_ids_contiguous(eng.sections());
    }
}
