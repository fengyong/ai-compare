//! PB-04 冒烟基准（docs/04 P0 交付物 ⑨）。
//!
//! 运行：`cargo bench -p ai-core`
//!
//! 说明：docs/03 §8.1 的正式 PB-04（10 万项目录滚动帧率）依赖 UI 虚拟列表，
//! P0 引擎侧冒烟以"1 万行文件 diff 耗时"替代（docs/04 任务书口径），
//! 目的仅为建立可运行的计时基线，不设强制达标线（正式线在 P5/P2 认领）。

use std::time::Instant;

use ai_core::{diff_text, inline_diff_line, LineTable, SectionKind, TextDiffOptions};

fn build_pair(lines: usize) -> (String, String) {
    let mut left = String::with_capacity(lines * 24);
    let mut right = String::with_capacity(lines * 24);
    for i in 0..lines {
        left.push_str(&format!("line {i} content {i} alpha\n"));
        // 每 100 行改 1 行，每 200 行插入 1 行，每 300 行删除 1 行
        if i % 200 == 0 {
            right.push_str(&format!("line {i} INSERTED extra\n"));
        }
        if i % 300 != 0 {
            if i % 100 == 0 {
                right.push_str(&format!("line {i} content {i} MODIFIED\n"));
            } else {
                right.push_str(&format!("line {i} content {i} alpha\n"));
            }
        }
    }
    (left, right)
}

fn main() {
    let lines = 10_000;
    let (left, right) = build_pair(lines);
    let lt = LineTable::from_text(&left);
    let rt = LineTable::from_text(&right);
    let opts = TextDiffOptions::default();

    println!("PB-04 smoke: {lines} 行文件 diff（含约 {lines}/200 插入、{lines}/300 删除、{lines}/100 修改）");

    // 预热 + 3 次计时
    let mut best = f64::MAX;
    for run in 1..=4 {
        let t0 = Instant::now();
        let out = diff_text(&lt, &rt, &opts).expect("diff 失败");
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if run == 1 {
            println!("  run 1: {ms:.1} ms（sections = {}）", out.summary.section_count);
        } else {
            best = best.min(ms);
        }
    }
    println!("  best of 3: {best:.1} ms  ->  {iops:.0} 行/s", iops = lines as f64 / best * 1000.0);

    // 行内 diff 冒烟（取典型修改行）
    let t0 = Instant::now();
    let r = inline_diff_line(
        "line 100 content 100 alpha",
        "line 100 content 100 MODIFIED",
        &opts.inline,
    );
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "  inline diff（词级）: {ms:.3} ms，truncated = {}，spans = {}",
        r.truncated,
        r.spans.len()
    );

    // 结果 sanity：应产生 Change section
    let out = diff_text(&lt, &rt, &opts).expect("diff 失败");
    assert!(out.summary.change_sections > 0, "语料应产生 Change section");
    assert_eq!(
        out.sections.iter().filter(|s| s.kind == SectionKind::Equal).count(),
        out.summary.equal_sections as usize
    );
    println!("PB-04 smoke 完成（仅打印基线，无达标线）");
}
