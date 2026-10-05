// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! MCP 服务端：把本程序的画布与主循环端给外部 AI 和游戏引擎。
//!
//! 方向说明：`mcp.rs` 是「本程序当客户端」——把外部工具服务器挂给内部模型。
//! 这里是反方向，「本程序当服务端」：外部进程连进来，用同一套
//! create / paint / export 能力把 AIPixel 接进别人的 AI 开发链路。
//! 两条路共用同一个 `AppState`，不存在两份真相。
//!
//! 传输只做 loopback HTTP：外部客户端（Claude Desktop、引擎里的 HTTP 调用、
//! 一条 curl）都不难发 JSON-RPC POST。为了不把整架 HTTP 框架拖进依赖，这里
//! 手写 HTTP/1.1 的一个够用子集——一条连接一次请求，答完就关。这在语义上
//! 等价于 MCP Streamable HTTP 的单次往返，客户端不必维护长连接。
//!
//! 默认不开：这个端口能按调用方给的路径写文件，是件该由人明确点头的事。
//! 用户在设置里打开开关，地址与已服务请求数一直摆在旁边，随时能核。

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_core::AgentSession;
use pixel_core::decode;
use pixel_core::document::Document;
use pixel_core::ops::{self, PixelOperation};
use pixel_core::pixelize::{self, FitMode, PixelizeOptions};
use pixel_core::Rgba;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use crate::commands::{
    export_canvas_bytes, read_image_context, run_turn, session_info, sized_document, SessionInfo,
};
use crate::editor::{
    apply_fill, apply_paperdoll_base, apply_resize, apply_stroke, StrokeCell, StrokeRequest,
};
use crate::state::AppState;
use crate::workflow;

/// 默认监听端口。撞车了就换：端口只是入口，不是契约。
pub const DEFAULT_PORT: u16 = 7815;

/// MCP 协议版本。客户端在 initialize 里报了自己的版本，就原样回它的：
/// 版本协商错了，客户端会直接判定握手失败。
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// 一条连接里请求体的上限。够放一大批 ops，又不给一个畸形 Content-Length
/// 撑爆内存的机会。
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// 等一个 agent 回合落定时，每隔多久去看一眼画布 revision。
const SETTLE_POLL_MS: u64 = 150;

// ---------- 设置与运行状态 ----------

/// 服务端设置。和 `mcp.rs` 的服务器登记一起落在 mcp.json 里，重启后原样恢复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerSettings {
    /// 开关。默认关：要人去设置里点一下才算数。
    #[serde(default)]
    pub enabled: bool,
    /// 监听端口。0 = 让内核挑一个空闲的，挑中的值由 `McpServerStatus::port` 回报。
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for McpServerSettings {
    fn default() -> Self {
        McpServerSettings {
            enabled: false,
            port: DEFAULT_PORT,
        }
    }
}

/// 计数与生命周期信号：跨任务共享，所以各自单独包一层 Arc。
#[derive(Debug, Default)]
pub struct ServerCounters {
    requests: AtomicU64,
    running: AtomicBool,
    bound: Mutex<Option<u16>>,
}

impl ServerCounters {
    fn bump(&self) {
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    /// 实际监听中的端口。端口填 0 时这才是真入口。
    fn bound_port(&self) -> Option<u16> {
        *self
            .bound
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 回给前端的服务端状态。endpoint 直接给全串：复制粘贴给别人接就行了。
#[derive(Debug, Clone, Serialize)]
pub struct McpServerStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub endpoint: String,
    pub requests: u64,
    pub last_error: Option<String>,
}

/// 一条服务端的生命周期。shutdown 干掉，监听就收摊；counters 留着，
/// 停机之后用户仍看得到这一回服务了多少请求。
pub struct McpServerRuntime {
    settings: McpServerSettings,
    counters: Arc<ServerCounters>,
    shutdown: Option<oneshot::Sender<()>>,
    last_error: Option<String>,
}

impl Default for McpServerRuntime {
    fn default() -> Self {
        McpServerRuntime::new(McpServerSettings::default())
    }
}

impl McpServerRuntime {
    pub fn new(settings: McpServerSettings) -> Self {
        McpServerRuntime {
            settings,
            counters: Arc::new(ServerCounters::default()),
            shutdown: None,
            last_error: None,
        }
    }

    pub fn settings(&self) -> McpServerSettings {
        self.settings.clone()
    }

    /// 只改设置不动监听。真正的起停由 `start` / `stop` 负责，别在这儿混。
    pub fn set_settings(&mut self, settings: McpServerSettings) {
        self.settings = settings;
    }

    pub fn set_last_error(&mut self, error: Option<String>) {
        self.last_error = error;
    }

    pub fn status(&self) -> McpServerStatus {
        let port = self.counters.bound_port().unwrap_or(self.settings.port);
        McpServerStatus {
            enabled: self.settings.enabled,
            running: self.counters.running.load(Ordering::SeqCst),
            port,
            endpoint: format!("http://127.0.0.1:{port}/mcp"),
            requests: self.counters.requests.load(Ordering::Relaxed),
            last_error: self.last_error.clone(),
        }
    }
}

// ---------- 起停 ----------

/// 启动时调用：设置里开着才拉起来。起不来只留一条错误状态，不冒泡——
/// 端口被别的东西占着不该让整个应用起不来。
pub fn start_if_enabled(state: &Arc<AppState>, app: &AppHandle) {
    let settings = state.mcp_server().settings();
    if !settings.enabled {
        return;
    }
    if let Err(e) = start(state, Some(app), settings) {
        eprintln!("cannot start the MCP server: {e}");
        let mut runtime = state.mcp_server();
        let mut settings = runtime.settings();
        settings.enabled = false;
        runtime.set_settings(settings);
        runtime.set_last_error(Some(e));
    }
}

/// 监听一个端口。port 为 0 时由内核挑，挑中的值写回 settings，
/// 下次启动还试同一个（内核给的通常也还空着）。
pub(crate) fn start(
    state: &Arc<AppState>,
    // app 传 None 时任务照样跑：单测没有 Tauri 会话，事件总线是可选件。
    app: Option<&AppHandle>,
    settings: McpServerSettings,
) -> Result<McpServerStatus, String> {
    // 先停旧的：改端口必须让旧监听让位，否则两个监听都活着，
    // 外部连进来的那一个永远不知道改过端口。
    stop(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], settings.port));
    // bind 走 std 而不是 tokio 的：start 可能在设置页的同步命令里被调用，
    // 那条 WebView IPC 回调线程上没有运行时上下文可 await。
    let listener = std::net::TcpListener::bind(addr)
        .map_err(|e| format!("cannot bind 127.0.0.1:{}: {e}", settings.port))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot arm 127.0.0.1:{}: {e}", settings.port))?;
    let bound = listener
        .local_addr()
        .map_err(|e| format!("cannot read back the bound port: {e}"))?
        .port();

    let counters = Arc::new(ServerCounters {
        requests: AtomicU64::new(0),
        running: AtomicBool::new(true),
        bound: Mutex::new(Some(bound)),
    });
    let (tx, rx) = oneshot::channel::<()>();
    // from_std 要把 socket 登记进 tokio 的 reactor，这一步拿不到运行时句柄就
    // 直接 panic。别在 start 的调用线程上做：开开关的是设置页的同步命令
    // （WebView 的 IPC 回调线程），开机自启来自 setup 的主线程，都没有上下文，
    // 在那里 from_std 会把整个进程带走。所以接管 socket 整个搬进运行时里的
    // 任务再做——std 端的错误上面都已经同步报完返回了，剩下的失败只在任务里
    // 记一条错误状态：用户看到「没在跑」总比进程崩掉强。
    tauri::async_runtime::spawn(adopt_and_serve(
        listener,
        settings.port,
        state.clone(),
        app.cloned(),
        counters.clone(),
        rx,
    ));

    let mut runtime = state.mcp_server();
    let effective = McpServerSettings {
        enabled: true,
        port: bound,
    };
    let status = {
        runtime.set_settings(effective);
        runtime.set_last_error(None);
        // 计数换一套新的：上一个实例服务了多少请求已经翻篇，
        // 继续累加只会让人以为流量没断过。
        *runtime = McpServerRuntime {
            settings: runtime.settings(),
            counters: counters.clone(),
            shutdown: Some(tx),
            last_error: None,
        };
        runtime.status()
    };
    Ok(status)
}

/// 在运行时上下文里接管 std 监听并开始服务。见 `start` 里的说明：这一步
/// 之所以单独放进任务，就因为 `TcpListener::from_std` 需要运行时句柄。
async fn adopt_and_serve(
    listener: std::net::TcpListener,
    wanted_port: u16,
    state: Arc<AppState>,
    app: Option<AppHandle>,
    counters: Arc<ServerCounters>,
    shutdown: oneshot::Receiver<()>,
) {
    let listener = match TcpListener::from_std(listener) {
        Ok(listener) => listener,
        Err(e) => {
            // 能走到这儿说明 bind 早就成功了，只剩登记 reactor 这一步。
            // 真失败了也只是收摊、把开关拨回去，并留一条原因。落盘不管：
            // 文件里那句「开着」留着，下次启动还会再试一次，端口被临时占着
            // 的时候不该让用户重新点一遍开关。
            counters.running.store(false, Ordering::SeqCst);
            let mut runtime = state.mcp_server();
            let mut settings = runtime.settings();
            settings.enabled = false;
            runtime.set_settings(settings);
            runtime.set_last_error(Some(format!("cannot adopt 127.0.0.1:{wanted_port}: {e}")));
            return;
        }
    };
    accept_loop(listener, state, app, counters, shutdown).await;
}

/// 收摊。信号发出去才算完：监听线程要自己走完 accept 的那一轮才退。
pub(crate) fn stop(state: &AppState) {
    let live = {
        let mut runtime = state.mcp_server();
        runtime
            .shutdown
            .take()
            .map(|tx| (tx, runtime.counters.clone()))
    };
    if let Some((tx, counters)) = live {
        let _ = tx.send(());
        counters.running.store(false, Ordering::SeqCst);
    }
}

pub fn status(state: &AppState) -> McpServerStatus {
    state.mcp_server().status()
}

/// 开/关服务端。开失败时把开关拨回去并留下原因：留一个「开着但没跑」的
/// 状态，用户会以为外部已经接得进来。
pub fn set_enabled(
    state: &Arc<AppState>,
    app: Option<&AppHandle>,
    enabled: bool,
) -> Result<McpServerStatus, String> {
    if !enabled {
        {
            let mut runtime = state.mcp_server();
            let mut settings = runtime.settings();
            settings.enabled = false;
            runtime.set_settings(settings);
            runtime.set_last_error(None);
        }
        stop(state);
        state.save_mcp_file();
        return Ok(status(state));
    }
    let settings = {
        let mut runtime = state.mcp_server();
        let mut settings = runtime.settings();
        settings.enabled = true;
        runtime.set_settings(settings.clone());
        settings
    };
    match start(state, app, settings) {
        Ok(status) => {
            state.save_mcp_file();
            Ok(status)
        }
        Err(e) => {
            let mut runtime = state.mcp_server();
            let mut settings = runtime.settings();
            settings.enabled = false;
            runtime.set_settings(settings);
            runtime.set_last_error(Some(e.clone()));
            Err(e)
        }
    }
}

/// 改端口。开着就当场重启监听——旧端口上还挂着一条连接的话，
/// 新端口才是唯一入口，这点必须让用户看见。
pub fn set_port(
    state: &Arc<AppState>,
    app: Option<&AppHandle>,
    port: u16,
) -> Result<McpServerStatus, String> {
    let settings = {
        let mut runtime = state.mcp_server();
        let mut settings = runtime.settings();
        settings.port = port;
        runtime.set_settings(settings.clone());
        settings
    };
    if settings.enabled {
        start(state, app, settings)?;
    }
    state.save_mcp_file();
    Ok(status(state))
}

/// 重启：设置没变也照样走一遍。排查「外部连不上」时，按一下比解释一段管用。
pub fn restart(state: &Arc<AppState>, app: Option<&AppHandle>) -> Result<McpServerStatus, String> {
    let settings = {
        let runtime = state.mcp_server();
        runtime.settings()
    };
    if settings.enabled {
        return match start(state, app, settings) {
            Ok(status) => {
                state.save_mcp_file();
                Ok(status)
            }
            Err(e) => {
                let mut runtime = state.mcp_server();
                runtime.set_last_error(Some(e.clone()));
                Err(e)
            }
        };
    }
    stop(state);
    Ok(status(state))
}

