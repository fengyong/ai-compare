//! RPC 方法实现：initialize、dir/open、dir/expand、dir/filter、dir/align、
//! dir/operate、text/open、text/sections、session/close。
//!
//! 输出顺序契约：handler 返回 [`Handled`]（应答 + 后续通知序列），
//! server 按序入队 —— 同一请求的响应先于其通知，各请求间输出不交错（串行链）。
//! 惰性会话（lazyScan）的后台 delta 由常驻 pump 任务经同一出站通道推送
//! （顺序约束放宽为 rev 单调；upsert 幂等，UI 无需依赖严格先后）。

use std::collections::HashMap;
use std::sync::Arc;

use ai_core::LineTable;
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
        ai_protocol::method::TEXT_SECTIONS => text_sections(params, ctx, out).await,
        ai_protocol::method::TEXT_LINES => text_lines(params, ctx).await,
        ai_protocol::method::TEXT_INLINE_DIFF => text_inline_diff(params, ctx).await,
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
            // P3：行内差异懒计算入口开放（text/inlineDiff；超长/超时降级见 inline.rs）
            inline_diff: true,
            cancel: true,
            delta_batching: true,
            max_delta_batch_rows: u32::try_from(crate::batch::DEFAULT_MAX_BATCH_ROWS)
                .unwrap_or(u32::MAX),
            // P3：text/lines 行内容分页入口开放
            lines_read: Some(true),
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

