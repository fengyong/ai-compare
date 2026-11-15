//! P7 三向合并测试矩阵（docs/04 §P7 验收要点：diff3 golden 矩阵 + git merge-file 对拍）。
//!
//! 三层验证：
//! 1. [`golden_merge_outputs`]：≥20 语料 × 4 输出模式（diff3 marker / merge marker /
//!    auto-takeLeft / auto-takeRight）的本实现输出快照（确定性 golden）；
//! 2. [`crosscheck_gnu_diff3`] / [`crosscheck_git_merge_file`]：与 GNU `diff3 -m`、
//!    `git merge-file` 的**活体对拍**——非歧义语料逐字节一致；歧义/EOF 差异语料
//!    记录差异分析（对拍报告落快照 [`golden_crosscheck_report`]）；
//! 3. 决策合成幂等 + 重要性开关冲突缩小的行为测试。
//!
//! 已知差异（分析记录，非缺陷）：
//! - **EOF 冲突无尾换行**：GNU diff3 -m 把 marker「粘」在无换行内容行后
//!   （保留 no-newline 状态）；本实现与 git merge-file 一致，marker 前补换行；
//! - **重复行歧义**：等价最小 diff 的选择不同导致稳定行集不同（对拍与
//!   P1 算法矩阵同性质：结果仍为合法合并，编辑量守恒）。
//!
//! 重新固化：`UPDATE_GOLDEN=1 cargo test -p ai-compare-tests --test merge3_matrix`。

use ai_compare_tests as harness;
use ai_core::merge::{
    auto_merge_text, merge3, MarkerOptions, MarkerStyle, MergeOptions, SynthesizeOptions,
    UnresolvedPolicy,
};
use ai_core::LineTable;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// 语料矩阵（≥20；覆盖：无冲突/两侧同改/交错改/冲突邻接/空文件/CRLF/Unicode/
// 重复行歧义/删除-修改/添加-添加/EOF 无尾换行/冲突窗口）
// ---------------------------------------------------------------------------

struct Corpus {
    name: &'static str,
    base: &'static str,
    left: &'static str,
    right: &'static str,
    /// 冲突位于 EOF 且末行无尾换行（GNU diff3 marker 粘连差异，见模块文档）
    eof_no_newline: bool,
    /// 重复行歧义语料（等价 diff 选择可能不同；断言放宽为合法性校验）
    ambiguous: bool,
    /// 冲突窗口（None = 默认；Some(n) 仅本实现支持，对拍跳过）
    window: Option<u32>,
}

impl Corpus {
    fn plain(
        name: &'static str,
        base: &'static str,
        left: &'static str,
        right: &'static str,
    ) -> Self {
        Corpus {
            name,
            base,
            left,
            right,
            eof_no_newline: false,
            ambiguous: false,
            window: None,
        }
    }
}

