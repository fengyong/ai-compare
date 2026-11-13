//! diff3 三向文本合并（docs/03 §4.5 MergeModel；FR-TMRG-001~007、FR-CLI-003
//! /automerge /favorleft|right /force 的引擎侧实现）。
//!
//! # 算法（经典 diff3，Khanna–Kunal–Pierce《A Formal Investigation of Diff3》语义）
//!
//! 1. base↔left、base↔right 两路行级 diff（[`crate::myers`]，在规范化比较键上
//!    运算、在原文上渲染），得到以 base 坐标表示的 hunk 序列；
//! 2. 按 base 坐标贪心合并两路 hunk：base 区间相接（`start <= 区域终点`，即
//!    之间无稳定行）的 hunk 归入同一不稳定区域，区域之间以稳定行分隔；
//! 3. 对每个不稳定区域的三方切片 (O, L, R) 分类：
//!    - L == O（键序列相等）→ [`ChunkOrigin::RightOnly`]（仅右侧变化）；
//!    - R == O → [`ChunkOrigin::LeftOnly`]；
//!    - L == R → [`ChunkOrigin::BothChanged`]（两侧相同变化，不算冲突，
//!      FR-TMRG-003）；
//!    - 其余 → [`ChunkOrigin::Conflict`]（类别 ModifyModify / DeleteModify /
//!      AddAdd）；
//!    - 三方全等的区域即稳定区 [`ChunkOrigin::Stable`]。
//! 4. 冲突窗口（FR-TMRG-003「N 行分隔」）：`conflict_window = Some(n)` 时，
//!    彼此间隔 ≤ n 稳定行的 LeftOnly/RightOnly 块合并为一个冲突块供审查。
//!
//! # BC 语义对齐（FR-MRG / FR-TMRG-005）
//!
//! - 重要性双键接入（P1 [`crate::keys`]）：冲突块内部做 L↔R 行级子对齐，
//!   逐行标注「仅不重要方面不同」（importance 键相等）；左右切片的
//!   importance 键序列整体相等 → **不重要冲突**，`favor`（受惠侧）或
//!   `ignore_unimportant`（/iu）开启时自动从受惠侧采用，不升级为未决冲突
//!   （不重要差异不产生冲突）；
//! - 块级决策（takeLeft/takeRight/takeBase/先左后右/手动编辑）→
//!   [`synthesize`] 合成输出；
//! - 输出带 conflict marker 模式（`<<<<<<<`/`|||||||`/`=======`/`>>>>>>>`，
//!   与 GNU diff3 -m / git merge-file 同形，供纯文本消费）。
//!
//! 与 GNU diff3 -m 的已知差异（对拍报告锚定，见 tests/merge3_matrix）：
//! - 冲突位于文件末尾且末行无换行符时，GNU diff3 会把 marker「粘」在内容行
//!   后（保留 no-newline 状态）；本实现与 git merge-file 一致，始终给 marker
//!   行与 marker 前内容行补换行。

use std::borrow::Cow;
use std::collections::HashMap;

use crate::keys::importance_key;
use crate::lines::{LineEnding, LineTable};
use crate::myers::{self, RawOp};
use crate::options::{ImportanceOptions, TextDiffOptions};
use crate::sections::{LineRange, SectionKind};

// ---------------------------------------------------------------------------
// 选项
// ---------------------------------------------------------------------------

/// 受惠侧（FR-TMRG-005 /favorleft|right）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FavorSide {
    Left,
    Right,
}

/// 未决冲突的默认策略（`auto_merge` 有冲突时逐冲突默认策略，可配）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictDefault {
    TakeLeft,
    TakeRight,
    TakeBase,
    /// 先左后右（FR-TMRG-004「先取左侧再取右侧」）
    TakeLeftThenRight,
    TakeRightThenLeft,
    /// 以 conflict marker 渲染（/force CVS 样式写冲突）
    Marker,
}

/// marker 风格：Merge（git merge-file 默认，无 base 段）/ Diff3（GNU diff3 -m，含 base 段）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkerStyle {
    Merge,
    Diff3,
}

/// marker 渲染标签
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerOptions {
    pub style: MarkerStyle,
    pub left_label: String,
    pub base_label: String,
    pub right_label: String,
}

impl MarkerOptions {
    /// 以三个文件名作标签
    pub fn from_names(base: &str, left: &str, right: &str, style: MarkerStyle) -> Self {
        MarkerOptions {
            style,
            left_label: left.to_string(),
            base_label: base.to_string(),
            right_label: right.to_string(),
        }
    }
}

/// 三向合并选项。
///
/// `diff` 为骨架选项（两路 base↔side diff 的规范化键语义）；重要性模型
/// （[`ImportanceOptions`]）只作用于冲突子对齐与不重要冲突判定，
/// 不改变行对齐骨架（与文本比较的 P1 语义一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOptions {
    pub diff: TextDiffOptions,
    pub importance: ImportanceOptions,
    /// 冲突窗口（FR-TMRG-003 N 行分隔；None = 仅更改不同的行，默认）
    pub conflict_window: Option<u32>,
    /// 受惠侧（不重要冲突自动采用）
    pub favor: Option<FavorSide>,
    /// 忽略不重要差异（/iu）：不重要冲突不升级为未决冲突
    pub ignore_unimportant: bool,
    /// 未决冲突默认策略（auto_merge；协议/CLI 层可配）
    pub conflict_default: ConflictDefault,
}

impl Default for MergeOptions {
    fn default() -> Self {
        MergeOptions {
            diff: TextDiffOptions::default(),
            importance: ImportanceOptions::default(),
            conflict_window: None,
            favor: None,
            ignore_unimportant: false,
            conflict_default: ConflictDefault::TakeLeft,
        }
    }
}

// ---------------------------------------------------------------------------
// 决策
// ---------------------------------------------------------------------------

/// 块级决策（FR-TMRG-004 采用命令族 + FR-TMRG-005 输出编辑）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeDecision {
    TakeLeft,
    TakeRight,
    TakeBase,
    TakeLeftThenRight,
    TakeRightThenLeft,
    /// 手动编辑文本（按 \n 分行；输出编辑，黄块语义）
    Manual(Vec<String>),
}

// ---------------------------------------------------------------------------
// 模型
// ---------------------------------------------------------------------------

/// 块来源（有序块序列；docs/03 §4.5 ChunkOrigin）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkOrigin {
    /// 稳定区（三方键相等）
    Stable,
    /// 仅左侧变化（取左）
    LeftOnly,
    /// 仅右侧变化（取右）
    RightOnly,
    /// 两侧相同变化（不算冲突）
    BothChanged,
    /// 左右不同变化（冲突）
    Conflict,
}

/// 冲突类别（验收语料分类：modify-modify / delete-modify / add-add）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictKind {
    ModifyModify,
    DeleteModify,
    AddAdd,
}

