//! # ai-dir（M03）：目录扫描、对齐与比较策略
//!
//! P0 范围（docs/04 P0 交付物 ⑤）：
//! - 两棵目录树扫描，`depth` 参数结构化预留（None = 全递归，1 = 仅顶层）；
//! - 按名对齐（[`ByNameAligner`]，双指针归并）；模式对齐（`*.gif`↔`*.png`）
//!   通过 [`NameAligner`] trait 扩展（FR-DIR-023 的 P2 落点）；
//! - 快速比较：size + mtime 秒容差（[`CompareCriteria`]）；
//! - 二进制判等：内容分块哈希（[`content_same`]）；
//! - 输出 docs/03 §4.2 arena + delta 模型：`Vec<DirNode>`（arena，NodeId = 下标）
//!   + [`rows_from_arena`]（协议 DirRow 序列，BFS 序、确定性）。
//!
//! 确定性约定：所有产出（arena 顺序、行序、状态）仅由输入内容决定，
//! 支撑 golden 快照与协议录制回放测试。

pub mod compare;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ai_protocol::{CompareCriteria, ContentCompareMode, DirEntryStatus, DirRow, DirStats, EntryKind, SideInfo};
use ai_vfs::{EntryKind as VfsEntryKind, VfsDirEntry, VfsError, VfsPath, VfsProvider};
use thiserror::Error;

pub use compare::{content_same, fold_chunk, ContentHashMode, CHUNK_SIZE};

/// arena 节点下标（docs/03 §4.2：NodeId = u32）
pub type NodeId = u32;

/// 取消旗标：扫描循环逐目录检查
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirScanOptions {
    /// None = 全递归；Some(k) = 展开 k 层（1 = 仅顶层；P0 默认）
    pub depth: Option<u32>,
    pub criteria: CompareCriteria,
}

impl Default for DirScanOptions {
    fn default() -> Self {
        DirScanOptions { depth: Some(1), criteria: CompareCriteria::default() }
    }
}

/// 目录节点（arena 布局，docs/03 §4.2 DirNode 的 P0 子集）
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
    /// 子节点区间 [start, end)（未展开为 None）
    pub children: Option<(NodeId, NodeId)>,
    /// 非同名配对（手动对齐/覆盖对齐，FR-DIR-022/023 的 P2 落点；P0 恒 None）
    pub aligned_with: Option<NodeId>,
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
}

/// 名称对齐器（扩展口：按名 / 模式 / Unicode 归一化 / 手动对齐，FR-DIR-010/020/022/023）
///
/// 契约：输入两侧条目**已按名字典序排序**；输出按名字典序的配对序列，
/// 且恰好覆盖两侧全部条目（每条目恰出现一次）。
pub trait NameAligner: Send + Sync {
    fn mode_name(&self) -> &'static str;
    fn align(&self, left: &[VfsDirEntry], right: &[VfsDirEntry]) -> Vec<AlignedPair>;
}

/// 一对对齐结果（单侧缺失为 None；索引为输入下标）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlignedPair {
    pub left: Option<u32>,
    pub right: Option<u32>,
    pub name: String,
}

/// 按名对齐（大小写敏感精确匹配；P0）
pub struct ByNameAligner;

impl NameAligner for ByNameAligner {
    fn mode_name(&self) -> &'static str {
        "byName"
    }

    fn align(&self, left: &[VfsDirEntry], right: &[VfsDirEntry]) -> Vec<AlignedPair> {
        debug_assert!(
            left.windows(2).all(|w| w[0].name <= w[1].name) && right.windows(2).all(|w| w[0].name <= w[1].name),
            "输入必须已排序",
        );
        let mut out = Vec::with_capacity(left.len() + right.len());
        let (mut i, mut j) = (0usize, 0usize);
        while i < left.len() && j < right.len() {
            match left[i].name.cmp(&right[j].name) {
                std::cmp::Ordering::Less => {
                    out.push(AlignedPair { left: Some(i as u32), right: None, name: left[i].name.clone() });
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    out.push(AlignedPair { left: None, right: Some(j as u32), name: right[j].name.clone() });
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    out.push(AlignedPair {
                        left: Some(i as u32),
                        right: Some(j as u32),
                        name: left[i].name.clone(),
                    });
                    i += 1;
                    j += 1;
                }
            }
        }
        while i < left.len() {
            out.push(AlignedPair { left: Some(i as u32), right: None, name: left[i].name.clone() });
            i += 1;
        }
        while j < right.len() {
            out.push(AlignedPair { left: None, right: Some(j as u32), name: right[j].name.clone() });
            j += 1;
        }
        out
    }
}

