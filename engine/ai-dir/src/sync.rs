//! 同步状态机数据层（SYNC-002 / SYNC-004 / DMRG-003）：5 预设 + 自定义规则的
//! **operate 计划生成**（不执行；执行属 P6/P8 文件操作引擎，FR-DIR-067），
//! P8 升级：调度策略（[`SyncStrategy`]，预设内可配）+ 端到端同步
//! [`run_sync`]（计划 → 确认参数 → 执行 → 进度 → 结果报告 [`SyncReport`]，
//! 对齐 FR-SYNC 的报告/日志需求：copied/deleted/skipped/failed 统计 + 明细 + 日志行）。
//!
//! 纯函数：输入 [`ai_protocol::DirRow`] 可见行序列（树或平铺视图皆可），
//! 输出 [`PlannedOperation`] 计划。"较新"按 mtime 毫秒判定（与 BC 同源语义）。

use ai_protocol::{DirEntryStatus, DirRow, PlannedOpKind, PlannedOperation, RuleSide, SideInfo, SyncPreset, SyncRule};
use std::collections::BTreeSet;

use crate::execute::{ExecuteOptions, ProgressFn};
use crate::filter::wildcard_match;
use crate::{CancelFlag, Side};

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
    reorder_case_collisions(plan)
}

/// 大小写不敏感文件系统上，左孤 `A.txt` 与右孤 `a.txt` 实为同一条目：镜像计划
/// 若先 copy 后 delete，copy 原地覆写 `a.txt`，随后 delete 把刚写入的内容删掉
/// → 目标侧清空（数据丢失且退出码 0）。把碰撞的 delete 提前到对应 copy 之前：
/// 不敏感卷上删后按左名重建、敏感卷上两独立文件各自正确，两种语义下结果都对。
fn reorder_case_collisions(plan: Vec<PlannedOperation>) -> Vec<PlannedOperation> {
    fn fold(s: &str) -> String {
        s.chars().flat_map(|c| c.to_lowercase()).collect()
    }
    fn copy_dest(op: PlannedOpKind) -> Option<Side> {
        match op {
            PlannedOpKind::CopyLeftToRight => Some(Side::Right),
            PlannedOpKind::CopyRightToLeft => Some(Side::Left),
            _ => None,
        }
    }
    fn del_side(op: PlannedOpKind) -> Option<Side> {
        match op {
            PlannedOpKind::DeleteRight => Some(Side::Right),
            PlannedOpKind::DeleteLeft => Some(Side::Left),
            _ => None,
        }
    }
    let mut before: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    let mut moved: BTreeSet<usize> = BTreeSet::new();
    for (ci, c) in plan.iter().enumerate() {
        let Some(cs) = copy_dest(c.op) else { continue };
        let cf = fold(&c.path);
        for (di, d) in plan.iter().enumerate().skip(ci + 1) {
            if moved.contains(&di) {
                continue;
            }
            if del_side(d.op) == Some(cs) && d.path != c.path && fold(&d.path) == cf {
                before.entry(ci).or_default().push(di);
                moved.insert(di);
            }
        }
    }
    if moved.is_empty() {
        return plan;
    }
    let mut out = Vec::with_capacity(plan.len());
    for (i, p) in plan.iter().enumerate() {
        if moved.contains(&i) {
            continue;
        }
        if let Some(ds) = before.get(&i) {
            for &di in ds {
                out.push(plan[di].clone());
            }
        }
        out.push(p.clone());
    }
    out
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
            // Update 语义不得对目录行做整树复制：目录 mtime 只反映条目增删，
            // 不代表内容新旧；整树复制会无条件覆写子项中较新的文件（数据丢失）。
            // 子文件行各自携带 Update 语义，目录行在此跳过。
            if row.kind == ai_protocol::EntryKind::Dir
                && matches!(
                    preset,
                    SyncPreset::UpdateLeftToRight | SyncPreset::UpdateRightToLeft | SyncPreset::UpdateBoth
                )
            {
                return;
            }
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

// ===========================================================================
// P8：调度策略（SYNC-004 预设内可配项）
// ===========================================================================

/// 时间戳冲突策略（两侧内容不同且时间无法区分先后时的取侧规则）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// 较新优先：mtime 可分则取较新侧；相等时确定性取左（BC 默认语义）
    #[default]
    NewerWins,
    /// 左侧优先：两侧内容不同即以左为准（右侧被覆盖），不看 mtime
    LeftWins,
}

