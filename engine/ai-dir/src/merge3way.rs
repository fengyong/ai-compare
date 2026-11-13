//! 文件夹三路合并数据层（FR-DMRG-001~003，P8）：
//! - base/left/right 三目录扫描对齐（[`align3`]：复用 [`crate::align_key`]
//!   扩展三路键；同名多重出现按序 zip，语义同两侧 [`crate::ByNameAligner`]）；
//! - 目录级 diff3 分类（[`MergeCategory`]：三方同 / 两侧同改 / 冲突需人工 /
//!   孤立 / 单侧删除跟随；三方同名文件内容比较以 CRC 判定）；
//! - 合并计划生成（[`plan_merge`]：复用 PlannedOperation 的 op/path/reason
//!   形态 + 三路语义 takeLeft/takeRight/both 目录递归合并）——**不执行**，
//!   执行走现有 execute 层（[`run_folder_merge`] 组装 FileOp 批次）。
//!
//! 分类语义（叶文件，b/l/r = base/left/right 内容）：
//! | 存在性      | 内容关系                     | 分类 |
//! |-------------|------------------------------|------|
//! | b+l+r       | b==l==r                      | Same |
//! | b+l+r       | b==l, b!=r                   | RightChanged（自动取右）|
//! | b+l+r       | b==r, b!=l                   | LeftChanged（自动取左）|
//! | b+l+r       | l==r!=b                      | BothSame（两侧同改）|
//! | b+l+r       | 互不相等                     | Conflict（需人工）|
//! | b+l         | b==l（右侧删除，左未改）     | DeletedRight（自动跟随删除）|
//! | b+l         | b!=l（改/删组合）            | Conflict |
//! | b+r         | b==r                         | DeletedLeft |
//! | b+r         | b!=r                         | Conflict |
//! | l+r（无 b） | l==r（两侧同增）             | BothSame |
//! | l+r（无 b） | l!=r                         | Conflict |
//! | 仅 l / 仅 r | —                            | LeftOnly / RightOnly（孤立）|
//! | 仅 b        | —                            | BaseOnly（两侧均已删除）|
//!
//! 类型冲突（同名 file↔dir）：判 Conflict，不入子树。
//! 目录行状态自底向上聚合（着色口径 [`MergeRowState`]：
//! 绿=无冲突可自动 / 红=冲突 / 灰=孤立 / 暗=三方一致）。

use std::collections::{BTreeMap, BTreeSet};

use ai_protocol::{EntryKind, SideInfo};
use ai_vfs::{EntryKind as VfsEntryKind, StatInfo, VfsDirEntry, VfsPath, VfsProvider};

use crate::compare::{content_same, ContentHashMode};
use crate::execute::{ExecuteOptions, FileOp, ProgressFn, Side};
use crate::sync::{BatchRunReport, BatchStats, OpRecord, RecordStatus};
use crate::{CancelFlag, DirScanError, NodeId};

// ===========================================================================
// 三路对齐（复用 align_key 扩展三路键）
// ===========================================================================

/// 三路对齐结果（索引为各侧输入下标；单侧缺失为 None）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlignedTriple {
    pub name: String,
    pub base: Option<u32>,
    pub left: Option<u32>,
    pub right: Option<u32>,
}

/// 按对齐键三路对齐（输入各侧已按名字典序排序；输出按键字典序且覆盖全部条目）。
/// 同名多重出现按序 zip（平铺场景复用两侧语义）。
pub fn align3(
    base: &[VfsDirEntry],
    left: &[VfsDirEntry],
    right: &[VfsDirEntry],
    key: impl Fn(&str) -> String,
) -> Vec<AlignedTriple> {
    let mut bmap: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut lmap: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut rmap: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (i, e) in base.iter().enumerate() {
        bmap.entry(key(&e.name)).or_default().push(i as u32);
    }
    for (i, e) in left.iter().enumerate() {
        lmap.entry(key(&e.name)).or_default().push(i as u32);
    }
    for (i, e) in right.iter().enumerate() {
        rmap.entry(key(&e.name)).or_default().push(i as u32);
    }
    let keys: BTreeSet<&String> = bmap.keys().chain(lmap.keys()).chain(rmap.keys()).collect();
    let mut out = Vec::with_capacity(bmap.len() + lmap.len() + rmap.len());
    for k in keys {
        let bs = bmap.get(k).cloned().unwrap_or_default();
        let ls = lmap.get(k).cloned().unwrap_or_default();
        let rs = rmap.get(k).cloned().unwrap_or_default();
        let n = bs.len().max(ls.len()).max(rs.len());
        for i in 0..n {
            let name = match (bs.get(i), ls.get(i), rs.get(i)) {
                (Some(b), _, _) => base[*b as usize].name.clone(),
                (None, Some(l), _) => left[*l as usize].name.clone(),
                (None, None, Some(r)) => right[*r as usize].name.clone(),
                _ => unreachable!("三侧同位不可能皆空"),
            };
            out.push(AlignedTriple {
                name,
                base: bs.get(i).copied(),
                left: ls.get(i).copied(),
                right: rs.get(i).copied(),
            });
        }
    }
    out
}

// ===========================================================================
// 扫描 + 分类
// ===========================================================================

/// 叶级 diff3 分类（三方同名文件内容比较判定）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeCategory {
    /// 三方同（b==l==r；或两侧同增且一致）
    Same,
    /// 仅左改（右与 base 一致）→ 自动取左
    LeftChanged,
    /// 仅右改 → 自动取右
    RightChanged,
    /// 两侧同改（l==r!=b 或两侧同增且一致）→ 可自动
    BothSame,
    /// 右删除、左未改（b==l）→ 自动跟随删除
    DeletedRight,
    /// 左删除、右未改（b==r）→ 自动跟随删除
    DeletedLeft,
    /// 冲突需人工（互不相等 / 改删组合 / 类型冲突）
    Conflict,
    /// 孤立：仅左侧存在（无 base）
    LeftOnly,
    /// 孤立：仅右侧存在（无 base）
    RightOnly,
    /// 仅 base 存在（两侧均已删除）
    BaseOnly,
}

impl MergeCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            MergeCategory::Same => "same",
            MergeCategory::LeftChanged => "leftChanged",
            MergeCategory::RightChanged => "rightChanged",
            MergeCategory::BothSame => "bothSame",
            MergeCategory::DeletedRight => "deletedRight",
            MergeCategory::DeletedLeft => "deletedLeft",
            MergeCategory::Conflict => "conflict",
            MergeCategory::LeftOnly => "leftOnly",
            MergeCategory::RightOnly => "rightOnly",
            MergeCategory::BaseOnly => "baseOnly",
        }
    }
}

/// 着色口径（FR-DMRG-002 合并色 → 视图三态映射；目录自底向上聚合）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeRowState {
    /// 三方一致（暗/黑）
    Same,
    /// 无冲突可自动（绿）
    Auto,
    /// 冲突需人工（红）
    Conflict,
    /// 孤立（灰）
    Orphan,
}

impl MergeRowState {
    pub fn as_str(&self) -> &'static str {
        match self {
            MergeRowState::Same => "same",
            MergeRowState::Auto => "auto",
            MergeRowState::Conflict => "conflict",
            MergeRowState::Orphan => "orphan",
        }
    }

    fn from_category(c: MergeCategory) -> Self {
        match c {
            MergeCategory::Same => MergeRowState::Same,
            MergeCategory::LeftChanged
            | MergeCategory::RightChanged
            | MergeCategory::BothSame
            | MergeCategory::DeletedLeft
            | MergeCategory::DeletedRight => MergeRowState::Auto,
            MergeCategory::Conflict => MergeRowState::Conflict,
            MergeCategory::LeftOnly | MergeCategory::RightOnly | MergeCategory::BaseOnly => MergeRowState::Orphan,
        }
    }
}

/// 合并节点（arena 布局；叶=判定，目录=聚合）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeNode {
    pub rel_path: String,
    pub name: String,
    pub kind: EntryKind,
    pub base: Option<SideInfo>,
    pub left: Option<SideInfo>,
    pub right: Option<SideInfo>,
    /// 叶级分类；目录行 = 子树最重分类（聚合）
    pub category: MergeCategory,
    /// 着色口径（目录聚合：Conflict > Orphan > Auto > Same）
    pub row_state: MergeRowState,
    /// 叶节点（文件 / 类型冲突叶）；目录聚合与计划下钻只作用于非叶目录
    pub leaf: bool,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

