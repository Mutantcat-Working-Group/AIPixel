// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! MCP 工具服务器的配置与命令层。配置（含 env / headers）只写本机 mcp.json，
//! 回前端的视图只透出键名，凭据明文永远不离开 Rust 这一侧。

use std::collections::BTreeMap;

use agent_core::{namespaced_tool, McpRegistry, McpServerConfig, McpTool, McpTransportConfig};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::state::AppState;

/// mcp.json：和 models.json 并列落在 app config 目录，重启后原样恢复。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpFile {
    #[serde(default)]
    pub entries: Vec<McpServerConfig>,
}

/// 传输的脱敏视图：命令、参数、url 可见，env / headers 只给键名。
#[derive(Debug, Clone, Serialize)]
pub struct McpTransportView {
    pub kind: String,
    pub command: String,
    pub args: Vec<String>,
    pub env_keys: Vec<String>,
    pub url: String,
    pub header_keys: Vec<String>,
}

/// 一个工具的视图。name 是命名空间后的全名：模型看到的就是这个名字。
#[derive(Debug, Clone, Serialize)]
pub struct McpToolView {
    pub name: String,
    pub server: String,
    pub tool: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpServerView {
    pub name: String,
    pub transport: McpTransportView,
    pub auto_connect: bool,
    pub connected: bool,
    pub tools: Vec<McpToolView>,
    /// 最近一次连接失败的原因；连上了就清空。
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpServersView {
    pub entries: Vec<McpServerView>,
}

fn transport_view(config: &McpServerConfig) -> McpTransportView {
    match &config.transport {
        McpTransportConfig::Stdio { command, args, env } => McpTransportView {
            kind: "stdio".into(),
            command: command.clone(),
            args: args.clone(),
            env_keys: env.keys().cloned().collect(),
            url: String::new(),
            header_keys: Vec::new(),
        },
        McpTransportConfig::Http { url, headers } => McpTransportView {
            kind: "http".into(),
            command: String::new(),
            args: Vec::new(),
            env_keys: Vec::new(),
            url: url.clone(),
            header_keys: headers.keys().cloned().collect(),
        },
    }
}

/// 工具清单 -> 视图。name 一律用命名空间后的全名：模型写进 tool_call 的
/// 就是这个名字，回执里出现裸工具名会让人对不上是哪个服务器的。
fn tool_views(server: &str, tools: &[McpTool]) -> Vec<McpToolView> {
    tools
        .iter()
        .map(|tool| McpToolView {
            name: namespaced_tool(server, &tool.name),
            server: server.to_string(),
            tool: tool.name.clone(),
            description: tool.description.clone(),
        })
        .collect()
}

/// 保存时「留空沿用」：前端只拿得到键名，值重新打一遍容易漏。
/// 空值且旧配置里有过这个键，就用旧值补上；键整行删掉才是真的清掉。
fn merge_blank_values(existing: &McpTransportConfig, incoming: &mut McpTransportConfig) {
    match (existing, incoming) {
        (
            McpTransportConfig::Stdio { env: old, .. },
            McpTransportConfig::Stdio { env: new, .. },
        ) => merge_map(old, new),
        (
            McpTransportConfig::Http { headers: old, .. },
            McpTransportConfig::Http { headers: new, .. },
        ) => merge_map(old, new),
        _ => {}
    }
}

fn merge_map(existing: &BTreeMap<String, String>, incoming: &mut BTreeMap<String, String>) {
    for (key, value) in incoming.iter_mut() {
        if value.trim().is_empty() {
            if let Some(keep) = existing.get(key) {
                *value = keep.clone();
            }
        }
    }
}

/// 一个服务器的视图。没连上时 tools 给空数组而不是 null：未握手的服务器
/// 本来就报不出清单，空数组让前端少判一种形状。
fn server_view(registry: &McpRegistry, config: &McpServerConfig) -> McpServerView {
    let connected = registry.is_connected(&config.name);
    let tools = if connected {
        registry.tools_of(&config.name)
    } else {
        Vec::new()
    };
    McpServerView {
        name: config.name.clone(),
        transport: transport_view(config),
        auto_connect: config.auto_connect,
        connected,
        tools: tool_views(&config.name, &tools),
        last_error: registry.last_error(&config.name),
    }
}

pub fn servers_view(registry: &McpRegistry) -> McpServersView {
    McpServersView {
        entries: registry
            .configs()
            .iter()
            .map(|config| server_view(registry, config))
            .collect(),
    }
}

/// 服务器清单：连接状态、各自暴露的工具、最近一次错误。
#[tauri::command]
pub fn mcp_list(state: State<'_, AppState>) -> McpServersView {
    servers_view(&state.mcp_registry())
}

/// 新增/更新一个服务器配置。只登记不连接，连不连由用户点。
#[tauri::command]
pub async fn mcp_upsert(
    state: State<'_, AppState>,
    mut config: McpServerConfig,
) -> Result<McpServersView, String> {
    config.validate()?;
    let registry = state.mcp_registry();
    if let Some(existing) = registry
        .configs()
        .into_iter()
        .find(|c| c.name == config.name)
    {
        merge_blank_values(&existing.transport, &mut config.transport);
    }
    registry.register(config).await?;
    state.save_mcp_file();
    Ok(servers_view(&registry))
}

/// 删除服务器并断开连接。
#[tauri::command]
pub async fn mcp_remove(
    state: State<'_, AppState>,
    name: String,
) -> Result<McpServersView, String> {
    let registry = state.mcp_registry();
    registry.unregister(&name).await?;
    state.save_mcp_file();
    Ok(servers_view(&registry))
}

/// 手动连接：握手 + 拉工具清单。失败原因随视图带回，弹窗里直接显示。
#[tauri::command]
pub async fn mcp_connect(
    state: State<'_, AppState>,
    name: String,
) -> Result<McpServersView, String> {
    let registry = state.mcp_registry();
    registry.connect(&name).await?;
    Ok(servers_view(&registry))
}

/// 断开但保留配置。
#[tauri::command]
pub async fn mcp_disconnect(
    state: State<'_, AppState>,
    name: String,
) -> Result<McpServersView, String> {
    let registry = state.mcp_registry();
    registry.disconnect(&name).await;
    Ok(servers_view(&registry))
}
