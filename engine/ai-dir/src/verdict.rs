//! 内容比较判定（FR-DIR-011/012）：CRC / 二进制 / 基于规则三方式，
//! 以及"二进制相同 / 基于规则相同 / 不重要差异 / 重要差异"四态结果。
//!
//! 判定权威在本 crate：CLI（`aic quick`、脚本 COMPARE）与引擎目录会话
//! 共用同一套语义，避免各写一份导致口径漂移（R3-16）。

use ai_core::{DiffConfig, ImportanceOptions, LineTable};
use ai_protocol::{ContentCompareMethod, ContentCompareResult};
use ai_vfs::{VfsPath, VfsProvider};

use crate::cache::HashCache;
use crate::compare::{content_same_stats, ContentHashMode, FileStatSummary};

/// 两文件内容判定（CLI 退出码 0/11/12/13 的数据源）
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
/// 差异恒重要，Similar 仅在空白/大小写不重要开关开启时可达）。
///
/// 编码口径：此处按严格 UTF-8 判"是否文本"，非 UTF-8 一律回退二进制比较
/// （BC 语义：规则比较不适用于二进制）。文本会话的编码检测/选择见
/// ai-vfs::encoding（R3-15），目录规则比较暂不跨编码判等。
pub async fn rules_classify_file(
    vfs: &dyn VfsProvider,
    left: &VfsPath,
    right: &VfsPath,
    imp: &ImportanceOptions,
) -> Result<QuickVerdict, String> {
    use ai_core::sections::{Importance, SectionKind};

    let l_text = vfs.read_file_text(left).await;
    let r_text = vfs.read_file_text(right).await;
    let (lt, rt) = match (l_text, r_text) {
        (Ok(l), Ok(r)) => (l, r),
        // 非文本（或读取失败）：回退二进制判等
        _ => {
            let same = crate::compare::content_same(vfs, left, right, ContentHashMode::Binary)
                .await
                .map_err(|e| e.to_string())?;
            return Ok(if same {
                QuickVerdict::Same
            } else {
                QuickVerdict::BinaryDiff
            });
        }
    };

    let ltab = LineTable::from_text(&lt);
    let rtab = LineTable::from_text(&rt);
    let cfg = DiffConfig {
        importance: imp.clone(),
        ..DiffConfig::default()
    };
    let keyed = ai_core::diff_config(&ltab, &rtab, &cfg).map_err(|e| e.to_string())?;
    let has_diff = keyed
        .output
        .sections
        .iter()
        .any(|s| s.kind != SectionKind::Equal);
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

/// 按方法判等两文件内容（FR-DIR-011 三方式；stat 由调用方提供，CRC 走缓存）。
///
/// - `Crc`/`Binary`：size 不同即判异；相同则分块哈希/逐字节比较。
/// - `RulesBased`：先做二进制比较（BC"基于规则比较内含二进制比较"），
///   字节全等直接 Same；否则按重要性模型分类为 Same/Similar/RulesDiff。
// 入参沿用 compare::content_same_stats 的 7 参形状 + 重要性模型（拆结构体反而割裂两处口径）
#[allow(clippy::too_many_arguments)]
pub async fn content_verdict_stats(
    vfs: &dyn VfsProvider,
    a: &VfsPath,
    b: &VfsPath,
    sa: FileStatSummary,
    sb: FileStatSummary,
    method: ContentCompareMethod,
    imp: &ImportanceOptions,
    cache: Option<&HashCache>,
) -> Result<QuickVerdict, String> {
    if method != ContentCompareMethod::RulesBased {
        let mode = match method {
            ContentCompareMethod::Binary => ContentHashMode::Binary,
            _ => ContentHashMode::Crc,
        };
        let same = content_same_stats(vfs, a, b, sa, sb, mode, cache)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(if same {
            QuickVerdict::Same
        } else {
            QuickVerdict::BinaryDiff
        });
    }
    // 规则比较不能按 size 短路：行尾风格差异（CRLF vs LF）必然 size 不同，
    // 但属不重要差异，BC 规则比较判同。字节全等才是确定性判同的快路径。
    let binary_same = content_same_stats(vfs, a, b, sa, sb, ContentHashMode::Binary, None)
        .await
        .map_err(|e| e.to_string())?;
    if binary_same {
        return Ok(QuickVerdict::Same);
    }
    rules_classify_file(vfs, a, b, imp).await
}

/// 内容判定 → 目录行状态（FR-DIR-012 四态；BC 行色：相同黑 / 差异红，
/// "仅不重要差异"仍属差异，靠中间列 contentsimilar 图标区分）
pub fn verdict_to_status(v: QuickVerdict) -> ai_protocol::DirEntryStatus {
    use ai_protocol::DirEntryStatus;
    match v {
        QuickVerdict::Same => DirEntryStatus::Same,
        QuickVerdict::Similar => DirEntryStatus::Similar,
        QuickVerdict::BinaryDiff | QuickVerdict::RulesDiff => DirEntryStatus::ContentDiff,
    }
}

/// 手动内容比较（FR-DIR-012）：比 [`content_verdict_stats`] 多区分
/// "二进制相同"与"基于规则相同"——前者字节全等，后者字节不同但差异全可忽略。
#[allow(clippy::too_many_arguments)] // 与 content_verdict_stats 同形，便于两入口共用口径
pub async fn content_compare_result(
    vfs: &dyn VfsProvider,
    a: &VfsPath,
    b: &VfsPath,
    sa: FileStatSummary,
    sb: FileStatSummary,
    method: ContentCompareMethod,
    imp: &ImportanceOptions,
    cache: Option<&HashCache>,
) -> Result<ContentCompareResult, String> {
    use ContentCompareResult as R;
    if method != ContentCompareMethod::RulesBased {
        return Ok(
            match content_verdict_stats(vfs, a, b, sa, sb, method, imp, cache).await? {
                QuickVerdict::Same => R::BinarySame,
                _ => R::BinaryDiff,
            },
        );
    }
    let binary_same = content_same_stats(vfs, a, b, sa, sb, ContentHashMode::Binary, None)
        .await
        .map_err(|e| e.to_string())?;
    if binary_same {
        return Ok(R::BinarySame);
    }
    Ok(match rules_classify_file(vfs, a, b, imp).await? {
        QuickVerdict::Same => R::RulesSame,
        QuickVerdict::Similar => R::UnimportantDiff,
        QuickVerdict::RulesDiff => R::ImportantDiff,
        QuickVerdict::BinaryDiff => R::BinaryDiff,
    })
}

/// 手动比较结果 → 目录行状态（与自动扫描同一口径）
pub fn result_to_status(r: ContentCompareResult) -> ai_protocol::DirEntryStatus {
    use ai_protocol::DirEntryStatus;
    match r {
        ContentCompareResult::BinarySame | ContentCompareResult::RulesSame => DirEntryStatus::Same,
        ContentCompareResult::UnimportantDiff => DirEntryStatus::Similar,
        ContentCompareResult::BinaryDiff | ContentCompareResult::ImportantDiff => {
            DirEntryStatus::ContentDiff
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_vfs::LocalFs;

    async fn write_pair(dir: &std::path::Path, l: &str, r: &str) -> (VfsPath, VfsPath) {
        let lp = dir.join("L.txt");
        let rp = dir.join("R.txt");
        std::fs::write(&lp, l).unwrap();
        std::fs::write(&rp, r).unwrap();
        (
            VfsPath::parse(lp.to_str().unwrap()).unwrap(),
            VfsPath::parse(rp.to_str().unwrap()).unwrap(),
        )
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

    #[tokio::test]
    async fn rules_same_text() {
        let dir = tempfile::tempdir().unwrap();
        let (l, r) = write_pair(dir.path(), "a\nb\n", "a\nb\n").await;
        let vfs = LocalFs::new();
        let v = rules_classify_file(&vfs, &l, &r, &ImportanceOptions::default())
            .await
            .unwrap();
        assert_eq!(v, QuickVerdict::Same);
    }

    #[tokio::test]
    async fn rules_important_diff() {
        let dir = tempfile::tempdir().unwrap();
        let (l, r) = write_pair(dir.path(), "alpha\n", "beta\n").await;
        let vfs = LocalFs::new();
        let v = rules_classify_file(&vfs, &l, &r, &ImportanceOptions::default())
            .await
            .unwrap();
        assert_eq!(v, QuickVerdict::RulesDiff);
    }

    #[tokio::test]
    async fn rules_whitespace_importance_switches() {
        let dir = tempfile::tempdir().unwrap();
        let vfs = LocalFs::new();
        // 默认模型：尾随空白重要 -> RulesDiff（FR-TXT-010 默认全重要）
        let (l, r) = write_pair(dir.path(), "alpha   \nbeta\n", "alpha\nbeta\n").await;
        let v = rules_classify_file(&vfs, &l, &r, &ImportanceOptions::default())
            .await
            .unwrap();
        assert_eq!(v, QuickVerdict::RulesDiff);
        // 尾随空白不重要（/iu 类开关）-> 仅不重要差异 = Similar（退出码 12 语义）
        let imp = ImportanceOptions {
            trailing_ws_unimportant: true,
            ..Default::default()
        };
        let v = rules_classify_file(&vfs, &l, &r, &imp).await.unwrap();
        assert_eq!(v, QuickVerdict::Similar);
        // 大小写不重要 -> "Hello" vs "hello" 判相似
        let (l, r) = write_pair(dir.path(), "Hello\n", "hello\n").await;
        let imp = ImportanceOptions {
            case_unimportant: true,
            ..Default::default()
        };
        let v = rules_classify_file(&vfs, &l, &r, &imp).await.unwrap();
        assert_eq!(v, QuickVerdict::Similar);
    }

    #[tokio::test]
    async fn rules_eol_style_only_is_same() {
        let dir = tempfile::tempdir().unwrap();
        // 行尾风格差异（CRLF vs LF）：默认模型下行尾不重要 -> Same
        let (l, r) = write_pair(dir.path(), "alpha\r\nbeta\r\n", "alpha\nbeta\n").await;
        let vfs = LocalFs::new();
        let v = rules_classify_file(&vfs, &l, &r, &ImportanceOptions::default())
            .await
            .unwrap();
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
        let vfs = LocalFs::new();
        let l = VfsPath::parse(lp.to_str().unwrap()).unwrap();
        let r = VfsPath::parse(rp.to_str().unwrap()).unwrap();
        let v = rules_classify_file(&vfs, &l, &r, &ImportanceOptions::default())
            .await
            .unwrap();
        assert_eq!(v, QuickVerdict::BinaryDiff, "非文本回退二进制判等");
        // 二进制相同
        let lp = dir.path().join("S1.bin");
        let rp = dir.path().join("S2.bin");
        std::fs::write(&lp, &bytes).unwrap();
        std::fs::write(&rp, &bytes).unwrap();
        let l = VfsPath::parse(lp.to_str().unwrap()).unwrap();
        let r = VfsPath::parse(rp.to_str().unwrap()).unwrap();
        let v = rules_classify_file(&vfs, &l, &r, &ImportanceOptions::default())
            .await
            .unwrap();
        assert_eq!(v, QuickVerdict::Same);
    }

    /// R3-16：三方式必须真的分派到不同实现，而不是都退化成 CRC。
    /// 用"字节不同而规则相同"（CRLF vs LF）区分 binary 与 rules。
    #[tokio::test]
    async fn three_methods_are_not_interchangeable() {
        let dir = tempfile::tempdir().unwrap();
        let vfs = LocalFs::new();
        let (l, r) = write_pair(dir.path(), "a\r\nb\r\n", "a\nb\n").await;
        let sl = vfs.stat(&l).await.unwrap();
        let sr = vfs.stat(&r).await.unwrap();
        let sa = FileStatSummary::new(sl.size, sl.mtime_ms);
        let sb = FileStatSummary::new(sr.size, sr.mtime_ms);
        let imp = ImportanceOptions::default();

        let crc =
            content_verdict_stats(&vfs, &l, &r, sa, sb, ContentCompareMethod::Crc, &imp, None)
                .await
                .unwrap();
        let bin = content_verdict_stats(
            &vfs,
            &l,
            &r,
            sa,
            sb,
            ContentCompareMethod::Binary,
            &imp,
            None,
        )
        .await
        .unwrap();
        let rules = content_verdict_stats(
            &vfs,
            &l,
            &r,
            sa,
            sb,
            ContentCompareMethod::RulesBased,
            &imp,
            None,
        )
        .await
        .unwrap();
        assert_eq!(crc, QuickVerdict::BinaryDiff, "CRC 看字节，行尾差异判异");
        assert_eq!(bin, QuickVerdict::BinaryDiff);
        assert_eq!(rules, QuickVerdict::Same, "规则比较忽略行尾差异");

        // 重要差异：规则模式给出 RulesDiff 而非 BinaryDiff
        let (l, r) = write_pair(dir.path(), "alpha\n", "beta\n").await;
        let sl = vfs.stat(&l).await.unwrap();
        let sr = vfs.stat(&r).await.unwrap();
        let sa = FileStatSummary::new(sl.size, sl.mtime_ms);
        let sb = FileStatSummary::new(sr.size, sr.mtime_ms);
        let rules = content_verdict_stats(
            &vfs,
            &l,
            &r,
            sa,
            sb,
            ContentCompareMethod::RulesBased,
            &imp,
            None,
        )
        .await
        .unwrap();
        assert_eq!(rules, QuickVerdict::RulesDiff);
    }

    /// size 不同不得短路规则比较：行尾风格差异必然 size 不同，但规则判同；
    /// 真实内容差异仍按重要性分类（不是二进制判异）。
    #[tokio::test]
    async fn rules_size_mismatch_is_not_short_circuited() {
        let dir = tempfile::tempdir().unwrap();
        let vfs = LocalFs::new();
        let imp = ImportanceOptions::default();
        // 仅行尾风格不同：6 字节 vs 4 字节
        let (l, r) = write_pair(dir.path(), "a\r\nb\r\n", "a\nb\n").await;
        let sl = vfs.stat(&l).await.unwrap();
        let sr = vfs.stat(&r).await.unwrap();
        assert_ne!(sl.size, sr.size, "前提：size 确实不同");
        let v = content_verdict_stats(
            &vfs,
            &l,
            &r,
            FileStatSummary::new(sl.size, sl.mtime_ms),
            FileStatSummary::new(sr.size, sr.mtime_ms),
            ContentCompareMethod::RulesBased,
            &imp,
            None,
        )
        .await
        .unwrap();
        assert_eq!(v, QuickVerdict::Same, "行尾差异在规则模式下不重要");
        // 真实内容差异（size 也不同）：按规则分类为重要差异
        let (l, r) = write_pair(dir.path(), "a\n", "a\nb\n").await;
        let sl = vfs.stat(&l).await.unwrap();
        let sr = vfs.stat(&r).await.unwrap();
        let v = content_verdict_stats(
            &vfs,
            &l,
            &r,
            FileStatSummary::new(sl.size, sl.mtime_ms),
            FileStatSummary::new(sr.size, sr.mtime_ms),
            ContentCompareMethod::RulesBased,
            &imp,
            None,
        )
        .await
        .unwrap();
        assert_eq!(v, QuickVerdict::RulesDiff);
    }
}
