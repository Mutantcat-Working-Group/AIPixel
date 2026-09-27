//! Provider 传输层：把协议无关的 `ChatRequest` 翻译成 Anthropic Messages 或 OpenAI 兼容
//! Chat Completions 请求，并把 SSE 流归一成 `LlmEvent`。只支持用户自带 base_url + api_key，
//! 无内置服务器 / 登录 / 计费。

use super::models::{ChatRequest, ContentBlock, LlmEvent, Message, ModelConfig, Protocol, Role};
use async_trait::async_trait;
use futures_util::Stream;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

pub const DEFAULT_MAX_TOKENS: u32 = 8192;

pub type EventStream = Pin<Box<dyn Stream<Item = Result<LlmEvent, ProviderError>> + Send>>;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("config error: {0}")]
    Config(String),
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn request(&self, req: &ChatRequest) -> Result<EventStream, ProviderError>;
    fn max_tokens(&self) -> u32 {
        DEFAULT_MAX_TOKENS
    }
}

/// 构造对应协议的 provider。
pub fn build_provider(config: &ModelConfig) -> Arc<dyn LlmProvider> {
    let client = reqwest::Client::new();
    match config.protocol {
        Protocol::Anthropic => Arc::new(AnthropicProvider {
            client,
            base_url: trim_trailing_slash(&config.base_url),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        }),
        Protocol::OpenAiCompat => Arc::new(OpenAiCompatProvider {
            client,
            base_url: trim_trailing_slash(&config.base_url),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        }),
    }
}

