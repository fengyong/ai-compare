//! 目录项对齐（FR-DIR-020/021/022/023）：
//! - [`ByNameAligner`]：按名从左到右（默认）；
//! - [`CaseFoldAligner`]：大小写不敏感文件系统上的按名对齐（可叠加不同
//!   扩展名/Unicode 归一化选项，FR-DIR-010 对齐项）；
//! - [`PatternAligner`]：模式对齐覆盖（FR-DIR-023 `*.gif`↔`*.png`，Pro 的
//!   数据层扩展口；本阶段作为规则引擎开放）；
//! - 手动对齐：[`ManualAlignApplier`] 在已建 arena 上施加/解除非同名配对
//!   （FR-DIR-022，协议 dir/align 的落点）。
//!
//! 契约（[`NameAligner`]）：输入两侧条目**已按名字典序排序**；输出按
//! 名字典序的配对序列，且恰好覆盖两侧全部条目（每条目恰出现一次）。

use std::collections::BTreeMap;

use ai_vfs::VfsDirEntry;

/// 一对对齐结果（单侧缺失为 None；索引为输入下标）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlignedPair {
    pub left: Option<u32>,
    pub right: Option<u32>,
    pub name: String,
}

/// 名称对齐器（扩展口：按名 / 大小写折叠 / 模式 / 手动对齐）
pub trait NameAligner: Send + Sync {
    fn mode_name(&self) -> &'static str;
    fn align(&self, left: &[VfsDirEntry], right: &[VfsDirEntry]) -> Vec<AlignedPair>;
}

/// 输出按键字典序且覆盖全部条目的配对构造器（各实现共用）
pub(crate) fn build_pairs(
    left: &[VfsDirEntry],
    right: &[VfsDirEntry],
    lkey: impl Fn(&VfsDirEntry) -> String,
    rkey: impl Fn(&VfsDirEntry) -> String,
) -> Vec<AlignedPair> {
    // key -> (左下标列表, 右下标列表)；同名多重出现按序 zip（平铺模式复用）
    let mut lmap: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut rmap: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (i, e) in left.iter().enumerate() {
        lmap.entry(lkey(e)).or_default().push(i as u32);
    }
    for (i, e) in right.iter().enumerate() {
        rmap.entry(rkey(e)).or_default().push(i as u32);
    }
    let mut out = Vec::with_capacity(left.len() + right.len());
    let keys: std::collections::BTreeSet<&String> = lmap.keys().chain(rmap.keys()).collect();
    for key in keys {
        let ls = lmap.get(key).cloned().unwrap_or_default();
        let rs = rmap.get(key).cloned().unwrap_or_default();
        let n = ls.len().max(rs.len());
        for k in 0..n {
            let name = match (ls.get(k), rs.get(k)) {
                (Some(li), _) => left[*li as usize].name.clone(),
                (None, Some(ri)) => right[*ri as usize].name.clone(),
                (None, None) => unreachable!(),
            };
            out.push(AlignedPair {
                left: ls.get(k).copied(),
                right: rs.get(k).copied(),
                name,
            });
        }
    }
    out
}

/// 按名对齐（大小写敏感精确匹配；P0/P2 默认）
#[derive(Debug, Clone, Copy, Default)]
pub struct ByNameAligner;

impl NameAligner for ByNameAligner {
    fn mode_name(&self) -> &'static str {
        "byName"
    }
    fn align(&self, left: &[VfsDirEntry], right: &[VfsDirEntry]) -> Vec<AlignedPair> {
        debug_assert!(
            left.windows(2).all(|w| w[0].name <= w[1].name)
                && right.windows(2).all(|w| w[0].name <= w[1].name),
            "输入必须已排序",
        );
        build_pairs(left, right, |e| e.name.clone(), |e| e.name.clone())
    }
}

/// 对齐键定制（FR-DIR-010 对齐项：不同扩展名 / Unicode 归一化 / 大小写）
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AlignKeyOptions {
    /// 忽略文件名大小写（大小写不敏感文件系统对齐）
    pub case_insensitive: bool,
    /// 对齐具有不同扩展名的文件名（`report.docx` ↔ `report.pdf`）
    pub ignore_extension: bool,
    /// Unicode 等价对齐（ Latin 重音剥离近似：é ↔ e + 组合符折叠）
    pub unicode_fold: bool,
}

/// 键函数：按选项折叠名字
pub fn align_key(name: &str, opts: AlignKeyOptions) -> String {
    let mut s = name.to_string();
    if opts.ignore_extension {
        if let Some(dot) = s.rfind('.') {
            if dot > 0 {
                s.truncate(dot);
            }
        }
    }
    if opts.unicode_fold {
        s = strip_accent_marks(&s);
    }
    if opts.case_insensitive {
        s = s.chars().flat_map(|c| c.to_lowercase()).collect();
    }
    s
}

