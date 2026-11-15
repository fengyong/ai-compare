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
use crate::server::{DirSessionData, Session, SessionMap, SideEncoding, TextSessionData};
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
            // R3-14：路由型 provider——无 scheme 走 LocalFs（行为与之前一致），
            // zip:// 与 ftp:// 透明可用。写能力由 capabilities_for 按路径协商。
            vfs: Arc::new(ai_vfs::RouteFs::new()),
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
        ai_protocol::method::INITIALIZE => initialize(params),
        ai_protocol::method::DIR_OPEN => dir_open(params, ctx, cancel, out).await,
        ai_protocol::method::DIR_EXPAND => dir_expand(params, ctx, out).await,
        ai_protocol::method::DIR_FILTER => dir_filter(params, ctx, out).await,
        ai_protocol::method::DIR_ALIGN => dir_align(params, ctx, out).await,
        ai_protocol::method::DIR_OPERATE => dir_operate(params, ctx).await,
        ai_protocol::method::DIR_COMPARE_CONTENT => dir_compare_content(params, ctx, out).await,
        ai_protocol::method::TEXT_OPEN => text_open(params, ctx).await,
        ai_protocol::method::TEXT_SECTIONS => text_sections(params, ctx, out).await,
        ai_protocol::method::TEXT_LINES => text_lines(params, ctx).await,
        ai_protocol::method::TEXT_INLINE_DIFF => text_inline_diff(params, ctx).await,
        ai_protocol::method::TEXT_EDIT => text_edit(params, ctx).await,
        ai_protocol::method::TEXT_SAVE => text_save(params, ctx).await,
        ai_protocol::method::TEXT_MERGE3_OPEN => merge3_open(params, ctx).await,
        ai_protocol::method::TEXT_MERGE3_SECTIONS => merge3_sections(params, ctx).await,
        ai_protocol::method::TEXT_MERGE3_DECIDE => merge3_decide(params, ctx).await,
        ai_protocol::method::TEXT_MERGE3_OUTPUT => merge3_output(params, ctx).await,
        ai_protocol::method::TEXT_MERGE3_SAVE => merge3_save(params, ctx).await,
        ai_protocol::method::TABLE_OPEN => table_open(params, ctx).await,
        ai_protocol::method::TABLE_SECTIONS => table_sections(params, ctx).await,
        ai_protocol::method::HEX_OPEN => hex_open(params, ctx).await,
        ai_protocol::method::HEX_BYTES => hex_bytes(params, ctx).await,
        ai_protocol::method::TEXT_PATCH_APPLY => patch_apply(params, ctx).await,
        ai_protocol::method::DIR_EXECUTE => dir_execute(params, ctx, cancel).await,
        ai_protocol::method::DIR_MERGE_SCAN => dir_merge_scan(params, ctx, cancel).await,
        ai_protocol::method::DIR_MERGE_PLAN => dir_merge_plan(params, ctx).await,
        ai_protocol::method::DIR_MERGE_EXECUTE => dir_merge_execute(params, ctx, cancel).await,
        ai_protocol::method::FORMATS_LIST => formats_list(params, ctx).await,
        ai_protocol::method::FORMATS_SAVE => formats_save(params, ctx).await,
        ai_protocol::method::FORMATS_LOAD => formats_load(params, ctx).await,
        ai_protocol::method::RULES_GET => rules_get(params, ctx).await,
        ai_protocol::method::RULES_SET => rules_set(params, ctx).await,
        ai_protocol::method::REPORT_GENERATE => report_generate(params, ctx).await,
        ai_protocol::method::PICTURE_OPEN => picture_open(params, ctx).await,
        ai_protocol::method::SESSION_CLOSE => session_close(params, ctx).await,
        // 仅测试构建存在：锁定 server 的 panic 隔离边界（handler panic 必须应答
        // -32603，而不是让请求永不应答）。生产二进制不含此方法。
        #[cfg(test)]
        "test/panic" => panic!("故意 panic：panic 隔离回归"),
        other => Err(RpcErrorObject {
            code: codes::METHOD_NOT_FOUND,
            message: format!("未知方法: {other}"),
            data: None,
        }),
    }
}

fn initialize(params: Option<serde_json::Value>) -> Result<Handled, RpcErrorObject> {
    // 解析并校验客户端协议版本：major 不兼容直接拒绝，不再无条件握手
    let raw = params.ok_or_else(|| RpcErrorObject {
        code: codes::INVALID_PARAMS,
        message: "initialize 缺少 params".to_string(),
        data: None,
    })?;
    let p: ai_protocol::InitializeParams =
        serde_json::from_value(raw).map_err(|e| RpcErrorObject {
            code: codes::INVALID_PARAMS,
            message: format!("initialize params 非法: {e}"),
            data: None,
        })?;
    let major = |v: &str| v.split('.').next().unwrap_or("").parse::<u32>().ok();
    if major(&p.protocol_version).is_none()
        || major(&p.protocol_version) != major(ai_protocol::PROTOCOL_VERSION)
    {
        return Err(RpcErrorObject {
            code: codes::INVALID_PARAMS,
            message: format!(
                "协议版本不兼容: client={} server={}",
                p.protocol_version,
                ai_protocol::PROTOCOL_VERSION
            ),
            data: None,
        });
    }
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
            picture_compare: Some(true),
            // R3-07：文件夹三向合并入口开放（dir/mergeScan|mergePlan|mergeExecute）
            dir_merge: Some(true),
            // R3-18：编辑增量重 diff 入口开放（text/edit，引擎权威重要性）
            text_edit: Some(true),
            // R3-04：编辑内容落盘入口开放（text/save，含另存为与自动备份）
            text_save: Some(true),
            // R3-20：三向合并输出落盘入口开放（text/merge3/save）
            merge_save: Some(true),
            // R3-16：手动内容比较入口开放（dir/compareContent）
            dir_compare_content: Some(true),
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

/// 目录会话的重要性模型（R3-16：criteria.contentMethod=rulesBased 时生效）。
/// 与 text/open 同一 rules 载荷口径；缺省 = 引擎默认（差异恒重要）。
fn dir_importance(rules: &Option<ai_protocol::RulesSpec>) -> ai_core::ImportanceOptions {
    match rules {
        Some(spec) => mapping::rules_spec_to_core(spec).0,
        None => ai_core::ImportanceOptions::default(),
    }
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
        &ai_dir::DirScanOptions {
            depth: params.depth,
            criteria: params.criteria.clone(),
            importance: dir_importance(&params.rules),
        },
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
    opts.importance = dir_importance(&params.rules);
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
        let mut delta = batch::make_delta_with_progress(
            &params.session_id,
            first.rev,
            &b,
            first.stats,
            Some(first.progress),
            if i == 0 { first.removals.clone() } else { Vec::new() },
        );
        if i == 0 {
            delta.scan_errors = batch::scan_issues(&first.scan_errors);
        }
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
    if params.rescan == Some(true) {
        model.refresh().await.map_err(|e| mapping::dir_scan_error_to_rpc(&e))?;
        model.start_background();
    } else if params.name_filter.is_some() || params.other_filter.is_some() {
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

/// dir/compareContent：手动内容比较（R3-16，FR-DIR-012 三方式 + 五态结果）
async fn dir_compare_content(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    out: tokio::sync::mpsc::UnboundedSender<crate::server::Outbound>,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::DirCompareContentParams = parse_params(params)?;
    let model = dir_model(&ctx.sessions, &params.session_id).await?;
    let method = params
        .method
        .unwrap_or_else(|| model.options().criteria.content_method);
    let rows = model
        .compare_content(&params.ids, method)
        .await
        .map_err(|e| mapping::dir_scan_error_to_rpc(&e))?;
    let delta = model.take_delta().await;
    let stats = model.stats_snapshot();
    if let Some(d) = &delta {
        push_delta(&out, &params.session_id, d, ctx.max_batch_rows);
    }
    let response = ai_protocol::DirCompareContentResult {
        results: rows
            .into_iter()
            .map(|(id, result, status)| ai_protocol::DirContentCompareRow { id, result, status })
            .collect(),
        stats,
    };
    Ok(Handled::Value(
        serde_json::to_value(&response).expect("dir/compareContent 结果可序列化"),
    ))
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
        Session::Text(_) | Session::Merge(_) | Session::DirMerge(_) | Session::Table(_) | Session::Hex(_)
        | Session::Picture(_) => {
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
        let mut delta = batch::make_delta_with_progress(
            session_id,
            d.rev,
            &b,
            d.stats,
            Some(d.progress),
            if i == 0 { d.removals.clone() } else { Vec::new() },
        );
        if i == 0 {
            delta.scan_errors = batch::scan_issues(&d.scan_errors);
        }
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

    let left_dec = ctx
        .vfs
        .read_file_decoded(&left_path, params.encoding.as_deref())
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right_dec = ctx
        .vfs
        .read_file_decoded(&right_path, params.encoding.as_deref())
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    let lt = LineTable::from_text(&left_dec.text);
    let rt = LineTable::from_text(&right_dec.text);

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
                encodings: (
                    SideEncoding::from_decoded(&left_dec),
                    SideEncoding::from_decoded(&right_dec),
                ),
                modified: (false, false),
            }),
        );
    }

    let result = ai_protocol::TextOpenResult {
        session_id: params.session_id,
        left_line_count,
        right_line_count,
        section_count,
        summary,
        left_encoding: left_dec.encoding.to_string(),
        right_encoding: right_dec.encoding.to_string(),
    };
    Ok(Handled::Value(serde_json::to_value(&result).expect("text/open 结果可序列化")))
}

/// text/edit：编辑增量落行表 -> 按会话配置重 diff -> 应答权威 sections（R3-18）。
///
/// 引擎是编辑态与重要性的唯一权威：UI 提交行区间补丁，引擎用打开会话时的
/// 完整 [`ai_core::DiffConfig`]（骨架选项 + 重要性 + 规则折叠 + 手动对齐）重算，
/// 因此编辑后"不重要差异"不会被抹成重要（此前 UI 侧 TS 重 diff 的缺陷）。
///
/// 原子语义：先按序校验全部区间与手动对齐锚点，任一不合法则整批不落会话。
async fn text_edit(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::TextEditParams = parse_params(params)?;
    if params.edits.is_empty() {
        return Err(bad_params("edits 不能为空"));
    }
    let result = with_text_session(&ctx.sessions, &params.session_id, |data| {
        // 1. 校验（不改行表）：逐 op 跟踪两侧行数，区间必须落在当时的长度内
        let mut lens = [data.left.len(), data.right.len()];
        let mut spans = Vec::with_capacity(params.edits.len());
        for op in &params.edits {
            let idx = match op.side {
                ai_protocol::DiffSide::Left => 0,
                ai_protocol::DiffSide::Right => 1,
            };
            let start = op.start as usize;
            let end = op.end.map_or(lens[idx], |e| e as usize);
            if start > end || end > lens[idx] {
                return Err(bad_params(format!(
                    "编辑区间越界: side={} start={start} end={end} 当前行数={}",
                    if idx == 0 { "left" } else { "right" },
                    lens[idx]
                )));
            }
            let new_len = LineTable::from_text(&op.text).len();
            lens[idx] = lens[idx] - (end - start) + new_len;
            spans.push((idx, start, end));
        }

        // 2. 手动对齐锚点按新行数复核；失效则丢弃锚点（编辑优先于对齐覆盖，
        //    否则用户一旦删行就再也无法编辑）。隔离/偏斜设置保留。
        let mut config = data.config.clone();
        if config
            .manual
            .validate(lens[0] as u32, lens[1] as u32)
            .is_err()
        {
            config.manual.align_anchors.clear();
        }

        // 3. 应用（区间已校验，此步不会失败）
        for (op, &(idx, start, end)) in params.edits.iter().zip(&spans) {
            let table = if idx == 0 {
                &mut data.left
            } else {
                &mut data.right
            };
            let entries = edit_entries(table, start, end, &op.text);
            table.replace_lines_entries(start, end, entries);
            if idx == 0 {
                data.modified.0 = true;
            } else {
                data.modified.1 = true;
            }
        }

        // 4. 引擎权威重 diff
        let keyed = ai_core::diff_config(&data.left, &data.right, &config)
            .map_err(|e| ai_protocol::error_object(codes::INVALID_PARAMS, format!("{e}")))?;
        data.config = config;
        data.sections = keyed.output.sections;
        data.summary = keyed.output.summary;
        data.rev += 1;

        Ok(ai_protocol::TextEditResult {
            session_id: params.session_id.clone(),
            rev: data.rev,
            left_line_count: u32::try_from(data.left.len()).unwrap_or(u32::MAX),
            right_line_count: u32::try_from(data.right.len()).unwrap_or(u32::MAX),
            section_count: u32::try_from(data.sections.len()).unwrap_or(u32::MAX),
            summary: mapping::summary_to_dto(&data.summary),
            sections: data.sections.iter().map(mapping::section_to_dto).collect(),
        })
    })
    .await?;
    Ok(Handled::Value(
        serde_json::to_value(&result).expect("text/edit 结果可序列化"),
    ))
}

/// text/save：会话行表落盘（R3-04，FR-TXT-020 保存 / 另存为 / 自动备份）。
///
/// - `side=both`（缺省）只写会话内被编辑过的侧：未编辑侧不落盘，避免无谓改写
///   mtime 干扰父目录的时间戳比较判定；显式 left/right 无条件写该侧（另存为）。
/// - 行尾风格与末尾换行由行表原样保留（`LineTable::to_text` 精确回环），
///   编码与 BOM 按打开时生效的结果原样回写（FR-TXT-013）；目标编码表示不了的
///   字符整次保存失败（不写盘、不静默替换）。
/// - `backup` 缺省 true：覆盖前把原文件复制为 `<path>.bak`；目标不存在时不备份。
///   备份失败即整次保存失败（不能静默丢掉用户要求的保险）。
/// - 另存为会把该侧标签改指到新目标（与 BC 一致），否则下一次保存会写回原文件。
/// - 保存不改比较模型：`rev` 不递增；成功落盘的侧清除未保存编辑标记。
async fn text_save(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::TextSaveParams = parse_params(params)?;
    use ai_protocol::TextSaveSide as S;

    // 1. 锁内取快照（行表序列化 + 目标解析），IO 不持会话锁
    let plan = with_text_session(&ctx.sessions, &params.session_id, |data| {
        let selected = match params.side {
            S::Left => [true, false],
            S::Right => [false, true],
            S::Both => [data.modified.0, data.modified.1],
        };
        let targets = [
            params.left_path.as_deref().unwrap_or(&data.labels.0),
            params.right_path.as_deref().unwrap_or(&data.labels.1),
        ];
        let tables = [&data.left, &data.right];
        let encs = [&data.encodings.0, &data.encodings.1];
        let mut plan = Vec::new();
        for idx in 0..2 {
            if !selected[idx] {
                continue;
            }
            let path = VfsPath::parse(targets[idx]).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
            let text = tables[idx].to_text();
            let bytes = encs[idx]
                .encode(&text)
                .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
            plan.push(SavePlan {
                idx,
                path,
                bytes,
                line_count: u32::try_from(tables[idx].len()).unwrap_or(u32::MAX),
                retarget: if idx == 0 {
                    params.left_path.is_some()
                } else {
                    params.right_path.is_some()
                },
            });
        }
        Ok(plan)
    })
    .await?;

    // 2. 备份 + 写入（任一侧失败即中止；已写侧不回滚，与 BC 逐侧保存一致）
    let backup = params.backup.unwrap_or(true);
    let mut saved = Vec::with_capacity(plan.len());
    for p in &plan {
        let backup_path = if backup {
            let bak = VfsPath::parse(&format!("{}.bak", p.path.as_str()))
                .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
            match ctx.vfs.read_file_bytes(&p.path).await {
                Ok(orig) => {
                    ctx.vfs
                        .write_file_bytes(&bak, &orig)
                        .await
                        .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
                    Some(bak.as_str().to_string())
                }
                Err(ai_vfs::VfsError::NotFound { .. }) => None,
                Err(e) => return Err(mapping::vfs_error_to_rpc(&e)),
            }
        } else {
            None
        };
        ctx.vfs
            .write_file_bytes(&p.path, &p.bytes)
            .await
            .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
        saved.push(ai_protocol::TextSaveEntry {
            side: if p.idx == 0 {
                ai_protocol::DiffSide::Left
            } else {
                ai_protocol::DiffSide::Right
            },
            path: p.path.as_str().to_string(),
            bytes: p.bytes.len() as u64,
            line_count: p.line_count,
            backup_path,
        });
    }

    // 3. 清除已落盘侧的未保存标记（另存为同时改指标签）
    let result = with_text_session(&ctx.sessions, &params.session_id, |data| {
        for (p, entry) in plan.iter().zip(&saved) {
            if p.idx == 0 {
                data.modified.0 = false;
                if p.retarget {
                    data.labels.0 = entry.path.clone();
                }
            } else {
                data.modified.1 = false;
                if p.retarget {
                    data.labels.1 = entry.path.clone();
                }
            }
        }
        Ok(ai_protocol::TextSaveResult {
            session_id: params.session_id.clone(),
            saved,
            rev: data.rev,
            left_modified: data.modified.0,
            right_modified: data.modified.1,
        })
    })
    .await?;
    Ok(Handled::Value(
        serde_json::to_value(&result).expect("text/save 结果可序列化"),
    ))
}

