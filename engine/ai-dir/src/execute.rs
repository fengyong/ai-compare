//! 文件操作执行层（P6，FR-DIR-060~065 / OP-001/002）。
//!
//! 消费 P2 的 [`PlannedOperation`] 计划（[`crate::sync::plan_operations`] 产物）
//! 与 UI 右键/批量操作构造的 [`FileOp`]，经 VFS 写路径（ai-vfs P6）落盘。
//!
//! 语义要点：
//! - **部分成功**（OP-002）：逐项执行、失败收集，单项失败不中断批次；
//!   报告成功/失败/跳过三清单；
//! - **执行前校验**：源存在性、目标类型冲突（文件↔目录）、rel 路径安全护栏
//!   （拒绝空路径/根路径/`..` 逃逸；目录树删除不跟随符号链接）；
//! - **dry_run**：只做校验不落盘，返回"将执行清单"（可对拍）；
//! - **进度**：逐项回调 `progress(done, total, op)`；**取消**：批次间检查
//!   [`CancelFlag`]，取消后剩余项记 Skipped；
//! - 复制保留源 mtime（BC 复制语义）；move = copy + 删源；
//!   rename 优先走 VFS 原子 rename（能力位 [`ai_vfs::VfsCaps::MOVE`]）。

use ai_protocol::{PlannedOperation, PlannedOpKind};
use ai_vfs::{EntryKind, StatInfo, VfsCaps, VfsPath, VfsProvider};

use crate::CancelFlag;

/// 操作侧
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Left => write!(f, "左"),
            Side::Right => write!(f, "右"),
        }
    }
}

/// 执行级操作（PlannedOperation 展开后的统一形态；右键/批量操作直接构造）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    /// 跨侧复制（FR-DIR-060）
    Copy { path: String, from: Side, to: Side },
    /// 跨侧移动 = 复制 + 删源（FR-DIR-062）
    Move { path: String, from: Side, to: Side },
    /// 同侧改名（FR-DIR-064；目标 = 源父目录 + new_name）
    Rename { path: String, side: Side, new_name: String },
    /// 删除（目录为递归删除；FR-DIR-063）
    Delete { path: String, side: Side },
    /// 时间戳：mtime_ms > 0 设为指定值；0 = 从对侧复制（FR-DIR-065 touch）
    Touch { path: String, side: Side, mtime_ms: u64 },
    /// 新建文件夹（FR-DIR-066）
    Mkdir { path: String, side: Side },
}

impl FileOp {
    /// P2 计划操作 -> 执行级操作（TouchLeft/Right = 从对侧复制时间戳）
    pub fn from_planned(p: &PlannedOperation) -> Self {
        match p.op {
            PlannedOpKind::CopyLeftToRight => FileOp::Copy { path: p.path.clone(), from: Side::Left, to: Side::Right },
            PlannedOpKind::CopyRightToLeft => FileOp::Copy { path: p.path.clone(), from: Side::Right, to: Side::Left },
            PlannedOpKind::DeleteLeft => FileOp::Delete { path: p.path.clone(), side: Side::Left },
            PlannedOpKind::DeleteRight => FileOp::Delete { path: p.path.clone(), side: Side::Right },
            PlannedOpKind::TouchLeft => FileOp::Touch { path: p.path.clone(), side: Side::Left, mtime_ms: 0 },
            PlannedOpKind::TouchRight => FileOp::Touch { path: p.path.clone(), side: Side::Right, mtime_ms: 0 },
        }
    }

    /// 人类可读描述（dry_run 清单 / 确认框明细行）
    pub fn describe(&self) -> String {
        match self {
            FileOp::Copy { path, from, to } => format!("复制 {from}→{to}: {path}"),
            FileOp::Move { path, from, to } => format!("移动 {from}→{to}: {path}"),
            FileOp::Rename { path, side, new_name } => format!("改名（{side}）: {path} → {new_name}"),
            FileOp::Delete { path, side } => format!("删除（{side}）: {path}"),
            FileOp::Touch { path, side, mtime_ms } => {
                if *mtime_ms == 0 {
                    format!("对时（{side}，取对侧时间戳）: {path}")
                } else {
                    format!("对时（{side}，mtime={mtime_ms}）: {path}")
                }
            }
            FileOp::Mkdir { path, side } => format!("新建文件夹（{side}）: {path}"),
        }
    }

    /// rel 路径（校验/展示用）
    pub fn path(&self) -> &str {
        match self {
            FileOp::Copy { path, .. } | FileOp::Move { path, .. } | FileOp::Rename { path, .. } => path,
            FileOp::Delete { path, .. } | FileOp::Touch { path, .. } | FileOp::Mkdir { path, .. } => path,
        }
    }
}

/// 执行选项
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExecuteOptions {
    /// 试运行：只校验不落盘（返回将执行清单，可对拍）
    pub dry_run: bool,
}

/// 单项结果状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpStatus {
    Succeeded,
    Failed,
    /// 未执行（不存在/已取消/护栏拒绝且可预期）
    Skipped,
}

/// 单项结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpResult {
    pub op: FileOp,
    pub status: OpStatus,
    /// 失败原因 / 跳过原因 / dry_run 说明
    pub detail: Option<String>,
}