/// 同步调度策略（FR-SYNC-004 处理选项的调度面；预设生成计划后施加）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncStrategy {
    /// 删除孤立项开关：false = 镜像预设保留对侧孤立项（仅复制不删除）
    pub delete_orphans: bool,
    /// 时间戳冲突策略
    pub conflict_policy: ConflictPolicy,
    /// 只读侧：写入该侧的操作（复制进/删除/对时）全部跳过（FR-SYNC-005 抑制）
    pub readonly: Option<Side>,
    /// 同步空文件夹（FR-SYNC-004）：false = 孤立空目录跳过（不复制不删除）
    pub sync_empty_dirs: bool,
}

impl Default for SyncStrategy {
    fn default() -> Self {
        SyncStrategy {
            delete_orphans: true,
            conflict_policy: ConflictPolicy::NewerWins,
            readonly: None,
            sync_empty_dirs: true,
        }
    }
}

impl SyncStrategy {
    /// 操作是否写入指定侧（复制进目标侧 / 删除 / 对时均视为写入）
    fn writes_to(op: PlannedOpKind, side: Side) -> bool {
        matches!(
            (op, side),
            (PlannedOpKind::CopyLeftToRight, Side::Right)
                | (PlannedOpKind::CopyRightToLeft, Side::Left)
                | (PlannedOpKind::DeleteLeft, Side::Left)
                | (PlannedOpKind::DeleteRight, Side::Right)
                | (PlannedOpKind::TouchLeft, Side::Left)
                | (PlannedOpKind::TouchRight, Side::Right)
        )
    }
}

/// 策略化计划产物：`ops` 为待执行项；`skipped` 为被策略拦下（不执行）的项
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncPlan {
    pub ops: Vec<PlannedOperation>,
    pub skipped: Vec<PlannedOperation>,
}

/// 预设计划 + 策略施加（SYNC-004 调度面主入口）。
///
/// `is_empty_dir`：rel 路径 -> 是否为空目录的判定回调（仅在
/// `strategy.sync_empty_dirs == false` 时查询；None = 全部视为非空，
/// 供纯行序列场景使用；端到端 [`run_sync`] 以 VFS list_dir 实测填充）。
pub fn plan_with_strategy(
    rows: &[DirRow],
    preset: Option<SyncPreset>,
    rules: Option<&[SyncRule]>,
    selection: Option<&[String]>,
    strategy: &SyncStrategy,
    is_empty_dir: Option<&dyn Fn(&str) -> bool>,
) -> SyncPlan {
    let base = plan_operations(rows, preset, rules, selection);
    apply_strategy(base, rows, strategy, is_empty_dir)
}

/// 对既有计划施加策略（纯函数；plan_with_strategy 的可复用内核）
pub fn apply_strategy(
    plan: Vec<PlannedOperation>,
    rows: &[DirRow],
    strategy: &SyncStrategy,
    is_empty_dir: Option<&dyn Fn(&str) -> bool>,
) -> SyncPlan {
    // 行索引：op.path == row.id（P2 计划口径）
    let by_id: std::collections::HashMap<&str, &DirRow> =
        rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let mut out = SyncPlan::default();
    for op in plan {
        let row = by_id.get(op.path.as_str()).copied();
        let status = row.map(|r| r.status);
        let kind_is_dir = row.map(|r| r.kind == ai_protocol::EntryKind::Dir).unwrap_or(false);

        // 1. 只读侧跳过（优先级最高：目标侧不可写）
        if let Some(ro) = strategy.readonly {
            if SyncStrategy::writes_to(op.op, ro) {
                out.skipped.push(PlannedOperation {
                    op: op.op,
                    path: op.path,
                    reason: format!("{}; strategy:readonlySide({:?})", op.reason, ro),
                });
                continue;
            }
        }

        // 2. 冲突策略：左侧优先时，配对差异行的右→左复制改向为左→右
        let mut op = op;
        if strategy.conflict_policy == ConflictPolicy::LeftWins
            && op.op == PlannedOpKind::CopyRightToLeft
            && matches!(status, Some(DirEntryStatus::ContentDiff) | Some(DirEntryStatus::QuickDiff) | Some(DirEntryStatus::DirDiff))
        {
            op.op = PlannedOpKind::CopyLeftToRight;
            op.reason.push_str("; strategy:leftWins");
        }

        // 3. 删除孤儿开关：镜像对孤立项的删除被关闭
        if !strategy.delete_orphans
            && matches!(op.op, PlannedOpKind::DeleteLeft | PlannedOpKind::DeleteRight)
            && matches!(status, Some(DirEntryStatus::LeftOnly) | Some(DirEntryStatus::RightOnly))
        {
            op.reason.push_str("; strategy:keepOrphans");
            out.skipped.push(op);
            continue;
        }

        // 4. 同步空文件夹开关：孤立空目录的复制/删除跳过
        if !strategy.sync_empty_dirs
            && kind_is_dir
            && matches!(status, Some(DirEntryStatus::LeftOnly) | Some(DirEntryStatus::RightOnly))
            && is_empty_dir.is_some_and(|f| f(&op.path))
            && matches!(op.op, PlannedOpKind::CopyLeftToRight | PlannedOpKind::CopyRightToLeft | PlannedOpKind::DeleteLeft | PlannedOpKind::DeleteRight)
        {
            op.reason.push_str("; strategy:skipEmptyDir");
            out.skipped.push(op);
            continue;
        }

        out.ops.push(op);
    }
    out
}

