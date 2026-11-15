//! # ai-dir（M03）：目录扫描、对齐与比较策略
//!
//! P2 范围（docs/04 P2 交付物，docs/03 §3.1.3 / ADR-004 / FR-DIR 全系）：
//! - arena + delta 模型：[`DirNode`]（parent/children/expanded 完整树布局）；
//! - 快速测试全集（[`quick`]：size / mtime 秒容差 / DST / 时区 / 大小写 /
//!   DOS 属性位 / Unix 元数据，均可配置）；
//! - 过滤器引擎侧求值（[`filter`]：BC 掩码语义全集 + 三类过滤）；
//! - CRC 内容比较 + LRU/SQLite 缓存（[`compare`]、[`cache`]，zip 中心目录
//!   预计算经 `VfsProvider::stored_crc32` 免读判定）；
//! - 同步状态机数据层（[`sync`]：5 预设 + 自定义规则 operate 计划，不执行；
//!   P8 增调度策略 [`sync::SyncStrategy`] 与端到端 [`sync::run_sync`] 报告）；
//! - 文件夹三路合并数据层（[`merge3way`]：base/left/right 扫描对齐、diff3
//!   分类、合并计划生成 + execute 层执行，P8）；
//! - 手动对齐 / 模式对齐扩展口（[`align`]）与平铺模式（[`session::DirModel`]）；
//! - 惰性 + 后台扫描状态机（[`session::DirModel`]，ADR-004：顶层立即应答、
//!   后台 tokio 任务推进 delta、聚合状态增量上推、全链路取消令牌、进度事件）。
//!
//! 确定性约定：同步路径（[`scan`]）的所有产出仅由输入内容决定，支撑 golden
//! 快照与协议录制回放测试；后台路径的行序同样由 arena id（BFS 序）决定。

pub mod align;
pub mod cache;
pub mod compare;
pub mod execute;
pub mod filter;
pub mod merge3way;
pub mod quick;
pub mod session;
pub mod sync;
pub mod verdict;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ai_protocol::{
    CompareCriteria, ContentCompareMode, DirEntryStatus, DirRow, DirStats, EntryKind, SideInfo,
};
use ai_vfs::{EntryKind as VfsEntryKind, VfsDirEntry, VfsError, VfsPath, VfsProvider};
use thiserror::Error;

pub use align::{AlignedPair, ByNameAligner, ManualAlignApplier, NameAligner, PatternAlignRule};
pub use cache::HashCache;
pub use compare::{content_same, fold_chunk, ContentHashMode, CHUNK_SIZE};
pub use filter::{NameFilter, OtherFilter};
pub use merge3way::{
    plan_merge, run_folder_merge, scan_merge, DirMergeRow, MergeCategory, MergeDecision, MergeNode,
    MergeOpKind, MergePlanParams, MergePlanResult, MergePlannedOp, MergeRoots, MergeRowState,
    MergeScanResult, MergeScanStats, MergeTarget,
};
pub use quick::QuickTest;
pub use session::{DirDeltaData, DirModel, DirSessionOptions};
pub use sync::{
    apply_strategy, plan_operations, plan_with_strategy, run_sync, BatchRunReport, BatchStats,
    ConflictPolicy, OpRecord, RecordStatus, SyncPlan, SyncReport, SyncRequest, SyncStrategy,
};
pub use verdict::{
    aggregate_verdicts, content_compare_result, content_verdict_stats, result_to_status,
    rules_classify_file, verdict_to_status, QuickVerdict,
};

pub use execute::{
    execute_ops, execute_planned_operations, sanitize_rel, ExecuteOptions, ExecuteReport, FileOp,
    OpResult, OpStatus, Side,
};

/// arena 节点下标（docs/03 §4.2：NodeId = u32）
pub type NodeId = u32;

