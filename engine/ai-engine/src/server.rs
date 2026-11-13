//! stdio 服务端：读循环、分发、串行链、取消注册表、录制、panic 边界。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use ai_protocol::{self, error::codes, RpcResponse};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, Mutex};

use crate::handlers::{dispatch, HandlerCtx, Handled};
use crate::log;
use crate::ServerOptions;

/// 目录比较会话数据
#[derive(Debug, Clone)]
pub struct DirSessionData {
    pub scan: ai_dir::DirScanResult,
}

/// 文本比较会话数据
#[derive(Debug, Clone)]
pub struct TextSessionData {
    pub sections: Vec<ai_protocol::Section>,
    pub summary: ai_protocol::DiffSummary,
}

#[derive(Debug, Clone)]
pub enum Session {
    Dir(DirSessionData),
    Text(TextSessionData),
}

pub type SessionMapInner = HashMap<String, Session>;
pub type SessionMap = Arc<Mutex<SessionMapInner>>;

/// 取消注册表：请求 id key -> 取消旗标
#[derive(Default)]
pub struct CancelRegistry {
    flags: StdMutex<HashMap<String, Arc<AtomicBool>>>,
}

impl CancelRegistry {
    pub fn new() -> Self {
        CancelRegistry { flags: StdMutex::new(HashMap::new()) }
    }

    pub fn register(&self, key: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.flags.lock().expect("取消注册表锁").insert(key.to_string(), Arc::clone(&flag));
        flag
    }

