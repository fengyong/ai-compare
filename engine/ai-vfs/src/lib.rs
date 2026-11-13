//! # ai-vfs（M04）：虚拟文件系统抽象
//!
//! L1 唯一做 IO 的层（docs/03 §2.2）。P0 仅本地只读实现 [`LocalFs`]；
//! trait 面向能力协商（[`VfsCaps`]）设计，压缩包（libarchive）/快照/.aicss/
//! 剪贴板/FTP/rclone sidecar 等后续 Provider 按能力降级实现同一接口。
//!
//! 设计对齐 docs/03 §3.1.4：`list / stat / open-read` 为核心读路径；
//! 写路径（write/create/delete/rename/watch/attrib/touch）由 caps 门控，
//! 在后续阶段引入（只加不改）。

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use thiserror::Error;

pub mod local;

pub use local::LocalFs;

/// VFS 错误（引擎上层映射为协议领域错误码）
#[derive(Debug, Error)]
pub enum VfsError {
    #[error("路径不存在: {path}")]
    NotFound { path: String },
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

/// stat 信息（mtime 为 epoch 毫秒；mtime 不可得时为 0）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatInfo {
    pub size: u64,
    pub mtime_ms: u64,
    pub kind: EntryKind,
}

/// 目录项（list_dir 返回）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VfsDirEntry {
    pub name: String,
    pub stat: StatInfo,
}

/// Provider 能力位（读路径必备；写/监听/随机访问等按实现降级）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VfsCaps(u8);

impl VfsCaps {
    pub const READ: VfsCaps = VfsCaps(1);
    pub const WRITE: VfsCaps = VfsCaps(1 << 1);
    pub const RANDOM_ACCESS: VfsCaps = VfsCaps(1 << 2);
    pub const WATCH: VfsCaps = VfsCaps(1 << 3);

    pub const fn union(self, other: VfsCaps) -> VfsCaps {
        VfsCaps(self.0 | other.0)
    }
    pub const fn has(self, cap: VfsCaps) -> bool {
        self.0 & cap.0 == cap.0
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

/// 统一异步 VFS 接口（docs/03 §3.1.4；P0 只读子集）
///
/// 实现约定：
/// - 所有方法允许内部使用阻塞 IO（本地盘可接受；远程实现必须真异步）；
/// - `list_dir` 返回的条目顺序由实现决定，消费方（ai-dir）负责排序；
/// - 符号链接默认不跟随（stat 用 lstat 语义）。
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
}

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

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
    StatInfo {
        size: meta.len(),
        mtime_ms: system_time_to_ms(meta.modified()),
        kind,
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