/// 折叠 Latin-1/Latin Extended-A 的预组合重音字符（近似 NFD；无依赖实现）。
/// 非覆盖完整 Unicode，但覆盖西欧常见名（FR-DIR-010 的 P2 近似）。
pub fn strip_accent_marks(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'Ç' => 'C',
            'ç' => 'c',
            'È' | 'É' | 'Ê' | 'Ë' => 'E',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'Ñ' => 'N',
            'ñ' => 'n',
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'O',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'Ý' => 'Y',
            'ý' | 'ÿ' => 'y',
            'Æ' => 'A', // 独立字符近似
            'æ' => 'a',
            'Ø' => 'O',
            'ø' => 'o',
            'ß' => 's',
            other => other,
        })
        .collect()
}

/// 带键选项的按名对齐
#[derive(Debug, Clone, Copy, Default)]
pub struct CaseFoldAligner {
    pub options: AlignKeyOptions,
}

impl NameAligner for CaseFoldAligner {
    fn mode_name(&self) -> &'static str {
        "caseFoldName"
    }
    fn align(&self, left: &[VfsDirEntry], right: &[VfsDirEntry]) -> Vec<AlignedPair> {
        build_pairs(
            left,
            right,
            |e| align_key(&e.name, self.options),
            |e| align_key(&e.name, self.options),
        )
    }
}

/// 模式对齐覆盖规则（FR-DIR-023：左掩码 ↔ 右掩码；DOS 通配符语义）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternAlignRule {
    /// 左侧掩码（如 `*.gif`）
    pub left_mask: String,
    /// 右侧掩码（如 `*.png`）
    pub right_mask: String,
    /// 限制到此文件夹（rel 路径前缀；None = 全树）
    pub limit_to_dir: Option<String>,
    /// 大小写不敏感（掩码匹配）
    pub case_insensitive: bool,
}

/// 模式对齐器：先按名对齐，未配对项再按规则表左掩码↔右掩码配对。
/// 规则按序尝试，首条双侧都命中且互不冲突者生效。
#[derive(Debug, Clone, Default)]
pub struct PatternAligner {
    pub rules: Vec<PatternAlignRule>,
    /// 基础键选项（不同扩展名对齐等，FR-DIR-010）
    pub key_options: AlignKeyOptions,
}

impl PatternAligner {
    /// 目录级限制由 session 层按 rel 前缀过滤（本层为单层条目匹配）
    pub fn dir_limited_rule(&self, rule: &PatternAlignRule, rel_dir: &str) -> bool {
        match &rule.limit_to_dir {
            None => true,
            Some(prefix) => rel_dir == prefix || rel_dir.starts_with(&format!("{prefix}/")),
        }
    }
}

