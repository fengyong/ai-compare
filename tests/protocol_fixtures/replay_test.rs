//! 协议录制回放测试（docs/03 §6.10 / §8.2-3）：
//! 1) 驱动一次引擎会话并 `--record`（双向 NDJSON）；
//! 2) 用录制中的 inbound 行原样回放第二个引擎实例；
//! 3) 断言两次出站序列一致（协议确定性）；
//! 4) 与已固化的录制 fixture 比对（归一化，跨机器稳定）。
//!
//! 重新固化：`UPDATE_GOLDEN=1 cargo test -p ai-compare-tests --test protocol_replay`。

use ai_compare_tests as harness;
use serde_json::{json, Value};

const FIXTURE_NAME: &str = "session1.record.ndjson";

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
    assert_eq!(
        out_b, out_a,
        "回放出站序列必须与首次运行逐字一致（协议确定性）"
    );

    // ---- 与固化 fixture 比对（归一化：剥 mtime、路径 token 化） ----
    let mut normalized: Vec<Value> = out_b.clone();
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
        harness::req(7, "shutdown", json!({})),
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
