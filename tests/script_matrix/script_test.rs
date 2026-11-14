//! P11 脚本处理器矩阵（docs/04 §P11 验收要点：31 命令参数矩阵 + 脚本错误路径
//! 退出码 105/106/107 + `aic @script` 无头运行）。
//!
//! 语料：≥15 个 .aicscript 脚本文件（全命令覆盖抽查/变量替换/条件/错误处理/
//! 退出码），经 `aic script run` 与 `aiscript` 独立入口双面驱动（进程级，
//! FR-SCR 端到端）。

use ai_compare_tests as harness;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Sandbox {
    _dir: tempfile::TempDir,
    l: PathBuf,
    r: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        Sandbox { _dir: dir, l, r }
    }

    fn ls(&self) -> String {
        self.l.to_string_lossy().into_owned()
    }
    fn rs(&self) -> String {
        self.r.to_string_lossy().into_owned()
    }
    fn path(&self, side: char, name: &str) -> String {
        match side {
            'l' => self.l.join(name),
            _ => self.r.join(name),
        }
        .to_string_lossy()
        .into_owned()
    }
    fn write(&self, side: char, name: &str, content: &str) {
        let p = match side {
            'l' => self.l.join(name),
            _ => self.r.join(name),
        };
        if let Some(parent) = Path::new(&p).parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }
    fn exists(&self, side: char, name: &str) -> bool {
        let p = match side {
            'l' => self.l.join(name),
            _ => self.r.join(name),
        };
        p.exists()
    }
}

