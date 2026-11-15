//! 内容判等：CRC-32（惰性分块 + LRU/SQLite 缓存 + zip 中心目录预计算）与
//! 逐块字节比较（binary 模式）。
//!
//! CRC 判等语义（FR-DIR-011）：size 相等为前提；zip 等提供预计算 CRC 的
//! 资源直接取 `VfsProvider::stored_crc32`（免读内容）；其余按需分块计算并
//! 以 `(path, size, mtime_ms)` 为键缓存（[`crate::cache::HashCache`]）。

use ai_vfs::{crc32_update, VfsError, VfsPath, VfsProvider, CRC32_INITIAL};

use crate::cache::HashCache;

/// 内容比较块大小（64KB，对齐 docs/03 §6.2 十六进制/哈希粒度惯例）
pub const CHUNK_SIZE: usize = 64 * 1024;

/// 内容比较模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentHashMode {
    /// CRC-32 分块哈希（快；碰撞概率与 BC 的 CRC 判等同级接受）
    Crc,
    /// 逐块字节精确比较
    Binary,
}

/// 64 位流式折叠哈希（FNV-1a 风格乘加混合；非加密，仅判等用；保留为
/// 公共 API 兼容 P0 调用方——P2 内容比较已切 CRC-32）。
/// 纯字节序：`fold(fold(a, x), y) == fold(a, x||y)`，任意分块结果一致。
pub fn fold_chunk(mut acc: u64, chunk: &[u8]) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01B3;
    for &b in chunk {
        acc ^= u64::from(b);
        acc = acc.wrapping_mul(PRIME);
    }
    acc
}

/// 单文件 CRC-32（流式分块；大文件不整载内存）
pub async fn file_crc32(vfs: &dyn VfsProvider, path: &VfsPath) -> Result<u32, VfsError> {
    let mut acc = CRC32_INITIAL;
    let mut offset: u64 = 0;
    loop {
        let chunk = vfs.read_range(path, offset, CHUNK_SIZE).await?;
        acc = crc32_update(acc, &chunk);
        if chunk.len() < CHUNK_SIZE {
            break;
        }
        offset += CHUNK_SIZE as u64;
    }
    Ok(acc ^ 0xFFFF_FFFF)
}

/// 带缓存的文件 CRC-32：先查 zip 预计算（stored_crc32），再查缓存，
/// 未命中读内容计算并回填。缓存键 `(path, size, mtime_ms)`。
pub async fn file_crc32_cached(
    vfs: &dyn VfsProvider,
    path: &VfsPath,
    size: u64,
    mtime_ms: u64,
    cache: Option<&HashCache>,
) -> Result<u32, VfsError> {
    if let Some(stored) = vfs.stored_crc32(path).await? {
        return Ok(stored);
    }
    if let Some(c) = cache {
        if let Some(crc) = c.get(path.as_str(), size, mtime_ms) {
            return Ok(crc);
        }
        let crc = file_crc32(vfs, path).await?;
        c.put(path.as_str(), size, mtime_ms, crc);
        return Ok(crc);
    }
    file_crc32(vfs, path).await
}

/// 单侧文件 stat 摘要（判等参数打包）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStatSummary {
    pub size: u64,
    pub mtime_ms: u64,
}

impl FileStatSummary {
    pub fn new(size: u64, mtime_ms: u64) -> Self {
        FileStatSummary { size, mtime_ms }
    }
}

/// 判等两文件内容（带缓存 CRC / 精确二进制两模式）。
/// stat 由调用方提供（避免重复 stat；扫描路径已持有）。
pub async fn content_same_stats(
    vfs: &dyn VfsProvider,
    a: &VfsPath,
    b: &VfsPath,
    sa: FileStatSummary,
    sb: FileStatSummary,
    mode: ContentHashMode,
    cache: Option<&HashCache>,
) -> Result<bool, VfsError> {
    if sa.size != sb.size {
        return Ok(false);
    }
    if sa.size == 0 {
        return Ok(true);
    }
    match mode {
        ContentHashMode::Crc => {
            let ca = file_crc32_cached(vfs, a, sa.size, sa.mtime_ms, cache).await?;
            let cb = file_crc32_cached(vfs, b, sb.size, sb.mtime_ms, cache).await?;
            Ok(ca == cb)
        }
        ContentHashMode::Binary => {
            let mut offset: u64 = 0;
            loop {
                let ca = vfs.read_range(a, offset, CHUNK_SIZE).await?;
                let cb = vfs.read_range(b, offset, CHUNK_SIZE).await?;
                if ca != cb {
                    return Ok(false);
                }
                if ca.len() < CHUNK_SIZE {
                    return Ok(true);
                }
                offset += CHUNK_SIZE as u64;
            }
        }
    }
}