fn trim_trailing_slash(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

// ---------------- SSE ----------------

/// 原始 SSE 流，产出每个 event 的 `data:` 载荷（String）。
struct SseStream {
    inner: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
    buf: String,
    pending: VecDeque<String>,
    inner_done: bool,
}

impl SseStream {
    fn drain_frames(&mut self) {
        while let Some(pos) = self.buf.find("\n\n") {
            let frame: String = self.buf[..pos].to_string();
            self.buf.drain(..pos + 2);
            if let Some(data) = frame_data(&frame) {
                self.pending.push_back(data);
            }
        }
    }
}

impl Stream for SseStream {
    type Item = Result<String, ProviderError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(f) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(f)));
            }
            if this.inner_done {
                return Poll::Ready(None);
            }
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    let s = String::from_utf8_lossy(&chunk);
                    this.buf.push_str(&s);
                    this.buf = this.buf.replace("\r\n", "\n").replace('\r', "\n");
                    this.drain_frames();
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(ProviderError::Network(e.to_string()))));
                }
                Poll::Ready(None) => {
                    this.inner_done = true;
                    if !this.buf.is_empty() {
                        let frame = std::mem::take(&mut this.buf);
                        if let Some(data) = frame_data(&frame) {
                            this.pending.push_back(data);
                        }
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// 提取一个 SSE frame 里的 data 载荷（可能多行，以 "\n" 连接）。
fn frame_data(frame: &str) -> Option<String> {
    let mut lines: Vec<&str> = Vec::new();
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            lines.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// provider 特定的增量解析累积状态。
#[derive(Default)]
struct ToolAcc {
    names: HashMap<usize, (String, String)>, // index -> (id, name)
    started: std::collections::HashSet<usize>,
    stop: String,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

type ParseFn = fn(&str, &mut ToolAcc) -> Result<Vec<LlmEvent>, ProviderError>;

/// 把 SSE 数据流归一成 `LlmEvent`。
struct ParsedStream {
    sse: SseStream,
    parse: ParseFn,
    acc: ToolAcc,
    out: VecDeque<LlmEvent>,
    finished: bool,
    emitted_done: bool,
}

impl Stream for ParsedStream {
    type Item = Result<LlmEvent, ProviderError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(ev) = this.out.pop_front() {
                return Poll::Ready(Some(Ok(ev)));
            }
            if this.finished {
                return Poll::Ready(None);
            }
            match Pin::new(&mut this.sse).poll_next(cx) {
                Poll::Ready(Some(Ok(data))) => {
                    if data.trim() == "[DONE]" {
                        this.push_done();
                        continue;
                    }
                    match (this.parse)(&data, &mut this.acc) {
                        Ok(events) => {
                            let mut has_done = false;
                            for ev in events {
                                if matches!(ev, LlmEvent::Done { .. }) {
                                    has_done = true;
                                }
                                this.out.push_back(ev);
                            }
                            if has_done {
                                this.push_done();
                            }
                        }
                        Err(e) => return Poll::Ready(Some(Err(e))),
                    }
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => {
                    this.push_done();
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl ParsedStream {
    fn push_done(&mut self) {
        if !self.emitted_done {
            let reason = if self.acc.stop.is_empty() {
                "stop".to_string()
            } else {
                self.acc.stop.clone()
            };
            self.out.push_back(LlmEvent::Done {
                stop_reason: reason,
            });
            self.emitted_done = true;
        }
        self.finished = true;
    }
}

fn anthropic_parse(payload: &str, acc: &mut ToolAcc) -> Result<Vec<LlmEvent>, ProviderError> {
    let v: Value =
        serde_json::from_str(payload).map_err(|e| ProviderError::Decode(e.to_string()))?;
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let events = match ty {
        "message_start" => {
            if let Some(u) = v["message"]["usage"]["input_tokens"].as_u64() {
                acc.input_tokens = Some(u as u32);
            }
            vec![]
        }
        "content_block_start" => {
            let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            let block = &v["content_block"];
            match block.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    acc.names.insert(idx, (id.clone(), name.clone()));
                    acc.started.insert(idx);
                    vec![LlmEvent::ToolUseStart {
                        index: idx,
                        id,
                        name,
                    }]
                }
                _ => vec![],
            }
        }
        "content_block_delta" => {
            let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            let delta = &v["delta"];
            match delta.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text_delta" => match delta.get("text").and_then(|t| t.as_str()) {
                    Some(t) => vec![LlmEvent::Token(t.to_string())],
                    None => vec![],
                },
                "thinking_delta" | "reasoning_delta" => {
                    let text = delta
                        .get("thinking")
                        .or_else(|| delta.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    vec![LlmEvent::Reasoning(text.to_string())]
                }
                "input_json_delta" => match delta.get("partial_json").and_then(|t| t.as_str()) {
                    Some(part) => vec![LlmEvent::ToolInputDelta {
                        index: idx,
                        json_partial: part.to_string(),
                    }],
                    None => vec![],
                },
                _ => vec![],
            }
        }
        "message_delta" => {
            let mut out = vec![];
            if let Some(stop) = v["delta"]["stop_reason"].as_str() {
                acc.stop = stop.to_string();
            }
            if let Some(o) = v["usage"]["output_tokens"].as_u64() {
                acc.output_tokens = Some(o as u32);
            }
            if acc.input_tokens.is_some() || acc.output_tokens.is_some() {
                out.push(LlmEvent::Usage {
                    input_tokens: acc.input_tokens,
                    output_tokens: acc.output_tokens,
                });
            }
            out
        }
        "message_stop" => {
            let reason = if acc.stop.is_empty() {
                "stop".to_string()
            } else {
                acc.stop.clone()
            };
            vec![LlmEvent::Done {
                stop_reason: reason,
            }]
        }
        "error" => {
            let msg = v["error"]["message"]
                .as_str()
                .unwrap_or("provider error")
                .to_string();
            return Err(ProviderError::Decode(msg));
        }
        _ => vec![],
    };
    Ok(events)
}

fn openai_parse(payload: &str, acc: &mut ToolAcc) -> Result<Vec<LlmEvent>, ProviderError> {
    let v: Value =
        serde_json::from_str(payload).map_err(|e| ProviderError::Decode(e.to_string()))?;
    let mut out = vec![];
    if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
        let input = u
            .get("prompt_tokens")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        let output = u
            .get("completion_tokens")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        acc.input_tokens = input;
        acc.output_tokens = output;
        if input.is_some() || output.is_some() {
            out.push(LlmEvent::Usage {
                input_tokens: input,
                output_tokens: output,
            });
        }
    }
    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        if let Some(choice) = choices.first() {
            let delta = &choice["delta"];
            if let Some(reasoning) = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(|t| t.as_str())
            {
                out.push(LlmEvent::Reasoning(reasoning.to_string()));
            }
            if let Some(content) = delta.get("content").and_then(|t| t.as_str()) {
                if !content.is_empty() {
                    out.push(LlmEvent::Token(content.to_string()));
                }
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tool_calls {
                    let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    let id = tc.get("id").and_then(|s| s.as_str());
                    let name = tc["function"].get("name").and_then(|s| s.as_str());
                    if !acc.started.contains(&index) {
                        if let (Some(id), Some(name)) = (id, name) {
                            acc.names.insert(index, (id.to_string(), name.to_string()));
                            acc.started.insert(index);
                            out.push(LlmEvent::ToolUseStart {
                                index,
                                id: id.to_string(),
                                name: name.to_string(),
                            });
                        }
                    }
                    if let Some(args) = tc["function"].get("arguments").and_then(|s| s.as_str()) {
                        if !args.is_empty() {
                            out.push(LlmEvent::ToolInputDelta {
                                index,
                                json_partial: args.to_string(),
                            });
                        }
                    }
                }
            }
            if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                acc.stop = reason.to_string();
            }
        }
    }
    Ok(out)
}

// ---------------- Anthropic ----------------

struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn request(&self, req: &ChatRequest) -> Result<EventStream, ProviderError> {
        if self.api_key.trim().is_empty() {
            return Err(ProviderError::Config("missing api key".into()));
        }
        let body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "stream": true,
            "system": req.system,
            "messages": to_anthropic_messages(&req.messages),
            "tools": req.tools.iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.schema,
            })).collect::<Vec<_>>(),
        });
        let url = format!("{}/messages", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        ensure_ok(resp).await.map(|sse| {
            Box::pin(ParsedStream {
                sse,
                parse: anthropic_parse,
                acc: ToolAcc::default(),
                out: VecDeque::new(),
                finished: false,
                emitted_done: false,
            }) as EventStream
        })
    }
}