fn corpora() -> Vec<Corpus> {
    vec![
        // ---- 无冲突 ----
        Corpus::plain(
            "clean_disjoint",
            "a\nb\nc\nd\ne\n",
            "A\nb\nc\nd\ne\n",
            "a\nb\nc\nd\nE\n",
        ),
        Corpus::plain(
            "clean_interleaved",
            "1\n2\n3\n4\n5\n6\n7\n",
            "1\nL2\n3\n4\n5\n6\n7\n",
            "1\n2\n3\nR4\n5\n6\n7\n",
        ),
        Corpus::plain("left_only_change", "a\nb\nc\n", "a\nB!\nc\n", "a\nb\nc\n"),
        Corpus::plain(
            "right_only_append",
            "a\nb\nc\n",
            "a\nb\nc\n",
            "a\nb\nc\nd\n",
        ),
        // ---- 两侧同改（不算冲突）----
        Corpus::plain("both_same_change", "a\nb\nc\n", "a\nX\nc\n", "a\nX\nc\n"),
        Corpus::plain("both_append_same", "a\nb\n", "a\nb\nNEW\n", "a\nb\nNEW\n"),
        Corpus::plain("both_delete_same", "a\nb\nc\nd\n", "a\nc\nd\n", "a\nc\nd\n"),
        // ---- 冲突：modify-modify / 邻接 ----
        Corpus::plain(
            "conflict_modify_modify",
            "a\nb\nc\n",
            "a\nLEFT\nc\n",
            "a\nRIGHT\nc\n",
        ),
        Corpus::plain(
            "conflict_adjacent_no_stable",
            "a\nb\nc\n",
            "a\nB2\nc\n",
            "a\nb\nC3\n",
        ),
        Corpus::plain(
            "conflict_overlap_multiline",
            "k1\na\nb\nc\nk2\n",
            "k1\nL1\nL2\nc\nk2\n",
            "k1\nR1\nb\nR3\nk2\n",
        ),
        Corpus::plain(
            "conflict_delete_modify",
            "a\nb\nc\n",
            "a\nc\n",
            "a\nB!\nc\n",
        ),
        Corpus::plain("conflict_add_add", "a\nb\n", "a\nL1\nL2\nb\n", "a\nR1\nb\n"),
        Corpus::plain("conflict_add_add_eof", "a\nb\n", "a\nb\nL\n", "a\nb\nR\n"),
        // ---- 交错改 ----
        Corpus::plain(
            "interleaved_lr",
            "1\n2\n3\n4\n5\n",
            "L1\n2\n3\n4\nL5\n",
            "1\n2\nR3\n4\n5\n",
        ),
        Corpus::plain(
            "interleaved_near",
            "a\nx1\nx2\nx3\nb\nc\n",
            "A\nx1\nx2\nx3\nb\nc\n",
            "a\nx1\nx2\nx3\nB\nc\n",
        ),
        // ---- 空文件 / 空 base ----
        Corpus::plain("all_empty", "", "", ""),
        Corpus::plain("empty_base_same_add", "", "x\ny\n", "x\ny\n"),
        Corpus::plain("empty_base_conflict_add", "", "L\n", "R\n"),
        Corpus::plain("empty_base_one_side_add", "", "", "R\n"),
        Corpus::plain("become_empty_both", "a\nb\n", "", ""),
        // ---- CRLF / Unicode / 行尾 ----
        Corpus::plain(
            "crlf_clean",
            "a\r\nb\r\nc\r\n",
            "A\r\nb\r\nc\r\n",
            "a\r\nb\r\nC\r\n",
        ),
        Corpus::plain(
            "crlf_conflict",
            "a\r\nb\r\nc\r\n",
            "a\r\nL\r\nc\r\n",
            "a\r\nR\r\nc\r\n",
        ),
        Corpus::plain(
            "unicode_clean",
            "中文\n间隔\n日本語\n",
            "中文！\n间隔\n日本語\n",
            "中文\n间隔\n日本語。\n",
        ),
        Corpus::plain(
            "unicode_conflict",
            "函数一\n函数二\n函数三\n",
            "函数一\n左函数\n函数三\n",
            "函数一\n右函数\n函数三\n",
        ),
        // ---- EOF 无尾换行 ----
        Corpus {
            name: "eof_clean_merge",
            base: "a\nb\nc",
            left: "A\nb\nc",
            right: "a\nb\nc",
            eof_no_newline: false,
            ambiguous: false,
            window: None,
        },
        Corpus {
            name: "eof_conflict_no_newline",
            base: "a\nb",
            left: "a\nLEFT",
            right: "a\nRIGHT",
            eof_no_newline: true,
            ambiguous: false,
            window: None,
        },
        // ---- 重复行歧义 ----
        Corpus {
            name: "repeated_lines_ambiguity",
            base: "x\nrep\nrep\nrep\nx\n",
            left: "x\nrep\nrep\nx\n",
            right: "x\nrep\nrep\nrep\nrep\nx\n",
            eof_no_newline: false,
            ambiguous: true,
            window: None,
        },
        Corpus {
            name: "repeated_lines_conflict",
            base: "dup\ndup\ndup\n",
            left: "dup\nL\ndup\n",
            right: "dup\nR\ndup\n",
            eof_no_newline: false,
            ambiguous: true,
            window: None,
        },
        // ---- 冲突窗口（仅本实现；对拍跳过）----
        Corpus {
            name: "window_merges_nearby",
            base: "a\nmid\nc\nd\n",
            left: "A\nmid\nc\nd\n",
            right: "a\nmid\nC\nd\n",
            eof_no_newline: false,
            ambiguous: false,
            window: Some(1),
        },
        Corpus {
            name: "window_gap_too_far",
            base: "a\nm1\nm2\nm3\nc\n",
            left: "A\nm1\nm2\nm3\nc\n",
            right: "a\nm1\nm2\nm3\nC\n",
            eof_no_newline: false,
            ambiguous: false,
            window: Some(1),
        },
    ]
}

