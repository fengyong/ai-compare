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


// ---------------------------------------------------------------------------
// P2 大规模语料：确定性伪随机树（PB-03/PB-10 基准与大规模冒烟用）
// ---------------------------------------------------------------------------

/// xorshift64* 确定性伪随机数（无外部依赖；种子相同 -> 树完全一致）
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// [0, n) 均匀
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

/// 大规模语料描述
pub struct BigCorpus {
    pub _dir: tempfile::TempDir,
    pub left: std::path::PathBuf,
    pub right: std::path::PathBuf,
    /// 实际写入的文件数（两侧合计）
    pub files_written: u64,
}

/// 生成确定性伪随机双树（约 `n_files` 个文件 / 每 64 文件一个目录，
/// 目录树三层）。差异分布（按 seed 确定性）：
/// - ~10% 两侧内容不同（同尺寸改一字节 / 尺寸不同各半）；
/// - ~5% 仅左侧、~5% 仅右侧（孤儿）；
/// - 其余两侧相同。
///
/// 返回值含 TempDir（调用方保持存活）。
pub fn build_big_corpus(n_files: u64, seed: u64) -> BigCorpus {
    use std::io::Write as _;
    let dir = tempfile::tempdir().expect("tempdir");
    let left = dir.path().join("left");
    let right = dir.path().join("right");
    std::fs::create_dir_all(&left).unwrap();
    std::fs::create_dir_all(&right).unwrap();
    let mut rng = Rng::new(seed);
    let mut written = 0u64;
    let files_per_dir = 64u64;
    let n_dirs = n_files.div_ceil(files_per_dir);

    // 预生成目录路径（三层：d0/<0..32>/sub<0..2>）
    let mut dir_paths: Vec<String> = Vec::with_capacity(n_dirs as usize);
    for i in 0..n_dirs {
        let top = i % 48;
        let mid = (i / 48) % 32;
        let leaf = i % 3;
        dir_paths.push(format!("d{top}/m{mid}/l{leaf}"));
    }
    for d in &dir_paths {
        std::fs::create_dir_all(left.join(d)).unwrap();
        std::fs::create_dir_all(right.join(d)).unwrap();
    }

    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    for i in 0..n_files {
        let d = &dir_paths[(i % n_dirs) as usize];
        let name = format!("f{:06}.bin", i);
        // 内容 128..2048 字节，模式由 seed 决定
        let size = 128 + rng.below(1920) as usize;
        let pattern = rng.next_u64();
        buf.clear();
        let mut cursor = pattern;
        for _ in 0..size {
            cursor = cursor.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            buf.push((cursor >> 33) as u8);
        }
        let roll = rng.below(100);
        let lp = left.join(d).join(&name);
        let rp = right.join(d).join(&name);
        if roll < 5 {
            // 仅左侧
            let mut f = std::fs::File::create(&lp).unwrap();
            let _ = f.write_all(&buf);
            written += 1;
        } else if roll < 10 {
            // 仅右侧
            let mut f = std::fs::File::create(&rp).unwrap();
            let _ = f.write_all(&buf);
            written += 1;
        } else if roll < 15 {
            // 同尺寸不同内容（右侧一字节差异）
            let mut rb = buf.clone();
            let last = rb.len() - 1;
            rb[last] ^= 0xff;
            let mut f = std::fs::File::create(&lp).unwrap();
            let _ = f.write_all(&buf);
            let mut f = std::fs::File::create(&rp).unwrap();
            let _ = f.write_all(&rb);
            written += 2;
        } else if roll < 20 {
            // 尺寸不同
            let mut rb = buf.clone();
            rb.truncate(rb.len() / 2 + 1);
            let mut f = std::fs::File::create(&lp).unwrap();
            let _ = f.write_all(&buf);
            let mut f = std::fs::File::create(&rp).unwrap();
            let _ = f.write_all(&rb);
            written += 2;
        } else {
            // 两侧相同
            let mut f = std::fs::File::create(&lp).unwrap();
            let _ = f.write_all(&buf);
            let mut f = std::fs::File::create(&rp).unwrap();
            let _ = f.write_all(&buf);
            written += 2;
        }
    }
    BigCorpus { _dir: dir, left, right, files_written: written }
}
