//! 快速测试全集（FR-DIR-010，docs/03 §3.1.3）：仅凭目录项元数据即可判定的
//! 比较项 —— size / mtime（秒容差可配 / DST / 时区边界）/ 文件名大小写 /
//! DOS 属性位 / Unix 权限元数据（可配置开关）。
//!
//! 纯函数、无 IO；输出 [`QuickOutcome`]，内容比较由上层（lib.rs/session.rs）
//! 按 `content_compare` 模式解析。mtime 以毫秒比较（LocalFs 提供 ms 精度）。

use ai_protocol::CompareCriteria;
use ai_vfs::dos_attr;

/// 一小时（毫秒）：DST / 时区判定的基准跨度
pub const HOUR_MS: u64 = 3_600_000;

/// 快速测试结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickOutcome {
    /// 全部快速项一致（注意：`content_compare=Always` 时上层仍复核内容）
    Same,
    /// 某快速项判异（内容比较仅可覆盖 Mtime 项；其余为最终判定）
    Diff(QuickDiffReason),
    /// 快速项不一致但可由内容比较解析（mtime 差异 + size 相同）
    NeedContent,
}

/// 快速判异原因
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickDiffReason {
    /// 文件大小不同（确定性判异）
    Size,
    /// 文件名大小写不同（compare_case 开启）
    Case,
    /// 选中的 DOS 属性位有差异（compare_attributes 开启）
    Attributes,
    /// Unix 权限/所有者/组有差异（unix_meta 开启）
    UnixMeta,
    /// mtime 超出容差且内容比较被禁用（Never）——上层映射 QuickDiff
    Mtime,
}

/// 快速测试器（按判据构造；无状态，可复用）
#[derive(Debug, Clone)]
pub struct QuickTest<'a> {
    criteria: &'a CompareCriteria,
    /// 参与比较的属性位掩码（attribute_letters 或全部已知位）
    attr_mask: u8,
}

impl<'a> QuickTest<'a> {
    pub fn new(criteria: &'a CompareCriteria) -> Self {
        let attr_mask = match &criteria.attribute_letters {
            Some(letters) => dos_attr::from_letters(letters),
            None => 0xFF,
        };
        QuickTest { criteria, attr_mask }
    }

    /// 综合快速判定。`lname`/`rname` 为两侧对齐后的文件名
    /// （默认对齐下相同；大小写不敏感对齐/不同扩展名对齐下可能不同）。
    pub fn verdict(&self, ls: &ai_vfs::StatInfo, rs: &ai_vfs::StatInfo, lname: &str, rname: &str) -> QuickOutcome {
        // 1. 大小：不同 = 确定性判异（内容必然不同，无需读内容）
        if ls.size != rs.size {
            return QuickOutcome::Diff(QuickDiffReason::Size);
        }
        // 2. 文件名大小写（FR-DIR-010 比较文件名大小写）
        if self.criteria.compare_case && lname != rname && name_eq_ignore_case(lname, rname) {
            return QuickOutcome::Diff(QuickDiffReason::Case);
        }
        // 3. DOS 属性位（默认关闭：复制文件即可改变存档属性）
        if self.criteria.compare_attributes {
            let l = ls.dos_attrs & self.attr_mask;
            let r = rs.dos_attrs & self.attr_mask;
            if l != r {
                return QuickOutcome::Diff(QuickDiffReason::Attributes);
            }
        }
        // 4. Unix 元数据（权限位/uid/gid；可配置开关；任一侧无数据则跳过）
        if self.criteria.unix_meta {
            if let (Some(lm), Some(rm)) = (ls.unix_mode, rs.unix_mode) {
                // 比较权限位 + 特殊位（不含文件类型位）
                if lm & 0o7777 != rm & 0o7777
                    || ls.uid != rs.uid
                    || ls.gid != rs.gid
                {
                    return QuickOutcome::Diff(QuickDiffReason::UnixMeta);
                }
            }
        }
        // 5. mtime：秒容差 / DST / 时区
        if Self::mtimes_match(ls.mtime_ms, rs.mtime_ms, self.criteria) {
            return match self.criteria.content_compare {
                // 覆盖快速测试结果（FR-DIR-011）：即便快速判同也复核内容
                ai_protocol::ContentCompareMode::Always => QuickOutcome::NeedContent,
                _ => QuickOutcome::Same,
            };
        }
        // mtime 判异：
        match self.criteria.content_compare {
            ai_protocol::ContentCompareMode::Never => QuickOutcome::Diff(QuickDiffReason::Mtime),
            // Auto：mtime 判异但 size 相同 -> 内容复核；Always 同理
            ai_protocol::ContentCompareMode::Auto | ai_protocol::ContentCompareMode::Always => {
                QuickOutcome::NeedContent
            }
        }
    }

    /// mtime 匹配：|d| <= 秒容差；或 DST（恰好 ±1h）；或时区（整小时倍数）。
    ///
    /// 边界：d == 0 恒同；DST 规则是时区规则的真子集（60min 整除）。
    pub fn mtimes_match(l_ms: u64, r_ms: u64, criteria: &CompareCriteria) -> bool {
        let d = l_ms.abs_diff(r_ms);
        // 容差以秒计（毫秒精度：|l-r| <= N 秒）
        let tol_ms = u64::from(criteria.mtime_tolerance_sec).saturating_mul(1000);
        if d <= tol_ms {
            return true;
        }
        if criteria.ignore_dst_hour && d == HOUR_MS {
            return true;
        }
        if criteria.ignore_tz_hour && d % HOUR_MS == 0 {
            return true;
        }
        false
    }
}

