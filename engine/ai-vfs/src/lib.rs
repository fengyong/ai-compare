//! # ai-vfs（M04）：虚拟文件系统抽象
//!
//! L1 唯一做 IO 的层（docs/03 §2.2）。P0 本地只读实现 [`LocalFs`]；
//! P6 扩展写路径（copy/move/rename/delete/mkdir/touch，FR-DIR-060~065），
//! 仍按能力位（[`VfsCaps`]）协商降级：zip 等只读 Provider 的写方法走
//! trait 默认实现返回 [`VfsError::Unsupported`]。
//!
//! 设计对齐 docs/03 §3.1.4：`list / stat / open-read` 为核心读路径；
//! 写路径（write/copy/move/delete/mkdir/touch）由 caps 门控。

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use thiserror::Error;

pub mod crc32;
pub mod local;
pub mod zip;

pub use crc32::{crc32, crc32_update};
pub use crc32::INITIAL as CRC32_INITIAL;
pub use local::LocalFs;
pub use zip::{ZipEntry, ZipFs};

/// VFS 错误（引擎上层映射为协议领域错误码）
#[derive(Debug, Error)]
pub enum VfsError {
    #[error("路径不存在: {path}")]
    NotFound { path: String },
    #[error("目标已存在: {path}")]
    AlreadyExists { path: String },
    #[error("不是目录: {path}")]
    NotADirectory { path: String },
    #[error("不是文件: {path}")]
    NotAFile { path: String },
    #[error("非法路径: {0}")]
    InvalidPath(String),
    #[error("文件不是合法 UTF-8 文本: {path}")]
    InvalidEncoding { path: String },
    #[error("操作被取消")]
    Cancelled,
    #[error("不支持的操作: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 条目类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
}

/// DOS 属性位（FR-DIR-010 属性比较；经典 DOS 位分配）
pub mod dos_attr {
    pub const READONLY: u8 = 0x01;
    pub const HIDDEN: u8 = 0x02;
    pub const SYSTEM: u8 = 0x04;
    pub const ARCHIVE: u8 = 0x20;

    /// 位 -> BC 属性字母（FR-DIR-041：r 只读、h 隐藏、s 系统、a 存档）
    pub fn letter(bit: u8) -> char {
        match bit {
            READONLY => 'r',
            HIDDEN => 'h',
            SYSTEM => 's',
            ARCHIVE => 'a',
            _ => '?',
        }
    }

    /// 属性位串 -> 位掩码（未知字母忽略）
    pub fn from_letters(letters: &str) -> u8 {
        let mut m = 0u8;
        for c in letters.chars() {
            match c {
                'r' | 'R' => m |= READONLY,
                'h' | 'H' => m |= HIDDEN,
                's' | 'S' => m |= SYSTEM,
                'a' | 'A' => m |= ARCHIVE,
                _ => {}
            }
        }
        m
    }

    /// 位掩码 -> 属性字母串（稳定顺序 r/h/s/a）
    pub fn to_letters(mask: u8) -> String {
        let mut s = String::new();
        for bit in [READONLY, HIDDEN, SYSTEM, ARCHIVE] {
            if mask & bit != 0 {
                s.push(letter(bit));
            }
        }
        s
    }
}

/// Unix 文件类型字母（FR-DIR-032 Unix 类型过滤：b/c/l/p/r/s）
pub fn unix_type_letter(kind: EntryKind, mode: Option<u32>) -> char {
    match kind {
        EntryKind::Symlink => 'l',
        EntryKind::Dir => 'd',
        EntryKind::File => match mode {
            Some(m) => match m & 0o170000 {
                0o060000 => 'b', // 块设备
                0o020000 => 'c', // 字符设备
                0o010000 => 'p', // FIFO
                0o140000 => 's', // 套接字
                _ => 'r',        // 普通文件
            },
            None => 'r',
        },
    }
}

/// stat 信息（mtime 为 epoch 毫秒；mtime 不可得时为 0）
///
/// P2 扩展：Unix 元数据（权限/uid/gid，非 Unix 平台为 None）与
/// DOS 属性位（Unix 平台近似：无写位 -> r；`.` 前缀 -> h）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatInfo {
    pub size: u64,
    pub mtime_ms: u64,
    pub kind: EntryKind,
    /// st_mode（含文件类型位）；不可得为 None
    pub unix_mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    /// DOS 属性位掩码（dos_attr 模块位）
    pub dos_attrs: u8,
}

impl StatInfo {
    /// 最小构造（size/mtime/kind；元数据置空）
    pub fn minimal(size: u64, mtime_ms: u64, kind: EntryKind) -> Self {
        StatInfo { size, mtime_ms, kind, unix_mode: None, uid: None, gid: None, dos_attrs: 0 }
    }
}

