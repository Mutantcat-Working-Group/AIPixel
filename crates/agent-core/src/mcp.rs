// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! MCP（Model Context Protocol）客户端：把用户自己配的 MCP 工具服务器接进 agent 主循环。
//!
//! 设计要点：
//! - 纯 Rust，不依赖 Tauri；stdio 与 streamable HTTP 两种传输都实现，凭据只在本机流转。
//! - 工具名统一加 `mcp__<server>__<tool>` 命名空间，和内建 `pixel_*` 工具永不撞名；
//!   超长时按模型 API 的 64 字符上限截断并接一个稳定短哈希。
//! - 工具结果映射成 `ToolOutcome`：text 块按行拼起来；image / audio / resource 块只回一行
//!   摘要，不把大位图塞进上下文（生图落地一律走 `pixel_pixelize_image`）。
//! - 服务器主动发过来的请求（sampling 之类）一律以 -32601 回绝：本客户端只当工具提供方。
//! - 前后端边界：配置（含 env / headers）只落在本机配置文件，webview 永远看不到明文。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::models::ToolSpec;
use super::tools::ToolOutcome;

/// 工具名前缀：主循环靠它把调用分流到 MCP registry。
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// 模型 API 对工具名的长度上限（Anthropic 是 64，OpenAI 更宽，按最严的来）。
const TOOL_NAME_LIMIT: usize = 64;

/// 客户端声明的协议版本，服务器不认就按历史版本依次退让。
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// 单次请求的等待上限。stdio 服务器冷启动（npx 拉包）可能很慢，initialize 单独放宽。
const REQUEST_TIMEOUT_MS: u64 = 60_000;
const INIT_TIMEOUT_MS: u64 = 120_000;

/// stderr 尾巴的保留长度：服务器猝死时靠这几行说话。
const STDERR_TAIL_CHARS: usize = 4000;

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("cannot start MCP server: {0}")]
    Spawn(String),
    #[error("stdio transport: {0}")]
    Io(String),
    #[error("http transport: {0}")]
    Http(String),
    #[error("mcp protocol: {0}")]
    Protocol(String),
    #[error("{0} did not answer in time")]
    Timeout(String),
}

/// 一个 MCP 服务器的用户配置。跟着 `mcp.json` 落盘在 app config 目录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// 唯一名字，同时进工具命名空间；只允许字母数字与 `-` `_`。
    pub name: String,
    pub transport: McpTransportConfig,
    /// 开机自动连接。误连一个坏服务器不该挡住启动，连接失败只记录状态。
    #[serde(default)]
    pub auto_connect: bool,
}

/// 传输方式：stdio 子进程，或 streamable HTTP。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

impl McpServerConfig {
    /// 名字进得了命名空间才算数：撞名/畸形名会让路由分成两半。
    pub fn validate(&self) -> Result<(), String> {
        let name_ok = !self.name.is_empty()
            && self.name.len() <= 24
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !name_ok {
            return Err(format!(
                "server name must be 1-24 chars of letters, digits, '-' or '_': {}",
                self.name
            ));
        }
        match &self.transport {
            McpTransportConfig::Stdio { command, .. } if command.trim().is_empty() => {
                Err("stdio server needs a command".into())
            }
            McpTransportConfig::Http { url, .. }
                if !(url.starts_with("http://") || url.starts_with("https://")) =>
            {
                Err(format!(
                    "http server url must start with http:// or https://: {url}"
                ))
            }
            _ => Ok(()),
        }
    }
}

/// `tools/list` 的一个条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 服务器给的 JSON Schema；缺了就当空对象。
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Option<Value>,
}

/// `tools/call` 拍平后的结果：文本拼好，is_error 原样带回。
#[derive(Debug, Clone)]
pub struct McpToolOutput {
    pub text: String,
    pub is_error: bool,
}

/// 传输层抽象：发一条请求等一个响应；通知发完就算送达。
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError>;
    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError>;
    /// 诊断信息（stdio：传输种类；http：url）。
    fn describe(&self) -> String;
    async fn shutdown(&self) {}
}

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(0);

fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst) + 1
}

/// 一行报文如果是对应 id 的响应，返回结果或错误；不是就返回 None（等待方继续读）。
fn response_for(line: &str, id: u64) -> Option<Result<Value, McpError>> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("id").and_then(|v| v.as_u64()) != Some(id) {
        return None;
    }
    if let Some(result) = value.get("result") {
        return Some(Ok(result.clone()));
    }
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Some(Err(McpError::Protocol(message.to_string())));
    }
    None
}

/// 服务器主动发来的请求（sampling、roots/list……）。带 id 的要回绝，纯通知直接忽略。
fn unsupported_request(line: &str) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    let id = value.get("id")?;
    let method = value
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("request");
    let reply = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32601,
            "message": format!("AIPixel does not serve {method} to MCP servers"),
        },
    });
    Some(format!("{reply}\n"))
}

