//! Patience Diff（Bram Cohen，FR-TXT-011；docs/04 P1 交付物 ①）。
//!
//! 算法：只用**两侧均唯一出现**的行做锚点，对锚点做最长递增子序列（LIS，
//! patience sorting，O(K log K)）得到可靠匹配对，再对各子问题递归；
//! 无锚点的子问题回退到 Myers O(ND)（与 git 的 xpatience 回退策略一致）。
//!
//! 输出与 [`crate::myers`] 相同的 [`RawOp`] 序列（经 coalesce）。

use std::collections::HashMap;
use std::hash::Hash;

use crate::myers::{self, RawOp};

enum Task {
    Range { a0: u32, a1: u32, b0: u32, b1: u32 },
    Eq(u32),
}

/// Patience diff：返回覆盖整个 a、b 的编辑脚本（相邻同类操作已合并）。
pub fn diff_seq<T>(a: &[T], b: &[T]) -> Vec<RawOp>
where
    T: PartialEq + Eq + Hash,
{
    let mut out: Vec<RawOp> = Vec::new();
    let mut stack = vec![Task::Range {
        a0: 0,
        a1: a.len() as u32,
        b0: 0,
        b1: b.len() as u32,
    }];

    while let Some(task) = stack.pop() {
        match task {
            Task::Eq(len) => out.push(RawOp::Equal(len)),
            Task::Range {
                mut a0,
                a1,
                mut b0,
                b1,
            } => {
                // 公共前缀/后缀裁剪（与 myers 同策略；后缀延后输出）
                let mut pre = 0u32;
                while a0 + pre < a1
                    && b0 + pre < b1
                    && a[(a0 + pre) as usize] == b[(b0 + pre) as usize]
                {
                    pre += 1;
                }
                if pre > 0 {
                    out.push(RawOp::Equal(pre));
                    a0 += pre;
                    b0 += pre;
                }
                let mut suf = 0u32;
                while a1 > a0 + suf
                    && b1 > b0 + suf
                    && a[(a1 - 1 - suf) as usize] == b[(b1 - 1 - suf) as usize]
                {
                    suf += 1;
                }
                let a1 = a1 - suf;
                let b1 = b1 - suf;
                let n = (a1 - a0) as usize;
                let m = (b1 - b0) as usize;

                // LIFO：先压 suffix，再压中段任务 -> 弹出顺序 中段 -> suffix
                if suf > 0 {
                    stack.push(Task::Eq(suf));
                }

                if n == 0 && m == 0 {
                    // 全部由前缀/后缀吸收
                } else if n == 0 {
                    out.push(RawOp::Insert(m as u32));
                } else if m == 0 {
                    out.push(RawOp::Delete(n as u32));
                } else if let Some(pairs) =
                    unique_anchor_lis(&a[a0 as usize..a1 as usize], &b[b0 as usize..b1 as usize])
                {
                    // 锚点对 (i, j)：严格递增；锚点之间递归，锚点本身输出 Equal(1)
                    // 按输出顺序构建任务，再倒序压栈（LIFO 弹出顺序 == 输出顺序）
                    let mut tasks: Vec<Task> = Vec::with_capacity(pairs.len() * 2 + 2);
                    let (mut prev_a, mut prev_b) = (a0, b0);
                    for (i, j) in &pairs {
                        let ai = a0 + i;
                        let bj = b0 + j;
                        tasks.push(Task::Range {
                            a0: prev_a,
                            a1: ai,
                            b0: prev_b,
                            b1: bj,
                        });
                        tasks.push(Task::Eq(1));
                        prev_a = ai + 1;
                        prev_b = bj + 1;
                    }
                    tasks.push(Task::Range {
                        a0: prev_a,
                        a1,
                        b0: prev_b,
                        b1,
                    });
                    for t in tasks.into_iter().rev() {
                        stack.push(t);
                    }
                } else {
                    // 无唯一锚点：回退 Myers（该子问题一次性最优）
                    let ops =
                        myers::diff_seq(&a[a0 as usize..a1 as usize], &b[b0 as usize..b1 as usize]);
                    out.extend(ops);
                }
            }
        }
    }
    myers::coalesce_ops(out)
}