/// text/open：读两侧文本 -> diff（P3 起走 DiffConfig：重要性 + 手动对齐）
/// -> 存会话（含两侧行表）-> 应答统计
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

    // P3：骨架选项 + 重要性模型 + 手动对齐（无新参数时与 P0 diff_text 行为一致）
    let mut config = ai_core::DiffConfig { base: mapping::text_options_from_dto(&opts_dto), ..ai_core::DiffConfig::default() };
    if let Some(spec) = &params.importance {
        config.importance = mapping::importance_from_dto(spec);
    }
    if let Some(spec) = &params.manual {
        config.manual = mapping::manual_from_dto(spec);
        config
            .manual
            .validate(lt.len() as u32, rt.len() as u32)
            .map_err(|e| ai_protocol::error_object(codes::INVALID_PARAMS, format!("{e}")))?;
    }

    let keyed = ai_core::diff_config(&lt, &rt, &config)
        .map_err(|e| ai_protocol::error_object(codes::INVALID_PARAMS, format!("{e}")))?;
    let out = keyed.output;

    let left_line_count = u32::try_from(lt.len()).unwrap_or(u32::MAX);
    let right_line_count = u32::try_from(rt.len()).unwrap_or(u32::MAX);
    let section_count = u32::try_from(out.sections.len()).unwrap_or(u32::MAX);
    let summary = mapping::summary_to_dto(&out.summary);

    {
        let mut sessions = ctx.sessions.lock().await;
        sessions.insert(
            params.session_id.clone(),
            Session::Text(TextSessionData {
                sections: out.sections,
                summary: out.summary,
                left: lt,
                right: rt,
                config,
                rev: 0,
            }),
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

/// 取文本会话（可变访问；闭包可失败。会话表锁内不做重活/不做 await）
async fn with_text_session<T>(
    sessions: &SessionMap,
    id: &str,
    f: impl FnOnce(&mut TextSessionData) -> Result<T, RpcErrorObject>,
) -> Result<T, RpcErrorObject> {
    let mut sessions = sessions.lock().await;
    let session = sessions.get_mut(id).ok_or_else(|| session_not_found(id))?;
    match session {
        Session::Text(data) => f(data),
        Session::Dir(_) => Err(bad_params(format!("会话不是文本会话: {id}"))),
    }
}

/// Change section 的行内懒计算（窗口化触发）：完成时把状态写回会话。
/// 返回 (逐行结果, 是否存在降级行)。
///
/// 逐行位置配对（第 i 行对第 i 行，与 ai-core inline_diff_section 同语义），
/// 但降级行（超长/超时）也保留在结果中（spans 空 + truncated=true），
/// 供 UI 按整行差异渲染并驱动 Partial 状态（docs/03 §6.2）。
fn compute_inline(
    data: &mut TextSessionData,
    core_idx: usize,
) -> (Vec<ai_protocol::InlineLineDiffDto>, bool) {
    let section = data.sections[core_idx];
    let (Some(l), Some(r)) = (section.left, section.right) else {
        return (Vec::new(), false);
    };
    let n = l.len().min(r.len()) as usize;
    let inline_opts = data.config.base.inline.clone();
    let mut dtos = Vec::with_capacity(n);
    let mut any_truncated = false;
    for k in 0..n {
        let li = l.start as usize + k;
        let ri = r.start as usize + k;
        let result = ai_core::inline_diff_line(&data.left.line(li).text, &data.right.line(ri).text, &inline_opts);
        any_truncated |= result.truncated;
        dtos.push(mapping::inline_line_diff_to_dto(&ai_core::InlineLineDiff {
            left_line: li as u32,
            right_line: ri as u32,
            result,
        }));
    }
    // 状态写回：Full / Partial（docs/03 §4.1 InlineState；NotComputed -> 懒计算完成）
    data.sections[core_idx].inline = if any_truncated {
        ai_core::sections::InlineState::Partial
    } else {
        ai_core::sections::InlineState::Full
    };
    (dtos, any_truncated)
}

/// text/sections：窗口化取 section
///
/// P3 扩展（全部可选，旧载荷行为不变）：
/// - 窗口内 Change section 触发行内懒计算（inline 状态 NotComputed -> Full/Partial，
///   能力位 inlineDiff=true 时 UI 可预期替换段带状态）；
/// - `withKeys`：响应附带窗口内差异行的逐行 (raw, importance) 双键；
/// - `importance`：以指定重要性模型重判（ImportanceUpdate 语义，不重跑 diff），
///   变更的 section id 按重要性分组推送 `text/sections` 通知。
async fn text_sections(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    out: tokio::sync::mpsc::UnboundedSender<crate::server::Outbound>,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::TextSectionsParams = parse_params(params)?;
    let session_id = params.session_id.clone();

    // 1. 重要性重判（先于取数；core classify_sections 按当前行表与选项重判）
    let mut importance_notifications: Vec<ai_protocol::RpcServerNotification> = Vec::new();
    if let Some(spec) = &params.importance {
        let imp = mapping::importance_from_dto(spec);
        let events = with_text_session(&ctx.sessions, &session_id, |data| {
            let classification = ai_core::classify_sections(
                &data.sections,
                &data.left,
                &data.right,
                &data.config.base,
                &imp,
                &data.config.manual,
            );
            let changed = ai_core::apply_classification(&mut data.sections, &classification);
            data.config.importance = imp;
            Ok((changed, data.summary))
        })
        .await?;
        let (changed, summary) = events;
        if !changed.is_empty() {
            data_rev_bump(&ctx.sessions, &session_id).await?;
            // core SectionEvent::ImportanceUpdate 语义：一组 ids 共享一个重要性；
            // 按 Importance 枚举序分组推送（顺序确定，录制/回放友好）
            let groups: [(ai_protocol::Importance, ai_core::sections::Importance); 3] = [
                (ai_protocol::Importance::Important, ai_core::sections::Importance::Important),
                (ai_protocol::Importance::Unimportant, ai_core::sections::Importance::Unimportant),
                (ai_protocol::Importance::Ignored, ai_core::sections::Importance::Ignored),
            ];
            let sections_snapshot =
                with_text_session(&ctx.sessions, &session_id, |data| Ok(data.sections.clone()))
                    .await?;
            for (dto_imp, core_imp) in groups {
                let ids: Vec<u32> = sections_snapshot
                    .iter()
                    .filter(|s| changed.contains(&s.id) && s.important == core_imp)
                    .map(|s| s.id)
                    .collect();
                if ids.is_empty() {
                    continue;
                }
                let rev = with_text_session(&ctx.sessions, &session_id, |data| Ok(data.rev)).await?;
                let payload = ai_protocol::TextSectionsNotification {
                    session_id: session_id.clone(),
                    rev,
                    appended: Vec::new(),
                    done: false,
                    summary: mapping::summary_to_dto(&summary),
                    event: ai_protocol::TextSectionEventKind::ImportanceUpdate,
                    replaced_span: None,
                    with: None,
                    updated_ids: Some(ids),
                    importance: Some(dto_imp),
                };
                importance_notifications.push(ai_protocol::RpcServerNotification::new(
                    ai_protocol::method::TEXT_SECTIONS_NOTIFY,
                    &payload,
                ));
            }
        }
    }

    // 2. 窗口取数 + 行内懒计算 + 双键
    let result = with_text_session(&ctx.sessions, &session_id, |data| {
        let total = u32::try_from(data.sections.len()).unwrap_or(u32::MAX);
        let start = params.offset.min(total) as usize;
        let end = start.saturating_add(params.count as usize).min(data.sections.len());
        let window = start..end;

        // Change section 行内懒计算（NotComputed 才算；已算过直接取状态）
        let need_inline: Vec<usize> = data.sections[window.clone()]
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.kind == ai_core::sections::SectionKind::Change
                    && s.inline == ai_core::sections::InlineState::NotComputed
            })
            .map(|(i, _)| i)
            .collect();
        for i in need_inline {
            let _ = compute_inline(data, window.start + i);
        }

        let dtos: Vec<ai_protocol::Section> = data.sections[window.clone()]
            .iter()
            .map(mapping::section_to_dto)
            .collect();

        // 逐行双键（FR-TXT-010：仅差异行，Equal section 不含）
        let line_keys = if params.with_keys == Some(true) {
            let opts = data.config.base.clone();
            let imp = data.config.importance.clone();
            let mut entries = Vec::new();
            for sec in &data.sections[window] {
                match sec.kind {
                    ai_core::sections::SectionKind::Equal => {}
                    ai_core::sections::SectionKind::Change | ai_core::sections::SectionKind::Delete => {
                        let l = sec.left.expect("Change/Delete 必有左区间");
                        for i in l.start..l.end {
                            entries.push(mapping::line_keys_entry(
                                ai_core::options::DiffSide::Left,
                                i,
                                &data.left,
                                i as usize,
                                &opts,
                                &imp,
                            ));
                        }
                        if sec.kind == ai_core::sections::SectionKind::Change {
                            let r = sec.right.expect("Change 必有右区间");
                            for i in r.start..r.end {
                                entries.push(mapping::line_keys_entry(
                                    ai_core::options::DiffSide::Right,
                                    i,
                                    &data.right,
                                    i as usize,
                                    &opts,
                                    &imp,
                                ));
                            }
                        }
                    }
                    ai_core::sections::SectionKind::Insert => {
                        let r = sec.right.expect("Insert 必有右区间");
                        for i in r.start..r.end {
                            entries.push(mapping::line_keys_entry(
                                ai_core::options::DiffSide::Right,
                                i,
                                &data.right,
                                i as usize,
                                &opts,
                                &imp,
                            ));
                        }
                    }
                }
            }
            Some(entries)
        } else {
            None
        };

        let has_more = end < data.sections.len();
        Ok(ai_protocol::TextSectionsResult {
            session_id: session_id.clone(),
            offset: params.offset,
            total,
            has_more,
            sections: dtos,
            line_keys,
        })
    })
    .await?;

    // 3. 通知按序入队：importance 组先于应答（响应在最后，见 dir/expand 同款顺序约定）
    for n in importance_notifications {
        let _ = out.send(crate::server::Outbound::Notification(n));
    }
    Ok(Handled::Value(
        serde_json::to_value(&result).expect("text/sections 结果可序列化"),
    ))
}

