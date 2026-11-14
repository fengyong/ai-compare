//! P12 scheme 路由：按路径前缀分发到对应 Provider。
//!
//! - 无 scheme（`/abs/path`、`C:/path`、相对）→ [`LocalFs`]
//! - `zip://<本地归档>![内部路径]` → [`ZipFsMut`]（可写归档；同归档路径
//!   复用同一实例，write-through 自动回写）
//! - `ftp://` / `ftps://` / `sftp://` → P12-c 预留（本阶段 Unsupported）
//!
//! [`RouteFs`] 实现 [`VfsProvider`]：引擎/CLI 以单一 provider 服务
//! 「本地目录 vs 压缩包内 vs 远程」混合会话（两侧 scheme 可不同）。
//! 路径的 scheme 部分由 [`VfsPath::parse`] 词法保留（大小写原样）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::ftp::{split_ftp_uri, FtpFs};
use crate::zip::ZipFsMut;
use crate::{StatInfo, VfsCaps, VfsDirEntry, VfsError, VfsPath, VfsProvider};

/// 检测路径是否带 ai-vfs 认识的 scheme（zip/ftp/ftps/sftp）
pub fn scheme_of(s: &str) -> Option<&'static str> {
    let lower_prefix = |p: &str| s.len() > p.len() && s[..p.len()].eq_ignore_ascii_case(p);
    if lower_prefix("zip://") {
        Some("zip")
    } else if lower_prefix("ftps://") {
        Some("ftps")
    } else if lower_prefix("ftp://") {
        Some("ftp")
    } else if lower_prefix("sftp://") {
        Some("sftp")
    } else {
        None
    }
}

/// 拆 zip URI：`zip://<本地归档路径>![内部路径]` -> (归档路径, 内部路径或空)。
/// 归档路径不含 `!`（zip 条目名允许 `!`，分隔取**第一个** `!`）。
fn split_zip_uri(s: &str) -> Option<(String, String)> {
    let rest = s.strip_prefix("zip://").or_else(|| s.strip_prefix("ZIP://"))?;
    match rest.split_once('!') {
        Some((archive, inner)) => Some((archive.to_string(), inner.to_string())),
        None => Some((rest.to_string(), String::new())),
    }
}

/// 剥 scheme 前缀的包装 Provider：`zip://repo.zip!/a.txt` -> 内层收到 `a.txt`。
/// 内层命名空间为条目相对路径（[`ZipFsMut`]）。
struct ZipPrefixProvider {
    inner: Arc<ZipFsMut>,
    /// 剥除前缀（`zip:///repo.zip!`，含尾分隔）
    base: String,
}

impl ZipPrefixProvider {
    fn strip(&self, path: &VfsPath) -> Result<VfsPath, VfsError> {
        let s = path.as_str();
        let rel = s
            .strip_prefix(&self.base)
            .ok_or_else(|| VfsError::InvalidPath(format!("路径不属于该归档: {s}")))?;
        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            Ok(VfsPath::parse("/").expect("根路径恒可解析"))
        } else {
            VfsPath::parse(rel)
        }
    }
}

#[async_trait]
impl VfsProvider for ZipPrefixProvider {
    fn scheme(&self) -> &'static str {
        "zip"
    }

    fn capabilities(&self) -> VfsCaps {
        self.inner.capabilities()
    }

    async fn stat(&self, path: &VfsPath) -> Result<StatInfo, VfsError> {
        self.inner.stat(&self.strip(path)?).await
    }

    async fn list_dir(&self, path: &VfsPath) -> Result<Vec<VfsDirEntry>, VfsError> {
        self.inner.list_dir(&self.strip(path)?).await
    }

    async fn read_file_bytes(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.inner.read_file_bytes(&self.strip(path)?).await
    }

    async fn stored_crc32(&self, path: &VfsPath) -> Result<Option<u32>, VfsError> {
        self.inner.stored_crc32(&self.strip(path)?).await
    }

    async fn write_file_bytes(&self, path: &VfsPath, data: &[u8]) -> Result<(), VfsError> {
        self.inner.write_file_bytes(&self.strip(path)?, data).await
    }

    async fn mkdir(&self, path: &VfsPath) -> Result<(), VfsError> {
        self.inner.mkdir(&self.strip(path)?).await
    }

    async fn rename(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.inner.rename(&self.strip(from)?, &self.strip(to)?).await
    }

    async fn remove(&self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.inner.remove(&self.strip(path)?, recursive).await
    }

    async fn copy_file(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.inner.copy_file(&self.strip(from)?, &self.strip(to)?).await
    }

    async fn set_mtime(&self, path: &VfsPath, mtime_ms: u64) -> Result<(), VfsError> {
        self.inner.set_mtime(&self.strip(path)?, mtime_ms).await
    }
}

