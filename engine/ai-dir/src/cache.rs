//! 持久 stat/hash 缓存（ADR-004：`(path, size, mtime_ms) -> CRC32` 跨会话复用，
//! Meld CacheResult 模式）：内存 LRU + 可选 SQLite 落盘。
//!
//! 失效规则（明确约定）：
//! - 键 `(path, size, mtime_ms, algo)` 不完全命中即失效（新版本写入覆盖旧行）；
//! - [`HashCache::invalidate`] 显式清除某路径的历史版本；
//! - 外部修改且 mtime 不变导致的陈旧命中风险被接受（与 BC 一致，ADR-004）。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// 内存 LRU 容量（条目数）
const LRU_CAPACITY: usize = 4096;

/// 缓存条目
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    size: u64,
    mtime_ms: u64,
    crc32: u32,
    used: u64,
}

#[derive(Debug, Default)]
struct MemLru {
    map: HashMap<String, Entry>,
    clock: u64,
}

impl MemLru {
    fn get(&mut self, path: &str, size: u64, mtime_ms: u64) -> Option<u32> {
        let e = self.map.get_mut(path)?;
        if e.size == size && e.mtime_ms == mtime_ms {
            self.clock += 1;
            e.used = self.clock;
            Some(e.crc32)
        } else {
            // 键不匹配即失效（新 stat 版本）
            self.map.remove(path);
            None
        }
    }

    fn put(&mut self, path: &str, size: u64, mtime_ms: u64, crc32: u32) {
        self.clock += 1;
        if self.map.len() >= LRU_CAPACITY && !self.map.contains_key(path) {
            // O(n) 驱逐最久未用；容量 4096 下均摊可接受
            if let Some(victim) = self
                .map
                .iter()
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
            {
                self.map.remove(&victim);
            }
        }
        self.map.insert(
            path.to_string(),
            Entry { size, mtime_ms, crc32, used: self.clock },
        );
    }

    fn invalidate(&mut self, path: &str) {
        self.map.remove(path);
    }

    fn len(&self) -> usize {
        self.map.len()
    }
}

/// stat/hash 缓存（线程安全；SQLite 部分可选）
pub struct HashCache {
    mem: Mutex<MemLru>,
    db: Option<OnceLock<Mutex<rusqlite::Connection>>>,
}

impl std::fmt::Debug for HashCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HashCache")
            .field("sqlite", &self.db.is_some())
            .finish()
    }
}

impl HashCache {
    /// 仅内存缓存（进程内复用；测试默认）
    pub fn in_memory() -> Self {
        HashCache { mem: Mutex::new(MemLru::default()), db: None }
    }

