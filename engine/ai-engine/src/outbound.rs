//! 出站背压（R3-22 工作包 B）：有界队列 + 满载等待 + 关闭解除阻塞。
//!
//! 修复前是 `mpsc::unbounded_channel::<Outbound>()`：后台 pump 与请求链持续
//! 把 delta/响应塞进队列，只要读者变慢或停止读取（例如桌面端 WebView 卡住、
//! 管道缓冲区写满），队列就无界增长，引擎内存随之无限上升。
//!
//! 本模块把出站队列固定为 `OUTBOUND_CAPACITY` 条。满载时生产者**等待**
//! （不丢消息、不合并、不重排响应），并留下一次性日志说明正在背压。
//! 两种退出信号可解除等待：
//! - 会话级取消旗标：pump 用它，EOF 时 `cancel_all_dir_sessions` 置位即可退出；
//! - 进程级 `Shutdown`：请求链 / stdout 写入卡死超过有界时限时触发。
//!
//! 正常路径（读者持续消费）两种信号都不触发，所有响应与通知完整写出。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

use crate::server::Outbound;

/// 出站队列容量（条）。单条上限由 `max_batch_rows` 决定，二者相乘即内存上界。
pub const OUTBOUND_CAPACITY: usize = 128;

/// 背压/取消检查周期（满载时生产者在此粒度内响应关闭信号）
pub const BACKPRESSURE_POLL: Duration = Duration::from_millis(100);

/// 进程级关闭广播。用 `watch` 而非 `Notify`：晚订阅者也能立刻看到已触发的状态。
pub struct Shutdown {
    tx: watch::Sender<bool>,
}

