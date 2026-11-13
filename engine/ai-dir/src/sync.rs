//! 同步状态机数据层（SYNC-002 / DMRG-003）：5 预设 + 自定义规则的
//! **operate 计划生成**（不执行；执行属 P6/P8 文件操作引擎，FR-DIR-067）。
//!
//! 纯函数：输入 [`ai_protocol::DirRow`] 可见行序列（树或平铺视图皆可），
//! 输出 [`PlannedOperation`] 计划。"较新"按 mtime 毫秒判定（与 BC 同源语义）。

use ai_protocol::{DirEntryStatus, DirRow, PlannedOpKind, PlannedOperation, RuleSide, SideInfo, SyncPreset, SyncRule};
use std::collections::BTreeSet;

use crate::filter::wildcard_match;

/// 依据预设生成操作计划（operate 的主入口）
pub fn plan_operations(
    rows: &[DirRow],
    preset: Option<SyncPreset>,
    rules: Option<&[SyncRule]>,
    selection: Option<&[String]>,
) -> Vec<PlannedOperation> {
    // 选择集过滤（None/空 = 全部）；遵循过滤器语义由调用方传"未隐藏行"
    let sel: Option<BTreeSet<&String>> = selection.map(|s| s.iter().collect());
    let mut plan = Vec::new();
    for row in rows {
        if let Some(sel) = &sel {
            if !sel.contains(&row.id) {
                continue;
            }
        }
        match rules {
            Some(rules) => apply_rules(row, rules, &mut plan),
            None => apply_preset(row, preset, &mut plan),
        }
    }
    plan
}

/// 是否"配对存在且状态为差异"（含目录聚合差异）
fn is_diff(status: DirEntryStatus) -> bool {
    matches!(
        status,
        DirEntryStatus::ContentDiff | DirEntryStatus::QuickDiff | DirEntryStatus::DirDiff
    )
}

/// 预设语义（FR-DIR-067 同步子菜单的 5 预设）
fn apply_preset(row: &DirRow, preset: Option<SyncPreset>, plan: &mut Vec<PlannedOperation>) {
    let Some(preset) = preset else { return };
    let reason = format!("preset:{preset:?}");
    let mt = |s: &Option<SideInfo>| s.as_ref().map(|x| x.mtime_ms).unwrap_or(0);
    match row.status {
        DirEntryStatus::LeftOnly => match preset {
            SyncPreset::MirrorLeftToRight | SyncPreset::UpdateLeftToRight => {
                push(plan, PlannedOpKind::CopyLeftToRight, &row.id, &reason);
            }
            SyncPreset::MirrorRightToLeft => {
                push(plan, PlannedOpKind::DeleteLeft, &row.id, &reason);
            }
            // 更新右→左 / 双向更新：左侧孤有项保留（不动）
            SyncPreset::UpdateRightToLeft | SyncPreset::UpdateBoth => {}
        },
        DirEntryStatus::RightOnly => match preset {
            SyncPreset::MirrorRightToLeft | SyncPreset::UpdateRightToLeft => {
                push(plan, PlannedOpKind::CopyRightToLeft, &row.id, &reason);
            }
            SyncPreset::MirrorLeftToRight => {
                push(plan, PlannedOpKind::DeleteRight, &row.id, &reason);
            }
            SyncPreset::UpdateLeftToRight | SyncPreset::UpdateBoth => {}
        },
        s if is_diff(s) => {
            let left_newer = mt(&row.left) >= mt(&row.right);
            let right_newer = mt(&row.right) > mt(&row.left);
            match preset {
                SyncPreset::MirrorLeftToRight => {
                    push(plan, PlannedOpKind::CopyLeftToRight, &row.id, &reason);
                }
                // 更新语义：仅当本侧不旧于对侧时复制（FR-DIR-067 Update）
                SyncPreset::UpdateLeftToRight => {
                    if mt(&row.left) >= mt(&row.right) {
                        push(plan, PlannedOpKind::CopyLeftToRight, &row.id, &reason);
                    }
                }
                SyncPreset::MirrorRightToLeft => {
                    push(plan, PlannedOpKind::CopyRightToLeft, &row.id, &reason);
                }
                SyncPreset::UpdateRightToLeft => {
                    if mt(&row.right) >= mt(&row.left) {
                        push(plan, PlannedOpKind::CopyRightToLeft, &row.id, &reason);
                    }
                }
                SyncPreset::UpdateBoth => {
                    if left_newer {
                        push(plan, PlannedOpKind::CopyLeftToRight, &row.id, &reason);
                    } else if right_newer {
                        push(plan, PlannedOpKind::CopyRightToLeft, &row.id, &reason);
                    } else {
                        // mtime 相等仍状态差异（如内容不同）：确定性取左→右
                        push(plan, PlannedOpKind::CopyLeftToRight, &row.id, &reason);
                    }
                }
            }
        }
        // 相同（含未判定）：不动
        _ => {}
    }
}