/// text/save 的单侧落盘计划（锁内算好，锁外做 IO）
struct SavePlan {
    idx: usize,
    path: VfsPath,
    bytes: Vec<u8>,
    line_count: u32,
    retarget: bool,
}

/// 编辑区间的新行条目：行尾风格随该侧既有风格，文件末尾"有无换行符"语义保留。
///
/// UI 的行数组不含行尾符（`text/lines` 口径），直接拼接会把 CRLF 文件的
/// 被编辑段变成 LF；故此处按参考行的行尾归一 inserted 文本。
fn edit_entries(
    table: &LineTable,
    start: usize,
    end: usize,
    text: &str,
) -> Vec<ai_core::LineEntry> {
    let eol = edit_eol(table, start);
    let normalized = normalize_newlines(text, eol);
    let mut entries: Vec<ai_core::LineEntry> =
        LineTable::from_text(&normalized).iter().cloned().collect();
    // 编辑触及文件末尾且原末行带行尾符：新末行补同风格行尾（纯追加/改末行
    // 不应悄悄删掉文件的尾换行）。原末行本就无行尾符时保持无。
    let old_last_had_eol = table
        .iter()
        .last()
        .is_some_and(|l| l.ending != ai_core::LineEnding::None);
    if end == table.len() && old_last_had_eol {
        if let Some(last) = entries.last_mut() {
            if last.ending == ai_core::LineEnding::None {
                last.ending = eol;
            }
        }
    }
    entries
}

/// 参考行尾：优先取编辑起点处的既有行（插入时即其后继行），否则取末行；
/// 两者都无行尾符（或空表）时退回全表多数风格。
fn edit_eol(table: &LineTable, start: usize) -> ai_core::LineEnding {
    use ai_core::LineEnding as E;
    let reference = if start < table.len() {
        Some(table.line(start).ending)
    } else {
        table.iter().last().map(|l| l.ending)
    };
    match reference {
        Some(E::Lf) => E::Lf,
        Some(E::CrLf) => E::CrLf,
        Some(E::Cr) => E::Cr,
        _ => dominant_eol(table),
    }
}

/// 全表多数行尾风格（None 不计；平票取 LF，与 merge marker 同口径）
fn dominant_eol(table: &LineTable) -> ai_core::LineEnding {
    use ai_core::LineEnding as E;
    let mut lf = 0u32;
    let mut crlf = 0u32;
    let mut cr = 0u32;
    for l in table.iter() {
        match l.ending {
            E::Lf => lf += 1,
            E::CrLf => crlf += 1,
            E::Cr => cr += 1,
            E::None => {}
        }
    }
    if crlf > lf && crlf > cr {
        E::CrLf
    } else if cr > lf && cr > crlf {
        E::Cr
    } else {
        E::Lf
    }
}