// ---------------- OpenAI compatible ----------------

struct OpenAiCompatProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    async fn request(&self, req: &ChatRequest) -> Result<EventStream, ProviderError> {
        if self.api_key.trim().is_empty() {
            return Err(ProviderError::Config("missing api key".into()));
        }
        let mut messages = vec![json!({"role": "system", "content": req.system})];
        messages.extend(to_openai_messages(&req.messages));
        let body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "stream": true,
            "stream_options": {"include_usage": true},
            "messages": messages,
            "tools": req.tools.iter().map(|t| json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.schema,
                },
            })).collect::<Vec<_>>(),
        });
        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        ensure_ok(resp).await.map(|sse| {
            Box::pin(ParsedStream {
                sse,
                parse: openai_parse,
                acc: ToolAcc::default(),
                out: VecDeque::new(),
                finished: false,
                emitted_done: false,
            }) as EventStream
        })
    }
}

async fn ensure_ok(resp: reqwest::Response) -> Result<SseStream, ProviderError> {
    let status = resp.status();
    if !status.is_success() {
        let code = status.as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::Http {
            status: code,
            body: text,
        });
    }
    Ok(SseStream {
        inner: Box::pin(resp.bytes_stream()),
        buf: String::new(),
        pending: VecDeque::new(),
        inner_done: false,
    })
}

// ---------------- message conversion ----------------

fn anthropic_block(b: &ContentBlock) -> Option<Value> {
    Some(match b {
        ContentBlock::Text { text } => json!({"type": "text", "text": text}),
        ContentBlock::Image {
            media_type,
            data_base64,
        } => json!({
            "type": "image",
            "source": {"type": "base64", "media_type": media_type, "data": data_base64},
        }),
        ContentBlock::ToolUse { id, name, input } => json!({
            "type": "tool_use", "id": id, "name": name, "input": input,
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error,
        }),
        // Anthropic 要求 thinking block 带签名，回灌会报错；直接丢弃推理块。
        ContentBlock::Reasoning { .. } => return None,
    })
}

pub(crate) fn to_anthropic_messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let m = &messages[i];
        match m.role {
            Role::System => {
                i += 1;
            }
            Role::Tool => {
                // 连续 tool_result 合并进一个 user turn。
                let mut blocks = Vec::new();
                while i < messages.len() && messages[i].role == Role::Tool {
                    for b in &messages[i].content {
                        if let Some(v) = anthropic_block(b) {
                            blocks.push(v);
                        }
                    }
                    i += 1;
                }
                out.push(json!({"role": "user", "content": blocks}));
            }
            Role::User | Role::Assistant => {
                let blocks: Vec<Value> = m.content.iter().filter_map(anthropic_block).collect();
                let role = match m.role {
                    Role::Assistant => "assistant",
                    _ => "user",
                };
                out.push(json!({"role": role, "content": blocks}));
                i += 1;
            }
        }
    }
    out
}

pub(crate) fn to_openai_messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::System => {}
            Role::User => {
                let has_image = m
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Image { .. }));
                if has_image {
                    let content: Vec<Value> = m
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(json!({"type": "text", "text": text})),
                            ContentBlock::Image {
                                media_type,
                                data_base64,
                            } => Some(json!({
                                "type": "image_url",
                                "image_url": {"url": format!("data:{media_type};base64,{data_base64}")},
                            })),
                            _ => None,
                        })
                        .collect();
                    out.push(json!({"role": "user", "content": content}));
                } else {
                    out.push(json!({"role": "user", "content": m.text_of()}));
                }
            }
            Role::Assistant => {
                let text = m.text_of();
                let text = if text.is_empty() {
                    Value::Null
                } else {
                    json!(text)
                };
                let tool_calls: Vec<Value> = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolUse { id, name, input } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": input.to_string()},
                        })),
                        _ => None,
                    })
                    .collect();
                let mut msg = json!({"role": "assistant", "content": text});
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = json!(tool_calls);
                }
                out.push(msg);
            }
            Role::Tool => {
                for b in &m.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = b
                    {
                        let mut c = content.clone();
                        if *is_error {
                            c = format!("ERROR: {c}");
                        }
                        out.push(
                            json!({"role": "tool", "tool_call_id": tool_use_id, "content": c}),
                        );
                    }
                }
            }
        }
    }
    out
}
