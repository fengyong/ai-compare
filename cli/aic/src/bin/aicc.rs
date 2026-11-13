//! `aicc` —— 等待型 CLI 包装（FR-CLI-001 aic/aicc 双形态的 aicc 形态）。
//!
//! 用途：版本控制系统（VCS）difftool/mergetool 调用面（BC 的 `bcomp` 对应物）：
//! 调用后**阻塞到比较完成**（无 GUI 形态下等待子进程退出即等待比较结束），
//! 退出码透传被包装命令（0 相同 / 1 不同 / 2 错误 / 11~13 细分）。
//!
//! VCS 约定兼容：
//! - `aicc <left> <right>`：裸双路径即 diff（git difftool 以 `"$LOCAL" "$REMOTE"` 调用）；
//! - `aicc <base> <local> <remote> <merged>`：git mergetool 四路径调用——
//!   以 `<local> <remote>` 两路 diff 展示（三向合并视图属 P7），退出码忽略语义下
//!   git 通过冲突标记判断是否已解决（本形态不写 merged，冒烟脚本验证该约定）；
//! - BC GUI 开关子集（`-fv/--fileviewer`、`--title1/--title2/--lefttitle/--righttitle`、
//!   `--ro/-readonly`、`--filters=`）**接受并忽略**（GUI 会话面属后续阶段）。
//!
//! 实现分层：
//! 1. 同目录存在 `aic` 可执行文件 -> spawn 子进程并等待，透传退出码（真包装）；
//! 2. 否则 -> 进程内直接调用 [`aic::app::run_cli`]（单文件部署的降级路径）。

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mapped = map_vcs_args(&args);

    match mapped {
        None => {
            aic::app::print_usage();
            ExitCode::from(aic::EXIT_ERROR)
        }
        Some(mapped) => {
            if let Some(code) = spawn_wrapped(&mapped) {
                code
            } else {
                // 进程内回退（aic 兄弟二进制缺失：单文件/测试环境）
                aic::app::run_cli(&mapped)
            }
        }
    }
}

/// aicc 专用参数整理：剥除 BC GUI 开关子集、按 VCS 调用形态映射为 aic 参数。
///
/// 返回 None 表示无可用位置参数（打印用法，退出码 2）。
fn map_vcs_args(args: &[String]) -> Option<Vec<String>> {
    // 1. 剥除 GUI 开关子集（接受并忽略）
    let mut rest: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i].as_str();
        let skip_with_value = matches!(a, "--title1" | "--title2" | "--lefttitle" | "--righttitle" | "--title3" | "--title4")
            || a == "-fv"
            || a == "--fileviewer";
        let ignored_flag = a == "--ro" || a == "-ro" || a == "--readonly"
            || a == "-ro1" || a == "-ro2" || a == "--leftreadonly" || a == "--rightreadonly";
        let ignored_kv = a.starts_with("--title1=")
            || a.starts_with("--title2=")
            || a.starts_with("--lefttitle=")
            || a.starts_with("--righttitle=")
            || a.starts_with("--title3=")
            || a.starts_with("--title4=")
            || a.starts_with("-fv=")
            || a.starts_with("--fileviewer=")
            || a.starts_with("--filters=")
            || a.starts_with("-filters=");
        if skip_with_value {
            i += 2; // 开关 + 取值
            continue;
        }
        if ignored_flag || ignored_kv {
            i += 1;
            continue;
        }
        rest.push(args[i].clone());
        i += 1;
    }

    // 2. 无子命令时按 VCS 约定映射
    let has_subcommand = rest
        .first()
        .map(String::as_str)
        .is_some_and(|a| matches!(a, "diff" | "quick" | "help" | "--help" | "-h" | "--version" | "-V"));
    if has_subcommand {
        return Some(rest);
    }

    // 位置路径计数（非 '-' 开头的参数）
    let path_count = rest.iter().filter(|a| !a.starts_with('-')).count();
    if path_count == 0 {
        return None; // 交给 run_cli 打印用法
    }
    // mergetool 四路径约定仅在**纯路径调用**下识别（git mergetool 只传路径不带开关；
    // 带开关时开关的取值本身会被计入路径数，如 --algo myers）
    let has_flags = rest.iter().any(|a| a.starts_with('-'));
    if !has_flags && path_count >= 3 {
        // git mergetool：BASE LOCAL REMOTE [MERGED] -> 以 LOCAL REMOTE 两路展示
        let mut out = vec!["diff".to_string()];
        let mut taken = 0usize;
        for a in &rest {
            taken += 1;
            if taken == 2 || taken == 3 {
                out.push(a.clone());
            }
            if taken == 3 {
                break;
            }
        }
        Some(out)
    } else {
        // difftool / 裸双路径 / 带开关的命令行：默认 diff
        let mut out = vec!["diff".to_string()];
        out.extend(rest);
        Some(out)
    }
}