/// 从 SSE 报文里捞出对应 id 的响应：事件之间空行分隔，`data:` 行可多行，注释行忽略。
fn sse_response(text: &str, id: u64) -> Result<Value, McpError> {
    let mut data = String::new();
    for line in text.lines() {
        if line.is_empty() {
            if !data.is_empty() {
                if let Some(result) = response_for(&data, id) {
                    return result;
                }
                data.clear();
            }
            continue;
        }
        if let Some(payload) = line.strip_prefix("data:") {
            data.push_str(payload.strip_prefix(' ').unwrap_or(payload));
        }
        // `:` 开头的注释（heartbeat）与 `event:` / `id:` 行都不影响我们取 JSON。
    }
    Err(McpError::Protocol(format!(
        "no JSON-RPC response with id {id} in the SSE stream"
    )))
}

/// stdio 传输：换行分隔的 JSON-RPC 2.0，一行一条。
pub struct StdioTransport {
    channel: Mutex<StdioChannel>,
}

struct StdioChannel {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    stderr_tail: Arc<Mutex<String>>,
    next_id: AtomicU64,
}

impl StdioTransport {
    /// 拉起子进程。stdio 两端都接管，stderr 单独抽干（不抽的话服务器写满管道缓冲就挂住）。
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Spawn(format!("{command}: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Spawn("child stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Spawn("child stdout unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| McpError::Spawn("child stderr unavailable".into()))?;
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut guard = tail.lock().await;
                guard.push_str(&line);
                guard.push('\n');
                let overflow = guard.chars().count().saturating_sub(STDERR_TAIL_CHARS);
                if overflow > 0 {
                    let cut = guard
                        .char_indices()
                        .nth(overflow)
                        .map(|(i, _)| i)
                        .unwrap_or(guard.len());
                    guard.drain(..cut);
                }
            }
        });
        Ok(Self {
            channel: Mutex::new(StdioChannel {
                child,
                stdin,
                stdout: BufReader::new(stdout),
                stderr_tail,
                next_id: AtomicU64::new(0),
            }),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let mut channel = self.channel.lock().await;
        let id = channel.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let payload = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut line = serde_json::to_string(&payload)
            .map_err(|e| McpError::Protocol(format!("cannot serialize request: {e}")))?;
        line.push('\n');
        channel
            .stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::Io(e.to_string()))?;
        channel
            .stdin
            .flush()
            .await
            .map_err(|e| McpError::Io(e.to_string()))?;
        let budget_ms = if method == "initialize" {
            INIT_TIMEOUT_MS
        } else {
            REQUEST_TIMEOUT_MS
        };
        let read = async {
            loop {
                let mut raw = String::new();
                let read = channel
                    .stdout
                    .read_line(&mut raw)
                    .await
                    .map_err(|e| McpError::Io(e.to_string()))?;
                if read == 0 {
                    let tail = channel.stderr_tail.lock().await;
                    let hint = if tail.trim().is_empty() {
                        "no stderr output".to_string()
                    } else {
                        format!("stderr: {}", tail.trim())
                    };
                    return Err(McpError::Io(format!("server closed the stream ({hint})")));
                }
                if let Some(reply) = unsupported_request(&raw) {
                    // 服务器在要 sampling 之类的能力：明确回绝，然后继续等我们的响应。
                    channel
                        .stdin
                        .write_all(reply.as_bytes())
                        .await
                        .map_err(|e| McpError::Io(e.to_string()))?;
                    let _ = channel.stdin.flush().await;
                    continue;
                }
                if let Some(result) = response_for(&raw, id) {
                    return result;
                }
            }
        };
        tokio::time::timeout(std::time::Duration::from_millis(budget_ms), read)
            .await
            .map_err(|_| McpError::Timeout(method.to_string()))?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let mut channel = self.channel.lock().await;
        let payload = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let mut line = serde_json::to_string(&payload)
            .map_err(|e| McpError::Protocol(format!("cannot serialize notification: {e}")))?;
        line.push('\n');
        channel
            .stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::Io(e.to_string()))?;
        channel
            .stdin
            .flush()
            .await
            .map_err(|e| McpError::Io(e.to_string()))
    }

    fn describe(&self) -> String {
        "stdio".to_string()
    }

    async fn shutdown(&self) {
        let mut channel = self.channel.lock().await;
        let _ = channel.child.kill().await;
    }
}

/// streamable HTTP 传输：POST JSON-RPC，响应可能是裸 JSON 或 SSE 流。
pub struct HttpTransport {
    url: String,
    headers: BTreeMap<String, String>,
    session_id: Mutex<Option<String>>,
    client: reqwest::Client,
}

