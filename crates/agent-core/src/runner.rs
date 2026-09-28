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
use tokio::sync::oneshot;

use super::imagegen::{self, ImageGenParams, LandSpot};
use super::mcp::{self, McpRegistry};
use super::models::{
    ActiveContext, AgentEvent, ApprovalDecision, Attachment, Capabilities, ChatRequest,
    ContentBlock, LlmEvent, Message, ModelConfig, PermissionMode, Role, RunnerConfig, ToolSpec,
};
use super::prompt;
use super::providers::{self, LlmProvider};
use super::roles::{ModelRole, RoleBinding};
use super::tools::{self, ToolOutcome, IMAGE_GEN_TOOL};
use pixel_core::decode;
use pixel_core::document::Document;

/// 一次待执行的工具调用（JSON 解析失败的也排进来，让模型收到可修复的错误）。
struct PlannedCall {
    id: String,
    name: String,
    input: Value,
    parse_error: Option<String>,
}

/// 挂起中的审批。一个 turn 顺序执行工具，同时最多挂一条，所以单槽就够。
struct ApprovalSlot {
    call_id: String,
    tx: oneshot::Sender<ApprovalDecision>,
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
    /// 生图 / 识图 / 读视频的另外三个模型。按角色各一个，没有就回落主模型。
    /// 和 engine 分成两处而不是塞进 Engine：主模型的语义是「会话本身那把」，
    /// 换它要连带提示词和工具默认值一起变，角色模型只是某段流程的替身。
    role_engines: Mutex<BTreeMap<ModelRole, Engine>>,
    runner_config: Mutex<RunnerConfig>,
    messages: Mutex<Vec<Message>>,
    document: Mutex<Document>,
    active: Mutex<ActiveContext>,
    turn: AtomicUsize,
    cancelled: AtomicBool,
    /// 当前挂起的审批发送端；None 表示没有调用在等用户。
    approval: Mutex<Option<ApprovalSlot>>,
    /// 用户自配的 MCP 工具服务器注册表；None 表示这个会话不接外部工具。
    mcp: Option<Arc<McpRegistry>>,
}

impl AgentSession {
    pub fn new(id: impl Into<String>, config: ModelConfig, document: Document) -> Self {
        let active = prompt::default_active(&document);
        let provider = providers::build_provider(&config);
        AgentSession {
            id: id.into(),
            runner_config: Mutex::new(RunnerConfig::default()),
            engine: Mutex::new(Engine { config, provider }),
            role_engines: Mutex::new(BTreeMap::new()),
            messages: Mutex::new(Vec::new()),
            document: Mutex::new(document),
            active: Mutex::new(active),
            turn: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            approval: Mutex::new(None),
            mcp: None,
        }
    }

    pub fn with_runner_config(mut self, config: RunnerConfig) -> Self {
        *self.runner_config.get_mut().unwrap() = config;
        self
    }

    /// 挂上 MCP 注册表：会话每轮把它 expose 的工具并进工具清单，
    /// `mcp__*` 调用分流过去。注册表是共享的，会话换模型不影响连接。
    pub fn with_mcp_registry(mut self, registry: Arc<McpRegistry>) -> Self {
        self.mcp = Some(registry);
        self
    }

    /// 把某个角色另绑到一个模型。配同一个角色就是换模型。
    /// 没配过这个角色时，它本来在蹭主模型的饭碗，现在才有自己的。
    pub fn rebind_role(&self, role: ModelRole, config: ModelConfig) {
        if !role.is_detachable() {
            // 主模型只能整体 rebind_provider，这里是调用方走错了门。
            return;
        }
        let provider = providers::build_provider(&config);
        self.role_engines
            .lock()
            .unwrap()
            .insert(role, Engine { config, provider });
    }

    /// 取消某个角色的单独绑定，让它回落去蹭主模型。没绑过就是空操作。
    pub fn clear_role(&self, role: ModelRole) {
        self.role_engines.lock().unwrap().remove(&role);
    }

    /// 这个角色实际该用哪个模型配置：单独绑了就用它，否则用主模型。
    /// 单模型用户什么都没配，所以拿到的永远是主模型，行为和以前一致。
    pub fn model_for_role(&self, role: ModelRole) -> ModelConfig {
        self.role_engines
            .lock()
            .unwrap()
            .get(&role)
            .map(|engine| engine.config.clone())
            .unwrap_or_else(|| self.model_config())
    }