/// 冲突块内行级子对齐提示（L↔R 行配对；unimportant = 仅不重要方面不同）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConflictRow {
    /// 左侧行号（全表坐标；None = 该行仅右侧存在）
    pub left: Option<u32>,
    /// 右侧行号（None = 该行仅左侧存在）
    pub right: Option<u32>,
    /// importance 键相等（仅空白/大小写/行尾等不重要方面不同）
    pub unimportant: bool,
}

/// 冲突块详情
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictDetail {
    pub kind: ConflictKind,
    /// L↔R 行级子对齐（UI 子对齐提示 + 不重要冲突判定证据）
    pub rows: Vec<ConflictRow>,
    /// 不重要冲突：左右切片 importance 键序列整体相等
    pub unimportant: bool,
}

/// 合并块（每块携带三方行区间；三方行内容可由三张 [`LineTable`] 按区间取出）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeChunk {
    pub id: u32,
    pub origin: ChunkOrigin,
    /// base 行区间（插入块为空区间占位）
    pub base: LineRange,
    pub left: LineRange,
    pub right: LineRange,
    /// Conflict 块独有
    pub conflict: Option<ConflictDetail>,
    /// favor / ignore_unimportant 自动采用（非 None 时冲突已决）
    pub auto_decision: Option<MergeDecision>,
}

/// 三向合并模型（有序块序列；三方行区间各自完备划分全表）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeModel {
    pub chunks: Vec<MergeChunk>,
    pub base_len: u32,
    pub left_len: u32,
    pub right_len: u32,
}

impl MergeModel {
    /// 冲突块总数（含已自动采用的）
    pub fn conflict_count(&self) -> u32 {
        self.chunks.iter().filter(|c| c.origin == ChunkOrigin::Conflict).count() as u32
    }

    /// 未决冲突块 id（无决策且无自动采用的冲突块）
    pub fn unresolved(&self) -> Vec<u32> {
        self.chunks
            .iter()
            .filter(|c| c.origin == ChunkOrigin::Conflict && c.auto_decision.is_none())
            .map(|c| c.id)
            .collect()
    }

    /// 计入决策后的未决冲突（decisions 显式覆盖 auto_decision）
    pub fn unresolved_with(&self, decisions: &HashMap<u32, MergeDecision>) -> Vec<u32> {
        self.chunks
            .iter()
            .filter(|c| {
                c.origin == ChunkOrigin::Conflict && !decisions.contains_key(&c.id) && c.auto_decision.is_none()
            })
            .map(|c| c.id)
            .collect()
    }

    /// 自检：三方行区间各自连续、完备划分全表（调试/测试断言用）
    pub fn is_well_formed(&self) -> bool {
        let mut expect = [(0u32, 0u32); 3];
        for c in &self.chunks {
            let ranges = [(c.base, self.base_len), (c.left, self.left_len), (c.right, self.right_len)];
            for (i, (r, len)) in ranges.into_iter().enumerate() {
                if r.start != expect[i].0 || r.end > len {
                    return false;
                }
                expect[i] = (r.end, len);
            }
        }
        expect[0].0 == self.base_len && expect[1].0 == self.left_len && expect[2].0 == self.right_len
    }
}

// ---------------------------------------------------------------------------
// merge3 主入口
// ---------------------------------------------------------------------------