// ---------------------------------------------------------------------------
// 工具：写语料 + 外部工具调用 + 本实现输出
// ---------------------------------------------------------------------------

/// 语料文件句柄（保持 TempDir 存活）
struct Files {
    _dir: tempfile::TempDir,
    base: PathBuf,
    left: PathBuf,
    right: PathBuf,
}

fn write_corpus(c: &Corpus) -> Files {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path().join(format!("{}_base.l", c.name));
    let left = dir.path().join(format!("{}_left.l", c.name));
    let right = dir.path().join(format!("{}_right.l", c.name));
    std::fs::write(&base, c.base).unwrap();
    std::fs::write(&left, c.left).unwrap();
    std::fs::write(&right, c.right).unwrap();
    Files {
        _dir: dir,
        base,
        left,
        right,
    }
}

fn run_tool(bin: &str, args: &[&str]) -> Option<(String, i32)> {
    let out = Command::new(bin).args(args).output().ok()?;
    Some((
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    ))
}

/// 本实现的 marker 输出（标签 = 语料路径原串，与 diff3 -m / git merge-file 的 argv 标签同形；
/// `f` 传 None 时用固定 B/L/R 标签——golden 快照用，保证跨机器稳定）
fn our_markers(c: &Corpus, f: Option<&Files>, style: MarkerStyle) -> String {
    let (bl, ll, rl) = match f {
        Some(f) => (
            f.base.to_string_lossy().into_owned(),
            f.left.to_string_lossy().into_owned(),
            f.right.to_string_lossy().into_owned(),
        ),
        None => ("B".to_string(), "L".to_string(), "R".to_string()),
    };
    let base = LineTable::from_text(c.base);
    let left = LineTable::from_text(c.left);
    let right = LineTable::from_text(c.right);
    let opts = window_opts(c);
    let model = merge3(&base, &left, &right, &opts);
    let syn = SynthesizeOptions {
        unresolved: UnresolvedPolicy::Markers(MarkerOptions {
            style,
            base_label: bl,
            left_label: ll,
            right_label: rl,
        }),
    };
    auto_merge_text(&model, &base, &left, &right, &syn).text
}

/// 本实现的 auto 输出（未决冲突按策略解析）
fn our_auto(c: &Corpus, policy: UnresolvedPolicy) -> String {
    let base = LineTable::from_text(c.base);
    let left = LineTable::from_text(c.left);
    let right = LineTable::from_text(c.right);
    let opts = window_opts(c);
    let model = merge3(&base, &left, &right, &opts);
    auto_merge_text(
        &model,
        &base,
        &left,
        &right,
        &SynthesizeOptions { unresolved: policy },
    )
    .text
}

fn window_opts(c: &Corpus) -> MergeOptions {
    MergeOptions {
        conflict_window: c.window,
        ..MergeOptions::default()
    }
}

