//! 协议合规与引擎进程生命周期回归（深度 review 修复锁定）
//!
//! 覆盖本次深度 review 修复的 6 项引擎侧缺陷：
//! 1. 客户端 EOF 后引擎进程必须退出（pump 持有出站通道克隆 → 挂死）；
//! 2. `"id": null` 的请求是合法请求，应答 id 回显 null（不得误判为非法信封）；
//! 3. JSON 解析失败 / 信封非法时应答 id 为 null（不得固定回 0 与真实请求撞号）；
//! 4. `jsonrpc` 版本必须为 "2.0"，否则 -32600 Invalid Request；
//! 5. `text/merge3/open` 同参数幂等（重挂载不得清空已提交的冲突决策）；
//! 6. handler panic 必须转为 -32603 InternalError（不得让请求永不应答）。

use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ai_compare_tests as harness;
use serde_json::{json, Value};

/// 起引擎：写入原始行（可为非法 JSON）→ 关 stdin → 收集 stdout。
/// 返回 (输出行, 进程是否在限时内退出)。
fn run_engine_raw(lines: &[String], wait: Duration) -> (Vec<Value>, bool) {
    harness::ensure_engine_fresh();
    let bin = harness::engine_bin();
    assert!(
        bin.exists(),
        "引擎二进制不存在: {}（先 `cargo build -p ai-engine --bin ai-engine`）",
        bin.display()
    );
    let mut cmd = Command::new(&bin);
    cmd.arg("--stdio").stdin(Stdio::piped()).stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    let mut child = cmd.spawn().expect("启动 ai-engine");

    let stdin = child.stdin.take().expect("stdin");
    let lines = lines.to_vec();
    let writer = std::thread::spawn(move || {
        let mut w = stdin;
        for line in &lines {
            let _ = w.write_all(line.as_bytes());
            let _ = w.write_all(b"\n");
        }
        // 关闭 stdin → 引擎读到 EOF
    });

    let stdout = child.stdout.take().expect("stdout");
    let reader = std::thread::spawn(move || {
        let mut data = String::new();
        let mut r = stdout;
        let _ = r.read_to_string(&mut data);
        data
    });

    // 计时起点 = 写出输入的时刻（stdin 随即关闭 → 引擎收到 EOF）。
    // 必须在读线程结束之后再取起点，否则"引擎迟迟不退出"会被读线程的
    // 阻塞时间掩盖（读线程要等到引擎退出才 EOF）—— 变异验证踩过此坑。
    let start = Instant::now();
    writer.join().expect("写线程");
    let out = reader.join().expect("读线程");

    // 进程退出轮询（客户端 EOF 后引擎必须自行退出）
    let deadline = start + wait;
    let mut exited = false;
    let mut status = None;
    while Instant::now() < deadline {
        match child.try_wait().expect("try_wait") {
            Some(s) => {
                status = Some(s);
                exited = true;
                break;
            }
            None => std::thread::sleep(Duration::from_millis(25)),
        }
    }
    if status.is_none() {
        let _ = child.kill();
    }
    // 无论哪条路径都回收子进程（避免僵尸进程）
    let _ = child.wait();

    let values = out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("引擎输出必须是合法 NDJSON"))
        .collect();
    (values, exited)
}

/// 按 id 找应答（id 为 JSON number）
fn resp_by_id(outputs: &[Value], id: i64) -> &Value {
    outputs
        .iter()
        .find(|m| m.get("id").and_then(|v| v.as_i64()) == Some(id))
        .unwrap_or_else(|| panic!("缺 id={id} 的应答: {outputs:?}"))
}

/// 按 id 找应答（id 为 JSON null）
fn resp_by_null_id(outputs: &[Value]) -> &Value {
    outputs
        .iter()
        .find(|m| m.get("id").map(|v| v.is_null()).unwrap_or(false))
        .unwrap_or_else(|| panic!("缺 id=null 的应答: {outputs:?}"))
}

