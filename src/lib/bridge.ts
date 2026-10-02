// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// Tauri 桥接层：所有 invoke / event 都收口在这里，UI 只面对类型。
// 参数名与 Rust 命令签名逐字对齐（Rust 侧是 snake_case，Tauri 会做 camelCase -> snake_case 转换）。

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import { subscribeLocal } from "./local-bus";

import type {
  ActiveContext,
  AgentEvent,
  AgentEventEnvelope,
  Attachment,
  ApprovalDecision,
  BatchEvent,
  BatchKind,
  BatchRecipe,
  BatchRecipeEntry,
  BatchScan,
  EditorOperation,
  InkColor,
  ImageSupport,
  LoopLimits,
  Message,
  McpServerConfig,
  McpServersView,
  ModelConfig,
  ModelsView,
  ModelRole,
 PermissionMode,
  Protocol,
  PixelDocument,
  RecipeImportReport,
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

export const BATCH_EVENT_CHANNEL = "batch-event";

/** 模型清单一览：Rust 侧 models.json 里存了什么这里就回什么，密钥只留在 Rust。 */
export function listModels(): Promise<ModelsView> {
  return invoke<ModelsView>("agent_list_models");
}

/** 新建或改一条模型定义：同 id 即覆盖更新，Rust 落盘之后才算数。 */
export function upsertModel(config: ModelConfig): Promise<ModelsView> {
  return invoke<ModelsView>("model_upsert", { config });
}

/** 删一条模型定义；删的是当前激活的那个，Rust 会把激活位让给下一条或留空。 */
export function removeModel(id: string): Promise<ModelsView> {
  return invoke<ModelsView>("model_remove", { id });
}

/** 切换全局激活模型：新建会话和顶栏「未绑定」时的回退都跟着它走。 */
export function setActiveModel(id: string): Promise<ModelsView> {
  return invoke<ModelsView>("model_set_active", { id });
}

/** 运行护栏当前值：续写、重试、纯思考各自封顶。 */
export function loopLimits(): Promise<LoopLimits> {
  return invoke<LoopLimits>("agent_loop_limits");
}

/** 存运行护栏。Rust 会把它夹到合理区间再落盘，所以回值才是真正生效的那份。 */
export function setLoopLimits(limits: LoopLimits): Promise<LoopLimits> {
  return invoke<LoopLimits>("agent_set_loop_limits", { limits });
}

/** MCP 总开关。false 时所有会话都看不见用户自配的外部工具。 */
export function mcpEnabled(): Promise<boolean> {
  return invoke<boolean>("agent_mcp_enabled");
}

/** 开/关 MCP。Rust 当场作用到活着的会话并落盘，回值是真正生效的状态。 */
export function setMcpEnabled(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("agent_set_mcp_enabled", { enabled });
}

/** 拉 provider 的模型清单。id 只为「沿用本机已存密钥」而传，不参与请求本身。 */
export function fetchModelList(params: {
  id?: string | null;
  baseUrl: string;
  apiKey: string;
  protocol: Protocol;
}): Promise<string[]> {
  return invoke<string[]>("model_fetch_models", {
    id: params.id ?? undefined,
    baseUrl: params.baseUrl,
    apiKey: params.apiKey,
    protocol: params.protocol,
  });
}

/** MCP 服务器一览：含连通状态，连不上的服务器其工具在各会话里直接缺席。 */
export function listMcpServers(): Promise<McpServersView> {
  return invoke<McpServersView>("mcp_list");
}

/**
 * 探测这个模型能不能出图。key 留空时后端沿用已存密钥，和「获取」一个口径。
 * 只回结论，不改任何设置：能力由用户声明，探测只是份建议。
 */
export function probeImageCapability(params: {
  id?: string | null;
  baseUrl: string;
  apiKey: string;
  protocol: Protocol;
  model: string;
}): Promise<ImageSupport> {
  return invoke<ImageSupport>("model_probe_image", {
    id: params.id ?? undefined,
    baseUrl: params.baseUrl,
    apiKey: params.apiKey,
    protocol: params.protocol,
    model: params.model,
  });
}

/** 登记一台外部工具服务器。登记只存配置，连不连是下一步的事。 */
export function upsertMcpServer(config: McpServerConfig): Promise<McpServersView> {
  return invoke<McpServersView>("mcp_upsert", { config });
}

/** 摘一台服务器：先断开再删，不留半开的连接。 */
export function removeMcpServer(name: string): Promise<McpServersView> {
  return invoke<McpServersView>("mcp_remove", { name });
}

/** 连上并拉它的工具清单；连失败不回滚配置，改好随时能再连。 */
export function connectMcpServer(name: string): Promise<McpServersView> {
  return invoke<McpServersView>("mcp_connect", { name });
}

/** 断开但保留配置：它的工具从各会话消失，设置还留在簿子里。 */
export function disconnectMcpServer(name: string): Promise<McpServersView> {
  return invoke<McpServersView>("mcp_disconnect", { name });
}

export function createSession(
  document?: PixelDocument,
  title?: string | null,
): Promise<SessionInfo> {
  // title 可空：空名由 Rust 用默认编号 s1、s2……，这里不该替他编。
  return invoke<SessionInfo>("session_create", { document: document ?? null, title: title ?? null });
}

/** 会话一览：标题、画布宽高、绑定模型、revision 全在里面，侧边栏只读这一份。 */
export function listSessions(): Promise<SessionInfo[]> {
  return invoke<SessionInfo[]>("session_list");
}

/** 删整条会话：文档、聊天记录、事件订阅一起没，Rust 侧是原子的。 */
export function dropSession(id: string): Promise<void> {
  return invoke("session_drop", { id });
}

/** 改侧边栏显示名。空白名 Rust 当取消。 */
export function renameSession(id: string, title: string): Promise<SessionInfo> {
  return invoke<SessionInfo>("session_rename", { id, title });
}

/** 拖动排序：ids 是排好后的完整次序，Rust 按新次序整批改写排序位。 */
export function reorderSessions(ids: string[]): Promise<void> {
  return invoke("session_reorder", { ids });
}

/** 给会话绑主模型：顶栏模型下拉的落点，换绑立即影响下一轮。 */
export function bindModel(id: string, modelId: string): Promise<SessionInfo> {
  return invoke<SessionInfo>("session_bind_model", { id, modelId });
}

/** 给生图 / 识图 / 读视频之一另绑一个模型；回来带着最新的分工概览。 */
export function bindSessionRole(
  id: string,
  role: ModelRole,
  modelId: string,
): Promise<SessionInfo> {
  return invoke<SessionInfo>("session_bind_role", { id, role, modelId });
}

/** 取消某个角色的单独绑定，让它回落去用会话主模型。 */
export function clearSessionRole(id: string, role: ModelRole): Promise<SessionInfo> {
  return invoke<SessionInfo>("session_clear_role", { id, role });
}

/**
 * 告诉 Rust 这条会话当前停在哪个帧、哪个图层。画笔、填充、导出都以它为准，
 * 所以切帧是一次实打实的 IPC——不是纯本地状态。
 */
export function setActive(id: string, active: ActiveContext): Promise<void> {
  return invoke("agent_set_active", { id, active });
}

/** 设这条会话的审批档位：Auto / Chat / Ask 决定每次工具调用问不问。 */
export function setPermission(id: string, permission: PermissionMode): Promise<void> {
  return invoke("agent_set_permission", { id, permission });
}

export function sendMessage(
  id: string,
  text: string,
  attachments: Attachment[],
  modelId?: string,
  style?: string | null,
  presets?: readonly string[],
): Promise<void> {
  return invoke("agent_send_message", {
    id,
    text,
    attachments,
    modelId: modelId ?? null,
    style: style ?? null,
    // 复数。收尾规矩可以叠几条一起上路，Rust 那头按上限收口（presets::MAX_STACKED）；
    // 名字写成单数的话整条 IPC 对不上，用户选的规矩一条都到不了模型那儿。
    presets: presets ? [...presets] : [],
  });
}

/** 打断当前这一轮。主循环跑在 Rust，停不停由它说了算，前端只是递个话。 */
export function interrupt(id: string): Promise<void> {
  return invoke("agent_interrupt", { id });
}

/**
 * 前端把整份文档推回 Rust：画笔、填充、撤销这类编辑器动作的最终落点。
 * 整份覆盖而不是逐条增量——文本网格不大，换来 Rust 永远持有权威状态，
 * 不用跟前端对账。回来的 revision 用来标记「之后的事件都比我新」。
 */
export function syncDocument(id: string, document: PixelDocument): Promise<{ revision: number }> {
  return invoke<{ revision: number }>("agent_sync_document", { id, document });
}

/** 拉一次权威文档 + revision：换会话、打开 .aip 之后前端靠它对齐，不靠事件回放。 */
export function documentSnapshot(id: string): Promise<{
  id: string;
  revision: number;
  document: PixelDocument;
  active?: ActiveContext | null;
}> {
  return invoke("agent_document", { id });
}

/** 恢复这条会话的聊天记录：消息由 Rust 跟着会话持久化，重开接着看。 */
export function agentHistory(id: string): Promise<Message[]> {
  return invoke<Message[]>("agent_history", { id });
}

/**
 * 当前帧（可指定）的 PNG data URL：缩略图与预览走这里。
 * 回 url 而不是字节，是为了让 <img> 直接吃，省一次前端解码。
 */
export function pngUrl(id: string, frame?: number): Promise<string> {
  return invoke<string>("document_png_url", { id, frame: frame ?? null });
}

/** 文档原文（v2 JSON）：「看 .aip」面板与调试用的就是这一口。 */
export function aipText(id: string): Promise<string> {
  return invoke<string>("aip_text", { id });
}

export function aipSave(id: string, path: string): Promise<string> {
  return invoke<string>("aip_save", { id, path });
}

/** 读一个 .aip 打开成会话文档；解析失败 Rust 直接报错，不留半份文档。 */
export function aipLoad(path: string): Promise<PixelDocument> {
  return invoke<PixelDocument>("aip_load", { path });
}

export function readImageContext(path: string): Promise<Attachment> {
  return invoke<Attachment>("read_image_context", { path });
}

/** 导出格式：Rust 侧 document_export 认领的这几种；aseprite 是 ase 的别名。 */
export type ExportFormat = "gif" | "sheet" | "ase" | "aseprite" | "frame" | "strip";

/** 导出：gif 是无限循环动画，sheet 是 PNG spritesheet（columns 0 = 排成一行）。 */
export function documentExport(
  id: string,
  format: ExportFormat,
  path: string,
  options: { columns?: number; frame?: number } = {}
): Promise<void> {
  return invoke<void>("document_export", {
    id,
    format,
    path,
    columns: options.columns ?? 0,
    frame: options.frame ?? null,
  });
}

// ---------- 工作流 ----------
// params 是嵌套结构体，Tauri 的 camelCase 转换只作用于顶层参数名，
// 所以 params 里的键必须逐字写 Rust 的 snake_case 字段名（from_frame / duration_ms ...）。

/** 问 Rust 这条会话现在有哪几条工作流、各自缺什么能力（readiness）。 */
export function workflowCatalog(id: string): Promise<WorkflowEntry[]> {
  return invoke<WorkflowEntry[]>("workflow_catalog", { id });
}

export function promptRefine(
  id: string,
  idea: string,
  width: number,
  height: number,
  target: RefineTarget,
  // 输入区那两个下拉点上的值。和主循环送的是同一份：用户在输入区选了画风与
  // 收尾预设，微调出来的九行提示词就得照同一套规矩写，不能各问各的。
  style?: string | null,
  presets?: string[] | null,
): Promise<RefinedPrompt> {
  return invoke<RefinedPrompt>("prompt_refine", {
    id,
    idea,
    width,
    height,
    target,
    style: style ?? null,
    presets: presets ?? [],
  });
}

/** 探一段视频的帧率与时长：抽帧之前先让用户估算自己会得到多少帧。 */
export function videoProbe(path: string): Promise<VideoProbeResult> {
  return invoke<VideoProbeResult>("video_probe", { path });
}

/** 视觉模型读一张参考图，产出结构化简报；识图用的是输入区选的示例图。 */
export function visionBrief(id: string, path: string): Promise<VisionBrief> {
  return invoke<VisionBrief>("vision_brief", { id, path });
}

/** 生图并量化落画布：模型出位图，Rust 侧 pixelize 之后写进文档。 */
export function workflowImageGen(
  id: string,
  params: ImageGenParams,
): Promise<WorkflowOutcome> {
  return invoke<WorkflowOutcome>("workflow_image_gen", { id, params });
}

/** 位图直接量化：不调模型的一次性栅格化，参数见 PixelizeParams。 */
export function workflowPixelize(
  id: string,
  params: PixelizeParams,
): Promise<WorkflowOutcome> {
  return invoke<WorkflowOutcome>("workflow_pixelize", { id, params });
}

/** 两帧之间插值插入中间帧：count 是要插入几帧，不是总帧数。 */
export function workflowTween(id: string, params: TweenParams): Promise<WorkflowOutcome> {
  return invoke<WorkflowOutcome>("workflow_tween", { id, params });
}

/** 抽帧并逐帧量化：count 为 0 表示全部抽出，上限由 Rust 侧夹住。 */
export function workflowVideoFrames(
  id: string,
  params: VideoFramesParams,
): Promise<WorkflowOutcome> {
  return invoke<WorkflowOutcome>("workflow_video_frames", { id, params });
}

/** 读视频模型产出运动简报：产物是文本，交给生图或聊天接手画。 */
export function videoBrief(
  id: string,
  params: VideoBriefParams,
): Promise<VideoBrief> {
  return invoke<VideoBrief>("video_brief", { id, params });
}

/** 订阅 agent 主循环与工作流的事件。handler 第二个参数是这条事件的会话归属——
 * 通道是全局的，别的会话的事件也会到这儿，收不收由 store 按 activeId 定夺。 */
export function listenAgentEvents(
  handler: (event: AgentEvent, sessionId: string) => void,
): Promise<UnlistenFn> {
  return listenSafe<AgentEventEnvelope>(AGENT_EVENT_CHANNEL, (payload) => {
    // 解不开说明通道上躺着一份没有 session_id 的旧载荷：当作别人的事丢掉，
    // 好过凭猜把它算到当前会话头上。
    if (!payload || typeof payload.session_id !== "string" || !payload.event) return;
    handler(payload.event, payload.session_id);
  });
}

/** Tauri 的事件桥通不通：`listen` 靠 `__TAURI_INTERNALS__.transformCallback` 注册回调，
 * 它不在（旧运行时、或者谁先注入了半成品 internals）这次调用就会抛。
 * 抛出来的代价很大：boot 里正 await 着它，后面读模型、建会话全都不做了。 */
function hasEventBridge(): boolean {
  if (typeof window === "undefined") return false;
  const internals = (window as unknown as { __TAURI_INTERNALS__?: { transformCallback?: unknown } })
    .__TAURI_INTERNALS__;
  return typeof internals?.transformCallback === "function";
}

/** 订阅一条 Rust 广播的通道。事件桥不在就退到页面内总线，绝不把异常抛给调用方。 */
function listenSafe<T>(channel: string, handler: (event: T) => void): Promise<UnlistenFn> {
  if (hasEventBridge()) {
    return listen<T>(channel, (event) => handler(event.payload));
  }
  return Promise.resolve(subscribeLocal(channel, handler as (payload: unknown) => void));
}

/** 扫一个文件夹，数清有几份对口素材。只读。 */
export function scanBatch(inputDir: string, kind: BatchKind): Promise<BatchScan> {
  return invoke<BatchScan>("batch_scan", { inputDir, kind });
}

/** 起一次批量。命令立即返回，过程走 batch-event。 */
export function runBatch(recipe: BatchRecipe): Promise<void> {
  return invoke("batch_run", { recipe });
}

/** 配方簿：跑熟的 recipe 存哪儿、取哪儿。读失败 Rust 给空簿，不当错误。 */
export function listBatchRecipes(): Promise<BatchRecipeEntry[]> {
  return invoke<BatchRecipeEntry[]>("batch_recipes_list");
}

/** 存一条配方；同名覆盖。名字不合法由 Rust 拒掉，带着原因回来。 */
export function saveBatchRecipe(name: string, recipe: BatchRecipe): Promise<void> {
  return invoke("batch_recipe_save", { name, recipe });
}

/** 删一条配方；删不存在的名字 Rust 当成功。 */
export function deleteBatchRecipe(name: string): Promise<void> {
  return invoke("batch_recipe_delete", { name });
}

/** 把点名几条配方写成 `.aipr`；返回真正落盘的那个路径（可能补了后缀）。 */
export function exportBatchRecipes(entries: BatchRecipeEntry[], path: string): Promise<string> {
  return invoke<string>("batch_recipe_export", { entries, path });
}

/** 从 `.aipr` 读配方并进来；回执带合并后的整本簿子，前端不必再问一次。 */
export function importBatchRecipes(path: string): Promise<RecipeImportReport> {
  return invoke<RecipeImportReport>("batch_recipe_import", { path });
}

export function listenBatchEvents(handler: (event: BatchEvent) => void): Promise<UnlistenFn> {
  return listenSafe<BatchEvent>(BATCH_EVENT_CHANNEL, handler);
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

/** 改画布宽高：左上角锚定，原有像素跟着搬家。返回新 revision。 */
export function resizeCanvas(id: string, width: number, height: number): Promise<number> {
  return invoke<number>("editor_resize_canvas", { id, width, height });
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