impl HttpTransport {
    pub fn new(url: &str, headers: &BTreeMap<String, String>) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            // 覆盖 initialize 的最长等待，其余请求另有自己的超时边界。
            .timeout(std::time::Duration::from_millis(INIT_TIMEOUT_MS))
            .build()
            .map_err(|e| format!("cannot build http client: {e}"))?;
        Ok(Self {
            url: url.to_string(),
            headers: headers.clone(),
            session_id: Mutex::new(None),
            client,
        })
    }

    fn jsonrpc(&self, method: &str, params: Value, notification: bool) -> Value {
        let mut payload = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if !notification {
            payload["id"] = json!(next_request_id());
        }
        payload
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn request(&self, method: &str, params: Value) -> Result<Value, McpError> {
        let payload = self.jsonrpc(method, params, false);
        let id = payload.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
        let mut request = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        if let Some(session) = self.session_id.lock().await.clone() {
            request = request.header("mcp-session-id", session);
        }
        let response = request
            .json(&payload)
            .send()
            .await
            .map_err(|e| McpError::Http(e.to_string()))?;
        let status = response.status();
        // 新会话的 id 由服务器在响应头里发，后续请求都要带。
        if let Some(session) = response.headers().get("mcp-session-id") {
            if let Ok(text) = session.to_str() {
                *self.session_id.lock().await = Some(text.to_string());
            }
        }
        if status.as_u16() == 202 {
            // 通知被受理，没有响应体。
            return Ok(json!({}));
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let head: String = body.chars().take(400).collect();
            return Err(McpError::Http(format!("HTTP {}: {head}", status.as_u16())));
        }
        let is_sse = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("text/event-stream"))
            .unwrap_or(false);
        let text = response
            .text()
            .await
            .map_err(|e| McpError::Http(e.to_string()))?;
        if is_sse {
            return sse_response(&text, id);
        }
        response_for(&text, id).ok_or_else(|| {
            McpError::Protocol(format!("http response did not answer request id {id}"))
        })?
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), McpError> {
        let payload = self.jsonrpc(method, params, true);
        let mut request = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        if let Some(session) = self.session_id.lock().await.clone() {
            request = request.header("mcp-session-id", session);
        }
        let response = request
            .json(&payload)
            .send()
            .await
            .map_err(|e| McpError::Http(e.to_string()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(McpError::Http(format!(
                "HTTP {}",
                response.status().as_u16()
            )))
        }
    }

    fn describe(&self) -> String {
        self.url.clone()
    }

    async fn shutdown(&self) {
        // 礼貌地告诉服务器会话结束；失败也无所谓，HTTP 无状态可丢。
        let mut request = self
            .client
            .delete(&self.url)
            .header("accept", "application/json, text/event-stream");
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        if let Some(session) = self.session_id.lock().await.clone() {
            request = request.header("mcp-session-id", session);
        }
        let _ = request.send().await;
    }
}

/// 单个服务器的连接：客户端 + 最近一次错误。错误留给 UI 说话，不在这里重试。
struct ServerEntry {
    config: McpServerConfig,
    client: Option<Arc<McpClient>>,
    last_error: Option<String>,
}

/// 工具名映射：`mcp__server__tool` -> (server, tool)。连接变化时整体重建，
/// 这样路由不依赖字符串切分，服务器名里带下划线也翻不了车。
type Routes = BTreeMap<String, (String, String)>;

