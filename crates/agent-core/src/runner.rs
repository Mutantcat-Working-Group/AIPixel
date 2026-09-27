//! Agent 主循环：prompt admission -> provider 流式输出 -> tool_use -> 工具执行 -> 结果回填 -> 续轮。
//!
//! 设计要点：
//! - 文档是唯一权威状态，每轮重新组装系统提示词（上一轮工具可能已经改过 canvas）。
//! - 模型永远不手写矩阵：所有绘制经由 `tools::execute`（ops / Lua 沙箱 / RLE 读回）。
//! - 预算：`max_tool_steps`（单 turn 工具步数）、`max_turns`（续轮次数）、
//!   `max_tool_result_bytes`（回灌截断），外加「同一个失败调用连续 3 次」的退避保护。
//! - 中断：流式期间按 120ms 轮询取消标志，`interrupt()` 立刻收尾并回 `Interrupted`。

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use super::models::{
    ActiveContext, AgentEvent, Attachment, ChatRequest, ContentBlock, LlmEvent, Message,
    ModelConfig, PermissionMode, Role, RunnerConfig,
};
use super::prompt;
use super::providers::{self, LlmProvider};
use super::tools::{self, ToolOutcome};
use pixel_core::document::Document;

/// 一次待执行的工具调用（JSON 解析失败的也排进来，让模型收到可修复的错误）。
struct PlannedCall {
    id: String,
    name: String,
    input: Value,
    parse_error: Option<String>,
}

/// 单个 agent 会话：持有文档（权威状态）、消息历史与 provider。
/// 纯 Rust，不依赖 Tauri；上层通过 `tokio::sync::mpsc` 通道收 `AgentEvent`。
struct Engine {
    config: ModelConfig,
    provider: Arc<dyn LlmProvider>,
}

pub struct AgentSession {
    pub id: String,
    /// 模型配置与 provider 放在同一把锁里，运行时切换模型不必重建会话、丢历史。
    engine: Mutex<Engine>,
    runner_config: Mutex<RunnerConfig>,
    messages: Mutex<Vec<Message>>,
    document: Mutex<Document>,
    active: Mutex<ActiveContext>,
    turn: AtomicUsize,
    cancelled: AtomicBool,
}

impl AgentSession {
    pub fn new(id: impl Into<String>, config: ModelConfig, document: Document) -> Self {
        let active = prompt::default_active(&document);
        let provider = providers::build_provider(&config);
        AgentSession {
            id: id.into(),
            runner_config: Mutex::new(RunnerConfig::default()),
            engine: Mutex::new(Engine { config, provider }),
            messages: Mutex::new(Vec::new()),
            document: Mutex::new(document),
            active: Mutex::new(active),
            turn: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
        }
    }

