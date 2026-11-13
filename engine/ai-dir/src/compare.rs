//! 内容判等：分块哈希（crc 模式）与逐块字节比较（binary 模式）。
//!
//! 分块哈希采用自研折叠（确定性、非加密；FP 概率与 BC 的 CRC 判等同级接受），
//! 配合 size 相等前提；哈希碰撞风险由 `--mode binary`（逐块字节比较）兜底。

use ai_vfs::{VfsError, VfsPath, VfsProvider};

/// 内容比较块大小（64KB，对齐 docs/03 §6.2 十六进制/哈希粒度惯例）
pub const CHUNK_SIZE: usize = 64 * 1024;

/// 内容比较模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentHashMode {
    /// 分块滚动哈希（快，理论存在碰撞；size 相等为前提）
    Crc,
    /// 逐块字节精确比较
    Binary,
}

/// 64 位流式折叠哈希（FNV-1a 风格乘加混合；非加密，仅判等用）。
/// 纯字节序：`fold(fold(a, x), y) == fold(a, x||y)`，任意分块结果一致。
pub fn fold_chunk(mut acc: u64, chunk: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01B3;
    for &b in chunk {
        acc ^= u64::from(b);
        acc = acc.wrapping_mul(PRIME);
    }
    acc
}

/// 判等两文件内容：先比 size，再按模式分块比较。
pub async fn content_same(
    vfs: &dyn VfsProvider,
    a: &VfsPath,
    b: &VfsPath,
    mode: ContentHashMode,
) -> Result<bool, VfsError> {
    let sa = vfs.stat(a).await?;
    let sb = vfs.stat(b).await?;
    if sa.size != sb.size {
        return Ok(false);
    }
    if sa.size == 0 {
        return Ok(true);
    }
    let mut offset: u64 = 0;
    let mut acc_a: u64 = 0;
    let mut acc_b: u64 = 0;
    loop {
        let ca = vfs.read_range(a, offset, CHUNK_SIZE).await?;
        let cb = vfs.read_range(b, offset, CHUNK_SIZE).await?;
        match mode {
            ContentHashMode::Crc => {
                acc_a = fold_chunk(acc_a, &ca);
                acc_b = fold_chunk(acc_b, &cb);
                // 长度不等（一侧 EOF 提前）即不同
                if ca.len() != cb.len() {
                    return Ok(false);
                }
                if ca.len() < CHUNK_SIZE {
                    return Ok(acc_a == acc_b);
                }
            }
            ContentHashMode::Binary => {
                if ca != cb {
                    return Ok(false);
                }
                if ca.len() < CHUNK_SIZE {
                    return Ok(true);
                }
            }
        }
        offset += CHUNK_SIZE as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_is_deterministic_and_order_sensitive() {
        let a = fold_chunk(0, b"hello world, this is a chunk");
        let b = fold_chunk(0, b"hello world, this is a chunk");
        assert_eq!(a, b);
        let c = fold_chunk(0, b"hello world, this is a chunl");
        assert_ne!(a, c);
        // 分块累计与单块全量一致
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let whole = fold_chunk(0, &data);
        let split = fold_chunk(fold_chunk(0, &data[..3000]), &data[3000..]);
        assert_eq!(whole, split);
    }
}