/// scheme 路由 Provider（P12）：单一入口服务混合来源会话。
pub struct RouteFs {
    local: Arc<dyn VfsProvider>,
    /// zip 归档实例缓存（同归档路径复用同一 ZipFsMut，跨方法状态一致）
    zips: Mutex<HashMap<String, Arc<ZipPrefixProvider>>>,
    /// ftp/ftps 实例缓存（key = scheme://authority/root）
    ftps: Mutex<HashMap<String, Arc<FtpFs>>>,
}

impl Default for RouteFs {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteFs {
    pub fn new() -> Self {
        RouteFs {
            local: Arc::new(crate::LocalFs::new()),
            zips: Mutex::new(HashMap::new()),
            ftps: Mutex::new(HashMap::new()),
        }
    }

    /// 按路径取目标 provider；zip 归档惰性装载并缓存
    fn route(&self, path: &VfsPath) -> Result<Arc<dyn VfsProvider>, VfsError> {
        match scheme_of(path.as_str()) {
            None => Ok(self.local.clone()),
            Some("zip") => {
                let (archive, _) = split_zip_uri(path.as_str())
                    .ok_or_else(|| VfsError::InvalidPath(path.as_str().to_string()))?;
                let mut zips = self.zips.lock().expect("route zip 缓存锁中毒");
                if let Some(p) = zips.get(&archive) {
                    return Ok(p.clone());
                }
                let inner = Arc::new(ZipFsMut::from_file(std::path::Path::new(&archive))?);
                let base = format!("zip://{archive}!");
                let prefixed = Arc::new(ZipPrefixProvider { inner, base });
                zips.insert(archive, prefixed.clone());
                Ok(prefixed)
            }
            Some("ftp") => {
                let mut ftps = self.ftps.lock().expect("route ftp 缓存锁中毒");
                let key = path.as_str().to_string();
                if let Some(p) = ftps.get(&key) {
                    return Ok(p.clone());
                }
                let (config, root) = split_ftp_uri(path.as_str())?;
                let ftp = Arc::new(FtpFs::new(config, root));
                ftps.insert(key, ftp.clone());
                Ok(ftp)
            }
            Some("ftps") => Err(VfsError::Unsupported(
                "ftps:// 需真实 FTPS 服务器验证，P12-c 后续批次交付".into(),
            )),
            Some("sftp") => Err(VfsError::Unsupported(
                "sftp:// 属 P12-c 后半（SFTP provider 评估接入中）".into(),
            )),
            Some(other) => Err(VfsError::Unsupported(format!("未知 scheme: {other}"))),
        }
    }
}

#[async_trait]
impl VfsProvider for RouteFs {
    fn scheme(&self) -> &'static str {
        "route"
    }

    fn capabilities(&self) -> VfsCaps {
        // 路由面能力取并集语义无意义（本地满配 + zip 可写 + 远程未接）；
        // 执行层按具体 provider 协商——这里声明读（最保守公共面）
        VfsCaps::READ
    }

    async fn stat(&self, path: &VfsPath) -> Result<StatInfo, VfsError> {
        self.route(path)?.stat(path).await
    }

    async fn list_dir(&self, path: &VfsPath) -> Result<Vec<VfsDirEntry>, VfsError> {
        self.route(path)?.list_dir(path).await
    }

    async fn read_file_bytes(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.route(path)?.read_file_bytes(path).await
    }

    async fn stored_crc32(&self, path: &VfsPath) -> Result<Option<u32>, VfsError> {
        self.route(path)?.stored_crc32(path).await
    }

    async fn write_file_bytes(&self, path: &VfsPath, data: &[u8]) -> Result<(), VfsError> {
        self.route(path)?.write_file_bytes(path, data).await
    }

    async fn mkdir(&self, path: &VfsPath) -> Result<(), VfsError> {
        self.route(path)?.mkdir(path).await
    }