/// 运行脚本文本（写入临时脚本文件后经 `aic script run` 执行；cwd 沙箱化到 box_cwd）
fn run_script(box_cwd: &Path, script_src: &str, args: &[String], extra: &[&str]) -> (i32, String, String) {
    let script_path = box_cwd.join("case.aicscript");
    std::fs::write(&script_path, script_src).unwrap();
    let mut cmd = Command::new(harness::cli_bin("aic"));
    cmd.current_dir(box_cwd);
    cmd.arg("script").arg("run").arg(script_path.to_str().unwrap());
    for a in args {
        cmd.arg(a);
    }
    for e in extra {
        cmd.arg(e);
    }
    let out = cmd.output().expect("运行脚本");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `aiscript <file> [args]` 独立入口形态
fn run_aiscript(box_cwd: &Path, script_src: &str, args: &[String], extra: &[&str]) -> (i32, String, String) {
    let script_path = box_cwd.join("case.aicscript");
    std::fs::write(&script_path, script_src).unwrap();
    let mut cmd = Command::new(harness::cli_bin("aiscript"));
    cmd.current_dir(box_cwd);
    cmd.arg(script_path.to_str().unwrap());
    for a in args {
        cmd.arg(a);
    }
    for e in extra {
        cmd.arg(e);
    }
    let out = cmd.output().expect("运行 aiscript");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// 独立沙箱目录（报告/日志/快照输出落盘处）
fn fresh_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("aicscript-matrix-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// ---------------------------------------------------------------------------
// 脚本语料矩阵（≥15 个脚本：全命令覆盖抽查）
// ---------------------------------------------------------------------------

#[test]
fn script_01_load_select_folder_report() {
    let boxd = fresh_dir("01");
    let sb = Sandbox::new();
    let src = format!(
        "# 语料 01：LOAD + SELECT + FOLDER-REPORT\n\
         LOAD {l} {r}\n\
         select left.orphan.files\n\
         copy left->right\n\
         FOLDER-REPORT layout:list output-to:report.txt\n",
        l = sb.ls(),
        r = sb.rs()
    );
    sb.write('l', "lonly.txt", "L\n");
    sb.write('r', "ronly.txt", "R\n");
    let (code, _out, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let report = std::fs::read_to_string(boxd.join("report.txt")).unwrap();
    // 复制后重扫：lonly.txt 双侧同内容（same 2 2）；ronly 仍为右侧孤项
    assert!(report.contains("same\tlonly.txt\t2\t2"), "{report}");
    assert!(report.contains("rightOnly\tronly.txt"), "{report}");
    assert!(sb.exists('r', "lonly.txt"), "孤项复制落盘");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_02_folder_report_xml_and_layout_matrix() {
    let boxd = fresh_dir("02");
    let sb = Sandbox::new();
    sb.write('l', "same.txt", "x\n");
    sb.write('r', "same.txt", "x\n");
    let src = format!(
        "load {l} {r}\n\
         folder-report layout:xml output-to:r.xml\n\
         folder-report layout:side-by-side output-to:r.txt\n\
         folder-report layout:html output-to:\"r web.html\"\n\
         folder-report layout:stats output-to:r.stats\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let xml = std::fs::read_to_string(boxd.join("r.xml")).unwrap();
    assert!(xml.starts_with("<?xml"), "{xml}");
    let stats = std::fs::read_to_string(boxd.join("r.stats")).unwrap();
    assert!(stats.contains("total=1"));
    let html = std::fs::read_to_string(boxd.join("r web.html")).unwrap();
    assert!(html.contains("<!DOCTYPE html>"), "带空格路径（引号参数）");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_03_text_report_patch_roundtrip() {
    let boxd = fresh_dir("03");
    let sb = Sandbox::new();
    sb.write('l', "t.txt", "alpha\nbeta\ngamma\n");
    sb.write('r', "t.txt", "alpha\nBETA\ngamma\n");
    let src = format!(
        "load {lf} {rf}\n\
         text-report layout:patch output-to:t.patch\n",
        lf = sb.path('l', "t.txt"),
        rf = sb.path('r', "t.txt")
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let patch = std::fs::read_to_string(boxd.join("t.patch")).unwrap();
    assert!(patch.contains("-beta") && patch.contains("+BETA"), "{patch}");
    // ai-core patch apply 还原右侧
    let parsed = ai_core::patch::parse_patch(&patch).unwrap();
    let applied = ai_core::patch::apply_file("alpha\nbeta\ngamma\n", &parsed.files[0], &ai_core::patch::PatchApplyOptions::default()).unwrap();
    assert!(matches!(applied, ai_core::patch::ApplyOutcome::Applied { ref text, .. } if text == "alpha\nBETA\ngamma\n"));
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_04_table_report_csv() {
    let boxd = fresh_dir("04");
    let sb = Sandbox::new();
    sb.write('l', "t.csv", "id,v\n1,a\n");
    sb.write('r', "t.csv", "id,v\n1,b\n2,c\n");
    let src = format!(
        "load {lf} {rf}\ntable-report layout:csv output-to:t.csv.txt\n",
        lf = sb.path('l', "t.csv"),
        rf = sb.path('r', "t.csv")
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let report = std::fs::read_to_string(boxd.join("t.csv.txt")).unwrap();
    assert!(report.contains("changed") && report.contains("added"), "{report}");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_05_hex_report() {
    let boxd = fresh_dir("05");
    let sb = Sandbox::new();
    std::fs::write(sb.l.join("a.bin"), b"AAAABBBB").unwrap();
    std::fs::write(sb.r.join("a.bin"), b"AAAAXXBB").unwrap();
    let src = format!(
        "load {lf} {rf}\nhex-report layout:stats output-to:h.txt\n",
        lf = sb.path('l', "a.bin"),
        rf = sb.path('r', "a.bin")
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let report = std::fs::read_to_string(boxd.join("h.txt")).unwrap();
    assert!(report.contains("changedBytes=2"), "{report}");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_06_file_report_auto_table() {
    let boxd = fresh_dir("06");
    let sb = Sandbox::new();
    sb.write('l', "t.csv", "id,v\n1,a\n");
    sb.write('r', "t.csv", "id,v\n1,b\n");
    sb.write('l', "n.txt", "x\n");
    sb.write('r', "n.txt", "y\n");
    let src = format!(
        "load {l} {r}\nfile-report layout:summary output-to:files.txt\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let report = std::fs::read_to_string(boxd.join("files.txt")).unwrap();
    assert!(report.contains("t.csv"), "逐文件段: {report}");
    assert!(report.contains("n.txt"));
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_07_delete_and_filter() {
    let boxd = fresh_dir("07");
    let sb = Sandbox::new();
    sb.write('r', "junk.tmp", "junk\n");
    sb.write('r', "keep.txt", "keep\n");
    sb.write('l', "l1.txt", "1\n");
    let src = format!(
        "load {l} {r}\n\
         filter \"*.tmp\"\n\
         select all\n\
         delete all\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(!sb.exists('r', "junk.tmp"), "过滤后 junk 应被删除");
    assert!(sb.exists('r', "keep.txt"), "keep.txt 不在过滤集内");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_08_move_and_moveto() {
    let boxd = fresh_dir("08");
    let sb = Sandbox::new();
    sb.write('l', "m.txt", "move me\n");
    let src = format!(
        "load {l} {r}\n\
         select left.orphan.files\n\
         move left->right\n\
         select right.orphan.files\n\
         moveto right out/\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(!sb.exists('l', "m.txt"), "move 删源");
    // moveto = 复制到外部目录 + 从会话侧移除（BC 语义）
    assert!(!sb.exists('r', "m.txt"), "moveto 后会话侧移除");
    assert!(boxd.join("out").join("m.txt").exists(), "moveto 复制到外部目录");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_09_mkdir_rename_touch() {
    let boxd = fresh_dir("09");
    let sb = Sandbox::new();
    sb.write('l', "old_name.txt", "r\n");
    sb.write('r', "old_name.txt", "r\n");
    // 顺序：先改名/对时（select all 为动态谓词，后建目录会被掩码命中并改名）
    let src = format!(
        "load {l} {r}\n\
         select all\n\
         rename \"renamed_?.txt\"\n\
         touch left->right\n\
         mkdir left newdir\n\
         mkdir right newdir\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(sb.exists('l', "newdir"), "mkdir 落盘");
    assert!(sb.exists('r', "newdir"));
    assert!(sb.exists('l', "renamed_o.txt"), "DOS 掩码改名（? 取源名首字符）: {err}");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_10_attrib_readonly() {
    let boxd = fresh_dir("10");
    let sb = Sandbox::new();
    sb.write('l', "ro.txt", "readonly\n");
    sb.write('r', "ro.txt", "readonly\n");
    let src = format!(
        "load {l} {r}\nselect all\nattrib +r\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(sb.l.join("ro.txt")).unwrap().permissions().mode();
        assert_eq!(mode & 0o222, 0, "+r 后应无写位");
        let _ = std::fs::set_permissions(sb.l.join("ro.txt"), std::fs::Permissions::from_mode(0o644));
    }
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_11_sync_mirror() {
    let boxd = fresh_dir("11");
    let sb = Sandbox::new();
    sb.write('l', "a.txt", "A\n");
    sb.write('l', "sub_only.txt", "S\n");
    sb.write('r', "stale.txt", "STALE\n");
    let src = format!(
        "load {l} {r}\n\
         sync mirror:left->right\n\
         folder-report layout:stats output-to:after.stats\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(sb.exists('r', "a.txt"), "镜像复制");
    assert!(sb.exists('r', "sub_only.txt"), "镜像复制孤项");
    assert!(!sb.exists('r', "stale.txt"), "镜像删除右侧多余项");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_12_snapshot_bcss() {
    let boxd = fresh_dir("12");
    let sb = Sandbox::new();
    sb.write('l', "f1.txt", "1\n");
    sb.write('l', "sub/f2.txt", "2\n");
    // 孤儿子树不进 arena（引擎 P0 约定）：sub 需双侧存在才可展开
    sb.write('r', "sub/f2.txt", "2\n");
    let src = format!(
        "load {l} {r}\n\
         expand all\n\
         snapshot left output:snap.bcss\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let snap = std::fs::read_to_string(boxd.join("snap.bcss")).unwrap();
    assert!(snap.contains("\"magic\":\"ai-compare-snapshot\""), "{snap}");
    assert!(snap.contains("\"path\":\"f1.txt\""));
    assert!(snap.contains("\"path\":\"sub/f2.txt\""), "{snap}");
    // path: 单目录变体
    let src2 = format!("snapshot path:{l} output:snap2\n", l = sb.ls());
    let (code2, _o2, err2) = run_script(&boxd, &src2, &[], &[]);
    assert_eq!(code2, 0, "{err2}");
    assert!(boxd.join("snap2.bcss").exists(), "无扩展名补 .bcss");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_13_criteria_compare_expand() {
    let boxd = fresh_dir("13");
    let sb = Sandbox::new();
    sb.write('l', "sub/d.txt", "v1\n");
    sb.write('r', "sub/d.txt", "v2\n");
    let src = format!(
        "load {l} {r}\n\
         criteria crc\n\
         compare crc\n\
         expand all\n\
         folder-report layout:stats output-to:c.stats\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let stats = std::fs::read_to_string(boxd.join("c.stats")).unwrap();
    assert!(stats.contains("contentDiff=1"), "EXPAND 后子项进入比较/统计: {stats}");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_14_variables_and_conditionals() {
    let boxd = fresh_dir("14");
    let sb = Sandbox::new();
    std::fs::write(boxd.join("aic-vars.env"), "k=v\n").unwrap();
    // %1/%2 位置参数 + IF-EXISTS/IF-NOT-EXISTS 分支控制
    let src = "LOAD %1 %2\n\
               IF-EXISTS aic-vars.env LOG verbose vars.log\n\
               IF-EXISTS no-such-file.xyz FOLDER-REPORT layout:stats output-to:wrong.stats\n\
               IF-NOT-EXISTS no-such-file.xyz FOLDER-REPORT layout:stats output-to:cond.stats\n";
    let (code, _o, err) = run_script(&boxd, src, &[sb.ls(), sb.rs()], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(boxd.join("cond.stats").exists(), "if-not-exists 分支执行");
    assert!(!boxd.join("wrong.stats").exists(), "假分支不应执行");
    assert!(boxd.join("vars.log").exists(), "if-exists 真分支执行（LOG）");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_15_env_var_and_positional_via_aiscript() {
    let boxd = fresh_dir("15");
    let sb = Sandbox::new();
    // aiscript 独立入口：%1/%2 = 路径，%3 = 普通词（/ 或 - 开头 token 剥离）
    let src = "load %1 %2\nfolder-report layout:stats output-to:out-%3.txt\n";
    let (code, _o, err) = run_aiscript(&boxd, src, &[sb.ls(), sb.rs(), "named".to_string()], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(boxd.join("out-named.txt").exists(), "%3 模板展开");
    // --var 注入 %VAR%（优先于环境变量）
    let src2 = "load %1 %2\nfolder-report layout:stats output-to:out-%AICTest%.txt\n";
    let (code2, _o2, err2) = run_aiscript(&boxd, src2, &[sb.ls(), sb.rs()], &["--var", "AICTest=envx"]);
    assert_eq!(code2, 0, "{err2}");
    assert!(boxd.join("out-envx.txt").exists(), "--var 注入 %VAR%");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_16_exit_codes_and_stop_on_error() {
    let boxd = fresh_dir("16");
    let sb = Sandbox::new();
    // 106 语法错误
    let (code, _o, err) = run_script(&boxd, "totally-bogus-cmd\n", &[], &[]);
    assert_eq!(code, 106, "语法错误 106: {err}");
    // 107 加载失败
    let (code, _o, err) = run_script(&boxd, &format!("load {}/missing {}/missing2\n", sb.ls(), sb.rs()), &[], &[]);
    assert_eq!(code, 107, "加载失败 107: {err}");
    // 105 脚本读取失败（aiscript 独立入口）
    let mut cmd = Command::new(harness::cli_bin("aiscript"));
    cmd.current_dir(&boxd).arg("no-such-script.aicscript");
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(105), "读脚本失败 105");
    // stop-on-error：运行期失败退出 2
    sb.write('l', "x.txt", "x\n");
    let src = format!(
        "option stop-on-error\nload {l} {r}\nmedia-report output-to:x.txt\nbeep\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 2, "stop-on-error 运行期失败退出 2: {err}");
    assert!(err.contains("media"), "{err}");
    // 无 stop-on-error：继续执行，退出 0
    let src = format!("load {l} {r}\nmedia-report output-to:x.txt\nbeep\n", l = sb.ls(), r = sb.rs());
    let (code, _o, _e) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "容错模式继续执行");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_17_log_written() {
    let boxd = fresh_dir("17");
    let sb = Sandbox::new();
    let src = format!(
        "LOG verbose run.log\nload {l} {r}\nselect all\nbeep\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let log = std::fs::read_to_string(boxd.join("run.log")).unwrap();
    assert!(log.contains("load"), "{log}");
    assert!(log.contains("select"));
    // LOG append 追加
    let (code2, _o2, err2) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code2, 0, "{err2}");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_18_at_positional_form_and_silent() {
    let boxd = fresh_dir("18");
    let sb = Sandbox::new();
    let script_path = boxd.join("at.aicscript");
    std::fs::write(
        &script_path,
        format!("load {l} {r}\nfolder-report layout:stats output-to:at.stats\n", l = sb.ls(), r = sb.rs()),
    )
    .unwrap();
    // FR-CLI-002：aic @script 位置参数形态
    let mut cmd = Command::new(harness::cli_bin("aic"));
    cmd.current_dir(&boxd);
    cmd.arg(format!("@{}", script_path.to_str().unwrap()));
    cmd.arg("--silent");
    let out = cmd.output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(boxd.join("at.stats").exists(), "@script 形态执行");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("[1/2]"), "--silent 抑制状态行: {stderr}");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_19_multiline_continuation_and_case() {
    let boxd = fresh_dir("19");
    let sb = Sandbox::new();
    // & 续行 + 大小写不敏感 + 注释
    let src = format!(
        "# 多形态语法\n\
         LoAd &\n   {l} &\n   {r}\n\
         FoLdEr-RePoRt &\n   layout:stats &\n   output-to:case.stats\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(boxd.join("case.stats").exists());
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_20_identical_session_zero_exit() {
    let boxd = fresh_dir("20");
    let sb = Sandbox::new();
    sb.write('l', "same.txt", "same\n");
    sb.write('r', "same.txt", "same\n");
    let src = format!(
        "load {l} {r}\nfolder-report layout:summary output-to:clean.txt\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let report = std::fs::read_to_string(boxd.join("clean.txt")).unwrap();
    assert!(report.contains("相同=1") || report.contains("same=1"), "{report}");
    let _ = std::fs::remove_dir_all(&boxd);
}

#[test]
fn script_21_option_confirm_and_collapse() {
    let boxd = fresh_dir("21");
    let sb = Sandbox::new();
    sb.write('l', "sub/x.txt", "1\n");
    sb.write('r', "sub/x.txt", "2\n");
    let src = format!(
        "load {l} {r}\n\
         option confirm:no-to-all\n\
         option stop-on-error\n\
         expand all\n\
         collapse sub\n\
         folder-report layout:list output-to:collapsed.txt\n",
        l = sb.ls(),
        r = sb.rs()
    );
    let (code, _o, err) = run_script(&boxd, &src, &[], &[]);
    assert_eq!(code, 0, "{err}");
    let report = std::fs::read_to_string(boxd.join("collapsed.txt")).unwrap();
    assert!(!report.contains("x.txt"), "COLLAPSE 后子项不可见: {report}");
    let _ = std::fs::remove_dir_all(&boxd);
}