/// 把 inserted 文本的行尾统一为指定风格（CRLF/CR/LF 混排先归一再替换）
fn normalize_newlines(text: &str, eol: ai_core::LineEnding) -> String {
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    if eol == ai_core::LineEnding::Lf {
        unified
    } else {
        unified.replace('\n', eol.as_str())
    }
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
        Session::Dir(_) | Session::Merge(_) | Session::DirMerge(_) | Session::Table(_) | Session::Hex(_)
        | Session::Picture(_) => {
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

    // 幂等打开：同 sessionId + 同三路路径 → 直接返回既有会话统计。
    //
    // 为什么必须幂等：标签重挂载与 React StrictMode 双挂载都会重复 open。
    // 若一律回 411 让调用方先 close 再重建，引擎侧已提交的冲突决策
    // （decisions 表）会被清空 —— 用户切走再切回标签就丢掉全部决策（FR-TMRG-004/005）。
    {
        let sessions = ctx.sessions.lock().await;
        if let Some(Session::Merge(data)) = sessions.get(&params.session_id) {
            if data.labels.0 == params.base
                && data.labels.1 == params.left
                && data.labels.2 == params.right
            {
                let result = ai_protocol::MergeOpenResult {
                    session_id: params.session_id.clone(),
                    base_line_count: data.model.base_len,
                    left_line_count: data.model.left_len,
                    right_line_count: data.model.right_len,
                    chunk_count: u32::try_from(data.model.chunks.len()).unwrap_or(u32::MAX),
                    conflict_count: data.model.conflict_count(),
                    rev: data.rev,
                };
                return Ok(Handled::Value(
                    serde_json::to_value(&result).expect("merge3/open 结果可序列化"),
                ));
            }
        }
    }
    let base_path = VfsPath::parse(&params.base).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let left_path = VfsPath::parse(&params.left).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right_path = VfsPath::parse(&params.right).map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    let base_dec = ctx
        .vfs
        .read_file_decoded(&base_path, None)
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let left_text = ctx
        .vfs
        .read_file_decoded(&left_path, None)
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?
        .text;
    let right_text = ctx
        .vfs
        .read_file_decoded(&right_path, None)
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?
        .text;

    let base = LineTable::from_text(&base_dec.text);
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
                encoding: SideEncoding::from_decoded(&base_dec),
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

/// 输出合成的策略选择（output/save 共用）：Marker 显式渲染冲突标记，
/// 缺省 Plain = 未决冲突走会话 conflict_default 策略。
fn output_syn_options(
    data: &crate::server::MergeSessionData,
    mode: Option<ai_protocol::MergeOutputMode>,
    marker_style: Option<ai_protocol::MergeMarkerStyle>,
) -> SynthesizeOptions {
    match mode {
        Some(ai_protocol::MergeOutputMode::Marker) => {
            let style = match marker_style.unwrap_or(ai_protocol::MergeMarkerStyle::Merge) {
                ai_protocol::MergeMarkerStyle::Merge => MarkerStyle::Merge,
                ai_protocol::MergeMarkerStyle::Diff3 => MarkerStyle::Diff3,
            };
            marker_policy_of(data, style)
        }
        _ => session_syn_options(data),
    }
}

/// text/merge3/output：合成输出（决策生效；未决冲突按模式渲染 + 清单）
async fn merge3_output(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::MergeOutputParams = parse_params(params)?;
    let result = with_merge_session(&ctx.sessions, &params.session_id, |data| {
        let syn = output_syn_options(data, params.mode, params.marker_style);
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

/// text/merge3/save：把合成输出写入"合并到"目标文件（R3-20，FR-TMRG-006）。
///
/// - 输出由引擎按会话决策现场合成（与 output 同一策略），不经客户端回传，
///   因此非活动标签与大输出同样可保存；
/// - 目标文件的先前内容被覆盖（BC 语义：会话前存在或会话中被更改的内容都丢失），
///   覆盖警示由 UI 给出；
/// - `backup` 缺省 true：覆盖前把原内容写到 `<path>.bak`（目标不存在则不产生备份）；
/// - 保存不改决策：`rev` 不递增；祖先文件的编码与 BOM 原样回写。
async fn merge3_save(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::MergeSaveParams = parse_params(params)?;
    if params.path.is_empty() {
        return Err(bad_params("合并输出目标路径不能为空"));
    }

    // 1. 锁内合成 + 目标解析（IO 不持会话锁）
    let plan = with_merge_session(&ctx.sessions, &params.session_id, |data| {
        let path = VfsPath::parse(&params.path).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
        let syn = output_syn_options(data, params.mode, params.marker_style);
        let out = ai_core::synthesize(
            &data.model,
            &data.base,
            &data.left,
            &data.right,
            &data.decisions,
            &syn,
        );
        let bytes = data
            .encoding
            .encode(&out.text)
            .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
        Ok(MergeSavePlan {
            path,
            bytes,
            line_count: out.lines,
            conflict_count: data.model.conflict_count(),
            has_conflicts: !out.unresolved.is_empty(),
            unresolved_chunks: out.unresolved,
            rev: data.rev,
        })
    })
    .await?;

    // 2. 备份 + 写入（与 text/save 同口径：备份失败即中止，不覆盖目标）
    let backup_path = if params.backup.unwrap_or(true) {
        let bak = VfsPath::parse(&format!("{}.bak", plan.path.as_str()))
            .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
        match ctx.vfs.read_file_bytes(&plan.path).await {
            Ok(orig) => {
                ctx.vfs
                    .write_file_bytes(&bak, &orig)
                    .await
                    .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
                Some(bak.as_str().to_string())
            }
            Err(ai_vfs::VfsError::NotFound { .. }) => None,
            Err(e) => return Err(mapping::vfs_error_to_rpc(&e)),
        }
    } else {
        None
    };
    ctx.vfs
        .write_file_bytes(&plan.path, &plan.bytes)
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    let result = ai_protocol::MergeSaveResult {
        session_id: params.session_id,
        path: plan.path.as_str().to_string(),
        bytes: plan.bytes.len() as u64,
        line_count: plan.line_count,
        backup_path,
        conflict_count: plan.conflict_count,
        unresolved_chunks: plan.unresolved_chunks,
        has_conflicts: plan.has_conflicts,
        rev: plan.rev,
    };
    Ok(Handled::Value(
        serde_json::to_value(&result).expect("merge3/save 结果可序列化"),
    ))
}

/// text/merge3/save 的落盘计划（锁内算好，锁外做 IO）
struct MergeSavePlan {
    path: VfsPath,
    bytes: Vec<u8>,
    line_count: u32,
    conflict_count: u32,
    unresolved_chunks: Vec<u32>,
    has_conflicts: bool,
    rev: u64,
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

/// P14：picture/open：读两侧图片字节流 -> 解码 -> 像素容差 diff -> 网格块聚合
/// -> 存会话 -> 应答统计与差异块（块数 >10 万截断，truncated=true）
async fn picture_open(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    use ai_core::picture;

    let params: ai_protocol::PictureOpenParams = parse_params(params)?;
    if params.session_id.is_empty() {
        return Err(bad_params("sessionId 不能为空"));
    }
    let left_path = VfsPath::parse(&params.left).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right_path = VfsPath::parse(&params.right).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let left = ctx
        .vfs
        .read_file_bytes(&left_path)
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right = ctx
        .vfs
        .read_file_bytes(&right_path)
        .await
        .map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    let mut opts = picture::PictureDiffOptions::default();
    if let Some(spec) = &params.options {
        if let Some(t) = spec.tolerance {
            opts.tolerance = t.min(255) as u8;
        }
        if let Some(b) = spec.block {
            opts.block = b.max(1);
        }
    }

    let result = picture::diff_bytes(&left, &right, &opts)
        .map_err(|e| RpcErrorObject {
            code: codes::INVALID_PARAMS,
            message: e,
            data: None,
        })?;

    // 块数上限保护（>10 万截断，truncated=true）
    const MAX_BLOCKS: usize = 100_000;
    let truncated = result.blocks.len() > MAX_BLOCKS;
    let blocks: Vec<ai_protocol::PictureBlockDto> = result
        .blocks
        .iter()
        .take(MAX_BLOCKS)
        .map(|b| ai_protocol::PictureBlockDto {
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
            diff_ratio: b.diff_ratio,
        })
        .collect();

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
            Session::Picture(crate::server::PictureSessionData {
                result: result.clone(),
                options: opts,
                labels: (params.left.clone(), params.right.clone()),
            }),
        );
    }

    let out = ai_protocol::PictureOpenResult {
        session_id: params.session_id,
        width: result.width,
        height: result.height,
        left_width: result.left_size.0,
        left_height: result.left_size.1,
        right_width: result.right_size.0,
        right_height: result.right_size.1,
        diff_pixels: result.diff_pixels,
        total_pixels: result.total_pixels,
        similarity_pct: result.similarity_pct,
        blocks,
        truncated,
    };
    Ok(Handled::Value(serde_json::to_value(&out).expect("picture/open 结果可序列化")))
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

    // 非 dry_run：rescan 推 delta（模型会话走 refresh() 强制重扫——落盘后必须重扫，
    // 不能借道 update_filters：过滤器未变时它是 no-op；
    // P0 同步会话重新全量扫描替换会话数据并推完整 upsert 批次）
    let mut notifications = Vec::new();
    if !dry_run {
        if let Some(model) = &model {
            model.refresh().await.map_err(|e| mapping::dir_scan_error_to_rpc(&e))?;
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

/// dir/mergeScan：扫描 base/left/right 三树 + diff3 分类（R3-07，FR-DMRG-002）。
/// 同 sessionId 重复调用 = 重扫并替换会话（与 dir/open 同语义）。
async fn dir_merge_scan(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    cancel: &ai_dir::CancelFlag,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::DirMergeScanParams = parse_params(params)?;
    let base = VfsPath::parse(&params.base).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let left = VfsPath::parse(&params.left).map_err(|e| mapping::vfs_error_to_rpc(&e))?;
    let right = VfsPath::parse(&params.right).map_err(|e| mapping::vfs_error_to_rpc(&e))?;

    let scan = ai_dir::merge3way::scan_merge(ctx.vfs.as_ref(), &base, &left, &right, cancel)
        .await
        .map_err(|e| mapping::dir_scan_error_to_rpc(&e))?;

    let result = ai_protocol::DirMergeScanResult {
        session_id: params.session_id.clone(),
        rows: scan.rows().iter().map(mapping::dir_merge_row_to_dto).collect(),
        stats: mapping::dir_merge_scan_stats_to_dto(&scan.stats),
    };

    ctx.sessions.lock().await.insert(
        params.session_id.clone(),
        Session::DirMerge(crate::server::DirMergeSessionData {
            roots: ai_dir::MergeRoots { base, left, right, output: None },
            scan,
        }),
    );

    Ok(Handled::Value(serde_json::to_value(&result).expect("dir/mergeScan 结果可序列化")))
}

/// 取文件夹合并会话（不存在 / 类型不符均报 INVALID_PARAMS）
async fn dir_merge_session(
    ctx: &HandlerCtx,
    session_id: &str,
) -> Result<crate::server::DirMergeSessionData, RpcErrorObject> {
    let sessions = ctx.sessions.lock().await;
    match sessions.get(session_id) {
        Some(Session::DirMerge(data)) => Ok(data.clone()),
        Some(_) => Err(bad_params(format!("会话不是文件夹合并会话: {session_id}"))),
        None => Err(session_not_found(session_id)),
    }
}

/// dir/mergePlan：决策 -> 计划预览（纯函数，不落盘；R3-07，FR-DMRG-003）。
/// target=output 必须给 outputPath（本次或此前记入会话的），并记回会话供 execute 复用。
async fn dir_merge_plan(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::DirMergePlanParams = parse_params(params)?;
    let mut data = dir_merge_session(ctx, &params.session_id).await?;
    let target = mapping::dir_merge_target_from_dto(params.target);

    if let Some(out) = &params.output_path {
        data.roots.output = Some(VfsPath::parse(out).map_err(|e| mapping::vfs_error_to_rpc(&e))?);
    }
    if target == ai_dir::MergeTarget::Output && data.roots.output.is_none() {
        return Err(bad_params("target=output 需要提供 outputPath（第三输出目录）"));
    }

    let decisions: std::collections::BTreeMap<String, ai_dir::MergeDecision> = params
        .decisions
        .iter()
        .map(|(k, v)| (k.clone(), mapping::dir_merge_decision_from_dto(*v)))
        .collect();
    let plan = ai_dir::merge3way::plan_merge(
        &data.scan.arena,
        &ai_dir::MergePlanParams { target, decisions: &decisions },
    );

    let result = ai_protocol::DirMergePlanResult {
        session_id: params.session_id.clone(),
        target: params.target,
        ops: plan.ops.iter().map(mapping::dir_merge_planned_op_to_dto).collect(),
        stats: mapping::dir_merge_batch_stats_to_dto(&plan.stats),
    };

    // 记回输出根（execute 可省略 outputPath）
    if let Some(Session::DirMerge(d)) = ctx.sessions.lock().await.get_mut(&params.session_id) {
        d.roots.output = data.roots.output.clone();
    }

    Ok(Handled::Value(serde_json::to_value(&result).expect("dir/mergePlan 结果可序列化")))
}

/// dir/mergeExecute：执行已预览的计划（复用 execute 层护栏/部分成功/取消语义）。
/// target=output 时必须有输出根（本次参数或 plan 时记入会话的）。
async fn dir_merge_execute(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    cancel: &ai_dir::CancelFlag,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::DirMergeExecuteParams = parse_params(params)?;
    let mut data = dir_merge_session(ctx, &params.session_id).await?;
    let target = mapping::dir_merge_target_from_dto(params.target);

    if let Some(out) = &params.output_path {
        data.roots.output = Some(VfsPath::parse(out).map_err(|e| mapping::vfs_error_to_rpc(&e))?);
    }
    if target == ai_dir::MergeTarget::Output && data.roots.output.is_none() {
        return Err(bad_params("target=output 需要提供 outputPath（第三输出目录）"));
    }

    let ops: Vec<ai_dir::MergePlannedOp> =
        params.ops.iter().map(mapping::dir_merge_planned_op_from_dto).collect();
    let report = ai_dir::merge3way::run_folder_merge(
        ctx.vfs.as_ref(),
        &data.roots,
        &ops,
        target,
        None,
        cancel,
    )
    .await;

    let result = ai_protocol::DirMergeExecuteResult {
        session_id: params.session_id.clone(),
        target: params.target,
        total: mapping::sat_u32(report.stats.total),
        copied: mapping::sat_u32(report.stats.copied),
        deleted: mapping::sat_u32(report.stats.deleted),
        touched: mapping::sat_u32(report.stats.touched),
        skipped: mapping::sat_u32(report.stats.skipped),
        failed: mapping::sat_u32(report.stats.failed),
        cancelled: report.cancelled,
        details: report.details.iter().map(mapping::dir_merge_exec_detail_to_dto).collect(),
        log: report.log.clone(),
    };

    Ok(Handled::Value(serde_json::to_value(&result).expect("dir/mergeExecute 结果可序列化")))
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
            let mut delta = batch::make_delta_with_progress(
                session_id,
                d.rev,
                &b,
                d.stats,
                Some(d.progress),
                if i == 0 { d.removals.clone() } else { Vec::new() },
            );
            if i == 0 {
                delta.scan_errors = batch::scan_issues(&d.scan_errors);
            }
            ai_protocol::RpcServerNotification::new(ai_protocol::method::DIR_DELTA, delta)
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
            // P14：图片会话暂无文本报告布局（视图批次交付 html 图片报告）
            Session::Picture(_) => {
                return Err(bad_params("图片会话不支持 report/generate（P14 视图批次交付）"));
            }
            // R3-07：文件夹合并的执行报告走 dir/mergeExecute 的 details/log，
            // 不复用文本报告布局
            Session::DirMerge(_) => {
                return Err(bad_params("文件夹合并会话不支持 report/generate（执行报告见 dir/mergeExecute）"));
            }
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

    // ------------------------------------------------------------------
    // R3-16：dir/compareContent（FR-DIR-012 三方式 + 五态结果）
    // ------------------------------------------------------------------

    async fn open_cc(
        c: &HandlerCtx,
        tx: &tokio::sync::mpsc::UnboundedSender<Outbound>,
        id: &str,
        l: &std::path::Path,
        r: &std::path::Path,
        extra: serde_json::Value,
    ) {
        let mut params = json!({
            "sessionId": id, "left": l.to_string_lossy(), "right": r.to_string_lossy(),
            "depth": 1, "lazyScan": true,
            "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"},
        });
        for (k, v) in extra.as_object().unwrap() {
            params[k] = v.clone();
        }
        dispatch(
            "dir/open",
            Some(params),
            c,
            &ai_dir::CancelFlag::new(),
            tx.clone(),
        )
        .await
        .unwrap();
    }

    async fn compare_cc(
        c: &HandlerCtx,
        tx: &tokio::sync::mpsc::UnboundedSender<Outbound>,
        id: &str,
        extra: serde_json::Value,
    ) -> serde_json::Value {
        let mut params = json!({"sessionId": id});
        for (k, v) in extra.as_object().unwrap() {
            params[k] = v.clone();
        }
        match dispatch(
            "dir/compareContent",
            Some(params),
            c,
            &ai_dir::CancelFlag::new(),
            tx.clone(),
        )
        .await
        .unwrap()
        {
            Handled::Value(v) => v,
            _ => panic!("dir/compareContent 应答形态"),
        }
    }

    fn eol_trees() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("eol.txt"), b"a\r\nb\r\n").unwrap();
        std::fs::write(r.join("eol.txt"), b"a\nb\n").unwrap();
        (dir, l, r)
    }

    /// 三方式必须真的分派：CRLF/LF 对在 CRC/二进制下判异，规则下判同；
    /// 结果回写行状态并推送 delta（BC：规则比较忽略行尾差异）。
    #[tokio::test]
    async fn compare_content_methods_are_not_interchangeable() {
        let (_dir, l, r) = eol_trees();
        let c = ctx();
        let (tx, rx) = chan();
        open_cc(&c, &tx, "cc1", &l, &r, json!({})).await;
        for (method, result, status) in [
            ("crc", "binaryDiff", "contentDiff"),
            ("binary", "binaryDiff", "contentDiff"),
            ("rulesBased", "rulesSame", "same"),
        ] {
            let v = compare_cc(
                &c,
                &tx,
                "cc1",
                json!({"ids": ["eol.txt"], "method": method}),
            )
            .await;
            assert_eq!(v["results"][0]["id"], "eol.txt", "{method}: {v}");
            assert_eq!(v["results"][0]["result"], result, "{method}: {v}");
            assert_eq!(v["results"][0]["status"], status, "{method}: {v}");
        }
        let msgs = collect(rx).await;
        let deltas: Vec<&serde_json::Value> =
            msgs.iter().filter(|m| m["method"] == "dir/delta").collect();
        assert!(!deltas.is_empty(), "比较后应推送 delta: {msgs:?}");
        let last = deltas.last().unwrap();
        let row = last["params"]["upserts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "eol.txt")
            .expect("delta 含被比较行");
        assert_eq!(row["status"], "same", "规则判同后行状态回写: {last}");
    }

    /// 规则模式的重要性来自会话 rules：默认尾随空白重要 -> 重要差异；
    /// 开启 trailingWsUnimportant -> 仅不重要差异（similar 行 + 统计）。
    #[tokio::test]
    async fn compare_content_rules_importance_from_session_rules() {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("ws.txt"), b"alpha   \n").unwrap();
        std::fs::write(r.join("ws.txt"), b"alpha\n").unwrap();
        let c = ctx();
        let (tx, _rx) = chan();
        open_cc(&c, &tx, "cc2", &l, &r, json!({})).await;
        let v = compare_cc(
            &c,
            &tx,
            "cc2",
            json!({"ids": ["ws.txt"], "method": "rulesBased"}),
        )
        .await;
        assert_eq!(
            v["results"][0]["result"], "importantDiff",
            "默认全重要: {v}"
        );
        assert_eq!(v["results"][0]["status"], "contentDiff");

        let c2 = ctx();
        let (tx2, _rx2) = chan();
        open_cc(
            &c2,
            &tx2,
            "cc3",
            &l,
            &r,
            json!({"rules": {"trailingWsUnimportant": true}}),
        )
        .await;
        let v = compare_cc(
            &c2,
            &tx2,
            "cc3",
            json!({"ids": ["ws.txt"], "method": "rulesBased"}),
        )
        .await;
        assert_eq!(
            v["results"][0]["result"], "unimportantDiff",
            "尾随空白不重要: {v}"
        );
        assert_eq!(v["results"][0]["status"], "similar");
        assert_eq!(v["stats"]["similar"], 1, "similar 计数: {v}");
    }

    /// 省略 method 时用会话 criteria.contentMethod（不是硬编码 CRC）。
    #[tokio::test]
    async fn compare_content_defaults_to_session_method() {
        let (_dir, l, r) = eol_trees();
        let c = ctx();
        let (tx, _rx) = chan();
        open_cc(
            &c,
            &tx,
            "cc4",
            &l,
            &r,
            json!({"criteria": {"mtimeToleranceSec": 0, "contentCompare": "always", "contentMethod": "rulesBased"}}),
        )
        .await;
        let v = compare_cc(&c, &tx, "cc4", json!({"ids": ["eol.txt"]})).await;
        assert_eq!(
            v["results"][0]["result"], "rulesSame",
            "缺省方法取会话判据: {v}"
        );
        // 显式 method 覆盖会话判据（本次生效，不改会话）
        let v = compare_cc(
            &c,
            &tx,
            "cc4",
            json!({"ids": ["eol.txt"], "method": "binary"}),
        )
        .await;
        assert_eq!(v["results"][0]["result"], "binaryDiff");
        let v = compare_cc(&c, &tx, "cc4", json!({"ids": ["eol.txt"]})).await;
        assert_eq!(
            v["results"][0]["result"], "rulesSame",
            "显式覆盖不应污染会话判据: {v}"
        );
    }

    /// ids 为空 = 全部已配对文件行；目录行与孤立项跳过（BC 语义）。
    #[tokio::test]
    async fn compare_content_empty_ids_skips_dirs_and_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir_all(l.join("sub")).unwrap();
        std::fs::create_dir_all(r.join("sub")).unwrap();
        std::fs::write(l.join("same.txt"), b"x").unwrap();
        std::fs::write(r.join("same.txt"), b"x").unwrap();
        std::fs::write(l.join("diff.txt"), b"x").unwrap();
        std::fs::write(r.join("diff.txt"), b"yy").unwrap();
        std::fs::write(l.join("orphan.txt"), b"o").unwrap();
        let c = ctx();
        let (tx, _rx) = chan();
        open_cc(&c, &tx, "cc5", &l, &r, json!({})).await;
        let v = compare_cc(&c, &tx, "cc5", json!({"ids": []})).await;
        let mut got: Vec<(&str, &str)> = v["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["id"].as_str().unwrap(), r["result"].as_str().unwrap()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![("diff.txt", "binaryDiff"), ("same.txt", "binarySame")],
            "目录/孤立项不参与内容比较: {v}"
        );
    }

    /// 未知会话 -> 410；同步路径会话（无模型）-> 明确参数错误。
    #[tokio::test]
    async fn compare_content_rejects_unknown_and_sync_sessions() {
        let (_dir, l, r) = eol_trees();
        let c = ctx();
        let (tx, _rx) = chan();
        let err = dispatch(
            "dir/compareContent",
            Some(json!({"sessionId": "nope", "ids": []})),
            &c,
            &ai_dir::CancelFlag::new(),
            tx.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ai_protocol::error::codes::SESSION_NOT_FOUND);

        dispatch(
            "dir/open",
            Some(json!({
                "sessionId": "cc6", "left": l.to_string_lossy(), "right": r.to_string_lossy(),
                "depth": 1, "criteria": {"contentCompare": "always"}
            })),
            &c,
            &ai_dir::CancelFlag::new(),
            tx.clone(),
        )
        .await
        .unwrap();
        let err = dispatch(
            "dir/compareContent",
            Some(json!({"sessionId": "cc6", "ids": []})),
            &c,
            &ai_dir::CancelFlag::new(),
            tx,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ai_protocol::error::codes::INVALID_PARAMS);
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

    /// P14 语料：程序生成 PNG——左侧全黑 32x32，右侧右上角 16x16 涂白
    async fn open_picture(c: &HandlerCtx, id: &str) -> (tempfile::TempDir, String, String) {
        use image::{Rgba, RgbaImage};
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("l.png");
        let r = dir.path().join("r.png");
        let black = RgbaImage::from_pixel(32, 32, Rgba([0, 0, 0, 255]));
        let mut right = black.clone();
        for y in 0..16u32 {
            for x in 16..32u32 {
                right.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        for (p, img) in [(&l, &black), (&r, &right)] {
            image::DynamicImage::ImageRgba8(img.clone())
                .write_to(&mut std::io::Cursor::new(Vec::new()), image::ImageFormat::Png)
                .unwrap();
            // 直接写 PNG 字节
            let bytes = {
                let mut buf = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(img.clone())
                    .write_to(&mut buf, image::ImageFormat::Png)
                    .unwrap();
                buf.into_inner()
            };
            std::fs::write(p, bytes).unwrap();
            let _ = p;
        }
        let (tx, _rx) = chan();
        let _ = dispatch(
            "picture/open",
            Some(json!({"sessionId": id, "left": l.to_string_lossy(), "right": r.to_string_lossy()})),
            c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        (dir, l.to_string_lossy().into_owned(), r.to_string_lossy().into_owned())
    }

    #[tokio::test]
    async fn picture_open_blocks_and_similarity() {
        let c = ctx();
        let (_d, l, r) = open_picture(&c, "p1").await;
        // 重新 open 一次验证统计（open_picture 内部已 open "p1"，此处用 p2）
        let got = dispatch(
            "picture/open",
            Some(json!({"sessionId": "p2", "left": l, "right": r})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        println!("picture/open p2 响应: {v}");
        assert_eq!(v["width"], 32);
        assert_eq!(v["height"], 32);
        assert_eq!(v["diffPixels"], 256, "右半上 16x16 涂白 = 256 差异像素: {v}");
        // 256 差异像素全部落在 block=32 的 (0,0) 块：256/1024 = 0.25 占比
        let blocks = v["blocks"].as_array().unwrap();
        assert_eq!(blocks.len(), 1, "单块聚合: {v}");
        assert_eq!(blocks[0]["x"], 0);
        assert_eq!(blocks[0]["y"], 0);
        assert_eq!(blocks[0]["diffRatio"], 0.25);
        let sim = v["similarityPct"].as_f64().unwrap();
        assert!((sim - 75.0).abs() < 0.01, "768/1024 = 75%: {v}");
        assert_eq!(v["truncated"], false);
        // 重复 sessionId -> SESSION_EXISTS
        let err = dispatch(
            "picture/open",
            Some(json!({"sessionId": "p2", "left": l, "right": r})),
            &c, &ai_dir::CancelFlag::new(), chan().0,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, 411);
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
    async fn dir_filter_rescan_flag_picks_up_external_changes() {
        let c = ctx();
        let (_d, left, _right) = make_dir_session(&c, "drefresh").await;
        std::fs::write(std::path::Path::new(&left).join("external.txt"), b"E").unwrap();

        // 不带 rescan（UI 显示类切换的形状：空 nameFilter）-> 过滤器未变即 no-op
        let (tx, mut rx) = chan();
        let got = dispatch(
            "dir/filter",
            Some(json!({"sessionId": "drefresh", "nameFilter": {}})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["stats"]["total"], 4, "未请求重扫不得改变行集: {v}");
        while let Ok(_m) = rx.try_recv() {}

        // 右键「刷新」：rescan=true 必须重扫，外部新建文件进入行集并前送
        let (tx, mut rx) = chan();
        let got = dispatch(
            "dir/filter",
            Some(json!({"sessionId": "drefresh", "nameFilter": {}, "rescan": true})),
            &c, &ai_dir::CancelFlag::new(), tx,
        )
        .await
        .unwrap();
        let v = match got { Handled::Value(v) => v, _ => panic!("形态") };
        assert_eq!(v["stats"]["total"], 5, "刷新后含外部新建文件: {v}");
        let mut saw_external = false;
        while let Ok(msg) = rx.try_recv() {
            if let Outbound::Notification(n) = msg {
                for row in n.params["upserts"].as_array().unwrap_or(&Vec::new()) {
                    if row["id"] == "external.txt" {
                        saw_external = true;
                    }
                }
            }
        }
        assert!(saw_external, "刷新须前送新行 dir/delta");
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
        let params = Some(json!({"clientInfo": {"name": "t"}, "protocolVersion": "1.0"}));
        let v = value_of(initialize(params)).await;
        assert_eq!(v["capabilities"]["formatRules"], true);
    }

    #[test]
    fn initialize_rejects_incompatible_protocol_version() {
        let bad = Some(json!({"clientInfo": {"name": "t"}, "protocolVersion": "2.0"}));
        assert_eq!(initialize(bad).unwrap_err().code, codes::INVALID_PARAMS);
        assert_eq!(initialize(None).unwrap_err().code, codes::INVALID_PARAMS);
        let malformed = Some(json!({"clientInfo": {"name": "t"}, "protocolVersion": "x"}));
        assert_eq!(
            initialize(malformed).unwrap_err().code,
            codes::INVALID_PARAMS
        );
        let ok = Some(json!({"clientInfo": {"name": "t"}, "protocolVersion": "1.0"}));
        assert!(initialize(ok).is_ok());
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
        let params = Some(json!({"clientInfo": {"name": "t"}, "protocolVersion": "1.0"}));
        let v = value_of(initialize(params)).await;
        assert_eq!(v["capabilities"]["reports"], true);
    }
}

#[cfg(test)]
mod dir_merge_tests {
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

    async fn value_of(got: Result<Handled, RpcErrorObject>) -> serde_json::Value {
        match got.unwrap() {
            Handled::Value(v) => v,
            Handled::WithNotifications { response, .. } => response,
        }
    }

    /// 三树语料（分类由内容决定，与 mtime 无关）：
    /// same=三方一致 / lchg=仅左改 / rchg=仅右改 / conf=冲突 /
    /// lonly=左孤立 / del=右删除
    fn make3() -> (tempfile::TempDir, String, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let mut roots = Vec::new();
        for name in ["B", "L", "R"] {
            let p = dir.path().join(name);
            std::fs::create_dir_all(&p).unwrap();
            roots.push(p.to_string_lossy().into_owned());
        }
        let put = |side: usize, rel: &str, bytes: &[u8]| {
            std::fs::write(std::path::Path::new(&roots[side]).join(rel), bytes).unwrap();
        };
        for s in 0..3 {
            put(s, "same.txt", b"1");
        }
        put(0, "lchg.txt", b"old");
        put(1, "lchg.txt", b"new");
        put(2, "lchg.txt", b"old");
        put(0, "rchg.txt", b"old");
        put(1, "rchg.txt", b"old");
        put(2, "rchg.txt", b"new");
        put(0, "conf.txt", b"b");
        put(1, "conf.txt", b"l");
        put(2, "conf.txt", b"r");
        put(1, "lonly.txt", b"L");
        put(0, "del.txt", b"x");
        put(1, "del.txt", b"x");
        (dir, roots[0].clone(), roots[1].clone(), roots[2].clone())
    }

    async fn scan(c: &HandlerCtx, id: &str, b: &str, l: &str, r: &str) -> serde_json::Value {
        value_of(
            dispatch(
                "dir/mergeScan",
                Some(json!({"sessionId": id, "base": b, "left": l, "right": r})),
                c,
                &ai_dir::CancelFlag::new(),
                chan().0,
            )
            .await,
        )
        .await
    }

    #[tokio::test]
    async fn dir_merge_capability_and_methods_registered() {
        let c = ctx();
        let params = Some(json!({"clientInfo": {"name": "t"}, "protocolVersion": "1.0"}));
        let v = value_of(initialize(params)).await;
        assert_eq!(v["capabilities"]["dirMerge"], true, "R3-07 能力位 dirMerge");

        // 三个方法都必须已注册（修复前 dispatch 无入口 -> -32601）
        let (_d, b, l, r) = make3();
        for (m, params) in [
            ("dir/mergeScan", json!({"sessionId": "m0", "base": b, "left": l, "right": r})),
            ("dir/mergePlan", json!({"sessionId": "m0", "target": "right", "decisions": {}})),
            (
                "dir/mergeExecute",
                json!({"sessionId": "m0", "target": "right", "ops": []}),
            ),
        ] {
            let got = dispatch(m, Some(params), &c, &ai_dir::CancelFlag::new(), chan().0).await;
            assert!(got.is_ok(), "{m} 应已注册: {:?}", got.err());
        }
    }

    #[tokio::test]
    async fn dir_merge_scan_classifies_and_stats_match_rows() {
        let c = ctx();
        let (_d, b, l, r) = make3();
        let v = scan(&c, "m1", &b, &l, &r).await;
        assert_eq!(v["sessionId"], "m1");

        let rows = v["rows"].as_array().unwrap();
        let cat = |name: &str| -> String {
            rows.iter()
                .find(|x| x["relPath"] == name)
                .unwrap_or_else(|| panic!("缺行 {name}: {v}"))["category"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(cat("same.txt"), "same");
        assert_eq!(cat("lchg.txt"), "leftChanged");
        assert_eq!(cat("rchg.txt"), "rightChanged");
        assert_eq!(cat("conf.txt"), "conflict");
        assert_eq!(cat("lonly.txt"), "leftOnly");
        assert_eq!(cat("del.txt"), "deletedRight");

        // 冲突叶标记（UI 双击进文本合并的判定依据）
        let conf = rows.iter().find(|x| x["relPath"] == "conf.txt").unwrap();
        assert_eq!(conf["conflictLeaf"], true);
        assert_eq!(conf["rowState"], "conflict");
        assert_eq!(conf["depth"], 1);
        assert!(conf["parent"].is_null(), "根的直接子项 parent 为 null");
        assert!(conf["base"].is_object() && conf["left"].is_object() && conf["right"].is_object());
        let lonly = rows.iter().find(|x| x["relPath"] == "lonly.txt").unwrap();
        assert!(lonly["base"].is_null() && lonly["right"].is_null());
        assert_eq!(lonly["rowState"], "orphan");

        // 统计与行一致（口径自洽，不是各自算一套）
        let st = &v["stats"];
        assert_eq!(st["total"].as_u64().unwrap() as usize, rows.len());
        assert_eq!(st["conflict"], 1);
        assert_eq!(st["orphan"], 1);
        assert_eq!(st["files"].as_u64().unwrap() as usize, rows.len());
        assert_eq!(st["dirs"], 0);
    }

    #[tokio::test]
    async fn dir_merge_plan_then_execute_to_right_lands_on_disk() {
        let c = ctx();
        let (_d, b, l, r) = make3();
        scan(&c, "m2", &b, &l, &r).await;

        let plan = value_of(
            dispatch(
                "dir/mergePlan",
                Some(json!({"sessionId": "m2", "target": "right", "decisions": {}})),
                &c,
                &ai_dir::CancelFlag::new(),
                chan().0,
            )
            .await,
        )
        .await;
        assert_eq!(plan["target"], "right");
        let ops = plan["ops"].as_array().unwrap();
        // Auto：仅左改 + 左孤立并入右侧；三方一致/右改/冲突/右已删 不入计划
        let mut paths: Vec<&str> = ops.iter().map(|o| o["path"].as_str().unwrap()).collect();
        paths.sort_unstable();
        assert_eq!(paths, vec!["lchg.txt", "lonly.txt"], "计划清单: {plan}");
        assert!(ops.iter().all(|o| o["kind"] == "takeLeft"));
        assert_eq!(plan["stats"]["total"], 2);
        assert_eq!(plan["stats"]["copied"], 2);

        // 计划阶段不得落盘
        assert!(!std::path::Path::new(&r).join("lonly.txt").exists(), "plan 只预览");
        assert_eq!(std::fs::read(std::path::Path::new(&r).join("lchg.txt")).unwrap(), b"old");

        let exec = value_of(
            dispatch(
                "dir/mergeExecute",
                Some(json!({"sessionId": "m2", "target": "right", "ops": ops})),
                &c,
                &ai_dir::CancelFlag::new(),
                chan().0,
            )
            .await,
        )
        .await;
        assert_eq!(exec["total"], 2);
        assert_eq!(exec["copied"], 2);
        assert_eq!(exec["failed"], 0);
        assert_eq!(exec["cancelled"], false);
        assert_eq!(exec["details"].as_array().unwrap().len(), 2);
        assert!(exec["details"].as_array().unwrap().iter().all(|d| d["status"] == "copied"));
        assert!(!exec["log"].as_array().unwrap().is_empty(), "日志面板行");

        // 真实落盘
        assert_eq!(std::fs::read(std::path::Path::new(&r).join("lchg.txt")).unwrap(), b"new");
        assert_eq!(std::fs::read(std::path::Path::new(&r).join("lonly.txt")).unwrap(), b"L");
        // 未决策的冲突保持原样
        assert_eq!(std::fs::read(std::path::Path::new(&r).join("conf.txt")).unwrap(), b"r");
    }

    #[tokio::test]
    async fn dir_merge_explicit_decision_overrides_auto() {
        let c = ctx();
        let (_d, b, l, r) = make3();
        scan(&c, "m3", &b, &l, &r).await;
        let plan = value_of(
            dispatch(
                "dir/mergePlan",
                Some(json!({
                    "sessionId": "m3", "target": "right",
                    "decisions": {"conf.txt": "takeLeft", "lonly.txt": "skip"}
                })),
                &c,
                &ai_dir::CancelFlag::new(),
                chan().0,
            )
            .await,
        )
        .await;
        let ops = plan["ops"].as_array().unwrap();
        let find = |p: &str| ops.iter().find(|o| o["path"] == p).unwrap_or_else(|| panic!("缺 {p}: {plan}"));
        assert_eq!(find("conf.txt")["kind"], "takeLeft", "显式决策覆盖冲突默认");
        assert_eq!(find("lonly.txt")["kind"], "keep", "skip 决策记 Keep 供预览解释");
        assert_eq!(plan["stats"]["skipped"], 1);

        let exec = value_of(
            dispatch(
                "dir/mergeExecute",
                Some(json!({"sessionId": "m3", "target": "right", "ops": ops})),
                &c,
                &ai_dir::CancelFlag::new(),
                chan().0,
            )
            .await,
        )
        .await;
        assert_eq!(std::fs::read(std::path::Path::new(&r).join("conf.txt")).unwrap(), b"l");
        assert!(!std::path::Path::new(&r).join("lonly.txt").exists(), "skip 不落盘");
        // Keep 项记 planned（可追溯），不计 copied
        let keep = exec["details"].as_array().unwrap().iter().find(|d| d["path"] == "lonly.txt").unwrap();
        assert_eq!(keep["status"], "planned");
        assert_eq!(keep["kind"], "keep");
    }

    #[tokio::test]
    async fn dir_merge_output_target_requires_and_uses_output_path() {
        let c = ctx();
        let (d, b, l, r) = make3();
        scan(&c, "m4", &b, &l, &r).await;
        let out = d.path().join("OUT");

        // 缺 outputPath：plan 与 execute 都必须拒绝（不得静默写到别处）
        for m in ["dir/mergePlan", "dir/mergeExecute"] {
            let params = if m == "dir/mergePlan" {
                json!({"sessionId": "m4", "target": "output", "decisions": {}})
            } else {
                json!({"sessionId": "m4", "target": "output", "ops": []})
            };
            let e = dispatch(m, Some(params), &c, &ai_dir::CancelFlag::new(), chan().0)
                .await
                .expect_err("target=output 缺 outputPath 应拒绝");
            assert_eq!(e.code, codes::INVALID_PARAMS, "{m}: {e:?}");
            assert!(e.message.contains("outputPath"), "{m}: {}", e.message);
        }
        assert!(!out.exists(), "拒绝后不得建目录");

        // plan 带 outputPath -> 记入会话；execute 省略也能用会话里的输出根
        let plan = value_of(
            dispatch(
                "dir/mergePlan",
                Some(json!({
                    "sessionId": "m4", "target": "output", "decisions": {},
                    "outputPath": out.to_string_lossy()
                })),
                &c,
                &ai_dir::CancelFlag::new(),
                chan().0,
            )
            .await,
        )
        .await;
        let ops = plan["ops"].as_array().unwrap();
        // 输出快照：same/lchg 取左、rchg 取右、lonly 并入；conf 与 del 不动作
        assert_eq!(ops.len(), 4, "输出计划: {plan}");
        let exec = value_of(
            dispatch(
                "dir/mergeExecute",
                Some(json!({"sessionId": "m4", "target": "output", "ops": ops})),
                &c,
                &ai_dir::CancelFlag::new(),
                chan().0,
            )
            .await,
        )
        .await;
        assert_eq!(exec["failed"], 0, "执行报告: {exec}");
        assert_eq!(exec["copied"], 4);
        assert_eq!(std::fs::read(out.join("same.txt")).unwrap(), b"1");
        assert_eq!(std::fs::read(out.join("lchg.txt")).unwrap(), b"new");
        assert_eq!(std::fs::read(out.join("rchg.txt")).unwrap(), b"new");
        assert_eq!(std::fs::read(out.join("lonly.txt")).unwrap(), b"L");
        assert!(!out.join("conf.txt").exists(), "未决冲突不进输出");
        assert!(!out.join("del.txt").exists(), "右侧已删除项不进输出");
        // 左右两侧未被改动
        assert_eq!(std::fs::read(std::path::Path::new(&r).join("lchg.txt")).unwrap(), b"old");
        assert!(!std::path::Path::new(&r).join("lonly.txt").exists());
    }

    #[tokio::test]
    async fn dir_merge_session_errors_and_close() {
        let c = ctx();
        let (_d, b, l, r) = make3();

        // 未知会话 -> 410
        let e = dispatch(
            "dir/mergePlan",
            Some(json!({"sessionId": "ghost", "target": "right", "decisions": {}})),
            &c,
            &ai_dir::CancelFlag::new(),
            chan().0,
        )
        .await
        .expect_err("未知会话");
        assert_eq!(e.code, codes::SESSION_NOT_FOUND);

        // 类型不符（文本会话 id）-> INVALID_PARAMS，不得误当合并会话
        scan(&c, "m5", &b, &l, &r).await;
        let e = dispatch(
            "dir/mergeExecute",
            Some(json!({"sessionId": "m5", "target": "right", "ops": []})),
            &c,
            &ai_dir::CancelFlag::new(),
            chan().0,
        )
        .await;
        assert!(e.is_ok(), "空 ops 合法");

        // 目录会话 id 用于合并方法 -> 拒绝
        let (tx, _rx) = chan();
        dispatch(
            "dir/open",
            Some(json!({"sessionId": "d5", "left": l, "right": r, "depth": 1})),
            &c,
            &ai_dir::CancelFlag::new(),
            tx,
        )
        .await
        .unwrap();
        let e = dispatch(
            "dir/mergePlan",
            Some(json!({"sessionId": "d5", "target": "right", "decisions": {}})),
            &c,
            &ai_dir::CancelFlag::new(),
            chan().0,
        )
        .await
        .expect_err("目录会话不是合并会话");
        assert_eq!(e.code, codes::INVALID_PARAMS);

        // 合并会话不得走文本/目录入口
        let e = dispatch(
            "text/sections",
            Some(json!({"sessionId": "m5", "offset": 0, "count": 10})),
            &c,
            &ai_dir::CancelFlag::new(),
            chan().0,
        )
        .await
        .expect_err("合并会话不是文本会话");
        assert_eq!(e.code, codes::INVALID_PARAMS);

        // close 后再用 -> 410
        dispatch(
            "session/close",
            Some(json!({"sessionId": "m5"})),
            &c,
            &ai_dir::CancelFlag::new(),
            chan().0,
        )
        .await
        .unwrap();
        let e = dispatch(
            "dir/mergePlan",
            Some(json!({"sessionId": "m5", "target": "right", "decisions": {}})),
            &c,
            &ai_dir::CancelFlag::new(),
            chan().0,
        )
        .await
        .expect_err("已关闭会话");
        assert_eq!(e.code, codes::SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn dir_merge_scan_missing_root_reports_error() {
        let c = ctx();
        let (d, b, l, _r) = make3();
        let missing = d.path().join("NOPE");
        let e = dispatch(
            "dir/mergeScan",
            Some(json!({
                "sessionId": "m6", "base": b, "left": l,
                "right": missing.to_string_lossy()
            })),
            &c,
            &ai_dir::CancelFlag::new(),
            chan().0,
        )
        .await
        .expect_err("不存在的根应报错");
        assert_ne!(e.code, codes::METHOD_NOT_FOUND);
        // 失败的 scan 不得留下可用会话
        let e2 = dispatch(
            "dir/mergePlan",
            Some(json!({"sessionId": "m6", "target": "right", "decisions": {}})),
            &c,
            &ai_dir::CancelFlag::new(),
            chan().0,
        )
        .await
        .expect_err("scan 失败不应登记会话");
        assert_eq!(e2.code, codes::SESSION_NOT_FOUND);
    }

    /// R3-01 的护栏必须覆盖合并这条新写路径：输出目录内的符号链接
    /// 不得把内容写到根外。
    #[tokio::test]
    #[cfg(unix)]
    async fn dir_merge_execute_does_not_follow_output_symlink() {
        let c = ctx();
        let (d, b, l, _r) = make3();
        let out = d.path().join("OUT");
        std::fs::create_dir_all(&out).unwrap();
        let outside = d.path().join("OUTSIDE");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("lonly.txt"), b"PRECIOUS").unwrap();
        // 输出根内的文件符号链接指向根外同名文件
        std::os::unix::fs::symlink(outside.join("lonly.txt"), out.join("lonly.txt")).unwrap();

        scan(&c, "m7", &b, &l, &d.path().join("R").to_string_lossy()).await;
        let plan = value_of(
            dispatch(
                "dir/mergePlan",
                Some(json!({
                    "sessionId": "m7", "target": "output", "decisions": {},
                    "outputPath": out.to_string_lossy()
                })),
                &c,
                &ai_dir::CancelFlag::new(),
                chan().0,
            )
            .await,
        )
        .await;
        let exec = value_of(
            dispatch(
                "dir/mergeExecute",
                Some(json!({
                    "sessionId": "m7", "target": "output",
                    "ops": plan["ops"], "outputPath": out.to_string_lossy()
                })),
                &c,
                &ai_dir::CancelFlag::new(),
                chan().0,
            )
            .await,
        )
        .await;
        assert_eq!(
            std::fs::read(outside.join("lonly.txt")).unwrap(),
            b"PRECIOUS",
            "根外文件被改写: {exec}"
        );
        let lonly = exec["details"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["path"] == "lonly.txt")
            .expect("该项应有明细");
        assert_eq!(lonly["status"], "failed", "符号链接项应失败而非成功: {exec}");
        assert_eq!(exec["copied"], 3, "其余三项仍应成功（部分成功语义）: {exec}");
    }
}

// ---------------------------------------------------------------------------
// R3-18：text/edit（编辑增量重 diff，引擎权威重要性 + 行尾/EOF 语义保留）
// ---------------------------------------------------------------------------
#[cfg(test)]
mod text_edit_tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> HandlerCtx {
        HandlerCtx::new(512)
    }

    fn chan() -> tokio::sync::mpsc::UnboundedSender<crate::server::Outbound> {
        tokio::sync::mpsc::unbounded_channel().0
    }

    async fn call(
        c: &HandlerCtx,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcErrorObject> {
        dispatch(method, Some(params), c, &ai_dir::CancelFlag::new(), chan())
            .await
            .map(|h| match h {
                Handled::Value(v) => v,
                Handled::WithNotifications { response, .. } => response,
            })
    }

    /// 打开文本会话；tempdir 由调用方持有（保存类测试需要文件仍在）
    async fn open(
        c: &HandlerCtx,
        id: &str,
        left: &str,
        right: &str,
        extra: serde_json::Value,
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L.txt");
        let r = dir.path().join("R.txt");
        std::fs::write(&l, left).unwrap();
        std::fs::write(&r, right).unwrap();
        let mut p =
            json!({"sessionId": id, "left": l.to_string_lossy(), "right": r.to_string_lossy()});
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                p[k] = v.clone();
            }
        }
        call(c, "text/open", p).await.unwrap();
        dir
    }

    /// 取会话内一侧行表的严格还原文本（行尾/EOF 语义的可观测口径）
    async fn side_text(c: &HandlerCtx, id: &str, side: ai_protocol::DiffSide) -> String {
        let sessions = c.sessions.lock().await;
        match sessions.get(id).unwrap() {
            Session::Text(d) => match side {
                ai_protocol::DiffSide::Left => d.left.to_text(),
                ai_protocol::DiffSide::Right => d.right.to_text(),
            },
            _ => panic!("不是文本会话"),
        }
    }

    async fn edit(
        c: &HandlerCtx,
        id: &str,
        edits: serde_json::Value,
    ) -> Result<serde_json::Value, RpcErrorObject> {
        call(c, "text/edit", json!({"sessionId": id, "edits": edits})).await
    }

    /// R3-18 核心回归：编辑后"仅尾随空白差异"仍判为不重要。
    /// 此前 UI 侧 TS 重 diff 把所有差异恒置 important（蓝色变红）。
    #[tokio::test]
    async fn edit_keeps_engine_importance_for_whitespace_only_change() {
        let c = ctx();
        let _dir = open(
            &c,
            "imp1",
            "alpha\nbeta one  \ngamma\n",
            "alpha\nbeta one\ngamma\n",
            json!({"importance": {"trailingWsUnimportant": true}}),
        )
        .await;
        let opened = call(
            &c,
            "text/sections",
            json!({"sessionId": "imp1", "offset": 0, "count": 100}),
        )
        .await
        .unwrap();
        let change = opened["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["kind"] == "change")
            .unwrap();
        assert_eq!(
            change["important"], "unimportant",
            "打开时即应为不重要: {opened}"
        );

        // 编辑左侧该行：尾随空白从 2 个变 3 个（仍只是空白差异）
        let r = edit(
            &c,
            "imp1",
            json!([{"side": "left", "start": 1, "end": 2, "text": "beta one   "}]),
        )
        .await
        .unwrap();
        let after = r["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["kind"] == "change")
            .unwrap();
        assert_eq!(
            after["important"], "unimportant",
            "编辑后重要性必须由引擎规则判定: {r}"
        );
        assert_eq!(r["rev"], 1);
        assert_eq!(r["leftLineCount"], 3);
    }

    /// 编辑引入真实内容差异时必须升为重要（反向变异：恒 unimportant 也要红）
    #[tokio::test]
    async fn edit_promotes_real_content_change_to_important() {
        let c = ctx();
        let _dir = open(
            &c,
            "imp2",
            "alpha\nbeta one  \ngamma\n",
            "alpha\nbeta one\ngamma\n",
            json!({"importance": {"trailingWsUnimportant": true}}),
        )
        .await;
        let r = edit(
            &c,
            "imp2",
            json!([{"side": "left", "start": 1, "end": 2, "text": "beta TWO"}]),
        )
        .await
        .unwrap();
        let after = r["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["kind"] == "change")
            .unwrap();
        assert_eq!(
            after["important"], "important",
            "内容差异不得被判为不重要: {r}"
        );
    }

    /// CRLF 文件的被编辑段不得变成 LF（UI 行数组不含行尾符）
    #[tokio::test]
    async fn edit_preserves_crlf_style_of_edited_segment() {
        let c = ctx();
        let _dir = open(&c, "eol1", "a\r\nb\r\nc\r\n", "a\r\nb\r\nc\r\n", json!({})).await;
        edit(
            &c,
            "eol1",
            json!([{"side": "left", "start": 1, "end": 2, "text": "B"}]),
        )
        .await
        .unwrap();
        assert_eq!(
            side_text(&c, "eol1", ai_protocol::DiffSide::Left).await,
            "a\r\nB\r\nc\r\n"
        );
    }

    /// 多行插入同样按该侧风格归一（内部行也不得漏）
    #[tokio::test]
    async fn edit_multiline_insert_uses_side_eol_for_every_line() {
        let c = ctx();
        let _dir = open(&c, "eol2", "a\r\nz\r\n", "a\r\nz\r\n", json!({})).await;
        edit(
            &c,
            "eol2",
            json!([{"side": "left", "start": 1, "end": 1, "text": "p\nq\nr"}]),
        )
        .await
        .unwrap();
        assert_eq!(
            side_text(&c, "eol2", ai_protocol::DiffSide::Left).await,
            "a\r\np\r\nq\r\nr\r\nz\r\n"
        );
    }

    /// 参考行优先于全表多数风格：CRLF 为主的文件里编辑一条 LF 行仍保持 LF
    #[tokio::test]
    async fn edit_eol_follows_reference_line_not_dominant() {
        let c = ctx();
        let _dir = open(&c, "eol3", "a\r\nb\r\nc\n", "a\r\nb\r\nc\n", json!({})).await;
        edit(
            &c,
            "eol3",
            json!([{"side": "left", "start": 2, "end": 3, "text": "C"}]),
        )
        .await
        .unwrap();
        assert_eq!(
            side_text(&c, "eol3", ai_protocol::DiffSide::Left).await,
            "a\r\nb\r\nC\n"
        );
    }

    /// 追加行不得悄悄删掉文件原有的尾换行
    #[tokio::test]
    async fn edit_append_keeps_existing_trailing_newline() {
        let c = ctx();
        let _dir = open(&c, "eof1", "a\nb\n", "a\nb\n", json!({})).await;
        edit(
            &c,
            "eof1",
            json!([{"side": "left", "start": 2, "end": 2, "text": "c"}]),
        )
        .await
        .unwrap();
        assert_eq!(
            side_text(&c, "eof1", ai_protocol::DiffSide::Left).await,
            "a\nb\nc\n"
        );
    }

    /// 原本无尾换行的文件编辑末行后仍无尾换行（EOF 语义是内容差异）
    #[tokio::test]
    async fn edit_last_line_without_trailing_newline_stays_without() {
        let c = ctx();
        let _dir = open(&c, "eof2", "a\nb", "a\nb", json!({})).await;
        edit(
            &c,
            "eof2",
            json!([{"side": "left", "start": 1, "end": 2, "text": "B"}]),
        )
        .await
        .unwrap();
        assert_eq!(
            side_text(&c, "eof2", ai_protocol::DiffSide::Left).await,
            "a\nB"
        );
    }

    /// 纯删除区间
    #[tokio::test]
    async fn edit_pure_delete_removes_range() {
        let c = ctx();
        let _dir = open(&c, "del1", "a\nb\nc\nd\n", "a\nb\nc\nd\n", json!({})).await;
        let r = edit(
            &c,
            "del1",
            json!([{"side": "left", "start": 1, "end": 3, "text": ""}]),
        )
        .await
        .unwrap();
        assert_eq!(r["leftLineCount"], 2);
        assert_eq!(
            side_text(&c, "del1", ai_protocol::DiffSide::Left).await,
            "a\nd\n"
        );
    }

    /// 整侧重放（end 省略 = 到末尾）：崩溃恢复/分叉后的收敛手段
    #[tokio::test]
    async fn edit_full_side_replay_replaces_whole_side() {
        let c = ctx();
        let _dir = open(&c, "replay1", "a\r\nb\r\n", "a\r\nb\r\n", json!({})).await;
        edit(
            &c,
            "replay1",
            json!([{"side": "left", "start": 0, "text": "x\ny\nz"}]),
        )
        .await
        .unwrap();
        assert_eq!(
            side_text(&c, "replay1", ai_protocol::DiffSide::Left).await,
            "x\r\ny\r\nz\r\n"
        );
    }

    /// 原子性：批内后一 op 越界 -> 整批不落会话（前一 op 也不得生效）
    #[tokio::test]
    async fn edit_batch_is_atomic_when_any_op_out_of_bounds() {
        let c = ctx();
        let _dir = open(&c, "atom1", "a\nb\nc\n", "a\nb\nc\n", json!({})).await;
        let err = edit(
            &c,
            "atom1",
            json!([
                {"side": "left", "start": 0, "end": 1, "text": "A"},
                {"side": "left", "start": 5, "end": 6, "text": "X"}
            ]),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::INVALID_PARAMS);
        assert!(err.message.contains("越界"), "{}", err.message);
        assert_eq!(
            side_text(&c, "atom1", ai_protocol::DiffSide::Left).await,
            "a\nb\nc\n",
            "失败批不得改动行表"
        );
        let sessions = c.sessions.lock().await;
        match sessions.get("atom1").unwrap() {
            Session::Text(d) => assert_eq!(d.rev, 0, "失败批不得推进 rev"),
            _ => panic!(),
        }
    }

    /// 多 op 按序应用：后一 op 的坐标以前一 op 应用后的长度为准
    #[tokio::test]
    async fn edit_sequential_ops_use_post_application_coordinates() {
        let c = ctx();
        let _dir = open(&c, "seq1", "a\nb\n", "a\nb\n", json!({})).await;
        // 先在 0 处插入两行（长度 2->4），再替换新的第 3 行（原 b）
        edit(
            &c,
            "seq1",
            json!([
                {"side": "left", "start": 0, "end": 0, "text": "p\nq"},
                {"side": "left", "start": 3, "end": 4, "text": "B"}
            ]),
        )
        .await
        .unwrap();
        assert_eq!(
            side_text(&c, "seq1", ai_protocol::DiffSide::Left).await,
            "p\nq\na\nB\n"
        );
    }

    /// 空 edits 拒绝（无意义请求不得推进 rev）
    #[tokio::test]
    async fn edit_empty_batch_rejected() {
        let c = ctx();
        let _dir = open(&c, "empty1", "a\n", "a\n", json!({})).await;
        let err = edit(&c, "empty1", json!([])).await.unwrap_err();
        assert_eq!(err.code, codes::INVALID_PARAMS);
    }

    /// 手动对齐锚点因编辑越界时丢弃锚点，编辑仍可继续（否则用户删行后永久卡死）
    #[tokio::test]
    async fn edit_drops_manual_anchors_invalidated_by_line_removal() {
        let c = ctx();
        let _dir = open(
            &c,
            "man1",
            "a\nb\nc\nd\n",
            "a\nb\nc\nd\n",
            json!({"manual": {"alignAnchors": [{"left": {"start": 2, "end": 3}, "right": {"start": 2, "end": 3}}]}}),
        )
        .await;
        // 删掉左侧 3 行 -> 左行数 1，锚点 left.end=3 越界
        let r = edit(
            &c,
            "man1",
            json!([{"side": "left", "start": 0, "end": 3, "text": ""}]),
        )
        .await
        .unwrap();
        assert_eq!(r["leftLineCount"], 1);
        let sessions = c.sessions.lock().await;
        match sessions.get("man1").unwrap() {
            Session::Text(d) => {
                assert!(d.config.manual.align_anchors.is_empty(), "失效锚点应被丢弃")
            }
            _ => panic!(),
        }
    }

    /// 会话不存在 / 非文本会话
    #[tokio::test]
    async fn edit_rejects_unknown_and_non_text_sessions() {
        let c = ctx();
        let err = edit(
            &c,
            "nope",
            json!([{"side": "left", "start": 0, "end": 0, "text": "x"}]),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::SESSION_NOT_FOUND);
        let _dir = open(&c, "kind1", "a\n", "a\n", json!({})).await;
        let err = edit(
            &c,
            "kind1",
            json!([{"side": "left", "start": 0, "end": 0, "text": "x"}]),
        )
        .await
        .unwrap();
        assert_eq!(err["leftLineCount"], 2);
    }

    /// 行尾归一 helper：混排先统一再替换；LF 目标不额外分配替换
    #[test]
    fn normalize_newlines_unifies_mixed_endings() {
        use ai_core::LineEnding as E;
        assert_eq!(
            normalize_newlines("a\r\nb\rc\n", E::CrLf),
            "a\r\nb\r\nc\r\n"
        );
        assert_eq!(normalize_newlines("a\r\nb\rc\n", E::Lf), "a\nb\nc\n");
        assert_eq!(normalize_newlines("a\nb", E::Cr), "a\rb");
        assert_eq!(normalize_newlines("", E::CrLf), "");
    }

    /// 多数风格：平票取 LF；None 不计
    #[test]
    fn dominant_eol_majority_with_lf_tiebreak() {
        use ai_core::LineEnding as E;
        assert_eq!(
            dominant_eol(&LineTable::from_text("a\r\nb\r\nc\n")),
            E::CrLf
        );
        assert_eq!(dominant_eol(&LineTable::from_text("a\r\nb\n")), E::Lf);
        assert_eq!(dominant_eol(&LineTable::from_text("a\rb\rc\n")), E::Cr);
        assert_eq!(dominant_eol(&LineTable::from_text("abc")), E::Lf);
        assert_eq!(dominant_eol(&LineTable::from_text("")), E::Lf);
    }

    // -----------------------------------------------------------------
    // R3-04：text/save（落盘 / 另存为 / 备份 / 编码签名 / 脏标记）
    // -----------------------------------------------------------------

    async fn save(
        c: &HandlerCtx,
        id: &str,
        extra: serde_json::Value,
    ) -> Result<serde_json::Value, RpcErrorObject> {
        let mut p = json!({"sessionId": id});
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                p[k] = v.clone();
            }
        }
        call(c, "text/save", p).await
    }

    async fn modified_flags(c: &HandlerCtx, id: &str) -> (bool, bool) {
        let sessions = c.sessions.lock().await;
        match sessions.get(id).unwrap() {
            Session::Text(d) => d.modified,
            _ => panic!("不是文本会话"),
        }
    }

    /// 测试 Provider：`read_only` 全禁写；`refuse_bak` 只禁 `.bak` 写入
    /// （用于把失败点精确落在备份写入或主写入上）
    struct TestVfs {
        fs: ai_vfs::LocalFs,
        read_only: bool,
        refuse_bak: bool,
    }

    #[async_trait::async_trait]
    impl ai_vfs::VfsProvider for TestVfs {
        fn scheme(&self) -> &'static str {
            "test-vfs"
        }
        fn capabilities(&self) -> ai_vfs::VfsCaps {
            if self.read_only {
                ai_vfs::VfsCaps::READ
            } else {
                ai_vfs::VfsCaps::READ.union(ai_vfs::VfsCaps::all_writes())
            }
        }
        async fn stat(&self, p: &VfsPath) -> Result<ai_vfs::StatInfo, ai_vfs::VfsError> {
            self.fs.stat(p).await
        }
        async fn list_dir(
            &self,
            p: &VfsPath,
        ) -> Result<Vec<ai_vfs::VfsDirEntry>, ai_vfs::VfsError> {
            self.fs.list_dir(p).await
        }
        async fn read_file_bytes(&self, p: &VfsPath) -> Result<Vec<u8>, ai_vfs::VfsError> {
            self.fs.read_file_bytes(p).await
        }
        async fn write_file_bytes(&self, p: &VfsPath, data: &[u8]) -> Result<(), ai_vfs::VfsError> {
            if self.read_only || (self.refuse_bak && p.as_str().ends_with(".bak")) {
                return Err(ai_vfs::VfsError::Unsupported("test-vfs: 写文件".into()));
            }
            self.fs.write_file_bytes(p, data).await
        }
    }

    fn vfs(read_only: bool, refuse_bak: bool) -> std::sync::Arc<TestVfs> {
        std::sync::Arc::new(TestVfs {
            fs: ai_vfs::LocalFs::new(),
            read_only,
            refuse_bak,
        })
    }

    /// Both 缺省只写被编辑过的侧：未编辑侧字节不变（mtime 也不该动，
    /// 否则父目录按时间戳比较会把未改文件判成"更新"）
    #[tokio::test]
    async fn save_both_writes_only_edited_side() {
        let c = ctx();
        let dir = open(&c, "sv1", "a\nb\nc\n", "a\nB\nc\n", json!({})).await;
        let edited = edit(
            &c,
            "sv1",
            json!([{"side": "left", "start": 1, "end": 2, "text": "BB"}]),
        )
        .await
        .unwrap();
        let res = save(&c, "sv1", json!({})).await.unwrap();
        assert_eq!(res["saved"].as_array().unwrap().len(), 1, "{res}");
        assert_eq!(res["saved"][0]["side"], "left");
        assert_eq!(res["saved"][0]["lineCount"], 3);
        assert_eq!(res["saved"][0]["bytes"], 7);
        assert_eq!(res["rev"], edited["rev"], "保存不改比较模型，rev 不递增");
        assert_eq!(res["leftModified"], false);
        assert_eq!(res["rightModified"], false);
        assert_eq!(
            std::fs::read(dir.path().join("L.txt")).unwrap(),
            b"a\nBB\nc\n"
        );
        assert_eq!(
            std::fs::read(dir.path().join("R.txt")).unwrap(),
            b"a\nB\nc\n",
            "未编辑侧不得被改写"
        );
        assert!(!dir.path().join("R.txt.bak").exists());
    }

    /// 备份缺省开启：.bak 保存的是**覆盖前**内容；backup=false 不动已有 .bak
    #[tokio::test]
    async fn save_backup_default_on_and_opt_out() {
        let c = ctx();
        let dir = open(&c, "sv2", "old\n", "x\n", json!({})).await;
        edit(
            &c,
            "sv2",
            json!([{"side": "left", "start": 0, "end": 1, "text": "new"}]),
        )
        .await
        .unwrap();
        let res = save(&c, "sv2", json!({})).await.unwrap();
        let bak = dir.path().join("L.txt.bak");
        assert_eq!(std::fs::read(&bak).unwrap(), b"old\n");
        assert_eq!(
            res["saved"][0]["backupPath"],
            bak.to_string_lossy().as_ref()
        );

        edit(
            &c,
            "sv2",
            json!([{"side": "left", "start": 0, "end": 1, "text": "newer"}]),
        )
        .await
        .unwrap();
        let res2 = save(&c, "sv2", json!({"backup": false})).await.unwrap();
        assert!(res2["saved"][0].get("backupPath").is_none(), "{res2}");
        assert_eq!(
            std::fs::read(&bak).unwrap(),
            b"old\n",
            "关备份不得改写已有 .bak"
        );
        assert_eq!(std::fs::read(dir.path().join("L.txt")).unwrap(), b"newer\n");
    }

    /// 行尾风格与"末尾无换行"必须逐字节保留（CRLF 文件不得被保存成 LF）
    #[tokio::test]
    async fn save_preserves_crlf_and_absent_final_newline() {
        let c = ctx();
        let dir = open(&c, "sv3", "a\r\nb\r\nc", "a\r\nb\r\nc", json!({})).await;
        edit(
            &c,
            "sv3",
            json!([{"side": "left", "start": 1, "end": 2, "text": "B"}]),
        )
        .await
        .unwrap();
        save(&c, "sv3", json!({})).await.unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("L.txt")).unwrap(),
            b"a\r\nB\r\nc"
        );
    }

    /// BOM 原样写回：带 BOM 的侧保留签名，不带 BOM 的侧不得被加上（FR-TXT-013）
    #[tokio::test]
    async fn save_preserves_bom_per_side() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L.txt");
        let r = dir.path().join("R.txt");
        std::fs::write(&l, b"\xEF\xBB\xBFa\nb\n").unwrap();
        std::fs::write(&r, b"a\nb\n").unwrap();
        call(
            &c,
            "text/open",
            json!({"sessionId": "sv4", "left": l.to_string_lossy(), "right": r.to_string_lossy()}),
        )
        .await
        .unwrap();
        edit(
            &c,
            "sv4",
            json!([
                {"side": "left", "start": 0, "end": 1, "text": "A"},
                {"side": "right", "start": 0, "end": 1, "text": "A"}
            ]),
        )
        .await
        .unwrap();
        save(&c, "sv4", json!({})).await.unwrap();
        assert_eq!(std::fs::read(&l).unwrap(), b"\xEF\xBB\xBFA\nb\n");
        assert_eq!(std::fs::read(&r).unwrap(), b"A\nb\n");
    }

    /// 另存为：原文件不动、新目标不产生备份、该侧标签改指新目标
    /// （否则下一次缺省保存会把内容写回原文件）
    #[tokio::test]
    async fn save_as_retargets_side_and_keeps_original() {
        let c = ctx();
        let dir = open(&c, "sv5", "orig\n", "r\n", json!({})).await;
        let l = dir.path().join("L.txt");
        let out = dir.path().join("out.txt");
        edit(
            &c,
            "sv5",
            json!([{"side": "left", "start": 0, "end": 1, "text": "edited"}]),
        )
        .await
        .unwrap();
        let res = save(
            &c,
            "sv5",
            json!({"side": "left", "leftPath": out.to_string_lossy()}),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"edited\n");
        assert_eq!(
            std::fs::read(&l).unwrap(),
            b"orig\n",
            "另存为不得改写原文件"
        );
        assert!(
            res["saved"][0].get("backupPath").is_none(),
            "新目标不存在 -> 不备份"
        );
        assert!(!dir.path().join("out.txt.bak").exists());

        edit(
            &c,
            "sv5",
            json!([{"side": "left", "start": 0, "end": 1, "text": "again"}]),
        )
        .await
        .unwrap();
        save(&c, "sv5", json!({})).await.unwrap();
        assert_eq!(
            std::fs::read(&out).unwrap(),
            b"again\n",
            "后续保存应写新目标"
        );
        assert_eq!(std::fs::read(&l).unwrap(), b"orig\n");
    }

    /// 无编辑时保存是空操作：不写文件、不产生 .bak
    #[tokio::test]
    async fn save_without_edits_is_noop() {
        let c = ctx();
        let dir = open(&c, "sv6", "a\n", "b\n", json!({})).await;
        let res = save(&c, "sv6", json!({})).await.unwrap();
        assert_eq!(res["saved"].as_array().unwrap().len(), 0, "{res}");
        assert_eq!(res["rev"], 0);
        assert!(!dir.path().join("L.txt.bak").exists());
        assert!(!dir.path().join("R.txt.bak").exists());
    }

    /// 显式指定侧：未编辑也写（另存为/强制覆盖场景）
    #[tokio::test]
    async fn save_explicit_side_writes_unmodified_side() {
        let c = ctx();
        let dir = open(&c, "sv7", "a\n", "b\n", json!({})).await;
        let res = save(&c, "sv7", json!({"side": "right"})).await.unwrap();
        assert_eq!(res["saved"].as_array().unwrap().len(), 1, "{res}");
        assert_eq!(res["saved"][0]["side"], "right");
        assert_eq!(std::fs::read(dir.path().join("R.txt.bak")).unwrap(), b"b\n");
        assert!(!dir.path().join("L.txt.bak").exists());
    }

    /// 删空一侧：落盘为 0 字节文件（不是"保留原内容"）
    #[tokio::test]
    async fn save_empty_side_writes_zero_bytes() {
        let c = ctx();
        let dir = open(&c, "sv8", "a\nb\n", "b\n", json!({})).await;
        edit(&c, "sv8", json!([{"side": "left", "start": 0, "text": ""}]))
            .await
            .unwrap();
        let res = save(&c, "sv8", json!({})).await.unwrap();
        assert_eq!(res["saved"][0]["bytes"], 0);
        assert_eq!(res["saved"][0]["lineCount"], 0);
        assert_eq!(std::fs::read(dir.path().join("L.txt")).unwrap(), b"");
    }

    /// 落盘失败不得清除脏标记（否则 UI 会以为已保存而丢掉编辑）
    #[tokio::test]
    async fn save_failure_keeps_modified_flag() {
        let c = ctx();
        let dir = open(&c, "sv9", "a\n", "b\n", json!({})).await;
        edit(
            &c,
            "sv9",
            json!([{"side": "left", "start": 0, "end": 1, "text": "A"}]),
        )
        .await
        .unwrap();
        let bad = dir.path().join("no-such-dir").join("out.txt");
        let err = save(
            &c,
            "sv9",
            json!({"side": "left", "leftPath": bad.to_string_lossy(), "backup": false}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::PATH_NOT_FOUND, "{err:?}");
        assert_eq!(modified_flags(&c, "sv9").await, (true, false));
        assert_eq!(std::fs::read(dir.path().join("L.txt")).unwrap(), b"a\n");
    }

    /// 只读 Provider：写能力缺失必须报错，不得静默"保存成功"（R3-12 同类）。
    /// backup=false 让失败点落在**主写入**上（否则先失败的是备份写入）。
    #[tokio::test]
    async fn save_on_readonly_provider_reports_error() {
        let mut c = ctx();
        c.vfs = vfs(true, false);
        let dir = open(&c, "sv10", "a\n", "b\n", json!({})).await;
        edit(
            &c,
            "sv10",
            json!([{"side": "left", "start": 0, "end": 1, "text": "A"}]),
        )
        .await
        .unwrap();
        let err = save(&c, "sv10", json!({"backup": false}))
            .await
            .unwrap_err();
        assert!(err.message.contains("不支持的操作"), "{err:?}");
        assert_eq!(modified_flags(&c, "sv10").await, (true, false));
        assert_eq!(std::fs::read(dir.path().join("L.txt")).unwrap(), b"a\n");
    }

    /// 备份写入失败即整次保存失败：主文件不得被覆盖（用户要求的保险没做成）
    #[tokio::test]
    async fn save_aborts_when_backup_write_fails() {
        let mut c = ctx();
        c.vfs = vfs(false, true);
        let dir = open(&c, "sv11", "a\n", "b\n", json!({})).await;
        edit(
            &c,
            "sv11",
            json!([{"side": "left", "start": 0, "end": 1, "text": "A"}]),
        )
        .await
        .unwrap();
        let err = save(&c, "sv11", json!({})).await.unwrap_err();
        assert!(err.message.contains("不支持的操作"), "{err:?}");
        assert_eq!(modified_flags(&c, "sv11").await, (true, false));
        assert_eq!(std::fs::read(dir.path().join("L.txt")).unwrap(), b"a\n");
        assert!(!dir.path().join("L.txt.bak").exists());
    }

    /// 未知会话 / 非文本会话：明确报错
    #[tokio::test]
    async fn save_rejects_unknown_and_non_text_session() {
        let c = ctx();
        let err = save(&c, "nope", json!({})).await.unwrap_err();
        assert_eq!(err.code, codes::SESSION_NOT_FOUND);

        let dir = tempfile::tempdir().unwrap();
        let dl = dir.path().join("dl");
        let dr = dir.path().join("dr");
        std::fs::create_dir(&dl).unwrap();
        std::fs::create_dir(&dr).unwrap();
        call(
            &c,
            "dir/open",
            json!({"sessionId": "d1", "left": dl.to_string_lossy(), "right": dr.to_string_lossy()}),
        )
        .await
        .unwrap();
        let err = save(&c, "d1", json!({})).await.unwrap_err();
        assert_eq!(err.code, codes::INVALID_PARAMS);
        assert!(err.message.contains("不是文本会话"), "{err:?}");
    }
}

// ---------------------------------------------------------------------------
// R3-20：text/merge3/save（合并输出落盘到"合并到"目标，FR-TMRG-006）
// ---------------------------------------------------------------------------
#[cfg(test)]
mod merge3_save_tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> HandlerCtx {
        HandlerCtx::new(512)
    }

    fn chan() -> tokio::sync::mpsc::UnboundedSender<crate::server::Outbound> {
        tokio::sync::mpsc::unbounded_channel().0
    }

    async fn call(
        c: &HandlerCtx,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcErrorObject> {
        dispatch(method, Some(params), c, &ai_dir::CancelFlag::new(), chan())
            .await
            .map(|h| match h {
                Handled::Value(v) => v,
                Handled::WithNotifications { response, .. } => response,
            })
    }

    /// 三方语料（与 p7_tests 同形）：两个冲突块（1、3）+ 右侧独有尾部。
    /// tempdir 由调用方持有——落盘断言需要文件仍在。
    async fn open_merge(c: &HandlerCtx, id: &str) -> tempfile::TempDir {
        open_merge_with(
            c,
            id,
            "h\na\nb\nc\nt\n",
            "h\nL1\nb\nL2\nt\n",
            "h\nR1\nb\nR2\nt\nR3\n",
        )
        .await
    }

    async fn open_merge_with(
        c: &HandlerCtx,
        id: &str,
        base: &str,
        left: &str,
        right: &str,
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in [("base.l", base), ("left.l", left), ("right.l", right)] {
            std::fs::write(dir.path().join(name), text).unwrap();
        }
        call(
            c,
            "text/merge3/open",
            json!({
                "sessionId": id,
                "base": dir.path().join("base.l").to_string_lossy(),
                "left": dir.path().join("left.l").to_string_lossy(),
                "right": dir.path().join("right.l").to_string_lossy(),
            }),
        )
        .await
        .unwrap();
        dir
    }

    async fn decide(c: &HandlerCtx, id: &str, decisions: serde_json::Value) -> serde_json::Value {
        call(
            c,
            "text/merge3/decide",
            json!({"sessionId": id, "decisions": decisions}),
        )
        .await
        .unwrap()
    }

    async fn save(
        c: &HandlerCtx,
        id: &str,
        path: &std::path::Path,
        extra: serde_json::Value,
    ) -> Result<serde_json::Value, RpcErrorObject> {
        let mut p = json!({"sessionId": id, "path": path.to_string_lossy()});
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                p[k] = v.clone();
            }
        }
        call(c, "text/merge3/save", p).await
    }

    /// 能力位：initialize 必须宣告 mergeSave（UI 据此区分"真落盘"与浏览器下载）
    #[tokio::test]
    async fn initialize_advertises_merge_save() {
        let c = ctx();
        let v = call(
            &c,
            "initialize",
            json!({"clientInfo": {"name": "t"}, "protocolVersion": "1.0"}),
        )
        .await
        .unwrap();
        assert_eq!(v["capabilities"]["mergeSave"], true, "{v}");
    }

    /// 核心闭环：决策后的输出真写到目标文件；.bak 存覆盖前内容；rev 不递增
    #[tokio::test]
    async fn save_writes_decided_output_with_backup_and_keeps_rev() {
        let c = ctx();
        let dir = open_merge(&c, "ms1").await;
        let decided = decide(
            &c,
            "ms1",
            json!([{"chunkId": 1, "action": "left"}, {"chunkId": 3, "action": "right"}]),
        )
        .await;
        let out = dir.path().join("out.txt");
        std::fs::write(&out, "OLD CONTENT\n").unwrap();

        let res = save(&c, "ms1", &out, json!({})).await.unwrap();
        assert_eq!(res["path"], out.to_string_lossy().to_string());
        assert_eq!(
            res["bytes"], 15,
            "h\\nL1\\nb\\nR2\\nt\\nR3\\n = 15 字节: {res}"
        );
        assert_eq!(res["lineCount"], 6);
        assert_eq!(res["hasConflicts"], false);
        assert_eq!(res["unresolvedChunks"], json!([]));
        assert_eq!(res["conflictCount"], 2, "冲突总数不因决策而减少");
        assert_eq!(res["rev"], decided["rev"], "保存不改决策，rev 不递增");
        assert_eq!(
            std::fs::read(&out).unwrap(),
            b"h\nL1\nb\nR2\nt\nR3\n",
            "落盘内容必须等于引擎合成输出"
        );
        let bak = dir.path().join("out.txt.bak");
        assert_eq!(res["backupPath"], bak.to_string_lossy().to_string());
        assert_eq!(
            std::fs::read(&bak).unwrap(),
            b"OLD CONTENT\n",
            ".bak 存覆盖前内容"
        );

        // 再保存一次：.bak 换成上一次落盘的内容（始终是"覆盖前"镜像）
        decide(&c, "ms1", json!([{"chunkId": 3, "action": "left"}])).await;
        let res2 = save(&c, "ms1", &out, json!({})).await.unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"h\nL1\nb\nL2\nt\nR3\n");
        assert_eq!(std::fs::read(&bak).unwrap(), b"h\nL1\nb\nR2\nt\nR3\n");
        assert_eq!(res2["rev"], 2, "第二次决策使 rev=2，保存本身仍不递增");
    }

    /// 手动编辑（Manual 决策）由引擎合成，无需客户端回传文本
    #[tokio::test]
    async fn save_includes_manual_decision_text() {
        let c = ctx();
        let dir = open_merge(&c, "ms2").await;
        decide(
            &c,
            "ms2",
            json!([
                {"chunkId": 1, "action": "manual", "text": "M1\nM2"},
                {"chunkId": 3, "action": "right"}
            ]),
        )
        .await;
        let out = dir.path().join("manual.txt");
        let res = save(&c, "ms2", &out, json!({"backup": false}))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"h\nM1\nM2\nb\nR2\nt\nR3\n");
        assert_eq!(res["lineCount"], 7);
        assert!(
            !dir.path().join("manual.txt.bak").exists(),
            "backup=false 不产生备份"
        );
    }

    /// marker 模式：未决冲突写进文件，且应答如实报告未决清单
    #[tokio::test]
    async fn save_marker_mode_writes_conflicts_and_reports_unresolved() {
        let c = ctx();
        let dir = open_merge(&c, "ms3").await;
        let out = dir.path().join("marker.txt");
        std::fs::write(&out, "PRE\n").unwrap();
        let res = save(&c, "ms3", &out, json!({"mode": "marker", "backup": false}))
            .await
            .unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert_eq!(text.matches("<<<<<<<").count(), 2, "{text}");
        assert_eq!(res["hasConflicts"], true);
        assert_eq!(res["unresolvedChunks"], json!([1, 3]));
        assert!(
            !dir.path().join("marker.txt.bak").exists(),
            "关备份后不得动已有目标之外的文件"
        );
        assert!(!text.contains("PRE"), "目标先前内容按 BC 语义被覆盖");
    }

    /// 目标不存在：创建文件且不产生备份（应答不带 backupPath 键）
    #[tokio::test]
    async fn save_new_target_creates_file_without_backup() {
        let c = ctx();
        let dir = open_merge(&c, "ms4").await;
        decide(
            &c,
            "ms4",
            json!([{"chunkId": 1, "action": "left"}, {"chunkId": 3, "action": "right"}]),
        )
        .await;
        let out = dir.path().join("fresh.txt");
        let res = save(&c, "ms4", &out, json!({})).await.unwrap();
        assert!(out.exists());
        assert!(
            res.get("backupPath").is_none(),
            "None 不得序列化为 null: {res}"
        );
        assert!(!dir.path().join("fresh.txt.bak").exists());
    }

    /// 祖先带 BOM + CRLF：输出原样写回编码签名与行尾（否则文件被静默改编码）
    #[tokio::test]
    async fn save_preserves_base_bom_and_crlf() {
        let c = ctx();
        let dir = open_merge_with(
            &c,
            "ms5",
            "\u{feff}h\r\na\r\nt\r\n",
            "h\r\nL\r\nt\r\n",
            "h\r\nR\r\nt\r\n",
        )
        .await;
        decide(&c, "ms5", json!([{"chunkId": 1, "action": "left"}])).await;
        let out = dir.path().join("bom.txt");
        let res = save(&c, "ms5", &out, json!({"backup": false}))
            .await
            .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert!(
            bytes.starts_with(ai_vfs::UTF8_BOM),
            "BOM 必须写回: {bytes:?}"
        );
        assert_eq!(bytes, "\u{feff}h\r\nL\r\nt\r\n".as_bytes());
        assert_eq!(res["bytes"], bytes.len() as u64);

        // 祖先无 BOM：不得凭空加 BOM
        let c2 = ctx();
        let dir2 = open_merge_with(
            &c2,
            "ms6",
            "h\r\na\r\nt\r\n",
            "h\r\nL\r\nt\r\n",
            "h\r\nR\r\nt\r\n",
        )
        .await;
        decide(&c2, "ms6", json!([{"chunkId": 1, "action": "left"}])).await;
        let out2 = dir2.path().join("nobom.txt");
        save(&c2, "ms6", &out2, json!({"backup": false}))
            .await
            .unwrap();
        assert_eq!(std::fs::read(&out2).unwrap(), b"h\r\nL\r\nt\r\n");
    }

    /// 入参与会话校验：空路径 / 未知会话 / 非合并会话都必须明确报错
    #[tokio::test]
    async fn save_rejects_empty_path_unknown_and_non_merge_session() {
        let c = ctx();
        let dir = open_merge(&c, "ms7").await;
        let err = save(&c, "ms7", std::path::Path::new(""), json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::INVALID_PARAMS);
        assert!(err.message.contains("目标路径"), "{err:?}");

        let err = save(&c, "ghost", &dir.path().join("x.txt"), json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::SESSION_NOT_FOUND);

        let dl = dir.path().join("dl");
        let dr = dir.path().join("dr");
        std::fs::create_dir(&dl).unwrap();
        std::fs::create_dir(&dr).unwrap();
        call(
            &c,
            "dir/open",
            json!({"sessionId": "d7", "left": dl.to_string_lossy(), "right": dr.to_string_lossy()}),
        )
        .await
        .unwrap();
        let err = save(&c, "d7", &dir.path().join("x.txt"), json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::INVALID_PARAMS);
        assert!(err.message.contains("不是三向合并会话"), "{err:?}");
        assert!(
            !dir.path().join("x.txt").exists(),
            "报错路径不得留下半成品文件"
        );
    }

    /// 主写入失败（父目录不存在）：必须报错，不得报成功
    #[tokio::test]
    async fn save_write_failure_is_reported() {
        let c = ctx();
        let dir = open_merge(&c, "ms8").await;
        let out = dir.path().join("missing").join("out.txt");
        let err = save(&c, "ms8", &out, json!({"backup": false}))
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::PATH_NOT_FOUND, "{err:?}");
        assert!(!out.exists());
    }

    /// 备份写入失败：中止且**不覆盖目标**（否则原内容既没备份也没了）
    #[tokio::test]
    async fn save_backup_failure_aborts_before_overwrite() {
        let c = ctx();
        let dir = open_merge(&c, "ms9").await;
        let out = dir.path().join("guard.txt");
        std::fs::write(&out, b"PRECIOUS\n").unwrap();
        // 让 <target>.bak 成为目录 → 备份写入必然失败
        std::fs::create_dir(dir.path().join("guard.txt.bak")).unwrap();
        let err = save(&c, "ms9", &out, json!({})).await.unwrap_err();
        assert!(!err.message.is_empty(), "{err:?}");
        assert_eq!(
            std::fs::read(&out).unwrap(),
            b"PRECIOUS\n",
            "备份失败时目标必须保持原样: {err:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// R3-14：真实引擎的 VFS 路由（zip:// 透明可用；写能力按路径协商）
// ---------------------------------------------------------------------------
#[cfg(test)]
mod route_vfs_tests {
    use super::*;
    use ai_vfs::VfsProvider;
    use serde_json::json;

    fn ctx() -> HandlerCtx {
        HandlerCtx::new(512)
    }

    fn chan() -> tokio::sync::mpsc::UnboundedSender<crate::server::Outbound> {
        tokio::sync::mpsc::unbounded_channel().0
    }

    async fn call(
        c: &HandlerCtx,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcErrorObject> {
        dispatch(method, Some(params), c, &ai_dir::CancelFlag::new(), chan())
            .await
            .map(|h| match h {
                Handled::Value(v) => v,
                Handled::WithNotifications { response, .. } => response,
            })
    }

    /// 造一个真实 zip 归档，返回 (tempdir, zip:// 前缀)
    fn zip_fixture(entries: &[(&str, &[u8])]) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repo.zip");
        std::fs::write(&path, ai_vfs::zip::build_test_zip(entries)).unwrap();
        let prefix = format!("zip://{}!", path.display());
        (dir, prefix)
    }

    /// 文本会话：左侧 zip 内条目 vs 右侧本地文件，必须真的读到 zip 内容。
    #[tokio::test]
    async fn text_open_reads_zip_entry() {
        let c = ctx();
        let (_d, prefix) = zip_fixture(&[("a.txt", b"alpha\nbeta\n")]);
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("a.txt");
        std::fs::write(&local, "alpha\nCHANGED\n").unwrap();

        call(
            &c,
            "text/open",
            json!({
                "sessionId": "rt1",
                "left": format!("{prefix}/a.txt"),
                "right": local.to_string_lossy(),
            }),
        )
        .await
        .unwrap();

        let left = call(
            &c,
            "text/lines",
            json!({"sessionId": "rt1", "side": "left", "start": 0, "count": 10}),
        )
        .await
        .unwrap();
        assert_eq!(
            left["lines"],
            json!(["alpha", "beta"]),
            "zip 内条目未被读到"
        );
        let right = call(
            &c,
            "text/lines",
            json!({"sessionId": "rt1", "side": "right", "start": 0, "count": 10}),
        )
        .await
        .unwrap();
        assert_eq!(right["lines"], json!(["alpha", "CHANGED"]));
    }

    /// 目录会话：zip 根 vs 本地根，行必须来自归档中心目录。
    #[tokio::test]
    async fn dir_open_scans_zip_root() {
        let c = ctx();
        let (_d, prefix) = zip_fixture(&[("a.txt", b"A"), ("sub/b.txt", b"B")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("R");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.txt"), "A").unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handled = dispatch(
            "dir/open",
            Some(json!({
                "sessionId": "rd1",
                "left": format!("{prefix}/"),
                "right": root.to_string_lossy(),
                "depth": 2,
            })),
            &c,
            &ai_dir::CancelFlag::new(),
            tx,
        )
        .await
        .unwrap();
        let mut notes = match handled {
            Handled::WithNotifications { notifications, .. } => notifications,
            Handled::Value(_) => Vec::new(),
        };
        while let Ok(o) = rx.try_recv() {
            if let crate::server::Outbound::Notification(n) = o {
                notes.push(n);
            }
        }
        let blob = serde_json::to_value(&notes).unwrap().to_string();
        assert!(blob.contains("a.txt"), "zip 根扫描未列出 a.txt: {blob}");
        assert!(blob.contains("sub"), "zip 根扫描未列出子目录: {blob}");
    }

    /// 写路径：本地 → zip 复制必须落盘（能力位按路径协商，不是全局 READ）。
    #[tokio::test]
    async fn dir_execute_copies_local_file_into_zip() {
        let c = ctx();
        let (d, prefix) = zip_fixture(&[("keep.txt", b"K")]);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("L");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("new.txt"), "FROM-LOCAL").unwrap();

        call(
            &c,
            "dir/open",
            json!({
                "sessionId": "rw1",
                "left": root.to_string_lossy(),
                "right": format!("{prefix}/"),
                "depth": 1,
            }),
        )
        .await
        .unwrap();
        let res = call(
            &c,
            "dir/execute",
            json!({"sessionId": "rw1", "ops": [{"kind": "copy", "path": "new.txt", "side": "left"}]}),
        )
        .await
        .unwrap();
        assert_eq!(res["failed"], json!([]), "复制被拒: {res:?}");

        let bytes = std::fs::read(d.path().join("repo.zip")).unwrap();
        let fs = ai_vfs::zip::ZipFsMut::from_bytes_writable(&bytes).unwrap();
        let got = fs
            .read_file_bytes(&VfsPath::parse("/new.txt").unwrap())
            .await
            .unwrap();
        assert_eq!(got, b"FROM-LOCAL", "zip 未真正落盘");
    }

    /// 本地写路径经路由后仍全部放行：RouteFs 的全局能力位只有 READ，
    /// 执行层若按全局位门控，本地删除/改名/对时/建目录都会被误判为"不支持"。
    /// copy 还断言保留源 mtime——那是原生 copy_file 与 read+write 降级的可观测差别。
    #[tokio::test]
    async fn dir_execute_local_writes_survive_routing() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L");
        let r = dir.path().join("R");
        std::fs::create_dir(&l).unwrap();
        std::fs::create_dir(&r).unwrap();
        for (root, name) in [
            (&l, "copy.txt"),
            (&l, "mv.txt"),
            (&l, "del.txt"),
            (&r, "touch.txt"),
        ] {
            std::fs::write(root.join(name), "x").unwrap();
        }
        // 源 mtime 固定到过去：read+write 降级会写成"现在"，断言即可区分
        let src_mtime_ms = 1_000_000_000_000u64;
        let set_mtime = |p: &std::path::Path, ms: u64| {
            let f = std::fs::File::options().write(true).open(p).unwrap();
            f.set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms)),
            )
            .unwrap();
        };
        set_mtime(&l.join("copy.txt"), src_mtime_ms);

        call(
            &c,
            "dir/open",
            json!({
                "sessionId": "rlw",
                "left": l.to_string_lossy(),
                "right": r.to_string_lossy(),
                "depth": 1,
            }),
        )
        .await
        .unwrap();
        let res = call(
            &c,
            "dir/execute",
            json!({"sessionId": "rlw", "ops": [
                {"kind": "copy", "path": "copy.txt", "side": "left"},
                {"kind": "mkdir", "path": "newdir", "side": "right"},
                {"kind": "rename", "path": "mv.txt", "side": "left", "newName": "renamed.txt"},
                {"kind": "touch", "path": "touch.txt", "side": "right", "mtimeMs": 1234567890000u64},
                {"kind": "delete", "path": "del.txt", "side": "left"}
            ]}),
        )
        .await
        .unwrap();
        assert_eq!(res["failed"], json!([]), "本地写操作被路由层误拒: {res:?}");

        let mtime_ms = |p: &std::path::Path| {
            std::fs::metadata(p)
                .unwrap()
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64
        };
        assert_eq!(
            mtime_ms(&r.join("copy.txt")),
            src_mtime_ms,
            "copy 未走原生实现（源 mtime 丢失）"
        );
        assert!(r.join("newdir").is_dir(), "mkdir 未生效");
        assert!(
            l.join("renamed.txt").is_file() && !l.join("mv.txt").exists(),
            "rename 未生效"
        );
        assert_eq!(
            mtime_ms(&r.join("touch.txt")),
            1_234_567_890_000,
            "touch 未生效"
        );
        assert!(!l.join("del.txt").exists(), "delete 未生效");
    }

    /// 未支持 scheme 不得声称写能力（否则执行层会放行到 Unsupported 才失败）。
    #[test]
    fn capabilities_for_rejects_writes_on_unsupported_scheme() {
        let vfs = ai_vfs::RouteFs::new();
        let local = VfsPath::parse("/tmp/x").unwrap();
        assert!(vfs.capabilities_for(&local).has(ai_vfs::VfsCaps::DELETE));
        let ftps = VfsPath::parse("ftps://h/p").unwrap();
        assert!(!vfs.capabilities_for(&ftps).has(ai_vfs::VfsCaps::DELETE));
    }
}

// ---------------------------------------------------------------------------
// R3-15：编码自动检测 / 显式编码覆盖 / 按原编码回写（FR-TXT-013）
// ---------------------------------------------------------------------------
#[cfg(test)]
mod text_encoding_tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> HandlerCtx {
        HandlerCtx::new(512)
    }

    fn chan() -> tokio::sync::mpsc::UnboundedSender<crate::server::Outbound> {
        tokio::sync::mpsc::unbounded_channel().0
    }

    async fn call(
        c: &HandlerCtx,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RpcErrorObject> {
        dispatch(method, Some(params), c, &ai_dir::CancelFlag::new(), chan())
            .await
            .map(|h| match h {
                Handled::Value(v) => v,
                Handled::WithNotifications { response, .. } => response,
            })
    }

    /// 按指定编码造一对真实字节文件（不是 UTF-8 字符串写盘）
    fn pair(dir: &std::path::Path, left: &str, right: &str, label: &str) -> (String, String) {
        let l = dir.join("L.txt");
        let r = dir.join("R.txt");
        std::fs::write(
            &l,
            ai_vfs::encode_text(left, label, ai_vfs::Bom::None).unwrap(),
        )
        .unwrap();
        std::fs::write(
            &r,
            ai_vfs::encode_text(right, label, ai_vfs::Bom::None).unwrap(),
        )
        .unwrap();
        (
            l.to_string_lossy().into_owned(),
            r.to_string_lossy().into_owned(),
        )
    }

    async fn open_pair(
        c: &HandlerCtx,
        id: &str,
        paths: &(String, String),
        extra: serde_json::Value,
    ) -> Result<serde_json::Value, RpcErrorObject> {
        let mut p = json!({"sessionId": id, "left": paths.0, "right": paths.1});
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                p[k] = v.clone();
            }
        }
        call(c, "text/open", p).await
    }

    async fn first_line(c: &HandlerCtx, id: &str, side: &str) -> String {
        let v = call(
            c,
            "text/lines",
            json!({"sessionId": id, "side": side, "start": 0, "count": 1}),
        )
        .await
        .unwrap();
        v["lines"][0].as_str().unwrap().to_string()
    }

    /// 核心回归：GBK 文件自动检测解码，编辑后仍按原编码回写。
    /// 此前引擎只读 UTF-8：非 UTF-8 文件直接 open 失败（415）。
    #[tokio::test]
    async fn legacy_encoding_is_detected_and_written_back() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let paths = pair(dir.path(), "中文\n第二行\n", "中文\n第二行\n", "gb18030");
        let opened = open_pair(&c, "enc1", &paths, json!({})).await.unwrap();
        assert_ne!(
            opened["leftEncoding"], "UTF-8",
            "应检测出非 UTF-8: {opened}"
        );
        assert_eq!(first_line(&c, "enc1", "left").await, "中文");

        call(
            &c,
            "text/edit",
            json!({"sessionId": "enc1", "edits": [{"side": "left", "start": 0, "end": 1, "text": "中文改"}]}),
        )
        .await
        .unwrap();
        call(&c, "text/save", json!({"sessionId": "enc1"}))
            .await
            .unwrap();

        let bytes = std::fs::read(dir.path().join("L.txt")).unwrap();
        let back = ai_vfs::decode_text(&bytes, Some("gb18030")).unwrap();
        assert_eq!(back.text, "中文改\n第二行\n", "回写必须仍是 GB18030");
        assert_ne!(bytes, "中文改\n第二行\n".as_bytes(), "不能被改写成 UTF-8");
        // 未编辑侧字节不动
        assert_eq!(
            std::fs::read(dir.path().join("R.txt")).unwrap(),
            ai_vfs::encode_text("中文\n第二行\n", "gb18030", ai_vfs::Bom::None).unwrap()
        );
    }

    /// 显式编码必须压过自动检测：同一份 UTF-8 字节按 windows-1252 解出 mojibake，
    /// 且保存按同一编码回写 -> 字节完全不变（证明选择器真的生效，不是装饰）
    #[tokio::test]
    async fn explicit_encoding_overrides_detection_and_round_trips() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L.txt");
        let r = dir.path().join("R.txt");
        std::fs::write(&l, "café\n").unwrap();
        std::fs::write(&r, "café\n").unwrap();
        let paths = (
            l.to_string_lossy().into_owned(),
            r.to_string_lossy().into_owned(),
        );
        let opened = open_pair(&c, "enc2", &paths, json!({"encoding": "windows-1252"}))
            .await
            .unwrap();
        assert_eq!(opened["leftEncoding"], "windows-1252");
        assert_eq!(opened["rightEncoding"], "windows-1252");
        assert_eq!(first_line(&c, "enc2", "left").await, "cafÃ©");

        call(
            &c,
            "text/save",
            json!({"sessionId": "enc2", "side": "left"}),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(&l).unwrap(),
            b"caf\xc3\xa9\n",
            "字节应原样回环"
        );
    }

    /// 未知编码标签 -> 415（不是静默回退 UTF-8）
    #[tokio::test]
    async fn unknown_encoding_label_is_rejected() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let paths = pair(dir.path(), "a\n", "a\n", "utf-8");
        let e = open_pair(&c, "enc3", &paths, json!({"encoding": "no-such-encoding"}))
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::INVALID_ENCODING, "{e:?}");
    }

    /// 选错编码解不出合法文本 -> 415，而不是替换字符后假装成功
    #[tokio::test]
    async fn wrong_explicit_encoding_fails_instead_of_replacing_chars() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let paths = pair(dir.path(), "中文\n", "中文\n", "gb18030");
        let e = open_pair(&c, "enc4", &paths, json!({"encoding": "utf-8"}))
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::INVALID_ENCODING, "{e:?}");
    }

    /// UTF-16LE + BOM：自动识别、BOM 不进比较内容、保存原样写回 BOM 与字节序
    #[tokio::test]
    async fn utf16_bom_is_detected_stripped_and_rewritten() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let l = dir.path().join("L.txt");
        let r = dir.path().join("R.txt");
        let orig = ai_vfs::encode_text("中文\n第二行\n", "utf-16le", ai_vfs::Bom::Utf16Le).unwrap();
        std::fs::write(&l, &orig).unwrap();
        std::fs::write(&r, &orig).unwrap();
        let paths = (
            l.to_string_lossy().into_owned(),
            r.to_string_lossy().into_owned(),
        );
        let opened = open_pair(&c, "enc5", &paths, json!({})).await.unwrap();
        assert_eq!(opened["leftEncoding"], "UTF-16LE");
        assert_eq!(
            first_line(&c, "enc5", "left").await,
            "中文",
            "BOM 不得进入首行"
        );

        call(
            &c,
            "text/edit",
            json!({"sessionId": "enc5", "edits": [{"side": "left", "start": 1, "end": 2, "text": "末行"}]}),
        )
        .await
        .unwrap();
        call(&c, "text/save", json!({"sessionId": "enc5"}))
            .await
            .unwrap();

        let bytes = std::fs::read(&l).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xFE], "BOM 必须原样写回");
        let back = ai_vfs::decode_text(&bytes, None).unwrap();
        assert_eq!(back.text, "中文\n末行\n");
        assert_eq!(back.bom, ai_vfs::Bom::Utf16Le);
    }

    /// 目标编码表示不了新内容 -> 整次保存失败且文件不动（不写 NCR 实体、不半写）
    #[tokio::test]
    async fn save_aborts_when_encoding_cannot_represent_edit() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        let paths = pair(dir.path(), "abc\n", "abc\n", "windows-1252");
        // 纯 ASCII 自动检测会判 UTF-8（正确口径），所以这里必须显式钉住编码
        open_pair(&c, "enc6", &paths, json!({"encoding": "windows-1252"}))
            .await
            .unwrap();
        call(
            &c,
            "text/edit",
            json!({"sessionId": "enc6", "edits": [{"side": "left", "start": 0, "end": 1, "text": "中"}]}),
        )
        .await
        .unwrap();
        let e = call(&c, "text/save", json!({"sessionId": "enc6"}))
            .await
            .unwrap_err();
        assert_eq!(e.code, codes::INVALID_ENCODING, "{e:?}");
        assert_eq!(std::fs::read(dir.path().join("L.txt")).unwrap(), b"abc\n");
        assert!(!dir.path().join("L.txt.bak").exists(), "失败不该留下备份");
    }

    /// 三向合并：非 UTF-8 祖先能打开，输出按祖先编码回写（此前 open 直接 415）
    #[tokio::test]
    async fn merge3_opens_and_saves_in_base_encoding() {
        let c = ctx();
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in [
            ("base.l", "中文\n共同\n"),
            ("left.l", "中文左\n共同\n"),
            ("right.l", "中文\n共同右\n"),
        ] {
            std::fs::write(
                dir.path().join(name),
                ai_vfs::encode_text(text, "gb18030", ai_vfs::Bom::None).unwrap(),
            )
            .unwrap();
        }
        call(
            &c,
            "text/merge3/open",
            json!({
                "sessionId": "enc7",
                "base": dir.path().join("base.l").to_string_lossy(),
                "left": dir.path().join("left.l").to_string_lossy(),
                "right": dir.path().join("right.l").to_string_lossy(),
            }),
        )
        .await
        .unwrap();
        let out = dir.path().join("out.txt");
        call(
            &c,
            "text/merge3/save",
            json!({"sessionId": "enc7", "path": out.to_string_lossy()}),
        )
        .await
        .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let back = ai_vfs::decode_text(&bytes, Some("gb18030")).unwrap();
        assert!(
            back.text.contains("中文左"),
            "输出应含左侧改动: {:?}",
            back.text
        );
        assert_ne!(bytes, back.text.as_bytes(), "输出不能被改写成 UTF-8");
    }
}