// ===========================================================================
// P8：结果报告数据模型（FR-SYNC 报告/日志需求的对齐面）
// ===========================================================================

/// 单项记录状态（细分类别供统计卡；同步/合并报告共用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordStatus {
    /// 已复制（dry_run = 将复制）
    Copied,
    /// 已删除（dry_run = 将删除）
    Deleted,
    /// 已对时
    Touched,
    /// 计划在案但不动作（三方一致 / 冲突跳过 / 目录聚合占位）
    Planned,
    /// 未执行（策略跳过 / 已取消 / 目标缺失）
    Skipped,
    Failed,
}

/// 单项记录（协议友好展开式：kind 为动作类别标签，serde 侧直接可报）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpRecord {
    /// 动作类别标签（copyLeftToRight / deleteLeft / takeLeft / bothDir / ...）
    pub kind: String,
    pub path: String,
    /// 触发原因（预设/规则/策略/分类摘要）
    pub reason: String,
    pub status: RecordStatus,
    pub detail: Option<String>,
}

/// 批次统计（统计卡口径）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchStats {
    pub total: usize,
    pub copied: usize,
    pub deleted: usize,
    pub touched: usize,
    /// 计划在案但不动作（含策略跳过与冲突保留）
    pub skipped: usize,
    pub failed: usize,
}

/// 批次运行报告（同步/合并共用；对齐 FR-SYNC 报告 + 视图>日志面板）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchRunReport {
    pub dry_run: bool,
    /// 取消后中止（剩余项记 Skipped）
    pub cancelled: bool,
    pub stats: BatchStats,
    pub details: Vec<OpRecord>,
    /// 日志面板行（无时间戳：产出仅由输入内容决定，支撑 golden 对拍）
    pub log: Vec<String>,
}

impl BatchRunReport {
    /// 汇总日志行（"同步完成：复制 a / 删除 b / 跳过 c / 失败 d"）
    pub fn summary_line(&self, verb: &str) -> String {
        let s = &self.stats;
        format!(
            "{verb}：{} 项 — 复制 {} / 删除 {} / 对时 {} / 跳过 {} / 失败 {}{}",
            s.total,
            s.copied,
            s.deleted,
            s.touched,
            s.skipped,
            s.failed,
            if self.cancelled { "（已取消，剩余未执行）" } else { "" },
        )
    }
}

/// 同步请求（端到端入口参数；计划 → 确认 → 执行 → 报告）
#[derive(Debug, Clone, Default)]
pub struct SyncRequest<'a> {
    pub preset: Option<SyncPreset>,
    pub rules: Option<&'a [SyncRule]>,
    pub strategy: SyncStrategy,
    /// 选中行 id（None/空 = 全部）
    pub selection: Option<&'a [String]>,
    /// 试运行：只产出"将执行"报告不落盘
    pub dry_run: bool,
}

/// 同步结果报告（P8 交付物 1 的数据模型）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub preset: Option<SyncPreset>,
    pub strategy: Option<SyncStrategy>,
    pub batch: BatchRunReport,
    /// 最终计划（策略过滤后的 ops，供报告回放）
    pub plan: Vec<PlannedOperation>,
    /// 被策略拦下的操作
    pub strategy_skipped: Vec<PlannedOperation>,
}