/// 找出两侧均唯一出现的行并做 LIS 匹配。
/// 返回 (a 内下标, b 内下标) 对（各自严格递增）；无锚点（含空序列）时返回 None。
pub(crate) fn unique_anchor_lis<T: PartialEq + Eq + Hash>(
    a: &[T],
    b: &[T],
) -> Option<Vec<(u32, u32)>> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let mut count_a: HashMap<&T, u32> = HashMap::with_capacity(a.len());
    for line in a {
        *count_a.entry(line).or_insert(0) += 1;
    }
    let mut count_b: HashMap<&T, u32> = HashMap::with_capacity(b.len());
    for line in b {
        *count_b.entry(line).or_insert(0) += 1;
    }
    // b 中唯一行 -> 其下标
    let mut pos_b: HashMap<&T, u32> = HashMap::with_capacity(b.len());
    for (idx, line) in b.iter().enumerate() {
        if count_b[line] == 1 {
            pos_b.insert(line, idx as u32);
        }
    }
    // a 中唯一且在 b 中唯一的候选（按 a 序）
    let mut candidates: Vec<(u32, u32)> = Vec::new(); // (a_idx, b_idx)
    for (idx, line) in a.iter().enumerate() {
        if count_a[line] == 1 {
            if let Some(&j) = pos_b.get(line) {
                candidates.push((idx as u32, j));
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }
    // patience sorting 求 (b_idx) 序列的严格递增 LIS（O(K log K)，带回溯指针）
    // piles[k] = (该堆顶 b_idx, 候选下标)；back[i] = 前驱候选下标
    let mut pile_top: Vec<u32> = Vec::with_capacity(candidates.len());
    let mut pile_idx: Vec<usize> = Vec::with_capacity(candidates.len());
    let mut back: Vec<Option<usize>> = vec![None; candidates.len()];
    for (ci, (_, bj)) in candidates.iter().enumerate() {
        // 二分找第一个 >= bj 的堆顶（严格递增 LIS）
        let pos = pile_top.partition_point(|&top| top < *bj);
        if pos == pile_top.len() {
            pile_top.push(*bj);
            pile_idx.push(ci);
        } else {
            pile_top[pos] = *bj;
            pile_idx[pos] = ci;
        }
        back[ci] = if pos > 0 {
            Some(pile_idx[pos - 1])
        } else {
            None
        };
    }
    // 回溯 LIS
    let mut result: Vec<(u32, u32)> = Vec::with_capacity(pile_idx.len());
    let mut cur = Some(*pile_idx.last().expect("candidates 非空"));
    while let Some(ci) = cur {
        result.push((candidates[ci].0, candidates[ci].1));
        cur = back[ci];
    }
    result.reverse();
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::myers::RawOp;

    fn replay_ok<T: Clone + PartialEq + std::fmt::Debug>(ops: &[RawOp], a: &[T], b: &[T]) {
        let mut ia = 0usize;
        let mut ib = 0usize;
        for op in ops {
            match op {
                RawOp::Equal(k) => {
                    assert_eq!(
                        &a[ia..ia + *k as usize],
                        &b[ib..ib + *k as usize],
                        "Equal 段必须真等"
                    );
                    ia += *k as usize;
                    ib += *k as usize;
                }
                RawOp::Delete(k) => ia += *k as usize,
                RawOp::Insert(k) => ib += *k as usize,
            }
        }
        assert_eq!(ia, a.len(), "脚本未覆盖全部 a");
        assert_eq!(ib, b.len(), "脚本未覆盖全部 b");
    }

    #[test]
    fn equal_and_empty() {
        let v = vec![1, 2, 3];
        assert_eq!(diff_seq(&v, &v), vec![RawOp::Equal(3)]);
        assert_eq!(diff_seq::<u8>(&[], &[]), vec![]);
        assert_eq!(diff_seq(&v, &[]), vec![RawOp::Delete(3)]);
        assert_eq!(diff_seq(&[], &v), vec![RawOp::Insert(3)]);
    }

    #[test]
    fn classic_patience_case() {
        // Myers 会把独特的 unique3 行作为"更改"的一部分；
        // patience 以唯一行（change1/2、unique1..3）为锚，保留独立块
        let a = vec![
            "void f() {",
            "    unique1();",
            "    unique2();",
            "    common1();",
            "}",
        ];
        let b = vec!["void g() {", "    common1();", "    unique3();", "}"];
        let ops = diff_seq(&a, &b);
        replay_ok(&ops, &a, &b);
        // common1 与 "}" 为两侧唯一共同行 → 必为 Equal 锚点
        let equal_total: u32 = ops
            .iter()
            .filter_map(|o| {
                if let RawOp::Equal(k) = o {
                    Some(*k)
                } else {
                    None
                }
            })
            .sum();
        assert_eq!(equal_total, 2, "common1 与右花括号各匹配一行");
    }

    #[test]
    fn moved_block() {
        let a: Vec<u32> = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let b: Vec<u32> = vec![1, 6, 7, 2, 3, 4, 5, 8];
        let ops = diff_seq(&a, &b);
        replay_ok(&ops, &a, &b);
    }

    #[test]
    fn no_unique_anchor_falls_back_to_myers() {
        // 全部行重复出现（无唯一锚点）→ 回退 Myers，结果最优
        let a = vec![1, 1, 2, 2, 1, 1];
        let b = vec![2, 2, 1, 1, 1, 1, 1];
        let ops = diff_seq(&a, &b);
        replay_ok(&ops, &a, &b);
        let del: u32 = ops
            .iter()
            .filter_map(|o| {
                if let RawOp::Delete(k) = o {
                    Some(*k)
                } else {
                    None
                }
            })
            .sum();
        let ins: u32 = ops
            .iter()
            .filter_map(|o| {
                if let RawOp::Insert(k) = o {
                    Some(*k)
                } else {
                    None
                }
            })
            .sum();
        // 最优编辑距离：m + n - 2*LCS；LCS([1,1,2,2,1,1],[2,2,1,1,1,1,1]) = 4
        assert_eq!(del + ins, 6 + 7 - 2 * 4);
    }

    #[test]
    fn random_pairs_replay_and_lis_property() {
        let mut state = 0x9E3779B97F4A7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..300 {
            let n = (next() % 20) as usize;
            let m = (next() % 20) as usize;
            let a: Vec<u8> = (0..n).map(|_| (next() % 5) as u8).collect();
            let b: Vec<u8> = (0..m).map(|_| (next() % 5) as u8).collect();
            let ops = diff_seq(&a, &b);
            replay_ok(&ops, &a, &b);
        }
    }

    #[test]
    fn nested_recursion_case() {
        // 锚点间仍有歧义 → 递归 + 回退
        let a = vec!["A", "x1", "B", "x2", "C", "x3", "D"];
        let b = vec!["A", "y1", "B", "C", "y2", "x3", "D"];
        let ops = diff_seq(&a, &b);
        replay_ok(&ops, &a, &b);
    }
}