/// 批次报告（部分成功语义：成功/失败/跳过三清单）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecuteReport {
    pub total: usize,
    pub succeeded: Vec<OpResult>,
    pub failed: Vec<OpResult>,
    pub skipped: Vec<OpResult>,
    pub dry_run: bool,
    /// 取消后中止（剩余项进 skipped）
    pub cancelled: bool,
}

impl ExecuteReport {
    pub fn is_all_ok(&self) -> bool {
        self.failed.is_empty()
    }

    /// dry_run 语义别名：将执行清单（校验通过项）
    pub fn will_execute(&self) -> &[OpResult] {
        &self.succeeded
    }
}

/// rel 路径安全护栏（FR-DIR-063 目录树删除防线）
///
/// 拒绝：空路径、绝对路径、含空/`.`/`..` 段、反斜杠分隔符。
/// 通过护栏后 `root.join(rel)` 不可能逃出根（VfsPath::join 词法拼接 + 无 `..`）。
pub fn sanitize_rel(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("拒绝空路径（根目录不可作为操作目标）".into());
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(format!("拒绝绝对路径: {path}"));
    }
    if path.contains('\\') {
        return Err(format!("拒绝反斜杠分隔符: {path}"));
    }
    for seg in path.split('/') {
        match seg {
            "" => return Err(format!("拒绝空路径段: {path}")),
            "." | ".." => return Err(format!("拒绝相对引用段（{seg}）: {path}")),
            _ => {}
        }
    }
    Ok(())
}

/// 执行进度回调：done 为已处理数（含当前项），total 为批次大小
pub type ProgressFn<'a> = dyn FnMut(usize, usize, &FileOp) + Send + 'a;

/// 消费 P2 计划执行（dir/operate -> 执行链路主入口）
pub async fn execute_planned_operations(
    vfs: &dyn VfsProvider,
    roots: &(VfsPath, VfsPath),
    plan: &[PlannedOperation],
    opts: &ExecuteOptions,
    progress: Option<&mut ProgressFn<'_>>,
    cancel: &CancelFlag,
) -> ExecuteReport {
    let ops: Vec<FileOp> = plan.iter().map(FileOp::from_planned).collect();
    execute_ops(vfs, roots, &ops, opts, progress, cancel).await
}

/// 执行操作批次（右键/批量操作入口）
pub async fn execute_ops(
    vfs: &dyn VfsProvider,
    roots: &(VfsPath, VfsPath),
    ops: &[FileOp],
    opts: &ExecuteOptions,
    mut progress: Option<&mut ProgressFn<'_>>,
    cancel: &CancelFlag,
) -> ExecuteReport {
    let total = ops.len();
    let mut report = ExecuteReport { total, dry_run: opts.dry_run, ..Default::default() };
    for (i, op) in ops.iter().enumerate() {
        if let Some(cb) = progress.as_deref_mut() {
            cb(i + 1, total, op);
        }
        if cancel.is_cancelled() {
            report.cancelled = true;
            report.skipped.push(OpResult {
                op: op.clone(),
                status: OpStatus::Skipped,
                detail: Some("已取消".into()),
            });
            continue;
        }
        let result = execute_one(vfs, roots, op, opts).await;
        match result.status {
            OpStatus::Succeeded => report.succeeded.push(result),
            OpStatus::Failed => report.failed.push(result),
            OpStatus::Skipped => report.skipped.push(result),
        }
    }
    report
}

/// 执行带独立根对的操作批次（P8 文件夹合并输出侧复用：
/// 每个 op 携带自己的 (源根, 目标根) 对，进度/取消语义与 [`execute_ops`] 一致）。
pub async fn execute_ops_with_roots(
    vfs: &dyn VfsProvider,
    items: &[(FileOp, VfsPath, VfsPath)],
    opts: &ExecuteOptions,
    mut progress: Option<&mut ProgressFn<'_>>,
    cancel: &CancelFlag,
) -> ExecuteReport {
    let total = items.len();
    let mut report = ExecuteReport { total, dry_run: opts.dry_run, ..Default::default() };
    for (i, (op, src_root, dst_root)) in items.iter().enumerate() {
        if let Some(cb) = progress.as_deref_mut() {
            cb(i + 1, total, op);
        }
        if cancel.is_cancelled() {
            report.cancelled = true;
            report.skipped.push(OpResult {
                op: op.clone(),
                status: OpStatus::Skipped,
                detail: Some("已取消".into()),
            });
            continue;
        }
        let result = execute_one(vfs, &(src_root.clone(), dst_root.clone()), op, opts).await;
        match result.status {
            OpStatus::Succeeded => report.succeeded.push(result),
            OpStatus::Failed => report.failed.push(result),
            OpStatus::Skipped => report.skipped.push(result),
        }
    }
    report
}