/// 扫描统计（信息面板口径）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MergeScanStats {
    pub total: usize,
    pub files: usize,
    pub dirs: usize,
    pub same: usize,
    pub auto: usize,
    pub conflict: usize,
    pub orphan: usize,
}

/// 扫描结果：arena（[0] 为根）+ 统计
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeScanResult {
    pub arena: Vec<MergeNode>,
    pub stats: MergeScanStats,
}

impl MergeScanResult {
    /// arena -> 协议友好平铺行（parent 为父 rel 路径；depth 从 1 起）
    pub fn rows(&self) -> Vec<DirMergeRow> {
        let mut out = Vec::with_capacity(self.arena.len());
        for (id, n) in self.arena.iter().enumerate().skip(1) {
            let mut d = 0u32;
            let mut cur = id;
            while cur != 0 {
                d += 1;
                cur = self.arena[cur].parent.map(|p| p as usize).unwrap_or(0);
            }
            out.push(DirMergeRow {
                rel_path: n.rel_path.clone(),
                name: n.name.clone(),
                kind: n.kind,
                base: n.base.clone(),
                left: n.left.clone(),
                right: n.right.clone(),
                category: n.category,
                row_state: n.row_state,
                parent: n
                    .parent
                    .map(|p| self.arena[p as usize].rel_path.clone())
                    .filter(|s| !s.is_empty()),
                depth: d,
                conflict_leaf: n.kind == EntryKind::File && n.category == MergeCategory::Conflict,
            });
        }
        out
    }
}

/// 协议友好合并行（同形于 UI DirMergeRow 提案 DTO；见 P8 RPC 变更清单）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirMergeRow {
    pub rel_path: String,
    pub name: String,
    pub kind: EntryKind,
    pub base: Option<SideInfo>,
    pub left: Option<SideInfo>,
    pub right: Option<SideInfo>,
    pub category: MergeCategory,
    pub row_state: MergeRowState,
    /// 父 rel 路径（根的直接子项为 None）
    pub parent: Option<String>,
    pub depth: u32,
    /// 叶子级内容冲突（UI 双击 → 文本合并视图占位的判定依据）
    pub conflict_leaf: bool,
}

fn side_info(stat: &StatInfo) -> SideInfo {
    SideInfo::base(stat.size, stat.mtime_ms)
}

async fn list_sorted(vfs: &dyn VfsProvider, dir: &VfsPath) -> Result<Vec<VfsDirEntry>, DirScanError> {
    let mut list = vfs.list_dir(dir).await?;
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(list)
}

/// 三目录扫描对齐 + diff3 分类（全递归；逐目录检查取消令牌）。
///
/// 目录展开：任一侧为 Dir 且无类型冲突即建目录节点，对存在的各侧子目录
/// 继续下钻；单侧孤立目录整树下钻（子项全记孤立）。
pub async fn scan_merge(
    vfs: &dyn VfsProvider,
    base: &VfsPath,
    left: &VfsPath,
    right: &VfsPath,
    cancel: &CancelFlag,
) -> Result<MergeScanResult, DirScanError> {
    for (side, path) in [("base", base), ("left", left), ("right", right)] {
        let stat = vfs.stat(path).await.map_err(|e| crate::map_root_err(side, path, e))?;
        if stat.kind != VfsEntryKind::Dir {
            return Err(DirScanError::Vfs(ai_vfs::VfsError::NotADirectory { path: path.as_str().into() }));
        }
    }
    let mut arena = vec![MergeNode {
        rel_path: String::new(),
        name: String::new(),
        kind: EntryKind::Dir,
        base: None,
        left: None,
        right: None,
        category: MergeCategory::Same,
        row_state: MergeRowState::Same,
        leaf: false,
        parent: None,
        children: Vec::new(),
    }];
    walk(vfs, Some(base), Some(left), Some(right), 0, &mut arena, cancel).await?;
    aggregate(&mut arena);
    let stats = summarize(&arena);
    Ok(MergeScanResult { arena, stats })
}

/// 目录递归下钻（parent 先入 arena；子项按键字典序；缺失侧为 None 不下钻）
fn walk<'a>(
    vfs: &'a dyn VfsProvider,
    bp: Option<&'a VfsPath>,
    lp: Option<&'a VfsPath>,
    rp: Option<&'a VfsPath>,
    parent: NodeId,
    arena: &'a mut Vec<MergeNode>,
    cancel: &'a CancelFlag,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), DirScanError>> + Send + 'a>> {
    Box::pin(async move {
        if cancel.is_cancelled() {
            return Err(DirScanError::Cancelled);
        }
        let empty: Vec<VfsDirEntry> = Vec::new();
        let be = match bp {
            Some(p) => list_sorted(vfs, p).await?,
            None => empty,
        };
        let le = match lp {
            Some(p) => list_sorted(vfs, p).await?,
            None => Vec::new(),
        };
        let re = match rp {
            Some(p) => list_sorted(vfs, p).await?,
            None => Vec::new(),
        };
        let triples = align3(&be, &le, &re, |n| n.to_string());

        for t in triples {
            let bstat = t.base.map(|i| &be[i as usize].stat);
            let lstat = t.left.map(|i| &le[i as usize].stat);
            let rstat = t.right.map(|i| &re[i as usize].stat);
            let bkind = bstat.map(|s| s.kind);
            let lkind = lstat.map(|s| s.kind);
            let rkind = rstat.map(|s| s.kind);

            let parent_rel = arena[parent as usize].rel_path.clone();
            let child_rel = if parent_rel.is_empty() { t.name.clone() } else { format!("{parent_rel}/{}", t.name) };
            let kind = if matches!(bkind, Some(VfsEntryKind::Dir))
                || matches!(lkind, Some(VfsEntryKind::Dir))
                || matches!(rkind, Some(VfsEntryKind::Dir))
            {
                EntryKind::Dir
            } else {
                EntryKind::File
            };

            // 类型冲突：任一侧 Dir 与另一侧 File 同名 -> 冲突叶（不入子树）
            let kinds = [bkind, lkind, rkind];
            let has_dir = kinds.contains(&Some(VfsEntryKind::Dir));
            let has_file = kinds.iter().any(|k| k.is_some() && *k != Some(VfsEntryKind::Dir));
            let clash = has_dir && has_file;

            let id = arena.len() as NodeId;
            type DirRecurse = Option<(Option<VfsPath>, Option<VfsPath>, Option<VfsPath>)>;
            let (category, recurse): (MergeCategory, DirRecurse) = if clash {
                (MergeCategory::Conflict, None)
            } else if kind == EntryKind::Dir {
                // 目录节点：为 Dir 的侧继续下钻；缺失/非目录侧置 None
                let sub = |k: Option<VfsEntryKind>, base: Option<&VfsPath>, name: &str| -> Option<VfsPath> {
                    if k == Some(VfsEntryKind::Dir) {
                        base.map(|p| p.join(name))
                    } else {
                        None
                    }
                };
                let b2 = sub(bkind, bp, &t.name);
                let l2 = sub(lkind, lp, &t.name);
                let r2 = sub(rkind, rp, &t.name);
                // 初始分类按存在性（目录行聚合后重算）
                let cat = match (t.base.is_some(), t.left.is_some(), t.right.is_some()) {
                    (false, true, false) => MergeCategory::LeftOnly,
                    (false, false, true) => MergeCategory::RightOnly,
                    (true, false, false) => MergeCategory::BaseOnly,
                    _ => MergeCategory::Same,
                };
                (cat, Some((b2, l2, r2)))
            } else {
                // 文件叶：三方同名内容比较判定
                let cat = classify_file(vfs, bp, lp, rp, &t).await?;
                (cat, None)
            };

            arena.push(MergeNode {
                rel_path: child_rel,
                name: t.name.clone(),
                kind,
                base: bstat.map(side_info),
                left: lstat.map(side_info),
                right: rstat.map(side_info),
                category,
                row_state: MergeRowState::from_category(category),
                leaf: recurse.is_none(),
                parent: Some(parent),
                children: Vec::new(),
            });
            arena[parent as usize].children.push(id);

            if let Some((b2, l2, r2)) = recurse {
                walk(vfs, b2.as_ref(), l2.as_ref(), r2.as_ref(), id, arena, cancel).await?;
            }
        }
        Ok(())
    })
}

