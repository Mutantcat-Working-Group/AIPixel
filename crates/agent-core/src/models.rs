//! agent 数据模型：会话消息、Provider 配置、工具规格、运行事件。
//! 全部可序列化，方便跨 Tauri 命令边界与未来的持久化层。

use super::workflows::WorkflowKind;
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
    pub fn user_text(text: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant(blocks: Vec<ContentBlock>) -> Self {
        Message {
            role: Role::Assistant,
            content: blocks,
        }
    }

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

/// 一条待发送的图片附件：角色 + media type + 不带 data: 前缀的 base64。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub role: AttachmentRole,
    pub media_type: String,
    pub data_base64: String,
}

impl Attachment {
    /// 生成提示词里的图片清单说明：模型必须知道每张图的身份与顺序。
    pub fn caption(items: &[Attachment]) -> String {
        let mut out = String::from("Images attached to this message, in order:\n");
        for (i, item) in items.iter().enumerate() {
            match item.role {
                AttachmentRole::Reference => out.push_str(&format!(
                    "{}. reference image ({}) - the user's visual ground truth: match its subject, proportions and palette, simplified into clean pixel art at the canvas resolution.\n",
                    i + 1,
                    item.media_type
                )),
                AttachmentRole::Snapshot => out.push_str(&format!(
                    "{}. canvas snapshot ({}) - the canvas as it was when you sent this message: context only, never a request to redraw it; the authoritative canvas is the text grid above plus tool results.\n",
                    i + 1,
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
        document: serde_json::Value,
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
        let caption = Attachment::caption(&items);
        assert!(caption.starts_with("Images attached to this message, in order:\n"));
        assert!(caption.contains("1. reference image (image/png)"));
        assert!(caption.contains("2. canvas snapshot (image/png)"));
        // 角色措辞必须可区分：快照不许被当成重绘请求，参考图是视觉真值。
        assert!(caption.contains("context only, never a request to redraw it"));
        assert!(caption.contains("visual ground truth"));
    }

    #[test]
    fn role_serializes_as_snake_case() {
        let value = serde_json::to_value(AttachmentRole::Snapshot).unwrap();
        assert_eq!(value, serde_json::json!("snapshot"));
        let value = serde_json::to_value(AttachmentRole::Reference).unwrap();
        assert_eq!(value, serde_json::json!("reference"));
    }
}
