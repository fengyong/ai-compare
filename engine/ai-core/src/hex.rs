//! 十六进制比较引擎（P9 / FR-HEX-001~003 引擎侧；docs/03 §3.1 L2 规则：
//! 只面对"字节流"的纯函数，无 IO）。
//!
//! 能力面（FR-HEX-002 对齐算法三态）：
//! - `None`（无）：逐字节对齐——同偏移字节比较，长出尾部为 inserted/deleted；
//! - `Fast`（快速）：分块哈希粗对齐（Myers O(ND) 于块哈希序列）+ 块内逐字节
//!   精对齐（不感知块内插入/删除），适合大文件；
//! - `Full`（完全）：分块哈希粗对齐 + 非同区域字节级 Myers 精对齐
//!   （插入/删除感知；区域 > 1 MiB 时降级为 Fast 行为防失控）；
//! - 输出 hex section 模型：same/changed/inserted/deleted 块 + 字节区间；
//! - 每字节状态图（same/changed/inserted/deleted）供十六进制视图着色；
//! - ASCII 侧栏数据：16 字节/行渲染（视图窗口按 offset/length 分页取数，
//!   大文件策略：引擎持有字节流 + 视窗查询，UI 虚拟网格按需拉取）。

use std::time::{Duration, Instant};

use crate::myers::{diff_seq_deadline, RawOp};

/// 对齐模式（FR-HEX-002：完全 / 快速 / 无）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HexAlignMode {
    /// 完全：复杂算法对齐字节（插入/删除感知）
    #[default]
    Full,
    /// 快速：更简单更快的对齐，适合大文件
    Fast,
    /// 无：逐字节对齐
    None,
}

/// hex 比较选项
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexOptions {
    /// 分块大小（字节；对齐块粒度）
    pub chunk_size: usize,
    pub align_mode: HexAlignMode,
}

impl Default for HexOptions {
    fn default() -> Self {
        HexOptions {
            chunk_size: DEFAULT_CHUNK_SIZE,
            align_mode: HexAlignMode::Full,
        }
    }
}

/// 默认分块 4 KiB
pub const DEFAULT_CHUNK_SIZE: usize = 4096;
/// Full 模式字节级精对齐的单区域上限（超过降级 Fast，防 O(ND) 失控）
pub const FULL_REGION_MAX: usize = 1024 * 1024;
/// 分块对齐（块哈希 + 区域字节级 Myers）总时间预算。尺寸上限挡不住无公共
/// 子序列的随机数据（D≈N → O(N²)），故以时间兜底：超时 Myers 降级为粗粒度
/// 全删全插（合法脚本），避免冻结引擎 worker（PB-06 十六进制打开 1s 目标内）。
pub const FULL_REFINE_BUDGET_MS: u64 = 1000;
/// ASCII 侧栏每行字节数（十六进制编辑器惯例 16）
pub const HEX_LINE_BYTES: usize = 16;

/// 字节区间 [offset, offset+len)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub offset: u64,
    pub len: u64,
}

/// 块类别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexBlockKind {
    Same,
    Changed,
    /// 仅右侧存在（新增字节）
    Inserted,
    /// 仅左侧存在（删除字节）
    Deleted,
}

/// diff 块（same/changed/inserted/deleted + 双侧字节区间）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HexBlock {
    pub kind: HexBlockKind,
    /// 左侧字节区间（Inserted 时 None）
    pub left: Option<ByteRange>,
    /// 右侧字节区间（Deleted 时 None）
    pub right: Option<ByteRange>,
}

/// 统计
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HexDiffSummary {
    pub same_bytes: u64,
    pub changed_bytes: u64,
    pub inserted_bytes: u64,
    pub deleted_bytes: u64,
}

/// hex diff 完整模型
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexDiffModel {
    pub left_len: u64,
    pub right_len: u64,
    pub blocks: Vec<HexBlock>,
    pub summary: HexDiffSummary,
}

/// 单字节状态（视图着色用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexByteStatus {
    Same,
    Changed,
    /// 该字节仅右侧存在（左视图不用）
    Inserted,
    /// 该字节仅左侧存在（右视图不用）
    Deleted,
}