/// 三向合并主入口（经典 diff3；见模块文档）。
pub fn merge3(base: &LineTable, left: &LineTable, right: &LineTable, opts: &MergeOptions) -> MergeModel {
    let dl = hunks_from_diff(base, left, &opts.diff);
    let dr = hunks_from_diff(base, right, &opts.diff);

    let mut chunks: Vec<MergeChunk> = Vec::new();
    // 游标：base / left / right 已消费行数；i/j 为两路 hunk 下标
    let (mut o, mut l, mut r) = (0u32, 0u32, 0u32);
    let (mut i, mut j) = (0usize, 0usize);

    loop {
        // 下一不稳定区域的起点（最早 hunk 的 base 起点）
        let next_a = dl.get(i).map(|h| h.base_start);
        let next_b = dr.get(j).map(|h| h.base_start);
        let p = match (next_a, next_b) {
            (None, None) => break,
            (Some(x), None) => x,
            (None, Some(y)) => y,
            (Some(x), Some(y)) => x.min(y),
        };
        // 稳定区 [o, p)：三方键相等，1:1 对应
        if p > o {
            let d = p - o;
            chunks.push(chunk(
                chunks.len() as u32,
                ChunkOrigin::Stable,
                LineRange::new(o, p),
                LineRange::new(l, l + d),
                LineRange::new(r, r + d),
                None,
            ));
            l += d;
            r += d;
        }
        // 收集不稳定区域：base 区间相接（之间无稳定行）的 hunk 全部并入
        // （o 游标随区域终点在下方统一推进）
        let start = p;
        let mut end = p;
        let (i0, j0) = (i, j);
        loop {
            let mut progressed = false;
            while i < dl.len() && dl[i].base_start <= end {
                end = end.max(dl[i].base_end);
                i += 1;
                progressed = true;
            }
            while j < dr.len() && dr[j].base_start <= end {
                end = end.max(dr[j].base_end);
                j += 1;
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
        // 三方切片：base [start, end)；left/right 按 hunk 膨胀量换算
        let l_expand: i64 = dl[i0..i].iter().map(|h| h.side_len() as i64 - h.base_len() as i64).sum();
        let r_expand: i64 = dr[j0..j].iter().map(|h| h.side_len() as i64 - h.base_len() as i64).sum();
        let l_len = (end - start) as i64 + l_expand;
        let r_len = (end - start) as i64 + r_expand;
        let (l_len, r_len) = (l_len.max(0) as u32, r_len.max(0) as u32);
        let (ls, le, rs, re) = (l, l + l_len, r, r + r_len);
        l = le;
        r = re;
        o = end;

        // 分类（键序列比较；L == O → RightOnly 等）
        let left_is_base = l_len == end - start
            && keys_equal(base, start, end, left, ls, le, &opts.diff);
        let right_is_base = r_len == end - start
            && keys_equal(base, start, end, right, rs, re, &opts.diff);
        let origin = if left_is_base && right_is_base {
            ChunkOrigin::Stable
        } else if left_is_base {
            ChunkOrigin::RightOnly
        } else if right_is_base {
            ChunkOrigin::LeftOnly
        } else if l_len == r_len && keys_equal(left, ls, le, right, rs, re, &opts.diff) {
            ChunkOrigin::BothChanged
        } else {
            ChunkOrigin::Conflict
        };

        let conflict = if origin == ChunkOrigin::Conflict {
            Some(build_conflict_detail(
                LineRange::new(start, end),
                left,
                LineRange::new(ls, le),
                right,
                LineRange::new(rs, re),
                opts,
            ))
        } else {
            None
        };
        chunks.push(chunk(
            chunks.len() as u32,
            origin,
            LineRange::new(start, end),
            LineRange::new(ls, le),
            LineRange::new(rs, re),
            conflict,
        ));
    }

    // 尾部稳定区
    if o < base.len() as u32 {
        let d = base.len() as u32 - o;
        chunks.push(chunk(
            chunks.len() as u32,
            ChunkOrigin::Stable,
            LineRange::new(o, o + d),
            LineRange::new(l, l + d),
            LineRange::new(r, r + d),
            None,
        ));
    }

    let mut model = MergeModel {
        chunks,
        base_len: base.len() as u32,
        left_len: left.len() as u32,
        right_len: right.len() as u32,
    };
    model.chunks = apply_conflict_window(model.chunks, left, right, opts);
    renumber(&mut model.chunks);
    apply_auto_resolve(&mut model, opts);
    debug_assert!(model.is_well_formed(), "块序列必须完备划分三方");
    model
}

/// 重编号 chunk id = 下标（决策按 id 引用，窗口合并后统一重排）
fn renumber(chunks: &mut [MergeChunk]) {
    for (i, c) in chunks.iter_mut().enumerate() {
        c.id = i as u32;
    }
}

/// 块构造（三方区间 + 冲突详情；auto_decision 由 apply_auto_resolve 统一填充）
fn chunk(id: u32, origin: ChunkOrigin, base: LineRange, left: LineRange, right: LineRange, conflict: Option<ConflictDetail>) -> MergeChunk {
    MergeChunk { id, origin, base, left, right, conflict, auto_decision: None }
}

/// 单路 diff 的 hunk（base 坐标 + 对侧坐标；半开区间）
#[derive(Debug, Clone, Copy)]
struct Hunk {
    base_start: u32,
    base_end: u32,
    side_start: u32,
    side_end: u32,
}

impl Hunk {
    fn base_len(&self) -> u32 {
        self.base_end - self.base_start
    }
    fn side_len(&self) -> u32 {
        self.side_end - self.side_start
    }
}

/// diff(base, side) -> hunk 序列（按 base 升序；相邻 hunk 之间至少隔一条稳定行
/// 或直接相接，由 [`build_section_vec`] 的 section 序列保证单调不重叠）
fn hunks_from_diff(base: &LineTable, side: &LineTable, opts: &TextDiffOptions) -> Vec<Hunk> {
    let out = crate::diff_text(base, side, opts).expect("无手动锚点的 diff 不会失败");
    let mut hunks = Vec::new();
    let (mut o, mut s) = (0u32, 0u32);
    for sec in &out.sections {
        match sec.kind {
            SectionKind::Equal => {
                let l = sec.left.expect("Equal 必有左区间");
                o += l.len();
                s += l.len();
            }
            SectionKind::Change => {
                let (Some(l), Some(r)) = (sec.left, sec.right) else {
                    unreachable!("Change 必有两侧区间");
                };
                hunks.push(Hunk { base_start: l.start, base_end: l.end, side_start: r.start, side_end: r.end });
                o = l.end;
                s = r.end;
            }
            SectionKind::Delete => {
                let l = sec.left.expect("Delete 必有左区间");
                hunks.push(Hunk { base_start: l.start, base_end: l.end, side_start: s, side_end: s });
                o = l.end;
            }
            SectionKind::Insert => {
                let r = sec.right.expect("Insert 必有右区间");
                hunks.push(Hunk { base_start: o, base_end: o, side_start: r.start, side_end: r.end });
                s = r.end;
            }
        }
    }
    hunks
}

/// 两行区间键序列相等（长度相等 + 逐行比较键相等）
fn keys_equal(
    a: &LineTable,
    a_start: u32,
    a_end: u32,
    b: &LineTable,
    b_start: u32,
    b_end: u32,
    opts: &TextDiffOptions,
) -> bool {
    if a_end - a_start != b_end - b_start {
        return false;
    }
    let n = a_end - a_start;
    for k in 0..n {
        if a.compare_key((a_start + k) as usize, opts) != b.compare_key((b_start + k) as usize, opts) {
            return false;
        }
    }
    true
}

/// 冲突详情：类别 + L↔R 行级子对齐 + 不重要判定
/// （base_span 为冲突块的 base 区间；仅用长度判定类别，base 表本身不参与）
fn build_conflict_detail(
    base_span: LineRange,
    left: &LineTable,
    left_span: LineRange,
    right: &LineTable,
    right_span: LineRange,
    opts: &MergeOptions,
) -> ConflictDetail {
    let (ls, le) = (left_span.start, left_span.end);
    let (rs, re) = (right_span.start, right_span.end);
    let kind = if base_span.is_empty() {
        ConflictKind::AddAdd
    } else if left_span.is_empty() || right_span.is_empty() {
        ConflictKind::DeleteModify
    } else {
        ConflictKind::ModifyModify
    };

    // 子对齐：L↔R 在 raw 键上 diff（Myers），逐行配对并标注 importance
    let l_keys: Vec<Cow<'_, str>> = (ls..le).map(|i| left.compare_key(i as usize, &opts.diff)).collect();
    let r_keys: Vec<Cow<'_, str>> = (rs..re).map(|i| right.compare_key(i as usize, &opts.diff)).collect();
    let ops = myers::diff_seq(&l_keys, &r_keys);
    let mut rows = Vec::new();
    let (mut li, mut ri) = (0usize, 0usize);
    let mut k = 0usize;
    while k < ops.len() {
        match ops[k] {
            RawOp::Equal(n) => {
                for _ in 0..n {
                    rows.push(ConflictRow {
                        left: Some(ls + li as u32),
                        right: Some(rs + ri as u32),
                        unimportant: true,
                    });
                    li += 1;
                    ri += 1;
                }
                k += 1;
            }
            RawOp::Delete(d) => {
                // 删除 + 可能相邻插入（coalesce 后 Delete 在前）= Change：位置配对
                // 行数 = max(d, ins)（多余一侧为孤儿行，与 keys.rs 配对语义一致）
                let ins = match ops.get(k + 1) {
                    Some(RawOp::Insert(m)) => *m,
                    _ => 0,
                };
                for p in 0..d.max(ins) as usize {
                    let l_pos = if p < d as usize { Some(ls + li as u32 + p as u32) } else { None };
                    let r_pos = if p < ins as usize { Some(rs + ri as u32 + p as u32) } else { None };
                    let unimportant = match (l_pos, r_pos) {
                        (Some(a), Some(b)) => {
                            importance_key(left.line(a as usize), &opts.diff, &opts.importance, Some(a as usize))
                                == importance_key(right.line(b as usize), &opts.diff, &opts.importance, Some(b as usize))
                        }
                        _ => false,
                    };
                    rows.push(ConflictRow { left: l_pos, right: r_pos, unimportant });
                }
                li += d as usize;
                ri += ins as usize;
                k += if ins > 0 { 2 } else { 1 };
            }
            RawOp::Insert(n) => {
                // 独立插入（前无删除）：孤儿行
                for p in 0..n {
                    rows.push(ConflictRow { left: None, right: Some(rs + ri as u32 + p), unimportant: false });
                }
                ri += n as usize;
                k += 1;
            }
        }
    }

    // 不重要冲突：左右切片 importance 键序列整体相等
    let unimportant = le - ls == re - rs
        && (ls..le).zip(rs..re).all(|(a, b)| {
            importance_key(left.line(a as usize), &opts.diff, &opts.importance, Some(a as usize))
                == importance_key(right.line(b as usize), &opts.diff, &opts.importance, Some(b as usize))
        });

    ConflictDetail { kind, rows, unimportant }
}

/// 冲突窗口（FR-TMRG-003）：间隔 ≤ n 稳定行的 LeftOnly/RightOnly 块合并为冲突块。
/// 单侧连续变化（如两个 LeftOnly）不合并不产生冲突。
///
/// 前置：merge3 主遍历产出的块序列严格 Stable/变化块交替（见模块文档），
/// 相邻变化块之间恰有一个非空 Stable 块（其 base 长度即间隔）。
fn apply_conflict_window(
    chunks: Vec<MergeChunk>,
    left: &LineTable,
    right: &LineTable,
    opts: &MergeOptions,
) -> Vec<MergeChunk> {
    let Some(window) = opts.conflict_window else {
        return chunks;
    };
    let mut out: Vec<MergeChunk> = Vec::with_capacity(chunks.len());
    let mut idx = 0usize;
    while idx < chunks.len() {
        let c = &chunks[idx];
        if !matches!(c.origin, ChunkOrigin::LeftOnly | ChunkOrigin::RightOnly) {
            out.push(c.clone());
            idx += 1;
            continue;
        }
        // 收集 run：变化块 [idx..=last]，相邻变化块间隔（中间 Stable 的 base 长度）≤ window
        let mut last = idx;
        let mut has_left = c.origin == ChunkOrigin::LeftOnly;
        let mut has_right = c.origin == ChunkOrigin::RightOnly;
        let mut mixed = false;
        loop {
            let next_change = last + 2;
            if next_change >= chunks.len()
                || chunks[last + 1].origin != ChunkOrigin::Stable
                || chunks[last + 1].base.len() > window
                || !matches!(chunks[next_change].origin, ChunkOrigin::LeftOnly | ChunkOrigin::RightOnly)
            {
                break;
            }
            last = next_change;
            has_left |= chunks[last].origin == ChunkOrigin::LeftOnly;
            has_right |= chunks[last].origin == ChunkOrigin::RightOnly;
            mixed |= has_left && has_right;
        }
        if mixed && last > idx {
            let first = &chunks[idx];
            let tail = &chunks[last];
            let detail = build_conflict_detail(
                LineRange::new(first.base.start, tail.base.end),
                left,
                LineRange::new(first.left.start, tail.left.end),
                right,
                LineRange::new(first.right.start, tail.right.end),
                opts,
            );
            out.push(chunk(
                0,
                ChunkOrigin::Conflict,
                LineRange::new(first.base.start, tail.base.end),
                LineRange::new(first.left.start, tail.left.end),
                LineRange::new(first.right.start, tail.right.end),
                Some(detail),
            ));
        } else {
            out.extend(chunks[idx..=last].iter().cloned());
        }
        idx = last + 1;
    }
    out
}

/// favor / ignore_unimportant：不重要冲突自动从受惠侧采用（FR-TMRG-005）
fn apply_auto_resolve(model: &mut MergeModel, opts: &MergeOptions) {
    let auto = opts.favor.is_some() || opts.ignore_unimportant;
    if !auto {
        return;
    }
    let take = match opts.favor {
        Some(FavorSide::Left) | None => MergeDecision::TakeLeft,
        Some(FavorSide::Right) => MergeDecision::TakeRight,
    };
    for c in &mut model.chunks {
        if c.origin == ChunkOrigin::Conflict
            && c.auto_decision.is_none()
            && c.conflict.as_ref().is_some_and(|d| d.unimportant)
        {
            c.auto_decision = Some(take.clone());
        }
    }
}

// ---------------------------------------------------------------------------
// 输出合成
// ---------------------------------------------------------------------------

/// 未决冲突渲染策略（auto_merge 逐冲突默认策略 + marker 模式）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnresolvedPolicy {
    TakeLeft,
    TakeRight,
    TakeBase,
    TakeLeftThenRight,
    TakeRightThenLeft,
    /// conflict marker 渲染（/force；style + 标签）
    Markers(MarkerOptions),
}

impl ConflictDefault {
    /// 转为未决冲突渲染策略（markers 提供默认标签；None 时用空标签）
    pub fn to_policy(self, markers: Option<&MarkerOptions>) -> UnresolvedPolicy {
        match self {
            ConflictDefault::TakeLeft => UnresolvedPolicy::TakeLeft,
            ConflictDefault::TakeRight => UnresolvedPolicy::TakeRight,
            ConflictDefault::TakeBase => UnresolvedPolicy::TakeBase,
            ConflictDefault::TakeLeftThenRight => UnresolvedPolicy::TakeLeftThenRight,
            ConflictDefault::TakeRightThenLeft => UnresolvedPolicy::TakeRightThenLeft,
            ConflictDefault::Marker => {
                UnresolvedPolicy::Markers(markers.cloned().unwrap_or_else(|| MarkerOptions {
                    style: MarkerStyle::Merge,
                    left_label: String::new(),
                    base_label: String::new(),
                    right_label: String::new(),
                }))
            }
        }
    }
}

/// 合成结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutput {
    pub text: String,
    /// 未决冲突块 id（决策与自动采用均未覆盖的冲突块）
    pub unresolved: Vec<u32>,
    /// 输出总行数
    pub lines: u32,
}