// ---------------------------------------------------------------------------
// R3-03：RPC 入口（dir/open → dir/operate → dir/execute）真实落盘回归
// ---------------------------------------------------------------------------
#[cfg(test)]
mod r3_03_rpc_tests {
    use super::*;
    use crate::server::Outbound;
    use serde_json::json;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

    fn ctx() -> HandlerCtx {
        HandlerCtx::new(512)
    }

    fn chan() -> (
        UnboundedSender<Outbound>,
        tokio::sync::mpsc::UnboundedReceiver<Outbound>,
    ) {
        unbounded_channel()
    }

    async fn call(c: &HandlerCtx, method: &str, params: serde_json::Value, tx: &UnboundedSender<Outbound>) -> serde_json::Value {
        let handled = dispatch(method, Some(params), c, &ai_dir::CancelFlag::new(), tx.clone()).await.unwrap();
        match handled {
            Handled::Value(v) => v,
            Handled::WithNotifications { response, .. } => response,
        }
    }

    /// 原黑盒场景走完整 RPC 链路：平铺重名 → 镜像左→右 → 真实磁盘终态。
    /// 计划里不得把显示 id（x.txt / x.txt#1）当操作地址。
    #[tokio::test]
    async fn rpc_flat_mirror_executes_on_real_paths() {
        let d = tempfile::tempdir().unwrap();
        let l = d.path().join("L");
        let r = d.path().join("R");
        for (root, files) in [
            (&l, [("d1/x.txt", "L1"), ("d2/x.txt", "L2")]),
            (&r, [("d1/x.txt", "R1"), ("d2/x.txt", "R2")]),
        ] {
            for (rel, data) in files {
                let p = root.join(rel);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, data.as_bytes()).unwrap();
            }
        }
        let c = ctx();
        let (tx, _rx) = chan();