/// 自定义规则：按序首条命中生效；无命中 = 跳过
fn apply_rules(row: &DirRow, rules: &[SyncRule], plan: &mut Vec<PlannedOperation>) {
    for (idx, rule) in rules.iter().enumerate() {
        let mut hit = true;
        if let Some(mask) = &rule.name_mask {
            if !wildcard_match(mask, &row.name, false) {
                hit = false;
            }
        }
        if hit {
            if let Some(status) = rule.status {
                if row.status != status {
                    hit = false;
                }
            }
        }
        if hit {
            if let Some(side) = rule.newer {
                let l = row.left.as_ref().map(|s| s.mtime_ms).unwrap_or(0);
                let r = row.right.as_ref().map(|s| s.mtime_ms).unwrap_or(0);
                let newer_is_left = l >= r;
                let ok = match side {
                    RuleSide::Left => newer_is_left,
                    RuleSide::Right => !newer_is_left,
                };
                if !ok {
                    hit = false;
                }
            }
        }
        if hit {
            let reason = format!("rule#{idx}");
            if let Some(op) = Option::<PlannedOpKind>::from(rule.action) {
                push(plan, op, &row.id, &reason);
            }
            return; // 首条命中生效（含 Skip：终止规则链）
        }
    }
}

fn push(plan: &mut Vec<PlannedOperation>, op: PlannedOpKind, path: &str, reason: &str) {
    plan.push(PlannedOperation { op, path: path.to_string(), reason: reason.to_string() });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_protocol::{EntryKind, RuleAction, SideInfo};

    fn row(id: &str, status: DirEntryStatus, lm: u64, rm: u64) -> DirRow {
        DirRow {
            id: id.into(),
            name: id.into(),
            kind: EntryKind::File,
            left: Some(SideInfo::base(1, lm)),
            right: Some(SideInfo::base(1, rm)),
            status,
            has_children: false,
            hidden: false,
            suppressed: false,
        }
    }

    fn ops(plan: &[PlannedOperation]) -> Vec<(PlannedOpKind, &str)> {
        plan.iter().map(|p| (p.op, p.path.as_str())).collect()
    }

    #[test]
    fn mirror_left_to_right() {
        let rows = vec![
            row("lonly", DirEntryStatus::LeftOnly, 0, 0),
            row("ronly", DirEntryStatus::RightOnly, 0, 0),
            row("diff", DirEntryStatus::ContentDiff, 0, 0),
            row("same", DirEntryStatus::Same, 0, 0),
        ];
        let plan = plan_operations(&rows, Some(SyncPreset::MirrorLeftToRight), None, None);
        assert_eq!(
            ops(&plan),
            vec![
                (PlannedOpKind::CopyLeftToRight, "lonly"),
                (PlannedOpKind::DeleteRight, "ronly"),
                (PlannedOpKind::CopyLeftToRight, "diff"),
            ]
        );
    }

    #[test]
    fn mirror_right_to_left() {
        let rows = vec![
            row("lonly", DirEntryStatus::LeftOnly, 0, 0),
            row("ronly", DirEntryStatus::RightOnly, 0, 0),
            row("diff", DirEntryStatus::QuickDiff, 0, 0),
        ];
        let plan = plan_operations(&rows, Some(SyncPreset::MirrorRightToLeft), None, None);
        assert_eq!(
            ops(&plan),
            vec![
                (PlannedOpKind::DeleteLeft, "lonly"),
                (PlannedOpKind::CopyRightToLeft, "ronly"),
                (PlannedOpKind::CopyRightToLeft, "diff"),
            ]
        );
    }

    #[test]
    fn update_left_to_right_keeps_right_orphans_and_older_left() {
        let rows = vec![
            row("lonly", DirEntryStatus::LeftOnly, 0, 0),
            row("ronly", DirEntryStatus::RightOnly, 0, 0),
            row("lnewer", DirEntryStatus::QuickDiff, 200, 100),
            row("rnewer", DirEntryStatus::QuickDiff, 100, 200),
            row("same", DirEntryStatus::Same, 0, 0),
        ];
        let plan = plan_operations(&rows, Some(SyncPreset::UpdateLeftToRight), None, None);
        assert_eq!(
            ops(&plan),
            vec![(PlannedOpKind::CopyLeftToRight, "lonly"), (PlannedOpKind::CopyLeftToRight, "lnewer")]
        );
    }

    #[test]
    fn update_both_follows_newer_side() {
        let rows = vec![
            row("lnewer", DirEntryStatus::QuickDiff, 200, 100),
            row("rnewer", DirEntryStatus::QuickDiff, 100, 200),
            row("lonly", DirEntryStatus::LeftOnly, 0, 0),
        ];
        let plan = plan_operations(&rows, Some(SyncPreset::UpdateBoth), None, None);
        assert_eq!(
            ops(&plan),
            vec![(PlannedOpKind::CopyLeftToRight, "lnewer"), (PlannedOpKind::CopyRightToLeft, "rnewer")]
        );
    }

    #[test]
    fn update_right_to_left() {
        let rows = vec![
            row("rnewer", DirEntryStatus::QuickDiff, 100, 200),
            row("lnewer", DirEntryStatus::QuickDiff, 200, 100),
            row("ronly", DirEntryStatus::RightOnly, 0, 0),
        ];
        let plan = plan_operations(&rows, Some(SyncPreset::UpdateRightToLeft), None, None);
        assert_eq!(
            ops(&plan),
            vec![(PlannedOpKind::CopyRightToLeft, "rnewer"), (PlannedOpKind::CopyRightToLeft, "ronly")]
        );
    }

    #[test]
    fn dir_diff_and_selection_filter() {
        let rows = vec![
            row("sub", DirEntryStatus::DirDiff, 0, 0),
            row("other", DirEntryStatus::ContentDiff, 0, 0),
        ];
        let selected = ["sub".to_string()];
        let plan = plan_operations(
            &rows,
            Some(SyncPreset::MirrorLeftToRight),
            None,
            Some(selected.as_slice()),
        );
        assert_eq!(ops(&plan), vec![(PlannedOpKind::CopyLeftToRight, "sub")]);
    }

    #[test]
    fn custom_rules_first_match_wins_and_skip_terminates() {
        let rules = vec![
            SyncRule {
                name_mask: Some("*.tmp".into()),
                status: None,
                newer: None,
                action: RuleAction::Skip,
            },
            SyncRule {
                name_mask: None,
                status: Some(DirEntryStatus::LeftOnly),
                newer: None,
                action: RuleAction::CopyLeftToRight,
            },
        ];
        let rows = vec![
            row("a.tmp", DirEntryStatus::LeftOnly, 0, 0),
            row("b.txt", DirEntryStatus::LeftOnly, 0, 0),
            row("c.txt", DirEntryStatus::ContentDiff, 0, 0),
        ];
        let plan = plan_operations(&rows, None, Some(&rules), None);
        // a.tmp 命中 Skip（规则链终止）；b.txt 命中规则2；c.txt 无命中 = 跳过
        assert_eq!(ops(&plan), vec![(PlannedOpKind::CopyLeftToRight, "b.txt")]);
    }

    #[test]
    fn custom_rules_newer_side_condition() {
        let rules = vec![SyncRule {
            name_mask: None,
            status: Some(DirEntryStatus::QuickDiff),
            newer: Some(RuleSide::Right),
            action: RuleAction::CopyRightToLeft,
        }];
        let rows = vec![
            row("lnewer", DirEntryStatus::QuickDiff, 200, 100),
            row("rnewer", DirEntryStatus::QuickDiff, 100, 200),
        ];
        let plan = plan_operations(&rows, None, Some(&rules), None);
        assert_eq!(ops(&plan), vec![(PlannedOpKind::CopyRightToLeft, "rnewer")]);
    }

    #[test]
    fn no_preset_no_rules_empty_plan() {
        let rows = vec![row("a", DirEntryStatus::ContentDiff, 0, 0)];
        assert!(plan_operations(&rows, None, None, None).is_empty());
    }
}