impl NameAligner for PatternAligner {
    fn mode_name(&self) -> &'static str {
        "pattern"
    }
    fn align(&self, left: &[VfsDirEntry], right: &[VfsDirEntry]) -> Vec<AlignedPair> {
        // 1. 按名对齐
        let pairs = build_pairs(
            left,
            right,
            |e| align_key(&e.name, self.key_options),
            |e| align_key(&e.name, self.key_options),
        );
        // 2. 未配对项按规则配对（仅文件；目录保持孤儿）。每条目至多被占用一次。
        let mut l_used: std::collections::BTreeSet<u32> = Default::default();
        let mut r_used: std::collections::BTreeSet<u32> = Default::default();
        let mut pattern_pairs: Vec<AlignedPair> = Vec::new();
        for rule in &self.rules {
            let li = left.iter().enumerate().position(|(i, e)| {
                !l_used.contains(&(i as u32))
                    && e.stat.kind != ai_vfs::EntryKind::Dir
                    && crate::filter::wildcard_match(
                        &rule.left_mask,
                        &e.name,
                        !rule.case_insensitive,
                    )
            });
            let li = match li {
                Some(i) => i as u32,
                None => continue,
            };
            let ri = right.iter().enumerate().position(|(i, e)| {
                !r_used.contains(&(i as u32))
                    && e.stat.kind != ai_vfs::EntryKind::Dir
                    && crate::filter::wildcard_match(
                        &rule.right_mask,
                        &e.name,
                        !rule.case_insensitive,
                    )
            });
            if let Some(ri) = ri.map(|i| i as u32) {
                l_used.insert(li);
                r_used.insert(ri);
                pattern_pairs.push(AlignedPair {
                    left: Some(li),
                    right: Some(ri),
                    name: left[li as usize].name.clone(),
                });
            }
        }
        // 3. 重组：被模式对占用的名字对拆散为剩余侧孤儿；两侧都被占用则丢弃
        let mut out: Vec<AlignedPair> = Vec::with_capacity(pairs.len() + pattern_pairs.len());
        for p in pairs {
            let l_taken = p.left.is_some_and(|l| l_used.contains(&l));
            let r_taken = p.right.is_some_and(|r| r_used.contains(&r));
            let (mut nl, mut nr) = (p.left, p.right);
            let mut name = p.name;
            if l_taken {
                nl = None;
                if let Some(r) = p.right {
                    name = right[r as usize].name.clone();
                }
            }
            if r_taken {
                nr = None;
                if let Some(l) = p.left {
                    name = left[l as usize].name.clone();
                }
            }
            if nl.is_none() && nr.is_none() {
                continue;
            }
            out.push(AlignedPair {
                left: nl,
                right: nr,
                name,
            });
        }
        out.extend(pattern_pairs);
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

/// 手动对齐施加器（FR-DIR-022）：在 arena 上建立/解除非同名配对。
/// 父文件夹必须并排（对 rel 路径的父段要求两侧同名）。
#[derive(Debug, Default)]
pub struct ManualAlignApplier;

#[cfg(test)]
mod tests {
    use super::*;
    use ai_vfs::{EntryKind, StatInfo};

    fn e(name: &str, dir: bool) -> VfsDirEntry {
        VfsDirEntry {
            name: name.to_string(),
            stat: StatInfo::minimal(1, 0, if dir { EntryKind::Dir } else { EntryKind::File }),
        }
    }

    fn names(pairs: &[AlignedPair]) -> Vec<(String, Option<u32>, Option<u32>)> {
        pairs
            .iter()
            .map(|p| (p.name.clone(), p.left, p.right))
            .collect()
    }

    #[allow(unused)]
    fn nm(n: &str) -> String {
        n.to_string()
    }

    #[test]
    fn by_name_covers_all_and_sorted() {
        let left = vec![e("a", false), e("b", false), e("d", false)];
        let right = vec![e("b", false), e("c", false), e("d", false)];
        let pairs = ByNameAligner.align(&left, &right);
        assert_eq!(
            names(&pairs),
            vec![
                (nm("a"), Some(0), None),
                (nm("b"), Some(1), Some(0)),
                (nm("c"), None, Some(1)),
                (nm("d"), Some(2), Some(2))
            ]
        );
    }

    #[test]
    fn case_insensitive_aligner_pairs_case_variants() {
        let left = vec![e("Readme.TXT", false)];
        let right = vec![e("readme.txt", false)];
        let pairs = CaseFoldAligner {
            options: AlignKeyOptions {
                case_insensitive: true,
                ..Default::default()
            },
        }
        .align(&left, &right);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].left.is_some() && pairs[0].right.is_some());
    }

    #[test]
    fn different_extension_alignment() {
        let left = vec![e("report.docx", false)];
        let right = vec![e("report.pdf", false)];
        let pairs = CaseFoldAligner {
            options: AlignKeyOptions {
                ignore_extension: true,
                ..Default::default()
            },
        }
        .align(&left, &right);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].left.is_some() && pairs[0].right.is_some());
    }

    #[test]
    fn unicode_accent_fold() {
        let left = vec![e("café.txt", false)];
        let right = vec![e("cafe.txt", false)];
        let pairs = CaseFoldAligner {
            options: AlignKeyOptions {
                unicode_fold: true,
                ..Default::default()
            },
        }
        .align(&left, &right);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].left.is_some() && pairs[0].right.is_some());
    }

    #[test]
    fn pattern_aligner_pairs_masks() {
        // *.gif ↔ *.png：按名不配对，模式配对
        let left = vec![e("icon.gif", false)];
        let right = vec![e("icon.png", false)];
        let aligner = PatternAligner {
            rules: vec![PatternAlignRule {
                left_mask: "*.gif".into(),
                right_mask: "*.png".into(),
                limit_to_dir: None,
                case_insensitive: false,
            }],
            key_options: AlignKeyOptions::default(),
        };
        let pairs = aligner.align(&left, &right);
        assert_eq!(pairs.len(), 1, "模式对齐应配对 gif↔png");
        assert!(pairs[0].left.is_some() && pairs[0].right.is_some());
    }

    #[test]
    fn duplicate_names_zip_in_order() {
        let left = vec![e("a", false), e("x/a", false)];
        let right = vec![e("a", false)];
        // 注意：输入约定为排序后的**单层**条目，重名仅出现在平铺场景
        let pairs = ByNameAligner.align(&left, &right);
        assert_eq!(pairs.len(), 2);
        assert!(pairs[0].left.is_some() && pairs[0].right.is_some());
        assert!(pairs[1].left.is_some() && pairs[1].right.is_none());
    }
}
