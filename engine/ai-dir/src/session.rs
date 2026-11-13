//! 目录会话模型（ADR-004：惰性 + 后台扫描 + 可视优先 + 聚合状态增量上推）。
//!
//! 生命周期：
//! 1. [`DirModel::open`]：立即同步展开可视层（默认顶层）并产出首个 delta
//!    —— UI 可立即滚动（FR-DIR-043 后台扫描语义）；
//! 2. [`DirModel::start_background`]：tokio worker 池继续全量扫描，
//!    信号量式限并发（worker 数 `min(32, cpus*4)`）、两级优先级队列
//!    （前台 expand > 后台全量）、逐目录聚合状态增量上推（FR-DIR-002）、
//!    取消令牌全链路、进度事件（scanned / total 估算）；
//! 3. [`DirModel::take_delta`]：引擎 pump 轮询取增量（rev 单调递增）；
//! 4. dir/expand（前台优先）/ dir/filter（重扫）/ dir/align（手动对齐）/
//!    平铺模式（FR-DIR-021）在会话存续期间动态施加。
//!
//! 并发纪律：`std::sync::Mutex<ModelState>` 只保护 arena/队列/脏集等纯数据；
//! 任何 IO（list_dir / 内容比较）在锁外执行；worker 在每个 await 后检查
//! 取消令牌与重扫代际，保证取消后无悬挂任务。
//!
//! 每条目恰被一个处理者展开：工作项在**出队时**登记 in_flight（持锁），
//! 已展开/在途的目录项直接丢弃；dir/expand 预登记后由 worker 或同步路径执行。

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai_protocol::{
    CompareCriteria, ContentCompareMode, DirEntryStatus, DirRow, DirStats, DisplayFilterSpec,
    EntryKind, NameFilterSpec, OtherFilterSpec, ProgressPhase, ScanProgress, SideInfo,
};
use ai_vfs::{dos_attr, EntryKind as VfsEntryKind, StatInfo as VfsStat, VfsDirEntry, VfsPath, VfsProvider};

use crate::align::{NameAligner, AlignedPair};
use crate::cache::HashCache;
use crate::filter::{NameFilter, OtherFilter};
use crate::quick::{QuickDiffReason, QuickOutcome, QuickTest};
use crate::{aggregate_children, DirNode, DirScanError, NodeId};

/// 后台扫描 worker 并发上限（ADR-004：本地默认 min(32, cpus*4)）
pub fn default_workers() -> usize {
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    (cpus * 4).clamp(1, 32)
}

/// 会话选项
#[derive(Clone)]
pub struct DirSessionOptions {
    /// 后台目标深度（None = 全量；与协议 DirOpenParams.depth 对应）
    pub target_depth: Option<u32>,
    /// open 时同步展开层级（可视优先；默认 1 = 顶层）
    pub visible_depth: u32,
    pub criteria: CompareCriteria,
    pub aligner: Arc<dyn NameAligner>,
    pub name_filter: Option<NameFilterSpec>,
    pub other_filter: Option<OtherFilterSpec>,
    pub display_filter: DisplayFilterSpec,
    pub suppress: bool,
    pub flat: bool,
    /// 手动对齐（两侧 rel 路径对）
    pub manual_pairs: Vec<(String, String)>,
    /// stat/hash 持久缓存
    pub cache: Option<Arc<HashCache>>,
    /// worker 数（0 = default_workers()）
    pub workers: usize,
}

impl std::fmt::Debug for DirSessionOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirSessionOptions")
            .field("target_depth", &self.target_depth)
            .field("visible_depth", &self.visible_depth)
            .field("criteria", &self.criteria)
            .field("aligner", &self.aligner.mode_name())
            .field("has_name_filter", &self.name_filter.is_some())
            .field("has_other_filter", &self.other_filter.is_some())
            .field("display_filter", &self.display_filter)
            .field("suppress", &self.suppress)
            .field("flat", &self.flat)
            .field("manual_pairs", &self.manual_pairs.len())
            .field("cache", &self.cache.is_some())
            .field("workers", &self.workers)
            .finish()
    }
}

