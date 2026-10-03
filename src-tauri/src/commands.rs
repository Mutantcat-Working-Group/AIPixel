// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! Tauri 命令层：把 agent-core 的主循环与 pixel-core 的文档能力暴露给前端。
//! 事件统一走 `agent-event` 通道，载荷是带 session_id 的 `AgentEventEnvelope`，
//! 里面的 `AgentEvent` 自带 kind tag，前端按 kind 分派。

use agent_core::{
    pins, ActiveContext, AgentEvent, AgentEventEnvelope, AgentSession, ApprovalDecision,
    Attachment, AttachmentRole, Capabilities, ImageSupport, LoopLimits, Message, ModelConfig,
    ModelRole, PermissionMode, Protocol,
};
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

use pixel_core::document::Document;

use crate::state::{default_document, AppState, ModelsView};
use std::sync::Arc;

/// 模型清单。api_key 一律不回传 webview，只给 has_api_key。
#[tauri::command]
pub fn agent_list_models(state: State<'_, Arc<AppState>>) -> ModelsView {
    state.models_view()
}

/// 运行护栏当前值：续写、重试、纯思考各自封顶。
#[tauri::command]
pub fn agent_loop_limits(state: State<'_, Arc<AppState>>) -> LoopLimits {
    state.limits()
}

/// 改运行护栏。立即推到所有活着的会话，并落盘供下次启动读回。
#[tauri::command]
pub fn agent_set_loop_limits(state: State<'_, Arc<AppState>>, limits: LoopLimits) -> LoopLimits {
    state.set_limits(limits);
    state.limits()
}

/// MCP 总开关当前值。true = 模型看得见用户自配的外部工具。
#[tauri::command]
pub fn agent_mcp_enabled(state: State<'_, Arc<AppState>>) -> bool {
    state.mcp_enabled()
}

/// 开/关 MCP。当场摘掉或挂回所有活会话的注册表，并落盘。
#[tauri::command]
pub fn agent_set_mcp_enabled(state: State<'_, Arc<AppState>>, enabled: bool) -> bool {
    state.set_mcp_enabled(enabled);
    state.mcp_enabled()
}

