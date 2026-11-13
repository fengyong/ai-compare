//! `aic` 的纯逻辑层：unified diff 渲染、目录递归 diff 渲染与 quick 判定（可单测）。
//!
//! 命令实现（diff/quick 参数面 + 入口）见 [`app`] 模块：aic 主二进制与
//! aicc 等待型包装（进程内回退路径）共用同一命令面。

pub mod app;

use ai_core::sections::SectionKind;
use ai_core::{DiffOutput, LineTable};
use ai_dir::DirNode;
use ai_protocol::{DirEntryStatus, EntryKind};

/// 退出码（docs/04 P0：0 相同 / 1 不同 / 2 错误；P3 细分 11/12/13/14）
///
/// 与 docs/01 §4.8 FR-CLI-004 的分层映射（P0 兼容优先，"相同"恒为 0）：
/// - `0` 成功/相同：P0 契约；FR-CLI-004 的"1 二进制相同 / 2 基于规则相同"
///   两个同侧细分并入 0（1/2 在 P0 已占用为 不同/错误，改义将破坏既有自动化）；
/// - `1` 不同：`aic diff` 与默认差异层（P0 契约不变）；
/// - `2` 错误：加载失败/参数非法等（P0 契约不变）；
/// - `11` 二进制差异：quick size/crc/binary 模式的差异细分；
/// - `12` 相似：rules 模式下仅有不重要差异（FR-TXT-010 重要性判等）；
/// - `13` 基于规则的差异：rules 模式下存在重要差异；
/// - `14` 检测到冲突：三向合并专用（P7 /automerge 接入，本阶段仅定义）。
///
/// 兼容性约定：0/非 0 的粗判定不变（11~14 均为"非相同"），细分码只增不改。
pub const EXIT_SAME: u8 = 0;
pub const EXIT_DIFF: u8 = 1;
pub const EXIT_ERROR: u8 = 2;
pub const EXIT_BINARY_DIFF: u8 = 11;
pub const EXIT_SIMILAR: u8 = 12;
pub const EXIT_RULES_DIFF: u8 = 13;
pub const EXIT_CONFLICT: u8 = 14;

/// ANSI 颜色
pub struct Palette {
    pub delete: &'static str,
    pub insert: &'static str,
    pub hunk: &'static str,
    pub header: &'static str,
    pub reset: &'static str,
}

pub const PLAIN: Palette = Palette { delete: "", insert: "", hunk: "", header: "", reset: "" };

pub const ANSI: Palette = Palette {
    delete: "\x1b[31m",
    insert: "\x1b[32m",
    hunk: "\x1b[36m",
    header: "\x1b[1m",
    reset: "\x1b[0m",
};

/// 脚本条目：Same(i) 引用左侧行 i（与右侧行 i 相同）；Left/Right 为删/增行
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Same(u32),
    Left(u32),
    Right(u32),
}

impl Entry {
    fn is_change(&self) -> bool {
        !matches!(self, Entry::Same(_))
    }
}

