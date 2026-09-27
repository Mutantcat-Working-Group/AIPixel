// Tauri 桥接层：所有 invoke / event 都收口在这里，UI 只面对类型。
// 参数名与 Rust 命令签名逐字对齐（Rust 侧是 snake_case，Tauri 会做 camelCase -> snake_case 转换）。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  ActiveContext,
  AgentEvent,
  Attachment,
  ApprovalDecision,
  EditorOperation,
  InkColor,
 Message,
  McpServerConfig,
  McpServersView,
 ModelConfig,
 ModelsView,
  PermissionMode,
  PixelDocument,
  SessionInfo,
  StrokeRequest,
  ImageGenParams,
  PixelizeParams,
  RefinedPrompt,
  RefineTarget,
  TweenParams,
  VideoBrief,
  VideoBriefParams,
  VideoFramesParams,
  VideoProbeResult,
  VisionBrief,
  WorkflowEntry,
  WorkflowOutcome,
} from "./types";

export const AGENT_EVENT_CHANNEL = "agent-event";

export function listModels(): Promise<ModelsView> {
  return invoke<ModelsView>("agent_list_models");
}

export function upsertModel(config: ModelConfig): Promise<ModelsView> {
  return invoke<ModelsView>("model_upsert", { config });
}

export function removeModel(id: string): Promise<ModelsView> {
  return invoke<ModelsView>("model_remove", { id });
}

export function setActiveModel(id: string): Promise<ModelsView> {
  return invoke<ModelsView>("model_set_active", { id });
}

export function listMcpServers(): Promise<McpServersView> {
  return invoke<McpServersView>("mcp_list");
}

export function upsertMcpServer(config: McpServerConfig): Promise<McpServersView> {
  return invoke<McpServersView>("mcp_upsert", { config });
}

export function removeMcpServer(name: string): Promise<McpServersView> {
  return invoke<McpServersView>("mcp_remove", { name });
}

export function connectMcpServer(name: string): Promise<McpServersView> {
  return invoke<McpServersView>("mcp_connect", { name });
}

export function disconnectMcpServer(name: string): Promise<McpServersView> {
  return invoke<McpServersView>("mcp_disconnect", { name });
}

export function createSession(document?: PixelDocument): Promise<SessionInfo> {
  return invoke<SessionInfo>("session_create", { document: document ?? null });
}

export function listSessions(): Promise<SessionInfo[]> {
  return invoke<SessionInfo[]>("session_list");
}

export function dropSession(id: string): Promise<void> {
  return invoke("session_drop", { id });
}

export function bindModel(id: string, modelId: string): Promise<SessionInfo> {
  return invoke<SessionInfo>("session_bind_model", { id, modelId });
}

export function setActive(id: string, active: ActiveContext): Promise<void> {
  return invoke("agent_set_active", { id, active });
}

export function setPermission(id: string, permission: PermissionMode): Promise<void> {
  return invoke("agent_set_permission", { id, permission });
}

export function sendMessage(
  id: string,
  text: string,
  attachments: Attachment[],
  modelId?: string,
): Promise<void> {
  return invoke("agent_send_message", { id, text, attachments, modelId: modelId ?? null });
}

export function interrupt(id: string): Promise<void> {
  return invoke("agent_interrupt", { id });
}

export function syncDocument(id: string, document: PixelDocument): Promise<{ revision: number }> {
  return invoke<{ revision: number }>("agent_sync_document", { id, document });
}

export function documentSnapshot(id: string): Promise<{
  id: string;
  revision: number;
  document: PixelDocument;
}> {
  return invoke("agent_document", { id });
}

export function agentHistory(id: string): Promise<Message[]> {
  return invoke<Message[]>("agent_history", { id });
}

export function pngUrl(id: string, frame?: number): Promise<string> {
  return invoke<string>("document_png_url", { id, frame: frame ?? null });
}

export function aipText(id: string): Promise<string> {
  return invoke<string>("aip_text", { id });
}

export function aipSave(id: string, path: string): Promise<string> {
  return invoke<string>("aip_save", { id, path });
}

export function aipLoad(path: string): Promise<PixelDocument> {
  return invoke<PixelDocument>("aip_load", { path });
}

export function readImageContext(path: string): Promise<Attachment> {
  return invoke<Attachment>("read_image_context", { path });
}

// ---------- 工作流 ----------
// params 是嵌套结构体，Tauri 的 camelCase 转换只作用于顶层参数名，
// 所以 params 里的键必须逐字写 Rust 的 snake_case 字段名（from_frame / duration_ms ...）。

export function workflowCatalog(id: string): Promise<WorkflowEntry[]> {
  return invoke<WorkflowEntry[]>("workflow_catalog", { id });
}

export function promptRefine(
  id: string,
  idea: string,
  width: number,
  height: number,
  target: RefineTarget,
): Promise<RefinedPrompt> {
  return invoke<RefinedPrompt>("prompt_refine", { id, idea, width, height, target });
}

export function videoProbe(path: string): Promise<VideoProbeResult> {
  return invoke<VideoProbeResult>("video_probe", { path });
}

export function visionBrief(id: string, path: string): Promise<VisionBrief> {
  return invoke<VisionBrief>("vision_brief", { id, path });
}

export function workflowImageGen(
  id: string,
  params: ImageGenParams,
): Promise<WorkflowOutcome> {
  return invoke<WorkflowOutcome>("workflow_image_gen", { id, params });
}

export function workflowPixelize(
  id: string,
  params: PixelizeParams,
): Promise<WorkflowOutcome> {
  return invoke<WorkflowOutcome>("workflow_pixelize", { id, params });
}

export function workflowTween(id: string, params: TweenParams): Promise<WorkflowOutcome> {
  return invoke<WorkflowOutcome>("workflow_tween", { id, params });
}

export function workflowVideoFrames(
  id: string,
  params: VideoFramesParams,
): Promise<WorkflowOutcome> {
  return invoke<WorkflowOutcome>("workflow_video_frames", { id, params });
}

export function videoBrief(
  id: string,
  params: VideoBriefParams,
): Promise<VideoBrief> {
  return invoke<VideoBrief>("video_brief", { id, params });
}

export function listenAgentEvents(handler: (event: AgentEvent) => void): Promise<UnlistenFn> {
  return listen<AgentEvent>(AGENT_EVENT_CHANNEL, (event) => handler(event.payload));
}

// ---------- 工作台编辑器与审批 ----------

/** 对挂起的工具调用给出决定。call_id 对不上（过期）Rust 会直接报错。 */
export function resolveApproval(
  id: string,
  callId: string,
  decision: ApprovalDecision,
): Promise<void> {
  return invoke("agent_resolve_approval", { id, callId, decision });
}

/** 结构与帧操作（建帧、复制帧、挪帧……），返回新 revision。 */
export function applyEditorOps(id: string, ops: EditorOperation[]): Promise<number> {
  return invoke<number>("editor_apply_ops", { id, ops });
}

/** 落一笔：抬笔时整笔发送，一笔一色。返回新 revision。 */
export function paintStroke(id: string, stroke: StrokeRequest): Promise<number> {
  return invoke<number>("editor_paint_stroke", { id, stroke });
}

/** 油漆桶：color 为 null 表示把整片区域浸回透明。返回新 revision。 */
export function fillCells(
  id: string,
  layer: string,
  frame: string,
  x: number,
  y: number,
  color: InkColor,
): Promise<number> {
  // 落点与颜色收成一个嵌套对象：Rust 侧对应 FillRequest，字段名逐字 snake_case。
  return invoke<number>("editor_fill", { id, fill: { layer, frame, x, y, color } });
}
