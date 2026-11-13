//! stderr 结构化日志（stdout 只承载 NDJSON 协议流，绝不混入日志）。
//!
//! 级别过滤走环境变量 `AI_ENGINE_LOG`（error/warn/info/debug，默认 info）。
//! 引擎内 P0 手写实现；P1+ 切换 tracing 时保持同样的 stderr 约定。

use std::io::Write;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
        }
    }
}

fn max_level() -> Level {
    static LEVEL: OnceLock<Level> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        match std::env::var("AI_ENGINE_LOG").as_deref() {
            Ok("error") => Level::Error,
            Ok("warn") => Level::Warn,
            Ok("debug") | Ok("trace") => Level::Debug,
            Ok("off") | Ok("none") => Level::Error,
            _ => Level::Info,
        }
    })
}

pub fn error(msg: impl AsRef<str>) {
    emit(Level::Error, msg);
}

pub fn warn(msg: impl AsRef<str>) {
    emit(Level::Warn, msg);
}

pub fn info(msg: impl AsRef<str>) {
    emit(Level::Info, msg);
}

pub fn debug(msg: impl AsRef<str>) {
    emit(Level::Debug, msg);
}

fn emit(level: Level, msg: impl AsRef<str>) {
    if level > max_level() {
        return;
    }
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "[ai-engine {}] {}", level.as_str(), msg.as_ref());
}

/// 测试辅助：当前级别（供 server 判断是否打印）
pub fn enabled(level: Level) -> bool {
    level <= max_level()
}