fn init_req(id: i64) -> Value {
    harness::req(
        id,
        "initialize",
        json!({"clientInfo": {"name": "conformance", "version": "0"}, "protocolVersion": "1.0"}),
    )
}

// ---------------------------------------------------------------------------
// 1. EOF 后引擎必须退出
// ---------------------------------------------------------------------------

/// dir/open 会创建常驻 delta pump 任务，它持有出站通道（UnboundedSender）的克隆。
/// 若退出路径不取消会话，pump 不退出 → 出站通道永不关闭 → writer 永久阻塞 →
/// 客户端断开后引擎进程挂死（只能 kill -9）。
#[test]
fn engine_exits_after_stdin_eof_with_open_dir_session() {
    let corpus = harness::build_corpus();
    let lines = vec![
        serde_json::to_string(&init_req(1)).unwrap(),
        serde_json::to_string(&harness::req(
            2,
            "dir/open",
            json!({
                "sessionId": "d1",
                "left": corpus.left_str(),
                "right": corpus.right_str(),
                "depth": 3,
                // lazyScan 走 P2 模型路径 —— 该路径会创建常驻 delta pump 任务
                "lazyScan": true,
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"}
            }),
        ))
        .unwrap(),
    ];
    // 窗口取 2s：正常路径 pump ≤150ms 内感知取消退出；超时兜底是 5s，
    // 故 2s 窗口既能抓到"pump 永不退出"，也能抓到"靠超时兜底才退出"。
    let (outputs, exited) = run_engine_raw(&lines, Duration::from_secs(2));
    assert!(
        exited,
        "客户端 EOF 后引擎进程未在 2s 内退出（挂死）：pump 持有出站通道克隆导致 writer 永不结束"
    );
    // 应答本身仍要正确（退出不能吞掉应答）
    assert!(resp_by_id(&outputs, 1).get("result").is_some());
    assert!(resp_by_id(&outputs, 2).get("result").is_some());
}

/// 无任何会话时 EOF 也必须正常退出（基线回归）
#[test]
fn engine_exits_after_stdin_eof_without_session() {
    let lines = vec![serde_json::to_string(&init_req(1)).unwrap()];
    let (outputs, exited) = run_engine_raw(&lines, Duration::from_secs(2));
    assert!(exited, "客户端 EOF 后引擎进程未在 2s 内退出");
    assert!(resp_by_id(&outputs, 1).get("result").is_some());
}

// ---------------------------------------------------------------------------
// 2. "id": null 的合法请求
// ---------------------------------------------------------------------------

#[test]
fn request_with_null_id_is_answered_with_null_id() {
    let lines = vec![serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": null,
        "method": "initialize",
        "params": {"clientInfo": {"name": "c", "version": "0"}, "protocolVersion": "1.0"}
    }))
    .unwrap()];
    let (outputs, _exited) = run_engine_raw(&lines, Duration::from_secs(10));
    let resp = resp_by_null_id(&outputs);
    assert!(
        resp.get("result").is_some(),
        "id=null 的合法请求应得到应答，实际: {resp:?}"
    );
    assert!(
        resp.get("error").is_none(),
        "id=null 的合法请求不应报错，实际: {resp:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. 解析失败 / 信封非法 → id 为 null
// ---------------------------------------------------------------------------

#[test]
fn parse_error_is_answered_with_null_id() {
    // 非法 JSON：无法定位任何 id
    let lines = vec!["{ this is not json".to_string()];
    let (outputs, _exited) = run_engine_raw(&lines, Duration::from_secs(10));
    let resp = resp_by_null_id(&outputs);
    assert_eq!(
        resp.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_i64()),
        Some(-32700),
        "解析失败应为 -32700 ParseError，实际: {resp:?}"
    );
}

#[test]
fn invalid_envelope_echoes_legal_id_when_present() {
    // id 合法但 method 缺失 → Invalid Request，且应回显原 id（不得固定回 0）
    let lines = vec![serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": 42,
        "params": {}
    }))
    .unwrap()];
    let (outputs, _exited) = run_engine_raw(&lines, Duration::from_secs(10));
    let resp = resp_by_id(&outputs, 42);
    assert_eq!(
        resp.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_i64()),
        Some(-32600),
        "信封非法应为 -32600，实际: {resp:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. jsonrpc 版本校验