/// 会话版本号 +1（重要性重判等模型变更）
async fn data_rev_bump(sessions: &SessionMap, id: &str) -> Result<(), RpcErrorObject> {
    with_text_session(sessions, id, |data| {
        data.rev += 1;
        Ok(())
    })
    .await
}

/// text/lines：行内容按需分页（docs/03 §2.3.2 大内容约定；能力位 linesRead）
///
/// 行数据来自会话内已解码的行表（text/open 时读入）；越界 start 钳制到表尾，
/// done = 本页之后无更多行（与 UI mock 实现语义一致）。
async fn text_lines(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::TextLinesParams = parse_params(params)?;
    let result = with_text_session(&ctx.sessions, &params.session_id, |data| {
        let (table_len, texts): (usize, Vec<String>) = match params.side {
            ai_protocol::DiffSide::Left => {
                let t = &data.left;
                (t.len(), t.lines_range(0, t.len()).iter().map(|e| e.text.clone()).collect())
            }
            ai_protocol::DiffSide::Right => {
                let t = &data.right;
                (t.len(), t.lines_range(0, t.len()).iter().map(|e| e.text.clone()).collect())
            }
        };
        let start = (params.start as usize).min(table_len);
        let end = start.saturating_add(params.count as usize).min(table_len);
        Ok(ai_protocol::TextLinesResult {
            side: params.side,
            start: start as u32,
            lines: texts[start..end].to_vec(),
            done: end >= table_len,
        })
    })
    .await?;
    Ok(Handled::Value(serde_json::to_value(&result).expect("text/lines 结果可序列化")))
}

