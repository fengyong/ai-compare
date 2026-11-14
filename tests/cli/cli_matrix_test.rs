//! P3 CLI 退出码矩阵 + aicc 等待型包装行为（docs/04 §P3 验收要点 / FR-CLI-004）。
//!
//! 退出码契约（lib.rs 常量文档的兼容分层）：
//! `0` 相同/成功；`1` 不同（diff 层）；`2` 错误；
//! `11` 二进制差异（quick size/crc/binary）；`12` 相似（仅不重要差异，rules）；
//! `13` 基于规则的差异（rules）；`14` 冲突（P7 预留常量）。

use std::process::{Command, Output};

fn aic() -> std::path::PathBuf {
    ai_compare_tests::cli_bin("aic")
}

fn aicc() -> std::path::PathBuf {
    ai_compare_tests::cli_bin("aicc")
}

/// 运行 CLI（stderr 合并进 stdout 便于断言错误信息）
fn run(bin: &std::path::Path, args: &[&str]) -> Output {
    Command::new(bin)
        .args(args)
        .output()
        .expect("CLI 进程启动")
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().expect("CLI 应正常退出（非信号）")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 语料句柄（保持 TempDir 存活）
struct Fixtures {
    _dir: tempfile::TempDir,
    same_l: std::path::PathBuf,
    same_r: std::path::PathBuf,
    text_diff_l: std::path::PathBuf,
    text_diff_r: std::path::PathBuf,
    trailing_l: std::path::PathBuf,
    trailing_r: std::path::PathBuf,
    case_l: std::path::PathBuf,
    case_r: std::path::PathBuf,
    bin_same_l: std::path::PathBuf,
    bin_same_r: std::path::PathBuf,
    bin_diff_l: std::path::PathBuf,
    bin_diff_r: std::path::PathBuf,
}

fn fixtures() -> Fixtures {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let p = |name: &str| root.join(name);
    std::fs::write(p("same_l.txt"), "alpha\nbeta\ngamma\n").unwrap();
    std::fs::write(p("same_r.txt"), "alpha\nbeta\ngamma\n").unwrap();
    std::fs::write(p("diff_l.txt"), "alpha\nbeta\ngamma\n").unwrap();
    std::fs::write(p("diff_r.txt"), "alpha\nOMEGA\ngamma\n").unwrap();
    std::fs::write(p("trail_l.txt"), "alpha\nbeta   \ngamma\n").unwrap();
    std::fs::write(p("trail_r.txt"), "alpha\nbeta\ngamma\n").unwrap();
    std::fs::write(p("case_l.txt"), "alpha\nBETA\ngamma\n").unwrap();
    std::fs::write(p("case_r.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let bytes: Vec<u8> = (0..=63u8).collect();
    std::fs::write(p("bin_same_l.bin"), &bytes).unwrap();
    std::fs::write(p("bin_same_r.bin"), &bytes).unwrap();
    let mut rb = bytes.clone();
    rb[10] ^= 0xff;
    std::fs::write(p("bin_diff_l.bin"), &bytes).unwrap();
    std::fs::write(p("bin_diff_r.bin"), &rb).unwrap();
    Fixtures {
        _dir: dir,
        same_l: p("same_l.txt"),
        same_r: p("same_r.txt"),
        text_diff_l: p("diff_l.txt"),
        text_diff_r: p("diff_r.txt"),
        trailing_l: p("trail_l.txt"),
        trailing_r: p("trail_r.txt"),
        case_l: p("case_l.txt"),
        case_r: p("case_r.txt"),
        bin_same_l: p("bin_same_l.bin"),
        bin_same_r: p("bin_same_r.bin"),
        bin_diff_l: p("bin_diff_l.bin"),
        bin_diff_r: p("bin_diff_r.bin"),
    }
}

fn s(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// aic diff 退出码矩阵（P0 层 0/1/2 不变）
// ---------------------------------------------------------------------------

#[test]
fn diff_exit_matrix() {
    let f = fixtures();
    // 相同 -> 0，且无输出
    let out = run(&aic(), &["diff", &s(&f.same_l), &s(&f.same_r)]);
    assert_eq!(exit_code(&out), 0, "相同文件 diff -> 0");
    assert_eq!(stdout(&out), "", "相同文件无 diff 输出");
    // 不同 -> 1，输出含 +/- 行
    let out = run(&aic(), &["diff", &s(&f.text_diff_l), &s(&f.text_diff_r)]);
    assert_eq!(exit_code(&out), 1, "不同文件 diff -> 1");
    let text = stdout(&out);
    assert!(text.contains("-beta") && text.contains("+OMEGA"), "输出含删/增行: {text}");
    assert!(text.starts_with("--- "), "unified 文件头");
    // 错误（文件不存在）-> 2
    let out = run(&aic(), &["diff", "/nonexistent/aic/x1", &s(&f.same_r)]);
    assert_eq!(exit_code(&out), 2, "路径不存在 -> 2");
    // 错误（未知参数）-> 2
    let out = run(&aic(), &["diff", "--nope", "a", "b"]);
    assert_eq!(exit_code(&out), 2, "未知参数 -> 2");
}

// ---------------------------------------------------------------------------
// aic quick 四模式退出码矩阵（P3 细分层 11/12/13）
// ---------------------------------------------------------------------------

#[test]
fn quick_exit_matrix_binary_modes() {
    let f = fixtures();
    // size 模式：同尺寸不同内容不比对内容 -> 0（size 语义）
    let out = run(&aic(), &["quick", &s(&f.bin_diff_l), &s(&f.bin_diff_r), "--mode", "size"]);
    assert_eq!(exit_code(&out), 0, "size 模式同尺寸 -> 0（不比内容）");
    // crc/binary 模式：内容差异 -> 11（二进制差异细分）
    for mode in ["crc", "binary"] {
        let out = run(&aic(), &["quick", &s(&f.bin_same_l), &s(&f.bin_same_r), "--mode", mode]);
        assert_eq!(exit_code(&out), 0, "{mode} 模式相同 -> 0");
        let out = run(&aic(), &["quick", &s(&f.bin_diff_l), &s(&f.bin_diff_r), "--mode", mode]);
        assert_eq!(exit_code(&out), 11, "{mode} 模式差异 -> 11（二进制差异）");
    }
}

#[test]
fn quick_exit_matrix_rules_mode() {
    let f = fixtures();
    // 规则相同 -> 0
    let out = run(&aic(), &["quick", &s(&f.same_l), &s(&f.same_r), "--mode", "rules"]);
    assert_eq!(exit_code(&out), 0);
    // 重要差异 -> 13（默认重要性模型：内容差异均重要）
    let out = run(&aic(), &["quick", &s(&f.text_diff_l), &s(&f.text_diff_r), "--mode", "rules"]);
    assert_eq!(exit_code(&out), 13, "规则差异 -> 13");
    // 二进制文件回退二进制判等：差异 -> 11
    let out = run(&aic(), &["quick", &s(&f.bin_diff_l), &s(&f.bin_diff_r), "--mode", "rules"]);
    assert_eq!(exit_code(&out), 11, "非文本差异回退二进制 -> 11");
    let out = run(&aic(), &["quick", &s(&f.bin_same_l), &s(&f.bin_same_r), "--mode", "rules"]);
    assert_eq!(exit_code(&out), 0, "非文本相同 -> 0");
    // 仅不重要差异（尾随空白 + --ignore-whitespace）-> 12（相似）
    let out = run(
        &aic(),
        &["quick", &s(&f.trailing_l), &s(&f.trailing_r), "--mode", "rules", "--ignore-whitespace"],
    );
    assert_eq!(exit_code(&out), 12, "仅尾随空白差异（不重要） -> 12 相似");
    // 同语料不加开关：尾随空白重要 -> 13
    let out = run(&aic(), &["quick", &s(&f.trailing_l), &s(&f.trailing_r), "--mode", "rules"]);
    assert_eq!(exit_code(&out), 13);
    // 仅大小写差异 + --ignore-case（大小写不重要） -> 12
    let out = run(&aic(), &["quick", &s(&f.case_l), &s(&f.case_r), "--mode", "rules", "--ignore-case"]);
    assert_eq!(exit_code(&out), 12, "仅大小写差异（--ignore-case 后不重要） -> 12");
    // 不加开关：大小写重要 -> 13
    let out = run(&aic(), &["quick", &s(&f.case_l), &s(&f.case_r), "--mode", "rules"]);
    assert_eq!(exit_code(&out), 13);
    // 错误 -> 2
    let out = run(&aic(), &["quick", "/nonexistent/aic/y1", &s(&f.same_r), "--mode", "rules"]);
    assert_eq!(exit_code(&out), 2);
}

#[test]
fn quick_rules_directory_aggregation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    // 仅尾随空白差异的文件对（rules --ignore-whitespace 下不重要 -> 相似 12）
    let dl = root.join("dl");
    let dr = root.join("dr");
    std::fs::create_dir_all(dl.join("sub")).unwrap();
    std::fs::create_dir_all(dr.join("sub")).unwrap();
    std::fs::write(dl.join("sub").join("trail.txt"), "keep\n").unwrap();
    std::fs::write(dr.join("sub").join("trail.txt"), "keep   \n").unwrap();
    let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
    let out = run(&aic(), &["quick", &s(&dl), &s(&dr), "--mode", "rules", "--ignore-whitespace"]);
    assert_eq!(exit_code(&out), 12, "目录仅不重要差异 -> 12 相似: {}", stdout(&out));
    // 默认模型（尾随空白重要）-> 13
    let out = run(&aic(), &["quick", &s(&dl), &s(&dr), "--mode", "rules"]);
    assert_eq!(exit_code(&out), 13, "目录重要差异 -> 13");
    // 相同目录 -> 0
    let out = run(&aic(), &["quick", &s(&dl), &s(&dl), "--mode", "rules"]);
    assert_eq!(exit_code(&out), 0, "目录相同 -> 0");
}

#[test]
fn quick_defaults_and_error_paths() {
    let f = fixtures();
    // 缺省模式 = crc
    let out = run(&aic(), &["quick", &s(&f.bin_diff_l), &s(&f.bin_diff_r)]);
    assert_eq!(exit_code(&out), 11, "缺省 mode=crc，差异 -> 11");
    // 非法 mode -> 2
    let out = run(&aic(), &["quick", "a", "b", "--mode", "nope"]);
    assert_eq!(exit_code(&out), 2);
    // 无参数 -> 2 + 用法提示
    let out = run(&aic(), &["quick"]);
    assert_eq!(exit_code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("用法"), "错误时打印用法");
    // help -> 0
    let out = run(&aic(), &["--help"]);
    assert_eq!(exit_code(&out), 0);
}

// ---------------------------------------------------------------------------
// aic diff 算法/规范化开关（映射 TextDiffOptions）
// ---------------------------------------------------------------------------

#[test]
fn diff_algo_switches_change_or_keep_semantics() {
    let f = fixtures();
    // 四种算法对简单替换语料都产出差异（退出码 1），输出 hunk 数一致
    for algo in ["standard", "myers", "patience", "histogram"] {
        let out = run(&aic(), &["diff", "--algo", algo, &s(&f.text_diff_l), &s(&f.text_diff_r)]);
        assert_eq!(exit_code(&out), 1, "--algo {algo} 差异 -> 1");
        let text = stdout(&out);
        assert_eq!(text.matches("@@ -").count(), 1, "--algo {algo} 单 hunk");
    }
    // 非法算法 -> 2
    let out = run(&aic(), &["diff", "--algo", "nope", "a", "b"]);
    assert_eq!(exit_code(&out), 2);
    // --ignore-whitespace：空白差异判同 -> 0
    let out = run(&aic(), &["diff", "--ignore-whitespace", &s(&f.trailing_l), &s(&f.trailing_r)]);
    assert_eq!(exit_code(&out), 0, "--ignore-whitespace 下空白差异 -> 0");
    // 同语料不加开关 -> 1
    let out = run(&aic(), &["diff", &s(&f.trailing_l), &s(&f.trailing_r)]);
    assert_eq!(exit_code(&out), 1);
    // --ignore-case：大小写差异判同 -> 0
    let out = run(&aic(), &["diff", "--ignore-case", &s(&f.case_l), &s(&f.case_r)]);
    assert_eq!(exit_code(&out), 0, "--ignore-case 下大小写差异 -> 0");
    // -w 短选项等价
    let out = run(&aic(), &["diff", "-w", &s(&f.trailing_l), &s(&f.trailing_r)]);
    assert_eq!(exit_code(&out), 0);
}

// ---------------------------------------------------------------------------
// aicc 等待型包装行为（FR-CLI-001 双形态）
// ---------------------------------------------------------------------------

#[test]
fn aicc_wraps_and_propagates_exit_codes() {
    let f = fixtures();
    // 裸双路径 = diff：相同 -> 0
    let out = run(&aicc(), &[&s(&f.same_l), &s(&f.same_r)]);
    assert_eq!(exit_code(&out), 0, "aicc 裸双路径相同 -> 0");
    // 不同 -> 1（等待子进程退出并透传）
    let out = run(&aicc(), &[&s(&f.text_diff_l), &s(&f.text_diff_r)]);
    assert_eq!(exit_code(&out), 1, "aicc 裸双路径不同 -> 1（透传）");
    let text = stdout(&out);
    assert!(text.contains("-beta") && text.contains("+OMEGA"), "aicc 输出 diff: {text}");
    // 透传细分码：quick 语义 + 11
    let out = run(&aicc(), &["quick", &s(&f.bin_diff_l), &s(&f.bin_diff_r), "--mode", "binary"]);
    assert_eq!(exit_code(&out), 11, "aicc 透传 quick 细分码 11");
    // 错误透传 -> 2
    let out = run(&aicc(), &["diff", "/nonexistent/aic/z1", &s(&f.same_r)]);
    assert_eq!(exit_code(&out), 2, "aicc 透传错误码 2");
}

#[test]
fn aicc_accepts_bc_gui_switch_subset() {
    let f = fixtures();
    // FR-CLI-003 子集（-fv/--title/--ro/--filters）接受并忽略
    let out = run(
        &aicc(),
        &[
            "--fileviewer=Text Compare",
            "--title1=LEFT",
            "--title2=RIGHT",
            "--ro",
            "--filters=*.cpp;*.h",
            &s(&f.text_diff_l),
            &s(&f.text_diff_r),
        ],
    );
    assert_eq!(exit_code(&out), 1, "GUI 开关被剥除后按 diff 执行 -> 1");
}

#[test]
fn aicc_mergetool_four_path_convention() {
    let f = fixtures();
    // git mergetool 约定：BASE LOCAL REMOTE [MERGED] -> 以 LOCAL REMOTE 两路 diff
    // （三向合并视图属 P7；此处验证调用约定与退出码语义）
    let out = run(
        &aicc(),
        &[&s(&f.same_l), &s(&f.text_diff_l), &s(&f.text_diff_r), &s(&f.same_r)],
    );
    assert_eq!(exit_code(&out), 1, "mergetool 四路径 -> diff(LOCAL, REMOTE) -> 1");
    let text = stdout(&out);
    assert!(text.contains("-beta") && text.contains("+OMEGA"), "输出应为 LOCAL/REMOTE 的 diff: {text}");
    assert!(!text.contains(&s(&f.same_l)), "BASE 不参与比较");
}

#[test]
fn aicc_no_args_prints_usage_and_errors() {
    let out = run(&aicc(), &[]);
    assert_eq!(exit_code(&out), 2);
    assert!(String::from_utf8_lossy(&out.stderr).contains("用法"), "打印用法");
}

// ---------------------------------------------------------------------------
// 退出码常量契约（编译期对齐文档：14 为 P7 预留，不产生于当前命令面）
// ---------------------------------------------------------------------------

#[test]
fn conflict_code_reserved_constant_documented() {
    // 14 仅由 P7 三向合并 /automerge 产生（docs/01 FR-CLI-004）；
    // 当前 CLI 全命令面不得返回 14 —— 遍历本矩阵语料的全部命令路径验证。
    let f = fixtures();
    let cases: Vec<Vec<String>> = vec![
        vec!["diff".into(), s(&f.text_diff_l)],
        vec!["quick".into(), s(&f.bin_diff_l), s(&f.bin_diff_r), "--mode".into(), "rules".into()],
        vec!["quick".into(), s(&f.bin_diff_l), s(&f.bin_diff_r)],
    ];
    for args in cases {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run(&aic(), &refs);
        assert_ne!(exit_code(&out), 14, "当前命令面不得返回冲突码: {args:?}");
    }
}

// ---------------------------------------------------------------------------
// aic merge（P7 三向合并）：退出码 0/1/2 + marker 输出 + --conflict-exit-code
// ---------------------------------------------------------------------------

/// 三方语料（独立于 fixtures 的 diff 语料）
struct MergeFixtures {
    _dir: tempfile::TempDir,
    base: std::path::PathBuf,
    left: std::path::PathBuf,
    right: std::path::PathBuf,
    /// 两侧同改（无冲突）
    same_base: std::path::PathBuf,
    same_left: std::path::PathBuf,
    same_right: std::path::PathBuf,
    /// 左改头右改尾（干净合并）
    clean_base: std::path::PathBuf,
    clean_left: std::path::PathBuf,
    clean_right: std::path::PathBuf,
}

fn merge_fixtures() -> MergeFixtures {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let p = |name: &str| root.join(name);
    std::fs::write(p("c_base.l"), "h\na\nb\nc\nt\n").unwrap();
    std::fs::write(p("c_left.l"), "h\nL1\nb\nL2\nt\n").unwrap();
    std::fs::write(p("c_right.l"), "h\nR1\nb\nR2\nt\n").unwrap();
    std::fs::write(p("s_base.l"), "a\nb\nc\n").unwrap();
    std::fs::write(p("s_left.l"), "a\nX\nc\n").unwrap();
    std::fs::write(p("s_right.l"), "a\nX\nc\n").unwrap();
    std::fs::write(p("k_base.l"), "1\n2\n3\n4\n5\n").unwrap();
    std::fs::write(p("k_left.l"), "K1\n2\n3\n4\n5\n").unwrap();
    std::fs::write(p("k_right.l"), "1\n2\n3\n4\nK5\n").unwrap();
    MergeFixtures {
        _dir: dir,
        base: p("c_base.l"),
        left: p("c_left.l"),
        right: p("c_right.l"),
        same_base: p("s_base.l"),
        same_left: p("s_left.l"),
        same_right: p("s_right.l"),
        clean_base: p("k_base.l"),
        clean_left: p("k_left.l"),
        clean_right: p("k_right.l"),
    }
}

/// 冲突码不得占用既有语义码（0 相同 / 2 错误 / 11 二进制差异 / 12 相似 / 13 规则差异），
/// 否则 VCS/CI 会把"有冲突"误判为其它结果。1 与 14 必须放行。
#[test]
fn merge_conflict_exit_code_rejects_reserved_codes() {
    let f = merge_fixtures();
    for reserved in ["0", "2", "11", "12", "13"] {
        let out = run(
            &aic(),
            &["merge", &s(&f.base), &s(&f.left), &s(&f.right), "--conflict-exit-code", reserved],
        );
        assert_eq!(exit_code(&out), 2, "保留码 {reserved} 应被拒绝");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("--conflict-exit-code"), "应说明拒绝原因: {err}");
    }
    for allowed in ["1", "14", "3", "200"] {
        let out = run(
            &aic(),
            &["merge", &s(&f.base), &s(&f.left), &s(&f.right), "--conflict-exit-code", allowed],
        );
        assert_eq!(exit_code(&out), allowed.parse::<i32>().unwrap(), "非保留码 {allowed} 应生效");
    }
}

#[test]
fn merge_exit_matrix() {
    let f = merge_fixtures();
    // 干净合并 -> 0，输出含两侧变化
    let out = run(&aic(), &["merge", &s(&f.clean_base), &s(&f.clean_left), &s(&f.clean_right)]);
    assert_eq!(exit_code(&out), 0, "无冲突自动合成 -> 0");
    assert_eq!(stdout(&out), "K1\n2\n3\n4\nK5\n");
    // 两侧同改 -> 0（不算冲突，FR-TMRG-003）
    let out = run(&aic(), &["merge", &s(&f.same_base), &s(&f.same_left), &s(&f.same_right)]);
    assert_eq!(exit_code(&out), 0, "两侧相同变化不算冲突");
    assert_eq!(stdout(&out), "a\nX\nc\n");
    // 冲突 -> 1（默认冲突码），marker 模式输出
    let out = run(&aic(), &["merge", &s(&f.base), &s(&f.left), &s(&f.right)]);
    assert_eq!(exit_code(&out), 1, "有未决冲突 -> 1");
    let text = stdout(&out);
    assert!(text.contains("<<<<<<<") && text.contains("=======") && text.contains(">>>>>>>"), "marker 模式: {text}");
    assert!(text.contains("L1") && text.contains("R1"));
    // --conflict-exit-code 14（FR-CLI-004 对齐）
    let out = run(
        &aic(),
        &["merge", &s(&f.base), &s(&f.left), &s(&f.right), "--conflict-exit-code", "14"],
    );
    assert_eq!(exit_code(&out), 14, "--conflict-exit-code 14 生效");
    // --auto：未决冲突按默认策略（left）解析为纯文本，退出码仍为 1
    let out = run(&aic(), &["merge", &s(&f.base), &s(&f.left), &s(&f.right), "--auto"]);
    assert_eq!(exit_code(&out), 1, "auto 模式有未决冲突仍 -> 1");
    assert_eq!(stdout(&out), "h\nL1\nb\nL2\nt\n", "auto 模式输出合成文本（默认取左）");
    // --auto --on-conflict base
    let out = run(
        &aic(),
        &["merge", &s(&f.base), &s(&f.left), &s(&f.right), "--auto", "--on-conflict", "base"],
    );
    assert_eq!(exit_code(&out), 1);
    assert_eq!(stdout(&out), "h\na\nb\nc\nt\n", "on-conflict base = 还原基");
    // 错误 -> 2（文件不存在 / 参数不足 / 未知参数）
    let out = run(&aic(), &["merge", "/nonexistent/aic/z1", &s(&f.left), &s(&f.right)]);
    assert_eq!(exit_code(&out), 2);
    let out = run(&aic(), &["merge", &s(&f.base), &s(&f.left)]);
    assert_eq!(exit_code(&out), 2, "参数不足 -> 2");
    let out = run(&aic(), &["merge", "--nope", &s(&f.base), &s(&f.left), &s(&f.right)]);
    assert_eq!(exit_code(&out), 2);
}

#[test]
fn merge_marker_styles_and_output_file() {
    let f = merge_fixtures();
    // 默认 merge 风格：无 ||||||| base 段
    let out = run(&aic(), &["merge", &s(&f.base), &s(&f.left), &s(&f.right)]);
    let text = stdout(&out);
    assert!(!text.contains("|||||||"), "默认 marker 无 base 段: {text}");
    // --diff3：含 base 段（GNU diff3 -m / git merge-file --diff3 同形）
    let out = run(&aic(), &["merge", &s(&f.base), &s(&f.left), &s(&f.right), "--diff3"]);
    let text = stdout(&out);
    assert!(text.contains("|||||||"), "--diff3 含 base 段: {text}");
    let base_name = f.base.file_name().unwrap().to_string_lossy().into_owned();
    assert!(text.contains(&base_name), "base 段标签 = base 路径: {text}");
    // -o 写文件（不进 stdout）
    let out_path = f._dir.path().join("merged.out");
    let out = run(
        &aic(),
        &["merge", &s(&f.clean_base), &s(&f.clean_left), &s(&f.clean_right), "-o", &s(&out_path)],
    );
    assert_eq!(exit_code(&out), 0);
    assert_eq!(stdout(&out), "", "-o 模式不写 stdout");
    assert_eq!(std::fs::read_to_string(&out_path).unwrap(), "K1\n2\n3\n4\nK5\n");
}

#[test]
fn merge_importance_and_window_flags() {
    let f = merge_fixtures();
    // 尾随空白不重要的同效冲突：-iu + favor-right 自动采用右侧 -> 0
    let dir = f._dir.path().to_path_buf();
    let b = dir.join("w_base.l");
    let l = dir.join("w_left.l");
    let r = dir.join("w_right.l");
    std::fs::write(&b, "a\nb\nc\n").unwrap();
    std::fs::write(&l, "a\nB  \nc\n").unwrap();
    std::fs::write(&r, "a\nB\nc\n").unwrap();
    let out = run(&aic(), &["merge", &s(&b), &s(&l), &s(&r)]);
    assert_eq!(exit_code(&out), 1, "默认重要性下为冲突 -> 1");
    let out = run(
        &aic(),
        &["merge", &s(&b), &s(&l), &s(&r), "-iu", "--favor-right"],
    );
    assert_eq!(exit_code(&out), 0, "-iu + favorRight 不重要冲突自动采用 -> 0");
    assert_eq!(stdout(&out), "a\nB\nc\n");
    // 冲突窗口：默认无冲突的两处接近变化，窗口 1 合并为冲突
    let b = dir.join("g_base.l");
    let l = dir.join("g_left.l");
    let r = dir.join("g_right.l");
    std::fs::write(&b, "a\nmid\nc\n").unwrap();
    std::fs::write(&l, "A\nmid\nc\n").unwrap();
    std::fs::write(&r, "a\nmid\nC\n").unwrap();
    let out = run(&aic(), &["merge", &s(&b), &s(&l), &s(&r)]);
    assert_eq!(exit_code(&out), 0, "默认（仅更改不同的行）无冲突");
    let out = run(&aic(), &["merge", &s(&b), &s(&l), &s(&r), "--conflict-window", "1"]);
    assert_eq!(exit_code(&out), 1, "窗口 1：接近变化标记为冲突供审查");
    assert!(stdout(&out).contains("<<<<<<<"), "窗口冲突写 marker");
}

#[test]
fn aicc_merge_subcommand_passthrough() {
    // aicc（等待型包装）对 merge 子命令原样透传（FR-CLI-001 双形态）
    let f = merge_fixtures();
    let args = ["merge", &s(&f.clean_base), &s(&f.clean_left), &s(&f.clean_right)];
    let out = run(&aicc(), &args);
    assert_eq!(exit_code(&out), 0, "aicc merge 干净合并 -> 0");
    assert_eq!(stdout(&out), "K1\n2\n3\n4\nK5\n");
    let args = ["merge", &s(&f.base), &s(&f.left), &s(&f.right)];
    let out = run(&aicc(), &args);
    assert_eq!(exit_code(&out), 1, "aicc merge 冲突 -> 1（透传）");
    assert!(stdout(&out).contains("<<<<<<<"));
}

/// `patch apply` 是"执行"而非"比较"：退出码只有 0（成功）/ 2（错误），
/// 没有 1（文件不同）。锁定第一轮 review 的 F-07 / F-08（此前无回归覆盖）。
///
/// - F-07：单文件应用失败旧实现退 1 → CI/VCS 会误判为"有差异"而非"出错"；
/// - F-08：--dir 多文件模式 alreadyApplied 属拒绝（git apply 同语义）→ 2，
///   skippedBinary 属设计语义（二进制不参与文本补丁）→ 0。
#[test]
fn patch_apply_exit_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let tree = root.join("tree");
    std::fs::create_dir_all(&tree).unwrap();
    let target = tree.join("f.txt");
    std::fs::write(&target, "a\nb\nc\n").unwrap();
    // 非 UTF-8 目标 → 按二进制跳过
    std::fs::write(tree.join("bin.dat"), [0xff, 0xfe, 0x00, b'x']).unwrap();

    let good = root.join("good.patch");
    std::fs::write(&good, "--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n").unwrap();
    let bad = root.join("bad.patch");
    std::fs::write(&bad, "--- a/f.txt\n+++ b/f.txt\n@@ -1,3 +1,3 @@\n a\n-XX\n+YY\n c\n").unwrap();
    let binp = root.join("bin.patch");
    std::fs::write(&binp, "--- a/bin.dat\n+++ b/bin.dat\n@@ -1,1 +1,1 @@\n-x\n+y\n").unwrap();
    let out_file = root.join("out.txt");

    // 单文件：上下文不匹配 → 2（不是 1）
    let out = run(
        &aic(),
        &["patch", "apply", &s(&bad), "--input", &s(&target), "-o", &s(&out_file)],
    );
    assert_eq!(exit_code(&out), 2, "应用失败必须是错误(2)，实际: {}", stdout(&out));
    assert!(!out_file.exists(), "失败不得产出输出文件");

    // 单文件：成功 → 0
    let out = run(
        &aic(),
        &["patch", "apply", &s(&good), "--input", &s(&target), "-o", &s(&out_file)],
    );
    assert_eq!(exit_code(&out), 0, "应用成功应退 0，实际: {}", stdout(&out));
    assert_eq!(std::fs::read_to_string(&out_file).unwrap(), "a\nB\nc\n");

    // --dir：首次应用 → 0
    let out = run(&aic(), &["patch", "apply", &s(&good), "--dir", &s(&tree)]);
    assert_eq!(exit_code(&out), 0, "首次应用应退 0，实际: {}", stdout(&out));
    assert!(stdout(&out).starts_with("applied\tf.txt"), "实际: {}", stdout(&out));

    // --dir：重复应用 → alreadyApplied 属拒绝 → 2（旧实现误判为差异 1）
    let out = run(&aic(), &["patch", "apply", &s(&good), "--dir", &s(&tree)]);
    assert_eq!(exit_code(&out), 2, "alreadyApplied 应退 2，实际: {}", stdout(&out));
    assert!(stdout(&out).starts_with("alreadyApplied\tf.txt"), "实际: {}", stdout(&out));

    // --dir：仅二进制跳过 → 0（设计语义，不是错误也不是差异）
    let out = run(&aic(), &["patch", "apply", &s(&binp), "--dir", &s(&tree)]);
    assert_eq!(exit_code(&out), 0, "skippedBinary 应退 0，实际: {}", stdout(&out));
    assert!(stdout(&out).starts_with("skippedBinary\tbin.dat"), "实际: {}", stdout(&out));

    // --dir：文本失败 + 二进制跳过混合 → 有失败即 2
    let mixed = root.join("mixed.patch");
    std::fs::write(
        &mixed,
        format!("{}{}", std::fs::read_to_string(&bad).unwrap(), std::fs::read_to_string(&binp).unwrap()),
    )
    .unwrap();
    let out = run(&aic(), &["patch", "apply", &s(&mixed), "--dir", &s(&tree)]);
    assert_eq!(exit_code(&out), 2, "任一文件失败即错误(2)，实际: {}", stdout(&out));
}