/// 拉 provider 的模型清单，供设置里「获取」后挑一个填入。
/// api_key 留空表示沿用本机已存密钥：改已有定义时用户不用把密钥再贴一遍。
#[tauri::command]
pub async fn model_fetch_models(
    state: State<'_, Arc<AppState>>,
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
    state: State<'_, Arc<AppState>>,
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
pub fn model_upsert(
    state: State<'_, Arc<AppState>>,
    config: ModelConfig,
) -> Result<ModelsView, String> {
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
pub fn model_remove(state: State<'_, Arc<AppState>>, id: String) -> Result<ModelsView, String> {
    state.remove_model(&id);
    Ok(state.models_view())
}

#[tauri::command]
pub fn model_set_active(state: State<'_, Arc<AppState>>, id: String) -> Result<ModelsView, String> {
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

/// 会话 -> 前端视图。前后端契约的唯一出口：Arc、文档原文、api_key
/// 都在这一层截断，后端形状再怎么变，前端拿到的一直是这份扁平结构。
pub(crate) fn session_info(session: &AgentSession) -> SessionInfo {
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
    state: State<'_, Arc<AppState>>,
    document: Option<Value>,
    title: Option<String>,
) -> Result<SessionInfo, String> {
    let doc = match document {
        Some(value) => serde_json::from_value::<pixel_core::document::Document>(value)
            .map_err(|e| format!("invalid document: {e}"))?,
        None => default_document(),
    };
    // 前端传什么都能进来，宽高不校验的话后面一次 width*height 分配就能打死进程。
    let doc = doc
        .validated()
        .map_err(|e| format!("invalid document: {e}"))?;
    let session = state.create_session(doc, title);
    Ok(session_info(&session))
}

/// 按宽高造一份空白文档。MCP 服务端的 create_canvas 用。
/// 走 `session_create` 那条路要经 JSON 序列化再解析，纯属绕远；
/// 而校验规则必须和那条路一模一样——同一个口子进人，就该同一把关。
pub(crate) fn sized_document(
    width: u32,
    height: u32,
    name: Option<String>,
) -> Result<Document, String> {
    let doc = Document::new(
        name.unwrap_or_else(|| "untitled".to_string()),
        width,
        height,
    )
    .map_err(|e| format!("invalid document: {e}"))?;
    // 前端传什么都能进来，宽高不校验的话后面一次 width*height 分配就能打死进程。
    doc.validated()
        .map_err(|e| format!("invalid document: {e}"))
}

#[tauri::command]
pub fn session_list(state: State<'_, Arc<AppState>>) -> Vec<SessionInfo> {
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
    state: State<'_, Arc<AppState>>,
    id: String,
    title: String,
) -> Result<SessionInfo, String> {
    let session = state.session(&id)?;
    session.set_title(Some(title));
    Ok(session_info(&session))
}

/// 拖动排序：按新次序整批改写排序位。id 不在簿里就当没发生过。
#[tauri::command]
pub fn session_reorder(state: State<'_, Arc<AppState>>, ids: Vec<String>) -> Result<(), String> {
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
pub fn session_drop(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.session(&id)?;
    state.drop_session(&id);
    Ok(())
}

/// 把会话改绑到另一个模型（保留文档与历史消息；只重建 provider）。
#[tauri::command]
pub fn session_bind_model(
    state: State<'_, Arc<AppState>>,
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
    state: State<'_, Arc<AppState>>,
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
    state: State<'_, Arc<AppState>>,
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
    state: State<'_, Arc<AppState>>,
    id: String,
    active: ActiveContext,
) -> Result<(), String> {
    state.session(&id)?.set_active(active);
    Ok(())
}

/// 权限模式：Auto 直接执行；Chat/Ask 先行提示（工作台阶段接审批交互）。
#[tauri::command]
pub fn agent_set_permission(
    state: State<'_, Arc<AppState>>,
    id: String,
    permission: PermissionMode,
) -> Result<(), String> {
    state.session(&id)?.set_permission(permission);
    Ok(())
}

/// 发一条消息并跑一个 agent turn。命令立即返回，过程事件经 `agent-event` 广播。
// 签名即 IPC 契约：每个参数都从桥那边按名字递进来。收成结构体就要改前端调用
// 形状和 mock，收益只是一处 lint 安静，不值当。
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub fn agent_send_message(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    id: String,
    text: String,
    attachments: Option<Vec<Attachment>>,
    model_id: Option<String>,
    style: Option<String>,
    presets: Option<Vec<String>>,
) -> Result<(), String> {
    run_turn(
        Some(&app),
        &state,
        &id,
        &text,
        attachments.unwrap_or_default(),
        model_id.as_deref(),
        style.as_deref(),
        presets.unwrap_or_default(),
    )
}

/// 起一个 agent 回合：界面发送和 MCP 服务端的 prompt_agent 共用这一份。
///
/// `app` 为 None 表示没有事件总线可投（MCP 的单测里就是这种）——
/// unbounded channel 不会阻塞，主循环照常跑完，只是没人收事件。
///
/// 参数刻意保持扁平：这是 IPC 契约的形状，收成结构体就要同时改前端调用
/// 形状和 mock，收益只是一处 lint 安静，不值当。
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_turn(
    app: Option<&AppHandle>,
    state: &AppState,
    id: &str,
    text: &str,
    attachments: Vec<Attachment>,
    model_id: Option<&str>,
    style: Option<&str>,
    presets: Vec<String>,
) -> Result<(), String> {
    let session = state.session(id)?;
    if let Some(model_id) = model_id.filter(|m| !m.trim().is_empty()) {
        let config = state.model_config(model_id)?;
        session.rebind_provider(config);
    }
    // 风格锁定与提示词预设都走 pins 那一份解析：微调和工作流坞问的是同一个问题，
    // 三处各写一遍就意味着补了一处、另外两处还在原地——用户选了「写实渲染」，
    // 成品却不带这条规矩，查的就是那种地方。报错语义（认不出就拒绝、超额就拒绝、
    // 「不限」放行）见 pins 模块自己的说明。
    let pinned_style = pins::pinned_style(style)?;
    let pinned_presets = pins::pinned_presets(presets)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    // 转发任务：把主循环事件搬上 Tauri 事件总线，流式期间不阻塞 UI。
    if let Some(forwarder) = app {
        let forwarder = forwarder.clone();
        // 会话标识跟着事件一起走：用户切走之后，上一个还没停干净的回合不能把
        // 它的 token 和 document_updated 落到新会话的对话与画布上。
        let owner = id.to_string();
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
    } else {
        // 没有事件总线可投：把收件端放掉。unbounded channel 的发送端不会
        // 因为没人收而阻塞，主循环照常跑完，只是这一回合的过程没人旁听。
        drop(rx);
    }
    // 主循环跑在独立任务里，命令拿到的是「已受理」而非「已跑完」。
    // 走守门员那层：主循环万一崩在半路，也要给这一回合补一个收口事件，
    // 不然前端的 running 永远不收，用户按什么都没反应。
    let text = text.to_string();
    tauri::async_runtime::spawn(async move {
        session
            .run_turn_guarded_with_preset(text, attachments, tx, pinned_style, pinned_presets)
            .await;
    });
    Ok(())
}

/// 中断当前 turn：流式轮询 120ms 内收尾，回一条 Interrupted 事件。
#[tauri::command]
pub fn agent_interrupt(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    state.session(&id)?.interrupt();
    Ok(())
}

/// 用户对一条挂起的工具调用给出决定。call_id 对不上（新一轮已经开始）会报错，
/// 前端的审批卡片据此自己收起来，不会把一次过期点击当成放行。
#[tauri::command]
pub fn agent_resolve_approval(
    state: State<'_, Arc<AppState>>,
    id: String,
    call_id: String,
    decision: ApprovalDecision,
) -> Result<(), String> {
    state.session(&id)?.resolve_approval(&call_id, decision)
}

/// 前端整体同步文档（打开 .aip、撤销、或工作台编辑后回灌），返回新 revision。
#[tauri::command]
pub fn agent_sync_document(
    state: State<'_, Arc<AppState>>,
    id: String,
    document: Value,
) -> Result<Value, String> {
    let session = state.session(&id)?;
    let doc = serde_json::from_value::<pixel_core::document::Document>(document)
        .map_err(|e| format!("invalid document: {e}"))?;
    let doc = doc
        .validated()
        .map_err(|e| format!("invalid document: {e}"))?;
    session.sync_document(doc);
    Ok(serde_json::json!({ "revision": session.revision() }))
}

/// 文档快照：revision + 文档 JSON（文本网格是权威状态）。
#[tauri::command]
pub fn agent_document(state: State<'_, Arc<AppState>>, id: String) -> Result<Value, String> {
    let session = state.session(&id)?;
    Ok(serde_json::json!({
        "id": session.id(),
        "revision": session.revision(),
        "document": session.document_json(),
        // 后端自己的选中。落图（生图、抽帧、量化）可能添一帧并把 active 挪过去，
        // document_updated 里没有帧号，前端只能按这个对齐帧条高亮和下一次落点。
        "active": session.active(),
    }))
}

/// 文档合成的 PNG data URL，画布预览与导出共用。
#[tauri::command]
pub fn document_png_url(
    state: State<'_, Arc<AppState>>,
    id: String,
    frame: Option<u32>,
) -> Result<String, String> {
    let session = state.session(&id)?;
    let doc = session.document();
    let img = match frame {
        Some(index) => pixel_core::png::composite_frame(&doc, index),
        None => {
            // 没点名帧就合成编辑器当前的活跃帧。铺开多帧是导出 sheet 的
            // 专属语义，画布预览要的是「此刻看到的那一帧」，否则多帧文档
            // 会在画布里拉成一条长图。
            let active_frame = session.active().frame;
            let index = fallback_frame_index(&doc, &active_frame);
            pixel_core::png::composite_frame(&doc, index)
        }
    };
    let bytes = pixel_core::png::encode_png(&img)?;
    Ok(format!(
        "data:image/png;base64,{}",
        pixel_core::png::base64_encode(&bytes)
    ))
}

/// 不点名帧时该合成哪一帧：编辑器当前停住的那一帧，对不上就第一帧。
/// 多帧横铺（`png::flatten`）是导出 sheet 的语义，画布预览永远不该看到它。
fn fallback_frame_index(doc: &pixel_core::document::Document, active_frame: &str) -> u32 {
    doc.frames
        .iter()
        .position(|f| f.id == active_frame)
        .unwrap_or(0) as u32
}

/// 把整个动画导出到磁盘：`gif` 走无限循环动画，`sheet` 走 PNG spritesheet。
/// `ase`/`aseprite` 写 Aseprite 文件（图层+帧语义原样保留），`frame` 写单帧 PNG
/// （`frame` 参数即帧索引，缺省第一帧），`strip` 把所有帧横向铺成一张 PNG。
/// `columns` 为 0 或 None 时 spritesheet 排成一行。
///
/// 用户选什么扩展名就写什么字节，引擎和素材库各取所需，不做二次确认。
#[tauri::command]
pub fn document_export(
    state: State<'_, Arc<AppState>>,
    id: String,
    format: String,
    path: String,
    columns: Option<u32>,
    frame: Option<u32>,
) -> Result<(), String> {
    let doc = state.session(&id)?.document();
    let bytes = export_canvas_bytes(&doc, &format, columns, frame)?;
    std::fs::write(&path, &bytes).map_err(|e| format!("cannot write {path}: {e}"))
}

/// 把一份文档编码成目标格式的字节。`document_export` 与 MCP 服务端的
/// `export_canvas` 共用：两边的格式名表必须一模一样，否则用户在界面上
/// 导得出来的东西，外部 AI 按 tools/list 报的名字去导反而失败。
pub(crate) fn export_canvas_bytes(
    doc: &Document,
    format: &str,
    columns: Option<u32>,
    frame: Option<u32>,
) -> Result<Vec<u8>, String> {
    Ok(match format {
        "gif" => pixel_core::sheet::encode_gif(doc)?,
        "ase" | "aseprite" => pixel_core::ase::encode_ase(doc)?,
        "frame" => {
            // 单帧导出：点名哪一帧就合哪一帧，没点名就合第一帧。
            let img = pixel_core::png::composite_frame(doc, frame.unwrap_or(0));
            pixel_core::png::encode_png(&img)?
        }
        // "png" 是 "frame" 的别名：list_export_formats 对外就是这么报的，
        // 少这一支的话，模型照着列表去调，第一个请求就被拒。
        "png" => {
            let img = pixel_core::png::composite_frame(doc, frame.unwrap_or(0));
            pixel_core::png::encode_png(&img)?
        }
        // 横向整条：所有帧并排贴成一张，逐帧动画的接缝一眼能看完。
        "strip" => {
            let img = pixel_core::png::flatten(doc);
            pixel_core::png::encode_png(&img)?
        }
        "sheet" => {
            let img = pixel_core::sheet::spritesheet(doc, columns.unwrap_or(0));
            pixel_core::png::encode_png(&img)?
        }
        other => return Err(format!("unsupported export format: {other}")),
    })
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
pub fn agent_history(state: State<'_, Arc<AppState>>, id: String) -> Result<Vec<Message>, String> {
    Ok(state.session(&id)?.history())
}

/// 当前文档的 .aip v2 文本，不落盘。文本网格是权威状态，前端直接渲染给用户看。
#[tauri::command]
pub fn aip_text(state: State<'_, Arc<AppState>>, id: String) -> Result<String, String> {
    state.session(&id)?.aip_text()
}

/// 当前文档另存为 .aip v2 文本，同时把文本回给前端做兜底。
#[tauri::command]
pub fn aip_save(
    state: State<'_, Arc<AppState>>,
    id: String,
    path: String,
) -> Result<String, String> {
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

    /// 回归：画布预览不点名帧时合成的是活跃帧，不是把所有帧横铺成一张长图。
    /// 以前 None 分支直接走 `png::flatten`，多帧文档会在画布里拉成一条，
    /// 用户看到的就是「选第一帧却显示所有帧连在一起」。
    #[test]
    fn png_preview_falls_back_to_the_active_frame() {
        use super::fallback_frame_index;
        use pixel_core::ops::{apply_batch, PixelOperation};

        let mut doc = pixel_core::document::Document::new("preview", 4, 3).unwrap();
        apply_batch(
            &mut doc,
            &[
                PixelOperation::CreateFrame {
                    after: None,
                    duration_ms: 100,
                    id: None,
                },
                PixelOperation::CreateFrame {
                    after: None,
                    duration_ms: 100,
                    id: None,
                },
            ],
        )
        .unwrap();
        assert_eq!(doc.frames.len(), 3);

        assert_eq!(fallback_frame_index(&doc, "F2"), 2);
        assert_eq!(fallback_frame_index(&doc, "F0"), 0);
        // 活跃帧已被删掉（改名、删帧、串了会话）时退回第一帧，仍不是横铺。
        assert_eq!(fallback_frame_index(&doc, "F99"), 0);

        // 活跃帧合出来的宽度就是画布宽度；横铺会是宽度的三倍。
        let img = pixel_core::png::composite_frame(&doc, fallback_frame_index(&doc, "F2"));
        assert_eq!(img.width(), 4);
        assert_eq!(img.height(), 3);
        // 对照：横铺确实是三倍宽，所以预览这条路要是走了 flatten 一眼就能看出来。
        assert_eq!(pixel_core::png::flatten(&doc).width(), 12);
    }
}