/// 定位并 spawn `aic`（与本可执行文件同目录）；等待退出并透传退出码。
/// 返回 None 表示未找到 aic（调用方走进程内回退）。
fn spawn_wrapped(args: &[String]) -> Option<ExitCode> {
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.parent()?.join(if cfg!(windows) { "aic.exe" } else { "aic" });
    if !sibling.is_file() {
        return None;
    }
    let status = Command::new(&sibling).args(args).status().ok()?;
    Some(match status.code() {
        Some(code) => ExitCode::from((code & 0xff) as u8),
        // 信号终止（无退出码）：按错误处理
        None => ExitCode::from(aic::EXIT_ERROR),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bare_two_paths_map_to_diff() {
        let m = map_vcs_args(&v(&["a.txt", "b.txt"])).unwrap();
        assert_eq!(m, v(&["diff", "a.txt", "b.txt"]));
    }

    #[test]
    fn flags_and_two_paths_map_to_diff() {
        let m = map_vcs_args(&v(&["--algo", "myers", "-w", "a.txt", "b.txt"])).unwrap();
        assert_eq!(m, v(&["diff", "--algo", "myers", "-w", "a.txt", "b.txt"]));
    }

    #[test]
    fn gui_switches_are_stripped() {
        let m = map_vcs_args(&v(&[
            "--fileviewer=Text Compare",
            "--title1=LEFT",
            "--title2=RIGHT",
            "--ro",
            "--filters=*.cpp;*.h",
            "a.txt",
            "b.txt",
        ]))
        .unwrap();
        assert_eq!(m, v(&["diff", "a.txt", "b.txt"]));
    }

    #[test]
    fn gui_switch_with_separate_value_is_stripped() {
        let m = map_vcs_args(&v(&["-fv", "Text Compare", "--lefttitle", "L", "a.txt", "b.txt"])).unwrap();
        assert_eq!(m, v(&["diff", "a.txt", "b.txt"]));
    }

    #[test]
    fn mergetool_four_paths_use_local_remote() {
        // git mergetool 调用：BASE LOCAL REMOTE MERGED -> diff LOCAL REMOTE
        let m = map_vcs_args(&v(&["base.txt", "local.txt", "remote.txt", "merged.txt"])).unwrap();
        assert_eq!(m, v(&["diff", "local.txt", "remote.txt"]));
    }

    #[test]
    fn flags_with_values_are_not_mistaken_for_paths() {
        // 带开关取值的命令行不走 mergetool 纯路径识别（开关取值不算路径）：
        // --algo myers -w a.txt b.txt -> diff 原样透传
        let m = map_vcs_args(&v(&["--algo", "myers", "-w", "a.txt", "b.txt"])).unwrap();
        assert_eq!(m, v(&["diff", "--algo", "myers", "-w", "a.txt", "b.txt"]));
    }

    #[test]
    fn explicit_subcommand_passes_through() {
        let m = map_vcs_args(&v(&["quick", "a.txt", "b.txt", "--mode", "crc"])).unwrap();
        assert_eq!(m, v(&["quick", "a.txt", "b.txt", "--mode", "crc"]));
        let m = map_vcs_args(&v(&["--help"])).unwrap();
        assert_eq!(m, v(&["--help"]));
    }

    #[test]
    fn no_positional_paths_returns_none() {
        assert!(map_vcs_args(&v(&[])).is_none());
        assert!(map_vcs_args(&v(&["--ro"])).is_none(), "仅 GUI 开关 = 无参数");
    }
}