/// 渲染 unified diff（`diff -u` 风格，hunk 头计数采用 git 约定：
/// count==0 时行号为前导行数）。无差异时返回空串。
pub fn render_unified(
    left_label: &str,
    right_label: &str,
    out: &DiffOutput,
    left: &LineTable,
    right: &LineTable,
    context: u32,
    palette: &Palette,
) -> String {
    let has_diff = out.sections.iter().any(|s| s.kind != SectionKind::Equal);
    if !has_diff {
        return String::new();
    }

    // 1. 展开为脚本条目序列（Change = 先删后增，git/unified 惯例）
    let mut script: Vec<Entry> = Vec::new();
    for s in &out.sections {
        match s.kind {
            SectionKind::Equal => {
                let l = s.left.expect("Equal 必有左区间");
                for i in l.start..l.end {
                    script.push(Entry::Same(i));
                }
            }
            SectionKind::Change | SectionKind::Delete => {
                let l = s.left.expect("Change/Delete 必有左区间");
                for i in l.start..l.end {
                    script.push(Entry::Left(i));
                }
                if s.kind == SectionKind::Change {
                    let r = s.right.expect("Change 必有右区间");
                    for i in r.start..r.end {
                        script.push(Entry::Right(i));
                    }
                }
            }
            SectionKind::Insert => {
                let r = s.right.expect("Insert 必有右区间");
                for i in r.start..r.end {
                    script.push(Entry::Right(i));
                }
            }
        }
    }

    // 2. 以非等条目为锚，按 context 扩展并合并相邻窗口
    let ctx = context as usize;
    let mut hunks: Vec<(usize, usize)> = Vec::new(); // [start, end) in script
    for (idx, e) in script.iter().enumerate() {
        if e.is_change() {
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

    // 3. 渲染
    let mut text = String::new();
    let paint = |color: &'static str, body: String| -> String {
        if color.is_empty() {
            body
        } else {
            format!("{color}{body}{}", palette.reset)
        }
    };

    text.push_str(&paint(palette.header, format!("--- {left_label}")));
    text.push('\n');
    text.push_str(&paint(palette.header, format!("+++ {right_label}")));
    text.push('\n');

    for (hs, he) in &hunks {
        // 窗口前左/右已消耗行数（= 下一行 0 基下标）
        let (mut left_before, mut right_before) = (0u32, 0u32);
        for e in &script[..*hs] {
            match e {
                Entry::Same(_) => {
                    left_before += 1;
                    right_before += 1;
                }
                Entry::Left(_) => left_before += 1,
                Entry::Right(_) => right_before += 1,
            }
        }
        let (mut left_count, mut right_count) = (0u32, 0u32);
        for e in &script[*hs..*he] {
            match e {
                Entry::Same(_) => {
                    left_count += 1;
                    right_count += 1;
                }
                Entry::Left(_) => left_count += 1,
                Entry::Right(_) => right_count += 1,
            }
        }
        // git 约定：count == 0 时显示前导行数（顶部插入显示 -0,0）；count == 1 时省略 `,1`
        let ls = if left_count > 0 { left_before + 1 } else { left_before };
        let rs = if right_count > 0 { right_before + 1 } else { right_before };
        let lpart = if left_count == 1 { format!("-{ls}") } else { format!("-{ls},{left_count}") };
        let rpart = if right_count == 1 { format!("+{rs}") } else { format!("+{rs},{right_count}") };
        text.push_str(&paint(palette.hunk, format!("@@ {lpart} {rpart} @@")));
        text.push('\n');
        for e in &script[*hs..*he] {
            match e {
                Entry::Same(i) => text.push_str(&format!(" {}\n", left.line(*i as usize).text)),
                Entry::Left(i) => {
                    text.push_str(&paint(palette.delete, format!("-{}", left.line(*i as usize).text)));
                    text.push('\n');
                }
                Entry::Right(i) => {
                    text.push_str(&paint(palette.insert, format!("+{}", right.line(*i as usize).text)));
                    text.push('\n');
                }
            }
        }
    }
    text
}

/// 目录递归 diff 渲染（`diff -rq` 风格；无差异返回空串）：
/// - `Files <l>/<x> and <r>/<x> differ`
/// - `Only in <side>: <path>`（目录差异子树逐行；目录聚合状态驱动）
pub fn render_dir_diff(left_label: &str, right_label: &str, arena: &[DirNode]) -> String {
    let mut out = String::new();
    for n in arena.iter().skip(1) {
        match n.status {
            DirEntryStatus::ContentDiff | DirEntryStatus::QuickDiff => {
                out.push_str(&format!(
                    "Files {left_label}/{} and {right_label}/{} differ\n",
                    n.rel_path, n.rel_path
                ));
            }
            DirEntryStatus::DirDiff => {
                // 目录聚合差异：内容差异在子行逐条输出（此处不打印目录行本身，
                // 与 diff -r 的"进入子目录"语义一致）
            }
            DirEntryStatus::LeftOnly => {
                out.push_str(&format!("Only in {left_label}: {}\n", display_rel(n)));
            }
            DirEntryStatus::RightOnly => {
                out.push_str(&format!("Only in {right_label}: {}\n", display_rel(n)));
            }
            _ => {}
        }
    }
    out
}

/// 孤立项显示路径（孤儿子树整体归到该侧）
fn display_rel(n: &DirNode) -> String {
    let slash = if n.kind == EntryKind::Dir { "/" } else { "" };
    format!("{}{}", n.rel_path, slash)
}

// ---------------------------------------------------------------------------
// quick 判定（P3：rules 模式接入 ai-core 重要性判等，替换 P2 二进制占位）
// ---------------------------------------------------------------------------

/// quick 比较判定（P3 细分退出码的数据源）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickVerdict {
    /// 相同（退出码 0）
    Same,
    /// 二进制差异（size/crc/binary 模式差异，退出码 11）
    BinaryDiff,
    /// 相似：仅有不重要差异（rules 模式，退出码 12）
    Similar,
    /// 基于规则的差异：存在重要差异（rules 模式，退出码 13）
    RulesDiff,
}

