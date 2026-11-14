//! P11 报告 golden 矩阵（docs/04 §P11 验收要点：报告 golden / HTML 归一化校验）：
//!
//! - 语料矩阵（≥12 文本对 + 3 文件夹对 + 表格/十六进制/合并快照）× 8 布局抽查；
//!   关键布局（summary/patch/html/tree/stats）落 golden 快照
//!   （重新固化：`UPDATE_GOLDEN=1 cargo test -p ai-compare-tests --test report_matrix`）；
//! - HTML/XML 结构校验（标签配对/转义）；
//! - roundtrip：aic diff 输出 ≡ patch 布局报告 -> ai-core patch apply 还原右侧文本；
//! - RPC 端到端：真实 ai-engine 二进制 report/generate（text/dir/table/hex 会话 +
//!   分块流 + 能力位）。
//!
//! 确定性保障：渲染器无时钟依赖（时间由模板注入方提供，本矩阵不展开时间占位符），
//! 同输入 -> 同输出，支撑 golden 快照。

use ai_compare_tests as harness;
use ai_core::{
    diff_config, DirReportStats, DirSnapshot, DiffConfig, HexSnapshot, LineTable, ReportEntryKind,
    ReportEntryStatus, ReportLayout, ReportOptions, ReportSideInfo, ReportSnapshot, TableSnapshot,
    TextDiffOptions, TextSnapshot,
};

// ---------------------------------------------------------------------------
// 语料矩阵（12+ 文本对；每对覆盖一类差异形态）
// ---------------------------------------------------------------------------

fn text_corpora() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("identical", "alpha\nbeta\ngamma\n", "alpha\nbeta\ngamma\n"),
        ("simple_change", "one\ntwo\nthree\n", "one\nTWO\nthree\n"),
        ("insert_head", "b\nc\n", "a\nb\nc\n"),
        ("delete_tail", "a\nb\nc\n", "a\nb\n"),
        ("mixed", "a\nb\nc\nd\ne\nf\n", "a\nX\nc\nd\nY\nf\n"),
        ("empty_vs_content", "", "first\nsecond\n"),
        ("content_vs_empty", "only\nleft\n", ""),
        ("crlf_vs_lf_content", "alpha\r\nbeta\r\n", "alpha\ngamma\n"),
        ("unicode", "中文第一行\n第二行\n", "中文第一行\n第二行改\n"),
        ("long_lines", "aaaa bbbb cccc dddd eeee\nnext\n", "AAAA BBBB CCCC DDDD EEEE\nnext\n"),
        ("no_trailing_newline", "x\ny", "x\ny\nz"),
        ("moves_blocks", "1\n2\n3\n4\n5\n6\n7\n8\n9\n", "3\n4\n1\n2\n5\n6\n7\n8\n9\n"),
        ("whitespace_only_diff", "int a;\nint  b;\n", "int a;\nint b;\n"),
        ("interleaved_edits", "a\nb\nc\n", "a\nB\nc\nd\ne\n"),
    ]
}

fn text_snapshot(label: &str, a: &str, b: &str) -> TextSnapshot {
    let lt = LineTable::from_text(a);
    let rt = LineTable::from_text(b);
    let out = diff_config(&lt, &rt, &DiffConfig::default()).unwrap();
    TextSnapshot::from_tables(format!("{label}.left"), format!("{label}.right"), &lt, &rt, out.output.sections)
}

// ---------------------------------------------------------------------------
// 文件夹语料（3 个确定性快照：无 IO——直接构造 arena 等价行）
// ---------------------------------------------------------------------------

fn dir_row(rel: &str, kind: ReportEntryKind, status: ReportEntryStatus, ls: Option<(u64, u64)>, rs: Option<(u64, u64)>) -> ai_core::DirReportRow {
    ai_core::DirReportRow {
        rel_path: rel.into(),
        name: rel.rsplit('/').next().unwrap_or(rel).into(),
        kind,
        status,
        left: ls.map(|(s, m)| ReportSideInfo::new(s, m)),
        right: rs.map(|(s, m)| ReportSideInfo::new(s, m)),
    }
}

