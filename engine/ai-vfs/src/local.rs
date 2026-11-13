//! 本地文件系统 Provider（P0 只读）。
//!
//! 约定：`stat`/`list_dir` 对符号链接取 lstat 语义（不跟随）；
//! `read_file_*` 打开最终目标（跟随）。

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::{
    dos_attr, stat_from_metadata, EntryKind, VfsCaps, VfsDirEntry, VfsError, VfsPath, VfsProvider,
    StatInfo,
};

/// 本地只读 Provider
#[derive(Debug, Clone, Default)]
pub struct LocalFs {
    /// 相对路径的解析基准（缺省为进程 cwd；测试可注入）
    base: Option<PathBuf>,
}

impl LocalFs {
    pub fn new() -> Self {
        LocalFs { base: None }
    }

    /// 以指定根解析相对路径
    pub fn with_base(base: impl Into<PathBuf>) -> Self {
        LocalFs { base: Some(base.into()) }
    }

    /// VfsPath -> 文件系统路径（相对路径补 cwd/base）
    pub fn to_fs_path(&self, path: &VfsPath) -> PathBuf {
        let p = Path::new(path.as_str());
        if path.is_absolute() {
            p.to_path_buf()
        } else {
            match &self.base {
                Some(base) => base.join(p),
                None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(p),
            }
        }
    }

    fn map_io(&self, path: &VfsPath, err: std::io::Error) -> VfsError {
        match err.kind() {
            std::io::ErrorKind::NotFound => VfsError::NotFound { path: path.as_str().to_string() },
            _ => VfsError::Io(err),
        }
    }

    /// 附加平台近似属性：`.` 前缀文件视为 DOS hidden（FR-DIR-041 Unix 近似）
    fn apply_name_attrs(&self, path: &VfsPath, mut stat: StatInfo) -> StatInfo {
        if let Some(name) = path.file_name() {
            if name.starts_with('.') {
                stat.dos_attrs |= dos_attr::HIDDEN;
            }
        }
        stat
    }
}

fn is_not_found(err: &std::io::Error) -> bool {
    matches!(err.kind(), std::io::ErrorKind::NotFound)
}