// ---------- Tauri 命令 ----------

/// 服务端状态：开关、跑没跑、端口、地址、已服务请求数、最近一次错误。
#[tauri::command]
pub fn mcp_server_status(state: State<'_, Arc<AppState>>) -> McpServerStatus {
    status(&state)
}

#[tauri::command]
pub fn mcp_server_set_enabled(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    enabled: bool,
) -> Result<McpServerStatus, String> {
    set_enabled(&state, Some(&app), enabled)
}

#[tauri::command]
pub fn mcp_server_set_port(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    port: u16,
) -> Result<McpServerStatus, String> {
    set_port(&state, Some(&app), port)
}

#[tauri::command]
pub fn mcp_server_restart(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<McpServerStatus, String> {
    restart(&state, Some(&app))
}

// ---------- HTTP ----------

struct HttpRequest {
    method: String,
    path: String,
    body: Option<Vec<u8>>,
}

/// 从一段已收到的字节里切出一个完整请求。body 没收全就回 None，
/// 调用方接着读下一段。
fn parse_http_request(buf: &[u8]) -> Option<HttpRequest> {
    let split = find_subslice(buf, b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let first = lines.next()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_ascii_uppercase();
    let path = parts.next()?.to_string();
    let mut content_length: Option<usize> = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let body_start = split + 4;
    let body = match content_length {
        Some(0) => Some(Vec::new()),
        Some(len) => {
            // 偏移用 checked 加法算：Content-Length 完全由对方说了算，报一个
            // `usize::MAX` 就能让 `body_start + len` 在 release 下绕回一个小值，
            // 下面的切片随即「起点大于终点」当场 panic，而 debug 下是溢出 panic。
            // 两种都等于对面一句话把连接任务打死（连 413 都回不去）。算不出
            // 合法终点就一律当作「还没收全」，让读循环的 MAX_BODY_BYTES 闸门
            // 去回 413。
            let end = body_start.checked_add(len)?;
            if buf.len() < end {
                return None;
            }
            Some(buf[body_start..end].to_vec())
        }
        None => None,
    };
    Some(HttpRequest { method, path, body })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn build_http_response(status: u16, reason: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status} {reason}\r\n");
    out.push_str(&format!("Content-Type: {content_type}\r\n"));
    out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    // CORS：网页里的游戏引擎直接 fetch 这个端口，浏览器要先拿到这句放行。
    out.push_str("Access-Control-Allow-Origin: *\r\n");
    out.push_str("Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n");
    out.push_str(
        "Access-Control-Allow-Headers: content-type, authorization, mcp-protocol-version\r\n",
    );
    // 一条连接一次请求，答完就关。写明了客户端才不会再等一个 keep-alive。
    out.push_str("Connection: close\r\n\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

async fn accept_loop(
    listener: TcpListener,
    state: Arc<AppState>,
    // 事件总线可以没有：MCP 的单测就是这么跑的（传 None），
    // 真实运行时刻才由 start 传 Some(app) 进去。
    app: Option<AppHandle>,
    counters: Arc<ServerCounters>,
    mut shutdown: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            // 停机信号优先：收摊比再接一条新连接重要。
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((stream, _peer)) = accepted else { continue };
                let state = state.clone();
                let app = app.clone();
                let counters = counters.clone();
                tauri::async_runtime::spawn(async move {
                    handle_connection(stream, state, app, counters).await;
                });
            }
        }
    }
    counters.running.store(false, Ordering::SeqCst);
}

async fn handle_connection(
    mut stream: TcpStream,
    state: Arc<AppState>,
    app: Option<AppHandle>,
    counters: Arc<ServerCounters>,
) {
    let _ = stream.set_nodelay(true);
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let response = loop {
        if buf.len() > MAX_BODY_BYTES {
            break build_http_response(
                413,
                "Payload Too Large",
                "text/plain; charset=utf-8",
                b"request body too large",
            );
        }
        match stream.read(&mut chunk).await {
            // 对手方先撤了：这一轮没什么可答的。
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(_) => return,
        }
        // 一次请求可能拆在好几个 TCP 段里，收满一个完整请求才动手。
        if let Some(request) = parse_http_request(&buf) {
            break respond(request, &state, app.as_ref(), &counters).await;
        }
    };
    let _ = stream.write_all(&response).await;
    let _ = stream.flush().await;
}

async fn respond(
    request: HttpRequest,
    state: &AppState,
    app: Option<&AppHandle>,
    counters: &ServerCounters,
) -> Vec<u8> {
    match request.method.as_str() {
        // 预检：浏览器里的引擎会先发一个 OPTIONS，空答配 CORS 头就算过。
        "OPTIONS" => build_http_response(204, "No Content", "", &[]),
        // GET 是给人用的健康检查：浏览器敲一下这个地址就知道服务端活着没。
        "GET" | "HEAD" => {
            let body = json!({
                "name": "aipixel-mcp",
                "title": "AIPixel MCP Server",
                "version": env!("CARGO_PKG_VERSION"),
                "endpoint": status(state).endpoint,
                // 把请求的路径回出来：客户端常常要 POST 到 /mcp 这一个固定
                // 后缀，敲错前缀时这儿能直接看出来是哪一条。
                "requested_path": &request.path,
                "transport": "streamable-http, one request per connection",
                "requests_served": counters.requests.load(Ordering::Relaxed),
                "usage": "POST JSON-RPC 2.0 here: initialize, tools/list, tools/call",
            });
            let text = serde_json::to_string_pretty(&body).unwrap_or_default();
            build_http_response(
                200,
                "OK",
                "application/json; charset=utf-8",
                text.as_bytes(),
            )
        }
        "POST" => {
            counters.bump();
            let Some(body) = request.body else {
                let payload = error_body(Value::Null, -32700, "request body is missing".into());
                let text = serde_json::to_string(&payload).unwrap_or_default();
                return build_http_response(
                    400,
                    "Bad Request",
                    "application/json; charset=utf-8",
                    text.as_bytes(),
                );
            };
            let payload: Value = match serde_json::from_slice(&body) {
                Ok(value) => value,
                Err(e) => {
                    let payload = error_body(Value::Null, -32700, format!("body is not JSON: {e}"));
                    let text = serde_json::to_string(&payload).unwrap_or_default();
                    return build_http_response(
                        400,
                        "Bad Request",
                        "application/json; charset=utf-8",
                        text.as_bytes(),
                    );
                }
            };
            match handle_jsonrpc(state, app, payload).await {
                // 通知没有 id，按协议不答。回 202 而不是 204：空响应会让
                // 一些客户端当成传输层中断重连。
                None => {
                    build_http_response(202, "Accepted", "application/json; charset=utf-8", b"{}")
                }
                Some(json) => {
                    let text = serde_json::to_string(&json).unwrap_or_default();
                    build_http_response(
                        200,
                        "OK",
                        "application/json; charset=utf-8",
                        text.as_bytes(),
                    )
                }
            }
        }
        _ => build_http_response(
            405,
            "Method Not Allowed",
            "text/plain; charset=utf-8",
            b"use GET for a health check or POST for JSON-RPC",
        ),
    }
}

// ---------- JSON-RPC ----------

fn ok_body(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_body(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

async fn handle_jsonrpc(
    state: &AppState,
    app: Option<&AppHandle>,
    payload: Value,
) -> Option<Value> {
    match payload {
        Value::Array(items) => {
            if items.is_empty() {
                return Some(error_body(Value::Null, -32600, "empty batch".into()));
            }
            let mut out = Vec::new();
            for item in items {
                if let Some(reply) = handle_single(state, app, item).await {
                    out.push(reply);
                }
            }
            // 一整批全是通知：没什么要答的，和单条通知一样处理。
            if out.is_empty() {
                None
            } else {
                Some(Value::Array(out))
            }
        }
        Value::Object(_) => handle_single(state, app, payload).await,
        _ => Some(error_body(
            Value::Null,
            -32600,
            "a JSON-RPC message must be an object or an array of objects".into(),
        )),
    }
}

async fn handle_single(state: &AppState, app: Option<&AppHandle>, payload: Value) -> Option<Value> {
    let Some(object) = payload.as_object() else {
        return Some(error_body(
            Value::Null,
            -32600,
            "invalid request: not an object".into(),
        ));
    };
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let params = object
        .get("params")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let has_id = object.contains_key("id");
    let id = object.get("id").cloned().unwrap_or(Value::Null);
    // 通知：协议里没有 id，也不该有应答。`notifications/` 前缀一律当通知，
    // 有些客户端会给它补一个 null 的 id。
    if !has_id || method.starts_with("notifications/") {
        return None;
    }

    let outcome = match method.as_str() {
        "initialize" => Ok(initialize_result(&params)),
        "ping" => Ok(json!({})),
        // 清单是静态的，却可能被客户端反复问：每次全量重建三十来个大段中文
        // 描述的 JSON 纯属白干，还可能正好撞上一次批量工具调用。
        "tools/list" => Ok(json!({ "tools": TOOL_SPECS.clone() })),
        "tools/call" => call_tool(state, app, &params).await,
        "" => Err((-32600, "invalid request: no method".into())),
        other => Err((-32601, format!("unknown method: {other}"))),
    };
    Some(match outcome {
        Ok(result) => ok_body(id, result),
        Err((code, message)) => error_body(id, code, message),
    })
}

fn initialize_result(params: &Value) -> Value {
    // 客户端报的版本原样回。版本协商错了客户端会当握手失败，
    // 而这里并不真的依赖哪个版本的行为差异。
    let protocol_version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION)
        .to_string();
    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "aipixel",
            "title": "AIPixel",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": "AIPixel is a pixel-art canvas driven by an agent loop. \
            Start with list_sessions, then create_canvas or get_canvas. \
            Paint with paint_stroke / fill_region / apply_ops; read back the canvas \
            with get_canvas and canvas_preview; write results out with export_canvas \
            or save_project. prompt_agent hands a turn to the model configured inside \
            AIPixel so the drawing can be planned rather than hand-plotted.",
    })
}

async fn call_tool(
    state: &AppState,
    app: Option<&AppHandle>,
    params: &Value,
) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "tools/call requires a string `name`".to_string()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    if !args.is_object() {
        return Err((-32602, "tool `arguments` must be a JSON object".to_string()));
    }
    let outcome = match dispatch(state, app, name, &args).await {
        Ok(outcome) => outcome,
        Err(message) => {
            // 工具内部的失败不是协议错：模型读得懂 isError，会改参数再试。
            return Ok(json!({
                "content": [{ "type": "text", "text": message }],
                "structuredContent": { "tool": name, "isError": true, "error": message },
                "isError": true,
            }));
        }
    };
    let mut content = vec![json!({ "type": "text", "text": outcome.text })];
    if let Some((mime, data)) = outcome.image {
        content.push(json!({ "type": "image", "data": data, "mimeType": mime }));
    }
    let mut structured = outcome.structured;
    if let Some(map) = structured.as_object_mut() {
        map.insert("tool".into(), json!(name));
    }
    Ok(json!({ "content": content, "structuredContent": structured, "isError": false }))
}

// ---------- 工具目录 ----------

fn spec(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "title": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
        },
    })
}

/// 工具清单的缓存。`tools/list` 是被客户端随手调的：有的客户端每次重连、
/// 每次换模型都要问一遍，而重建一次要把三十来个大段中文描述重新拼装序列化。
/// 清单内容整个进程生命期内不变，所以算一次就够，命中时只是一次 clone。
static TOOL_SPECS: std::sync::LazyLock<Vec<Value>> = std::sync::LazyLock::new(tool_specs);