    /// 四个角色各自实际在干的活，给 UI 摆「谁负责哪段流程」。
    /// 没有分工引擎的会话也照答：回落主模型，`detached` 是 false。
    pub fn role_bindings(&self) -> Vec<RoleBinding> {
        let primary = self.model_config();
        let roles = self.role_engines.lock().unwrap();
        ModelRole::all()
            .into_iter()
            .map(|role| match roles.get(&role) {
                Some(engine) => RoleBinding {
                    role,
                    model_id: engine.config.id.clone(),
                    model_label: engine.config.label.clone(),
                    detached: true,
                },
                None => RoleBinding {
                    role,
                    model_id: primary.id.clone(),
                    model_label: primary.label.clone(),
                    detached: false,
                },
            })
            .collect()
    }

    /// 会话实际能跑的工作流能力：把各角色自己那份能力并进来。
    /// 单模型用户没有角色引擎，并集就等于主模型自己的能力。
    pub fn effective_capabilities(&self) -> Capabilities {
        let mut caps = self.model_config().capabilities;
        for engine in self.role_engines.lock().unwrap().values() {
            caps.vision |= engine.config.capabilities.vision;
            caps.image_gen |= engine.config.capabilities.image_gen;
            caps.video |= engine.config.capabilities.video;
        }
        caps
    }