/// 文件叶分类（最多两次内容比较：b==l 后再 b==r 或 l==r）
async fn classify_file(
    vfs: &dyn VfsProvider,
    bp: Option<&VfsPath>,
    lp: Option<&VfsPath>,
    rp: Option<&VfsPath>,
    t: &AlignedTriple,
) -> Result<MergeCategory, DirScanError> {
    let name = t.name.as_str();
    async fn same(
        vfs: &dyn VfsProvider,
        name: &str,
        a: Option<&VfsPath>,
        b: Option<&VfsPath>,
    ) -> Result<bool, DirScanError> {
        let (a, b) = (a.expect("比较侧必存在"), b.expect("比较侧必存在"));
        content_same(vfs, &a.join(name), &b.join(name), ContentHashMode::Crc)
            .await
            .map_err(DirScanError::from)
    }
    match (t.base, t.left, t.right) {
        (Some(_), Some(_), Some(_)) => {
            let bl = same(vfs, name, bp, lp).await?;
            if bl {
                let br = same(vfs, name, bp, rp).await?;
                Ok(if br { MergeCategory::Same } else { MergeCategory::RightChanged })
            } else {
                let br = same(vfs, name, bp, rp).await?;
                if br {
                    Ok(MergeCategory::LeftChanged)
                } else {
                    let lr = same(vfs, name, lp, rp).await?;
                    Ok(if lr { MergeCategory::BothSame } else { MergeCategory::Conflict })
                }
            }
        }
        (Some(_), Some(_), None) => {
            let bl = same(vfs, name, bp, lp).await?;
            Ok(if bl { MergeCategory::DeletedRight } else { MergeCategory::Conflict })
        }
        (Some(_), None, Some(_)) => {
            let br = same(vfs, name, bp, rp).await?;
            Ok(if br { MergeCategory::DeletedLeft } else { MergeCategory::Conflict })
        }
        (None, Some(_), Some(_)) => {
            let lr = same(vfs, name, lp, rp).await?;
            Ok(if lr { MergeCategory::BothSame } else { MergeCategory::Conflict })
        }
        (None, Some(_), None) => Ok(MergeCategory::LeftOnly),
        (None, None, Some(_)) => Ok(MergeCategory::RightOnly),
        (Some(_), None, None) => Ok(MergeCategory::BaseOnly),
        (None, None, None) => unreachable!("对齐器输出必含单侧"),
    }
}

/// 目录聚合（自底向上：Conflict > Orphan > Auto > Same；叠加节点自身存在性）
fn aggregate(arena: &mut [MergeNode]) {
    for id in (0..arena.len()).rev() {
        let (state, cat) = {
            let node = &arena[id];
            if node.kind != EntryKind::Dir || node.parent.is_none() || node.leaf {
                continue;
            }
            // 自身存在性（孤立/两侧已删的空目录无子项可聚合）
            let presence_cat = match (node.base.is_some(), node.left.is_some(), node.right.is_some()) {
                (false, true, false) => Some(MergeCategory::LeftOnly),
                (false, false, true) => Some(MergeCategory::RightOnly),
                (true, false, false) => Some(MergeCategory::BaseOnly),
                _ => None,
            };
            // 目录分类 = 最重（自身存在性 vs 子分类）
            let cat = presence_cat
                .into_iter()
                .chain(node.children.iter().map(|&c| arena[c as usize].category))
                .min_by_key(|c| category_rank(*c))
                .unwrap_or(MergeCategory::Same);
            let child_state = if node.children.iter().any(|&c| arena[c as usize].row_state == MergeRowState::Conflict) {
                MergeRowState::Conflict
            } else if node.children.iter().any(|&c| arena[c as usize].row_state == MergeRowState::Orphan) {
                MergeRowState::Orphan
            } else if node.children.iter().any(|&c| arena[c as usize].row_state == MergeRowState::Auto) {
                MergeRowState::Auto
            } else {
                MergeRowState::Same
            };
            let own_state = MergeRowState::from_category(presence_cat.unwrap_or(MergeCategory::Same));
            let state = worse_state(own_state, child_state);
            (state, cat)
        };
        arena[id].row_state = state;
        arena[id].category = cat;
    }
}

/// 行状态权重（Conflict > Orphan > Auto > Same；取更重者）
fn worse_state(a: MergeRowState, b: MergeRowState) -> MergeRowState {
    let rank = |s: MergeRowState| match s {
        MergeRowState::Conflict => 0,
        MergeRowState::Orphan => 1,
        MergeRowState::Auto => 2,
        MergeRowState::Same => 3,
    };
    if rank(a) <= rank(b) {
        a
    } else {
        b
    }
}

/// 分类权重（聚合取最重；Conflict 最高）
fn category_rank(c: MergeCategory) -> u8 {
    match c {
        MergeCategory::Conflict => 0,
        MergeCategory::LeftOnly | MergeCategory::RightOnly | MergeCategory::BaseOnly => 1,
        MergeCategory::LeftChanged
        | MergeCategory::RightChanged
        | MergeCategory::BothSame
        | MergeCategory::DeletedLeft
        | MergeCategory::DeletedRight => 2,
        MergeCategory::Same => 3,
    }
}

fn summarize(arena: &[MergeNode]) -> MergeScanStats {
    let mut s = MergeScanStats::default();
    for n in &arena[1..] {
        s.total += 1;
        match n.kind {
            EntryKind::Dir => s.dirs += 1,
            // 文件/符号链接均计入文件口径（符号链接按目标内容判定，P8 近似）
            _ => s.files += 1,
        }
        match n.row_state {
            MergeRowState::Same => s.same += 1,
            MergeRowState::Auto => s.auto += 1,
            MergeRowState::Conflict => s.conflict += 1,
            MergeRowState::Orphan => s.orphan += 1,
        }
    }
    s
}

// ===========================================================================
// 合并计划生成（复用 PlannedOperation 形态 + 三路语义；不执行）
// ===========================================================================

/// 合并目标（FR-DMRG-001 合并目标四选的引擎面：左/右/第三输出文件夹）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeTarget {
    Left,
    Right,
    Output,
}

/// 行决策（UI 冲突行交互面；Auto = 按分类默认）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeDecision {
    Auto,
    TakeLeft,
    TakeRight,
    /// 目录 = 递归合并两侧（子项按各自决策/默认）；文件 = 降级为分类默认
    Both,
    Skip,
}

/// 合并计划动作（PlannedOperation 的 op/path/reason 同形）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeOpKind {
    /// 输出取左（目标侧被左内容覆盖/新建）
    TakeLeft,
    TakeRight,
    /// 从目标侧删除（自动跟随对侧删除）
    DeleteTarget,
    /// 计划在案但不动作（决策跳过 / 目标已是所需状态）
    Keep,
}

impl MergeOpKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MergeOpKind::TakeLeft => "takeLeft",
            MergeOpKind::TakeRight => "takeRight",
            MergeOpKind::DeleteTarget => "deleteTarget",
            MergeOpKind::Keep => "keep",
        }
    }
}

/// 一条合并计划操作（复用 PlannedOperation 三字段形态）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePlannedOp {
    pub kind: MergeOpKind,
    pub path: String,
    pub reason: String,
}

/// 计划产物：动作项（含 Skip 子树的 Keep 记录）+ 统计
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergePlanResult {
    pub ops: Vec<MergePlannedOp>,
    pub stats: BatchStats,
}

/// 合并计划参数
#[derive(Debug, Clone)]
pub struct MergePlanParams<'a> {
    pub target: MergeTarget,
    /// rel 路径 -> 显式决策（覆盖分类默认；目录决策递归展开到子树）
    pub decisions: &'a BTreeMap<String, MergeDecision>,
}