    pub fn cancel(&self, key: &str) {
        if let Some(flag) = self.flags.lock().expect("取消注册表锁").get(key) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    pub fn unregister(&self, key: &str) {
        self.flags.lock().expect("取消注册表锁").remove(key);
    }

    pub fn len(&self) -> usize {
        self.flags.lock().expect("取消注册表锁").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 出站消息
#[derive(Debug)]
pub enum Outbound {
    Response(RpcResponse),
    Notification(ai_protocol::RpcServerNotification),
}

/// 录制器：`--record <file>` 双向 NDJSON
struct Recorder {
    file: Option<StdMutex<std::io::BufWriter<std::fs::File>>>,
}

impl Recorder {
    fn open(path: Option<&std::path::Path>) -> Self {
        let file = path.and_then(|p| {
            std::fs::File::create(p)
                .ok()
                .map(|f| StdMutex::new(std::io::BufWriter::new(f)))
        });
        Recorder { file }
    }

    fn record(&self, dir: &str, value: &serde_json::Value) {
        if let Some(mutex) = &self.file {
            use std::io::Write;
            let mut w = mutex.lock().expect("录制锁");
            let _ = serde_json::to_writer(&mut *w, &serde_json::json!({ "dir": dir, "msg": value }));
            let _ = w.write_all(b"\n");
            let _ = w.flush();
        }
    }
}

/// 运行 stdio 服务端直到 EOF / shutdown。
pub async fn run_stdio(opts: ServerOptions) -> std::io::Result<()> {
    let max_batch = opts.max_delta_batch_rows();
    let ctx = HandlerCtx::new(max_batch);
    let cancels = Arc::new(CancelRegistry::new());
    let recorder = Arc::new(Recorder::open(opts.record_path.as_deref()));

    let (tx, rx) = mpsc::unbounded_channel::<Outbound>();
    let writer = tokio::spawn(writer_task(rx, tokio::io::stdout(), Arc::clone(&recorder)));

    log::info("stdio 服务端启动（NDJSON / JSON-RPC 2.0）");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    // 串行链：后一个请求先 await 前一个请求任务完成（保证输出确定性）
    let mut chain_prev: Option<tokio::task::JoinHandle<()>> = None;

    loop {
        let line = match lines.next_line().await? {
            Some(l) => l,
            None => break, // EOF：管道关闭
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                // 解析失败无法定位会话/顺序，直接应答（id 未知，RFC 允许 null）
                log::warn(format!("解析失败: {e}"));
                let resp = RpcResponse::failure(
                    ai_protocol::RequestId::Number(0),
                    ai_protocol::error_object(codes::PARSE_ERROR, format!("JSON 解析失败: {e}")),
                );
                send_outbound(&tx, Outbound::Response(resp));
                continue;
            }
        };
        recorder.record("in", &value);

        let has_id = value.get("id").is_some();

        if has_id {
            // ---- 请求 ----
            let req: ai_protocol::RpcRequest = match serde_json::from_value(value) {
                Ok(r) => r,
                Err(e) => {
                    log::warn(format!("请求信封不合法: {e}"));
                    send_outbound(
                        &tx,
                        Outbound::Response(RpcResponse::failure(
                            ai_protocol::RequestId::Number(0),
                            ai_protocol::error_object(codes::INVALID_REQUEST, format!("请求信封不合法: {e}")),
                        )),
                    );
                    continue;
                }
            };
            if req.method == ai_protocol::method::SHUTDOWN {
                // shutdown 也走串行链：应答必须排在先到达请求的全部输出之后；
                // 随后停止读入，等待在途任务收尾（LSP 语义：shutdown 后退出）
                let id = req.id;
                let prev = chain_prev.take();
                let tx2 = tx.clone();
                chain_prev = Some(tokio::spawn(async move {
                    if let Some(p) = prev {
                        let _ = p.await;
                    }
                    send_outbound(&tx2, Outbound::Response(RpcResponse::success(id, serde_json::Value::Null)));
                }));
                break;
            }
            let id = req.id.clone();
            let key = id.cancel_key();
            let cancel_flag = cancels.register(&key);
            let fut = process_request(req, clone_ctx(&ctx), Arc::clone(&cancels), cancel_flag, tx.clone());
            let prev = chain_prev.take();
            chain_prev = Some(tokio::spawn(async move {
                if let Some(p) = prev {
                    let _ = p.await; // 前序任务完成（panic 也继续，见下方 waiter）
                }
                fut.await;
            }));
        } else {
            // ---- 通知 ----
            let notif: ai_protocol::RpcNotification = match serde_json::from_value(value) {
                Ok(n) => n,
                Err(e) => {
                    log::warn(format!("通知信封不合法: {e}"));
                    continue;
                }
            };
            if notif.method == ai_protocol::method::CANCEL {
                match serde_json::from_value::<ai_protocol::CancelParams>(
                    notif.params.unwrap_or(serde_json::Value::Null),
                ) {
                    Ok(p) => {
                        log::info(format!("收到取消: {}", p.id));
                        cancels.cancel(&p.id.cancel_key());
                    }
                    Err(e) => log::warn(format!("$/cancel 参数不合法: {e}")),
                }
            } else {
                log::debug(format!("忽略通知: {}", notif.method));
            }
        }
    }

    // 等待在途请求（至多一个）收尾
    if let Some(prev) = chain_prev {
        let _ = prev.await;
    }
    drop(tx);
    let _ = writer.await;
    log::info("stdio 服务端退出");
    Ok(())
}

/// HandlerCtx 内部均为 Arc，克隆廉价（请求任务需要拥有所有权）
fn clone_ctx(ctx: &HandlerCtx) -> HandlerCtx {
    HandlerCtx {
        sessions: Arc::clone(&ctx.sessions),
        vfs: Arc::clone(&ctx.vfs),
        max_batch_rows: ctx.max_batch_rows,
    }
}

/// 单个请求的处理：分发 -> 出站入队；panic 捕获为 InternalError 应答。
async fn process_request(
    req: ai_protocol::RpcRequest,
    ctx: HandlerCtx,
    cancels: Arc<CancelRegistry>,
    cancel_flag: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<Outbound>,
) {
    let id = req.id.clone();
    let key = id.cancel_key();
    let result = dispatch(&req.method, req.params.clone(), &ctx, &ai_dir::CancelFlag(cancel_flag)).await;
    cancels.unregister(&key);
    match result {
        Ok(Handled::Value(v)) => {
            send_outbound(&tx, Outbound::Response(RpcResponse::success(id, v)));
        }
        Ok(Handled::WithNotifications { response, notifications }) => {
            send_outbound(&tx, Outbound::Response(RpcResponse::success(id, response)));
            for n in notifications {
                send_outbound(&tx, Outbound::Notification(n));
            }
        }
        Err(err) => {
            log::warn(format!("{} {} 失败: code={} {}", req.method, id, err.code, err.message));
            send_outbound(&tx, Outbound::Response(RpcResponse::failure(id, err)));
        }
    }
}

async fn writer_task(
    mut rx: mpsc::UnboundedReceiver<Outbound>,
    stdout: tokio::io::Stdout,
    recorder: Arc<Recorder>,
) {
    use tokio::io::AsyncWriteExt;
    let mut out = stdout;
    while let Some(msg) = rx.recv().await {
        // 录制点：录制文件顺序 == stdout 写出顺序
        let (value, line) = match &msg {
            Outbound::Response(r) => (
                serde_json::to_value(r),
                serde_json::to_string(r),
            ),
            Outbound::Notification(n) => (
                serde_json::to_value(n),
                serde_json::to_string(n),
            ),
        };
        match line {
            Ok(mut s) => {
                if let Ok(v) = value {
                    recorder.record("out", &v);
                }
                s.push('\n');
                if out.write_all(s.as_bytes()).await.is_err() {
                    break;
                }
                let _ = out.flush().await;
            }
            Err(e) => {
                log::error(format!("出站消息序列化失败: {e}"));
            }
        }
    }
    let _ = out.flush().await;
}

fn send_outbound(tx: &mpsc::UnboundedSender<Outbound>, msg: Outbound) {
    if tx.send(msg).is_err() {
        log::error("输出通道已关闭（客户端断开）");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_registry_lifecycle() {
        let reg = CancelRegistry::new();
        let key = "n:1";
        let flag = reg.register(key);
        assert!(!reg.is_empty());
        assert!(!flag.load(Ordering::SeqCst));
        reg.cancel(key);
        assert!(flag.load(Ordering::SeqCst));
        reg.unregister(key);
        assert!(reg.is_empty());
        // 未注册的 key 取消是 no-op
        reg.cancel("n:999");
    }

    #[test]
    fn recorder_writes_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.ndjson");
        {
            let rec = Recorder::open(Some(&path));
            rec.record("in", &serde_json::json!({"method": "initialize"}));
            rec.record("out", &serde_json::json!({"result": null}));
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with(r#"{"dir":"in""#));
        assert!(lines[1].starts_with(r#"{"dir":"out""#));
    }
}
