//! ai-compare 桌面壳（Tauri 2）。
//!
//! 职责边界（docs/03 §2.1 / ADR-003）：Tauri 通道仅作字节管道——
//! 本壳只负责 ① 主窗口；② spawn 引擎进程 `ai-engine --stdio` 并把
//! stdin/stdout 按行透传给前端（NDJSON JSON-RPC）。协议语义全部在
//! 前端 `ui/src/protocol/` 与引擎 `ai-engine` 之间，壳层不做任何解析。
//!
//! 引擎二进制解析顺序：
//! 1. 显式参数 / `AI_ENGINE_PATH` 环境变量；
//! 2. debug 构建：`<repo>/engine/target/debug/ai-engine`（编译期定位，dev 约定）；
//! 3. release：与 UI 可执行文件同目录。
//!
//! 找不到引擎时 `engine_start` 返回带指引的错误文案，前端展示优雅降级横幅
//! （可重试或切内置演示引擎）。

use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};

const ENGINE_NAME: &str = "ai-engine";
const EVENT_STDOUT: &str = "engine-stdout";
const EVENT_STDERR: &str = "engine-stderr";
const EVENT_EXIT: &str = "engine-exit";

struct EngineProc {
    child: Child,
    stdin: ChildStdin,
    pid: u32,
    path: String,
}

/// 引擎进程持有状态（单实例；P0 不做引擎重启队列，重连由前端发起）
#[derive(Default)]
struct EngineState(Mutex<Option<EngineProc>>);

#[derive(Serialize)]
struct EngineStartInfo {
    pid: u32,
    path: String,
    engine: String,
}

fn dev_engine_path() -> PathBuf {
    // ui/src-tauri → 仓库根 target/debug（workspace 根统一输出，docs/03 §7；
    // 根 Cargo.toml 未设 target-dir，workspace 默认 target/ 在仓库根）
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let exe = if cfg!(windows) {
        format!("{ENGINE_NAME}.exe")
    } else {
        ENGINE_NAME.to_string()
    };
    manifest_dir.join("..").join("..").join("target").join("debug").join(exe)
}

fn resolve_engine_path() -> Result<PathBuf, String> {
    let mut tried: Vec<String> = Vec::new();

    if let Ok(p) = std::env::var("AI_ENGINE_PATH") {
        let pb = PathBuf::from(&p);
        if pb.is_file() {
            return Ok(pb);
        }
        tried.push(format!("AI_ENGINE_PATH={p}（不存在）"));
    }

    let dev = dev_engine_path();
    if dev.is_file() {
        return Ok(dev);
    }
    tried.push(dev.display().to_string());

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let exe_name = if cfg!(windows) {
                format!("{ENGINE_NAME}.exe")
            } else {
                ENGINE_NAME.to_string()
            };
            let p = dir.join(&exe_name);
            if p.is_file() {
                return Ok(p);
            }
            tried.push(p.display().to_string());
        }
    }

    Err(format!(
        "未找到引擎可执行文件（{ENGINE_NAME}）。已尝试：{}\n请先构建引擎：在仓库根执行 `cargo build -p ai-engine`，\n或设置 AI_ENGINE_PATH 指向引擎二进制；浏览器中可用 `pnpm dev` 以内置演示数据运行 UI。",
        tried.join("；")
    ))
}

/// 启动引擎进程（幂等：已在运行则直接返回信息）。
/// stdout/stderr 各起一个转发线程，按行 emit 事件给 WebView。
#[tauri::command]
fn engine_start(
    app: AppHandle,
    state: State<'_, EngineState>,
    path: Option<String>,
) -> Result<EngineStartInfo, String> {
    let mut guard = state.0.lock().map_err(|e| format!("引擎状态锁中毒: {e}"))?;
    if let Some(p) = guard.as_ref() {
        return Ok(EngineStartInfo {
            pid: p.pid,
            path: p.path.clone(),
            engine: ENGINE_NAME.to_string(),
        });
    }

    let resolved = match path {
        Some(p) => {
            let pb = PathBuf::from(&p);
            if pb.is_file() {
                pb
            } else {
                return Err(format!("指定的引擎路径不存在: {p}"));
            }
        }
        None => resolve_engine_path()?,
    };

    let mut child = Command::new(&resolved)
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("启动引擎失败（{}）: {e}", resolved.display()))?;

    let pid = child.id();
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "无法获取引擎 stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法获取引擎 stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法获取引擎 stderr".to_string())?;

    // stdout 转发线程：NDJSON 按行推送；行流结束（进程退出）时通知前端
    {
        let app = app.clone();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if app.emit(EVENT_STDOUT, line).is_err() {
                    break;
                }
            }
            let _ = app.emit(EVENT_EXIT, serde_json::json!({ "reason": "stdout closed" }));
            // 注：子进程本体在本壳 Exit 时统一回收（见 run()），P0 不做即时 waitpid
        });
    }

    // stderr 转发线程：引擎 tracing 诊断（docs/03 §6.10），仅透传
    {
        let app = app.clone();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if app.emit(EVENT_STDERR, line).is_err() {
                    break;
                }
            }
        });
    }

    let info = EngineStartInfo {
        pid,
        path: resolved.display().to_string(),
        engine: ENGINE_NAME.to_string(),
    };
    *guard = Some(EngineProc {
        child,
        stdin,
        pid: info.pid,
        path: info.path.clone(),
    });
    Ok(info)
}

/// 向引擎 stdin 写一行 NDJSON（UTF-8，自动补换行）
#[tauri::command]
fn engine_send(state: State<'_, EngineState>, line: String) -> Result<(), String> {
    use std::io::Write;
    let mut guard = state.0.lock().map_err(|e| format!("引擎状态锁中毒: {e}"))?;
    let proc = guard
        .as_mut()
        .ok_or_else(|| "引擎未启动（engine_start 未成功调用）".to_string())?;
    writeln!(proc.stdin, "{line}").map_err(|e| format!("写入引擎失败（引擎可能已退出）: {e}"))?;
    proc.stdin
        .flush()
        .map_err(|e| format!("刷新引擎 stdin 失败: {e}"))
}

/// 停止引擎进程（幂等）
#[tauri::command]
fn engine_stop(state: State<'_, EngineState>) -> Result<(), String> {
    if let Ok(mut guard) = state.0.lock() {
        if let Some(mut proc) = guard.take() {
            let _ = proc.child.kill();
            let _ = proc.child.wait();
        }
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(EngineState::default())
        .invoke_handler(tauri::generate_handler![engine_start, engine_send, engine_stop])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // 退出时回收引擎子进程，避免孤儿进程
            if let RunEvent::Exit = event {
                if let Ok(mut guard) = app_handle.state::<EngineState>().0.lock() {
                    if let Some(mut proc) = guard.take() {
                        let _ = proc.child.kill();
                        let _ = proc.child.wait();
                    }
                }
            }
        });
}