    /// 本会话可见的 MCP 工具规格（没挂注册表就空）。
    fn mcp_specs(&self) -> Vec<ToolSpec> {
        self.mcp.as_ref().map(|r| r.specs()).unwrap_or_default()
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

    /// 用户对一条挂起的工具调用给出决定。call_id 对不上说明这是过期决定
    /// （新 turn 已经开始），直接拒绝而不是把它送进死队列。
    pub fn resolve_approval(
        &self,
        call_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), String> {
        let slot = self.approval.lock().unwrap().take();
        match slot {
            Some(pending) if pending.call_id == call_id => {
                let _ = pending.tx.send(decision);
                Ok(())
            }
            Some(_) => Err("approval request is stale".into()),
            None => Err("no approval is pending".into()),
        }
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

    /// 借出文档做只读的一次性计算（渲染垫图、导出预览）。
    /// 和 `with_document_mut` 用同一把锁，但不会 bump revision，也不会让外界改到文档。
    pub fn with_document<T>(&self, f: impl FnOnce(&Document) -> T) -> T {
        let doc = self.document.lock().unwrap();
        f(&doc)
    }

    /// 借出文档做一次性原地修改（插帧、量化、编辑器操作），完成后返回新 revision。
    /// 主循环的 agent turn 也走同一把锁，所以这里不会和并发 turn 交织。
    pub fn with_document_mut<T>(&self, f: impl FnOnce(&mut Document) -> T) -> T {
        let mut doc = self.document.lock().unwrap();
        f(&mut doc)
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
        // 有审批挂着的话，发送端一掉，等待中的主循环立刻收手，不会和用户赌手感。
        self.approval.lock().unwrap().take();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// 挂起一条审批并等待用户决定。等待期间照样本轮询取消标志，
    /// 所以中断不会把这一笔调用彻底卡死。Err(()) = 没人再会给这一笔发决定。
    async fn await_approval(
        &self,
        tx: &UnboundedSender<AgentEvent>,
        call: &PlannedCall,
    ) -> Result<ApprovalDecision, ()> {
        let (sender, mut receiver) = oneshot::channel();
        {
            // 上一轮的挂起没清掉就顶掉：turn 顺序执行，出现即异常，顶掉保证不死等。
            let mut slot = self.approval.lock().unwrap();
            *slot = Some(ApprovalSlot {
                call_id: call.id.clone(),
                tx: sender,
            });
        }
        emit(
            tx,
            AgentEvent::ApprovalRequest {
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call.input.clone(),
            },
        );
        let mut tick = tokio::time::interval(Duration::from_millis(120));
        loop {
            tokio::select! {
                decision = &mut receiver => return decision.map_err(|_| ()),
                _ = tick.tick() => {
                    if self.is_cancelled() {
                        return Err(());
                    }
                }
            }
        }
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
        // mut：Ask 模式下用户点「本次放行」会把它降级成 Auto，只影响本 turn。
        let mut runner_config = self.runner_config();
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
                    // MCP 工具追加在内建 pixel_* 之后：模型每轮看到的都是当前真实能力。
                    tools: {
                        let mut specs = tools::specs();
                        specs.extend(self.mcp_specs());
                        specs
                    },
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

                // 权限门：放行之后才动文档。否掉不当失败调用（不进退避计数），
                // 喂一条 tool_result 让模型换方向，对话继续。
                if needs_approval(runner_config.permission, &call.name) {
                    match self.await_approval(&tx, &call).await {
                        Ok(ApprovalDecision::Approve) => {}
                        Ok(ApprovalDecision::ApproveAll) => {
                            // 「这次别烦了」只降级本 turn；会话配置不动。
                            runner_config.permission = PermissionMode::Auto;
                        }
                        Ok(ApprovalDecision::Reject) => {
                            emit(
                                &tx,
                                AgentEvent::ToolResult {
                                    id: call.id.clone(),
                                    name: call.name.clone(),
                                    summary: "rejected by user".into(),
                                    is_error: false,
                                },
                            );
                            self.messages.lock().unwrap().push(Message::tool_result(
                                call.id.clone(),
                                "the user rejected this tool call. Do not retry it as-is; explain what you were about to do and ask how you should proceed.",
                                false,
                            ));
                            continue;
                        }
                        // 没人再会给这一笔发决定（中断，或挂起被新 turn 顶掉）。
                        Err(()) => {
                            emit(&tx, AgentEvent::Interrupted);
                            return;
                        }
                    }
                }

                let outcome: ToolOutcome = match &call.parse_error {
                    Some(e) => ToolOutcome {
                        content: format!(
                            "tool call rejected: arguments were not valid JSON ({e}). Resend the same tool call with corrected JSON arguments."
                        ),
                        is_error: true,
                    },
                    None => {
                        let registry = self.mcp.clone();
                        if call.name.starts_with(mcp::MCP_TOOL_PREFIX) {
                            // 外部工具：分流到用户自配的 MCP 服务器，不进文档锁。
                            match registry {
                                Some(registry) => {
                                    match registry.dispatch(&call.name, &call.input).await {
                                        Ok(outcome) => outcome,
                                        Err(e) => ToolOutcome {
                                            content: e,
                                            is_error: true,
                                        },
                                    }
                                }
                                None => ToolOutcome {
                                    content: format!(
                                        "no MCP registry is attached to this session, so {call} cannot run",
                                        call = call.name
                                    ),
                                    is_error: true,
                                },
                            }
                        } else if call.name == tools::IMAGE_GEN_TOOL {
                            // 生图要等模型回图，异步跑；await 期间绝不持有文档锁。
                            self.run_image_gen(&call.input).await
                        } else {
                            let mut doc = self.document.lock().unwrap();
                            let active = self.active.lock().unwrap();
                            tools::execute(&mut doc, &active, &call.name, &call.input)
                        }
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
                // MCP 工具在文档之外跑（不回推文档事件）；读操作本来也不推。
                if call.name != "pixel_read_canvas" && !call.name.starts_with(mcp::MCP_TOOL_PREFIX)
                {
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
    /// agent 生图工具：让模型直接产出位图，再量化落到画布。与同步工具分开跑，
    /// 因为它要等模型回图，期间绝不能占着文档锁。锁顺序仍是 document -> active。
    async fn run_image_gen(&self, input: &Value) -> ToolOutcome {
        let params = match tools::ImageGenToolParams::parse(input) {
            Ok(p) => p,
            Err(e) => {
                return ToolOutcome {
                    content: e,
                    is_error: true,
                }
            }
        };
        let config = self.model_config();
        if !config.capabilities.image_gen {
            return ToolOutcome {
                content: "this model is not able to generate images; enable image generation in model settings and point it at an OpenAI-compatible image model".into(),
                is_error: true,
            };
        }
        // 垫图先在读锁里渲染；await 之前必须放掉文档锁。
        let reference = match params.reference_frame.as_deref() {
            Some(frame) => match self.with_document(|doc| imagegen::frame_reference(doc, frame)) {
                Ok(att) => Some(att),
                Err(e) => {
                    return ToolOutcome {
                        content: format!("{IMAGE_GEN_TOOL}: {e}"),
                        is_error: true,
                    }
                }
            },
            None => None,
        };
        let generator = imagegen::build_image_generator(&config);
        let request = ImageGenParams {
            prompt: params.prompt.clone(),
            size: params.size.clone(),
            reference,
        };
        let image = match generator.generate(&request).await {
            Ok(img) => img,
            Err(e) => {
                return ToolOutcome {
                    content: format!("{IMAGE_GEN_TOOL}: image generation failed: {e}"),
                    is_error: true,
                }
            }
        };
        let (rgba, width, height) = match decode::decode_image(&image.bytes, &image.media_type) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutcome {
                    content: format!("{IMAGE_GEN_TOOL}: could not decode the model's image: {e}"),
                    is_error: true,
                }
            }
        };
        let active = self.active();
        let landed = self.with_document_mut(|doc| {
            tools::land_generated(doc, &active, &params, &rgba, width, height)
        });
        let landed = match landed {
            Ok(l) => l,
            Err(e) => {
                return ToolOutcome {
                    content: format!("{IMAGE_GEN_TOOL}: could not land the image: {e}"),
                    is_error: true,
                }
            }
        };
        // 新建帧时把激活帧挪过去：下一轮工具该接着这一帧画，用户看到的也对得上。
        if params.spot == LandSpot::NewFrame {
            let mut next = active;
            next.frame = landed.frame.clone();
            self.set_active(next);
        }
        let mut content = format!(
            "generated a {w}x{h} image ({transport}) and landed it on layer {layer} frame {frame}: {colors} color(s) used, +{added} palette color(s).\n",
            w = width,
            h = height,
            transport = image.transport,
            layer = landed.layer,
            frame = landed.frame,
            colors = landed.report.colors_used,
            added = landed.report.palette_added,
        );
        if !image.note.trim().is_empty() {
            content.push_str(&format!("model note: {}\n", image.note.trim()));
        }
        let grid = self.with_document(|doc| tools::active_grid(doc, &landed.layer, &landed.frame));
        content.push_str(&grid);
        ToolOutcome {
            content,
            is_error: false,
        }
    }
}

fn doc_has_layer(doc: &Document, id: &str) -> bool {
    doc.layers.iter().any(|l| l.id == id)
}

fn doc_has_frame(doc: &Document, id: &str) -> bool {
    doc.frames.iter().any(|f| f.id == id)
}

/// 这次调用要不要审批。Auto 全放行；Ask 每个调用都问；Chat 放行只读的读回
/// （读网格不改文档），写操作一律过问——用户要盯的是「模型在改我的画」。
fn needs_approval(mode: PermissionMode, name: &str) -> bool {
    match mode {
        PermissionMode::Auto => false,
        PermissionMode::Ask => true,
        PermissionMode::Chat => name != "pixel_read_canvas",
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use pixel_core::document::Document;

    fn session() -> AgentSession {
        AgentSession::new(
            "s1",
            ModelConfig {
                id: "m1".into(),
                label: "m1".into(),
                protocol: super::super::models::Protocol::Anthropic,
                base_url: "https://example.invalid".into(),
                api_key: String::new(),
                model: "test".into(),
                max_tokens: None,
                temperature: None,
                capabilities: Default::default(),
            },
            Document::new("t", 8, 8).unwrap(),
        )
    }

    fn model_with(id: &str, caps: Capabilities) -> ModelConfig {
        ModelConfig {
            id: id.into(),
            label: format!("{id} label"),
            protocol: super::super::models::Protocol::OpenAiCompat,
            base_url: "https://example.invalid".into(),
            api_key: String::new(),
            model: "test".into(),
            max_tokens: None,
            temperature: None,
            capabilities: caps,
        }
    }

    #[test]
    fn a_session_without_role_models_answers_with_the_session_model() {
        let s = session();
        for role in ModelRole::all() {
            assert_eq!(s.model_for_role(role).id, "m1", "{role:?} should fall back");
        }
        // 回落时 UI 要显示「正在用主模型凑」，所以 detached 必须是 false。
        for binding in s.role_bindings() {
            assert!(!binding.detached, "{:?} is not detached", binding.role);
            assert_eq!(binding.model_id, "m1");
        }
        assert_eq!(s.effective_capabilities(), Capabilities::default());
    }

    #[test]
    fn binding_a_role_moves_only_that_role() {
        let s = session();
        s.rebind_role(
            ModelRole::ImageGen,
            model_with(
                "img",
                Capabilities {
                    image_gen: true,
                    ..Default::default()
                },
            ),
        );

        assert_eq!(s.model_for_role(ModelRole::ImageGen).id, "img");
        // 另外三个角色没被惊动，还指着主模型。
        assert_eq!(s.model_for_role(ModelRole::Chat).id, "m1");
        assert_eq!(s.model_for_role(ModelRole::Vision).id, "m1");
        assert_eq!(s.model_for_role(ModelRole::Video).id, "m1");

        let bindings = s.role_bindings();
        let gen = bindings
            .iter()
            .find(|b| b.role == ModelRole::ImageGen)
            .unwrap();
        assert!(gen.detached);
        assert_eq!(gen.model_id, "img");
        assert_eq!(gen.model_label, "img label");
    }

    #[test]
    fn effective_capabilities_unions_every_role() {
        let s = session();
        assert!(!s.effective_capabilities().any());

        s.rebind_role(
            ModelRole::ImageGen,
            model_with(
                "img",
                Capabilities {
                    image_gen: true,
                    ..Default::default()
                },
            ),
        );
        s.rebind_role(
            ModelRole::Video,
            model_with(
                "vid",
                Capabilities {
                    video: true,
                    ..Default::default()
                },
            ),
        );

        let caps = s.effective_capabilities();
        assert!(caps.image_gen);
        assert!(caps.video);
        // 没人认领识图，所以它还是关着：并集不是无条件全开。
        assert!(!caps.vision);
    }

    #[test]
    fn rebinding_the_same_role_replaces_the_model() {
        let s = session();
        s.rebind_role(ModelRole::Vision, model_with("v1", Capabilities::default()));
        s.rebind_role(ModelRole::Vision, model_with("v2", Capabilities::default()));
        assert_eq!(s.model_for_role(ModelRole::Vision).id, "v2");
        assert_eq!(
            s.role_bindings()
                .iter()
                .find(|b| b.role == ModelRole::Vision)
                .unwrap()
                .model_id,
            "v2"
        );
    }

    #[test]
    fn clearing_a_role_falls_back_to_the_session_model() {
        let s = session();
        s.rebind_role(
            ModelRole::ImageGen,
            model_with(
                "img",
                Capabilities {
                    image_gen: true,
                    ..Default::default()
                },
            ),
        );
        assert!(s.effective_capabilities().image_gen);

        s.clear_role(ModelRole::ImageGen);
        assert_eq!(s.model_for_role(ModelRole::ImageGen).id, "m1");
        assert!(!s.effective_capabilities().image_gen);
        for binding in s.role_bindings() {
            assert!(!binding.detached);
        }
    }

    #[test]
    fn the_chat_role_cannot_be_rebound_through_the_role_door() {
        // 主模型只能整体 rebind_provider；从角色门改它会让 UI 以为有两个主模型。
        let s = session();
        s.rebind_role(
            ModelRole::Chat,
            model_with("other", Capabilities::default()),
        );
        assert_eq!(s.model_for_role(ModelRole::Chat).id, "m1");
        assert!(s.role_bindings().iter().all(|b| !b.detached));

        // 清一个没绑过的角色也不该炸。
        s.clear_role(ModelRole::Video);
        assert_eq!(s.model_for_role(ModelRole::Video).id, "m1");
    }

    #[test]
    fn auto_lets_everything_through_ask_gates_everything() {
        assert!(!needs_approval(
            PermissionMode::Auto,
            "pixel_apply_operations"
        ));
        assert!(!needs_approval(PermissionMode::Auto, "pixel_run_shader"));
        assert!(needs_approval(
            PermissionMode::Ask,
            "pixel_apply_operations"
        ));
        assert!(needs_approval(PermissionMode::Ask, "pixel_read_canvas"));
    }

    #[test]
    fn chat_gates_writes_and_lets_reads_pass() {
        assert!(needs_approval(
            PermissionMode::Chat,
            "pixel_apply_operations"
        ));
        assert!(needs_approval(PermissionMode::Chat, "pixel_run_shader"));
        assert!(!needs_approval(PermissionMode::Chat, "pixel_read_canvas"));
    }

    #[tokio::test]
    async fn resolving_without_a_pending_request_is_an_error() {
        let s = session();
        assert!(s
            .resolve_approval("nope", ApprovalDecision::Approve)
            .is_err());
    }

    #[tokio::test]
    async fn interrupt_clears_a_pending_approval_so_the_wait_unblocks() {
        let s = session();
        let (tx, mut rx) = oneshot::channel();
        *s.approval.lock().unwrap() = Some(ApprovalSlot {
            call_id: "call-1".into(),
            tx,
        });
        s.interrupt();
        // 发送端被 take 掉，等待端收到的是「通道关闭」，也就是取消，不是放行。
        assert!(rx.try_recv().is_err());
        assert!(s.is_cancelled());
        assert!(s
            .resolve_approval("call-1", ApprovalDecision::Approve)
            .is_err());
    }

    #[tokio::test]
    async fn a_decision_reaches_the_waiting_turn() {
        let s = session();
        let (tx, rx) = oneshot::channel();
        *s.approval.lock().unwrap() = Some(ApprovalSlot {
            call_id: "call-1".into(),
            tx,
        });
        assert!(s
            .resolve_approval("call-1", ApprovalDecision::ApproveAll)
            .is_ok());
        assert_eq!(rx.await.unwrap(), ApprovalDecision::ApproveAll);
        // 槽已空，同一笔再解决一次就是「没有挂起的审批」。
        assert!(s
            .resolve_approval("call-1", ApprovalDecision::Approve)
            .is_err());
    }
}