/// ASCII 不区分大小写的文件名相等（BC 名称比较默认行为）
pub fn name_eq_ignore_case(a: &str, b: &str) -> bool {
    a.chars().count() == b.chars().count()
        && a.chars().zip(b.chars()).all(|(x, y)| {
            x.to_lowercase().eq(y.to_lowercase())
        })
}

/// DOS 属性位串（SideInfo.attribute 用；与 ai_vfs::dos_attr 对齐）
pub fn attr_letters(mask: u8) -> String {
    dos_attr::to_letters(mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_protocol::{CompareCriteria, ContentCompareMode};
    use ai_vfs::StatInfo;

    fn stat(size: u64, mtime_ms: u64, attrs: u8, mode: Option<u32>) -> StatInfo {
        StatInfo {
            size,
            mtime_ms,
            kind: ai_vfs::EntryKind::File,
            unix_mode: mode,
            uid: Some(1000),
            gid: Some(1000),
            dos_attrs: attrs,
        }
    }

    fn crit(tol: u32, mode: ContentCompareMode) -> CompareCriteria {
        CompareCriteria { mtime_tolerance_sec: tol, content_compare: mode, ..Default::default() }
    }

    #[test]
    fn size_diff_is_deterministic() {
        let c = crit(2, ContentCompareMode::Auto);
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(10, 0, 0, None), &stat(12, 0, 0, None), "a", "a"), QuickOutcome::Diff(QuickDiffReason::Size));
    }

    #[test]
    fn mtime_tolerance_and_dst_and_tz() {
        let mut c = crit(2, ContentCompareMode::Never);
        // 恰好 3600s 差异：默认 ignoreDstHour=true 判同
        c.mtime_tolerance_sec = 0;
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 0, 0, None), &stat(1, HOUR_MS, 0, None), "a", "a"), QuickOutcome::Same);
        // 关闭 DST：3600s 差异判异（Never -> Mtime）
        c.ignore_dst_hour = false;
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 0, 0, None), &stat(1, HOUR_MS, 0, None), "a", "a"), QuickOutcome::Diff(QuickDiffReason::Mtime));
        // 时区：2 小时（2*3600ms 整倍）仅在 ignoreTzHour 开启时判同
        c.ignore_tz_hour = true;
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 0, 0, None), &stat(1, 2 * HOUR_MS, 0, None), "a", "a"), QuickOutcome::Same);
        // 非 3600 整倍（如 5400s）不判同
        assert_eq!(t.verdict(&stat(1, 0, 0, None), &stat(1, 5_400_000, 0, None), "a", "a"), QuickOutcome::Diff(QuickDiffReason::Mtime));
        // 容差上限：|d| == tol*1000 同；+1ms 异
        let c2 = crit(3, ContentCompareMode::Never);
        let t2 = QuickTest::new(&c2);
        assert_eq!(t2.verdict(&stat(1, 1_000_000, 0, None), &stat(1, 1_003_000, 0, None), "a", "a"), QuickOutcome::Same);
        assert_eq!(t2.verdict(&stat(1, 1_000_000, 0, None), &stat(1, 1_003_001, 0, None), "a", "a"), QuickOutcome::Diff(QuickDiffReason::Mtime));
    }

    #[test]
    fn case_difference_only_when_enabled() {
        let mut c = crit(60, ContentCompareMode::Never);
        c.compare_case = true;
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 0, 0, None), &stat(1, 0, 0, None), "Readme.TXT", "readme.txt"), QuickOutcome::Diff(QuickDiffReason::Case));
        c.compare_case = false;
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 0, 0, None), &stat(1, 0, 0, None), "Readme.TXT", "readme.txt"), QuickOutcome::Same);
        // 完全不同名（对齐器不会配对，防御性断言：不算 Case 原因）
        assert!(!name_eq_ignore_case("a", "b"));
    }

    #[test]
    fn attribute_bits_respect_letters_subset() {
        let mut c = crit(0, ContentCompareMode::Never);
        c.compare_attributes = true;
        // 左 r+hidden，右仅 r：默认全集参与 -> 判异
        c.attribute_letters = None;
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 0, 0x03, None), &stat(1, 0, 0x01, None), "a", "a"), QuickOutcome::Diff(QuickDiffReason::Attributes));
        // 只比较 r 位（"r"）：hidden 不参与 -> 判同
        c.attribute_letters = Some("r".into());
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 0, 0x03, None), &stat(1, 0, 0x01, None), "a", "a"), QuickOutcome::Same);
    }

    #[test]
    fn unix_meta_gate() {
        let mut c = crit(0, ContentCompareMode::Never);
        c.unix_meta = true;
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 0, 0, Some(0o100_644)), &stat(1, 0, 0, Some(0o100_600)), "a", "a"), QuickOutcome::Diff(QuickDiffReason::UnixMeta));
        // uid 差异
        let mut l = stat(1, 0, 0, Some(0o100_644));
        let mut r = l;
        l.uid = Some(0);
        r.uid = Some(1000);
        assert_eq!(t.verdict(&l, &r, "a", "a"), QuickOutcome::Diff(QuickDiffReason::UnixMeta));
        // 关闭开关：不判异
        c.unix_meta = false;
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 0, 0, Some(0o100_644)), &stat(1, 0, 0, Some(0o100_600)), "a", "a"), QuickOutcome::Same);
        // 任意一侧无 mode 数据：跳过（视为不参与）
        c.unix_meta = true;
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 0, 0, None), &stat(1, 0, 0, Some(0o100_600)), "a", "a"), QuickOutcome::Same);
    }

    #[test]
    fn always_mode_demands_content_even_when_quick_same() {
        let c = crit(60, ContentCompareMode::Always);
        let t = QuickTest::new(&c);
        assert_eq!(t.verdict(&stat(1, 5, 0, None), &stat(1, 5, 0, None), "a", "a"), QuickOutcome::NeedContent);
    }
}