/// text/inlineDiff：单个 section 的行内差异懒计算（能力位 inlineDiff=true）
///
/// - Change section：逐行位置配对计算（超长行 > maxLineChars 或超时降级，
///   对应行 truncated=true，section 状态记 Partial）；
/// - Equal/Insert/Delete：无行内差异内容，状态记 Full；
/// - 计算结果状态写回会话（下次直接命中，不重算）。
async fn text_inline_diff(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::TextInlineDiffParams = parse_params(params)?;
    let result = with_text_session(&ctx.sessions, &params.session_id, |data| {
        let core_idx = data
            .sections
            .iter()
            .position(|s| s.id == params.section_id)
            .ok_or_else(|| {
                ai_protocol::error_object_with_data(
                    codes::INVALID_PARAMS,
                    format!("section 不存在: {}", params.section_id),
                    serde_json::json!({ "sessionId": params.session_id, "sectionId": params.section_id }),
                )
            })?;
        let kind = data.sections[core_idx].kind;
        let (lines, state) = match kind {
            ai_core::sections::SectionKind::Change => {
                let (dtos, any_truncated) = compute_inline(data, core_idx);
                let state = if any_truncated {
                    ai_protocol::InlineState::Partial
                } else {
                    ai_protocol::InlineState::Full
                };
                (dtos, state)
            }
            // Equal/Insert/Delete：无行内差异内容，无待算工作
            _ => (Vec::new(), ai_protocol::InlineState::Full),
        };
        Ok(ai_protocol::TextInlineDiffResult {
            session_id: params.session_id.clone(),
            section_id: params.section_id,
            kind: mapping::section_to_dto(&data.sections[core_idx]).kind,
            state,
            lines,
        })
    })
    .await?;
    Ok(Handled::Value(serde_json::to_value(&result).expect("text/inlineDiff 结果可序列化")))
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

#[cfg(test)]
mod p3_tests {
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

    /// 文本语料：首尾相同 + Change（含尾随空白差异行）+ Insert
    async fn open_text(c: &HandlerCtx, id: &str) {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L.txt");
        let r = dir.path().join("R.txt");
        std::fs::write(&l, "alpha\nbeta one  \ngamma\n").unwrap();
        std::fs::write(&r, "alpha\nbeta one\ngamma\nNEW\n").unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "text/open",
            Some(json!({"sessionId": id, "left": l.to_string_lossy(), "right": r.to_string_lossy()})),
            c,
            &ai_dir::CancelFlag::new(),
            tx,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn text_lines_pagination_clamps_and_reports_done() {
        let c = ctx();
        open_text(&c, "t1").await;
        // 第 1 页：2 行，done=false
        let r = dispatch(
            "text/lines",
            Some(json!({"sessionId": "t1", "side": "left", "start": 0, "count": 2})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["side"], "left");
        assert_eq!(v["start"], 0);
        assert_eq!(v["lines"].as_array().unwrap().len(), 2);
        assert_eq!(v["done"], false);
        // 第 2 页越界钳制：start=2 取 10 行 -> 只剩 1 行，done=true
        let r = dispatch(
            "text/lines",
            Some(json!({"sessionId": "t1", "side": "left", "start": 2, "count": 10})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["start"], 2);
        assert_eq!(v["lines"].as_array().unwrap().len(), 1);
        assert_eq!(v["done"], true);
        // 越界 start（>= 行数）钳制为空页
        let r = dispatch(
            "text/lines",
            Some(json!({"sessionId": "t1", "side": "right", "start": 99, "count": 5})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["start"], 4, "right 侧行数 4，start 钳制到表尾");
        assert_eq!(v["lines"].as_array().unwrap().len(), 0);
        assert_eq!(v["done"], true);
        // 会话不存在 -> 410
        let e = dispatch(
            "text/lines",
            Some(json!({"sessionId": "ghost", "side": "left", "start": 0, "count": 1})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn text_inline_diff_lazy_compute_and_states() {
        let c = ctx();
        open_text(&c, "t2").await;
        // sections：equal(0) / change(1) / insert(2)
        let r = dispatch(
            "text/sections",
            Some(json!({"sessionId": "t2", "offset": 0, "count": 10})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        let secs = v["sections"].as_array().unwrap();
        assert_eq!(secs[1]["inline"], "full", "text/sections 窗口内 Change 触发懒计算: {v}");
        assert_eq!(secs[2]["inline"], "notComputed", "Insert 无行内内容");
        // inlineDiff RPC：Change section 返回 spans（"beta one  " vs "beta one" 词级 Replace）
        let r = dispatch(
            "text/inlineDiff",
            Some(json!({"sessionId": "t2", "sectionId": 1})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["kind"], "change");
        assert_eq!(v["state"], "full");
        let lines = v["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1, "Change 单行配对");
        assert_eq!(lines[0]["leftLine"], 1);
        assert_eq!(lines[0]["rightLine"], 1);
        let kinds: Vec<&str> = lines[0]["spans"].as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap()).collect();
        assert!(kinds.contains(&"delete"), "尾随空白词级差异: {kinds:?}");
        // 非 Change section：状态 Full、无行内容
        let r = dispatch(
            "text/inlineDiff",
            Some(json!({"sessionId": "t2", "sectionId": 0})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["kind"], "equal");
        assert_eq!(v["state"], "full");
        assert_eq!(v["lines"].as_array().unwrap().len(), 0);
        // section 不存在 -> INVALID_PARAMS
        let e = dispatch(
            "text/inlineDiff",
            Some(json!({"sessionId": "t2", "sectionId": 99})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn text_inline_diff_partial_on_oversized_line() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(2000);
        let l = dir.path().join("L.txt");
        let r = dir.path().join("R.txt");
        std::fs::write(&l, format!("{big}\n")).unwrap();
        std::fs::write(&r, format!("{big}y\n")).unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "text/open",
            Some(json!({"sessionId": "t3", "left": l.to_string_lossy(), "right": r.to_string_lossy()})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let r = dispatch(
            "text/inlineDiff",
            Some(json!({"sessionId": "t3", "sectionId": 0})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["state"], "partial", "超长行（> maxLineChars 1000）降级: {v}");
        let lines = v["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["truncated"], true);
        assert_eq!(lines[0]["spans"].as_array().unwrap().len(), 0, "降级行无 spans");
    }

    #[tokio::test]
    async fn text_sections_with_keys_and_importance_update() {
        let c = ctx();
        open_text(&c, "t4").await;
        // withKeys：Change 段（左行 1 + 右行 1）+ Insert 段（右行 3）= 3 个双键条目
        let r = dispatch(
            "text/sections",
            Some(json!({"sessionId": "t4", "offset": 0, "count": 10, "withKeys": true})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        let keys = v["lineKeys"].as_array().expect("withKeys 应返回 lineKeys");
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[0]["side"], "left");
        assert_eq!(keys[0]["line"], 1);
        assert_eq!(keys[0]["raw"], "beta one  ");
        assert_eq!(keys[1]["side"], "right");
        assert_eq!(keys[1]["raw"], "beta one");
        // 默认重要性模型下（尾随空白重要）两侧键不同
        assert_ne!(keys[0]["importance"], keys[1]["importance"]);

        // importance 重判：尾随空白不重要 -> Change 段转 Unimportant，通知推送 ImportanceUpdate
        let (tx, mut rx) = chan();
        let r = dispatch(
            "text/sections",
            Some(json!({
                "sessionId": "t4", "offset": 0, "count": 10,
                "importance": {"trailingWsUnimportant": true}
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        let secs = v["sections"].as_array().unwrap();
        assert_eq!(secs[1]["important"], "unimportant", "尾随空白差异重判为不重要: {v}");
        assert_eq!(secs[2]["important"], "important", "孤儿行始终重要（FR-TXT-010）");
        // 通知：通知先于应答入队（此时应答尚在返回值中），rx 里只有通知
        let mut got = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            got.push(msg);
        }
        let notif = got
            .iter()
            .find(|m| matches!(m, Outbound::Notification(n) if n.method == "text/sections"))
            .expect("应推送 text/sections ImportanceUpdate 通知");
        let payload = match notif {
            Outbound::Notification(n) => &n.params,
            _ => unreachable!(),
        };
        assert_eq!(payload["event"], "importanceUpdate");
        assert_eq!(payload["updatedIds"], json!([1]));
        assert_eq!(payload["importance"], "unimportant");
        // 重判后 withKeys 的重要性键随新规则折叠 -> 两侧一致
        let r = dispatch(
            "text/sections",
            Some(json!({"sessionId": "t4", "offset": 0, "count": 10, "withKeys": true})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        let keys = v["lineKeys"].as_array().unwrap();
        assert_eq!(keys[0]["importance"], keys[1]["importance"], "重判后双键一致");
    }

    #[tokio::test]
    async fn manual_align_options_on_text_open() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L.txt");
        let r = dir.path().join("R.txt");
        // 左：keep, A, B, tail；右：keep, tail
        std::fs::write(&l, "keep\nA\nB\ntail\n").unwrap();
        std::fs::write(&r, "keep\ntail\n").unwrap();
        let (tx, _rx) = chan();
        // 手动对齐：keep(0,1) 对 keep(0,1)（锚点无效组合会被 validate 拒绝）
        let err = dispatch(
            "text/open",
            Some(json!({
                "sessionId": "t5", "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "manual": {"alignAnchors": [{"left": {"start": 0, "end": 2}, "right": {"start": 0, "end": 1}}]}
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::INVALID_PARAMS, "等长校验：锚点行数不等应拒绝");
        // 隔离左侧行 1..3 -> A/B 不与右侧行配对（独立 Delete），不产生 Change
        let (tx, _rx) = chan();
        let r = dispatch(
            "text/open",
            Some(json!({
                "sessionId": "t5", "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "manual": {"isolate": [{"side": "left", "range": {"start": 1, "end": 3}}]}
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        let _ = v;
        let r = dispatch(
            "text/sections",
            Some(json!({"sessionId": "t5", "offset": 0, "count": 10})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        let kinds: Vec<&str> = v["sections"].as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap()).collect();
        assert!(kinds.contains(&"delete"), "隔离后 A/B 成 Delete 块: {kinds:?}");
        assert!(!kinds.contains(&"change"), "隔离行不与对侧配对成 Change: {kinds:?}");
    }
}
