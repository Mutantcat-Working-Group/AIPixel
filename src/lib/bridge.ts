// Tauri 桥接层：所有 invoke / event 都收口在这里，UI 只面对类型。
// 参数名与 Rust 命令签名逐字对齐（Rust 侧是 snake_case，Tauri 会做 camelCase -> snake_case 转换）。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  ActiveContext,
  AgentEvent,
  Attachment,
  Message,
  ModelConfig,
  ModelsView,
  PermissionMode,
  PixelDocument,
  SessionInfo,
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

export function listenAgentEvents(handler: (event: AgentEvent) => void): Promise<UnlistenFn> {
  return listen<AgentEvent>(AGENT_EVENT_CHANNEL, (event) => handler(event.payload));
}
