//! agent 数据模型：会话消息、Provider 配置、工具规格、运行事件。
//! 全部可序列化，方便跨 Tauri 命令边界与未来的持久化层。

use serde::{Deserialize, Serialize};

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
}

/// 权限模式：Auto 直接执行；Chat/Ask 需要审批（工作台阶段接交互，主循环预留）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Auto,
    Chat,
    Ask,
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
    pub name: &'static str,
    pub description: &'static str,
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
        }
    }
}

/// 广播给 UI 的进程事件（对应前端的 agent-event / agent-document）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    Status {
        message: String,
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
        message: String,
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
