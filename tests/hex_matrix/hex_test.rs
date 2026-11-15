//! 十六进制对拍矩阵（P9 / docs/04 §P9 验收要点：HEX 边角语料 + xxd 自洽）。
//!
//! 三层验证：
//! 1. **xxd 自洽**：ai-core 16 字节/行渲染与 `xxd -g1` 输出逐行一致
//!    （地址列 / hex 列 / ASCII 侧栏，`.` 占位非可打印字节）；
//! 2. **对齐 golden**：FR-HEX-002 三种对齐模式（Full/Fast/None）在固定
//!    字节语料上的块模型（same/changed/inserted/deleted + 字节区间）；
//! 3. **CLI 面**：`aic hex` 退出码与摘要输出、`aic table` 退出码。

use ai_compare_tests as _;
use ai_core::hex::{
    diff_bytes, render_hex_lines, status_maps, HexAlignMode, HexBlockKind, HexOptions,
};
use std::process::Command;

// ---------------------------------------------------------------------------
// 1. xxd 自洽
// ---------------------------------------------------------------------------

fn xxd_lines(data: &[u8]) -> Vec<(u64, String, String)> {
    let mut cmd = Command::new("xxd");
    cmd.arg("-g1").arg("-"); // 单字节分组：每字节一个空格分隔 token
    use std::io::Write as _;
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("xxd 可用（macOS/Linux 自带）");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(data)
        .expect("写入 xxd stdin");
    let out = child.wait_with_output().expect("xxd 运行");
    assert!(out.status.success(), "xxd 退出码非 0");
    let text = String::from_utf8_lossy(&out.stdout);
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // 固定列宽格式（xxd -g1）：addr(8) ':' ' ' hexarea(47) '  ' ascii(≤16)
        // hexarea = 16 × "xx " 去尾空格；短行以空格填充到 ascii 列起点
        let (addr, rest) = line.split_once(':').expect("xxd 行含地址冒号");
        let body = rest.strip_prefix(' ').unwrap_or(rest);
        let (hex_area, ascii_part) = if body.len() > 49 {
            (&body[..47], &body[49..])
        } else {
            (body.trim_end(), "")
        };
        let hex: String = hex_area
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        rows.push((
            u64::from_str_radix(addr, 16).expect("地址"),
            hex,
            ascii_part.to_string(),
        ));
    }
    rows
}

#[test]
fn render_matches_xxd_fixed_corpora() {
    let corpora: Vec<Vec<u8>> = vec![
        b"Hello, hex world!".to_vec(),
        (0u8..=255).collect(),
        vec![0u8; 33], // 两行 + 1 字节尾行
        b"\x00\x01\x02\x1f\x7f\x80\xff".to_vec(),
        Vec::new(),
        vec![0x20; 16], // 恰一行空格
    ];
    for (i, data) in corpora.iter().enumerate() {
        let total_rows = data.len().div_ceil(16);
        let ours = render_hex_lines(data, 0, total_rows.max(1));
        let theirs = xxd_lines(data);
        assert_eq!(
            ours.len(),
            theirs.len(),
            "语料 {i}：行数与 xxd 不一致（ours={} theirs={}）",
            ours.len(),
            theirs.len()
        );
        for (k, (o, t)) in ours.iter().zip(theirs.iter()).enumerate() {
            assert_eq!(o.addr, t.0, "语料 {i} 行 {k} 地址不一致");
            assert_eq!(
                o.hex, t.1,
                "语料 {i} 行 {k} hex 列不一致:\n ours: {}\n xxd : {}",
                o.hex, t.1
            );
            assert_eq!(
                o.ascii, t.2,
                "语料 {i} 行 {k} ASCII 侧栏不一致:\n ours: {}\n xxd : {}",
                o.ascii, t.2
            );
        }
    }
}

/// 分页窗口渲染与整表渲染的窗口切片一致（hex/bytes 分页语义）
#[test]
fn paged_render_equals_full_render_window() {
    let data: Vec<u8> = (0u8..=255).collect();
    let full = render_hex_lines(&data, 0, 17);
    for start_row in 0..17u64 {
        let page = render_hex_lines(&data, start_row, 1);
        assert_eq!(page.len(), if start_row < 16 { 1 } else { 0 });
        if !page.is_empty() {
            assert_eq!(
                page[0], full[start_row as usize],
                "行 {start_row} 分页切片不一致"
            );
        }
    }
    // 越界行 -> 空页
    assert!(render_hex_lines(&data, 99, 2).is_empty());
}

