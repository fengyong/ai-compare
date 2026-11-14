//! ai-dir 场景测试（docs/04 P0 测试清单：空目录、单侧存在、重命名配对、
//! mtime 秒容差、二进制相同/不同）+ 对齐器与 delta 行模型测试。
//!
//! mtime 稳定性策略：不依赖精确 mtime 注入（std 无 API、避免额外依赖），
//! 容差机制用 tolerance=0（关闭快速判同）做确定性验证；
//! 容差正向路径用大容差（60s）保证跨 CI 抖动稳定。

use ai_protocol::{CompareCriteria, ContentCompareMode, DirEntryStatus};
use ai_vfs::{LocalFs, VfsPath};

use crate::{ByNameAligner, CancelFlag, DirScanOptions, NameAligner, rows_from_arena, scan};

async fn make_tree(files: &[(&str, &[u8])]) -> (tempfile::TempDir, VfsPath) {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, bytes) in files {
        let p = dir.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&p, bytes).expect("write");
    }
    let root = VfsPath::parse(dir.path().to_str().unwrap()).unwrap();
    (dir, root)
}

fn criteria(tol: u32, mode: ContentCompareMode) -> CompareCriteria {
    CompareCriteria { mtime_tolerance_sec: tol, content_compare: mode, ..Default::default() }
}

async fn scan_tops(left: &VfsPath, right: &VfsPath, crit: CompareCriteria) -> crate::DirScanResult {
    let vfs = LocalFs::new();
    let opts = DirScanOptions {
        depth: Some(1),
        criteria: crit,
        ..Default::default()
    };
    scan(&vfs, left, right, &opts, &ByNameAligner, &CancelFlag::new())
        .await
        .expect("scan")
}