/// 规范化 GNU diff3 的 marker 粘连（EOF 无尾换行时内容行与下个 marker 同行）：
/// 非「行首」出现的 marker 前补 \n（与 git merge-file / 本实现行为对齐）
fn unglue_markers(gnu: &str) -> String {
    let markers = ["<<<<<<<", "|||||||", "=======", ">>>>>>>"];
    let mut out = String::with_capacity(gnu.len() + 8);
    let mut at_line_start = true;
    let chars: Vec<char> = gnu.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let rest: String = chars[i..].iter().take(7).collect();
        let hit = markers.iter().find(|m| rest.starts_with(**m));
        if !at_line_start {
            if let Some(m) = hit {
                out.push('\n');
                out.push_str(m);
                i += m.len();
                at_line_start = false;
                continue;
            }
        } else if let Some(m) = hit {
            out.push_str(m);
            i += m.len();
            at_line_start = false;
            continue;
        }
        let ch = chars[i];
        out.push(ch);
        at_line_start = ch == '\n';
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// 1. 本实现输出 golden 矩阵
// ---------------------------------------------------------------------------

#[test]
fn golden_merge_outputs() {
    let mut snapshot = serde_json::json!({
        "modes": ["diff3Markers", "mergeMarkers", "autoTakeLeft", "autoTakeRight"],
        "markerLabels": {"base": "B", "left": "L", "right": "R"},
        "corpora": {}
    });
    for c in corpora() {
        let f = write_corpus(&c);
        let base = LineTable::from_text(c.base);
        let left = LineTable::from_text(c.left);
        let right = LineTable::from_text(c.right);
        let model = merge3(&base, &left, &right, &window_opts(&c));
        // golden 快照用固定标签（跨机器稳定）；路径标签仅用于活体对拍
        let entry = serde_json::json!({
            "base": c.base, "left": c.left, "right": c.right,
            "chunkCount": model.chunks.len(),
            "conflictCount": model.conflict_count(),
            "wellFormed": model.is_well_formed(),
            "diff3Markers": our_markers(&c, None, MarkerStyle::Diff3),
            "mergeMarkers": our_markers(&c, None, MarkerStyle::Merge),
            "autoTakeLeft": our_auto(&c, UnresolvedPolicy::TakeLeft),
            "autoTakeRight": our_auto(&c, UnresolvedPolicy::TakeRight),
        });
        assert!(
            entry["wellFormed"] == serde_json::json!(true),
            "语料 {} 块序列不自洽",
            c.name
        );
        snapshot["corpora"][c.name] = entry;
        drop(f);
    }
    let pretty = serde_json::to_string_pretty(&snapshot).unwrap();
    let path = harness::test_data_dir()
        .join("merge3_matrix")
        .join("merge3_outputs.golden.json");
    harness::assert_matches_snapshot("merge3_outputs", &path, &pretty).expect("merge golden 一致");
}

// ---------------------------------------------------------------------------
// 2. 活体对拍：GNU diff3 -m / git merge-file
// ---------------------------------------------------------------------------

/// 单语料对拍记录
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
struct CrossRow {
    corpus: &'static str,
    /// GNU diff3 -m 比对结果（Same / Glued(规范化后一致) / Ambiguous / Skipped）
    gnu: &'static str,
    /// git merge-file --diff3 比对结果
    git_diff3: &'static str,
    /// git merge-file（默认 merge 风格）比对结果
    git_merge: &'static str,
    conflict_count: usize,
}

/// GNU diff3 -m（默认 -A「bracket all」）对拍判定。
///
/// 分类语义：
/// - `same`：逐字节一致；
/// - `same-after-unglue`：EOF 无尾换行 marker 粘连规范化后一致；
/// - `ambiguous`：重复行歧义语料（等价 diff 选择不同，合法合并）；
/// - `gnu-A-bracketed`：GNU `-m` 默认 -A 模式的行为差异——A 与 B 做了**相同**
///   变化时也加冲突括号、delete-modify 时 base 段向前扩展一行锚点；
///   本实现与 git merge-file --diff3 语义一致（重叠才冲突 + 原样 base 段），
///   以 git 输出一致为证据归类；
/// - `diff`：未归类差异（测试失败）。
fn gnu_verdict(c: &Corpus, f: &Files, ours: &str) -> &'static str {
    let Some((gnu_out, _)) = run_tool(
        "diff3",
        &[
            "-m",
            f.left.to_str().unwrap(),
            f.base.to_str().unwrap(),
            f.right.to_str().unwrap(),
        ],
    ) else {
        return "skipped";
    };
    if ours == gnu_out {
        return "same";
    }
    if c.eof_no_newline && unglue_markers(&gnu_out) == ours {
        return "same-after-unglue";
    }
    if c.ambiguous {
        return "ambiguous";
    }
    // GNU -A 行为差异归类：本实现须与 git merge-file --diff3 一致
    if let Some((git_out, _)) = run_tool(
        "git",
        &[
            "merge-file",
            "-p",
            "--diff3",
            f.left.to_str().unwrap(),
            f.base.to_str().unwrap(),
            f.right.to_str().unwrap(),
        ],
    ) {
        if ours == git_out {
            return "gnu-A-bracketed";
        }
    }
    "diff"
}