// ---------------------------------------------------------------------------
// 2. 对齐 golden（FR-HEX-002 三模式）
// ---------------------------------------------------------------------------

fn blocks(m: &ai_core::hex::HexDiffModel) -> Vec<(HexBlockKind, u64, u64, u64, u64)> {
    m.blocks
        .iter()
        .map(|b| {
            (
                b.kind,
                b.left.map_or(u64::MAX, |r| r.offset),
                b.left.map_or(u64::MAX, |r| r.len),
                b.right.map_or(u64::MAX, |r| r.offset),
                b.right.map_or(u64::MAX, |r| r.len),
            )
        })
        .collect()
}

/// None 模式：同偏移逐字节；尾部 inserted/deleted
#[test]
fn golden_none_mode() {
    let opts = HexOptions {
        chunk_size: 4,
        align_mode: HexAlignMode::None,
    };
    let m = diff_bytes(b"AAAABXBB", b"AAAAYXBB", &opts);
    // B≠Y 变化（1 字节），X==X 与后续 BB 同段合并
    assert_eq!(
        blocks(&m),
        vec![
            (HexBlockKind::Same, 0, 4, 0, 4),
            (HexBlockKind::Changed, 4, 1, 4, 1),
            (HexBlockKind::Same, 5, 3, 5, 3)
        ]
    );
    let m = diff_bytes(b"AB", b"ABXY", &opts);
    assert_eq!(
        blocks(&m),
        vec![
            (HexBlockKind::Same, 0, 2, 0, 2),
            (HexBlockKind::Inserted, u64::MAX, u64::MAX, 2, 2)
        ]
    );
    let m = diff_bytes(b"ABXY", b"AB", &opts);
    assert_eq!(
        blocks(&m),
        vec![
            (HexBlockKind::Same, 0, 2, 0, 2),
            (HexBlockKind::Deleted, 2, 2, u64::MAX, u64::MAX)
        ]
    );
}

/// Full 模式：中段插入/删除感知（FR-HEX-002 "完全"）
#[test]
fn golden_full_mode_insert_delete_aware() {
    let opts = HexOptions {
        chunk_size: 4,
        align_mode: HexAlignMode::Full,
    };
    // 中段插入 "brave "
    let m = diff_bytes(b"hello world", b"hello brave world", &opts);
    assert_eq!(m.summary.inserted_bytes, 6, "{m:?}");
    assert_eq!(m.summary.changed_bytes, 0);
    assert_eq!(m.summary.deleted_bytes, 0);
    assert_eq!(m.summary.same_bytes, 11);
    // 中段删除
    let m = diff_bytes(b"hello brave world", b"hello world", &opts);
    assert_eq!(m.summary.deleted_bytes, 6);
    assert_eq!(m.summary.inserted_bytes, 0);
    // 块区间单调且无缝覆盖（两侧独立校验）
    let (mut lo, mut ro) = (0u64, 0u64);
    for b in &m.blocks {
        if let Some(l) = b.left {
            assert_eq!(l.offset, lo);
            lo += l.len;
        }
        if let Some(r) = b.right {
            assert_eq!(r.offset, ro);
            ro += r.len;
        }
    }
    assert_eq!((lo, ro), (m.left_len, m.right_len));
}

/// Fast 模式：块哈希粗对齐 + 块内等位精对齐（无块内插入/删除感知）
#[test]
fn golden_fast_mode_changed_pair() {
    let opts = HexOptions {
        chunk_size: 4,
        align_mode: HexAlignMode::Fast,
    };
    let m = diff_bytes(b"AAAABBBB", b"AAAAXXYY", &opts);
    assert_eq!(
        m.summary.changed_bytes, 4,
        "等位精对齐：BB→XXYY 全部计 changed"
    );
    assert_eq!(m.summary.inserted_bytes + m.summary.deleted_bytes, 0);
}

/// 分块边界跨块变化（chunk_size=1 的退化分块）
#[test]
fn golden_small_chunks_equivalent_to_bytewise() {
    let l: Vec<u8> = (0u8..64).chain([0xEE, 0xFF]).collect();
    let r: Vec<u8> = (0u8..64).chain([0x11, 0x22]).collect();
    let none = diff_bytes(
        &l,
        &r,
        &HexOptions {
            chunk_size: 4,
            align_mode: HexAlignMode::None,
        },
    );
    let full = diff_bytes(
        &l,
        &r,
        &HexOptions {
            chunk_size: 4,
            align_mode: HexAlignMode::Full,
        },
    );
    assert_eq!(
        none.summary, full.summary,
        "尾部替换语料：None 与 Full 统计一致"
    );
    assert_eq!(none.summary.changed_bytes, 2);
}