/// 端到端同步（SYNC-005 运行同步）：计划 → 执行（进度/取消）→ 结果报告。
///
/// 流程对齐 FR-SYNC-001：接受按钮生成预览（UI 侧走 [`plan_with_strategy`]），
/// 确认后调本函数执行；`dry_run=true` 产出"将执行"报告不落盘。
/// 空目录判定：`strategy.sync_empty_dirs == false` 时对 Dir 行逐个 list_dir 实测。
pub async fn run_sync(
    vfs: &dyn ai_vfs::VfsProvider,
    roots: &(ai_vfs::VfsPath, ai_vfs::VfsPath),
    rows: &[DirRow],
    req: &SyncRequest<'_>,
    progress: Option<&mut ProgressFn<'_>>,
    cancel: &CancelFlag,
) -> SyncReport {
    // 1. 空目录实测集（仅开关关闭时需要）
    let empty_dirs = if !req.strategy.sync_empty_dirs {
        let mut set = std::collections::HashSet::new();
        for row in rows {
            if row.kind == ai_protocol::EntryKind::Dir {
                let path = if row.id.is_empty() {
                    roots.0.clone()
                } else {
                    roots.0.join(&row.id)
                };
                if let Ok(entries) = vfs.list_dir(&path).await {
                    if entries.is_empty() {
                        set.insert(row.id.clone());
                    }
                }
            }
        }
        Some(set)
    } else {
        None
    };
    let is_empty = |rel: &str| empty_dirs.as_ref().is_some_and(|s| s.contains(rel));

    // 2. 计划 + 策略
    let plan = plan_with_strategy(rows, req.preset, req.rules, req.selection, &req.strategy, Some(&is_empty));

    // 3. 执行（复用 execute 层：护栏/部分成功/取消/进度）
    let opts = ExecuteOptions { dry_run: req.dry_run };
    let exec = crate::execute::execute_planned_operations(vfs, roots, &plan.ops, &opts, progress, cancel).await;

    // 4. 组报告
    let mut report = SyncReport {
        preset: req.preset,
        strategy: Some(req.strategy),
        batch: BatchRunReport {
            dry_run: req.dry_run,
            cancelled: exec.cancelled,
            stats: BatchStats { total: exec.total, ..Default::default() },
            details: Vec::with_capacity(exec.total),
            log: Vec::new(),
        },
        plan: plan.ops.clone(),
        strategy_skipped: plan.skipped.clone(),
    };
    // 策略拦下项记入明细（跳过原因可追溯）
    for op in &plan.skipped {
        report.batch.details.push(OpRecord {
            kind: format!("{:?}", op.op),
            path: op.path.clone(),
            reason: op.reason.clone(),
            status: RecordStatus::Skipped,
            detail: Some("策略跳过".into()),
        });
        report.batch.stats.skipped += 1;
    }
    for r in &exec.succeeded {
        let status = if exec.dry_run {
            RecordStatus::Planned
        } else {
            match r.op {
                crate::execute::FileOp::Copy { .. } | crate::execute::FileOp::Move { .. } => RecordStatus::Copied,
                crate::execute::FileOp::Delete { .. } => RecordStatus::Deleted,
                crate::execute::FileOp::Touch { .. } => RecordStatus::Touched,
                _ => RecordStatus::Copied,
            }
        };
        bump_stat(&mut report.batch.stats, status);
        report.batch.details.push(OpRecord {
            kind: format!("{:?}", r.op),
            path: r.op.path().to_string(),
            reason: plan_reason(&plan.ops, r.op.path()),
            status,
            detail: r.detail.clone(),
        });
    }
    for r in &exec.failed {
        report.batch.stats.failed += 1;
        report.batch.details.push(OpRecord {
            kind: format!("{:?}", r.op),
            path: r.op.path().to_string(),
            reason: plan_reason(&plan.ops, r.op.path()),
            status: RecordStatus::Failed,
            detail: r.detail.clone(),
        });
    }
    for r in &exec.skipped {
        report.batch.stats.skipped += 1;
        report.batch.details.push(OpRecord {
            kind: format!("{:?}", r.op),
            path: r.op.path().to_string(),
            reason: plan_reason(&plan.ops, r.op.path()),
            status: RecordStatus::Skipped,
            detail: r.detail.clone(),
        });
    }

    // 5. 日志行（FR-SYNC 视图>日志面板）
    let verb = if req.dry_run { "同步试运行" } else { "同步完成" };
    report.batch.log.push(report.batch.summary_line(verb));
    for d in &report.batch.details {
        if d.status == RecordStatus::Failed {
            report.batch.log.push(format!("失败 {}: {} — {}", d.path, d.kind, d.detail.clone().unwrap_or_default()));
        } else if d.status == RecordStatus::Skipped {
            report.batch.log.push(format!("跳过 {}: {} — {}", d.path, d.kind, d.detail.clone().unwrap_or_default()));
        }
    }
    report
}