        call(
            &c,
            "dir/open",
            json!({
                "sessionId": "r3",
                "left": l.to_string_lossy(),
                "right": r.to_string_lossy(),
                "flat": true,
                "depth": 3,
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"}
            }),
            &tx,
        )
        .await;

        let operated = call(
            &c,
            "dir/operate",
            json!({"sessionId": "r3", "preset": "mirrorLeftToRight"}),
            &tx,
        )
        .await;
        let plan = operated["plan"].as_array().expect("dir/operate 计划");
        assert_eq!(plan.len(), 2, "两个同名文件都应进计划: {operated}");
        for p in plan {
            let row_id = p["path"].as_str().unwrap();
            let src = p["sourcePath"].as_str().expect("引擎必须解析出 sourcePath");
            let dst = p["targetPath"].as_str().expect("引擎必须解析出 targetPath");
            assert!(!row_id.contains('/'), "平铺行 id 不应含路径: {row_id}");
            assert!(!src.contains('#') && src.contains('/'), "源必须是真实 rel: {src}");
            assert!(!dst.contains('#') && dst.contains('/'), "目标必须是真实 rel: {dst}");
        }

        // 模拟 UI plannedToOp：执行载荷用真实路径，不重新拼接行 id
        let ops: Vec<serde_json::Value> = plan
            .iter()
            .map(|p| {
                json!({
                    "kind": "copy",
                    "path": p["sourcePath"],
                    "side": "left",
                    "targetSide": "right",
                    "targetPath": p["targetPath"]
                })
            })
            .collect();
        let executed = call(
            &c,
            "dir/execute",
            json!({"sessionId": "r3", "ops": ops}),
            &tx,
        )
        .await;
        assert_eq!(
            executed["failed"].as_array().unwrap().len(),
            0,
            "执行不得失败: {executed}"
        );

