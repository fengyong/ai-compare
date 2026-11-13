//! 第一个 golden 测试（docs/04 P0 交付物 ⑧ / docs/03 §8.2-2）：
//! 固定目录树 + 文本对 -> 协议响应快照（JSON 落盘比对）。
//!
//! 确定性保障：判据 `content_compare: Always`（不做 mtime 快速判同），
//! 扫描按名字典序、BFS 序，请求串行处理 —— 输出完全由输入决定。
//! 重新固化：`UPDATE_GOLDEN=1 cargo test -p ai-compare-tests --test golden`。

use ai_compare_tests as harness;
use serde_json::{json, Value};

fn dir_open_params(corpus: &harness::Corpus) -> Value {
    json!({
        "sessionId": "s1",
        "left": corpus.left_str(),
        "right": corpus.right_str(),
        "depth": 1,
        "criteria": { "mtimeToleranceSec": 2, "contentCompare": "always" }
    })
}

#[test]
fn golden_dir_and_text_session_snapshot() {
    let corpus = harness::build_corpus();

    let inputs = vec![
        harness::req(
            1,
            "initialize",
            json!({"clientInfo": {"name": "golden-test", "version": "0"}, "protocolVersion": "1.0"}),
        ),
        harness::req(2, "dir/open", dir_open_params(&corpus)),
        harness::req(3, "text/open", json!({
            "sessionId": "t1",
            "left": corpus.left.join("changed.txt").to_string_lossy(),
            "right": corpus.right.join("changed.txt").to_string_lossy(),
        })),
        harness::req(4, "text/sections", json!({"sessionId": "t1", "offset": 0, "count": 10})),
        harness::req(5, "text/sections", json!({"sessionId": "t1", "offset": 1, "count": 1})),
        harness::req(6, "session/close", json!({"sessionId": "s1"})),
        harness::req(7, "unknown/method", json!({})),
        harness::req(8, "text/sections", json!({"sessionId": "ghost", "offset": 0, "count": 1})),
        harness::req(9, "shutdown", json!({})),
    ];
    let mut outputs = harness::run_engine(&inputs, None);

    // ---- 行为断言（独立于快照，防止快照"盲比对"） ----
    assert_eq!(outputs[0]["id"], 1, "initialize 应答");
    assert_eq!(outputs[0]["result"]["protocolVersion"], "1.0");
    assert_eq!(outputs[0]["result"]["capabilities"]["deltaBatching"], true);

    assert_eq!(outputs[1]["id"], 2);
    // 行数：bin×2 + changed + renamed×2 + only×2 + same + size_diff + sub = 10
    assert_eq!(outputs[1]["result"]["rowCount"], 10);
    assert_eq!(outputs[1]["result"]["scanState"], "completed");

    // delta 通知：1 批、10 行、rev=1
    let delta = &outputs[2];
    assert_eq!(delta["method"], "dir/delta");
    assert_eq!(delta["params"]["rev"], 1);
    assert_eq!(delta["params"]["batch"]["index"], 0);
    assert_eq!(delta["params"]["batch"]["hasMore"], false);
    let upserts = delta["params"]["upserts"].as_array().unwrap();
    assert_eq!(upserts.len(), 10);
    let names: Vec<&str> = upserts.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "bin_diff.bin",
            "bin_same.bin",
            "changed.txt",
            "new_name.txt",
            "old_name.txt",
            "only_left.txt",
            "only_right.txt",
            "same.txt",
            "size_diff.bin",
            "sub",
        ]
    );
    let by_name = |n: &str| upserts
        .iter()
        .find(|r| r["name"] == n)
        .unwrap_or_else(|| panic!("行不存在: {n}，实际行集: {names:?}"));
    assert_eq!(by_name("same.txt")["status"], "same");
    assert_eq!(by_name("changed.txt")["status"], "contentDiff");
    assert_eq!(by_name("bin_diff.bin")["status"], "contentDiff");
    assert_eq!(by_name("bin_same.bin")["status"], "same");
    assert_eq!(by_name("old_name.txt")["status"], "leftOnly");
    assert_eq!(by_name("new_name.txt")["status"], "rightOnly");
    assert_eq!(by_name("size_diff.bin")["status"], "contentDiff");
    assert_eq!(by_name("sub")["status"], "dirSame");
    assert_eq!(by_name("sub")["kind"], "dir");

    // 文本 diff：Equal/Change/Equal/Insert 四段
    assert_eq!(outputs[3]["id"], 3);
    assert_eq!(outputs[3]["result"]["leftLineCount"], 4);
    assert_eq!(outputs[3]["result"]["rightLineCount"], 5);
    assert_eq!(outputs[3]["result"]["sectionCount"], 4);
    assert_eq!(outputs[4]["id"], 4);
    assert_eq!(outputs[4]["result"]["total"], 4);
    assert_eq!(outputs[4]["result"]["hasMore"], false);
    let kinds: Vec<&str> = outputs[4]["result"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["equal", "change", "equal", "insert"]);
    // 窗口取数：offset=1 count=1 -> 只有 change 段，hasMore=true
    assert_eq!(outputs[5]["id"], 5);
    assert_eq!(outputs[5]["result"]["sections"].as_array().unwrap().len(), 1);
    assert_eq!(outputs[5]["result"]["sections"][0]["kind"], "change");
    assert_eq!(outputs[5]["result"]["hasMore"], true);

    // session/close 幂等成功
    assert_eq!(outputs[6]["id"], 6);
    assert!(outputs[6].get("result").is_some());
    // 未知方法 -> -32601
    assert_eq!(outputs[7]["id"], 7);
    assert_eq!(outputs[7]["error"]["code"], -32601);
    // 不存在的会话 -> 410
    assert_eq!(outputs[8]["id"], 8);
    assert_eq!(outputs[8]["error"]["code"], 410);
    // shutdown -> null 应答
    assert_eq!(outputs[9]["id"], 9);
    assert_eq!(outputs[9]["result"], Value::Null);
    assert_eq!(outputs.len(), 10, "应恰好 10 条输出（9 响应 + 1 条 delta 通知）");

    // ---- 快照比对 ----
    for out in outputs.iter_mut() {
        harness::normalize(
            out,
            &[(&corpus.left_str(), "<LEFT>"), (&corpus.right_str(), "<RIGHT>")],
        );
    }
    let actual = serde_json::to_string_pretty(&outputs).expect("快照可序列化");
    let snapshot = harness::snapshots_dir().join("dir_text_session.golden.json");
    harness::assert_matches_snapshot("dir_text_session", &snapshot, &actual).expect("golden 快照一致");
}