/// 输出合成选项：未决冲突策略（默认 = marker，git merge 风格、空标签）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesizeOptions {
    pub unresolved: UnresolvedPolicy,
}

impl Default for SynthesizeOptions {
    fn default() -> Self {
        SynthesizeOptions { unresolved: UnresolvedPolicy::Markers(MarkerOptions::from_names("", "", "", MarkerStyle::Merge)) }
    }
}

/// 决策表 -> 合成输出（FR-TMRG-004/005：块级决策 + auto_merge + marker 模式）。
///
/// 幂等：同一 (model, decisions, options) 恒产出同一文本。
pub fn synthesize(
    model: &MergeModel,
    base: &LineTable,
    left: &LineTable,
    right: &LineTable,
    decisions: &HashMap<u32, MergeDecision>,
    opts: &SynthesizeOptions,
) -> MergeOutput {
    let mut parts: Vec<(String, LineEnding)> = Vec::new();
    let mut unresolved = Vec::new();

    for c in &model.chunks {
        let decision = decisions.get(&c.id).or(c.auto_decision.as_ref());
        match decision {
            Some(d) => push_decision(&mut parts, base, left, right, c, d),
            None => match c.origin {
                ChunkOrigin::Stable | ChunkOrigin::LeftOnly | ChunkOrigin::BothChanged => {
                    push_lines(&mut parts, left, c.left.start, c.left.end);
                }
                ChunkOrigin::RightOnly => push_lines(&mut parts, right, c.right.start, c.right.end),
                ChunkOrigin::Conflict => {
                    unresolved.push(c.id);
                    match &opts.unresolved {
                        UnresolvedPolicy::TakeLeft => push_lines(&mut parts, left, c.left.start, c.left.end),
                        UnresolvedPolicy::TakeRight => push_lines(&mut parts, right, c.right.start, c.right.end),
                        UnresolvedPolicy::TakeBase => push_lines(&mut parts, base, c.base.start, c.base.end),
                        UnresolvedPolicy::TakeLeftThenRight => {
                            push_lines(&mut parts, left, c.left.start, c.left.end);
                            push_lines(&mut parts, right, c.right.start, c.right.end);
                        }
                        UnresolvedPolicy::TakeRightThenLeft => {
                            push_lines(&mut parts, right, c.right.start, c.right.end);
                            push_lines(&mut parts, left, c.left.start, c.left.end);
                        }
                        UnresolvedPolicy::Markers(m) => {
                            push_markers(&mut parts, base, left, right, c, m);
                        }
                    }
                }
            },
        }
    }

    let text = join_parts(&parts);
    MergeOutput { text, unresolved, lines: parts.len() as u32 }
}

