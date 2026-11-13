//! section 化输出模型（docs/03 §4.1 TextDiffResult：equal/insert/delete/change
//! section 序列 + 流式 SectionEvent + DiffSummary）。

/// section id（0 起连续编号，同一 diff 输出内稳定）
pub type SectionId = u32;

/// section 类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionKind {
    Equal,
    /// 仅右侧有行（新增）
    Insert,
    /// 仅左侧有行（删除）
    Delete,
    /// 左右均有行但不同（替换；相邻 Delete+Insert 合并而成）
    Change,
}

/// 重要性（FR-TXT-010；P0 恒为 Important，P1 由规则系统重判）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Importance {
    Important,
    Unimportant,
    Ignored,
}

/// 行区间 [start, end)，0 起、半开
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

impl LineRange {
    pub fn new(start: u32, end: u32) -> Self {
        LineRange { start, end }
    }
    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// diff section（不可变结果模型，docs/03 §6.2-4）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub id: SectionId,
    pub kind: SectionKind,
    /// 左侧行区间（Insert 时为 None）
    pub left: Option<LineRange>,
    /// 右侧行区间（Delete 时为 None）
    pub right: Option<LineRange>,
    pub important: Importance,
    /// 行内差异状态（懒计算；P0 恒为 NotComputed）
    pub inline: InlineState,
}

/// 行内差异计算状态（docs/03 §4.1 InlineState）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineState {
    NotComputed,
    Partial,
    Full,
}

/// 流式事件（docs/03 §4.1 SectionEvent；Replace/ImportanceUpdate 为 P1 增量重算预留）
#[derive(Debug, Clone, PartialEq)]
pub enum SectionEvent {
    Append(Section),
    /// 编辑后局部重算（P1，imara-diff 历史驱动）
    Replace { span: (SectionId, SectionId), with: Vec<Section> },
    /// 重要性规则变更后重判（不重跑 diff）
    ImportanceUpdate { ids: Vec<SectionId>, important: Importance },
    Done(DiffSummary),
}

/// diff 统计
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffSummary {
    pub section_count: u32,
    pub equal_sections: u32,
    pub insert_sections: u32,
    pub delete_sections: u32,
    pub change_sections: u32,
    pub equal_lines: u32,
    pub inserted_lines: u32,
    pub deleted_lines: u32,
}

impl DiffSummary {
    fn from_sections(sections: &[Section]) -> Self {
        let mut s = DiffSummary::default();
        for sec in sections {
            s.section_count += 1;
            match sec.kind {
                SectionKind::Equal => {
                    s.equal_sections += 1;
                    s.equal_lines += sec.left.map_or(0, |r| r.len());
                }
                SectionKind::Insert => {
                    s.insert_sections += 1;
                    s.inserted_lines += sec.right.map_or(0, |r| r.len());
                }
                SectionKind::Delete => {
                    s.delete_sections += 1;
                    s.deleted_lines += sec.left.map_or(0, |r| r.len());
                }
                SectionKind::Change => {
                    s.change_sections += 1;
                    s.deleted_lines += sec.left.map_or(0, |r| r.len());
                    s.inserted_lines += sec.right.map_or(0, |r| r.len());
                }
            }
        }
        s
    }
}

/// 一次性 diff 输出
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOutput {
    pub sections: Vec<Section>,
    pub summary: DiffSummary,
}

/// diff 入口错误（算法层；P0 仅一种）
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DiffError {
    #[error("不支持的行对齐算法（P0 仅实现 Standard/Myers）")]
    UnsupportedAlgorithm(crate::options::LineAlignAlgo),
}