impl HexByteStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            HexByteStatus::Same => "same",
            HexByteStatus::Changed => "changed",
            HexByteStatus::Inserted => "inserted",
            HexByteStatus::Deleted => "deleted",
        }
    }
}

/// FNV-1a 64（块哈希；比较仍以字节相等复核，哈希仅用于粗对齐）
fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 块 diff 输出装配器：游标推进 + 相邻同类别块合并
struct BlockBuilder {
    blocks: Vec<HexBlock>,
    lo: u64,
    ro: u64,
    summary: HexDiffSummary,
}

impl BlockBuilder {
    fn new() -> Self {
        BlockBuilder {
            blocks: Vec::new(),
            lo: 0,
            ro: 0,
            summary: HexDiffSummary::default(),
        }
    }

    fn push(&mut self, kind: HexBlockKind, llen: u64, rlen: u64) {
        let left = (llen > 0).then_some(ByteRange {
            offset: self.lo,
            len: llen,
        });
        let right = (rlen > 0).then_some(ByteRange {
            offset: self.ro,
            len: rlen,
        });
        // 相邻同类别且区间连续 -> 合并
        if let Some(last) = self.blocks.last_mut() {
            let mergeable = last.kind == kind
                && match (&last.right, right) {
                    (Some(a), Some(b)) => a.offset + a.len == b.offset,
                    (None, None) => true,
                    _ => false,
                }
                && match (&last.left, left) {
                    (Some(a), Some(b)) => a.offset + a.len == b.offset,
                    (None, None) => true,
                    _ => false,
                };
            if mergeable {
                if let (Some(a), Some(b)) = (&mut last.left, left) {
                    a.len += b.len;
                }
                if let (Some(a), Some(b)) = (&mut last.right, right) {
                    a.len += b.len;
                }
                self.lo += llen;
                self.ro += rlen;
                self.tally(kind, llen, rlen);
                return;
            }
        }
        self.blocks.push(HexBlock { kind, left, right });
        self.lo += llen;
        self.ro += rlen;
        self.tally(kind, llen, rlen);
    }

    fn tally(&mut self, kind: HexBlockKind, llen: u64, rlen: u64) {
        match kind {
            HexBlockKind::Same => self.summary.same_bytes += llen,
            HexBlockKind::Changed => self.summary.changed_bytes += llen,
            HexBlockKind::Deleted => self.summary.deleted_bytes += llen,
            HexBlockKind::Inserted => self.summary.inserted_bytes += rlen,
        }
    }
}

/// 字节级 hex diff 主入口（FR-HEX-002 三种对齐）
pub fn diff_bytes(left: &[u8], right: &[u8], opts: &HexOptions) -> HexDiffModel {
    let mut b = BlockBuilder::new();
    let deadline = Instant::now() + Duration::from_millis(FULL_REFINE_BUDGET_MS);
    match opts.align_mode {
        HexAlignMode::None => diff_bytewise(left, right, &mut b),
        HexAlignMode::Fast => diff_chunked(left, right, opts.chunk_size.max(1), false, deadline, &mut b),
        HexAlignMode::Full => diff_chunked(left, right, opts.chunk_size.max(1), true, deadline, &mut b),
    }
    let summary = b.summary;
    HexDiffModel {
        left_len: left.len() as u64,
        right_len: right.len() as u64,
        blocks: b.blocks,
        summary,
    }
}

/// 无对齐：逐字节比较（FR-HEX-002 "无"）
fn diff_bytewise(left: &[u8], right: &[u8], b: &mut BlockBuilder) {
    let n = left.len().min(right.len());
    let mut i = 0usize;
    while i < n {
        let same = left[i] == right[i];
        let run_start = i;
        while i < n && (left[i] == right[i]) == same {
            i += 1;
        }
        let len = (i - run_start) as u64;
        b.push(
            if same {
                HexBlockKind::Same
            } else {
                HexBlockKind::Changed
            },
            len,
            len,
        );
    }
    if left.len() > n {
        b.push(HexBlockKind::Deleted, (left.len() - n) as u64, 0);
    }
    if right.len() > n {
        b.push(HexBlockKind::Inserted, 0, (right.len() - n) as u64);
    }
}