#[tokio::test]
async fn empty_vs_empty_zero_rows() {
    let (ld, left) = make_tree(&[]).await;
    let (rd, right) = make_tree(&[]).await;
    let res = scan_tops(&left, &right, criteria(2, ContentCompareMode::Auto)).await;
    assert_eq!(res.arena.len(), 1, "仅根节点");
    assert!(rows_from_arena(&res.arena).is_empty());
    assert_eq!(res.stats.total, 0);
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn one_side_empty_all_orphans() {
    let (ld, left) = make_tree(&[("a.txt", b"aa"), ("b.txt", b"bb")]).await;
    let (rd, right) = make_tree(&[]).await;
    let res = scan_tops(&left, &right, criteria(2, ContentCompareMode::Auto)).await;
    let rows = rows_from_arena(&res.arena);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.status == DirEntryStatus::LeftOnly));
    assert!(rows.iter().all(|r| r.left.is_some() && r.right.is_none()));
    assert_eq!(res.stats.left_only, 2);
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn mtime_within_tolerance_is_same_without_content_read() {
    let content = b"identical";
    let (ld, left) = make_tree(&[("f.txt", content)]).await;
    let (rd, right) = make_tree(&[("f.txt", content)]).await;
    // 容差 60s：两侧 mtime 必然落在容差内 -> Same（Never 模式下不做内容比较）
    let res = scan_tops(&left, &right, criteria(60, ContentCompareMode::Never)).await;
    assert_eq!(rows_from_arena(&res.arena)[0].status, DirEntryStatus::Same);
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn mtime_tolerance_zero_disables_quick_same() {
    let content = b"stable";
    let (ld, left) = make_tree(&[("f.txt", content)]).await;
    let (rd, right) = make_tree(&[("f.txt", content)]).await;
    // tolerance=0：只要秒值不完全相等就判异；Never 模式 -> QuickDiff（未复核）
    let res = scan_tops(&left, &right, criteria(0, ContentCompareMode::Never)).await;
    let status = rows_from_arena(&res.arena)[0].status;
    assert!(status == DirEntryStatus::QuickDiff || status == DirEntryStatus::Same,
        "秒值恰好相等时 Same，否则 QuickDiff；实际 {status:?}");
    // Auto 模式：mtime 判异 -> 内容复核 -> 内容相同 -> Same（确定性）
    let res = scan_tops(&left, &right, criteria(0, ContentCompareMode::Auto)).await;
    assert_eq!(rows_from_arena(&res.arena)[0].status, DirEntryStatus::Same);
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn binary_same_and_different() {
    let blob: Vec<u8> = (0..200_000u32).map(|i| (i.wrapping_mul(31) % 256) as u8).collect();
    let mut diff_blob = blob.clone();
    let last = diff_blob.len() - 1;
    diff_blob[last] ^= 0xff; // 同长度、跨多块的一字节差异

    let (ld, left) = make_tree(&[("a.bin", &blob)]).await;
    let (rd, right) = make_tree(&[("a.bin", &blob)]).await;
    // Always 模式强制内容路径（分块哈希）
    let res = scan_tops(&left, &right, criteria(0, ContentCompareMode::Always)).await;
    assert_eq!(rows_from_arena(&res.arena)[0].status, DirEntryStatus::Same);
    drop(ld);
    drop(rd);

    let (ld, left) = make_tree(&[("a.bin", &blob)]).await;
    let (rd, right) = make_tree(&[("a.bin", &diff_blob)]).await;
    let res = scan_tops(&left, &right, criteria(0, ContentCompareMode::Always)).await;
    assert_eq!(rows_from_arena(&res.arena)[0].status, DirEntryStatus::ContentDiff);
    drop(ld);
    drop(rd);

    // size 不同：直接判异，不读内容（Never 模式同样成立）
    let (ld, left) = make_tree(&[("a.bin", &blob)]).await;
    let (rd, right) = make_tree(&[("a.bin", &diff_blob[..1000])]).await;
    let res = scan_tops(&left, &right, criteria(0, ContentCompareMode::Never)).await;
    assert_eq!(rows_from_arena(&res.arena)[0].status, DirEntryStatus::ContentDiff);
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn rename_becomes_orphan_pair() {
    // 按名对齐下，重命名 = LeftOnly + RightOnly 一对；
    // 模式/相似度配对走 NameAligner 扩展口（FR-DIR-023，P2）。
    let (ld, left) = make_tree(&[("old_name.txt", b"same content")]).await;
    let (rd, right) = make_tree(&[("new_name.txt", b"same content")]).await;
    let res = scan_tops(&left, &right, criteria(2, ContentCompareMode::Auto)).await;
    let rows = rows_from_arena(&res.arena);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "new_name.txt");
    assert_eq!(rows[0].status, DirEntryStatus::RightOnly);
    assert_eq!(rows[1].id, "old_name.txt");
    assert_eq!(rows[1].status, DirEntryStatus::LeftOnly);
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn depth_controls_recursion_and_rows_deterministic() {
    let (ld, left) = make_tree(&[("top.txt", b"1"), ("sub/inner.txt", b"2"), ("sub/deep/deeper.txt", b"3")]).await;
    let (rd, right) = make_tree(&[("top.txt", b"1"), ("sub/inner.txt", b"2"), ("sub/deep/deeper.txt", b"3")]).await;
    let vfs = LocalFs::new();

    // depth=1：仅顶层
    let res = scan(
        &vfs,
        &left,
        &right,
        &DirScanOptions {
            depth: Some(1),
            criteria: criteria(2, ContentCompareMode::Auto),
            ..Default::default()
        },
        &ByNameAligner,
        &CancelFlag::new(),
    )
    .await
    .unwrap();
    let ids: Vec<String> = rows_from_arena(&res.arena)
        .iter()
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(ids, vec!["sub", "top.txt"], "BFS 序：字典序");
    let rows = rows_from_arena(&res.arena);
    assert_eq!(rows[0].status, DirEntryStatus::DirSame, "目录两侧存在 -> DirSame（P0 占位，P2 聚合）");
    assert!(rows[0].has_children);
    assert!(!ids.iter().any(|i| i.contains('/')), "depth=1 不展开子目录");

    // depth=None：全递归
    let res = scan(
        &vfs,
        &left,
        &right,
        &DirScanOptions {
            depth: None,
            criteria: criteria(2, ContentCompareMode::Auto),
            ..Default::default()
        },
        &ByNameAligner,
        &CancelFlag::new(),
    )
    .await
    .unwrap();
    let ids: Vec<String> = rows_from_arena(&res.arena)
        .iter()
        .map(|r| r.id.clone())
        .collect();
    assert_eq!(
        ids,
        vec!["sub", "top.txt", "sub/deep", "sub/inner.txt", "sub/deep/deeper.txt"],
        "BFS：根孩子 -> sub 孩子"
    );
    drop(ld);
    drop(rd);
}

#[tokio::test]
async fn root_missing_is_error() {
    let (ld, left) = make_tree(&[]).await;
    let missing = left.join("does-not-exist");
    let vfs = LocalFs::new();
    let err = scan(&vfs, &left, &missing, &DirScanOptions::default(), &ByNameAligner, &CancelFlag::new()).await.unwrap_err();
    assert!(matches!(err, crate::DirScanError::RootNotFound { side: "right", .. }));
    drop(ld);
}

#[tokio::test]
async fn cancel_flag_stops_scan() {
    let (ld, left) = make_tree(&[("a.txt", b"1")]).await;
    let (rd, right) = make_tree(&[("a.txt", b"1")]).await;
    let vfs = LocalFs::new();
    let flag = CancelFlag::new();
    flag.cancel();
    let err = scan(&vfs, &left, &right, &DirScanOptions::default(), &ByNameAligner, &flag).await.unwrap_err();
    assert!(matches!(err, crate::DirScanError::Cancelled));
    drop(ld);
    drop(rd);
}

#[test]
fn by_name_aligner_covers_all() {
    let mk = |name: &str, size: u64| ai_vfs::VfsDirEntry {
        name: name.to_string(),
        stat: ai_vfs::StatInfo::minimal(size, 0, ai_vfs::EntryKind::File),
    };
    let left = vec![mk("a", 1), mk("b", 2), mk("d", 4)];
    let right = vec![mk("b", 2), mk("c", 3), mk("d", 5)];
    let pairs = ByNameAligner.align(&left, &right);
    let names: Vec<&str> = pairs.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c", "d"]);
    assert_eq!((pairs[0].left, pairs[0].right), (Some(0), None));
    assert_eq!((pairs[1].left, pairs[1].right), (Some(1), Some(0)));
    assert_eq!((pairs[2].left, pairs[2].right), (None, Some(1)));
    assert_eq!((pairs[3].left, pairs[3].right), (Some(2), Some(2)));
}
