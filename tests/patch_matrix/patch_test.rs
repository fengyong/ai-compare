//! 补丁对拍矩阵（P9 / docs/04 §P9 验收要点：patch 解析/应用对拍）。
//!
//! 双通道：
//! 1. **git apply --check 对拍**（12 条语料）：同一目标树 + 补丁文本，
//!    `git apply --check -p1` 的接受/拒绝结论与 ai-core `apply_patch`
//!    报告一致（一致性矩阵；已知设计差异：二进制跳过归类为差异——
//!    git 拒绝无数据的 binary patch，我们跳过并报告不算失败）；
//! 2. **aic 自产自应用 roundtrip**：`aic diff` 输出的 unified patch
//!    经 aic patch apply 还原右侧（与 CLI 链路端到端）。
//!
//! 语料：精确/位置漂移/模糊上下文/CRLF/新增删除文件头/时间戳容错/
//! no-newline/多文件/二进制/纯 git 头/ghost 目标。

use ai_compare_tests as _;
use ai_core::patch::{
    apply_file, apply_patch, parse_patch, PatchApplyOptions, PatchApplyReport, PatchError, StdFs,
};
use std::path::{Path, PathBuf};
use std::process::Command;

const FUZZ: u32 = 3;

struct Corpus {
    name: &'static str,
    /// 目标树（rel path -> content；None 表示不应预先存在）
    files: &'static [(&'static str, &'static str)],
    patch: &'static str,
    /// ai-core 侧期望：Some(expected rel->content) = 应用后文件内容；None = 期望失败
    expect: Option<&'static [(&'static str, &'static str)]>,
    /// git apply --check 预期：Accept / Reject / Skip / KnownDiff（已知设计差异）
    git: GitExpect,
    /// 覆盖默认应用选项
    opts: Option<PatchApplyOptions>,
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum GitExpect {
    /// git 接受 且 我们接受
    Accept,
    /// git 拒绝 且 我们拒绝
    Reject,
    /// 不与 git 对拍（无数据 binary patch 等）
    Skip,
    /// 已知设计差异：git 拒绝、我们接受（特性语义）
    KnownDiff,
}

fn default_opts() -> PatchApplyOptions {
    PatchApplyOptions { dry_run: false, fuzz: FUZZ, crlf_tolerant: true, ignore_whitespace: false }
}

const CORPORA: &[Corpus] = &[
    Corpus {
        name: "01_exact_modify",
        files: &[("t.txt", "1\n2\n3\n")],
        patch: "diff --git a/t.txt b/t.txt\nindex 111..222 100644\n--- a/t.txt\n+++ b/t.txt\n@@ -1,3 +1,3 @@\n 1\n-2\n+TWO\n 3\n",
        expect: Some(&[("t.txt", "1\nTWO\n3\n")]),
        git: GitExpect::Accept,
        opts: None,
    },
    Corpus {
        name: "02_two_hunks",
        files: &[("t.txt", "a\nb\nc\nd\ne\nf\ng\nh\n")],
        patch: "--- a/t.txt\n+++ b/t.txt\n@@ -1,2 +1,2 @@\n-a\n+A\n b\n@@ -7,2 +7,2 @@\n-g\n+G2\n h\n",
        expect: Some(&[("t.txt", "A\nb\nc\nd\ne\nf\nG2\nh\n")]),
        git: GitExpect::Accept,
        opts: None,
    },
    Corpus {
        name: "03_new_file",
        files: &[],
        patch: "diff --git a/made.txt b/made.txt\nnew file mode 100644\nindex 0000000..aaaa111\n--- /dev/null\n+++ b/made.txt\n@@ -0,0 +1,2 @@\n+hello\n+world\n",
        expect: Some(&[("made.txt", "hello\nworld\n")]),
        git: GitExpect::Accept,
        opts: None,
    },
    Corpus {
        name: "04_delete_file",
        files: &[("victim.txt", "victim\n")],
        patch: "diff --git a/victim.txt b/victim.txt\ndeleted file mode 100644\nindex bbbb..0000000\n--- a/victim.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-victim\n",
        expect: Some(&[]),
        git: GitExpect::Accept,
        opts: None,
    },
    Corpus {
        name: "05_crlf_target_tolerant",
        files: &[("crlf.txt", "one\r\ntwo\r\nthree\r\n")],
        patch: "--- a/crlf.txt\n+++ b/crlf.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n",
        // 已知设计差异（特性）：CRLF 容忍 = 匹配忽略行尾 CR —— git 默认逐字节拒绝
        expect: Some(&[("crlf.txt", "one\r\nTWO\r\nthree\r\n")]),
        git: GitExpect::KnownDiff,
        opts: None,
    },
    Corpus {
        name: "06_crlf_strict_rejects_lf_patch",
        files: &[("crlf.txt", "one\r\ntwo\r\nthree\r\n")],
        patch: "--- a/crlf.txt\n+++ b/crlf.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n",
        // git apply 对 CRLF 目标的 LF 补丁同样拒绝（上下文含 \r 差异）
        expect: None,
        git: GitExpect::Reject,
        opts: Some(PatchApplyOptions { dry_run: false, fuzz: 0, crlf_tolerant: false, ignore_whitespace: false }),
    },
    Corpus {
        name: "07_offset_drift",
        files: &[("t.txt", "h1\nh2\n1\n2\n3\n4\nold5\n6\n")],
        patch: "--- a/t.txt\n+++ b/t.txt\n@@ -5,4 +5,4 @@\n 3\n 4\n-old5\n+NEW5\n 6\n",
        expect: Some(&[("t.txt", "h1\nh2\n1\n2\n3\n4\nNEW5\n6\n")]),
        git: GitExpect::Accept,
        opts: None,
    },
    Corpus {
        name: "07b_offset_drift_no_context",
        files: &[("t2.txt", "h1\nh2\n1\n2\n3\n4\nold5\n6\n")],
        patch: "--- a/t2.txt\n+++ b/t2.txt\n@@ -5 +5 @@\n-old5\n+NEW5\n",
        // 已知设计差异（特性）：无上下文 hunk 我们全文件搜索补齐位置漂移；
        // git apply 对零上下文 hunk 仅匹配声明位置
        expect: Some(&[("t2.txt", "h1\nh2\n1\n2\n3\n4\nNEW5\n6\n")]),
        git: GitExpect::KnownDiff,
        opts: None,
    },
    Corpus {
        name: "08_fuzzy_context",
        files: &[("t.txt", "CHANGED-top\nmiddle\nctx-bottom\n")],
        patch: "--- a/t.txt\n+++ b/t.txt\n@@ -1,3 +1,3 @@\n ctx-top\n-middle\n+MIDDLE\n ctx-bottom\n",
        // 已知设计差异（特性）：模糊上下文（fuzz=3 丢弃首行上下文重试）——
        // git apply 默认不做模糊匹配（GNU patch 行为）
        expect: Some(&[("t.txt", "CHANGED-top\nMIDDLE\nctx-bottom\n")]),
        git: GitExpect::KnownDiff,
        opts: None,
    },
    Corpus {
        name: "09_no_newline_fix_up",
        files: &[("eof.txt", "old")],
        patch: "--- a/eof.txt\n+++ b/eof.txt\n@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n",
        expect: Some(&[("eof.txt", "new\n")]),
        git: GitExpect::Accept,
        opts: None,
    },
    Corpus {
        name: "10_no_newline_strip",
        files: &[("eof.txt", "old\n")],
        patch: "--- a/eof.txt\n+++ b/eof.txt\n@@ -1 +1 @@\n-old\n+new\n\\ No newline at end of file\n",
        expect: Some(&[("eof.txt", "new")]),
        git: GitExpect::Accept,
        opts: None,
    },
    Corpus {
        name: "11_binary_skip_reported",
        files: &[("img.png", "\u{89}PNG\r\n\u{1a}\n")],
        patch: "diff --git a/img.png b/img.png\nindex aaa..bbb 100644\nBinary files a/img.png and b/img.png differ\n",
        // 已知设计差异：git 拒绝无数据的 binary patch；我们跳过并报告（不算失败）
        expect: Some(&[("img.png", "\u{89}PNG\r\n\u{1a}\n")]),
        git: GitExpect::Skip,
        opts: None,
    },
    Corpus {
        name: "12_timestamp_tolerance",
        files: &[("t.txt", "1\n2\n3\n")],
        patch: "--- a/t.txt\t2026-11-14 03:00:00.000000000 +0800\n+++ b/t.txt\t2026-11-14 03:00:01.000000000 +0800\n@@ -1,3 +1,3 @@\n 1\n-2\n+2.5\n 3\n",
        expect: Some(&[("t.txt", "1\n2.5\n3\n")]),
        git: GitExpect::Accept,
        opts: None,
    },
    Corpus {
        name: "13_multi_file",
        files: &[("a.txt", "alpha\n"), ("b.txt", "beta\n")],
        patch: "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-alpha\n+ALPHA\ndiff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-beta\n+BETA\n",
        expect: Some(&[("a.txt", "ALPHA\n"), ("b.txt", "BETA\n")]),
        git: GitExpect::Accept,
        opts: None,
    },
    Corpus {
        name: "14_ghost_context_fails",
        files: &[("t.txt", "totally\nunrelated\ncontent\n")],
        patch: "--- a/t.txt\n+++ b/t.txt\n@@ -1,3 +1,3 @@\n ctx-top\n-middle\n+MIDDLE\n ctx-bottom\n",
        expect: None,
        git: GitExpect::Reject,
        opts: None,
    },
    Corpus {
        name: "15_rename_headers_as_modify",
        files: &[("old_name.txt", "same-content\n")],
        patch: "diff --git a/old_name.txt b/new_name.txt\nsimilarity index 100%\nrename from old_name.txt\nrename to new_name.txt\n--- a/old_name.txt\n+++ b/new_name.txt\n@@ -1 +1 @@\n-same-content\n+new-content\n",
        // 不对拍（范围说明）：git apply 支持 rename 语义（建新删旧）；
        // P9 引擎按 unified hunk 应用——目标不存在即失败，rename 语义属文件操作层
        expect: None,
        git: GitExpect::Skip,
        opts: None,
    },
];

fn write_tree(root: &Path, files: &[(&str, &str)]) {
    for (rel, content) in files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
}

fn read_tree(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push((rel, String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned()));
            }
        }
    }
    walk(root, root, &mut out);
    out.sort();
    out
}

