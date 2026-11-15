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
    out: crate::outbound::OutboundTx,
    max_batch_rows: usize,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // 虚拟初值 Running：首轮若已完成（目标深度被可视层覆盖）也发送一次
        // dir/progress(completed)，保证客户端总能收到终态进度事件
        let mut last_phase = ai_protocol::ProgressPhase::Running;
        // R3-22 工作包 B：出站有界。pump 的每次发送都带会话取消旗标，
        // 因此 EOF / session/close 置位后，即使慢读者让队列满载，pump 也会
        // 在 BACKPRESSURE_POLL 内放弃等待并退出（不会挂死进程）。
        let cancel = Arc::clone(&model.cancel_token().0);
        loop {
            // 取消/会话关闭：排空后退出
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                drain(&session_id, &model, &out, &cancel, max_batch_rows).await;
                out.send_with_cancel(
                    Outbound::Notification(progress_event(
                        &session_id,
                        &model,
                        ai_protocol::ProgressPhase::Cancelled,
                    )),
                    Some(&cancel),
                )
                .await;
                return;
            }
            // 等待模型通知（带超时兜底：关闭/取消不依赖通知到达）
            let _ = tokio::time::timeout(POLL_INTERVAL, model.notified()).await;
            if !drain(&session_id, &model, &out, &cancel, max_batch_rows).await {
                return;
            }
            let phase = model.phase();
            if phase != last_phase {
                if !out
                    .send_with_cancel(
                        Outbound::Notification(progress_event(&session_id, &model, phase)),
                        Some(&cancel),
                    )
                    .await
                {
                    return;
                }
                last_phase = phase;
                if phase == ai_protocol::ProgressPhase::Cancelled {
                    return;
                }
            }
        }
    })
}

/// 取尽当前增量并按批推送；返回 false 表示出站被取消/关闭，pump 应退出。
async fn drain(
    session_id: &str,
    model: &Arc<DirModel>,
    out: &crate::outbound::OutboundTx,
    cancel: &std::sync::atomic::AtomicBool,
    max_batch_rows: usize,
) -> bool {
    while let Some(d) = model.take_delta().await {
        let batches = batch::batch_rows(d.upserts, max_batch_rows);
        // 扫描错误必须回到 UI：读不到的子目录已置 Unknown，但用户还需要知道
        // 是哪条路径、为什么读不到（权限/IO），否则只看到一个灰色的未知行。
        for (i, b) in batches.into_iter().enumerate() {
            let mut delta = batch::make_delta_with_progress(
                session_id,
                d.rev,
                &b,
                d.stats,
                Some(d.progress),
                if i == 0 {
                    d.removals.clone()
                } else {
                    Vec::new()
                },
            );
            if i == 0 {
                delta.scan_errors = batch::scan_issues(&d.scan_errors);
            }
            if !out
                .send_with_cancel(
                    Outbound::Notification(ai_protocol::RpcServerNotification::new(
                        ai_protocol::method::DIR_DELTA,
                        &delta,
                    )),
                    Some(cancel),
                )
                .await
            {
                return false;
            }
        }
    }
    true
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