    async fn mkdir_all(&self, path: &VfsPath) -> Result<(), VfsError> {
        // 仅本地支持父链逐级创建；zip 内经 write/mkdir 组合由执行层处理
        if scheme_of(path.as_str()).is_none() {
            return self.local.mkdir_all(path).await;
        }
        self.route(path)?.mkdir_all(path).await
    }

    async fn rename(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        // 跨 scheme 重命名不支持（本地<->zip 内容搬运应走 copy+delete 组合）
        match (scheme_of(from.as_str()), scheme_of(to.as_str())) {
            (None, None) => self.local.rename(from, to).await,
            (Some(a), Some(b)) if a == b => self.route(from)?.rename(from, to).await,
            _ => Err(VfsError::Unsupported("跨来源 rename 不支持（用 copy+delete）".into())),
        }
    }

    async fn remove(&self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.route(path)?.remove(path, recursive).await
    }

    async fn copy_file(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        match (scheme_of(from.as_str()), scheme_of(to.as_str())) {
            (None, None) => self.local.copy_file(from, to).await,
            // 本地 -> zip：读源字节写入 zip（zip 侧 write_file 覆盖语义）
            (None, Some("zip")) => {
                let data = self.local.read_file_bytes(from).await?;
                self.route(to)?.write_file_bytes(to, &data).await
            }
            // zip -> 本地：读条目字节写本地
            (Some("zip"), None) => {
                let data = self.route(from)?.read_file_bytes(from).await?;
                self.local.write_file_bytes(to, &data).await
            }
            (Some(a), Some(b)) if a == b => self.route(from)?.copy_file(from, to).await,
            _ => Err(VfsError::Unsupported("该来源组合的 copy 暂不支持".into())),
        }
    }

    async fn set_mtime(&self, path: &VfsPath, mtime_ms: u64) -> Result<(), VfsError> {
        self.route(path)?.set_mtime(path, mtime_ms).await
    }
}