fn dir_snapshots() -> Vec<(&'static str, DirSnapshot)> {
    vec![
        (
            "dir_mixed",
            DirSnapshot {
                left_label: "L".into(),
                right_label: "R".into(),
                rows: vec![
                    dir_row("changed.txt", ReportEntryKind::File, ReportEntryStatus::ContentDiff, Some((20, 1000)), Some((24, 2000))),
                    dir_row("only_l.txt", ReportEntryKind::File, ReportEntryStatus::LeftOnly, Some((5, 1500)), None),
                    dir_row("only_r.txt", ReportEntryKind::File, ReportEntryStatus::RightOnly, None, Some((7, 1600))),
                    dir_row("same.txt", ReportEntryKind::File, ReportEntryStatus::Same, Some((10, 999)), Some((10, 999))),
                    dir_row("sub", ReportEntryKind::Dir, ReportEntryStatus::DirDiff, Some((0, 0)), Some((0, 0))),
                    dir_row("sub/inner.txt", ReportEntryKind::File, ReportEntryStatus::ContentDiff, Some((9, 99)), Some((10, 100))),
                ],
                stats: DirReportStats {
                    total: 6,
                    same: 1,
                    content_diff: 3,
                    left_only: 1,
                    right_only: 1,
                    dir_diff: 1,
                    ..Default::default()
                },
            },
        ),
        (
            "dir_identical",
            DirSnapshot {
                left_label: "L".into(),
                right_label: "R".into(),
                rows: vec![dir_row(
                    "a.txt",
                    ReportEntryKind::File,
                    ReportEntryStatus::Same,
                    Some((1, 1)),
                    Some((1, 1)),
                )],
                stats: DirReportStats { total: 1, same: 1, ..Default::default() },
            },
        ),
        (
            "dir_orphans_only",
            DirSnapshot {
                left_label: "L".into(),
                right_label: "R".into(),
                rows: vec![
                    dir_row("l_orphan", ReportEntryKind::Dir, ReportEntryStatus::LeftOnly, Some((0, 5)), None),
                    dir_row("r_orphan.bin", ReportEntryKind::File, ReportEntryStatus::RightOnly, None, Some((8, 6))),
                ],
                stats: DirReportStats { total: 2, left_only: 1, right_only: 1, ..Default::default() },
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// 1) golden 矩阵：12+ 文本语料 × 8 布局抽查 + 关键布局 golden 落盘
// ---------------------------------------------------------------------------

const ALL_LAYOUTS: [ReportLayout; 8] = [
    ReportLayout::Summary,
    ReportLayout::Patch,
    ReportLayout::Html,
    ReportLayout::Xml,
    ReportLayout::Csv,
    ReportLayout::List,
    ReportLayout::Stats,
    ReportLayout::Tree,
];

#[test]
fn report_golden_matrix() {
    let mut blessed_files = 0usize;
    for (name, a, b) in text_corpora() {
        let snap = ReportSnapshot::Text(text_snapshot(name, a, b));
        for layout in ALL_LAYOUTS {
            // tree 布局为目录专用：文本快照必须报不支持（布局×会话矩阵语义）
            let opts = ReportOptions { layout, ..Default::default() };
            let r = ai_core::render_report(&snap, &opts);
            if layout == ReportLayout::Tree {
                assert!(r.is_err(), "{name}/{layout:?}: 文本快照 tree 应不支持");
                continue;
            }
            let text = r.unwrap_or_else(|e| panic!("{name}/{layout:?}: {e}"));
            // 确定性：两次渲染逐字节一致
            let again = ai_core::render_report(&snap, &opts).unwrap();
            assert_eq!(text, again, "{name}/{layout:?} 渲染不确定");
            // stats 布局不得泄漏内容（只统计）
            if layout == ReportLayout::Stats {
                for word in ["alpha", "beta", "中文"] {
                    assert!(!text.contains(word), "{name}/stats 不得含内容词 {word}");
                }
            }
            // golden 快照：关键布局 × 语料（抽查 summary/patch/html/stats）
            if matches!(layout, ReportLayout::Summary | ReportLayout::Patch | ReportLayout::Html | ReportLayout::Stats) {
                let file = harness::snapshots_dir().join("report").join(format!("{name}_{}.golden.txt", layout.as_str()));
                harness::assert_matches_snapshot(
                    &format!("{name}_{}", layout.as_str()),
                    &file,
                    &text,
                )
                .unwrap_or_else(|e| panic!("{e}"));
                blessed_files += 1;
                if harness::should_bless() {
                    // bless 模式下继续写其余快照
                }
            }
        }
    }
    assert!(blessed_files >= 12 * 4, "golden 快照数量不足: {blessed_files}");
}

#[test]
fn dir_report_golden_matrix() {
    for (name, snap) in dir_snapshots() {
        let snap = ReportSnapshot::Dir(snap);
        for layout in ALL_LAYOUTS {
            let opts = ReportOptions { layout, ..Default::default() };
            // patch/interleaved 为文本专用；文本簇外布局对目录快照报不支持
            if matches!(layout, ReportLayout::Patch) {
                assert!(ai_core::render_report(&snap, &opts).is_err(), "{name} dir patch 应不支持");
                continue;
            }
            let text = ai_core::render_report(&snap, &opts).unwrap_or_else(|e| panic!("{name}/{layout:?}: {e}"));
            assert_eq!(text, ai_core::render_report(&snap, &opts).unwrap(), "{name}/{layout:?} 不确定");
            let file = harness::snapshots_dir().join("report").join(format!("{name}_{}.golden.txt", layout.as_str()));
            harness::assert_matches_snapshot(&format!("{name}_{}", layout.as_str()), &file, &text).unwrap_or_else(|e| panic!("{e}"));
        }
    }
}

// ---------------------------------------------------------------------------
// 2) 表格 / 十六进制 / 合并快照 × 8 布局冒烟（引擎侧模型直驱）
// ---------------------------------------------------------------------------

#[test]
fn table_hex_merge_layout_smoke() {
    // 表格
    let model = ai_core::table::diff_tables(
        "id,name,qty\n1,Alpha,10\n2,Beta,20\n",
        "id,name,qty\n1,Alpha,10\n2,BETA,25\n3,Gamma,30\n",
        &ai_core::table::TableDiffOptions::default(),
    )
    .unwrap();
    let table = ReportSnapshot::Table(TableSnapshot {
        left_label: "l.csv".into(),
        right_label: "r.csv".into(),
        columns: model.columns.clone(),
        rows: model.rows.clone(),
        summary: model.summary,
        has_header: true,
    });
    for layout in ALL_LAYOUTS {
        let opts = ReportOptions { layout, ..Default::default() };
        if matches!(layout, ReportLayout::Tree | ReportLayout::Patch | ReportLayout::SideBySide | ReportLayout::Interleaved) {
            assert!(ai_core::render_report(&table, &opts).is_err(), "table {layout:?} 应不支持");
            continue;
        }
        let out = ai_core::render_report(&table, &opts).unwrap();
        assert!(!out.is_empty(), "table {layout:?} 输出为空");
    }
    // CSV 抽查：差异行 + 新增行
    let out = ai_core::render_report(&table, &ReportOptions { layout: ReportLayout::Csv, ..Default::default() }).unwrap();
    assert!(out.contains("changed"), "{out}");
    assert!(out.contains("added"), "{out}");

    // 十六进制
    let hex_model = ai_core::hex::diff_bytes(b"AAAABBBBCCCC", b"AAAAXXBBCCCCZZ", &ai_core::hex::HexOptions::default());
    let (sl, sr) = ai_core::hex::status_maps(&hex_model);
    let hex = ReportSnapshot::Hex(HexSnapshot {
        left_label: "l.bin".into(),
        right_label: "r.bin".into(),
        left: b"AAAABBBBCCCC".to_vec(),
        right: b"AAAAXXBBCCCCZZ".to_vec(),
        model: hex_model,
        status_left: sl,
        status_right: sr,
    });
    for layout in ALL_LAYOUTS {
        let opts = ReportOptions { layout, ..Default::default() };
        if matches!(layout, ReportLayout::Tree | ReportLayout::Patch | ReportLayout::SideBySide | ReportLayout::Interleaved) {
            assert!(ai_core::render_report(&hex, &opts).is_err(), "hex {layout:?} 应不支持");
            continue;
        }
        assert!(!ai_core::render_report(&hex, &opts).unwrap().is_empty(), "hex {layout:?} 输出为空");
    }

    // 合并
    let base = LineTable::from_text("a\nb\nc\n");
    let left = LineTable::from_text("a\nB\nc\n");
    let right = LineTable::from_text("a\nC\nc\n");
    let mm = ai_core::merge3(&base, &left, &right, &ai_core::MergeOptions::default());
    let merge = ReportSnapshot::Merge(ai_core::MergeSnapshot {
        base_label: "base".into(),
        left_label: "l".into(),
        right_label: "r".into(),
        model: mm,
        unresolved: Vec::new(),
    });
    for layout in ALL_LAYOUTS {
        let opts = ReportOptions { layout, ..Default::default() };
        if matches!(layout, ReportLayout::Tree | ReportLayout::Patch | ReportLayout::SideBySide | ReportLayout::Interleaved) {
            assert!(ai_core::render_report(&merge, &opts).is_err(), "merge {layout:?} 应不支持");
            continue;
        }
        assert!(!ai_core::render_report(&merge, &opts).unwrap().is_empty(), "merge {layout:?} 输出为空");
    }
}

// ---------------------------------------------------------------------------
// 3) HTML / XML 结构校验（标签配对 + 转义）——FR-RPT-001 html 归一化验收
// ---------------------------------------------------------------------------

/// 轻量标签配对校验：忽略自闭合与 void 标签，栈式匹配
fn assert_html_balanced(html: &str) {
    let void = ["meta", "br", "hr", "img", "link", "input"];
    let mut stack: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        let after = &rest[lt + 1..];
        let Some(gt) = after.find('>') else { break };
        let tag = &after[..gt];
        let inner = tag.trim();
        if let Some(name) = inner.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            let top = stack.pop().unwrap_or_else(|| panic!("多余闭合 </{name}>"));
            assert_eq!(top, name, "标签错配: 期望 </{top}> 实际 </{name}>");
        } else if inner.starts_with('!') || inner.ends_with('/') || void.contains(&inner.split_whitespace().next().unwrap_or("")) {
            // doctype / 自闭合 / void
        } else {
            let name = inner.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            stack.push(name);
        }
        rest = &after[gt + 1..];
    }
    assert!(stack.is_empty(), "未闭合标签: {stack:?}");
}

#[test]
fn html_structure_and_escaping() {
    for (name, a, b) in text_corpora() {
        let snap = ReportSnapshot::Text(text_snapshot(name, a, b));
        let html = ai_core::render_report(&snap, &ReportOptions { layout: ReportLayout::Html, ..Default::default() }).unwrap();
        assert_html_balanced(&html);
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<style>"), "样式必须内联（自包含）");
        assert!(!html.contains("<style src"), "不得引用外部样式表");
    }
    // 显式转义用例
    let snap = ReportSnapshot::Text(text_snapshot("esc", "<b>&\"q\"</b>\n<script>x</script>\n", "safe\n<script>y</script>\n"));
    let html = ai_core::render_report(&snap, &ReportOptions { layout: ReportLayout::Html, ..Default::default() }).unwrap();
    assert!(html.contains("&lt;b&gt;&amp;&quot;q&quot;&lt;/b&gt;"), "内容须转义:\n{html}");
    assert!(html.contains("&lt;script&gt;"), "script 标签须转义（防注入）");
    assert_html_balanced(&html);
    // mono 模式
    let html = ai_core::render_report(
        &snap,
        &ReportOptions { layout: ReportLayout::Html, color: ai_core::ColorMode::Mono, ..Default::default() },
    )
    .unwrap();
    assert_html_balanced(&html);
    assert!(!html.contains("tr class=\"chg\"{background"), "mono 无配色");
    // word wrap
    let html = ai_core::render_report(
        &snap,
        &ReportOptions { layout: ReportLayout::Html, wrap: ai_core::WrapMode::Word, ..Default::default() },
    )
    .unwrap();
    assert!(html.contains("pre-wrap"));
}

#[test]
fn xml_structure_and_escaping() {
    for (name, a, b) in text_corpora() {
        let snap = ReportSnapshot::Text(text_snapshot(name, a, b));
        let xml = ai_core::render_report(&snap, &ReportOptions { layout: ReportLayout::Xml, ..Default::default() }).unwrap();
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert_eq!(xml.matches("<section ").count(), xml.matches("</section>").count(), "{name}");
        if a.contains('<') || a.contains('&') || b.contains('<') {
            assert!(!xml.contains("<b>"), "原始标签不得直出: {name}");
        }
    }
    // 目录 XML
    for (name, snap) in dir_snapshots() {
        let xml = ai_core::render_report(
            &ReportSnapshot::Dir(snap),
            &ReportOptions { layout: ReportLayout::Xml, ..Default::default() },
        )
        .unwrap();
        assert!(xml.contains("<rows>") && xml.contains("</rows>"), "{name}");
    }
}

/// XML 1.0 的 Char 产生式排除 0x00-0x08/0x0B/0x0C/0x0E-0x1F 与 0xFFFE/0xFFFF；
/// 这些码点连字符引用都非法，直出会让整份报告无法被任何 XML 解析器读取。
#[test]
fn xml_report_strips_illegal_control_chars() {
    fn legal_xml_char(c: char) -> bool {
        matches!(c, '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
    }
    for (label, a, b) in [
        ("ctl", "ok\u{1}bad\n", "ok\u{2}worse\n"),
        ("nul", "a\u{0}b\n", "a\u{8}b\n"),
        ("noncharacter", "x\u{FFFE}y\n", "x\u{FFFF}y\n"),
        ("legal_kept", "tab\tlf\n", "cr\r\n"),
    ] {
        let snap = ReportSnapshot::Text(text_snapshot(label, a, b));
        let xml = ai_core::render_report(&snap, &ReportOptions { layout: ReportLayout::Xml, ..Default::default() }).unwrap();
        let illegal: Vec<String> = xml.chars().filter(|c| !legal_xml_char(*c)).map(|c| format!("U+{:04X}", c as u32)).collect();
        assert!(illegal.is_empty(), "{label}: XML 含非法码点 {illegal:?}");
        assert!(!xml.contains('\u{1}') && !xml.contains('\u{2}'), "{label}: 控制字符必须剔除而非直出");
    }
    // 合法空白（TAB/LF/CR）不得被误删
    let snap = ReportSnapshot::Text(text_snapshot("ws", "a\tb\n", "a\tc\r\n"));
    let xml = ai_core::render_report(&snap, &ReportOptions { layout: ReportLayout::Xml, ..Default::default() }).unwrap();
    assert!(xml.contains('\t'), "合法 TAB 应保留");
}

// ---------------------------------------------------------------------------
// 4) roundtrip：aic diff ≡ patch 布局 -> patch apply 还原
// ---------------------------------------------------------------------------

#[test]
fn patch_layout_roundtrip_with_aic_diff_and_apply() {
    for (name, a, b) in text_corpora() {
        // 1. patch 布局渲染（标签同 aic diff 的 argv 形态）
        let snap = text_snapshot(name, a, b);
        let opts = ReportOptions { layout: ReportLayout::Patch, context_lines: 3, ..Default::default() };
        let patch_text = ai_core::render_report(&ReportSnapshot::Text(snap), &opts).unwrap();

        // 2. 与 aic diff 输出逐字节等价（同一 unified 算法契约，FR-PATCH-010）
        let lt = LineTable::from_text(a);
        let rt = LineTable::from_text(b);
        let diff_out = diff_config(&lt, &rt, &DiffConfig::default()).unwrap();
        let aic_style = aic_style_unified(&format!("{name}.left"), &format!("{name}.right"), &diff_out.output, &lt, &rt, 3);
        assert_eq!(patch_text, aic_style, "{name}: patch 布局应与 aic diff 输出逐字节一致");

        if patch_text.is_empty() {
            // 无差异语料：patch 为空，原文还原
            continue;
        }

        // 3. ai-core patch 解析 + 应用 -> 还原右侧文本（roundtrip 闭环）
        //    补丁引擎保留原文行尾风格（CRLF 容忍语义，patch.rs 头注）：行尾差异
        //    语料（crlf_vs_lf_content）按行内容归一化比对。
        let parsed = ai_core::patch::parse_patch(&patch_text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(parsed.files.len(), 1, "{name}");
        let applied = ai_core::patch::apply_file(a, &parsed.files[0], &ai_core::patch::PatchApplyOptions::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let norm = |t: &str| t.replace("\r\n", "\n");
        match applied {
            ai_core::patch::ApplyOutcome::Applied { text, .. } => {
                assert_eq!(norm(&text), norm(b), "{name}: 应用结果 != 右侧原文")
            }
            ai_core::patch::ApplyOutcome::NoOp => panic!("{name}: 应产生应用结果"),
        }
    }
}

/// 与 aic CLI render_unified 同构（对拍基准；git 行号约定一致）
fn aic_style_unified(
    left_label: &str,
    right_label: &str,
    out: &ai_core::DiffOutput,
    left: &LineTable,
    right: &LineTable,
    context: u32,
) -> String {
    use ai_core::SectionKind;
    #[derive(Clone, Copy, PartialEq)]
    enum Entry {
        Same(u32),
        Left(u32),
        Right(u32),
    }
    let mut script: Vec<Entry> = Vec::new();
    for s in &out.sections {
        match s.kind {
            SectionKind::Equal => {
                let l = s.left.expect("左区间");
                for i in l.start..l.end {
                    script.push(Entry::Same(i));
                }
            }
            SectionKind::Change | SectionKind::Delete => {
                let l = s.left.expect("左区间");
                for i in l.start..l.end {
                    script.push(Entry::Left(i));
                }
                if s.kind == SectionKind::Change {
                    let r = s.right.expect("右区间");
                    for i in r.start..r.end {
                        script.push(Entry::Right(i));
                    }
                }
            }
            SectionKind::Insert => {
                let r = s.right.expect("右区间");
                for i in r.start..r.end {
                    script.push(Entry::Right(i));
                }
            }
        }
    }
    if script.iter().all(|e| matches!(e, Entry::Same(_))) {
        return String::new();
    }
    let ctx = context as usize;
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for (idx, e) in script.iter().enumerate() {
        if !matches!(e, Entry::Same(_)) {
            let s = idx.saturating_sub(ctx);
            let e = (idx + ctx + 1).min(script.len());
            match hunks.last_mut() {
                Some((_, pe)) if s <= *pe => {
                    if e > *pe {
                        *pe = e;
                    }
                }
                _ => hunks.push((s, e)),
            }
        }
    }
    let mut text = String::new();
    text.push_str(&format!("--- {left_label}\n+++ {right_label}\n"));
    let no_nl = |t: &LineTable| !t.is_empty() && t.line(t.len() - 1).ending == ai_core::lines::LineEnding::None;
    let left_no_nl = no_nl(left);
    let right_no_nl = no_nl(right);
    let last_left = left.len().saturating_sub(1) as u32;
    let last_right = right.len().saturating_sub(1) as u32;
    const NO_NEWLINE_MARKER: &str = "\\ No newline at end of file\n";
    for (hs, he) in &hunks {
        let (mut lb, mut rb) = (0u32, 0u32);
        for e in &script[..*hs] {
            match e {
                Entry::Same(_) => {
                    lb += 1;
                    rb += 1;
                }
                Entry::Left(_) => lb += 1,
                Entry::Right(_) => rb += 1,
            }
        }
        let (mut lc, mut rc) = (0u32, 0u32);
        for e in &script[*hs..*he] {
            match e {
                Entry::Same(_) => {
                    lc += 1;
                    rc += 1;
                }
                Entry::Left(_) => lc += 1,
                Entry::Right(_) => rc += 1,
            }
        }
        let head = |before: u32, count: u32, sign: char| -> String {
            let start = if count > 0 { before + 1 } else { before };
            if count == 1 {
                format!("{sign}{start}")
            } else {
                format!("{sign}{start},{count}")
            }
        };
        text.push_str(&format!("@@ {} {} @@\n", head(lb, lc, '-'), head(rb, rc, '+')));
        for e in &script[*hs..*he] {
            match e {
                Entry::Same(i) => {
                    text.push_str(&format!(" {}\n", left.line(*i as usize).text));
                    if (*i == last_left && left_no_nl) || (*i == last_right && right_no_nl) {
                        text.push_str(NO_NEWLINE_MARKER);
                    }
                }
                Entry::Left(i) => {
                    text.push_str(&format!("-{}\n", left.line(*i as usize).text));
                    if *i == last_left && left_no_nl {
                        text.push_str(NO_NEWLINE_MARKER);
                    }
                }
                Entry::Right(i) => {
                    text.push_str(&format!("+{}\n", right.line(*i as usize).text));
                    if *i == last_right && right_no_nl {
                        text.push_str(NO_NEWLINE_MARKER);
                    }
                }
            }
        }
    }
    text
}

/// aic diff 二进制对拍（进程级）：`aic diff a.txt b.txt` 输出 ≡ patch 布局报告
#[test]
fn aic_diff_binary_matches_patch_layout() {
    let dir = tempfile::tempdir().unwrap();
    let l = dir.path().join("a.txt");
    let r = dir.path().join("b.txt");
    std::fs::write(&l, "alpha\nbeta\ngamma\ndelta\n").unwrap();
    std::fs::write(&r, "alpha\nBETA\ngamma\ndelta\nepsilon\n").unwrap();

    let out = std::process::Command::new(harness::cli_bin("aic"))
        .args(["diff", l.to_str().unwrap(), r.to_str().unwrap()])
        .output()
        .expect("aic diff");
    assert_eq!(out.status.code(), Some(1), "差异退出码 1");
    let aic_text = String::from_utf8(out.stdout).unwrap();

    // patch 布局（引擎库内渲染，标签 = argv 路径原串）
    let lt_text = std::fs::read_to_string(&l).unwrap();
    let rt_text = std::fs::read_to_string(&r).unwrap();
    let snap = text_snap_with_labels(&lt_text, &rt_text, l.to_str().unwrap(), r.to_str().unwrap());
    let patch = ai_core::render_report(
        &ReportSnapshot::Text(snap),
        &ReportOptions { layout: ReportLayout::Patch, context_lines: 3, ..Default::default() },
    )
    .unwrap();
    assert_eq!(aic_text, patch, "aic diff 输出 ≡ patch 布局报告（逐字节）");

    // 还原闭环：patch apply -> b.txt 原文
    let parsed = ai_core::patch::parse_patch(&patch).unwrap();
    let applied = ai_core::patch::apply_file(&lt_text, &parsed.files[0], &ai_core::patch::PatchApplyOptions::default()).unwrap();
    assert!(matches!(applied, ai_core::patch::ApplyOutcome::Applied { ref text, .. } if text == &rt_text));
}

fn text_snap_with_labels(a: &str, b: &str, la: &str, lb: &str) -> TextSnapshot {
    let lt = LineTable::from_text(a);
    let rt = LineTable::from_text(b);
    let out = diff_text_default(&lt, &rt);
    TextSnapshot::from_tables(la, lb, &lt, &rt, out)
}

fn diff_text_default(lt: &LineTable, rt: &LineTable) -> Vec<ai_core::Section> {
    let out = ai_core::diff_text(lt, rt, &TextDiffOptions::default()).unwrap();
    out.sections
}

// ---------------------------------------------------------------------------
// 5) RPC 端到端：真实引擎二进制 report/generate
// ---------------------------------------------------------------------------

#[test]
fn report_generate_rpc_e2e() {
    let dir = tempfile::tempdir().unwrap();
    let l = dir.path().join("a.txt");
    let r = dir.path().join("b.txt");
    std::fs::write(&l, "one\ntwo\nthree\n").unwrap();
    std::fs::write(&r, "one\nTWO\nthree\nfour\n").unwrap();
    let lcsv = dir.path().join("l.csv");
    let rcsv = dir.path().join("r.csv");
    std::fs::write(&lcsv, "id,v\n1,a\n").unwrap();
    std::fs::write(&rcsv, "id,v\n1,b\n2,c\n").unwrap();
    let lb = dir.path().join("l.bin");
    let rb = dir.path().join("r.bin");
    std::fs::write(&lb, b"AAAABBBB").unwrap();
    std::fs::write(&rb, b"AAAAXXBB").unwrap();
    let ld = dir.path().join("L");
    let rd = dir.path().join("R");
    std::fs::create_dir_all(&ld).unwrap();
    std::fs::create_dir_all(&rd).unwrap();
    std::fs::write(ld.join("same.txt"), "1").unwrap();
    std::fs::write(rd.join("same.txt"), "1").unwrap();
    std::fs::write(ld.join("lonly.txt"), "L").unwrap();
    std::fs::write(rd.join("ronly.txt"), "R").unwrap();

    let inputs = vec![
        harness::req(1, "initialize", serde_json::json!({"clientInfo": {"name": "rpt", "version": "0"}, "protocolVersion": "1.0"})),
        harness::req(2, "text/open", serde_json::json!({"sessionId": "t", "left": l.to_string_lossy(), "right": r.to_string_lossy()})),
        harness::req(3, "report/generate", serde_json::json!({"sessionId": "t", "layout": "patch"})),
        harness::req(4, "report/generate", serde_json::json!({"sessionId": "t", "layout": "html", "title": "T", "lineNumbers": true})),
        harness::req(5, "report/generate", serde_json::json!({"sessionId": "t", "layout": "stats"})),
        // 分块流：offset/limit 切片 + hasMore
        harness::req(6, "report/generate", serde_json::json!({"sessionId": "t", "layout": "xml", "offset": 0, "limit": 16})),
        harness::req(7, "report/generate", serde_json::json!({"sessionId": "t", "layout": "xml", "offset": 16, "limit": 16})),
        // dir 会话（同步扫描路径；确定性）
        harness::req(8, "dir/open", serde_json::json!({
            "sessionId": "d", "left": ld.to_string_lossy(), "right": rd.to_string_lossy(),
            "depth": 1, "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"}
        })),
        harness::req(9, "report/generate", serde_json::json!({"sessionId": "d", "layout": "summary"})),
        harness::req(10, "report/generate", serde_json::json!({"sessionId": "d", "layout": "tree", "dirDisplay": "orphans"})),
        harness::req(11, "table/open", serde_json::json!({"sessionId": "tb", "left": lcsv.to_string_lossy(), "right": rcsv.to_string_lossy()})),
        harness::req(12, "report/generate", serde_json::json!({"sessionId": "tb", "layout": "csv", "delimiter": "tsv"})),
        harness::req(13, "hex/open", serde_json::json!({"sessionId": "hx", "left": lb.to_string_lossy(), "right": rb.to_string_lossy()})),
        harness::req(14, "report/generate", serde_json::json!({"sessionId": "hx", "layout": "stats"})),
        harness::req(15, "report/generate", serde_json::json!({"sessionId": "t", "layout": "xml"})),
        harness::req(16, "report/generate", serde_json::json!({"sessionId": "ghost", "layout": "summary"})),
        harness::req(17, "shutdown", serde_json::json!({})),
    ];
    let outputs = harness::run_engine(&inputs, None);
    let by_id = |id: i64| -> serde_json::Value {
        outputs
            .iter()
            .find(|m| m.get("id").and_then(|v| v.as_i64()) == Some(id))
            .unwrap_or_else(|| panic!("缺 id={id} 应答: {outputs:?}"))
            .clone()
    };

    // 能力位
    let init = by_id(1);
    assert_eq!(init["result"]["capabilities"]["reports"], true);

    // text patch
    let p = by_id(3);
    let content = p["result"]["content"].as_str().unwrap();
    assert!(content.starts_with("--- ") && content.contains("-two\n+TWO\n"), "{content}");
    assert_eq!(p["result"]["hasMore"], false);

    // html + 行号
    let h = by_id(4);
    let html = h["result"]["content"].as_str().unwrap();
    assert!(html.contains("<style>") && html.contains("one"), "{html}");

    // stats
    let resp_s = by_id(5);
    let s = resp_s["result"]["content"].as_str().unwrap();
    assert!(s.contains("insertedLines=2") && s.contains("deletedLines=1"), "{s}");

    // 分块流：两块拼接 == 全量
    let resp_full = by_id(3);
    let full = resp_full["result"]["content"].as_str().unwrap(); // patch 全量做拼接基准改用 xml
    let _ = full;
    let resp_c1 = by_id(6);
    let c1 = resp_c1["result"]["content"].as_str().unwrap();
    assert_eq!(c1.chars().count(), 16);
    assert_eq!(by_id(6)["result"]["hasMore"], true);
    let resp_c2 = by_id(7);
    let c2 = resp_c2["result"]["content"].as_str().unwrap();
    let xml_full = format!("{c1}{c2}");
    assert!(xml_full.starts_with("<?xml"), "首块从头开始: {xml_full}");
    assert!(!c2.is_empty(), "续块非空");

    // dir summary + tree orphans
    let resp_ds = by_id(9);
    let ds = resp_ds["result"]["content"].as_str().unwrap();
    assert!(ds.contains("差异清单") && ds.contains("lonly.txt"), "{ds}");
    let resp_dt = by_id(10);
    let dt = resp_dt["result"]["content"].as_str().unwrap();
    assert!(dt.contains("[<] lonly.txt") && dt.contains("[>] ronly.txt"), "{dt}");
    assert!(!dt.contains("same.txt"), "orphans 过滤: {dt}");

    // table tsv
    let resp_tc = by_id(12);
    let tc = resp_tc["result"]["content"].as_str().unwrap();
    assert!(tc.contains('\t') && tc.contains("added"), "{tc}");

    // hex stats
    let resp_hs = by_id(14);
    let hs = resp_hs["result"]["content"].as_str().unwrap();
    assert!(hs.contains("changedBytes=2"), "{hs}");

    // 分块拼接 == 全量前缀
    let resp_full = by_id(15);
    let full_xml = resp_full["result"]["content"].as_str().unwrap();
    assert_eq!(by_id(15)["result"]["hasMore"], false);
    assert!(full_xml.starts_with(&xml_full), "拼接 == 全量前缀");

    // 未知会话 -> JSON-RPC 错误
    let err = by_id(16);
    assert!(err.get("error").is_some(), "{err}");
}