/// 生成合并计划（纯函数；DFS 展开，父决策继承、子决策覆盖）。
pub fn plan_merge(nodes: &[MergeNode], params: &MergePlanParams<'_>) -> MergePlanResult {
    let mut out = MergePlanResult::default();
    // 根的子项 DFS（根 rel 为空串不可能有决策）
    for &child in &nodes[0].children {
        walk_plan(nodes, child, params, None, &mut out);
    }
    out.stats.total = out.ops.len();
    out
}

/// 有效决策 = 显式决策 > 继承决策 > Auto
fn walk_plan(
    nodes: &[MergeNode],
    id: NodeId,
    params: &MergePlanParams<'_>,
    inherit: Option<MergeDecision>,
    out: &mut MergePlanResult,
) {
    let node = &nodes[id as usize];
    let effective = params
        .decisions
        .get(&node.rel_path)
        .copied()
        .or(inherit)
        .unwrap_or(MergeDecision::Auto);

    if node.kind == EntryKind::Dir && !node.leaf {
        if effective == MergeDecision::Skip {
            // 整树跳过：记一条 Keep（预览可解释），不下钻
            out.ops.push(MergePlannedOp {
                kind: MergeOpKind::Keep,
                path: node.rel_path.clone(),
                reason: "决策:跳过（含子树）".into(),
            });
            out.stats.skipped += 1;
            return;
        }
        // 空孤立目录并入（非空目录由子项操作经 mkdir 链隐式建目录）
        let adopt = children_absent_on_target(node.category, params.target);
        if node.children.is_empty() && adopt {
            let kind = match effective {
                MergeDecision::TakeRight => MergeOpKind::TakeRight,
                MergeDecision::TakeLeft => MergeOpKind::TakeLeft,
                _ => match (node.category, params.target) {
                    (MergeCategory::RightOnly, _) => MergeOpKind::TakeRight,
                    _ => MergeOpKind::TakeLeft,
                },
            };
            let dir = if kind == MergeOpKind::TakeLeft { "左" } else { "右" };
            let reason = if effective == MergeDecision::Auto {
                format!("孤立空目录并入（取{dir}）")
            } else {
                "决策:目录并入（空目录）".into()
            };
            out.ops.push(MergePlannedOp { kind, path: node.rel_path.clone(), reason });
            out.stats.copied += 1;
        }
        let pass_down = if effective == MergeDecision::Auto { None } else { Some(effective) };
        for &child in &node.children {
            walk_plan(nodes, child, params, pass_down, out);
        }
        return;
    }

    // 文件叶
    let (kind, reason) =
        decide_file(effective, node.category, params.target, node.left.is_some(), node.right.is_some());
    match kind {
        MergeOpKind::Keep => {
            // 决策跳过 / 目标已是所需状态：记 Keep 便于预览解释
            if effective != MergeDecision::Auto {
                out.ops.push(MergePlannedOp { kind, path: node.rel_path.clone(), reason });
                out.stats.skipped += 1;
            }
            // Auto 的 Keep（三方一致等）不入计划（对齐 sync 计划口径：只含动作）
        }
        _ => {
            out.stats.copied += usize::from(matches!(kind, MergeOpKind::TakeLeft | MergeOpKind::TakeRight));
            out.stats.deleted += usize::from(kind == MergeOpKind::DeleteTarget);
            out.ops.push(MergePlannedOp { kind, path: node.rel_path.clone(), reason });
        }
    }
}

/// 目标侧是否缺失该目录（孤立目录并入判定）
fn children_absent_on_target(cat: MergeCategory, target: MergeTarget) -> bool {
    match target {
        MergeTarget::Right => cat == MergeCategory::LeftOnly,
        MergeTarget::Left => cat == MergeCategory::RightOnly,
        // 输出侧为第三目录：两侧孤立的空目录都并入
        MergeTarget::Output => matches!(cat, MergeCategory::LeftOnly | MergeCategory::RightOnly),
    }
}

/// 文件叶决策（含 Auto 分类默认表；注释语义见模块头）
fn decide_file(
    effective: MergeDecision,
    cat: MergeCategory,
    target: MergeTarget,
    has_left: bool,
    has_right: bool,
) -> (MergeOpKind, String) {
    match effective {
        MergeDecision::Skip => (MergeOpKind::Keep, "决策:跳过".into()),
        MergeDecision::TakeLeft => {
            if !has_left {
                (MergeOpKind::Keep, "决策:取左（左侧缺失，保持现状）".into())
            } else if target == MergeTarget::Left {
                (MergeOpKind::Keep, "决策:取左（目标侧即左侧）".into())
            } else {
                (MergeOpKind::TakeLeft, "决策:取左".into())
            }
        }
        MergeDecision::TakeRight => {
            if !has_right {
                (MergeOpKind::Keep, "决策:取右（右侧缺失，保持现状）".into())
            } else if target == MergeTarget::Right {
                (MergeOpKind::Keep, "决策:取右（目标侧即右侧）".into())
            } else {
                (MergeOpKind::TakeRight, "决策:取右".into())
            }
        }
        MergeDecision::Both => {
            let (k, r) = auto_default(cat, target);
            (k, format!("决策:both（单文件降级默认）· {r}"))
        }
        MergeDecision::Auto => auto_default(cat, target),
    }
}

/// Auto 分类默认表（目标 = Right 时取左改/并左孤儿；镜像于 Left；Output 输出快照）
fn auto_default(cat: MergeCategory, target: MergeTarget) -> (MergeOpKind, String) {
    match (cat, target) {
        (MergeCategory::Same, MergeTarget::Output) => (MergeOpKind::TakeLeft, "三方一致（输出取左）".into()),
        (MergeCategory::Same, _) => (MergeOpKind::Keep, "三方一致".into()),
        (MergeCategory::LeftChanged, MergeTarget::Left) => (MergeOpKind::Keep, "目标侧即左侧（已含修改）".into()),
        (MergeCategory::LeftChanged, _) => (MergeOpKind::TakeLeft, "仅左改 → 取左".into()),
        (MergeCategory::RightChanged, MergeTarget::Right) => (MergeOpKind::Keep, "目标侧即右侧（已含修改）".into()),
        (MergeCategory::RightChanged, _) => (MergeOpKind::TakeRight, "仅右改 → 取右".into()),
        (MergeCategory::BothSame, MergeTarget::Output) => {
            (MergeOpKind::TakeLeft, "两侧同改一致（输出取左，等价取右）".into())
        }
        (MergeCategory::BothSame, _) => (MergeOpKind::Keep, "两侧同改一致".into()),
        (MergeCategory::DeletedRight, MergeTarget::Right) => (MergeOpKind::Keep, "目标侧已删除".into()),
        (MergeCategory::DeletedRight, MergeTarget::Left) => (MergeOpKind::DeleteTarget, "跟随右侧删除".into()),
        (MergeCategory::DeletedRight, MergeTarget::Output) => (MergeOpKind::Keep, "右侧已删除（输出不含该项）".into()),
        (MergeCategory::DeletedLeft, MergeTarget::Left) => (MergeOpKind::Keep, "目标侧已删除".into()),
        (MergeCategory::DeletedLeft, MergeTarget::Right) => (MergeOpKind::DeleteTarget, "跟随左侧删除".into()),
        (MergeCategory::DeletedLeft, MergeTarget::Output) => (MergeOpKind::Keep, "左侧已删除（输出不含该项）".into()),
        (MergeCategory::Conflict, _) => (MergeOpKind::Keep, "冲突需人工处理".into()),
        (MergeCategory::LeftOnly, MergeTarget::Right) => (MergeOpKind::TakeLeft, "左孤立并入目标".into()),
        (MergeCategory::LeftOnly, MergeTarget::Left) => (MergeOpKind::Keep, "目标侧即左侧".into()),
        (MergeCategory::LeftOnly, MergeTarget::Output) => (MergeOpKind::TakeLeft, "左孤立并入输出".into()),
        (MergeCategory::RightOnly, MergeTarget::Left) => (MergeOpKind::TakeRight, "右孤立并入目标".into()),
        (MergeCategory::RightOnly, MergeTarget::Right) => (MergeOpKind::Keep, "目标侧即右侧".into()),
        (MergeCategory::RightOnly, MergeTarget::Output) => (MergeOpKind::TakeRight, "右孤立并入输出".into()),
        (MergeCategory::BaseOnly, _) => (MergeOpKind::Keep, "两侧均已删除".into()),
    }
}

