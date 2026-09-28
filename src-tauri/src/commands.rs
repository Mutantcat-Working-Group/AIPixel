//! Tauri 命令层：把 agent-core 的主循环与 pixel-core 的文档能力暴露给前端。
//! 事件统一走 `agent-event` 通道，`AgentEvent` 自带 kind tag，前端按 kind 分派。

use agent_core::{
    ActiveContext, AgentEvent, AgentSession, ApprovalDecision, Attachment, AttachmentRole, Message,
    ModelConfig, PermissionMode,
};
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

use crate::state::{default_document, AppState, ModelsView};

/// 模型清单。api_key 一律不回传 webview，只给 has_api_key。
#[tauri::command]
pub fn agent_list_models(state: State<'_, AppState>) -> ModelsView {
    state.models_view()
}

/// 新增/更新一个模型。api_key 留空表示沿用本机已存密钥。
#[tauri::command]
pub fn model_upsert(state: State<'_, AppState>, config: ModelConfig) -> Result<ModelsView, String> {
    if config.id.trim().is_empty() {
        return Err("model id must not be empty".into());
    }
    if config.model.trim().is_empty() {
        return Err("model name must not be empty".into());
    }
    if config.base_url.trim().is_empty() {
        return Err("base_url must not be empty".into());
    }
    let previous_key = if config.api_key.trim().is_empty() {
        state.stored_api_key(&config.id)
    } else {
        None
    };
    state.upsert_model(config, previous_key);
    Ok(state.models_view())
}

#[tauri::command]
pub fn model_remove(state: State<'_, AppState>, id: String) -> Result<ModelsView, String> {
    state.remove_model(&id);
    Ok(state.models_view())
}

#[tauri::command]
pub fn model_set_active(state: State<'_, AppState>, id: String) -> Result<ModelsView, String> {
    state.set_active_model(&id)?;
    Ok(state.models_view())
}

/// 会话概览，用于侧边会话列表。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub model_id: String,
    pub model_label: String,
    pub width: u32,
    pub height: u32,
    pub revision: u64,
}

fn session_info(session: &AgentSession) -> SessionInfo {
    let config = session.model_config();
    let doc = session.document();
    SessionInfo {
        id: session.id().to_string(),
        model_id: config.id,
        model_label: config.label,
        width: doc.width,
        height: doc.height,
        revision: doc.revision,
    }
}

/// 建一个会话；`document` 可带初始文档 JSON，缺省 64x64 空白画布。
#[tauri::command]
pub fn session_create(
    state: State<'_, AppState>,
    document: Option<Value>,
) -> Result<SessionInfo, String> {
    let doc = match document {
        Some(value) => serde_json::from_value::<pixel_core::document::Document>(value)
            .map_err(|e| format!("invalid document: {e}"))?,
        None => default_document(),
    };
    let session = state.create_session(doc);
    Ok(session_info(&session))
}

#[tauri::command]
pub fn session_list(state: State<'_, AppState>) -> Vec<SessionInfo> {
    let mut ids = state.session_ids();
    ids.sort();
    ids.iter()
        .filter_map(|id| state.session(id).ok())
        .map(|s| session_info(&s))
        .collect()
}

#[tauri::command]
pub fn session_drop(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.session(&id)?;
    state.drop_session(&id);
    Ok(())
}

/// 把会话改绑到另一个模型（保留文档与历史消息；只重建 provider）。
#[tauri::command]
pub fn session_bind_model(
    state: State<'_, AppState>,
    id: String,
    model_id: String,
) -> Result<SessionInfo, String> {
    let session = state.session(&id)?;
    let config = state.model_config(&model_id)?;
    session.rebind_provider(config);
    Ok(session_info(&session))
}

/// 切换会话的激活图层/帧/笔刷颜色，进系统提示词与工具默认值。
#[tauri::command]
pub fn agent_set_active(
    state: State<'_, AppState>,
    id: String,
    active: ActiveContext,
) -> Result<(), String> {
    state.session(&id)?.set_active(active);
    Ok(())
}

/// 权限模式：Auto 直接执行；Chat/Ask 先行提示（工作台阶段接审批交互）。
#[tauri::command]
pub fn agent_set_permission(
    state: State<'_, AppState>,
    id: String,
    permission: PermissionMode,
) -> Result<(), String> {
    state.session(&id)?.set_permission(permission);
    Ok(())
}

/// 发一条消息并跑一个 agent turn。命令立即返回，过程事件经 `agent-event` 广播。
#[tauri::command]
pub fn agent_send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    text: String,
    attachments: Option<Vec<Attachment>>,
    model_id: Option<String>,
) -> Result<(), String> {
    let session = state.session(&id)?;
    if let Some(model_id) = model_id.filter(|m| !m.trim().is_empty()) {
        let config = state.model_config(&model_id)?;
        session.rebind_provider(config);
    }
    let attachments = attachments.unwrap_or_default();

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    // 转发任务：把主循环事件搬上 Tauri 事件总线，流式期间不阻塞 UI。
    let forwarder = app.clone();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = forwarder.emit("agent-event", event);
        }
    });
    // 主循环跑在独立任务里，命令拿到的是「已受理」而非「已跑完」。
    tokio::spawn(async move {
        session.run_turn(text, attachments, tx).await;
    });
    Ok(())
}