/// 16 个工具。名字一律动词开头，字段一律 snake_case 的英文：
/// 读这份清单的是外部模型，字段名越接近日常英语，它填错的概率越低。
fn tool_specs() -> Vec<Value> {
    vec![
        spec(
            "list_sessions",
            "列出 AIPixel 里所有画布会话（id、标题、宽高、帧数、revision）。调任何其他工具前先调它拿 session id。",
            json!({}),
            &[],
        ),
        spec(
            "create_canvas",
            "按指定宽高新建一个画布会话，返回新会话的 id。一个会话就是一份独立工程。",
            json!({
                "width": { "type": "integer", "description": "画布宽度（像素），1..4096" },
                "height": { "type": "integer", "description": "画布高度（像素），1..4096" },
                "name": { "type": "string", "description": "会话名（可选）" },
            }),
            &["width", "height"],
        ),
        spec(
            "drop_canvas",
            "删除一个画布会话。未导出的内容会丢，调用方自己负责先 save_project。",
            json!({ "id": { "type": "string", "description": "会话 id" } }),
            &["id"],
        ),
        spec(
            "rename_canvas",
            "改一个会话的显示名。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "title": { "type": "string", "description": "新的会话名" },
            }),
            &["id", "title"],
        ),
        spec(
            "get_canvas",
            "读一个会话的完整结构：宽高、帧（id 与时长）、图层（id、名字、配色范围、是否上锁）、调色板、命名调色板库、当前活跃图层与帧。改画布前后都该调一次。",
            json!({ "id": { "type": "string", "description": "会话 id" } }),
            &["id"],
        ),
        spec(
            "canvas_preview",
            "把某一帧合成成 PNG 回传（base64 image content）。给带视觉的模型看当前画面对不对。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "frame": { "type": "integer", "description": "帧索引，缺省当前活跃帧" },
            }),
            &["id"],
        ),
        spec(
            "paint_stroke",
            "从 (from_x, from_y) 画一条线到 (to_x, to_y)。坐标可以越界，会被夹进画布；color 传 hex（#e8823a）或 \"transparent\" 擦除；size 是笔头方块边长。layer / frame 省略时用会话当前活跃的那一层/那一帧。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "from_x": { "type": "integer" },
                "from_y": { "type": "integer" },
                "to_x": { "type": "integer" },
                "to_y": { "type": "integer" },
                "color": { "type": "string", "description": "#rrggbb 或 \"transparent\"" },
                "size": { "type": "integer", "description": "笔头大小，1..64，缺省 1" },
                "layer": { "type": "string", "description": "图层 id 或图层名，可选" },
                "frame": { "type": "string", "description": "帧 id 或帧序号，可选" },
            }),
            // color 不进 required：它是可选的，省掉时整条线都是橡皮。
            // 写成必填就是逼着外部模型每次都补一个 "transparent"。
            &["id", "from_x", "from_y", "to_x", "to_y"],
        ),
        spec(
            "fill_region",
            "从 (x, y) 起浸染一片同色区域，遇到别的颜色就停。color 传 hex 或 \"transparent\"。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "x": { "type": "integer" },
                "y": { "type": "integer" },
                "color": { "type": "string" },
                "layer": { "type": "string", "description": "图层 id 或图层名，可选" },
                "frame": { "type": "string", "description": "帧 id 或帧序号，可选" },
            }),
            &["id", "x", "y"],
        ),
        spec(
            "apply_ops",
            "按像素操作数组批量改结构：建/删/复用帧与图层、重命名、改时长、画线、画矩形、画椭圆、填充、清区、上下移动、设置帧持续时间。一次原子事务，失败整批回滚。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "ops": { "type": "array", "description": "PixelOperation 数组，每项带 \"op\" 字段" },
            }),
            &["id", "ops"],
        ),
        spec(
            "resize_canvas",
            "改画布宽高，左上角锚定，装得下的像素原样保留。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "width": { "type": "integer" },
                "height": { "type": "integer" },
            }),
            &["id", "width", "height"],
        ),
        spec(
            "lay_paperdoll_base",
            "在指定图层铺一版 RPG Maker 角色行走图白膜：4 行（下/左/右/上）x 3 或 4 列的人形剪影，按部件分区给浅灰，七级灰各自独立，方便之后按区换色。画布必须是 4 行网格（如 144x192、96x128、128x128、72x128），否则报错不铺。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "layer": { "type": "string", "description": "图层 id 或图层名，可选，缺省当前活跃层" },
            }),
            &["id"],
        ),
        spec(
            "list_export_formats",
            "列出支持的导出格式（gif / spritesheet / strip / 单帧 PNG / Aseprite）与各自含义。",
            json!({}),
            &[],
        ),
        spec(
            "export_canvas",
            "把会话导出到磁盘上的指定绝对路径。父目录不存在会自动创建。format 取 list_export_formats 返回的 id。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "format": { "type": "string", "description": "gif | sheet | strip | frame | png | aseprite | ase" },
                "path": { "type": "string", "description": "目标绝对路径" },
                "columns": { "type": "integer", "description": "spritesheet 每行几帧，0 = 排成一行" },
                "frame": { "type": "integer", "description": "format=frame/png 时导哪一帧，缺省第 0 帧" },
            }),
            &["id", "format", "path"],
        ),
        spec(
            "save_project",
            "把会话另存成 .aip 工程文件（含图层、帧、调色板，可再 import_project 回读）。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "path": { "type": "string", "description": "目标 .aip 绝对路径" },
            }),
            &["id", "path"],
        ),
        spec(
            "import_project",
            "读一个 .aip 文件并开成一个新会话，返回新会话 id。想接着旧工程改就用它。",
            json!({
                "path": { "type": "string", "description": ".aip 绝对路径" },
                "name": { "type": "string", "description": "新会话名，可选" },
            }),
            &["path"],
        ),
        spec(
            "import_image",
            "把一张位图（外部 AI 自己生出来的图）降采样量化进画布某一格，走和内置生图工作流同一套量化。二选一给图：`path` 本地文件，或 `image_base64` 内联。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "path": { "type": "string", "description": "位图绝对路径，PNG / JPEG / GIF / WebP / BMP / TIFF" },
                "image_base64": { "type": "string", "description": "内联位图，data URL 或 base64（后者还要 media_type）" },
                "media_type": { "type": "string", "description": "裸 base64 时的类型，如 image/png" },
                "layer": { "type": "string", "description": "图层 id 或名，可选" },
                "frame": { "type": "string", "description": "帧 id 或序号，可选" },
                "max_colors": { "type": "integer", "description": "最多提取多少种主色，2..256，默认 32" },
                "dither": { "type": "boolean", "description": "两个最近色之间做有序抖动，默认 false" },
                "snap_tolerance": { "type": "integer", "description": "命中已有调色板的容差，0..128，默认 12" },
                "expand_palette": { "type": "boolean", "description": "找不到近似色时是否新增调色项，默认 true" },
                "alpha_threshold": { "type": "integer", "description": "alpha 低于此值算透明，0..255，默认 128" },
                "fit": { "type": "string", "description": "contain（保比例，默认）或 stretch（拉伸铺满）" },
            }),
            &["id"],
        ),
        spec(
            "prompt_agent",
            "把一句话交给 AIPixel 内置的主循环去画（风格参照、读图、识图、正逆向提示词那一整套都会跑）。wait_ms > 0 时会等到画布 revision 变化或超时。",
            json!({
                "id": { "type": "string", "description": "会话 id" },
                "message": { "type": "string", "description": "用户想说的一句话" },
                "wait_ms": { "type": "integer", "description": "最多等多少毫秒，0 = 立即返回" },
                "reference_paths": { "type": "array", "description": "参考图绝对路径，可选" },
            }),
            &["id", "message"],
        ),
        spec(
            "interrupt_agent",
            "叫停一个会话里正在跑的回合。之后可以再发新消息。",
            json!({ "id": { "type": "string", "description": "会话 id" } }),
            &["id"],
        ),
    ]
}

// ---------- 工具分发 ----------

/// Debug 是给单测用的：断言失败时 unwrap_err 要把实际内容打出来，
/// 少了它一条失败只会干说「这是 Err」，看不出是哪一步不对。
#[derive(Debug)]
struct ToolOutcome {
    text: String,
    structured: Value,
    /// (mime_type, base64)。给了就在 content 里多一条 image。
    image: Option<(String, String)>,
}

impl ToolOutcome {
    fn text_only(text: String, structured: Value) -> Self {
        ToolOutcome {
            text,
            structured,
            image: None,
        }
    }

    fn with_image(mut self, mime: &str, data: String) -> Self {
        self.image = Some((mime.to_string(), data));
        self
    }
}

async fn dispatch(
    state: &AppState,
    app: Option<&AppHandle>,
    name: &str,
    args: &Value,
) -> Result<ToolOutcome, String> {
    match name {
        "list_sessions" => tool_list_sessions(state),
        "create_canvas" => tool_create_canvas(state, app, args),
        "drop_canvas" => tool_drop_canvas(state, app, args),
        "rename_canvas" => tool_rename_canvas(state, app, args),
        "get_canvas" => tool_get_canvas(state, args),
        "canvas_preview" => tool_canvas_preview(state, args),
        "paint_stroke" => tool_paint_stroke(state, app, args),
        "fill_region" => tool_fill_region(state, app, args),
        "apply_ops" => tool_apply_ops(state, app, args),
        "resize_canvas" => tool_resize_canvas(state, app, args),
        "lay_paperdoll_base" => tool_lay_paperdoll_base(state, app, args),
        "list_export_formats" => tool_list_export_formats(),
        "export_canvas" => tool_export_canvas(state, args),
        "save_project" => tool_save_project(state, args),
        "import_project" => tool_import_project(state, app, args),
        "import_image" => tool_import_image(state, app, args),
        "prompt_agent" => tool_prompt_agent(state, app, args).await,
        "interrupt_agent" => tool_interrupt_agent(state, args),
        other => Err(format!(
            "unknown tool \"{other}\"; call tools/list for the available set"
        )),
    }
}

// ---------- 参数读取 ----------

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("`{key}` is required and must be a non-empty string"))
}

fn arg_u32(args: &Value, key: &str) -> Result<u32, String> {
    let value = args
        .get(key)
        .ok_or_else(|| format!("`{key}` is required and must be an integer"))?;
    number_as_u32(value).ok_or_else(|| format!("`{key}` must be a non-negative integer"))
}

fn arg_i64(args: &Value, key: &str) -> Result<i64, String> {
    let value = args
        .get(key)
        .ok_or_else(|| format!("`{key}` is required and must be a number"))?;
    number_as_i64(value).ok_or_else(|| format!("`{key}` must be a number"))
}

fn arg_opt_u32(args: &Value, key: &str) -> Result<Option<u32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => number_as_u32(value)
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
    }
}

fn arg_opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 数字统一走 f64 再过一遍：模型时常把 32 写成 32.0，
/// 而 as_u64 对浮点是返回 None 的。
fn number_as_u32(value: &Value) -> Option<u32> {
    number_as_f64(value).and_then(|n| {
        if n < 0.0 || n > u32::MAX as f64 || n.fract() != 0.0 {
            None
        } else {
            Some(n as u32)
        }
    })
}

fn number_as_i64(value: &Value) -> Option<i64> {
    number_as_f64(value).and_then(|n| {
        if !n.is_finite() || n.abs() >= i64::MAX as f64 || n.fract() != 0.0 {
            None
        } else {
            Some(n as i64)
        }
    })
}

fn number_as_f64(value: &Value) -> Option<f64> {
    if let Some(n) = value.as_f64() {
        return Some(n);
    }
    value.as_str().and_then(|s| s.trim().parse::<f64>().ok())
}

// ---------- 工具实现 ----------

fn tool_list_sessions(state: &AppState) -> Result<ToolOutcome, String> {
    let infos = session_infos(state);
    let count = infos.len();
    let summary: Vec<Value> = infos
        .iter()
        .map(|info| {
            json!({
                "id": info.id,
                "title": info.title,
                "width": info.width,
                "height": info.height,
                "revision": info.revision,
                "model_label": info.model_label,
            })
        })
        .collect();
    let structured = json!({ "count": count, "sessions": summary });
    let text = if count == 0 {
        "no canvas session yet; call create_canvas first".to_string()
    } else {
        format!("{count} canvas session(s): {}", list_ids(&infos))
    };
    Ok(ToolOutcome::text_only(text, structured))
}

