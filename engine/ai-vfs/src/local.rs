//! 本地文件系统 Provider（P0 只读 + P6 写路径）。
//!
//! 约定：`stat`/`list_dir` 对符号链接取 lstat 语义（不跟随）；
//! `read_file_*` 打开最终目标（跟随）；
//! 写路径（P6，FR-DIR-060~065）：copy/move/rename/delete/mkdir/touch，
//! 删除对符号链接只删链接本身（逃逸防护），copy_file 尽力保留源 mtime。

use std::future::Future;
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::{
    dos_attr, stat_from_metadata, EntryKind, VfsCaps, VfsDirEntry, VfsError, VfsPath, VfsProvider,
    StatInfo,
};

/// io::ErrorKind -> VfsError（写路径错误分类：NotFound / AlreadyExists / 其余 Io）
fn map_write_io(path: &VfsPath, err: std::io::Error) -> VfsError {
    match err.kind() {
        std::io::ErrorKind::NotFound => VfsError::NotFound { path: path.as_str().to_string() },
        std::io::ErrorKind::AlreadyExists => VfsError::AlreadyExists { path: path.as_str().to_string() },
        _ => VfsError::Io(err),
    }
}

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
        VfsCaps::READ
            .union(VfsCaps::RANDOM_ACCESS)
            .union(VfsCaps::WRITE)
            .union(VfsCaps::all_writes())
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

    // ------------------------------------------------------------------
    // 写路径（P6，FR-DIR-060~065）
    // ------------------------------------------------------------------

    async fn write_file_bytes(&self, path: &VfsPath, data: &[u8]) -> Result<(), VfsError> {
        let fs_path = self.to_fs_path(path);
        tokio::fs::write(&fs_path, data).await.map_err(|e| map_write_io(path, e))
    }

    async fn mkdir(&self, path: &VfsPath) -> Result<(), VfsError> {
        let fs_path = self.to_fs_path(path);
        tokio::fs::create_dir(&fs_path).await.map_err(|e| map_write_io(path, e))
    }

    async fn rename(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let from_fs = self.to_fs_path(from);
        let to_fs = self.to_fs_path(to);
        tokio::fs::rename(&from_fs, &to_fs)
            .await
            .map_err(|e| map_write_io(from, e))
    }

    async fn remove(&self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        let fs_path = self.to_fs_path(path);
        // lstat 语义：符号链接只删链接本身，绝不跟随（逃逸防护）
        let meta = tokio::fs::symlink_metadata(&fs_path)
            .await
            .map_err(|e| map_write_io(path, e))?;
        if meta.file_type().is_symlink() {
            return tokio::fs::remove_file(&fs_path).await.map_err(|e| map_write_io(path, e));
        }
        if meta.is_dir() {
            return if recursive {
                remove_dir_tree(&fs_path).await.map_err(|e| map_write_io(path, e))
            } else {
                tokio::fs::remove_dir(&fs_path).await.map_err(|e| map_write_io(path, e))
            };
        }
        tokio::fs::remove_file(&fs_path).await.map_err(|e| map_write_io(path, e))
    }

    async fn copy_file(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let from_fs = self.to_fs_path(from);
        let to_fs = self.to_fs_path(to);
        // 源 lstat：复制目标是符号链接时复制链接指向的内容（读跟随语义）
        let src_meta = tokio::fs::metadata(&from_fs).await.map_err(|e| map_write_io(from, e))?;
        if src_meta.is_dir() {
            return Err(VfsError::NotAFile { path: from.as_str().to_string() });
        }
        tokio::fs::copy(&from_fs, &to_fs)
            .await
            .map_err(|e| map_write_io(to, e))?;
        // 尽力保留源 mtime（BC 复制语义；失败不影响复制结果）
        if let Ok(st) = self.stat(from).await {
            let _ = self.set_mtime(to, st.mtime_ms).await;
        }
        Ok(())
    }

    async fn set_mtime(&self, path: &VfsPath, mtime_ms: u64) -> Result<(), VfsError> {
        let fs_path = self.to_fs_path(path);
        // std 同步打开（约定允许阻塞 IO；Rust 1.75+ File::set_times）
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&fs_path)
            .map_err(|e| map_write_io(path, e))?;
        let times = std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(mtime_ms));
        file.set_times(times).map_err(VfsError::Io)
    }
}

