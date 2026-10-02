// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! agent 数据模型：会话消息、Provider 配置、工具规格、运行事件。
//! 全部可序列化，方便跨 Tauri 命令边界与未来的持久化层。

use super::workflows::WorkflowKind;
use pixel_core::DocPatch;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 一条消息里的内容块。文本网格是权威状态，图片只是上下文。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// base64 位图 + media type（如 image/png），作为上下文附在用户消息上。
    Image {
        media_type: String,
        data_base64: String,
    },
    /// 模型推理片段（不回灌给提供方，只用于 UI 展示）。
    Reasoning {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    /// 纯文本用户消息。图片不进这个构造器：附件带角色（快照 / 参考），
    /// 角色必须在过协议边界之前定好，所以带图的消息另行组装。
    pub fn user_text(text: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    /// 助手消息。直传 blocks 而不是拼字符串：工具调用和推理块都在这里，
    /// 提前拼平成文本会把「模型调了哪个工具」从历史里抹掉。
    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Message {
            role: Role::Assistant,
            content: blocks,
        }
    }

    /// 工具结果消息。`is_error` 一路带到协议层：把失败也写成成功，
    /// 模型会因为「看起来做完了」而收工，用户拿到一张空画布。
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: content.into(),
                is_error,
            }],
        }
    }

    /// 只取文本块。用于展示和续写判定，不含工具调用——
    /// 「这一轮到底动没动笔」要看的是有没有画图工具，不是说了多少话。
    pub fn text_of(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Provider 协议：Anthropic Messages 或 OpenAI 兼容 Chat Completions。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Anthropic,
    OpenAiCompat,
}

/// 用户自带的模型/Provider 配置。无登录、无计费。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub id: String,
    pub label: String,
    pub protocol: Protocol,
    /// API 根地址，含版本段，如 https://api.openai.com/v1 或 https://api.anthropic.com/v1。
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    /// 关掉模型的思考（thinking）环节。
    ///
    /// `None` = 不干预：模型自己想就多想。但这会出事——LongCat、DeepSeek-R1
    /// 这一类拿到工具也先把整份 Lua 在脑子里写完，一轮预算烧光、一个工具都没
    /// 调，用户看到的是永远在思考的节点。所以 `None` 时 runner 仍留了一手：
    /// 撞了输出上限又没调工具，就自动关思考重问一次（见 runner 的兜底）。
    /// `Some(true)` = 每轮都关，最跟手；`Some(false)` = 用户要留着思考看。
    #[serde(default)]
    pub disable_thinking: Option<bool>,
    /// 用户声明的模型能力，决定哪些工作流可跑（见 workflows.rs）。
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// 用户模型的能力勾选。跟着模型配置一起持久化。
///
/// 刻意不做自动探测：探测要靠猜端点的返回，容易把「模型不支持」误判成「网络坏了」，
/// 而且会让能力随对方服务端的静默变更而漂移。由用户声明，错了也能立刻自己改。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// 能读参考图（多模态输入）。
    #[serde(default)]
    pub vision: bool,
    /// 能直接产出位图（images/generations 或 modalities 带 image）。
    #[serde(default)]
    pub image_gen: bool,
    /// 能吃视频输入。
    #[serde(default)]
    pub video: bool,
    /// 会先输出推理（thinking）内容，聊天里按思考块展示。
    #[serde(default)]
    pub reasoning: bool,
}

impl Capabilities {
    pub fn any(self) -> bool {
        self.vision || self.image_gen || self.video
    }

    /// 这个工作流需要但当前模型没有的能力名，空列表表示齐了。
    pub fn missing_for(self, kind: WorkflowKind) -> Vec<&'static str> {
        let mut out = Vec::new();
        if kind.needs_vision() && !self.vision {
            out.push("vision");
        }
        if kind.needs_image_gen() && !self.image_gen {
            out.push("image_gen");
        }
        if kind.needs_video() && !self.video {
            out.push("video");
        }
        out
    }
}

/// 权限模式：Auto 直接执行；Ask 每个调用都问；Chat 只拦写操作（读回网格直接放行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Auto,
    Chat,
    Ask,
}

/// 一条待审批工具调用的用户决定。ApproveAll 只把当前 turn 降级成 Auto，
/// 不写回会话：用户说的是「这次别烦我」，不是「以后都别问」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Reject,
    ApproveAll,
}

/// 编辑器当前激活的图层 / 帧 / 笔刷颜色，注入提示词与工具调用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveContext {
    pub layer: String,
    pub frame: String,
    #[serde(default)]
    pub color: Option<String>,
}