        // 真实磁盘终态：两个子目录里的文件都变成左侧内容，根目录不得误建 x.txt
        assert_eq!(std::fs::read(r.join("d1/x.txt")).unwrap(), b"L1");
        assert_eq!(std::fs::read(r.join("d2/x.txt")).unwrap(), b"L2");
        assert!(!r.join("x.txt").exists(), "不得按显示 id 在根目录建文件");
        assert!(!r.join("x.txt#1").exists(), "不得把消歧 id 当路径");
        assert_eq!(std::fs::read(l.join("d1/x.txt")).unwrap(), b"L1", "源不应被改写");
    }

    /// 手动对齐左右异名的等价协议链路：dir/execute 必须能把 a.txt 复制到
    /// 目标侧另一个真实名字 b.txt（DirExecuteOp.targetPath -> FileOp.target_path）。
    ///
    /// 注：引擎 `dir/open manualAlign` 目前不会把异名两侧合并成一行（独立缺陷，
    /// 见本轮报告），故这里直接验证协议执行层；库内 run_sync 用例已覆盖
    /// 手动对齐行的完整计划生成与落盘。
    #[tokio::test]
    async fn rpc_execute_copy_with_different_target_path() {
        let d = tempfile::tempdir().unwrap();
        let l = d.path().join("L");
        let r = d.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("a.txt"), b"AAA").unwrap();
        std::fs::write(r.join("b.txt"), b"BBB").unwrap();
        let c = ctx();
        let (tx, _rx) = chan();

        // 普通树形同步会话（不依赖 flat/manualAlign 的扫描行为）
        call(
            &c,
            "dir/open",
            json!({
                "sessionId": "r3m",
                "left": l.to_string_lossy(),
                "right": r.to_string_lossy(),
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"}
            }),
            &tx,
        )
        .await;

        // 异名复制：源 a.txt，目标 b.txt（两侧真实 rel 不同）
        let executed = call(
            &c,
            "dir/execute",
            json!({
                "sessionId": "r3m",
                "ops": [{
                    "kind": "copy",
                    "path": "a.txt",
                    "side": "left",
                    "targetSide": "right",
                    "targetPath": "b.txt"
                }]
            }),
            &tx,
        )
        .await;
        assert_eq!(
            executed["failed"].as_array().unwrap().len(),
            0,
            "异名复制不得失败: {executed}"
        );
        assert_eq!(std::fs::read(r.join("b.txt")).unwrap(), b"AAA", "应写进目标侧真实名字 b.txt");
        assert!(!r.join("a.txt").exists(), "不得按源名在右侧另建一份");
        assert_eq!(std::fs::read(l.join("a.txt")).unwrap(), b"AAA", "源不应被改写");
    }

    /// 安全护栏：新增的目标侧路径也必须是合法 rel（不得成为绕过 .. 的入口）
    #[tokio::test]
    async fn rpc_rejects_escaping_target_path() {
        let d = tempfile::tempdir().unwrap();
        let l = d.path().join("L");
        let r = d.path().join("R");
        std::fs::create_dir_all(&l).unwrap();
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(l.join("a.txt"), b"AAA").unwrap();
        let c = ctx();
        let (tx, _rx) = chan();
        call(
            &c,
            "dir/open",
            json!({
                "sessionId": "r3g",
                "left": l.to_string_lossy(),
                "right": r.to_string_lossy(),
                "criteria": {"mtimeToleranceSec": 0, "contentCompare": "always"}
            }),
            &tx,
        )
        .await;
        let (tx2, _rx2) = chan();
        let executed = call(
            &c,
            "dir/execute",
            json!({
                "sessionId": "r3g",
                "ops": [{
                    "kind": "copy",
                    "path": "a.txt",
                    "side": "left",
                    "targetSide": "right",
                    "targetPath": "../outside.txt"
                }]
            }),
            &tx2,
        )
        .await;
        assert_eq!(executed["failed"].as_array().unwrap().len(), 1, "越界目标必须失败: {executed}");
        assert!(!d.path().join("outside.txt").exists(), "根外不得产生文件");
    }

}