/// 目录项（list_dir 返回）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VfsDirEntry {
    pub name: String,
    pub stat: StatInfo,
}

/// Provider 能力位（读路径必备；写/监听/随机访问等按实现降级）
///
/// P6 细分写能力（FR-DIR-060~065 文件操作按能力降级）：
/// - [`VfsCaps::WRITE`]：文件内容写入（write_file_bytes）；
/// - [`VfsCaps::MKDIR`]：目录创建（mkdir / mkdir_all）；
/// - [`VfsCaps::DELETE`]：删除（remove 文件/目录树）；
/// - [`VfsCaps::MOVE`]：改名/移动（rename，同 Provider 内原子）；
/// - [`VfsCaps::COPY`]：复制（copy_file；缺省可走 read+write 跨 Provider 组合）；
/// - [`VfsCaps::TOUCH`]：修改时间戳（set_mtime，FR-DIR-065 touch）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VfsCaps(u16);

impl VfsCaps {
    pub const READ: VfsCaps = VfsCaps(1);
    pub const WRITE: VfsCaps = VfsCaps(1 << 1);
    pub const RANDOM_ACCESS: VfsCaps = VfsCaps(1 << 2);
    pub const WATCH: VfsCaps = VfsCaps(1 << 3);
    pub const MKDIR: VfsCaps = VfsCaps(1 << 4);
    pub const DELETE: VfsCaps = VfsCaps(1 << 5);
    pub const MOVE: VfsCaps = VfsCaps(1 << 6);
    pub const COPY: VfsCaps = VfsCaps(1 << 7);
    pub const TOUCH: VfsCaps = VfsCaps(1 << 8);

    pub const fn union(self, other: VfsCaps) -> VfsCaps {
        VfsCaps(self.0 | other.0)
    }
    pub const fn has(self, cap: VfsCaps) -> bool {
        self.0 & cap.0 == cap.0
    }
    /// 全部写能力（本地文件系统满配）
    pub const fn all_writes() -> VfsCaps {
        VfsCaps(Self::WRITE.0 | Self::MKDIR.0 | Self::DELETE.0 | Self::MOVE.0 | Self::COPY.0 | Self::TOUCH.0)
    }
}

/// 虚拟路径：词法规范化（POSIX 风格分隔符，`.`/`..` 已解析，无重复分隔符）
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VfsPath {
    inner: String,
}

impl VfsPath {
    /// 解析并规范化路径字符串。接受 `/` 与 `\` 分隔；相对路径由
    /// [`LocalFs`] 解析到 cwd。Windows 盘符前缀（`C:/...`）保留。
    pub fn parse(s: &str) -> Result<Self, VfsError> {
        let raw = s.replace('\\', "/");
        let is_abs = raw.starts_with('/');
        let has_drive = raw.len() >= 2 && raw.as_bytes()[1] == b':' && raw.as_bytes()[0].is_ascii_alphabetic();
        if raw.trim().is_empty() {
            return Err(VfsError::InvalidPath("空路径".into()));
        }
        let mut parts: Vec<&str> = Vec::new();
        for seg in raw.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    if let Some(last) = parts.last() {
                        if *last != ".." {
                            parts.pop();
                            continue;
                        }
                    }
                    if is_abs || has_drive {
                        return Err(VfsError::InvalidPath(format!("越界路径: {s}")));
                    }
                    parts.push("..");
                }
                other => parts.push(other),
            }
        }
        let joined = parts.join("/");
        let inner = if is_abs {
            format!("/{joined}")
        } else if has_drive {
            let rest = parts[1..].join("/");
            format!("{}/{rest}", parts[0])
        } else if joined.is_empty() {
            ".".to_string()
        } else {
            joined
        };
        Ok(VfsPath { inner })
    }

    pub fn as_str(&self) -> &str {
        &self.inner
    }

    pub fn is_absolute(&self) -> bool {
        self.inner.starts_with('/')
    }

    /// 词法拼接子路径（不触碰文件系统）
    pub fn join(&self, name: &str) -> VfsPath {
        if name.is_empty() {
            return self.clone();
        }
        // 父路径 + 规范化的子段（子段按单段解析）
        let child = name.replace('\\', "/");
        let child = child.trim_matches('/');
        VfsPath { inner: format!("{}/{}", self.inner.trim_end_matches('/'), child) }
    }

    /// 文件名段（最后一段）
    pub fn file_name(&self) -> Option<&str> {
        self.inner.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty())
    }
}

impl fmt::Display for VfsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.inner)
    }
}

impl From<&str> for VfsPath {
    fn from(s: &str) -> Self {
        VfsPath::parse(s).unwrap_or_else(|_| VfsPath { inner: s.to_string() })
    }
}