/// 递归删除目录树（符号链接不跟随：链接项一律 remove_file）
fn remove_dir_tree<'a>(root: &'a Path) -> std::pin::Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + 'a>> {
    Box::pin(async move { remove_dir_tree_inner(root).await })
}

async fn remove_dir_tree_inner(root: &Path) -> std::io::Result<()> {
    let meta = tokio::fs::symlink_metadata(root).await?;
    if meta.file_type().is_symlink() {
        return tokio::fs::remove_file(root).await;
    }
    if !meta.is_dir() {
        return tokio::fs::remove_file(root).await;
    }
    let mut rd = tokio::fs::read_dir(root).await?;
    while let Some(entry) = rd.next_entry().await? {
        let child = entry.path();
        let ft = entry.file_type().await?;
        if ft.is_dir() {
            remove_dir_tree(&child).await?;
        } else {
            // 文件与符号链接统一删链接/文件本身
            tokio::fs::remove_file(&child).await?;
        }
    }
    tokio::fs::remove_dir(root).await
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

    // ---------------- P6 写路径（FR-DIR-060~065） ----------------

    fn write_caps() -> [VfsCaps; 6] {
        [
            VfsCaps::WRITE,
            VfsCaps::MKDIR,
            VfsCaps::DELETE,
            VfsCaps::MOVE,
            VfsCaps::COPY,
            VfsCaps::TOUCH,
        ]
    }

    #[tokio::test]
    async fn local_fs_declares_all_write_caps() {
        let fs = LocalFs::new();
        for cap in write_caps() {
            assert!(fs.capabilities().has(cap), "LocalFs 应声明写能力位 {cap:?}");
        }
    }

    #[tokio::test]
    async fn write_and_mkdir() {
        let (dir, root) = setup().await;
        let fs = LocalFs::new();
        fs.mkdir(&root.join("newdir")).await.unwrap();
        assert_eq!(fs.stat(&root.join("newdir")).await.unwrap().kind, EntryKind::Dir);
        fs.write_file_bytes(&root.join("newdir").join("a.txt"), b"AAA").await.unwrap();
        assert_eq!(fs.read_file_bytes(&root.join("newdir").join("a.txt")).await.unwrap(), b"AAA");

        // 已存在目录重复创建 -> AlreadyExists
        assert!(matches!(
            fs.mkdir(&root.join("newdir")).await,
            Err(VfsError::AlreadyExists { .. })
        ));
        drop(dir);
    }

    #[tokio::test]
    async fn mkdir_all_creates_chain_and_idempotent() {
        let (dir, root) = setup().await;
        let fs = LocalFs::new();
        fs.mkdir_all(&root.join("a/b/c")).await.unwrap();
        assert_eq!(fs.stat(&root.join("a/b/c")).await.unwrap().kind, EntryKind::Dir);
        // 幂等：全部已存在
        fs.mkdir_all(&root.join("a/b/c")).await.unwrap();
        // 中间存在同名文件 -> AlreadyExists
        fs.write_file_bytes(&root.join("block"), b"x").await.unwrap();
        assert!(matches!(
            fs.mkdir_all(&root.join("block/sub")).await,
            Err(VfsError::AlreadyExists { .. })
        ));
        drop(dir);
    }

    #[tokio::test]
    async fn copy_file_preserves_content_and_mtime() {
        let (dir, root) = setup().await;
        let fs = LocalFs::new();
        let src = root.join("hello.txt");
        let before = fs.stat(&src).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        fs.copy_file(&src, &root.join("copy.txt")).await.unwrap();
        let dst = fs.stat(&root.join("copy.txt")).await.unwrap();
        assert_eq!(dst.size, before.size);
        assert_eq!(dst.mtime_ms, before.mtime_ms, "copy 应保留源 mtime");
        assert_eq!(
            fs.read_file_bytes(&root.join("copy.txt")).await.unwrap(),
            fs.read_file_bytes(&src).await.unwrap()
        );
        // 目录不可 copy_file
        assert!(matches!(fs.copy_file(&root.join("sub"), &root.join("x")).await, Err(VfsError::NotAFile { .. })));
        drop(dir);
    }

    #[tokio::test]
    async fn rename_moves_and_overwrites() {
        let (dir, root) = setup().await;
        let fs = LocalFs::new();
        fs.rename(&root.join("hello.txt"), &root.join("renamed.txt")).await.unwrap();
        assert!(fs.stat(&root.join("hello.txt")).await.is_err());
        assert!(fs.stat(&root.join("renamed.txt")).await.is_ok());
        // 目标已存在（覆盖语义）
        fs.rename(&root.join("bad.bin"), &root.join("renamed.txt")).await.unwrap();
        assert_eq!(fs.stat(&root.join("renamed.txt")).await.unwrap().size, 3);
        drop(dir);
    }

    #[tokio::test]
    async fn remove_file_and_empty_dir() {
        let (dir, root) = setup().await;
        let fs = LocalFs::new();
        fs.remove(&root.join("hello.txt"), false).await.unwrap();
        assert!(fs.stat(&root.join("hello.txt")).await.is_err());
        // 非空目录 recursive=false 报错
        assert!(fs.remove(&root.join("sub"), false).await.is_err());
        fs.remove(&root.join("sub"), true).await.unwrap();
        assert!(fs.stat(&root.join("sub")).await.is_err());
        // 不存在
        assert!(matches!(
            fs.remove(&root.join("nope"), false).await,
            Err(VfsError::NotFound { .. })
        ));
        drop(dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remove_symlink_only_removes_link() {
        use std::os::unix::fs::symlink;
        let (dir, _root) = setup().await;
        let fs = LocalFs::new();
        let link = dir.path().join("link");
        symlink(dir.path().join("sub"), &link).unwrap();
        let lp = VfsPath::parse(link.to_str().unwrap()).unwrap();
        fs.remove(&lp, true).await.unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err(), "链接应被删除");
        assert!(dir.path().join("sub").is_dir(), "目标目录不得跟随删除");
        drop(dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recursive_tree_delete_never_descends_symlink() {
        use std::os::unix::fs::symlink;
        let (dir, root) = setup().await;
        let fs = LocalFs::new();
        // tree/ 内含指向 tree 外部 victim/ 的符号链接
        let tree = dir.path().join("tree");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join("f.txt"), b"f").unwrap();
        let victim = dir.path().join("victim");
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("keep.txt"), b"keep").unwrap();
        symlink(&victim, tree.join("esc")).unwrap();
        fs.remove(&root.join("tree"), true).await.unwrap();
        assert!(std::fs::symlink_metadata(&tree).is_err(), "树应被删除");
        assert!(victim.join("keep.txt").exists(), "符号链接目标不得被删（逃逸防护）");
        drop(dir);
    }

    #[tokio::test]
    async fn set_mtime_touches_timestamp() {
        let (dir, root) = setup().await;
        let fs = LocalFs::new();
        let target = root.join("hello.txt");
        let future_ms: u64 = 1_800_000_000_000; // 2027-01-15 前后
        fs.set_mtime(&target, future_ms).await.unwrap();
        let st = fs.stat(&target).await.unwrap();
        assert_eq!(st.mtime_ms, future_ms);
        drop(dir);
    }
}
