//! Myers O(ND) 行级 diff（原创实现，基于 Myers 1986 论文《An O(ND) Difference
//! Algorithm and Its Variations》第 4b 节的线性空间分治描述；未参考任何 GPL 实现）。
//!
//! - 时间 O((N+M)·D)，空间 O(N+M)（线性空间中点分治，显式工作栈避免深递归）；
//! - 输出为连续 [`RawOp`] 序列（Equal/Delete/Insert），经 [`coalesce_ops`] 合并
//!   后由 sections 模块构建 section。

/// 原始编辑操作（长度制，位置由消费方游标推进）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawOp {
    /// 两侧相等的连续行数
    Equal(u32),
    /// 左侧删除的连续行数
    Delete(u32),
    /// 右侧插入的连续行数
    Insert(u32),
}

const NONE: isize = isize::MIN;

/// 分治工作项：Range 为待对齐子问题（两侧半开区间）；Eq/Ins/Del 为直接输出
enum Task {
    Range { a0: u32, a1: u32, b0: u32, b1: u32 },
    Eq(u32),
    Ins(u32),
    Del(u32),
}

/// 计算编辑脚本（覆盖整个 a 与 b；输出顺序即应用顺序）。
/// 相邻同类操作会被合并（coalesce）。
pub fn diff_seq<T: PartialEq>(a: &[T], b: &[T]) -> Vec<RawOp> {
    diff_seq_deadline(a, b, None).0
}

/// 带 deadline 的编辑脚本：超时后子问题降级为"全删全插"（合法但非最优），
/// `degraded = true` 供调用方按超时语义处理（FR-BIG / docs/03 §6.2 行内策略）。
/// `deadline = None` 时行为与 [`diff_seq`] 完全一致（无额外开销）。
pub fn diff_seq_deadline<T: PartialEq>(
    a: &[T],
    b: &[T],
    deadline: Option<std::time::Instant>,
) -> (Vec<RawOp>, bool) {
    let mut degraded = false;
    let out = diff_seq_inner(a, b, deadline, &mut degraded);
    (out, degraded)
}

fn diff_seq_inner<T: PartialEq>(
    a: &[T],
    b: &[T],
    deadline: Option<std::time::Instant>,
    degraded: &mut bool,
) -> Vec<RawOp> {
    let mut out: Vec<RawOp> = Vec::new();
    let mut stack: Vec<Task> = vec![Task::Range {
        a0: 0,
        a1: a.len() as u32,
        b0: 0,
        b1: b.len() as u32,
    }];

    while let Some(task) = stack.pop() {
        match task {
            Task::Eq(len) => out.push(RawOp::Equal(len)),
            Task::Ins(len) => out.push(RawOp::Insert(len)),
            Task::Del(len) => out.push(RawOp::Delete(len)),
            Task::Range {
                mut a0,
                a1,
                mut b0,
                b1,
            } => {
                // 超时抢占：剩余子问题降级为全删全插（脚本仍合法）
                if let Some(dl) = deadline {
                    if std::time::Instant::now() > dl {
                        let n = (a1 - a0) as usize;
                        let m = (b1 - b0) as usize;
                        if n > 0 {
                            out.push(RawOp::Delete(n as u32));
                        }
                        if m > 0 {
                            out.push(RawOp::Insert(m as u32));
                        }
                        *degraded = true;
                        continue;
                    }
                }
                // 公共前缀：直接输出
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
                // 公共后缀：压栈延后输出（必须位于中段之后）
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

                let mid: Option<Vec<Task>> = if n == 0 && m == 0 {
                    Some(Vec::new())
                } else if n == 0 {
                    Some(vec![Task::Ins(m as u32)])
                } else if m == 0 {
                    Some(vec![Task::Del(n as u32)])
                } else {
                    match middle_snake(&a[a0 as usize..a1 as usize], &b[b0 as usize..b1 as usize], deadline) {
                        Some((x, y))
                            // 防退化守卫：分割点必须两侧都有进展，否则回退全删全插
                            if !((x == 0 && y == 0) || (x == n && y == m)) =>
                        {
                            let (x, y) = (x as u32, y as u32);
                            Some(vec![
                                Task::Range { a0, a1: a0 + x, b0, b1: b0 + y },
                                Task::Range { a0: a0 + x, a1, b0: b0 + y, b1 },
                            ])
                        }
                        _ => {
                            // 仅当 deadline 确实已过才标记降级（理论退化兜底不算超时）
                            if deadline.is_some_and(|dl| std::time::Instant::now() > dl) {
                                *degraded = true;
                            }
                            Some(vec![Task::Del(n as u32), Task::Ins(m as u32)])
                        }
                    }
                };

                // LIFO：压栈顺序 suffix -> right -> left，弹出顺序 left -> right -> suffix
                if suf > 0 {
                    stack.push(Task::Eq(suf));
                }
                if let Some(mut tasks) = mid {
                    while let Some(t) = tasks.pop() {
                        stack.push(t);
                    }
                }
            }
        }
    }
    coalesce_ops(out)
}

