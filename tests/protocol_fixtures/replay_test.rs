//! 协议录制回放测试（docs/03 §6.10 / §8.2-3）：
//! 1) 驱动一次引擎会话并 `--record`（双向 NDJSON）；
//! 2) 用录制中的 inbound 行原样回放第二个引擎实例；
//! 3) 断言两次出站序列一致（协议确定性）；
//! 4) 与已固化的录制 fixture 比对（归一化，跨机器稳定）。
//!
//! P3 扩展：session_inputs 覆盖新方法面 —— dir/open 模型路径（displayFilter）+
//! dir/expand、text/lines（分页）、text/inlineDiff（懒计算）、
//! text/sections（withKeys 双键 + importance 重判 / ImportanceUpdate 通知）。
//!
//! 确定性归一化：目录模型会话的 pump 异步推送 `dir/progress` 通知（ADR-004，
//! 顺序约束放宽为 rev 单调），其相对位置不保证跨运行一致 —— 比较时剥除全部
//! dir/progress 通知，另行断言两轮运行的通知计数一致（内容确定性不变）。
//!
//! 重新固化：`UPDATE_GOLDEN=1 cargo test -p ai-compare-tests --test protocol_replay`。

use ai_compare_tests as harness;
use serde_json::{json, Value};

const FIXTURE_NAME: &str = "session1.record.ndjson";

/// 剥除异步推送的 dir/progress 通知（确定性归一化；见模块文档）
fn strip_async_progress(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .filter(|m| m.get("method").and_then(|v| v.as_str()) != Some("dir/progress"))
        .cloned()
        .collect()
}

fn count_progress(messages: &[Value]) -> usize {
    messages
        .iter()
        .filter(|m| m.get("method").and_then(|v| v.as_str()) == Some("dir/progress"))
        .count()
}

#[test]
fn record_then_replay_is_identical() {
    let corpus = harness::build_corpus();
    let record_dir = tempfile::tempdir().expect("record tempdir");
    let record_path = record_dir.path().join(FIXTURE_NAME);

    let inputs = session_inputs(&corpus);

    // ---- 运行 A：录制 ----
    let out_a = harness::run_engine(&inputs, Some(&record_path));
    assert!(record_path.exists(), "--record 应产生录制文件");

    // 解析录制：inbound（喂给回放）/ outbound（期望）
    let record_text = std::fs::read_to_string(&record_path).expect("读录制");
    let mut in_lines: Vec<Value> = Vec::new();
    let mut out_lines: Vec<Value> = Vec::new();
    for line in record_text.lines().filter(|l| !l.is_empty()) {
        let v: Value = serde_json::from_str(line).expect("录制行合法 JSON");
        let dir = v["dir"].as_str().expect("录制行带 dir");
        let msg = v["msg"].clone();
        match dir {
            "in" => in_lines.push(msg),
            "out" => out_lines.push(msg),
            other => panic!("未知录制方向: {other}"),
        }
    }
    assert!(!in_lines.is_empty());
    assert_eq!(in_lines.len(), inputs.len(), "录制应包含全部 inbound 请求");
    assert_eq!(out_lines.len(), out_a.len(), "录制 outbound 与 stdout 一致");

    // ---- 运行 B：回放（同一语料，inbound 逐字重放） ----
    let out_b = harness::run_engine(&in_lines, None);
    // 同步消息逐字一致（协议确定性）；异步 dir/progress 单独比较计数
    assert_eq!(
        strip_async_progress(&out_a),
        strip_async_progress(&out_b),
        "回放出站序列必须与首次运行逐字一致（协议确定性；异步 progress 归一化）"
    );

    // ---- 与固化 fixture 比对（归一化：剥 mtime、路径 token 化、剥异步 progress） ----
    let progress_a = count_progress(&out_a);
    assert!(progress_a >= 1, "模型会话应产生 dir/progress 终态通知（A={progress_a}）");
    assert_eq!(count_progress(&out_b), progress_a, "回放 progress 通知计数一致");

    let mut normalized = strip_async_progress(&out_a);
    for v in normalized.iter_mut() {
        harness::normalize(v, &[(&corpus.left_str(), "<LEFT>"), (&corpus.right_str(), "<RIGHT>")]);
    }
    let actual = record_format(&normalized);
    let fixture_path = harness::fixtures_dir().join(FIXTURE_NAME);
    harness::assert_matches_snapshot(FIXTURE_NAME, &fixture_path, &actual).expect("录制 fixture 一致");
}

