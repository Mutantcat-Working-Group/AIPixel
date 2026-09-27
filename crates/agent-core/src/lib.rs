//! agent-core: 像素画 agent 的 Rust 执行主循环。
//!
//! 面向开源与「只用用户自带模型」重做的像素画 agent 主循环：
//! - 不内置服务器、无登录、无计费；只接受用户在 UI 配置的 Provider（Anthropic / OpenAI 兼容）。
//! - 文档是唯一权威状态（文本网格优先），所有生图经由 pixel-core 的三个落地工具完成，
//!   「不让模型手写矩阵」：结构改动走 ops，绘制/动画走 Lua 沙箱，读回走 RLE。
//! - 执行流：prompt admission -> provider 流式输出 -> tool_use -> 工具执行 -> 结果回填 -> 续轮，
//!   带工具步数 / 字节 / Turn 预算与中断保护。
//!
//! 前后端边界：本 crate 只做纯 Rust，不依赖 Tauri。UI 通过一个 `tokio::sync::mpsc` 通道接收
//! `AgentEvent`，由上层（src-tauri）转成 Tauri 事件广播。

pub mod models;
pub mod prompt;
pub mod providers;
pub mod runner;
pub mod tools;

pub use models::{
    ActiveContext, AgentEvent, Attachment, AttachmentRole, ChatRequest, ContentBlock, LlmEvent,
    Message, ModelConfig, PermissionMode, Protocol, Role, RunnerConfig, ToolSpec,
};
pub use providers::{build_provider, EventStream, LlmProvider, ProviderError};
pub use runner::AgentSession;