// ---------------------------------------------------------------------------

#[test]
fn wrong_jsonrpc_version_is_rejected() {
    let lines = vec![serde_json::to_string(&json!({
        "jsonrpc": "1.0",
        "id": 7,
        "method": "initialize",
        "params": {"clientInfo": {"name": "c", "version": "0"}, "protocolVersion": "1.0"}
    }))
    .unwrap()];
    let (outputs, _exited) = run_engine_raw(&lines, Duration::from_secs(10));
    let resp = resp_by_id(&outputs, 7);
    assert_eq!(
        resp.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_i64()),
        Some(-32600),
        "jsonrpc 版本不符应为 -32600，实际: {resp:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. text/merge3/open 幂等（决策不丢）
// ---------------------------------------------------------------------------

/// 同参数重复 open 必须幂等返回既有会话统计，而不是 411 让调用方 close 重建 ——
/// 后者会清空引擎侧已提交的冲突决策（标签重挂载 / StrictMode 双挂载都会触发）。
#[test]
fn merge3_open_is_idempotent_and_keeps_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base.txt");
    let left = dir.path().join("left.txt");
    let right = dir.path().join("right.txt");
    std::fs::write(&base, "a\nb\nc\n").unwrap();
    std::fs::write(&left, "a\nLEFT\nc\n").unwrap();
    std::fs::write(&right, "a\nRIGHT\nc\n").unwrap();
    let (b, l, r) = (
        base.to_string_lossy().into_owned(),
        left.to_string_lossy().into_owned(),
        right.to_string_lossy().into_owned(),
    );

    let inputs = vec![
        harness::req(1, "initialize", json!({"clientInfo": {"name": "c", "version": "0"}, "protocolVersion": "1.0"})),
        harness::req(
            2,
            "text/merge3/open",
            json!({"sessionId": "m1", "base": b, "left": l, "right": r, "options": null}),
        ),
        harness::req(3, "text/merge3/sections", json!({"sessionId": "m1", "offset": 0, "count": 100})),
    ];
    let outputs = harness::run_engine(&inputs, None);
    let opened = resp_by_id(&outputs, 2);
    assert!(
        opened.get("result").is_some(),
        "首次 merge3/open 应成功，实际: {opened:?}"
    );
    let sections = resp_by_id(&outputs, 3);
    let chunks = sections["result"]["chunks"].as_array().expect("chunks 数组");
    let conflict = chunks
        .iter()
        .find(|c| c["origin"] == "conflict")
        .unwrap_or_else(|| panic!("未找到冲突块: {chunks:?}"));
    let chunk_id = conflict["id"].as_u64().expect("chunk id");

    // 提交决策（取左）
    let inputs2 = vec![
        harness::req(1, "initialize", json!({"clientInfo": {"name": "c", "version": "0"}, "protocolVersion": "1.0"})),
        harness::req(
            2,
            "text/merge3/open",
            json!({"sessionId": "m1", "base": b, "left": l, "right": r, "options": null}),
        ),
        harness::req(
            3,
            "text/merge3/decide",
            json!({"sessionId": "m1", "decisions": [{"chunkId": chunk_id, "action": "left"}]}),
        ),
        // 同参数再 open：必须幂等成功（此前返回 411 → UI close+重建 → 决策被清空）
        harness::req(
            4,
            "text/merge3/open",
            json!({"sessionId": "m1", "base": b, "left": l, "right": r, "options": null}),
        ),
        harness::req(5, "text/merge3/sections", json!({"sessionId": "m1", "offset": 0, "count": 100})),
    ];
    let outputs2 = harness::run_engine(&inputs2, None);
    let reopened = resp_by_id(&outputs2, 4);
    assert!(
        reopened.get("result").is_some(),
        "同参数重复 merge3/open 必须幂等成功（决策不能因此丢失），实际: {reopened:?}"
    );
    let sections2 = resp_by_id(&outputs2, 5);
    let chunks2 = sections2["result"]["chunks"].as_array().expect("chunks 数组");
    let still = chunks2
        .iter()
        .find(|c| c["id"].as_u64() == Some(chunk_id))
        .expect("重开后仍应存在该块");
    assert_eq!(
        still["decided"], json!(true),
        "重开后已提交的决策必须保留，实际: {still:?}"
    );
    assert_eq!(still["decision"], json!("left"));
}

