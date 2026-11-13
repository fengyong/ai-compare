//! zip 归档中心目录解析（P2：CRC 预计算元数据，FR-DIR-011）。
//!
//! P2 范围：解析中心目录 -> 条目名/大小/CRC-32/mtime/类型；[`ZipFs`]
//! Provider 暴露 stat/list_dir/stored_crc32。条目内容解压（inflate）属
//! P1 压缩包 VFS，本实现 `read_file_bytes` 返回 Unsupported。
//!
//! 兼容性：标准 PKWARE APPNOTE 布局（本地文件头 + 数据 + 中心目录 + EOCD）；
//! 不支持 ZIP64（>4G 条目/偏移）与加密条目——遇到时按解析失败/Unsupported 处理。

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::crc32::crc32;
use crate::{EntryKind, StatInfo, VfsCaps, VfsDirEntry, VfsError, VfsPath, VfsProvider, unix_type_letter};

/// 单个归档条目（中心目录记录）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    /// 条目名（zip 分隔符 '/' 已保留；目录条目以 '/' 结尾，此处已剥除）
    pub name: String,
    pub size: u64,
    pub crc32: u32,
    /// DOS 时间转 epoch 毫秒（UTC 语义）
    pub mtime_ms: u64,
    pub is_dir: bool,
    /// Unix 权限（external attributes 高 16 位；非 Unix 打包为 None）
    pub unix_mode: Option<u32>,
}

/// EOCD 签名（End Of Central Directory）
const SIG_EOCD: u32 = 0x0605_4b50;
/// 中心目录条目签名
const SIG_CD: u32 = 0x0201_4b50;
/// EOCD 最小长度（不含注释）
const EOCD_LEN: usize = 22;
/// 中心目录条目固定部分长度
const CD_LEN: usize = 46;

fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// DOS 时间/日期 -> epoch 毫秒（UTC；DOS 时间戳无时区，按 UTC 解释）
fn dos_to_epoch_ms(dos_time: u16, dos_date: u16) -> u64 {
    let year = 1980 + u32::from(dos_date >> 9);
    let month = ((dos_date >> 5) & 0x0F).max(1) as u32;
    let day = (dos_date & 0x1F).max(1) as u32;
    let hour = u32::from(dos_time >> 11);
    let min = u32::from((dos_time >> 5) & 0x3F);
    let sec = 2 * u32::from(dos_time & 0x1F);
    // days_from_civil（Howard Hinnant 算法）：1970-01-01 为纪元
    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let era = y.div_euclid(400);
    let yoe = y - era * 400; // [0, 399]
    let mp = if month > 2 { i64::from(month) - 3 } else { i64::from(month) + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + i64::from(hour * 3600 + min * 60 + sec);
    (secs * 1000) as u64
}

/// 解析整个归档字节流的中心目录。返回按中心目录顺序的条目列表
/// （目录条目带 `is_dir`，名字已剥除尾部 '/'）。
pub fn parse_central_directory(data: &[u8]) -> Result<Vec<ZipEntry>, VfsError> {
    if data.len() < EOCD_LEN {
        return Err(VfsError::InvalidPath("不是 zip 归档（过短）".into()));
    }
    // 从尾部搜索 EOCD 签名（注释最长 64KB + 22）
    let scan_start = data.len().saturating_sub(EOCD_LEN + 65_536);
    let mut eocd_off = None;
    let mut i = data.len() - EOCD_LEN;
    loop {
        if rd_u32(data, i) == SIG_EOCD {
            eocd_off = Some(i);
            break;
        }
        if i == scan_start {
            break;
        }
        i -= 1;
    }
    let eocd = eocd_off.ok_or_else(|| VfsError::InvalidPath("不是 zip 归档（未找到 EOCD）".into()))?;
    let total_entries = usize::from(rd_u16(data, eocd + 10));
    let cd_size = rd_u32(data, eocd + 12) as usize;
    let cd_offset = rd_u32(data, eocd + 16) as usize;
    if cd_offset + cd_size > data.len() {
        return Err(VfsError::InvalidPath("中心目录越界（ZIP64 不支持）".into()));
    }

    let mut out = Vec::with_capacity(total_entries);
    let mut p = cd_offset;
    for _ in 0..total_entries {
        if p + CD_LEN > data.len() || rd_u32(data, p) != SIG_CD {
            return Err(VfsError::InvalidPath(format!("中心目录条目损坏（偏移 {p}）")));
        }
        let dos_time = rd_u16(data, p + 12);
        let dos_date = rd_u16(data, p + 14);
        let crc = rd_u32(data, p + 16);
        let size = rd_u32(data, p + 24) as u64;
        let name_len = usize::from(rd_u16(data, p + 28));
        let extra_len = usize::from(rd_u16(data, p + 30));
        let comment_len = usize::from(rd_u16(data, p + 32));
        let ext_attrs = rd_u32(data, p + 38);
        let name_start = p + CD_LEN;
        if name_start + name_len > data.len() {
            return Err(VfsError::InvalidPath("条目名越界".into()));
        }
        let raw_name = String::from_utf8_lossy(&data[name_start..name_start + name_len]).into_owned();
        p = name_start + name_len + extra_len + comment_len;

        let unix_mode_raw = (ext_attrs >> 16) & 0xFFFF;
        let unix_mode = if unix_mode_raw != 0 { Some(unix_mode_raw) } else { None };
        let is_dir = raw_name.ends_with('/')
            || matches!(unix_mode, Some(m) if m & 0o170000 == 0o040000);
        let name = raw_name.trim_end_matches('/').to_string();
        if name.is_empty() {
            continue;
        }
        out.push(ZipEntry {
            name,
            size,
            crc32: crc,
            mtime_ms: dos_to_epoch_ms(dos_time, dos_date),
            is_dir,
            unix_mode,
        });
    }
    Ok(out)
}

/// 基于 zip 中心目录的只读 Provider（P2：元数据 + CRC 预计算）。
///
/// 命名空间：条目相对路径（zip 分隔符已归一为 '/'），根为空串。
#[derive(Debug, Clone)]
pub struct ZipFs {
    entries: BTreeMap<String, ZipEntry>,
    /// 条目所在目录集合（自动从条目路径推导）
    dirs: BTreeMap<String, ()>,
}

impl ZipFs {
    /// 从归档字节流构建
    pub fn from_bytes(data: &[u8]) -> Result<Self, VfsError> {
        let list = parse_central_directory(data)?;
        let mut entries = BTreeMap::new();
        let mut dirs: BTreeMap<String, ()> = BTreeMap::new();
        dirs.insert(String::new(), ());
        for e in list {
            // 推导父目录链
            let mut acc = String::new();
            for seg in e.name.split('/') {
                if seg.is_empty() {
                    continue;
                }
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(seg);
                dirs.insert(acc.clone(), ());
            }
            if !e.is_dir {
                entries.insert(e.name.clone(), e);
            }
        }
        Ok(ZipFs { entries, dirs })
    }

    fn stat_of(&self, name: &str) -> Option<StatInfo> {
        if let Some(e) = self.entries.get(name) {
            let kind = if e.is_dir { EntryKind::Dir } else { EntryKind::File };
            return Some(StatInfo {
                size: e.size,
                mtime_ms: e.mtime_ms,
                kind,
                unix_mode: e.unix_mode,
                uid: None,
                gid: None,
                dos_attrs: 0,
            });
        }
        if self.dirs.contains_key(name) {
            return Some(StatInfo::minimal(0, 0, EntryKind::Dir));
        }
        None
    }

    fn normalize(path: &VfsPath) -> String {
        path.as_str().trim_matches('/').to_string()
    }
}

fn not_found(path: &VfsPath) -> VfsError {
    VfsError::NotFound { path: path.as_str().to_string() }
}

#[async_trait]
impl VfsProvider for ZipFs {
    fn scheme(&self) -> &'static str {
        "zip"
    }

    fn capabilities(&self) -> VfsCaps {
        VfsCaps::READ
    }

    async fn stat(&self, path: &VfsPath) -> Result<StatInfo, VfsError> {
        let name = Self::normalize(path);
        self.stat_of(&name).ok_or_else(|| not_found(path))
    }

    async fn list_dir(&self, path: &VfsPath) -> Result<Vec<VfsDirEntry>, VfsError> {
        let dir = Self::normalize(path);
        if !self.dirs.contains_key(&dir) {
            return Err(not_found(path));
        }
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        let mut out = Vec::new();
        // BTreeMap 顺序即字典序；同一目录的直接子项（首段）去重
        let mut seen: Option<String> = None;
        for (name, e) in self.entries.range(prefix.clone()..) {
            if !name.starts_with(&prefix) {
                break;
            }
            let rest = &name[prefix.len()..];
            if rest.is_empty() {
                continue;
            }
            let first = rest.split('/').next().expect("非空串必有首段");
            if seen.as_deref() == Some(first) {
                continue;
            }
            seen = Some(first.to_string());
            let child = format!("{prefix}{first}");
            let stat = self
                .stat_of(&child)
                .unwrap_or_else(|| StatInfo::minimal(e.size, e.mtime_ms, EntryKind::File));
            out.push(VfsDirEntry { name: first.to_string(), stat });
        }
        // 纯目录条目（如空目录）单独收集
        for (d, _) in self.dirs.range(prefix.clone()..) {
            if !d.starts_with(&prefix) || *d == dir {
                break;
            }
            let rest = &d[prefix.len()..];
            if rest.is_empty() {
                continue;
            }
            let first = rest.split('/').next().expect("非空串必有首段");
            if out.iter().any(|e: &VfsDirEntry| e.name == first) {
                continue;
            }
            let child = format!("{prefix}{first}");
            if let Some(stat) = self.stat_of(&child) {
                out.push(VfsDirEntry { name: first.to_string(), stat });
            }
        }
        Ok(out)
    }

    async fn read_file_bytes(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let _ = self.stat(path).await?;
        Err(VfsError::Unsupported(
            "zip 条目内容读取（inflate）属 P1 压缩包 VFS；P2 仅中心目录元数据/CRC".into(),
        ))
    }

    async fn stored_crc32(&self, path: &VfsPath) -> Result<Option<u32>, VfsError> {
        let name = Self::normalize(path);
        Ok(self.entries.get(&name).map(|e| e.crc32))
    }
}

