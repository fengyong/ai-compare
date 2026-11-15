//! CRC-32（IEEE 802.3，反射多项式 0xEDB88320）：zip 中心目录存储值与
//! ai-dir 内容比较共用（FR-DIR-011 CRC 比较）。
//!
//! 流式接口：`crc32_update(acc, chunk)` 可任意分块累计，与全量一次计算结果一致。

const POLY: u32 = 0xEDB8_8320;

/// 256 项查表（首次惰性构建；单线程初始化OnceLock）
static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();

fn table() -> &'static [u32; 256] {
    TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, item) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { POLY ^ (c >> 1) } else { c >> 1 };
            }
            *item = c;
        }
        t
    })
}

/// CRC-32 起始值（标准初始余数）
pub const INITIAL: u32 = 0xFFFF_FFFF;

/// 流式累计一块数据：`crc32_update(crc32_update(INITIAL, a), b) == crc32(a || b)`
pub fn crc32_update(acc: u32, data: &[u8]) -> u32 {
    let t = table();
    let mut c = acc;
    for &b in data {
        c = t[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    c
}

/// 一次性全量 CRC-32
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(INITIAL, data) ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // 标准校验向量
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn streaming_matches_whole() {
        let data: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
        let whole = crc32(&data);
        let split = crc32_update(
            crc32_update(crc32_update(INITIAL, &data[..3]), &data[3..7777]),
            &data[7777..],
        );
        assert_eq!(whole, split ^ 0xFFFF_FFFF);
    }
}
