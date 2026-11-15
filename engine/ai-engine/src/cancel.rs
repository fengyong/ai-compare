//! 取消注册表（R3-22）：请求级与会话级双层生命周期。
//!
//! 之前只有一张 `request id -> flag` 表，且请求应答后立即注销，导致：
//! - `dir/open` 应答后仍在跑的后台扫描无法按原请求 id 取消；
//! - `session/close` 与会话级取消没有独立的旗标归属。
//!
//! 现在同一个 `Arc<AtomicBool>` 可以同时挂在两张索引上：
//! `dir/open`（P2 模型路径）把请求旗标「提升」为会话旗标，DirModel 的后台
//! worker 持有的正是这个 Arc。于是：
//! - `$/cancel {id}` 在请求在途时置位（请求结束注销 request 索引）；
//! - `$/cancel {sessionId}` 在应答后仍能置位，后台 worker / pump 感知退出；
//! - `session/close` 注销会话索引并置位，相关后台任务终止。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

/// 旗标索引：`requests` 按 `RequestId::cancel_key()`，`sessions` 按 sessionId。
#[derive(Default)]
pub struct CancelRegistry {
    requests: StdMutex<HashMap<String, Arc<AtomicBool>>>,
    sessions: StdMutex<HashMap<String, Arc<AtomicBool>>>,
}

impl CancelRegistry {
    pub fn new() -> Self {
        CancelRegistry {
            requests: StdMutex::new(HashMap::new()),
            sessions: StdMutex::new(HashMap::new()),
        }
    }

    /// 注册在途请求，返回交给 handler 的旗标。
    pub fn register_request(&self, key: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.requests
            .lock()
            .expect("取消注册表锁")
            .insert(key.to_string(), Arc::clone(&flag));
        flag
    }

    /// 是否命中一个在途请求（未命中说明请求已应答或 id 不合法）。
    pub fn cancel_request(&self, key: &str) -> bool {
        let Some(flag) = self
            .requests
            .lock()
            .expect("取消注册表锁")
            .get(key)
            .cloned()
        else {
            return false;
        };
        flag.store(true, Ordering::SeqCst);
        true
    }

    pub fn unregister_request(&self, key: &str) {
        self.requests.lock().expect("取消注册表锁").remove(key);
    }

    /// 把已有旗标绑定为某会话的后台扫描旗标（dir/open 成功后调用）。
    ///
    /// 为什么传旗标而不是请求 key：handler 只有 `CancelFlag`（其 `.0` 就是
    /// 请求旗标），拿不到请求 id；而且这样绑定后两者物理上就是同一个 Arc，
    /// DirModel 的 worker 无需改动即可被会话级取消置位。
    pub fn bind_session_flag(&self, session_id: &str, flag: Arc<AtomicBool>) {
        self.sessions
            .lock()
            .expect("会话取消注册表锁")
            .insert(session_id.to_string(), flag);
    }

    /// 是否命中一个会话（未命中说明会话不存在或已关闭）。
    pub fn cancel_session(&self, session_id: &str) -> bool {
        let Some(flag) = self
            .sessions
            .lock()
            .expect("会话取消注册表锁")
            .get(session_id)
            .cloned()
        else {
            return false;
        };
        flag.store(true, Ordering::SeqCst);
        true
    }

    pub fn unregister_session(&self, session_id: &str) {
        self.sessions
            .lock()
            .expect("会话取消注册表锁")
            .remove(session_id);
    }

    /// EOF / shutdown 时停止全部会话级后台任务（pump 由此退出）。
    pub fn cancel_all_sessions(&self) -> usize {
        let flags: Vec<Arc<AtomicBool>> = self
            .sessions
            .lock()
            .expect("会话取消注册表锁")
            .values()
            .cloned()
            .collect();
        for flag in &flags {
            flag.store(true, Ordering::SeqCst);
        }
        flags.len()
    }

    #[cfg(test)]
    pub fn request_len(&self) -> usize {
        self.requests.lock().expect("取消注册表锁").len()
    }

    #[cfg(test)]
    pub fn session_len(&self) -> usize {
        self.sessions.lock().expect("会话取消注册表锁").len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.request_len() == 0 && self.session_len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_flag_is_cancelled_and_unregistered() {
        let reg = CancelRegistry::new();
        let flag = reg.register_request("n:1");
        assert_eq!(reg.request_len(), 1);
        assert!(!flag.load(Ordering::SeqCst));
        assert!(reg.cancel_request("n:1"));
        assert!(flag.load(Ordering::SeqCst));
        reg.unregister_request("n:1");
        assert_eq!(reg.request_len(), 0);
        assert!(!reg.cancel_request("n:1"), "已注销的请求不能再被取消");
        assert!(!reg.cancel_request("n:999"));
    }

    #[test]
    fn session_flag_survives_request_unregistration() {
        // R3-22 核心：dir/open 应答后请求索引注销，但会话索引仍持有同一个
        // Arc，因此后台扫描仍可按 sessionId 取消。
        let reg = CancelRegistry::new();
        let flag = reg.register_request("n:2");
        reg.bind_session_flag("d1", Arc::clone(&flag));
        reg.unregister_request("n:2");
        assert_eq!(reg.request_len(), 0);
        assert_eq!(reg.session_len(), 1);
        assert!(!reg.cancel_request("n:2"), "应答后按旧请求 id 取消应无效");
        assert!(!flag.load(Ordering::SeqCst));
        assert!(reg.cancel_session("d1"));
        assert!(
            flag.load(Ordering::SeqCst),
            "会话级取消必须置位后台扫描旗标"
        );
        reg.unregister_session("d1");
        assert!(reg.is_empty());
        assert!(!reg.cancel_session("d1"));
    }

    #[test]
    fn cancel_all_sessions_stops_every_background_flag() {
        let reg = CancelRegistry::new();
        let a = Arc::new(AtomicBool::new(false));
        let b = Arc::new(AtomicBool::new(false));
        reg.bind_session_flag("d1", Arc::clone(&a));
        reg.bind_session_flag("d2", Arc::clone(&b));
        assert_eq!(reg.cancel_all_sessions(), 2);
        assert!(a.load(Ordering::SeqCst));
        assert!(b.load(Ordering::SeqCst));
        assert_eq!(reg.cancel_all_sessions(), 2, "重复调用幂等");
    }

    #[test]
    fn reopening_session_replaces_cancelled_flag() {
        let reg = CancelRegistry::new();
        let old = reg.register_request("n:1");
        reg.bind_session_flag("d1", Arc::clone(&old));
        reg.cancel_session("d1");
        // close 后重开：新旗标覆盖旧会话条目，不能残留已取消状态
        reg.unregister_session("d1");
        let fresh = reg.register_request("n:2");
        reg.bind_session_flag("d1", Arc::clone(&fresh));
        assert!(!fresh.load(Ordering::SeqCst), "新会话不得继承旧取消状态");
        assert_eq!(reg.session_len(), 1);
    }
}
