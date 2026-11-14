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
#[derive(Clone, Default)]
pub struct DirSessionData {
    /// P2 会话模型（lazyScan/过滤器/平铺/对齐路径）
    pub model: Option<Arc<ai_dir::DirModel>>,
    /// P0 同步扫描结果（旧载荷路径）
    pub scan: Option<ai_dir::DirScanResult>,
    /// P9：两侧根路径（dir/execute 执行文件操作用；DirModel.roots 私有，
    /// 由 dir/open 打开时记录）
    pub roots: Option<(ai_vfs::VfsPath, ai_vfs::VfsPath)>,
}

/// 文本比较会话数据
///
/// P3 起保存两侧行表与完整 diff 配置：text/lines（行分页）、text/inlineDiff
/// （懒计算）、重要性重判（ImportanceUpdate）与逐行双键（withKeys）均以
/// 本结构为数据源（docs/03 §2.3.2 / §4.1）。
#[derive(Debug, Clone)]
pub struct TextSessionData {
    /// 骨架 diff 结果（core 域；importance 可被 text/sections importance 重判更新）
    pub sections: Vec<ai_core::sections::Section>,
    pub summary: ai_core::DiffSummary,
    /// 两侧行表（text/lines 分页与双键计算的数据源）
    pub left: ai_core::LineTable,
    pub right: ai_core::LineTable,
    /// 打开会话时生效的完整配置（骨架选项 + 重要性模型 + 手动对齐）
    pub config: ai_core::DiffConfig,
    /// P11：两侧标签（打开时的路径原串；report/generate 报告头用）
    pub labels: (String, String),
    /// 会话内单调递增版本号（重要性重判等模型变更时 +1）
    pub rev: u64,
    /// R3-04：两侧打开时是否带 UTF-8 BOM（保存时原样写回，FR-TXT-013）
    pub boms: (bool, bool),
    /// R3-04：两侧是否有未落盘编辑（text/edit 置位，text/save 落盘后清除）
    pub modified: (bool, bool),
}

/// 三向合并会话数据（P7，text/merge3/*）
///
/// 模型不可变（打开时一次求出）；决策表可变（text/merge3/decide 提交），
/// 输出由 model + decisions 确定性合成（幂等）。
#[derive(Debug, Clone)]
pub struct MergeSessionData {
    /// 三方行表（块行内容与双键数据源）
    pub base: ai_core::LineTable,
    pub left: ai_core::LineTable,
    pub right: ai_core::LineTable,
    /// 合并模型（有序块序列；打开时一次求出）
    pub model: ai_core::merge::MergeModel,
    /// 打开会话时生效的合并选项（未决冲突默认策略等）
    pub options: ai_core::merge::MergeOptions,
    /// marker 标签（(base, left, right)，取打开时的路径原串）
    pub labels: (String, String, String),
    /// 块决策表（chunkId -> 决策；显式决策覆盖 auto_decision）
    pub decisions: std::collections::HashMap<u32, ai_core::merge::MergeDecision>,
    /// 会话内单调递增版本号（决策内容变化时 +1）
    pub rev: u64,
}

/// 表格比较会话数据（P9，table/open|sections）
///
/// 模型不可变（打开时一次求出）；行内容按需分页。
#[derive(Debug, Clone)]
pub struct TableSessionData {
    /// 两侧解析后的表格数据（sections 行内容数据源）
    pub left: ai_core::table::TableData,
    pub right: ai_core::table::TableData,
    /// diff 模型（比较列 + diff 行序列 + 统计）
    pub model: ai_core::table::TableDiffModel,
    /// 打开会话时生效的选项
    pub options: ai_core::table::TableDiffOptions,
    /// P11：两侧标签（打开时的路径原串；report/generate 报告头用）
    pub labels: (String, String),
}

