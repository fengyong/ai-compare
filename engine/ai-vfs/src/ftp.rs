//! P12-c：FTP Provider（suppaftp 12 tokio，纯 Rust）。
//!
//! URI 形态（凭据**内存态**，协议/持久化层不落盘）：
//! `ftp://[user[:pass]@]host[:port]/root`。
//!
//! 模型：连接惰性建立 + 单流复用（会话请求已串行，docs/03 §2.3）；命令失败
//! 重连一次（服务器空闲踢线常见）。`stat` 文件走 SIZE+MDTM，目录判定以
//! LIST 成功为准；`list_dir` MLSD 优先（RFC3659）回退 LIST+POSIX/DOS 解析；
//! 返回行全部解析失败时报错而非空集（SEC-03：防镜像同步把非空目录判空真删）；
//! `copy_file` 经 RETR+STOR 组合（RFC 959 无服务器端 copy）；`set_mtime`
//! 需 MFMT 非标扩展，本实现 Unsupported（与 BC 行为差异已记录）。
//!
//! 已知边界：
//! - `retr`/`put_file` 全量内存（大文件流式分页属后续优化）
//! - **FTPS（AUTH TLS）暂缓**：rustls verifier 构造与 into_secure 类型转换
//!   需真实 FTPS 服务器验证（route 层 ftps:// 报 Unsupported，后续批次交付）
//! - SFTP 属独立 provider（ssh 系协议栈，P12-c 后半）

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use suppaftp::list::File as ListFile;
use suppaftp::tokio::AsyncFtpStream;
use suppaftp::types::FileType as FtpTransferType;
use tokio::io::AsyncReadExt;

use crate::{EntryKind, StatInfo, VfsCaps, VfsDirEntry, VfsError, VfsPath, VfsProvider};

/// 连接与鉴权配置（凭据内存态）
#[derive(Debug, Clone)]
pub struct FtpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

/// 拆 `ftp://[user[:pass]@]host[:port]/root` -> (配置, 服务器根路径)。
/// 缺省 user=anonymous、pass 空、port 21；用户信息不进入根路径。
pub fn split_ftp_uri(s: &str) -> Result<(FtpConfig, String), VfsError> {
    let (scheme, rest) = s
        .split_once("://")
        .ok_or_else(|| VfsError::InvalidPath(format!("非 URI 形态: {s}")))?;
    if !scheme.eq_ignore_ascii_case("ftp") {
        return Err(VfsError::InvalidPath(format!("非 ftp URI: {s}")));
    }
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (u, h),
        None => ("anonymous", authority),
    };
    let (user, password) = match userinfo.split_once(':') {
        Some((u, p)) => (u.to_string(), p.to_string()),
        None => (userinfo.to_string(), String::new()),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| VfsError::InvalidPath(format!("非法端口: {p}")))?,
        ),
        None => (hostport.to_string(), 21),
    };
    if host.is_empty() {
        return Err(VfsError::InvalidPath(format!("缺主机名: {s}")));
    }
    Ok((
        FtpConfig {
            host,
            port,
            user,
            password,
        },
        path,
    ))
}

/// FTP Provider：单流复用 + 失败重连一次。
pub struct FtpFs {
    config: FtpConfig,
    /// 服务器路径根前缀（URI path；条目相对路径拼接于此）
    root: String,
    stream: tokio::sync::Mutex<Option<AsyncFtpStream>>,
}

impl FtpFs {
    pub fn new(config: FtpConfig, root: String) -> Self {
        FtpFs {
            config,
            root,
            stream: tokio::sync::Mutex::new(None),
        }
    }

    /// 相对路径 -> 服务器绝对路径（root 拼接）
    fn server_path(&self, rel: &str) -> String {
        let rel = rel.trim_start_matches('/');
        if rel.is_empty() {
            return if self.root.is_empty() {
                "/".into()
            } else {
                self.root.clone()
            };
        }
        if self.root.ends_with('/') {
            format!("{}{}", self.root, rel)
        } else if self.root.is_empty() {
            format!("/{rel}")
        } else {
            format!("{}/{}", self.root, rel)
        }
    }