    /// 打开（或创建）SQLite 落盘缓存。表结构与失效规则见模块文档。
    pub fn open_sqlite(path: &std::path::Path) -> Result<Self, CacheError> {
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS hash_cache (
                 path     TEXT    NOT NULL,
                 size     INTEGER NOT NULL,
                 mtime_ms INTEGER NOT NULL,
                 algo     TEXT    NOT NULL DEFAULT 'crc32',
                 crc32    INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY (path, size, mtime_ms, algo)
             );",
        )?;
        let db = OnceLock::new();
        let _ = db.set(Mutex::new(conn));
        Ok(HashCache { mem: Mutex::new(MemLru::default()), db: Some(db) })
    }

    /// 查询（内存优先，SQLite 兜底并回填内存）
    pub fn get(&self, path: &str, size: u64, mtime_ms: u64) -> Option<u32> {
        {
            let mut mem = self.mem.lock().expect("缓存锁");
            if let Some(crc) = mem.get(path, size, mtime_ms) {
                return Some(crc);
            }
        }
        if let Some(db) = &self.db {
            let conn = db.get().expect("SQLite 初始化").lock().expect("SQLite 锁");
            let crc: Result<Option<i64>, _> = conn
                .query_row(
                    "SELECT crc32 FROM hash_cache WHERE path = ?1 AND size = ?2 AND mtime_ms = ?3 AND algo = 'crc32'",
                    rusqlite::params![path, size as i64, mtime_ms as i64],
                    |row| row.get(0),
                );
            if let Ok(Some(crc)) = crc {
                let crc = u32::try_from(crc).unwrap_or(0);
                self.mem.lock().expect("缓存锁").put(path, size, mtime_ms, crc);
                return Some(crc);
            }
        }
        None
    }

    /// 写入（内存 + SQLite；SQLite 写失败静默忽略——缓存不影响正确性）
    pub fn put(&self, path: &str, size: u64, mtime_ms: u64, crc32: u32) {
        self.mem.lock().expect("缓存锁").put(path, size, mtime_ms, crc32);
        if let Some(db) = &self.db {
            let conn = db.get().expect("SQLite 初始化").lock().expect("SQLite 锁");
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let _ = conn.execute(
                "INSERT INTO hash_cache (path, size, mtime_ms, algo, crc32, updated_at)
                 VALUES (?1, ?2, ?3, 'crc32', ?4, ?5)
                 ON CONFLICT(path, size, mtime_ms, algo) DO UPDATE SET crc32 = ?4, updated_at = ?5",
                rusqlite::params![path, size as i64, mtime_ms as i64, crc32 as i64, now],
            );
        }
    }

    /// 显式失效：清除某路径全部历史版本
    pub fn invalidate(&self, path: &str) {
        self.mem.lock().expect("缓存锁").invalidate(path);
        if let Some(db) = &self.db {
            let conn = db.get().expect("SQLite 初始化").lock().expect("SQLite 锁");
            let _ = conn.execute("DELETE FROM hash_cache WHERE path = ?1", rusqlite::params![path]);
        }
    }

    /// 内存条目数（测试/诊断）
    pub fn mem_len(&self) -> usize {
        self.mem.lock().expect("缓存锁").len()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("SQLite 打开/建表失败: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_lru_hit_miss_and_invalidate() {
        let c = HashCache::in_memory();
        assert_eq!(c.get("/a", 10, 100), None);
        c.put("/a", 10, 100, 0xdead_beef);
        assert_eq!(c.get("/a", 10, 100), Some(0xdead_beef));
        // size 变化 -> 键不匹配失效
        assert_eq!(c.get("/a", 11, 100), None);
        assert_eq!(c.mem_len(), 0, "键不匹配即移除");
        c.put("/a", 11, 100, 1);
        c.invalidate("/a");
        assert_eq!(c.get("/a", 11, 100), None);
    }

    #[test]
    fn lru_capacity_eviction() {
        let c = HashCache::in_memory();
        for i in 0..(LRU_CAPACITY as u64 + 10) {
            c.put(&format!("/f{i}"), i, 1, 42);
        }
        assert!(c.mem_len() <= LRU_CAPACITY);
        // 早期的条目被驱逐
        assert_eq!(c.get("/f0", 0, 1), None);
        // 新条目命中
        assert_eq!(c.get(&format!("/f{}", LRU_CAPACITY + 9), (LRU_CAPACITY + 9) as u64, 1), Some(42));
    }

    #[test]
    fn sqlite_persists_across_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("cache.db");
        {
            let c = HashCache::open_sqlite(&db_path).unwrap();
            assert_eq!(c.get("/x", 5, 500), None);
            c.put("/x", 5, 500, 0x1234);
        }
        // "会话 2"：重新打开同一库
        let c2 = HashCache::open_sqlite(&db_path).unwrap();
        assert_eq!(c2.get("/x", 5, 500), Some(0x1234), "SQLite 缓存跨会话复用");
        // size/mtime 变化 -> 失效（键不命中）
        assert_eq!(c2.get("/x", 6, 500), None);
        // invalidate 清除
        c2.invalidate("/x");
        assert_eq!(c2.get("/x", 5, 500), None);
    }

    #[test]
    fn sqlite_updates_on_new_version() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("cache.db");
        let c = HashCache::open_sqlite(&db_path).unwrap();
        c.put("/x", 5, 500, 0x1111);
        // 同路径新版本（size 不同）写入 -> 覆盖为新键行
        c.put("/x", 6, 500, 0x2222);
        assert_eq!(c.get("/x", 5, 500), Some(0x1111));
        assert_eq!(c.get("/x", 6, 500), Some(0x2222));
    }
}
