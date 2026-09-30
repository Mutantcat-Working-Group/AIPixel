//! Tauri 命令层：把 agent-core 的主循环与 pixel-core 的文档能力暴露给前端。
//! 事件统一走 `agent-event` 通道，载荷是带 session_id 的 `AgentEventEnvelope`，
//! 里面的 `AgentEvent` 自带 kind tag，前端按 kind 分派。

use agent_core::{
    ActiveContext, AgentEvent, AgentEventEnvelope, AgentSession, ApprovalDecision, Attachment,
    AttachmentRole, Capabilities, ImageSupport, LoopLimits, Message, ModelConfig, ModelRole,
    PermissionMode, Protocol,
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

/// 运行护栏当前值：续写、重试、纯思考各自封顶。
#[tauri::command]
pub fn agent_loop_limits(state: State<'_, AppState>) -> LoopLimits {
    state.limits()
}

/// 改运行护栏。立即推到所有活着的会话，并落盘供下次启动读回。
#[tauri::command]
pub fn agent_set_loop_limits(state: State<'_, AppState>, limits: LoopLimits) -> LoopLimits {
    state.set_limits(limits);
    state.limits()
}

/// MCP 总开关当前值。true = 模型看得见用户自配的外部工具。
#[tauri::command]
pub fn agent_mcp_enabled(state: State<'_, AppState>) -> bool {
    state.mcp_enabled()
}

/// 开/关 MCP。当场摘掉或挂回所有活会话的注册表，并落盘。
#[tauri::command]
pub fn agent_set_mcp_enabled(state: State<'_, AppState>, enabled: bool) -> bool {
    state.set_mcp_enabled(enabled);
    state.mcp_enabled()
}

/// 拉 provider 的模型清单，供设置里「获取」后挑一个填入。
/// api_key 留空表示沿用本机已存密钥：改已有定义时用户不用把密钥再贴一遍。
#[tauri::command]
pub async fn model_fetch_models(
    state: State<'_, AppState>,
    id: Option<String>,
    base_url: String,
    api_key: String,
    protocol: Protocol,
) -> Result<Vec<String>, String> {
    let config = ModelConfig {
        id: id.as_deref().unwrap_or("fetch").to_string(),
        label: String::new(),
        protocol,
        base_url,
        api_key: probe_api_key(&state, id.as_deref(), &api_key),
        model: String::new(),
        max_tokens: None,
        temperature: None,
        disable_thinking: None,
        capabilities: Capabilities::default(),
    };
    agent_core::providers::list_models(&config)
        .await
        .map_err(|e| e.to_string())
}

/// 探测这个模型能不能出图，供设置里「探测」按钮用。
///
/// 只回结论，不改设置：能力是用户声明的，探测只是份建议，勾不勾由人决定。
/// api_key 留空时和「获取」一样，沿用这个模型定义已存的密钥。
#[tauri::command]
pub async fn model_probe_image(
    state: State<'_, AppState>,
    id: Option<String>,
    base_url: String,
    api_key: String,
    protocol: Protocol,
    model: String,
) -> Result<ImageSupport, String> {
    let config = ModelConfig {
        id: id.as_deref().unwrap_or("probe").to_string(),
        label: String::new(),
        protocol,
        base_url,
        api_key: probe_api_key(&state, id.as_deref(), &api_key),
        model,
        max_tokens: None,
        temperature: None,
        disable_thinking: None,
        capabilities: Capabilities::default(),
    };
    Ok(agent_core::probe_image_support(&config).await)
}

/// 表单里密钥留空时，退回这个模型定义已存的那把。
fn probe_api_key(state: &AppState, id: Option<&str>, api_key: &str) -> String {
    if api_key.trim().is_empty() {
        id.and_then(|id| state.stored_api_key(id))
            .unwrap_or_default()
    } else {
        api_key.to_string()
    }
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
    /// 四个角色各自在干活的模型。没分工就是主模型，前端据此显示「谁负责哪段」。
    pub roles: Vec<agent_core::RoleBinding>,
    pub width: u32,
    pub height: u32,
    pub revision: u64,
    /// 用户改过的显示名；None = 拿默认编号显示。
    pub title: Option<String>,
    /// 侧边栏排序位。拖动排序后整批改写。
    pub order: u64,
}

fn session_info(session: &AgentSession) -> SessionInfo {
    let config = session.model_config();
    let doc = session.document();
    SessionInfo {
        id: session.id().to_string(),
        model_id: config.id,
        model_label: config.label,
        roles: session.role_bindings(),
        width: doc.width,
        height: doc.height,
        revision: doc.revision,
        title: session.title(),
        order: session.order(),
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
    let mut infos: Vec<SessionInfo> = ids
        .into_iter()
        .filter_map(|id| state.session(&id).ok())
        .map(|session| session_info(&session))
        .collect();
    // 按排序位排，不按 id 字典序排：用户拖动过的顺序必须原样端出来。
    // 同值时用 id 兜底，排序仍然是确定的。
    infos.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    infos
}

/// 改侧边栏显示名。空白名当取消：不留一个看不见的会话标题。
#[tauri::command]
pub fn session_rename(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> Result<SessionInfo, String> {
    let session = state.session(&id)?;
    session.set_title(Some(title));
    Ok(session_info(&session))
}

/// 拖动排序：按新次序整批改写排序位。id 不在簿里就当没发生过。
#[tauri::command]
pub fn session_reorder(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    for (index, id) in ids.iter().enumerate() {
        let Ok(session) = state.session(id) else {
            continue;
        };
        // 从 1 起排：0 留给还没落位的会话。
        session.set_order(index as u64 + 1);
    }
    Ok(())
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

/// 给某个角色另绑一个模型。生图 / 识图 / 读视频各能配一个和会话主模型不同的，
/// 这样用户不用为了跑一次生图把整个会话改绑过去再改回来。
/// 主模型不允许从这里改：那是 `session_bind_model` 的活。
#[tauri::command]
pub fn session_bind_role(
    state: State<'_, AppState>,
    id: String,
    role: ModelRole,
    model_id: String,
) -> Result<SessionInfo, String> {
    let session = state.session(&id)?;
    if !role.is_detachable() {
        return Err("the chat role is the session model itself; rebind the session instead".into());
    }
    let config = state.model_config(&model_id)?;
    session.rebind_role(role, config);
    Ok(session_info(&session))
}

/// 取消某个角色的单独绑定，让它回落去用会话主模型。
#[tauri::command]
pub fn session_clear_role(
    state: State<'_, AppState>,
    id: String,
    role: ModelRole,
) -> Result<SessionInfo, String> {
    let session = state.session(&id)?;
    if !role.is_detachable() {
        return Err("the chat role is the session model itself; rebind the session instead".into());
    }
    session.clear_role(role);
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
    // 会话标识跟着事件一起走：用户切走之后，上一个还没停干净的回合不能把
    // 它的 token 和 document_updated 落到新会话的对话与画布上。
    let owner = id.clone();
    // 同步命令跑在主线程（WebView 的 IPC 回调线程），那里没有 tokio 运行时上下文，
    // 直接 tokio::spawn 会 panic 并把整个进程带崩；必须走 Tauri 自己的异步运行时。
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            let _ = forwarder.emit(
                "agent-event",
                AgentEventEnvelope {
                    session_id: owner.clone(),
                    event,
                },
            );
        }
    });
    // 主循环跑在独立任务里，命令拿到的是「已受理」而非「已跑完」。
    tauri::async_runtime::spawn(async move {
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
/// `ase`/`aseprite` 写 Aseprite 文件（图层+帧语义原样保留），`frame` 写单帧 PNG
/// （`frame` 参数即帧索引，缺省第一帧），`strip` 把所有帧横向铺成一张 PNG。
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
            // 单帧导出：点名哪一帧就合哪一帧，没点名就合第一帧。
            let img = pixel_core::png::composite_frame(&doc, frame.unwrap_or(0));
            pixel_core::png::encode_png(&img)?
        }
        // 横向整条：所有帧并排贴成一张，逐帧动画的接缝一眼能看完。
        "strip" => {
            let img = pixel_core::png::flatten(&doc);
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

#[cfg(test)]
mod tests {
    use std::panic::AssertUnwindSafe;
    use std::sync::mpsc;
    use std::time::Duration;

    /// 回归：同步命令的执行线程（macOS 上是 WebView 的 IPC 回调线程）没有 tokio
    /// 运行时上下文，裸 `tokio::spawn` 会直接 panic 并把进程带崩 —— 用户一按回车
    /// 应用就消失。所以 agent_send_message / batch_run 一律走 tauri::async_runtime。
    #[test]
    fn spawning_off_the_runtime_context() {
        let (tx, rx) = mpsc::sync_channel(1);
        let panicked = std::panic::catch_unwind(AssertUnwindSafe(|| {
            tokio::spawn(async move {
                let _ = tx.send("bare tokio::spawn ran");
            });
        }))
        .is_err();
        assert!(
            panicked,
            "bare tokio::spawn must refuse to run off a runtime context"
        );
        assert!(rx.try_recv().is_err(), "nothing should have been scheduled");

        let (tx, rx) = mpsc::sync_channel(1);
        tauri::async_runtime::spawn(async move {
            let _ = tx.send("tauri::async_runtime::spawn ran");
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10))
                .expect("task must run"),
            "tauri::async_runtime::spawn ran"
        );
    }
}
