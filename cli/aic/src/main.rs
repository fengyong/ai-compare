//! `aic` —— ai-compare CLI（进程内链接引擎库；docs/03 §2.1 关键约束 2）。
//!
//! 命令实现见 [`aic::app`]（aicc 等待型包装复用同一实现作为进程内回退）；
//! 退出码契约见 [`aic`] 常量文档（FR-CLI-004 兼容分层）。

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    aic::app::run_cli(&args)
}