/// 按路径字符串构造会话级 provider（供引擎/CLI 的既有单 provider 形态）：
/// 含 scheme 时返回 RouteFs；纯本地路径返回 LocalFs（P0 行为不变）。
pub fn provider_for_session(any_spec_path: &str) -> Arc<dyn VfsProvider> {
    if scheme_of(any_spec_path).is_some() {
        Arc::new(RouteFs::new())
    } else {
        Arc::new(crate::LocalFs::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zip::build_test_zip;

    fn temp_zip(entries: &[(&str, &[u8])]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repo.zip");
        std::fs::write(&path, build_test_zip(entries)).unwrap();
        (dir, path)
    }

    #[test]
    fn vfs_path_preserves_scheme() {
        // scheme:// 原样保留；其后路径规范化
        let p = VfsPath::parse("zip:///repo.zip!/src/main.rs").unwrap();
        assert_eq!(p.as_str(), "zip:///repo.zip!/src/main.rs");
        // 空段/点段规范化只作用 scheme 后路径
        let p = VfsPath::parse("zip:///repo.zip!/src/./sub//x.rs").unwrap();
        assert_eq!(p.as_str(), "zip:///repo.zip!/src/sub/x.rs");
        // Windows 盘符在 scheme 后保留
        let p = VfsPath::parse("zip://C:/data/repo.zip").unwrap();
        assert_eq!(p.as_str(), "zip://C:/data/repo.zip");
        // ftp 形态
        let p = VfsPath::parse("ftp://host.example.com/pub/f.txt").unwrap();
        assert_eq!(p.as_str(), "ftp://host.example.com/pub/f.txt");
        // 无 scheme 路径行为不变
        let p = VfsPath::parse("/a/b/c.txt").unwrap();
        assert_eq!(p.as_str(), "/a/b/c.txt");
        // scheme 大小写原样保留（路由端大小写不敏感识别）
        let p = VfsPath::parse("ZIP:///r.zip!/a").unwrap();
        assert_eq!(p.as_str(), "ZIP:///r.zip!/a");
    }

    #[test]
    fn scheme_of_detects_known_prefixes() {
        assert_eq!(scheme_of("zip:///a.zip"), Some("zip"));
        assert_eq!(scheme_of("FTP://host"), Some("ftp"));
        assert_eq!(scheme_of("sftp://host"), Some("sftp"));
        assert_eq!(scheme_of("ftps://host"), Some("ftps"));
        assert_eq!(scheme_of("/plain/path"), None);
        assert_eq!(scheme_of("C:/win.txt"), None);
    }

    #[tokio::test]
    async fn routefs_local_unaffected() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"local").unwrap();
        let rf = RouteFs::new();
        let p = VfsPath::parse(&f.display().to_string()).unwrap();
        assert_eq!(rf.read_file_bytes(&p).await.unwrap(), b"local");
    }

    #[tokio::test]
    async fn routefs_zip_read_write_through() {
        let (_dir, zip_path) = temp_zip(&[("src/a.txt", b"alpha"), ("src/b.log", b"beta")]);
        let rf = RouteFs::new();
        let uri = format!("zip://{}!/src/a.txt", zip_path.display());
        let p = VfsPath::parse(&uri).unwrap();
        // 读
        assert_eq!(rf.read_file_bytes(&p).await.unwrap(), b"alpha");
        assert_eq!(rf.stored_crc32(&p).await.unwrap(), Some(crate::crc32::crc32(b"alpha")));
        // 写（write-through：落盘）
        rf.write_file_bytes(&p, b"ALPHA-2").await.unwrap();
        // 同实例读回
        assert_eq!(rf.read_file_bytes(&p).await.unwrap(), b"ALPHA-2");
        // 落盘验证：重新独立装载归档
        let fresh = ZipFsMut::from_file(&zip_path).unwrap();
        assert_eq!(
            fresh
                .read_file_bytes(&VfsPath::parse("/src/a.txt").unwrap())
                .await
                .unwrap(),
            b"ALPHA-2"
        );
        // 目录列举
        let dir_uri = format!("zip://{}!/src", zip_path.display());
        let entries = rf.list_dir(&VfsPath::parse(&dir_uri).unwrap()).await.unwrap();
        let mut names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["a.txt", "b.log"]);
    }

    #[tokio::test]
    async fn routefs_local_to_zip_copy() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local.txt");
        std::fs::write(&local, b"from local fs").unwrap();
        let (_zdir, zip_path) = temp_zip(&[("seed.txt", b"s")]);
        let rf = RouteFs::new();
        rf.copy_file(
            &VfsPath::parse(&local.display().to_string()).unwrap(),
            &VfsPath::parse(&format!("zip://{}!/copied/local.txt", zip_path.display())).unwrap(),
        )
        .await
        .unwrap();
        // 落盘 + 父链自动建立
        let fresh = ZipFsMut::from_file(&zip_path).unwrap();
        assert_eq!(
            fresh
                .read_file_bytes(&VfsPath::parse("/copied/local.txt").unwrap())
                .await
                .unwrap(),
            b"from local fs"
        );
    }

    #[tokio::test]
    async fn routefs_cross_scheme_rename_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.txt");
        std::fs::write(&local, b"x").unwrap();
        let (_zdir, zip_path) = temp_zip(&[("z.txt", b"z")]);
        let rf = RouteFs::new();
        let err = rf
            .rename(
                &VfsPath::parse(&local.display().to_string()).unwrap(),
                &VfsPath::parse(&format!("zip://{}!/moved.txt", zip_path.display())).unwrap(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, VfsError::Unsupported(_)));
    }

    #[tokio::test]
    async fn ftp_scheme_routes_to_real_provider() {
        // P12-c：ftp:// 路由到 FtpFs（真实 provider）；ftps:// 仍为暂缓占位
        let rf = RouteFs::new();
        assert_eq!(rf.route(&VfsPath::parse("ftp://host/pub").unwrap()).unwrap().scheme(), "ftp");
        let err = rf
            .stat(&VfsPath::parse("ftps://host/pub/f.txt").unwrap())
            .await
            .unwrap_err();
        assert!(matches!(err, VfsError::Unsupported(_)));
        // provider_for_session 对 ftp 返回路由面（scheme=route）
        let p = provider_for_session("ftp://host");
        assert_eq!(p.scheme(), "route");
    }

    #[test]
    fn provider_for_session_local_is_localfs_semantics() {
        // 纯本地路径返回 LocalFs（无 scheme）——P0 行为不变
        let p = provider_for_session("/some/dir");
        assert_eq!(p.scheme(), "file");
        let p = provider_for_session("zip:///r.zip");
        assert_eq!(p.scheme(), "route");
    }
}
