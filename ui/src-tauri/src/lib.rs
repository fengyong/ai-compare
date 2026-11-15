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

use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};

const ENGINE_NAME: &str = "ai-engine";
const EVENT_STDOUT: &str = "engine-stdout";
const EVENT_STDERR: &str = "engine-stderr";
const EVENT_EXIT: &str = "engine-exit";
/// 便携模式标记文件（可执行文件旁存在任一即启用相对路径存储，FR-SET-001）
const PORTABLE_MARKERS: [&str; 2] = ["portable.ini", "ai-compare.ini"];

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

#[cfg(debug_assertions)]
fn dev_engine_path() -> PathBuf {
    // ui/src-tauri → 仓库根 target/debug（workspace 根统一输出，docs/03 §7；
    // 根 Cargo.toml 未设 target-dir，workspace 默认 target/ 在仓库根）
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let exe = if cfg!(windows) {
        format!("{ENGINE_NAME}.exe")
    } else {
        ENGINE_NAME.to_string()
    };
    manifest_dir
        .join("..")
        .join("..")
        .join("target")
        .join("debug")
        .join(exe)
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

    // dev 约定路径只在 debug 构建生效：release 包必须用随包引擎，
    // 否则会命中开发机上的 target/debug 二进制（慢且与包内版本不一致）
    #[cfg(debug_assertions)]
    {
        let dev = dev_engine_path();
        if dev.is_file() {
            return Ok(dev);
        }
        tried.push(dev.display().to_string());
    }

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
        "未找到引擎可执行文件（{ENGINE_NAME}）。已尝试：{}\n{}",
        tried.join("；"),
        if cfg!(debug_assertions) {
            "请先构建引擎：在仓库根执行 `cargo build -p ai-engine`，或设置 AI_ENGINE_PATH 指向引擎二进制；浏览器中可用 `pnpm dev` 以内置演示数据运行 UI。"
        } else {
            // 发行包场景：桌面用户没有仓库可构建，缺引擎说明安装包不完整
            "当前安装包不完整（缺少随包引擎 ai-engine）——请重新获取完整安装包；开发者可用 AI_ENGINE_PATH 指向引擎二进制。"
        }
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
    if let Some(p) = guard.as_mut() {
        // 探活（REVIEW P0-1）：子进程已退出（崩溃）则收尸并摘除僵尸条目，
        // 走正常 spawn 路径——否则崩溃后重连永远拿到死 pid，自动重启不可用。
        match p.child.try_wait() {
            Ok(Some(status)) => {
                eprintln!(
                    "[engine] 检测到引擎进程已退出（pid={}，status={status}），清理后重新启动",
                    p.pid
                );
                guard.take();
            }
            Ok(None) => {
                // 仍在运行：幂等返回
                return Ok(EngineStartInfo {
                    pid: p.pid,
                    path: p.path.clone(),
                    engine: ENGINE_NAME.to_string(),
                });
            }
            Err(e) => {
                // 探活失败（罕见）：按已死处理，避免僵尸条目永久驻留
                eprintln!(
                    "[engine] 探活引擎进程失败（pid={}）: {e}，清理后重新启动",
                    p.pid
                );
                guard.take();
            }
        }
    }

    let resolved = match path {
        Some(p) => {
            // R3-23：调用方传入的任意路径只在 debug 构建生效（dev/测试约定）。
            // release 包强制走 resolve_engine_path（随包引擎），避免 WebView 被
            // 注入后借 engine_start 启动任意本地可执行文件。前端正常调用不传 path。
            if cfg!(debug_assertions) {
                let pb = PathBuf::from(&p);
                if pb.is_file() {
                    pb
                } else {
                    return Err(format!("指定的引擎路径不存在: {p}"));
                }
            } else {
                eprintln!("[engine] release 构建忽略调用方引擎路径，改用随包引擎: {p}");
                resolve_engine_path()?
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

// ---------------------------------------------------------------------------
// 设置文件存取（SET-001 / ADR-005）：设置目录 + 便携模式 + 原子写 + .bak
// 语义对齐 ui/src/session/storage.ts 的 AtomicWritePlan（前端纯函数层单测）。
// Tauri 壳只做字节级文件 IO，不做任何 JSON 解析（对齐 ADR-003 壳=管道的边界）。
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AppDirsInfo {
    /// 设置根目录（便携模式 = exe 目录）
    base_dir: String,
    exe_dir: String,
    portable: bool,
}

fn exe_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .ok_or_else(|| "无法定位可执行文件目录".to_string())
}

fn default_base_dir() -> PathBuf {
    // macOS: ~/Library/Application Support/ai-compare；Windows: %APPDATA%/ai-compare；
    // 其余（Linux）: ~/.config/ai-compare（SRS 2.6 平台口径）
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("ai-compare");
    }
    if let Ok(home) = std::env::var("HOME") {
        if cfg!(target_os = "macos") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("ai-compare");
        }
        return PathBuf::from(home).join(".config").join("ai-compare");
    }
    PathBuf::from(".")
}

/// 设置目录推导（便携感知）：返回 (base_dir, exe_dir, portable)。
/// base_dir 是设置文件唯一允许落盘的根（R3-23 命令面收敛的依据）。
fn app_dirs_raw() -> Result<(PathBuf, PathBuf, bool), String> {
    let exe = exe_dir()?;
    let portable = PORTABLE_MARKERS.iter().any(|name| exe.join(name).is_file());
    let base = if portable {
        exe.clone()
    } else {
        default_base_dir()
    };
    Ok((base, exe, portable))
}

/// 设置目录探测：exe 旁 portable.ini / ai-compare.ini → 便携模式（base = exe 目录）
#[tauri::command]
fn settings_app_dirs() -> Result<AppDirsInfo, String> {
    let (base_dir, exe_dir, portable) = app_dirs_raw()?;
    std::fs::create_dir_all(&base_dir).map_err(|e| format!("创建设置目录失败: {e}"))?;
    Ok(AppDirsInfo {
        base_dir: base_dir.display().to_string(),
        exe_dir: exe_dir.display().to_string(),
        portable,
    })
}

/// 把 IPC 传入的路径收敛到 base 内（R3-23）：拒绝绝对越界、`..` 穿越与符号链接逃逸。
/// 前端合法路径恒为 `<base>/<固定文件名>` 或 `<base>/clipboard/<name>`，均在 base 内。
fn confine_within(path: &str, base: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(base).map_err(|e| format!("创建设置目录失败: {e}"))?;
    let real_base = std::fs::canonicalize(base).map_err(|e| format!("解析设置目录失败: {e}"))?;
    let pb = PathBuf::from(path);
    let abs = if pb.is_absolute() {
        pb
    } else {
        real_base.join(pb)
    };
    if abs
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("设置路径越界（禁止 .. 穿越）: {path}"));
    }
    let canon = canonicalize_lenient(&abs)?;
    if canon.starts_with(&real_base) {
        Ok(canon)
    } else {
        Err(format!("设置路径越界（仅限应用设置目录）: {path}"))
    }
}

