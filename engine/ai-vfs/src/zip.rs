//! zip 归档 Provider（P2 只读元数据 + P12 可写 ZipFsMut）。
//!
//! **只读 `ZipFs`**（P2）：解析中心目录 -> 条目名/大小/CRC-32/mtime/类型；
//! stat/list_dir/stored_crc32 零内存装载（ai-dir CRC 预计算用）；P12 起支持
//! 条目内容读取（stored 切片 / deflate inflate，`raw` 归档字节随实例持有）。
//!
//! **可写 `ZipFsMut`**（P12）：「装载 -> 内存改动 -> 保存回写」模型——
//! `from_bytes_writable` 全量装载（条目内容解压驻留内存），内部状态
//! `Arc<RwLock<ZipState>>`（trait &self 写路径经写锁，会话模型已串行），
//! 写方法全套（write/mkdir/rename/remove/copy/set_mtime），`to_bytes`
//! 重打包（deflate level 6 + 重算 CRC + DOS 时间写回）。大 zip 的流式
//! 就地修改不支持（与 BC 的「装载-保存」行为预期一致）；ZIP64 与加密
//! 条目不支持（遇到报 Unsupported/解析失败）。
//!
//! 兼容性：标准 PKWARE APPNOTE 布局（本地文件头 + 数据 + 中心目录 + EOCD）；
//! method 仅 stored(0)/deflate(8)；加密（flags bit0）报 Unsupported；
//! data-descriptor（bit3）条目尺寸取自中心目录（本地头占位不可信）。

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::crc32::crc32;
use crate::{EntryKind, StatInfo, VfsCaps, VfsDirEntry, VfsError, VfsPath, VfsProvider, unix_type_letter};

/// 单个归档条目（中心目录记录；只读视图）
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
/// 本地文件头签名
const SIG_LFH: u32 = 0x0403_4b50;
/// EOCD 最小长度（不含注释）
const EOCD_LEN: usize = 22;
/// 中心目录条目固定部分长度
const CD_LEN: usize = 46;
/// 本地文件头固定部分长度
const LFH_LEN: usize = 30;
/// zip method：stored（不压缩）
const METHOD_STORED: u16 = 0;
/// zip method：deflate（raw deflate，RFC1951）
const METHOD_DEFLATE: u16 = 8;

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

/// epoch 毫秒 -> DOS 时间/日期（写回用；<1980 钳 1980-01-01，>2107 钳上限；
/// 2 秒精度（DOS 秒字段单位），`set_mtime` 的精度损失即此，文档化）
fn epoch_ms_to_dos(epoch_ms: u64) -> (u16, u16) {
    let total_secs = (epoch_ms / 1000).min(4_354_815_199) as i64; // 2107-12-31 23:59:59
    let days = total_secs.div_euclid(86_400);
    let secs_of_day = total_secs.rem_euclid(86_400);
    // civil_from_days（Howard Hinnant 算法）
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    let year = year.clamp(1980, 2107) as u16;
    let month = u16::try_from(m).unwrap_or(12);
    let day = u16::try_from(d).unwrap_or(31);
    let hour = u16::try_from(secs_of_day / 3600).unwrap_or(23);
    let min = u16::try_from((secs_of_day % 3600) / 60).unwrap_or(59);
    let sec = u16::try_from((secs_of_day % 60) / 2).unwrap_or(29);
    let dos_date = ((year - 1980) << 9) | (month << 5) | day;
    let dos_time = (hour << 11) | (min << 5) | sec;
    (dos_time, dos_date)
}

/// 中心目录解析产物：条目 + 本地数据定位信息（只读 ZipFs 按需读取用）
#[derive(Debug, Clone)]
struct CdRecord {
    entry: ZipEntry,
    method: u16,
    flags: u16,
    /// 本地文件头在归档中的偏移
    lfh_offset: usize,
    compressed_size: u64,
}

/// 从本地文件头定位条目数据起点（本地头的 extra 长度可能与中心目录不同）
fn locate_local_data(data: &[u8], rec: &CdRecord) -> Result<(u16, usize), VfsError> {
    let off = rec.lfh_offset;
    if off + LFH_LEN > data.len() || rd_u32(data, off) != SIG_LFH {
        return Err(VfsError::InvalidPath(format!(
            "本地文件头损坏（偏移 {off}）"
        )));
    }
    let name_len = usize::from(rd_u16(data, off + 26));
    let extra_len = usize::from(rd_u16(data, off + 28));
    let start = off + LFH_LEN + name_len + extra_len;
    let end = start
        .checked_add(rec.compressed_size as usize)
        .ok_or_else(|| VfsError::InvalidPath("条目数据越界".into()))?;
    if end > data.len() {
        return Err(VfsError::InvalidPath("条目数据越界".into()));
    }
    Ok((rec.method, start))
}

/// 解压条目内容（stored 切片 / raw-deflate inflate；加密或未知 method 报 Unsupported）
fn inflate_entry(data: &[u8], rec: &CdRecord) -> Result<Vec<u8>, VfsError> {
    if rec.flags & 0x0001 != 0 {
        return Err(VfsError::Unsupported("zip 加密条目不支持".into()));
    }
    let (method, start) = locate_local_data(data, rec)?;
    let cdata = &data[start..start + rec.compressed_size as usize];
    match method {
        METHOD_STORED => Ok(cdata.to_vec()),
        METHOD_DEFLATE => miniz_oxide::inflate::decompress_to_vec(cdata)
            .map_err(|e| VfsError::InvalidPath(format!("deflate 解压失败: {e:?}"))),
        other => Err(VfsError::Unsupported(format!(
            "zip 压缩方法 {other} 不支持（仅 stored/deflate）"
        ))),
    }
}

