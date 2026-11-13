//! RPC 方法实现：initialize、dir/open、dir/expand、dir/filter、dir/align、
//! dir/operate、text/open、text/sections、session/close。
//!
//! 输出顺序契约：handler 返回 [`Handled`]（应答 + 后续通知序列），
//! server 按序入队 —— 同一请求的响应先于其通知，各请求间输出不交错（串行链）。
//! 惰性会话（lazyScan）的后台 delta 由常驻 pump 任务经同一出站通道推送
//! （顺序约束放宽为 rev 单调；upsert 幂等，UI 无需依赖严格先后）。

use std::collections::HashMap;
use std::sync::Arc;

use ai_core::{LineTable, TextDiffOptions};
use ai_protocol::{error::codes, RpcErrorObject};
use ai_vfs::VfsPath;
use serde_json::json;
use tokio::sync::Mutex;

use crate::batch;
use crate::mapping;
use crate::server::{DirSessionData, Session, SessionMap, TextSessionData};
use crate::pump;

/// 会话表 + 共享状态的容器
pub struct HandlerCtx {
    pub sessions: SessionMap,
    pub vfs: Arc<dyn ai_vfs::VfsProvider>,
    pub max_batch_rows: usize,
}

impl HandlerCtx {
    pub fn new(max_batch_rows: usize) -> Self {
        HandlerCtx {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            vfs: Arc::new(ai_vfs::LocalFs::new()),
            max_batch_rows,
        }
    }
}

/// handler 的产出：应答值 + 附加通知（按序入队）
#[derive(Debug)]
pub enum Handled {
    Value(serde_json::Value),
    /// 先发送 response，再依序发送 notifications（dir/open 的 delta 批次）
    WithNotifications {
        response: serde_json::Value,
        notifications: Vec<ai_protocol::RpcServerNotification>,
    },
}

/// 方法分发入口
pub async fn dispatch(
    method: &str,
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    cancel: &ai_dir::CancelFlag,
    out: tokio::sync::mpsc::UnboundedSender<crate::server::Outbound>,
) -> Result<Handled, RpcErrorObject> {
    match method {
        ai_protocol::method::INITIALIZE => initialize(),
        ai_protocol::method::DIR_OPEN => dir_open(params, ctx, cancel, out).await,
        ai_protocol::method::DIR_EXPAND => dir_expand(params, ctx, out).await,
        ai_protocol::method::DIR_FILTER => dir_filter(params, ctx, out).await,
        ai_protocol::method::DIR_ALIGN => dir_align(params, ctx, out).await,
        ai_protocol::method::DIR_OPERATE => dir_operate(params, ctx).await,
        ai_protocol::method::TEXT_OPEN => text_open(params, ctx).await,
        ai_protocol::method::TEXT_SECTIONS => text_sections(params, ctx).await,
        ai_protocol::method::SESSION_CLOSE => session_close(params, ctx).await,
        other => Err(RpcErrorObject {
            code: codes::METHOD_NOT_FOUND,
            message: format!("未知方法: {other}"),
            data: None,
        }),
    }
}