/// P2-06：`--record` 目标不可写时必须在 stderr 告警。
///
/// 旧实现 `File::create(p).ok()` 静默禁用录制：实测 exit=0、stderr 无任何
/// 告警、文件未创建 —— 用户以为在录制，问题现场无法复现。
#[test]
fn record_target_unwritable_warns_on_stderr() {
    harness::ensure_engine_fresh();
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("missing-subdir").join("rec.ndjson");
    let mut child = Command::new(harness::engine_bin())
        .args(["--stdio", "--record", bad.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 ai-engine");
    let mut stdin = child.stdin.take().expect("stdin");
    let _ = writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"1.0","clientInfo":{{"name":"t","version":"0"}}}}}}"#
    );
    let _ = writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"shutdown"}}"#);
    drop(stdin);
    let out = child.wait_with_output().expect("等待引擎退出");
    assert!(out.status.success(), "录制目标不可写不应让引擎拒绝服务");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--record"), "必须告警录制已禁用，实际 stderr: {err}");
    assert!(err.contains("missing-subdir"), "告警需含目标路径，实际 stderr: {err}");
    assert!(!bad.exists(), "不可写目标不应被创建");
}

// ---------------------------------------------------------------------------
// R3-21：数字契约（u64/i64 线上为 JSON number + 安全整数边界）
// ---------------------------------------------------------------------------

/// 入站整数超过 2^53-1 时必须在协议边界拒绝，避免客户端 JSON.parse 静默丢精度。
#[test]
fn unsafe_integer_in_request_is_rejected() {
    let lines = vec![
        serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "hex/bytes",
            "params": {"sessionId": "h1", "side": "left", "offset": 9007199254740992u64, "count": 16}
        }))
        .unwrap(),
    ];
    let (outputs, _exited) = run_engine_raw(&lines, Duration::from_secs(5));
    let resp = resp_by_id(&outputs, 9);
    assert_eq!(
        resp.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_i64()),
        Some(-32602),
        "越界整数应回 -32602，实际: {resp:?}"
    );
    assert!(
        resp["error"]["message"].as_str().unwrap_or("").contains("2^53-1"),
        "错误信息应指出安全整数上限: {resp:?}"
    );
}

/// 边界内的整数必须正常放行（不能误伤 epoch 毫秒、文件大小等合法值）。
#[test]
fn max_safe_integer_in_request_is_accepted() {
    let lines = vec![
        serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 10,
            "method": "hex/bytes",
            "params": {"sessionId": "h1", "side": "left", "offset": 9007199254740991u64, "count": 16}
        }))
        .unwrap(),
    ];
    let (outputs, _exited) = run_engine_raw(&lines, Duration::from_secs(5));
    let resp = resp_by_id(&outputs, 10);
    let msg = resp["error"]["message"].as_str().unwrap_or("");
    assert!(
        !msg.contains("2^53-1"),
        "边界内整数不应被安全整数约束拒绝（会话不存在等其它错误不算）: {resp:?}"
    );
}