/// 全部 MCP 服务器的注册表。AppState 持有一个 Arc，所有会话共享。
pub struct McpRegistry {
    servers: RwLock<BTreeMap<String, ServerEntry>>,
    routes: RwLock<Routes>,
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(BTreeMap::new()),
            routes: RwLock::new(Routes::new()),
        }
    }

    /// 登记配置（开机时从 mcp.json 恢复，或用户在 UI 里新增/编辑）。已连接的先断开。
    pub async fn register(&self, config: McpServerConfig) -> Result<(), String> {
        config.validate()?;
        let stale = self.servers.write().unwrap().remove(&config.name);
        if let Some(entry) = stale {
            if let Some(client) = entry.client {
                client.shutdown().await;
            }
        }
        self.servers.write().unwrap().insert(
            config.name.clone(),
            ServerEntry {
                config,
                client: None,
                last_error: None,
            },
        );
        self.rebuild_routes();
        Ok(())
    }

    /// 移除服务器并断开连接。
    pub async fn unregister(&self, name: &str) -> Result<(), String> {
        let stale = self.servers.write().unwrap().remove(name);
        if let Some(entry) = stale {
            if let Some(client) = entry.client {
                client.shutdown().await;
            }
        }
        self.rebuild_routes();
        Ok(())
    }

    /// 连接一个已登记的服务器：initialize + tools/list。失败只记录错误并返回 Err。
    pub async fn connect(&self, name: &str) -> Result<Vec<McpTool>, String> {
        let config = {
            let servers = self.servers.read().unwrap();
            match servers.get(name) {
                Some(entry) => entry.config.clone(),
                None => return Err(format!("unknown MCP server: {name}")),
            }
        };
        match McpClient::connect(&config).await {
            Ok(client) => {
                let tools = client.tools();
                if let Some(entry) = self.servers.write().unwrap().get_mut(name) {
                    entry.client = Some(client);
                    entry.last_error = None;
                }
                self.rebuild_routes();
                Ok(tools)
            }
            Err(e) => {
                let message = format!("{e} (transport: {})", config.transport.describe_short());
                if let Some(entry) = self.servers.write().unwrap().get_mut(name) {
                    entry.client = None;
                    entry.last_error = Some(message.clone());
                }
                Err(message)
            }
        }
    }

    /// 断开但保留配置。
    pub async fn disconnect(&self, name: &str) {
        let stale = {
            let mut servers = self.servers.write().unwrap();
            servers
                .get_mut(name)
                .map(|entry| (entry.client.take(), entry.last_error.take()))
                .unwrap_or((None, None))
        };
        if let Some(client) = stale.0 {
            client.shutdown().await;
        }
        self.rebuild_routes();
    }

    pub fn configs(&self) -> Vec<McpServerConfig> {
        self.servers
            .read()
            .unwrap()
            .values()
            .map(|entry| entry.config.clone())
            .collect()
    }

    pub fn is_connected(&self, name: &str) -> bool {
        self.servers
            .read()
            .unwrap()
            .get(name)
            .map(|entry| entry.client.is_some())
            .unwrap_or(false)
    }

    pub fn last_error(&self, name: &str) -> Option<String> {
        self.servers
            .read()
            .unwrap()
            .get(name)
            .and_then(|entry| entry.last_error.clone())
    }

    /// 某个服务器当前暴露的工具（已连接才非空）。
    pub fn tools_of(&self, name: &str) -> Vec<McpTool> {
        self.servers
            .read()
            .unwrap()
            .get(name)
            .and_then(|entry| entry.client.clone())
            .map(|client| client.tools())
            .unwrap_or_default()
    }

    /// 合并后的工具规格：内建 pixel_* 由调用方追加，这里只出 MCP 部分。
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut specs = Vec::new();
        for (name, entry) in self.servers.read().unwrap().iter() {
            let Some(client) = entry.client.clone() else {
                continue;
            };
            for tool in client.tools() {
                specs.push(ToolSpec {
                    name: namespaced_tool(name, &tool.name).into(),
                    description: if tool.description.is_empty() {
                        format!(
                            "[MCP server '{name}'] external tool from a user-configured MCP server"
                        )
                        .into()
                    } else {
                        format!("[MCP server '{name}'] {}", tool.description).into()
                    },
                    schema: tool
                        .input_schema
                        .unwrap_or_else(|| json!({"type": "object"})),
                });
            }
        }
        specs
    }

    /// 把一个 MCP 工具调用送到对应服务器。
    pub async fn dispatch(&self, tool_name: &str, input: &Value) -> Result<ToolOutcome, String> {
        let route = self.routes.read().unwrap().get(tool_name).cloned();
        let Some(route) = route else {
            return Err(match self.owner_of(tool_name) {
                Some(server) => format!("MCP server '{server}' is not connected"),
                None => format!("unknown MCP tool: {tool_name}"),
            });
        };
        let client = {
            let servers = self.servers.read().unwrap();
            servers.get(&route.0).and_then(|entry| entry.client.clone())
        };
        let Some(client) = client else {
            return Err(format!("MCP server '{}' is not connected", route.0));
        };
        match client.call_tool(&route.1, input.clone()).await {
            Ok(output) => Ok(ToolOutcome {
                content: output.text,
                is_error: output.is_error,
            }),
            Err(e) => Err(format!("MCP server '{}' tool '{}': {e}", route.0, route.1)),
        }
    }

    /// 配置名 -> (server, tool) 路由表整体重建：工具集只在连接时变化，重建足够。
    fn rebuild_routes(&self) {
        let servers = self.servers.read().unwrap();
        let mut routes = Routes::new();
        for (server, entry) in servers.iter() {
            let Some(client) = entry.client.clone() else {
                continue;
            };
            for tool in client.tools() {
                routes.insert(
                    namespaced_tool(server, &tool.name),
                    (server.clone(), tool.name),
                );
            }
        }
        *self.routes.write().unwrap() = routes;
    }

    /// 路由没命中时反查归属：拿注册过的服务器名（长的优先，防 "a" 抢走 "a_b"）
    /// 和 `mcp__` 后的尾巴对前缀。查到了说明「配了但没连」，查不到才是「没配过」。
    fn owner_of(&self, tool_name: &str) -> Option<String> {
        let rest = tool_name.strip_prefix(MCP_TOOL_PREFIX)?;
        let servers = self.servers.read().unwrap();
        let mut names: Vec<&String> = servers.keys().collect();
        names.sort_by_key(|name| std::cmp::Reverse(name.len()));
        names
            .into_iter()
            .find(|name| {
                rest.strip_prefix(sanitize(name).as_str())
                    .is_some_and(|tail| tail.starts_with("__"))
            })
            .cloned()
    }
}

impl McpTransportConfig {
    fn describe_short(&self) -> String {
        match self {
            McpTransportConfig::Stdio { command, args, .. } => {
                if args.is_empty() {
                    command.clone()
                } else {
                    format!("{command} {}", args.join(" "))
                }
            }
            McpTransportConfig::Http { url, .. } => url.clone(),
        }
    }
}