/// 单项执行（校验 + 落盘；dry_run 只校验）
async fn execute_one(vfs: &dyn VfsProvider, roots: &(VfsPath, VfsPath), op: &FileOp, opts: &ExecuteOptions) -> OpResult {
    let mk = |status: OpStatus, detail: Option<String>| OpResult { op: op.clone(), status, detail };
    let detail_of = |r: Result<OpStatus, String>| match r {
        Ok(s) => mk(s, None),
        Err(msg) => mk(OpStatus::Failed, Some(msg)),
    };

    // 1. 安全护栏（全部操作一致：路径不可逃逸）
    if let Err(msg) = sanitize_rel(op.path()) {
        return mk(OpStatus::Failed, Some(msg));
    }
    // 符号链接逃逸护栏必须在此公共路径上：validate 只在 dry_run 与 copy_path
    // 内被调用，真实落盘的 delete/rename/touch/mkdir 根本不经过它。
    if let Err(msg) = reject_symlink_escape(vfs, roots, op).await {
        return mk(OpStatus::Failed, Some(msg));
    }

    // 2. dry_run：只做存在性/冲突校验，产出将执行清单
    if opts.dry_run {
        return match validate(vfs, roots, op).await {
            Ok(Some(note)) => mk(OpStatus::Succeeded, Some(note)),
            Ok(None) => mk(OpStatus::Succeeded, Some(format!("DRY: {}", op.describe()))),
            Err(msg) => mk(OpStatus::Failed, Some(msg)),
        };
    }

    // 3. 落盘
    let ok = |r: Result<(), String>| match r {
        Ok(()) => mk(OpStatus::Succeeded, None),
        Err(msg) => mk(OpStatus::Failed, Some(msg)),
    };
    match op {
        FileOp::Copy { path, from, to } => ok(copy_path(vfs, roots, path, *from, *to).await),
        FileOp::Move { path, from, to } => {
            let r = copy_path(vfs, roots, path, *from, *to).await;
            if let Err(msg) = r {
                return mk(OpStatus::Failed, Some(msg));
            }
            detail_of(remove_side(vfs, roots, path, *from, false).await)
        }
        FileOp::Rename { path, side, new_name } => ok(rename_side(vfs, roots, path, *side, new_name).await),
        FileOp::Delete { path, side } => detail_of(remove_side(vfs, roots, path, *side, true).await),
        FileOp::Touch { path, side, mtime_ms } => ok(touch_side(vfs, roots, path, *side, *mtime_ms).await),
        FileOp::Mkdir { path, side } => ok(mkdir_side(vfs, roots, path, *side).await),
    }
}

/// 根路径（side 对应侧）
fn side_root(roots: &(VfsPath, VfsPath), side: Side) -> &VfsPath {
    match side {
        Side::Left => &roots.0,
        Side::Right => &roots.1,
    }
}

/// rel -> 侧内绝对 VfsPath（护栏已保证不逃逸）
fn resolve(roots: &(VfsPath, VfsPath), side: Side, rel: &str) -> VfsPath {
    side_root(roots, side).join(rel)
}

/// rel 父目录（无父 = 根）
fn parent_rel(rel: &str) -> String {
    match rel.rsplit_once('/') {
        Some((parent, _)) => parent.to_string(),
        None => String::new(),
    }
}

/// 符号链接逃逸护栏（FR-DIR-063）。
///
/// `sanitize_rel` 只做词法校验，其「通过护栏后不可能逃出根」的断言仅对词法路径
/// 成立：符号链接分量会在文件系统层解析到比较根之外，使 copy 覆写、delete 销毁
/// 根外数据。`vfs.stat` 为 lstat 语义，逐分量检查即可拦住。
///
/// 只检查**祖先**分量：删除符号链接本身是安全的 unlink（不跟随），必须继续放行。
async fn reject_symlink_ancestors(vfs: &dyn VfsProvider, root: &VfsPath, rel: &str) -> Result<(), String> {
    let mut segs = rel.split('/').peekable();
    let mut cur = root.clone();
    while let Some(seg) = segs.next() {
        cur = cur.join(seg);
        if segs.peek().is_none() {
            break; // 末分量交由各操作自行判定
        }
        if let Ok(st) = vfs.stat(&cur).await {
            if st.kind == EntryKind::Symlink {
                return Err(format!("拒绝符号链接目录分量（{rel}）：会越过比较根"));
            }
        }
    }
    Ok(())
}

/// 写入型操作的目标末分量若为符号链接，写入会跟随它落到根外，必须拒绝。
async fn reject_symlink_target(vfs: &dyn VfsProvider, root: &VfsPath, rel: &str) -> Result<(), String> {
    if let Ok(st) = vfs.stat(&root.join(rel)).await {
        if st.kind == EntryKind::Symlink {
            return Err(format!("拒绝写入符号链接目标（{rel}）：会越过比较根"));
        }
    }
    Ok(())
}

/// 对一项操作涉及的全部侧做祖先分量护栏；写入型操作再加末分量护栏。
async fn reject_symlink_escape(vfs: &dyn VfsProvider, roots: &(VfsPath, VfsPath), op: &FileOp) -> Result<(), String> {
    let targets: Vec<(Side, &str)> = match op {
        FileOp::Copy { path, from, to } | FileOp::Move { path, from, to } => {
            vec![(*from, path.as_str()), (*to, path.as_str())]
        }
        FileOp::Rename { path, side, .. }
        | FileOp::Delete { path, side }
        | FileOp::Touch { path, side, .. }
        | FileOp::Mkdir { path, side } => vec![(*side, path.as_str())],
    };
    for (side, rel) in &targets {
        reject_symlink_ancestors(vfs, side_root(roots, *side), rel).await?;
    }
    // 写入会跟随末分量符号链接落到根外；删除符号链接本身是安全 unlink，放行。
    match op {
        FileOp::Copy { path, to, .. } | FileOp::Move { path, to, .. } => {
            reject_symlink_target(vfs, side_root(roots, *to), path).await
        }
        FileOp::Touch { path, side, .. } => {
            reject_symlink_target(vfs, side_root(roots, *side), path).await
        }
        _ => Ok(()),
    }
}