/// 真实 RPC 应答里的代表性 u64 字段必须是 JSON number（不是 string / 不是 null）。
#[test]
fn rpc_numeric_fields_are_json_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let l = dir.path().join("L");
    let r = dir.path().join("R");
    std::fs::create_dir_all(&l).unwrap();
    std::fs::create_dir_all(&r).unwrap();
    std::fs::write(l.join("a.txt"), b"left").unwrap();
    std::fs::write(r.join("a.txt"), b"right!").unwrap();
    let inputs = vec![
        harness::req(1, "initialize", json!({"clientInfo": {"name": "c", "version": "0"}, "protocolVersion": "1.0"})),
        harness::req(2, "dir/open", json!({
            "sessionId": "num1",
            "left": l.to_string_lossy(),
            "right": r.to_string_lossy(),
            "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"}
        })),
    ];
    let outputs = harness::run_engine(&inputs, None);
    let open = outputs
        .iter()
        .find(|m| m.get("id").and_then(|v| v.as_i64()) == Some(2))
        .expect("dir/open 应答");
    // rev: u64 → JSON number
    assert!(open["result"]["rev"].is_number(), "rev 应为 JSON number: {open}");
    assert!(open["result"]["rowCount"].is_number(), "rowCount 应为 JSON number: {open}");

    // dir/delta 里的 SideInfo.size / mtimeMs 是 u64
    let delta = outputs
        .iter()
        .find(|m| m.get("method") == Some(&json!("dir/delta")))
        .expect("dir/delta 通知");
    let rows = delta["params"]["upserts"].as_array().expect("upserts");
    assert!(!rows.is_empty(), "应有目录行: {delta}");
    for row in rows {
        for side in ["left", "right"] {
            if let Some(info) = row.get(side).filter(|v| !v.is_null()) {
                assert!(info["size"].is_number(), "{side}.size 应为 JSON number: {info}");
                assert!(info["mtimeMs"].is_number(), "{side}.mtimeMs 应为 JSON number: {info}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// R3-22 工作包 A：真实 stdio 交互下的取消能力声明与生命周期
// ---------------------------------------------------------------------------

/// 保持 stdin 打开的交互式引擎。
///
/// `run_engine_raw` 一次性写完就关 stdin，无法表达「先收到应答、再发取消」
/// 这一真实时序；而 `$/cancel {sessionId}` 正是要在应答之后才生效。
struct InteractiveEngine {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    rx: std::sync::mpsc::Receiver<Value>,
    all: Vec<Value>,
}

impl InteractiveEngine {
    fn spawn() -> Self {
        harness::ensure_engine_fresh();
        let mut cmd = Command::new(harness::engine_bin());
        cmd.arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().expect("启动 ai-engine");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(&line) else { break };
                if tx.send(v).is_err() {
                    break;
                }
            }
        });
        InteractiveEngine { child, stdin: Some(stdin), rx, all: Vec::new() }
    }

    fn send(&mut self, v: &Value) {
        let w = self.stdin.as_mut().expect("stdin 仍打开");
        writeln!(w, "{v}").expect("写入请求");
        w.flush().expect("刷新 stdin");
    }

    /// 等待满足条件的消息；途中收到的全部消息记入 `all`（顺序即线上顺序）。
    fn wait_for(&mut self, pred: impl Fn(&Value) -> bool, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match self.rx.recv_timeout(remaining) {
                Ok(v) => {
                    let hit = pred(&v);
                    self.all.push(v.clone());
                    if hit {
                        return Some(v);
                    }
                }
                Err(_) => return None,
            }
        }
    }

    /// 观察一段静默期，返回其间到达的消息数（用于证明后台任务已退出）。
    fn quiet_for(&mut self, d: Duration) -> usize {
        let before = self.all.len();
        let deadline = Instant::now() + d;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.rx.recv_timeout(remaining) {
                Ok(v) => self.all.push(v),
                Err(_) => break,
            }
        }
        self.all.len() - before
    }

    fn finish(mut self, wait: Duration) -> (Vec<Value>, bool) {
        drop(self.stdin.take()); // EOF → 引擎退出
        let deadline = Instant::now() + wait;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.rx.recv_timeout(remaining) {
                Ok(v) => self.all.push(v),
                Err(_) => break,
            }
        }
        let mut exited = false;
        let poll_deadline = Instant::now() + wait;
        while Instant::now() < poll_deadline {
            match self.child.try_wait().expect("try_wait") {
                Some(_) => {
                    exited = true;
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        if !exited {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        (self.all, exited)
    }
}

fn by_id(m: &Value, id: i64) -> bool {
    m.get("id").and_then(|v| v.as_i64()) == Some(id)
}

fn is_cancelled_progress(m: &Value) -> bool {
    m.get("method") == Some(&json!("dir/progress"))
        && m["params"]["phase"] == json!("cancelled")
}

/// 每侧 files 个文件分在 subdirs 个子目录：内容比较下扫描需要多个 await 周期，
/// 保证「慢速大输入」不是一句话，而是取消通知确实能在扫描进行中到达。
fn big_trees(files: usize, subdirs: usize) -> (tempfile::TempDir, String, String) {
    let d = tempfile::tempdir().unwrap();
    let l = d.path().join("L");
    let r = d.path().join("R");
    for i in 0..subdirs {
        std::fs::create_dir_all(l.join(format!("d{i}"))).unwrap();
        std::fs::create_dir_all(r.join(format!("d{i}"))).unwrap();
    }
    for i in 0..files {
        let rel = format!("d{}/f{i}.txt", i % subdirs);
        std::fs::write(l.join(&rel), format!("LEFT-{i}")).unwrap();
        std::fs::write(r.join(&rel), format!("RIGHT-{i}")).unwrap();
    }
    (
        d,
        l.to_string_lossy().into_owned(),
        r.to_string_lossy().into_owned(),
    )
}

/// 能力声明必须精确到方法；`cancel:true` 不再被当成「所有方法可取消」。
#[test]
fn initialize_declares_precise_cancel_scope() {
    let inputs = vec![init_req(1), harness::req(2, "shutdown", json!({}))];
    let outputs = harness::run_engine(&inputs, None);
    let caps = &resp_by_id(&outputs, 1)["result"]["capabilities"];
    assert_eq!(caps["cancel"], true);
    let methods: Vec<&str> = caps["cancelMethods"]
        .as_array()
        .expect("cancelMethods 必须出现")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        methods,
        vec!["dir/open", "dir/execute", "dir/mergeScan", "dir/mergeExecute"]
    );
    assert_eq!(caps["sessionCancel"], true);
    for m in ["text/open", "text/inlineDiff", "report/generate", "dir/filter"] {
        assert!(!methods.contains(&m), "{m} 未接 CancelFlag，不得声明可取消");
    }
}

/// 请求级取消：慢速大输入在途发 `$/cancel {id}`，引擎必须及时停止并回
/// -32800；同一语料不取消时正常成功（证明确实是取消改变了结果，而非语料出错）。
/// 表外方法 text/open 在同样旗标下不受影响（能力表准确）。
#[test]
fn request_cancel_stops_long_scan_and_uncancellable_method_is_unaffected() {
    let (dir, l, r) = big_trees(1200, 24);
    let lp = dir.path().join("left.txt");
    let rp = dir.path().join("right.txt");
    std::fs::write(&lp, "alpha\n").unwrap();
    std::fs::write(&rp, "beta\n").unwrap();

    let mut e = InteractiveEngine::spawn();
    e.send(&init_req(1));
    assert!(e.wait_for(|m| by_id(m, 1), Duration::from_secs(10)).is_some());

    let scan = json!({
        "sessionId": "d1",
        "left": l,
        "right": r,
        "depth": null,
        "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"}
    });
    e.send(&harness::req(2, "dir/open", scan.clone()));
    e.send(&harness::note("$/cancel", json!({"id": 2})));
    let cancelled = e
        .wait_for(|m| by_id(m, 2), Duration::from_secs(30))
        .expect("取消后仍必须应答");
    assert_eq!(
        cancelled["error"]["code"], -32800,
        "请求级取消必须回 -32800（不是让请求永不应答），实际: {cancelled:?}"
    );

    // 对照 1：同一语料不取消能正常扫完（排除语料/路径本身有问题）
    e.send(&harness::req(3, "dir/open", scan));
    let ok = e
        .wait_for(|m| by_id(m, 3), Duration::from_secs(30))
        .expect("未取消的扫描必须应答");
    assert!(ok["result"]["rowCount"].is_number(), "对照扫描应成功: {ok:?}");

    // 对照 2：text/open 不在能力清单里，取消旗标不影响它
    e.send(&harness::req(
        4,
        "text/open",
        json!({"sessionId": "t1", "left": lp.to_string_lossy(), "right": rp.to_string_lossy()}),
    ));
    e.send(&harness::note("$/cancel", json!({"id": 4})));
    let text = e
        .wait_for(|m| by_id(m, 4), Duration::from_secs(10))
        .expect("表外方法也必须应答");
    assert!(text.get("result").is_some(), "表外方法不应被请求级取消中断: {text:?}");

    e.send(&harness::req(5, "shutdown", json!({})));
    let (_all, exited) = e.finish(Duration::from_secs(5));
    assert!(exited, "取消后的会话仍可正常关闭并退出引擎");
}