impl Shutdown {
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(false);
        Shutdown { tx }
    }

    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.tx.subscribe()
    }

    pub fn fire(&self) {
        let _ = self.tx.send(true);
    }

    pub fn is_fired(&self) -> bool {
        *self.tx.borrow()
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

/// 有界出站发送端（Clone 后共享同一队列、同一关闭信号、同一日志去重位）。
#[derive(Clone)]
pub struct OutboundTx {
    tx: mpsc::Sender<Outbound>,
    shutdown: Arc<Shutdown>,
    capacity: usize,
    logged: Arc<AtomicBool>,
}

impl OutboundTx {
    pub fn new(capacity: usize, shutdown: Arc<Shutdown>) -> (Self, mpsc::Receiver<Outbound>) {
        assert!(capacity > 0, "出站队列容量必须为正");
        let (tx, rx) = mpsc::channel(capacity);
        (
            OutboundTx {
                tx,
                shutdown,
                capacity,
                logged: Arc::new(AtomicBool::new(false)),
            },
            rx,
        )
    }

    #[cfg(test)]
    pub fn for_test(capacity: usize) -> (Self, mpsc::Receiver<Outbound>) {
        Self::new(capacity, Arc::new(Shutdown::new()))
    }

    /// 发送一条出站消息，返回 false 表示通道已关闭或收到取消/关闭信号。
    ///
    /// `reserve()` 是 cancellation-safe 的：满载时 select 取消它不会让消息
    /// 入队，也不会改变顺序；拿到 permit 后一次性写入，响应与通知共用同一
    /// FIFO，不重排。
    pub async fn send(&self, msg: Outbound) -> bool {
        self.send_with_cancel(msg, None).await
    }

    /// 带会话级取消旗标的发送（pump 用）：旗标置位后不再无限等待慢读者。
    pub async fn send_with_cancel(&self, msg: Outbound, cancel: Option<&AtomicBool>) -> bool {
        // watch 新订阅者的初值就是当前状态，`changed()` 只报告之后的变化；
        // 因此先显式检查一次，避免"已关闭后新发起的 send"要多等一个轮询周期。
        if self.shutdown.is_fired() {
            return false;
        }
        let mut shutdown_rx = self.shutdown.subscribe();
        let mut pending = Some(msg);
        loop {
            tokio::select! {
                res = self.tx.reserve() => {
                    return match res {
                        Ok(permit) => {
                            permit.send(pending.take().expect("未完成消息仍在"));
                            true
                        }
                        Err(_) => {
                            crate::log::error("输出通道已关闭（客户端断开）");
                            false
                        }
                    };
                }
                _ = shutdown_rx.changed() => {
                    crate::log::warn("出站等待被关闭信号解除，放弃该条出站消息");
                    return false;
                }
                _ = tokio::time::sleep(BACKPRESSURE_POLL) => {
                    let cancelled = cancel.is_some_and(|f| f.load(Ordering::SeqCst));
                    if cancelled || self.shutdown.is_fired() {
                        crate::log::info("出站等待被取消，放弃该条出站消息");
                        return false;
                    }
                    if !self.logged.swap(true, Ordering::SeqCst) {
                        crate::log::warn(format!(
                            "出站背压：队列已满（容量 {} 条），生产者等待读者消费",
                            self.capacity
                        ));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::Outbound;
    use ai_protocol::RpcResponse;
    use serde_json::json;
    use std::sync::atomic::AtomicBool;

    fn msg(n: i64) -> Outbound {
        Outbound::Response(RpcResponse::success(
            ai_protocol::RequestId::Number(n),
            json!({}),
        ))
    }

    fn drained(rx: &mut mpsc::Receiver<Outbound>) -> usize {
        let mut n = 0;
        while rx.try_recv().is_ok() {
            n += 1;
        }
        n
    }

    /// 队列容量就是硬上界：多出来的 send 必须等待，而不是被无界缓存。
    #[tokio::test]
    async fn bounded_queue_blocks_producer_at_capacity() {
        let (tx, mut rx) = OutboundTx::for_test(2);
        assert!(tx.send(msg(1)).await);
        assert!(tx.send(msg(2)).await);
        // 第三条在读者消费前不能完成（等待，不丢、不排队到无限）
        let blocked = tokio::time::timeout(Duration::from_millis(200), tx.send(msg(3))).await;
        assert!(blocked.is_err(), "容量 2 已满时第三条必须等待");

        assert_eq!(drained(&mut rx), 2, "队列内只应有容量上限条消息");
        // 读者消费后，等待中的生产者会自动解除并完成
        let late = tokio::spawn({
            let tx = tx.clone();
            async move { tx.send(msg(3)).await }
        });
        assert!(late.await.unwrap(), "解除背压后消息应成功入队");
        assert_eq!(drained(&mut rx), 1);
    }

    /// 慢读者恢复后所有消息按顺序送达（最终状态收敛、响应不重排）。
    #[tokio::test]
    async fn slow_reader_eventually_receives_every_message_in_order() {
        let (tx, mut rx) = OutboundTx::for_test(3);
        let sender = tokio::spawn(async move {
            for n in 1..=12 {
                assert!(tx.send(msg(n)).await, "第 {n} 条不应丢失");
            }
            drop(tx);
        });
        // 读者故意延迟启动，模拟暂停读者：这段时间 sender 应被容量卡住
        tokio::time::sleep(Duration::from_millis(120)).await;
        let reader = tokio::spawn(async move {
            let mut ids = Vec::new();
            while let Some(m) = rx.recv().await {
                if let Outbound::Response(r) = m {
                    ids.push(r.id);
                }
            }
            ids
        });
        sender.await.unwrap();
        let ids = reader.await.unwrap();
        assert_eq!(ids.len(), 12, "一条都不能丢");
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(
                *id,
                ai_protocol::RequestId::Number(i as i64 + 1),
                "响应顺序不得重排"
            );
        }
    }

    /// 会话级取消能解除满载等待（EOF 时 pump 不会被慢读者永久挂住）。
    #[tokio::test]
    async fn cancel_flag_unblocks_full_queue() {
        let (tx, _rx) = OutboundTx::for_test(1);
        assert!(tx.send(msg(1)).await);
        let cancel = Arc::new(AtomicBool::new(false));
        let gate = Arc::clone(&cancel);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            gate.store(true, Ordering::SeqCst);
        });
        let started = std::time::Instant::now();
        let ok = tx.send_with_cancel(msg(2), Some(&cancel)).await;
        assert!(!ok, "取消后应放弃等待中的消息");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "取消必须在有界时间内解除阻塞: {:?}",
            started.elapsed()
        );
    }

    /// 进程级关闭信号能解除满载等待，且晚订阅者也能立刻看到状态。
    #[tokio::test]
    async fn shutdown_unblocks_full_queue() {
        let shutdown = Arc::new(Shutdown::new());
        let (tx, _rx) = OutboundTx::new(1, Arc::clone(&shutdown));
        assert!(tx.send(msg(1)).await);
        let fire = Arc::clone(&shutdown);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            fire.fire();
        });
        let res = tokio::time::timeout(Duration::from_secs(2), tx.send(msg(2))).await;
        assert!(res.is_ok(), "关闭信号必须解除出站等待");
        assert!(!res.unwrap(), "被解除的 send 返回 false");
        assert!(shutdown.is_fired());
        // 关闭后再订阅也应立刻观察到（watch 初值即当前状态）
        let mut late = shutdown.subscribe();
        assert!(*late.borrow_and_update(), "晚订阅者应看到已触发的关闭状态");
        // 已关闭后新发起的 send 应立即放弃，不再等一个背压轮询周期
        assert!(!tx.send(msg(3)).await);
    }
}