/// canonicalize 要求路径存在，而写入新文件时目标尚不存在：向上取最近的存在祖先
/// 做 canonicalize（解析符号链接），再补回不存在的尾部（尾部已无 `..`，不会逃逸）。
fn canonicalize_lenient(path: &Path) -> Result<PathBuf, String> {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                existing.pop();
            }
            None => break,
        }
    }
    let mut canon = std::fs::canonicalize(&existing)
        .map_err(|e| format!("解析设置路径失败（{}）: {e}", existing.display()))?;
    for comp in tail.iter().rev() {
        canon.push(comp);
    }
    Ok(canon)
}

/// 读取设置文件（不存在 → found=false；不做解析，损坏交给前端恢复层）
#[tauri::command]
fn settings_read(path: String) -> Result<SettingsReadResult, String> {
    let (base, _, _) = app_dirs_raw()?;
    let confined = confine_within(&path, &base)?;
    settings_read_inner(confined.display().to_string())
}

fn settings_read_inner(path: String) -> Result<SettingsReadResult, String> {
    match std::fs::read_to_string(&path) {
        Ok(contents) => Ok(SettingsReadResult {
            found: true,
            contents: Some(contents),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SettingsReadResult {
            found: false,
            contents: None,
        }),
        Err(e) => Err(format!("读取设置文件失败（{path}）: {e}")),
    }
}

#[derive(Serialize)]
struct SettingsReadResult {
    found: bool,
    contents: Option<String>,
}

/// 原子写设置文件：唯一临时文件 + fsync → 旧内容**复制**为 `<name>.bak` →
/// rename 原子替换 → 父目录 fsync。
///
/// 备份用 copy 而非 rename（R3-13）：rename 会先把主文件搬走，随后的替换一旦
/// 失败主文件就不存在了，旧注释"目标与备份保持旧值"并不成立。copy 之后主文件
/// 始终在位，任何一步失败主文件都仍是旧值（.bak 至多被更新为同一份旧值）。
/// 临时名带 pid + 进程内序号：同进程并发写不再共用一个 tmp 路径。
#[tauri::command]
fn settings_write(path: String, contents: String) -> Result<(), String> {
    let (base, _, _) = app_dirs_raw()?;
    let confined = confine_within(&path, &base)?;
    settings_write_inner(confined.display().to_string(), contents)
}

/// 原子写二进制文件（演示数据落盘用）：与 settings_write 共用同一安全收敛
/// （confine_within，仅限应用设置目录）与同一原子写核心，只是字节来源是
/// Vec<u8> 而非 UTF-8 字符串——图片/ZIP/非 UTF-8 编码等非文本资产无法用字符串
/// 接口表达（任意字节序列未必是合法 UTF-8）。
#[tauri::command]
fn settings_write_bytes(path: String, contents: Vec<u8>) -> Result<(), String> {
    let (base, _, _) = app_dirs_raw()?;
    let confined = confine_within(&path, &base)?;
    settings_write_bytes_inner(confined.display().to_string(), contents)
}

fn settings_write_inner(path: String, contents: String) -> Result<(), String> {
    settings_write_bytes_inner(path, contents.into_bytes())
}

fn settings_write_bytes_inner(path: String, contents: Vec<u8>) -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static WRITE_SEQ: AtomicU64 = AtomicU64::new(0);
    let path_buf = PathBuf::from(&path);
    let parent = path_buf
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| format!("设置路径缺少父目录: {path}"))?;
    std::fs::create_dir_all(&parent).map_err(|e| format!("创建设置目录失败: {e}"))?;

    let file_name = path_buf
        .file_name()
        .ok_or_else(|| format!("设置路径非法: {path}"))?
        .to_string_lossy()
        .into_owned();
    let tmp = parent.join(format!(
        "{}.tmp-{}-{}",
        file_name,
        std::process::id(),
        WRITE_SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&contents)?;
        // 落盘耐久性：rename 之前 tmp 内容必须已刷到介质，否则掉电后
        // 目录项指向一个空/半截文件（文档承诺的"原子写"不含这一步就不成立）
        f.sync_all()?;
        if path_buf.is_file() {
            let bak = parent.join(format!("{file_name}.bak"));
            std::fs::copy(&path_buf, &bak)?;
        }
        std::fs::rename(&tmp, &path_buf)?;
        // 父目录 fsync 让 rename 本身耐久；Windows 无法以 File 打开目录，
        // 且此时数据已可见，故该步失败不得反过来报写入失败。
        if let Ok(dir) = std::fs::File::open(&parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("写设置文件失败（{path}）: {e}")
    })
}