fn list_ids(infos: &[SessionInfo]) -> String {
    infos
        .iter()
        .map(|info| format!("{} ({}x{})", info.id, info.width, info.height))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 和侧边栏同一套排序：按用户拖动出来的 order，同值按 id 兜底。
fn session_infos(state: &AppState) -> Vec<SessionInfo> {
    let mut infos: Vec<SessionInfo> = state
        .session_ids()
        .into_iter()
        .filter_map(|id| state.session(&id).ok())
        .map(|session| session_info(&session))
        .collect();
    infos.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    infos
}

fn tool_create_canvas(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let width = arg_u32(args, "width")?;
    let height = arg_u32(args, "height")?;
    let name = arg_opt_str(args, "name");
    let doc = sized_document(width, height, name.clone())?;
    let session = state.create_session(doc, name);
    let info = session_info(&session);
    // 外部建的新画布要让界面上看得见：侧栏多一条，并且焦点挪过去。
    // 少了这一步，用户看着屏幕以为调用失败了，会原样再发一次。
    emit_session_list(state, app, Some(info.id.clone()));
    let structured = json!(info);
    let text = format!(
        "created canvas \"{}\" ({}), {}x{}",
        info.title.as_deref().unwrap_or(&info.id),
        info.id,
        info.width,
        info.height
    );
    Ok(ToolOutcome::text_only(text, structured))
}

fn tool_drop_canvas(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    state.session(&id)?;
    state.drop_session(&id);
    // 不带 focus：删掉的会话不配再抢焦点。若它正是界面当前那个，
    // 前端自己从剩下的里挑一个——界面不能继续挂在一个不存在的 id 上。
    emit_session_list(state, app, None);
    let structured = json!({ "dropped": id });
    Ok(ToolOutcome::text_only(
        format!("dropped canvas session {id}"),
        structured,
    ))
}

fn tool_rename_canvas(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let title = arg_str(args, "title")?;
    let session = state.session(&id)?;
    session.set_title(Some(title.clone()));
    // 侧栏显示的就是这个标题：不广播的话，改完名字界面还是旧名字，
    // 下一次用户改回来会发现「咦它其实已经叫这个了」。
    emit_session_list(state, app, None);
    let structured = json!({ "id": id, "title": title });
    Ok(ToolOutcome::text_only(
        format!("renamed {id} to \"{title}\""),
        structured,
    ))
}

fn tool_get_canvas(state: &AppState, args: &Value) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let session = state.session(&id)?;
    let doc = session.document();
    let active = session.active();
    let frames: Vec<Value> = doc
        .frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            json!({ "index": index, "id": frame.id, "duration_ms": frame.duration_ms })
        })
        .collect();
    let layers: Vec<Value> = doc
        .layers
        .iter()
        .map(|layer| {
            json!({
                "id": layer.id,
                "name": layer.name,
                "visible": layer.visible,
                "opacity": layer.opacity,
                "palette_id": layer.palette_id,
                "locked": layer.locked,
            })
        })
        .collect();
    let palette: Vec<String> = doc.palette.iter().map(|c| c.to_hex()).collect();
    let named: Vec<Value> = doc
        .palettes
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "builtin": p.builtin,
                "colors": p.colors.iter().map(|c| c.to_hex()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let structured = json!({
        "tool": "get_canvas",
        "id": id,
        "title": session.title(),
        "revision": doc.revision,
        "width": doc.width,
        "height": doc.height,
        "frames": frames,
        "layers": layers,
        "palette": palette,
        "palettes": named,
        "active_layer": active.layer,
        "active_frame": active.frame,
    });
    let text = format!(
        "{id}: {}x{}, {} frame(s), {} layer(s), {} color(s) in the palette, revision {}",
        doc.width,
        doc.height,
        doc.frames.len(),
        doc.layers.len(),
        doc.palette.len(),
        doc.revision
    );
    Ok(ToolOutcome::text_only(text, structured))
}

fn tool_canvas_preview(state: &AppState, args: &Value) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let frame = arg_opt_u32(args, "frame")?;
    let session = state.session(&id)?;
    let doc = session.document();
    let index = frame.unwrap_or_else(|| active_frame_index(&session, &doc));
    let image = pixel_core::png::composite_frame(&doc, index);
    let bytes = pixel_core::png::encode_png(&image)?;
    let data = pixel_core::png::base64_encode(&bytes);
    let structured = json!({
        "id": id,
        "frame": index,
        "width": doc.width,
        "height": doc.height,
        "bytes": bytes.len(),
    });
    let text = format!(
        "{id} frame {index}: {}x{} PNG, {} bytes",
        doc.width,
        doc.height,
        bytes.len()
    );
    Ok(ToolOutcome::text_only(text, structured).with_image("image/png", data))
}

fn tool_paint_stroke(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let (from_x, from_y) = (arg_i64(args, "from_x")?, arg_i64(args, "from_y")?);
    let (to_x, to_y) = (arg_i64(args, "to_x")?, arg_i64(args, "to_y")?);
    let color = parse_color(arg_opt_str(args, "color"))?;
    let size = arg_opt_u32(args, "size")?.unwrap_or(1).clamp(1, 64);
    let session = state.session(&id)?;
    let (layer, frame, cells) = {
        let doc = session.document();
        let layer = resolve_layer(&session, &doc, arg_opt_str(args, "layer").as_deref())?;
        let frame = resolve_frame(&session, &doc, arg_opt_str(args, "frame").as_deref())?;
        let cells = brush_cells(doc.width, doc.height, from_x, from_y, to_x, to_y, size);
        (layer, frame, cells)
    };
    let revision = session.with_document_mut(|doc| {
        apply_stroke(
            doc,
            &StrokeRequest {
                layer: layer.clone(),
                frame: frame.clone(),
                cells: cells.clone(),
                color: color.clone(),
            },
        )
    })?;
    session.note_edit(format!(
        "paint_stroke: {} cell(s) on {layer}/{frame} with {}",
        cells.len(),
        color.clone().unwrap_or_else(|| "transparent".into())
    ));
    emit_change(app, &session);
    // 外部进程改的画布也是用户的资产，一样要落盘。
    state.note_sessions_dirty();
    let structured = json!({
        "id": id,
        "revision": revision,
        "layer": layer,
        "frame": frame,
        "cells": cells.len(),
        "color": color,
    });
    Ok(ToolOutcome::text_only(
        format!(
            "painted {} cell(s) on {id} {layer}/{frame}; revision is now {revision}",
            cells.len()
        ),
        structured,
    ))
}

fn tool_fill_region(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let (x, y) = (arg_u32(args, "x")?, arg_u32(args, "y")?);
    let color = parse_color(arg_opt_str(args, "color"))?;
    let session = state.session(&id)?;
    let (layer, frame) = {
        let doc = session.document();
        (
            resolve_layer(&session, &doc, arg_opt_str(args, "layer").as_deref())?,
            resolve_frame(&session, &doc, arg_opt_str(args, "frame").as_deref())?,
        )
    };
    let revision =
        session.with_document_mut(|doc| apply_fill(doc, &layer, &frame, x, y, color.as_deref()))?;
    session.note_edit(format!(
        "fill_region: ({x},{y}) on {layer}/{frame} with {}",
        color.clone().unwrap_or_else(|| "transparent".into())
    ));
    emit_change(app, &session);
    // 外部进程改的画布也是用户的资产，一样要落盘。
    state.note_sessions_dirty();
    let structured =
        json!({ "id": id, "revision": revision, "layer": layer, "frame": frame, "color": color });
    Ok(ToolOutcome::text_only(
        format!(
            "filled a region on {id} {layer}/{frame} from ({x},{y}); revision is now {revision}"
        ),
        structured,
    ))
}

fn tool_apply_ops(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let raw = args
        .get("ops")
        .ok_or_else(|| "`ops` is required (an array of pixel operations)".to_string())?;
    let ops: Vec<PixelOperation> = serde_json::from_value(raw.clone())
        .map_err(|e| format!("`ops` is not a valid operation array: {e}"))?;
    if ops.is_empty() {
        return Err("`ops` is empty; nothing to apply".into());
    }
    let session = state.session(&id)?;
    let revision =
        session.with_document_mut(|doc| ops::apply_batch(doc, &ops).map_err(|e| e.to_string()))?;
    session.note_edit(format!("apply_ops: {} operation(s)", ops.len()));
    emit_change(app, &session);
    // 外部进程改的画布也是用户的资产，一样要落盘。
    state.note_sessions_dirty();
    let structured = json!({ "id": id, "revision": revision, "applied": ops.len() });
    Ok(ToolOutcome::text_only(
        format!(
            "applied {} operation(s) to {id}; revision is now {revision}",
            ops.len()
        ),
        structured,
    ))
}

fn tool_resize_canvas(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let width = arg_u32(args, "width")?;
    let height = arg_u32(args, "height")?;
    let session = state.session(&id)?;
    let before = session.document();
    let (old_width, old_height) = (before.width, before.height);
    if old_width == width && old_height == height {
        let structured =
            json!({ "id": id, "revision": before.revision, "width": width, "height": height });
        return Ok(ToolOutcome::text_only(
            format!("{id} is already {width}x{height}; nothing changed"),
            structured,
        ));
    }
    let revision = session.with_document_mut(|doc| apply_resize(doc, width, height))?;
    session.note_edit(format!(
        "canvas resized from {old_width}x{old_height} to {width}x{height}"
    ));
    emit_change(app, &session);
    // 侧栏每条会话都挂着 WxH：画布那份广播只管画面，宽高得另说一句。
    // 缩放不在高频路径上（一次改动一次 IPC），不心疼。
    emit_session_list(state, app, None);
    // 外部进程改的画布也是用户的资产，一样要落盘。
    state.note_sessions_dirty();
    let structured = json!({ "id": id, "revision": revision, "width": width, "height": height });
    Ok(ToolOutcome::text_only(
        format!("resized {id} from {old_width}x{old_height} to {width}x{height}"),
        structured,
    ))
}

fn tool_lay_paperdoll_base(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let session = state.session(&id)?;
    let (layer, before) = {
        let doc = session.document();
        (
            resolve_layer(&session, &doc, arg_opt_str(args, "layer").as_deref())?,
            doc.clone(),
        )
    };
    // 尺寸对不上就别铺：4 行网格是引擎的契约，硬铺出来的图被整行错位切开，
    // 比没有底稿更难发现。报错里把当前尺寸说清楚，调用方好自己调 resize_canvas。
    if pixel_core::paperdoll::detect(before.width, before.height).is_none() {
        return Err(format!(
            "canvas {}x{} is not a 4-row character sheet grid; resize it to one of \
             144x192, 96x128, 128x128 or 72x128 first",
            before.width, before.height
        ));
    }
    let revision = session.with_document_mut(|doc| apply_paperdoll_base(doc, &layer))?;
    session.note_edit(format!(
        "lay_paperdoll_base: layer {layer} on {}x{}",
        before.width, before.height
    ));
    emit_change(app, &session);
    // 外部进程改的画布也是用户的资产，一样要落盘。
    state.note_sessions_dirty();
    let structured = json!({
        "id": id,
        "revision": revision,
        "layer": layer,
        "width": before.width,
        "height": before.height,
        "hint": "白膜是平色剪影按部件分区（灰阶由浅到深：高光/脸/衣/发/裤/远侧肢体/五官），之后按区上色、换色轮廓都不会跑",
    });
    Ok(ToolOutcome::text_only(
        format!(
            "laid a paper-doll base on {id} {layer} ({}x{}, {} frames painted); revision is now {revision}",
            before.width, before.height, before.frames.len()
        ),
        structured,
    ))
}
fn tool_list_export_formats() -> Result<ToolOutcome, String> {
    let formats = json!([
        { "id": "gif", "extension": ".gif", "description": "无限循环的 GIF 动画，游戏原型里最省事" },
        { "id": "sheet", "extension": ".png", "description": "PNG spritesheet；columns 指定每行几帧，0 = 排成一行" },
        { "id": "strip", "extension": ".png", "description": "所有帧横向铺成一张 PNG，逐帧接缝一眼能看完" },
        { "id": "frame", "extension": ".png", "description": "单帧 PNG；frame 指定帧索引，缺省第 0 帧" },
        { "id": "png", "extension": ".png", "description": "frame 的别名，等价于 frame 0" },
        { "id": "aseprite", "extension": ".ase", "description": "Aseprite 文件，图层与帧语义原样保留" },
        { "id": "ase", "extension": ".ase", "description": "aseprite 的别名" },
        { "id": "gpl", "extension": ".gpl", "description": "GIMP Palette 调色板文件，跨工具最广" },
        { "id": "pal", "extension": ".pal", "description": "JASC PAL 调色板文件，Aseprite / Photoshop 认" },
        { "id": "act", "extension": ".act", "description": "Adobe Color Table，恒 256 槽，Autodesk / Maya / Substance 认" },
        { "id": "manifest", "extension": ".json", "description": "交付清单：尺寸、每色像素数（拼豆备料）、图层配色范围、帧时长" },
    ]);
    let structured = json!({
        "formats": formats,
        "project_formats": [
            { "id": "aip", "extension": ".aip", "description": "AIPixel 工程文件，可用 import_project 回读" },
        ],
    });
    Ok(ToolOutcome::text_only(
        "export formats: gif, sheet, strip, frame, png, aseprite, ase, gpl, pal, act, manifest; \
         project format: aip"
            .into(),
        structured,
    ))
}