/// 执行前校验（dry_run / 复制冲突检查共用）
///
/// 返回 Ok(Some(note))：附注（如"将覆盖目标"）；Ok(None)：无附注。
async fn validate(vfs: &dyn VfsProvider, roots: &(VfsPath, VfsPath), op: &FileOp) -> Result<Option<String>, String> {
    match op {
        FileOp::Copy { path, from, to } | FileOp::Move { path, from, to } => {
            let src = resolve(roots, *from, path);
            let src_stat = vfs
                .stat(&src)
                .await
                .map_err(|e| format!("源不存在或不可读（{from}）: {path}: {e}"))?;
            let dst = resolve(roots, *to, path);
            match vfs.stat(&dst).await {
                Ok(dst_stat) => {
                    type_conflict(&src_stat, &dst_stat, path)?;
                    Ok(Some(format!("将覆盖目标（{to}）: {path}")))
                }
                Err(_) => Ok(None),
            }
        }
        FileOp::Rename { path, side, new_name } => {
            let src = resolve(roots, *side, path);
            vfs.stat(&src).await.map_err(|e| format!("源不存在（{side}）: {path}: {e}"))?;
            if sanitize_rel(new_name).is_err() {
                return Err(format!("新名称非法: {new_name}"));
            }
            let parent = parent_rel(path);
            let dst = side_root(roots, *side).join(&parent).join(new_name);
            if dst.as_str() != resolve(roots, *side, path).as_str() && vfs.stat(&dst).await.is_ok() {
                return Err(format!("目标名已存在（{side}）: {parent}/{new_name}"));
            }
            Ok(None)
        }
        FileOp::Delete { path, side } => {
            let target = resolve(roots, *side, path);
            match vfs.stat(&target).await {
                Ok(_) => Ok(None),
                Err(_) => Err(format!("目标不存在（{side}）: {path}")),
            }
        }
        FileOp::Touch { path, side, mtime_ms } => {
            let target = resolve(roots, *side, path);
            vfs.stat(&target).await.map_err(|e| format!("目标不存在（{side}）: {path}: {e}"))?;
            if *mtime_ms == 0 {
                let other = match side {
                    Side::Left => Side::Right,
                    Side::Right => Side::Left,
                };
                let src = resolve(roots, other, path);
                vfs.stat(&src).await.map_err(|e| format!("对侧不存在，无法取时间戳: {path}: {e}"))?;
            }
            Ok(None)
        }
        FileOp::Mkdir { path, side } => {
            let target = resolve(roots, *side, path);
            if vfs.stat(&target).await.is_ok() {
                return Err(format!("目标已存在（{side}）: {path}"));
            }
            Ok(None)
        }
    }
}

/// 文件↔目录类型冲突
fn type_conflict(src: &StatInfo, dst: &StatInfo, path: &str) -> Result<(), String> {
    let src_dir = src.kind == EntryKind::Dir;
    let dst_dir = dst.kind == EntryKind::Dir;
    if src_dir != dst_dir {
        return Err(format!("目标类型冲突（文件/目录）：{path}"));
    }
    Ok(())
}

/// 跨侧复制（文件 or 目录树；覆盖语义；保留源 mtime）
async fn copy_path(vfs: &dyn VfsProvider, roots: &(VfsPath, VfsPath), rel: &str, from: Side, to: Side) -> Result<(), String> {
    validate(vfs, roots, &FileOp::Copy { path: rel.to_string(), from, to }).await?;
    let src = resolve(roots, from, rel);
    let dst = resolve(roots, to, rel);
    // 父目录链
    mkdir_parents(vfs, roots, to, rel).await?;
    let src_stat = vfs.stat(&src).await.map_err(|e| format!("源不可读（{from}）: {rel}: {e}"))?;
    if src_stat.kind == EntryKind::Dir {
        copy_tree(vfs, &src, &dst, rel).await
    } else {
        copy_file_via(vfs, &src, &dst, rel).await
    }
}

/// 复制根 rel 的父目录链（目的侧）
async fn mkdir_parents(vfs: &dyn VfsProvider, roots: &(VfsPath, VfsPath), side: Side, rel: &str) -> Result<(), String> {
    let parent = parent_rel(rel);
    if parent.is_empty() {
        return Ok(());
    }
    let dst_parent = resolve(roots, side, &parent);
    vfs.mkdir_all(&dst_parent)
        .await
        .map_err(|e| format!("创建目标父目录失败（{side}）: {parent}: {e}"))
}