/// 中断当前 turn：流式轮询 120ms 内收尾，回一条 Interrupted 事件。
#[tauri::command]
pub fn agent_interrupt(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.session(&id)?.interrupt();
    Ok(())
}

/// 用户对一条挂起的工具调用给出决定。call_id 对不上（新一轮已经开始）会报错，
/// 前端的审批卡片据此自己收起来，不会把一次过期点击当成放行。
#[tauri::command]
pub fn agent_resolve_approval(
    state: State<'_, AppState>,
    id: String,
    call_id: String,
    decision: ApprovalDecision,
) -> Result<(), String> {
    state.session(&id)?.resolve_approval(&call_id, decision)
}

/// 前端整体同步文档（打开 .aip、撤销、或工作台编辑后回灌），返回新 revision。
#[tauri::command]
pub fn agent_sync_document(
    state: State<'_, AppState>,
    id: String,
    document: Value,
) -> Result<Value, String> {
    let session = state.session(&id)?;
    let doc = serde_json::from_value::<pixel_core::document::Document>(document)
        .map_err(|e| format!("invalid document: {e}"))?;
    session.sync_document(doc);
    Ok(serde_json::json!({ "revision": session.revision() }))
}

/// 文档快照：revision + 文档 JSON（文本网格是权威状态）。
#[tauri::command]
pub fn agent_document(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    let session = state.session(&id)?;
    Ok(serde_json::json!({
        "id": session.id(),
        "revision": session.revision(),
        "document": session.document_json(),
    }))
}

/// 文档合成的 PNG data URL，画布预览与导出共用。
#[tauri::command]
pub fn document_png_url(
    state: State<'_, AppState>,
    id: String,
    frame: Option<u32>,
) -> Result<String, String> {
    let session = state.session(&id)?;
    let doc = session.document();
    let img = match frame {
        Some(index) => pixel_core::png::composite_frame(&doc, index),
        None => pixel_core::png::flatten(&doc),
    };
    let bytes = pixel_core::png::encode_png(&img)?;
    Ok(format!(
        "data:image/png;base64,{}",
        pixel_core::png::base64_encode(&bytes)
    ))
}

/// 把整个动画导出到磁盘：`gif` 走无限循环动画，`sheet` 走 PNG spritesheet。
/// `ase`/`aseprite` 写 Aseprite 文件（图层+帧语义原样保留），`frame` 写当前帧 PNG
/// （`frame` 参数即帧索引，缺省铺平整幅）。
/// `columns` 为 0 或 None 时 spritesheet 排成一行。
///
/// 用户选什么扩展名就写什么字节，引擎和素材库各取所需，不做二次确认。
#[tauri::command]
pub fn document_export(
    state: State<'_, AppState>,
    id: String,
    format: String,
    path: String,
    columns: Option<u32>,
    frame: Option<u32>,
) -> Result<(), String> {
    let doc = state.session(&id)?.document();
    let bytes = match format.as_str() {
        "gif" => pixel_core::sheet::encode_gif(&doc)?,
        "ase" | "aseprite" => pixel_core::ase::encode_ase(&doc)?,
        "frame" => {
            let img = match frame {
                Some(index) => pixel_core::png::composite_frame(&doc, index),
                None => pixel_core::png::flatten(&doc),
            };
            pixel_core::png::encode_png(&img)?
        }
        "sheet" => {
            let img = pixel_core::sheet::spritesheet(&doc, columns.unwrap_or(0));
            pixel_core::png::encode_png(&img)?
        }
        other => return Err(format!("unsupported export format: {other}")),
    };
    std::fs::write(&path, &bytes).map_err(|e| format!("cannot write {path}: {e}"))
}

/// 读一张参考图，转成 send_message 可用的附件（角色为 reference）。
#[tauri::command]
pub fn read_image_context(path: String) -> Result<Attachment, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let media_type = match std::path::Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    };
    Ok(Attachment {
        role: AttachmentRole::Reference,
        media_type: media_type.to_string(),
        data_base64: pixel_core::png::base64_encode(&bytes),
    })
}

/// 会话历史：切回会话时前端用它重建对话视图（文档仍以 agent_document 为准）。
#[tauri::command]
pub fn agent_history(state: State<'_, AppState>, id: String) -> Result<Vec<Message>, String> {
    Ok(state.session(&id)?.history())
}

/// 当前文档的 .aip v2 文本，不落盘。文本网格是权威状态，前端直接渲染给用户看。
#[tauri::command]
pub fn aip_text(state: State<'_, AppState>, id: String) -> Result<String, String> {
    state.session(&id)?.aip_text()
}

/// 当前文档另存为 .aip v2 文本，同时把文本回给前端做兜底。
#[tauri::command]
pub fn aip_save(state: State<'_, AppState>, id: String, path: String) -> Result<String, String> {
    let session = state.session(&id)?;
    let text = session.aip_text()?;
    std::fs::write(&path, text.as_bytes()).map_err(|e| format!("cannot write {path}: {e}"))?;
    Ok(text)
}

/// 打开 .aip（v2 或 v1 遗留）并解析成文档 JSON，交给 agent_sync_document 回灌。
#[tauri::command]
pub fn aip_load(path: String) -> Result<Value, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let doc = pixel_core::aip::import_any(&text).map_err(|e| e.to_string())?;
    serde_json::to_value(&doc).map_err(|e| e.to_string())
}