fn tool_export_canvas(state: &AppState, args: &Value) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let format = arg_str(args, "format")?;
    let path = arg_str(args, "path")?;
    let columns = arg_opt_u32(args, "columns")?;
    let frame = arg_opt_u32(args, "frame")?;
    let session = state.session(&id)?;
    let doc = session.document();
    let bytes = export_canvas_bytes(&doc, &format, columns, frame)?;
    // 父目录自动建：外部脚本经常直接把导出路径指到 engine/assets/
    // 下一个还没建的子目录里。
    ensure_parent_dir(&path)?;
    std::fs::write(&path, &bytes).map_err(|e| format!("cannot write {path}: {e}"))?;
    let structured = json!({ "id": id, "format": format, "path": path, "bytes": bytes.len() });
    Ok(ToolOutcome::text_only(
        format!(
            "exported {id} as {format} to {path} ({} bytes)",
            bytes.len()
        ),
        structured,
    ))
}

fn tool_save_project(state: &AppState, args: &Value) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let path = arg_str(args, "path")?;
    let session = state.session(&id)?;
    let text = session.aip_text()?;
    ensure_parent_dir(&path)?;
    std::fs::write(&path, text.as_bytes()).map_err(|e| format!("cannot write {path}: {e}"))?;
    let structured = json!({ "id": id, "path": path, "bytes": text.len() });
    Ok(ToolOutcome::text_only(
        format!(
            "saved {id} as an .aip project to {path} ({} bytes)",
            text.len()
        ),
        structured,
    ))
}

fn tool_import_project(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let path = arg_str(args, "path")?;
    let name = arg_opt_str(args, "name");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let doc = pixel_core::aip::import_any(&text).map_err(|e| e.to_string())?;
    let doc = doc.validated().map_err(|e| e.to_string())?;
    let title = name.or_else(|| {
        Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
    });
    let session = state.create_session(doc, title);
    let info = session_info(&session);
    // 导入的就是一份新工程：和 create_canvas 一样要让用户看见并切过去。
    emit_session_list(state, app, Some(info.id.clone()));
    let structured = json!(info);
    Ok(ToolOutcome::text_only(
        format!(
            "imported {} as canvas \"{}\" ({}), {}x{}",
            path,
            info.id,
            info.title.as_deref().unwrap_or("untitled"),
            info.width,
            info.height
        ),
        structured,
    ))
}

/// 外部 AI 出的位图落到画布上：读盘 -> 解码 -> 面积平均降采样 -> 量化成索引网格。
///
/// 闭环缺的一直是这一环。`export_canvas` 能把结果递出去，可外部智能体若带着
/// 自己的生图模型，它的图也得有条正门进来；否则它只能把像素一个个手传给
/// `apply_ops`，一帧 64x64 要背 4096 个坐标。这里复用程序内建生图工作流同一套
/// `pixelize_into_cel`，内部与外部两条例落地的结果是同一份量化。
/// 位图只是原料，权威状态仍然是调色板索引网格——模型永远不手写矩阵。
fn tool_import_image(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let from_path = arg_opt_str(args, "path");
    let from_base64 = arg_opt_str(args, "image_base64");
    // 先定落点再解码：几百 KB 的解码不该挡在一个拼错的帧 id 前面。
    let (layer, frame) = {
        let session = state.session(&id)?;
        let doc = session.document();
        (
            resolve_layer(&session, &doc, arg_opt_str(args, "layer").as_deref())?,
            resolve_frame(&session, &doc, arg_opt_str(args, "frame").as_deref())?,
        )
    };
    // 二选一，且不允许都给了：静默丢掉一份会让调用方以为落下的是它给的那份。
    let (rgba, src_w, src_h) = match (from_path, from_base64) {
        (Some(path), None) => {
            let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
            // 扩展名只当解码的首选提示，认不出就交给内容嗅探。
            let media = match Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "jpg" | "jpeg" => "image/jpeg",
                "webp" => "image/webp",
                "gif" => "image/gif",
                "bmp" => "image/bmp",
                "tif" | "tiff" => "image/tiff",
                _ => "image/png",
            };
            decode::decode_image(&bytes, media).map_err(|e| format!("{path}: {e}"))?
        }
        (None, Some(encoded)) => {
            if encoded.trim_start().starts_with("data:") {
                decode::decode_data_url(&encoded).map_err(|e| format!("image_base64: {e}"))?
            } else {
                // 裸 base64 必须有媒体类型，否则连 PNG 还是 JPEG 都判断不了。
                let media = args
                    .get("media_type")
                    .and_then(Value::as_str)
                    .filter(|m| !m.trim().is_empty())
                    .ok_or_else(|| {
                        "bare base64 needs `media_type` such as image/png".to_string()
                    })?;
                let bytes =
                    decode::decode_base64(&encoded).map_err(|e| format!("image_base64: {e}"))?;
                decode::decode_image(&bytes, media).map_err(|e| format!("image_base64: {e}"))?
            }
        }
        (Some(_), Some(_)) => return Err("give either `path` or `image_base64`, not both".into()),
        (None, None) => return Err("either `path` or `image_base64` is required".into()),
    };
    // 缺省沿用内部生图工作流的那套默认值，调用方按需再收。
    let mut opts = PixelizeOptions::default();
    if let Some(v) = args.get("max_colors").and_then(Value::as_u64) {
        opts.max_colors = (v as usize).clamp(2, 256);
    }
    if let Some(v) = args.get("dither").and_then(Value::as_bool) {
        opts.dither = v;
    }
    if let Some(v) = args.get("snap_tolerance").and_then(Value::as_u64) {
        opts.snap_tolerance = (v as u32).clamp(0, 128);
    }
    if let Some(v) = args.get("expand_palette").and_then(Value::as_bool) {
        opts.expand_palette = v;
    }
    if let Some(v) = args.get("alpha_threshold").and_then(Value::as_u64) {
        opts.alpha_threshold = v.clamp(0, 255) as u8;
    }
    if args.get("fit").and_then(Value::as_str) == Some("stretch") {
        opts.fit = FitMode::Stretch;
    }
    let session = state.session(&id)?;
    let report = session.with_document_mut(|doc| {
        pixelize::pixelize_into_cel(doc, &layer, &frame, &rgba, src_w, src_h, &opts)
    })?;
    session.note_edit(format!(
        "import_image: {src_w}x{src_h} bitmap onto {layer}/{frame}"
    ));
    emit_change(app, &session);
    // 外部进程改的画布也是用户的资产，一样要落盘。
    state.note_sessions_dirty();
    let structured = json!({
        "id": id,
        "revision": session.document().revision,
        "layer": layer,
        "frame": frame,
        "source": { "width": src_w, "height": src_h },
        "fit": match report.fit { FitMode::Contain => "contain", FitMode::Stretch => "stretch" },
        "opaque_pixels": report.opaque_pixels,
        "transparent_pixels": report.transparent_pixels,
        "colors_used": report.colors_used,
        "palette_added": report.palette_added,
    });
    Ok(ToolOutcome::text_only(
        format!(
            "pixelized a {src_w}x{src_h} bitmap onto {id} {layer}/{frame}: {} opaque, {} transparent, {} color(s) used, +{} palette color(s)",
            report.opaque_pixels,
            report.transparent_pixels,
            report.colors_used,
            report.palette_added
        ),
        structured,
    ))
}

async fn tool_prompt_agent(
    state: &AppState,
    app: Option<&AppHandle>,
    args: &Value,
) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let message = arg_str(args, "message")?;
    let wait_ms = arg_opt_u32(args, "wait_ms")?.unwrap_or(0);
    let reference_paths = args
        .get("reference_paths")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let session = state.session(&id)?;
    // 先确认会话在场再起回合：不然模型收到一句「受理了」，
    // 却不知道那条消息烧在了一个不存在的会话上。
    let mut attachments = Vec::new();
    for path in &reference_paths {
        attachments.push(read_image_context(path.clone())?);
    }
    let started = session.revision();
    run_turn(
        app,
        state,
        &id,
        &message,
        attachments,
        None,
        None,
        Vec::new(),
    )?;
    let settled = if wait_ms == 0 {
        false
    } else {
        wait_for_revision(&session, started, wait_ms as u64).await
    };
    let structured = json!({
        "id": id,
        "accepted": true,
        "settled": settled,
        "revision": session.revision(),
        "wait_ms": wait_ms,
    });
    let text = if settled {
        format!(
            "the agent finished a turn on {id}; canvas revision is now {}",
            session.revision()
        )
    } else {
        format!(
            "the agent accepted the message on {id} and is still working; \
             poll get_canvas for revision changes or call interrupt_agent to stop it"
        )
    };
    Ok(ToolOutcome::text_only(text, structured))
}