impl QuickVerdict {
    /// 映射为退出码（0 / 11 / 12 / 13）
    pub fn exit_code(self) -> u8 {
        match self {
            QuickVerdict::Same => EXIT_SAME,
            QuickVerdict::BinaryDiff => EXIT_BINARY_DIFF,
            QuickVerdict::Similar => EXIT_SIMILAR,
            QuickVerdict::RulesDiff => EXIT_RULES_DIFF,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            QuickVerdict::Same => "same",
            QuickVerdict::BinaryDiff => "differ",
            QuickVerdict::Similar => "similar",
            QuickVerdict::RulesDiff => "differ(rules)",
        }
    }
}

/// 多项判定的聚合（目录 rules 模式）：重要差异 > 二进制差异 > 相似 > 相同
pub fn aggregate_verdicts(vs: impl IntoIterator<Item = QuickVerdict>) -> QuickVerdict {
    let mut agg = QuickVerdict::Same;
    for v in vs {
        agg = match (agg, v) {
            (_, QuickVerdict::RulesDiff) => QuickVerdict::RulesDiff,
            (QuickVerdict::RulesDiff, _) => QuickVerdict::RulesDiff,
            (_, QuickVerdict::BinaryDiff) => QuickVerdict::BinaryDiff,
            (QuickVerdict::BinaryDiff, _) => QuickVerdict::BinaryDiff,
            (_, QuickVerdict::Similar) => QuickVerdict::Similar,
            (QuickVerdict::Similar, _) => QuickVerdict::Similar,
            _ => QuickVerdict::Same,
        };
    }
    agg
}