/// 分块哈希粗对齐 + 块内精对齐（Fast/Full 共用骨架）
fn diff_chunked(left: &[u8], right: &[u8], chunk: usize, full_refine: bool, deadline: Instant, b: &mut BlockBuilder) {
    let lh = chunk_hashes(left, chunk);
    let rh = chunk_hashes(right, chunk);
    let lhashes: Vec<u64> = lh.iter().map(|c| c.hash).collect();
    let rhashes: Vec<u64> = rh.iter().map(|c| c.hash).collect();
    let (ops, _degraded) = diff_seq_deadline(&lhashes, &rhashes, Some(deadline));

    let mut li = 0usize; // chunk index
    let mut ri = 0usize;
    let mut k = 0usize;
    while k < ops.len() {
        if matches!(ops[k], RawOp::Equal(_)) {
            let n = match ops[k] {
                RawOp::Equal(n) => n as usize,
                _ => unreachable!(),
            };
            // 哈希相等 -> 字节复核（防哈希碰撞假同）；连续 Same 块由 builder 合并
            for _ in 0..n {
                let lb = chunk_slice(left, &lh[li]);
                let rb = chunk_slice(right, &rh[ri]);
                if lb == rb {
                    b.push(HexBlockKind::Same, lb.len() as u64, rb.len() as u64);
                } else {
                    refine_pair(lb, rb, full_refine, b);
                }
                li += 1;
                ri += 1;
            }
            k += 1;
            continue;
        }
        // 等值段之间的 Delete/Insert 连续段（任意交错）作为一个区域精对齐
        let mut d = 0usize;
        let mut ins = 0usize;
        while k < ops.len() && !matches!(ops[k], RawOp::Equal(_)) {
            match ops[k] {
                RawOp::Delete(n) => d += n as usize,
                RawOp::Insert(n) => ins += n as usize,
                RawOp::Equal(_) => unreachable!(),
            }
            k += 1;
        }
        if d > 0 {
            let lspan = chunk_span(&lh[li..li + d], left);
            if ins > 0 {
                let rspan = chunk_span(&rh[ri..ri + ins], right);
                refine_region(left, lspan, right, rspan, full_refine, deadline, b);
            } else {
                emit_leftover(left, lspan, full_refine, b);
            }
        } else if ins > 0 {
            let rspan = chunk_span(&rh[ri..ri + ins], right);
            emit_rightover(right, rspan, full_refine, b);
        }
        li += d;
        ri += ins;
    }
}

struct ChunkHash {
    hash: u64,
    offset: usize,
    len: usize,
}

fn chunk_hashes(data: &[u8], chunk: usize) -> Vec<ChunkHash> {
    data.chunks(chunk.max(1))
        .enumerate()
        .map(|(i, c)| ChunkHash {
            hash: fnv1a(c),
            offset: i * chunk,
            len: c.len(),
        })
        .collect()
}

fn chunk_slice<'a>(data: &'a [u8], c: &ChunkHash) -> &'a [u8] {
    &data[c.offset..c.offset + c.len]
}

/// chunk 序列覆盖的字节区间
fn chunk_span(chunks: &[ChunkHash], data: &[u8]) -> (usize, usize) {
    if chunks.is_empty() {
        return (0, 0);
    }
    let start = chunks[0].offset;
    let last = &chunks[chunks.len() - 1];
    let end = (last.offset + last.len).min(data.len());
    (start, end)
}

/// 等位配对区精对齐：min 长度逐字节 -> same/changed 交替；长出尾部 inserted/deleted
fn refine_pair(lb: &[u8], rb: &[u8], _full: bool, b: &mut BlockBuilder) {
    let n = lb.len().min(rb.len());
    let mut i = 0usize;
    while i < n {
        let same = lb[i] == rb[i];
        let run_start = i;
        while i < n && (lb[i] == rb[i]) == same {
            i += 1;
        }
        let len = (i - run_start) as u64;
        b.push(
            if same {
                HexBlockKind::Same
            } else {
                HexBlockKind::Changed
            },
            len,
            len,
        );
    }
    if lb.len() > n {
        b.push(HexBlockKind::Deleted, (lb.len() - n) as u64, 0);
    }
    if rb.len() > n {
        b.push(HexBlockKind::Inserted, 0, (rb.len() - n) as u64);
    }
}

