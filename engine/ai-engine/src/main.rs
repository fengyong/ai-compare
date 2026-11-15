//! `ai-engine` 进程入口。
//!
//! 用法：
//! ```text
//! ai-engine [--stdio] [--record <file>] [--log-level <level>] [--help] [--version]
//! ```
//!
//! - stdin/stdout 承载 NDJSON JSON-RPC 2.0；日志仅走 stderr；
//! - `--record <file>`：录制双向消息流（回放测试 / 用户问题复现，docs/03 §6.10）。

use std::process::ExitCode;

use ai_engine::{log, server, ServerOptions};

fn main() -> ExitCode {
    match parse_args() {
        Ok(Some(opts)) => run(opts),
        Ok(None) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// 返回 Ok(Some(opts)) 正常运行；Ok(None) 表示 --help/--version 已处理；Err 为错误退出码
fn parse_args() -> Result<Option<ServerOptions>, ExitCode> {
    let mut opts = ServerOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stdio" => {}
            "--record" => match args.next() {
                Some(p) => opts.record_path = Some(std::path::PathBuf::from(p)),
                None => {
                    eprintln!("错误: --record 需要文件路径参数");
                    return Err(ExitCode::from(2));
                }
            },
            "--log-level" => {
                let level = args.next().unwrap_or_default();
                match level.as_str() {
                    "error" | "warn" | "info" | "debug" | "off" => {
                        // log.rs 从环境变量读取级别；此处透传
                        std::env::set_var("AI_ENGINE_LOG", level);
                    }
                    _ => {
                        eprintln!("错误: --log-level 仅支持 error|warn|info|debug|off");
                        return Err(ExitCode::from(2));
                    }
                }
            }
            "--help" | "-h" => {
                print_usage();
                return Ok(None);
            }
            "--version" | "-V" => {
                println!("ai-engine {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            other => {
                eprintln!("未知参数: {other}");
                print_usage();
                return Err(ExitCode::from(2));
            }
        }
    }
    Ok(Some(opts))
}

fn run(opts: ServerOptions) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("错误: tokio 运行时创建失败: {e}");
            return ExitCode::from(1);
        }
    };
    let code = match runtime.block_on(server::run_stdio(opts)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error(format!("服务端异常退出: {e}"));
            ExitCode::from(1)
        }
    };
    // R3-22 工作包 B：`tokio::io::Stdout` 的写入走 tokio blocking 线程池。
    // 读者永不消费时，最后一次 OS write 会一直卡在管道缓冲区上；runtime
    // 默认 drop 会无限等待 blocking 池，导致「引擎逻辑已退出」但进程 hang。
    // 给一个有界关闭窗口：正常路径立即返回；卡住时超时后进程退出，
    // 由 OS 终止残留写线程（客户端已离开，无需保证尾部字节写出）。
    runtime.shutdown_timeout(std::time::Duration::from_millis(300));
    code
}

fn print_usage() {
    eprintln!(
        "ai-engine —— ai-compare headless 引擎进程\n\
         用法: ai-engine [--stdio] [--record <file>] [--log-level <level>]\n\
         传输: JSON-RPC 2.0 over stdio（NDJSON 帧）；日志走 stderr（AI_ENGINE_LOG=error|warn|info|debug）"
    );
}