/// 取消旗标：扫描循环逐目录检查（全链路：open / expand / 后台 worker / 内容比较）
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(pub Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        CancelFlag(Arc::new(AtomicBool::new(false)))
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// 目录比较会话选项（criteria 承载判据；depth 结构化预留）
#[derive(Debug, Clone, PartialEq)]
pub struct DirScanOptions {
    /// None = 全递归；Some(k) = 展开 k 层（1 = 仅顶层；P0 默认）
    pub depth: Option<u32>,
    pub criteria: CompareCriteria,
    /// 基于规则内容比较的重要性模型（FR-DIR-011 rulesBased；其余方法忽略）
    pub importance: ai_core::ImportanceOptions,
}

impl Default for DirScanOptions {
    fn default() -> Self {
        DirScanOptions {
            depth: Some(1),
            criteria: CompareCriteria::default(),
            importance: ai_core::ImportanceOptions::default(),
        }
    }
}

/// 目录节点（arena 布局，docs/03 §4.2 DirNode）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirNode {
    /// 相对根的 POSIX 风格路径（根为空串）
    pub rel_path: String,
    pub name: String,
    pub kind: EntryKind,
    /// 各侧元数据（侧缺失为 None）
    pub left: Option<SideInfo>,
    pub right: Option<SideInfo>,
    pub status: DirEntryStatus,
    /// 父节点（根为 None；聚合上推用）
    pub parent: Option<NodeId>,
    /// 子节点 id（按名字典序；未展开 = 空且 expanded=false）
    pub children: Vec<NodeId>,
    /// 已执行过 list_dir 展开（区别于空目录叶子）
    pub expanded: bool,
    /// 非同名配对（手动对齐/覆盖对齐，FR-DIR-022/023）
    pub aligned_with: Option<NodeId>,
    /// list_dir 失败（权限/IO）。该目录内容未知，不得聚合成 DirSame。
    pub scan_failed: bool,
}

impl DirNode {
    /// 是否两侧都存在的目录（可展开/可聚合）
    pub fn is_paired_dir(&self) -> bool {
        self.kind == EntryKind::Dir && self.left.is_some() && self.right.is_some()
    }
}

/// 扫描结果：arena + 状态计数
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirScanResult {
    /// arena[0] 为根；其余按 BFS 序（父先于子、名字典序）
    pub arena: Vec<DirNode>,
    pub stats: DirStats,
}