    pub fn with_runner_config(mut self, config: RunnerConfig) -> Self {
        *self.runner_config.get_mut().unwrap() = config;
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn model_config(&self) -> ModelConfig {
        self.engine.lock().unwrap().config.clone()
    }

    pub fn runner_config(&self) -> RunnerConfig {
        self.runner_config.lock().unwrap().clone()
    }

    pub fn set_runner_config(&self, config: RunnerConfig) {
        *self.runner_config.lock().unwrap() = config;
    }

    /// 运行时切换模型：重建 provider，保留文档与消息历史。
    pub fn rebind_provider(&self, config: ModelConfig) {
        let provider = providers::build_provider(&config);
        *self.engine.lock().unwrap() = Engine { config, provider };
    }

    /// 用外部文档整体替换当前文档（前端加载 .aip 或撤销后同步回来）。
    /// 激活图层/帧若已不存在则回落到首个，避免后续工具调用落空。
    pub fn sync_document(&self, document: Document) {
        let mut doc = self.document.lock().unwrap();
        let mut active = self.active.lock().unwrap();
        if !doc_has_layer(&doc, &active.layer) {
            if let Some(first) = doc.layers.first() {
                active.layer = first.id.clone();
            }
        }
        if !doc_has_frame(&doc, &active.frame) {
            if let Some(first) = doc.frames.first() {
                active.frame = first.id.clone();
            }
        }
        *doc = document;
    }

    pub fn set_active(&self, active: ActiveContext) {
        *self.active.lock().unwrap() = active;
    }

    pub fn active(&self) -> ActiveContext {
        self.active.lock().unwrap().clone()
    }

    pub fn set_permission(&self, permission: PermissionMode) {
        self.runner_config.lock().unwrap().permission = permission;
    }

    pub fn history(&self) -> Vec<Message> {
        self.messages.lock().unwrap().clone()
    }

    pub fn load_history(&self, messages: Vec<Message>) {
        *self.messages.lock().unwrap() = messages;
    }

    pub fn clear_history(&self) {
        self.messages.lock().unwrap().clear();
    }

    pub fn document(&self) -> Document {
        self.document.lock().unwrap().clone()
    }

    pub fn document_json(&self) -> Value {
        let doc = self.document.lock().unwrap();
        serde_json::to_value(&*doc).unwrap_or(Value::Null)
    }

    pub fn revision(&self) -> u64 {
        self.document.lock().unwrap().revision
    }

    /// 序列化成 .aip v2 文本（中间文件格式不变）。
    pub fn aip_text(&self) -> Result<String, String> {
        let doc = self.document.lock().unwrap();
        pixel_core::context::to_aip(&doc)
    }

    pub fn interrupt(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// 跑一个 turn：发一条用户消息，驱动模型通过工具改画布，直到它不再调工具。
    /// `images` 为 `(media_type, base64)` 对；所有过程事件经 `tx` 广播给 UI。
    pub async fn run_turn(
        &self,
        text: String,
        attachments: Vec<Attachment>,
        tx: UnboundedSender<AgentEvent>,
    ) {
        self.turn.store(0, Ordering::SeqCst);
        self.cancelled.store(false, Ordering::SeqCst);

        if text.trim().is_empty() && attachments.is_empty() {
            emit(
                &tx,
                AgentEvent::Error {
                    message: "empty message".into(),
                },
            );
            return;
        }

        let mut content: Vec<ContentBlock> = Vec::new();
        if !text.trim().is_empty() {
            content.push(ContentBlock::Text { text: text.clone() });
        }
        // 图片清单走在图片前面：模型必须知道每张图的身份与顺序，
        // 否则「快照只是上下文、参考图才是真值」这条约束无从执行。
        if !attachments.is_empty() {
            content.push(ContentBlock::Text {
                text: Attachment::caption(&attachments),
            });
        }
        for attachment in attachments {
            content.push(ContentBlock::Image {
                media_type: attachment.media_type,
                data_base64: attachment.data_base64,
            });
        }
        self.messages.lock().unwrap().push(Message {
            role: Role::User,
            content,
        });

        // 进入 turn 时快照一份预算，避免中途改设置导致行为漂移。
        let runner_config = self.runner_config();
        let mut steps = 0usize;
        let mut last_failure: Option<(String, String)> = None;
        let mut failure_streak = 0usize;
        let mut usage_in: Option<u32> = None;
        let mut usage_out: Option<u32> = None;

        loop {
            if self.is_cancelled() {
                emit(&tx, AgentEvent::Interrupted);
                return;
            }

            let round = self.turn.fetch_add(1, Ordering::SeqCst) + 1;
            if round > runner_config.max_turns {
                emit(
                    &tx,
                    AgentEvent::Error {
                        message: format!(
                            "turn budget exhausted after {} round(s)",
                            runner_config.max_turns
                        ),
                    },
                );
                return;
            }

            // 每轮重新组装系统提示词：文档是权威状态，随时可能被上一轮工具改写。
            // 锁顺序固定为 document -> active -> messages，避免自锁。
            let request = {
                let doc = self.document.lock().unwrap();
                let active = self.active.lock().unwrap();
                let engine = self.engine.lock().unwrap();
                ChatRequest {
                    system: prompt::build_system_prompt(
                        &doc,
                        &active.layer,
                        &active.frame,
                        active.color.as_deref(),
                        runner_config.canvas_context_chars,
                    ),
                    messages: self.messages.lock().unwrap().clone(),
                    tools: tools::specs(),
                    max_tokens: engine
                        .config
                        .max_tokens
                        .unwrap_or(providers::DEFAULT_MAX_TOKENS),
                    temperature: engine.config.temperature,
                }
            };

            let provider = self.engine.lock().unwrap().provider.clone();
            let stream = match provider.request(&request).await {
                Ok(stream) => stream,
                Err(e) => {
                    emit(
                        &tx,
                        AgentEvent::Error {
                            message: e.to_string(),
                        },
                    );
                    return;
                }
            };

            let mut stream = stream;
            let mut text_out = String::new();
            let mut reasoning_out = String::new();
            let mut accumulator: BTreeMap<usize, (String, String, String)> = BTreeMap::new();
            let mut tick = tokio::time::interval(Duration::from_millis(120));

            loop {
                tokio::select! {
                    item = stream.next() => {
                        match item {
                            Some(Ok(event)) => match event {
                                LlmEvent::Token(t) => {
                                    text_out.push_str(&t);
                                    emit(&tx, AgentEvent::Token { text: t });
                                }
                                LlmEvent::Reasoning(t) => {
                                    reasoning_out.push_str(&t);
                                    emit(&tx, AgentEvent::Reasoning { text: t });
                                }
                                LlmEvent::ToolUseStart { index, id, name } => {
                                    accumulator.insert(index, (id, name, String::new()));
                                }
                                LlmEvent::ToolInputDelta { index, json_partial } => {
                                    if let Some(slot) = accumulator.get_mut(&index) {
                                        slot.2.push_str(&json_partial);
                                    }
                                }
                                LlmEvent::Usage { input_tokens, output_tokens } => {
                                    if input_tokens.is_some() {
                                        usage_in = input_tokens;
                                    }
                                    if output_tokens.is_some() {
                                        usage_out = output_tokens;
                                    }
                                }
                                LlmEvent::Done { .. } => break,
                            },
                            Some(Err(e)) => {
                                emit(&tx, AgentEvent::Error { message: format!("stream error: {e}") });
                                return;
                            }
                            None => break,
                        }
                    }
                    _ = tick.tick() => {
                        if self.is_cancelled() {
                            drop(stream);
                            emit(&tx, AgentEvent::Interrupted);
                            return;
                        }
                    }
                }
            }

            let mut blocks: Vec<ContentBlock> = Vec::new();
            if !reasoning_out.is_empty() {
                blocks.push(ContentBlock::Reasoning {
                    text: reasoning_out,
                });
            }
            if !text_out.is_empty() {
                blocks.push(ContentBlock::Text { text: text_out });
            }

            let mut calls: Vec<PlannedCall> = Vec::new();
            for (_, (id, name, partial)) in accumulator {
                let trimmed = partial.trim();
                let (input, parse_error) = if trimmed.is_empty() {
                    (json!({}), None)
                } else {
                    match serde_json::from_str::<Value>(trimmed) {
                        Ok(value) => (value, None),
                        Err(e) => (
                            json!({ "_raw": partial, "_error": e.to_string() }),
                            Some(e.to_string()),
                        ),
                    }
                };
                emit(
                    &tx,
                    AgentEvent::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                );
                calls.push(PlannedCall {
                    id,
                    name,
                    input,
                    parse_error,
                });
            }
            for call in &calls {
                blocks.push(ContentBlock::ToolUse {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                });
            }
            if !blocks.is_empty() {
                self.messages
                    .lock()
                    .unwrap()
                    .push(Message::assistant(blocks));
            }

            // 没有工具调用：这一轮就是最终答复。
            if calls.is_empty() {
                emit(
                    &tx,
                    AgentEvent::Usage {
                        input_tokens: usage_in,
                        output_tokens: usage_out,
                    },
                );
                emit(&tx, AgentEvent::Completed { turns: round });
                return;
            }

            if runner_config.permission != PermissionMode::Auto {
                emit(
                    &tx,
                    AgentEvent::Status {
                        message: format!(
                            "permission mode {:?} is not wired to an approval prompt yet; tools are executing directly",
                            runner_config.permission
                        ),
                    },
                );
            }

            for call in calls {
                if self.is_cancelled() {
                    emit(&tx, AgentEvent::Interrupted);
                    return;
                }
                if steps >= runner_config.max_tool_steps {
                    emit(
                        &tx,
                        AgentEvent::Error {
                            message: format!("tool step budget exhausted after {steps} step(s)"),
                        },
                    );
                    return;
                }
                steps += 1;

                let outcome: ToolOutcome = match &call.parse_error {
                    Some(e) => ToolOutcome {
                        content: format!(
                            "tool call rejected: arguments were not valid JSON ({e}). Resend the same tool call with corrected JSON arguments."
                        ),
                        is_error: true,
                    },
                    None => {
                        let mut doc = self.document.lock().unwrap();
                        let active = self.active.lock().unwrap();
                        tools::execute(&mut doc, &active, &call.name, &call.input)
                    }
                };

                emit(
                    &tx,
                    AgentEvent::ToolResult {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        summary: summarize(&outcome.content),
                        is_error: outcome.is_error,
                    },
                );

                let mut content = outcome.content;
                let budget = runner_config.max_tool_result_bytes;
                if content.chars().count() > budget {
                    let head: String = content.chars().take(budget).collect();
                    content = format!("{head}\n...[truncated to {budget} chars]");
                }
                self.messages.lock().unwrap().push(Message::tool_result(
                    call.id,
                    content,
                    outcome.is_error,
                ));

                if outcome.is_error {
                    let key = (call.name.clone(), call.input.to_string());
                    if last_failure.as_ref() == Some(&key) {
                        failure_streak += 1;
                    } else {
                        last_failure = Some(key);
                        failure_streak = 1;
                    }
                    if failure_streak >= 3 {
                        emit(
                            &tx,
                            AgentEvent::Error {
                                message: "the same tool call failed three times in a row; stopping so you can adjust the request".into(),
                            },
                        );
                        return;
                    }
                } else {
                    last_failure = None;
                    failure_streak = 0;
                }

                // 读操作不改文档，不推 DocumentUpdated。
                if call.name != "pixel_read_canvas" {
                    let (revision, document) = {
                        let doc = self.document.lock().unwrap();
                        (
                            doc.revision,
                            serde_json::to_value(&*doc).unwrap_or(Value::Null),
                        )
                    };
                    emit(&tx, AgentEvent::DocumentUpdated { revision, document });
                }
            }
        }
    }
}

fn doc_has_layer(doc: &Document, id: &str) -> bool {
    doc.layers.iter().any(|l| l.id == id)
}

fn doc_has_frame(doc: &Document, id: &str) -> bool {
    doc.frames.iter().any(|f| f.id == id)
}

fn emit(tx: &UnboundedSender<AgentEvent>, event: AgentEvent) {
    let _ = tx.send(event);
}

/// 工具结果首行摘要，用于 UI 工具卡片标题。
fn summarize(content: &str) -> String {
    let first = content.lines().next().unwrap_or("").trim();
    let count = first.chars().count();
    if count == 0 {
        return "(no output)".into();
    }
    if count <= 180 {
        return first.to_string();
    }
    let mut out: String = first.chars().take(180).collect();
    out.push_str("...");
    out
}