impl Default for ActiveContext {
    fn default() -> Self {
        ActiveContext {
            layer: "L0".into(),
            frame: "F0".into(),
            color: None,
        }
    }
}

/// 工具的对外描述（协议无关）。schema 为 JSON Schema。
#[derive(Debug, Clone)]
pub struct ToolSpec {
    /// 内建工具编译期借用；MCP 工具运行时来自服务器，用 `Cow::Owned`。
    pub name: Cow<'static, str>,
    pub description: Cow<'static, str>,
    pub schema: serde_json::Value,
}

/// 一次 chat 请求（协议无关，由 provider 负责翻译）。
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    /// 这一发请求要不要关掉模型的思考环节。
    ///
    /// 由 `ModelConfig::disable_thinking` 与 runner 的自动兜底共同决定。
    /// provider 按各自协议的官方写法翻译：OpenAI 兼容端点认
    /// `thinking.type=disabled` 与 `chat_template_kwargs.enable_thinking=false`
    /// （LongCat / 通义 / vLLM 这条 FaQ 走通了），Anthropic 认
    /// `thinking.type=disabled`。
    pub disable_thinking: bool,
    /// 是否把 assistant 的推理内容回灌给模型。
    ///
    /// DeepSeek-R1 一类的推理模型要求每轮都把上一轮的 `reasoning_content` 原样带回，
    /// 否则它会当作全新的一轮重新想一遍——续写时表现为把前面写过的又念一次。
    /// 但不是所有 OpenAI 兼容端点都认这个字段，所以由 provider 按模型名决定。
    pub echo_reasoning: bool,
}

/// 运行预算与策略。
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    /// 单个 turn 内允许的最大工具步数。
    pub max_tool_steps: usize,
    /// 单个工具结果回灌给模型时的最大字符数（截断保护）。
    pub max_tool_result_bytes: usize,
    /// 单个 turn 内允许的最大续轮次数（模型反复调工具时的兜底）。
    pub max_turns: usize,
    /// 系统提示词里 canvas RLE 窗口的字符预算。
    pub canvas_context_chars: usize,
    pub permission: PermissionMode,
    /// 防死循环护栏：续写、重试、纯思考续写各自封顶。可在设置里改。
    pub loop_limits: LoopLimits,
}

/// 运行护栏：把「续写、重试、只吐思考」三件事都封顶。
///
/// 模型再轴，这一轮也有收摊的时刻。没有这道闸，推理模型能把整轮预算全烧在
/// 思考上，续写二十次还在想——用户盯着一个永远转圈的思考节点，什么也点不了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoopLimits {
    /// 撞上输出上限后替用户自动续写几次。0 表示不续写，撞线就现形。
    pub max_continuations: usize,
    /// 一次请求失败后重发几次。0 表示失败立即现形。
    pub max_retries: usize,
    /// 连续几轮只吐推理、正文和工具调用一个都没有就收手。这条比续写总数更硬：
    /// 「一直在想、始终不动笔」正是无限思考的形态，续写只会让它想得更久。
    pub max_reasoning_continuations: usize,
}

impl LoopLimits {
    /// 默认值。写成常量，好让 runner 的剧本测试按同一组数排 request。
    pub const DEFAULT: LoopLimits = LoopLimits {
        max_continuations: 20,
        max_retries: 5,
        max_reasoning_continuations: 2,
    };

    /// 把用户填的数收进合理区间。填 0 是合法意愿（关掉这项自动行为），
    /// 填个十万只会把用户自己坑死，所以封顶。
    pub fn clamped(mut self) -> Self {
        self.max_continuations = self.max_continuations.min(50);
        self.max_retries = self.max_retries.min(10);
        self.max_reasoning_continuations = self.max_reasoning_continuations.min(10);
        self
    }
}

impl Default for LoopLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 图片附件的角色。快照只是上下文，参考图是用户的视觉真值——
/// 系统提示词对两者的约束完全不同，所以角色必须随图片一起过边界。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentRole {
    Snapshot,
    Reference,
}

/// 参考图的参照方式。用户贴一张图进来，可能是「只借画风」，也可能是「照着实临摹」；
/// 两种说法在提示词里的约束完全相反，所以必须显式区分，不能凭模型猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceMode {
    Style,
    Full,
}