/// 由合并后的 RawOp 序列构建 section 序列（相邻 Delete+Insert 合并为 Change）。
pub fn build_sections(ops: &[crate::myers::RawOp]) -> DiffOutput {
    let mut sections: Vec<Section> = Vec::new();
    let mut la = 0u32; // 左侧行游标
    let mut lr = 0u32; // 右侧行游标
    let mut i = 0usize;
    let mut next_id: SectionId = 0;

    while i < ops.len() {
        match ops[i] {
            crate::myers::RawOp::Equal(len) => {
                sections.push(Section {
                    id: next_id,
                    kind: SectionKind::Equal,
                    left: Some(LineRange::new(la, la + len)),
                    right: Some(LineRange::new(lr, lr + len)),
                    important: Importance::Important,
                    inline: InlineState::NotComputed,
                });
                next_id += 1;
                la += len;
                lr += len;
                i += 1;
            }
            crate::myers::RawOp::Delete(dlen) => {
                // 后随 Insert -> Change
                let ilen = match ops.get(i + 1) {
                    Some(crate::myers::RawOp::Insert(n)) => *n,
                    _ => 0,
                };
                let (kind, right) = if ilen > 0 {
                    i += 1;
                    (SectionKind::Change, Some(LineRange::new(lr, lr + ilen)))
                } else {
                    (SectionKind::Delete, None)
                };
                sections.push(Section {
                    id: next_id,
                    kind,
                    left: Some(LineRange::new(la, la + dlen)),
                    right,
                    important: Importance::Important,
                    inline: InlineState::NotComputed,
                });
                next_id += 1;
                la += dlen;
                lr += ilen;
                i += 1;
            }
            crate::myers::RawOp::Insert(ilen) => {
                // 后随 Delete -> Change（Myers 产出顺序可能是 Ins+Del，规范化为同一 Change）
                let dlen = match ops.get(i + 1) {
                    Some(crate::myers::RawOp::Delete(n)) => *n,
                    _ => 0,
                };
                let (kind, left) = if dlen > 0 {
                    i += 1;
                    (SectionKind::Change, Some(LineRange::new(la, la + dlen)))
                } else {
                    (SectionKind::Insert, None)
                };
                sections.push(Section {
                    id: next_id,
                    kind,
                    left,
                    right: Some(LineRange::new(lr, lr + ilen)),
                    important: Importance::Important,
                    inline: InlineState::NotComputed,
                });
                next_id += 1;
                la += dlen;
                lr += ilen;
                i += 1;
            }
        }
    }

    let summary = DiffSummary::from_sections(&sections);
    DiffOutput { sections, summary }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::myers::{coalesce_ops, RawOp};

    #[test]
    fn delete_plus_insert_becomes_change() {
        let out = build_sections(&[RawOp::Equal(2), RawOp::Delete(3), RawOp::Insert(5), RawOp::Equal(1)]);
        assert_eq!(out.sections.len(), 3);
        assert_eq!(out.sections[0].kind, SectionKind::Equal);
        assert_eq!(out.sections[1].kind, SectionKind::Change);
        assert_eq!(out.sections[1].left, Some(LineRange::new(2, 5)));
        assert_eq!(out.sections[1].right, Some(LineRange::new(2, 7)));
        assert_eq!(out.sections[2].kind, SectionKind::Equal);
        assert_eq!(out.summary.deleted_lines, 3);
        assert_eq!(out.summary.inserted_lines, 5);
        assert_eq!(out.summary.equal_lines, 3);
    }

    #[test]
    fn lone_delete_and_lone_insert() {
        let out = build_sections(&[RawOp::Delete(2)]);
        assert_eq!(out.sections[0].kind, SectionKind::Delete);
        assert_eq!(out.sections[0].right, None);
        let out = build_sections(&[RawOp::Insert(4)]);
        assert_eq!(out.sections[0].kind, SectionKind::Insert);
        assert_eq!(out.sections[0].left, None);
    }

    #[test]
    fn ids_are_sequential() {
        let out = build_sections(&coalesce_ops(vec![
            RawOp::Equal(1),
            RawOp::Delete(1),
            RawOp::Insert(1),
            RawOp::Equal(1),
            RawOp::Delete(1),
            RawOp::Equal(1),
        ]));
        for (i, s) in out.sections.iter().enumerate() {
            assert_eq!(s.id, i as SectionId);
        }
        assert_eq!(out.summary.section_count, 5);
    }
}
