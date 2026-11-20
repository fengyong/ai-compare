//! 原子落盘临时文件创建（local.rs 异步 / zip.rs 同步共用）。
//!
//! 两条安全/正确性约束（R5-SEC-01 + R5-VFS-02）：
//! - 临时名短且不含原名：`.tmp-aic-<16hex>`（25 字节，远小于 NAME_MAX）。
//!   旧名 `.{orig}.tmp-{pid}-{seq}` 在原名接近 NAME_MAX 时溢出 → 合法长名
//!   文件无法原子写（VFS-02 回归）。
//! - 以 `create_new`（O_EXCL|O_CREAT）创建：路径已存在（含被预置的符号链接）
//!   即失败换新名重试，绝不打开/截断已存在路径，杜绝符号链接劫持（SEC-01）。
//!   名字由每进程随机种子（RandomState）+ pid + 序号 + 纳秒时间戳派生，不可预测。

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 名字碰撞重试上限（含被预置符号链接占位的情形）
const MAX_ATTEMPTS: u32 = 16;

static SEQ: AtomicU64 = AtomicU64::new(0);

/// 短随机临时名（不含原名，25 字节）。保留 `.tmp-` 子串以复用残留清理守卫。
pub(crate) fn random_tmp_name() -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    h.write_u64(SEQ.fetch_add(1, Ordering::Relaxed));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    h.write_u128(nanos);
    format!(".tmp-aic-{:016x}", h.finish())
}

fn collision_exhausted() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "原子写临时名碰撞重试耗尽",
    )
}

/// 同步：以 O_EXCL 在 parent 下建新临时文件；碰撞换新名重试（zip.rs 用）。
/// `name_gen` 可注入以便确定性测试符号链接占位场景。
pub(crate) fn create_new_tmp_with(
    parent: &Path,
    mut name_gen: impl FnMut() -> String,
) -> std::io::Result<(PathBuf, std::fs::File)> {
    for _ in 0..MAX_ATTEMPTS {
        let p = parent.join(name_gen());
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
        {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(collision_exhausted())
}

pub(crate) fn create_new_tmp(parent: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    create_new_tmp_with(parent, random_tmp_name)
}

/// 异步：以 O_EXCL 在 parent 下建新临时文件；碰撞换新名重试（local.rs 用）。
pub(crate) async fn create_new_tmp_async_with(
    parent: &Path,
    mut name_gen: impl FnMut() -> String,
) -> std::io::Result<(PathBuf, tokio::fs::File)> {
    for _ in 0..MAX_ATTEMPTS {
        let p = parent.join(name_gen());
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)
            .await
        {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(collision_exhausted())
}

pub(crate) async fn create_new_tmp_async(
    parent: &Path,
) -> std::io::Result<(PathBuf, tokio::fs::File)> {
    create_new_tmp_async_with(parent, random_tmp_name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmp_name_is_short_and_unique() {
        let a = random_tmp_name();
        let b = random_tmp_name();
        assert_ne!(a, b, "两次名字须不同（不可预测/避免碰撞）");
        assert!(a.len() <= 32, "临时名须远小于 NAME_MAX：{a}");
        assert!(a.contains(".tmp-"), "保留 .tmp- 以复用残留守卫：{a}");
        assert!(!a.contains("hello.txt"), "临时名不得嵌入原名");
    }

    /// R5-SEC-01：候选名被预置符号链接占位时，O_EXCL 须拒绝并换新名，
    /// 绝不跟随链接截断 victim。旧实现 File::create 会写穿 victim。
    #[cfg(unix)]
    #[test]
    fn sync_create_refuses_preplaced_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, b"SAFE").unwrap();
        let link = dir.path().join(".tmp-aic-0000000000000000");
        std::os::unix::fs::symlink(&victim, &link).unwrap();

        let cand = [
            ".tmp-aic-0000000000000000".to_string(),
            ".tmp-aic-1111111111111111".to_string(),
        ];
        let i = AtomicU64::new(0);
        let (p, _f) = create_new_tmp_with(dir.path(), || {
            cand[i.fetch_add(1, Ordering::Relaxed) as usize].clone()
        })
        .unwrap();
        assert_eq!(
            p.file_name().unwrap().to_str().unwrap(),
            ".tmp-aic-1111111111111111",
            "碰撞须换新名而非打开符号链接"
        );
        assert_eq!(std::fs::read(&victim).unwrap(), b"SAFE", "victim 不得被截断");
        assert!(
            std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
            "占位符号链接本身不得被替换"
        );
    }

    /// R5-SEC-01 异步版（local.rs 路径）。
    #[cfg(unix)]
    #[tokio::test]
    async fn async_create_refuses_preplaced_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        tokio::fs::write(&victim, b"SAFE").await.unwrap();
        let link = dir.path().join(".tmp-aic-0000000000000000");
        tokio::fs::symlink(&victim, &link).await.unwrap();

        let cand = [
            ".tmp-aic-0000000000000000".to_string(),
            ".tmp-aic-1111111111111111".to_string(),
        ];
        let i = AtomicU64::new(0);
        let (p, _f) = create_new_tmp_async_with(dir.path(), || {
            cand[i.fetch_add(1, Ordering::Relaxed) as usize].clone()
        })
        .await
        .unwrap();
        assert_eq!(
            p.file_name().unwrap().to_str().unwrap(),
            ".tmp-aic-1111111111111111"
        );
        assert_eq!(tokio::fs::read(&victim).await.unwrap(), b"SAFE");
        assert!(
            tokio::fs::symlink_metadata(&link)
                .await
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    /// 碰撞重试耗尽须报错（不得无限循环或退化为打开已存在路径）。
    #[test]
    fn sync_create_exhausts_retries() {
        let dir = tempfile::tempdir().unwrap();
        let occupy = dir.path().join(".tmp-aic-0000000000000000");
        std::fs::write(&occupy, b"x").unwrap();
        let err = create_new_tmp_with(dir.path(), || ".tmp-aic-0000000000000000".to_string())
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }
}