/// 删除设置文件（及其 .bak）。不存在视为成功（幂等）；其余错误必须上抛，
/// 否则"删除会话"会静默失败而 UI 以为已删（R3-13）。
#[tauri::command]
fn settings_delete(path: String) -> Result<(), String> {
    let (base, _, _) = app_dirs_raw()?;
    let confined = confine_within(&path, &base)?;
    settings_delete_inner(confined.display().to_string())
}

fn settings_delete_inner(path: String) -> Result<(), String> {
    for p in [path.clone(), format!("{path}.bak")] {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("删除设置文件失败（{p}）: {e}")),
        }
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(EngineState::default())
        .invoke_handler(tauri::generate_handler![
            engine_start,
            engine_send,
            engine_stop,
            settings_app_dirs,
            settings_read,
            settings_write,
            settings_write_bytes,
            settings_delete
        ])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &std::path::Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn entries(dir: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn settings_write_creates_parent_dirs_and_readable_content() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested/deep/sessions.json");
        settings_write_inner(p.display().to_string(), "{\"v\":1}".into()).unwrap();
        let r = settings_read_inner(p.display().to_string()).unwrap();
        assert!(r.found);
        assert_eq!(r.contents.as_deref(), Some("{\"v\":1}"));
        // 首次写入无旧值 → 不产生 .bak
        assert_eq!(entries(p.parent().unwrap()), vec!["sessions.json"]);
    }

    #[test]
    fn settings_write_backs_up_previous_content() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        settings_write_inner(p.display().to_string(), "v1".into()).unwrap();
        settings_write_inner(p.display().to_string(), "v2".into()).unwrap();
        assert_eq!(read(&p).as_deref(), Some("v2"));
        assert_eq!(read(&p.with_extension("json.bak")).as_deref(), Some("v1"));
        // 第三次写：.bak 跟进到 v2，且不留临时文件
        settings_write_inner(p.display().to_string(), "v3".into()).unwrap();
        assert_eq!(read(&p.with_extension("json.bak")).as_deref(), Some("v2"));
        assert_eq!(entries(dir.path()), vec!["s.json", "s.json.bak"]);
    }

    /// R3-13 核心：失败路径上主文件与 .bak 都保持旧值。
    /// 旧实现先 rename(primary → bak)，替换失败时主文件已不存在。
    /// （权限型断言需非 root 运行。）
    #[cfg(unix)]
    #[test]
    fn settings_write_failure_keeps_primary_and_backup() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        settings_write_inner(p.display().to_string(), "v1".into()).unwrap();
        settings_write_inner(p.display().to_string(), "v2".into()).unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let res = settings_write_inner(p.display().to_string(), "v3".into());

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(res.is_err(), "只读目录写入必须报错，实际: {res:?}");
        assert_eq!(read(&p).as_deref(), Some("v2"), "主文件被破坏");
        assert_eq!(
            read(&p.with_extension("json.bak")).as_deref(),
            Some("v1"),
            ".bak 被破坏"
        );
        assert_eq!(
            entries(dir.path()),
            vec!["s.json", "s.json.bak"],
            "临时文件残留"
        );
    }

    /// 归档路径被换成非空目录 → 原子替换必失败（与 uid 无关的失败注入）。
    #[test]
    fn settings_write_reports_replace_failure_and_cleans_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        settings_write_inner(p.display().to_string(), "v1".into()).unwrap();
        settings_write_inner(p.display().to_string(), "v2".into()).unwrap();
        std::fs::remove_file(&p).unwrap();
        std::fs::create_dir(&p).unwrap();
        std::fs::write(p.join("blocker"), "x").unwrap();

        let res = settings_write_inner(p.display().to_string(), "v3".into());
        assert!(res.is_err(), "实际: {res:?}");
        // .bak 仍是上一次的旧值；父目录无 tmp 残留
        assert_eq!(read(&p.with_extension("json.bak")).as_deref(), Some("v1"));
        assert_eq!(entries(dir.path()), vec!["s.json", "s.json.bak"]);
    }

    /// 同进程并发写：临时名必须每次唯一，否则互相删/覆盖对方的 tmp。
    #[test]
    fn settings_write_concurrent_writes_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        settings_write_inner(p.display().to_string(), "seed".into()).unwrap();
        let path = p.display().to_string();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || settings_write_inner(path, format!("writer-{i}")))
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let final_content = read(&p).unwrap();
        assert!(
            (0..8).any(|i| final_content == format!("writer-{i}")),
            "主文件内容不是任何一次完整写入: {final_content:?}"
        );
        assert_eq!(
            entries(dir.path()),
            vec!["s.json", "s.json.bak"],
            "临时文件残留"
        );
    }

    #[test]
    fn settings_delete_removes_primary_and_backup_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        settings_write_inner(p.display().to_string(), "v1".into()).unwrap();
        settings_write_inner(p.display().to_string(), "v2".into()).unwrap();
        settings_delete_inner(p.display().to_string()).unwrap();
        assert_eq!(entries(dir.path()), Vec::<String>::new());
        // 再删一次仍成功（不存在 = 幂等）
        settings_delete_inner(p.display().to_string()).unwrap();
    }

    /// 删除失败必须上抛（旧实现 `let _ =` 吞掉全部错误）。
    /// .bak 换成非空目录 → remove_file 必失败，与 uid 无关。
    #[test]
    fn settings_delete_reports_errors() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        settings_write_inner(p.display().to_string(), "v1".into()).unwrap();
        let bak = p.with_extension("json.bak");
        std::fs::create_dir(&bak).unwrap();
        std::fs::write(bak.join("blocker"), "x").unwrap();
        let res = settings_delete_inner(p.display().to_string());
        assert!(res.is_err(), "实际: {res:?}");
        assert!(res.unwrap_err().contains("s.json.bak"));
    }

    #[test]
    fn settings_read_missing_file_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let r = settings_read_inner(dir.path().join("nope.json").display().to_string()).unwrap();
        assert!(!r.found);
        assert!(r.contents.is_none());
    }

    /// R3-13 的关键窗口：备份已生成、最终替换失败 → 主文件必须仍是旧值。
    /// 只读目录注入走不到这一步（tmp 会先建不出来）；用 macOS 的 uchg 标记
    /// 让 rename 失败，而 tmp 创建与备份 copy 都正常完成。
    #[cfg(target_os = "macos")]
    #[test]
    fn settings_write_replace_failure_after_backup_keeps_primary() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        settings_write_inner(p.display().to_string(), "v1".into()).unwrap();
        settings_write_inner(p.display().to_string(), "v2".into()).unwrap();
        let flag = |on: bool| {
            let st = std::process::Command::new("chflags")
                .arg(if on { "uchg" } else { "nouchg" })
                .arg(&p)
                .status()
                .unwrap();
            assert!(st.success(), "chflags 执行失败");
        };
        flag(true);
        let res = settings_write_inner(p.display().to_string(), "v3".into());
        flag(false);

        assert!(
            res.is_err(),
            "immutable 目标上的替换必须报错，实际: {res:?}"
        );
        assert_eq!(read(&p).as_deref(), Some("v2"), "主文件丢失或被改写");
        assert_eq!(
            read(&p.with_extension("json.bak")).as_deref(),
            Some("v2"),
            ".bak 必须是同一份旧值"
        );
        assert_eq!(
            entries(dir.path()),
            vec!["s.json", "s.json.bak"],
            "临时文件残留"
        );
    }

    /// 原子替换的可观测语义：替换前已打开的读句柄仍读到完整旧内容。
    /// 就地覆盖会截断同一 inode，旧句柄会看到新内容。
    #[cfg(unix)]
    #[test]
    fn settings_write_replaces_file_without_mutating_open_readers() {
        use std::io::{Read, Seek, SeekFrom};

        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        settings_write_inner(p.display().to_string(), "old-content-longer".into()).unwrap();
        let mut held = std::fs::File::open(&p).unwrap();
        settings_write_inner(p.display().to_string(), "new".into()).unwrap();

        let mut seen = String::new();
        held.seek(SeekFrom::Start(0)).unwrap();
        held.read_to_string(&mut seen).unwrap();
        assert_eq!(
            seen, "old-content-longer",
            "已打开的读句柄看到了新内容 → 不是原子替换"
        );
    }

    // ---- 二进制写（演示数据资产：图片/ZIP/非 UTF-8 编码）----

    /// 非 UTF-8 字节必须精确落盘（PNG 签名含 0x89/0x1A 等 >127 字节，字符串接口
    /// 无法表达）；同时验证建父目录。若误走 UTF-8 字符串路径，0xFF/0xFE 会被改写。
    #[test]
    fn settings_write_bytes_writes_exact_non_utf8_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested/img.png");
        let bytes: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0xFF, 0x00, 0xFE,
        ];
        settings_write_bytes_inner(p.display().to_string(), bytes.clone()).unwrap();
        assert_eq!(
            std::fs::read(&p).unwrap(),
            bytes,
            "字节被改写（疑似走了 UTF-8 字符串路径）"
        );
        assert_eq!(entries(p.parent().unwrap()), vec!["img.png"]);
    }

    /// 二进制写复用同一原子核心：覆盖产生 .bak（旧字节）、无 tmp 残留。
    #[test]
    fn settings_write_bytes_backs_up_previous_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("blob.dat");
        settings_write_bytes_inner(p.display().to_string(), vec![0x00, 0x01, 0x02]).unwrap();
        settings_write_bytes_inner(p.display().to_string(), vec![0x03, 0x04]).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), vec![0x03, 0x04]);
        assert_eq!(
            std::fs::read(p.with_extension("dat.bak")).unwrap(),
            vec![0x00, 0x01, 0x02]
        );
        assert_eq!(entries(dir.path()), vec!["blob.dat", "blob.dat.bak"]);
    }

    /// 文本写委托二进制核心后行为不变（UTF-8 往返一致）。
    #[test]
    fn settings_write_text_and_bytes_share_core() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.txt");
        settings_write_inner(p.display().to_string(), "héllo\n".into()).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), "héllo\n".as_bytes());
        settings_write_bytes_inner(p.display().to_string(), "héllo\n".as_bytes().to_vec()).unwrap();
        assert_eq!(read(&p).as_deref(), Some("héllo\n"));
    }

    // ---- R3-23：设置命令面收敛（confine_within 是纯边界逻辑，命令仅做委托）----

    #[test]
    fn confine_allows_files_and_nested_dirs_within_base() {
        let base = tempfile::tempdir().unwrap();
        let real_base = std::fs::canonicalize(base.path()).unwrap();
        // 直接子文件（尚不存在）与嵌套子目录文件（clipboard/ 尚不存在）都允许
        let f = confine_within(
            &base.path().join("sessions.json").display().to_string(),
            base.path(),
        )
        .unwrap();
        assert_eq!(f, real_base.join("sessions.json"));
        let nested = base.path().join("clipboard/clip-1.txt");
        let n = confine_within(&nested.display().to_string(), base.path()).unwrap();
        assert_eq!(n, real_base.join("clipboard/clip-1.txt"));
    }

    #[test]
    fn confine_relative_path_resolves_under_base() {
        let base = tempfile::tempdir().unwrap();
        let got = confine_within("sessions.json", base.path()).unwrap();
        assert_eq!(
            got,
            std::fs::canonicalize(base.path())
                .unwrap()
                .join("sessions.json")
        );
    }

    #[test]
    fn confine_rejects_absolute_path_outside_base() {
        let base = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let p = outside.path().join("secret.json");
        let res = confine_within(&p.display().to_string(), base.path());
        assert!(res.is_err(), "base 外绝对路径必须被拒绝，实际: {res:?}");
    }

    #[test]
    fn confine_rejects_parent_traversal() {
        let base = tempfile::tempdir().unwrap();
        let escape = base.path().join("../outside.json");
        let res = confine_within(&escape.display().to_string(), base.path());
        assert!(res.is_err(), ".. 穿越必须被拒绝，实际: {res:?}");
        let abs = confine_within("/tmp/../etc/passwd", base.path());
        assert!(abs.is_err(), "绝对路径中的 .. 也必须被拒绝，实际: {abs:?}");
    }

    #[cfg(unix)]
    #[test]
    fn confine_rejects_symlink_escape() {
        let base = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.json"), "x").unwrap();
        let link = base.path().join("evil");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let res = confine_within(&link.join("secret.json").display().to_string(), base.path());
        assert!(res.is_err(), "符号链接逃逸必须被拒绝，实际: {res:?}");
    }
}