/// 十六进制比较会话数据（P9，hex/open|bytes）
///
/// 字节流驻内存 + 视窗分页查询（大文件策略：UI 虚拟网格按需拉取，
/// docs/03 §2.3.2 大内容约定）。
#[derive(Debug, Clone)]
pub struct HexSessionData {
    pub left: Vec<u8>,
    pub right: Vec<u8>,
    /// diff 模型（块序列 + 统计）
    pub model: ai_core::hex::HexDiffModel,
    /// 每字节状态图（same/changed/inserted/deleted；hex/bytes 着色数据源）
    pub status_left: Vec<ai_core::hex::HexByteStatus>,
    pub status_right: Vec<ai_core::hex::HexByteStatus>,
    /// 打开会话时生效的选项
    pub options: ai_core::hex::HexOptions,
    /// P11：两侧标签（打开时的路径原串；report/generate 报告头用）
    pub labels: (String, String),
}

/// P14：图片比较会话（差异结果全驻内存；块数上限保护在 open 时截断）
#[derive(Debug, Clone)]
pub struct PictureSessionData {
    pub result: ai_core::picture::PictureDiffResult,
    /// 打开会话时的选项
    pub options: ai_core::picture::PictureDiffOptions,
    /// 两侧标签（打开时的路径原串）
    pub labels: (String, String),
}

/// R3-07：文件夹三向合并会话（dir/mergeScan|mergePlan|mergeExecute）
///
/// 扫描 arena 打开时一次求出；plan 是纯函数（决策 -> 计划），execute 复用
/// ai-dir execute 层的护栏/部分成功/取消语义。output 根仅在 target=output 时需要。
#[derive(Debug, Clone)]
pub struct DirMergeSessionData {
    pub roots: ai_dir::MergeRoots,
    pub scan: ai_dir::MergeScanResult,
}

#[derive(Clone)]
pub enum Session {
    Dir(DirSessionData),
    Text(TextSessionData),
    Merge(MergeSessionData),
    DirMerge(DirMergeSessionData),
    Table(TableSessionData),
    Hex(HexSessionData),
    Picture(PictureSessionData),
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
    /// 打开录制目标；失败时返回原因供调用方告警。
    ///
    /// 为什么不能静默禁用：用户以为在录制、实际无文件，问题现场无法复现。
    fn open(path: Option<&std::path::Path>) -> (Self, Option<String>) {
        let Some(p) = path else { return (Recorder { file: None }, None) };
        match std::fs::File::create(p) {
            Ok(f) => (Recorder { file: Some(StdMutex::new(std::io::BufWriter::new(f))) }, None),
            Err(e) => (Recorder { file: None }, Some(format!("{}: {e}", p.display()))),
        }
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
    let (recorder, record_err) = Recorder::open(opts.record_path.as_deref());
    if let Some(msg) = record_err {
        log::warn(format!("--record 录制已禁用（{msg}）"));
    }
    let recorder = Arc::new(recorder);

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
                // 解析失败无法定位请求 id：按 JSON-RPC 2.0 §4 应答 "id": null。
                // 此前固定回 id=0，会与真实 id=0 的请求撞号（客户端幽灵应答）。
                log::warn(format!("解析失败: {e}"));
                let resp = RpcResponse::failure(
                    ai_protocol::RequestId::Null(None),
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
            // 先取原始 id（from_value 会消费 value）：信封非法时尽量回显原 id。
            let raw_id = value.get("id").cloned();
            let req: ai_protocol::RpcRequest = match serde_json::from_value(value) {
                Ok(r) => r,
                Err(e) => {
                    log::warn(format!("请求信封不合法: {e}"));
                    // 信封非法但 id 可能合法：尽量回显原 id；id 不可解析时回 null。
                    let id = raw_id
                        .and_then(|v| serde_json::from_value::<ai_protocol::RequestId>(v).ok())
                        .unwrap_or(ai_protocol::RequestId::Null(None));
                    send_outbound(
                        &tx,
                        Outbound::Response(RpcResponse::failure(
                            id,
                            ai_protocol::error_object(codes::INVALID_REQUEST, format!("请求信封不合法: {e}")),
                        )),
                    );
                    continue;
                }
            };
            // JSON-RPC 2.0 §4：jsonrpc 成员必须为 "2.0"，否则 Invalid Request。
            if req.jsonrpc != ai_protocol::JSONRPC_VERSION {
                send_outbound(
                    &tx,
                    Outbound::Response(RpcResponse::failure(
                        req.id.clone(),
                        ai_protocol::error_object(
                            codes::INVALID_REQUEST,
                            format!("jsonrpc 版本必须为 \"2.0\"，收到 {:?}", req.jsonrpc),
                        ),
                    )),
                );
                continue;
            }
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
    // 关闭全部会话：取消后台扫描，使常驻 delta pump 任务退出。
    //
    // 为什么必须做：pump 持有出站通道（UnboundedSender）的克隆。若不取消，
    // 出站通道永不关闭 → writer_task 的 rx.recv() 永不返回 None →
    // writer.await 永久阻塞 → 客户端 EOF 后引擎进程挂死（只能 kill -9）。
    cancel_all_dir_sessions(&ctx).await;
    drop(tx);
    // 正常路径：pump 在下一次轮询（≤150ms）内感知取消 → 退出 → 出站通道关闭 →
    // writer 自然结束。这里只作"pump 异常卡死"的有界保险（5s），
    // 不可把它当作正常退出路径（否则"pump 永不退出"的回归会被超时掩盖）。
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), writer).await;
    log::info("stdio 服务端退出");
    Ok(())
}