impl ReferenceMode {
    /// 字符串形式给模型入参和提示词用：工具 schema 里没有枚举类型，
    /// 模型写进来的是字符串，回出去的也必须是同一个词，否则对不上账。
    pub fn as_str(self) -> &'static str {
        match self {
            ReferenceMode::Style => "style",
            ReferenceMode::Full => "full",
        }
    }

    /// 从模型入参里认模式。别名是防手滑的：同一个意思模型能写出四五种拼法，
    /// 认不出的调用方报错，绝不静默按完全参照处理——那会画出一个复刻。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "style" | "style_reference" | "style only" | "art_style" => Some(ReferenceMode::Style),
            "full" | "full_reference" | "complete" | "exact" | "copy" => Some(ReferenceMode::Full),
            _ => None,
        }
    }
}

impl std::fmt::Display for ReferenceMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一条待发送的图片附件：角色 + media type + 不带 data: 前缀的 base64。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub role: AttachmentRole,
    pub media_type: String,
    pub data_base64: String,
}

impl Attachment {
    /// 生成提示词里的图片清单说明：模型必须知道每张图的身份与顺序。
    ///
    /// `modes` 与 `items` 等长，参考图那位给出参照方式（`TurnPlan` 定的）。
    /// 约束原文跟着每张图写清楚：定性当场生效，模型不必再猜「这算哪种参照」。
    pub fn caption(items: &[Attachment], modes: &[Option<ReferenceMode>]) -> String {
        let mut out = String::from("Images attached to this message, in order:\n");
        for (i, item) in items.iter().enumerate() {
            let number = i + 1;
            match item.role {
                AttachmentRole::Reference => {
                    // 缺模式号时按完全参照兜底：宁可照着实临摹，也不能让模型
                    // 拿到一张没有约束的参考图自由发挥。
                    let mode = modes
                        .get(i)
                        .copied()
                        .flatten()
                        .unwrap_or(ReferenceMode::Full);
                    out.push_str(&format!(
                        "{number}. reference image ({}) - reference mode: {mode}. {}\n",
                        item.media_type,
                        super::references::rules(mode)
                    ));
                }
                AttachmentRole::Snapshot => out.push_str(&format!(
                    "{number}. canvas snapshot ({}) - the canvas as it was when you sent this message: context only, never a request to redraw it; the authoritative canvas is the text grid above plus tool results.\n",
                    item.media_type
                )),
            }
        }
        out
    }
}

impl Default for RunnerConfig {
    fn default() -> Self {
        RunnerConfig {
            max_tool_steps: 24,
            max_tool_result_bytes: 6000,
            max_turns: 12,
            canvas_context_chars: 4096,
            permission: PermissionMode::Auto,
            loop_limits: LoopLimits::default(),
        }
    }
}

/// 键控界面文案：Rust 只说键和变量，措辞由前端字典按当前语言拼。
/// 字典缺键（版本错位）时退回 fallback，因此用户永远看得到一句完整的话。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiText {
    pub key: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, serde_json::Value>,
    pub fallback: String,
}

impl UiText {
    pub fn new(key: impl Into<String>, fallback: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            vars: BTreeMap::new(),
            fallback: fallback.into(),
        }
    }

    pub fn with(mut self, name: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.vars.insert(name.into(), value.into());
        self
    }
}

/// 广播给 UI 的进程事件（对应前端的 agent-event / agent-document）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    Status {
        message: UiText,
    },
    Token {
        text: String,
    },
    Reasoning {
        text: String,
    },
    /// 一次工具调用已定型（含完整入参）。
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// 权限模式要求审批：主循环停在这一步，等 `resolve_approval` 给决定。
    ApprovalRequest {
        call_id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        id: String,
        name: String,
        summary: String,
        is_error: bool,
    },
    /// 工具改完文档后把新文档推回前端（文本网格为权威）。
    DocumentUpdated {
        revision: u64,
        /// 相对上一次广播的增量，不是整份文档。256x256x8层x30帧整份序列化
        /// 约 47MB，一轮几十次工具调用会把 IPC 灌满；前端本地已持有一份
        /// 完整文档，所以只推元数据加变化过的 cel。
        patch: DocPatch,
    },
    Usage {
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    },
    Completed {
        turns: usize,
    },
    Error {
        message: UiText,
    },
    Interrupted,
}

/// `agent-event` 通道上的载荷：事件本身，加上它属于哪个会话。
///
/// 通道是全局的，而会话随时在切。用户切走时上一个回合并不会当场去世——中断要等
/// 主循环下一次轮询取消旗（百来毫秒），这期间它还在往外发。少了会话标识，那些残
/// 事件就会落到新会话头上：token 拼进新对话的尾巴，document_updated 把新画布
/// 整个盖成上一个会话的画面。所以每个事件都必须自己说清楚是谁发的。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEventEnvelope {
    pub session_id: String,
    pub event: AgentEvent,
}