/// 基于规则的两文件判定（FR-CLI /qc 缺省模式；FR-TXT-010 重要性判等）：
/// 1. 任一侧不是合法 UTF-8 文本 -> 二进制比较（Same / BinaryDiff）；
/// 2. 全部 section 相等 -> Same；
/// 3. 任一差异 section 重要性为 Important -> RulesDiff；
/// 4. 其余（差异全部 Unimportant/Ignored）-> Similar。
///
/// `imp` 为重要性模型（FR-CLI-003 /iu 语义的开关面；ai-core 默认模型下
/// 差异恒重要，Similar 仅在空白/大小写不重要开关开启时可达）；
/// 规则库自定义（FR-SEL-001）属 P10 接入面。
pub async fn rules_classify_file(
    vfs: &dyn ai_vfs::VfsProvider,
    left: &ai_vfs::VfsPath,
    right: &ai_vfs::VfsPath,
    imp: &ai_core::ImportanceOptions,
) -> Result<QuickVerdict, String> {
    use ai_core::sections::{Importance, SectionKind};

    let l_text = vfs.read_file_text(left).await;
    let r_text = vfs.read_file_text(right).await;
    let (lt, rt) = match (l_text, r_text) {
        (Ok(l), Ok(r)) => (l, r),
        // 非文本（或读取失败）：回退二进制判等（BC 语义：规则比较不适用于二进制）
        _ => {
            let same = ai_dir::content_same(vfs, left, right, ai_dir::ContentHashMode::Binary)
                .await
                .map_err(|e| e.to_string())?;
            return Ok(if same { QuickVerdict::Same } else { QuickVerdict::BinaryDiff });
        }
    };

    let ltab = ai_core::LineTable::from_text(&lt);
    let rtab = ai_core::LineTable::from_text(&rt);
    let cfg = ai_core::DiffConfig { importance: imp.clone(), ..ai_core::DiffConfig::default() };
    let keyed = ai_core::diff_config(&ltab, &rtab, &cfg).map_err(|e| e.to_string())?;
    let has_diff = keyed.output.sections.iter().any(|s| s.kind != SectionKind::Equal);
    if !has_diff {
        return Ok(QuickVerdict::Same);
    }
    let any_important = keyed
        .output
        .sections
        .iter()
        // Equal section 恒 Important（不是差异），不参与判定
        .any(|s| s.kind != SectionKind::Equal && s.important == Importance::Important);
    if any_important {
        Ok(QuickVerdict::RulesDiff)
    } else {
        Ok(QuickVerdict::Similar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_core::TextDiffOptions;

    fn diff(a: &str, b: &str) -> (DiffOutput, LineTable, LineTable) {
        let lt = LineTable::from_text(a);
        let rt = LineTable::from_text(b);
        let out = ai_core::diff_text(&lt, &rt, &TextDiffOptions::default()).unwrap();
        (out, lt, rt)
    }

    #[test]
    fn no_diff_renders_empty() {
        let (out, lt, rt) = diff("a\nb\n", "a\nb\n");
        let text = render_unified("a", "b", &out, &lt, &rt, 3, &PLAIN);
        assert_eq!(text, "");
    }

    #[test]
    fn simple_replacement_hunk() {
        let (out, lt, rt) = diff("one\ntwo\nthree\n", "one\nTWO\nthree\n");
        let text = render_unified("left.c", "right.c", &out, &lt, &rt, 3, &PLAIN);
        let expect = "\
--- left.c
+++ right.c
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
";
        assert_eq!(text, expect);
    }

    #[test]
    fn insertion_at_top_uses_zero_start() {
        let (out, lt, rt) = diff("b\n", "a\nb\n");
        let text = render_unified("l", "r", &out, &lt, &rt, 0, &PLAIN);
        let expect = "\
--- l
+++ r
@@ -0,0 +1 @@
+a
";
        assert_eq!(text, expect);
    }

    #[test]
    fn context_limits_and_merges_adjacent_hunks() {
        let a = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n";
        let b = "1\n2\n3\nX\n5\n6\n7\nY\n9\n10\n11\n12\n";
        let (out, lt, rt) = diff(a, b);
        let text = render_unified("l", "r", &out, &lt, &rt, 1, &PLAIN);
        // 两处修改相距 3 行，context=1 不合并 -> 两个 hunk（每 hunk 头一个 "@@ -"）
        assert_eq!(text.matches("@@ -").count(), 2);
        let text2 = render_unified("l", "r", &out, &lt, &rt, 2, &PLAIN);
        // context=2 时窗口相接 -> 合并为一个 hunk
        assert_eq!(text2.matches("@@ -").count(), 1);
        assert!(text2.contains("-4\n+X\n 5"));
        assert!(text2.contains("-8\n+Y"));
    }

    #[test]
    fn hunk_position_numbers_are_consistent() {
        let a = "1\n2\n3\n4\n5\n";
        let b = "1\n2\n3\n4\n5\n6\n";
        let (out, lt, rt) = diff(a, b);
        let text = render_unified("l", "r", &out, &lt, &rt, 0, &PLAIN);
        // 尾部追加：git 约定 count==0 -> 前导行数 5
        assert!(text.contains("@@ -5,0 +6 @@\n+6\n"), "实际输出:\n{text}");
    }

    #[test]
    fn ansi_coloring_wraps_lines() {
        let (out, lt, rt) = diff("a\n", "b\n");
        let text = render_unified("l", "r", &out, &lt, &rt, 0, &ANSI);
        assert!(text.contains("\x1b[31m-a\x1b[0m"));
        assert!(text.contains("\x1b[32m+b\x1b[0m"));
        assert!(text.contains("\x1b[36m@@ -1 +1 @@\x1b[0m"));
        assert!(text.contains("\x1b[1m--- l\x1b[0m"));
    }

    #[test]
    fn exit_code_consts_match_docs() {
        assert_eq!(EXIT_SAME, 0);
        assert_eq!(EXIT_DIFF, 1);
        assert_eq!(EXIT_ERROR, 2);
        // P3 细分层（FR-CLI-004 差异侧）
        assert_eq!(EXIT_BINARY_DIFF, 11);
        assert_eq!(EXIT_SIMILAR, 12);
        assert_eq!(EXIT_RULES_DIFF, 13);
        assert_eq!(EXIT_CONFLICT, 14);
    }

    #[test]
    fn verdict_exit_codes() {
        assert_eq!(QuickVerdict::Same.exit_code(), 0);
        assert_eq!(QuickVerdict::BinaryDiff.exit_code(), 11);
        assert_eq!(QuickVerdict::Similar.exit_code(), 12);
        assert_eq!(QuickVerdict::RulesDiff.exit_code(), 13);
    }

    #[test]
    fn aggregate_prioritizes_important_diff() {
        use QuickVerdict::*;
        assert_eq!(aggregate_verdicts([Same, Same]), Same);
        assert_eq!(aggregate_verdicts([Same, Similar]), Similar);
        assert_eq!(aggregate_verdicts([Similar, BinaryDiff]), BinaryDiff);
        assert_eq!(aggregate_verdicts([BinaryDiff, RulesDiff]), RulesDiff);
        assert_eq!(aggregate_verdicts([RulesDiff, Same]), RulesDiff);
    }
}

#[cfg(test)]
mod dir_diff_tests {
    use super::*;
    use ai_protocol::SideInfo;

    fn node(rel: &str, name: &str, kind: EntryKind, status: DirEntryStatus) -> DirNode {
        DirNode {
            rel_path: rel.into(),
            name: name.into(),
            kind,
            left: Some(SideInfo::base(1, 1)),
            right: Some(SideInfo::base(1, 1)),
            status,
            parent: None,
            children: Vec::new(),
            expanded: false,
            aligned_with: None,
        }
    }

    #[test]
    fn dir_diff_rendering() {
        let arena = vec![
            DirNode {
                rel_path: String::new(),
                name: String::new(),
                kind: EntryKind::Dir,
                left: None,
                right: None,
                status: DirEntryStatus::DirSame,
                parent: None,
                children: Vec::new(),
                expanded: true,
                aligned_with: None,
            },
            node("a.txt", "a.txt", EntryKind::File, DirEntryStatus::ContentDiff),
            node("same.txt", "same.txt", EntryKind::File, DirEntryStatus::Same),
            node("sub", "sub", EntryKind::Dir, DirEntryStatus::DirDiff),
            node("only_l.txt", "only_l.txt", EntryKind::File, DirEntryStatus::LeftOnly),
            node("only_r", "only_r", EntryKind::Dir, DirEntryStatus::RightOnly),
        ];
        let text = render_dir_diff("L", "R", &arena);
        assert_eq!(
            text,
            "Files L/a.txt and R/a.txt differ\n\
             Only in L: only_l.txt\n\
             Only in R: only_r/\n"
        );
        // 无差异 -> 空串
        let clean = [node("x", "x", EntryKind::File, DirEntryStatus::Same)];
        assert_eq!(render_dir_diff("L", "R", &[node("", "", EntryKind::Dir, DirEntryStatus::DirSame), clean[0].clone()]), "");
    }
}

#[cfg(test)]
mod rules_tests {
    use super::*;

    async fn write_pair(
        dir: &std::path::Path,
        l: &str,
        r: &str,
    ) -> (ai_vfs::VfsPath, ai_vfs::VfsPath) {
        let lp = dir.join("L.txt");
        let rp = dir.join("R.txt");
        std::fs::write(&lp, l).unwrap();
        std::fs::write(&rp, r).unwrap();
        (
            ai_vfs::VfsPath::parse(lp.to_str().unwrap()).unwrap(),
            ai_vfs::VfsPath::parse(rp.to_str().unwrap()).unwrap(),
        )
    }

    #[tokio::test]
    async fn rules_same_text() {
        let dir = tempfile::tempdir().unwrap();
        let (l, r) = write_pair(dir.path(), "a\nb\n", "a\nb\n").await;
        let vfs = ai_vfs::LocalFs::new();
        let v = rules_classify_file(&vfs, &l, &r, &ai_core::ImportanceOptions::default()).await.unwrap();
        assert_eq!(v, QuickVerdict::Same);
    }

    #[tokio::test]
    async fn rules_important_diff() {
        let dir = tempfile::tempdir().unwrap();
        let (l, r) = write_pair(dir.path(), "alpha\n", "beta\n").await;
        let vfs = ai_vfs::LocalFs::new();
        let v = rules_classify_file(&vfs, &l, &r, &ai_core::ImportanceOptions::default()).await.unwrap();
        assert_eq!(v, QuickVerdict::RulesDiff);
    }

    #[tokio::test]
    async fn rules_whitespace_importance_switches() {
        let dir = tempfile::tempdir().unwrap();
        let vfs = ai_vfs::LocalFs::new();
        // 默认模型：尾随空白重要 -> RulesDiff（FR-TXT-010 默认全重要）
        let (l, r) = write_pair(dir.path(), "alpha   \nbeta\n", "alpha\nbeta\n").await;
        let v = rules_classify_file(&vfs, &l, &r, &ai_core::ImportanceOptions::default()).await.unwrap();
        assert_eq!(v, QuickVerdict::RulesDiff);
        // 尾随空白不重要（/iu 类开关）-> 仅不重要差异 = Similar（退出码 12 语义）
        let imp = ai_core::ImportanceOptions { trailing_ws_unimportant: true, ..Default::default() };
        let v = rules_classify_file(&vfs, &l, &r, &imp).await.unwrap();
        assert_eq!(v, QuickVerdict::Similar);
        // 大小写不重要 -> "Hello" vs "hello" 判相似
        let (l, r) = write_pair(dir.path(), "Hello\n", "hello\n").await;
        let imp = ai_core::ImportanceOptions { case_unimportant: true, ..Default::default() };
        let v = rules_classify_file(&vfs, &l, &r, &imp).await.unwrap();
        assert_eq!(v, QuickVerdict::Similar);
    }

    #[tokio::test]
    async fn rules_eol_style_only_is_same() {
        let dir = tempfile::tempdir().unwrap();
        // 行尾风格差异（CRLF vs LF）：默认模型下行尾不重要 -> Same
        let (l, r) = write_pair(dir.path(), "alpha\r\nbeta\r\n", "alpha\nbeta\n").await;
        let vfs = ai_vfs::LocalFs::new();
        let v = rules_classify_file(&vfs, &l, &r, &ai_core::ImportanceOptions::default()).await.unwrap();
        assert_eq!(v, QuickVerdict::Same);
    }

    #[tokio::test]
    async fn rules_binary_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..64u8).collect();
        let lp = dir.path().join("L.bin");
        let rp = dir.path().join("R.bin");
        std::fs::write(&lp, &bytes).unwrap();
        let mut rb = bytes.clone();
        rb[0] ^= 0xff;
        std::fs::write(&rp, &rb).unwrap();
        let vfs = ai_vfs::LocalFs::new();
        let l = ai_vfs::VfsPath::parse(lp.to_str().unwrap()).unwrap();
        let r = ai_vfs::VfsPath::parse(rp.to_str().unwrap()).unwrap();
        let v = rules_classify_file(&vfs, &l, &r, &ai_core::ImportanceOptions::default()).await.unwrap();
        assert_eq!(v, QuickVerdict::BinaryDiff, "非文本回退二进制判等");
        // 二进制相同
        let lp = dir.path().join("S1.bin");
        let rp = dir.path().join("S2.bin");
        std::fs::write(&lp, &bytes).unwrap();
        std::fs::write(&rp, &bytes).unwrap();
        let l = ai_vfs::VfsPath::parse(lp.to_str().unwrap()).unwrap();
        let r = ai_vfs::VfsPath::parse(rp.to_str().unwrap()).unwrap();
        let v = rules_classify_file(&vfs, &l, &r, &ai_core::ImportanceOptions::default()).await.unwrap();
        assert_eq!(v, QuickVerdict::Same);
    }
}