/// 取消所有目录会话的后台扫描（触发 pump 退出）
async fn cancel_all_dir_sessions(ctx: &HandlerCtx) {
    let sessions = ctx.sessions.lock().await;
    for session in sessions.values() {
        if let Session::Dir(data) = session {
            if let Some(model) = &data.model {
                model.cancel();
            }
        }
    }
}

/// HandlerCtx 内部均为 Arc，克隆廉价（请求任务需要拥有所有权）
fn clone_ctx(ctx: &HandlerCtx) -> HandlerCtx {
    HandlerCtx {
        sessions: Arc::clone(&ctx.sessions),
        vfs: Arc::clone(&ctx.vfs),
        max_batch_rows: ctx.max_batch_rows,
        formats: Arc::clone(&ctx.formats),
        rules: Arc::clone(&ctx.rules),
    }
}

/// 单个请求的处理：分发 -> 出站入队；panic 捕获为 InternalError 应答。
///
/// panic 隔离是硬要求：handler 内的 panic 若不兜底，tokio 任务会带
/// JoinError::Panic 结束，而 `chain_prev` 的 `let _ = p.await` 会把错误吞掉 ——
/// 结果该请求**永不应答**，UI 侧只能等到超时。tokio::spawn 把 panic 转成
/// JoinError，这里统一回 -32603 InternalError。
async fn process_request(
    req: ai_protocol::RpcRequest,
    ctx: HandlerCtx,
    cancels: Arc<CancelRegistry>,
    cancel_flag: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<Outbound>,
) {
    let id = req.id.clone();
    let key = id.cancel_key();
    let method = req.method.clone();
    let params = req.params.clone();
    let tx_inner = tx.clone();
    let method_for_dispatch = method.clone();
    let outcome = tokio::spawn(std::panic::AssertUnwindSafe(async move {
        dispatch(
            &method_for_dispatch,
            params,
            &ctx,
            &ai_dir::CancelFlag(cancel_flag),
            tx_inner.clone(),
        )
        .await
    }))
    .await;
    cancels.unregister(&key);
    let result = match outcome {
        Ok(r) => r,
        Err(join_err) => {
            if join_err.is_panic() {
                log::warn(format!("{} {} handler panic，已转为 InternalError", method, id));
                Err(ai_protocol::error_object(
                    codes::INTERNAL_ERROR,
                    format!("handler panic: {method}"),
                ))
            } else {
                Err(ai_protocol::error_object(
                    codes::INTERNAL_ERROR,
                    format!("handler 任务被取消: {method}"),
                ))
            }
        }
    };
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
            log::warn(format!("{} {} 失败: code={} {}", method, id, err.code, err.message));
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
            let (rec, err) = Recorder::open(Some(&path));
            assert!(err.is_none(), "可写目标不应报错: {err:?}");
            rec.record("in", &serde_json::json!({"method": "initialize"}));
            rec.record("out", &serde_json::json!({"result": null}));
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with(r#"{"dir":"in""#));
        assert!(lines[1].starts_with(r#"{"dir":"out""#));
    }

    /// P2-06：录制目标不可写时必须把原因交回调用方（旧实现 `.ok()` 吞掉，
    /// 用户以为在录制、实际无文件，问题现场无法复现）。
    #[test]
    fn recorder_open_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("no-such-subdir").join("rec.ndjson");
        let (rec, err) = Recorder::open(Some(&bad));
        assert!(rec.file.is_none(), "打不开就不该持有文件句柄");
        let msg = err.expect("不可写目标必须返回失败原因");
        assert!(msg.contains("no-such-subdir"), "原因需含目标路径: {msg}");

        let (rec, err) = Recorder::open(None);
        assert!(rec.file.is_none());
        assert!(err.is_none(), "未请求录制不是错误");
    }

    /// panic 隔离边界（F-02 / P1-09）：handler panic 必须转成 -32603 应答。
    ///
    /// 没有隔离时 tokio 任务带 JoinError::Panic 结束，而串行链的
    /// `let _ = p.await` 会吞掉它 —— 该请求永不应答，UI 只能等超时。
    async fn call(
        ctx: &HandlerCtx,
        cancels: &Arc<CancelRegistry>,
        method: &str,
        id: ai_protocol::RequestId,
    ) -> Vec<Outbound> {
        let (tx, mut rx) = mpsc::unbounded_channel::<Outbound>();
        let flag = cancels.register(&id.cancel_key());
        let req = ai_protocol::RpcRequest {
            jsonrpc: ai_protocol::JSONRPC_VERSION.to_string(),
            id,
            method: method.to_string(),
            params: None,
        };
        process_request(req, clone_ctx(ctx), Arc::clone(cancels), flag, tx).await;
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            out.push(msg);
        }
        out
    }

    fn response_json(out: &[Outbound]) -> serde_json::Value {
        assert_eq!(out.len(), 1, "必须恰好应答一次（永不应答即为缺陷）");
        match &out[0] {
            Outbound::Response(r) => serde_json::to_value(r).unwrap(),
            other => panic!("期望 Response，实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn handler_panic_is_answered_as_internal_error() {
        let ctx = HandlerCtx::new(crate::batch::DEFAULT_MAX_BATCH_ROWS);
        let cancels = Arc::new(CancelRegistry::new());
        let out = call(&ctx, &cancels, "test/panic", ai_protocol::RequestId::Number(7)).await;
        let v = response_json(&out);
        assert_eq!(v["id"], 7, "必须回显原请求 id");
        assert_eq!(v["error"]["code"], codes::INTERNAL_ERROR);
        assert!(v.get("result").is_none(), "error 与 result 不得并存");
        assert!(cancels.is_empty(), "panic 路径也必须注销取消旗标（否则注册表泄漏）");
    }

    #[tokio::test]
    async fn handler_panic_does_not_poison_shared_state() {
        // 隔离的意义是"只坏这一个请求"：同一 ctx / 取消注册表在 panic 之后
        // 仍须能正常服务后续请求（会话锁未被毒化、注册表未泄漏）。
        let ctx = HandlerCtx::new(crate::batch::DEFAULT_MAX_BATCH_ROWS);
        let cancels = Arc::new(CancelRegistry::new());
        let bad = call(&ctx, &cancels, "test/panic", ai_protocol::RequestId::Number(1)).await;
        assert_eq!(response_json(&bad)["error"]["code"], codes::INTERNAL_ERROR);

        let good = call(&ctx, &cancels, "initialize", ai_protocol::RequestId::Number(2)).await;
        let v = response_json(&good);
        assert_eq!(v["id"], 2);
        assert_eq!(v["result"]["capabilities"]["textCompare"], true);
        assert!(cancels.is_empty());
    }
}