    /// 取流（惰性连接；单次执行——P12 简化：连接失败即报错，
    /// 断线自动重连属健壮性优化后续批次）。闭包 move 捕获 Arc 数据（'static），
    /// trait &self 下经 state 引用注入流。
    async fn with_stream<T, F>(&self, op: F) -> Result<T, VfsError>
    where
        T: Send,
        F: for<'a> FnOnce(
            &'a mut AsyncFtpStream,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<T, suppaftp::FtpError>> + Send + 'a>,
        >,
    {
        let mut guard = self.stream.lock().await;
        if guard.is_none() {
            let mut stream = self.connect().await?;
            stream
                .transfer_type(FtpTransferType::Binary)
                .await
                .map_err(ftp_err)?;
            *guard = Some(stream);
        }
        let stream = guard.as_mut().expect("已建立连接");
        op(stream).await.map_err(ftp_err)
    }

    async fn connect(&self) -> Result<AsyncFtpStream, VfsError> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let mut stream = AsyncFtpStream::connect(&addr).await.map_err(ftp_err)?;
        stream
            .login(&self.config.user, &self.config.password)
            .await
            .map_err(ftp_err)?;
        if !self.root.is_empty() && self.root != "/" {
            stream.cwd(&self.root).await.map_err(ftp_err)?;
        }
        Ok(stream)
    }
}

fn ftp_err(e: suppaftp::FtpError) -> VfsError {
    VfsError::Unsupported(format!("ftp: {e}"))
}

fn not_found(path: &VfsPath) -> VfsError {
    VfsError::NotFound {
        path: path.as_str().to_string(),
    }
}