/// provider 流式事件（由 providers 解析填充，交给 runner 归一）。
#[derive(Debug, Clone)]
pub enum LlmEvent {
    Token(String),
    Reasoning(String),
    ToolUseStart {
        index: usize,
        id: String,
        name: String,
    },
    ToolInputDelta {
        index: usize,
        json_partial: String,
    },
    Usage {
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
    },
    /// 流正常结束，附带 stop_reason。
    Done {
        stop_reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixel_core::Document;

    #[test]
    fn caption_numbers_attachments_in_order_and_names_their_role() {
        let items = vec![
            Attachment {
                role: AttachmentRole::Reference,
                media_type: "image/png".into(),
                data_base64: "AAAA".into(),
            },
            Attachment {
                role: AttachmentRole::Snapshot,
                media_type: "image/png".into(),
                data_base64: "BBBB".into(),
            },
        ];
        let caption = Attachment::caption(&items, &[Some(ReferenceMode::Full), None]);
        assert!(caption.starts_with("Images attached to this message, in order:\n"));
        assert!(caption.contains("1. reference image (image/png) - reference mode: full."));
        assert!(caption.contains("2. canvas snapshot (image/png)"));
        // 角色措辞必须可区分：快照不许被当成重绘请求，参考图是视觉真值。
        assert!(caption.contains("context only, never a request to redraw it"));
        assert!(caption.contains("reproduce its subject, composition, proportions and palette"));
    }

    #[test]
    fn a_style_reference_hands_the_subject_back_to_the_user() {
        let items = vec![Attachment {
            role: AttachmentRole::Reference,
            media_type: "image/png".into(),
            data_base64: "AAAA".into(),
        }];
        let caption = Attachment::caption(&items, &[Some(ReferenceMode::Style)]);
        assert!(caption.contains("reference mode: style."), "{caption}");
        // 风格参照下主体必须听用户的：只借用色、光向、描边和抖动。
        assert!(caption.contains("take ONLY its palette, ramps, light direction"));
        assert!(caption.contains("must come from the user's words"));
    }

    #[test]
    fn a_missing_mode_falls_back_to_full_rather_than_no_constraint() {
        let items = vec![Attachment {
            role: AttachmentRole::Reference,
            media_type: "image/png".into(),
            data_base64: "AAAA".into(),
        }];
        let caption = Attachment::caption(&items, &[]);
        assert!(caption.contains("reference mode: full."), "{caption}");
    }

    #[test]
    fn role_serializes_as_snake_case() {
        let value = serde_json::to_value(AttachmentRole::Snapshot).unwrap();
        assert_eq!(value, serde_json::json!("snapshot"));
        let value = serde_json::to_value(AttachmentRole::Reference).unwrap();
        assert_eq!(value, serde_json::json!("reference"));
    }

    #[test]
    fn the_envelope_carries_the_session_alongside_the_event() {
        let doc = Document::new("t", 8, 8).expect("8x8 is within limits");
        let envelope = AgentEventEnvelope {
            session_id: "s-1".into(),
            event: AgentEvent::DocumentUpdated {
                revision: 7,
                patch: DocPatch::full(&doc),
            },
        };
        let value = serde_json::to_value(&envelope).unwrap();
        // 会话标识与事件平级：前端先看 session_id 再决定要不要展开 event。
        assert_eq!(value["session_id"], serde_json::json!("s-1"));
        assert_eq!(
            value["event"]["kind"],
            serde_json::json!("document_updated")
        );
        assert_eq!(value["event"]["revision"], serde_json::json!(7));
        // 载荷是增量不是整份文档：patch 里的 cel 是 (layer, frame, indices) 三元组。
        assert_eq!(value["event"]["patch"]["width"], serde_json::json!(8));
        // full 增量带着全部 cel：8x8 文档每层每帧一格，新文档就是一格。
        assert_eq!(
            value["event"]["patch"]["cels"].as_array().map(|c| c.len()),
            Some(1)
        );
        // 回程也得通：前端那条路是反序列化，字段名一字都不能差。
        let back: AgentEventEnvelope = serde_json::from_value(value).unwrap();
        assert_eq!(back.session_id, "s-1");
        match back.event {
            AgentEvent::DocumentUpdated { revision, .. } => assert_eq!(revision, 7),
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