/// 便捷入口：无决策自动合成（auto_merge）
pub fn auto_merge_text(
    model: &MergeModel,
    base: &LineTable,
    left: &LineTable,
    right: &LineTable,
    opts: &SynthesizeOptions,
) -> MergeOutput {
    synthesize(model, base, left, right, &HashMap::new(), opts)
}

fn push_decision(
    parts: &mut Vec<(String, LineEnding)>,
    base: &LineTable,
    left: &LineTable,
    right: &LineTable,
    c: &MergeChunk,
    d: &MergeDecision,
) {
    match d {
        MergeDecision::TakeLeft => push_lines(parts, left, c.left.start, c.left.end),
        MergeDecision::TakeRight => push_lines(parts, right, c.right.start, c.right.end),
        MergeDecision::TakeBase => push_lines(parts, base, c.base.start, c.base.end),
        MergeDecision::TakeLeftThenRight => {
            push_lines(parts, left, c.left.start, c.left.end);
            push_lines(parts, right, c.right.start, c.right.end);
        }
        MergeDecision::TakeRightThenLeft => {
            push_lines(parts, right, c.right.start, c.right.end);
            push_lines(parts, left, c.left.start, c.left.end);
        }
        MergeDecision::Manual(lines) => push_manual(parts, left, right, base, c, lines),
    }
}

/// 从行表取行（保留原文与行尾风格）
fn push_lines(parts: &mut Vec<(String, LineEnding)>, table: &LineTable, start: u32, end: u32) {
    for i in start..end {
        let e = table.line(i as usize);
        parts.push((e.text.clone(), e.ending));
    }
}

/// 手动编辑行：行尾取 LF；末行继承该块自然内容末行的行尾（保持文件级
/// 尾换行语义确定性；FR-TMRG-005 输出编辑）
fn push_manual(
    parts: &mut Vec<(String, LineEnding)>,
    left: &LineTable,
    right: &LineTable,
    base: &LineTable,
    c: &MergeChunk,
    lines: &[String],
) {
    let natural_last = if !c.left.is_empty() {
        Some(left.line((c.left.end - 1) as usize).ending)
    } else if !c.right.is_empty() {
        Some(right.line((c.right.end - 1) as usize).ending)
    } else if !c.base.is_empty() {
        Some(base.line((c.base.end - 1) as usize).ending)
    } else {
        None
    };
    let n = lines.len();
    for (k, text) in lines.iter().enumerate() {
        let ending = if k + 1 < n {
            LineEnding::Lf
        } else {
            natural_last.unwrap_or(LineEnding::Lf)
        };
        parts.push((text.clone(), ending));
    }
}

/// conflict marker 块（FR-TMRG-005 /force CVS 样式；Diff3 风格含 base 段）。
/// marker 行尾随冲突块内容的多数行尾风格（CRLF 文件输出 CRLF marker，与
/// git merge-file 同形）；marker 前的内容行末行缺行尾时补同风格行尾（git
/// merge-file 行为；GNU diff3 -m 会保留 no-newline 粘连，见模块文档已知差异）。
fn push_markers(
    parts: &mut Vec<(String, LineEnding)>,
    base: &LineTable,
    left: &LineTable,
    right: &LineTable,
    c: &MergeChunk,
    m: &MarkerOptions,
) {
    let eol = marker_ending(base, left, right, c);
    parts.push((format!("<<<<<<< {}", m.left_label), eol));
    push_lines_forced(parts, left, c.left.start, c.left.end, eol);
    if m.style == MarkerStyle::Diff3 {
        parts.push((format!("||||||| {}", m.base_label), eol));
        push_lines_forced(parts, base, c.base.start, c.base.end, eol);
    }
    parts.push(("=======".to_string(), eol));
    push_lines_forced(parts, right, c.right.start, c.right.end, eol);
    parts.push((format!(">>>>>>> {}", m.right_label), eol));
}

/// marker 行尾风格 = 冲突块三方内容行的多数行尾（None 不计；平票取 LF）
fn marker_ending(base: &LineTable, left: &LineTable, right: &LineTable, c: &MergeChunk) -> LineEnding {
    let mut crlf = 0u32;
    let mut lf = 0u32;
    let mut cr = 0u32;
    for (t, r) in [(base, c.base), (left, c.left), (right, c.right)] {
        for i in r.start..r.end {
            match t.line(i as usize).ending {
                LineEnding::CrLf => crlf += 1,
                LineEnding::Lf => lf += 1,
                LineEnding::Cr => cr += 1,
                LineEnding::None => {}
            }
        }
    }
    if crlf > lf && crlf > cr {
        LineEnding::CrLf
    } else if cr > lf && cr > crlf {
        LineEnding::Cr
    } else {
        LineEnding::Lf
    }
}

/// 取行并强制末行行尾为指定风格（marker 块内不允许无行尾行）
fn push_lines_forced(
    parts: &mut Vec<(String, LineEnding)>,
    table: &LineTable,
    start: u32,
    end: u32,
    forced: LineEnding,
) {
    for i in start..end {
        let e = table.line(i as usize);
        let ending = if i + 1 == end && e.ending == LineEnding::None {
            forced
        } else {
            e.ending
        };
        parts.push((e.text.clone(), ending));
    }
}

