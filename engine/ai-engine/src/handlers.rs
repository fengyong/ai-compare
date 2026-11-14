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
use ai_core::{MergeDecision, MarkerOptions, MarkerStyle, SynthesizeOptions, UnresolvedPolicy};
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
    /// P10：文件格式注册表（内置 + 会话期 formats/save|load 的自定义格式）。
    /// 进程内权威态；持久化由 UI 设置层负责（session-defaults.json / formats.json）
    pub formats: Arc<Mutex<ai_core::FormatRegistry>>,
    /// P10：规则集（global/kind/session 三层 + 格式绑定）
    pub rules: Arc<Mutex<ai_core::RuleSet>>,
}

impl HandlerCtx {
    pub fn new(max_batch_rows: usize) -> Self {
        HandlerCtx {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            vfs: Arc::new(ai_vfs::LocalFs::new()),
            max_batch_rows,
            formats: Arc::new(Mutex::new(ai_core::FormatRegistry::builtin())),
            rules: Arc::new(Mutex::new(ai_core::RuleSet::default())),
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
        ai_protocol::method::TEXT_MERGE3_OPEN => merge3_open(params, ctx).await,
        ai_protocol::method::TEXT_MERGE3_SECTIONS => merge3_sections(params, ctx).await,
        ai_protocol::method::TEXT_MERGE3_DECIDE => merge3_decide(params, ctx).await,
        ai_protocol::method::TEXT_MERGE3_OUTPUT => merge3_output(params, ctx).await,
        ai_protocol::method::TABLE_OPEN => table_open(params, ctx).await,
        ai_protocol::method::TABLE_SECTIONS => table_sections(params, ctx).await,
        ai_protocol::method::HEX_OPEN => hex_open(params, ctx).await,
        ai_protocol::method::HEX_BYTES => hex_bytes(params, ctx).await,
        ai_protocol::method::TEXT_PATCH_APPLY => patch_apply(params, ctx).await,
        ai_protocol::method::DIR_EXECUTE => dir_execute(params, ctx, cancel).await,
        ai_protocol::method::FORMATS_LIST => formats_list(params, ctx).await,
        ai_protocol::method::FORMATS_SAVE => formats_save(params, ctx).await,
        ai_protocol::method::FORMATS_LOAD => formats_load(params, ctx).await,
        ai_protocol::method::RULES_GET => rules_get(params, ctx).await,
        ai_protocol::method::RULES_SET => rules_set(params, ctx).await,
        ai_protocol::method::REPORT_GENERATE => report_generate(params, ctx).await,
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
            // P7：三向文本合并入口开放（text/merge3/*）
            three_way_merge: Some(true),
            // P9：表格 / 十六进制 / 补丁应用入口开放（table/*、hex/*、text/patch/apply）
            table_compare: Some(true),
            hex_compare: Some(true),
            patch_apply: Some(true),
            // P6 提案合入：dir/execute 文件操作执行
            dir_execute: Some(true),
            // P10：文件格式与规则体系（formats/*、rules/*、text/open rules、dir formatAware）
            format_rules: Some(true),
            // P11：报告生成入口（report/generate）
            reports: Some(true),
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
    // P10：formatAware = 文本 vs 二进制按格式表 + 内容嗅探判定（首批 delta 注解）
    let annotations = if params.format_aware == Some(true) {
        annotate_file_kinds(ctx, &rows, &left, &right).await
    } else {
        HashMap::new()
    };
    let batches = batch::batch_rows(rows, ctx.max_batch_rows);
    let rev = batches.len() as u64;

    {
        let mut sessions = ctx.sessions.lock().await;
        sessions.insert(
            params.session_id.clone(),
            Session::Dir(DirSessionData { model: None, scan: Some(scan_result.clone()), roots: Some((left.clone(), right.clone())) }),
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
        notifications.push(format_aware_delta_notification(
            &params.session_id,
            &delta,
            &annotations,
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

    // P10：formatAware 首批注解（后台 pump 推进的 delta 不注解——fileKind
    // 为可选注解字段，UI 未收到时按原语义渲染）
    let annotations = if params.format_aware == Some(true) {
        annotate_file_kinds(ctx, &first.upserts, &left, &right).await
    } else {
        HashMap::new()
    };
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
            Session::Dir(DirSessionData { model: Some(model.clone()), scan: None, roots: Some((left.clone(), right.clone())) }),
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
        notifications.push(format_aware_delta_notification(
            &params.session_id,
            &delta,
            &annotations,
        ));
    }
    Ok(Handled::WithNotifications {
        response: serde_json::to_value(&response).expect("dir/open 结果可序列化"),
        notifications,
    })
}

// ---------------------------------------------------------------------------
// P10：dir/open 格式感知（文本 vs 二进制按格式表 + 内容嗅探）
// ---------------------------------------------------------------------------

/// 嗅探读取上限（字节）
const FORMAT_SNIFF_HEAD_BYTES: usize = 8192;
/// 单次 open 的嗅探行上限（防全量扫描开销；超出部分不注解）
const MAX_FORMAT_SNIFF_ROWS: usize = 512;

/// 为首批 delta 行对计算 fileKind 注解（两侧俱在的文件行；左侧文件嗅探）。
/// DirRow 结构保持 P9 形态——注解在通知序列化后的 Value 上注入 `fileKind`。
async fn annotate_file_kinds(
    ctx: &HandlerCtx,
    rows: &[ai_protocol::DirRow],
    left_root: &VfsPath,
    _right_root: &VfsPath,
) -> HashMap<String, ai_protocol::DetectedFileKind> {
    let mut out: HashMap<String, ai_protocol::DetectedFileKind> = HashMap::new();
    if rows.is_empty() {
        return out;
    }
    let reg = ctx.formats.lock().await;
    for row in rows.iter().take(MAX_FORMAT_SNIFF_ROWS) {
        if row.kind != ai_protocol::EntryKind::File {
            continue;
        }
        let (Some(_l), Some(_r)) = (&row.left, &row.right) else {
            continue;
        };
        let path = left_root.join(&row.id);
        let head = match ctx.vfs.read_range(&path, 0, FORMAT_SNIFF_HEAD_BYTES).await {
            Ok(h) => h,
            Err(_) => continue, // 读取失败不注解（UI 按缺省语义渲染）
        };
        let kind = match reg.classify_file(&row.name, &head) {
            ai_core::FileClass::Binary => ai_protocol::DetectedFileKind::Binary,
            ai_core::FileClass::Text { .. } => ai_protocol::DetectedFileKind::Text,
        };
        out.insert(row.id.clone(), kind);
    }
    out
}

/// 构造 dir/delta 通知（无注解时与既有字节完全一致；有注解时在
/// upserts[i].fileKind 注入 "text"/"binary"）
fn format_aware_delta_notification(
    _session_id: &str,
    delta: &ai_protocol::DirDelta,
    annotations: &HashMap<String, ai_protocol::DetectedFileKind>,
) -> ai_protocol::RpcServerNotification {
    let mut value = serde_json::to_value(delta).expect("dir/delta 可序列化");
    if !annotations.is_empty() {
        if let Some(upserts) = value.get_mut("upserts").and_then(|u| u.as_array_mut()) {
            for row in upserts {
                let id = row.get("id").and_then(|i| i.as_str()).map(String::from);
                if let Some(id) = id {
                    if let (Some(kind), Some(obj)) = (annotations.get(&id), row.as_object_mut()) {
                        obj.insert(
                            "fileKind".to_string(),
                            serde_json::to_value(kind).unwrap_or(serde_json::Value::Null),
                        );
                    }
                }
            }
        }
    }
    ai_protocol::RpcServerNotification::new(ai_protocol::method::DIR_DELTA, value)
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
        Session::Text(_) | Session::Merge(_) | Session::Table(_) | Session::Hex(_) => {
            Err(bad_params(format!("会话不是目录会话: {id}")))
        }
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
    // P10：会话规则应用链（rules → ImportanceOptions + 语法折叠 line_classify 钩子）。
    // unimportantElements 非空时按左侧文件名（缺省右）检测文件格式，
    // 取其语法做重要性键折叠（骨架不变；格式未命中 → 不折叠）
    if let Some(spec) = &params.rules {
        let (imp, elements) = mapping::rules_spec_to_core(spec);
        config.importance = imp;
        if let Some(elements) = elements.filter(|e| !e.is_empty()) {
            let reg = ctx.formats.lock().await;
            let fname = left_path
                .file_name()
                .or_else(|| right_path.file_name())
                .unwrap_or_default();
            if let Some(fmt) = reg.detect(fname, &[]) {
                let effective = ai_core::EffectiveRules {
                    compare: ai_core::CompareRules::default(),
                    unimportant_elements: elements,
                };
                config.base.rules = ai_core::RulesOptions::resolve(&effective, Some(fmt));
            }
        }
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
                labels: (params.left.clone(), params.right.clone()),
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
        Session::Dir(_) | Session::Merge(_) | Session::Table(_) | Session::Hex(_) => {
            Err(bad_params(format!("会话不是文本会话: {id}")))
        }
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
// P7：text/merge3/*（三向合并；docs/03 §4.5 / FR-TMRG）
// ---------------------------------------------------------------------------

/// text/merge3/open：读三方文本 -> merge3 模型（一次求出）-> 存会话 -> 应答统计。
/// 块清单经 text/merge3/sections 分批取回（与 text/open + text/sections 同构）。
async fn merge3_open(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::MergeOpenParams = parse_params(params)?;
    if params.session_id.is_empty() {
        return Err(bad_params("sessionId 不能为空"));
    }
    let base_path = VfsPath::parse(&params.base).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let left_path = VfsPath::parse(&params.left).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right_path = VfsPath::parse(&params.right).map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    let base_text = ctx.vfs.read_file_text(&base_path).await.map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let left_text = ctx.vfs.read_file_text(&left_path).await.map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right_text = ctx.vfs.read_file_text(&right_path).await.map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    let base = LineTable::from_text(&base_text);
    let left = LineTable::from_text(&left_text);
    let right = LineTable::from_text(&right_text);

    let options = params
        .options
        .as_ref()
        .map(mapping::merge_options_from_dto)
        .unwrap_or_default();

    let model = ai_core::merge3(&base, &left, &right, &options);
    let conflict_count = model.conflict_count();
    let chunk_count = u32::try_from(model.chunks.len()).unwrap_or(u32::MAX);
    let (base_len, left_len, right_len) = (model.base_len, model.left_len, model.right_len);

    {
        let mut sessions = ctx.sessions.lock().await;
        if sessions.contains_key(&params.session_id) {
            return Err(RpcErrorObject {
                code: codes::SESSION_EXISTS,
                message: format!("会话已存在: {}", params.session_id),
                data: Some(json!({ "sessionId": params.session_id })),
            });
        }
        sessions.insert(
            params.session_id.clone(),
            Session::Merge(crate::server::MergeSessionData {
                base,
                left,
                right,
                model,
                options,
                labels: (params.base.clone(), params.left.clone(), params.right.clone()),
                decisions: HashMap::new(),
                rev: 0,
            }),
        );
    }

    let result = ai_protocol::MergeOpenResult {
        session_id: params.session_id,
        base_line_count: base_len,
        left_line_count: left_len,
        right_line_count: right_len,
        chunk_count,
        conflict_count,
        rev: 0,
    };
    Ok(Handled::Value(serde_json::to_value(&result).expect("merge3/open 结果可序列化")))
}

/// 取合并会话（可变访问；会话表锁内不做重活/不做 await）
async fn with_merge_session<T>(
    sessions: &SessionMap,
    id: &str,
    f: impl FnOnce(&mut crate::server::MergeSessionData) -> Result<T, RpcErrorObject>,
) -> Result<T, RpcErrorObject> {
    let mut sessions = sessions.lock().await;
    let session = sessions.get_mut(id).ok_or_else(|| session_not_found(id))?;
    match session {
        Session::Merge(data) => f(data),
        _ => Err(bad_params(format!("会话不是三向合并会话: {id}"))),
    }
}

/// 会话决策状态的合成策略（sections/output 共用；未决冲突渲染 marker 行为空）
fn session_syn_options(data: &crate::server::MergeSessionData) -> SynthesizeOptions {
    SynthesizeOptions {
        unresolved: data.options.conflict_default.clone().to_policy(None),
    }
}

/// marker 模式的策略（标签 = 打开时路径原串）
fn marker_policy_of(data: &crate::server::MergeSessionData, style: MarkerStyle) -> SynthesizeOptions {
    SynthesizeOptions {
        unresolved: UnresolvedPolicy::Markers(MarkerOptions {
            style,
            left_label: data.labels.1.clone(),
            base_label: data.labels.0.clone(),
            right_label: data.labels.2.clone(),
        }),
    }
}

/// text/merge3/sections：块清单分批窗口（含三方行内容与当前决策状态）
async fn merge3_sections(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::MergeSectionsParams = parse_params(params)?;
    let result = with_merge_session(&ctx.sessions, &params.session_id, |data| {
        let total = u32::try_from(data.model.chunks.len()).unwrap_or(u32::MAX);
        let start = params.offset.min(total) as usize;
        let end = start.saturating_add(params.count as usize).min(data.model.chunks.len());
        let syn = session_syn_options(data);
        let chunks: Vec<ai_protocol::MergeChunkDto> = data.model.chunks[start..end]
            .iter()
            .map(|c| mapping::merge_chunk_to_dto(c, &data.decisions, &data.base, &data.left, &data.right, &syn))
            .collect();
        let has_more = end < data.model.chunks.len();
        Ok(ai_protocol::MergeSectionsResult {
            session_id: params.session_id.clone(),
            offset: params.offset,
            total,
            has_more,
            rev: data.rev,
            chunks,
        })
    })
    .await?;
    Ok(Handled::Value(serde_json::to_value(&result).expect("merge3/sections 结果可序列化")))
}

/// text/merge3/decide：块决策提交（幂等——决策内容不变时 rev 与结果不变）
async fn merge3_decide(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::MergeDecideParams = parse_params(params)?;
    let session_id = params.session_id.clone();
    let decided = u32::try_from(params.decisions.len()).unwrap_or(u32::MAX);
    let result = with_merge_session(&ctx.sessions, &session_id, |data| {
        // 先解析全部决策（任一非法则整批拒绝）
        let mut parsed: Vec<(u32, MergeDecision)> = Vec::with_capacity(params.decisions.len());
        for d in &params.decisions {
            if d.chunk_id as usize >= data.model.chunks.len() {
                return Err(bad_params(format!("块不存在: {}", d.chunk_id)));
            }
            let core = mapping::merge_decision_from_dto(d).map_err(bad_params)?;
            parsed.push((d.chunk_id, core));
        }
        // 幂等：决策内容与现状完全一致时不递增 rev、不改会话
        let changed = parsed.iter().any(|(id, dec)| data.decisions.get(id) != Some(dec));
        if changed {
            for (id, dec) in parsed {
                data.decisions.insert(id, dec);
            }
            data.rev += 1;
        }
        let unresolved = data.model.unresolved_with(&data.decisions);
        Ok(ai_protocol::MergeDecideResult {
            session_id: session_id.clone(),
            rev: data.rev,
            decided,
            conflict_count: data.model.conflict_count(),
            unresolved_chunks: unresolved,
        })
    })
    .await?;
    Ok(Handled::Value(serde_json::to_value(&result).expect("merge3/decide 结果可序列化")))
}

/// text/merge3/output：合成输出（决策生效；未决冲突按模式渲染 + 清单）
async fn merge3_output(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::MergeOutputParams = parse_params(params)?;
    let result = with_merge_session(&ctx.sessions, &params.session_id, |data| {
        let syn = match params.mode {
            Some(ai_protocol::MergeOutputMode::Marker) => {
                let style = match params.marker_style.unwrap_or(ai_protocol::MergeMarkerStyle::Merge) {
                    ai_protocol::MergeMarkerStyle::Merge => MarkerStyle::Merge,
                    ai_protocol::MergeMarkerStyle::Diff3 => MarkerStyle::Diff3,
                };
                marker_policy_of(data, style)
            }
            // 缺省 Plain：未决冲突走会话 conflict_default 策略
            _ => session_syn_options(data),
        };
        let out = ai_core::synthesize(&data.model, &data.base, &data.left, &data.right, &data.decisions, &syn);
        let unresolved = data.model.unresolved_with(&data.decisions);
        let has_conflicts = !unresolved.is_empty();
        Ok(ai_protocol::MergeOutputResult {
            session_id: params.session_id.clone(),
            text: out.text,
            conflict_count: data.model.conflict_count(),
            unresolved_chunks: unresolved,
            has_conflicts,
            decided_count: u32::try_from(data.decisions.len()).unwrap_or(u32::MAX),
        })
    })
    .await?;
    Ok(Handled::Value(serde_json::to_value(&result).expect("merge3/output 结果可序列化")))
}

// ---------------------------------------------------------------------------
// P9：table/open|sections（表格比较）与 hex/open|bytes（十六进制比较）
// ---------------------------------------------------------------------------

/// table/open：读两侧 CSV/TSV -> 解析（RFC4180）-> 列对齐 + 行对齐 + 单元格 diff
/// -> 存会话 -> 应答比较列模型与统计。行清单经 table/sections 分批取回。
async fn table_open(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::TableOpenParams = parse_params(params)?;
    if params.session_id.is_empty() {
        return Err(bad_params("sessionId 不能为空"));
    }
    let left_path = VfsPath::parse(&params.left).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right_path = VfsPath::parse(&params.right).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let left_text = ctx.vfs.read_file_text(&left_path).await.map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right_text = ctx.vfs.read_file_text(&right_path).await.map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    let opts = table_options_from_dto(params.options.as_ref());
    let left = ai_core::table::parse_table(&left_text, &opts.parse)
        .map_err(|e| ai_protocol::error_object(codes::INVALID_PARAMS, format!("左侧表格解析失败: {e}")))?;
    let right = ai_core::table::parse_table(&right_text, &opts.parse)
        .map_err(|e| ai_protocol::error_object(codes::INVALID_PARAMS, format!("右侧表格解析失败: {e}")))?;
    let model = ai_core::table::diff_table_data(&left, &right, &opts);

    let row_count = u32::try_from(model.rows.len()).unwrap_or(u32::MAX);
    let (left_row_count, right_row_count) = (
        u32::try_from(left.row_count()).unwrap_or(u32::MAX),
        u32::try_from(right.row_count()).unwrap_or(u32::MAX),
    );
    let columns = model.columns.iter().map(mapping::table_column_to_dto).collect();
    let summary = mapping::table_summary_to_dto(&model.summary);

    {
        let mut sessions = ctx.sessions.lock().await;
        if sessions.contains_key(&params.session_id) {
            return Err(RpcErrorObject {
                code: codes::SESSION_EXISTS,
                message: format!("会话已存在: {}", params.session_id),
                data: Some(json!({ "sessionId": params.session_id })),
            });
        }
        sessions.insert(
            params.session_id.clone(),
            Session::Table(crate::server::TableSessionData {
                left,
                right,
                model,
                options: opts,
                labels: (params.left.clone(), params.right.clone()),
            }),
        );
    }

    let result = ai_protocol::TableOpenResult {
        session_id: params.session_id,
        row_count,
        left_row_count,
        right_row_count,
        columns,
        summary,
    };
    Ok(Handled::Value(serde_json::to_value(&result).expect("table/open 结果可序列化")))
}

/// ai_protocol TableOptionsSpec -> ai_core TableDiffOptions（缺省 = 引擎默认）
fn table_options_from_dto(spec: Option<&ai_protocol::TableOptionsSpec>) -> ai_core::table::TableDiffOptions {
    let mut opts = ai_core::table::TableDiffOptions::default();
    let Some(spec) = spec else { return opts };
    if let Some(d) = &spec.delimiter {
        if let Some(c) = d.chars().next() {
            opts.parse.delimiter = c;
        }
    }
    if let Some(h) = spec.has_header {
        opts.parse.has_header = h;
    }
    if let Some(keys) = &spec.key_columns {
        opts.key_columns = keys.iter().map(|&k| k as usize).collect();
    }
    if let Some(a) = spec.column_align {
        opts.column_align = match a {
            ai_protocol::TableColumnAlign::None => ai_core::table::ColumnAlign::None,
            ai_protocol::TableColumnAlign::ByLeftName => ai_core::table::ColumnAlign::ByLeftName,
            ai_protocol::TableColumnAlign::ByRightName => ai_core::table::ColumnAlign::ByRightName,
        };
    }
    if let Some(a) = spec.row_align {
        opts.row_align = match a {
            ai_protocol::TableRowAlign::Unaligned => ai_core::table::RowAlign::Unaligned,
            ai_protocol::TableRowAlign::Standard => ai_core::table::RowAlign::Standard,
        };
    }
    if let Some(sk) = spec.skew_tolerance {
        opts.skew_tolerance = sk;
    }
    if spec.case_unimportant.is_some() || spec.whitespace.is_some() || spec.numeric_tolerance.is_some() {
        let mut imp = ai_core::table::TableImportance::default();
        if let Some(c) = spec.case_unimportant {
            imp.case_unimportant = c;
        }
        if let Some(w) = spec.whitespace {
            imp.whitespace = match w {
                ai_protocol::WhitespaceMode::Significant => ai_core::options::WhitespaceMode::Significant,
                ai_protocol::WhitespaceMode::IgnoreAmount => ai_core::options::WhitespaceMode::IgnoreAmount,
                ai_protocol::WhitespaceMode::IgnoreAll => ai_core::options::WhitespaceMode::IgnoreAll,
            };
        }
        imp.numeric_tolerance = spec.numeric_tolerance;
        opts.importance = imp;
    }
    opts
}

/// table/sections：diff 行清单分批窗口
async fn table_sections(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::TableSectionsParams = parse_params(params)?;
    let result = with_table_session(&ctx.sessions, &params.session_id, |data| {
        let total = u32::try_from(data.model.rows.len()).unwrap_or(u32::MAX);
        let start = params.offset.min(total) as usize;
        let end = start.saturating_add(params.count as usize).min(data.model.rows.len());
        let rows: Vec<ai_protocol::TableRowDto> = data.model.rows[start..end]
            .iter()
            .map(mapping::table_row_to_dto)
            .collect();
        let has_more = end < data.model.rows.len();
        Ok(ai_protocol::TableSectionsResult {
            session_id: params.session_id.clone(),
            offset: params.offset,
            total,
            has_more,
            rows,
        })
    })
    .await?;
    Ok(Handled::Value(serde_json::to_value(&result).expect("table/sections 结果可序列化")))
}

/// 取表格会话（可变访问；会话表锁内不做重活/不做 await）
async fn with_table_session<T>(
    sessions: &SessionMap,
    id: &str,
    f: impl FnOnce(&mut crate::server::TableSessionData) -> Result<T, RpcErrorObject>,
) -> Result<T, RpcErrorObject> {
    let mut sessions = sessions.lock().await;
    let session = sessions.get_mut(id).ok_or_else(|| session_not_found(id))?;
    match session {
        Session::Table(data) => f(data),
        _ => Err(bad_params(format!("会话不是表格会话: {id}"))),
    }
}

/// hex/open：读两侧字节流 -> 分块哈希粗对齐 + 精对齐 -> 块模型 + 每字节状态图
/// -> 存会话 -> 应答统计。十六进制行经 hex/bytes 分页取数。
async fn hex_open(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::HexOpenParams = parse_params(params)?;
    if params.session_id.is_empty() {
        return Err(bad_params("sessionId 不能为空"));
    }
    let left_path = VfsPath::parse(&params.left).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right_path = VfsPath::parse(&params.right).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let left = ctx.vfs.read_file_bytes(&left_path).await.map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right = ctx.vfs.read_file_bytes(&right_path).await.map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    let mut opts = ai_core::hex::HexOptions::default();
    if let Some(spec) = &params.options {
        if let Some(c) = spec.chunk_size {
            opts.chunk_size = c.max(1) as usize;
        }
        if let Some(m) = spec.align_mode {
            opts.align_mode = match m {
                ai_protocol::HexAlignMode::Full => ai_core::hex::HexAlignMode::Full,
                ai_protocol::HexAlignMode::Fast => ai_core::hex::HexAlignMode::Fast,
                ai_protocol::HexAlignMode::None => ai_core::hex::HexAlignMode::None,
            };
        }
    }
    let model = ai_core::hex::diff_bytes(&left, &right, &opts);
    let (status_left, status_right) = ai_core::hex::status_maps(&model);
    let block_count = u32::try_from(model.blocks.len()).unwrap_or(u32::MAX);
    let summary = ai_protocol::HexSummary {
        same_bytes: model.summary.same_bytes,
        changed_bytes: model.summary.changed_bytes,
        inserted_bytes: model.summary.inserted_bytes,
        deleted_bytes: model.summary.deleted_bytes,
    };
    let left_size = model.left_len;
    let right_size = model.right_len;

    {
        let mut sessions = ctx.sessions.lock().await;
        if sessions.contains_key(&params.session_id) {
            return Err(RpcErrorObject {
                code: codes::SESSION_EXISTS,
                message: format!("会话已存在: {}", params.session_id),
                data: Some(json!({ "sessionId": params.session_id })),
            });
        }
        sessions.insert(
            params.session_id.clone(),
            Session::Hex(crate::server::HexSessionData {
                status_left,
                status_right,
                options: opts,
                model,
                left,
                right,
                labels: (params.left.clone(), params.right.clone()),
            }),
        );
    }

    let result = ai_protocol::HexOpenResult {
        session_id: params.session_id,
        left_size,
        right_size,
        block_count,
        summary,
    };
    Ok(Handled::Value(serde_json::to_value(&result).expect("hex/open 结果可序列化")))
}

/// 取十六进制会话（可变访问）
async fn with_hex_session<T>(
    sessions: &SessionMap,
    id: &str,
    f: impl FnOnce(&mut crate::server::HexSessionData) -> Result<T, RpcErrorObject>,
) -> Result<T, RpcErrorObject> {
    let mut sessions = sessions.lock().await;
    let session = sessions.get_mut(id).ok_or_else(|| session_not_found(id))?;
    match session {
        Session::Hex(data) => f(data),
        _ => Err(bad_params(format!("会话不是十六进制会话: {id}"))),
    }
}

/// hex/bytes：十六进制行分页（16 字节/行；offset 对齐行首；含逐字节状态串）
async fn hex_bytes(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::HexBytesParams = parse_params(params)?;
    const PAGE_BYTES: usize = 4096; // 单页上限 256 行
    const ROW: u64 = ai_core::hex::HEX_LINE_BYTES as u64;
    let result = with_hex_session(&ctx.sessions, &params.session_id, |data| {
        let (bytes, status): (&Vec<u8>, &Vec<ai_core::hex::HexByteStatus>) = match params.side {
            ai_protocol::DiffSide::Left => (&data.left, &data.status_left),
            ai_protocol::DiffSide::Right => (&data.right, &data.status_right),
        };
        let size = bytes.len() as u64;
        // offset 向下对齐行首；越界钳制到表尾行
        let row_index = (params.offset / ROW).min(size.div_ceil(ROW).saturating_sub(1));
        let start = row_index * ROW;
        let max_len = (size - start) as usize;
        let want = (params.length.max(1) as usize).min(PAGE_BYTES).min(max_len);
        let want = want.div_ceil(ai_core::hex::HEX_LINE_BYTES) * ai_core::hex::HEX_LINE_BYTES;
        let end = (start as usize + want).min(bytes.len());
        let lines: Vec<ai_protocol::HexLineDto> = ai_core::hex::render_hex_lines(bytes, row_index, (end - start as usize).div_ceil(ai_core::hex::HEX_LINE_BYTES))
            .into_iter()
            .map(|l| {
                let b0 = l.addr as usize;
                let b1 = (b0 + ai_core::hex::HEX_LINE_BYTES).min(bytes.len());
                let status: String = bytes[b0..b1]
                    .iter()
                    .zip(&status[b0..b1])
                    .map(|(_, s)| match s {
                        ai_core::hex::HexByteStatus::Same => '=',
                        ai_core::hex::HexByteStatus::Changed => 'x',
                        ai_core::hex::HexByteStatus::Inserted => '+',
                        ai_core::hex::HexByteStatus::Deleted => '-',
                    })
                    .collect();
                ai_protocol::HexLineDto { addr: l.addr, hex: l.hex, ascii: l.ascii, status }
            })
            .collect();
        let done = end >= bytes.len();
        Ok(ai_protocol::HexBytesResult {
            session_id: params.session_id.clone(),
            side: params.side,
            offset: start,
            lines,
            done,
        })
    })
    .await?;
    Ok(Handled::Value(serde_json::to_value(&result).expect("hex/bytes 结果可序列化")))
}

// ---------------------------------------------------------------------------
// P9：text/patch/apply（补丁应用；无会话一次性）
// ---------------------------------------------------------------------------

/// text/patch/apply：输入补丁文本 + 目标根目录 -> 解析 + 应用 -> 结果报告。
/// dry_run 只校验；二进制文件跳过并报告；模糊/CRLF 语义见 ai-core patch 模块。
async fn patch_apply(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::PatchApplyParams = parse_params(params)?;
    let _ = &ctx;
    if params.root.is_empty() {
        return Err(bad_params("root 不能为空（补丁相对路径的目标根目录）"));
    }
    let opts = ai_core::patch::PatchApplyOptions {
        dry_run: params.dry_run.unwrap_or(false),
        fuzz: params.fuzz.unwrap_or(3),
        crlf_tolerant: params.crlf_tolerant.unwrap_or(true),
        ignore_whitespace: false,
    };
    let report = ai_core::patch::apply_patch(
        &ai_core::patch::StdFs,
        std::path::Path::new(&params.root),
        &params.patch,
        &opts,
    )
    .map_err(|e| ai_protocol::error_object(codes::INVALID_PARAMS, format!("补丁解析失败: {e}")))?;
    let result = mapping::patch_report_to_dto(&report);
    Ok(Handled::Value(serde_json::to_value(&result).expect("text/patch/apply 结果可序列化")))
}

// ---------------------------------------------------------------------------
// P6 提案合入：dir/execute（文件操作执行 + rescan delta）
// ---------------------------------------------------------------------------

/// dir/execute：执行文件操作批次（dryRun 只校验）；
/// 非 dry_run 且会话为目录会话时执行后 rescan 并推送 dir/delta（P6 提案语义）。
async fn dir_execute(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    cancel: &ai_dir::CancelFlag,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::DirExecuteParams = parse_params(params)?;
    let session_id = params.session_id.clone();
    let dry_run = params.dry_run.unwrap_or(false);

    // 会话 + 根路径（dir/open 打开时记录）
    let (roots, model, scan) = {
        let sessions = ctx.sessions.lock().await;
        let session = sessions.get(&session_id).ok_or_else(|| session_not_found(&session_id))?;
        match session {
            Session::Dir(data) => (
                data.roots.clone().ok_or_else(|| bad_params("会话缺少根路径（非目录打开路径）"))?,
                data.model.clone(),
                data.scan.clone(),
            ),
            _ => return Err(bad_params(format!("会话不是目录会话: {session_id}"))),
        }
    };

    // DTO -> ai-dir FileOp（同构映射）
    let mut ops = Vec::with_capacity(params.ops.len());
    for (i, op) in params.ops.iter().enumerate() {
        match mapping::dir_execute_op_from_dto(op) {
            Ok(f) => ops.push(f),
            Err(msg) => {
                return Err(bad_params(format!("ops[{i}]: {msg}")));
            }
        }
    }

    let report = ai_dir::execute::execute_ops(
        ctx.vfs.as_ref(),
        &roots,
        &ops,
        &ai_dir::ExecuteOptions { dry_run },
        None,
        cancel,
    )
    .await;

    let result = ai_protocol::DirExecuteResult {
        session_id: session_id.clone(),
        total: u32::try_from(report.total).unwrap_or(u32::MAX),
        succeeded: report
            .succeeded
            .iter()
            .map(|r| mapping::dir_execute_result_to_dto(r, ai_protocol::DirExecuteOpStatus::Succeeded))
            .collect(),
        failed: report
            .failed
            .iter()
            .map(|r| mapping::dir_execute_result_to_dto(r, ai_protocol::DirExecuteOpStatus::Failed))
            .collect(),
        skipped: report
            .skipped
            .iter()
            .map(|r| mapping::dir_execute_result_to_dto(r, ai_protocol::DirExecuteOpStatus::Skipped))
            .collect(),
        dry_run: report.dry_run,
        cancelled: report.cancelled,
    };

    // 非 dry_run：rescan 推 delta（模型会话走 update_filters(None,None) 触发重扫；
    // P0 同步会话重新全量扫描替换会话数据并推完整 upsert 批次）
    let mut notifications = Vec::new();
    if !dry_run {
        if let Some(model) = &model {
            model.update_filters(None, None).await.map_err(|e| mapping::dir_scan_error_to_rpc(&e))?;
            if let Some(d) = model.take_delta().await {
                notifications = delta_notifications(&session_id, &d, ctx.max_batch_rows);
            }
        } else if let Some(old_scan) = &scan {
            // P0 同步路径：以默认判据重扫（旧路径无会话判据记录；delta 行 id 稳定，
            // UI 按 upsert/removals 幂等追平）
            let rescanned = ai_dir::scan(
                ctx.vfs.as_ref(),
                &roots.0,
                &roots.1,
                &ai_dir::DirScanOptions::default(),
                &ai_dir::ByNameAligner,
                cancel,
            )
            .await
            .map_err(|e| mapping::dir_scan_error_to_rpc(&e))?;
            let rows = ai_dir::rows_from_arena(&rescanned.arena);
            let batches = batch::batch_rows(rows, ctx.max_batch_rows);
            for (i, b) in batches.into_iter().enumerate() {
                notifications.push(ai_protocol::RpcServerNotification::new(
                    ai_protocol::method::DIR_DELTA,
                    batch::make_delta(&session_id, i as u64 + 1, &b, rescanned.stats),
                ));
            }
            let mut sessions = ctx.sessions.lock().await;
            if let Some(Session::Dir(data)) = sessions.get_mut(&session_id) {
                data.scan = Some(rescanned);
            }
            let _ = old_scan;
        }
    }

    if notifications.is_empty() {
        Ok(Handled::Value(serde_json::to_value(&result).expect("dir/execute 结果可序列化")))
    } else {
        Ok(Handled::WithNotifications {
            response: serde_json::to_value(&result).expect("dir/execute 结果可序列化"),
            notifications,
        })
    }
}

/// 一份 delta 数据切批为通知（removals 只随首批；同 push_delta 的返回版）
fn delta_notifications(
    session_id: &str,
    d: &ai_dir::DirDeltaData,
    max_batch: usize,
) -> Vec<ai_protocol::RpcServerNotification> {
    let batches = batch::batch_rows(d.upserts.clone(), max_batch);
    batches
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            ai_protocol::RpcServerNotification::new(
                ai_protocol::method::DIR_DELTA,
                batch::make_delta_with_progress(
                    session_id,
                    d.rev,
                    &b,
                    d.stats,
                    Some(d.progress),
                    if i == 0 { d.removals.clone() } else { Vec::new() },
                ),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// P10：formats/list|save|load、rules/get|set（FR-FMT-001 / FR-SES-006）
// ---------------------------------------------------------------------------

/// formats/list：全量格式列表（内置 + 会话期自定义，按生效顺序）
async fn formats_list(
    _params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let reg = ctx.formats.lock().await;
    let formats: Vec<ai_protocol::FileFormatSpec> =
        reg.formats.iter().map(mapping::format_to_spec).collect();
    let builtin_count = u32::try_from(
        reg.formats.iter().filter(|f| f.id.starts_with("builtin-")).count(),
    )
    .unwrap_or(u32::MAX);
    let response = ai_protocol::FormatsListResult { formats, builtin_count };
    Ok(Handled::Value(serde_json::to_value(&response).expect("formats/list 结果可序列化")))
}

/// formats/save：按 id 覆盖/追加重注册表（引擎会话期态；持久化由 UI 负责）
async fn formats_save(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::FormatsSaveParams = parse_params(params)?;
    let saved = params.formats.len() as u32;
    let mut reg = ctx.formats.lock().await;
    for f in &params.formats {
        reg.upsert(mapping::format_from_spec(f));
    }
    let formats: Vec<ai_protocol::FileFormatSpec> =
        reg.formats.iter().map(mapping::format_to_spec).collect();
    drop(reg);
    let response = ai_protocol::FormatsSaveResult { saved, formats };
    Ok(Handled::Value(serde_json::to_value(&response).expect("formats/save 结果可序列化")))
}

/// formats/load：从文件加载格式集（.bcpack / JSON；版本 > 1 拒绝）
async fn formats_load(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::FormatsLoadParams = parse_params(params)?;
    let path = VfsPath::parse(&params.path).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let bytes = ctx
        .vfs
        .read_file_bytes(&path)
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let text = String::from_utf8(bytes).map_err(|_| {
        ai_protocol::error_object(codes::INVALID_ENCODING, "格式集文件不是合法 UTF-8")
    })?;
    let pack: ai_protocol::FormatPackSpec = serde_json::from_str(&text)
        .map_err(|e| bad_params(format!("格式集解析失败: {e}")))?;
    if pack.version > 1 {
        return Err(bad_params(format!("格式集版本不支持: {}", pack.version)));
    }
    let mut loaded = 0u32;
    let mut reg = ctx.formats.lock().await;
    for f in &pack.formats {
        if reg.upsert(mapping::format_from_spec(f)) {
            loaded += 1;
        }
    }
    let formats: Vec<ai_protocol::FileFormatSpec> =
        reg.formats.iter().map(mapping::format_to_spec).collect();
    drop(reg);
    let response = ai_protocol::FormatsLoadResult { loaded, formats };
    Ok(Handled::Value(serde_json::to_value(&response).expect("formats/load 结果可序列化")))
}

/// rules/get：三层原始层 + 生效解析（Global → Kind → Session → FormatBinding）
async fn rules_get(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::RulesGetParams = parse_params(params)?;
    let set = ctx.rules.lock().await;
    let effective = set.resolve(params.kind.as_deref(), params.session_id.as_deref(), None);
    let response = ai_protocol::RulesGetResult {
        effective: mapping::effective_rules_to_spec(&effective),
        global: mapping::rule_layer_to_spec(&set.global),
        kind: params
            .kind
            .as_deref()
            .and_then(|k| set.type_defaults.get(k))
            .map(mapping::rule_layer_to_spec),
        session: params
            .session_id
            .as_deref()
            .and_then(|s| set.sessions.get(s))
            .map(mapping::rule_layer_to_spec),
        format_bindings: set
            .format_bindings
            .iter()
            .map(|b| ai_protocol::RuleFormatBindingSpec {
                format_id: b.format_id.clone(),
                unimportant_elements: b.unimportant_elements.clone(),
            })
            .collect(),
    };
    Ok(Handled::Value(serde_json::to_value(&response).expect("rules/get 结果可序列化")))
}

/// rules/set：写一层（global/kind/session）；可选全量替换格式绑定
async fn rules_set(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::RulesSetParams = parse_params(params)?;
    let target = match params.scope {
        ai_protocol::RuleScope::Global => ai_core::RuleTarget::Global,
        ai_protocol::RuleScope::Kind => ai_core::RuleTarget::TypeDefault(
            params.kind.clone().filter(|k| !k.is_empty()).ok_or_else(|| bad_params("scope=kind 需要 kind"))?,
        ),
        ai_protocol::RuleScope::Session => ai_core::RuleTarget::Session(
            params
                .session_id
                .clone()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| bad_params("scope=session 需要 sessionId"))?,
        ),
    };
    {
        let mut set = ctx.rules.lock().await;
        set.set_layer(target, mapping::rule_layer_from_spec(&params.layer));
        if params.clear_format_bindings {
            set.format_bindings.clear();
        } else if !params.format_bindings.is_empty() {
            set.format_bindings = params
                .format_bindings
                .iter()
                .map(|b| ai_core::RuleFormatBinding {
                    format_id: b.format_id.clone(),
                    unimportant_elements: b.unimportant_elements.clone(),
                })
                .collect();
        }
    }
    rules_effective_response(params.kind.as_deref(), params.session_id.as_deref(), ctx).await
}

/// rules/set 应答：写入后按（kind/sessionId）重新解析的生效规则
async fn rules_effective_response(
    kind: Option<&str>,
    session_id: Option<&str>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let set = ctx.rules.lock().await;
    let effective = set.resolve(kind, session_id, None);
    let response = ai_protocol::RulesSetResult {
        effective: mapping::effective_rules_to_spec(&effective),
    };
    Ok(Handled::Value(serde_json::to_value(&response).expect("rules/set 结果可序列化")))
}

// ---------------------------------------------------------------------------
// P11：report/generate（FR-RPT：会话快照 + 布局参数 -> 报告字符串或分块流）
// ---------------------------------------------------------------------------

/// report/generate：dir/text/merge/table/hex 会话均可出报告。
/// 应答 content 为本块内容（offset/limit 分块流；缺省全量）。
async fn report_generate(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::ReportGenerateParams = parse_params(params)?;
    if params.session_id.is_empty() {
        return Err(bad_params("sessionId 不能为空"));
    }

    // 会话快照构建（持锁只做取数，渲染在锁外）
    let snap = {
        let sessions = ctx.sessions.lock().await;
        let session = sessions.get(&params.session_id).ok_or_else(|| session_not_found(&params.session_id))?;
        match session {
            Session::Text(data) => {
                ai_core::ReportSnapshot::Text(ai_core::TextSnapshot::from_tables(
                    data.labels.0.clone(),
                    data.labels.1.clone(),
                    &data.left,
                    &data.right,
                    data.sections.clone(),
                ))
            }
            Session::Dir(data) => {
                let (left_label, right_label) = data
                    .roots
                    .as_ref()
                    .map(|(l, r)| (l.as_str().to_string(), r.as_str().to_string()))
                    .unwrap_or_default();
                let (rows, stats) = if let Some(model) = &data.model {
                    let rows = model.rows_snapshot().await.iter().map(mapping::dir_row_to_report_row).collect();
                    let stats = mapping::dir_stats_to_report_stats(&model.stats_snapshot());
                    (rows, stats)
                } else if let Some(scan) = &data.scan {
                    let rows = ai_dir::rows_from_arena(&scan.arena)
                        .iter()
                        .map(mapping::dir_row_to_report_row)
                        .collect();
                    let stats = mapping::dir_stats_to_report_stats(&scan.stats);
                    (rows, stats)
                } else {
                    (Vec::new(), ai_core::DirReportStats::default())
                };
                ai_core::ReportSnapshot::Dir(ai_core::DirSnapshot { left_label, right_label, rows, stats })
            }
            Session::Merge(data) => {
                let unresolved = data.model.unresolved_with(&data.decisions);
                ai_core::ReportSnapshot::Merge(ai_core::MergeSnapshot {
                    base_label: data.labels.0.clone(),
                    left_label: data.labels.1.clone(),
                    right_label: data.labels.2.clone(),
                    model: data.model.clone(),
                    unresolved,
                })
            }
            Session::Table(data) => ai_core::ReportSnapshot::Table(ai_core::TableSnapshot {
                left_label: data.labels.0.clone(),
                right_label: data.labels.1.clone(),
                columns: data.model.columns.clone(),
                rows: data.model.rows.clone(),
                summary: data.model.summary,
                has_header: data.options.parse.has_header,
            }),
            Session::Hex(data) => ai_core::ReportSnapshot::Hex(ai_core::HexSnapshot {
                left_label: data.labels.0.clone(),
                right_label: data.labels.1.clone(),
                left: data.left.clone(),
                right: data.right.clone(),
                model: data.model.clone(),
                status_left: data.status_left.clone(),
                status_right: data.status_right.clone(),
            }),
        }
    };

    let opts = mapping::report_options_from_dto(&params);
    let rendered = ai_core::report::render_report(&snap, &opts)
        .map_err(|e| ai_protocol::error_object(codes::INVALID_PARAMS, e.to_string()))?;

    // 分块流：字符（Unicode 标量）偏移切片，避免劈开 UTF-8
    let total = rendered.chars().count() as u64;
    let offset = params.offset.unwrap_or(0).min(total);
    let limit = params.limit;
    let content: String = {
        let chars = rendered.chars().skip(offset as usize);
        match limit {
            Some(l) => chars.take(l as usize).collect(),
            None => chars.collect(),
        }
    };
    let taken = content.chars().count() as u64;
    let has_more = offset + taken < total;
    let result = ai_protocol::ReportGenerateResult {
        layout: params.layout,
        content,
        total_chars: total,
        offset,
        has_more,
    };
    Ok(Handled::Value(serde_json::to_value(&result).expect("report/generate 结果可序列化")))
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

#[cfg(test)]
mod p7_tests {
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

    /// 三方语料：base 头尾同 + 两个冲突块（行 1、行 3）+ 右侧独有尾部
    async fn open_merge(c: &HandlerCtx, id: &str) -> (tempfile::TempDir, String, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let b = dir.path().join("base.l");
        let l = dir.path().join("left.l");
        let r = dir.path().join("right.l");
        std::fs::write(&b, "h\na\nb\nc\nt\n").unwrap();
        std::fs::write(&l, "h\nL1\nb\nL2\nt\n").unwrap();
        std::fs::write(&r, "h\nR1\nb\nR2\nt\nR3\n").unwrap();
        let (tx, _rx) = chan();
        let handled = dispatch(
            "text/merge3/open",
            Some(json!({
                "sessionId": id,
                "base": b.to_string_lossy(), "left": l.to_string_lossy(), "right": r.to_string_lossy(),
            })),
            c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let v = match handled { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["chunkCount"], 6, "Stable+冲突+Stable+冲突+Stable+RightOnly: {v}");
        assert_eq!(v["conflictCount"], 2);
        (dir, b.to_string_lossy().into_owned(), l.to_string_lossy().into_owned(), r.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn merge3_open_sections_and_capability() {
        let c = ctx();
        let (tx, _rx) = chan();
        let init = dispatch("initialize", Some(json!({"clientInfo": {"name": "t"}, "protocolVersion": "1.0"})), &c, &ai_dir::CancelFlag::new(), tx).await.unwrap();
        let v = match init { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["capabilities"]["threeWayMerge"], true, "P7 能力位");

        let (_d, _b, _l, _r) = open_merge(&c, "m1").await;
        // sections 分批：offset=1 count=2 -> 冲突块 + 稳定块
        let r = dispatch(
            "text/merge3/sections",
            Some(json!({"sessionId": "m1", "offset": 1, "count": 2})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["total"], 6);
        assert_eq!(v["hasMore"], true);
        assert_eq!(v["rev"], 0);
        let chunks = v["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0]["origin"], "conflict");
        assert_eq!(chunks[0]["conflictKind"], "modifyModify");
        assert_eq!(chunks[0]["conflictUnimportant"], false);
        assert_eq!(chunks[0]["leftLines"], json!(["L1"]));
        assert_eq!(chunks[0]["rightLines"], json!(["R1"]));
        assert_eq!(chunks[0]["unresolved"], true);
        // 行区间
        assert_eq!(chunks[0]["base"], json!({"start": 1, "end": 2}));
        // 越界钳制
        let r = dispatch(
            "text/merge3/sections",
            Some(json!({"sessionId": "m1", "offset": 99, "count": 3})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["chunks"].as_array().unwrap().len(), 0);
        // 会话不存在 -> 410
        let e = dispatch(
            "text/merge3/sections",
            Some(json!({"sessionId": "ghost", "offset": 0, "count": 1})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn merge3_decide_idempotent_and_output() {
        let c = ctx();
        let (_d, _b, _l, _r) = open_merge(&c, "m2").await;

        // 1. 未决策 output：marker 模式含两处冲突
        let r = dispatch(
            "text/merge3/output",
            Some(json!({"sessionId": "m2", "mode": "marker"})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        let marker_text = v["text"].as_str().unwrap();
        assert_eq!(v["unresolvedChunks"], json!([1, 3]));
        assert_eq!(v["hasConflicts"], true);
        assert!(marker_text.contains("<<<<<<<"), "marker 模式渲染冲突: {marker_text}");
        assert_eq!(marker_text.matches("<<<<<<<").count(), 2);

        // 2. 决策：块 1 取左、块 3 取右
        let r = dispatch(
            "text/merge3/decide",
            Some(json!({
                "sessionId": "m2",
                "decisions": [
                    {"chunkId": 1, "action": "left"},
                    {"chunkId": 3, "action": "right"}
                ]
            })),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["rev"], 1);
        assert_eq!(v["unresolvedChunks"], json!([]), "决策后无未决冲突");
        assert_eq!(v["conflictCount"], 2);

        // 3. 输出稳定（plain 模式 = 决策内容）
        let r = dispatch(
            "text/merge3/output",
            Some(json!({"sessionId": "m2"})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["text"], "h\nL1\nb\nR2\nt\nR3\n");
        assert_eq!(v["hasConflicts"], false);
        assert_eq!(v["decidedCount"], 2);

        // 4. 幂等：同一决策重复提交 -> rev 不变、输出不漂移
        let r = dispatch(
            "text/merge3/decide",
            Some(json!({
                "sessionId": "m2",
                "decisions": [
                    {"chunkId": 1, "action": "left"},
                    {"chunkId": 3, "action": "right"}
                ]
            })),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["rev"], 1, "幂等提交不递增 rev");
        let r = dispatch(
            "text/merge3/output",
            Some(json!({"sessionId": "m2"})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["text"], "h\nL1\nb\nR2\nt\nR3\n", "重复 decide 不漂移");

        // 5. manual 决策
        let r = dispatch(
            "text/merge3/decide",
            Some(json!({"sessionId": "m2", "decisions": [{"chunkId": 1, "action": "manual", "text": "手\n工"}]})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["rev"], 2);
        let r = dispatch(
            "text/merge3/output",
            Some(json!({"sessionId": "m2"})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["text"], "h\n手\n工\nb\nR2\nt\nR3\n");

        // 6. Manual 缺 text -> INVALID_PARAMS
        let e = dispatch(
            "text/merge3/decide",
            Some(json!({"sessionId": "m2", "decisions": [{"chunkId": 1, "action": "manual"}]})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::INVALID_PARAMS);
        // 7. 块不存在 -> INVALID_PARAMS
        let e = dispatch(
            "text/merge3/decide",
            Some(json!({"sessionId": "m2", "decisions": [{"chunkId": 99, "action": "left"}]})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::INVALID_PARAMS);
    }

    #[tokio::test]
    async fn merge3_importance_shrinks_conflicts() {
        // 两侧把同一行改成仅尾随空白不同：默认 -> 冲突；尾随空白不重要 + favorRight -> 自动采用
        let dir = tempfile::tempdir().unwrap();
        let b = dir.path().join("b.l");
        let l = dir.path().join("l.l");
        let r = dir.path().join("r.l");
        std::fs::write(&b, "a\nb\nc\n").unwrap();
        std::fs::write(&l, "a\nB  \nc\n").unwrap();
        std::fs::write(&r, "a\nB\nc\n").unwrap();
        let c = ctx();
        let (tx, _rx) = chan();
        // 默认选项：1 个未决冲突
        let _ = dispatch(
            "text/merge3/open",
            Some(json!({"sessionId": "mi1", "base": b.to_string_lossy(), "left": l.to_string_lossy(), "right": r.to_string_lossy()})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let got = dispatch(
            "text/merge3/output",
            Some(json!({"sessionId": "mi1"})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["unresolvedChunks"], json!([1]), "默认重要性下为未决冲突: {v}");

        // /iu + favorRight：不重要冲突自动采用右侧，不升级为未决
        let (tx, _rx) = chan();
        let _ = dispatch(
            "text/merge3/open",
            Some(json!({
                "sessionId": "mi2",
                "base": b.to_string_lossy(), "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "options": {"importance": {"trailingWsUnimportant": true}, "favor": "right", "ignoreUnimportant": true}
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let got = dispatch(
            "text/merge3/sections",
            Some(json!({"sessionId": "mi2", "offset": 0, "count": 10})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        let chunks = v["chunks"].as_array().unwrap();
        let cf = chunks.iter().find(|x| x["origin"] == "conflict").expect("冲突块仍在（来源标记）");
        assert_eq!(cf["conflictUnimportant"], true);
        assert_eq!(cf["autoResolved"], true);
        assert_eq!(cf["unresolved"], false);
        assert_eq!(cf["decision"], "right", "favorRight 自动采用");
        let got = dispatch(
            "text/merge3/output",
            Some(json!({"sessionId": "mi2"})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["text"], "a\nB\nc\n", "不重要冲突从受惠侧采用");
        assert_eq!(v["hasConflicts"], false);
    }

    #[tokio::test]
    async fn merge3_conflict_window_option() {
        // 左改行 0、右改行 2，中间 1 条稳定行：默认无冲突；窗口 1 -> 合并为冲突
        let dir = tempfile::tempdir().unwrap();
        let b = dir.path().join("b.l");
        let l = dir.path().join("l.l");
        let r = dir.path().join("r.l");
        std::fs::write(&b, "a\nmid\nc\n").unwrap();
        std::fs::write(&l, "A\nmid\nc\n").unwrap();
        std::fs::write(&r, "a\nmid\nC\n").unwrap();
        let c = ctx();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "text/merge3/open",
            Some(json!({
                "sessionId": "mw1",
                "base": b.to_string_lossy(), "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "options": {"conflictWindow": 1}
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let got = dispatch(
            "text/merge3/sections",
            Some(json!({"sessionId": "mw1", "offset": 0, "count": 10})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        let chunks = v["chunks"].as_array().unwrap();
        let conflicts: Vec<_> = chunks.iter().filter(|x| x["origin"] == "conflict").collect();
        assert_eq!(conflicts.len(), 1, "窗口合并: {v}");
        assert_eq!(conflicts[0]["base"], json!({"start": 0, "end": 3}), "冲突块跨稳定间隔");
        assert_eq!(v["total"], 1, "整个文件合并为单一冲突块（无尾部稳定块）");
    }

    #[tokio::test]
    async fn merge3_conflict_default_policy_and_diff3_style() {
        let c = ctx();
        let (_d, _b, _l, _r) = open_merge(&c, "m3").await;
        // 会话级 conflictDefault=base：plain 模式未决冲突取基
        // （open_merge 的会话未带 options，此处直接验证 marker diff3 风格）
        let r = dispatch(
            "text/merge3/output",
            Some(json!({"sessionId": "m3", "mode": "marker", "markerStyle": "diff3"})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        let text = v["text"].as_str().unwrap();
        assert!(text.contains("<<<<<<<"), "{text}");
        assert!(text.contains("|||||||"), "diff3 风格含 base 段");
        assert!(text.contains("=======") && text.contains(">>>>>>>"));
        // Plain 模式（缺省 conflictDefault=TakeLeft）：未决冲突取左，文本连续
        let r = dispatch(
            "text/merge3/output",
            Some(json!({"sessionId": "m3"})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match r { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["text"], "h\nL1\nb\nL2\nt\nR3\n");
        assert_eq!(v["hasConflicts"], true, "plain 模式未决冲突仍列入清单");
    }
}

#[cfg(test)]
mod p9_tests {
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

    /// 表格语料：键列 + 两处单元格差异 + 右侧新增行
    async fn open_table(c: &HandlerCtx, id: &str) {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("l.csv");
        let r = dir.path().join("r.csv");
        std::fs::write(&l, "id,name,qty\n1,Alpha,10\n2,Beta,20\n3,Gamma,30\n").unwrap();
        std::fs::write(&r, "id,name,qty\n1,Alpha,10\n2,BETA,20\n3,Gamma,35\n4,Delta,40\n").unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "table/open",
            Some(json!({
                "sessionId": id, "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "options": {"keyColumns": [0]}
            })),
            c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn table_open_sections_and_capability() {
        let c = ctx();
        let (tx, _rx) = chan();
        let init = dispatch("initialize", Some(json!({"clientInfo": {"name": "t"}, "protocolVersion": "1.0"})), &c, &ai_dir::CancelFlag::new(), tx).await.unwrap();
        let v = match init { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["capabilities"]["tableCompare"], true, "P9 能力位 tableCompare");
        assert_eq!(v["capabilities"]["hexCompare"], true);
        assert_eq!(v["capabilities"]["patchApply"], true);
        assert_eq!(v["capabilities"]["dirExecute"], true);

        open_table(&c, "tb1").await;
        let got = dispatch(
            "table/sections",
            Some(json!({"sessionId": "tb1", "offset": 0, "count": 10})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["total"], 4, "same + changed×2 + added: {v}");
        let rows = v["rows"].as_array().unwrap();
        assert_eq!(rows[0]["kind"], "same");
        assert_eq!(rows[1]["kind"], "changed");
        assert_eq!(rows[1]["important"], true, "BETA 与 Beta 大小写默认重要");
        assert_eq!(rows[1]["cells"][1]["status"], "importantDiff");
        assert_eq!(rows[3]["kind"], "added");
        assert_eq!(rows[3]["left"], serde_json::Value::Null, "Added 行省略 left");
        // 会话不存在 -> 410
        let e = dispatch(
            "table/sections",
            Some(json!({"sessionId": "ghost", "offset": 0, "count": 1})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn table_importance_option_marks_unimportant() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("l.csv");
        let r = dir.path().join("r.csv");
        std::fs::write(&l, "id,note\n1,Abc\n").unwrap();
        std::fs::write(&r, "id,note\n1,abc \n").unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "table/open",
            Some(json!({
                "sessionId": "tb2", "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "options": {"keyColumns": [0], "caseUnimportant": true, "whitespace": "ignoreAll"}
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let got = dispatch(
            "table/sections",
            Some(json!({"sessionId": "tb2", "offset": 0, "count": 10})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        let rows = v["rows"].as_array().unwrap();
        assert_eq!(rows[0]["kind"], "changed");
        assert_eq!(rows[0]["important"], false, "仅不重要差异 -> 蓝点行: {v}");
        assert_eq!(rows[0]["cells"][1]["status"], "unimportantDiff");
    }

    /// hex 语料：前 8 字节同、后 4 字节变化、右侧多 2 字节
    async fn open_hex(c: &HandlerCtx, id: &str) -> (tempfile::TempDir, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("l.bin");
        let r = dir.path().join("r.bin");
        std::fs::write(&l, [0x00, 0x01, 0x02, 0x03, 0xAA, 0xBB, 0xCC, 0xDD]).unwrap();
        std::fs::write(&r, [0x00, 0x01, 0x02, 0x03, 0xAA, 0xBB, 0xEE, 0xFF, 0x11, 0x22]).unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "hex/open",
            Some(json!({"sessionId": id, "left": l.to_string_lossy(), "right": r.to_string_lossy()})),
            c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        (dir, l.to_string_lossy().into_owned(), r.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn hex_open_and_bytes_paging() {
        let c = ctx();
        let (_d, _l, _r) = open_hex(&c, "h1").await;
        let got = dispatch(
            "hex/bytes",
            Some(json!({"sessionId": "h1", "side": "left", "offset": 0, "length": 16})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        let lines = v["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1, "8 字节 -> 1 行: {v}");
        assert_eq!(lines[0]["addr"], 0);
        assert_eq!(lines[0]["hex"], "00 01 02 03 aa bb cc dd");
        assert_eq!(lines[0]["ascii"], "........", "全部字节均非可打印 ASCII");
        // Full 模式字节级 Myers 最优对齐：CC↔EE 变化（x），DD 删除（-），
        // FF/11/22 插入（+）——D=4 < 逐字节替换 D=6
        assert_eq!(lines[0]["status"], "======x-", "左 8 字节：6 同 + 1 变化 + 1 删除");
        // 右侧：6 同 + 3 插入 + 1 变化（Myers 最优对齐的另一等价解）
        let got = dispatch(
            "hex/bytes",
            Some(json!({"sessionId": "h1", "side": "right", "offset": 0, "length": 16})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        let lines = v["lines"].as_array().unwrap();
        assert_eq!(lines[0]["status"], "======+++x", "{v}");
        assert_eq!(lines[0]["hex"], "00 01 02 03 aa bb ee ff 11 22");
        // done 标记
        assert_eq!(v["done"], true);
        // 越界 offset 钳制
        let got = dispatch(
            "hex/bytes",
            Some(json!({"sessionId": "h1", "side": "left", "offset": 999, "length": 16})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["lines"].as_array().unwrap().len(), 1, "钳制到最后行");
        // 会话不存在 -> 410
        let e = dispatch(
            "hex/bytes",
            Some(json!({"sessionId": "ghost", "side": "left", "offset": 0, "length": 16})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap_err();
        assert_eq!(e.code, codes::SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn patch_apply_rpc_roundtrip() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("t.txt"), "1\n2\n3\n").unwrap();
        let patch = "--- a/t.txt\n+++ b/t.txt\n@@ -1,3 +1,3 @@\n 1\n-2\n+TWO\n 3\n";
        let (tx, _rx) = chan();
        // dry run：不落盘
        let got = dispatch(
            "text/patch/apply",
            Some(json!({"patch": patch, "root": root.to_string_lossy(), "dryRun": true})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["dryRun"], true);
        assert_eq!(v["applied"], 1);
        assert_eq!(v["allApplied"], true);
        assert_eq!(std::fs::read_to_string(root.join("t.txt")).unwrap(), "1\n2\n3\n", "dry run 未写盘");
        // 实际应用
        let (tx, _rx) = chan();
        let got = dispatch(
            "text/patch/apply",
            Some(json!({"patch": patch, "root": root.to_string_lossy()})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["applied"], 1);
        assert_eq!(std::fs::read_to_string(root.join("t.txt")).unwrap(), "1\nTWO\n3\n");
        // 重放：已应用 -> failed（git apply 同语义）
        let (tx, _rx) = chan();
        let got = dispatch(
            "text/patch/apply",
            Some(json!({"patch": patch, "root": root.to_string_lossy()})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["failed"], 1);
        assert_eq!(v["results"][0]["status"], "alreadyApplied");
    }

    #[tokio::test]
    async fn patch_apply_binary_skip_reported() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("img.png"), b"\x89PNG\r\n").unwrap();
        let patch = "diff --git a/img.png b/img.png\nindex aaa..bbb 100644\nBinary files a/img.png and b/img.png differ\n";
        let (tx, _rx) = chan();
        let got = dispatch(
            "text/patch/apply",
            Some(json!({"patch": patch, "root": root.to_string_lossy()})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["results"][0]["status"], "skippedBinary");
        assert_eq!(v["failed"], 0, "二进制跳过不算失败");
    }

    /// 目录树：dryRun + 执行 + rescan delta（P6 提案端到端）
    async fn make_dir_session(c: &HandlerCtx, id: &str) -> (tempfile::TempDir, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("same.txt"), b"1").unwrap();
        std::fs::write(r.join("same.txt"), b"1").unwrap();
        std::fs::write(l.join("lonly.txt"), b"L").unwrap();
        std::fs::write(r.join("ronly.txt"), b"R").unwrap();
        std::fs::write(l.join("stale.txt"), b"old").unwrap();
        std::fs::write(r.join("stale.txt"), b"old").unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "dir/open",
            Some(json!({
                "sessionId": id, "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "depth": 1,
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"},
                "lazyScan": true
            })),
            c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        (dir, l.to_string_lossy().into_owned(), r.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn dir_execute_dry_run_then_execute_with_rescan_delta() {
        let c = ctx();
        let (_d, _left, right) = make_dir_session(&c, "d9").await;

        // 1. dryRun：复制 lonly.txt + 删除 ronly.txt + 删除 stale.txt —— 只校验不落盘
        let (tx, mut rx) = chan();
        let got = dispatch(
            "dir/execute",
            Some(json!({
                "sessionId": "d9",
                "ops": [
                    {"kind": "copy", "path": "lonly.txt", "side": "left"},
                    {"kind": "delete", "path": "ronly.txt", "side": "right"},
                    {"kind": "delete", "path": "stale.txt", "side": "both"}
                ],
                "dryRun": true
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await;
        // side:"both" 非法（DiffSide 只有 left/right）-> 参数错误
        assert!(got.is_err(), "非法 side 应拒绝");
        let _ = rx.try_recv();

        // 2. dryRun：合法操作
        let (tx, _rx) = chan();
        let got = dispatch(
            "dir/execute",
            Some(json!({
                "sessionId": "d9",
                "ops": [
                    {"kind": "copy", "path": "lonly.txt", "side": "left"},
                    {"kind": "delete", "path": "ronly.txt", "side": "right"},
                    {"kind": "touch", "path": "stale.txt", "side": "left", "mtimeMs": 0}
                ],
                "dryRun": true
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["dryRun"], true);
        assert_eq!(v["total"], 3);
        assert_eq!(v["succeeded"].as_array().unwrap().len(), 3);
        assert_eq!(v["failed"].as_array().unwrap().len(), 0);
        // 文件系统未变
        assert!(!std::path::Path::new(&right).join("lonly.txt").exists(), "dry run 不落盘");
        assert!(std::path::Path::new(&right).join("ronly.txt").exists());

        // 3. 实际执行：应答 + dir/delta 通知（rescan 推送）
        let (tx, mut rx) = chan();
        let got = dispatch(
            "dir/execute",
            Some(json!({
                "sessionId": "d9",
                "ops": [
                    {"kind": "copy", "path": "lonly.txt", "side": "left"},
                    {"kind": "delete", "path": "ronly.txt", "side": "right"}
                ],
                "dryRun": false
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let (response, notifications) = match got {
            Handled::WithNotifications { response, notifications } => (response, notifications),
            _ => panic!("执行后应有 rescan delta 通知"),
        };
        assert_eq!(response["succeeded"].as_array().unwrap().len(), 2);
        assert_eq!(response["dryRun"], false);
        // 落盘验证
        assert!(std::path::Path::new(&right).join("lonly.txt").exists(), "复制已落盘");
        assert!(!std::path::Path::new(&right).join("ronly.txt").exists(), "删除已落盘");
        // delta 通知：rescan 全量重建（removals + upserts）
        assert!(!notifications.is_empty(), "应推送 dir/delta");
        let mut saw_removals = false;
        let mut saw_upsert_lonly = false;
        for n in &notifications {
            let payload = &n.params;
            if !payload["removals"].as_array().unwrap_or(&Vec::new()).is_empty() {
                saw_removals = true;
            }
            for row in payload["upserts"].as_array().unwrap_or(&Vec::new()) {
                if row["id"] == "lonly.txt" && row["status"] == "same" {
                    saw_upsert_lonly = true;
                }
            }
        }
        assert!(saw_removals, "rescan delta 含旧行 removals");
        assert!(saw_upsert_lonly, "rescan delta 含复制后的 same 行: {notifications:?}");
        // 通知已按序入队（本 handler 走 WithNotifications，server 会在应答后发送）
        while let Ok(_msg) = rx.try_recv() {}

        // 4. rename + mkdir + 部分失败：ghost 文件失败不中断其余操作
        let (tx, _rx) = chan();
        let got = dispatch(
            "dir/execute",
            Some(json!({
                "sessionId": "d9",
                "ops": [
                    {"kind": "mkdir", "path": "newdir", "side": "right"},
                    {"kind": "copy", "path": "ghost.txt", "side": "left"},
                    {"kind": "rename", "path": "lonly.txt", "side": "right", "newName": "renamed.txt"}
                ],
                "dryRun": false
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let (v, _notes) = match got {
            Handled::Value(v) => (v, Vec::new()),
            Handled::WithNotifications { response, notifications } => (response, notifications),
        };
        assert_eq!(v["failed"].as_array().unwrap().len(), 1, "ghost 失败: {v}");
        assert_eq!(v["succeeded"].as_array().unwrap().len(), 2, "部分成功语义");
        assert!(std::path::Path::new(&right).join("newdir").exists());
        assert!(std::path::Path::new(&right).join("renamed.txt").exists());

        // 5. rename 缺 newName -> 参数错误
        let (tx, _rx) = chan();
        let err = dispatch(
            "dir/execute",
            Some(json!({
                "sessionId": "d9",
                "ops": [{"kind": "rename", "path": "lonly.txt", "side": "right"}],
                "dryRun": false
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::INVALID_PARAMS);

        // 6. 非目录会话 -> 参数错误
        let (tx, _rx) = chan();
        let err = dispatch(
            "dir/execute",
            Some(json!({"sessionId": "no-such-session", "ops": [], "dryRun": true})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn dir_execute_sync_session_rescans_via_scan() {
        // P0 同步路径会话（无模型）：执行后走全量重扫推 delta
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("a.txt"), b"1").unwrap();
        std::fs::write(r.join("a.txt"), b"1").unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "dir/open",
            Some(json!({
                "sessionId": "d10", "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "depth": 1,
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"}
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        std::fs::write(l.join("b.txt"), b"2").unwrap();
        let (tx, _rx) = chan();
        let got = dispatch(
            "dir/execute",
            Some(json!({
                "sessionId": "d10",
                "ops": [{"kind": "mkdir", "path": "sub", "side": "left"}],
                "dryRun": false
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let (response, notifications) = match got {
            Handled::WithNotifications { response, notifications } => (response, notifications),
            _ => panic!("同步会话执行后应推 rescan delta"),
        };
        assert_eq!(response["succeeded"].as_array().unwrap().len(), 1);
        let mut saw_b = false;
        let mut saw_sub = false;
        for n in &notifications {
            for row in n.params["upserts"].as_array().unwrap_or(&Vec::new()) {
                if row["id"] == "b.txt" {
                    saw_b = true;
                }
                if row["id"] == "sub" {
                    saw_sub = true;
                }
            }
        }
        assert!(saw_b && saw_sub, "重扫 delta 应含新文件与新目录");
    }
}

#[cfg(test)]
mod p10_tests {
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

    async fn value_of(r: Result<Handled, RpcErrorObject>) -> serde_json::Value {
        match r.expect("应答") {
            Handled::Value(v) => v,
            _ => panic!("形态"),
        }
    }

    // ---- formats/list|save|load ----

    #[tokio::test]
    async fn formats_list_returns_builtin_table() {
        let c = ctx();
        let v = value_of(formats_list(None, &c).await).await;
        let formats = v["formats"].as_array().unwrap();
        assert!(formats.len() >= 12, "内置 ≥12 语言，实际 {}", formats.len());
        assert_eq!(v["builtinCount"], formats.len() as u64);
        let ids: Vec<&str> = formats.iter().map(|f| f["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"builtin-cpp") && ids.contains(&"builtin-python"));
        // 掩码与语法元素随定义返回
        let cpp = formats.iter().find(|f| f["id"] == "builtin-cpp").unwrap();
        let masks: Vec<&str> = cpp["masks"].as_array().unwrap().iter().map(|m| m.as_str().unwrap()).collect();
        assert!(masks.contains(&"*.cpp"));
        let grammar = cpp["grammar"].as_array().unwrap();
        assert!(grammar.iter().any(|e| e["name"] == "注释"), "C 格式带注释元素");
    }

    #[tokio::test]
    async fn formats_save_upserts_and_list_reflects() {
        let c = ctx();
        let params = json!({"formats":[{"id":"fmt-team","enabled":true,"masks":["*.team"],"description":"团队格式","viewKind":"text","grammar":[{"name":"注释","items":[{"kind":"delimited","start":"#","toEol":true}]}]}]});
        let v = value_of(formats_save(Some(params), &c).await).await;
        assert_eq!(v["saved"], 1);
        let ids: Vec<&str> = v["formats"].as_array().unwrap().iter().map(|f| f["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"fmt-team"), "保存后列表含自定义格式");
        // 再存同 id = 覆盖（saved 仍计 1，列表不重复）
        let params2 = json!({"formats":[{"id":"fmt-team","enabled":true,"masks":["*.team2"],"description":"v2","viewKind":"text"}]});
        let v2 = value_of(formats_save(Some(params2), &c).await).await;
        assert_eq!(v2["saved"], 1);
        let count = v2["formats"].as_array().unwrap().iter().filter(|f| f["id"] == "fmt-team").count();
        assert_eq!(count, 1, "按 id 覆盖不重复");
    }

    #[tokio::test]
    async fn formats_load_bcpack_and_version_guard() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let pack_path = dir.path().join("team.bcpack");
        std::fs::write(
            &pack_path,
            r#"{"version":1,"kind":"ai-compare-format-pack","formats":[{"id":"fmt-loaded","enabled":true,"masks":["*.ldd"],"description":"载入","viewKind":"text"}]}"#,
        )
        .unwrap();
        let params = json!({"path": pack_path.to_string_lossy()});
        let v = value_of(formats_load(Some(params), &c).await).await;
        assert_eq!(v["loaded"], 1);
        let ids: Vec<&str> = v["formats"].as_array().unwrap().iter().map(|f| f["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"fmt-loaded"));

        // 版本 > 1 拒绝
        let bad = dir.path().join("future.bcpack");
        std::fs::write(&bad, r#"{"version":2,"formats":[]}"#).unwrap();
        let err = formats_load(Some(json!({"path": bad.to_string_lossy()})), &c).await;
        assert!(err.is_err(), "未来版本拒绝");
    }

    // ---- rules/get|set（三层作用域） ----

    #[tokio::test]
    async fn rules_set_get_layer_scopes() {
        let c = ctx();
        // 全局层
        let g = json!({"scope":"global","layer":{"compare":{"caseUnimportant":true},"unimportantElements":["注释"]}});
        let v = value_of(rules_set(Some(g), &c).await).await;
        assert_eq!(v["effective"]["compare"]["caseUnimportant"], true);
        assert_eq!(v["effective"]["unimportantElements"], json!(["注释"]));
        // kind 层覆盖 compare（整体覆盖语义）
        let k = json!({"scope":"kind","kind":"text-compare","layer":{"compare":{"caseUnimportant":false}}});
        let v = value_of(rules_set(Some(k), &c).await).await;
        assert_eq!(v["effective"]["compare"]["caseUnimportant"], false, "kind 层覆盖全局");
        assert_eq!(v["effective"]["unimportantElements"], json!(["注释"]), "元素集 None 回落全局");
        // session 层再覆盖元素集
        let s = json!({"scope":"session","sessionId":"sess-1","layer":{"unimportantElements":["字符串"]}});
        let _ = value_of(rules_set(Some(s), &c).await).await;
        let get = json!({"kind":"text-compare","sessionId":"sess-1"});
        let v = value_of(rules_get(Some(get), &c).await).await;
        assert_eq!(v["session"]["unimportantElements"], json!(["字符串"]), "session 层回显");
        assert_eq!(v["effective"]["unimportantElements"], json!(["字符串"]), "会话层覆盖");
        assert_eq!(v["effective"]["compare"]["caseUnimportant"], false, "compare 来自 kind 层");
        // 不带 sessionId 的解析不含会话层
        let v2 = value_of(rules_get(Some(json!({"kind":"text-compare"})), &c).await).await;
        assert_eq!(v2["effective"]["unimportantElements"], json!(["注释"]));
    }

    #[tokio::test]
    async fn rules_set_target_validation() {
        let c = ctx();
        assert!(rules_set(Some(json!({"scope":"kind","layer":{}})), &c).await.is_err(), "kind 缺 kind");
        assert!(rules_set(Some(json!({"scope":"session","layer":{}})), &c).await.is_err(), "session 缺 sessionId");
        assert!(rules_set(Some(json!({"scope":"global","layer":{}})), &c).await.is_ok());
    }

    #[tokio::test]
    async fn rules_set_format_bindings_replace() {
        let c = ctx();
        let b = json!({"scope":"global","layer":{},"formatBindings":[{"formatId":"builtin-cpp","unimportantElements":["注释"]}],"clearFormatBindings":false});
        let _ = value_of(rules_set(Some(b), &c).await).await;
        let v = value_of(rules_get(Some(json!({})), &c).await).await;
        assert_eq!(v["formatBindings"][0]["formatId"], "builtin-cpp");
        // 全量替换
        let b2 = json!({"scope":"global","layer":{},"formatBindings":[{"formatId":"builtin-json","unimportantElements":["注释"]}],"clearFormatBindings":false});
        let _ = value_of(rules_set(Some(b2), &c).await).await;
        let v = value_of(rules_get(Some(json!({})), &c).await).await;
        let ids: Vec<&str> = v["formatBindings"].as_array().unwrap().iter().map(|x| x["formatId"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["builtin-json"], "绑定全量替换");
        // clear
        let b3 = json!({"scope":"global","layer":{},"clearFormatBindings":true});
        let _ = value_of(rules_set(Some(b3), &c).await).await;
        let v = value_of(rules_get(Some(json!({})), &c).await).await;
        assert!(v.get("formatBindings").is_none(), "清空后绑定省略（skip empty）");
    }

    // ---- text/open 会话规则应用链 ----

    /// 语料：仅注释行不同的 C 文件对（两侧不同文件名，避免同路径覆盖）
    async fn open_c_pair_with_rules(c: &HandlerCtx, rules: serde_json::Value) -> serde_json::Value {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("main.c");
        let r = dir.path().join("main2.c");
        std::fs::write(&l, b"int a;\n// old note\nint b;\n").unwrap();
        std::fs::write(&r, b"int a;\n// new note\nint b;\n").unwrap();
        let params = json!({
            "sessionId": "t-rules",
            "left": l.to_string_lossy(),
            "right": r.to_string_lossy(),
            "rules": rules,
        });
        let (_tx, _rx) = chan();
        let r = text_open(Some(params), c).await;
        drop(dir);
        value_of(r).await
    }

    #[tokio::test]
    async fn text_open_rules_comment_lines_unimportant() {
        let c = ctx();
        // 注释元素规则（引擎按左侧 .c 文件名检测 builtin-cpp 语法）
        let v = open_c_pair_with_rules(&c, json!({"unimportantElements":["注释"]})).await;
        assert_eq!(v["sectionCount"], 3, "骨架：Equal + Change + Equal");
        let (tx, _rx) = chan();
        let v2 = value_of(text_sections(Some(json!({"sessionId":"t-rules","offset":0,"count":100})), &c, tx).await).await;
        let secs = v2["sections"].as_array().unwrap();
        assert_eq!(secs[1]["important"], "unimportant", "注释行差异不重要（语法折叠）");
        // raw 骨架不受影响：中间段仍为 change
        assert_eq!(secs[1]["kind"], "change");
    }

    #[tokio::test]
    async fn text_open_rules_case_flag() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("v1.txt");
        let r = dir.path().join("v2.txt");
        std::fs::write(&l, b"Value = 1\n").unwrap();
        std::fs::write(&r, b"value = 1\n").unwrap();
        let params = json!({
            "sessionId": "t-case",
            "left": l.to_string_lossy(),
            "right": r.to_string_lossy(),
            "rules": {"caseUnimportant": true},
        });
        value_of(text_open(Some(params), &c).await).await;
        let (tx, _rx) = chan();
        let v = value_of(text_sections(Some(json!({"sessionId":"t-case","offset":0,"count":100})), &c, tx).await).await;
        assert_eq!(v["sections"][0]["important"], "unimportant", "大小写不重要经 rules 链生效");
    }

    #[tokio::test]
    async fn text_open_without_rules_stays_important() {
        let c = ctx();
        let v = open_c_pair_with_rules(&c, json!({})).await;
        assert_eq!(v["sectionCount"], 3);
        let (tx, _rx) = chan();
        let v2 = value_of(text_sections(Some(json!({"sessionId":"t-rules","offset":0,"count":100})), &c, tx).await).await;
        assert_eq!(v2["sections"][1]["important"], "important", "无规则 = 注释差异重要（行为不变）");
    }

    // ---- dir/open formatAware ----

    /// 语料：同名文本对 + 同名二进制对（内容相同 → Same，仍可注解 fileKind）
    async fn make_kind_trees() -> (tempfile::TempDir, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("code.txt"), b"hello\n").unwrap();
        std::fs::write(r.join("code.txt"), b"hello\n").unwrap();
        std::fs::write(l.join("blob.bin"), b"abc\0def").unwrap();
        std::fs::write(r.join("blob.bin"), b"abc\0def").unwrap();
        (dir, l.to_string_lossy().to_string(), r.to_string_lossy().to_string())
    }

    async fn dir_open_collect(id: &str, l: &str, r: &str, format_aware: bool) -> (serde_json::Value, Vec<serde_json::Value>) {
        let c = ctx();
        let (tx, mut rx) = chan();
        let mut params = json!({"sessionId": id, "left": l, "right": r});
        if format_aware {
            params["formatAware"] = json!(true);
        }
        let handled = dir_open(Some(params), &c, &ai_dir::CancelFlag::new(), tx).await.unwrap();
        let (response, notifications) = match handled {
            Handled::Value(v) => (v, Vec::new()),
            Handled::WithNotifications { response, notifications } => (response, notifications),
        };
        let mut deltas: Vec<serde_json::Value> = notifications
            .iter()
            .filter(|n| n.method == ai_protocol::method::DIR_DELTA)
            .map(|n| n.params.clone())
            .collect();
        // 同路径下 pump（若有）也经 tx 推送
        while let Ok(msg) = rx.try_recv() {
            if let Outbound::Notification(n) = msg {
                if n.method == ai_protocol::method::DIR_DELTA {
                    deltas.push(n.params);
                }
            }
        }
        (response, deltas)
    }

    #[tokio::test]
    async fn dir_open_format_aware_annotates_file_kind() {
        let (_dir, l, r) = make_kind_trees().await;
        let (resp, deltas) = dir_open_collect("d-anno", &l, &r, true).await;
        assert_eq!(resp["rowCount"], 2);
        assert_eq!(deltas.len(), 1);
        let upserts = deltas[0]["upserts"].as_array().unwrap();
        let kinds: std::collections::HashMap<String, &str> = upserts
            .iter()
            .map(|u| (u["id"].as_str().unwrap().to_string(), u["fileKind"].as_str().unwrap()))
            .collect();
        assert_eq!(kinds["code.txt"], "text");
        assert_eq!(kinds["blob.bin"], "binary", "NUL 字节判二进制");
    }

    #[tokio::test]
    async fn dir_open_without_format_aware_keeps_payload_shape() {
        let (_dir, l, r) = make_kind_trees().await;
        let (resp, deltas) = dir_open_collect("d-plain", &l, &r, false).await;
        assert_eq!(resp["rowCount"], 2);
        for d in &deltas {
            for u in d["upserts"].as_array().unwrap() {
                assert!(u.get("fileKind").is_none(), "未开启 formatAware 时载荷无 fileKind（P9 字节兼容）");
            }
        }
    }

    #[tokio::test]
    async fn dir_open_format_aware_model_path_annotates() {
        let (_dir, l, r) = make_kind_trees().await;
        let c = ctx();
        let (tx, mut rx) = chan();
        let params = json!({
            "sessionId": "d-model",
            "left": l,
            "right": r,
            "lazyScan": true,
            "formatAware": true,
        });
        let handled = dir_open(Some(params), &c, &ai_dir::CancelFlag::new(), tx).await.unwrap();
        assert!(matches!(handled, Handled::WithNotifications { .. }));
        let mut kinds = std::collections::HashMap::new();
        // 首批 delta 随应答通知返回（含注解）；后台 pump 增量经 tx
        if let Handled::WithNotifications { notifications, .. } = &handled {
            for n in notifications {
                if n.method == ai_protocol::method::DIR_DELTA {
                    for u in n.params["upserts"].as_array().unwrap() {
                        if let Some(k) = u.get("fileKind").and_then(|k| k.as_str()) {
                            kinds.insert(u["id"].as_str().unwrap().to_string(), k.to_string());
                        }
                    }
                }
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while kinds.len() < 2 && std::time::Instant::now() < deadline {
            let msg = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
            match msg {
                Ok(Some(Outbound::Notification(n))) => {
                    if n.method == ai_protocol::method::DIR_DELTA {
                        for u in n.params["upserts"].as_array().unwrap() {
                            if let Some(k) = u.get("fileKind").and_then(|k| k.as_str()) {
                                kinds.insert(u["id"].as_str().unwrap().to_string(), k.to_string());
                            }
                        }
                    }
                }
                Ok(Some(_)) => {}
                _ => {}
            }
        }
        assert_eq!(kinds.get("code.txt").map(String::as_str), Some("text"));
        assert_eq!(kinds.get("blob.bin").map(String::as_str), Some("binary"));
    }

    #[tokio::test]
    async fn initialize_reports_format_rules_capability() {
        let v = value_of(initialize()).await;
        assert_eq!(v["capabilities"]["formatRules"], true);
    }
}

#[cfg(test)]
mod p11_report_tests {
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

    async fn value_of(r: Result<Handled, RpcErrorObject>) -> serde_json::Value {
        match r.expect("应答") {
            Handled::Value(v) => v,
            _ => panic!("形态"),
        }
    }

    /// 本模块本地目录会话（P9 测试模块的 make_dir_session 跨模块不可见）
    async fn make_dir_session(c: &HandlerCtx, id: &str) -> (tempfile::TempDir, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("same.txt"), b"1").unwrap();
        std::fs::write(r.join("same.txt"), b"1").unwrap();
        std::fs::write(l.join("lonly.txt"), b"L").unwrap();
        std::fs::write(r.join("ronly.txt"), b"R").unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "dir/open",
            Some(json!({
                "sessionId": id, "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "depth": 1,
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"},
                "lazyScan": true
            })),
            c, &ai_dir::CancelFlag::new(), tx,
        )
        .await;
        (dir, l.to_string_lossy().into_owned(), r.to_string_lossy().into_owned())
    }

    async fn open_text_session(c: &HandlerCtx, id: &str, l: &std::path::Path, r: &std::path::Path) {
        let (tx, _rx) = chan();
        let _ = dispatch(
            "text/open",
            Some(json!({"sessionId": id, "left": l.to_string_lossy(), "right": r.to_string_lossy()})),
            c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .expect("text/open");
    }

    #[tokio::test]
    async fn report_generate_text_patch_roundtrip_ready() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("a.txt");
        let r = dir.path().join("b.txt");
        std::fs::write(&l, "alpha\nbeta\ngamma\n").unwrap();
        std::fs::write(&r, "alpha\nBETA\ngamma\ndelta\n").unwrap();
        open_text_session(&c, "t1", &l, &r).await;
        let v = value_of(
            dispatch(
                "report/generate",
                Some(json!({"sessionId": "t1", "layout": "patch", "contextLines": 1})),
                &c, &ai_dir::CancelFlag::new(), chan().0,
            )
            .await,
        )
        .await;
        let content = v["content"].as_str().unwrap();
        assert!(content.starts_with("--- ") && content.contains("+++ "), "{content}");
        assert!(content.contains("-beta\n+BETA\n gamma\n+delta\n"), "{content}");
        assert_eq!(v["layout"], "patch");
        assert_eq!(v["hasMore"], false);
        // 全文总长与 content 一致（未分块）
        assert_eq!(v["totalChars"].as_u64().unwrap(), content.chars().count() as u64);
    }

    #[tokio::test]
    async fn report_generate_text_chunked_stream() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("a.txt");
        let r = dir.path().join("b.txt");
        std::fs::write(&l, "1\n2\n3\n").unwrap();
        std::fs::write(&r, "1\nX\n3\n").unwrap();
        open_text_session(&c, "t2", &l, &r).await;
        let fetch = |offset: u64, limit: u64| {
            let c = &c;
            async move {
                value_of(
                    dispatch(
                        "report/generate",
                        Some(json!({"sessionId": "t2", "layout": "xml", "offset": offset, "limit": limit})),
                        c, &ai_dir::CancelFlag::new(), chan().0,
                    )
                    .await,
                )
                .await
            }
        };
        let first = fetch(0, 10).await;
        assert_eq!(first["offset"], 0);
        assert_eq!(first["hasMore"], true);
        assert_eq!(first["content"].as_str().unwrap().chars().count(), 10);
        let total = first["totalChars"].as_u64().unwrap();
        // 续块拼回全文
        let mut assembled = first["content"].as_str().unwrap().to_string();
        let mut off = 10u64;
        while off < total {
            let blk = fetch(off, 7).await;
            assembled.push_str(blk["content"].as_str().unwrap());
            off = blk["offset"].as_u64().unwrap() + blk["content"].as_str().unwrap().chars().count() as u64;
            if !blk["hasMore"].as_bool().unwrap() {
                break;
            }
        }
        let full = fetch(0, total).await;
        assert_eq!(assembled, full["content"].as_str().unwrap(), "分块拼接 == 全量");
        assert!(assembled.contains("<report kind=\"text\""));
    }

    #[tokio::test]
    async fn report_generate_dir_summary_and_tree() {
        let c = ctx();
        let (dir, l, r) = make_dir_session(&c, "d11").await;
        let _ = dir;
        // 等待后台扫描完成（lazyScan 模型）
        {
            let sessions = c.sessions.lock().await;
            if let Some(Session::Dir(data)) = sessions.get("d11") {
                if let Some(model) = &data.model {
                    let _ = model.wait_until_done(std::time::Duration::from_secs(5)).await;
                }
            }
        }
        let v = value_of(
            dispatch(
                "report/generate",
                Some(json!({"sessionId": "d11", "layout": "summary"})),
                &c, &ai_dir::CancelFlag::new(), chan().0,
            )
            .await,
        )
        .await;
        let summary = v["content"].as_str().unwrap();
        assert!(summary.contains("差异清单"), "{summary}");
        assert!(summary.contains("lonly.txt") && summary.contains("ronly.txt"), "{summary}");
        // tree 布局 + 孤项过滤
        let v = value_of(
            dispatch(
                "report/generate",
                Some(json!({"sessionId": "d11", "layout": "tree", "dirDisplay": "orphans"})),
                &c, &ai_dir::CancelFlag::new(), chan().0,
            )
            .await,
        )
        .await;
        let tree = v["content"].as_str().unwrap();
        assert!(tree.contains("[<] lonly.txt"), "{tree}");
        assert!(tree.contains("[>] ronly.txt"));
        assert!(!tree.contains("same.txt"), "orphans 过滤后不含相同项: {tree}");
        let _ = (l, r);
    }

    #[tokio::test]
    async fn report_generate_table_hex_merge() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let lcsv = dir.path().join("l.csv");
        let rcsv = dir.path().join("r.csv");
        std::fs::write(&lcsv, "id,v\n1,a\n2,b\n").unwrap();
        std::fs::write(&rcsv, "id,v\n1,a\n2,B\n3,c\n").unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "table/open",
            Some(json!({"sessionId": "tb1", "left": lcsv.to_string_lossy(), "right": rcsv.to_string_lossy()})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .expect("table/open");
        let v = value_of(
            dispatch(
                "report/generate",
                Some(json!({"sessionId": "tb1", "layout": "csv", "delimiter": "tsv"})),
                &c, &ai_dir::CancelFlag::new(), chan().0,
            )
            .await,
        )
        .await;
        let content = v["content"].as_str().unwrap();
        assert!(content.contains('\t') && content.contains("added"), "{content}");

        let lbin = dir.path().join("l.bin");
        let rbin = dir.path().join("r.bin");
        std::fs::write(&lbin, b"AAAABBBB").unwrap();
        std::fs::write(&rbin, b"AAAAXXBB").unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "hex/open",
            Some(json!({"sessionId": "hx1", "left": lbin.to_string_lossy(), "right": rbin.to_string_lossy()})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .expect("hex/open");
        let v = value_of(
            dispatch(
                "report/generate",
                Some(json!({"sessionId": "hx1", "layout": "stats"})),
                &c, &ai_dir::CancelFlag::new(), chan().0,
            )
            .await,
        )
        .await;
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("changedBytes=2"), "{content}");

        let lb = dir.path().join("lb.txt");
        let mb = dir.path().join("mb.txt");
        let rb = dir.path().join("rb.txt");
        std::fs::write(&lb, "a\nb\nc\n").unwrap();
        std::fs::write(&mb, "a\nb\nc\n").unwrap();
        std::fs::write(&rb, "a\nC\nc\n").unwrap();
        let (tx, _rx) = chan();
        let _ = dispatch(
            "text/merge3/open",
            Some(json!({
                "sessionId": "m1",
                "base": mb.to_string_lossy(), "left": lb.to_string_lossy(), "right": rb.to_string_lossy()
            })),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .expect("merge3/open");
        let v = value_of(
            dispatch(
                "report/generate",
                Some(json!({"sessionId": "m1", "layout": "xml"})),
                &c, &ai_dir::CancelFlag::new(), chan().0,
            )
            .await,
        )
        .await;
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("<report kind=\"merge\""), "{content}");
        assert!(content.contains("origin=\"rightOnly\"") || content.contains("origin=\"leftOnly\"") || content.contains("origin=\"conflict\""), "{content}");
    }

    #[tokio::test]
    async fn report_generate_unknown_session_and_bad_layout() {
        let c = ctx();
        let r = dispatch(
            "report/generate",
            Some(json!({"sessionId": "ghost", "layout": "summary"})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await;
        assert!(r.is_err(), "未知会话应报错");
        // patch 布局对目录会话不支持
        let (dir, l, r2) = make_dir_session(&c, "d12").await;
        let _ = (dir, l, r2);
        let res = dispatch(
            "report/generate",
            Some(json!({"sessionId": "d12", "layout": "patch"})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await;
        assert!(res.is_err(), "目录会话 patch 布局应报不支持");
    }

    #[tokio::test]
    async fn initialize_reports_reports_capability() {
        let v = value_of(initialize()).await;
        assert_eq!(v["capabilities"]["reports"], true);
    }
}
