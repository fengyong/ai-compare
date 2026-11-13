//! 目录会话 delta 泵：把 DirModel 的增量（含后台扫描推进）按序推送到出站通道。
//!
//! 生命周期：dir/open（P2 模型路径）时创建；会话关闭（模型取消）或
//! Cancelled 阶段后退出；Completed 后保持监听（dir/filter 重扫可能再次
//! 置 Running）。rev 单调由模型保证；upsert 幂等，UI 无需依赖批次顺序。

use std::sync::Arc;
use std::time::Duration;

use ai_dir::DirModel;

use crate::batch;
use crate::server::Outbound;

/// 推送间隔上限（无通知时的轮询周期；cancel/关闭在此粒度内生效）
const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// 启动常驻 pump 任务
pub fn spawn(
    session_id: String,
    model: Arc<DirModel>,
    out: tokio::sync::mpsc::UnboundedSender<Outbound>,
    max_batch_rows: usize,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // 虚拟初值 Running：首轮若已完成（目标深度被可视层覆盖）也发送一次
        // dir/progress(completed)，保证客户端总能收到终态进度事件
        let mut last_phase = ai_protocol::ProgressPhase::Running;
        loop {
            // 取消/会话关闭：排空后退出
            if model.cancel_token().is_cancelled() {
                drain(&session_id, &model, &out, max_batch_rows).await;
                let _ = out.send(Outbound::Notification(progress_event(
                    &session_id,
                    &model,
                    ai_protocol::ProgressPhase::Cancelled,
                )));
                return;
            }
            // 等待模型通知（带超时兜底：关闭/取消不依赖通知到达）
            let _ = tokio::time::timeout(POLL_INTERVAL, model.notified()).await;
            drain(&session_id, &model, &out, max_batch_rows).await;
            let phase = model.phase();
            if phase != last_phase {
                let _ = out.send(Outbound::Notification(progress_event(&session_id, &model, phase)));
                last_phase = phase;
                if phase == ai_protocol::ProgressPhase::Cancelled {
                    return;
                }
            }
        }
    })
}

/// 取尽当前增量并按批推送
async fn drain(
    session_id: &str,
    model: &Arc<DirModel>,
    out: &tokio::sync::mpsc::UnboundedSender<Outbound>,
    max_batch_rows: usize,
) {
    while let Some(d) = model.take_delta().await {
        let batches = batch::batch_rows(d.upserts, max_batch_rows);
        for (i, b) in batches.into_iter().enumerate() {
            let delta = batch::make_delta_with_progress(
                session_id,
                d.rev,
                &b,
                d.stats,
                Some(d.progress),
                if i == 0 { d.removals.clone() } else { Vec::new() },
            );
            let _ = out.send(Outbound::Notification(ai_protocol::RpcServerNotification::new(
                ai_protocol::method::DIR_DELTA,
                &delta,
            )));
        }
    }
}

fn progress_event(
    session_id: &str,
    model: &Arc<DirModel>,
    phase: ai_protocol::ProgressPhase,
) -> ai_protocol::RpcServerNotification {
    let p = model.progress();
    ai_protocol::RpcServerNotification::new(
        ai_protocol::method::DIR_PROGRESS,
        &ai_protocol::DirProgressEvent {
            session_id: session_id.to_string(),
            phase,
            scanned: p.scanned,
            total_estimate: p.total,
        },
    )
}