/// 非同区域（删除+插入区间对）精对齐：
/// - Full 且区域 ≤ FULL_REGION_MAX：字节级 Myers（插入/删除感知）；
/// - 其余（Fast / 超大区域）：等位配对 + 尾部 inserted/deleted（refine_pair 语义）。
fn refine_region(
    left: &[u8],
    lspan: (usize, usize),
    right: &[u8],
    rspan: (usize, usize),
    full: bool,
    deadline: Instant,
    b: &mut BlockBuilder,
) {
    let lb = &left[lspan.0..lspan.1];
    let rb = &right[rspan.0..rspan.1];
    let use_myers = full && lb.len() <= FULL_REGION_MAX && rb.len() <= FULL_REGION_MAX;
    if !use_myers {
        refine_pair(lb, rb, full, b);
        return;
    }
    let (ops, _degraded) = diff_seq_deadline(lb, rb, Some(deadline));
    let mut li = 0usize;
    let mut ri = 0usize;
    let mut k = 0usize;
    while k < ops.len() {
        match ops[k] {
            RawOp::Equal(n) => {
                b.push(HexBlockKind::Same, n as u64, n as u64);
                li += n as usize;
                ri += n as usize;
                k += 1;
            }
            RawOp::Delete(d) => {
                let ins = match ops.get(k + 1) {
                    Some(RawOp::Insert(n)) => *n,
                    _ => 0,
                };
                // 相邻 Delete+Insert：min 部分为字节变化，余量 inserted/deleted
                let paired = d.min(ins);
                if paired > 0 {
                    b.push(HexBlockKind::Changed, paired as u64, paired as u64);
                }
                if d > paired {
                    b.push(HexBlockKind::Deleted, (d - paired) as u64, 0);
                }
                if ins > paired {
                    b.push(HexBlockKind::Inserted, 0, (ins - paired) as u64);
                }
                li += d as usize;
                ri += ins as usize;
                if ins > 0 {
                    k += 2;
                } else {
                    k += 1;
                }
            }
            RawOp::Insert(n) => {
                b.push(HexBlockKind::Inserted, 0, n as u64);
                ri += n as usize;
                k += 1;
            }
        }
    }
    let _ = (li, ri);
}

/// 孤立删除区域（无右侧伴）：Full 时字节 Myers 无对齐意义，直接 Deleted 块；
/// 与后续 Inserted 区域的合并交给 builder（不合并：kind 不同）。
fn emit_leftover(left: &[u8], lspan: (usize, usize), full: bool, b: &mut BlockBuilder) {
    let lb = &left[lspan.0..lspan.1];
    if lb.is_empty() {
        return;
    }
    // 独立删除区没有配对伴 -> 纯 Deleted（refine_region 需要 rspan）
    let _ = full;
    b.push(HexBlockKind::Deleted, lb.len() as u64, 0);
}

/// 孤立插入区域：纯 Inserted 块
fn emit_rightover(right: &[u8], rspan: (usize, usize), full: bool, b: &mut BlockBuilder) {
    let rb = &right[rspan.0..rspan.1];
    if rb.is_empty() {
        return;
    }
    let _ = full;
    b.push(HexBlockKind::Inserted, 0, rb.len() as u64);
}

// ---------------------------------------------------------------------------
// 视图数据：每字节状态图 + ASCII 侧栏行渲染（hex/bytes 分页取数的数据源）
// ---------------------------------------------------------------------------

/// 两侧每字节状态图（下标 = 字节偏移；O(字节数)，open 时一次求出供视窗查询）
pub fn status_maps(model: &HexDiffModel) -> (Vec<HexByteStatus>, Vec<HexByteStatus>) {
    let mut l = vec![HexByteStatus::Same; model.left_len as usize];
    let mut r = vec![HexByteStatus::Same; model.right_len as usize];
    for blk in &model.blocks {
        match blk.kind {
            HexBlockKind::Same => {}
            HexBlockKind::Changed => {
                let (Some(lr), Some(rr)) = (blk.left, blk.right) else {
                    continue;
                };
                for i in 0..lr.len as usize {
                    l[lr.offset as usize + i] = HexByteStatus::Changed;
                }
                for i in 0..rr.len as usize {
                    r[rr.offset as usize + i] = HexByteStatus::Changed;
                }
            }
            HexBlockKind::Deleted => {
                let Some(lr) = blk.left else { continue };
                for i in 0..lr.len as usize {
                    l[lr.offset as usize + i] = HexByteStatus::Deleted;
                }
            }
            HexBlockKind::Inserted => {
                let Some(rr) = blk.right else { continue };
                for i in 0..rr.len as usize {
                    r[rr.offset as usize + i] = HexByteStatus::Inserted;
                }
            }
        }
    }
    (l, r)
}