#[async_trait]
impl VfsProvider for LocalFs {
    fn scheme(&self) -> &'static str {
        "file"
    }

    fn capabilities(&self) -> VfsCaps {
        VfsCaps::READ.union(VfsCaps::RANDOM_ACCESS).union(VfsCaps::WRITE)
    }

    async fn stat(&self, path: &VfsPath) -> Result<StatInfo, VfsError> {
        let fs_path = self.to_fs_path(path);
        // lstat 语义：不跟随符号链接
        let meta = tokio::fs::symlink_metadata(&fs_path)
            .await
            .map_err(|e| self.map_io(path, e))?;
        Ok(self.apply_name_attrs(path, stat_from_metadata(&meta)))
    }

    async fn list_dir(&self, path: &VfsPath) -> Result<Vec<VfsDirEntry>, VfsError> {
        let fs_path = self.to_fs_path(path);
        let meta = tokio::fs::symlink_metadata(&fs_path)
            .await
            .map_err(|e| self.map_io(path, e))?;
        if !meta.is_dir() {
            return Err(VfsError::NotADirectory { path: path.as_str().to_string() });
        }
        let mut rd = tokio::fs::read_dir(&fs_path).await.map_err(|e| self.map_io(path, e))?;
        let mut out = Vec::new();
        while let Some(entry) = rd.next_entry().await.map_err(VfsError::Io)? {
            let ft = entry.file_type().await.map_err(VfsError::Io)?;
            let kind = if ft.is_dir() {
                EntryKind::Dir
            } else if ft.is_file() {
                EntryKind::File
            } else {
                EntryKind::Symlink
            };
            // 子项元数据：文件/目录取 lstat；断链 symlink 元数据不可得时降级为 size 0
            let stat = if ft.is_symlink() {
                match tokio::fs::symlink_metadata(entry.path()).await {
                    Ok(m) => stat_from_metadata(&m),
                    Err(e) if is_not_found(&e) => {
                        StatInfo::minimal(0, 0, kind)
                    }
                    Err(e) => return Err(VfsError::Io(e)),
                }
            } else {
                match entry.metadata().await {
                    Ok(m) => stat_from_metadata(&m),
                    Err(e) => return Err(VfsError::Io(e)),
                }
            };
            let mut stat = StatInfo { kind, ..stat };
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                stat.dos_attrs |= dos_attr::HIDDEN;
            }
            out.push(VfsDirEntry { name, stat });
        }
        Ok(out)
    }

    async fn read_file_bytes(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let fs_path = self.to_fs_path(path);
        let meta = tokio::fs::symlink_metadata(&fs_path)
            .await
            .map_err(|e| self.map_io(path, e))?;
        if meta.is_dir() {
            return Err(VfsError::NotAFile { path: path.as_str().to_string() });
        }
        tokio::fs::read(&fs_path).await.map_err(|e| self.map_io(path, e))
    }

    async fn read_range(&self, path: &VfsPath, offset: u64, len: usize) -> Result<Vec<u8>, VfsError> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let fs_path = self.to_fs_path(path);
        let mut file = tokio::fs::File::open(&fs_path)
            .await
            .map_err(|e| self.map_io(path, e))?;
        if offset > 0 {
            file.seek(std::io::SeekFrom::Start(offset)).await.map_err(VfsError::Io)?;
        }
        let mut buf = vec![0u8; len];
        let mut filled = 0usize;
        while filled < len {
            let n = file.read(&mut buf[filled..]).await.map_err(VfsError::Io)?;
            if n == 0 {
                break; // EOF 短读
            }
            filled += n;
        }
        buf.truncate(filled);
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (tempfile::TempDir, VfsPath) {
        let dir = tempfile::tempdir().expect("临时目录");
        let root = VfsPath::parse(dir.path().to_str().unwrap()).unwrap();
        tokio::fs::write(dir.path().join("hello.txt"), "héllo\r\nworld\n").await.unwrap();
        tokio::fs::create_dir(dir.path().join("sub")).await.unwrap();
        tokio::fs::write(dir.path().join("sub").join("bom.txt"), "\u{feff}bom content").await.unwrap();
        let _ = tokio::fs::write(dir.path().join("bad.bin"), [0xff, 0xfe, 0x01]).await;
        (dir, root)
    }

    #[tokio::test]
    async fn stat_and_list() {
        let (dir, root) = setup().await;
        let fs = LocalFs::new();
        let st = fs.stat(&root).await.unwrap();
        assert_eq!(st.kind, EntryKind::Dir);

        let mut entries = fs.list_dir(&root).await.unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["bad.bin", "hello.txt", "sub"]);
        let hello = entries.iter().find(|e| e.name == "hello.txt").unwrap();
        assert_eq!(hello.stat.kind, EntryKind::File);
        assert_eq!(hello.stat.size, "héllo\r\nworld\n".len() as u64);

        // 非目录 list 报错
        let file = root.join("hello.txt");
        assert!(matches!(fs.list_dir(&file).await, Err(VfsError::NotADirectory { .. })));
        // 不存在
        let missing = root.join("nope");
        assert!(matches!(fs.stat(&missing).await, Err(VfsError::NotFound { .. })));
        drop(dir);
    }

    #[tokio::test]
    async fn read_text_utf8_and_bom() {
        let (dir, root) = setup().await;
        let fs = LocalFs::new();
        let text = fs.read_file_text(&root.join("hello.txt")).await.unwrap();
        assert_eq!(text, "héllo\r\nworld\n");

        let bom = fs.read_file_text(&root.join("sub").join("bom.txt")).await.unwrap();
        assert_eq!(bom, "bom content", "UTF-8 BOM 应被剥离");

        let err = fs.read_file_text(&root.join("bad.bin")).await.unwrap_err();
        assert!(matches!(err, VfsError::InvalidEncoding { .. }));
        drop(dir);
    }

    #[tokio::test]
    async fn relative_path_with_base() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.txt"), "X").unwrap();
        let fs = LocalFs::with_base(dir.path());
        let p = VfsPath::parse("x.txt").unwrap();
        assert_eq!(fs.read_file_bytes(&p).await.unwrap(), b"X");
        drop(dir);
    }
}