/// 等到画布 revision 动一下，或者超时。
/// 超时不去 interrupt：模型可能只是在写字还没落笔，把人家的回合掐了更亏。
async fn wait_for_revision(session: &AgentSession, started: u64, wait_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(wait_ms);
    loop {
        tokio::time::sleep(Duration::from_millis(SETTLE_POLL_MS)).await;
        if session.revision() != started {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
    }
}

fn tool_interrupt_agent(state: &AppState, args: &Value) -> Result<ToolOutcome, String> {
    let id = arg_str(args, "id")?;
    let session = state.session(&id)?;
    session.interrupt();
    let structured = json!({ "id": id, "interrupted": true });
    Ok(ToolOutcome::text_only(
        format!("interrupted the running turn on {id}"),
        structured,
    ))
}

// ---------- 小工具 ----------

/// 外部改完画布也走同一条广播：前端只有一条刷新路径，
/// 用户在界面上看到的和外部模型画出的是同一份文档。
///
/// 拿 Arc 而不是借用：emit_document 要把会话挪进合并窗口尾巴上的那个任务里，
/// 借用活不到那时候。合并本身见 `broadcast::coalesced`。
fn emit_change(app: Option<&AppHandle>, session: &Arc<AgentSession>) {
    if let Some(app) = app {
        workflow::emit_document(app, session);
    }
}

/// 会话簿变化 -> 前端侧栏。
///
/// 只给 create / drop / rename / import_project 用：这四个动的是「有哪些会话」
/// 本身，画布一笔未动，前端要重画的是侧栏而不是画布。`focus` 只给新建类操作——
/// 外部模型刚建好的那个画布，用户在界面上得能看见，否则这一趟白跑。
fn emit_session_list(state: &AppState, app: Option<&AppHandle>, focus: Option<String>) {
    if let Some(app) = app {
        crate::commands::emit_session_list(app, session_infos(state), focus);
    }
}

fn active_frame_index(session: &AgentSession, doc: &Document) -> u32 {
    let active = session.active().frame;
    doc.frames
        .iter()
        .position(|frame| frame.id == active)
        .unwrap_or(0) as u32
}

/// 把外部给的图层落到真实 id 上。省略就用会话当前活跃的那一层。
/// 先按 id 认，认不出再按名字认：用户在图层面板里改过名字之后，
/// 脚本手里那个旧名字还该指到同一层。
fn resolve_layer(
    session: &AgentSession,
    doc: &Document,
    requested: Option<&str>,
) -> Result<String, String> {
    let active = session.active().layer;
    let Some(requested) = requested else {
        return Ok(if doc.layers.iter().any(|l| l.id == active) {
            active
        } else {
            // 活跃层已经被删了：落到第一层，比报一个谁也看不懂的错有用。
            doc.layers.first().map(|l| l.id.clone()).unwrap_or(active)
        });
    };
    if doc.layers.iter().any(|l| l.id == requested) {
        return Ok(requested.to_string());
    }
    if let Some(layer) = doc.layers.iter().find(|l| l.name == requested) {
        return Ok(layer.id.clone());
    }
    Err(format!(
        "unknown layer \"{requested}\"; available layers: {}",
        describe_layers(doc)
    ))
}

fn resolve_frame(
    session: &AgentSession,
    doc: &Document,
    requested: Option<&str>,
) -> Result<String, String> {
    let active = session.active().frame;
    let Some(requested) = requested else {
        return Ok(if doc.frames.iter().any(|f| f.id == active) {
            active
        } else {
            doc.frames.first().map(|f| f.id.clone()).unwrap_or(active)
        });
    };
    if doc.frames.iter().any(|f| f.id == requested) {
        return Ok(requested.to_string());
    }
    // 纯数字当索引认：get_canvas 同时给 index 和 id，外部脚本很自然会
    // 拿着 canvas_preview 用过的那个序号回来问。认不出就报错并列出可用值，
    // 不猜——猜错一帧画上去，模型还以为自己改对了地方。
    if let Ok(index) = requested.parse::<usize>() {
        if let Some(frame) = doc.frames.get(index) {
            return Ok(frame.id.clone());
        }
    }
    Err(format!(
        "unknown frame \"{requested}\"; available frames: {}",
        describe_frames(doc)
    ))
}

fn describe_layers(doc: &Document) -> String {
    doc.layers
        .iter()
        .map(|l| format!("{} (\"{}\")", l.id, l.name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn describe_frames(doc: &Document) -> String {
    doc.frames
        .iter()
        .enumerate()
        // 序号一起报：调用方拿错帧名时，照着这条就能换成对的那个。
        .map(|(index, frame)| format!("{index}={}", frame.id))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 颜色字面量 -> hex。None = 擦回透明。
/// 空值和几个同义词都当橡皮：外部脚本写 `"color": "transparent"` 比写
/// `"color": null` 顺手，而 null 在 JSON 里又常被当成「缺这个字段」。
fn parse_color(raw: Option<String>) -> Result<Option<String>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "transparent" | "erase" | "eraser" | "clear" | "none" | "null"
    ) {
        return Ok(None);
    }
    if Rgba::parse_hex(trimmed).is_none() {
        return Err(format!(
            "`color` must be a hex literal like \"#e8823a\" (or \"transparent\" to erase), got \"{trimmed}\""
        ));
    }
    Ok(Some(trimmed.to_string()))
}

/// 一条带笔头大小的笔画要落下哪些格子。步进和 ops::draw_line 同一套
/// Bresenham，每个落点再按 size 盖一个实心方块。
/// 坐标先夹进画布：外部脚本经常直接把模型输出的负坐标搬过来，
/// 夹过之后 Bresenham 的步数就有上界，不需要另设防呆上限。
fn brush_cells(w: u32, h: u32, x0: i64, y0: i64, x1: i64, y1: i64, size: u32) -> Vec<StrokeCell> {
    let clamp = |value: i64, limit: u32| value.clamp(0, limit as i64 - 1);
    let (x0, y0) = (clamp(x0, w), clamp(y0, h));
    let (x1, y1) = (clamp(x1, w), clamp(y1, h));
    let size = size as i64;
    let half = (size - 1) / 2;
    let (w, h) = (w as i64, h as i64);
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    let mut cells: Vec<StrokeCell> = Vec::new();
    // 笔头方块：以落点为中心盖 size×size，越界的角丢掉。去重不能省——
    // 同一格落两次会让上层看到的 cells 数虚高，笔画预算也白白翻倍。
    let mut stamp = |cx: i64, cy: i64| {
        for oy in -half..(size - half) {
            for ox in -half..(size - half) {
                let (x, y) = (cx + ox, cy + oy);
                if x < 0 || y < 0 || x >= w || y >= h {
                    continue;
                }
                let cell = StrokeCell {
                    x: x as u32,
                    y: y as u32,
                };
                if seen.insert((cell.x, cell.y)) {
                    cells.push(cell);
                }
            }
        }
    };
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx - dy;
    let (mut cx, mut cy) = (x0, y0);
    loop {
        stamp(cx, cy);
        if cx == x1 && cy == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 > -dy {
            err -= dy;
            cx += sx;
        }
        if e2 < dx {
            err += dx;
            cy += sy;
        }
    }
    cells
}

fn ensure_parent_dir(path: &str) -> Result<(), String> {
    let Some(parent) = Path::new(path).parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每份用例一份独立 state：自带临时配置目录，并行用例互不干扰，
    /// 也不会往仓库里写含 api_key 的模型配置。
    fn state() -> AppState {
        AppState::default()
    }

    /// 拼一段原始 HTTP 报文。body 给 None 时不带 Content-Length。
    fn http(method: &str, path: &str, body: Option<&str>) -> Vec<u8> {
        let mut out = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
        if let Some(body) = body {
            out.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        out.push_str("\r\n");
        let mut bytes = out.into_bytes();
        if let Some(body) = body {
            bytes.extend_from_slice(body.as_bytes());
        }
        bytes
    }

    /// 一条连接一次请求：写进去，读到服务端关连接为止。
    async fn round_trip(addr: &SocketAddr, request: &[u8]) -> Vec<u8> {
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        stream.write_all(request).await.expect("write");
        stream.flush().await.expect("flush");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read to EOF");
        buf
    }

    fn text_of(body: &[u8]) -> String {
        String::from_utf8_lossy(body).into_owned()
    }

    /// parse 的分界：body 没收全就必须说「还没齐」，不能把半个请求当整条用。
    #[test]
    fn parse_http_request_waits_for_the_whole_body() {
        let parsed = parse_http_request(&http("GET", "/mcp", None)).expect("无 body 也完整");
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.path, "/mcp");
        assert_eq!(parsed.body, None);

        // body 只到了一半：回 None，调用方接着读下一段。
        let mut partial = http("POST", "/mcp", Some("{}"));
        partial.truncate(partial.len() - 1);
        assert!(parse_http_request(&partial).is_none());

        let whole = http("POST", "/mcp", Some("{}"));
        let parsed = parse_http_request(&whole).expect("body 齐了就该认");
        assert_eq!(parsed.body.as_deref(), Some(&b"{}"[..]));

        // Content-Length: 0 是完整请求：空 body 而不是「没有 body」。
        let empty = b"POST /mcp HTTP/1.1\r\ncontent-length: 0\r\n\r\n";
        assert_eq!(
            parse_http_request(empty).unwrap().body.as_deref(),
            Some(&[][..])
        );

        // 没有 Content-Length 的 POST：body 当 None，respond 会回 400，
        // 不会把下一条连接的数据误当成这一条的 body。
        let no_len = b"POST /mcp HTTP/1.1\r\n\r\n{\"a\":1}";
        assert_eq!(parse_http_request(no_len).unwrap().body, None);

        // 方法名大小写不敏感。
        let lower = b"get /mcp HTTP/1.1\r\n\r\n";
        assert_eq!(parse_http_request(lower).unwrap().method, "GET");
    }

    /// 荒唐的 Content-Length 只能把请求判成「没收全」，绝不能把连接任务打死。
    /// 曾经这里直接算 `body_start + len`：release 下大数绕回小值、切片起点大于
    /// 终点当场 panic；debug 下是溢出 panic。对面只要写一行超长长度就能把这一条
    /// 连接炸掉，连 413 都回不去。
    #[test]
    fn parse_http_request_survives_an_absurd_content_length() {
        for huge in [usize::MAX, usize::MAX - 4, usize::MAX / 2] {
            let raw = format!("POST /mcp HTTP/1.1\r\ncontent-length: {huge}\r\n\r\n{{}}");
            assert!(
                parse_http_request(raw.as_bytes()).is_none(),
                "报 {huge} 时该判成没收全，而不是算出个非法终点"
            );
        }
        // 界内但还没收全的，仍旧只是「再等等」，别把正常的大 body 一并误伤。
        let raw = "POST /mcp HTTP/1.1\r\ncontent-length: 100\r\n\r\n{}";
        assert!(parse_http_request(raw.as_bytes()).is_none());
    }

    /// 响应头是客户端能不能接上的关键：少一个 CORS 头，浏览器里的引擎就过不去。
    #[test]
    fn build_http_response_declares_what_clients_need() {
        let body = b"{}";
        let text = text_of(&build_http_response(200, "OK", "application/json", body));
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains(&format!("Content-Length: {}\r\n", body.len())));
        assert!(text.contains("Access-Control-Allow-Origin: *"));
        assert!(text.contains("Access-Control-Allow-Methods:"));
        assert!(text.contains("Access-Control-Allow-Headers:"));
        // 写得明白，客户端才不会守着这个连接等 keep-alive。
        assert!(text.contains("Connection: close"));
        assert!(text.ends_with("\r\n\r\n{}"));
    }

    #[test]
    fn parse_color_reads_erase_words_as_transparent() {
        assert_eq!(parse_color(None).unwrap(), None);
        // 外部脚本写 "transparent" 比写 null 顺手，几种口语都要收。
        for word in [
            "transparent",
            " ERASE ",
            "Eraser",
            "clear",
            "none",
            "null",
            "",
        ] {
            assert_eq!(
                parse_color(Some(word.to_string())).unwrap(),
                None,
                "{word:?} 应当当橡皮"
            );
        }
        assert_eq!(
            parse_color(Some(" #e8823a ".into())).unwrap().unwrap(),
            "#e8823a"
        );
        // 报错要给得出能照做的格式，不能让模型瞎猜。
        let err = parse_color(Some("orange".into())).unwrap_err();
        assert!(err.contains("hex"), "错误要说清格式：{err}");
    }

    #[test]
    fn brush_cells_clamp_out_of_bounds_and_never_stamp_twice() {
        // 负坐标夹到 0：外部脚本常把模型输出的负坐标原样搬过来。
        let cells = brush_cells(8, 8, -3, -3, 2, 2, 1);
        assert!(cells.iter().all(|c| c.x < 8 && c.y < 8));

        // 一条斜线配 3x3 笔头：每个格子只能落一次，重复落会让
        // MAX_STROKE_CELLS 之类的上限被同一批格子白白占掉。
        let mut seen = HashSet::new();
        for cell in brush_cells(8, 8, 1, 1, 6, 6, 3) {
            assert!(seen.insert((cell.x, cell.y)), "同一个格子落了两次");
        }

        assert_eq!(brush_cells(8, 8, 3, 3, 3, 3, 1).len(), 1);
        assert_eq!(brush_cells(64, 64, 32, 32, 32, 32, 3).len(), 9);
        assert_eq!(brush_cells(64, 64, 0, 0, 0, 0, 2).len(), 4);
    }

    /// 模型把 7 写成 7.0 是常态，as_u64 会拒；字符串数字也得收。
    #[test]
    fn numbers_accept_what_models_actually_send() {
        assert_eq!(number_as_u32(&json!(7)), Some(7));
        assert_eq!(number_as_u32(&json!(7.0)), Some(7));
        assert_eq!(number_as_u32(&json!("7")), Some(7));
        assert_eq!(number_as_u32(&json!(-1)), None);
        assert_eq!(number_as_u32(&json!(1.5)), None);
        assert_eq!(number_as_i64(&json!(-4)), Some(-4));
        assert_eq!(number_as_i64(&json!(-4.0)), Some(-4));
        assert_eq!(number_as_i64(&json!(1.25)), None);

        // 可选字段：缺和 null 都当没给， Schema 里就是这么说的。
        assert_eq!(arg_opt_u32(&json!({}), "columns").unwrap(), None);
        assert_eq!(
            arg_opt_u32(&json!({"columns": null}), "columns").unwrap(),
            None
        );
        assert_eq!(
            arg_opt_u32(&json!({"columns": "3"}), "columns").unwrap(),
            Some(3)
        );
        assert!(arg_str(&json!({"id": "  "}), "id")
            .unwrap_err()
            .contains("required"));
        assert!(arg_str(&json!({"id": "s1"}), "id").is_ok());
        assert!(arg_str(&json!({"id": 7}), "id").is_err());
        assert!(arg_u32(&json!({}), "width").is_err());
    }

    /// 默认必须关：这个端口能按调用方给的路径写文件，不能替用户点头。
    #[test]
    fn server_settings_default_to_off_and_status_names_the_endpoint() {
        let settings = McpServerSettings::default();
        assert!(!settings.enabled);
        assert_eq!(settings.port, DEFAULT_PORT);

        // 老 mcp.json 里没有 server 字段：补齐默认值而不是解析失败。
        let back: McpServerSettings = serde_json::from_str("{}").unwrap();
        assert!(!back.enabled);
        assert_eq!(back.port, DEFAULT_PORT);

        let runtime = McpServerRuntime::new(settings);
        let status = runtime.status();
        assert!(!status.enabled);
        assert!(!status.running);
        assert_eq!(status.requests, 0);
        // 没绑过端口时回设置里的那个：endpoint 始终给得出一个能看的地址。
        assert_eq!(
            status.endpoint,
            format!("http://127.0.0.1:{DEFAULT_PORT}/mcp")
        );

        // 端口填 0 时，状态要报内核挑中的那个真端口。
        let runtime = McpServerRuntime::new(McpServerSettings {
            enabled: true,
            port: 0,
        });
        runtime.counters.bound.lock().unwrap().replace(19099);
        let status = runtime.status();
        assert_eq!(status.port, 19099);
        assert_eq!(status.endpoint, "http://127.0.0.1:19099/mcp");
    }

    /// 工具清单是外部模型的「说明书」：名字、参数、可选性都在这儿。
    #[test]
    fn tool_specs_cover_the_closed_loop_and_keep_color_optional() {
        let specs = tool_specs();
        let expected = [
            "list_sessions",
            "create_canvas",
            "drop_canvas",
            "rename_canvas",
            "get_canvas",
            "canvas_preview",
            "paint_stroke",
            "fill_region",
            "apply_ops",
            "resize_canvas",
            "lay_paperdoll_base",
            "list_export_formats",
            "export_canvas",
            "save_project",
            "import_project",
            "import_image",
            "prompt_agent",
            "interrupt_agent",
        ];
        let names: Vec<&str> = specs.iter().filter_map(|s| s["name"].as_str()).collect();
        for want in expected {
            assert!(names.contains(&want), "工具清单少了 {want}");
        }
        assert_eq!(names.len(), expected.len(), "多出来的工具也要有名字");

        // color 不进 required：省掉就是橡皮。写成必填等于逼着外部模型
        // 每次擦除都补一个 "transparent"。
        let stroke = specs
            .iter()
            .find(|s| s["name"] == "paint_stroke")
            .expect("paint_stroke 在清单里");
        let required = stroke["inputSchema"]["required"].as_array().unwrap();
        assert!(!required.iter().any(|v| v == "color"));
        assert!(required.iter().any(|v| v == "from_x"));

        // 每个工具都得有 description：模型是照它决定调不调的。
        for spec in &specs {
            let description = spec["description"].as_str().unwrap_or_default();
            assert!(!description.is_empty(), "{} 缺说明", spec["name"]);
        }
    }

    #[tokio::test]
    async fn layer_and_frame_resolution_takes_id_name_and_index() {
        let state = state();
        let doc = sized_document(8, 8, Some("resolve".into())).unwrap();
        let session = state.create_session(doc, Some("resolve".into()));
        let guard = session.document();
        let layer = guard.layers[0].clone();
        let frame = guard.frames[0].clone();

        // 省略：用会话当前活跃的那一个。
        assert_eq!(resolve_frame(&session, &guard, None).unwrap(), frame.id);
        assert_eq!(resolve_layer(&session, &guard, None).unwrap(), layer.id);
        // id 和名字都认：用户在面板里改过名字，旧名字还该指到同一层。
        assert_eq!(
            resolve_layer(&session, &guard, Some(&layer.name)).unwrap(),
            layer.id
        );
        // 纯数字当序号认：get_canvas 把 index 和 id 一起给了外部，
        // 脚本很自然会拿 canvas_preview 用过的序号回来问。
        assert_eq!(
            resolve_frame(&session, &guard, Some("0")).unwrap(),
            frame.id
        );

        // 认不出时报错要列出可用值，帧还要报成 index=id 对。
        let err = resolve_frame(&session, &guard, Some("F99")).unwrap_err();
        assert!(err.contains("F99"), "{err}");
        assert!(err.contains("0="), "可用帧要连序号一起报：{err}");
        let err = resolve_layer(&session, &guard, Some("ghost")).unwrap_err();
        assert!(
            err.contains("ghost") && err.contains("available layers"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn jsonrpc_layer_speaks_the_protocol() {
        let state = state();

        // initialize 回声客户端报的版本：协商错了客户端会当握手失败。
        let reply = handle_jsonrpc(
            &state,
            None,
            json!({"jsonrpc": "2.0", "id": 1, "params": {"protocolVersion": "2024-11-05"}, "method": "initialize"}),
        )
        .await
        .expect("initialize 要应答");
        assert_eq!(reply["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(reply["result"]["serverInfo"]["name"], "aipixel");
        assert_eq!(
            reply["result"]["capabilities"]["tools"]["listChanged"],
            false
        );

        // ping 空结果。
        let reply = handle_jsonrpc(
            &state,
            None,
            json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
        )
        .await
        .unwrap();
        assert_eq!(reply["result"], json!({}));

        // tools/list 全量在手：外部客户端不先 list 就不知道有什么可调。
        let reply = handle_jsonrpc(
            &state,
            None,
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
        )
        .await
        .unwrap();
        assert_eq!(reply["result"]["tools"].as_array().unwrap().len(), 18);

        // 未知方法是协议错。
        let reply = handle_jsonrpc(
            &state,
            None,
            json!({"jsonrpc": "2.0", "id": 4, "method": "nope"}),
        )
        .await
        .unwrap();
        assert_eq!(reply["error"]["code"], -32601);

        // 未知工具不是协议错：模型读得懂 isError，会改参数再试。
        let reply = handle_jsonrpc(
            &state,
            None,
            json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "draw_dragon"}}),
        )
        .await
        .unwrap();
        assert!(reply.get("error").is_none(), "工具失败不该长成协议错");
        assert_eq!(reply["result"]["isError"], true);
        assert!(reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));

        // 缺 name 是参数错。
        let reply = handle_jsonrpc(
            &state,
            None,
            json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {}}),
        )
        .await
        .unwrap();
        assert_eq!(reply["error"]["code"], -32602);

        // 通知没有 id，一律不答。
        assert!(
            handle_jsonrpc(&state, None, json!({"jsonrpc": "2.0", "method": "ping"}))
                .await
                .is_none()
        );
        assert!(handle_jsonrpc(
            &state,
            None,
            json!({"jsonrpc": "2.0", "id": null, "method": "notifications/initialized"})
        )
        .await
        .is_none());

        // 批量：数组进数组出；整批全是通知就没有答复。
        let reply = handle_jsonrpc(
            &state,
            None,
            json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}, {"jsonrpc": "2.0", "method": "ping"}]),
        )
        .await
        .expect("批里有一条要答");
        assert_eq!(reply.as_array().unwrap().len(), 1);
        assert!(
            handle_jsonrpc(&state, None, json!([{"jsonrpc": "2.0", "method": "ping"}]))
                .await
                .is_none()
        );

        // 空批、不是对象：-32600。
        let reply = handle_jsonrpc(&state, None, json!([])).await.unwrap();
        assert_eq!(reply["error"]["code"], -32600);
        let reply = handle_jsonrpc(&state, None, json!("ping")).await.unwrap();
        assert_eq!(reply["error"]["code"], -32600);
    }

    /// 从创建到导出的完整闭环：外部 AI / 游戏引擎要的就是这一条路。
    #[tokio::test]
    async fn the_whole_loop_from_create_to_export_round_trips() {
        let state = state();
        let root = std::env::temp_dir().join("aipixel-mcp-closure");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // 空簿子：list 要说清该先 create_canvas，别只回一个 0。
        let out = dispatch(&state, None, "list_sessions", &json!({}))
            .await
            .unwrap();
        assert_eq!(out.structured["count"], 0);
        assert!(out.text.contains("create_canvas"));

        let out = dispatch(
            &state,
            None,
            "create_canvas",
            &json!({"width": 8, "height": 6, "name": "cat"}),
        )
        .await
        .unwrap();
        let id = out.structured["id"].as_str().unwrap().to_string();
        assert_eq!(out.structured["width"], 8);
        assert_eq!(out.structured["height"], 6);

        let out = dispatch(&state, None, "list_sessions", &json!({}))
            .await
            .unwrap();
        assert_eq!(out.structured["count"], 1);
        assert!(out.text.contains(&id));

        // get_canvas 要报全结构：帧、图层、命名配色范围都在。
        let out = dispatch(&state, None, "get_canvas", &json!({"id": id}))
            .await
            .unwrap();
        assert_eq!(out.structured["frames"].as_array().unwrap().len(), 1);
        assert!(!out.structured["layers"].as_array().unwrap().is_empty());
        assert!(!out.structured["palettes"].as_array().unwrap().is_empty());

        // 画一条线：cells 数就是 Bresenham 的落点数。
        let out = dispatch(
            &state,
            None,
            "paint_stroke",
            // frame 用序号「0」：见 resolve_frame 的约定。
            &json!({"id": id, "from_x": 1, "from_y": 1, "to_x": 6, "to_y": 1, "color": "#e8823a", "frame": "0"}),
        )
        .await
        .unwrap();
        assert_eq!(out.structured["cells"], 6);
        assert_eq!(out.structured["frame"], "F0");
        let painted = out.structured["revision"].as_u64().unwrap();
        // 像素真的落下去了：document() 拿得到那一格。
        let doc = state.session(&id).unwrap().document();
        assert_eq!(doc.width, 8);

        // 同一条线再擦一次：橡皮也走 stroke，revision 照涨。
        let out = dispatch(
            &state,
            None,
            "paint_stroke",
            &json!({"id": id, "from_x": 1, "from_y": 1, "to_x": 6, "from_y_unused": 0, "to_y": 1, "color": "transparent"}),
        )
        .await;
        // 上面故意多带一个不认识的字段：字段多余不该让整条请求失败。
        let out = out.unwrap();
        assert_eq!(out.structured["cells"], 6);
        assert!(out.structured["revision"].as_u64().unwrap() > painted);

        let filled_at = dispatch(
            &state,
            None,
            "fill_region",
            &json!({"id": id, "x": 0, "y": 0, "color": "#ff004d"}),
        )
        .await
        .unwrap()
        .structured["revision"]
            .as_u64()
            .unwrap();

        // 预览要带 PNG 回来：带视觉的模型靠这一眼判断画得对不对。
        let out = dispatch(&state, None, "canvas_preview", &json!({"id": id}))
            .await
            .unwrap();
        let (mime, data) = out.image.expect("预览要带图");
        assert_eq!(mime, "image/png");
        assert!(!data.is_empty());
        assert!(out.structured["bytes"].as_u64().unwrap() > 0);

        // 批量 ops：建帧、改时长、画矩形一趟办完。
        let out = dispatch(
            &state,
            None,
            "apply_ops",
            &json!({"id": id, "ops": [
                {"op": "create_frame", "duration_ms": 120},
                {"op": "set_frame_duration", "frame": "F0", "duration_ms": 100},
                {"op": "draw_shape", "layer": "L0", "frame": "F0", "shape": "rect", "x0": 0, "y0": 0, "x1": 3, "y1": 3, "color": "#ff004d"},
            ]}),
        )
        .await
        .unwrap();
        assert_eq!(out.structured["applied"], 3);
        let applied = out.structured["revision"].as_u64().unwrap();
        assert_eq!(state.session(&id).unwrap().document().frames.len(), 2);

        // 坏 ops 整批拒绝，revision 一步都不许动：原子事务是契约。
        let bad = dispatch(
            &state,
            None,
            "apply_ops",
            &json!({"id": id, "ops": [{"op": "rename_layer", "layer": "ghost", "name": "x"}]}),
        )
        .await;
        assert!(bad.is_err(), "坏操作必须整批回滚");
        assert_eq!(state.session(&id).unwrap().revision(), applied);
        assert!(
            dispatch(&state, None, "apply_ops", &json!({"id": id, "ops": []}),)
                .await
                .is_err()
        );

        // resize：相同尺寸再说一次不报错也不动 revision。
        let out = dispatch(
            &state,
            None,
            "resize_canvas",
            &json!({"id": id, "width": 10, "height": 10}),
        )
        .await
        .unwrap();
        let resized = out.structured["revision"].as_u64().unwrap();
        assert_eq!(out.structured["width"], 10);
        let again = dispatch(
            &state,
            None,
            "resize_canvas",
            &json!({"id": id, "width": 10, "height": 10}),
        )
        .await
        .unwrap();
        assert!(again.text.contains("already"), "{:?}", again.text);
        assert_eq!(again.structured["revision"].as_u64().unwrap(), resized);
        assert!(again.structured["revision"].as_u64().unwrap() > filled_at);

        // 改名。
        let out = dispatch(
            &state,
            None,
            "rename_canvas",
            &json!({"id": id, "title": "橘猫"}),
        )
        .await
        .unwrap();
        assert_eq!(out.structured["title"], "橘猫");

        // 导出各格式：魔数对得上，字节数和落盘的一致。
        for format in ["png", "sheet", "strip", "frame", "gif", "aseprite", "ase"] {
            let extension = if format.contains("ase") {
                "ase"
            } else {
                format
            };
            let path = root.join(format!("out.{extension}"));
            let out = dispatch(
                &state,
                None,
                "export_canvas",
                &json!({"id": id, "format": format, "path": path.to_str().unwrap()}),
            )
            .await
            .unwrap_or_else(|e| panic!("导出 {format} 失败：{e}"));
            let bytes = std::fs::read(&path).unwrap();
            assert!(!bytes.is_empty(), "{format} 导出空文件");
            assert_eq!(
                out.structured["bytes"].as_u64().unwrap() as usize,
                bytes.len()
            );
        }
        assert!(std::fs::read(root.join("out.png"))
            .unwrap()
            .starts_with(b"\x89PNG"));
        assert!(std::fs::read(root.join("out.gif"))
            .unwrap()
            .starts_with(b"GIF89a"));
        // ASE 文件头 magic 0xA5E0 小端。
        assert_eq!(
            &std::fs::read(root.join("out.ase")).unwrap()[4..6],
            &[0xE0, 0xA5]
        );

        // 认不出的格式要拒，并提示去哪查清单。
        let err = dispatch(
            &state,
            None,
            "export_canvas",
            &json!({"id": id, "format": "bmp", "path": root.join("x.bmp").to_str().unwrap()}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("unsupported export format"), "{err}");
        let out = dispatch(&state, None, "list_export_formats", &json!({}))
            .await
            .unwrap();
        let ids: Vec<&str> = out.structured["formats"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f["id"].as_str())
            .collect();
        for want in [
            "gif", "sheet", "strip", "frame", "png", "aseprite", "ase", "gpl", "pal", "act",
            "manifest",
        ] {
            assert!(ids.contains(&want), "格式清单少了 {want}");
        }
        assert_eq!(out.structured["project_formats"][0]["id"], "aip");

        // 调色板文件与交付清单也要真能导出来，不然清单上多一行就是空话。
        for (format, magic) in [
            ("gpl", b"GIMP Palette\n".to_vec()),
            ("pal", b"RIFF\ndata\n".to_vec()),
            ("manifest", b"{\n".to_vec()),
        ] {
            let path = root.join(format!("cat.{format}"));
            dispatch(
                &state,
                None,
                "export_canvas",
                &json!({"id": id, "format": format, "path": path.to_str().unwrap()}),
            )
            .await
            .unwrap();
            let bytes = std::fs::read(&path).unwrap();
            assert!(
                bytes.starts_with(&magic),
                "{format} 的文件头不对：{bytes:?}"
            );
        }
        // ACT 是二进制魔数 8BCB，长度恒 778。
        let act = root.join("cat.act");
        dispatch(
            &state,
            None,
            "export_canvas",
            &json!({"id": id, "format": "act", "path": act.to_str().unwrap()}),
        )
        .await
        .unwrap();
        assert_eq!(
            &std::fs::read(&act).unwrap()[..4],
            &[0x38, 0x42, 0x43, 0x42]
        );

        // 导出到还没建的深层目录：脚本常直接指 engine/assets/ 下不存在的子目录。
        let deep = root.join("engine/assets/sprites/cat.png");
        dispatch(
            &state,
            None,
            "export_canvas",
            &json!({"id": id, "format": "png", "path": deep.to_str().unwrap()}),
        )
        .await
        .unwrap();
        assert!(deep.exists(), "父目录要自动建");

        // 工程存档往返：存出去、读回来开成新会话。
        let aip = root.join("cat.aip");
        dispatch(
            &state,
            None,
            "save_project",
            &json!({"id": id, "path": aip.to_str().unwrap()}),
        )
        .await
        .unwrap();
        let out = dispatch(
            &state,
            None,
            "import_project",
            &json!({"path": aip.to_str().unwrap(), "name": "回读"}),
        )
        .await
        .unwrap();
        let back = out.structured["id"].as_str().unwrap().to_string();
        assert_ne!(back, id, "回读要开成新会话");
        assert_eq!(out.structured["width"], 10);

        // 删会话：之后任何针对它的调用都要带回得上话的错。
        dispatch(&state, None, "drop_canvas", &json!({"id": id}))
            .await
            .unwrap();
        let err = dispatch(&state, None, "get_canvas", &json!({"id": id}))
            .await
            .unwrap_err();
        assert!(err.contains(&id), "{err}");
        dispatch(&state, None, "drop_canvas", &json!({"id": back}))
            .await
            .unwrap();
        let out = dispatch(&state, None, "list_sessions", &json!({}))
            .await
            .unwrap();
        assert_eq!(out.structured["count"], 0);

        // prompt_agent / interrupt_agent 的会话存在性校验：不存在的会话
        // 要当场报错，不能收下一句「受理了」然后烧在空气上。
        assert!(dispatch(
            &state,
            None,
            "prompt_agent",
            &json!({"id": "ghost", "message": "画只猫"}),
        )
        .await
        .is_err());
        assert!(
            dispatch(&state, None, "interrupt_agent", &json!({"id": "ghost"}))
                .await
                .is_err()
        );
        // 缺必填参数也要拒：arg_str 的文案要点得出缺哪个字段。
        let err = dispatch(&state, None, "create_canvas", &json!({"width": 8}))
            .await
            .unwrap_err();
        assert!(err.contains("height"), "{err}");
    }

    /// 造一张 2x2 的 PNG：左上格不透明红，其余透明。用它验量化落盘。
    fn two_red_pixels_png() -> Vec<u8> {
        let mut img = image::ImageBuffer::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([220, 60, 40, 255]));
        for (x, y) in [(1, 0), (0, 1), (1, 1)] {
            img.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
        }
        pixel_core::png::encode_png(&img).unwrap()
    }

    /// 外部 AI 自己出的图要能落进画布：这是「AI 开发闭环」缺的那一环。
    #[tokio::test]
    async fn an_external_bitmap_lands_on_the_canvas() {
        let state = state();
        let root = std::env::temp_dir().join("aipixel-mcp-import-image");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let png = root.join("hero.png");
        std::fs::write(&png, two_red_pixels_png()).unwrap();

        let out = dispatch(
            &state,
            None,
            "create_canvas",
            &json!({"width": 8, "height": 8, "name": "hero"}),
        )
        .await
        .unwrap();
        let id = out.structured["id"].as_str().unwrap().to_string();

        // 落点先校验：拼错的帧名不许烧掉一次解码。
        let err = dispatch(
            &state,
            None,
            "import_image",
            &json!({"id": id, "path": png.to_str().unwrap(), "frame": "F99"}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("F99"), "{err}");

        // 正式落一次：量化进活跃层/帧，调色板长出色，revision 跟着涨。
        let out = dispatch(
            &state,
            None,
            "import_image",
            &json!({"id": id, "path": png.to_str().unwrap()}),
        )
        .await
        .unwrap();
        assert_eq!(out.structured["source"]["width"], 2);
        assert_eq!(out.structured["source"]["height"], 2);
        assert!(out.structured["opaque_pixels"].as_u64().unwrap() > 0);
        assert!(out.structured["palette_added"].as_u64().unwrap() > 0);
        assert!(out.structured["revision"].as_u64().unwrap() > 0);

        // 两张来源同时给、以及一张都不给，都要当场拒：
        // 静默丢掉一份会让调用方以为落下的是它给的那份。
        let err = dispatch(
            &state,
            None,
            "import_image",
            &json!({"id": id, "path": png.to_str().unwrap(), "image_base64": "AAAA"}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("not both"), "{err}");
        let err = dispatch(&state, None, "import_image", &json!({"id": id}))
            .await
            .unwrap_err();
        assert!(err.contains("required"), "{err}");
        // 文件不存在也要把路径念出来，别只说一句读失败。
        let err = dispatch(
            &state,
            None,
            "import_image",
            &json!({"id": id, "path": root.join("ghost.png").to_str().unwrap()}),
        )
        .await
        .unwrap_err();
        assert!(err.contains("ghost.png"), "{err}");
    }

    /// 回归：设置页里打开「本程序当 MCP 端口」的开关，start 跑在 WebView 的
    /// IPC 回调线程上，那条线程没有 tokio 运行时上下文。从前接管 socket 的
    /// `TcpListener::from_std` 就写在那儿，`Handle::current` 一提不到运行时
    /// 句柄直接 panic，整个进程跟着崩——用户看到的就是一打开开关应用消失。
    /// 开机自启走 setup 的主线程，同样没有上下文，一条用例把两条路都罩住。
    #[test]
    fn starting_off_the_runtime_context_serves_instead_of_panicking() {
        let state = Arc::new(state());
        // 一条彻底的裸线程：没有任何 tokio 运行时上下文，跟出事的位置一样。
        let started = std::thread::spawn({
            let state = state.clone();
            move || {
                start(
                    &state,
                    None,
                    McpServerSettings {
                        enabled: true,
                        // 端口 0：让内核挑个空闲的，别跟别的用例抢号。
                        port: 0,
                    },
                )
            }
        });
        let serving = started
            .join()
            .expect("start 不该在没有运行时上下文的线程上 panic")
            .expect("端口要起得来");
        assert!(serving.enabled);
        assert!(serving.running, "起不来要说清楚：{:?}", serving.last_error);
        assert_ne!(serving.port, 0, "端口要回报内核挑中的那一个");
        assert!(serving.endpoint.starts_with("http://127.0.0.1:"));

        // 任务里 from_std 走完之后要真的在听：连一下才算数。给收摊前的 accept
        // 留一瞬时间——spawn 到真正开始 poll 之间隔着一次调度。
        let mut connected = None;
        for _ in 0..40 {
            if let Ok(stream) = std::net::TcpStream::connect(("127.0.0.1", serving.port)) {
                connected = Some(stream);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(connected.is_some(), "监听要真的接得进来");
        drop(connected);

        stop(&state);
        assert!(!status(&state).running, "停机之后不能再算在跑");
        // 端口要真的还给系统：旧的监听还占着的话，改端口和重启都是空话。
        let mut freed = false;
        for _ in 0..40 {
            if std::net::TcpListener::bind(("127.0.0.1", serving.port)).is_ok() {
                freed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(freed, "停机之后端口还要让出来");
    }

    /// 真 socket 上跑一遍：手写 HTTP 有没有和真客户端对上。
    #[tokio::test]
    async fn a_raw_http_connection_gets_health_checks_and_jsonrpc() {
        let state = Arc::new(state());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let counters = Arc::new(ServerCounters {
            requests: AtomicU64::new(0),
            running: AtomicBool::new(true),
            bound: Mutex::new(Some(addr.port())),
        });
        let (tx, rx) = oneshot::channel::<()>();
        // app 传 None：单测没有 Tauri 会话，这条路走得通才说明
        // 事件总线是可选的，不是硬依赖。
        tokio::spawn(accept_loop(
            listener,
            state.clone(),
            None,
            counters.clone(),
            rx,
        ));

        // GET 健康检查：人肉排查「外部连不上」时敲一下这个地址。
        let body = round_trip(&addr, &http("GET", "/mcp", None)).await;
        let text = text_of(&body);
        assert!(text.starts_with("HTTP/1.1 200 OK"));
        assert!(text.contains("aipixel-mcp"));
        assert!(text.contains("POST JSON-RPC"));
        // 请求的路径要回出来：敲错前缀时这儿能直接看出是哪一条。
        assert!(text.contains("\"requested_path\": \"/mcp\""));

        // OPTIONS 预检：204 + CORS 头。
        let body = round_trip(&addr, &http("OPTIONS", "/mcp", None)).await;
        let text = text_of(&body);
        assert!(text.starts_with("HTTP/1.1 204"));
        assert!(text.contains("Access-Control-Allow-Origin: *"));

        // POST initialize：整条 JSON-RPC 往返真 socket。
        let payload = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}).to_string();
        let body = round_trip(&addr, &http("POST", "/mcp", Some(&payload))).await;
        let text = text_of(&body);
        assert!(text.starts_with("HTTP/1.1 200 OK"));
        assert!(text.contains("\"jsonrpc\":\"2.0\""));
        assert!(text.contains("aipixel"));

        // 通知：202 空答，不断连接的语义。
        let notification = json!({"jsonrpc": "2.0", "method": "ping"}).to_string();
        let body = round_trip(&addr, &http("POST", "/mcp", Some(&notification))).await;
        assert!(text_of(&body).starts_with("HTTP/1.1 202"));

        // 坏 JSON：HTTP 400 + 协议错 -32700。
        let body = round_trip(&addr, &http("POST", "/mcp", Some("not{{json"))).await;
        let text = text_of(&body);
        assert!(text.starts_with("HTTP/1.1 400"));
        assert!(text.contains("-32700"));

        // 没有 body 的 POST：同一条路，也是 400。
        let body = round_trip(&addr, &http("POST", "/mcp", None)).await;
        assert!(text_of(&body).starts_with("HTTP/1.1 400"));

        // 别的方法：405，并告诉对方该用什么。
        let body = round_trip(&addr, &http("DELETE", "/mcp", None)).await;
        let text = text_of(&body);
        assert!(text.starts_with("HTTP/1.1 405"));
        assert!(text.contains("POST"));

        // 只有 POST 计入请求数：GET 健康检查不该把「已服务请求」撑起来。
        assert!(counters.requests.load(Ordering::Relaxed) >= 3);

        // 停机：发出去的信号要能让 accept 循环收摊。
        let _ = tx.send(());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