/// 判等两文件内容：先比 size，再按模式分块比较（P0 兼容入口，无缓存）。
pub async fn content_same(
    vfs: &dyn VfsProvider,
    a: &VfsPath,
    b: &VfsPath,
    mode: ContentHashMode,
) -> Result<bool, VfsError> {
    let sa = vfs.stat(a).await?;
    let sb = vfs.stat(b).await?;
    content_same_stats(
        vfs,
        a,
        b,
        FileStatSummary::new(sa.size, sa.mtime_ms),
        FileStatSummary::new(sb.size, sb.mtime_ms),
        mode,
        None,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_vfs::LocalFs;

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

    #[tokio::test]
    async fn crc_mode_matches_and_differs() {
        let dir = tempfile::tempdir().unwrap();
        let big: Vec<u8> = (0..200_000u32)
            .map(|i| (i.wrapping_mul(31) % 256) as u8)
            .collect();
        std::fs::write(dir.path().join("a.bin"), &big).unwrap();
        std::fs::write(dir.path().join("a2.bin"), &big).unwrap();
        let mut diff = big.clone();
        let last = diff.len() - 1;
        diff[last] ^= 0xff;
        std::fs::write(dir.path().join("b.bin"), &diff).unwrap();

        let vfs = LocalFs::new();
        let pa = VfsPath::parse(dir.path().join("a.bin").to_str().unwrap()).unwrap();
        let pa2 = VfsPath::parse(dir.path().join("a2.bin").to_str().unwrap()).unwrap();
        let pb = VfsPath::parse(dir.path().join("b.bin").to_str().unwrap()).unwrap();
        assert!(content_same(&vfs, &pa, &pa2, ContentHashMode::Crc)
            .await
            .unwrap());
        assert!(!content_same(&vfs, &pa, &pb, ContentHashMode::Crc)
            .await
            .unwrap());
        assert!(content_same(&vfs, &pa, &pa2, ContentHashMode::Binary)
            .await
            .unwrap());
        assert!(!content_same(&vfs, &pa, &pb, ContentHashMode::Binary)
            .await
            .unwrap());
        assert_eq!(file_crc32(&vfs, &pa).await.unwrap(), ai_vfs::crc32(&big));
    }

    #[tokio::test]
    async fn cached_crc_uses_cache_without_reading_twice() {
        // 缓存命中路径：人为伪造缓存值应影响结果（证明走了缓存）
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.bin"), b"actual-content").unwrap();
        std::fs::write(dir.path().join("y.bin"), b"actual-content").unwrap();
        let vfs = LocalFs::new();
        let cache = HashCache::in_memory();
        let px = VfsPath::parse(dir.path().join("x.bin").to_str().unwrap()).unwrap();
        let py = VfsPath::parse(dir.path().join("y.bin").to_str().unwrap()).unwrap();

        // 真实计算并缓存
        let crc = file_crc32_cached(&vfs, &px, 14, 1, Some(&cache))
            .await
            .unwrap();
        assert_eq!(crc, ai_vfs::crc32(b"actual-content"));
        // 篡改缓存：第二次读取必须走缓存（结果被伪造值覆盖）
        cache.put(py.as_str(), 14, 1, 0xCAFE);
        let again = file_crc32_cached(&vfs, &py, 14, 1, Some(&cache))
            .await
            .unwrap();
        assert_eq!(again, 0xCAFE, "缓存命中不应再读文件");
    }

    #[tokio::test]
    async fn stored_crc_takes_precedence() {
        // ZipFs 的 stored_crc32 优先于读内容（zip 内容读取本身不支持）
        let data = ai_vfs::zip::build_test_zip(&[("hello.txt", b"hello world")]);
        let vfs = ai_vfs::ZipFs::from_bytes(&data).unwrap();
        let p = VfsPath::parse("/hello.txt").unwrap();
        let st = vfs.stat(&p).await.unwrap();
        let crc = file_crc32_cached(&vfs, &p, st.size, st.mtime_ms, None)
            .await
            .unwrap();
        assert_eq!(
            crc,
            ai_vfs::crc32(b"hello world"),
            "应使用中心目录预计算 CRC"
        );
    }
}