/// 扫描错误
#[derive(Debug, Error)]
pub enum DirScanError {
    #[error("根路径不存在（{side}）: {path}")]
    RootNotFound { side: &'static str, path: String },
    #[error("扫描被取消")]
    Cancelled,
    #[error(transparent)]
    Vfs(#[from] VfsError),
    /// 内容比较失败（IO 或规则判定；R3-16 三方式统一走 String 错误面）
    #[error("内容比较失败: {0}")]
    Content(String),
}

/// 对齐输入侧条目（list_dir 产物排序后）
fn sorted_entries(mut list: Vec<VfsDirEntry>) -> Vec<VfsDirEntry> {
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// 扫描 + 对齐 + 判等，产出 arena（BFS 序）。
///
/// 根必须两侧都存在（orphan 根由上层处理）；子目录展开深度由 `options.depth`
/// 控制；两侧均存在的目录展开（orphan 子树不展开，P0 约定保留）。
/// 聚合状态（FR-DIR-002）自底向上重算：已展开目录由子项聚合出 DirSame/DirDiff。
pub async fn scan(
    vfs: &dyn VfsProvider,
    left: &VfsPath,
    right: &VfsPath,
    options: &DirScanOptions,
    aligner: &dyn NameAligner,
    cancel: &CancelFlag,
) -> Result<DirScanResult, DirScanError> {
    let left_stat = vfs
        .stat(left)
        .await
        .map_err(|e| map_root_err("left", left, e))?;
    let right_stat = vfs
        .stat(right)
        .await
        .map_err(|e| map_root_err("right", right, e))?;
    if left_stat.kind != VfsEntryKind::Dir || right_stat.kind != VfsEntryKind::Dir {
        return Err(DirScanError::Vfs(VfsError::NotADirectory {
            path: format!("{left} / {right}"),
        }));
    }

    let mut arena: Vec<DirNode> = vec![DirNode {
        rel_path: String::new(),
        name: String::new(),
        kind: EntryKind::Dir,
        left: Some(side_info_gated(&left_stat, &options.criteria)),
        right: Some(side_info_gated(&right_stat, &options.criteria)),
        status: DirEntryStatus::DirSame,
        parent: None,
        children: Vec::new(),
        expanded: true,
        aligned_with: None,
        scan_failed: false,
    }];

    // BFS 队列：(目录节点 id, 左路径, 右路径, 层级)。只有两侧都存在的目录展开。
    let mut queue: std::collections::VecDeque<(NodeId, VfsPath, VfsPath, u32)> =
        std::collections::VecDeque::new();
    queue.push_back((0, left.clone(), right.clone(), 0));

    while let Some((dir_id, lp, rp, level)) = queue.pop_front() {
        if cancel.is_cancelled() {
            return Err(DirScanError::Cancelled);
        }
        // depth: None = 全递归；Some(k) = 展开 k 层（根 = 第 0 层）
        if options.depth.is_some_and(|d| level >= d) {
            continue;
        }
        let le = sorted_entries(vfs.list_dir(&lp).await?);
        let re = sorted_entries(vfs.list_dir(&rp).await?);
        let pairs = aligner.align(&le, &re);

        for pair in pairs {
            let child_rel = if dir_id == 0 {
                pair.name.clone()
            } else {
                format!("{}/{}", arena[dir_id as usize].rel_path, pair.name)
            };
            let linfo = pair
                .left
                .map(|i| side_info_gated(&le[i as usize].stat, &options.criteria));
            let rinfo = pair
                .right
                .map(|i| side_info_gated(&re[i as usize].stat, &options.criteria));
            let lkind = pair.left.map(|i| le[i as usize].stat.kind);
            let rkind = pair.right.map(|i| re[i as usize].stat.kind);

            // 目录判定：任一侧为目录即按目录行展示（BC 语义）
            let is_dir = lkind == Some(VfsEntryKind::Dir) || rkind == Some(VfsEntryKind::Dir);
            let kind = if is_dir {
                EntryKind::Dir
            } else {
                EntryKind::File
            };

            let status = match (pair.left, pair.right) {
                (Some(li), Some(ri)) => {
                    if is_dir {
                        DirEntryStatus::DirSame
                    } else {
                        file_verdict(
                            vfs,
                            &lp.join(&pair.name),
                            &rp.join(&pair.name),
                            &le[li as usize].stat,
                            &re[ri as usize].stat,
                            &pair.name,
                            &options.criteria,
                            &options.importance,
                        )
                        .await?
                    }
                }
                (Some(_), None) => DirEntryStatus::LeftOnly,
                (None, Some(_)) => DirEntryStatus::RightOnly,
                (None, None) => unreachable!("对齐器输出必含单侧"),
            };

            let id = arena.len() as NodeId;
            arena.push(DirNode {
                rel_path: child_rel,
                name: pair.name,
                kind,
                left: linfo,
                right: rinfo,
                status,
                parent: Some(dir_id),
                children: Vec::new(),
                expanded: false,
                aligned_with: None,
                scan_failed: false,
            });
            arena[dir_id as usize].children.push(id);

            // 两侧均为目录（且对齐成功）-> 展开子树
            if let (Some(li), Some(ri)) = (pair.left, pair.right) {
                if le[li as usize].stat.kind == VfsEntryKind::Dir
                    && re[ri as usize].stat.kind == VfsEntryKind::Dir
                {
                    let name = arena[id as usize].name.clone();
                    queue.push_back((id, lp.join(&name), rp.join(&name), level + 1));
                }
            }
        }
        arena[dir_id as usize].expanded = true;
    }

    aggregate_all(&mut arena);
    let stats = compute_stats(&arena);
    Ok(DirScanResult { arena, stats })
}

pub(crate) fn map_root_err(side: &'static str, path: &VfsPath, e: VfsError) -> DirScanError {
    match e {
        VfsError::NotFound { .. } => DirScanError::RootNotFound {
            side,
            path: path.as_str().into(),
        },
        other => DirScanError::Vfs(other),
    }
}

/// 聚合全部已展开的两侧目录状态（自底向上：逆 arena 序保证子先于父）。
pub fn aggregate_all(arena: &mut [DirNode]) {
    for id in (0..arena.len() as NodeId).rev() {
        let node = &arena[id as usize];
        if node.is_paired_dir() && node.expanded {
            let status = aggregate_children(node, arena);
            arena[id as usize].status = status;
        }
    }
}

/// 由子项聚合目录状态（FR-DIR-002）：
/// 本目录 list_dir 失败、或任一子项未判定（Unknown / 自身扫描失败）-> Unknown；
/// 任一子项存在差异（ContentDiff/QuickDiff/DirDiff）或孤立项（单侧）-> DirDiff；
/// 全部相同（含空目录）-> DirSame。
///
/// 为什么 Unknown 必须上推而不是落到 DirSame：比较工具的基本原则是「不可把
/// 未知当相同」。读不到的子目录里可能藏着差异，报 DirSame 会让用户以为已核对。
pub fn aggregate_children(dir: &DirNode, arena: &[DirNode]) -> DirEntryStatus {
    if dir.scan_failed {
        return DirEntryStatus::Unknown;
    }
    let mut unknown = false;
    let mut diff = false;
    for &c in &dir.children {
        let child = &arena[c as usize];
        if child.scan_failed || child.status == DirEntryStatus::Unknown {
            unknown = true;
        }
        if matches!(
            child.status,
            DirEntryStatus::ContentDiff
                | DirEntryStatus::Similar
                | DirEntryStatus::QuickDiff
                | DirEntryStatus::DirDiff
                | DirEntryStatus::LeftOnly
                | DirEntryStatus::RightOnly
        ) {
            diff = true;
        }
    }
    if unknown {
        DirEntryStatus::Unknown
    } else if diff {
        DirEntryStatus::DirDiff
    } else {
        DirEntryStatus::DirSame
    }
}

/// arena -> 协议 DirRow 序列（BFS 序，跳过根；id = rel_path）
pub fn rows_from_arena(arena: &[DirNode]) -> Vec<DirRow> {
    arena
        .iter()
        .skip(1)
        .map(|n| DirRow {
            id: n.rel_path.clone(),
            name: n.name.clone(),
            kind: n.kind,
            left: n.left.clone(),
            right: n.right.clone(),
            status: n.status,
            has_children: n.kind == EntryKind::Dir,
            hidden: false,
            suppressed: false,
            left_path: n.left.as_ref().map(|_| n.rel_path.clone()),
            right_path: n.right.as_ref().map(|_| n.rel_path.clone()),
        })
        .collect()
}

pub fn compute_stats(arena: &[DirNode]) -> DirStats {
    let mut s = DirStats::default();
    for n in &arena[1..] {
        s.total += 1;
        match n.status {
            DirEntryStatus::Same => s.same += 1,
            DirEntryStatus::QuickDiff => s.quick_diff += 1,
            DirEntryStatus::ContentDiff => s.content_diff += 1,
            DirEntryStatus::Similar => s.similar += 1,
            DirEntryStatus::LeftOnly => s.left_only += 1,
            DirEntryStatus::RightOnly => s.right_only += 1,
            DirEntryStatus::DirSame => s.dir_same += 1,
            DirEntryStatus::DirDiff => s.dir_diff += 1,
            DirEntryStatus::Unknown => {}
        }
    }
    s
}

/// 按判据门控的协议元数据（compare_attributes/unix_meta 开启时填充扩展字段）
pub(crate) fn side_info_gated(stat: &ai_vfs::StatInfo, criteria: &CompareCriteria) -> SideInfo {
    session::proto_side(stat, criteria)
}

/// 文件判等：快速测试（size/case/attr/unix/mtime）-> 内容比较
/// （docs/03 §3.1.3 + FR-DIR-010/011；P2 无缓存版，缓存见 [`session::DirModel`]）
#[allow(clippy::too_many_arguments)] // R3-16 追加重要性模型；两侧 stat/路径/判据均为必需上下文
pub(crate) async fn file_verdict(
    vfs: &dyn VfsProvider,
    lp: &VfsPath,
    rp: &VfsPath,
    ls: &ai_vfs::StatInfo,
    rs: &ai_vfs::StatInfo,
    name: &str,
    criteria: &CompareCriteria,
    importance: &ai_core::ImportanceOptions,
) -> Result<DirEntryStatus, DirScanError> {
    let tester = quick::QuickTest::new(criteria);
    match tester.verdict(ls, rs, name, name) {
        quick::QuickOutcome::Same => Ok(DirEntryStatus::Same),
        // mtime 判异且内容禁用 -> QuickDiff（未证实）；size/属性/大小写/
        // Unix 元数据差异为最终判定（内容比较不覆盖）
        quick::QuickOutcome::Diff(reason) => Ok(match reason {
            quick::QuickDiffReason::Mtime => DirEntryStatus::QuickDiff,
            _ => DirEntryStatus::ContentDiff,
        }),
        quick::QuickOutcome::NeedContent => {
            resolve_content(vfs, lp, rp, ls, rs, criteria, importance).await
        }
    }
}

/// 内容比较解析（NeedContent 时）：Never -> QuickDiff；
/// Auto/Always -> 按 criteria.content_method 三方式判等（R3-16）
pub(crate) async fn resolve_content(
    vfs: &dyn VfsProvider,
    lp: &VfsPath,
    rp: &VfsPath,
    ls: &ai_vfs::StatInfo,
    rs: &ai_vfs::StatInfo,
    criteria: &CompareCriteria,
    importance: &ai_core::ImportanceOptions,
) -> Result<DirEntryStatus, DirScanError> {
    match criteria.content_compare {
        ContentCompareMode::Never => Ok(DirEntryStatus::QuickDiff),
        ContentCompareMode::Auto | ContentCompareMode::Always => {
            let verdict = verdict::content_verdict_stats(
                vfs,
                lp,
                rp,
                compare::FileStatSummary::new(ls.size, ls.mtime_ms),
                compare::FileStatSummary::new(rs.size, rs.mtime_ms),
                criteria.content_method,
                importance,
                None,
            )
            .await
            .map_err(DirScanError::Content)?;
            Ok(verdict::verdict_to_status(verdict))
        }
    }
}

#[cfg(test)]
mod tests;
