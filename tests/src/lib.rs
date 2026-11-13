//! 集成测试公共设施：引擎二进制定位、NDJSON 会话驱动、语料构建、快照归一化。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 引擎二进制路径（`AIC_ENGINE_BIN` 可覆盖；默认 <workspace>/target/<profile>/ai-engine）
pub fn engine_bin() -> PathBuf {
    if let Ok(p) = std::env::var("AIC_ENGINE_BIN") {
        return PathBuf::from(p);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // <root>/tests
    let root = manifest.parent().expect("workspace root");
    let target = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("target"));
    let target = if target.is_absolute() { target } else { root.join(target) };
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    let mut p = target.join(profile).join("ai-engine");
    if cfg!(windows) {
        p.set_extension("exe");
    }
    p
}

/// 与引擎跑一轮 stdio 会话：写入请求行（逐行 NDJSON）-> 读全部输出行。
/// 输入应以 shutdown 请求收尾（引擎处理后退出）。
pub fn run_engine(inputs: &[serde_json::Value], record: Option<&Path>) -> Vec<serde_json::Value> {
    let bin = engine_bin();
    assert!(
        bin.exists(),
        "引擎二进制不存在: {}（先 `cargo build -p ai-engine --bin ai-engine`，或设置 AIC_ENGINE_BIN）",
        bin.display()
    );
    let mut cmd = Command::new(&bin);
    cmd.arg("--stdio").stdin(Stdio::piped()).stdout(Stdio::piped());
    if let Some(path) = record {
        cmd.arg("--record").arg(path);
    }
    cmd.stderr(Stdio::null());
    let mut child = cmd.spawn().expect("启动 ai-engine");

    // 写线程：先写全部输入再关 stdin（避免大输出时管道死锁）
    let stdin = child.stdin.take().expect("stdin");
    let inputs_clone: Vec<String> = inputs
        .iter()
        .map(|v| serde_json::to_string(v).expect("输入可序列化"))
        .collect();
    let writer = std::thread::spawn(move || {
        let mut w = stdin;
        for line in &inputs_clone {
            let _ = w.write_all(line.as_bytes());
            let _ = w.write_all(b"\n");
        }
    });

    // 读线程：收集 stdout 全部行
    let stdout = child.stdout.take().expect("stdout");
    let reader = std::thread::spawn(move || {
        let mut data = String::new();
        use std::io::Read;
        let mut r = stdout;
        let _ = r.read_to_string(&mut data);
        data
    });

    writer.join().expect("写线程");
    let out = reader.join().expect("读线程");
    let _ = child.wait();

    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("引擎输出必须是合法 NDJSON"))
        .collect()
}

// ---------------------------------------------------------------------------
// 语料：固定目录树 + 文本对（确定性内容；判据 Always 保证状态确定）
// ---------------------------------------------------------------------------

/// 语料根（保持 TempDir 存活）
pub struct Corpus {
    pub _dir: tempfile::TempDir,
    pub left: PathBuf,
    pub right: PathBuf,
}

impl Corpus {
    pub fn left_str(&self) -> String {
        self.left.to_string_lossy().into_owned()
    }
    pub fn right_str(&self) -> String {
        self.right.to_string_lossy().into_owned()
    }
}