/// 单文件复制：优先 VFS 原生 copy_file（含 mtime 保留）；
/// 无 COPY 能力位时 read+write 组合降级（跨 Provider 组合）
async fn copy_file_via(vfs: &dyn VfsProvider, src: &VfsPath, dst: &VfsPath, rel: &str) -> Result<(), String> {
    if vfs.capabilities().has(VfsCaps::COPY) {
        return vfs
            .copy_file(src, dst)
            .await
            .map_err(|e| format!("复制失败: {rel}: {e}"));
    }
    let bytes = vfs.read_file_bytes(src).await.map_err(|e| format!("读源失败: {rel}: {e}"))?;
    vfs.write_file_bytes(dst, &bytes).await.map_err(|e| format!("写目标失败: {rel}: {e}"))
}

/// 目录树递归复制（覆盖语义；符号链接按目标内容复制，失败项终止该树）
fn copy_tree<'a>(
    vfs: &'a dyn VfsProvider,
    src_dir: &'a VfsPath,
    dst_dir: &'a VfsPath,
    rel_prefix: &'a str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
    Box::pin(async move { copy_tree_inner(vfs, src_dir, dst_dir, rel_prefix).await })
}

async fn copy_tree_inner(vfs: &dyn VfsProvider, src_dir: &VfsPath, dst_dir: &VfsPath, rel_prefix: &str) -> Result<(), String> {
    vfs.mkdir_all(dst_dir).await.map_err(|e| format!("创建目录失败: {rel_prefix}: {e}"))?;
    let entries = vfs
        .list_dir(src_dir)
        .await
        .map_err(|e| format!("读源目录失败: {rel_prefix}: {e}"))?;
    for entry in entries {
        let child_src = src_dir.join(&entry.name);
        let child_dst = dst_dir.join(&entry.name);
        let child_rel = format!("{rel_prefix}/{}", entry.name);
        if entry.stat.kind == EntryKind::Dir {
            copy_tree(vfs, &child_src, &child_dst, &child_rel).await?;
        } else {
            copy_file_via(vfs, &child_src, &child_dst, &child_rel).await?;
        }
    }
    Ok(())
}

/// 删除（side 内；recursive 仅用于目录；对已不存在目标返回 Skipped）
async fn remove_side(vfs: &dyn VfsProvider, roots: &(VfsPath, VfsPath), rel: &str, side: Side, strict_missing: bool) -> Result<OpStatus, String> {
    let target = resolve(roots, side, rel);
    if !vfs.capabilities().has(VfsCaps::DELETE) {
        return Err(format!("目标侧不支持删除（能力位缺失，{side}）"));
    }
    let stat = match vfs.stat(&target).await {
        Ok(s) => s,
        Err(_) => {
            if strict_missing {
                return Err(format!("目标不存在（{side}）: {rel}"));
            }
            return Ok(OpStatus::Skipped);
        }
    };
    // 根护栏双保险：resolve(root, "") == root；sanitize_rel 已拒绝空路径
    if target.as_str() == side_root(roots, side).as_str() {
        return Err(format!("拒绝删除根目录（{side}）"));
    }
    let recursive = stat.kind == EntryKind::Dir;
    vfs.remove(&target, recursive).await.map_err(|e| format!("删除失败（{side}）: {rel}: {e}"))?;
    Ok(OpStatus::Succeeded)
}

/// 同侧改名（优先 VFS 原子 rename）
async fn rename_side(vfs: &dyn VfsProvider, roots: &(VfsPath, VfsPath), rel: &str, side: Side, new_name: &str) -> Result<(), String> {
    if !vfs.capabilities().has(VfsCaps::MOVE) {
        return Err(format!("目标侧不支持改名（能力位缺失，{side}）"));
    }
    validate(vfs, roots, &FileOp::Rename { path: rel.to_string(), side, new_name: new_name.to_string() }).await?;
    let src = resolve(roots, side, rel);
    let dst = side_root(roots, side).join(&parent_rel(rel)).join(new_name);
    vfs.rename(&src, &dst).await.map_err(|e| format!("改名失败（{side}）: {rel} → {new_name}: {e}"))
}

/// 时间戳（mtime_ms=0 取对侧；FR-DIR-065 touch）
async fn touch_side(vfs: &dyn VfsProvider, roots: &(VfsPath, VfsPath), rel: &str, side: Side, mtime_ms: u64) -> Result<(), String> {
    if !vfs.capabilities().has(VfsCaps::TOUCH) {
        return Err(format!("目标侧不支持修改时间戳（能力位缺失，{side}）"));
    }
    let target = resolve(roots, side, rel);
    let mtime = if mtime_ms > 0 {
        mtime_ms
    } else {
        let other = match side {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        };
        let src_stat = vfs
            .stat(&resolve(roots, other, rel))
            .await
            .map_err(|e| format!("对侧不存在，无法取时间戳: {rel}: {e}"))?;
        src_stat.mtime_ms
    };
    vfs.set_mtime(&target, mtime).await.map_err(|e| format!("对时失败（{side}）: {rel}: {e}"))
}