/// 扫描 + 对齐 + 判等，产出 arena（BFS 序）。
///
/// 根必须两侧都存在（orphan 根由上层处理）；orphan 子树本阶段不展开
/// （children = None），子目录展开深度由 `options.depth` 控制。
pub async fn scan(
    vfs: &dyn VfsProvider,
    left: &VfsPath,
    right: &VfsPath,
    options: &DirScanOptions,
    aligner: &dyn NameAligner,
    cancel: &CancelFlag,
) -> Result<DirScanResult, DirScanError> {
    let left_stat = vfs.stat(left).await.map_err(|e| match e {
        VfsError::NotFound { .. } => DirScanError::RootNotFound { side: "left", path: left.as_str().into() },
        other => DirScanError::Vfs(other),
    })?;
    let right_stat = vfs.stat(right).await.map_err(|e| match e {
        VfsError::NotFound { .. } => DirScanError::RootNotFound { side: "right", path: right.as_str().into() },
        other => DirScanError::Vfs(other),
    })?;
    if left_stat.kind != VfsEntryKind::Dir || right_stat.kind != VfsEntryKind::Dir {
        return Err(DirScanError::Vfs(VfsError::NotADirectory { path: format!("{left} / {right}") }));
    }

    let mut arena: Vec<DirNode> = vec![DirNode {
        rel_path: String::new(),
        name: String::new(),
        kind: EntryKind::Dir,
        left: Some(side_info(&left_stat)),
        right: Some(side_info(&right_stat)),
        status: DirEntryStatus::DirSame,
        children: None,
        aligned_with: None,
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
        let mut le = vfs.list_dir(&lp).await?;
        let mut re = vfs.list_dir(&rp).await?;
        le.sort_by(|a, b| a.name.cmp(&b.name));
        re.sort_by(|a, b| a.name.cmp(&b.name));
        let pairs = aligner.align(&le, &re);

        let start = arena.len() as NodeId;
        for pair in pairs {
            let child_rel = if dir_id == 0 {
                pair.name.clone()
            } else {
                format!("{}/{}", arena[dir_id as usize].rel_path, pair.name)
            };
            let linfo = pair.left.map(|i| side_info(&le[i as usize].stat));
            let rinfo = pair.right.map(|i| side_info(&re[i as usize].stat));
            let lkind = pair.left.map(|i| le[i as usize].stat.kind);
            let rkind = pair.right.map(|i| re[i as usize].stat.kind);

            // 目录判定：任一侧为目录即按目录行展示（BC 语义）
            let is_dir = lkind == Some(VfsEntryKind::Dir) || rkind == Some(VfsEntryKind::Dir);
            let kind = if is_dir { EntryKind::Dir } else { EntryKind::File };

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
                            &options.criteria,
                            cancel,
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
                children: None,
                aligned_with: None,
            });

            // 两侧均为目录（且对齐成功）-> 展开子树
            if let (Some(li), Some(ri)) = (pair.left, pair.right) {
                if le[li as usize].stat.kind == VfsEntryKind::Dir
                    && re[ri as usize].stat.kind == VfsEntryKind::Dir
                {
                    queue.push_back((id, lp.join(&arena[id as usize].name), rp.join(&arena[id as usize].name), level + 1));
                }
            }
        }
        let end = arena.len() as NodeId;
        if end > start {
            arena[dir_id as usize].children = Some((start, end));
        }
    }

    let stats = compute_stats(&arena);
    Ok(DirScanResult { arena, stats })
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
            left: n.left,
            right: n.right,
            status: n.status,
            has_children: n.kind == EntryKind::Dir,
        })
        .collect()
}

fn compute_stats(arena: &[DirNode]) -> DirStats {
    let mut s = DirStats::default();
    for n in &arena[1..] {
        s.total += 1;
        match n.status {
            DirEntryStatus::Same => s.same += 1,
            DirEntryStatus::QuickDiff => s.quick_diff += 1,
            DirEntryStatus::ContentDiff => s.content_diff += 1,
            DirEntryStatus::LeftOnly => s.left_only += 1,
            DirEntryStatus::RightOnly => s.right_only += 1,
            DirEntryStatus::DirSame => s.dir_same += 1,
            DirEntryStatus::Unknown => {}
        }
    }
    s
}

fn side_info(stat: &ai_vfs::StatInfo) -> SideInfo {
    SideInfo { size: stat.size, mtime_ms: stat.mtime_ms }
}

/// 文件判等：size -> mtime 秒容差 -> 内容比较（docs/03 §3.1.3 快速测试 + FR-DIR-011）
async fn file_verdict(
    vfs: &dyn VfsProvider,
    lp: &VfsPath,
    rp: &VfsPath,
    ls: &ai_vfs::StatInfo,
    rs: &ai_vfs::StatInfo,
    criteria: &CompareCriteria,
    _cancel: &CancelFlag,
) -> Result<DirEntryStatus, DirScanError> {
    // size 不同 => 内容必然不同（确定性判异，无需读内容）
    if ls.size != rs.size {
        return Ok(DirEntryStatus::ContentDiff);
    }
    // Always：覆盖快速测试结果（FR-DIR-011），即便 mtime/size 判同也做内容复核
    if criteria.content_compare == ContentCompareMode::Always {
        let same = content_same(vfs, lp, rp, compare::ContentHashMode::Crc).await?;
        return Ok(if same { DirEntryStatus::Same } else { DirEntryStatus::ContentDiff });
    }
    // mtime 秒容差（快速判同）
    let lsec = ls.mtime_ms / 1000;
    let rsec = rs.mtime_ms / 1000;
    if lsec.abs_diff(rsec) <= u64::from(criteria.mtime_tolerance_sec) {
        return Ok(DirEntryStatus::Same);
    }
    match criteria.content_compare {
        ContentCompareMode::Never => Ok(DirEntryStatus::QuickDiff),
        // Auto：mtime 判异但 size 相同 -> 内容复核
        ContentCompareMode::Auto => {
            let same = content_same(vfs, lp, rp, compare::ContentHashMode::Crc).await?;
            Ok(if same { DirEntryStatus::Same } else { DirEntryStatus::ContentDiff })
        }
        ContentCompareMode::Always => unreachable!("已在上方处理"),
    }
}

#[cfg(test)]
mod tests;