/// 标准语料：
/// - same.txt 两侧行等；changed.txt 内容不同；
/// - only_left.txt / only_right.txt 单侧存在；
/// - old_name.txt / new_name.txt 重命名配对（按名对齐下成为 orphan 对）；
/// - bin_same.bin 字节相同；bin_diff.bin 同长不同内容；size_a/size_b 大小不同；
/// - sub/ 两侧目录（depth=1 下 DirSame，内部文件不同不展开）
pub fn build_corpus() -> Corpus {
    let dir = tempfile::tempdir().expect("tempdir");
    let left = dir.path().join("left");
    let right = dir.path().join("right");
    for d in [&left, &right] {
        std::fs::create_dir_all(d.join("sub")).expect("mkdir");
    }
    let same = "alpha\nbeta\ngamma\n";
    std::fs::write(left.join("same.txt"), same).unwrap();
    std::fs::write(right.join("same.txt"), same).unwrap();

    std::fs::write(left.join("changed.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    std::fs::write(right.join("changed.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();

    std::fs::write(left.join("only_left.txt"), "L\n").unwrap();
    std::fs::write(right.join("only_right.txt"), "R\n").unwrap();

    std::fs::write(left.join("old_name.txt"), "renamed\n").unwrap();
    std::fs::write(right.join("new_name.txt"), "renamed\n").unwrap();

    let bin: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
    std::fs::write(left.join("bin_same.bin"), &bin).unwrap();
    std::fs::write(right.join("bin_same.bin"), &bin).unwrap();

    let mut db = vec![7u8; 64];
    db[63] ^= 0xff;
    std::fs::write(left.join("bin_diff.bin"), &db).unwrap();
    let mut db_right = db.clone();
    db_right[0] ^= 0xa5; // 同长度、内容不同（Always 判据下必为 contentDiff）
    std::fs::write(right.join("bin_diff.bin"), &db_right).unwrap();

    std::fs::write(left.join("size_diff.bin"), b"0123456789").unwrap();
    std::fs::write(right.join("size_diff.bin"), b"0123456789AB").unwrap();

    std::fs::write(left.join("sub").join("inner.txt"), "sub left\n").unwrap();
    std::fs::write(right.join("sub").join("inner.txt"), "sub right\n").unwrap();

    Corpus { _dir: dir, left, right }
}

// ---------------------------------------------------------------------------
// 快照归一化
// ---------------------------------------------------------------------------

/// 递归剥离易变字段（mtimeMs）并把任意字符串中出现的实际路径替换为占位 token
pub fn normalize(value: &mut serde_json::Value, roots: &[(&str, &str)]) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("mtimeMs");
            for (_, v) in map.iter_mut() {
                normalize(v, roots);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                normalize(v, roots);
            }
        }
        serde_json::Value::String(s) => {
            for (actual, token) in roots {
                if s.contains(actual) {
                    *s = s.replace(actual, token);
                }
            }
        }
        _ => {}
    }
}

/// golden / fixture 文件目录
pub fn test_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn snapshots_dir() -> PathBuf {
    test_data_dir().join("golden").join("snapshots")
}

pub fn fixtures_dir() -> PathBuf {
    test_data_dir().join("protocol_fixtures")
}

/// 快照 bless 开关：UPDATE_GOLDEN=1 时把实际结果写回快照（需人工 review diff）
pub fn should_bless() -> bool {
    std::env::var("UPDATE_GOLDEN").ok().as_deref() == Some("1")
}

pub fn assert_matches_snapshot(
    name: &str,
    path: &Path,
    actual_pretty: &str,
) -> Result<(), String> {
    if should_bless() {
        std::fs::create_dir_all(path.parent().expect("快照父目录")).expect("创建快照目录");
        std::fs::write(path, actual_pretty).expect("写快照");
        eprintln!("[bless] 已更新快照 {name}: {}", path.display());
        return Ok(());
    }
    let expected = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "快照缺失 {}: {e}（用 UPDATE_GOLDEN=1 cargo test -p ai-compare-tests 生成）",
            path.display()
        )
    })?;
    if expected != actual_pretty {
        let mut diff_note = String::new();
        for (i, (a, b)) in expected.lines().zip(actual_pretty.lines()).enumerate() {
            if a != b {
                diff_note.push_str(&format!("  第 {} 行:\n    期望: {a}\n    实际: {b}\n", i + 1));
                if diff_note.len() > 2000 {
                    break;
                }
            }
        }
        return Err(format!(
            "golden 快照不一致: {name}（{}\n如为有意变更，运行 UPDATE_GOLDEN=1 重新固化并人工 review）",
            diff_note
        ));
    }
    Ok(())
}

/// 组装带 id 的请求
pub fn req(id: i64, method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// 组装通知
pub fn note(method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params})
}