/// 规范化：相邻 `Insert, Delete` 交换为 `Delete, Insert`（两侧操作互不相干，交换安全），
/// 再合并相邻同类操作。保证 Change section（先删后增）与 unified diff 惯例一致。
pub fn coalesce_ops(ops: Vec<RawOp>) -> Vec<RawOp> {
    // 1. 交换相邻 Insert/Delete
    let mut ops = ops;
    let mut i = 0;
    while i + 1 < ops.len() {
        if matches!(ops[i], RawOp::Insert(_)) && matches!(ops[i + 1], RawOp::Delete(_)) {
            ops.swap(i, i + 1);
            i += 1; // 交换后 i 位是 Delete，跳过它继续
        } else {
            i += 1;
        }
    }
    // 2. 合并相邻同类
    let mut out: Vec<RawOp> = Vec::with_capacity(ops.len());
    for op in ops {
        match (out.last_mut(), op) {
            (Some(RawOp::Equal(p)), RawOp::Equal(n)) => *p += n,
            (Some(RawOp::Delete(p)), RawOp::Delete(n)) => *p += n,
            (Some(RawOp::Insert(p)), RawOp::Insert(n)) => *p += n,
            _ => out.push(op),
        }
    }
    out
}

/// 线性空间中点分治：寻找"中蛇"（middle snake），
/// 返回蛇末端 (x, y)（0 起、半开）作为分割点。
/// `deadline` 超时返回 None（每 32 轮 d 迭代检查一次，摊销开销可忽略）。
fn middle_snake<T: PartialEq>(
    a: &[T],
    b: &[T],
    deadline: Option<std::time::Instant>,
) -> Option<(usize, usize)> {
    let n = a.len();
    let m = b.len();
    debug_assert!(n > 0 && m > 0, "调用方已排除空侧");
    let max_d = (n + m).div_ceil(2);
    let delta = n as isize - m as isize;
    let even_delta = delta % 2 == 0;
    // V 数组按绝对对角线 k（= x - y）索引，偏移 off 保证非负
    let off = (n + m) as isize;
    let size = 2 * (n + m) + 1;
    let mut vf = vec![NONE; size]; // forward：对角线 k 上可达的最大 x
    let mut vb = vec![NONE; size]; // backward：对角线 k 上可达的最小 x

    // backward d = 0：起点 (n, m) 沿对角线 delta 回走蛇
    {
        let (mut x, mut y) = (n as isize, m as isize);
        while x > 0 && y > 0 && a[(x - 1) as usize] == b[(y - 1) as usize] {
            x -= 1;
            y -= 1;
        }
        vb[(delta + off) as usize] = x;
    }

    for d in 0..=max_d {
        let di = d as isize;
        if let Some(dl) = deadline {
            if d % 32 == 0 && std::time::Instant::now() > dl {
                return None;
            }
        }

        // ---- forward d：对角线 |k| <= d 且 k ≡ d (mod 2)；
        //      绝对对角线有效域 [-m, n]（x ∈ [0,n], y ∈ [0,m] => k = x-y）----
        for k in -di..=di {
            if (k - di) % 2 != 0 {
                continue;
            }
            if k < -(m as isize) || k > n as isize {
                continue;
            }
            let ki = (k + off) as usize;
            let mut best = NONE;
            if k > -di {
                let v = vf[(k - 1 + off) as usize];
                if v != NONE {
                    best = best.max(v + 1);
                }
            }
            if k < di {
                let v = vf[(k + 1 + off) as usize];
                if v != NONE {
                    best = best.max(v);
                }
            }
            if d == 0 {
                best = 0;
            }
            if best == NONE {
                continue;
            }
            let mut x = best.min(n as isize);
            let mut y = x - k;
            if y < 0 || y > m as isize {
                continue;
            }
            while x < n as isize && y < m as isize && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            vf[ki] = x;

            // delta 为奇数：forward d 与 backward (d-1) 在同对角线相遇 -> 总代价 2d-1
            if !even_delta && (k - delta).abs() < di {
                let bv = vb[ki];
                if bv != NONE && x >= bv {
                    return Some((x as usize, (x - k) as usize));
                }
            }
        }

        // ---- backward d：对角线 delta-d..=delta+d 且 k ≡ delta + d (mod 2)，
        //      同样约束在绝对有效域 [-m, n] ----
        for k in (delta - di)..=(delta + di) {
            if (k - delta - di) % 2 != 0 {
                continue;
            }
            if k < -(m as isize) || k > n as isize {
                continue;
            }
            let ki = (k + off) as usize;
            // backward 取"最小可达 x"（越接近 (0,0) 越远），哨兵用 isize::MAX
            let mut best = isize::MAX;
            if k - delta < di {
                let v = vb[(k + 1 + off) as usize];
                if v != NONE && v > 0 {
                    best = best.min(v - 1); // 从对角线 k+1 横移（x-1）
                }
            }
            if k - delta > -di {
                let v = vb[(k - 1 + off) as usize];
                if v != NONE {
                    best = best.min(v); // 从对角线 k-1 纵移（x 不变）
                }
            }
            if d == 0 {
                best = n as isize;
            }
            if best == isize::MAX || best < 0 {
                continue;
            }
            let mut x = best;
            let mut y = x - k;
            if y < 0 || y > m as isize {
                continue;
            }
            while x > 0 && y > 0 && a[(x - 1) as usize] == b[(y - 1) as usize] {
                x -= 1;
                y -= 1;
            }
            vb[ki] = x;

            // delta 为偶数：backward d 与 forward d（本轮已展开）相遇 -> 总代价 2d
            if even_delta && k.abs() <= di {
                let fv = vf[ki];
                if fv != NONE && x <= fv {
                    return Some((x as usize, (x - k) as usize));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply<T: Clone>(ops: &[RawOp], a: &[T], b: &[T]) -> (Vec<T>, Vec<T>) {
        // 按脚本重放：Equal -> 两侧各前进 len；Delete -> 跳过 a；Insert -> 跳过 b
        let mut ia = 0usize;
        let mut ib = 0usize;
        for op in ops {
            let len = match op {
                RawOp::Equal(n) => *n as usize,
                RawOp::Delete(n) => {
                    ia += *n as usize;
                    0
                }
                RawOp::Insert(n) => {
                    ib += *n as usize;
                    0
                }
            };
            ia += len;
            ib += len;
        }
        (a[..ia].to_vec(), b[..ib].to_vec())
    }

    #[test]
    fn equal_sequences() {
        let v = vec![1, 2, 3, 4];
        let ops = diff_seq(&v, &v);
        assert_eq!(ops, vec![RawOp::Equal(4)]);
    }

    #[test]
    fn empty_sides() {
        let v = vec![1, 2];
        assert_eq!(diff_seq(&[], &v), vec![RawOp::Insert(2)]);
        assert_eq!(diff_seq(&v, &[]), vec![RawOp::Delete(2)]);
        assert!(diff_seq::<u8>(&[], &[]).is_empty());
    }

    #[test]
    fn disjoint_sequences() {
        let ops = diff_seq(&[1, 2, 3], &[4, 5]);
        assert_eq!(ops, vec![RawOp::Delete(3), RawOp::Insert(2)]);
    }

    #[test]
    fn moved_block() {
        let a = vec![1, 2, 3, 4, 5, 6];
        let b = vec![1, 4, 5, 2, 3, 6];
        let ops = diff_seq(&a, &b);
        // 脚本重放必须精确还原两侧（正确性）
        let (ra, rb) = apply(&ops, &a, &b);
        assert_eq!(ra, a);
        assert_eq!(rb, b);
    }

    #[test]
    fn random_pairs_match_dp_oracle() {
        // 与 O(NM) DP 最优编辑距离对拍：脚本重放正确 + 编辑距离与 DP 一致（最优性）
        let mut state = 0x2545F4914F6CDD1Du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..300 {
            let n = (next() % 12) as usize;
            let m = (next() % 12) as usize;
            let a: Vec<u8> = (0..n).map(|_| (next() % 4) as u8).collect();
            let b: Vec<u8> = (0..m).map(|_| (next() % 4) as u8).collect();
            let ops = diff_seq(&a, &b);
            // 重放正确性
            let mut ia = 0;
            let mut ib = 0;
            for op in &ops {
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
            // 最优性：Delete+Insert 行数 == DP 编辑距离
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
            let dp = edit_distance_dp(&a, &b);
            assert_eq!(
                del as usize + ins as usize,
                dp,
                "脚本非最优（a={a:?} b={b:?}）"
            );
        }
    }

    fn edit_distance_dp(a: &[u8], b: &[u8]) -> usize {
        // 纯删除/插入距离（无替换）：= m + n - 2*LCS
        let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for (i, row) in dp.iter_mut().enumerate() {
            row[0] = i;
        }
        for (j, cell) in dp[0].iter_mut().enumerate() {
            *cell = j;
        }
        for i in 1..=a.len() {
            for j in 1..=b.len() {
                dp[i][j] = if a[i - 1] == b[j - 1] {
                    dp[i - 1][j - 1]
                } else {
                    dp[i - 1][j].min(dp[i][j - 1]) + 1
                };
            }
        }
        dp[a.len()][b.len()]
    }

    #[test]
    fn large_fully_different_input_terminates() {
        // 压力：两侧各 5000 行全异（验证线性空间分治不爆内存）
        let a: Vec<usize> = (0..5000).collect();
        let b: Vec<usize> = (5000..10000).collect();
        let ops = diff_seq(&a, &b);
        assert_eq!(ops, vec![RawOp::Delete(5000), RawOp::Insert(5000)]);
    }

    #[test]
    fn prefix_suffix_trim_with_inner_change() {
        let a: Vec<u8> = vec![0, 1, 2, 9, 4, 5];
        let b: Vec<u8> = vec![0, 1, 2, 8, 4, 5];
        let ops = diff_seq(&a, &b);
        assert_eq!(
            ops,
            vec![
                RawOp::Equal(3),
                RawOp::Delete(1),
                RawOp::Insert(1),
                RawOp::Equal(2)
            ]
        );
    }
}
