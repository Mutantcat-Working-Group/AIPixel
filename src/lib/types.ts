// 与 Rust 侧 models.rs / document.rs 一一对应的前端类型。
// serde 的 rename_all = "snake_case" 决定这里的字面量拼写，改 Rust 要同步。

export type AttachmentRole = "snapshot" | "reference";

/** Protocol::OpenAiCompat 的 snake_case 是 open_ai_compat，不是 openai_compat。 */
export type Protocol = "anthropic" | "open_ai_compat";
export type PermissionMode = "auto" | "chat" | "ask";

export interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

export interface Layer {
  id: string;
  name: string;
  visible: boolean;
  opacity: number;
}

export interface Frame {
  id: string;
  duration_ms: number;
}

/** 一个 cel 是 layer x frame 上的调色板索引网格，索引 0 = 透明。 */
export interface Cel {
  indices: number[];
}

/** 权威状态。cels 按 layer -> frame 嵌套（元组键过不了 serde_json）。 */
export interface PixelDocument {
  name: string;
  width: number;
  height: number;
  palette: Rgba[];
  layers: Layer[];
  frames: Frame[];
  cels: Record<string, Record<string, Cel>>;
  revision: number;
}

export type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; media_type: string; data_base64: string }
  | { type: "reasoning"; text: string }
  | { type: "tool_use"; id: string; name: string; input: Record<string, unknown> }
  | { type: "tool_result"; tool_use_id: string; content: string; is_error: boolean };

export interface Message {
  role: "system" | "user" | "assistant" | "tool";
  content: ContentBlock[];
}

/** 用户自带的模型配置。api_key 只在本机流转，不回传 webview。 */
export interface ModelConfig {
  id: string;
  label: string;
  protocol: Protocol;
  base_url: string;
  api_key: string;
  model: string;
  max_tokens: number | null;
  temperature: number | null;
}

/** 回给前端的模型视图：只剩 has_api_key 标志，密钥不离开 Rust。 */
export interface ModelView {
  id: string;
  label: string;
  protocol: Protocol;
  base_url: string;
  model: string;
  max_tokens: number | null;
  temperature: number | null;
  has_api_key: boolean;
}

export interface ModelsView {
  active_id: string;
  entries: ModelView[];
}

export interface ActiveContext {
  layer: string;
  frame: string;
  color?: string | null;
}

export interface SessionInfo {
  id: string;
  model_id: string;
  model_label: string;
  width: number;
  height: number;
  revision: number;
}

export interface Attachment {
  role: AttachmentRole;
  media_type: string;
  data_base64: string;
}

/** AgentEvent 的 kind tag，snake_case。 */
export type AgentEvent =
  | { kind: "status"; message: string }
  | { kind: "token"; text: string }
  | { kind: "reasoning"; text: string }
  | { kind: "tool_call"; id: string; name: string; input: Record<string, unknown> }
  | { kind: "tool_result"; id: string; name: string; summary: string; is_error: boolean }
  | { kind: "document_updated"; revision: number; document: PixelDocument }
  | { kind: "usage"; input_tokens: number | null; output_tokens: number | null }
  | { kind: "completed"; turns: number }
  | { kind: "error"; message: string }
  | { kind: "interrupted" };

export interface Usage {
  input: number | null;
  output: number | null;
}

/** 待发送的附件：role + 名字 + 前端预览，发送时只带 role/media_type/base64。 */
export interface PendingAttachment {
  key: string;
  role: AttachmentRole;
  name: string;
  mediaType: string;
  dataBase64: string;
  previewUrl: string;
}

/** 前端展示用的消息条目，从 AgentEvent 流推导而来。 */
export type TranscriptEntry =
  | { key: string; kind: "user"; text: string; attachments: PendingAttachment[] }
  | { key: string; kind: "assistant"; text: string; live: boolean }
  | { key: string; kind: "reasoning"; text: string; live: boolean }
  | {
      key: string;
      kind: "tool";
      id: string;
      name: string;
      input: Record<string, unknown>;
      summary: string | null;
      isError: boolean;
      live: boolean;
    }
  | { key: string; kind: "notice"; text: string; isError: boolean };

export type ToolName = "pixel_apply_operations" | "pixel_read_canvas" | "pixel_run_shader";
