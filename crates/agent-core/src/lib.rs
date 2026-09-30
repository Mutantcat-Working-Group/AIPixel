// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
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

pub mod artstyle;
pub mod colornames;
pub mod glossary;
pub mod imagegen;
pub mod intent;
pub mod knowledge;
pub mod limits;
pub mod mcp;
pub mod models;
pub mod one_shot;
pub mod plan;
pub mod prompt;
pub mod providers;
pub mod references;
pub mod refine;
pub mod roles;
pub mod runner;
pub mod tools;
pub mod video;
pub mod video_brief;
pub mod vision;
pub mod workflows;

pub use artstyle::{classify_text as classify_art_style, ArtStyle};
pub use colornames::{describe as describe_color, nearest_named, NAMED_COLORS};
pub use imagegen::{probe_image_support, ImageSupport, LandSpot};
pub use intent::{classify_text as classify_intent, Intent};
pub use knowledge::{ids as matched_knowledge_ids, section as knowledge_section, KnowledgeEntry};
pub use mcp::{
    namespaced_tool, McpClient, McpRegistry, McpServerConfig, McpTool, McpTransportConfig,
    MCP_TOOL_PREFIX,
};
pub use models::{
    ActiveContext, AgentEvent, AgentEventEnvelope, ApprovalDecision, Attachment, AttachmentRole,
    Capabilities, ChatRequest, ContentBlock, LlmEvent, LoopLimits, Message, ModelConfig,
    PermissionMode, Protocol, ReferenceMode, Role, RunnerConfig, ToolSpec, UiText,
};
pub use plan::{parse_updates as parse_plan_updates, spec as plan_spec, TurnPlan, PLAN_TOOL};
pub use providers::{build_provider, EventStream, LlmProvider, ProviderError};
pub use references::{
    classify_text as classify_reference_mode, parse_updates as parse_reference_updates, rules,
    ReferenceUpdate,
};
pub use refine::{refine, RefineRequest, RefineTarget, RefinedPrompt};
pub use roles::{ModelRole, RoleBinding};
pub use runner::AgentSession;
pub use video::{extract_frames, probe, ProbeSource, VideoProbe};
pub use video_brief::{
    brief_frame_indices, brief_video, source_note, VideoBrief, BRIEF_THUMB_MAX_DIM,
    MAX_BRIEF_FRAMES,
};
pub use vision::{brief_reference, VisionBrief};
pub use workflows::{available, catalog, info, readiness, Readiness, WorkflowInfo, WorkflowKind};
