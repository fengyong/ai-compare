//! # ai-engine（M22）：headless 引擎进程
//!
//! `ai-engine --stdio`：stdin 读 NDJSON、stdout 写 NDJSON、日志走 stderr
//! （docs/03 §2.3 / ADR-003）。CLI（`aic`）不经进程边界，直接链接引擎库；
//! 本进程形态供 GUI sidecar 与协议级测试（golden / 录制回放）使用。
//!
//! 并发模型（P0 串行链 + P2 后台 pump，语义与 ADR-004 兼容）：
//! - 请求按到达顺序**串行执行**（链式 JoinHandle），保证输出确定性
//!   （golden/回放测试的基石）；
//! - `$/cancel` 通知由分发器即时受理并置位目标请求的取消旗标，
//!   正在执行的扫描在 await 点之间检查旗标；
//! - P2 惰性目录会话（lazyScan）：dir/open 立即应答后由 [`pump`] 常驻任务
//!   推送后台 delta 与 dir/progress；请求串行链之外的唯一异步输出源，
//!   rev 单调 + upsert 幂等保证 UI 端一致性；
//! - handler 在独立任务中运行，panic 被捕获并转换为 `InternalError` 应答
//!   （docs/03 §8.3 引擎 panic 边界）。

pub mod batch;
pub mod cancel;
pub mod handlers;
pub mod log;
pub mod mapping;
pub mod pump;
pub mod server;

use std::path::PathBuf;

/// 服务端运行选项
#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    /// `--record <file>`：双向 NDJSON 录制（{"dir":"in"|"out","msg":{...}}，
    /// docs/03 §6.10 协议录制）
    pub record_path: Option<PathBuf>,
    /// dir/delta 单批最大行数（默认 512）
    pub max_delta_batch_rows: usize,
}

impl ServerOptions {
    pub fn max_delta_batch_rows(&self) -> usize {
        if self.max_delta_batch_rows == 0 {
            batch::DEFAULT_MAX_BATCH_ROWS
        } else {
            self.max_delta_batch_rows
        }
    }
}