fn session_inputs(corpus: &harness::Corpus) -> Vec<Value> {
    vec![
        harness::req(
            1,
            "initialize",
            json!({"clientInfo": {"name": "replay-test", "version": "0"}, "protocolVersion": "1.0"}),
        ),
        harness::req(2, "dir/open", json!({
            "sessionId": "s1",
            "left": corpus.left_str(),
            "right": corpus.right_str(),
            "depth": 1,
            "criteria": { "mtimeToleranceSec": 2, "contentCompare": "always" }
        })),
        harness::note("$/cancel", json!({"id": 999})), // 无在途请求：no-op 通知
        harness::req(3, "text/open", json!({
            "sessionId": "t1",
            "left": corpus.left.join("same.txt").to_string_lossy(),
            "right": corpus.right.join("same.txt").to_string_lossy(),
        })),
        harness::req(4, "text/sections", json!({"sessionId": "t1", "offset": 0, "count": 5})),
        harness::req(5, "session/close", json!({"sessionId": "t1"})),
        harness::req(6, "text/open", json!({
            "sessionId": "t2",
            "left": corpus.left.join("nope.txt").to_string_lossy(),
            "right": corpus.right.join("same.txt").to_string_lossy(),
        })),
        // ---- P3：dir/expand（P2 模型会话；displayFilter 触发模型路径，非惰性全内联） ----
        harness::req(7, "dir/open", json!({
            "sessionId": "s2",
            "left": corpus.left_str(),
            "right": corpus.right_str(),
            "depth": 1,
            "criteria": { "mtimeToleranceSec": 2, "contentCompare": "always" },
            "displayFilter": {}
        })),
        harness::req(8, "dir/expand", json!({"sessionId": "s2", "path": "sub", "depth": 1})),
        harness::req(9, "session/close", json!({"sessionId": "s2"})),
        // ---- P3：text/lines 分页 ----
        harness::req(10, "text/open", json!({
            "sessionId": "t3",
            "left": corpus.left.join("changed.txt").to_string_lossy(),
            "right": corpus.right.join("changed.txt").to_string_lossy(),
        })),
        harness::req(11, "text/lines", json!({"sessionId": "t3", "side": "left", "start": 0, "count": 2})),
        harness::req(12, "text/lines", json!({"sessionId": "t3", "side": "right", "start": 2, "count": 10})),
        harness::req(13, "text/lines", json!({"sessionId": "t3", "side": "left", "start": 100, "count": 5})),
        // ---- P3：text/inlineDiff（changed.txt 的 Change section id=1） ----
        harness::req(14, "text/sections", json!({"sessionId": "t3", "offset": 0, "count": 10})),
        harness::req(15, "text/inlineDiff", json!({"sessionId": "t3", "sectionId": 1})),
        harness::req(16, "text/inlineDiff", json!({"sessionId": "t3", "sectionId": 99})),
        // ---- P3：withKeys 双键 + importance 重判（"two" vs "TWO" 大小写不重要 -> ImportanceUpdate） ----
        harness::req(17, "text/sections", json!({"sessionId": "t3", "offset": 0, "count": 10, "withKeys": true})),
        harness::req(18, "text/sections", json!({
            "sessionId": "t3", "offset": 0, "count": 10,
            "importance": { "caseUnimportant": true }
        })),
        harness::req(19, "session/close", json!({"sessionId": "t3"})),
        // ---- P7：text/merge3/*（三向合并录制面：open/sections/decide/output） ----
        harness::req(20, "text/merge3/open", json!({
            "sessionId": "m1",
            "base": corpus.left.join("same.txt").to_string_lossy(),
            "left": corpus.left.join("changed.txt").to_string_lossy(),
            "right": corpus.right.join("changed.txt").to_string_lossy(),
        })),
        harness::req(21, "text/merge3/sections", json!({"sessionId": "m1", "offset": 0, "count": 10})),
        harness::req(22, "text/merge3/decide", json!({
            "sessionId": "m1",
            "decisions": [{"chunkId": 0, "action": "left"}]
        })),
        harness::req(23, "text/merge3/output", json!({"sessionId": "m1"})),
        harness::req(24, "session/close", json!({"sessionId": "m1"})),
        harness::req(25, "shutdown", json!({})),
    ]
}

/// fixture 采用与引擎录制相同的行格式（dir + msg 的 NDJSON）
fn record_format(out_values: &[Value]) -> String {
    let mut s = String::new();
    for v in out_values {
        let line = serde_json::json!({"dir": "out", "msg": v});
        s.push_str(&serde_json::to_string(&line).expect("fixture 行可序列化"));
        s.push('\n');
    }
    s
}
