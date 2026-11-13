//! P9 RPC 端到端（真实 ai-engine 二进制，协议级）：
//! initialize（能力位）→ table/open → table/sections → hex/open → hex/bytes
//! → text/patch/apply（dryRun+应用+已应用拒绝）→ dir/execute（dryRun + 执行
//! + rescan 推 dir/delta）→ shutdown。
//!
//! dir/execute 端到端 = P6 提案接线验收：执行落盘后引擎 rescan 目录会话并
//! 推送 dir/delta（removals + upserts 幂等追平）。

use ai_compare_tests as harness;
use serde_json::{json, Value};

/// 按 id 查找应答（dir/delta 等通知夹在应答之间，下标不可靠）
fn resp_by_id(outputs: &[Value], id: i64) -> &Value {
    outputs
        .iter()
        .find(|m| m.get("id").and_then(|v| v.as_i64()) == Some(id))
        .unwrap_or_else(|| panic!("缺 id={id} 的应答: {outputs:?}"))
}

#[test]
fn p9_rpc_e2e_table_hex_patch_dir_execute() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir_all(&root).unwrap();

    // ---- 表格语料 ----
    let l_csv = root.join("l.csv");
    let r_csv = root.join("r.csv");
    std::fs::write(&l_csv, "id,name,qty\n1,Alpha,10\n2,Beta,20\n3,Gamma,30\n").unwrap();
    std::fs::write(&r_csv, "id,name,qty\n1,Alpha,10\n2,BETA,20\n3,Gamma,35\n4,Delta,40\n").unwrap();

    // ---- hex 语料：前 4 字节同 + 中段变化 + 尾部插入 ----
    let l_bin = root.join("l.bin");
    let r_bin = root.join("r.bin");
    std::fs::write(&l_bin, b"AAAABBBBCCCC").unwrap();
    std::fs::write(&r_bin, b"AAAAXXBBCCCCZZ").unwrap();

    // ---- patch 语料 ----
    std::fs::write(root.join("t.txt"), "1\n2\n3\n").unwrap();
    let patch = "--- a/t.txt\n+++ b/t.txt\n@@ -1,3 +1,3 @@\n 1\n-2\n+TWO\n 3\n";

    // ---- dir 会话语料：L/R 两目录 ----
    let l_dir = dir.path().join("L");
    let r_dir = dir.path().join("R");
    std::fs::create_dir_all(&l_dir).unwrap();
    std::fs::create_dir_all(&r_dir).unwrap();
    std::fs::write(l_dir.join("same.txt"), b"1").unwrap();
    std::fs::write(r_dir.join("same.txt"), b"1").unwrap();
    std::fs::write(l_dir.join("lonly.txt"), b"L").unwrap();
    std::fs::write(r_dir.join("ronly.txt"), b"R").unwrap();

    let root_s = root.to_string_lossy().into_owned();
    let l_dir_s = l_dir.to_string_lossy().into_owned();
    let r_dir_s = r_dir.to_string_lossy().into_owned();

    let inputs = vec![
        harness::req(
            1,
            "initialize",
            json!({"clientInfo": {"name": "p9-e2e", "version": "0"}, "protocolVersion": "1.0"}),
        ),
        // ---- table ----
        harness::req(
            2,
            "table/open",
            json!({
                "sessionId": "tb", "left": l_csv.to_string_lossy(), "right": r_csv.to_string_lossy(),
                "options": {"keyColumns": [0]}
            }),
        ),
        harness::req(3, "table/sections", json!({"sessionId": "tb", "offset": 0, "count": 10})),
        // ---- hex ----
        harness::req(
            4,
            "hex/open",
            json!({"sessionId": "hx", "left": l_bin.to_string_lossy(), "right": r_bin.to_string_lossy()}),
        ),
        harness::req(
            5,
            "hex/bytes",
            json!({"sessionId": "hx", "side": "right", "offset": 0, "length": 16}),
        ),
        // ---- patch: dryRun ----
        harness::req(6, "text/patch/apply", json!({"patch": patch, "root": root_s, "dryRun": true})),
        // ---- patch: 应用 ----
        harness::req(7, "text/patch/apply", json!({"patch": patch, "root": root_s})),
        // ---- patch: 重放 -> alreadyApplied ----
        harness::req(8, "text/patch/apply", json!({"patch": patch, "root": root_s})),
        // ---- dir/open（lazyScan 模型会话） ----
        harness::req(
            9,
            "dir/open",
            json!({
                "sessionId": "d1", "left": l_dir_s, "right": r_dir_s,
                "depth": 1,
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"},
                "lazyScan": true
            }),
        ),
        // ---- dir/execute dryRun：复制 lonly + 删除 ronly ----
        harness::req(
            10,
            "dir/execute",
            json!({
                "sessionId": "d1",
                "ops": [
                    {"kind": "copy", "path": "lonly.txt", "side": "left"},
                    {"kind": "delete", "path": "ronly.txt", "side": "right"}
                ],
                "dryRun": true
            }),
        ),
        // ---- dir/execute 执行（应答后跟 rescan delta 通知） ----
        harness::req(
            11,
            "dir/execute",
            json!({
                "sessionId": "d1",
                "ops": [
                    {"kind": "copy", "path": "lonly.txt", "side": "left"},
                    {"kind": "delete", "path": "ronly.txt", "side": "right"}
                ],
                "dryRun": false
            }),
        ),
        harness::req(12, "session/close", json!({"sessionId": "d1"})),
        harness::req(13, "shutdown", json!({})),
    ];
    let outputs = harness::run_engine(&inputs, None);

    // ---- initialize：P9 能力位 ----
    let caps = &resp_by_id(&outputs, 1)["result"]["capabilities"];
    assert_eq!(caps["tableCompare"], true, "{caps}");
    assert_eq!(caps["hexCompare"], true);
    assert_eq!(caps["patchApply"], true);
    assert_eq!(caps["dirExecute"], true);

    // ---- table/open：4 diff 行（same + changed×2 + added） ----
    let t = &resp_by_id(&outputs, 2)["result"];
    assert_eq!(t["rowCount"], 4, "{t}");
    assert_eq!(t["summary"]["sameRows"], 1);
    assert_eq!(t["summary"]["changedRows"], 2);
    assert_eq!(t["summary"]["addedRows"], 1);
    let columns = t["columns"].as_array().unwrap();
    assert_eq!(columns.len(), 3);
    assert_eq!(columns[0]["ty"], "number", "id 列类型推断");

    // ---- table/sections：行类别与单元格 ----
    let rows = resp_by_id(&outputs, 3)["result"]["rows"].as_array().unwrap();
    assert_eq!(rows[0]["kind"], "same");
    assert_eq!(rows[1]["kind"], "changed");
    assert_eq!(rows[1]["important"], true, "BETA 大小写默认重要");
    assert_eq!(rows[3]["kind"], "added");

    // ---- hex/open：块统计 ----
    let h = &resp_by_id(&outputs, 4)["result"];
    assert_eq!(h["leftSize"], 12, "{h}");
    assert_eq!(h["rightSize"], 14);
    // 全文件 Myers 最小 D=6（l 的 BBBB 只有 2 个 B 可在 r 对齐）：
    // same=10 + changed=1(B↔X) + deleted=1(B) + inserted=3(X+ZZ)
    assert_eq!(h["summary"]["sameBytes"], 10);
    assert_eq!(h["summary"]["changedBytes"], 1);
    assert_eq!(h["summary"]["insertedBytes"], 3);

    // ---- hex/bytes：hex 列 + ASCII 侧栏 + 逐字节状态 ----
    let lines = resp_by_id(&outputs, 5)["result"]["lines"].as_array().unwrap();
    assert_eq!(lines[0]["hex"], "41 41 41 41 58 58 42 42 43 43 43 43 5a 5a");
    assert_eq!(lines[0]["ascii"], "AAAAXXBBCCCCZZ");
    let status = lines[0]["status"].as_str().unwrap();
    assert_eq!(status, "====x+======++", "4 同 + 变化(B↔X) + 插入(X) + 6 同 + 尾部插入(ZZ): {status}");

    // ---- patch：dryRun 报告语义（文件状态断言在下方独立会话验证——
    // 本会话所有请求完成后才做断言，apply 请求已先行落盘） ----
    let dry = &resp_by_id(&outputs, 6)["result"];
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert_eq!(dry["applied"], 1);
    assert_eq!(dry["results"][0]["status"], "applied");
    assert_eq!(dry["allApplied"], true);

    // ---- patch：应用成功 ----
    let applied = &resp_by_id(&outputs, 7)["result"];
    assert_eq!(applied["applied"], 1);
    assert_eq!(applied["allApplied"], true);
    assert_eq!(std::fs::read_to_string(root.join("t.txt")).unwrap(), "1\nTWO\n3\n");

    // ---- patch：重放 -> alreadyApplied（git apply --check 同语义拒绝） ----
    assert_eq!(
        resp_by_id(&outputs, 8)["result"]["results"][0]["status"],
        "alreadyApplied",
        "重放应报 alreadyApplied"
    );

    // ---- dir/execute dryRun：只校验 ----
    let dr = &resp_by_id(&outputs, 10)["result"];
    assert_eq!(dr["dryRun"], true, "{dr}");
    assert_eq!(dr["succeeded"].as_array().unwrap().len(), 2);
    assert_eq!(dr["failed"].as_array().unwrap().len(), 0);
    // dryRun 非落盘由下方独立会话 + handlers 单测覆盖（本会话后续 execute
    // 请求已先行落盘，此处无法按文件状态断言）

    // ---- dir/execute 执行：应答 + rescan dir/delta ----
    let ex = &resp_by_id(&outputs, 11)["result"];
    assert_eq!(ex["dryRun"], false);
    assert_eq!(ex["succeeded"].as_array().unwrap().len(), 2);
    assert!(r_dir.join("lonly.txt").exists(), "复制已落盘");
    assert!(!r_dir.join("ronly.txt").exists(), "删除已落盘");

    // 应答之后的通知 = rescan delta（含 removals + lonly.txt same 行）
    let deltas: Vec<&Value> = outputs
        .iter()
        .filter(|m| m.get("method").and_then(|v| v.as_str()) == Some("dir/delta"))
        .collect();
    assert!(!deltas.is_empty(), "执行后应推送 rescan dir/delta: {:#?}", outputs);
    let mut saw_removal = false;
    let mut saw_lonly_same = false;
    for d in deltas {
        let p = &d["params"];
        if !p["removals"].as_array().unwrap_or(&Vec::new()).is_empty() {
            saw_removal = true;
        }
        for row in p["upserts"].as_array().unwrap_or(&Vec::new()) {
            if row["id"] == "lonly.txt" && row["status"] == "same" {
                saw_lonly_same = true;
            }
        }
    }
    assert!(saw_removal, "rescan delta 含旧行 removals");
    assert!(saw_lonly_same, "rescan delta 含复制后的 same 行");

    // shutdown 应答
    assert_eq!(resp_by_id(&outputs, 13)["result"], Value::Null);
}