/// ASCII 侧栏行（HEX_LINE_BYTES 字节/行；hex 小写两位空格分隔；非可打印 ASCII -> '.'）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexLine {
    /// 行首字节地址
    pub addr: u64,
    /// 十六进制列（实际字节数；末行短于 16 字节不填充）
    pub hex: String,
    /// ASCII 列（与 hex 字节数一致）
    pub ascii: String,
}

/// 渲染字节流的十六进制行（[start_row * 16, start_row * 16 + rows * 16) 视窗）
pub fn render_hex_lines(data: &[u8], start_row: u64, rows: usize) -> Vec<HexLine> {
    let mut out = Vec::new();
    let base = (start_row as usize).saturating_mul(HEX_LINE_BYTES);
    for r in 0..rows {
        let off = base + r * HEX_LINE_BYTES;
        if off >= data.len() {
            break;
        }
        let end = (off + HEX_LINE_BYTES).min(data.len());
        let slice = &data[off..end];
        let mut hex = String::with_capacity(slice.len() * 3);
        let mut ascii = String::with_capacity(slice.len());
        for (i, &byte) in slice.iter().enumerate() {
            if i > 0 {
                hex.push(' ');
            }
            hex.push_str(&format!("{byte:02x}"));
            ascii.push(if (0x20..=0x7e).contains(&byte) {
                byte as char
            } else {
                '.'
            });
        }
        out.push(HexLine {
            addr: off as u64,
            hex,
            ascii,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks_of(m: &HexDiffModel) -> Vec<(HexBlockKind, u64, u64, u64, u64)> {
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

    #[test]
    fn full_refine_degrades_on_expired_deadline() {
        // 尺寸上限挡不住无公共子序列数据的 O(ND)：以时间预算兜底。已过期
        // deadline 下块哈希与字节级 Myers 均立即降级为全删全插，产出粗粒度
        // Changed（无 Same），但仍覆盖全部字节（合法脚本，不冻结引擎）。
        let mut left = vec![0u8; 8192];
        left.extend_from_slice(&[1u8; 4096]);
        let mut right = vec![0u8; 8192];
        right.extend_from_slice(&[2u8; 4096]);
        let mut b = BlockBuilder::new();
        // 前 8192 字节相同：最优对齐会给出 Same；过期 deadline 强制降级
        diff_chunked(&left, &right, 4096, true, Instant::now(), &mut b);
        let lsum: u64 = b.blocks.iter().filter_map(|x| x.left.map(|r| r.len)).sum();
        let rsum: u64 = b.blocks.iter().filter_map(|x| x.right.map(|r| r.len)).sum();
        assert_eq!(lsum, left.len() as u64, "降级脚本仍须覆盖全部左字节");
        assert_eq!(rsum, right.len() as u64, "降级脚本仍须覆盖全部右字节");
        assert!(
            !b.blocks.iter().any(|x| x.kind == HexBlockKind::Same),
            "过期 deadline 应降级为粗粒度，不应出现 Same（最优对齐才会）"
        );
    }

    #[test]
    fn identical_bytes_single_same_block() {
        let data: Vec<u8> = (0..100u8).collect();
        let m = diff_bytes(&data, &data, &HexOptions::default());
        assert_eq!(blocks_of(&m).len(), 1);
        assert_eq!(m.blocks[0].kind, HexBlockKind::Same);
        assert_eq!(m.summary.same_bytes, 100);
        assert_eq!(
            (
                m.summary.changed_bytes,
                m.summary.inserted_bytes,
                m.summary.deleted_bytes
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn bytewise_mode_marks_changed_runs() {
        let l = b"AAAABBBBCCCC".to_vec();
        let r = b"AAAAXXBBCCCC".to_vec(); // 下标 4..6 变化
        let opts = HexOptions {
            align_mode: HexAlignMode::None,
            chunk_size: DEFAULT_CHUNK_SIZE,
        };
        let m = diff_bytes(&l, &r, &opts);
        assert_eq!(
            blocks_of(&m),
            vec![
                (HexBlockKind::Same, 0, 4, 0, 4),
                (HexBlockKind::Changed, 4, 2, 4, 2),
                (HexBlockKind::Same, 6, 6, 6, 6)
            ]
        );
    }

    #[test]
    fn bytewise_tail_is_insert_or_delete() {
        let opts = HexOptions {
            align_mode: HexAlignMode::None,
            chunk_size: DEFAULT_CHUNK_SIZE,
        };
        let m = diff_bytes(b"abc", b"abcdef", &opts);
        assert_eq!(
            blocks_of(&m),
            vec![
                (HexBlockKind::Same, 0, 3, 0, 3),
                (HexBlockKind::Inserted, u64::MAX, u64::MAX, 3, 3)
            ]
        );
        let m = diff_bytes(b"abcdef", b"abc", &opts);
        assert_eq!(
            blocks_of(&m),
            vec![
                (HexBlockKind::Same, 0, 3, 0, 3),
                (HexBlockKind::Deleted, 3, 3, u64::MAX, u64::MAX)
            ]
        );
        let m = diff_bytes(b"", b"xyz", &opts);
        assert_eq!(
            blocks_of(&m),
            vec![(HexBlockKind::Inserted, u64::MAX, u64::MAX, 0, 3)]
        );
        let m = diff_bytes(b"", b"", &opts);
        assert!(m.blocks.is_empty());
    }

    #[test]
    fn full_mode_aligns_inserted_middle_run() {
        // 中段插入：Full 应感知为 Same + Inserted + Same（插入/删除感知）
        let l = b"hello world".to_vec();
        let r = b"hello brave world".to_vec();
        let m = diff_bytes(
            &l,
            &r,
            &HexOptions {
                chunk_size: 4,
                align_mode: HexAlignMode::Full,
            },
        );
        let inserted = m
            .blocks
            .iter()
            .filter(|b| b.kind == HexBlockKind::Inserted)
            .count();
        assert_eq!(inserted, 1, "Full 模式应定位单插入区: {m:?}");
        assert_eq!(m.summary.inserted_bytes, 6, "brave=6 字节: {m:?}");
        assert_eq!(m.summary.changed_bytes, 0);
        // 每字节状态：左全同；右 6 字节 inserted
        let (ls, rs) = status_maps(&m);
        assert!(ls.iter().all(|&s| s == HexByteStatus::Same));
        assert_eq!(
            rs.iter().filter(|&&s| s == HexByteStatus::Inserted).count(),
            6
        );
    }

    #[test]
    fn full_mode_aligns_deleted_middle_run() {
        let l = b"hello brave world".to_vec();
        let r = b"hello world".to_vec();
        let m = diff_bytes(
            &l,
            &r,
            &HexOptions {
                chunk_size: 4,
                align_mode: HexAlignMode::Full,
            },
        );
        assert_eq!(m.summary.deleted_bytes, 6, "{m:?}");
        assert_eq!(m.summary.inserted_bytes, 0);
        assert_eq!(m.summary.changed_bytes, 0);
    }

    #[test]
    fn fast_mode_changed_pair_and_byte_refine() {
        let l = b"AAAABBBB".to_vec();
        let r = b"AAAAXXYY".to_vec(); // 后块部分变化
        let m = diff_bytes(
            &l,
            &r,
            &HexOptions {
                chunk_size: 4,
                align_mode: HexAlignMode::Fast,
            },
        );
        // 块哈希不同 -> Changed；块内逐字节精分：BB→XX（changed 2）+ BB→YY（changed 2）
        assert_eq!(m.summary.changed_bytes, 4, "Fast 等位精对齐: {m:?}");
        assert_eq!(
            blocks_of(&m),
            vec![
                (HexBlockKind::Same, 0, 4, 0, 4),
                (HexBlockKind::Changed, 4, 4, 4, 4)
            ]
        );
    }

    #[test]
    fn chunked_tail_insert_and_delete() {
        let opts = HexOptions {
            chunk_size: 4,
            align_mode: HexAlignMode::Full,
        };
        let m = diff_bytes(b"aaaa", b"aaaaZZZZ", &opts);
        assert_eq!(m.summary.inserted_bytes, 4, "{m:?}");
        let m = diff_bytes(b"aaaaZZZZ", b"aaaa", &opts);
        assert_eq!(m.summary.deleted_bytes, 4);
        let m = diff_bytes(b"", b"", &opts);
        assert!(m.blocks.is_empty() && m.summary == HexDiffSummary::default());
    }

    #[test]
    fn blocks_carry_monotonic_ranges() {
        let l = b"0123456789ABCDEF".to_vec();
        let r = b"0123XXXX89ABCDEF".to_vec();
        let m = diff_bytes(
            &l,
            &r,
            &HexOptions {
                chunk_size: 8,
                align_mode: HexAlignMode::Full,
            },
        );
        let (ls, rs) = status_maps(&m);
        assert_eq!(ls.len(), 16);
        assert_eq!(rs.len(), 16);
        for i in 0..16 {
            let expect = if (4..8).contains(&i) {
                HexByteStatus::Changed
            } else {
                HexByteStatus::Same
            };
            assert_eq!(ls[i], expect, "左字节 {i}");
            assert_eq!(rs[i], expect);
        }
        // 块区间单调且无缝覆盖
        let mut lo = 0u64;
        let mut ro = 0u64;
        for b in &m.blocks {
            if let Some(lr) = b.left {
                assert_eq!(lr.offset, lo);
                lo += lr.len;
            }
            if let Some(rr) = b.right {
                assert_eq!(rr.offset, ro);
                ro += rr.len;
            }
        }
        assert_eq!(lo, m.left_len);
        assert_eq!(ro, m.right_len);
    }

    #[test]
    fn render_hex_lines_layout_and_ascii() {
        let data: Vec<u8> = (0u8..=0x7f).collect();
        let lines = render_hex_lines(&data, 0, 2);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].addr, 0);
        assert_eq!(lines[0].hex.len(), 16 * 3 - 1, "47 字符 hex 列");
        assert!(lines[0].hex.starts_with("00 01 02"));
        assert_eq!(lines[0].ascii.len(), 16);
        assert!(lines[0].ascii.starts_with("..."), "控制字符 -> '.'");
        // 第 4 行（0x40..0x4f）含可打印 ASCII
        let rows = render_hex_lines(&data, 4, 1);
        assert!(
            rows[0].ascii.contains('A'),
            "可打印 ASCII 原样: {}",
            rows[0].ascii
        );
        // 末行截短：10 字节文件
        let lines = render_hex_lines(&[0u8; 10], 0, 4);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].hex, "00 00 00 00 00 00 00 00 00 00");
        // 越界窗口 -> 空
        assert!(render_hex_lines(&[0u8; 10], 1, 4).is_empty());
        // 非可打印/高字节
        let lines = render_hex_lines(&[0x00, 0x1f, 0x20, 0x7e, 0x7f, 0x80, 0xff], 0, 1);
        assert_eq!(lines[0].hex, "00 1f 20 7e 7f 80 ff");
        assert_eq!(
            lines[0].ascii, ".. ~...",
            "ascii 侧栏非可打印字节以 '.' 呈现: {}",
            lines[0].ascii
        );
    }

    #[test]
    fn status_maps_full_coverage() {
        use HexByteStatus::{Changed, Inserted, Same};
        // same + changed + inserted + deleted 混合
        let l = b"ABCDEF".to_vec();
        let r = b"ABXYEFgh".to_vec(); // CD→XY changed，g h inserted
        let m = diff_bytes(
            &l,
            &r,
            &HexOptions {
                align_mode: HexAlignMode::Full,
                chunk_size: 3,
            },
        );
        let (ls, rs) = status_maps(&m);
        assert_eq!(ls, vec![Same, Same, Changed, Changed, Same, Same]);
        assert_eq!(
            rs,
            vec![Same, Same, Changed, Changed, Same, Same, Inserted, Inserted]
        );
    }
}