impl DirSessionOptions {
    pub fn new(criteria: CompareCriteria, aligner: Arc<dyn NameAligner>) -> Self {
        DirSessionOptions {
            target_depth: None,
            visible_depth: 1,
            criteria,
            aligner,
            name_filter: None,
            other_filter: None,
            display_filter: DisplayFilterSpec::default(),
            suppress: false,
            flat: false,
            manual_pairs: Vec::new(),
            cache: None,
            workers: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prio {
    Foreground,
    Background,
}

#[derive(Debug, Clone)]
struct WorkItem {
    dir_id: NodeId,
    lp: VfsPath,
    rp: VfsPath,
    level: u32,
    #[allow(dead_code)]
    prio: Prio,
    /// 重扫代际（代际不符的项被 worker 丢弃）
    gen: u64,
}

/// delta 快照（引擎 pump 的取数结果）
#[derive(Debug, Clone, PartialEq)]
pub struct DirDeltaData {
    pub rev: u64,
    pub upserts: Vec<DirRow>,
    pub removals: Vec<String>,
    pub stats: DirStats,
    pub progress: ScanProgress,
}

struct ModelState {
    arena: Vec<DirNode>,
    index: HashMap<String, NodeId>,
    dirty: BTreeSet<NodeId>,
    removals: Vec<String>,
    fg: VecDeque<WorkItem>,
    bg: VecDeque<WorkItem>,
    pending: usize,
    in_flight: BTreeSet<NodeId>,
    manual_pairs: Vec<(String, String)>,
    stats: DirStats,
    errors: Vec<(String, String)>,
    /// 平铺视图缓存（id -> 上次发出的行；差分输出用）
    flat_cache: BTreeMap2,
    flat_ids: std::collections::BTreeSet<String>,
    /// 平铺视图需要重算（扫描推进/显示状态变化）
    flat_dirty: bool,
    gen: u64,
}

/// BTreeMap<String, DirRow> 别名（保持字段块排版可读）
type BTreeMap2 = std::collections::BTreeMap<String, DirRow>;

const PHASE_RUNNING: u8 = 0;
const PHASE_COMPLETED: u8 = 1;
const PHASE_CANCELLED: u8 = 2;

/// 目录会话模型
pub struct DirModel {
    opts: Mutex<DirSessionOptions>,
    vfs: Arc<dyn VfsProvider>,
    roots: (VfsPath, VfsPath),
    cancel: crate::CancelFlag,
    state: Mutex<ModelState>,
    scanned: AtomicU32,
    phase: AtomicU8,
    est_extra: AtomicU32,
    avg_fanout: AtomicU32,
    rev: AtomicU64,
    /// 存活 worker 数（start_background 幂等用）
    live_workers: std::sync::atomic::AtomicUsize,
    notify: tokio::sync::Notify,
}

impl DirModel {
    // ------------------------------------------------------------------
    // 打开（同步可视层）
    // ------------------------------------------------------------------

    /// 打开会话：根 stat + 同步展开 `visible_depth` 层 + 应用手动对齐。
    /// 返回（模型, 首个 delta）。目标深度已被可视层覆盖时不启动后台。
    pub async fn open(
        vfs: Arc<dyn VfsProvider>,
        left: &VfsPath,
        right: &VfsPath,
        opts: DirSessionOptions,
        cancel: crate::CancelFlag,
    ) -> Result<(Arc<DirModel>, DirDeltaData), DirScanError> {
        let ls = vfs.stat(left).await.map_err(|e| crate::map_root_err("left", left, e))?;
        let rs = vfs.stat(right).await.map_err(|e| crate::map_root_err("right", right, e))?;
        if ls.kind != VfsEntryKind::Dir || rs.kind != VfsEntryKind::Dir {
            return Err(DirScanError::Vfs(ai_vfs::VfsError::NotADirectory { path: format!("{left} / {right}") }));
        }

        let arena = vec![DirNode {
            rel_path: String::new(),
            name: String::new(),
            kind: EntryKind::Dir,
            left: Some(proto_side(&ls, &opts.criteria)),
            right: Some(proto_side(&rs, &opts.criteria)),
            status: DirEntryStatus::DirSame,
            parent: None,
            children: Vec::new(),
            expanded: false,
            aligned_with: None,
        }];
        // 根节点不计入可见行统计（旧 compute_stats 跳过 arena[0] 的语义）
        let stats = DirStats::default();
        let model = Arc::new(DirModel {
            opts: Mutex::new(opts.clone()),
            vfs,
            roots: (left.clone(), right.clone()),
            cancel,
            state: Mutex::new(ModelState {
                arena,
                index: HashMap::from([(String::new(), 0)]),
                dirty: BTreeSet::from([0]),
                removals: Vec::new(),
                fg: VecDeque::new(),
                bg: VecDeque::new(),
                pending: 0,
                in_flight: BTreeSet::new(),
                manual_pairs: opts.manual_pairs.clone(),
                stats,
                errors: Vec::new(),
                flat_cache: Default::default(),
                flat_ids: Default::default(),
                flat_dirty: true,
                gen: 0,
            }),
            scanned: AtomicU32::new(0),
            phase: AtomicU8::new(PHASE_RUNNING),
            est_extra: AtomicU32::new(0),
            avg_fanout: AtomicU32::new(0),
            rev: AtomicU64::new(0),
            live_workers: std::sync::atomic::AtomicUsize::new(0),
            notify: tokio::sync::Notify::new(),
        });

        // 同步展开可视层（前台）
        model.expand_sync(0, 0, opts.visible_depth).await;
        if model.cancel.is_cancelled() {
            model.phase.store(PHASE_CANCELLED, Ordering::SeqCst);
        }
        // 应用预置手动对齐
        let pairs = model.manual_pairs();
        for (l, r) in &pairs {
            let _ = model.align(l, r).await;
        }
        let done_now = opts.target_depth.is_some_and(|d| d <= opts.visible_depth.max(1));
        if done_now && model.phase.load(Ordering::SeqCst) == PHASE_RUNNING {
            model.finish_locked();
        } else if !done_now {
            model.enqueue_frontier_background();
        }
        let _ = model.rev.fetch_add(1, Ordering::SeqCst);
        let delta = model.take_delta().await.unwrap_or_else(|| DirDeltaData {
            rev: 1,
            upserts: Vec::new(),
            removals: Vec::new(),
            stats: DirStats::default(),
            progress: model.progress(),
        });
        model.notify.notify_waiters();
        Ok((model, delta))
    }

    fn manual_pairs(&self) -> Vec<(String, String)> {
        self.state.lock().expect("模型锁").manual_pairs.clone()
    }

    /// 把可视层展开后仍未展开的两侧目录入队后台（深度不超目标）
    fn enqueue_frontier_background(&self) {
        let target = self.target_depth().unwrap_or(u32::MAX);
        let mut st = self.state.lock().expect("模型锁");
        let snapshot: Vec<(NodeId, String, u32)> = st
            .arena
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_paired_dir() && !n.expanded)
            .map(|(i, n)| {
                let level = if n.rel_path.is_empty() { 0 } else { n.rel_path.split('/').count() as u32 };
                (i as NodeId, n.rel_path.clone(), level)
            })
            .collect();
        let gen = st.gen;
        for (id, rel, level) in snapshot {
            if level > target {
                continue;
            }
            let lp = self.roots.0.join(&rel);
            let rp = self.roots.1.join(&rel);
            st.bg.push_back(WorkItem { dir_id: id, lp, rp, level, prio: Prio::Background, gen });
            st.pending += 1;
        }
        self.recompute_est(&mut st);
    }

    // ------------------------------------------------------------------
    // 后台 worker 池
    // ------------------------------------------------------------------

    /// 启动后台扫描 worker 池（返回句柄供取消后泄漏断言）。
    /// 幂等：已有存活 worker 或已不在 Running 时不重复启动。
    pub fn start_background(self: &Arc<Self>) -> Vec<tokio::task::JoinHandle<()>> {
        if self.phase.load(Ordering::SeqCst) != PHASE_RUNNING {
            return Vec::new();
        }
        if self.live_workers.load(Ordering::SeqCst) > 0 {
            return Vec::new();
        }
        let workers = {
            let opts = self.opts.lock().expect("选项锁");
            if opts.workers == 0 { default_workers() } else { opts.workers }
        };
        let handles: Vec<tokio::task::JoinHandle<()>> = (0..workers)
            .map(|_| {
                let m = Arc::clone(self);
                tokio::spawn(async move { m.worker_loop().await })
            })
            .collect();
        self.live_workers.fetch_add(handles.len(), Ordering::SeqCst);
        handles
    }

    /// 出队 + in_flight 登记（持锁）。已展开/在途的项丢弃（pending 一并回收）。
    fn pop_item(&self) -> Option<WorkItem> {
        let mut st = self.state.lock().expect("模型锁");
        loop {
            let item = st.fg.pop_front().or_else(|| st.bg.pop_front())?;
            if st.gen != item.gen || st.arena[item.dir_id as usize].expanded || st.in_flight.contains(&item.dir_id) {
                st.pending = st.pending.saturating_sub(1);
                continue;
            }
            st.in_flight.insert(item.dir_id);
            return Some(item);
        }
    }

    async fn worker_loop(self: Arc<Self>) {
        let _guard = WorkerGuard(Arc::clone(&self));
        loop {
            if self.cancel.is_cancelled() {
                if self.phase.load(Ordering::SeqCst) == PHASE_RUNNING {
                    self.phase.store(PHASE_CANCELLED, Ordering::SeqCst);
                    self.notify.notify_waiters();
                }
                return;
            }
            match self.pop_item() {
                Some(item) => {
                    self.process_item(item).await;
                    {
                        let mut st = self.state.lock().expect("模型锁");
                        st.pending = st.pending.saturating_sub(1);
                    }
                    self.notify.notify_waiters();
                }
                None => {
                    let done = {
                        let st = self.state.lock().expect("模型锁");
                        st.pending == 0
                    };
                    if done {
                        if self.phase.load(Ordering::SeqCst) == PHASE_RUNNING {
                            self.finish_locked();
                        }
                        self.notify.notify_waiters();
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            }
        }
    }

    /// 终态：全量聚合 + 估算清零 + 置 Completed（持锁调用）
    fn finish_locked(&self) {
        let mut st = self.state.lock().expect("模型锁");
        for id in (0..st.arena.len() as NodeId).rev() {
            let node = &st.arena[id as usize];
            if node.is_paired_dir() && node.expanded {
                let new_status = aggregate_children(node, &st.arena);
                self.set_status(&mut st, id, new_status);
            }
        }
        st.flat_dirty = true;
        self.est_extra.store(0, Ordering::SeqCst);
        self.phase.store(PHASE_COMPLETED, Ordering::SeqCst);
    }

    /// 处理一个目录工作项（IO 在锁外；提交时校验代际与展开状态）
    async fn process_item(&self, item: WorkItem) {
        if self.cancel.is_cancelled() {
            return;
        }
        let target = self.target_depth().unwrap_or(u32::MAX);
        if item.level > target {
            let mut st = self.state.lock().expect("模型锁");
            self.mark_expanded(&mut st, item.dir_id);
            return;
        }
        let opts = self.opts.lock().expect("选项锁").clone();
        let name_filter = NameFilter::from_spec(opts.name_filter.as_ref().unwrap_or(&NameFilterSpec::default()));
        let parent_rel = {
            let st = self.state.lock().expect("模型锁");
            st.arena[item.dir_id as usize].rel_path.clone()
        };
        let dir_segments: Vec<&str> = parent_rel.split('/').filter(|s| !s.is_empty()).collect();
        if name_filter.dir_pruned(&dir_segments) {
            let mut st = self.state.lock().expect("模型锁");
            self.mark_expanded(&mut st, item.dir_id);
            return;
        }

        let (mut le, mut re) = match (self.vfs.list_dir(&item.lp).await, self.vfs.list_dir(&item.rp).await) {
            (Ok(l), Ok(r)) => (l, r),
            (Err(e), _) | (_, Err(e)) => {
                let mut st = self.state.lock().expect("模型锁");
                if st.gen == item.gen {
                    let rel = st.arena[item.dir_id as usize].rel_path.clone();
                    st.errors.push((rel, e.to_string()));
                    self.mark_expanded(&mut st, item.dir_id);
                }
                return;
            }
        };
        // await 后校验：取消 / 代际失效 / 已被他人展开
        if self.cancel.is_cancelled() {
            return;
        }
        {
            let st = self.state.lock().expect("模型锁");
            if st.gen != item.gen || st.arena[item.dir_id as usize].expanded {
                return;
            }
        }
        le.sort_by(|a, b| a.name.cmp(&b.name));
        re.sort_by(|a, b| a.name.cmp(&b.name));
        let pairs = opts.aligner.align(&le, &re);

        let dctx = DecideCtx { opts: &opts, name_filter: &name_filter };
        let mut decided: Vec<DecidedChild> = Vec::with_capacity(pairs.len());
        for pair in &pairs {
            if self.cancel.is_cancelled() {
                return;
            }
            if let Some(d) = self.decide_pair(&dctx, &parent_rel, pair, &le, &re).await {
                decided.push(d);
            }
        }

        let fanout = decided.len() as u32;
        {
            let mut st = self.state.lock().expect("模型锁");
            if st.gen != item.gen || st.arena[item.dir_id as usize].expanded {
                return;
            }
            self.commit_children(&mut st, &opts, item.dir_id, item.level, decided);
            self.mark_expanded(&mut st, item.dir_id);
            self.recompute_est(&mut st);
        }
        self.scanned.fetch_add(fanout, Ordering::Relaxed);
    }

    /// 单个对齐对的过滤 + 判定（锁外）
    async fn decide_pair(
        &self,
        ctx: &DecideCtx<'_>,
        parent_rel: &str,
        pair: &AlignedPair,
        le: &[VfsDirEntry],
        re: &[VfsDirEntry],
    ) -> Option<DecidedChild> {
        let opts = ctx.opts;
        let name_filter = ctx.name_filter;
        let rel = if parent_rel.is_empty() { pair.name.clone() } else { format!("{parent_rel}/{}", pair.name) };
        let segments: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        let lkind = pair.left.map(|i| le[i as usize].stat.kind);
        let rkind = pair.right.map(|i| re[i as usize].stat.kind);
        let is_dir = lkind == Some(VfsEntryKind::Dir) || rkind == Some(VfsEntryKind::Dir);
        let target_kind = if is_dir { VfsEntryKind::Dir } else { VfsEntryKind::File };

        // 名称过滤（目录递归语义：排除目录剪枝 / include 父链自动包含）
        if is_dir {
            if name_filter.dir_pruned(&segments) {
                return None;
            }
        } else if !name_filter.file_allowed(&segments) {
            return None;
        }

        // 其他过滤器（仅文件；目录由子内容聚合）
        if !is_dir {
            if let Some(spec) = &opts.other_filter {
                let of = OtherFilter::from_spec(spec.clone());
                let ok = match pair.left {
                    Some(li) => of.allows(self.vfs.as_ref(), &self.roots.0.join(&rel), &le[li as usize].stat).await,
                    None => of
                        .allows(self.vfs.as_ref(), &self.roots.1.join(&rel), &re[pair.right.expect("对齐必有单侧") as usize].stat)
                        .await,
                };
                if !ok {
                    return None;
                }
            }
        }

        let left = pair.left.map(|i| proto_side(&le[i as usize].stat, &opts.criteria));
        let right = pair.right.map(|i| proto_side(&re[i as usize].stat, &opts.criteria));
        let status = match (pair.left, pair.right) {
            (Some(li), Some(ri)) => {
                if is_dir {
                    DirEntryStatus::DirSame
                } else {
                    self.file_status(
                        opts,
                        &SidePaths {
                            left: self.roots.0.join(&rel),
                            right: self.roots.1.join(&rel),
                        },
                        &le[li as usize].stat,
                        &re[ri as usize].stat,
                        &SideNames {
                            left: le[li as usize].name.clone(),
                            right: re[ri as usize].name.clone(),
                        },
                    )
                    .await
                }
            }
            (Some(_), None) => DirEntryStatus::LeftOnly,
            (None, Some(_)) => DirEntryStatus::RightOnly,
            (None, None) => unreachable!("对齐器输出必含单侧"),
        };
        Some(DecidedChild {
            name: pair.name.clone(),
            rel,
            target_kind,
            left,
            right,
            status,
        })
    }

    /// 文件判定（快速测试 -> 内容比较；CRC 走缓存 / zip 预计算）。
    /// lname/rname 为两侧真实文件名（大小写差异检测依赖）。
    async fn file_status(
        &self,
        opts: &DirSessionOptions,
        paths: &SidePaths,
        ls: &VfsStat,
        rs: &VfsStat,
        names: &SideNames,
    ) -> DirEntryStatus {
        let tester = QuickTest::new(&opts.criteria);
        match tester.verdict(ls, rs, &names.left, &names.right) {
            QuickOutcome::Same => DirEntryStatus::Same,
            QuickOutcome::Diff(reason) => match reason {
                // size 判异确定性成立；mtime 判异且内容禁用 = QuickDiff；
                // 属性/大小写/Unix 元数据差异为最终判定（内容比较不覆盖）
                QuickDiffReason::Mtime => DirEntryStatus::QuickDiff,
                _ => DirEntryStatus::ContentDiff,
            },
            QuickOutcome::NeedContent => {
                if opts.criteria.content_compare == ContentCompareMode::Never {
                    return DirEntryStatus::QuickDiff;
                }
                let same = crate::compare::content_same_stats(
                    self.vfs.as_ref(),
                    &paths.left,
                    &paths.right,
                    crate::compare::FileStatSummary::new(ls.size, ls.mtime_ms),
                    crate::compare::FileStatSummary::new(rs.size, rs.mtime_ms),
                    crate::compare::ContentHashMode::Crc,
                    opts.cache.as_deref(),
                )
                .await;
                match same {
                    Ok(true) => DirEntryStatus::Same,
                    Ok(false) => DirEntryStatus::ContentDiff,
                    Err(_) => DirEntryStatus::QuickDiff, // IO 失败不中断扫描，标记未证实
                }
            }
        }
    }

    /// 提交子项到 arena（持锁调用）：建节点、连父子、入队子目录、聚合上推
    fn commit_children(&self, st: &mut ModelState, _opts: &DirSessionOptions, dir_id: NodeId, level: u32, decided: Vec<DecidedChild>) {
        let target = self.target_depth().unwrap_or(u32::MAX);
        let fanout = decided.len() as u32;
        let gen = st.gen;
        for d in decided {
            let id = st.arena.len() as NodeId;
            let kind = if d.target_kind == VfsEntryKind::Dir { EntryKind::Dir } else { EntryKind::File };
            let has_both_sides = d.left.is_some() && d.right.is_some();
            let rel = d.rel.clone();
            st.arena.push(DirNode {
                rel_path: rel.clone(),
                name: d.name.clone(),
                kind,
                left: d.left,
                right: d.right,
                status: d.status,
                parent: Some(dir_id),
                children: Vec::new(),
                expanded: false,
                aligned_with: None,
            });
            st.index.insert(rel.clone(), id);
            st.arena[dir_id as usize].children.push(id);
            st.dirty.insert(id);
            stats_add(&mut st.stats, d.status);
            st.stats.total += 1;
            // 两侧目录且未超目标深度 -> 入队后台
            if d.target_kind == VfsEntryKind::Dir && has_both_sides {
                let child_level = level + 1;
                if child_level <= target {
                    st.bg.push_back(WorkItem {
                        dir_id: id,
                        lp: self.roots.0.join(&rel),
                        rp: self.roots.1.join(&rel),
                        level: child_level,
                        prio: Prio::Background,
                        gen,
                    });
                    st.pending += 1;
                }
            }
        }
        // 聚合上推（父链自底向上）
        let mut cur = st.arena[dir_id as usize].parent;
        while let Some(pid) = cur {
            let node = &st.arena[pid as usize];
            if node.is_paired_dir() && node.expanded {
                let new_status = aggregate_children(node, &st.arena);
                self.set_status(st, pid, new_status);
            }
            cur = st.arena[pid as usize].parent;
        }
        // 运行平均扇出（估算用）
        if fanout > 0 {
            let prev = self.avg_fanout.load(Ordering::Relaxed);
            self.avg_fanout.store((prev + fanout) / 2, Ordering::Relaxed);
        }
        st.flat_dirty = true;
    }

    fn mark_expanded(&self, st: &mut ModelState, id: NodeId) {
        st.arena[id as usize].expanded = true;
        st.in_flight.remove(&id);
    }

    /// 状态变更 + 统计增量调整 + 脏标记（持锁）
    fn set_status(&self, st: &mut ModelState, id: NodeId, new: DirEntryStatus) {
        let old = st.arena[id as usize].status;
        if old == new {
            return;
        }
        if id == 0 {
            // 根节点：仅更新状态（不进统计/不进 delta 行）
            st.arena[id as usize].status = new;
            st.flat_dirty = true;
            return;
        }
        stats_remove(&mut st.stats, old);
        st.arena[id as usize].status = new;
        stats_add(&mut st.stats, new);
        st.dirty.insert(id);
        st.flat_dirty = true;
    }

    fn recompute_est(&self, st: &mut ModelState) {
        let queued = st.fg.len() + st.bg.len() + st.in_flight.len();
        let avg = self.avg_fanout.load(Ordering::Relaxed).max(1);
        self.est_extra.store((queued as u32).saturating_mul(avg), Ordering::Relaxed);
    }

    // ------------------------------------------------------------------
    // 同步展开（open 可视层 / dir/expand 深层）
    // ------------------------------------------------------------------

    /// 同步展开 dir_id 之下 max_level 层（BFS 迭代；幂等跳过已展开/在途目录）
    async fn expand_sync(&self, dir_id: NodeId, level: u32, max_level: u32) {
        let mut queue: VecDeque<(NodeId, u32)> = VecDeque::new();
        queue.push_back((dir_id, level));
        while let Some((id, lv)) = queue.pop_front() {
            if lv >= max_level {
                continue;
            }
            {
                let mut st = self.state.lock().expect("模型锁");
                if st.arena[id as usize].expanded || st.in_flight.contains(&id) {
                    continue;
                }
                st.in_flight.insert(id);
            }
            self.process_item_sync(id, lv).await;
            // 子目录入队（深层）
            let children: Vec<NodeId> = {
                let st = self.state.lock().expect("模型锁");
                st.arena[id as usize]
                    .children
                    .iter()
                    .copied()
                    .filter(|&c| st.arena[c as usize].is_paired_dir() && !st.arena[c as usize].expanded)
                    .collect()
            };
            for c in children {
                queue.push_back((c, lv + 1));
            }
        }
    }

    /// 同步处理一个目录（复用 process_item；前台语义）
    async fn process_item_sync(&self, dir_id: NodeId, level: u32) {
        let (lp, rp, gen) = {
            let st = self.state.lock().expect("模型锁");
            let rel = st.arena[dir_id as usize].rel_path.clone();
            let gen = st.gen;
            (
                if dir_id == 0 { self.roots.0.clone() } else { self.roots.0.join(&rel) },
                if dir_id == 0 { self.roots.1.clone() } else { self.roots.1.join(&rel) },
                gen,
            )
        };
        let item = WorkItem { dir_id, lp, rp, level, prio: Prio::Foreground, gen };
        self.process_item(item).await;
        let mut st = self.state.lock().expect("模型锁");
        st.in_flight.remove(&dir_id);
    }

    /// dir/expand：展开指定目录 `depth` 层（前台优先），返回新增可见行数
    pub async fn expand(&self, path: &str, depth: u32) -> Result<u32, DirScanError> {
        let target_id = {
            let st = self.state.lock().expect("模型锁");
            st.index.get(path).copied().ok_or_else(|| {
                DirScanError::Vfs(ai_vfs::VfsError::NotFound { path: path.to_string() })
            })?
        };
        let kind = {
            let st = self.state.lock().expect("模型锁");
            st.arena[target_id as usize].kind
        };
        if kind != EntryKind::Dir {
            return Err(DirScanError::Vfs(ai_vfs::VfsError::NotADirectory { path: path.to_string() }));
        }
        let before = self.row_estimate();
        let level = if path.is_empty() { 0 } else { path.split('/').count() as u32 };
        // 前台处理本层 + 深层（expand_sync 幂等；已入后台的目录等待其完成）
        {
            let mut st = self.state.lock().expect("模型锁");
            if !st.arena[target_id as usize].expanded && !st.in_flight.contains(&target_id) {
                let gen = st.gen;
                st.in_flight.insert(target_id);
                let (lp, rp) = (self.roots.0.join(path), self.roots.1.join(path));
                st.fg.push_back(WorkItem { dir_id: target_id, lp, rp, level, prio: Prio::Foreground, gen });
                st.pending += 1;
                st.flat_dirty = true;
            }
        }
        self.notify.notify_waiters();
        // 等待本层展开（worker 或无 worker 时同步执行；超时 30s 防悬挂）
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let expanded = {
                let st = self.state.lock().expect("模型锁");
                st.arena[target_id as usize].expanded
            };
            if expanded {
                break;
            }
            if self.phase.load(Ordering::SeqCst) == PHASE_COMPLETED {
                // 无 worker 在跑（会话未 start_background）：同步执行
                self.process_item_sync(target_id, level).await;
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // depth > 1：递归展开深层
        if depth > 1 {
            self.expand_sync(target_id, level, level + depth - 1).await;
        }
        let after = self.row_estimate();
        let _ = self.rev.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_waiters();
        Ok(after.saturating_sub(before))
    }

    fn row_estimate(&self) -> u32 {
        let st = self.state.lock().expect("模型锁");
        (st.arena.len() as u32).saturating_sub(1)
    }

    // ------------------------------------------------------------------
    // delta / 进度
    // ------------------------------------------------------------------

    /// 取增量（脏行 + 移除 + 统计 + 进度）；无变化返回 None。
    /// 行序 = arena id 序（创建/BFS 序，确定性）。
    pub async fn take_delta(&self) -> Option<DirDeltaData> {
        let opts = self.opts.lock().expect("选项锁").clone();
        let flat = opts.flat;
        // 阶段 1（持锁）：收集脏行 id / 平铺待重判对
        #[derive(Debug)]
        enum Plan {
            None,
            Tree,
            Flat,
        }
        let plan = {
            let st = self.state.lock().expect("模型锁");
            if flat {
                if st.flat_dirty { Plan::Flat } else { Plan::None }
            } else if st.dirty.is_empty() && st.removals.is_empty() {
                Plan::None
            } else {
                Plan::Tree
            }
        };

        if matches!(plan, Plan::None) {
            return None;
        }
        // 阶段 2（锁外）：平铺跨路径配对的重判（内容比较 IO）
        let flat_cross: Vec<(String, String, String)> = if matches!(plan, Plan::Flat) {
            self.flat_cross_pairs().await
        } else {
            Vec::new()
        };
        // 阶段 3（持锁）：构建行
        let mut st = self.state.lock().expect("模型锁");
        let upserts = if matches!(plan, Plan::Flat) {
            self.flat_rows(&mut st, &opts, &flat_cross)
        } else {
            self.tree_rows(&mut st, &opts)
        };
        let removals = std::mem::take(&mut st.removals);
        st.dirty.clear();
        st.flat_dirty = false;
        let rev = self.rev.fetch_add(1, Ordering::SeqCst) + 1;
        let stats = st.stats;
        // progress() 内部需要 state 锁：先释放本锁再取（同线程重入会死锁）
        drop(st);
        let progress = self.progress();
        Some(DirDeltaData { rev, upserts, removals, stats, progress })
    }

    /// 平铺视图里需要跨路径重判的配对（rel 左, rel 右, 名键）
    async fn flat_cross_pairs(&self) -> Vec<(String, String, String)> {
        // 收集全部文件节点（按 key 分组）—— 与 flat_rows 相同的分组逻辑，
        // 此处只挑出"两侧 rel 不同"的配对供异步重判。
        let opts = self.opts.lock().expect("选项锁").clone();
        let name_filter = NameFilter::from_spec(opts.name_filter.as_ref().unwrap_or(&NameFilterSpec::default()));
        #[derive(Clone)]
        struct Occ {
            rel: String,
        }
        let mut groups: std::collections::BTreeMap<String, (Vec<Occ>, Vec<Occ>)> = Default::default();
        {
            let st = self.state.lock().expect("模型锁");
            for node in st.arena.iter().skip(1) {
                if node.kind != EntryKind::File {
                    continue;
                }
                let segments: Vec<&str> = node.rel_path.split('/').filter(|s| !s.is_empty()).collect();
                if !name_filter.file_allowed(&segments) {
                    continue;
                }
                let key = if opts.criteria.compare_case {
                    node.name.clone()
                } else {
                    crate::align::align_key(
                        &node.name,
                        crate::align::AlignKeyOptions { case_insensitive: true, ..Default::default() },
                    )
                };
                let g = groups.entry(key).or_default();
                if node.left.is_some() {
                    g.0.push(Occ { rel: node.rel_path.clone() });
                }
                if node.right.is_some() {
                    g.1.push(Occ { rel: node.rel_path.clone() });
                }
            }
        }
        let mut out = Vec::new();
        for (key, (ls, rs)) in &groups {
            let n = ls.len().max(rs.len());
            for i in 0..n {
                if let (Some(l), Some(r)) = (ls.get(i), rs.get(i)) {
                    if l.rel != r.rel {
                        out.push((l.rel.clone(), r.rel.clone(), key.clone()));
                    }
                }
            }
        }
        out
    }

    /// 树视图行（dirty 集；显示过滤器 + 名称过滤目录可见性 -> hidden/suppressed）
    fn tree_rows(&self, st: &mut ModelState, opts: &DirSessionOptions) -> Vec<DirRow> {
        let name_filter = NameFilter::from_spec(opts.name_filter.as_ref().unwrap_or(&NameFilterSpec::default()));
        let mut rows = Vec::with_capacity(st.dirty.len());
        for &id in &st.dirty {
            if id == 0 {
                continue;
            }
            let node = &st.arena[id as usize];
            let segments: Vec<&str> = node.rel_path.split('/').filter(|s| !s.is_empty()).collect();
            let mut display_hidden = !display_visible(node.status, node.left.clone(), node.right.clone(), &opts.display_filter);
            if node.kind == EntryKind::Dir && name_filter.has_includes() {
                // FR-DIR-034 默认语义：仅当目录包含可见文件（自身命中 include 或
                // 后代存在被包含项）时出现
                let visible = name_filter.dir_visible(&segments) || dir_has_included_descendant(&st.arena, id, &name_filter);
                if !visible {
                    display_hidden = true;
                }
            }
            rows.push(DirRow {
                id: node.rel_path.clone(),
                name: node.name.clone(),
                kind: node.kind,
                left: node.left.clone(),
                right: node.right.clone(),
                status: node.status,
                has_children: node.kind == EntryKind::Dir,
                hidden: display_hidden && !opts.suppress,
                suppressed: display_hidden && opts.suppress,
            });
        }
        rows
    }

    /// 平铺视图行（FR-DIR-021）：跨目录按名对齐；差分输出变更行。
    /// `flat_cross` 携带跨路径配对的重判状态。
    fn flat_rows(&self, st: &mut ModelState, opts: &DirSessionOptions, flat_cross: &[(String, String, String)]) -> Vec<DirRow> {
        let name_filter = NameFilter::from_spec(opts.name_filter.as_ref().unwrap_or(&NameFilterSpec::default()));
        // 跨路径配对的重判使用轻量判定：size+mtime 快速判定（与 file_status
        // 相同的快速部分；内容复核在平铺视图降级为 QuickDiff 未证实标记）。
        #[derive(Clone)]
        struct Occ {
            rel: String,
            left: Option<SideInfo>,
            right: Option<SideInfo>,
        }
        let mut groups: std::collections::BTreeMap<String, (Vec<Occ>, Vec<Occ>)> = Default::default();
        for node in st.arena.iter().skip(1) {
            if node.kind != EntryKind::File {
                continue;
            }
            let segments: Vec<&str> = node.rel_path.split('/').filter(|s| !s.is_empty()).collect();
            if !name_filter.file_allowed(&segments) {
                continue;
            }
            let key = if opts.criteria.compare_case {
                node.name.clone()
            } else {
                crate::align::align_key(
                    &node.name,
                    crate::align::AlignKeyOptions { case_insensitive: true, ..Default::default() },
                )
            };
            let g = groups.entry(key).or_default();
            if node.left.is_some() {
                g.0.push(Occ { rel: node.rel_path.clone(), left: node.left.clone(), right: node.right.clone() });
            }
            if node.right.is_some() {
                g.1.push(Occ { rel: node.rel_path.clone(), left: node.left.clone(), right: node.right.clone() });
            }
        }
        // 跨路径重判状态（rel 对 -> status）
        let mut cross_status: HashMap<(String, String), DirEntryStatus> = HashMap::new();
        for (lrel, rrel, key) in flat_cross {
            let tester = QuickTest::new(&opts.criteria);
            let l = st.index.get(lrel).and_then(|i| st.arena[*i as usize].left.clone());
            let r = st.index.get(rrel).and_then(|i| st.arena[*i as usize].right.clone());
            let status = match (l, r) {
                (Some(l), Some(r)) => {
                    let lstat = VfsStat::minimal(l.size, l.mtime_ms, VfsEntryKind::File);
                    let rstat = VfsStat::minimal(r.size, r.mtime_ms, VfsEntryKind::File);
                    match tester.verdict(&lstat, &rstat, key, key) {
                        QuickOutcome::Same => DirEntryStatus::Same,
                        QuickOutcome::Diff(_) => DirEntryStatus::ContentDiff,
                        // 跨路径内容复核在平铺视图降级为 QuickDiff（未证实）
                        QuickOutcome::NeedContent => DirEntryStatus::QuickDiff,
                    }
                }
                _ => DirEntryStatus::Unknown,
            };
            cross_status.insert((lrel.clone(), rrel.clone()), status);
        }

        let mut new_rows: Vec<DirRow> = Vec::new();
        for (key, (ls, rs)) in &groups {
            let n = ls.len().max(rs.len());
            let single = n == 1;
            for i in 0..n {
                let name = ls
                    .get(i)
                    .map(|o| o.rel.rsplit('/').next().unwrap_or(key).to_string())
                    .or_else(|| rs.get(i).map(|o| o.rel.rsplit('/').next().unwrap_or(key).to_string()))
                    .unwrap_or_else(|| key.clone());
                let id = if single { key.clone() } else { format!("{key}#{i}") };
                let (lside, rside) = (ls.get(i), rs.get(i));
                let status = match (lside, rside) {
                    (Some(l), Some(r)) => {
                        if l.rel == r.rel {
                            // 树内同名对：复用节点状态
                            st.index.get(&l.rel).map(|nid| st.arena[*nid as usize].status).unwrap_or(DirEntryStatus::Unknown)
                        } else {
                            cross_status
                                .get(&(l.rel.clone(), r.rel.clone()))
                                .copied()
                                .unwrap_or(DirEntryStatus::QuickDiff)
                        }
                    }
                    (Some(_), None) => DirEntryStatus::LeftOnly,
                    (None, Some(_)) => DirEntryStatus::RightOnly,
                    (None, None) => unreachable!(),
                };
                new_rows.push(DirRow {
                    id,
                    name,
                    kind: EntryKind::File,
                    left: lside.and_then(|o| o.left.clone()),
                    right: rside.and_then(|o| o.right.clone()),
                    status,
                    has_children: false,
                    hidden: false,
                    suppressed: false,
                });
            }
        }
        // 显示过滤器（平铺行同样遵守；FR-DIR-033）
        for row in &mut new_rows {
            let display_hidden = !display_visible(row.status, row.left.clone(), row.right.clone(), &opts.display_filter);
            row.hidden = display_hidden && !opts.suppress;
            row.suppressed = display_hidden && opts.suppress;
        }
        // 差分 + 统计重算
        let mut rows = Vec::new();
        let mut new_stats = DirStats::default();
        let mut new_ids: std::collections::BTreeSet<String> = Default::default();
        for row in new_rows {
            new_stats = stats_after_row(&row, new_stats);
            new_ids.insert(row.id.clone());
            let changed = match st.flat_cache.get(&row.id) {
                Some(old) => old != &row,
                None => true,
            };
            if changed {
                rows.push(row.clone());
            }
            st.flat_cache.insert(row.id.clone(), row);
        }
        // 移除项（旧 id 不在新集合）
        let stale: Vec<String> = st
            .flat_ids
            .iter()
            .filter(|id| !new_ids.contains(*id))
            .cloned()
            .collect();
        st.removals.extend(stale);
        st.flat_ids = new_ids;
        st.stats = new_stats;
        rows
    }

    /// 当前进度（scanned / total 估算；ADR-004 进度事件）
    pub fn progress(&self) -> ScanProgress {
        let scanned = self.scanned.load(Ordering::Relaxed);
        let arena_rows = self.row_estimate();
        let total = if self.phase.load(Ordering::SeqCst) == PHASE_COMPLETED {
            Some(arena_rows)
        } else {
            Some(arena_rows + self.est_extra.load(Ordering::Relaxed))
        };
        ScanProgress { scanned, total }
    }

    pub fn phase(&self) -> ProgressPhase {
        match self.phase.load(Ordering::SeqCst) {
            PHASE_COMPLETED => ProgressPhase::Completed,
            PHASE_CANCELLED => ProgressPhase::Cancelled,
            _ => ProgressPhase::Running,
        }
    }

    /// 等待后台扫描完成（测试辅助；超时返回当前 phase）
    pub async fn wait_until_done(&self, timeout: Duration) -> ProgressPhase {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let phase = self.phase();
            if phase != ProgressPhase::Running {
                return phase;
            }
            if tokio::time::Instant::now() >= deadline {
                return phase;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    // ------------------------------------------------------------------
    // 过滤器 / 显示 / 平铺 / 对齐
    // ------------------------------------------------------------------

    /// 更新名称/其他过滤器（FR-DIR-031/032）：触发重扫（旧行全部移除）
    pub async fn update_filters(
        &self,
        name_filter: Option<NameFilterSpec>,
        other_filter: Option<OtherFilterSpec>,
    ) -> Result<(), DirScanError> {
        {
            let mut opts = self.opts.lock().expect("选项锁");
            if let Some(nf) = name_filter {
                opts.name_filter = Some(nf);
            }
            if let Some(of) = other_filter {
                opts.other_filter = Some(of);
            }
        }
        self.rescan().await
    }

    /// 更新显示过滤器 / 抑制 / 平铺（只影响显示，不重扫：FR-DIR-033/035/021）
    pub fn set_display(&self, display_filter: Option<DisplayFilterSpec>, suppress: Option<bool>, flat: Option<bool>) {
        {
            let mut opts = self.opts.lock().expect("选项锁");
            if let Some(df) = display_filter {
                opts.display_filter = df;
            }
            if let Some(s) = suppress {
                opts.suppress = s;
            }
            if let Some(f) = flat {
                opts.flat = f;
            }
        }
        let mut st = self.state.lock().expect("模型锁");
        if flat.is_some() {
            st.flat_cache.clear();
            st.flat_ids.clear();
        }
        // 显示状态影响所有行的 hidden/suppressed 标记：全部重发
        for id in 1..st.arena.len() as NodeId {
            st.dirty.insert(id);
        }
        st.flat_dirty = true;
    }

    /// 重扫：代际 +1（后台丢弃在途项），arena 重建，旧行全部移除
    async fn rescan(&self) -> Result<(), DirScanError> {
        let (visible_depth, opts_new) = {
            let mut opts = self.opts.lock().expect("选项锁");
            let pairs = self.manual_pairs();
            opts.manual_pairs = pairs;
            (opts.visible_depth, opts.clone())
        };
        // 根 stat（锁外：tokio::fs 走 spawn_blocking，持 std 锁跨 await 会
        // 卡死并发 worker 的阻塞 lock —— 并发纪律红线）
        let ls = self.vfs.stat(&self.roots.0).await?;
        let rs = self.vfs.stat(&self.roots.1).await?;
        // 代际推进 + 重建状态
        {
            let mut st = self.state.lock().expect("模型锁");
            st.gen += 1;
            st.removals = st.arena.iter().skip(1).map(|n| n.rel_path.clone()).collect();
            st.arena = vec![DirNode {
                rel_path: String::new(),
                name: String::new(),
                kind: EntryKind::Dir,
                left: Some(proto_side(&ls, &opts_new.criteria)),
                right: Some(proto_side(&rs, &opts_new.criteria)),
                status: DirEntryStatus::DirSame,
                parent: None,
                children: Vec::new(),
                expanded: false,
                aligned_with: None,
            }];
            st.index = HashMap::from([(String::new(), 0)]);
            st.dirty = BTreeSet::from([0]);
            st.fg.clear();
            st.bg.clear();
            st.in_flight.clear();
            st.pending = 0;
            // 根节点不计入可见行统计
            st.stats = DirStats::default();
            st.errors.clear();
            st.flat_cache.clear();
            st.flat_ids.clear();
            st.flat_dirty = true;
            self.scanned.store(0, Ordering::Relaxed);
            self.est_extra.store(0, Ordering::Relaxed);
            self.avg_fanout.store(0, Ordering::Relaxed);
            self.phase.store(PHASE_RUNNING, Ordering::SeqCst);
        }
        // 可视层同步重扫（代际已变，旧 worker 的结果会被丢弃）
        self.expand_sync_raw(0, 0, visible_depth).await;
        let pairs = self.manual_pairs();
        for (l, r) in &pairs {
            let _ = self.align(l, r).await;
        }
        let done_now = opts_new.target_depth.is_some_and(|d| d <= visible_depth.max(1));
        if done_now {
            self.finish_locked();
        } else {
            self.enqueue_frontier_background();
        }
        let _ = self.rev.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_waiters();
        Ok(())
    }

    /// rescan 专用：跳过 in_flight 幂等检查的同步展开（调用时 in_flight 必空）
    async fn expand_sync_raw(&self, dir_id: NodeId, level: u32, max_level: u32) {
        self.expand_sync(dir_id, level, max_level).await;
    }

    /// 手动对齐（FR-DIR-022）：两侧 rel 路径配对（父须并排），重判状态
    pub async fn align(&self, left: &str, right: &str) -> Result<u32, DirScanError> {
        let (lid, rid) = {
            let st = self.state.lock().expect("模型锁");
            (st.index.get(left).copied(), st.index.get(right).copied())
        };
        let (lid, rid) = match (lid, rid) {
            (Some(l), Some(r)) => (l, r),
            _ => {
                return Err(DirScanError::Vfs(ai_vfs::VfsError::NotFound { path: format!("{left} | {right}") }));
            }
        };
        {
            let mut st = self.state.lock().expect("模型锁");
            // 父文件夹必须并排（FR-DIR-022）：同父且非根
            let lp = st.arena[lid as usize].parent;
            let rp = st.arena[rid as usize].parent;
            if lp.is_none() || lp != rp {
                return Err(DirScanError::Vfs(ai_vfs::VfsError::InvalidPath(
                    "手动对齐要求父文件夹并排（同父）".into(),
                )));
            }
            if st.arena[lid as usize].kind != EntryKind::File || st.arena[rid as usize].kind != EntryKind::File {
                return Err(DirScanError::Vfs(ai_vfs::VfsError::InvalidPath(
                    "P2 手动对齐仅支持文件项（目录保持树形对齐）".into(),
                )));
            }
            // 记录配对（幂等）
            let pair = (left.to_string(), right.to_string());
            if !st.manual_pairs.contains(&pair) {
                st.manual_pairs.push(pair);
            }
            st.arena[lid as usize].aligned_with = Some(rid);
            st.arena[rid as usize].aligned_with = Some(lid);
        }
        let changed = self.rejudge_pair(lid, rid, true).await;
        let _ = self.rev.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_waiters();
        Ok(changed)
    }

    /// 解除对齐：涉及 path 的全部配对（恢复孤儿/同名配对状态）
    pub async fn unalign(&self, path: &str) -> Result<u32, DirScanError> {
        let id = {
            let st = self.state.lock().expect("模型锁");
            st.index.get(path).copied()
        };
        let Some(id) = id else { return Ok(0) };
        let other = {
            let mut st = self.state.lock().expect("模型锁");
            st.manual_pairs.retain(|(l, r)| l != path && r != path);
            st.arena[id as usize].aligned_with.take()
        };
        let mut changed = 0u32;
        if let Some(other) = other {
            {
                let mut st = self.state.lock().expect("模型锁");
                st.arena[other as usize].aligned_with = None;
            }
            changed += self.rejudge_pair(id, other, false).await;
        } else {
            changed += self.rejudge_solo(id).await as u32;
        }
        let _ = self.rev.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_waiters();
        Ok(changed)
    }

    /// 重判一对：aligned=true（对齐生效）合并两侧元数据判定；
    /// false（解除对齐）各侧独立按孤儿/同名恢复判定
    async fn rejudge_pair(&self, a: NodeId, b: NodeId, aligned: bool) -> u32 {
        let opts = self.opts.lock().expect("选项锁").clone();
        let (arel, brel, aleft, aright, bleft, bright, aname, bname) = {
            let st = self.state.lock().expect("模型锁");
            (
                st.arena[a as usize].rel_path.clone(),
                st.arena[b as usize].rel_path.clone(),
                st.arena[a as usize].left.clone(),
                st.arena[a as usize].right.clone(),
                st.arena[b as usize].left.clone(),
                st.arena[b as usize].right.clone(),
                st.arena[a as usize].name.clone(),
                st.arena[b as usize].name.clone(),
            )
        };
        let status = if aligned {
            // 合并两侧元数据（以 a 的 left 有值优先确定左右）
            let (lrel, rrel, ls, rs, lname, rname) = if aleft.is_some() {
                (arel.clone(), brel.clone(), aleft, bright, aname.clone(), bname.clone())
            } else {
                (brel.clone(), arel.clone(), bleft, aright, bname.clone(), aname.clone())
            };
            match (ls, rs) {
                (Some(l), Some(r)) => {
                    let lstat = VfsStat::minimal(l.size, l.mtime_ms, VfsEntryKind::File);
                    let rstat = VfsStat::minimal(r.size, r.mtime_ms, VfsEntryKind::File);
                    self.file_status(
                        &opts,
                        &SidePaths { left: self.roots.0.join(&lrel), right: self.roots.1.join(&rrel) },
                        &lstat,
                        &rstat,
                        &SideNames { left: lname, right: rname },
                    )
                    .await
                }
                (Some(_), None) => DirEntryStatus::LeftOnly,
                (None, Some(_)) => DirEntryStatus::RightOnly,
                (None, None) => DirEntryStatus::Unknown,
            }
        } else {
            // 解除对齐：各自按持有侧判定（同名恢复配对由按名对齐的原始状态承担）
            let a_status = match (aleft, aright) {
                (Some(_), Some(_)) => DirEntryStatus::Same,
                (Some(_), None) => DirEntryStatus::LeftOnly,
                (None, Some(_)) => DirEntryStatus::RightOnly,
                (None, None) => DirEntryStatus::Unknown,
            };
            let b_status = match (bleft, bright) {
                (Some(_), Some(_)) => DirEntryStatus::Same,
                (Some(_), None) => DirEntryStatus::LeftOnly,
                (None, Some(_)) => DirEntryStatus::RightOnly,
                (None, None) => DirEntryStatus::Unknown,
            };
            {
                let mut st = self.state.lock().expect("模型锁");
                self.set_status(&mut st, a, a_status);
                self.set_status(&mut st, b, b_status);
            }
            self.aggregate_after(a);
            self.aggregate_after(b);
            return 2;
        };
        {
            let mut st = self.state.lock().expect("模型锁");
            self.set_status(&mut st, a, status);
            self.set_status(&mut st, b, status);
        }
        self.aggregate_after(a);
        self.aggregate_after(b);
        2
    }

    /// 单节点重判（解除对齐后另一侧已重判时）
    async fn rejudge_solo(&self, id: NodeId) -> bool {
        let (left, right) = {
            let st = self.state.lock().expect("模型锁");
            (st.arena[id as usize].left.clone(), st.arena[id as usize].right.clone())
        };
        let status = match (left, right) {
            (Some(_), Some(_)) => DirEntryStatus::Same,
            (Some(_), None) => DirEntryStatus::LeftOnly,
            (None, Some(_)) => DirEntryStatus::RightOnly,
            (None, None) => DirEntryStatus::Unknown,
        };
        {
            let mut st = self.state.lock().expect("模型锁");
            self.set_status(&mut st, id, status);
        }
        self.aggregate_after(id);
        true
    }

    /// 节点状态变化后聚合其祖先链
    fn aggregate_after(&self, id: NodeId) {
        let mut st = self.state.lock().expect("模型锁");
        let mut cur = st.arena[id as usize].parent;
        while let Some(pid) = cur {
            let node = &st.arena[pid as usize];
            if node.is_paired_dir() && node.expanded {
                let new_status = aggregate_children(node, &st.arena);
                self.set_status(&mut st, pid, new_status);
            }
            cur = st.arena[pid as usize].parent;
        }
    }

    // ------------------------------------------------------------------
    // 快照（operate 计划数据源）
    // ------------------------------------------------------------------

    /// 当前全部可见行（含 hidden/suppressed 标记；operate 计划数据源）。
    /// 平铺模式下返回平铺全量行。
    pub async fn rows_snapshot(&self) -> Vec<DirRow> {
        let opts = self.opts.lock().expect("选项锁").clone();
        if !opts.flat {
            let st = self.state.lock().expect("模型锁");
            let name_filter = NameFilter::from_spec(opts.name_filter.as_ref().unwrap_or(&NameFilterSpec::default()));
            let mut rows = Vec::with_capacity(st.arena.len());
            for (idx, node) in st.arena.iter().enumerate().skip(1) {
                let id = idx as NodeId;
                let segments: Vec<&str> = node.rel_path.split('/').filter(|s| !s.is_empty()).collect();
                let mut display_hidden = !display_visible(node.status, node.left.clone(), node.right.clone(), &opts.display_filter);
                if node.kind == EntryKind::Dir && name_filter.has_includes() {
                    let visible = name_filter.dir_visible(&segments) || dir_has_included_descendant(&st.arena, id, &name_filter);
                    if !visible {
                        display_hidden = true;
                    }
                }
                rows.push(DirRow {
                    id: node.rel_path.clone(),
                    name: node.name.clone(),
                    kind: node.kind,
                    left: node.left.clone(),
                    right: node.right.clone(),
                    status: node.status,
                    has_children: node.kind == EntryKind::Dir,
                    hidden: display_hidden && !opts.suppress,
                    suppressed: display_hidden && opts.suppress,
                });
            }
            return rows;
        }
        // 平铺：触发一次全量重算并回收全部行
        {
            let mut st = self.state.lock().expect("模型锁");
            st.flat_cache.clear();
            st.flat_ids.clear();
            st.flat_dirty = true;
        }
        match self.take_delta().await {
            Some(d) => d.upserts,
            None => Vec::new(),
        }
    }

    /// 统计快照（handler 应答用）
    pub fn stats_snapshot(&self) -> DirStats {
        self.state.lock().expect("模型锁").stats
    }

    /// 当前 rev 快照
    pub fn rev_snapshot(&self) -> u64 {
        self.rev.load(Ordering::SeqCst)
    }

    /// 会话选项快照（判据等；供 sync 计划器使用）
    pub fn options(&self) -> DirSessionOptions {
        self.opts.lock().expect("选项锁").clone()
    }

    /// 扫描错误（目录不可读等；rel -> 消息）
    pub fn scan_errors(&self) -> Vec<(String, String)> {
        self.state.lock().expect("模型锁").errors.clone()
    }

    /// 取消（全链路：后台 worker / expand / 内容比较在 await 点间检查）
    pub fn cancel(&self) {
        self.cancel.cancel();
        self.notify.notify_waiters();
    }

    pub fn cancel_token(&self) -> crate::CancelFlag {
        self.cancel.clone()
    }

    /// 订阅通知（引擎 pump 等待增量）
    pub async fn notified(&self) {
        self.notify.notified().await;
    }

    fn target_depth(&self) -> Option<u32> {
        self.opts.lock().expect("选项锁").target_depth
    }
}

/// 目录子树中是否存在被名称过滤器包含的文件（FR-DIR-034 默认可见性判定）
fn dir_has_included_descendant(arena: &[DirNode], dir_id: NodeId, filter: &NameFilter) -> bool {
    let node = &arena[dir_id as usize];
    for &c in &node.children {
        let child = &arena[c as usize];
        let segments: Vec<&str> = child.rel_path.split('/').filter(|s| !s.is_empty()).collect();
        match child.kind {
            EntryKind::File => {
                if filter.file_allowed(&segments) {
                    return true;
                }
            }
            EntryKind::Dir => {
                if filter.dir_visible(&segments) || dir_has_included_descendant(arena, c, filter) {
                    return true;
                }
            }
            EntryKind::Symlink => {}
        }
    }
    false
}

/// worker 存活计数 RAII（退出时递减）
struct WorkerGuard(Arc<DirModel>);

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.live_workers.fetch_sub(1, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }
}

/// 判定上下文（参数打包：避免超长签名）
struct DecideCtx<'a> {
    opts: &'a DirSessionOptions,
    name_filter: &'a NameFilter,
}

/// 两侧 VFS 路径对（file_status 判定参数打包）
struct SidePaths {
    left: VfsPath,
    right: VfsPath,
}

/// 两侧文件名对
struct SideNames {
    left: String,
    right: String,
}

/// 提交前的中间判定结果
struct DecidedChild {
    name: String,
    rel: String,
    target_kind: VfsEntryKind,
    left: Option<SideInfo>,
    right: Option<SideInfo>,
    status: DirEntryStatus,
}

/// 协议 SideInfo（按判据门控扩展字段——旧载荷字节级兼容）
pub(crate) fn proto_side(stat: &VfsStat, criteria: &CompareCriteria) -> SideInfo {
    let mut si = SideInfo::base(stat.size, stat.mtime_ms);
    if criteria.compare_attributes {
        si.attribute = Some(dos_attr::to_letters(stat.dos_attrs));
    }
    if criteria.unix_meta {
        si.unix_mode = stat.unix_mode;
        si.uid = stat.uid;
        si.gid = stat.gid;
    }
    si
}

/// 显示过滤器可见性（FR-DIR-033：状态 -> 类别；QuickDiff 按 mtime 分左右较新）
pub fn display_visible(status: DirEntryStatus, left: Option<SideInfo>, right: Option<SideInfo>, df: &DisplayFilterSpec) -> bool {
    match status {
        DirEntryStatus::Same | DirEntryStatus::DirSame => df.show_same,
        DirEntryStatus::ContentDiff => df.show_content_diff,
        DirEntryStatus::DirDiff => df.show_dir_diff,
        DirEntryStatus::QuickDiff => {
            let l = left.map(|s| s.mtime_ms).unwrap_or(0);
            let r = right.map(|s| s.mtime_ms).unwrap_or(0);
            if l >= r {
                df.show_left_newer
            } else {
                df.show_right_newer
            }
        }
        DirEntryStatus::LeftOnly => df.show_left_only,
        DirEntryStatus::RightOnly => df.show_right_only,
        DirEntryStatus::Unknown => true,
    }
}

pub(crate) fn stats_after_row(row: &DirRow, mut stats: DirStats) -> DirStats {
    stats_add(&mut stats, row.status);
    stats.total += 1;
    stats
}

pub(crate) fn stats_add(stats: &mut DirStats, status: DirEntryStatus) {
    match status {
        DirEntryStatus::Same => stats.same += 1,
        DirEntryStatus::QuickDiff => stats.quick_diff += 1,
        DirEntryStatus::ContentDiff => stats.content_diff += 1,
        DirEntryStatus::LeftOnly => stats.left_only += 1,
        DirEntryStatus::RightOnly => stats.right_only += 1,
        DirEntryStatus::DirSame => stats.dir_same += 1,
        DirEntryStatus::DirDiff => stats.dir_diff += 1,
        DirEntryStatus::Unknown => {}
    }
}

pub(crate) fn stats_remove(stats: &mut DirStats, status: DirEntryStatus) {
    match status {
        DirEntryStatus::Same => stats.same = stats.same.saturating_sub(1),
        DirEntryStatus::QuickDiff => stats.quick_diff = stats.quick_diff.saturating_sub(1),
        DirEntryStatus::ContentDiff => stats.content_diff = stats.content_diff.saturating_sub(1),
        DirEntryStatus::LeftOnly => stats.left_only = stats.left_only.saturating_sub(1),
        DirEntryStatus::RightOnly => stats.right_only = stats.right_only.saturating_sub(1),
        DirEntryStatus::DirSame => stats.dir_same = stats.dir_same.saturating_sub(1),
        DirEntryStatus::DirDiff => stats.dir_diff = stats.dir_diff.saturating_sub(1),
        DirEntryStatus::Unknown => {}
    }
}

#[cfg(test)]
mod tests;