/// dry-run 非落盘：独立会话（会话内仅 dryRun，无后续 apply 请求——
/// 落盘断言才可与请求后状态对应）
#[test]
fn p9_rpc_e2e_patch_dry_run_does_not_write() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("t.txt"), "1\n2\n3\n").unwrap();
    let patch = "--- a/t.txt\n+++ b/t.txt\n@@ -1,3 +1,3 @@\n 1\n-2\n+TWO\n 3\n";

    let inputs = vec![
        harness::req(1, "initialize", json!({"clientInfo": {"name": "p9-dry", "version": "0"}, "protocolVersion": "1.0"})),
        harness::req(2, "text/patch/apply", json!({"patch": patch, "root": root.to_string_lossy(), "dryRun": true})),
        harness::req(3, "shutdown", json!({})),
    ];
    let outputs = harness::run_engine(&inputs, None);
    assert_eq!(outputs[1]["result"]["dryRun"], true);
    assert_eq!(outputs[1]["result"]["applied"], 1);
    assert_eq!(outputs[1]["result"]["allApplied"], true);
    // 会话结束文件保持原样（dryRun 不落盘）
    assert_eq!(std::fs::read_to_string(root.join("t.txt")).unwrap(), "1\n2\n3\n", "dryRun 不得写盘");
}