// ===========================================================================
// 执行（复用现有 execute 层：FileOp 批次 + 护栏/部分成功/取消/进度）
// ===========================================================================

/// 合并根（Output 目标需要第三目录）
#[derive(Debug, Clone)]
pub struct MergeRoots {
    pub base: VfsPath,
    pub left: VfsPath,
    pub right: VfsPath,
    pub output: Option<VfsPath>,
}

/// 计划 -> (FileOp, 源根, 目标根) 批次项（输出侧借 execute 双根形态）
fn plan_to_exec(roots: &MergeRoots, target: MergeTarget, plan: &MergePlannedOp) -> Result<(FileOp, VfsPath, VfsPath), String> {
    match (target, plan.kind) {
        (MergeTarget::Right, MergeOpKind::TakeLeft) => Ok((
            FileOp::Copy { path: plan.path.clone(), from: Side::Left, to: Side::Right },
            roots.left.clone(),
            roots.right.clone(),
        )),
        (MergeTarget::Left, MergeOpKind::TakeRight) => Ok((
            FileOp::Copy { path: plan.path.clone(), from: Side::Right, to: Side::Left },
            roots.right.clone(),
            roots.left.clone(),
        )),
        (MergeTarget::Right, MergeOpKind::DeleteTarget) => Ok((
            FileOp::Delete { path: plan.path.clone(), side: Side::Right },
            roots.left.clone(),
            roots.right.clone(),
        )),
        (MergeTarget::Left, MergeOpKind::DeleteTarget) => Ok((
            FileOp::Delete { path: plan.path.clone(), side: Side::Left },
            roots.left.clone(),
            roots.right.clone(),
        )),
        (MergeTarget::Output, MergeOpKind::TakeLeft) => {
            let out = output_root(roots)?;
            Ok((
                FileOp::Copy { path: plan.path.clone(), from: Side::Left, to: Side::Right },
                roots.left.clone(),
                out,
            ))
        }
        (MergeTarget::Output, MergeOpKind::TakeRight) => {
            let out = output_root(roots)?;
            Ok((
                FileOp::Copy { path: plan.path.clone(), from: Side::Left, to: Side::Right },
                roots.right.clone(),
                out,
            ))
        }
        (MergeTarget::Output, MergeOpKind::DeleteTarget) => {
            let out = output_root(roots)?;
            Ok((FileOp::Delete { path: plan.path.clone(), side: Side::Right }, out.clone(), out))
        }
        (_, MergeOpKind::Keep) => Err("Keep 项不进入执行批次".into()),
        (MergeTarget::Right, MergeOpKind::TakeRight) | (MergeTarget::Left, MergeOpKind::TakeLeft) => {
            Err("目标即源侧的取侧决策不应出现在计划中".into())
        }
    }
}

fn output_root(roots: &MergeRoots) -> Result<VfsPath, String> {
    roots.output.clone().ok_or_else(|| "输出目标需要提供第三目录".to_string())
}