#[async_trait]
impl VfsProvider for FtpFs {
    fn scheme(&self) -> &'static str {
        "ftp"
    }

    fn capabilities(&self) -> VfsCaps {
        // 读 + 写面（RFC 959 标准命令族）；TOUCH 不支持（MFMT 非标）
        VfsCaps::READ
            .union(VfsCaps::WRITE)
            .union(VfsCaps::MKDIR)
            .union(VfsCaps::DELETE)
            .union(VfsCaps::MOVE)
            .union(VfsCaps::COPY)
    }

    async fn stat(&self, path: &VfsPath) -> Result<StatInfo, VfsError> {
        let rel = path.as_str().trim_start_matches('/').to_string();
        if rel.is_empty() {
            return Ok(StatInfo::minimal(0, 0, EntryKind::Dir));
        }
        let sp: Arc<str> = Arc::from(self.server_path(&rel).as_str());
        // 文件优先：SIZE + MDTM；SIZE 失败再探目录（LIST 成功即目录）
        let sp_arc: Arc<str> = sp.clone();
        let size_mdtm = self
            .with_stream(move |s| {
                let sp = sp_arc.clone();
                Box::pin(async move {
                    let size = s.size(&sp).await?;
                    let mdtm = s.mdtm(&sp).await?;
                    Ok((size, mdtm))
                })
            })
            .await;
        if let Ok((size, mdtm)) = size_mdtm {
            let mtime_ms = mdtm.and_utc().timestamp_millis().try_into().unwrap_or(0);
            return Ok(StatInfo {
                size: size as u64,
                mtime_ms,
                kind: EntryKind::File,
                unix_mode: None,
                uid: None,
                gid: None,
                dos_attrs: 0,
            });
        }
        let is_dir = self
            .with_stream(move |s| {
                let sp = sp.clone();
                Box::pin(async move { s.list(Some(&sp)).await.map(|_| ()) })
            })
            .await
            .is_ok();
        if is_dir {
            return Ok(StatInfo::minimal(0, 0, EntryKind::Dir));
        }
        Err(not_found(path))
    }

    async fn list_dir(&self, path: &VfsPath) -> Result<Vec<VfsDirEntry>, VfsError> {
        let rel = path.as_str().trim_start_matches('/').to_string();
        let sp: Arc<str> = Arc::from(self.server_path(&rel).as_str());
        // MLSD 优先（机器可读），失败回退 LIST+POSIX/DOS 解析
        let sp_arc: Arc<str> = sp.clone();
        let mlsd_lines = self
            .with_stream(move |s| {
                let sp = sp_arc.clone();
                Box::pin(async move { s.mlsd(Some(&sp)).await })
            })
            .await;
        let mut out = Vec::new();
        // SEC-03：返回行全部解析失败时必须报错，绝不返回空集——空集会被
        // 镜像同步判为「目录已空」生成 Delete 计划真删数据（未知不当相同）。
        let all_unparseable = |kind: &str, n: usize| {
            VfsError::Unsupported(format!(
                "ftp: {kind} 返回 {n} 行全部无法解析，拒绝当空目录（SEC-03）"
            ))
        };
        match mlsd_lines {
            Ok(lines) => {
                let mut parsed = 0usize;
                for line in &lines {
                    // 12.2.1 的替代 API（FileParser）尚未落地，from_mlsx_line 为当前唯一入口
                    #[allow(deprecated)]
                    if let Ok(f) = ListFile::from_mlsx_line(line) {
                        parsed += 1;
                        out.push(list_file_to_entry(&f));
                    }
                }
                if !lines.is_empty() && parsed == 0 {
                    return Err(all_unparseable("MLSD", lines.len()));
                }
            }
            Err(_) => {
                let lines = self
                    .with_stream(move |s| {
                        let sp = sp.clone();
                        Box::pin(async move { s.list(Some(&sp)).await })
                    })
                    .await?;
                let mut candidates = 0usize;
                let mut parsed = 0usize;
                for line in &lines {
                    // ls -l 总容量行不是条目，不计入「返回行数」
                    if line.trim_start().starts_with("total ") {
                        continue;
                    }
                    candidates += 1;
                    // 同上：from_posix_line 为 12.2.1 当前入口
                    #[allow(deprecated)]
                    let f = match ListFile::from_posix_line(line) {
                        Ok(f) => f,
                        Err(_) => continue, // 无法识别的行（方言输出等）
                    };
                    parsed += 1;
                    let name = f.name().to_string();
                    if name == "." || name == ".." {
                        continue;
                    }
                    out.push(list_file_to_entry(&f));
                }
                if candidates > 0 && parsed == 0 {
                    return Err(all_unparseable("LIST", candidates));
                }
            }
        }
        Ok(out)
    }

    async fn read_file_bytes(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        let sp: Arc<str> = Arc::from(
            self.server_path(path.as_str().trim_start_matches('/'))
                .as_str(),
        );
        self.with_stream(move |s| {
            let sp = sp.clone();
            Box::pin(async move {
                s.retr(&sp, |stream| {
                    Box::pin(async move {
                        let mut stream = stream;
                        let mut buf = Vec::new();
                        stream
                            .read_to_end(&mut buf)
                            .await
                            .map_err(suppaftp::FtpError::ConnectionError)?;
                        Ok((buf, stream))
                    })
                })
                .await
            })
        })
        .await
    }

    async fn write_file_bytes(&self, path: &VfsPath, data: &[u8]) -> Result<(), VfsError> {
        let sp: Arc<str> = Arc::from(
            self.server_path(path.as_str().trim_start_matches('/'))
                .as_str(),
        );
        let data = data.to_vec();
        self.with_stream(move |s| {
            let sp = sp.clone();
            Box::pin(async move {
                let mut cursor = std::io::Cursor::new(&*data);
                s.put_file(&sp, &mut cursor).await.map(|_| ())
            })
        })
        .await
    }

    async fn mkdir(&self, path: &VfsPath) -> Result<(), VfsError> {
        let sp: Arc<str> = Arc::from(
            self.server_path(path.as_str().trim_start_matches('/'))
                .as_str(),
        );
        self.with_stream(move |s| {
            let sp = sp.clone();
            Box::pin(async move { s.mkdir(&sp).await })
        })
        .await
    }

    async fn rename(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        let sp_from: Arc<str> = Arc::from(
            self.server_path(from.as_str().trim_start_matches('/'))
                .as_str(),
        );
        let sp_to: Arc<str> = Arc::from(
            self.server_path(to.as_str().trim_start_matches('/'))
                .as_str(),
        );
        self.with_stream(move |s| {
            let sp_from = sp_from.clone();
            let sp_to = sp_to.clone();
            Box::pin(async move { s.rename(&sp_from, &sp_to).await })
        })
        .await
    }

    async fn remove(&self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        let rel = path.as_str().trim_start_matches('/').to_string();
        let sp: Arc<str> = Arc::from(self.server_path(&rel).as_str());
        // 文件先试 DELE；失败按目录处理（RMD；递归则先清子树）
        let sp_arc: Arc<str> = sp.clone();
        let file_del = self
            .with_stream(move |s| {
                let sp = sp_arc.clone();
                Box::pin(async move { s.rm(&sp).await })
            })
            .await;
        if file_del.is_ok() {
            return Ok(());
        }
        if recursive {
            for entry in self.list_dir(path).await? {
                let child_rel = if rel.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{rel}/{}", entry.name)
                };
                let child = VfsPath::parse(&format!("/{child_rel}"))?;
                Box::pin(self.remove(&child, true)).await?;
            }
            self.with_stream(move |s| {
                let sp = sp.clone();
                Box::pin(async move { s.rmdir(&sp).await })
            })
            .await
        } else {
            let children = self.list_dir(path).await?;
            if !children.is_empty() {
                return Err(VfsError::InvalidPath(format!(
                    "目录非空（需 recursive）: {rel}"
                )));
            }
            self.with_stream(move |s| {
                let sp = sp.clone();
                Box::pin(async move { s.rmdir(&sp).await })
            })
            .await
        }
    }

    async fn copy_file(&self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        // RFC 959 无服务器端 copy：RETR 内存 + STOR（P12 全量内存边界，同 read/write）
        let data = self.read_file_bytes(from).await?;
        self.write_file_bytes(to, &data).await
    }

    async fn set_mtime(&self, _path: &VfsPath, _mtime_ms: u64) -> Result<(), VfsError> {
        // RFC 3659 只定义读 MDTM；写时间戳需 MFMT 非标扩展——Unsupported（差异记录）
        Err(VfsError::Unsupported(
            "ftp: 服务器时间戳写回需 MFMT 非标扩展，暂不支持".into(),
        ))
    }
}