/// 统一异步 VFS 接口（docs/03 §3.1.4；读路径 + P6 写路径）
///
/// 实现约定：
/// - 所有方法允许内部使用阻塞 IO（本地盘可接受；远程实现必须真异步）；
/// - `list_dir` 返回的条目顺序由实现决定，消费方（ai-dir）负责排序；
/// - 符号链接默认不跟随（stat 用 lstat 语义）；
/// - 写方法按 [`VfsCaps`] 能力协商：未声明能力的实现走默认实现返回
///   [`VfsError::Unsupported`]（只读 Provider 如 zip 无需任何代码）；
/// - `remove` 对符号链接只删除链接本身，绝不跟随（逃逸防护，P6）。
#[async_trait]
pub trait VfsProvider: Send + Sync {
    /// scheme 标识：`file` / `zip` / `ftp` / `profile` / ...
    fn scheme(&self) -> &'static str;

    fn capabilities(&self) -> VfsCaps;

    async fn stat(&self, path: &VfsPath) -> Result<StatInfo, VfsError>;

    async fn list_dir(&self, path: &VfsPath) -> Result<Vec<VfsDirEntry>, VfsError>;

    /// 读取整个文件字节（P0 全量读；流式/range 读在 P1+ 引入）
    async fn read_file_bytes(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError>;

    /// 读取文件 [offset, offset+len) 字节；EOF 处允许短读。
    /// 默认实现委托全量读（内存换兼容）；本地实现用 seek（大文件友好）。
    async fn read_range(&self, path: &VfsPath, offset: u64, len: usize) -> Result<Vec<u8>, VfsError> {
        let all = self.read_file_bytes(path).await?;
        let start = offset.min(all.len() as u64) as usize;
        let end = start.saturating_add(len).min(all.len());
        Ok(all[start..end].to_vec())
    }

    /// 读取并解码为文本：UTF-8 / UTF-8 BOM（BOM 剥离）；P0 不做其他编码检测
    async fn read_file_text(&self, path: &VfsPath) -> Result<String, VfsError> {
        let bytes = self.read_file_bytes(path).await?;
        let stripped = strip_bom(&bytes);
        String::from_utf8(stripped.to_vec()).map_err(|_| VfsError::InvalidEncoding {
            path: path.as_str().to_string(),
        })
    }

    /// 预计算 CRC-32（IEEE）：提供中心目录等预计算元数据的 Provider
    /// （如 zip）直接返回存储值；默认 None = 需读取内容计算。
    /// （FR-DIR-011 CRC 比较：zip 内文件免读判定）
    async fn stored_crc32(&self, _path: &VfsPath) -> Result<Option<u32>, VfsError> {
        Ok(None)
    }

    // ------------------------------------------------------------------
    // 写路径（P6，FR-DIR-060~065）：默认 Unsupported（能力位门控）
    // ------------------------------------------------------------------

    /// 写入整个文件字节（覆盖语义；[`VfsCaps::WRITE`]）
    async fn write_file_bytes(&self, path: &VfsPath, _data: &[u8]) -> Result<(), VfsError> {
        let _ = path;
        Err(VfsError::Unsupported(format!("{}: 写文件", self.scheme())))
    }

    /// 创建目录（父目录必须已存在；[`VfsCaps::MKDIR`]）
    async fn mkdir(&self, path: &VfsPath) -> Result<(), VfsError> {
        let _ = path;
        Err(VfsError::Unsupported(format!("{}: 创建目录", self.scheme())))
    }

    /// 逐级创建目录链（`mkdir -p` 语义：幂等，已存在的目录/目录符号链接跳过；
    /// [`VfsCaps::MKDIR`]）。中间路径含同名文件时报 AlreadyExists。
    async fn mkdir_all(&self, path: &VfsPath) -> Result<(), VfsError> {
        let s = path.as_str();
        let has_drive = s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic();
        let mut segments: Vec<&str> = s.split('/').filter(|seg| !seg.is_empty()).collect();
        let mut cur = if path.is_absolute() {
            VfsPath::parse("/").expect("/ 可解析")
        } else if has_drive {
            // 盘符路径：从 `C:` 根起逐段展开
            let root = segments.remove(0);
            VfsPath::parse(root).expect("盘符根可解析")
        } else {
            VfsPath::parse(".").expect(". 可解析")
        };
        for seg in segments {
            cur = cur.join(seg);
            match self.mkdir(&cur).await {
                Ok(()) => {}
                // 已存在（目录或指向目录的链接）继续下一段（mkdir -p 幂等语义）
                Err(VfsError::AlreadyExists { .. }) => {}
                Err(VfsError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                // 父路径被同名文件占据（POSIX ENOTDIR）：归类为 AlreadyExists 冲突
                Err(VfsError::Io(e)) if e.raw_os_error() == Some(ENOTDIR) => {
                    return Err(VfsError::AlreadyExists { path: cur.as_str().to_string() })
                }
                Err(e) => return Err(e),
            }
        }
        // 终态校验：最终路径必须是目录（lstat 为符号链接时视为目录入口）
        match self.stat(&cur).await? {
            st if st.kind == EntryKind::Dir => Ok(()),
            st if st.kind == EntryKind::Symlink => Ok(()),
            _ => Err(VfsError::AlreadyExists { path: cur.as_str().to_string() }),
        }
    }

    /// 同 Provider 内改名/移动（原子尽力；目标已存在时覆盖；[`VfsCaps::MOVE`]）
    async fn rename(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let _ = (from, to);
        Err(VfsError::Unsupported(format!("{}: 改名/移动", self.scheme())))
    }

    /// 删除文件或目录（`recursive=true` 删目录树；符号链接只删链接本身；
    /// [`VfsCaps::DELETE`]）
    async fn remove(&self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        let _ = (path, recursive);
        Err(VfsError::Unsupported(format!("{}: 删除", self.scheme())))
    }

    /// 复制单个文件（覆盖语义；[`VfsCaps::COPY`]）
    async fn copy_file(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let _ = (from, to);
        Err(VfsError::Unsupported(format!("{}: 复制文件", self.scheme())))
    }

    /// 修改时间戳（epoch 毫秒；FR-DIR-065 touch；[`VfsCaps::TOUCH`]）
    async fn set_mtime(&self, path: &VfsPath, _mtime_ms: u64) -> Result<(), VfsError> {
        let _ = path;
        Err(VfsError::Unsupported(format!("{}: 修改时间戳", self.scheme())))
    }
}

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// POSIX ENOTDIR（路径中间段被同名文件占据；mkdir_all 冲突归类用）
const ENOTDIR: i32 = 20;

pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes)
}