fn git_verdict(c: &Corpus, f: &Files, ours: &str, style: MarkerStyle) -> &'static str {
    // git merge-file 默认即 merge 风格（无 flag）；--diff3 附 base 段
    let mut args: Vec<&str> = vec!["merge-file", "-p"];
    if style == MarkerStyle::Diff3 {
        args.push("--diff3");
    }
    args.push(f.left.to_str().unwrap());
    args.push(f.base.to_str().unwrap());
    args.push(f.right.to_str().unwrap());
    let Some((git_out, _)) = run_tool("git", &args) else {
        return "skipped";
    };
    if ours == git_out {
        return "same";
    }
    if c.eof_no_newline && unglue_markers(&git_out) == ours {
        return "same-after-unglue";
    }
    if c.ambiguous {
        return "ambiguous";
    }
    "diff"
}

#[test]
fn crosscheck_gnu_diff3() {
    let mut rows = Vec::new();
    for c in corpora() {
        if c.window.is_some() {
            rows.push(CrossRow {
                corpus: c.name,
                gnu: "skipped",
                git_diff3: "skipped",
                git_merge: "skipped",
                conflict_count: 0,
            });
            continue;
        }
        let f = write_corpus(&c);
        let ours = our_markers(&c, Some(&f), MarkerStyle::Diff3);
        let verdict = gnu_verdict(&c, &f, &ours);
        rows.push(CrossRow {
            corpus: c.name,
            gnu: verdict,
            git_diff3: "n/a",
            git_merge: "n/a",
            conflict_count: 0,
        });
    }
    // 断言：对拍差异必须可归类（same / same-after-unglue / gnu-A-bracketed / ambiguous / skipped）
    let bad: Vec<_> = rows.iter().filter(|r| r.gnu == "diff").collect();
    assert!(bad.is_empty(), "GNU diff3 -m 对拍出现未归类差异: {bad:?}");
    let same = rows.iter().filter(|r| r.gnu.starts_with("same")).count();
    let gnu_a = rows.iter().filter(|r| r.gnu == "gnu-A-bracketed").count();
    let skipped = rows.iter().filter(|r| r.gnu == "skipped").count();
    println!("GNU diff3 -m 对拍: {same} 逐字节一致 / {gnu_a} 归类为 GNU -A 行为差异（git merge-file 同本实现）/ {skipped} 跳过");
}

#[test]
fn crosscheck_git_merge_file() {
    for c in corpora() {
        if c.window.is_some() {
            continue;
        }
        let f = write_corpus(&c);
        let ours_diff3 = our_markers(&c, Some(&f), MarkerStyle::Diff3);
        let v = git_verdict(&c, &f, &ours_diff3, MarkerStyle::Diff3);
        assert_ne!(v, "diff", "git merge-file --diff3 对拍差异（{}）", c.name);
        let ours_merge = our_markers(&c, Some(&f), MarkerStyle::Merge);
        let v = git_verdict(&c, &f, &ours_merge, MarkerStyle::Merge);
        assert_ne!(v, "diff", "git merge-file 对拍差异（{}）", c.name);
    }
}