/// 解析整个归档字节流的中心目录（内部：含 method/flags/定位）。
fn parse_cd_records(data: &[u8]) -> Result<Vec<CdRecord>, VfsError> {
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
        let compressed_size = rd_u32(data, p + 20) as u64;
        let name_len = usize::from(rd_u16(data, p + 28));
        let extra_len = usize::from(rd_u16(data, p + 30));
        let comment_len = usize::from(rd_u16(data, p + 32));
        let ext_attrs = rd_u32(data, p + 38);
        let lfh_offset = rd_u32(data, p + 42) as usize;
        let method = rd_u16(data, p + 10);
        let flags = rd_u16(data, p + 8);
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
        out.push(CdRecord {
            entry: ZipEntry {
                name,
                size,
                crc32: crc,
                mtime_ms: dos_to_epoch_ms(dos_time, dos_date),
                is_dir,
                unix_mode,
            },
            method,
            flags,
            lfh_offset,
            compressed_size,
        });
    }
    Ok(out)
}

/// 解析整个归档字节流的中心目录。返回按中心目录顺序的条目列表
/// （目录条目带 `is_dir`，名字已剥除尾部 '/'）。
pub fn parse_central_directory(data: &[u8]) -> Result<Vec<ZipEntry>, VfsError> {
    Ok(parse_cd_records(data)?.into_iter().map(|r| r.entry).collect())
}

/// 基于 zip 中心目录的只读 Provider（P2 元数据/CRC 预计算；P12 起内容可读）。
///
/// 命名空间：条目相对路径（zip 分隔符已归一为 '/'），根为空串。
/// `raw` 归档字节随实例持有（`read_file_bytes` 按需解压）。
#[derive(Debug, Clone)]
pub struct ZipFs {
    raw: Vec<u8>,
    records: Vec<CdRecord>,
    entries: BTreeMap<String, usize>,
    /// 条目所在目录集合（自动从条目路径推导）
    dirs: BTreeMap<String, ()>,
}

impl ZipFs {
    /// 从归档字节流构建（只读；内容按需解压）
    pub fn from_bytes(data: &[u8]) -> Result<Self, VfsError> {
        let records = parse_cd_records(data)?;
        let mut entries = BTreeMap::new();
        let mut dirs: BTreeMap<String, ()> = BTreeMap::new();
        dirs.insert(String::new(), ());
        for (idx, rec) in records.iter().enumerate() {
            // 推导父目录链
            let mut acc = String::new();
            for seg in rec.entry.name.split('/') {
                if seg.is_empty() {
                    continue;
                }
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(seg);
                dirs.insert(acc.clone(), ());
            }
            if !rec.entry.is_dir {
                entries.insert(rec.entry.name.clone(), idx);
            }
        }
        Ok(ZipFs {
            raw: data.to_vec(),
            records,
            entries,
            dirs,
        })
    }