/// 构建 git apply 可执行环境（git 仓库内避免 safeguard 差异）
fn git_apply_check(tree: &Path, patch: &Path) -> (i32, String) {
    let repo = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .expect("git 可用");
        String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout)
    };
    let _ = run(&["init", "-q"]);
    let _ = run(&["config", "user.email", "t@t"]);
    let _ = run(&["config", "user.name", "t"]);
    // 目标树落盘
    for entry in std::fs::read_dir(tree).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        let content = std::fs::read(entry.path()).unwrap();
        std::fs::write(repo.path().join(&name), content).unwrap();
    }
    let patch_dst = repo.path().join("p.patch");
    std::fs::copy(patch, &patch_dst).unwrap();
    let out = Command::new("git")
        .args(["apply", "--check", "-p1", "p.patch"])
        .current_dir(repo.path())
        .output()
        .expect("git apply 可用");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// 主对拍：git apply --check 结论 vs ai-core 报告（一致性矩阵）
#[test]
fn crosscheck_with_git_apply_check() {
    let mut consistent = 0usize;
    let mut classified = Vec::new();
    for c in CORPORA {
        // ---- ai-core 侧 ----
        let ours_dir = tempfile::tempdir().unwrap();
        write_tree(ours_dir.path(), c.files);
        let opts = c.opts.clone().unwrap_or_else(default_opts);
        let report: Result<PatchApplyReport, PatchError> =
            apply_patch(&StdFs, ours_dir.path(), c.patch, &opts);
        // 期望拒绝 = 解析报错 或 报告存在 Failed/AlreadyApplied（未全部应用）
        let our_ok = match (&c.expect, report) {
            (Some(expected), Ok(report)) => {
                // 全部应用成功（SkippedBinary/NoOp 不算失败——二进制跳过是设计语义）
                assert!(
                    report.results.iter().all(|r| !matches!(r.status,
                        ai_core::patch::FileApplyStatus::Failed | ai_core::patch::FileApplyStatus::AlreadyApplied)),
                    "语料 {}: 期望无失败项，报告: {report:?}",
                    c.name
                );
                let actual = read_tree(ours_dir.path());
                let mut expected_sorted: Vec<(String, String)> =
                    expected.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
                expected_sorted.sort();
                assert_eq!(actual, expected_sorted, "语料 {}: 应用结果不符", c.name);
                true
            }
            (None, Err(_)) => true, // 期望拒绝：解析层直接报错
            (Some(_), Err(e)) => panic!("语料 {}: 期望应用成功，实际失败: {e}", c.name),
            (None, Ok(report)) => {
                assert!(
                    !report.all_applied(),
                    "语料 {}: 期望拒绝，实际全部应用成功: {report:?}",
                    c.name
                );
                false
            }
        };

        // ---- git 侧 ----
        match c.git {
            GitExpect::Skip => {
                classified.push((c.name, "git 拒绝无数据 binary patch（设计差异：我们跳过并报告）"));
                continue;
            }
            GitExpect::KnownDiff => {
                let tree = tempfile::tempdir().unwrap();
                write_tree(tree.path(), c.files);
                let patch_file = tree.path().join("p.patch");
                std::fs::write(&patch_file, c.patch).unwrap();
                let (code, _stderr) = git_apply_check(tree.path(), &patch_file);
                assert_ne!(code, 0, "语料 {}: 已知设计差异要求 git 拒绝", c.name);
                assert!(our_ok, "语料 {}: 已知设计差异要求我们接受（CRLF 容忍）", c.name);
                classified.push((c.name, "CRLF 容忍：git 逐字节拒绝、我们按特性接受"));
                continue;
            }
            GitExpect::Accept | GitExpect::Reject => {
                let tree = tempfile::tempdir().unwrap();
                write_tree(tree.path(), c.files);
                let patch_file = tree.path().join("p.patch");
                std::fs::write(&patch_file, c.patch).unwrap();
                let (code, stderr) = git_apply_check(tree.path(), &patch_file);
                let git_ok = code == 0;
                assert_eq!(
                    git_ok, our_ok,
                    "语料 {}: git apply --check 与 ai-core 结论不一致（git={code} {stderr}）",
                    c.name
                );
                consistent += 1;
            }
        }
    }
    // 一致性摘要（对拍报告数据源）
    eprintln!("patch_matrix: git apply --check 对拍一致 {}/{}（另有 {} 条设计差异归类）", consistent, CORPORA.len() - classified.len(), classified.len());
    // 16 条语料：11 条与 git apply 结论完全一致；3 条已知设计差异（CRLF 容忍/
    // 无上下文漂移搜索/模糊上下文——均为任务书要求的特性语义）；2 条不对拍
    // （binary 无数据 / rename 语义属文件操作层）
    assert_eq!(consistent, 11, "git apply --check 对拍一致数");
    assert_eq!(classified.len(), 5, "设计差异归类条数");
}

