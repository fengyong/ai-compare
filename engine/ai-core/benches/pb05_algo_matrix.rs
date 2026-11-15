//! PB-05 算法矩阵基准（docs/04 P1 交付物 ⑥；docs/03 §8.1 PB-05）。
//!
//! 语料：20 万行 / ~10MB 文本对（周期性修改/插入/删除，约 0.4% 编辑密度），
//! 四种行对齐算法（Standard / Myers / Patience / Histogram）各计时 3 次取最优，
//! 以 PB 报表行输出（达标线：PB-05 文本打开 10MB/20 万行 理想 800ms / 可接受 2s，
//! 本基准测其中"行级 diff"一段，为整条链路预算的一部分）。
//!
//! 运行：`cargo bench -p ai-core --bench pb05_algo_matrix`

use std::time::Instant;

use ai_core::options::{DiffConfig, LineAlignAlgo, ManualAlignOptions, TextDiffOptions};
use ai_core::{diff_config, LineTable};

const LINES: usize = 200_000;

/// 构建两侧语料：每 500 行改 1 行、每 1000 行插 1 行、每 1000 行删 1 行
///（编辑密度 0.4%，D ≈ 1.6k，Myers O((N+M)D) 在 release 下秒级内）。
fn build_pair() -> (String, String) {
    let mut left = String::with_capacity(LINES * 52);
    let mut right = String::with_capacity(LINES * 52);
    for i in 0..LINES {
        if i % 1000 == 999 {
            // 每 1000 行跳过 1 行（右侧相对左侧删除）
            continue;
        }
        left.push_str(&format!(
            "line {i:06} payload alpha beta gamma delta epsilon\n"
        ));
        if i % 1000 == 500 {
            right.push_str(&format!("line {i:06} payload INSERTED extra tail\n"));
        } else if i % 500 == 250 {
            right.push_str(&format!("line {i:06} payload MODIFIED zeta eta theta\n"));
        } else {
            right.push_str(&format!(
                "line {i:06} payload alpha beta gamma delta epsilon\n"
            ));
        }
        if i % 1000 == 499 {
            right.push_str(&format!("line {i:06} payload ADDED tail iota kappa\n"));
        }
    }
    (left, right)
}

fn run_algo(name: &str, lt: &LineTable, rt: &LineTable, cfg: &DiffConfig) {
    // 预热 1 次 + 计时 3 次取最优
    let warm = diff_config(lt, rt, cfg).expect("diff 失败");
    let mut best = f64::MAX;
    let mut sections = 0u32;
    for _ in 0..3 {
        let t0 = Instant::now();
        let out = diff_config(lt, rt, cfg).expect("diff 失败");
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        best = best.min(ms);
        sections = out.output.summary.section_count;
    }
    let lines_per_s = LINES as f64 / (best / 1000.0);
    println!(
        "PB-05 | text-diff | corpus=200k-lines(~10MB) | algo={name:<9} | best={best:8.1} ms | {lines_per_s:9.0} 行/s | sections={sections} | 预热校验 sections={} | 达标线(整链路): 800ms/2000ms",
        warm.output.summary.section_count
    );
}

fn main() {
    println!("PB-05 算法矩阵基准：{LINES} 行文本对，四种行对齐算法（docs/04 P1 交付物 ⑥）");
    let t0 = Instant::now();
    let (left, right) = build_pair();
    let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let lt = LineTable::from_text(&left);
    let rt = LineTable::from_text(&right);
    println!(
        "  语料：{} 行 / {:.1} MB（左） vs {} 行 / {:.1} MB（右）；构建+分行 {:.0} ms（不计入）",
        lt.len(),
        left.len() as f64 / 1e6,
        rt.len(),
        right.len() as f64 / 1e6,
        build_ms
    );

    for (name, algo) in [
        ("standard", LineAlignAlgo::Standard),
        ("myers", LineAlignAlgo::Myers),
        ("patience", LineAlignAlgo::Patience),
        ("histogram", LineAlignAlgo::Histogram),
    ] {
        let cfg = DiffConfig {
            base: TextDiffOptions {
                algo,
                ..TextDiffOptions::default()
            },
            manual: ManualAlignOptions::default(),
            ..DiffConfig::default()
        };
        run_algo(name, &lt, &rt, &cfg);
    }
    println!("PB-05 完成（best of 3；语料编辑密度 0.4%）");
}