    fn stat_of(&self, name: &str) -> Option<StatInfo> {
        if let Some(&idx) = self.entries.get(name) {
            let e = &self.records[idx].entry;
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
        for (name, &idx) in self.entries.range(prefix.clone()..) {
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
                .unwrap_or_else(|| StatInfo::minimal(self.records[idx].entry.size, self.records[idx].entry.mtime_ms, EntryKind::File));
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
        let name = Self::normalize(path);
        let &idx = self
            .entries
            .get(&name)
            .ok_or_else(|| not_found(path))?;
        inflate_entry(&self.raw, &self.records[idx])
    }

    async fn stored_crc32(&self, path: &VfsPath) -> Result<Option<u32>, VfsError> {
        let name = Self::normalize(path);
        Ok(self.entries.get(&name).map(|&idx| self.records[idx].entry.crc32))
    }
}

/// 可写条目（内容解压驻留内存；crc32 为当前 data 的 CRC，写路径维护）
#[derive(Debug, Clone)]
pub struct ZipEntryMut {
    pub size: u64,
    pub crc32: u32,
    pub mtime_ms: u64,
    pub unix_mode: Option<u32>,
    pub data: Vec<u8>,
}

/// 显式目录条目（空目录/目录 mtime；推导目录 mtime = 0）
#[derive(Debug, Clone)]
struct DirMeta {
    mtime_ms: u64,
    unix_mode: Option<u32>,
}

impl Default for DirMeta {
    fn default() -> Self {
        DirMeta { mtime_ms: 0, unix_mode: Some(0o040755) }
    }
}

/// 可变内部状态（ZipFsMut 的 RwLock 载荷；纯逻辑方法集中在此）
#[derive(Debug, Default)]
struct ZipState {
    entries: BTreeMap<String, ZipEntryMut>,
    dirs: BTreeMap<String, DirMeta>,
}

impl ZipState {
    fn stat_of(&self, name: &str) -> Option<StatInfo> {
        if let Some(e) = self.entries.get(name) {
            return Some(StatInfo {
                size: e.size,
                mtime_ms: e.mtime_ms,
                kind: EntryKind::File,
                unix_mode: e.unix_mode,
                uid: None,
                gid: None,
                dos_attrs: 0,
            });
        }
        self.dirs
            .get(name)
            .map(|d| StatInfo::minimal(0, d.mtime_ms, EntryKind::Dir))
    }

    fn norm_rel(path: &VfsPath) -> Result<String, VfsError> {
        let s = path.as_str().trim_matches('/');
        if s.is_empty() {
            return Err(VfsError::InvalidPath("根路径不可作条目名".into()));
        }
        if s.split('/').any(|seg| seg.is_empty() || seg == ".." || seg == ".") {
            return Err(VfsError::InvalidPath(format!("条目路径含空段/相对段: {s}")));
        }
        Ok(s.to_string())
    }

    /// 建立父目录链（a/b/c.txt -> a/, a/b/ 入 dirs）；父链命中文件报 NotADirectory
    fn ensure_parent_chain(&mut self, name: &str) -> Result<(), VfsError> {
        let mut acc = String::new();
        let segs: Vec<&str> = name.split('/').collect();
        for seg in &segs[..segs.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(seg);
            if self.entries.contains_key(&acc) {
                return Err(VfsError::NotADirectory { path: acc });
            }
            self.dirs.entry(acc.clone()).or_default();
        }
        Ok(())
    }

    /// 空父链修剪（删除条目后清理仅因该条目存在的目录）
    fn prune_parent_chain(&mut self, name: &str) {
        let mut cur = name.to_string();
        while let Some(idx) = cur.rfind('/') {
            cur.truncate(idx);
            let prefix = format!("{cur}/");
            let has_child = self.entries.keys().filter(|k| k.starts_with(&prefix)).count() > 0
                || self.dirs.keys().filter(|k| k.starts_with(&prefix)).count() > 0;
            if has_child {
                break;
            }
            self.dirs.remove(&cur);
        }
    }

    /// 全量重建目录集合（rename/remove 后调用；保留显式目录元数据）。
    /// 只推父前缀（不含条目名本身）——目录集不得混入文件路径；
    /// 显式目录与文件名冲突时以文件为准（zip 合法结构中二者不同名）。
    fn rebuild_dirs(&mut self) {
        let mut dirs: BTreeMap<String, DirMeta> = BTreeMap::new();
        dirs.insert(String::new(), DirMeta::default());
        for name in self.entries.keys() {
            let mut acc = String::new();
            let segs: Vec<&str> = name.split('/').collect();
            for seg in &segs[..segs.len().saturating_sub(1)] {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(seg);
                dirs.entry(acc.clone()).or_default();
            }
        }
        for (k, d) in &self.dirs {
            if !self.entries.contains_key(k) {
                dirs.entry(k.clone()).or_insert(d.clone());
            }
        }
        self.dirs = dirs;
    }

    fn write_file(&mut self, path: &VfsPath, data: &[u8]) -> Result<(), VfsError> {
        let name = Self::norm_rel(path)?;
        self.ensure_parent_chain(&name)?;
        let (mtime, mode) = match self.entries.get(&name) {
            Some(e) => (e.mtime_ms, e.unix_mode), // 替换保留原 mtime/mode（对拍稳定语义）
            None => (0, Some(0o100644)),          // 新增 1980 元年 + 常规文件位
        };
        self.dirs.remove(&name); // 同名显式目录条目被文件覆盖
        self.entries.insert(
            name,
            ZipEntryMut {
                size: data.len() as u64,
                crc32: crc32(data),
                mtime_ms: mtime,
                unix_mode: mode,
                data: data.to_vec(),
            },
        );
        Ok(())
    }

    fn create_dir(&mut self, path: &VfsPath) -> Result<(), VfsError> {
        let name = Self::norm_rel(path)?;
        if self.entries.contains_key(&name) || self.dirs.contains_key(&name) {
            return Err(VfsError::AlreadyExists { path: name });
        }
        self.ensure_parent_chain(&name)?;
        self.dirs.insert(name, DirMeta::default());
        Ok(())
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let src = Self::norm_rel(from)?;
        let dst = Self::norm_rel(to)?;
        if dst == src || dst.starts_with(&format!("{src}/")) {
            return Err(VfsError::InvalidPath("重命名目标在源子树内".into()));
        }
        if self.entries.contains_key(&dst) || self.dirs.contains_key(&dst) {
            return Err(VfsError::AlreadyExists { path: dst });
        }
        self.ensure_parent_chain(&dst)?;
        if let Some(e) = self.entries.remove(&src) {
            self.dirs.remove(&src);
            self.entries.insert(dst, e);
            self.prune_parent_chain(&src);
            return Ok(());
        }
        if let Some(d) = self.dirs.remove(&src) {
            // 目录子树：entries + dirs 前缀移动
            let subtree_prefix = format!("{src}/");
            let moved_entries: Vec<(String, ZipEntryMut)> = self
                .entries
                .iter()
                .filter(|(k, _)| k.starts_with(&subtree_prefix))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (k, v) in moved_entries {
                self.entries.remove(&k);
                self.entries.insert(format!("{dst}/{}", &k[src.len() + 1..]), v);
            }
            let moved_dirs: Vec<(String, DirMeta)> = self
                .dirs
                .iter()
                .filter(|(k, _)| k.starts_with(&subtree_prefix))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for (k, v) in moved_dirs {
                self.dirs.remove(&k);
                self.dirs.insert(format!("{dst}/{}", &k[src.len() + 1..]), v);
            }
            self.dirs.insert(dst.clone(), d);
            self.prune_parent_chain(&src);
            self.rebuild_dirs();
            let _ = dst;
            return Ok(());
        }
        Err(VfsError::NotFound { path: src })
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        let name = Self::norm_rel(path)?;
        if self.entries.remove(&name).is_some() {
            self.prune_parent_chain(&name);
            return Ok(());
        }
        if self.dirs.contains_key(&name) {
            let prefix = format!("{name}/");
            let entry_children = self.entries.keys().filter(|k| k.starts_with(&prefix)).count();
            if entry_children > 0 && !recursive {
                return Err(VfsError::InvalidPath(format!("目录非空（需 recursive）: {name}")));
            }
            let keys: Vec<String> = self.entries.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
            for k in keys {
                self.entries.remove(&k);
            }
            let dkeys: Vec<String> = self.dirs.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
            for k in dkeys {
                self.dirs.remove(&k);
            }
            self.dirs.remove(&name);
            self.rebuild_dirs();
            return Ok(());
        }
        Err(VfsError::NotFound { path: name })
    }

    fn copy_file(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let src = Self::norm_rel(from)?;
        let dst = Self::norm_rel(to)?;
        let e = self
            .entries
            .get(&src)
            .ok_or(VfsError::NotAFile { path: src })?;
        let (data, mtime, mode) = (e.data.clone(), e.mtime_ms, e.unix_mode);
        self.ensure_parent_chain(&dst)?;
        self.dirs.remove(&dst);
        self.entries.insert(
            dst,
            ZipEntryMut {
                size: data.len() as u64,
                crc32: crc32(&data),
                mtime_ms: mtime,
                unix_mode: mode,
                data,
            },
        );
        Ok(())
    }

    fn set_mtime(&mut self, path: &VfsPath, mtime_ms: u64) -> Result<(), VfsError> {
        let name = Self::norm_rel(path)?;
        if let Some(e) = self.entries.get_mut(&name) {
            e.mtime_ms = mtime_ms;
            return Ok(());
        }
        if let Some(d) = self.dirs.get_mut(&name) {
            d.mtime_ms = mtime_ms;
            return Ok(());
        }
        Err(VfsError::NotFound { path: name })
    }

    /// 重打包字节流（deflate level 6；重算全部 CRC；显式目录条目一并写出）
    fn to_bytes(&self) -> Vec<u8> {
        let mut body = Vec::new();
        let mut cd = Vec::new();
        let mut offsets = Vec::new();
        // 统一写出：文件条目 + 显式目录条目（字典序；名字带 '/'）
        let mut out_names: Vec<(String, bool)> = self.entries.keys().map(|n| (n.clone(), false)).collect();
        for d in self.dirs.keys() {
            if !d.is_empty() && !self.entries.contains_key(d) {
                out_names.push((d.clone(), true));
            }
        }
        out_names.sort();
        for (name, is_dir) in &out_names {
            let (crc, size, payload): (u32, u64, Vec<u8>) = if *is_dir {
                (crc32(&[]), 0, Vec::new())
            } else {
                let e = &self.entries[name];
                let compressed = miniz_oxide::deflate::compress_to_vec(&e.data, 6);
                (e.crc32, e.data.len() as u64, compressed)
            };
            let (mtime_ms, _unix_mode) = if *is_dir {
                let d = &self.dirs[name];
                (d.mtime_ms, d.unix_mode)
            } else {
                (self.entries[name].mtime_ms, self.entries[name].unix_mode)
            };
            let (dos_time, dos_date) = epoch_ms_to_dos(mtime_ms);
            let stored_name = if *is_dir { format!("{name}/") } else { name.clone() };
            offsets.push(body.len() as u32);
            body.extend_from_slice(&SIG_LFH.to_le_bytes());
            body.extend_from_slice(&20u16.to_le_bytes()); // version needed
            body.extend_from_slice(&0u16.to_le_bytes()); // flags
            body.extend_from_slice(&METHOD_DEFLATE.to_le_bytes());
            body.extend_from_slice(&dos_time.to_le_bytes());
            body.extend_from_slice(&dos_date.to_le_bytes());
            body.extend_from_slice(&crc.to_le_bytes());
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(&(size as u32).to_le_bytes());
            body.extend_from_slice(&(stored_name.len() as u16).to_le_bytes());
            body.extend_from_slice(&0u16.to_le_bytes()); // extra len
            body.extend_from_slice(stored_name.as_bytes());
            body.extend_from_slice(&payload);
        }
        for (i, (name, is_dir)) in out_names.iter().enumerate() {
            let (crc, size, csize) = if *is_dir {
                (crc32(&[]), 0u64, 0u64)
            } else {
                let e = &self.entries[name];
                let compressed_len = {
                    // 本地头已写出对应压缩体：按 body 中该条目压缩尺寸回填
                    let lfh = offsets[i] as usize;
                    let name_len = usize::from(rd_u16(&body, lfh + 26));
                    let extra_len = usize::from(rd_u16(&body, lfh + 28));
                    let data_start = lfh + LFH_LEN + name_len + extra_len;
                    let csize = rd_u32(&body, lfh + 18) as usize;
                    (data_start + csize) as u64 - data_start as u64
                };
                (e.crc32, e.size, compressed_len)
            };
            let (mtime_ms, unix_mode) = if *is_dir {
                let d = &self.dirs[name];
                (d.mtime_ms, d.unix_mode)
            } else {
                (self.entries[name].mtime_ms, self.entries[name].unix_mode)
            };
            let (dos_time, dos_date) = epoch_ms_to_dos(mtime_ms);
            let ext_attrs: u32 = match unix_mode {
                Some(m) => m << 16,
                None if *is_dir => 0o040755u32 << 16,
                None => 0o100644u32 << 16,
            };
            let stored_name = if *is_dir { format!("{name}/") } else { name.clone() };
            cd.extend_from_slice(&SIG_CD.to_le_bytes());
            cd.extend_from_slice(&20u16.to_le_bytes()); // version made by
            cd.extend_from_slice(&20u16.to_le_bytes()); // version needed
            cd.extend_from_slice(&0u16.to_le_bytes()); // flags
            cd.extend_from_slice(&METHOD_DEFLATE.to_le_bytes());
            cd.extend_from_slice(&dos_time.to_le_bytes());
            cd.extend_from_slice(&dos_date.to_le_bytes());
            cd.extend_from_slice(&crc.to_le_bytes());
            cd.extend_from_slice(&(csize as u32).to_le_bytes());
            cd.extend_from_slice(&(size as u32).to_le_bytes());
            cd.extend_from_slice(&(stored_name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes()); // extra
            cd.extend_from_slice(&0u16.to_le_bytes()); // comment
            cd.extend_from_slice(&0u16.to_le_bytes()); // disk start
            cd.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            cd.extend_from_slice(&ext_attrs.to_le_bytes());
            cd.extend_from_slice(&offsets[i].to_le_bytes());
            cd.extend_from_slice(stored_name.as_bytes());
        }
        let mut out = body;
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&cd);
        out.extend_from_slice(&SIG_EOCD.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // disk
        out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
        out.extend_from_slice(&(out_names.len() as u16).to_le_bytes());
        out.extend_from_slice(&(out_names.len() as u16).to_le_bytes());
        out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out
    }
}

/// 可写 zip Provider（P12）：「装载 -> 内存改动 -> 保存回写」。
///
/// 内部状态 `Arc<RwLock<ZipState>>`：trait `&self` 写路径经写锁即时生效
/// （ai-engine 会话请求已串行，锁仅防 Api 层并发）；`to_bytes` 读锁重打包。
#[derive(Debug, Clone, Default)]
pub struct ZipFsMut {
    state: Arc<RwLock<ZipState>>,
    /// 归档文件路径（from_file 装载时携带）：trait 写方法成功后 write-through
    /// 自动回写归档（BC「复制到压缩包」直觉语义）；from_bytes/empty 构造为
    /// None（纯内存，调用方 to_bytes 自行落盘）。
    archive: Option<Arc<std::path::PathBuf>>,
}

impl ZipFsMut {
    /// 全量装载（条目内容解压驻留内存）
    pub fn from_bytes_writable(data: &[u8]) -> Result<Self, VfsError> {
        let records = parse_cd_records(data)?;
        let mut st = ZipState::default();
        for rec in records {
            let name = rec.entry.name.clone();
            if rec.entry.is_dir {
                st.dirs.insert(
                    name,
                    DirMeta {
                        mtime_ms: rec.entry.mtime_ms,
                        unix_mode: rec.entry.unix_mode,
                    },
                );
            } else {
                let content = inflate_entry(data, &rec)?;
                st.entries.insert(
                    name,
                    ZipEntryMut {
                        size: rec.entry.size,
                        crc32: rec.entry.crc32,
                        mtime_ms: rec.entry.mtime_ms,
                        unix_mode: rec.entry.unix_mode,
                        data: content,
                    },
                );
            }
        }
        st.rebuild_dirs();
        Ok(ZipFsMut {
            state: Arc::new(RwLock::new(st)),
            archive: None,
        })
    }

    /// 空归档（新增 zip 会话用）
    pub fn empty() -> Self {
        Self::default()
    }

    /// 从可写路径装载（路径须指向本地 .zip 文件）。
    /// 携带归档路径：trait 写方法成功后自动回写（write-through）。
    pub fn from_file(path: &std::path::Path) -> Result<Self, VfsError> {
        let data = std::fs::read(path).map_err(|_| VfsError::NotFound {
            path: path.display().to_string(),
        })?;
        let mut fs = Self::from_bytes_writable(&data)?;
        fs.archive = Some(Arc::new(path.to_path_buf()));
        Ok(fs)
    }

    /// 写回字节流（调用方负责落盘）
    pub fn to_bytes(&self) -> Vec<u8> {
        self.state.read().expect("zip 状态锁中毒").to_bytes()
    }

    /// write-through：装载自文件时，把当前内存模型回写归档
    fn persist(&self) {
        if let Some(path) = &self.archive {
            let bytes = self.to_bytes();
            if let Err(e) = std::fs::write(path.as_ref(), &bytes) {
                eprintln!("[zip] 归档回写失败（{}）: {e}", path.display());
            }
        }
    }
}

#[async_trait]
impl VfsProvider for ZipFsMut {
    fn scheme(&self) -> &'static str {
        "zip"
    }

    fn capabilities(&self) -> VfsCaps {
        VfsCaps::READ.union(VfsCaps::all_writes())
    }

    async fn stat(&self, path: &VfsPath) -> Result<StatInfo, VfsError> {
        let name = path.as_str().trim_matches('/').to_string();
        self.state
            .read()
            .expect("zip 状态锁中毒")
            .stat_of(&name)
            .ok_or_else(|| not_found(path))
    }

    async fn list_dir(&self, path: &VfsPath) -> Result<Vec<VfsDirEntry>, VfsError> {
        let st = self.state.read().expect("zip 状态锁中毒");
        let dir = path.as_str().trim_matches('/').to_string();
        if !st.dirs.contains_key(&dir) {
            return Err(not_found(path));
        }
        let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
        let mut out: Vec<VfsDirEntry> = Vec::new();
        let mut seen: Option<String> = None;
        for (name, _) in st.entries.range(prefix.clone()..) {
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
            if let Some(stat) = st.stat_of(&child) {
                out.push(VfsDirEntry { name: first.to_string(), stat });
            }
        }
        for (d, meta) in st.dirs.range(prefix.clone()..) {
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
            out.push(VfsDirEntry {
                name: first.to_string(),
                stat: StatInfo::minimal(0, meta.mtime_ms, EntryKind::Dir),
            });
        }
        Ok(out)
    }

    async fn read_file_bytes(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let name = path.as_str().trim_matches('/').to_string();
        let st = self.state.read().expect("zip 状态锁中毒");
        let e = st.entries.get(&name).ok_or_else(|| not_found(path))?;
        Ok(e.data.clone())
    }

    async fn stored_crc32(&self, path: &VfsPath) -> Result<Option<u32>, VfsError> {
        let name = path.as_str().trim_matches('/').to_string();
        Ok(self
            .state
            .read()
            .expect("zip 状态锁中毒")
            .entries
            .get(&name)
            .map(|e| e.crc32))
    }

    async fn write_file_bytes(&self, path: &VfsPath, data: &[u8]) -> Result<(), VfsError> {
        self.state
            .write()
            .expect("zip 状态锁中毒")
            .write_file(path, data)?;
        self.persist();
        Ok(())
    }

    async fn mkdir(&self, path: &VfsPath) -> Result<(), VfsError> {
        self.state.write().expect("zip 状态锁中毒").create_dir(path)?;
        self.persist();
        Ok(())
    }

    async fn rename(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.state.write().expect("zip 状态锁中毒").rename(from, to)?;
        self.persist();
        Ok(())
    }

    async fn remove(&self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.state
            .write()
            .expect("zip 状态锁中毒")
            .remove(path, recursive)?;
        self.persist();
        Ok(())
    }

    async fn copy_file(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.state.write().expect("zip 状态锁中毒").copy_file(from, to)?;
        self.persist();
        Ok(())
    }

    async fn set_mtime(&self, path: &VfsPath, mtime_ms: u64) -> Result<(), VfsError> {
        self.state
            .write()
            .expect("zip 状态锁中毒")
            .set_mtime(path, mtime_ms)?;
        self.persist();
        Ok(())
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

    #[test]
    fn epoch_dos_roundtrip_clamps() {
        // roundtrip：毫秒 -> DOS -> 毫秒（2 秒精度内一致）
        for ms in [315_532_800_000u64, 1_709_210_096_000, 1_799_999_999_999] {
            let (t, d) = epoch_ms_to_dos(ms);
            let back = dos_to_epoch_ms(t, d);
            assert!((back as i64 - ms as i64).abs() <= 2000, "{ms} -> {back}");
        }
        // 越界钳制：1970 -> 1980；超远未来 -> 2107 上限
        let (t, d) = epoch_ms_to_dos(0);
        assert_eq!(dos_to_epoch_ms(t, d), 315_532_800_000);
        let (t, d) = epoch_ms_to_dos(u64::MAX);
        assert!(dos_to_epoch_ms(t, d) <= 4_354_815_199_000 + 2000);
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
        // P12：只读 ZipFs 内容可读（stored 切片）
        let content = fs.read_file_bytes(&hello_path).await.unwrap();
        assert_eq!(content, b"hello world");
        let content2 = fs.read_file_bytes(&inner).await.unwrap();
        assert_eq!(content2, b"log line\n");
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
            assert!(!caps.has(cap), "只读 ZipFs 不应声明写能力位 {cap:?}");
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

    // ---------------- P12：ZipFsMut 可写 ----------------

    #[tokio::test]
    async fn mut_roundtrip_preserves_entries() {
        let src = build_zip(&[
            ("hello.txt", b"hello world"),
            ("sub/deep/x.bin", &[1, 2, 3, 4, 5]),
        ]);
        let fs = ZipFsMut::from_bytes_writable(&src).unwrap();
        let out = fs.to_bytes();
        // 回读：条目集合一致、内容一致（deflate 压缩体由只读 ZipFs 按需解压验证）
        let reparsed = ZipFs::from_bytes(&out).unwrap();
        let hello = VfsPath::parse("/hello.txt").unwrap();
        assert_eq!(reparsed.read_file_bytes(&hello).await.unwrap(), b"hello world");
        let x = VfsPath::parse("/sub/deep/x.bin").unwrap();
        assert_eq!(reparsed.read_file_bytes(&x).await.unwrap(), &[1, 2, 3, 4, 5]);
        // ZipFsMut 自身读取面
        assert_eq!(fs.read_file_bytes(&hello).await.unwrap(), b"hello world");
    }

    #[tokio::test]
    async fn mut_write_new_and_replace_updates_crc_size() {
        let src = build_zip(&[("a.txt", b"old")]);
        let fs = ZipFsMut::from_bytes_writable(&src).unwrap();
        let orig_mtime = fs.stat(&VfsPath::parse("/a.txt").unwrap()).await.unwrap().mtime_ms;
        fs.write_file_bytes(&VfsPath::parse("/new.txt").unwrap(), b"brand new content").await.unwrap();
        fs.write_file_bytes(&VfsPath::parse("/a.txt").unwrap(), b"newer").await.unwrap();
        let out = fs.to_bytes();
        let re = parse_central_directory(&out).unwrap();
        let a = re.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!(a.size, 5);
        assert_eq!(a.crc32, crc32(b"newer"));
        assert_eq!(a.mtime_ms, orig_mtime); // 替换保留原 mtime（对拍稳定语义）
        let n = re.iter().find(|e| e.name == "new.txt").unwrap();
        assert_eq!(n.size, 17);
        assert_eq!(n.crc32, crc32(b"brand new content"));
        // trait 读取面
        assert_eq!(fs.read_file_bytes(&VfsPath::parse("/new.txt").unwrap()).await.unwrap(), b"brand new content");
    }

    #[tokio::test]
    async fn mut_mkdir_and_empty_dir_entry() {
        let fs = ZipFsMut::empty();
        fs.mkdir(&VfsPath::parse("/docs/notes").unwrap()).await.unwrap();
        fs.write_file_bytes(&VfsPath::parse("/docs/notes/a.md").unwrap(), b"# t").await.unwrap();
        fs.mkdir(&VfsPath::parse("/empty").unwrap()).await.unwrap();
        let out = fs.to_bytes();
        let re = parse_central_directory(&out).unwrap();
        assert!(re.iter().any(|e| e.name == "docs/notes/a.md"));
        let empty = re.iter().find(|e| e.name == "empty").expect("空目录条目应写出");
        assert!(empty.is_dir);
        assert_eq!(empty.size, 0);
    }

    #[tokio::test]
    async fn mut_mkdir_conflicts() {
        let src = build_zip(&[("a.txt", b"x")]);
        let fs = ZipFsMut::from_bytes_writable(&src).unwrap();
        // 同名文件已存在
        assert!(matches!(
            fs.mkdir(&VfsPath::parse("/a.txt").unwrap()).await,
            Err(VfsError::AlreadyExists { .. })
        ));
        // 重复创建
        fs.mkdir(&VfsPath::parse("/d").unwrap()).await.unwrap();
        assert!(matches!(
            fs.mkdir(&VfsPath::parse("/d").unwrap()).await,
            Err(VfsError::AlreadyExists { .. })
        ));
        // 非法段（..）在 VfsPath 词法规范化层已被消解（/x/../y -> /y），
        // ZipState::norm_rel 的防御检查对内部构造的路径生效——此处验证
        // 词法规范化后的语义：mkdir 成功建立 /y
        fs.mkdir(&VfsPath::parse("/x/../y").unwrap()).await.unwrap();
        assert!(fs.stat(&VfsPath::parse("/y").unwrap()).await.is_ok());
        // 父为文件
        assert!(matches!(
            fs.mkdir(&VfsPath::parse("/a.txt/sub").unwrap()).await,
            Err(VfsError::NotADirectory { .. })
        ));
    }

    #[tokio::test]
    async fn mut_rename_file_and_dir_subtree() {
        let src = build_zip(&[("a.txt", b"A"), ("dir/b.txt", b"B"), ("dir/sub/c.txt", b"C")]);
        let fs = ZipFsMut::from_bytes_writable(&src).unwrap();
        // 文件改名
        fs.rename(&VfsPath::parse("/a.txt").unwrap(), &VfsPath::parse("/z.txt").unwrap()).await.unwrap();
        assert!(fs.stat(&VfsPath::parse("/a.txt").unwrap()).await.is_err());
        assert_eq!(fs.stat(&VfsPath::parse("/z.txt").unwrap()).await.unwrap().size, 1);
        // 目录子树移动
        fs.rename(&VfsPath::parse("/dir").unwrap(), &VfsPath::parse("/top").unwrap()).await.unwrap();
        assert!(fs.stat(&VfsPath::parse("/dir").unwrap()).await.is_err());
        assert_eq!(fs.stat(&VfsPath::parse("/top/b.txt").unwrap()).await.unwrap().size, 1);
        assert_eq!(fs.stat(&VfsPath::parse("/top/sub/c.txt").unwrap()).await.unwrap().size, 1);
        // 回写后一致
        let out = fs.to_bytes();
        let re = parse_central_directory(&out).unwrap();
        assert!(re.iter().any(|e| e.name == "top/sub/c.txt"));
        assert!(!re.iter().any(|e| e.name.starts_with("dir/")));
        // 目标已存在
        assert!(matches!(
            fs.rename(&VfsPath::parse("/z.txt").unwrap(), &VfsPath::parse("/top").unwrap()).await,
            Err(VfsError::AlreadyExists { .. })
        ));
        // 源不存在
        assert!(matches!(
            fs.rename(&VfsPath::parse("/nope").unwrap(), &VfsPath::parse("/x").unwrap()).await,
            Err(VfsError::NotFound { .. })
        ));
        // 目标在源子树内
        assert!(matches!(
            fs.rename(&VfsPath::parse("/top").unwrap(), &VfsPath::parse("/top/inner").unwrap()).await,
            Err(VfsError::InvalidPath(_))
        ));
    }

    #[tokio::test]
    async fn mut_remove_file_and_recursive_dir() {
        let src = build_zip(&[("a.txt", b"A"), ("d/b.txt", b"B"), ("d/c.txt", b"C")]);
        let fs = ZipFsMut::from_bytes_writable(&src).unwrap();
        // 非空目录非递归拒绝
        assert!(matches!(
            fs.remove(&VfsPath::parse("/d").unwrap(), false).await,
            Err(VfsError::InvalidPath(_))
        ));
        fs.remove(&VfsPath::parse("/d").unwrap(), true).await.unwrap();
        assert!(fs.stat(&VfsPath::parse("/d").unwrap()).await.is_err());
        assert!(fs.stat(&VfsPath::parse("/d/b.txt").unwrap()).await.is_err());
        assert!(fs.stat(&VfsPath::parse("/d/c.txt").unwrap()).await.is_err());
        fs.remove(&VfsPath::parse("/a.txt").unwrap(), false).await.unwrap();
        assert!(fs.stat(&VfsPath::parse("/a.txt").unwrap()).await.is_err());
        // 不存在
        assert!(matches!(
            fs.remove(&VfsPath::parse("/nope").unwrap(), true).await,
            Err(VfsError::NotFound { .. })
        ));
        // 全删后回写为空归档（EOCD 仍合法）
        let out = fs.to_bytes();
        assert!(parse_central_directory(&out).unwrap().is_empty());
    }

    #[tokio::test]
    async fn mut_copy_and_set_mtime() {
        let src = build_zip(&[("src.txt", b"payload")]);
        let fs = ZipFsMut::from_bytes_writable(&src).unwrap();
        let orig_mtime = fs.stat(&VfsPath::parse("/src.txt").unwrap()).await.unwrap().mtime_ms;
        fs.copy_file(
            &VfsPath::parse("/src.txt").unwrap(),
            &VfsPath::parse("/dst/deep.txt").unwrap(),
        )
        .await
        .unwrap();
        // 保留源 mtime + 父链自动建立
        assert_eq!(fs.stat(&VfsPath::parse("/dst/deep.txt").unwrap()).await.unwrap().mtime_ms, orig_mtime);
        assert_eq!(
            fs.read_file_bytes(&VfsPath::parse("/dst/deep.txt").unwrap()).await.unwrap(),
            b"payload"
        );
        fs.set_mtime(&VfsPath::parse("/src.txt").unwrap(), 1_709_210_096_000).await.unwrap();
        assert_eq!(fs.stat(&VfsPath::parse("/src.txt").unwrap()).await.unwrap().mtime_ms, 1_709_210_096_000);
        // set_mtime 到不存在路径
        assert!(matches!(
            fs.set_mtime(&VfsPath::parse("/nope").unwrap(), 0).await,
            Err(VfsError::NotFound { .. })
        ));
    }

    /// P12：deflate 条目读取（method 8）
    #[tokio::test]
    async fn readonly_reads_deflate_entries() {
        let payload = b"deflate me ".repeat(20); // 可压
        let compressed = miniz_oxide::deflate::compress_to_vec(&payload, 6);
        let crc = crc32(&payload);
        let name = "z.bin";
        let mut body = Vec::new();
        body.extend_from_slice(&SIG_LFH.to_le_bytes());
        body.extend_from_slice(&20u16.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&METHOD_DEFLATE.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&((1u16 << 5) | 1).to_le_bytes());
        body.extend_from_slice(&crc.to_le_bytes());
        body.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&(name.len() as u16).to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(&compressed);
        let lfh_offset = 0u32;
        let mut cd = Vec::new();
        cd.extend_from_slice(&SIG_CD.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&METHOD_DEFLATE.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&((1u16 << 5) | 1).to_le_bytes());
        cd.extend_from_slice(&crc.to_le_bytes());
        cd.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&(0o100644u32).to_le_bytes());
        cd.extend_from_slice(&lfh_offset.to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
        let mut out = body;
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&cd);
        out.extend_from_slice(&SIG_EOCD.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());

        let fs = ZipFs::from_bytes(&out).unwrap();
        let p = VfsPath::parse("/z.bin").unwrap();
        let got = fs.read_file_bytes(&p).await.unwrap();
        assert_eq!(got, payload);
        // 可写装载同路径
        let m = ZipFsMut::from_bytes_writable(&out).unwrap();
        assert_eq!(m.read_file_bytes(&p).await.unwrap(), payload);
        m.write_file_bytes(&p, b"rewritten").await.unwrap();
        let re = parse_central_directory(&m.to_bytes()).unwrap();
        let e = re.iter().find(|e| e.name == "z.bin").unwrap();
        assert_eq!(e.size, 9);
        assert_eq!(e.crc32, crc32(b"rewritten"));
    }

    /// P12：加密条目（flags bit0）报 Unsupported
    #[tokio::test]
    async fn encrypted_entry_is_unsupported() {
        let mut data = build_zip(&[("a.txt", b"x")]);
        // 结构：LFH 30B + 1B 数据 + CD 46B+5 + EOCD 22B → LFH@0, CD@31
        data[8] |= 1; // 本地头 flags bit0
        data[44] |= 1; // 中心目录 flags bit0（CD 起点 = LFH30+name5+data1 = 36，flags 在 CD 内偏移 8）
        let fs = ZipFs::from_bytes(&data).unwrap();
        let p = VfsPath::parse("/a.txt").unwrap();
        let err = fs.read_file_bytes(&p).await.unwrap_err();
        assert!(matches!(err, VfsError::Unsupported(_)), "实际: {err:?}");
        assert!(ZipFsMut::from_bytes_writable(&data).is_err());
    }

    /// P12：trait &self 写路径（RwLock 内部可变性）端到端
    #[tokio::test]
    async fn mut_trait_write_path_end_to_end() {
        let fs = ZipFsMut::empty();
        fs.write_file_bytes(&VfsPath::parse("/x/y.txt").unwrap(), b"trait path").await.unwrap();
        assert_eq!(fs.capabilities(), VfsCaps::READ.union(VfsCaps::all_writes()));
        let st = fs.stat(&VfsPath::parse("/x/y.txt").unwrap()).await.unwrap();
        assert_eq!(st.size, 10);
        // to_bytes 与写入面一致
        let re = parse_central_directory(&fs.to_bytes()).unwrap();
        assert!(re.iter().any(|e| e.name == "x/y.txt"));
    }
}
