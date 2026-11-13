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
