//! PB 基准（docs/03 §7 / docs/04 P2 验收要点）：
//! - PB-03 目录扫描吞吐：10 万项目录两树扫描耗时（目标 ≤2s 起步，不强制达标，
//!   实测值入 PB 报表）；
//! - PB-10 delta 序列化：10 万行 DirRow -> JSON 批次（512/批）总耗时。
//!
//! 运行（默认测试集跳过；语料构建本身需要数十秒）：
//! ```text
//! cargo test -p ai-compare-tests --test pb_bench --release --ignored -- --nocapture
//! ```
//! `AI_COMPARE_BENCH_ROWS` 可覆盖文件数（默认 100_000）。
//!
//! 确定性：语料由 xorshift64* 生成（seed 固定），同一 seed 跨机器树完全一致。

use ai_compare_tests::{build_big_corpus, Rng};
use ai_dir::{rows_from_arena, ByNameAligner, CancelFlag, DirScanOptions};
use ai_protocol::{CompareCriteria, ContentCompareMode, DirRow};
use ai_vfs::{LocalFs, VfsPath};
use std::time::Instant;

fn rows() -> u64 {
    std::env::var("AI_COMPARE_BENCH_ROWS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100_000)
}

/// PB-03：10 万项目录扫描吞吐（快速测试路径：mtime 大容差 + 内容禁用，
/// 隔离纯扫描/判等开销；内容比较吞吐随 PB-11 单列）
#[test]
#[ignore = "基准：cargo test -p ai-compare-tests --test pb_bench --release --ignored -- --nocapture"]
fn pb03_scan_throughput_100k() {
    let n = rows();
    eprintln!("[PB-03] 生成语料：{n} 文件（seed=42）...");
    let t0 = Instant::now();
    let corpus = build_big_corpus(n, 42);
    eprintln!(
        "[PB-03] 语料就绪：{} 文件 / {:?}（不计入扫描耗时）",
        corpus.files_written,
        t0.elapsed()
    );

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let vfs = LocalFs::new();
    let left = VfsPath::parse(corpus.left.to_str().unwrap()).unwrap();
    let right = VfsPath::parse(corpus.right.to_str().unwrap()).unwrap();
    let opts = DirScanOptions {
        depth: None,
        criteria: CompareCriteria {
            mtime_tolerance_sec: 3600, // 快速测试判同：隔离内容比较
            content_compare: ContentCompareMode::Never,
            ..Default::default()
        },
        importance: Default::default(),
    };

    // 预热一次（page cache），计时第二次
    let warm = rt.block_on(ai_dir::scan(&vfs, &left, &right, &opts, &ByNameAligner, &CancelFlag::new()));
    eprintln!("[PB-03] 预热扫描完成：{} 行", warm.map(|r| r.stats.total).unwrap_or(0));
    let t1 = Instant::now();
    let result = rt
        .block_on(ai_dir::scan(&vfs, &left, &right, &opts, &ByNameAligner, &CancelFlag::new()))
        .expect("基准扫描成功");
    let elapsed = t1.elapsed();
    let total = result.stats.total as f64;
    let rows_len = rows_from_arena(&result.arena).len();
    eprintln!(
        "[PB-03] 扫描 {total:.0} 行耗时 {:?}（吞吐 {:.0} 行/s）；rows={rows_len} stats={:?}",
        elapsed,
        total / elapsed.as_secs_f64(),
        result.stats
    );
    eprintln!(
        "[PB-03] 目标 ≤2s（起步值，不强制达标）：{}",
        if elapsed.as_secs_f64() <= 2.0 { "达标" } else { "未达标（记录实测值）" }
    );
}

/// PB-10：10 万行 delta 序列化耗时（DirRow -> JSON，512 行/批，docs/03 R9）
#[test]
#[ignore = "基准：cargo test -p ai-compare-tests --test pb_bench --release --ignored -- --nocapture"]
fn pb10_delta_serialization_100k() {
    let n = rows();
    eprintln!("[PB-10] 构造 {n} 行 DirRow...");
    let mut rng = Rng::new(7);
    let statuses = [
        ai_protocol::DirEntryStatus::Same,
        ai_protocol::DirEntryStatus::ContentDiff,
        ai_protocol::DirEntryStatus::LeftOnly,
        ai_protocol::DirEntryStatus::RightOnly,
        ai_protocol::DirEntryStatus::DirSame,
    ];
    let all: Vec<DirRow> = (0..n)
        .map(|i| DirRow {
            id: format!("d{}/m{}/f{:06}.bin", i % 48, (i / 48) % 32, i),
            name: format!("f{:06}.bin", i),
            kind: ai_protocol::EntryKind::File,
            left: Some(ai_protocol::SideInfo::base(rng.below(4096), rng.next_u64() / 2)),
            right: Some(ai_protocol::SideInfo::base(rng.below(4096), rng.next_u64() / 2)),
            status: statuses[(rng.below(5)) as usize],
            has_children: false,
            hidden: false,
            suppressed: false,
            left_path: Some(format!("d{}/m{}/f{:06}.bin", i % 48, (i / 48) % 32, i)),
            right_path: Some(format!("d{}/m{}/f{:06}.bin", i % 48, (i / 48) % 32, i)),
        })
        .collect();

    // 批次切分 + serde_json 序列化（引擎 pump 的真实路径形态）
    const BATCH: usize = 512;
    let t1 = Instant::now();
    let mut bytes = 0usize;
    for (idx, chunk) in all.chunks(BATCH).enumerate() {
        let delta = ai_protocol::DirDelta {
            session_id: "bench".into(),
            rev: idx as u64 + 1,
            upserts: chunk.to_vec(),
            removals: Vec::new(),
            stats: Default::default(),
            progress: Some(ai_protocol::ScanProgress {
                scanned: (idx * BATCH) as u32,
                total: Some(n as u32),
            }),
            batch: ai_protocol::BatchInfo { index: idx as u32, has_more: true },
            scan_errors: Vec::new(),
        };
        let s = serde_json::to_string(&delta).expect("序列化");
        bytes += s.len();
    }
    let elapsed = t1.elapsed();
    eprintln!(
        "[PB-10] {} 行 / {} 批（512/批）序列化耗时 {:?}（吞吐 {:.0} 行/s，共 {bytes} 字节）",
        n,
        n.div_ceil(BATCH as u64),
        elapsed,
        n as f64 / elapsed.as_secs_f64(),
    );
}