/// 会话级取消：`dir/open` 应答后按 sessionId 取消，后台扫描/pump 必须停止；
/// close 后不再有通知；同 sessionId 可干净重开，不继承已取消状态。
#[test]
fn session_cancel_after_response_close_and_reopen_are_clean() {
    let (_dir, l, r) = big_trees(900, 16);
    let open = json!({
        "sessionId": "d1",
        "left": l,
        "right": r,
        "depth": 3,
        "lazyScan": true,
        "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"}
    });

    let mut e = InteractiveEngine::spawn();
    e.send(&init_req(1));
    assert!(e.wait_for(|m| by_id(m, 1), Duration::from_secs(10)).is_some());

    e.send(&harness::req(2, "dir/open", open.clone()));
    let opened = e
        .wait_for(|m| by_id(m, 2), Duration::from_secs(20))
        .expect("dir/open 应答");
    assert!(opened["result"]["sessionId"] == json!("d1"), "打开成功: {opened:?}");

    // 应答之后再取消：旧实现此时已注销请求 id，后台扫描再也停不下来
    e.send(&harness::note("$/cancel", json!({"sessionId": "d1"})));
    assert!(
        e.wait_for(is_cancelled_progress, Duration::from_secs(10)).is_some(),
        "会话级取消后必须收到 dir/progress(cancelled)，而不是只有 UI 改文案"
    );
    let cancelled_idx = e
        .all
        .iter()
        .position(is_cancelled_progress)
        .expect("已记录 cancelled 事件");

    e.send(&harness::req(3, "session/close", json!({"sessionId": "d1"})));
    assert!(e.wait_for(|m| by_id(m, 3), Duration::from_secs(10)).is_some());

    let noise = e.quiet_for(Duration::from_millis(500));
    assert_eq!(noise, 0, "close 后后台 pump/worker 必须退出，不再产生通知");
    assert!(
        !e.all[cancelled_idx + 1..]
            .iter()
            .any(|m| m.get("method") == Some(&json!("dir/delta"))),
        "取消之后不得再推脏 delta"
    );

    // 关闭后重开同 sessionId：新旗标不继承旧的已取消状态
    e.send(&harness::req(4, "dir/open", open));
    let reopened = e
        .wait_for(|m| by_id(m, 4), Duration::from_secs(20))
        .expect("重开必须应答");
    assert!(
        reopened.get("result").is_some(),
        "取消后关闭重开应成功（无泄漏/无 411），实际: {reopened:?}"
    );

    e.send(&harness::req(5, "session/close", json!({"sessionId": "d1"})));
    e.send(&harness::req(6, "shutdown", json!({})));
    let (all, exited) = e.finish(Duration::from_secs(5));
    assert!(exited, "引擎必须正常退出（无悬挂后台任务）");
    assert!(by_id(resp_by_id(&all, 6), 6), "shutdown 应答不得被取消流程吞掉");
}