/// 新建文件夹（父目录链自动创建；目标已存在报错）
async fn mkdir_side(vfs: &dyn VfsProvider, roots: &(VfsPath, VfsPath), rel: &str, side: Side) -> Result<(), String> {
    if !vfs.capabilities().has(VfsCaps::MKDIR) {
        return Err(format!("目标侧不支持创建目录（能力位缺失，{side}）"));
    }
    let target = resolve(roots, side, rel);
    if vfs.stat(&target).await.is_ok() {
        return Err(format!("目标已存在（{side}）: {rel}"));
    }
    vfs.mkdir_all(&target).await.map_err(|e| format!("创建目录失败（{side}）: {rel}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_vfs::LocalFs;

    /// 两侧测试树：
    /// L: same.txt("L") / diff.txt("L") / lonly.txt("L") / tree/(a.txt b.txt)
    /// R: same.txt("R") / diff.txt("R") / ronly.txt("R")
    async fn make_trees() -> (tempfile::TempDir, VfsPath, VfsPath) {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(l.join("tree")).unwrap();
        std::fs::write(l.join("same.txt"), b"L").unwrap();
        std::fs::write(l.join("diff.txt"), b"LEFT").unwrap();
        std::fs::write(l.join("lonly.txt"), b"L").unwrap();
        std::fs::write(l.join("tree").join("a.txt"), b"a-L").unwrap();
        std::fs::write(l.join("tree").join("b.txt"), b"b-L").unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(r.join("same.txt"), b"R").unwrap();
        std::fs::write(r.join("diff.txt"), b"RIGHT").unwrap();
        std::fs::write(r.join("ronly.txt"), b"R").unwrap();
        (
            dir,
            VfsPath::parse(l.to_str().unwrap()).unwrap(),
            VfsPath::parse(r.to_str().unwrap()).unwrap(),
        )
    }

    fn opts(dry_run: bool) -> ExecuteOptions {
        ExecuteOptions { dry_run }
    }

    fn copy_l2r(rel: &str) -> FileOp {
        FileOp::Copy { path: rel.into(), from: Side::Left, to: Side::Right }
    }

    fn planned(kind: PlannedOpKind, path: &str) -> PlannedOperation {
        PlannedOperation { op: kind, path: path.into(), reason: "test".into() }
    }

    #[tokio::test]
    async fn guard_rejects_empty_root_and_escape() {
        assert!(sanitize_rel("").is_err(), "空路径必须拒绝");
        assert!(sanitize_rel("/etc/passwd").is_err(), "绝对路径必须拒绝");
        assert!(sanitize_rel("a/../b").is_err(), ".. 必须拒绝");
        assert!(sanitize_rel("../escape").is_err());
        assert!(sanitize_rel("a//b").is_err(), "空段必须拒绝");
        assert!(sanitize_rel("a\\b").is_err(), "反斜杠必须拒绝");
        assert!(sanitize_rel("a/b.txt").is_ok());
        assert!(sanitize_rel("single.txt").is_ok());
    }

    #[tokio::test]
    async fn dry_run_lists_without_touching_fs() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let plan = vec![planned(PlannedOpKind::CopyLeftToRight, "lonly.txt"), planned(PlannedOpKind::DeleteRight, "ronly.txt")];
        let mut progress_count = 0usize;
        let report = execute_planned_operations(
            &vfs,
            &roots,
            &plan,
            &opts(true),
            Some(&mut |_, _, _| progress_count += 1),
            &CancelFlag::new(),
        )
        .await;
        assert!(report.dry_run);
        assert_eq!(report.total, 2);
        assert_eq!(report.succeeded.len(), 2, "校验通过项进将执行清单: {report:?}");
        assert_eq!(progress_count, 2, "dry_run 同样产出进度事件");
        // 文件系统未变：右仍存在、左→右复制未发生
        assert!(vfs.stat(&r.join("ronly.txt")).await.is_ok());
        assert!(vfs.stat(&r.join("lonly.txt")).await.is_err());
        // 源不存在的项应进失败清单
        let plan2 = vec![planned(PlannedOpKind::CopyLeftToRight, "ghost.txt")];
        let report2 = execute_planned_operations(&vfs, &roots, &plan2, &opts(true), None, &CancelFlag::new()).await;
        assert_eq!(report2.failed.len(), 1);
        assert!(report2.failed[0].detail.as_deref().unwrap().contains("ghost.txt"));
    }

    #[tokio::test]
    async fn copy_file_left_to_right() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let report = execute_ops(&vfs, &roots, &[copy_l2r("lonly.txt")], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 1, "{report:?}");
        let st = vfs.stat(&r.join("lonly.txt")).await.unwrap();
        assert_eq!(st.size, 1);
        assert_eq!(vfs.read_file_bytes(&r.join("lonly.txt")).await.unwrap(), b"L");
        // mtime 保留（BC 复制语义）
        let src = vfs.stat(&l.join("lonly.txt")).await.unwrap();
        assert_eq!(st.mtime_ms, src.mtime_ms);
    }

    #[tokio::test]
    async fn copy_dir_tree_recursive() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let report = execute_ops(&vfs, &roots, &[copy_l2r("tree")], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 1, "{report:?}");
        assert_eq!(vfs.read_file_bytes(&r.join("tree/a.txt")).await.unwrap(), b"a-L");
        assert_eq!(vfs.read_file_bytes(&r.join("tree/b.txt")).await.unwrap(), b"b-L");
    }

    #[tokio::test]
    async fn copy_overwrites_existing_file_but_rejects_type_conflict() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        // 文件→文件：覆盖
        let report = execute_ops(&vfs, &roots, &[copy_l2r("diff.txt")], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 1);
        assert_eq!(vfs.read_file_bytes(&r.join("diff.txt")).await.unwrap(), b"LEFT");
        // 文件→目录：类型冲突
        std::fs::create_dir_all(r.as_str().to_string() + "/clash").unwrap();
        let report2 = execute_ops(&vfs, &roots, &[copy_l2r("lonly.txt")], &opts(false), None, &CancelFlag::new()).await;
        // 注意路径不同名，此处构造同名冲突：lonly.txt 在右建为目录
        let _ = report2;
        let roots2 = (l.clone(), r.clone());
        let op = FileOp::Copy { path: "same.txt".into(), from: Side::Left, to: Side::Right };
        std::fs::remove_file(r.as_str().to_string() + "/same.txt").unwrap();
        std::fs::create_dir(r.as_str().to_string() + "/same.txt").unwrap();
        let report3 = execute_ops(&vfs, &roots2, &[op], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report3.failed.len(), 1, "文件→目录应报类型冲突: {report3:?}");
        assert!(report3.failed[0].detail.as_deref().unwrap().contains("类型冲突"));
    }

    #[tokio::test]
    async fn delete_file_and_dir_tree() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let ops = vec![
            FileOp::Delete { path: "lonly.txt".into(), side: Side::Left },
            FileOp::Delete { path: "tree".into(), side: Side::Left },
        ];
        let report = execute_ops(&vfs, &roots, &ops, &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 2, "{report:?}");
        assert!(vfs.stat(&l.join("lonly.txt")).await.is_err());
        assert!(vfs.stat(&l.join("tree")).await.is_err());
        // 删除不存在 → 失败（带明细）
        let report2 = execute_ops(&vfs, &roots, &ops, &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report2.failed.len(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_symlink_does_not_follow() {
        use std::os::unix::fs::symlink;
        let (_dir, l, r) = make_trees().await;
        let fs_path = l.as_str().to_string();
        let esc_path = fs_path.clone() + "/esc";
        symlink(r.as_str(), &esc_path).unwrap();
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let op = FileOp::Delete { path: "esc".into(), side: Side::Left };
        let report = execute_ops(&vfs, &roots, &[op], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 1, "{report:?}");
        assert!(std::fs::symlink_metadata(&esc_path).is_err(), "链接已删");
        assert!(vfs.stat(&r.join("same.txt")).await.is_ok(), "链接目标（右根）完好无损");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn copy_through_destination_symlink_is_rejected() {
        use std::os::unix::fs::symlink;
        let (dir, l, r) = make_trees().await;
        let outside = dir.path().join("OUTSIDE");
        std::fs::create_dir_all(&outside).unwrap();
        let precious = outside.join("precious.txt");
        std::fs::write(&precious, "PRECIOUS-OUTSIDE").unwrap();
        std::fs::write(std::path::Path::new(l.as_str()).join("notes.txt"), "LEFT-CONTENT").unwrap();
        symlink(&precious, std::path::Path::new(r.as_str()).join("notes.txt")).unwrap();
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let op = FileOp::Copy { path: "notes.txt".into(), from: Side::Left, to: Side::Right };
        let report = execute_ops(&vfs, &roots, &[op], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 0, "不得穿过目标符号链接写出根外: {report:?}");
        assert_eq!(std::fs::read_to_string(&precious).unwrap(), "PRECIOUS-OUTSIDE", "根外文件必须完好");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn delete_through_symlinked_parent_is_rejected() {
        use std::os::unix::fs::symlink;
        let (dir, l, r) = make_trees().await;
        let outside = dir.path().join("OUTSIDE2");
        std::fs::create_dir_all(&outside).unwrap();
        let victim = outside.join("victim.txt");
        std::fs::write(&victim, "victim").unwrap();
        symlink(&outside, std::path::Path::new(r.as_str()).join("sub")).unwrap();
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let op = FileOp::Delete { path: "sub/victim.txt".into(), side: Side::Right };
        let report = execute_ops(&vfs, &roots, &[op], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 0, "不得穿过符号链接父目录删除根外文件: {report:?}");
        assert!(victim.exists(), "根外文件必须完好");
    }

    #[tokio::test]
    async fn move_copies_then_removes_source() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let op = FileOp::Move { path: "lonly.txt".into(), from: Side::Left, to: Side::Right };
        let report = execute_ops(&vfs, &roots, &[op], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 1, "{report:?}");
        assert_eq!(vfs.read_file_bytes(&r.join("lonly.txt")).await.unwrap(), b"L");
        assert!(vfs.stat(&l.join("lonly.txt")).await.is_err(), "源应已删除");
    }

    #[tokio::test]
    async fn rename_within_side() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let op = FileOp::Rename { path: "lonly.txt".into(), side: Side::Left, new_name: "renamed.txt".into() };
        let report = execute_ops(&vfs, &roots, &[op], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 1, "{report:?}");
        assert!(vfs.stat(&l.join("renamed.txt")).await.is_ok());
        assert!(vfs.stat(&l.join("lonly.txt")).await.is_err());
        // 目标名已存在 → 失败
        let op2 = FileOp::Rename { path: "diff.txt".into(), side: Side::Left, new_name: "same.txt".into() };
        let report2 = execute_ops(&vfs, &roots, &[op2], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report2.failed.len(), 1);
    }

    #[tokio::test]
    async fn touch_copies_other_side_mtime() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        // 左 diff.txt mtime < 右 diff.txt mtime（先读，再对时）
        let before = vfs.stat(&l.join("diff.txt")).await.unwrap();
        std::fs::write(r.as_str().to_string() + "/diff.txt", b"RIGHT2").unwrap();
        let op = FileOp::Touch { path: "diff.txt".into(), side: Side::Left, mtime_ms: 0 };
        let report = execute_ops(&vfs, &roots, &[op], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 1, "{report:?}");
        let after = vfs.stat(&l.join("diff.txt")).await.unwrap();
        assert_ne!(after.mtime_ms, before.mtime_ms, "touch 应改写时间戳");
        let r_stat = vfs.stat(&r.join("diff.txt")).await.unwrap();
        assert_eq!(after.mtime_ms, r_stat.mtime_ms, "应取对侧时间戳");
    }

    #[tokio::test]
    async fn mkdir_creates_folder() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let op = FileOp::Mkdir { path: "newfolder".into(), side: Side::Right };
        let report = execute_ops(&vfs, &roots, std::slice::from_ref(&op), &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 1, "{report:?}");
        assert_eq!(vfs.stat(&r.join("newfolder")).await.unwrap().kind, EntryKind::Dir);
        // 已存在 → 失败
        let report2 = execute_ops(&vfs, &roots, &[op], &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report2.failed.len(), 1);
    }

    #[tokio::test]
    async fn partial_success_collects_failures() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let ops = vec![
            copy_l2r("lonly.txt"),       // 成功
            copy_l2r("ghost.txt"),       // 失败：源不存在
            FileOp::Delete { path: "ronly.txt".into(), side: Side::Right }, // 成功
            FileOp::Delete { path: "missing.txt".into(), side: Side::Right }, // 失败
        ];
        let report = execute_ops(&vfs, &roots, &ops, &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.total, 4);
        assert_eq!(report.succeeded.len(), 2, "{report:?}");
        assert_eq!(report.failed.len(), 2);
        assert!(!report.is_all_ok());
        // 部分成功：成功项已落盘
        assert!(vfs.stat(&r.join("lonly.txt")).await.is_ok());
        assert!(vfs.stat(&r.join("ronly.txt")).await.is_err());
    }

    #[tokio::test]
    async fn progress_callback_counts_each_op() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let ops = vec![copy_l2r("lonly.txt"), copy_l2r("diff.txt"), copy_l2r("same.txt")];
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = std::sync::Arc::clone(&seen);
        let report = execute_ops(&vfs, &roots, &ops, &opts(false), Some(&mut move |done, total, op| {
            seen2.lock().unwrap().push((done, total, op.path().to_string()));
        }), &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 3);
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], (1, 3, "lonly.txt".to_string()));
        assert_eq!(seen[2], (3, 3, "same.txt".to_string()));
    }

    #[tokio::test]
    async fn cancel_skips_remaining_ops() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let ops = vec![copy_l2r("lonly.txt"), copy_l2r("diff.txt")];
        let cancel = CancelFlag::new();
        cancel.cancel();
        let report = execute_ops(&vfs, &roots, &ops, &opts(false), None, &cancel).await;
        assert!(report.cancelled);
        assert_eq!(report.succeeded.len(), 0);
        assert_eq!(report.skipped.len(), 2, "{report:?}");
        assert!(vfs.stat(&r.join("lonly.txt")).await.is_err(), "取消后不落盘");
    }

    #[tokio::test]
    async fn planned_ops_map_and_execute_mirror_plan() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        // 镜像 左→右 手工计划（sync.rs plan 语义同形）
        let plan = vec![
            planned(PlannedOpKind::CopyLeftToRight, "lonly.txt"),
            planned(PlannedOpKind::DeleteRight, "ronly.txt"),
            planned(PlannedOpKind::CopyLeftToRight, "diff.txt"),
        ];
        let report = execute_planned_operations(&vfs, &roots, &plan, &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.succeeded.len(), 3, "{report:?}");
        assert!(vfs.stat(&r.join("lonly.txt")).await.is_ok());
        assert!(vfs.stat(&r.join("ronly.txt")).await.is_err());
        assert_eq!(vfs.read_file_bytes(&r.join("diff.txt")).await.unwrap(), b"LEFT");
    }

    #[tokio::test]
    async fn guard_blocks_root_and_empty_paths_at_execute() {
        let (_dir, l, r) = make_trees().await;
        let vfs = LocalFs::new();
        let roots = (l.clone(), r.clone());
        let ops = vec![
            FileOp::Delete { path: String::new(), side: Side::Left },
            FileOp::Delete { path: "a/../..".into(), side: Side::Right },
            FileOp::Delete { path: "/etc".into(), side: Side::Right },
        ];
        let report = execute_ops(&vfs, &roots, &ops, &opts(false), None, &CancelFlag::new()).await;
        assert_eq!(report.failed.len(), 3, "全部被护栏拒绝: {report:?}");
        // 根目录内容完好
        assert!(vfs.stat(&l.join("same.txt")).await.is_ok());
    }
}
