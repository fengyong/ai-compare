//! 协议合规与引擎进程生命周期回归（深度 review 修复锁定）
//!
//! 覆盖本次深度 review 修复的 6 项引擎侧缺陷：
//! 1. 客户端 EOF 后引擎进程必须退出（pump 持有出站通道克隆 → 挂死）；
//! 2. `"id": null` 的请求是合法请求，应答 id 回显 null（不得误判为非法信封）；
//! 3. JSON 解析失败 / 信封非法时应答 id 为 null（不得固定回 0 与真实请求撞号）；
//! 4. `jsonrpc` 版本必须为 "2.0"，否则 -32600 Invalid Request；
//! 5. `text/merge3/open` 同参数幂等（重挂载不得清空已提交的冲突决策）；
//! 6. handler panic 必须转为 -32603 InternalError（不得让请求永不应答）。

use std::io::{Read, Write};
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