fn initialize() -> Result<Handled, RpcErrorObject> {
    let result = ai_protocol::InitializeResult {
        protocol_version: ai_protocol::PROTOCOL_VERSION.to_string(),
        engine: ai_protocol::EngineInfo {
            name: "ai-engine".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        capabilities: ai_protocol::EngineCapabilities {
            dir_compare: true,
            text_compare: true,
            inline_diff: false,
            cancel: true,
            delta_batching: true,
            max_delta_batch_rows: u32::try_from(crate::batch::DEFAULT_MAX_BATCH_ROWS)
                .unwrap_or(u32::MAX),
            dir_background_scan: true,
            dir_filters: true,
            dir_operate: true,
        },
        license: ai_protocol::LicenseCapabilities { pro_unlocked: true },
    };
    Ok(Handled::Value(serde_json::to_value(&result).expect("initialize 结果可序列化")))
}

/// 是否使用 P2 会话模型（任一 P2 能力出现）
fn uses_model(params: &ai_protocol::DirOpenParams) -> bool {
    params.lazy_scan == Some(true)
        || params.flat == Some(true)
        || params.name_filter.is_some()
        || params.other_filter.is_some()
        || params.display_filter.is_some()
        || params.manual_align.is_some()
}

/// dir/open：
/// - 旧载荷（无 P2 字段）：同步扫描（P0 行为不变）-> 应答 -> delta 批次；
/// - P2 模型路径：DirModel 打开（可视层立即 / 目标深度内联），应答 scanState
///   后由常驻 pump 推送后台 delta 与 dir/progress。
async fn dir_open(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    cancel: &ai_dir::CancelFlag,
    out: tokio::sync::mpsc::UnboundedSender<crate::server::Outbound>,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::DirOpenParams = parse_params(params)?;
    if params.session_id.is_empty() {
        return Err(bad_params("sessionId 不能为空"));
    }
    let left = VfsPath::parse(&params.left).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right = VfsPath::parse(&params.right).map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    {
        let sessions = ctx.sessions.lock().await;
        if sessions.contains_key(&params.session_id) {
            return Err(RpcErrorObject {
                code: codes::SESSION_EXISTS,
                message: format!("会话已存在: {}", params.session_id),
                data: Some(json!({ "sessionId": params.session_id })),
            });
        }
    }

    if !uses_model(&params) {
        return dir_open_sync(params, left, right, ctx, cancel).await;
    }
    dir_open_model(params, left, right, ctx, cancel, out).await
}

/// P0 同步路径（行为不变；golden/回放契约锚点）
async fn dir_open_sync(
    params: ai_protocol::DirOpenParams,
    left: VfsPath,
    right: VfsPath,
    ctx: &HandlerCtx,
    cancel: &ai_dir::CancelFlag,
) -> Result<Handled, RpcErrorObject> {
    let scan_result = ai_dir::scan(
        ctx.vfs.as_ref(),
        &left,
        &right,
        &ai_dir::DirScanOptions { depth: params.depth, criteria: params.criteria.clone() },
        &ai_dir::ByNameAligner,
        cancel,
    )
    .await
    .map_err(|e| mapping::dir_scan_error_to_rpc(&e))?;

    let rows = ai_dir::rows_from_arena(&scan_result.arena);
    let row_count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let batches = batch::batch_rows(rows, ctx.max_batch_rows);
    let rev = batches.len() as u64;

    {
        let mut sessions = ctx.sessions.lock().await;
        sessions.insert(
            params.session_id.clone(),
            Session::Dir(DirSessionData { model: None, scan: Some(scan_result.clone()) }),
        );
    }

    let response = ai_protocol::DirOpenResult {
        session_id: params.session_id.clone(),
        row_count,
        rev,
        scan_state: ai_protocol::ScanState::Completed,
    };
    let mut notifications = Vec::with_capacity(batches.len());
    for (i, b) in batches.into_iter().enumerate() {
        let delta = batch::make_delta(&params.session_id, i as u64 + 1, &b, scan_result.stats);
        notifications.push(ai_protocol::RpcServerNotification::new(
            ai_protocol::method::DIR_DELTA,
            &delta,
        ));
    }
    Ok(Handled::WithNotifications {
        response: serde_json::to_value(&response).expect("dir/open 结果可序列化"),
        notifications,
    })
}

/// P2 模型路径（ADR-004：可视层立即应答；lazy 时后台继续推进）
async fn dir_open_model(
    params: ai_protocol::DirOpenParams,
    left: VfsPath,
    right: VfsPath,
    ctx: &HandlerCtx,
    cancel: &ai_dir::CancelFlag,
    out: tokio::sync::mpsc::UnboundedSender<crate::server::Outbound>,
) -> Result<Handled, RpcErrorObject> {
    let lazy = params.lazy_scan == Some(true);
    let target_depth = params.depth;
    let visible_depth = if lazy {
        1
    } else {
        // 非惰性（仅过滤器/平铺/对齐）：完整内联扫描，行为接近 P0 同步路径
        target_depth.unwrap_or(u32::MAX)
    };
    let mut opts = ai_dir::DirSessionOptions::new(params.criteria.clone(), Arc::new(ai_dir::ByNameAligner));
    opts.target_depth = target_depth;
    opts.visible_depth = visible_depth;
    opts.name_filter = params.name_filter.clone();
    opts.other_filter = params.other_filter.clone();
    if let Some(df) = &params.display_filter {
        opts.display_filter = *df;
    }
    opts.flat = params.flat == Some(true);
    opts.manual_pairs = params
        .manual_align
        .iter()
        .flat_map(|pairs| pairs.iter().map(|p| (p.left.clone(), p.right.clone())).collect::<Vec<_>>())
        .collect();

    let (model, first) = ai_dir::DirModel::open(
        Arc::clone(&ctx.vfs),
        &left,
        &right,
        opts,
        ai_dir::CancelFlag(cancel.0.clone()),
    )
    .await
    .map_err(|e| mapping::dir_scan_error_to_rpc(&e))?;

    // 后台推进（惰性时启动 worker；非惰性已覆盖目标深度 -> 无 worker）
    model.start_background();

    let batches = batch::batch_rows(first.upserts.clone(), ctx.max_batch_rows);
    let row_count = u32::try_from(first.upserts.len()).unwrap_or(u32::MAX);
    let scan_state = if model.phase() == ai_protocol::ProgressPhase::Running {
        ai_protocol::ScanState::Running
    } else {
        ai_protocol::ScanState::Completed
    };

    {
        let mut sessions = ctx.sessions.lock().await;
        sessions.insert(
            params.session_id.clone(),
            Session::Dir(DirSessionData { model: Some(model.clone()), scan: None }),
        );
    }

    // 常驻 pump：响应入队后推送后台 delta 与 dir/progress（会话关闭时退出）
    pump::spawn(params.session_id.clone(), model, out, ctx.max_batch_rows);

    let response = ai_protocol::DirOpenResult {
        session_id: params.session_id.clone(),
        row_count,
        rev: first.rev,
        scan_state,
    };
    let mut notifications = Vec::with_capacity(batches.len());
    for (i, b) in batches.into_iter().enumerate() {
        let delta = batch::make_delta_with_progress(
            &params.session_id,
            first.rev,
            &b,
            first.stats,
            Some(first.progress),
            if i == 0 { first.removals.clone() } else { Vec::new() },
        );
        notifications.push(ai_protocol::RpcServerNotification::new(
            ai_protocol::method::DIR_DELTA,
            &delta,
        ));
    }
    Ok(Handled::WithNotifications {
        response: serde_json::to_value(&response).expect("dir/open 结果可序列化"),
        notifications,
    })
}

/// dir/expand：展开子树（前台优先），增量直接推送
async fn dir_expand(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    out: tokio::sync::mpsc::UnboundedSender<crate::server::Outbound>,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::DirExpandParams = parse_params(params)?;
    let model = dir_model(&ctx.sessions, &params.session_id).await?;
    let new_rows = model
        .expand(&params.path, params.depth.unwrap_or(1))
        .await
        .map_err(|e| mapping::dir_scan_error_to_rpc(&e))?;
    let delta = model.take_delta().await;
    let stats = model.stats_snapshot();
    let progress = model.progress();
    let rev = delta.as_ref().map(|d| d.rev).unwrap_or(model.rev_snapshot());
    if let Some(d) = &delta {
        push_delta(&out, &params.session_id, d, ctx.max_batch_rows);
    }
    let response = ai_protocol::DirExpandResult {
        session_id: params.session_id,
        rev,
        new_rows,
        stats,
        progress: Some(progress),
    };
    Ok(Handled::Value(serde_json::to_value(&response).expect("dir/expand 结果可序列化")))
}

/// dir/filter：名称/其他过滤（重扫）+ 显示过滤/抑制/平铺（仅显示）
async fn dir_filter(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    out: tokio::sync::mpsc::UnboundedSender<crate::server::Outbound>,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::DirFilterParams = parse_params(params)?;
    let model = dir_model(&ctx.sessions, &params.session_id).await?;
    if params.name_filter.is_some() || params.other_filter.is_some() {
        model.update_filters(params.name_filter.clone(), params.other_filter.clone()).await
            .map_err(|e| mapping::dir_scan_error_to_rpc(&e))?;
        model.start_background(); // 重扫后的后台推进（幂等）
    }
    if params.display_filter.is_some() || params.suppress.is_some() || params.flat.is_some() {
        model.set_display(params.display_filter, params.suppress, params.flat);
    }
    let delta = model.take_delta().await;
    let stats = model.stats_snapshot();
    let rev = delta.as_ref().map(|d| d.rev).unwrap_or(model.rev_snapshot());
    if let Some(d) = &delta {
        push_delta(&out, &params.session_id, d, ctx.max_batch_rows);
    }
    let rows = model.rows_snapshot().await;
    let visible = rows.iter().filter(|r| !r.hidden).count() as u32;
    let hidden = rows.iter().filter(|r| r.hidden).count() as u32;
    let response = ai_protocol::DirFilterResult {
        session_id: params.session_id,
        rev,
        visible_rows: visible,
        hidden_rows: hidden,
        stats,
    };
    Ok(Handled::Value(serde_json::to_value(&response).expect("dir/filter 结果可序列化")))
}

/// dir/align：手动对齐 / 解除对齐（FR-DIR-022）
async fn dir_align(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    out: tokio::sync::mpsc::UnboundedSender<crate::server::Outbound>,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::DirAlignParams = parse_params(params)?;
    let model = dir_model(&ctx.sessions, &params.session_id).await?;
    let updated = match (&params.left, &params.right, &params.unalign) {
        (Some(l), Some(r), _) => model.align(l, r).await.map_err(|e| mapping::dir_scan_error_to_rpc(&e))?,
        (None, None, Some(p)) => model.unalign(p).await.map_err(|e| mapping::dir_scan_error_to_rpc(&e))?,
        _ => return Err(bad_params("dir/align 需要 left+right（对齐）或 unalign（解除）")),
    };
    let delta = model.take_delta().await;
    let stats = model.stats_snapshot();
    let rev = delta.as_ref().map(|d| d.rev).unwrap_or(model.rev_snapshot());
    if let Some(d) = &delta {
        push_delta(&out, &params.session_id, d, ctx.max_batch_rows);
    }
    let response = ai_protocol::DirAlignResult {
        session_id: params.session_id,
        rev,
        updated_rows: updated,
        stats,
    };
    Ok(Handled::Value(serde_json::to_value(&response).expect("dir/align 结果可序列化")))
}

/// dir/operate：生成同步操作计划（不执行；SYNC-002/DMRG-003 数据层）
async fn dir_operate(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::DirOperateParams = parse_params(params)?;
    let model = dir_model(&ctx.sessions, &params.session_id).await?;
    let rows = model.rows_snapshot().await;
    // FR-DIR-068 操作遵循过滤器：隐藏行不进入计划（抑制开启时行未隐藏）
    let visible: Vec<ai_protocol::DirRow> = rows.into_iter().filter(|r| !r.hidden).collect();
    let plan = ai_dir::plan_operations(
        &visible,
        params.preset,
        params.rules.as_deref(),
        params.paths.as_deref().filter(|p| !p.is_empty()),
    );
    let total = u32::try_from(plan.len()).unwrap_or(u32::MAX);
    let response = ai_protocol::DirOperateResult {
        session_id: params.session_id,
        total,
        plan,
    };
    Ok(Handled::Value(serde_json::to_value(&response).expect("dir/operate 结果可序列化")))
}

/// 获取目录模型会话（P2 路径）；同步路径会话返回明确错误
async fn dir_model(
    sessions: &SessionMap,
    id: &str,
) -> Result<Arc<ai_dir::DirModel>, RpcErrorObject> {
    let sessions = sessions.lock().await;
    let session = sessions
        .get(id)
        .ok_or_else(|| session_not_found(id))?;
    match session {
        Session::Dir(data) => data.model.clone().ok_or_else(|| {
            bad_params("该会话未启用 P2 会话模型（lazyScan/过滤器/平铺/对齐任一即可启用）")
        }),
        Session::Text(_) => Err(bad_params(format!("会话不是目录会话: {id}"))),
    }
}

/// 把一份 delta 数据切批推送（removals 只随首批）
fn push_delta(
    out: &tokio::sync::mpsc::UnboundedSender<crate::server::Outbound>,
    session_id: &str,
    d: &ai_dir::DirDeltaData,
    max_batch: usize,
) {
    let batches = batch::batch_rows(d.upserts.clone(), max_batch);
    for (i, b) in batches.into_iter().enumerate() {
        let delta = batch::make_delta_with_progress(
            session_id,
            d.rev,
            &b,
            d.stats,
            Some(d.progress),
            if i == 0 { d.removals.clone() } else { Vec::new() },
        );
        let _ = out.send(crate::server::Outbound::Notification(
            ai_protocol::RpcServerNotification::new(ai_protocol::method::DIR_DELTA, &delta),
        ));
    }
}

/// text/open：读两侧文本 -> diff -> 存会话 -> 应答统计
async fn text_open(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::TextOpenParams = parse_params(params)?;
    if params.session_id.is_empty() {
        return Err(bad_params("sessionId 不能为空"));
    }
    let left_path = VfsPath::parse(&params.left).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right_path = VfsPath::parse(&params.right).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let opts_dto = params.options.clone().unwrap_or_default();
    let opts: TextDiffOptions = mapping::text_options_from_dto(&opts_dto);

    let left_text = ctx
        .vfs
        .read_file_text(&left_path)
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right_text = ctx
        .vfs
        .read_file_text(&right_path)
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    let lt = LineTable::from_text(&left_text);
    let rt = LineTable::from_text(&right_text);
    let out = ai_core::diff_text(&lt, &rt, &opts)
        .map_err(|e| ai_protocol::error_object(codes::INVALID_PARAMS, format!("{e}")))?;

    let left_line_count = u32::try_from(lt.len()).unwrap_or(u32::MAX);
    let right_line_count = u32::try_from(rt.len()).unwrap_or(u32::MAX);
    let dtos: Vec<ai_protocol::Section> = out.sections.iter().map(mapping::section_to_dto).collect();
    let section_count = dtos.len() as u32;
    let summary = mapping::summary_to_dto(&out.summary);

    {
        let mut sessions = ctx.sessions.lock().await;
        sessions.insert(
            params.session_id.clone(),
            Session::Text(TextSessionData { sections: dtos, summary }),
        );
    }

    let result = ai_protocol::TextOpenResult {
        session_id: params.session_id,
        left_line_count,
        right_line_count,
        section_count,
        summary,
    };
    Ok(Handled::Value(serde_json::to_value(&result).expect("text/open 结果可序列化")))
}

/// text/sections：窗口化取 section
async fn text_sections(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::TextSectionsParams = parse_params(params)?;
    let sessions = ctx.sessions.lock().await;
    let session = sessions
        .get(&params.session_id)
        .ok_or_else(|| session_not_found(&params.session_id))?;
    let data = match session {
        Session::Text(data) => data,
        Session::Dir(_) => return Err(bad_params(format!("会话不是文本会话: {}", params.session_id))),
    };
    let total = u32::try_from(data.sections.len()).unwrap_or(u32::MAX);
    let start = params.offset.min(total) as usize;
    let end = start.saturating_add(params.count as usize).min(data.sections.len());
    let result = ai_protocol::TextSectionsResult {
        session_id: params.session_id,
        offset: params.offset,
        total,
        has_more: end < data.sections.len(),
        sections: data.sections[start..end].to_vec(),
    };
    Ok(Handled::Value(serde_json::to_value(&result).expect("text/sections 结果可序列化")))
}

/// session/close：幂等关闭（不存在也返回成功）；目录会话取消后台扫描
async fn session_close(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::SessionCloseParams = parse_params(params)?;
    let mut sessions = ctx.sessions.lock().await;
    if let Some(Session::Dir(data)) = sessions.remove(&params.session_id) {
        if let Some(model) = &data.model {
            model.cancel();
        }
    }
    Ok(Handled::Value(serde_json::Value::Null))
}

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

fn parse_params<T: serde::de::DeserializeOwned>(
    params: Option<serde_json::Value>,
) -> Result<T, RpcErrorObject> {
    let value = params.unwrap_or(serde_json::Value::Null);
    serde_json::from_value(value).map_err(|e| bad_params(format!("参数不合法: {e}")))
}

fn bad_params(msg: impl Into<String>) -> RpcErrorObject {
    ai_protocol::error_object(codes::INVALID_PARAMS, msg)
}

fn session_not_found(id: &str) -> RpcErrorObject {
    ai_protocol::error_object_with_data(
        codes::SESSION_NOT_FOUND,
        format!("会话不存在: {id}"),
        json!({ "sessionId": id }),
    )
}

#[cfg(test)]
mod p2_tests {
    use super::*;
    use crate::server::Outbound;
    use serde_json::json;

    fn ctx() -> HandlerCtx {
        HandlerCtx::new(512)
    }

    fn chan() -> (
        tokio::sync::mpsc::UnboundedSender<Outbound>,
        tokio::sync::mpsc::UnboundedReceiver<Outbound>,
    ) {
        tokio::sync::mpsc::unbounded_channel()
    }

    async fn make_trees() -> (tempfile::TempDir, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(l.join("sub")).unwrap();
        std::fs::create_dir_all(r.join("sub")).unwrap();
        std::fs::write(l.join("same.txt"), b"1").unwrap();
        std::fs::write(r.join("same.txt"), b"1").unwrap();
        std::fs::write(l.join("diff.txt"), b"LEFT").unwrap();
        std::fs::write(r.join("diff.txt"), b"RIGHT").unwrap();
        std::fs::write(l.join("sub").join("inner.txt"), b"IL").unwrap();
        std::fs::write(r.join("sub").join("inner.txt"), b"IR").unwrap();
        std::fs::write(l.join("skip.log"), b"log").unwrap();
        std::fs::write(r.join("skip.log"), b"log").unwrap();
        (dir, l.to_string_lossy().into_owned(), r.to_string_lossy().into_owned())
    }

    async fn collect(mut rx: tokio::sync::mpsc::UnboundedReceiver<Outbound>) -> Vec<serde_json::Value> {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Outbound::Response(r) => out.push(serde_json::to_value(&r).unwrap()),
                Outbound::Notification(n) => out.push(serde_json::to_value(&n).unwrap()),
            }
        }
        out
    }

    #[tokio::test]
    async fn lazy_open_then_expand_filter_operate_flow() {
        let (_dir, left, right) = make_trees().await;
        let c = ctx();
        let (tx, rx) = chan();

        // 1. lazy dir/open（Always 判据保证状态确定）
        let handled = dispatch(
            "dir/open",
            Some(json!({
                "sessionId": "d1", "left": left, "right": right,
                "depth": 1,
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"},
                "lazyScan": true
            })),
            &c,
            &ai_dir::CancelFlag::new(),
            tx.clone(),
        )
        .await
        .unwrap();
        let resp = match &handled {
            Handled::WithNotifications { response, .. } => response.clone(),
            _ => panic!("应答形态错误"),
        };
        // depth=1 已被可视层覆盖 -> completed；纯惰性（depth=null）才是 running
        assert_eq!(resp["scanState"], "completed", "惰性+目标覆盖应答 scanState: {}", resp);
        for n in match &handled {
            Handled::WithNotifications { notifications, .. } => notifications.clone(),
            _ => Vec::new(),
        } {
            let _ = tx.send(Outbound::Notification(n));
        }

        // 2. dir/expand 展开 sub（先展开使 sub/inner.txt 进入可见集）
        let r2 = dispatch(
            "dir/expand",
            Some(json!({"sessionId": "d1", "path": "sub", "depth": 1})),
            &c, &ai_dir::CancelFlag::new(), tx.clone(),
        )
        .await
        .unwrap();
        let expand_resp = match r2 {
            Handled::Value(v) => v,
            _ => panic!("dir/expand 应答形态"),
        };
        assert_eq!(expand_resp["newRows"], 1, "sub/inner.txt: {}", expand_resp);

        // 3. dir/filter 排除 *.log（重扫；depth=1 已被可视层覆盖，无后台时序）
        let r3 = dispatch(
            "dir/filter",
            Some(json!({"sessionId": "d1", "nameFilter": {"excludeFiles": ["*.log"]}})),
            &c, &ai_dir::CancelFlag::new(), tx.clone(),
        )
        .await
        .unwrap();
        let filter_resp = match r3 {
            Handled::Value(v) => v,
            _ => panic!("dir/filter 应答形态"),
        };
        // 重扫后可见：same.txt / diff.txt / sub（skip.log 被排除；
        // 重扫会重置展开状态，sub/inner.txt 需再次 expand）
        assert_eq!(filter_resp["visibleRows"], 3, "排除 skip.log 后可见行: {}", filter_resp);

        // 4. dir/operate 镜像左→右 计划
        let r4 = dispatch(
            "dir/operate",
            Some(json!({"sessionId": "d1", "preset": "mirrorLeftToRight"})),
            &c, &ai_dir::CancelFlag::new(), tx.clone(),
        )
        .await
        .unwrap();
        let ops = match r4 {
            Handled::Value(v) => v,
            _ => panic!("dir/operate 应答形态"),
        };
        let plan: Vec<&str> = ops["plan"].as_array().unwrap().iter().map(|p| p["op"].as_str().unwrap()).collect();
        assert!(plan.contains(&"copyLeftToRight"), "计划: {}", ops);

        // 5. 收集通知：delta + progress（completed）
        let msgs = collect(rx).await;
        assert!(msgs.iter().any(|m| m["method"] == "dir/delta"), "应推送 delta");
        assert!(
            msgs.iter().any(|m| m["method"] == "dir/progress" && m["params"]["phase"] == "completed"),
            "应推送完成进度: {:?}",
            msgs.iter().map(|m| m["method"].clone()).collect::<Vec<_>>()
        );

        // 6. session/close -> 取消 -> pump 退出
        let r6 = dispatch(
            "session/close",
            Some(json!({"sessionId": "d1"})),
            &c, &ai_dir::CancelFlag::new(), tx.clone(),
        )
        .await
        .unwrap();
        assert!(matches!(r6, Handled::Value(_)));
        drop(tx);
    }

    #[tokio::test]
    async fn dir_open_sync_path_unchanged() {
        let (_dir, left, right) = make_trees().await;
        let c = ctx();
        let (tx, rx) = chan();
        let handled = dispatch(
            "dir/open",
            Some(json!({
                "sessionId": "s1", "left": left, "right": right,
                "depth": 1,
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"}
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let resp = match &handled {
            Handled::WithNotifications { response, .. } => response.clone(),
            _ => panic!("应答形态"),
        };
        assert_eq!(resp["scanState"], "completed", "旧载荷路径行为不变");
        assert_eq!(resp["rowCount"], 4, "same/diff/sub/skip.log 四行");
        drop(rx);
    }

    #[tokio::test]
    async fn sync_session_rejects_p2_methods() {
        let (_dir, left, right) = make_trees().await;
        let c = ctx();
        let (tx, rx) = chan();
        let _ = dispatch(
            "dir/open",
            Some(json!({"sessionId": "s2", "left": left, "right": right, "depth": 1})),
            &c, &ai_dir::CancelFlag::new(), tx.clone(),
        )
        .await
        .unwrap();
        let err = dispatch(
            "dir/expand",
            Some(json!({"sessionId": "s2", "path": ""})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::INVALID_PARAMS);
        drop(rx);
    }

    #[tokio::test]
    async fn manual_align_via_protocol() {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("old.txt"), b"same").unwrap();
        std::fs::write(r.join("new.txt"), b"same").unwrap();
        let c = ctx();
        let (tx, rx) = chan();
        let _ = dispatch(
            "dir/open",
            Some(json!({
                "sessionId": "s3", "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "depth": 1,
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"},
                "lazyScan": true
            })),
            &c, &ai_dir::CancelFlag::new(), tx.clone(),
        )
        .await
        .unwrap();
        let r = dispatch(
            "dir/align",
            Some(json!({"sessionId": "s3", "left": "old.txt", "right": "new.txt"})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let resp = match r {
            Handled::Value(v) => v,
            _ => panic!("形态"),
        };
        assert_eq!(resp["updatedRows"], 2, "对齐后两侧重判: {}", resp);
        drop(rx);
    }
}
