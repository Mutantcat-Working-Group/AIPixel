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

/** 用户勾选的模型能力：决定哪些工作流跑得动（见 agent-core workflows.rs）。 */
export interface Capabilities {
  /** 能吃参考图（多模态输入）。 */
  vision: boolean;
  /** 能直接产出位图。 */
  image_gen: boolean;
  /** 能吃视频输入。 */
  video: boolean;
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
  capabilities: Capabilities;
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
  capabilities: Capabilities;
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

/** MCP 传输的脱敏视图：env / headers 只有键名，明文凭据不回 webview。 */
export interface McpTransportView {
  kind: "stdio" | "http";
  command: string;
  args: string[];
  env_keys: string[];
  url: string;
  header_keys: string[];
}

/** 一个 MCP 工具的视图。name 是命名空间后的全名，模型看到的就是它。 */
export interface McpToolView {
  name: string;
  server: string;
  tool: string;
  description: string;
}

export interface McpServerView {
  name: string;
  transport: McpTransportView;
  auto_connect: boolean;
  connected: boolean;
  tools: McpToolView[];
  /** 最近一次连接失败的原因；连上了就是 null。 */
  last_error: string | null;
}

export interface McpServersView {
  entries: McpServerView[];
}

/** 发给 Rust 的服务器配置。env / headers 只出不进：视图永不回传明文。 */
export type McpTransportConfig =
  | { kind: "stdio"; command: string; args: string[]; env: Record<string, string> }
  | { kind: "http"; url: string; headers: Record<string, string> };

export interface McpServerConfig {
  name: string;
  transport: McpTransportConfig;
  auto_connect: boolean;
}

export interface SessionInfo {
  id: string;
  model_id: string;
  model_label: string;
  /** 四个角色各自在干活的模型。没分工就是主模型，`detached` 是 false。 */
  roles: RoleBinding[];
  width: number;
  height: number;
  revision: number;
}

/** 会话里的一种模型分工。chat 就是会话主模型，另外三个能另绑一个。 */
export type ModelRole = "chat" | "image_gen" | "vision" | "video";

/** 一个角色实际在干活的活。回落主模型时 model_id/model_label 是主模型那两个。 */
export interface RoleBinding {
  role: ModelRole;
  model_id: string;
  model_label: string;
  detached: boolean;
}

export interface Attachment {
  role: AttachmentRole;
  media_type: string;
  data_base64: string;
}

/** AgentEvent 的 kind tag，snake_case。 */
export type AgentEvent =
  | { kind: "status"; message: UiText }
  | { kind: "token"; text: string }
  | { kind: "reasoning"; text: string }
  | { kind: "tool_call"; id: string; name: string; input: Record<string, unknown> }
  | {
      kind: "approval_request";
      call_id: string;
      name: string;
      input: Record<string, unknown>;
    }
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

export type ToolName =
  | "pixel_apply_operations"
  | "pixel_read_canvas"
  | "pixel_run_shader"
  | "pixel_tween_frames"
  | "pixel_pixelize_image"
  | "pixel_generate_image";

// ---------- 工作台 ----------

/** 审批的三种决定，对应 Rust ApprovalDecision 的 snake_case 拼写。 */
export type ApprovalDecision = "approve" | "reject" | "approve_all";

/** 主循环停在工具调用上等决定时，前端持的这张票（字段转成前端习惯的 camelCase）。 */
export interface PendingApproval {
  callId: string;
  name: string;
  input: Record<string, unknown>;
}

/**
 * 编辑器直接下的结构与帧操作：op tag 与字段名逐字锚定 Rust PixelOperation
 * （serde 内部 tag = "op"），改 Rust 要同步这里。
 */
export type EditorOperation =
  | { op: "create_frame"; after?: string | null; duration_ms?: number; id?: string | null }
  | { op: "delete_frame"; id: string }
  | { op: "duplicate_frame"; id: string }
  | { op: "move_frame"; id: string; to_index: number }
  | { op: "set_frame_duration"; id: string; duration_ms: number }
  | { op: "create_layer"; after?: string | null; name?: string | null; id?: string | null }
  | { op: "delete_layer"; id: string }
  | { op: "move_layer"; id: string; to_index: number }
  | { op: "rename_layer"; id: string; name: string }
  | { op: "set_layer_properties"; id: string; visible?: boolean | null; opacity?: number | null }
  | { op: "add_palette_colors"; colors: string[] }
  | { op: "set_palette"; colors: string[] };

/** 落笔颜色：hex 字面量；null = 擦回透明（索引 0）。 */
export type InkColor = string | null;

/** 编辑器工具。橡皮是独立工具：落笔即擦回透明，不必先去调色板点到透明格。 */
export type EditorTool = "brush" | "eraser" | "fill";

export interface StrokeCell {
  x: number;
  y: number;
}

/** 一笔笔画：跨 move 事件采样，抬笔时才整笔发给 Rust。 */
export interface StrokeRequest {
  layer: string;
  frame: string;
  cells: StrokeCell[];
  color: InkColor;
}

// ---------- 工作流 ----------

/** 七条工作流。前五条覆盖「不同模型怎么做同一件事」，后两条是编辑助手。 */
export type WorkflowKind =
  | "agent"
  | "image_gen"
  | "vision_brief"
  | "video_frames"
  | "video_brief"
  | "frame_tween"
  | "prompt_refine";

/**
 * 面板里可选的条目。比 WorkflowKind 多一个 quantize：把一张现成位图量化到画布上，
 * 纯本机、不吃模型能力，所以它不进能力目录，但确实是常用的一条。
 */
export type DockKind = WorkflowKind | "quantize";

export interface WorkflowInfo {
  kind: WorkflowKind;
  id: string;
  title: string;
  summary: string;
  needs: Capabilities;
  /** 跑完会得到什么，给 UI 当结果说明。 */
  output: string;
}

/** 目录项：WorkflowInfo 在 Rust 侧是 #[serde(flatten)]，所以前台看到的是同一层字段。 */
export interface WorkflowEntry extends WorkflowInfo {
  readiness: Readiness;
  /** 这条流程实际由哪个角色干活；角色没单独绑模型时是 chat。 */
  served_by: ModelRole;
  /** 干活那个模型的名字，给面板显示「生图由 X 提供」。 */
  served_by_label: string;
}

/** 当前会话绑的模型跑不跑得动。 */
export type Readiness =
  | { state: "ready" }
  | { state: "blocked"; missing: string[] };

/**
 * Rust 侧传来的一句话：键 + 插值变量 + 一句英文后备。
 * 界面语言只存在于前端字典里，所以 Rust 永远不给拼好的句子；
 * 键不在字典里（版本错位）时用 fallback 原样展示。
 */
export interface UiText {
  key: string;
  vars?: Record<string, string | number>;
  fallback: string;
}

/** 位图落点：盖在当前 cel 上，还是新建一帧。 */
export type LandSpot = "active_cel" | "new_frame";

/** 提示词微调的用途：给 Lua shader 还是给生图模型。 */
export type RefineTarget = "shader" | "image_gen";

export type FitMode = "contain" | "stretch";

export type TweenMode = "copy" | "blend" | "migrate";

export type MigrateOrder = "scan" | "radial" | "scatter";

export interface PixelizeOptions {
  /** 从位图里最多提取多少种主色。 */
  max_colors: number;
  /** 命中画布已有调色板的容差。 */
  snap_tolerance: number;
  /** 允许为找不到近似色的主色新增调色板项。 */
  expand_palette: boolean;
  /** 有序抖动（Bayer 4x4）。 */
  dither: boolean;
  /** alpha 低于该值的像素视为透明（索引 0）。 */
  alpha_threshold: number;
  fit: FitMode;
}

export interface RefinedPrompt {
  /** 微调后的提示词，用户要逐行改的就是这段。 */
  prompt: string;
  /** 模型原文，解析不全时留个痕迹。 */
  raw: string;
}

export interface VisionBrief {
  subject: string;
  silhouette: string;
  /** 主色到暗色的有序色板，hex。 */
  palette: string[];
  pose_notes: string;
  proportions: string;
  craft_notes: string;
  raw: string;
}

export interface VideoProbe {
  width: number | null;
  height: number | null;
  duration_s: number | null;
  fps: number | null;
  frame_count: number | null;
  codec: string | null;
  has_audio: boolean;
}

/** 信息是从哪儿来的：目录来源没有 fps / duration 可言，文案要跟着改。 */
export type ProbeSource = "ffprobe" | "directory" | "none";

export interface VideoProbeResult {
  probe: VideoProbe;
  source: ProbeSource;
}

/**
 * 视频运动简报：读视频模型把一段视频读成「怎么动」。
 * 与 VisionBrief 的分工就在字段上：这边全是运动语义，没有剪影和比例。
 */
export interface VideoBrief {
  subject: string;
  motion: string;
  /** 按时间顺序的关键姿态。 */
  key_poses: string[];
  timing: string;
  palette: string[];
  craft_notes: string;
  raw: string;
}

export interface TweenParams {
  from_frame: string;
  to_frame: string;
  count?: number;
  mode?: TweenMode;
  order?: MigrateOrder;
  ease?: boolean;
  duration_ms?: number;
  layer?: string | null;
}

export interface PixelizeParams {
  image_base64: string;
  /** 裸 base64 时必须给；带 data: 前缀时忽略。 */
  media_type?: string | null;
  options?: PixelizeOptions | null;
  layer?: string | null;
  frame?: string | null;
}

export interface ImageGenParams {
  prompt: string;
  /** 形如 "1024x1024"，只有 chat modalities 传输认这个。 */
  size?: string | null;
  /** 垫图路径：拿一张图让模型照着改。 */
  reference_path?: string | null;
  /** 垫图帧：把文档里这一帧合成交给模型，与 reference_path 互斥。 */
  reference_frame?: string | null;
  options?: PixelizeOptions | null;
  spot?: LandSpot;
  duration_ms?: number;
}

export interface VideoFramesParams {
  path: string;
  /** 0 表示「全都要」。 */
  count?: number;
  options?: PixelizeOptions | null;
  duration_ms?: number;
}

export interface VideoBriefParams {
  path: string;
  /** 从素材里抽多少帧给模型看；0 或省略走 Rust 侧默认。 */
  count?: number;
}

export interface WorkflowOutcome {
  revision: number;
  summary: UiText;
  detail?: Record<string, unknown> | null;
}

/**
 * 工作流坞七条面板共用的表单草稿。
 * 放 store 里而不是坞组件里：切到画布看一眼调色板再切回来，刚敲的提示词不该丢。
 */
export interface DockDraft {
  prompt: string;
  /** 形如 "1024x1024"，只有 chat modalities 传输认这个。 */
  size: string;
  genPath: string | null;
  /** 垫图从哪来。画布帧才是「改这一帧」：磁盘那张可能是旧导出。 */
  genSource: "none" | "file" | "frame";
  /** genSource 为 frame 时的帧 id。 */
  genFrame: string;
  spot: LandSpot;
  durationMs: number;
  options: PixelizeOptions;
  idea: string;
  visionPath: string | null;
  videoPath: string | null;
  /** 0 = 全都要，上限由 Rust 的 MAX_EXTRACT_FRAMES 管。 */
  videoCount: number;
  /** 给读视频模型看几帧；0 或省略走 Rust 默认。与 videoCount 分开：那边 0 是「全部」，这边不是。 */
  briefCount: number;
  quantizePath: string | null;
  tweenFrom: string;
  tweenTo: string;
  tweenCount: number;
  tweenMode: TweenMode;
  tweenOrder: MigrateOrder;
  tweenEase: boolean;
}

// ---------- 批量工作台 ----------
// 纯本机批处理：一个文件夹进、一个文件夹出，不吃模型，与 agent 会话互补。

export type BatchKind = "quantize" | "export";

export type ExportFormat = "png" | "gif";

/** 可序列化的批量脚本。字段与 Rust 侧 `BatchRecipe` 逐字对齐（camelCase 由 Tauri 转换）。 */
export interface BatchRecipe {
  kind: BatchKind;
  input_dir: string;
  output_dir: string;
  options: PixelizeOptions;
  match_source_size: boolean;
  target_w: number;
  target_h: number;
  export_format: ExportFormat;
}

export interface BatchScan {
  kind: BatchKind;
  dir: string;
  count: number;
  truncated: boolean;
  files: string[];
}

/** 一条存下来的配方：名字加一份串好的 recipe，落在 app config 目录的 recipes.json。 */
export interface BatchRecipeEntry {
  name: string;
  recipe: BatchRecipe;
}

/** 一条配方从 `.aipr` 进来后的去向。 */
export type RecipeImportState = "imported" | "renamed" | "skipped";

/** 回执里的一行：文件里请求的名字、实际落下的名字、跳过原因。 */
export interface RecipeImportRow {
  name: string;
  final_name: string;
  state: RecipeImportState;
  /** 为什么跳过；进簿子了就是空串。 */
  note: string;
}

/** 导入回执：逐条交代，外加合并后的整本簿子。 */
export interface RecipeImportReport {
  rows: RecipeImportRow[];
  entries: BatchRecipeEntry[];
}

export type BatchItemState = "ok" | "skipped" | "error";

export type BatchEvent =
  | { kind: "started"; total: number }
  | {
      kind: "progress";
      index: number;
      total: number;
      file: string;
      state: BatchItemState;
      note: string;
    }
  | {
      kind: "done";
      batch_kind: BatchKind;
      ok: number;
      skipped: number;
      failed: number;
      output_dir: string;
    };
