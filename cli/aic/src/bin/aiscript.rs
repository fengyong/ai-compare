//! `aiscript` —— 脚本处理器独立入口（P11 / FR-SCR）。
//!
//! 与 `aic script run` 共享同一脚本引擎（`aic::script`），为调度器/CI 提供
//! 专用薄壳（对称于 aic/aicc 双形态约定，docs/03 §2.1 关键约束 2）：
//!
//! ```text
//! aiscript <file.aicscript> [args...] [--var name=value] [--silent]
//! ```
//!
//! 退出码：0 成功 / 105 读脚本失败 / 106 语法错误 / 107 加载失败 /
//! 2 运行期命令失败（stop-on-error）。

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    aic::script::run_cli_script(&args)
}
