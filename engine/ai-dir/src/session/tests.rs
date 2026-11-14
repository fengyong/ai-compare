//! DirModel 会话级测试（docs/04 P2 验收要点）：惰性+后台扫描、取消无悬挂任务、
//! 聚合状态上推、expand、过滤器重扫、显示过滤、手动对齐、平铺模式、进度。

use std::sync::Arc;
use std::time::Duration;

use ai_protocol::{CompareCriteria, ContentCompareMode, DirEntryStatus, EntryKind, NameFilterSpec};
use ai_vfs::LocalFs;

use crate::align::ByNameAligner;
use crate::session::{DirDeltaData, DirModel, DirSessionOptions};

fn opts() -> DirSessionOptions {
    DirSessionOptions::new(
        CompareCriteria { mtime_tolerance_sec: 2, content_compare: ContentCompareMode::Auto, ..Default::default() },
        Arc::new(ByNameAligner),
    )
}

async fn make_tree(files: &[(&str, &[u8])]) -> (tempfile::TempDir, ai_vfs::VfsPath) {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, bytes) in files {
        let p = dir.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&p, bytes).expect("write");
    }
    let root = ai_vfs::VfsPath::parse(dir.path().to_str().unwrap()).unwrap();
    (dir, root)
}

#[tokio::test]
async fn lazy_open_answers_top_level_and_background_completes() {
    let (ld, left) = make_tree(&[("top.txt", b"1"), ("sub/a.txt", b"A1"), ("sub/b.txt", b"B1")]).await;
    let (rd, right) = make_tree(&[("top.txt", b"1"), ("sub/a.txt", b"A2"), ("sub/b.txt", b"B1")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    // Always：内容差异判定的确定性（Auto+秒容差下毫秒级创建时间差判同，语义正确）
    o.criteria.content_compare = ContentCompareMode::Always;
    o.target_depth = None; // 全量后台
    o.visible_depth = 1;
    let (model, first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    // 首个 delta：顶层立即（top.txt + sub 目录行）
    assert_eq!(first.upserts.len(), 2, "顶层立即可见：{:?}", first.upserts.iter().map(|r| r.id.clone()).collect::<Vec<_>>());
    assert!(first.upserts.iter().any(|r| r.id == "sub" && r.has_children));
    // 后台完成
    let handles = model.start_background();
    let phase = model.wait_until_done(Duration::from_secs(10)).await;
    assert_eq!(phase, ai_protocol::ProgressPhase::Completed);
    // worker 全部退出（无悬挂任务）：限时 join 断言
    for h in handles {
        let joined = tokio::time::timeout(Duration::from_secs(5), h).await;
        assert!(joined.is_ok(), "worker 应在 5s 内退出（无悬挂任务）");
    }
    // 聚合上推：sub 目录行变 DirDiff（a.txt 差异）
    let mut sub_status = None;
    let mut total_upserts = 0usize;
    while let Some(d) = model.take_delta().await {
        total_upserts += d.upserts.len();
        for r in d.upserts {
            if r.id == "sub" {
                sub_status = Some(r.status);
            }
        }
    }
    assert_eq!(sub_status, Some(DirEntryStatus::DirDiff), "聚合状态应上推到 sub 目录行");
    assert!(total_upserts > 0);
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn cancel_leaves_no_hanging_tasks() {
    // 深树 + 大量文件，让扫描需要多个 await 周期
    let mut files: Vec<(&str, &[u8])> = Vec::new();
    for i in 0..40 {
        files.push((Box::leak(format!("d{i}/f.txt").into_boxed_str()), b"x".as_slice()));
    }
    let (ld, left) = make_tree(&files).await;
    let (rd, right) = make_tree(&files).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.workers = 2;
    o.target_depth = None;
    let (model, _first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    let handles = model.start_background();
    // 立即取消
    model.cancel();
    let phase = model.wait_until_done(Duration::from_secs(5)).await;
    assert_eq!(phase, ai_protocol::ProgressPhase::Cancelled);
    for h in handles {
        let joined = tokio::time::timeout(Duration::from_secs(5), h).await;
        assert!(joined.is_ok(), "worker 应在 5s 内退出（无悬挂任务）");
    }
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn expand_returns_new_rows_and_updates_stats() {
    let (ld, left) = make_tree(&[("sub/a.txt", b"1"), ("sub/deep.txt", b"2")]).await;
    let (rd, right) = make_tree(&[("sub/a.txt", b"1"), ("sub/deep.txt", b"2")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.target_depth = Some(1);
    let (model, first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    assert_eq!(first.upserts.len(), 1);
    assert_eq!(first.upserts[0].id, "sub");
    let new_rows = model.expand("sub", 1).await.unwrap();
    assert_eq!(new_rows, 2, "sub 的两个子项");
    let d = model.take_delta().await.expect("expand 产生 delta");
    assert!(d.upserts.iter().any(|r| r.id == "sub/a.txt"));
    // 重复展开幂等
    assert_eq!(model.expand("sub", 1).await.unwrap(), 0);
    assert_eq!(model.phase(), ai_protocol::ProgressPhase::Completed, "target_depth=1 已覆盖");
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn update_filters_rescans_and_removes_old_rows() {
    let (ld, left) = make_tree(&[("keep.txt", b"k"), ("drop.log", b"d"), ("inner/x.txt", b"i")]).await;
    let (rd, right) = make_tree(&[("keep.txt", b"k"), ("drop.log", b"d"), ("inner/x.txt", b"i")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.target_depth = Some(1);
    let (model, _first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    model.update_filters(
        Some(NameFilterSpec {
            exclude_files: vec!["*.log".into()],
            ..Default::default()
        }),
        None,
    ).await.unwrap();
    let d = model.take_delta().await.expect("重扫 delta");
    assert!(d.removals.contains(&"drop.log".to_string()), "旧行被移除：{:?}", d.removals);
    let ids: Vec<&str> = d.upserts.iter().map(|r| r.id.as_str()).collect();
    assert!(!ids.contains(&"drop.log"), "排除行不再出现");
    assert!(ids.contains(&"keep.txt"));
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn unchanged_name_filter_must_not_rescan_away_expanded_rows() {
    // 回归：UI 的显示类切换（平铺/抑制/显示过滤）每次都附带当前 nameFilter（空 spec）。
    // 若 update_filters 无条件重扫，已展开子树会被整体移除 → 平铺视图丢光文件、
    // 切回树形也只剩折叠顶层（用户实测缺陷）。
    let (ld, left) = make_tree(&[("sub/a.txt", b"1"), ("top.txt", b"t")]).await;
    let (rd, right) = make_tree(&[("sub/a.txt", b"1"), ("top.txt", b"t")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.target_depth = Some(1);
    let (model, _first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    assert_eq!(model.expand("sub", 1).await.unwrap(), 1);
    let _ = model.take_delta().await;

    // 空 spec（= UI parseMaskInput("") 的形状）重复下发：不得重扫
    model.update_filters(Some(NameFilterSpec::default()), None).await.unwrap();
    assert!(model.take_delta().await.is_none(), "过滤器未变不应产生重扫 delta");

    // 显示类切换后平铺行必须仍含已展开子树里的文件
    model.set_display(None, None, Some(true));
    let d = model.take_delta().await.expect("平铺 delta");
    assert!(d.removals.is_empty(), "显示切换不得移除行：{:?}", d.removals);
    let ids: Vec<&str> = d.upserts.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"a.txt"), "平铺须含子树文件：{ids:?}");

    // 切回树形：子树行仍在（展开状态未被重扫摧毁）
    model.set_display(None, None, Some(false));
    let d = model.take_delta().await.expect("树形 delta");
    let ids: Vec<&str> = d.upserts.iter().map(|r| r.id.as_str()).collect();
    assert!(ids.contains(&"sub/a.txt"), "切回树形子树行须仍在：{ids:?}");
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn display_filter_hides_rows_without_affecting_status() {
    let (ld, left) = make_tree(&[("same.txt", b"s"), ("diff.txt", b"L")]).await;
    let (rd, right) = make_tree(&[("same.txt", b"s"), ("diff.txt", b"R")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.criteria.content_compare = ContentCompareMode::Always;
    o.target_depth = Some(1);
    let (model, _first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    // 状态判定不受显示过滤器影响：先取全量行
    let before = model.rows_snapshot().await;
    let diff_status = before.iter().find(|r| r.id == "diff.txt").unwrap().status;
    assert_eq!(diff_status, DirEntryStatus::ContentDiff);
    // 隐藏相同行
    model.set_display(
        Some(ai_protocol::DisplayFilterSpec { show_same: false, ..Default::default() }),
        None,
        None,
    );
    let d = model.take_delta().await.expect("显示过滤 delta");
    let same = d.upserts.iter().find(|r| r.id == "same.txt").unwrap();
    assert!(same.hidden, "same.txt 应被显示过滤器隐藏");
    assert_eq!(same.status, DirEntryStatus::Same, "状态判定不变");
    // 抑制过滤器：隐藏行以 suppressed 可见
    model.set_display(None, Some(true), None);
    let d = model.take_delta().await.expect("抑制 delta");
    let same = d.upserts.iter().find(|r| r.id == "same.txt").unwrap();
    assert!(!same.hidden && same.suppressed, "抑制后行可见且带标记");
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn manual_align_pairs_and_unalign_restores() {
    let (ld, left) = make_tree(&[("old_name.txt", b"same")]).await;
    let (rd, right) = make_tree(&[("new_name.txt", b"same")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.criteria.content_compare = ContentCompareMode::Always;
    o.target_depth = Some(1);
    let (model, _first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    let changed = model.align("old_name.txt", "new_name.txt").await.unwrap();
    assert_eq!(changed, 2);
    let rows = model.rows_snapshot().await;
    let old_row = rows.iter().find(|r| r.id == "old_name.txt").unwrap();
    let new_row = rows.iter().find(|r| r.id == "new_name.txt").unwrap();
    assert_eq!(old_row.status, DirEntryStatus::Same, "手动对齐后内容判同");
    assert_eq!(new_row.status, DirEntryStatus::Same);
    // 解除对齐 -> 恢复孤儿
    model.unalign("old_name.txt").await.unwrap();
    let rows = model.rows_snapshot().await;
    let old_row = rows.iter().find(|r| r.id == "old_name.txt").unwrap();
    assert_eq!(old_row.status, DirEntryStatus::LeftOnly);
    // 父不同 -> 报错（FR-DIR-022 父须并排）
    let err = model.align("nope_left", "new_name.txt").await;
    assert!(err.is_err());
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn flat_mode_aligns_by_name_across_dirs() {
    let (ld, left) = make_tree(&[("a.txt", b"1"), ("sub/a.txt", b"1"), ("sub/only_l.txt", b"l")]).await;
    let (rd, right) = make_tree(&[("a.txt", b"1"), ("other/a.txt", b"1"), ("sub/only_r.txt", b"r")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.target_depth = None;
    o.flat = true;
    let (model, first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    let _handles = model.start_background();
    let _ = model.wait_until_done(Duration::from_secs(10)).await;
    let rows = model.rows_snapshot().await;
    let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    // 平铺：a.txt 跨目录按名对齐（多出现 -> a.txt 与 a.txt#1）
    assert!(ids.iter().any(|i| i.starts_with("a.txt")), "平铺行: {ids:?}");
    assert!(ids.iter().all(|i| !i.contains('/') || i.contains('#')), "平铺 id 不含树形路径（消歧除外）: {ids:?}");
    // 孤儿单侧出现
    assert!(ids.contains(&"only_l.txt") && ids.contains(&"only_r.txt"));
    let a_rows: Vec<&DirDeltaData> = Vec::new();
    let _ = (a_rows, first);
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn progress_reports_scanned_and_estimate() {
    let (ld, left) = make_tree(&[("a.txt", b"1"), ("sub/b.txt", b"2")]).await;
    let (rd, right) = make_tree(&[("a.txt", b"1"), ("sub/b.txt", b"2")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.target_depth = None;
    let (model, _first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    // 扫描中：total 为估算（>= scanned）
    let p0 = model.progress();
    assert!(p0.total.unwrap_or(0) >= p0.scanned);
    let handles = model.start_background();
    let _ = model.wait_until_done(Duration::from_secs(10)).await;
    let p1 = model.progress();
    assert_eq!(p1.total, Some(p1.scanned), "完成后 total == scanned == 实际总数");
    assert_eq!(p1.scanned, 3, "a.txt + sub 目录 + sub/b.txt");
    for h in handles {
        let _ = h.await;
    }
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn unix_and_attribute_criteria_fill_side_info() {
    let (ld, left) = make_tree(&[("f.txt", b"x")]).await;
    let (rd, right) = make_tree(&[("f.txt", b"x")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.target_depth = Some(1);
    o.criteria.compare_attributes = true;
    o.criteria.unix_meta = true;
    let (_model, first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    let row = &first.upserts[0];
    let l = row.left.as_ref().unwrap();
    assert!(l.attribute.is_some(), "compareAttributes 开启 -> 属性字母");
    assert!(l.unix_mode.is_some(), "unixMeta 开启 -> 权限位");
    assert!(l.uid.is_some());
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn case_insensitive_alignment_marks_case_diff() {
    // 大小写不敏感对齐（macOS 等大小写不敏感文件系统语义）
    let (ld, left) = make_tree(&[("Readme.TXT", b"R")]).await;
    let (rd, right) = make_tree(&[("readme.txt", b"R")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = DirSessionOptions::new(
        CompareCriteria {
            mtime_tolerance_sec: 60,
            content_compare: ContentCompareMode::Never,
            compare_case: true,
            ..Default::default()
        },
        Arc::new(crate::align::CaseFoldAligner {
            options: crate::align::AlignKeyOptions { case_insensitive: true, ..Default::default() },
        }),
    );
    o.target_depth = Some(1);
    let (model, first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    assert_eq!(first.upserts.len(), 1);
    assert_eq!(first.upserts[0].status, DirEntryStatus::ContentDiff, "大小写不同 -> 判异");
    // 关闭 compare_case -> Same
    drop(model);
    let vfs2 = Arc::new(LocalFs::new());
    let mut o2 = DirSessionOptions::new(
        CompareCriteria {
            mtime_tolerance_sec: 60,
            content_compare: ContentCompareMode::Never,
            compare_case: false,
            ..Default::default()
        },
        Arc::new(crate::align::CaseFoldAligner {
            options: crate::align::AlignKeyOptions { case_insensitive: true, ..Default::default() },
        }),
    );
    o2.target_depth = Some(1);
    let (model2, first2) = DirModel::open(vfs2, &left, &right, o2, crate::CancelFlag::new()).await.unwrap();
    assert_eq!(first2.upserts[0].status, DirEntryStatus::Same);
    drop(model2);
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn entry_kinds_and_dirs_aggregate_at_finish() {
    let (ld, left) = make_tree(&[("d1/x.txt", b"1"), ("d2/y.txt", b"2")]).await;
    let (rd, right) = make_tree(&[("d1/x.txt", b"1"), ("d2/y.txt", b"DIFF")]).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.criteria.mtime_tolerance_sec = 0;
    o.criteria.content_compare = ContentCompareMode::Always;
    o.target_depth = None;
    let (model, _f) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    let handles = model.start_background();
    let _ = model.wait_until_done(Duration::from_secs(10)).await;
    for h in handles {
        let _ = h.await;
    }
    let rows = model.rows_snapshot().await;
    let d1 = rows.iter().find(|r| r.id == "d1").unwrap();
    let d2 = rows.iter().find(|r| r.id == "d2").unwrap();
    assert_eq!(d1.status, DirEntryStatus::DirSame);
    assert_eq!(d2.status, DirEntryStatus::DirDiff, "含差异子项的目录聚合为 DirDiff");
    // 根也被聚合
    let stats = model.take_delta().await.map(|d| d.stats).unwrap_or_default();
    // stats 只在 delta 有变化时返回；直接用快照状态断言即可
    let _ = stats;
    drop(ld);
    drop(rd);
}

#[allow(dead_code)]
fn kind_check(k: EntryKind) {
    let _ = k;
}

// ---------------------------------------------------------------------------
// R3-09：读不到的子目录不得被当成「相同」
// ---------------------------------------------------------------------------

/// 只在指定路径上让 list_dir 失败的 VFS 包装（其余委托 LocalFs）。
/// 用权限位造 EACCES 在 root 下会失效，包装器让失败确定性可复现。
struct DenyListFs {
    inner: LocalFs,
    deny_suffix: String,
}

#[async_trait::async_trait]
impl ai_vfs::VfsProvider for DenyListFs {
    fn scheme(&self) -> &'static str {
        "file"
    }
    fn capabilities(&self) -> ai_vfs::VfsCaps {
        self.inner.capabilities()
    }
    async fn stat(&self, path: &ai_vfs::VfsPath) -> Result<ai_vfs::StatInfo, ai_vfs::VfsError> {
        self.inner.stat(path).await
    }
    async fn list_dir(&self, path: &ai_vfs::VfsPath) -> Result<Vec<ai_vfs::VfsDirEntry>, ai_vfs::VfsError> {
        if path.as_str().ends_with(&self.deny_suffix) {
            return Err(ai_vfs::VfsError::Unsupported(format!("模拟不可读: {}", path.as_str())));
        }
        self.inner.list_dir(path).await
    }
    async fn read_file_bytes(&self, path: &ai_vfs::VfsPath) -> Result<Vec<u8>, ai_vfs::VfsError> {
        self.inner.read_file_bytes(path).await
    }
}

/// 子目录 list_dir 失败 → 该行 Unknown（不是 DirSame），错误随 delta 回到调用方，
/// 且祖先目录不得聚合成 DirSame。
#[tokio::test]
async fn unreadable_subdir_is_unknown_not_same_and_error_is_surfaced() {
    let (ld, left) = make_tree(&[("sub/hidden.txt", b"L")]).await;
    let (rd, right) = make_tree(&[("sub/hidden.txt", b"R")]).await;
    let vfs = Arc::new(DenyListFs { inner: LocalFs::new(), deny_suffix: "/sub".into() });
    let mut o = opts();
    o.target_depth = None;
    o.visible_depth = 1;
    let (model, _first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    let handles = model.start_background();
    let phase = model.wait_until_done(Duration::from_secs(10)).await;
    assert_eq!(phase, ai_protocol::ProgressPhase::Completed);
    for h in handles {
        let _ = h.await;
    }

    let rows = model.rows_snapshot().await;
    let sub = rows.iter().find(|r| r.id == "sub").expect("sub 行应存在");
    assert_eq!(sub.status, DirEntryStatus::Unknown, "读不到的目录不得报 DirSame: {sub:?}");

    let errors = model.scan_errors();
    assert!(errors.iter().any(|(p, _)| p == "sub"), "扫描错误必须记录 rel: {errors:?}");

    let mut surfaced = Vec::new();
    while let Some(d) = model.take_delta().await {
        surfaced.extend(d.scan_errors);
    }
    assert!(surfaced.iter().any(|(p, _)| p == "sub"), "错误必须随 delta 回到 UI: {surfaced:?}");
    drop(ld);
    drop(rd);
}

/// 平铺模式：id 是显示用消歧 id（basename 或 `basename#i`），不是文件地址；
/// 左右必须各自携带真实 rel，否则 UI 拿 id 当路径会操作到错误目标。
#[tokio::test]
async fn flat_rows_carry_real_side_paths_not_display_id() {
    let files = [("a.txt", b"1".as_slice()), ("dup/x.txt", b"1"), ("other/x.txt", b"2")];
    let (ld, left) = make_tree(&files).await;
    let (rd, right) = make_tree(&files).await;
    let vfs = Arc::new(LocalFs::new());
    let mut o = opts();
    o.flat = true;
    o.target_depth = None;
    o.visible_depth = 1;
    let (model, _first) = DirModel::open(vfs, &left, &right, o, crate::CancelFlag::new()).await.unwrap();
    let handles = model.start_background();
    let _ = model.wait_until_done(Duration::from_secs(10)).await;
    for h in handles {
        let _ = h.await;
    }
    let rows = model.rows_snapshot().await;
    let real: std::collections::HashSet<&str> = ["a.txt", "dup/x.txt", "other/x.txt"].into_iter().collect();
    assert!(!rows.is_empty());
    for r in &rows {
        let lp = r.left_path.as_deref().expect("平铺行必须携带左侧真实 rel");
        let rp = r.right_path.as_deref().expect("平铺行必须携带右侧真实 rel");
        assert!(real.contains(lp), "left_path 必须是真实 rel: {r:?}");
        assert!(real.contains(rp), "right_path 必须是真实 rel: {r:?}");
        assert!(!lp.contains('#') && !rp.contains('#'), "真实 rel 不得带显示消歧后缀: {r:?}");
    }
    // 同名两处 → `x.txt#0` / `x.txt#1`：id 与真实 rel 明显不同，正是 R3-03 的根因
    let xs: Vec<_> = rows.iter().filter(|r| r.name == "x.txt").collect();
    assert_eq!(xs.len(), 2, "同名两处应各占一行: {:?}", xs.iter().map(|r| &r.id).collect::<Vec<_>>());
    assert!(xs.iter().all(|r| r.id.contains('#')), "同名多 occurrence 应产生 #i 消歧 id: {:?}", xs.iter().map(|r| &r.id).collect::<Vec<_>>());
    assert!(xs.iter().any(|r| r.id != r.left_path.clone().unwrap()), "id 不得被当作文件地址");
    let paths: std::collections::HashSet<_> = xs.iter().map(|r| r.left_path.clone().unwrap()).collect();
    assert_eq!(paths.len(), 2, "两个消歧 id 必须指向不同真实 rel: {paths:?}");
    drop(ld);
    drop(rd);
}