/// 端到端文件夹合并：计划（已由 [`plan_merge`] 预览确认）-> 执行 -> 报告。
///
/// 进度回调 (done, total, op) 为全批次统一编号；取消后剩余项记 Skipped。
/// 计划中的 Keep 项记 Planned（统计入 skipped，明细可追溯）。
pub async fn run_folder_merge(
    vfs: &dyn VfsProvider,
    roots: &MergeRoots,
    plan: &[MergePlannedOp],
    target: MergeTarget,
    progress: Option<&mut ProgressFn<'_>>,
    cancel: &CancelFlag,
) -> BatchRunReport {
    let mut report = BatchRunReport {
        dry_run: false,
        cancelled: false,
        stats: BatchStats { total: plan.len(), ..Default::default() },
        details: Vec::with_capacity(plan.len()),
        log: vec![format!(
            "合并目标：{}",
            match target {
                MergeTarget::Left => "左侧",
                MergeTarget::Right => "右侧",
                MergeTarget::Output => "输出文件夹",
            }
        )],
    };
    if target == MergeTarget::Output {
        // 输出目标先行校验（缺第三目录立即失败）
        if let Err(msg) = output_root(roots) {
            report.log.push(format!("失败 {msg}"));
            return report;
        }
    }

    // Keep 项直接记 Planned（不出现在执行批次）
    let mut items: Vec<(FileOp, VfsPath, VfsPath)> = Vec::with_capacity(plan.len());
    let mut planned_by_path: BTreeMap<String, &MergePlannedOp> = BTreeMap::new();
    for p in plan {
        match p.kind {
            MergeOpKind::Keep => {
                report.stats.skipped += 1;
                report.details.push(OpRecord {
                    kind: p.kind.as_str().into(),
                    path: p.path.clone(),
                    reason: p.reason.clone(),
                    status: RecordStatus::Planned,
                    detail: None,
                });
            }
            _ => match plan_to_exec(roots, target, p) {
                Ok(item) => {
                    planned_by_path.insert(p.path.clone(), p);
                    items.push(item);
                }
                Err(msg) => {
                    report.stats.failed += 1;
                    report.details.push(OpRecord {
                        kind: p.kind.as_str().into(),
                        path: p.path.clone(),
                        reason: p.reason.clone(),
                        status: RecordStatus::Failed,
                        detail: Some(msg),
                    });
                }
            },
        }
    }

    let exec = crate::execute::execute_ops_with_roots(vfs, &items, &ExecuteOptions::default(), progress, cancel).await;
    report.cancelled |= exec.cancelled;
    for r in exec.succeeded.iter() {
        let p = planned_by_path.get(r.op.path()).copied();
        report.stats.copied +=
            usize::from(matches!(p.map(|x| x.kind), Some(MergeOpKind::TakeLeft | MergeOpKind::TakeRight)));
        report.stats.deleted += usize::from(p.is_some_and(|x| x.kind == MergeOpKind::DeleteTarget));
        report.details.push(OpRecord {
            kind: p.map(|x| x.kind.as_str().into()).unwrap_or_else(|| "unknown".into()),
            path: r.op.path().into(),
            reason: p.map(|x| x.reason.clone()).unwrap_or_default(),
            status: match p.map(|x| x.kind) {
                Some(MergeOpKind::DeleteTarget) => RecordStatus::Deleted,
                _ => RecordStatus::Copied,
            },
            detail: r.detail.clone(),
        });
    }
    for (r, status) in exec.failed.iter().map(|r| (r, RecordStatus::Failed)).chain(exec.skipped.iter().map(|r| (r, RecordStatus::Skipped))) {
        let p = planned_by_path.get(r.op.path()).copied();
        if status == RecordStatus::Failed {
            report.stats.failed += 1;
        } else {
            report.stats.skipped += 1;
        }
        report.details.push(OpRecord {
            kind: p.map(|x| x.kind.as_str().into()).unwrap_or_else(|| "unknown".into()),
            path: r.op.path().into(),
            reason: p.map(|x| x.reason.clone()).unwrap_or_default(),
            status,
            detail: r.detail.clone(),
        });
    }

    report.log.push(report.summary_line("合并完成"));
    for d in &report.details {
        if d.status == RecordStatus::Failed {
            report.log.push(format!("失败 {}: {} — {}", d.path, d.kind, d.detail.clone().unwrap_or_default()));
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_vfs::LocalFs;

    /// 三树构造：每侧为 (rel, bytes) 列表；rel 以 '/' 结尾 = 仅建目录
    async fn make3(
        b: &[(&str, &[u8])],
        l: &[(&str, &[u8])],
        r: &[(&str, &[u8])],
    ) -> (tempfile::TempDir, VfsPath, VfsPath, VfsPath) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for name in ["B", "L", "R"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        for (name, spec) in [("B", b), ("L", l), ("R", r)] {
            for (rel, bytes) in spec.iter() {
                let p = root.join(name).join(rel);
                if rel.ends_with('/') {
                    std::fs::create_dir_all(&p).unwrap();
                } else {
                    if let Some(parent) = p.parent() {
                        std::fs::create_dir_all(parent).unwrap();
                    }
                    std::fs::write(&p, bytes).unwrap();
                }
            }
        }
        (
            dir,
            VfsPath::parse(root.join("B").to_str().unwrap()).unwrap(),
            VfsPath::parse(root.join("L").to_str().unwrap()).unwrap(),
            VfsPath::parse(root.join("R").to_str().unwrap()).unwrap(),
        )
    }

    async fn scan3(b: &VfsPath, l: &VfsPath, r: &VfsPath) -> MergeScanResult {
        let vfs = LocalFs::new();
        scan_merge(&vfs, b, l, r, &CancelFlag::new()).await.expect("scan")
    }

    fn cat_of(res: &MergeScanResult, rel: &str) -> MergeCategory {
        res.arena.iter().find(|n| n.rel_path == rel).expect("node").category
    }

    fn state_of(res: &MergeScanResult, rel: &str) -> MergeRowState {
        res.arena.iter().find(|n| n.rel_path == rel).expect("node").row_state
    }

    fn plan_of(res: &MergeScanResult, target: MergeTarget, decisions: &BTreeMap<String, MergeDecision>) -> MergePlanResult {
        plan_merge(&res.arena, &MergePlanParams { target, decisions })
    }

    fn op_tuples(plan: &MergePlanResult) -> Vec<(MergeOpKind, String)> {
        plan.ops.iter().map(|p| (p.kind, p.path.clone())).collect()
    }

    fn dec(pairs: &[(&str, MergeDecision)]) -> BTreeMap<String, MergeDecision> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    // ---- 扫描分类（真实 tempdir 三树） ----

    #[tokio::test]
    async fn three_way_all_same() {
        let (_d, b, l, r) = make3(&[("a.txt", b"v1")], &[("a.txt", b"v1")], &[("a.txt", b"v1")]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "a.txt"), MergeCategory::Same);
        assert_eq!(state_of(&res, "a.txt"), MergeRowState::Same);
        assert_eq!(res.stats.same, 1);
    }

    #[tokio::test]
    async fn left_changed_only_is_auto() {
        let (_d, b, l, r) = make3(&[("a.txt", b"v0")], &[("a.txt", b"v1")], &[("a.txt", b"v0")]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "a.txt"), MergeCategory::LeftChanged);
        assert_eq!(state_of(&res, "a.txt"), MergeRowState::Auto);
    }

    #[tokio::test]
    async fn right_changed_only_is_auto() {
        let (_d, b, l, r) = make3(&[("a.txt", b"v0")], &[("a.txt", b"v0")], &[("a.txt", b"v2")]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "a.txt"), MergeCategory::RightChanged);
    }

    #[tokio::test]
    async fn both_changed_same_way_is_both_same() {
        let (_d, b, l, r) = make3(&[("a.txt", b"v0")], &[("a.txt", b"v1")], &[("a.txt", b"v1")]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "a.txt"), MergeCategory::BothSame);
    }

    #[tokio::test]
    async fn different_changes_conflict() {
        let (_d, b, l, r) = make3(&[("a.txt", b"v0")], &[("a.txt", b"v1")], &[("a.txt", b"v2")]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "a.txt"), MergeCategory::Conflict);
        assert_eq!(state_of(&res, "a.txt"), MergeRowState::Conflict);
        let rows = res.rows();
        assert!(rows.iter().find(|x| x.rel_path == "a.txt").unwrap().conflict_leaf);
    }

    #[tokio::test]
    async fn left_added_file_orphan() {
        let (_d, b, l, r) = make3(&[], &[("new.txt", b"n")], &[]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "new.txt"), MergeCategory::LeftOnly);
        assert_eq!(state_of(&res, "new.txt"), MergeRowState::Orphan);
    }

    #[tokio::test]
    async fn both_added_same_content() {
        let (_d, b, l, r) = make3(&[], &[("add.txt", b"x")], &[("add.txt", b"x")]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "add.txt"), MergeCategory::BothSame);
    }

    #[tokio::test]
    async fn base_only_both_deleted() {
        let (_d, b, l, r) = make3(&[("gone.txt", b"g")], &[], &[]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "gone.txt"), MergeCategory::BaseOnly);
    }

    #[tokio::test]
    async fn right_deleted_left_unchanged_auto_follow() {
        let (_d, b, l, r) = make3(&[("f.txt", b"v0")], &[("f.txt", b"v0")], &[]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "f.txt"), MergeCategory::DeletedRight);
        assert_eq!(state_of(&res, "f.txt"), MergeRowState::Auto);
    }

    #[tokio::test]
    async fn modify_delete_combination_conflicts() {
        let (_d, b, l, r) = make3(&[("f.txt", b"v0")], &[("f.txt", b"v1")], &[]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "f.txt"), MergeCategory::Conflict);
    }

    #[tokio::test]
    async fn dir_aggregates_conflict_from_child() {
        let (_d, b, l, r) = make3(
            &[("d/x.txt", b"v0"), ("d/y.txt", b"v0")],
            &[("d/x.txt", b"v1"), ("d/y.txt", b"v0")],
            &[("d/x.txt", b"v2"), ("d/y.txt", b"v0")],
        )
        .await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "d/x.txt"), MergeCategory::Conflict);
        assert_eq!(state_of(&res, "d"), MergeRowState::Conflict);
        assert_eq!(res.stats.conflict, 2, "目录行 + 冲突叶");
        assert_eq!(res.stats.dirs, 1);
    }

    #[tokio::test]
    async fn orphan_dir_subtree_gray() {
        let (_d, b, l, r) = make3(&[], &[("od/a.txt", b"a"), ("od/sub/b.txt", b"b")], &[]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(state_of(&res, "od"), MergeRowState::Orphan);
        assert_eq!(cat_of(&res, "od/a.txt"), MergeCategory::LeftOnly);
        assert_eq!(cat_of(&res, "od/sub/b.txt"), MergeCategory::LeftOnly);
        assert_eq!(state_of(&res, "od/sub"), MergeRowState::Orphan);
        let rows = res.rows();
        let nested = rows.iter().find(|x| x.rel_path == "od/sub/b.txt").unwrap();
        assert_eq!(nested.parent.as_deref(), Some("od/sub"));
        assert_eq!(nested.depth, 3);
    }

    #[tokio::test]
    async fn type_conflict_file_vs_dir() {
        let (_d, b, l, r) = make3(&[("clash", b"file")], &[("clash/inner.txt", b"x")], &[("clash", b"file")]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(cat_of(&res, "clash"), MergeCategory::Conflict);
        // 不入子树：左目录内容不展开
        assert!(res.arena.iter().all(|n| n.rel_path != "clash/inner.txt"));
    }

    #[test]
    fn align3_covers_all_entries_sorted() {
        let mk = |name: &str| VfsDirEntry { name: name.into(), stat: ai_vfs::StatInfo::minimal(1, 0, ai_vfs::EntryKind::File) };
        let b = vec![mk("a"), mk("c")];
        let l = vec![mk("a"), mk("b")];
        let r = vec![mk("b"), mk("c"), mk("d")];
        let t = align3(&b, &l, &r, |n| n.to_string());
        assert_eq!(
            t.iter().map(|x| (x.name.as_str(), x.base, x.left, x.right)).collect::<Vec<_>>(),
            vec![
                ("a", Some(0), Some(0), None),
                ("b", None, Some(1), Some(0)),
                ("c", Some(1), None, Some(1)),
                ("d", None, None, Some(2)),
            ]
        );
    }

    // ---- 计划生成（默认决策表 + 显式决策） ----

    /// 六类叶形态的合成树：
    /// same=三方同；lc=仅左改；rc=仅右改；cf=冲突；dl=左删（base+右）；lo=左孤立
    async fn make_mixed_tree() -> (tempfile::TempDir, MergeScanResult) {
        let (_d, b, l, r) = make3(
            &[("same.txt", b"s"), ("lc.txt", b"v0"), ("rc.txt", b"v0"), ("cf.txt", b"v0"), ("dl.txt", b"v0")],
            &[("same.txt", b"s"), ("lc.txt", b"v1"), ("rc.txt", b"v0"), ("cf.txt", b"v1"), ("lo.txt", b"o")],
            &[("same.txt", b"s"), ("lc.txt", b"v0"), ("rc.txt", b"v2"), ("cf.txt", b"v2"), ("dl.txt", b"v0")],
        )
        .await;
        let res = scan3(&b, &l, &r).await;
        (_d, res)
    }

    #[tokio::test]
    async fn plan_defaults_target_right() {
        let (_d, res) = make_mixed_tree().await;
        let plan = plan_of(&res, MergeTarget::Right, &BTreeMap::new());
        // 仅左改/左孤立取左；左删跟随删除；右改/三方同/冲突不动
        assert_eq!(
            op_tuples(&plan),
            vec![
                (MergeOpKind::DeleteTarget, "dl.txt".to_string()),
                (MergeOpKind::TakeLeft, "lc.txt".to_string()),
                (MergeOpKind::TakeLeft, "lo.txt".to_string()),
            ]
        );
        assert_eq!(plan.stats.copied, 2);
        assert_eq!(plan.stats.deleted, 1);
    }

    #[tokio::test]
    async fn plan_defaults_target_left_mirrored() {
        let (_d, res) = make_mixed_tree().await;
        let plan = plan_of(&res, MergeTarget::Left, &BTreeMap::new());
        // 右改取右；右删（dr）跟随删除 —— 本树无右删/右孤立，仅 rc 需动作
        assert_eq!(op_tuples(&plan), vec![(MergeOpKind::TakeRight, "rc.txt".to_string())]);
    }

    #[tokio::test]
    async fn plan_output_snapshot_copies_authoritative_sides() {
        let (_d, res) = make_mixed_tree().await;
        let plan = plan_of(&res, MergeTarget::Output, &BTreeMap::new());
        // 输出快照：三方同也复制；冲突/两侧已删不动
        let ops = op_tuples(&plan);
        assert!(ops.contains(&(MergeOpKind::TakeLeft, "same.txt".to_string())));
        assert!(ops.contains(&(MergeOpKind::TakeLeft, "lc.txt".to_string())));
        assert!(ops.contains(&(MergeOpKind::TakeRight, "rc.txt".to_string())));
        assert!(ops.contains(&(MergeOpKind::TakeLeft, "lo.txt".to_string())));
        assert!(ops.iter().all(|(_k, p)| !p.contains("cf.txt") && !p.contains("dl.txt")), "冲突/已删不进输出: {ops:?}");
        assert_eq!(plan.stats.copied, 4);
    }

    #[tokio::test]
    async fn plan_decision_take_left_on_conflict() {
        let (_d, res) = make_mixed_tree().await;
        let d = dec(&[("cf.txt", MergeDecision::TakeLeft)]);
        let plan = plan_of(&res, MergeTarget::Right, &d);
        assert!(op_tuples(&plan).contains(&(MergeOpKind::TakeLeft, "cf.txt".to_string())));
        // 显式决策保留原默认动作外还应有决策原因
        let op = plan.ops.iter().find(|p| p.path == "cf.txt").unwrap();
        assert!(op.reason.contains("取左"));
    }

    #[tokio::test]
    async fn plan_decision_skip_keeps_conflict() {
        let (_d, res) = make_mixed_tree().await;
        let d = dec(&[("cf.txt", MergeDecision::Skip)]);
        let plan = plan_of(&res, MergeTarget::Right, &d);
        let op = plan.ops.iter().find(|p| p.path == "cf.txt").unwrap();
        assert_eq!(op.kind, MergeOpKind::Keep);
        assert!(op.reason.contains("跳过"));
    }

    #[tokio::test]
    async fn plan_decision_skip_dir_expands_to_subtree() {
        let (_d, b, l, r) = make3(
            &[("d/x.txt", b"v0")],
            &[("d/x.txt", b"v1")],
            &[("d/x.txt", b"v2")],
        )
        .await;
        let res = scan3(&b, &l, &r).await;
        let d = dec(&[("d", MergeDecision::Skip)]);
        let plan = plan_of(&res, MergeTarget::Right, &d);
        // 整树跳过：单条 Keep 记录，无子项动作
        assert_eq!(op_tuples(&plan), vec![(MergeOpKind::Keep, "d".to_string())]);
        assert_eq!(plan.stats.skipped, 1);
    }

    #[tokio::test]
    async fn plan_decision_inheritance_take_right_on_dir() {
        let (_d, b, l, r) = make3(
            &[("d/x.txt", b"v0"), ("d/y.txt", b"v0")],
            &[("d/x.txt", b"v1"), ("d/y.txt", b"v1")],
            &[("d/x.txt", b"v2"), ("d/y.txt", b"v2")],
        )
        .await;
        let res = scan3(&b, &l, &r).await;
        let d = dec(&[("d", MergeDecision::TakeRight)]);
        let plan = plan_of(&res, MergeTarget::Right, &d);
        // 目录取右继承到子项：全部 takeRight（目标=右侧，取右即保持 → Keep 不入计划）
        assert!(plan.ops.iter().all(|p| p.kind == MergeOpKind::Keep), "{:?}", plan.ops);
        // 换目标 = 左：子项全部复制右→左
        let plan_l = plan_of(&res, MergeTarget::Left, &d);
        assert_eq!(
            op_tuples(&plan_l),
            vec![(MergeOpKind::TakeRight, "d/x.txt".to_string()), (MergeOpKind::TakeRight, "d/y.txt".to_string())]
        );
    }

    #[tokio::test]
    async fn plan_decision_both_dir_falls_back_to_defaults() {
        let (_d, b, l, r) = make3(
            &[("d/x.txt", b"v0")],
            &[("d/x.txt", b"v1")],
            &[("d/x.txt", b"v2")],
        )
        .await;
        let res = scan3(&b, &l, &r).await;
        let d = dec(&[("d", MergeDecision::Both)]);
        let plan = plan_of(&res, MergeTarget::Right, &d);
        // 冲突子项在 both 继承下仍按默认（人工冲突不动）
        let op = plan.ops.iter().find(|p| p.path == "d/x.txt").unwrap();
        assert_eq!(op.kind, MergeOpKind::Keep);
        assert!(op.reason.contains("冲突") || op.reason.contains("both"), "{}", op.reason);
    }

    #[tokio::test]
    async fn plan_empty_orphan_dir_adopted() {
        let (_d, b, l, r) = make3(&[], &[("ed/", b"")], &[]).await;
        let res = scan3(&b, &l, &r).await;
        assert_eq!(state_of(&res, "ed"), MergeRowState::Orphan);
        let plan = plan_of(&res, MergeTarget::Right, &BTreeMap::new());
        assert_eq!(op_tuples(&plan), vec![(MergeOpKind::TakeLeft, "ed".to_string())]);
        assert!(plan.ops[0].reason.contains("空目录"));
    }

    #[tokio::test]
    async fn plan_take_left_on_missing_source_keeps() {
        let (_d, b, l, r) = make3(&[("only_r.txt", b"x")], &[], &[("only_r.txt", b"x")]).await;
        let res = scan3(&b, &l, &r).await;
        let d = dec(&[("only_r.txt", MergeDecision::TakeLeft)]);
        let plan = plan_of(&res, MergeTarget::Right, &d);
        let op = plan.ops.iter().find(|p| p.path == "only_r.txt").unwrap();
        assert_eq!(op.kind, MergeOpKind::Keep);
        assert!(op.reason.contains("左侧缺失"));
    }

    // ---- 端到端执行（tempdir 落盘） ----

    #[tokio::test]
    async fn run_merge_to_right_end_to_end() {
        let (_dir, b, l, r) = make3(
            &[("same.txt", b"s"), ("lc.txt", b"v0"), ("cf.txt", b"v0"), ("dl.txt", b"v0")],
            &[("same.txt", b"s"), ("lc.txt", b"v1"), ("cf.txt", b"v1"), ("new.txt", b"n")],
            &[("same.txt", b"s"), ("lc.txt", b"v0"), ("cf.txt", b"v2"), ("dl.txt", b"v0")],
        )
        .await;
        let vfs = LocalFs::new();
        let res = scan_merge(&vfs, &b, &l, &r, &CancelFlag::new()).await.unwrap();
        let plan = plan_merge(&res.arena, &MergePlanParams { target: MergeTarget::Right, decisions: &BTreeMap::new() });
        let mut progress_seen = 0usize;
        let roots = MergeRoots { base: b.clone(), left: l.clone(), right: r.clone(), output: None };
        let report = run_folder_merge(&vfs, &roots, &plan.ops, MergeTarget::Right, Some(&mut |done, _t, _| progress_seen = done), &CancelFlag::new()).await;
        assert_eq!(report.stats.failed, 0, "{:?}", report.details.iter().filter(|d| d.status == RecordStatus::Failed).collect::<Vec<_>>());
        // lc：左改 → 右侧被左覆盖；new：左孤立并入；dl：左删 → 右侧删除；cf：冲突保持
        assert_eq!(vfs.read_file_bytes(&r.join("lc.txt")).await.unwrap(), b"v1");
        assert_eq!(vfs.read_file_bytes(&r.join("new.txt")).await.unwrap(), b"n");
        assert!(vfs.stat(&r.join("dl.txt")).await.is_err());
        assert_eq!(vfs.read_file_bytes(&r.join("cf.txt")).await.unwrap(), b"v2");
        assert_eq!(report.stats.copied, 2);
        assert_eq!(report.stats.deleted, 1);
        assert!(progress_seen >= 3, "进度统一编号: {progress_seen}");
        assert!(report.log[0].contains("合并目标：右侧"));
    }

    #[tokio::test]
    async fn run_merge_to_output_snapshot() {
        let (dir, b, l, r) = make3(
            &[("same.txt", b"s"), ("lc.txt", b"v0"), ("cf.txt", b"v0")],
            &[("same.txt", b"s"), ("lc.txt", b"v1"), ("cf.txt", b"v1"), ("lo.txt", b"o")],
            &[("same.txt", b"s"), ("lc.txt", b"v0"), ("cf.txt", b"v2")],
        )
        .await;
        let out_root = dir.path().join("OUT");
        std::fs::create_dir_all(&out_root).unwrap();
        let vfs = LocalFs::new();
        let res = scan_merge(&vfs, &b, &l, &r, &CancelFlag::new()).await.unwrap();
        let plan = plan_merge(&res.arena, &MergePlanParams { target: MergeTarget::Output, decisions: &BTreeMap::new() });
        let roots = MergeRoots {
            base: b.clone(),
            left: l.clone(),
            right: r.clone(),
            output: Some(VfsPath::parse(out_root.to_str().unwrap()).unwrap()),
        };
        let report = run_folder_merge(&vfs, &roots, &plan.ops, MergeTarget::Output, None, &CancelFlag::new()).await;
        assert_eq!(report.stats.failed, 0, "{:?}", report.details);
        let out = VfsPath::parse(out_root.to_str().unwrap()).unwrap();
        assert_eq!(vfs.read_file_bytes(&out.join("same.txt")).await.unwrap(), b"s");
        assert_eq!(vfs.read_file_bytes(&out.join("lc.txt")).await.unwrap(), b"v1", "输出取左");
        assert_eq!(vfs.read_file_bytes(&out.join("lo.txt")).await.unwrap(), b"o", "左孤立并入输出");
        assert!(vfs.stat(&out.join("cf.txt")).await.is_err(), "冲突项不进输出");
        assert!(!report.cancelled);
    }

    #[tokio::test]
    async fn run_merge_output_requires_third_root() {
        let (_d, b, l, r) = make3(&[("a.txt", b"v0")], &[("a.txt", b"v1")], &[("a.txt", b"v0")]).await;
        let vfs = LocalFs::new();
        let res = scan_merge(&vfs, &b, &l, &r, &CancelFlag::new()).await.unwrap();
        let plan = plan_merge(&res.arena, &MergePlanParams { target: MergeTarget::Output, decisions: &BTreeMap::new() });
        let roots = MergeRoots { base: b, left: l, right: r, output: None };
        let report = run_folder_merge(&vfs, &roots, &plan.ops, MergeTarget::Output, None, &CancelFlag::new()).await;
        assert_eq!(report.stats.copied, 0);
        assert!(report.log.iter().any(|l| l.contains("第三目录")));
    }

    #[tokio::test]
    async fn run_merge_cancel_skips_remaining() {
        let (_d, b, l, r) = make3(&[("a.txt", b"v0")], &[("a.txt", b"v1")], &[("a.txt", b"v0")]).await;
        let vfs = LocalFs::new();
        let res = scan_merge(&vfs, &b, &l, &r, &CancelFlag::new()).await.unwrap();
        let plan = plan_merge(&res.arena, &MergePlanParams { target: MergeTarget::Right, decisions: &BTreeMap::new() });
        let cancel = CancelFlag::new();
        cancel.cancel();
        let roots = MergeRoots { base: b.clone(), left: l.clone(), right: r.clone(), output: None };
        let report = run_folder_merge(&vfs, &roots, &plan.ops, MergeTarget::Right, None, &cancel).await;
        assert!(report.cancelled);
        assert_eq!(report.stats.copied, 0, "取消后不落盘");
        assert!(report.stats.skipped >= 1);
        assert_eq!(vfs.read_file_bytes(&r.join("a.txt")).await.unwrap(), b"v0");
    }

    #[tokio::test]
    async fn run_merge_empty_orphan_dir_created_on_target() {
        let (_d, b, l, r) = make3(&[], &[("ed/", b"")], &[]).await;
        let vfs = LocalFs::new();
        let res = scan_merge(&vfs, &b, &l, &r, &CancelFlag::new()).await.unwrap();
        let plan = plan_merge(&res.arena, &MergePlanParams { target: MergeTarget::Right, decisions: &BTreeMap::new() });
        let roots = MergeRoots { base: b.clone(), left: l.clone(), right: r.clone(), output: None };
        let report = run_folder_merge(&vfs, &roots, &plan.ops, MergeTarget::Right, None, &CancelFlag::new()).await;
        assert_eq!(report.stats.failed, 0, "{:?}", report.details);
        let st = vfs.stat(&r.join("ed")).await.unwrap();
        assert_eq!(st.kind, ai_vfs::EntryKind::Dir);
    }

    #[tokio::test]
    async fn run_merge_partial_success_collects_failures() {
        // 构造“计划有动作但目标侧类型冲突”的场景：右为目录、计划 takeLeft 文件覆盖
        let (_d, b, l, r) = make3(&[("x", b"v0")], &[("x", b"v1")], &[("x", b"v0")]).await;
        // 把右侧 x 改成目录（扫描后再改，制造执行期冲突）
        let vfs = LocalFs::new();
        let res = scan_merge(&vfs, &b, &l, &r, &CancelFlag::new()).await.unwrap();
        std::fs::remove_file(r.as_str().to_string() + "/x").unwrap();
        std::fs::create_dir(r.as_str().to_string() + "/x").unwrap();
        let plan = plan_merge(&res.arena, &MergePlanParams { target: MergeTarget::Right, decisions: &BTreeMap::new() });
        let roots = MergeRoots { base: b.clone(), left: l.clone(), right: r.clone(), output: None };
        let report = run_folder_merge(&vfs, &roots, &plan.ops, MergeTarget::Right, None, &CancelFlag::new()).await;
        assert_eq!(report.stats.failed, 1, "{:?}", report.details);
        assert!(report.log.iter().any(|l| l.contains("失败")));
    }

    #[test]
    fn merge_stats_and_rows_shape() {
        // 纯结构验证：DirMergeRow 字段形态（协议提案 DTO 同形锚点）
        let row = DirMergeRow {
            rel_path: "a/b.txt".into(),
            name: "b.txt".into(),
            kind: EntryKind::File,
            base: None,
            left: None,
            right: None,
            category: MergeCategory::Conflict,
            row_state: MergeRowState::Conflict,
            parent: Some("a".into()),
            depth: 2,
            conflict_leaf: true,
        };
        assert_eq!(row.category.as_str(), "conflict");
        assert_eq!(row.row_state.as_str(), "conflict");
    }
}