/// 每字节状态图完整覆盖（same/changed/inserted/deleted 四态）
#[test]
fn status_maps_cover_all_kinds() {
    use ai_core::hex::HexByteStatus as S;
    let l = b"ABCDEF".to_vec();
    let r = b"ABCXYEFgh".to_vec(); // DE→XY changed；gh inserted
    let m = diff_bytes(
        &l,
        &r,
        &HexOptions {
            chunk_size: 3,
            align_mode: HexAlignMode::Full,
        },
    );
    let (ls, rs) = status_maps(&m);
    assert_eq!(ls.len(), 6);
    assert_eq!(rs.len(), 9);
    let n_changed_l = ls.iter().filter(|&&s| s == S::Changed).count();
    let n_same_l = ls.iter().filter(|&&s| s == S::Same).count();
    assert_eq!(n_changed_l + n_same_l, 6, "左视图无 inserted");
    assert_eq!(n_changed_l, 1, "D→X 变化");
    // Myers 最优对齐：Delete(D)+Insert(XY) 的 min 段记 changed，余量 Y 与 gh 记 inserted
    assert_eq!(
        rs.iter().filter(|&&s| s == S::Inserted).count(),
        3,
        "Y + gh inserted"
    );
    assert!(ls.iter().all(|&s| s != S::Inserted));
    assert!(rs.iter().all(|&s| s != S::Deleted));
}

// ---------------------------------------------------------------------------
// 3. CLI 面（aic hex / aic table 退出码）
// ---------------------------------------------------------------------------

fn aic() -> std::path::PathBuf {
    ai_compare_tests::cli_bin("aic")
}

fn run(a: &[&str]) -> (i32, String) {
    let out = Command::new(aic()).args(a).output().expect("aic 启动");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn cli_hex_exit_codes_and_summary() {
    let dir = tempfile::tempdir().unwrap();
    let l = dir.path().join("l.bin");
    let r = dir.path().join("r.bin");
    std::fs::write(&l, b"AAAABBBB").unwrap();
    std::fs::write(&r, b"AAAABBBB").unwrap();
    let (code, out) = run(&["hex", l.to_str().unwrap(), r.to_str().unwrap()]);
    assert_eq!(code, 0, "相同文件退出 0，输出应为空: {out}");
    assert!(out.is_empty());

    std::fs::write(&r, b"AAAAXXBB").unwrap();
    let (code, out) = run(&["hex", l.to_str().unwrap(), r.to_str().unwrap()]);
    assert_eq!(code, 1, "有差异退出 1");
    assert!(out.starts_with("# same="), "摘要头: {out}");
    assert!(out.contains("changed"), "差异块行: {out}");

    // 错误路径：文件不存在 -> 2
    let (code, _out) = run(&["hex", l.to_str().unwrap(), "/nonexistent/xx.bin"]);
    assert_eq!(code, 2);
}

#[test]
fn cli_table_exit_codes_and_tsv() {
    let dir = tempfile::tempdir().unwrap();
    let l = dir.path().join("l.csv");
    let r = dir.path().join("r.csv");
    std::fs::write(&l, "id,v\n1,a\n2,b\n").unwrap();
    std::fs::write(&r, "id,v\n1,a\n2,b\n").unwrap();
    let (code, out) = run(&[
        "table",
        l.to_str().unwrap(),
        r.to_str().unwrap(),
        "--key",
        "0",
    ]);
    assert_eq!(code, 0, "相同表格退出 0: {out}");
    assert!(out.is_empty());

    std::fs::write(&r, "id,v\n1,a\n2,B\n3,c\n").unwrap();
    let (code, out) = run(&[
        "table",
        l.to_str().unwrap(),
        r.to_str().unwrap(),
        "--key",
        "0",
    ]);
    assert_eq!(code, 1, "有差异退出 1");
    assert!(
        out.contains("# same=1\tchanged=1\tadded=1\tremoved=0"),
        "摘要头: {out}"
    );
    assert!(out.contains("changed\t1\t1"), "changed 行 TSV: {out}");
    assert!(out.contains("added\t-\t2"), "added 行 TSV: {out}");

    // --delimiter 单字符 + --no-header
    let (code, out) = run(&[
        "table",
        l.to_str().unwrap(),
        r.to_str().unwrap(),
        "--key",
        "0",
        "--delimiter",
        ",",
        "--no-header",
    ]);
    assert_eq!(code, 1);
    assert!(out.contains("added"), "{out}");
}
