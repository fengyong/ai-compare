//! RPC 方法实现：initialize、dir/open、text/open、text/sections、session/close。
//!
//! 输出顺序契约：handler 返回 [`Handled`]（应答 + 后续通知序列），
//! server 按序入队 —— 同一请求的响应先于其通知，各请求间输出不交错（串行链）。

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
) -> Result<Handled, RpcErrorObject> {
    match method {
        ai_protocol::method::INITIALIZE => initialize(),
        ai_protocol::method::DIR_OPEN => dir_open(params, ctx, cancel).await,
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
        },
        license: ai_protocol::LicenseCapabilities { pro_unlocked: true },
    };
    Ok(Handled::Value(serde_json::to_value(&result).expect("initialize 结果可序列化")))
}

/// dir/open：同步扫描（P0）-> 应答（含终态统计）-> dir/delta 通知分批推送
async fn dir_open(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
    cancel: &ai_dir::CancelFlag,
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
            Session::Dir(DirSessionData { scan: scan_result.clone() }),
        );
    }

    // 应答先于 delta 批次（docs/03 §5.1 时序：result 之后推 delta）
    let response = ai_protocol::DirOpenResult {
        session_id: params.session_id.clone(),
        row_count,
        rev,
        scan_state: ai_protocol::ScanState::Completed,
    };
    let mut notifications = Vec::with_capacity(batches.len());
    for (i, b) in batches.into_iter().enumerate() {
        let delta = batch::make_delta(
            &params.session_id,
            i as u64 + 1,
            &b,
            scan_result.stats,
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

/// session/close：幂等关闭（不存在也返回成功）
async fn session_close(
    params: Option<serde_json::Value>,
    ctx: &HandlerCtx,
) -> Result<Handled, RpcErrorObject> {
    let params: ai_protocol::SessionCloseParams = parse_params(params)?;
    let mut sessions = ctx.sessions.lock().await;
    sessions.remove(&params.session_id);
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