fn system_time_to_ms(t: std::io::Result<SystemTime>) -> u64 {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

pub(crate) fn stat_from_metadata(meta: &std::fs::Metadata) -> StatInfo {
    let kind = if meta.is_dir() {
        EntryKind::Dir
    } else if meta.is_file() {
        EntryKind::File
    } else {
        EntryKind::Symlink
    };
    stat_with_unix(meta, kind)
}

#[cfg(unix)]
fn stat_with_unix(meta: &std::fs::Metadata, kind: EntryKind) -> StatInfo {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let mode = meta.permissions().mode();
    let mut dos_attrs = 0u8;
    if mode & 0o222 == 0 {
        dos_attrs |= dos_attr::READONLY;
    }
    StatInfo {
        size: meta.len(),
        mtime_ms: system_time_to_ms(meta.modified()),
        kind,
        unix_mode: Some(mode),
        uid: Some(meta.uid()),
        gid: Some(meta.gid()),
        dos_attrs,
    }
}

#[cfg(not(unix))]
fn stat_with_unix(meta: &std::fs::Metadata, kind: EntryKind) -> StatInfo {
    StatInfo {
        size: meta.len(),
        mtime_ms: system_time_to_ms(meta.modified()),
        kind,
        unix_mode: None,
        uid: None,
        gid: None,
        dos_attrs: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_parse_normalization() {
        assert_eq!(VfsPath::parse("/a/b/c").unwrap().as_str(), "/a/b/c");
        assert_eq!(VfsPath::parse("/a//b/./c/").unwrap().as_str(), "/a/b/c");
        assert_eq!(VfsPath::parse("/a/b/../c").unwrap().as_str(), "/a/c");
        assert_eq!(VfsPath::parse("a/b").unwrap().as_str(), "a/b");
        assert_eq!(VfsPath::parse("../a").unwrap().as_str(), "../a");
        assert_eq!(VfsPath::parse("C:/x/y").unwrap().as_str(), "C:/x/y");
        assert_eq!(VfsPath::parse("a\\b\\c").unwrap().as_str(), "a/b/c");
        assert!(VfsPath::parse("").is_err());
        assert!(VfsPath::parse("/../a").is_err(), "绝对路径越界应报错");
    }

    #[test]
    fn path_join_and_file_name() {
        let p = VfsPath::parse("/base").unwrap();
        assert_eq!(p.join("child").as_str(), "/base/child");
        assert_eq!(p.join("child").file_name(), Some("child"));
        assert_eq!(VfsPath::parse("/base/x.txt").unwrap().file_name(), Some("x.txt"));
    }

    #[test]
    fn caps_and_bom() {
        let caps = VfsCaps::READ.union(VfsCaps::WATCH);
        assert!(caps.has(VfsCaps::READ));
        assert!(!caps.has(VfsCaps::WRITE));
        assert_eq!(strip_bom(&[0xEF, 0xBB, 0xBF, b'a']), b"a");
        assert_eq!(strip_bom(b"ab"), b"ab");
    }
}