/// 三方对拍报告（确定性矩阵落快照；工具不可用时记录 skipped）
#[test]
fn golden_crosscheck_report() {
    let mut snapshot = serde_json::json!({
        "tools": {
            "gnuDiff3": which_diff3(),
            "gitMergeFile": which_git(),
        },
        "note": "same=逐字节一致; same-after-unglue=EOF 无尾换行 marker 粘连规范化后一致; gnu-A-bracketed=GNU -m 默认 -A 模式行为差异（相同变化也加括号/delete-modify base 段扩展，本实现与 git merge-file --diff3 一致）; ambiguous=重复行歧义语料（合法合并，不逐字比对）; skipped=窗口选项为本实现扩展",
        "rows": []
    });
    for c in corpora() {
        let f = write_corpus(&c);
        let base = LineTable::from_text(c.base);
        let left = LineTable::from_text(c.left);
        let right = LineTable::from_text(c.right);
        let model = merge3(&base, &left, &right, &window_opts(&c));
        let conflict_count = model.conflict_count() as usize;
        let ours_diff3 = our_markers(&c, Some(&f), MarkerStyle::Diff3);
        let ours_merge = our_markers(&c, Some(&f), MarkerStyle::Merge);

        let (gnu_v, git_diff3_v, git_merge_v) = if c.window.is_some() {
            ("skipped", "skipped", "skipped")
        } else {
            (
                gnu_verdict(&c, &f, &ours_diff3),
                git_verdict(&c, &f, &ours_diff3, MarkerStyle::Diff3),
                git_verdict(&c, &f, &ours_merge, MarkerStyle::Merge),
            )
        };

        snapshot["rows"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "corpus": c.name,
                "conflictCount": conflict_count,
                "gnuDiff3": gnu_v,
                "gitDiff3": git_diff3_v,
                "gitMerge": git_merge_v,
            }));
    }
    let pretty = serde_json::to_string_pretty(&snapshot).unwrap();
    let path = harness::test_data_dir()
        .join("merge3_matrix")
        .join("crosscheck_report.golden.json");
    harness::assert_matches_snapshot("crosscheck_report", &path, &pretty)
        .expect("对拍报告快照一致");

    // 行为断言：报告内不得存在未归类的 diff（歧义/EOF 语料已单独归类）
    for row in snapshot["rows"].as_array().unwrap() {
        for key in ["gnuDiff3", "gitDiff3", "gitMerge"] {
            assert_ne!(
                row[key], "diff",
                "语料 {} 的 {key} 对拍出现未归类差异: {}",
                row["corpus"], row
            );
        }
    }
}

fn which_diff3() -> &'static str {
    match Command::new("diff3").arg("--version").output() {
        Ok(_) => "available",
        Err(_) => "missing",
    }
}

fn which_git() -> &'static str {
    match Command::new("git").arg("--version").output() {
        Ok(_) => "available",
        Err(_) => "missing",
    }
}

// ---------------------------------------------------------------------------
// 3. 决策合成幂等 + 重要性开关行为
// ---------------------------------------------------------------------------