/// 便捷：条目 Unix 类型字母（unix_types 过滤用）
pub fn entry_unix_type(e: &ZipEntry) -> char {
    let kind = if e.is_dir { EntryKind::Dir } else { EntryKind::File };
    unix_type_letter(kind, e.unix_mode)
}

/// 手工构造最小 zip（PKWARE 布局：本地头 + 数据 + 中心目录 + EOCD）。
/// `#[doc(hidden)]` 测试夹具：下游 crate（ai-dir 等）构造 CRC 预计算场景复用。
#[doc(hidden)]
pub fn build_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        let mut cd = Vec::new();
        let mut offsets = Vec::new();
        let base_ts: u16 = 0; // DOS date 0 -> 1980-01-01
        let base_dt: u16 = 1 << 5 | 1; // 1980-01-01（月=1 日=1）
        for (name, data) in entries {
            let crc = crc32(data);
            offsets.push(body.len() as u32);
            // 本地文件头
            body.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            body.extend_from_slice(&20u16.to_le_bytes()); // version needed
            body.extend_from_slice(&0u16.to_le_bytes()); // flags
            body.extend_from_slice(&0u16.to_le_bytes()); // method: stored
            body.extend_from_slice(&base_ts.to_le_bytes()); // time
            body.extend_from_slice(&base_dt.to_le_bytes()); // date
            body.extend_from_slice(&crc.to_le_bytes());
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(&(name.len() as u16).to_le_bytes());
            body.extend_from_slice(&0u16.to_le_bytes()); // extra len
            body.extend_from_slice(name.as_bytes());
            body.extend_from_slice(data);
        }
        for (i, (name, data)) in entries.iter().enumerate() {
            let crc = crc32(data);
            let ext_attrs: u32 = if name.ends_with('/') {
                (0o040755u32) << 16
            } else {
                (0o100644u32) << 16
            };
            cd.extend_from_slice(&SIG_CD.to_le_bytes());
            cd.extend_from_slice(&20u16.to_le_bytes()); // version made by
            cd.extend_from_slice(&20u16.to_le_bytes()); // version needed
            cd.extend_from_slice(&0u16.to_le_bytes()); // flags
            cd.extend_from_slice(&0u16.to_le_bytes()); // method
            cd.extend_from_slice(&base_ts.to_le_bytes());
            cd.extend_from_slice(&base_dt.to_le_bytes());
            cd.extend_from_slice(&crc.to_le_bytes());
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes()); // extra
            cd.extend_from_slice(&0u16.to_le_bytes()); // comment
            cd.extend_from_slice(&0u16.to_le_bytes()); // disk start
            cd.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            cd.extend_from_slice(&ext_attrs.to_le_bytes());
            cd.extend_from_slice(&offsets[i].to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
    let mut out = body;
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&cd);
    out.extend_from_slice(&SIG_EOCD.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk
    out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc32;

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        build_test_zip(entries)
    }

    #[test]
    fn dos_to_epoch_basic() {
        // 1980-01-01 00:00:00 UTC
        assert_eq!(dos_to_epoch_ms(0, (1 << 5) | 1), 315_532_800_000);
        // 2024-02-29 12:34:56 UTC（2024-03-01 00:00 = 1709251200，回退一天 + 45296s）
        let t = ((12u16) << 11) | (34 << 5) | (28); // sec/2 = 28 -> 56s
        let d = ((2024 - 1980) << 9) | (2 << 5) | 29;
        assert_eq!(dos_to_epoch_ms(t, d), 1_709_210_096_000);
    }

    #[tokio::test]
    async fn parse_and_query_minimal_zip() {
        let data = build_zip(&[
            ("hello.txt", b"hello world"),
            ("sub/", b""),
            ("sub/app.log", b"log line\n"),
        ]);
        let entries = parse_central_directory(&data).expect("解析");
        assert_eq!(entries.len(), 3);
        let hello = entries.iter().find(|e| e.name == "hello.txt").unwrap();
        assert_eq!(hello.crc32, crc32(b"hello world"));
        assert_eq!(hello.size, 11);
        assert!(!hello.is_dir);
        let sub = entries.iter().find(|e| e.name == "sub").unwrap();
        assert!(sub.is_dir);
        let app = entries.iter().find(|e| e.name == "sub/app.log").unwrap();
        assert_eq!(app.unix_mode, Some(0o100644));

        // ZipFs 查询面
        let fs = ZipFs::from_bytes(&data).unwrap();
        assert_eq!(fs.scheme(), "zip");
        let root = VfsPath::parse("/").unwrap();
        let mut top = fs.list_dir(&root).await.unwrap();
        top.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = top.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["hello.txt", "sub"]);

        let hello_path = VfsPath::parse("/hello.txt").unwrap();
        let st = fs.stat(&hello_path).await.unwrap();
        assert_eq!(st.size, 11);
        assert_eq!(fs.stored_crc32(&hello_path).await.unwrap(), Some(crc32(b"hello world")));
        let inner = VfsPath::parse("/sub/app.log").unwrap();
        assert_eq!(fs.stored_crc32(&inner).await.unwrap(), Some(crc32(b"log line\n")));
        assert_eq!(entry_unix_type(app), 'r');
        // 内容读取明确不支持（P1 范围）
        let err = fs.read_file_bytes(&hello_path).await.unwrap_err();
        assert!(matches!(err, VfsError::Unsupported(_)));
    }

    #[test]
    fn corrupt_zip_is_error() {
        let mut data = build_zip(&[("a.txt", b"x")]);
        // 破坏 EOCD 签名
        let n = data.len();
        data[n - 22..n - 18].copy_from_slice(&[0u8; 4]);
        assert!(parse_central_directory(&data).is_err());
        assert!(parse_central_directory(b"short").is_err());
    }

    /// P6 写路径：zip 只读 Provider 能力位 false，写方法走 trait 默认实现
    #[tokio::test]
    async fn readonly_provider_rejects_write_ops() {
        let fs = ZipFs::from_bytes(&build_zip(&[("a.txt", b"x")])).unwrap();
        let caps = fs.capabilities();
        assert!(caps.has(crate::VfsCaps::READ));
        for cap in [
            crate::VfsCaps::WRITE,
            crate::VfsCaps::MKDIR,
            crate::VfsCaps::DELETE,
            crate::VfsCaps::MOVE,
            crate::VfsCaps::COPY,
            crate::VfsCaps::TOUCH,
        ] {
            assert!(!caps.has(cap), "zip 不应声明写能力位 {cap:?}");
        }
        let p = VfsPath::parse("/a.txt").unwrap();
        let q = VfsPath::parse("/b.txt").unwrap();
        assert!(matches!(fs.write_file_bytes(&p, b"x").await, Err(VfsError::Unsupported(_))));
        assert!(matches!(fs.mkdir(&q).await, Err(VfsError::Unsupported(_))));
        assert!(matches!(fs.rename(&p, &q).await, Err(VfsError::Unsupported(_))));
        assert!(matches!(fs.remove(&p, true).await, Err(VfsError::Unsupported(_))));
        assert!(matches!(fs.copy_file(&p, &q).await, Err(VfsError::Unsupported(_))));
        assert!(matches!(fs.set_mtime(&p, 0).await, Err(VfsError::Unsupported(_))));
        // mkdir_all 走 stat 路径：能力协商在执行层判断，这里只验默认 mkdir 已拦
        assert!(matches!(fs.mkdir_all(&q).await, Err(VfsError::Unsupported(_))));
    }
}