/// 从计划回查触发原因（执行级 OpResult 只带路径）
fn plan_reason(plan: &[PlannedOperation], path: &str) -> String {
    plan.iter().find(|p| p.path == path).map(|p| p.reason.clone()).unwrap_or_default()
}

fn bump_stat(stats: &mut BatchStats, status: RecordStatus) {
    match status {
        RecordStatus::Copied => stats.copied += 1,
        RecordStatus::Deleted => stats.deleted += 1,
        RecordStatus::Touched => stats.touched += 1,
        RecordStatus::Planned => {} // 计划项不计入动作计数（dry_run 口径见 plan 计数）
        RecordStatus::Skipped => stats.skipped += 1,
        RecordStatus::Failed => stats.failed += 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_protocol::{EntryKind, RuleAction, SideInfo};
    #[allow(unused_imports)]
    use ai_vfs::VfsProvider;

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
            left_path: Some(id.into()),
            right_path: Some(id.into()),
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

    // ---- P8：调度策略 ----

    use crate::execute::Side;
    use crate::sync::{apply_strategy, plan_with_strategy, ConflictPolicy, SyncStrategy};

    #[test]
    fn update_preset_must_not_tree_copy_dir_rows() {
        // 目录 mtime 较新不代表内容较新：整树复制会无条件覆写子项中较新的文件。
        let mut dir = row("sub", DirEntryStatus::DirDiff, 200, 100);
        dir.kind = EntryKind::Dir;
        let rows = vec![
            dir,
            row("sub/f.txt", DirEntryStatus::ContentDiff, 100, 200), // 右较新 → 不复制
            row("sub/g.txt", DirEntryStatus::RightOnly, 0, 0),       // 右孤 → 保留
            row("sub/h.txt", DirEntryStatus::ContentDiff, 300, 200), // 左较新 → 复制
        ];
        let plan = plan_operations(&rows, Some(SyncPreset::UpdateLeftToRight), None, None);
        assert_eq!(
            ops(&plan),
            vec![(PlannedOpKind::CopyLeftToRight, "sub/h.txt")],
            "Update 只按子文件行判定，不得整树复制目录行"
        );
        // Mirror 语义=使两侧一致，仍允许整树复制
        let m = plan_operations(&rows, Some(SyncPreset::MirrorLeftToRight), None, None);
        assert!(ops(&m).contains(&(PlannedOpKind::CopyLeftToRight, "sub")));
    }

    #[test]
    fn mirror_case_variant_delete_runs_before_copy() {
        // 不敏感卷上 A.txt/a.txt 同一条目：先 copy 后 delete 会清空目标侧。
        let rows = vec![
            row("A.txt", DirEntryStatus::LeftOnly, 0, 0),
            row("a.txt", DirEntryStatus::RightOnly, 0, 0),
        ];
        let plan = plan_operations(&rows, Some(SyncPreset::MirrorLeftToRight), None, None);
        assert_eq!(
            ops(&plan),
            vec![(PlannedOpKind::DeleteRight, "a.txt"), (PlannedOpKind::CopyLeftToRight, "A.txt")],
            "大小写碰撞的 delete 必须先于 copy"
        );
        // 非碰撞（不同名）不重排，保持原序
        let rows2 = vec![
            row("x.txt", DirEntryStatus::LeftOnly, 0, 0),
            row("y.txt", DirEntryStatus::RightOnly, 0, 0),
        ];
        let p2 = plan_operations(&rows2, Some(SyncPreset::MirrorLeftToRight), None, None);
        assert_eq!(
            ops(&p2),
            vec![(PlannedOpKind::CopyLeftToRight, "x.txt"), (PlannedOpKind::DeleteRight, "y.txt")]
        );
    }

    fn mirror_rows() -> Vec<DirRow> {
        vec![
            row("lonly", DirEntryStatus::LeftOnly, 0, 0),
            row("ronly", DirEntryStatus::RightOnly, 0, 0),
            row("diff", DirEntryStatus::ContentDiff, 200, 100),
        ]
    }

    #[test]
    fn strategy_keep_orphans_drops_mirror_deletes() {
        let rows = mirror_rows();
        let plan = plan_with_strategy(&rows, Some(SyncPreset::MirrorRightToLeft), None, None, &SyncStrategy { delete_orphans: false, ..Default::default() }, None);
        // 左孤立项不再被删除，但右孤立项仍复制到左
        assert_eq!(ops(&plan.ops), vec![(PlannedOpKind::CopyRightToLeft, "ronly"), (PlannedOpKind::CopyRightToLeft, "diff")]);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].op, PlannedOpKind::DeleteLeft);
        assert!(plan.skipped[0].reason.contains("keepOrphans"));
    }

    #[test]
    fn strategy_left_wins_redirects_copy_direction() {
        let rows = vec![
            row("lnewer", DirEntryStatus::ContentDiff, 200, 100),
            row("rnewer", DirEntryStatus::ContentDiff, 100, 200),
        ];
        let plan = plan_with_strategy(&rows, Some(SyncPreset::UpdateBoth), None, None, &SyncStrategy { conflict_policy: ConflictPolicy::LeftWins, ..Default::default() }, None);
        // 左侧优先：两行都改从左复制（不看 mtime）
        assert_eq!(ops(&plan.ops), vec![(PlannedOpKind::CopyLeftToRight, "lnewer"), (PlannedOpKind::CopyLeftToRight, "rnewer")]);
    }

    #[test]
    fn strategy_left_wins_flips_mirror_right_to_left() {
        let rows = mirror_rows();
        let plan = plan_with_strategy(&rows, Some(SyncPreset::MirrorRightToLeft), None, None, &SyncStrategy { conflict_policy: ConflictPolicy::LeftWins, ..Default::default() }, None);
        // 配对差异行改向；孤立项删除/复制方向不变（非差异行不受策略影响）
        assert_eq!(
            ops(&plan.ops),
            vec![(PlannedOpKind::DeleteLeft, "lonly"), (PlannedOpKind::CopyRightToLeft, "ronly"), (PlannedOpKind::CopyLeftToRight, "diff")]
        );
    }

    #[test]
    fn strategy_readonly_side_skips_writes() {
        let rows = mirror_rows();
        // 镜像左→右的全部操作（复制进右 / 删除右孤立项）都写右：只读右 = 全部跳过
        let plan = plan_with_strategy(&rows, Some(SyncPreset::MirrorLeftToRight), None, None, &SyncStrategy { readonly: Some(Side::Right), ..Default::default() }, None);
        assert!(plan.ops.is_empty());
        assert_eq!(plan.skipped.len(), 3);
        assert!(plan.skipped.iter().all(|p| p.reason.contains("readonlySide")));
        // 镜像右→左在只读左下同样全部跳过（删除左 / 复制进左）
        let plan2 = plan_with_strategy(&rows, Some(SyncPreset::MirrorRightToLeft), None, None, &SyncStrategy { readonly: Some(Side::Left), ..Default::default() }, None);
        assert!(plan2.ops.is_empty());
        assert_eq!(plan2.skipped.len(), 3);
    }

    #[test]
    fn strategy_skip_empty_dirs_gates_orphan_dir_ops() {
        let mut r = row("emptydir", DirEntryStatus::LeftOnly, 0, 0);
        r.kind = ai_protocol::EntryKind::Dir;
        let rows = vec![r, row("lonly.txt", DirEntryStatus::LeftOnly, 0, 0)];
        let empty = |rel: &str| rel == "emptydir";
        let plan = plan_with_strategy(&rows, Some(SyncPreset::MirrorLeftToRight), None, None, &SyncStrategy { sync_empty_dirs: false, ..Default::default() }, Some(&empty));
        // 空目录的复制被跳过；非空孤儿文件照常复制
        assert_eq!(ops(&plan.ops), vec![(PlannedOpKind::CopyLeftToRight, "lonly.txt")]);
        assert_eq!(plan.skipped.len(), 1);
        assert!(plan.skipped[0].reason.contains("skipEmptyDir"));
    }

    #[test]
    fn strategy_sync_empty_dirs_on_copies_orphan_dir() {
        let mut r = row("emptydir", DirEntryStatus::LeftOnly, 0, 0);
        r.kind = ai_protocol::EntryKind::Dir;
        let rows = vec![r];
        let empty = |_: &str| true;
        let plan = plan_with_strategy(&rows, Some(SyncPreset::MirrorLeftToRight), None, None, &SyncStrategy::default(), Some(&empty));
        assert_eq!(ops(&plan.ops), vec![(PlannedOpKind::CopyLeftToRight, "emptydir")]);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn apply_strategy_preserves_unrelated_ops() {
        // 自定义规则产生的对孤儿非空目录删除不受 keepOrphans 之外策略影响
        let rows = mirror_rows();
        let base = plan_operations(&rows, Some(SyncPreset::MirrorLeftToRight), None, None);
        let plan = apply_strategy(base, &rows, &SyncStrategy::default(), None);
        assert_eq!(plan.skipped.len(), 0);
        assert_eq!(plan.ops.len(), 3);
    }

    #[test]
    fn strategy_default_keeps_everything() {
        let rows = mirror_rows();
        let plan = plan_with_strategy(&rows, Some(SyncPreset::MirrorLeftToRight), None, None, &SyncStrategy::default(), None);
        assert_eq!(plan.ops.len(), 3);
        assert!(plan.skipped.is_empty());
    }

    // ---- P8：端到端 run_sync（tempdir 落盘） ----

    use crate::execute::Side as ExecSide;

    /// 端到端同步测试树（人工 DirRow + 真实落盘文件）：
    /// L: same.txt("S") / diff.txt("LEFT"左新) / lonly.txt("L") / edir/(空目录)
    /// R: same.txt("S") / diff.txt("RIGHT"右新) / ronly.txt("R")
    async fn make_sync_trees() -> (tempfile::TempDir, ai_vfs::VfsPath, ai_vfs::VfsPath) {
        use ai_vfs::VfsPath;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for name in ["L", "R"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        std::fs::create_dir_all(root.join("L").join("edir")).unwrap();
        std::fs::write(root.join("L").join("same.txt"), b"S").unwrap();
        std::fs::write(root.join("R").join("same.txt"), b"S").unwrap();
        std::fs::write(root.join("L").join("diff.txt"), b"LEFT").unwrap();
        std::fs::write(root.join("R").join("diff.txt"), b"RIGHT").unwrap();
        std::fs::write(root.join("L").join("lonly.txt"), b"L").unwrap();
        std::fs::write(root.join("R").join("ronly.txt"), b"R").unwrap();
        (
            dir,
            VfsPath::parse(root.join("L").to_str().unwrap()).unwrap(),
            VfsPath::parse(root.join("R").to_str().unwrap()).unwrap(),
        )
    }

    /// 与 make_sync_trees 对应的行模型（mtime 固定，判定确定性）
    fn sync_rows() -> Vec<DirRow> {
        vec![
            row("diff.txt", DirEntryStatus::ContentDiff, 100, 200),
            row("edir", DirEntryStatus::LeftOnly, 0, 0),
            row("lonly.txt", DirEntryStatus::LeftOnly, 0, 0),
            row("ronly.txt", DirEntryStatus::RightOnly, 0, 0),
            row("same.txt", DirEntryStatus::Same, 0, 0),
        ]
    }

    #[tokio::test]
    async fn run_sync_mirror_end_to_end() {
        use ai_vfs::LocalFs;
        let (dir, l, r) = make_sync_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let rows = sync_rows();
        let mut edir_row = sync_rows();
        edir_row[1].kind = ai_protocol::EntryKind::Dir;
        let req = SyncRequest { preset: Some(SyncPreset::MirrorLeftToRight), strategy: SyncStrategy::default(), dry_run: false, ..Default::default() };
        let mut progress_seen = Vec::new();
        let report = run_sync(&vfs, &roots, &edir_row, &req, Some(&mut |d, t, op| progress_seen.push((d, t, op.path().to_string()))), &CancelFlag::new()).await;
        // 落盘断言：左→右复制（diff/lonly/edir），右孤立项删除，same 不动
        assert_eq!(vfs.read_file_bytes(&r.join("diff.txt")).await.unwrap(), b"LEFT");
        assert_eq!(vfs.read_file_bytes(&r.join("lonly.txt")).await.unwrap(), b"L");
        assert_eq!(vfs.stat(&r.join("edir")).await.unwrap().kind, ai_vfs::EntryKind::Dir);
        assert!(vfs.stat(&r.join("ronly.txt")).await.is_err(), "镜像应删除右孤立项");
        assert_eq!(vfs.read_file_bytes(&r.join("same.txt")).await.unwrap(), b"S");
        // 报告统计（计划 5 项：diff/lonly 复制、edir 复制、ronly 删除；same 无操作）
        assert_eq!(report.batch.stats.copied, 3, "{:?}", report.batch.details);
        assert_eq!(report.batch.stats.deleted, 1);
        assert_eq!(report.batch.stats.failed, 0);
        assert!(!report.batch.cancelled);
        // 明细可追溯 + 日志面板行
        assert!(report.batch.details.iter().any(|d| d.status == RecordStatus::Copied && d.path == "diff.txt"));
        assert!(report.batch.log.iter().any(|l| l.contains("同步完成")));
        // 进度统一编号 (done,total)
        assert!(progress_seen.iter().all(|(d, t, _)| *d <= *t) && !progress_seen.is_empty());
        let _ = (rows, ExecSide::Left, dir);
    }

    #[tokio::test]
    async fn run_sync_dry_run_leaves_fs_untouched() {
        use ai_vfs::LocalFs;
        let (dir, l, r) = make_sync_trees().await;
        let _ = dir;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let mut rows = sync_rows();
        rows[1].kind = ai_protocol::EntryKind::Dir;
        let req = SyncRequest { preset: Some(SyncPreset::MirrorLeftToRight), strategy: SyncStrategy::default(), dry_run: true, ..Default::default() };
        let report = run_sync(&vfs, &roots, &rows, &req, None, &CancelFlag::new()).await;
        assert!(report.batch.dry_run);
        // 试运行不落盘
        assert!(vfs.stat(&r.join("lonly.txt")).await.is_err(), "未复制");
        assert!(vfs.stat(&r.join("ronly.txt")).await.is_ok(), "未删除");
        // 明细为 Planned 状态
        assert!(report.batch.details.iter().all(|d| d.status == RecordStatus::Planned || d.status == RecordStatus::Skipped));
        assert_eq!(report.batch.details.iter().filter(|d| d.status == RecordStatus::Planned).count(), 4);
        assert!(report.batch.log.iter().any(|l| l.contains("同步试运行")));
    }

    #[tokio::test]
    async fn run_sync_readonly_side_blocks_writes() {
        use ai_vfs::LocalFs;
        let (_dir, l, r) = make_sync_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let mut rows = sync_rows();
        rows[1].kind = ai_protocol::EntryKind::Dir;
        let req = SyncRequest {
            preset: Some(SyncPreset::MirrorLeftToRight),
            strategy: SyncStrategy { readonly: Some(ExecSide::Right), ..Default::default() },
            dry_run: false,
            ..Default::default()
        };
        let report = run_sync(&vfs, &roots, &rows, &req, None, &CancelFlag::new()).await;
        // 只读右：全部写右操作被策略拦截，落盘无变化
        assert_eq!(report.batch.stats.copied, 0);
        assert_eq!(report.batch.stats.deleted, 0);
        assert_eq!(report.batch.stats.skipped, 4, "策略拦下 4 项: {:?}", report.batch.details);
        assert!(vfs.stat(&r.join("ronly.txt")).await.is_ok(), "右孤立项保留");
        assert!(vfs.stat(&r.join("lonly.txt")).await.is_err());
    }

    #[tokio::test]
    async fn run_sync_keep_orphans_and_skip_empty_dir() {
        use ai_vfs::LocalFs;
        let (_dir, l, r) = make_sync_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let mut rows = sync_rows();
        rows[1].kind = ai_protocol::EntryKind::Dir;
        let req = SyncRequest {
            preset: Some(SyncPreset::MirrorLeftToRight),
            strategy: SyncStrategy { delete_orphans: false, sync_empty_dirs: false, ..Default::default() },
            dry_run: false,
            ..Default::default()
        };
        let report = run_sync(&vfs, &roots, &rows, &req, None, &CancelFlag::new()).await;
        // 孤立空目录跳过；右孤立项不删除；复制照常
        assert_eq!(report.batch.stats.copied, 2, "{:?}", report.batch.details);
        assert_eq!(report.batch.stats.deleted, 0);
        assert_eq!(report.batch.stats.skipped, 2, "空目录复制 + 右孤立项删除被跳过");
        assert!(vfs.stat(&r.join("edir")).await.is_err(), "空目录未创建");
        assert!(vfs.stat(&r.join("ronly.txt")).await.is_ok(), "右孤立项保留");
        assert_eq!(vfs.read_file_bytes(&r.join("diff.txt")).await.unwrap(), b"LEFT");
    }

    #[tokio::test]
    async fn run_sync_left_wins_policy_takes_left() {
        use ai_vfs::LocalFs;
        let (_dir, l, r) = make_sync_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let rows = sync_rows();
        let req = SyncRequest {
            preset: Some(SyncPreset::UpdateBoth),
            strategy: SyncStrategy { conflict_policy: ConflictPolicy::LeftWins, ..Default::default() },
            dry_run: false,
            ..Default::default()
        };
        let report = run_sync(&vfs, &roots, &rows, &req, None, &CancelFlag::new()).await;
        // 双向更新 + 左侧优先：diff.txt 左新也取左；孤立项保留（更新语义）
        assert_eq!(vfs.read_file_bytes(&r.join("diff.txt")).await.unwrap(), b"LEFT");
        assert_eq!(report.batch.stats.copied, 1, "{:?}", report.batch.details);
        assert_eq!(report.batch.stats.failed, 0);
    }
}