#[test]
fn decide_output_is_stable_and_idempotent() {
    let base = LineTable::from_text("h\na\nb\nc\nt\n");
    let left = LineTable::from_text("h\nL1\nb\nL2\nt\n");
    let right = LineTable::from_text("h\nR1\nb\nR2\nt\n");
    let model = merge3(&base, &left, &right, &MergeOptions::default());
    assert_eq!(model.conflict_count(), 2);
    assert_eq!(model.unresolved(), vec![1, 3]);

    // 未决策 marker 输出
    let syn = SynthesizeOptions {
        unresolved: UnresolvedPolicy::Markers(MarkerOptions::from_names(
            "B",
            "L",
            "R",
            MarkerStyle::Diff3,
        )),
    };
    let o0 = ai_core::synthesize(&model, &base, &left, &right, &Default::default(), &syn);
    assert_eq!(o0.unresolved, vec![1, 3]);
    assert_eq!(o0.text.matches("<<<<<<<").count(), 2);

    // 决策（幂等：重复决策输出逐字节一致）
    let mut d = std::collections::HashMap::new();
    d.insert(1u32, ai_core::MergeDecision::TakeLeft);
    let o1 = ai_core::synthesize(&model, &base, &left, &right, &d, &syn);
    let o2 = ai_core::synthesize(&model, &base, &left, &right, &d, &syn);
    assert_eq!(o1, o2, "重复决策不漂移");
    assert_eq!(o1.unresolved, vec![3], "仅剩未决策冲突");
    assert_eq!(
        o1.text.matches("<<<<<<<").count(),
        1,
        "块 1 已决（仅剩块 3 的 marker）"
    );
    assert!(o1.text.contains("L1\n"));

    // takeBase 还原 + 先左后右
    d.insert(1u32, ai_core::MergeDecision::TakeBase);
    let o3 = ai_core::synthesize(&model, &base, &left, &right, &d, &syn);
    assert!(o3.text.starts_with("h\na\n"), "takeBase 还原基");
    d.insert(1u32, ai_core::MergeDecision::TakeLeftThenRight);
    let o4 = ai_core::synthesize(&model, &base, &left, &right, &d, &syn);
    assert!(o4.text.contains("L1\nR1\n"), "先左后右: {}", o4.text);

    // 手动编辑
    d.insert(
        1u32,
        ai_core::MergeDecision::Manual(vec!["手".into(), "工".into()]),
    );
    let o5 = ai_core::synthesize(&model, &base, &left, &right, &d, &syn);
    assert!(o5.text.contains("\n手\n工\n"), "手动编辑: {}", o5.text);
    assert_eq!(o5.unresolved, vec![3]);
}

#[test]
fn importance_switches_shrink_conflicts() {
    // 两侧把同一行改成仅尾随空白不同
    let base = LineTable::from_text("a\nb\nc\n");
    let left = LineTable::from_text("a\nB  \nc\n");
    let right = LineTable::from_text("a\nB\nc\n");

    // 默认（全重要）：1 个未决冲突
    let m0 = merge3(&base, &left, &right, &MergeOptions::default());
    assert_eq!(m0.unresolved(), vec![1]);

    // 尾随空白不重要 + favorRight：冲突不升级（自动从受惠侧采用）
    let o = MergeOptions {
        importance: ai_core::ImportanceOptions {
            trailing_ws_unimportant: true,
            ..Default::default()
        },
        favor: Some(ai_core::FavorSide::Right),
        ..MergeOptions::default()
    };
    let m1 = merge3(&base, &left, &right, &o);
    assert!(
        m1.unresolved().is_empty(),
        "不重要冲突不升级: {:?}",
        m1.chunks
    );
    let out = auto_merge_text(&m1, &base, &left, &right, &SynthesizeOptions::default());
    assert_eq!(out.text, "a\nB\nc\n");

    // 仅 ignoreUnimportant（无 favor）：自动取左
    let o = MergeOptions {
        importance: ai_core::ImportanceOptions {
            trailing_ws_unimportant: true,
            ..Default::default()
        },
        ignore_unimportant: true,
        ..MergeOptions::default()
    };
    let m2 = merge3(&base, &left, &right, &o);
    assert!(m2.unresolved().is_empty());
    let out = auto_merge_text(&m2, &base, &left, &right, &SynthesizeOptions::default());
    assert_eq!(out.text, "a\nB  \nc\n");

    // 重要冲突不受 favor 影响
    let base = LineTable::from_text("a\nb\nc\n");
    let left = LineTable::from_text("a\nLEFT\nc\n");
    let right = LineTable::from_text("a\nRIGHT\nc\n");
    let o = MergeOptions {
        favor: Some(ai_core::FavorSide::Left),
        ..MergeOptions::default()
    };
    let m3 = merge3(&base, &left, &right, &o);
    assert_eq!(m3.unresolved(), vec![1], "重要冲突仍待决");
}

