//! Histogram 对齐算法（git 同源实现，经 imara-diff crate 接入；
//! Apache-2.0。docs/04 P1 交付物 ①，FR-TXT-011）。
//!
//! 接线方式：先把比较键映射为共享 u32 id（自己的 HashMap，保证与
//! 空白/大小写规范化语义一致），再以 `Diff::compute_with` 在 Token 序列上
//! 运行 `Algorithm::Histogram`（完全绕开 imara 的行分割/interner）。
//! Hunks 转换回 [`RawOp`] 序列。
//!
//! 注意：Histogram 面向"人类可读"而非最小编辑序列（与 git 行为一致），
//! 对高重复 token 有内建 Myers 回退以避免二次方行为。

use std::collections::HashMap;

use imara_diff::{Algorithm, Diff, Token};

use crate::myers::{coalesce_ops, RawOp};

/// Histogram diff：键序列 -> 编辑脚本（相邻同类操作已合并）。
pub fn diff_keys(a: &[&str], b: &[&str]) -> Vec<RawOp> {
    // 共享 id 映射（两侧一致 -> 相等性语义由调用方的键保证）
    let mut ids: HashMap<&str, u32> = HashMap::with_capacity(a.len() + b.len());
    let mut next_id = 0u32;
    fn intern<'m>(s: &'m str, ids: &mut HashMap<&'m str, u32>, next_id: &mut u32) -> u32 {
        *ids.entry(s).or_insert_with(|| {
            let id = *next_id;
            *next_id += 1;
            id
        })
    }
    let mut before = Vec::with_capacity(a.len());
    for s in a {
        before.push(Token(intern(s, &mut ids, &mut next_id)));
    }
    let mut after = Vec::with_capacity(b.len());
    for s in b {
        after.push(Token(intern(s, &mut ids, &mut next_id)));
    }
    let num_tokens = next_id;

    let mut diff = Diff::default();
    diff.compute_with(Algorithm::Histogram, &before, &after, num_tokens);

    // hunks（单调递增的 modify 区间）-> RawOp 序列
    let mut ops: Vec<RawOp> = Vec::new();
    let (mut cur_a, mut cur_b) = (0u32, 0u32);
    for hunk in diff.hunks() {
        let gap = hunk.before.start - cur_a;
        debug_assert_eq!(hunk.after.start, cur_b + gap, "等值间隙两侧必须一致");
        if gap > 0 {
            ops.push(RawOp::Equal(gap));
        }
        let d = hunk.before.len() as u32;
        let i = hunk.after.len() as u32;
        if d > 0 {
            ops.push(RawOp::Delete(d));
        }
        if i > 0 {
            ops.push(RawOp::Insert(i));
        }
        cur_a = hunk.before.end;
        cur_b = hunk.after.end;
    }
    let rest_a = a.len() as u32 - cur_a;
    let rest_b = b.len() as u32 - cur_b;
    debug_assert_eq!(rest_a, rest_b, "尾部等值段两侧必须一致");
    if rest_a > 0 {
        ops.push(RawOp::Equal(rest_a));
    }
    coalesce_ops(ops)
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
                    assert_eq!(&a[ia..ia + *k as usize], &b[ib..ib + *k as usize], "Equal 段必须真等");
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

    fn strs<'a>(v: &[&'a str]) -> Vec<&'a str> {
        v.to_vec()
    }

    #[test]
    fn equal_and_empty() {
        let v = strs(&["a", "b", "c"]);
        assert_eq!(diff_keys(&v, &v), vec![RawOp::Equal(3)]);
        assert!(diff_keys(&[], &[]).is_empty());
        assert_eq!(diff_keys(&v, &[]), vec![RawOp::Delete(3)]);
        assert_eq!(diff_keys(&[], &v), vec![RawOp::Insert(3)]);
    }

    #[test]
    fn single_change() {
        let a = strs(&["one", "two", "three"]);
        let b = strs(&["one", "TWO", "three"]);
        let ops = diff_keys(&a, &b);
        replay_ok(&ops, &a, &b);
        assert_eq!(ops, vec![RawOp::Equal(1), RawOp::Delete(1), RawOp::Insert(1), RawOp::Equal(1)]);
    }

    #[test]
    fn moved_block() {
        let a = strs(&["1", "2", "3", "4", "5", "6", "7", "8"]);
        let b = strs(&["1", "6", "7", "2", "3", "4", "5", "8"]);
        let ops = diff_keys(&a, &b);
        replay_ok(&ops, &a, &b);
    }

    #[test]
    fn highly_repeated_tokens_terminate() {
        // 高重复 token（直方图回退路径）：必须终止且重放正确
        let a: Vec<&str> = (0..2000).map(|i| if i % 3 == 0 { "x" } else { "y" }).collect();
        let b: Vec<&str> = (0..2100).map(|i| if i % 5 == 0 { "x" } else { "y" }).collect();
        let ops = diff_keys(&a, &b);
        replay_ok(&ops, &a, &b);
    }

    #[test]
    fn random_pairs_replay() {
        let mut state = 0xA24BAED4963EE407u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..200 {
            let n = (next() % 24) as usize;
            let m = (next() % 24) as usize;
            let a: Vec<String> = (0..n).map(|_| format!("l{}", next() % 6)).collect();
            let b: Vec<String> = (0..m).map(|_| format!("l{}", next() % 6)).collect();
            let ar: Vec<&str> = a.iter().map(String::as_str).collect();
            let br: Vec<&str> = b.iter().map(String::as_str).collect();
            let ops = diff_keys(&ar, &br);
            replay_ok(&ops, &ar, &br);
        }
    }
}