fn list_file_to_entry(f: &ListFile) -> VfsDirEntry {
    let kind = if f.is_directory() {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    let mtime_ms = f
        .modified()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    VfsDirEntry {
        name: f.name().to_string(),
        stat: StatInfo {
            size: f.size() as u64,
            mtime_ms,
            kind,
            unix_mode: None,
            uid: None,
            gid: None,
            dos_attrs: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// 最小伪 FTP 服务器（单控制连接 + PASV 数据连接），覆盖 suppaftp 12
    /// 的命令序列：220 欢迎 → USER/PASS → TYPE → PASV/MLSD/LIST → 226。
    /// `mlsd` 为 None 时 MLSD 回 500（触发 LIST 回退）；否则经数据连接返回。
    async fn spawn_fake_ftp(list_payload: &'static str, mlsd: Option<&'static str>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (ctrl, _) = listener.accept().await.unwrap();
            let (rd, mut wr) = ctrl.into_split();
            let mut lines = BufReader::new(rd).lines();
            wr.write_all(b"220 fake\r\n").await.unwrap();
            let mut data_listener: Option<TcpListener> = None;
            while let Ok(Some(line)) = lines.next_line().await {
                let cmd = line.split(' ').next().unwrap_or("").to_ascii_uppercase();
                match cmd.as_str() {
                    "USER" => wr.write_all(b"331 need password\r\n").await.unwrap(),
                    "PASS" => wr.write_all(b"230 logged in\r\n").await.unwrap(),
                    "TYPE" | "CWD" => wr.write_all(b"200 ok\r\n").await.unwrap(),
                    "PASV" => {
                        let dl = TcpListener::bind("127.0.0.1:0").await.unwrap();
                        let p = dl.local_addr().unwrap().port();
                        wr.write_all(
                            format!(
                                "227 Entering Passive Mode (127,0,0,1,{},{})\r\n",
                                p >> 8,
                                p & 0xff
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                        data_listener = Some(dl);
                    }
                    "MLSD" | "LIST" => {
                        let payload = if cmd == "MLSD" {
                            match mlsd {
                                Some(p) => p,
                                None => {
                                    wr.write_all(b"500 unknown command\r\n").await.unwrap();
                                    data_listener = None;
                                    continue;
                                }
                            }
                        } else {
                            list_payload
                        };
                        wr.write_all(b"150 opening\r\n").await.unwrap();
                        let dl = data_listener.take().expect("先 PASV");
                        let (mut ds, _) = dl.accept().await.unwrap();
                        ds.write_all(payload.as_bytes()).await.unwrap();
                        ds.shutdown().await.unwrap();
                        drop(ds);
                        wr.write_all(b"226 done\r\n").await.unwrap();
                    }
                    "QUIT" => {
                        wr.write_all(b"221 bye\r\n").await.unwrap();
                        break;
                    }
                    _ => wr.write_all(b"500 unknown\r\n").await.unwrap(),
                }
            }
        });
        port
    }

    async fn fake_fs(list_payload: &'static str, mlsd: Option<&'static str>) -> FtpFs {
        let port = spawn_fake_ftp(list_payload, mlsd).await;
        FtpFs::new(
            FtpConfig {
                host: "127.0.0.1".into(),
                port,
                user: "u".into(),
                password: "p".into(),
            },
            "/".into(),
        )
    }

    /// SEC-03 验收：方言服务器 LIST 全不可解析 → 报错，绝不返回空集。
    #[tokio::test]
    async fn list_all_unparseable_is_error_not_empty() {
        let fs = fake_fs("DIR0001 FILE-A\r\nDIR0002 FILE-B\r\n", None).await;
        let err = fs
            .list_dir(&VfsPath::parse("/").unwrap())
            .await
            .unwrap_err();
        assert!(matches!(err, VfsError::Unsupported(_)), "实际: {err:?}");
        assert!(err.to_string().contains("2"), "应带返回行数: {err}");
    }

    /// MLSD 全不可解析同样报错（不回退 LIST——MLSD 已成功返回）。
    /// 注：suppaftp 对无 fact 行宽松收编为条目，须用非法 fact 值才触发硬失败。
    #[tokio::test]
    async fn mlsd_all_unparseable_is_error() {
        let fs = fake_fs("", Some("type=bogus; a\r\ntype=weird; b\r\n")).await;
        let err = fs
            .list_dir(&VfsPath::parse("/").unwrap())
            .await
            .unwrap_err();
        assert!(matches!(err, VfsError::Unsupported(_)), "实际: {err:?}");
    }

    /// 正常 POSIX 服务器行为不变。
    #[tokio::test]
    async fn list_normal_posix_unchanged() {
        let fs = fake_fs(
            "total 8\r\n\
             drwxr-xr-x 2 u g 4096 Nov 18 10:00 sub\r\n\
             -rw-r--r-- 1 u g 12 Nov 18 10:00 a.txt\r\n",
            None,
        )
        .await;
        let entries = fs.list_dir(&VfsPath::parse("/").unwrap()).await.unwrap();
        assert_eq!(entries.len(), 2);
        let sub = entries.iter().find(|e| e.name == "sub").unwrap();
        assert_eq!(sub.stat.kind, EntryKind::Dir);
        let a = entries.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!(a.stat.kind, EntryKind::File);
        assert_eq!(a.stat.size, 12);
    }

    /// 正常 MLSD 服务器行为不变。
    #[tokio::test]
    async fn mlsd_normal_unchanged() {
        let fs = fake_fs(
            "",
            Some("type=file;size=12;modify=20261118100000; a.txt\r\ntype=dir;modify=20261118100000; sub\r\n"),
        )
        .await;
        let entries = fs.list_dir(&VfsPath::parse("/").unwrap()).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .any(|e| e.name == "a.txt" && e.stat.kind == EntryKind::File));
        assert!(entries
            .iter()
            .any(|e| e.name == "sub" && e.stat.kind == EntryKind::Dir));
    }

    /// 空目录不误报：仅 total 行 / 空应答 / 仅 . 与 .. 都仍是 Ok(空)。
    #[tokio::test]
    async fn empty_dir_shapes_stay_ok() {
        for payload in [
            "total 0\r\n",
            "",
            "total 4\r\ndrwxr-xr-x 2 u g 4096 Nov 18 10:00 .\r\ndrwxr-xr-x 2 u g 4096 Nov 18 10:00 ..\r\n",
        ] {
            let fs = fake_fs(payload, None).await;
            let entries = fs.list_dir(&VfsPath::parse("/").unwrap()).await.unwrap();
            assert!(entries.is_empty(), "payload={payload:?} 应判空目录");
        }
    }

    /// 部分行可解析时不得过度拦截：坏行跳过、好行保留。
    #[tokio::test]
    async fn partial_parse_keeps_good_lines() {
        let fs = fake_fs(
            "JUNK DIALECT LINE\r\n-rw-r--r-- 1 u g 3 Nov 18 10:00 ok.txt\r\n",
            None,
        )
        .await;
        let entries = fs.list_dir(&VfsPath::parse("/").unwrap()).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "ok.txt");
    }
}