/// 单个 MCP 服务器连接：传输 + 协商出的协议版本 + 工具清单。
pub struct McpClient {
    server: String,
    transport: Arc<dyn McpTransport>,
    protocol_version: RwLock<String>,
    tools: RwLock<Vec<McpTool>>,
}

impl McpClient {
    /// 从配置拉一个完整连接。任何一步失败都算连不上，错误带传输层诊断。
    pub async fn connect(config: &McpServerConfig) -> Result<Arc<Self>, McpError> {
        let transport: Arc<dyn McpTransport> = match &config.transport {
            McpTransportConfig::Stdio { command, args, env } => {
                Arc::new(StdioTransport::spawn(command, args, env)?)
            }
            McpTransportConfig::Http { url, headers } => {
                Arc::new(HttpTransport::new(url, headers).map_err(McpError::Http)?)
            }
        };
        let client = Arc::new(Self::with_transport(config.name.clone(), transport));
        client.initialize().await?;
        client.refresh_tools().await?;
        Ok(client)
    }

    /// 用现成传输建客户端（测试用；上线路径走 `connect`）。
    pub fn with_transport(server: impl Into<String>, transport: Arc<dyn McpTransport>) -> Self {
        Self {
            server: server.into(),
            transport,
            protocol_version: RwLock::new(String::new()),
            tools: RwLock::new(Vec::new()),
        }
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    pub fn protocol_version(&self) -> String {
        self.protocol_version.read().unwrap().clone()
    }

    pub fn tools(&self) -> Vec<McpTool> {
        self.tools.read().unwrap().clone()
    }

    async fn initialize(&self) -> Result<(), McpError> {
        // 协议版本谈不拢就退一档；其余错误（起不来、超时）直接抛，不重试。
        let mut last_error = None;
        for version in PROTOCOL_VERSIONS {
            let params = json!({
                "protocolVersion": version,
                "capabilities": {"roots": {"listChanged": false}},
                "clientInfo": {"name": "AIPixel", "version": env!("CARGO_PKG_VERSION")},
            });
            match self.transport.request("initialize", params).await {
                Ok(result) => {
                    let negotiated = result
                        .get("protocolVersion")
                        .and_then(|v| v.as_str())
                        .unwrap_or(version)
                        .to_string();
                    *self.protocol_version.write().unwrap() = negotiated;
                    self.transport
                        .notify("notifications/initialized", json!({}))
                        .await?;
                    return Ok(());
                }
                Err(e) => {
                    let text = e.to_string().to_lowercase();
                    let about_version = text.contains("protocol") || text.contains("version");
                    if about_version {
                        last_error = Some(e);
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| McpError::Protocol("initialize failed".into())))
    }

    /// 拉工具清单，带 cursor 翻页。
    pub async fn refresh_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let mut cursor: Option<String> = None;
        let mut all: Vec<McpTool> = Vec::new();
        loop {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let result = self.transport.request("tools/list", params).await?;
            let page: ToolsPage = serde_json::from_value(result)
                .map_err(|e| McpError::Protocol(format!("tools/list: {e}")))?;
            all.extend(page.tools);
            match page.next_cursor {
                Some(next) if !next.is_empty() => cursor = Some(next),
                _ => break,
            }
        }
        *self.tools.write().unwrap() = all.clone();
        Ok(all)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolOutput, McpError> {
        let result = self
            .transport
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        Ok(tool_output_from(&result))
    }

    pub async fn shutdown(&self) {
        self.transport.shutdown().await;
    }
}

/// `tools/list` 的翻页响应。
#[derive(Debug, Deserialize)]
struct ToolsPage {
    #[serde(default)]
    tools: Vec<McpTool>,
    #[serde(default, rename = "nextCursor")]
    next_cursor: Option<String>,
}

/// 把 `tools/call` 的 content 块数组拍平成一段文本：
/// text 块按行拼；image / audio / resource 只留一行摘要，不把大负载塞进上下文。
pub fn tool_output_from(value: &Value) -> McpToolOutput {
    let is_error = value
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut parts: Vec<String> = Vec::new();
    if let Some(blocks) = value.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        parts.push(text.to_string());
                    }
                }
                Some("image") => {
                    let mime = block
                        .get("mimeType")
                        .and_then(|m| m.as_str())
                        .unwrap_or("image/*");
                    parts.push(format!(
                        "[image {mime}, ~{} bytes, not inlined: land bitmaps with pixel_pixelize_image]",
                        base64_len(block.get("data").and_then(|d| d.as_str()).unwrap_or(""))
                    ));
                }
                Some("audio") => {
                    let mime = block
                        .get("mimeType")
                        .and_then(|m| m.as_str())
                        .unwrap_or("audio/*");
                    parts.push(format!(
                        "[audio {mime}, ~{} bytes, not inlined]",
                        base64_len(block.get("data").and_then(|d| d.as_str()).unwrap_or(""))
                    ));
                }
                Some("resource") => {
                    let resource = block.get("resource").unwrap_or(block);
                    let uri = resource
                        .get("uri")
                        .and_then(|u| u.as_str())
                        .unwrap_or("unknown resource");
                    if let Some(text) = resource.get("text").and_then(|t| t.as_str()) {
                        parts.push(format!("resource {uri}:\n{text}"));
                    } else {
                        parts.push(format!(
                            "[resource {uri}, ~{} bytes, not inlined]",
                            base64_len(resource.get("blob").and_then(|b| b.as_str()).unwrap_or(""))
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    McpToolOutput {
        text: if parts.is_empty() {
            "(tool returned no content)".to_string()
        } else {
            parts.join("\n")
        },
        is_error,
    }
}

/// base64 字符串的大致字节数，只用于给人看的摘要。
fn base64_len(data: &str) -> usize {
    data.len() * 3 / 4
}

/// 服务器/工具名进命名空间：非 [A-Za-z0-9_] 一律换成下划线，首尾修剪掉，
/// 免得 `my srv!` 尾巴的 `_` 和分隔符糊成三个。连字符也转，两种写法归一。
fn sanitize(segment: &str) -> String {
    segment
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// FNV-1a：截断后的名字要靠它区分「前 N 个字符相同」的不同工具。
fn fnv1a(text: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in text.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// 完整工具名。超 64 字符时截断并接 8 位十六进制短哈希，保证同名稳定、异名不撞。
pub fn namespaced_tool(server: &str, tool: &str) -> String {
    let full = format!("{MCP_TOOL_PREFIX}{}__{}", sanitize(server), sanitize(tool));
    if full.chars().count() <= TOOL_NAME_LIMIT {
        return full;
    }
    let hash = fnv1a(&full);
    let keep = TOOL_NAME_LIMIT - 10; // "__" + 8 位哈希
    let mut out: String = full.chars().take(keep).collect();
    out.push_str(&format!("__{hash:08x}"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// 按方法名出栈预置响应的假传输，顺带记录调用顺序。
    struct FakeTransport {
        responses: Mutex<VecDeque<(String, Value)>>,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl FakeTransport {
        fn scripted(log: Arc<Mutex<Vec<String>>>, steps: Vec<(&str, Value)>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(steps.into_iter().map(|(m, v)| (m.to_string(), v)).collect()),
                log,
            })
        }
    }

    #[async_trait]
    impl McpTransport for FakeTransport {
        async fn request(&self, method: &str, _params: Value) -> Result<Value, McpError> {
            self.log.lock().await.push(method.to_string());
            let mut queue = self.responses.lock().await;
            match queue.pop_front() {
                Some((expected, value)) if expected == method => match value.get("error") {
                    // 和真传输一个规矩：error 响应翻 Err，initialize 的版本降级才触发得了。
                    Some(error) => Err(McpError::Protocol(
                        error
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error")
                            .to_string(),
                    )),
                    None => Ok(value),
                },
                Some((expected, _)) => Err(McpError::Protocol(format!(
                    "expected {expected}, got {method}"
                ))),
                None => Err(McpError::Protocol(format!(
                    "no scripted response for {method}"
                ))),
            }
        }

        async fn notify(&self, method: &str, _params: Value) -> Result<(), McpError> {
            self.log.lock().await.push(format!("notify:{method}"));
            Ok(())
        }

        fn describe(&self) -> String {
            "fake".to_string()
        }
    }

    fn initialize_result(version: &str) -> Value {
        json!({"protocolVersion": version, "serverInfo": {"name": "fake"}})
    }

    fn tools_page() -> Value {
        json!({"tools": [
            {"name": "ping", "description": "answer pong", "inputSchema": {"type": "object"}},
            {"name": "echo", "description": "", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}
        ]})
    }

    async fn client_with_tools(server: &str, steps: Vec<(&str, Value)>) -> Arc<McpClient> {
        let log = Arc::new(Mutex::new(Vec::new()));
        let client = McpClient::with_transport(server, FakeTransport::scripted(log.clone(), steps));
        client.initialize().await.expect("initialize");
        client.refresh_tools().await.expect("tools/list");
        Arc::new(client)
    }

    #[tokio::test]
    async fn initialize_negotiates_the_protocol_version_and_announces_ready() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let client = McpClient::with_transport(
            "srv",
            FakeTransport::scripted(
                log.clone(),
                vec![("initialize", initialize_result("2024-11-05"))],
            ),
        );
        client.initialize().await.expect("initialize");
        assert_eq!(client.protocol_version(), "2024-11-05");
        let log = log.lock().await;
        assert_eq!(log[0], "initialize");
        assert_eq!(log[1], "notify:notifications/initialized");
    }

    #[tokio::test]
    async fn initialize_falls_back_to_an_older_protocol_version() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let client = McpClient::with_transport(
            "srv",
            FakeTransport::scripted(
                log,
                vec![
                    (
                        "initialize",
                        json!({"error": {"code": -32602, "message": "unsupported protocol version"}}),
                    ),
                    ("initialize", initialize_result("2025-03-26")),
                ],
            ),
        );
        client.initialize().await.expect("initialize");
        assert_eq!(client.protocol_version(), "2025-03-26");
    }

    #[tokio::test]
    async fn tools_list_loads_the_catalog() {
        let client = client_with_tools(
            "srv",
            vec![
                ("initialize", initialize_result("2025-06-18")),
                ("tools/list", tools_page()),
            ],
        )
        .await;
        let tools = client.tools();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "ping");
    }

    #[tokio::test]
    async fn call_tool_flattens_text_and_summarizes_images() {
        let client = client_with_tools(
            "srv",
            vec![
                ("initialize", initialize_result("2025-06-18")),
                ("tools/list", tools_page()),
                (
                    "tools/call",
                    json!({"content": [
                        {"type": "text", "text": "first line"},
                        {"type": "text", "text": "second line"},
                        {"type": "image", "mimeType": "image/png", "data": "AAAAAAAA"}
                    ]}),
                ),
            ],
        )
        .await;
        let output = client.call_tool("ping", json!({})).await.expect("call");
        assert!(
            output.text.starts_with("first line\nsecond line"),
            "{}",
            output.text
        );
        assert!(output.text.contains("image image/png"), "{}", output.text);
        assert!(!output.is_error);
    }

    #[tokio::test]
    async fn call_tool_carries_the_server_error_flag() {
        let client = client_with_tools(
            "srv",
            vec![
                ("initialize", initialize_result("2025-06-18")),
                ("tools/list", tools_page()),
                (
                    "tools/call",
                    json!({"isError": true, "content": [{"type": "text", "text": "boom"}]}),
                ),
            ],
        )
        .await;
        let output = client.call_tool("ping", json!({})).await.expect("call");
        assert!(output.is_error);
        assert_eq!(output.text, "boom");
    }

    #[tokio::test]
    async fn tool_output_from_handles_empty_and_resource_blocks() {
        let empty = tool_output_from(&json!({"content": []}));
        assert_eq!(empty.text, "(tool returned no content)");
        assert!(!empty.is_error);

        let resource = tool_output_from(&json!({"content": [
            {"type": "resource", "resource": {"uri": "file:///notes.txt", "text": "hello"}}
        ]}));
        assert!(
            resource.text.contains("file:///notes.txt"),
            "{}",
            resource.text
        );
        assert!(resource.text.contains("hello"), "{}", resource.text);
    }

    #[tokio::test]
    async fn registry_specs_namespace_tools_and_prefix_the_server() {
        let registry = McpRegistry::new();
        registry
            .register(McpServerConfig {
                name: "pixels".into(),
                transport: McpTransportConfig::Http {
                    url: "https://mcp.example.com/mcp".into(),
                    headers: BTreeMap::new(),
                },
                auto_connect: false,
            })
            .await
            .expect("register");
        // 没连接就没有规格：UI 上「已连接」必须是真连过。
        assert!(registry.specs().is_empty());
        assert!(!registry.is_connected("pixels"));

        let client = client_with_tools(
            "pixels",
            vec![
                ("initialize", initialize_result("2025-06-18")),
                ("tools/list", tools_page()),
            ],
        )
        .await;
        {
            let mut servers = registry.servers.write().unwrap();
            servers.get_mut("pixels").unwrap().client = Some(client);
        }
        registry.rebuild_routes();

        let specs = registry.specs();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "mcp__pixels__ping");
        assert!(
            specs[0].description.contains("pixels"),
            "{}",
            specs[0].description
        );
        assert_eq!(specs[1].schema["type"], "object");
    }

    #[tokio::test]
    async fn registry_dispatch_routes_by_name_not_by_string_splitting() {
        // 两个服务器都有 ping：路由必须按完整名字区分，切字符串会把 my_srv 切散。
        let registry = McpRegistry::new();
        let mut servers = Vec::new();
        for (server, answer) in [("my_srv", "alpha-pong"), ("beta", "beta-pong")] {
            registry
                .register(McpServerConfig {
                    name: server.into(),
                    transport: McpTransportConfig::Stdio {
                        command: "echo".into(),
                        args: vec![],
                        env: BTreeMap::new(),
                    },
                    auto_connect: false,
                })
                .await
                .expect("register");
            servers.push((server, answer));
        }
        for (server, answer) in servers {
            let client = client_with_tools(
                server,
                vec![
                    ("initialize", initialize_result("2025-06-18")),
                    ("tools/list", tools_page()),
                    (
                        "tools/call",
                        json!({"content": [{"type": "text", "text": answer}]}),
                    ),
                ],
            )
            .await;
            let mut guard = registry.servers.write().unwrap();
            guard.get_mut(server).unwrap().client = Some(client);
        }
        registry.rebuild_routes();

        let outcome = registry
            .dispatch("mcp__my_srv__ping", &json!({}))
            .await
            .expect("dispatch");
        assert_eq!(outcome.content, "alpha-pong");
        let outcome = registry
            .dispatch("mcp__beta__ping", &json!({}))
            .await
            .expect("dispatch");
        assert_eq!(outcome.content, "beta-pong");
    }

    #[tokio::test]
    async fn registry_reports_unknown_and_offline_servers() {
        let registry = McpRegistry::new();
        let err = registry
            .dispatch("mcp__ghost__ping", &json!({}))
            .await
            .expect_err("unknown server");
        assert!(err.contains("unknown MCP tool"), "{err}");

        registry
            .register(McpServerConfig {
                name: "offline".into(),
                transport: McpTransportConfig::Http {
                    url: "https://mcp.example.com/mcp".into(),
                    headers: BTreeMap::new(),
                },
                auto_connect: false,
            })
            .await
            .expect("register");
        let err = registry
            .dispatch("mcp__offline__ping", &json!({}))
            .await
            .expect_err("not connected");
        assert!(err.contains("not connected"), "{err}");
    }

    #[tokio::test]
    async fn unregister_drops_routes() {
        let registry = McpRegistry::new();
        registry
            .register(McpServerConfig {
                name: "tmp".into(),
                transport: McpTransportConfig::Http {
                    url: "https://mcp.example.com/mcp".into(),
                    headers: BTreeMap::new(),
                },
                auto_connect: false,
            })
            .await
            .expect("register");
        assert_eq!(registry.configs().len(), 1);
        registry.unregister("tmp").await.expect("unregister");
        assert!(registry.configs().is_empty());
        assert!(registry.routes.read().unwrap().is_empty());
    }

    #[test]
    fn namespaced_names_sanitize_collide_and_truncate() {
        assert_eq!(namespaced_tool("srv", "ping"), "mcp__srv__ping");
        assert_eq!(
            namespaced_tool("my srv!", "fetch-url"),
            "mcp__my_srv__fetch_url"
        );
        let long_tool = "x".repeat(80);
        let name = namespaced_tool("server-with-a-long-name", &long_tool);
        assert!(name.chars().count() <= 64, "{name}");
        // 哈希算在 sanitize 之后的完整名上：截断后还能区分「前 N 字符相同」的不同工具。
        let expected = format!(
            "__{:08x}",
            fnv1a(&format!(
                "mcp__{}__{long_tool}",
                sanitize("server-with-a-long-name")
            ))
        );
        assert!(name.ends_with(&expected), "{name}");
        // 同一对名字永远得到同一个工具名：模型缓存与偏好都靠它稳定。
        assert_eq!(name, namespaced_tool("server-with-a-long-name", &long_tool));
    }

    #[test]
    fn config_validation_rejects_bad_names_and_urls() {
        let base = McpServerConfig {
            name: "ok".into(),
            transport: McpTransportConfig::Http {
                url: "https://mcp.example.com/mcp".into(),
                headers: BTreeMap::new(),
            },
            auto_connect: false,
        };
        assert!(base.validate().is_ok());

        let mut spaced = base.clone();
        spaced.name = "my server".into();
        assert!(spaced.validate().is_err());

        let mut no_url = base.clone();
        no_url.transport = McpTransportConfig::Http {
            url: "mcp.example.com".into(),
            headers: BTreeMap::new(),
        };
        assert!(no_url.validate().is_err());

        let mut no_command = base;
        no_command.transport = McpTransportConfig::Stdio {
            command: "  ".into(),
            args: vec![],
            env: BTreeMap::new(),
        };
        assert!(no_command.validate().is_err());
    }

    #[test]
    fn response_for_matches_the_id_and_surfaces_errors() {
        let ok = r#"{"jsonrpc":"2.0","id":7,"result":{"tools":[]}}"#;
        assert!(response_for(ok, 7).is_some());
        assert!(response_for(ok, 8).is_none());

        let bad = r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32601,"message":"no such tool"}}"#;
        let outcome = response_for(bad, 7).expect("error response");
        let err = outcome.expect_err("errors propagate");
        assert!(err.to_string().contains("no such tool"), "{err}");

        // 通知（无 id）与垃圾行都不该被当成响应。
        assert!(response_for(r#"{"jsonrpc":"2.0","method":"x"}"#, 7).is_none());
        assert!(response_for("not json", 7).is_none());
    }

    #[test]
    fn sse_parser_skips_comments_and_picks_the_matching_event() {
        let stream = ":heartbeat\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"note\"}\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"ok\":true}}\n\n";
        let result = sse_response(stream, 3).expect("found");
        assert_eq!(result, json!({"ok": true}));
        assert!(sse_response(stream, 4).is_err());
    }

    #[test]
    fn unsupported_server_requests_get_a_method_not_found_reply() {
        let reply =
            unsupported_request(r#"{"jsonrpc":"2.0","id":9,"method":"sampling/createMessage"}"#)
                .expect("reply");
        assert!(reply.contains("-32601"), "{reply}");
        assert!(reply.contains("sampling/createMessage"), "{reply}");
        // 纯通知不回话。
        assert!(unsupported_request(r#"{"jsonrpc":"2.0","method":"note"}"#).is_none());
    }
}