// ---------------------------------------------------------------------------
// aic 自产自应用 roundtrip（CLI 链路端到端）
// ---------------------------------------------------------------------------

fn aic() -> PathBuf {
    ai_compare_tests::cli_bin("aic")
}

#[test]
fn aic_diff_output_roundtrips_through_patch_apply() {
    let work = tempfile::tempdir().unwrap();
    let a = work.path().join("a");
    let b = work.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let left = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
    let right = "alpha\nBETA\ngamma\ndelta\nEPSILON\nzeta\n";
    std::fs::write(a.join("t.txt"), left).unwrap();
    std::fs::write(b.join("t.txt"), right).unwrap();

    // aic diff（unified 输出即补丁文本）
    let out = Command::new(aic())
        .args(["diff", "a/t.txt", "b/t.txt"])
        .current_dir(work.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "aic diff 退出 1");
    let patch_text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(patch_text.starts_with("--- "), "unified 头: {patch_text}");

    // 应用回左侧 -> 还原右侧
    let target = tempfile::tempdir().unwrap();
    std::fs::write(target.path().join("t.txt"), left).unwrap();
    let patch_path = target.path().join("p.patch");
    std::fs::write(&patch_path, &patch_text).unwrap();
    let out_path = target.path().join("out.txt");
    let out = Command::new(aic())
        .args([
            "patch",
            "apply",
            patch_path.to_str().unwrap(),
            "--input",
            target.path().join("t.txt").to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "aic patch apply 退出 0: {}", String::from_utf8_lossy(&out.stderr));
    let applied = std::fs::read_to_string(&out_path).unwrap();
    assert_eq!(applied, right, "自产补丁应用还原右侧");
}

#[test]
fn aic_diff_roundtrip_git_apply_check_consistent() {
    let work = tempfile::tempdir().unwrap();
    let left = "one\ntwo\nthree\nfour\nfive\n";
    let right = "one\nTWO\nthree\nfour\nFIVE+\n";
    std::fs::write(work.path().join("l.txt"), left).unwrap();
    std::fs::write(work.path().join("r.txt"), right).unwrap();
    let out = Command::new(aic())
        .args(["diff", "l.txt", "r.txt"])
        .current_dir(work.path())
        .output()
        .unwrap();
    let patch_text = String::from_utf8_lossy(&out.stdout).into_owned();
    // 标签重写为 git 风格路径（aic diff 原样回显命令行标签）
    let git_patch = patch_text.replace("--- l.txt", "--- a/l.txt").replace("+++ r.txt", "+++ b/l.txt");
    let tree = tempfile::tempdir().unwrap();
    std::fs::write(tree.path().join("l.txt"), left).unwrap();
    let patch_file = tree.path().join("p.patch");
    std::fs::write(&patch_file, git_patch).unwrap();
    let (code, stderr) = git_apply_check(tree.path(), &patch_file);
    assert_eq!(code, 0, "git apply --check 应接受 aic diff 产物: {stderr}");

    // 同一补丁经我们应用（--input 左侧 -> stdout == 右侧）
    let out = Command::new(aic())
        .args(["patch", "apply", patch_file.to_str().unwrap(), "--input", tree.path().join("l.txt").to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let applied = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    assert_eq!(applied, right, "aic patch apply 结果 == 右侧文本");
}

// ---------------------------------------------------------------------------
// 解析器容错 golden（P9 验收要点：git/diff -u corpora 解析）
// ---------------------------------------------------------------------------

#[test]
fn parser_tolerance_goldens() {
    // git 全套头（mode/rename/copy/similarity）不干扰 hunk
    let p = parse_patch(
        "diff --git a/x.c b/x.c\nold mode 100644\nnew mode 100755\nsimilarity index 95%\nrename from x.c\nrename to y.c\nindex abc..def 100644\n--- a/x.c\n+++ b/y.c\n@@ -1 +1 @@\n-a\n+b\n",
    )
    .unwrap();
    assert_eq!(p.files.len(), 1);
    assert_eq!(p.files[0].old_path.as_deref(), Some("x.c"));
    assert_eq!(p.files[0].new_path.as_deref(), Some("y.c"));
    assert!(!p.files[0].is_new && !p.files[0].is_delete);

    // GIT binary patch 识别
    let p = parse_patch("diff --git a/blob b/blob\nindex abc..def 100644\nGIT binary patch\nliteral 8\nZcmV^Z*<fake>\n\n").unwrap();
    assert!(p.files[0].binary && p.files[0].hunks.is_empty());

    // 空 patch -> Empty
    assert!(matches!(parse_patch("not a patch\n"), Err(PatchError::Empty)));

    // 已应用检测（AlreadyApplied -> git apply --check 亦拒绝：roundtrip 二次应用失败）
    let patch = "--- a\n+++ b\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
    let f = &parse_patch(patch).unwrap().files[0];
    let err = apply_file("a\nB\nc\n", f, &default_opts()).unwrap_err();
    assert!(err.contains("已应用"));
}