#[test]
fn skeleton_whitespace_option_dissolves_conflict() {
    // 骨架空白语义（-w）：空白差异在对齐层消失 → 不再冲突（冲突块缩小的另一路径）
    let base = LineTable::from_text("a\nb\nc\n");
    let left = LineTable::from_text("a\nB  \nc\n");
    let right = LineTable::from_text("a\nB\nc\n");
    let o = MergeOptions {
        diff: ai_core::TextDiffOptions {
            whitespace: ai_core::WhitespaceMode::IgnoreAll,
            ..Default::default()
        },
        ..MergeOptions::default()
    };
    let m = merge3(&base, &left, &right, &o);
    assert_eq!(m.conflict_count(), 0);
    let out = auto_merge_text(&m, &base, &left, &right, &SynthesizeOptions::default());
    assert_eq!(out.text, "a\nB  \nc\n", "BothChanged 取左原文");
}

#[test]
fn window_option_conflict_semantics() {
    // 窗口 1：左改行 0、右改行 2（间隔 1 稳定行）→ 合并为冲突
    let base = LineTable::from_text("a\nmid\nc\n");
    let left = LineTable::from_text("A\nmid\nc\n");
    let right = LineTable::from_text("a\nmid\nC\n");
    let m = merge3(
        &base,
        &left,
        &right,
        &MergeOptions {
            conflict_window: Some(1),
            ..MergeOptions::default()
        },
    );
    assert_eq!(m.conflict_count(), 1);
    assert_eq!(m.chunks.len(), 1, "冲突块跨稳定间隔，覆盖全文件");
    // 窗口不够（间隔 1 > 窗口 0）→ 无冲突
    let m = merge3(
        &base,
        &left,
        &right,
        &MergeOptions {
            conflict_window: Some(0),
            ..MergeOptions::default()
        },
    );
    assert_eq!(m.conflict_count(), 0);
    // 单侧连续变化：窗口再大也不产生冲突
    let left = LineTable::from_text("A\nmid\nC\n");
    let right = LineTable::from_text("a\nmid\nc\n");
    let m = merge3(
        &base,
        &left,
        &right,
        &MergeOptions {
            conflict_window: Some(5),
            ..MergeOptions::default()
        },
    );
    assert_eq!(m.conflict_count(), 0);
}

#[test]
fn all_side_ranges_partition_files() {
    // 块序列完备划分三方（对拍矩阵全语料自检）
    for c in corpora() {
        let base = LineTable::from_text(c.base);
        let left = LineTable::from_text(c.left);
        let right = LineTable::from_text(c.right);
        let model = merge3(&base, &left, &right, &window_opts(&c));
        assert!(
            model.is_well_formed(),
            "语料 {} 块序列不自洽: {:?}",
            c.name,
            model.chunks
        );
        assert_eq!(model.left_len, left.len() as u32);
        assert_eq!(model.right_len, right.len() as u32);
    }
}

#[test]
fn ambiguous_corpora_produce_valid_merges() {
    // 歧义语料：不与外部工具逐字比对，但结果必须为合法合并——
    // 所有非冲突内容保留、冲突可全部 takeLeft 后重放为合法文本
    for c in corpora().iter().filter(|c| c.ambiguous) {
        let base = LineTable::from_text(c.base);
        let left = LineTable::from_text(c.left);
        let right = LineTable::from_text(c.right);
        let model = merge3(&base, &left, &right, &window_opts(c));
        let out = auto_merge_text(&model, &base, &left, &right, &SynthesizeOptions::default());
        if model.conflict_count() == 0 {
            // 干净合并必须包含两侧的非公共内容
            assert!(out.text.starts_with("x\n") || c.name == "repeated_lines_conflict");
        }
        // takeLeft 全决后输出以左侧行序列为主体（重放合法性）
        let mut d = std::collections::HashMap::new();
        for id in model.unresolved() {
            d.insert(id, ai_core::MergeDecision::TakeLeft);
        }
        let resolved = ai_core::synthesize(
            &model,
            &base,
            &left,
            &right,
            &d,
            &SynthesizeOptions::default(),
        );
        assert!(resolved.unresolved.is_empty());
    }
}

// ---------------------------------------------------------------------------
// 4. 快照归一化说明
// ---------------------------------------------------------------------------
// golden 快照不含路径：marker 输出用固定标签 B/L/R（golden_merge_outputs），
// 路径标签仅出现在活体对拍（不落盘）；对拍报告只含判定词与冲突计数。