fn join_parts(parts: &[(String, LineEnding)]) -> String {
    let mut out = String::with_capacity(parts.iter().map(|(t, e)| t.len() + e.as_str().len()).sum());
    for (t, e) in parts {
        out.push_str(t);
        out.push_str(e.as_str());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::WhitespaceMode;

    fn t(text: &str) -> LineTable {
        LineTable::from_text(text)
    }

    fn opts() -> MergeOptions {
        MergeOptions::default()
    }

    fn marker_policy(style: MarkerStyle) -> SynthesizeOptions {
        SynthesizeOptions {
            unresolved: UnresolvedPolicy::Markers(MarkerOptions::from_names("B", "L", "R", style)),
        }
    }

    // ------------------------------------------------------------------
    // 基础分类
    // ------------------------------------------------------------------

    #[test]
    fn identical_inputs_single_stable_chunk() {
        let m = merge3(&t("a\nb\n"), &t("a\nb\n"), &t("a\nb\n"), &opts());
        assert_eq!(m.chunks.len(), 1);
        assert_eq!(m.chunks[0].origin, ChunkOrigin::Stable);
        assert!(m.is_well_formed());
        assert_eq!(m.conflict_count(), 0);
    }

    #[test]
    fn disjoint_changes_merge_cleanly() {
        // 左改头、右改尾（不相邻）→ 无冲突自动合成
        let base = t("a\nb\nc\nd\ne\n");
        let left = t("A\nb\nc\nd\ne\n");
        let right = t("a\nb\nc\nd\nE\n");
        let m = merge3(&base, &left, &right, &opts());
        let kinds: Vec<ChunkOrigin> = m.chunks.iter().map(|c| c.origin).collect();
        assert_eq!(kinds, vec![ChunkOrigin::LeftOnly, ChunkOrigin::Stable, ChunkOrigin::RightOnly]);
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "A\nb\nc\nd\nE\n");
        assert!(out.unresolved.is_empty());
    }

    #[test]
    fn same_change_both_sides_is_not_conflict() {
        // 两侧相同变化 → bothChanged，不算冲突（FR-TMRG-003）
        let base = t("a\nb\nc\n");
        let left = t("a\nX\nc\n");
        let right = t("a\nX\nc\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.chunks[1].origin, ChunkOrigin::BothChanged);
        assert_eq!(m.conflict_count(), 0);
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "a\nX\nc\n");
    }

    #[test]
    fn modify_modify_conflict_with_markers() {
        let base = t("a\nb\nc\n");
        let left = t("a\nLEFT\nc\n");
        let right = t("a\nRIGHT\nc\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.chunks.len(), 3);
        assert_eq!(m.chunks[1].origin, ChunkOrigin::Conflict);
        let cf = m.chunks[1].conflict.as_ref().unwrap();
        assert_eq!(cf.kind, ConflictKind::ModifyModify);
        assert!(!cf.unimportant);
        // 子对齐：一行对一行、不重要 = false
        assert_eq!(cf.rows.len(), 1);
        assert_eq!(cf.rows[0].left, Some(1));
        assert_eq!(cf.rows[0].right, Some(1));
        assert!(!cf.rows[0].unimportant);

        // Diff3 风格 marker（与 GNU diff3 -m 同形）
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(
            out.text,
            "a\n<<<<<<< L\nLEFT\n||||||| B\nb\n=======\nRIGHT\n>>>>>>> R\nc\n"
        );
        assert_eq!(out.unresolved, vec![1]);
        // git merge 风格（无 base 段）
        let out2 = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Merge));
        assert_eq!(out2.text, "a\n<<<<<<< L\nLEFT\n=======\nRIGHT\n>>>>>>> R\nc\n");
    }

    #[test]
    fn add_add_and_delete_modify_kinds() {
        // 同一位置两侧插入不同内容 → AddAdd
        let base = t("a\nb\n");
        let left = t("a\nL1\nL2\nb\n");
        let right = t("a\nR1\nb\n");
        let m = merge3(&base, &left, &right, &opts());
        let cf = m.chunks.iter().find(|c| c.origin == ChunkOrigin::Conflict).unwrap();
        assert_eq!(cf.conflict.as_ref().unwrap().kind, ConflictKind::AddAdd);

        // 左删右改 → DeleteModify
        let base = t("a\nb\nc\n");
        let left = t("a\nc\n");
        let right = t("a\nB!\nc\n");
        let m = merge3(&base, &left, &right, &opts());
        let cf = m.chunks.iter().find(|c| c.origin == ChunkOrigin::Conflict).unwrap();
        assert_eq!(cf.conflict.as_ref().unwrap().kind, ConflictKind::DeleteModify);
    }

    #[test]
    fn interleaved_changes_are_separate_chunks() {
        // 交错变化（左改行 1、右改行 3、左改行 5）→ 各自独立块，无冲突
        let base = t("1\n2\n3\n4\n5\n");
        let left = t("L1\n2\n3\n4\nL5\n");
        let right = t("1\n2\nR3\n4\n5\n");
        let m = merge3(&base, &left, &right, &opts());
        let kinds: Vec<ChunkOrigin> = m.chunks.iter().map(|c| c.origin).collect();
        assert_eq!(
            kinds,
            vec![
                ChunkOrigin::LeftOnly,
                ChunkOrigin::Stable,
                ChunkOrigin::RightOnly,
                ChunkOrigin::Stable,
                ChunkOrigin::LeftOnly
            ]
        );
        assert_eq!(m.conflict_count(), 0);
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "L1\n2\nR3\n4\nL5\n");
    }

    #[test]
    fn adjacent_inserts_conflict() {
        // 同一位置两侧插入不同内容（相接无稳定行）→ 单一冲突块
        let base = t("a\nb\n");
        let left = t("a\nL1\nL2\nb\n");
        let right = t("a\nR1\nR2\nb\n");
        let m = merge3(&base, &left, &right, &opts());
        let conflicts: Vec<&MergeChunk> = m.chunks.iter().filter(|c| c.origin == ChunkOrigin::Conflict).collect();
        assert_eq!(conflicts.len(), 1, "相邻插入合并为一个冲突块: {m:?}");
        assert_eq!(conflicts[0].conflict.as_ref().unwrap().kind, ConflictKind::AddAdd);
    }

    // ------------------------------------------------------------------
    // 空文件 / 边界
    // ------------------------------------------------------------------

    #[test]
    fn empty_base_both_sides_add_same() {
        let base = t("");
        let left = t("x\n");
        let right = t("x\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.chunks[0].origin, ChunkOrigin::BothChanged);
        assert_eq!(auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3)).text, "x\n");
    }

    #[test]
    fn empty_base_conflicting_adds() {
        let base = t("");
        let left = t("L\n");
        let right = t("R\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.chunks[0].origin, ChunkOrigin::Conflict);
        assert_eq!(m.chunks[0].conflict.as_ref().unwrap().kind, ConflictKind::AddAdd);
    }

    #[test]
    fn empty_side_deletion_vs_other_change() {
        let base = t("a\n");
        let left = t(""); // 全删
        let right = t("a\nextra\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.chunks[0].origin, ChunkOrigin::Conflict);
        assert_eq!(m.chunks[0].conflict.as_ref().unwrap().kind, ConflictKind::DeleteModify);
    }

    #[test]
    fn both_delete_same_region_is_clean() {
        let base = t("a\nb\nc\n");
        let left = t("a\nc\n");
        let right = t("a\nc\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.conflict_count(), 0);
        assert_eq!(auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3)).text, "a\nc\n");
    }

    #[test]
    fn eof_inserts_both_sides() {
        // 文件末尾两侧各加不同行（相邻）→ 冲突覆盖到 EOF
        let base = t("a\n");
        let left = t("a\nL\n");
        let right = t("a\nR\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.chunks.len(), 2, "Stable + Conflict");
        assert_eq!(m.chunks[1].origin, ChunkOrigin::Conflict);
    }

    // ------------------------------------------------------------------
    // 决策合成
    // ------------------------------------------------------------------

    #[test]
    fn decisions_take_left_right_base() {
        let base = t("a\nb\nc\n");
        let left = t("a\nLEFT\nc\n");
        let right = t("a\nRIGHT\nc\n");
        let m = merge3(&base, &left, &right, &opts());

        let mut d = HashMap::new();
        d.insert(1u32, MergeDecision::TakeRight);
        let out = synthesize(&m, &base, &left, &right, &d, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "a\nRIGHT\nc\n");
        assert!(out.unresolved.is_empty(), "决策后冲突已决");

        d.insert(1, MergeDecision::TakeBase);
        let out = synthesize(&m, &base, &left, &right, &d, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "a\nb\nc\n");

        d.insert(1, MergeDecision::TakeLeftThenRight);
        let out = synthesize(&m, &base, &left, &right, &d, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "a\nLEFT\nRIGHT\nc\n");
    }

    #[test]
    fn manual_edit_replaces_chunk() {
        let base = t("a\nb\nc\n");
        let left = t("a\nLEFT\nc\n");
        let right = t("a\nRIGHT\nc\n");
        let m = merge3(&base, &left, &right, &opts());
        let mut d = HashMap::new();
        d.insert(1u32, MergeDecision::Manual(vec!["手动一".into(), "手动二".into()]));
        let out = synthesize(&m, &base, &left, &right, &d, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "a\n手动一\n手动二\nc\n");
    }

    #[test]
    fn decide_on_non_conflict_chunk_reverts() {
        // 对 leftOnly 块 takeBase = 还原为基（FR-TMRG-005 再次采用还原语义）
        let base = t("a\nb\nc\n");
        let left = t("a\nX\nc\n");
        let right = t("a\nb\nc\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.chunks[1].origin, ChunkOrigin::LeftOnly);
        let mut d = HashMap::new();
        d.insert(1u32, MergeDecision::TakeBase);
        let out = synthesize(&m, &base, &left, &right, &d, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "a\nb\nc\n");
    }

    #[test]
    fn synthesize_is_idempotent_and_stable() {
        let base = t("h\na\nb\nc\nt\n");
        let left = t("h\nL1\nb\nL2\nt\n");
        let right = t("h\nR1\nb\nR2\nt\n");
        let m = merge3(&base, &left, &right, &opts());
        // 两个冲突块（行 1、行 3 各一）分别决策
        let mut d = HashMap::new();
        d.insert(1u32, MergeDecision::TakeLeft);
        d.insert(3u32, MergeDecision::TakeRight);
        let o1 = synthesize(&m, &base, &left, &right, &d, &marker_policy(MarkerStyle::Diff3));
        let o2 = synthesize(&m, &base, &left, &right, &d, &marker_policy(MarkerStyle::Diff3));
        let o3 = synthesize(&m, &base, &left, &right, &d, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(o1, o2);
        assert_eq!(o2, o3, "重复决策不漂移");
        assert_eq!(o1.text, "h\nL1\nb\nR2\nt\n");
        assert!(o1.unresolved.is_empty());
    }

    // ------------------------------------------------------------------
    // 冲突窗口（FR-TMRG-003 N 行分隔）
    // ------------------------------------------------------------------

    #[test]
    fn conflict_window_merges_nearby_changes() {
        // 左改行 1、右改行 3，中间 1 条稳定行
        let base = t("a\nmid\nc\nd\n");
        let left = t("A\nmid\nc\nd\n");
        let right = t("a\nmid\nC\nd\n");
        // 默认（仅更改不同的行）：两个独立块，无冲突
        let m0 = merge3(&base, &left, &right, &opts());
        assert_eq!(m0.conflict_count(), 0);
        // 窗口 N=1（间隔 ≤1 行即冲突）→ 合并为一个冲突块
        let m1 = merge3(
            &base,
            &left,
            &right,
            &MergeOptions { conflict_window: Some(1), ..opts() },
        );
        assert_eq!(m1.conflict_count(), 1, "{m1:?}");
        assert!(m1.chunks.iter().any(|c| c.origin == ChunkOrigin::Conflict));
        let cf = m1.chunks.iter().find(|c| c.origin == ChunkOrigin::Conflict).unwrap();
        assert_eq!(cf.base, LineRange::new(0, 3), "冲突块跨稳定间隔");
        // 窗口 N=0 不够近（间隔 1 行 > 0）→ 不合并
        let m2 = merge3(
            &base,
            &left,
            &right,
            &MergeOptions { conflict_window: Some(0), ..opts() },
        );
        assert_eq!(m2.conflict_count(), 0);
    }

    #[test]
    fn conflict_window_ignores_same_side_runs() {
        // 同侧两个相邻变化：窗口开启也不产生冲突
        let base = t("a\nmid\nc\n");
        let left = t("A\nmid\nC\n");
        let right = t("a\nmid\nc\n");
        let m = merge3(
            &base,
            &left,
            &right,
            &MergeOptions { conflict_window: Some(5), ..opts() },
        );
        assert_eq!(m.conflict_count(), 0);
        let kinds: Vec<ChunkOrigin> = m.chunks.iter().map(|c| c.origin).collect();
        assert!(kinds.iter().all(|k| !matches!(k, ChunkOrigin::Conflict)));
    }

    // ------------------------------------------------------------------
    // 重要性（FR-MRG：不重要差异不产生冲突）
    // ------------------------------------------------------------------

    #[test]
    fn unimportant_conflict_auto_resolves_with_favor() {
        // 两侧把同一行改成仅尾随空白不同：默认（全重要）→ 冲突；
        // favor + 尾随空白不重要 → 自动采用受惠侧，不升级为未决冲突
        let base = t("a\nb\nc\n");
        let left = t("a\nB  \nc\n");
        let right = t("a\nB\nc\n");
        let plain = merge3(&base, &left, &right, &opts());
        assert_eq!(plain.conflict_count(), 1);
        assert_eq!(plain.unresolved(), vec![1]);

        let mut o = opts();
        o.importance.trailing_ws_unimportant = true;
        o.favor = Some(FavorSide::Right);
        let m = merge3(&base, &left, &right, &o);
        assert_eq!(m.conflict_count(), 1, "仍是冲突块（来源标记保留）");
        assert!(m.unresolved().is_empty(), "不重要冲突已自动采用受惠侧");
        assert_eq!(m.chunks[1].auto_decision, Some(MergeDecision::TakeRight));
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "a\nB\nc\n");
    }

    #[test]
    fn ignore_unimportant_reduces_conflicts() {
        // /iu：不重要冲突自动解决（无 favor 时取左）
        let base = t("a\nb\nc\n");
        let left = t("a\nB\t\nc\n");
        let right = t("a\nB\nc\n");
        let o = MergeOptions {
            ignore_unimportant: true,
            importance: ImportanceOptions { trailing_ws_unimportant: true, ..ImportanceOptions::default() },
            ..opts()
        };
        let m = merge3(&base, &left, &right, &o);
        assert!(m.unresolved().is_empty());
        assert_eq!(m.chunks[1].auto_decision, Some(MergeDecision::TakeLeft));
    }

    #[test]
    fn skeleton_options_shrink_conflicts_via_keys() {
        // 骨架空白语义开启（-w）：空白差异在对齐层消失 → 同效变化不再冲突
        let base = t("a\nb\nc\n");
        let left = t("a\nB  \nc\n");
        let right = t("a\nB\nc\n");
        let o = MergeOptions {
            diff: TextDiffOptions { whitespace: WhitespaceMode::IgnoreAll, ..TextDiffOptions::default() },
            ..opts()
        };
        let m = merge3(&base, &left, &right, &o);
        assert_eq!(m.conflict_count(), 0);
        assert_eq!(m.chunks[1].origin, ChunkOrigin::BothChanged);
    }

    #[test]
    fn important_conflict_stays_with_favor() {
        // 重要冲突不受 favor 影响（仍为未决）
        let base = t("a\nb\nc\n");
        let left = t("a\nLEFT\nc\n");
        let right = t("a\nRIGHT\nc\n");
        let o = MergeOptions { favor: Some(FavorSide::Left), ..opts() };
        let m = merge3(&base, &left, &right, &o);
        assert_eq!(m.unresolved(), vec![1], "重要冲突不自动采用");
    }

    #[test]
    fn conflict_rows_mark_unimportant_pairs() {
        // 混合冲突（重叠区域）：子对齐逐行标注重要性（其中一行对仅空白不同）
        let base = t("k1\na\nb\nc\nk2\n");
        let left = t("k1\nL\nb  \nc\nk2\n");
        let right = t("k1\nR\nb\nc!\nk2\n");
        let mut o = opts();
        o.importance.trailing_ws_unimportant = true;
        let m = merge3(&base, &left, &right, &o);
        let cf = m.chunks.iter().find(|c| c.origin == ChunkOrigin::Conflict).unwrap();
        let d = cf.conflict.as_ref().unwrap();
        assert!(!d.unimportant, "整体重要（L vs R 行重要差异）");
        assert_eq!(d.rows.len(), 3, "L=[L,b  ,c] R=[R,b,c!] 位置配对 3 行");
        assert!(!d.rows[0].unimportant, "L vs R 重要");
        assert!(d.rows[1].unimportant, "b vs b  仅尾随空白不同");
        assert!(!d.rows[2].unimportant, "c vs c! 重要");
    }

    // ------------------------------------------------------------------
    // CRLF / Unicode / 行尾
    // ------------------------------------------------------------------

    #[test]
    fn crlf_corpus_preserves_endings() {
        let base = t("a\r\nb\r\nc\r\n");
        let left = t("A\r\nb\r\nc\r\n");
        let right = t("a\r\nb\r\nC\r\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.conflict_count(), 0);
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "A\r\nb\r\nC\r\n");
    }

    #[test]
    fn unicode_lines_merge() {
        // 非相邻变化（中间隔稳定行）→ 干净合并；相邻变化按 diff3 语义为冲突
        let base = t("中文\n中隔\n日本語\n");
        let left = t("中文！\n中隔\n日本語\n");
        let right = t("中文\n中隔\n日本語。\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.conflict_count(), 0);
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "中文！\n中隔\n日本語。\n");

        // 相邻变化（无稳定行分隔）→ 冲突（与 GNU diff3 -m 实测一致）
        let base = t("中文\n日本語\n");
        let left = t("中文！\n日本語\n");
        let right = t("中文\n日本語。\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.conflict_count(), 1, "相邻变化合并为一个冲突区");
    }

    #[test]
    fn eol_style_only_change_is_not_conflict() {
        // 默认忽略行尾风格：仅 CRLF/LF 差异的两侧变化 = bothChanged
        let base = t("a\nb\n");
        let left = t("a\r\nb\r\n");
        let right = t("a\nb\n");
        let m = merge3(&base, &left, &right, &opts());
        assert_eq!(m.chunks[0].origin, ChunkOrigin::Stable);
    }

    // ------------------------------------------------------------------
    // marker 边界
    // ------------------------------------------------------------------

    #[test]
    fn eof_conflict_forces_newline_before_markers() {
        // 冲突在 EOF 且末行无换行：内容行补 LF（git merge-file 行为）
        let base = t("a\nb");
        let left = t("a\nLEFT");
        let right = t("a\nRIGHT");
        let m = merge3(&base, &left, &right, &opts());
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Merge));
        assert_eq!(out.text, "a\n<<<<<<< L\nLEFT\n=======\nRIGHT\n>>>>>>> R\n");
    }

    #[test]
    fn crlf_conflict_uses_crlf_markers() {
        // CRLF 文件的 marker 行尾随内容多数行尾（git merge-file 同形）
        let base = t("a\r\nb\r\nc\r\n");
        let left = t("a\r\nL\r\nc\r\n");
        let right = t("a\r\nR\r\nc\r\n");
        let m = merge3(&base, &left, &right, &opts());
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Merge));
        assert_eq!(
            out.text,
            "a\r\n<<<<<<< L\r\nL\r\n=======\r\nR\r\n>>>>>>> R\r\nc\r\n"
        );
    }

    #[test]
    fn no_conflict_output_preserves_missing_trailing_newline() {
        let base = t("a\nb\nc");
        let left = t("A\nb\nc");
        let right = t("a\nb\nc");
        let m = merge3(&base, &left, &right, &opts());
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3));
        assert_eq!(out.text, "A\nb\nc");
    }

    // ------------------------------------------------------------------
    // 重复行歧义：结果仍须自洽（完备划分 + 重放正确）
    // ------------------------------------------------------------------

    #[test]
    fn repeated_lines_ambiguity_stays_well_formed() {
        let base = t("x\nrep\nrep\nrep\nx\n");
        let left = t("x\nrep\nrep\nx\n"); // 左删一行 rep
        let right = t("x\nrep\nrep\nrep\nrep\nx\n"); // 右加一行 rep
        let m = merge3(&base, &left, &right, &opts());
        assert!(m.is_well_formed());
        // 无论对齐歧义如何，自动合成不冲突时必须可重放
        let out = auto_merge_text(&m, &base, &left, &right, &marker_policy(MarkerStyle::Diff3));
        if m.conflict_count() == 0 {
            assert!(out.text.starts_with("x\n") && out.text.ends_with("x\n"));
        }
    }
}
