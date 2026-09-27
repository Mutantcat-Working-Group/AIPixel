// 应用状态：会话表、模型配置、对话条目、权威文档与画布预览。
// Rust 主循环只通过 agent-event 说话；所有副作用都收敛成 bridge 调用。

import { create } from "zustand";

import { renderUiText, translate, type Lang, type TVARS, type TKey } from "./i18n";
import * as bridge from "./bridge";
import {
  emptyTranscript,
  historyToTranscript,
  pushNotice,
  pushUserMessage,
  reduceEvent,
  sealTranscript,
} from "./transcript";
import type {
  ActiveContext,
  Attachment,
  AgentEvent,
  ApprovalDecision,
  EditorOperation,
  DockKind,
  DockDraft,
 ImageGenParams,
  McpServerConfig,
  McpServersView,
 ModelConfig,
 ModelsView,
  PixelizeParams,
  PendingAttachment,
  PendingApproval,
  PermissionMode,
  PixelDocument,
  PixelizeOptions,
  RefinedPrompt,
  RefineTarget,
  SessionInfo,
  TweenParams,
  VideoBrief,
  VideoFramesParams,
  VideoProbeResult,
  VisionBrief,
  WorkflowEntry,
  WorkflowOutcome,
  TranscriptEntry,
  Usage,
  StrokeCell,
} from "./types";

export interface DocumentSnapshot {
  document: PixelDocument | null;
  revision: number;
  pngUrl: string | null;
  /** 已经渲染过 PNG 的 revision，用来丢弃过期的异步结果。 */
  pngRevision: number;
  frameIndex: number;
}

/** 工作流面板的状态。目录跟着会话走：会话换绑模型，能力就变。 */
export interface WorkflowState {
  workflows: WorkflowEntry[];
  /** 目录没取回来之前不让点 Run，否则用户以为工具坏了。 */
  catalogReady: boolean;
  kind: DockKind;
  workflowBusy: boolean;
  /** 最近一次跑完的回执；同时投一条 notice 进对话流。 */
  outcome: WorkflowOutcome | null;
  outcomeError: string | null;
  /** 微调出来的提示词，要留着让用户逐行改。 */
  refined: RefinedPrompt | null;
  refineTarget: RefineTarget;
  vision: VisionBrief | null;
  probe: VideoProbeResult | null;
  /** 读视频模型给的运动简报。不落文档，是一次性的中间产物。 */
  videoBrief: VideoBrief | null;
  /** 微调结果的可编辑副本；用户改的就是这段，refined.prompt 留作原文对照。 */
  refinedDraft: string;
  dockDraft: DockDraft;
  /** 交给聊天输入框的文本。带 nonce，同一句话发两次也能触发。 */
  composeRequest: { text: string; nonce: number } | null;
}

/** 会动文档的四条工作流入参。image_gen / frame_tween / video_frames 走 runWorkflow。 */
export type WorkflowParams = TweenParams | PixelizeParams | ImageGenParams | VideoFramesParams;

interface StoreState extends DocumentSnapshot, WorkflowState {
  booted: boolean;
  models: ModelsView;
  /** 用户自配的 MCP 服务器；连上的才带工具清单。 */
  mcpServers: McpServersView;
  sessions: SessionInfo[];
  activeId: string | null;
  entries: TranscriptEntry[];
  running: boolean;
  usage: Usage | null;
  attachments: PendingAttachment[];
  permission: PermissionMode;
  active: ActiveContext;
  busy: boolean;
  /** 主循环正停在一条工具调用上等决定；Ask / Chat 模式下才有。 */
  pendingApproval: PendingApproval | null;
  /** 结构操作（建帧、复制帧、挪帧）后想选到哪一帧，等新文档到达时结算。 */
  pendingFrameIndex: number | null;
  /** 编辑器自己的改动快照，最新一版在栈顶。模型改动不进栈。 */
  undoStack: PixelDocument[];
  notice: { text: string; isError: boolean } | null;
  settingsOpen: boolean;
  mcpOpen: boolean;
  /** 连接/保存进行中：期间按钮全灭，防止连点把服务器打爆。 */
  mcpBusy: boolean;
  /** 界面语言。默认中文，用户可在设置里改成英语；只影响这一层，不回灌 Rust。 */
  lang: Lang;
}

export interface StoreActions {
  boot: () => Promise<void>;
  setLang: (lang: Lang) => void;
  selectSession: (id: string) => Promise<void>;
  createSession: (width?: number, height?: number) => Promise<void>;
  removeSession: (id: string) => Promise<void>;
  bindSessionModel: (modelId: string) => Promise<void>;
  setPermissionMode: (mode: PermissionMode) => Promise<void>;
  send: (text: string) => Promise<void>;
  interrupt: () => Promise<void>;
  upsertModel: (config: ModelConfig) => Promise<void>;
  removeModel: (id: string) => Promise<void>;
  activateModel: (id: string) => Promise<void>;
  refreshMcp: () => Promise<void>;
  openMcp: () => void;
  closeMcp: () => void;
  upsertMcpServer: (config: McpServerConfig) => Promise<void>;
  removeMcpServer: (name: string) => Promise<void>;
  connectMcpServer: (name: string) => Promise<void>;
  disconnectMcpServer: (name: string) => Promise<void>;
  attachReferenceImages: (paths: string[]) => Promise<void>;
  attachSnapshot: () => Promise<void>;
  removeAttachment: (key: string) => void;
  setActiveLayer: (layerId: string) => void;
  setActiveFrame: (index: number) => void;
  setActiveColor: (color: string | null) => void;
  refreshDocument: () => Promise<void>;
  refreshPng: () => Promise<void>;
  refreshSessions: () => Promise<void>;
  openAip: (path: string) => Promise<void>;
  saveAip: (path: string) => Promise<void>;
  readAipText: () => Promise<string | null>;
  clearNotice: () => void;
  openSettings: () => void;
  closeSettings: () => void;
  refreshWorkflows: () => Promise<void>;
  setKind: (kind: DockKind) => void;
  setRefineTarget: (target: RefineTarget) => void;
  runWorkflow: (
    kind: DockKind,
    params: WorkflowParams,
  ) => Promise<WorkflowOutcome | null>;
  runPixelize: (params: PixelizeParams) => Promise<WorkflowOutcome | null>;
  refinePrompt: (idea: string) => Promise<void>;
  briefReference: (path: string) => Promise<void>;
  briefVideo: (path: string, count: number) => Promise<void>;
  probeVideo: (path: string) => Promise<void>;
  clearWorkflowResult: () => void;
  patchDraft: (patch: Partial<DockDraft>) => void;
  setRefinedPrompt: (text: string) => void;
  /** 坞里的本机失败（读文件、解析）没法包成 Tauri 错误，从这里进回执和对话流。 */
  surfaceError: (message: string) => void;
  /** 把一段文本塞进聊天输入框；不让用户手动复制粘贴。 */
  requestCompose: (text: string) => void;
  /** 「用这条提示词生图」：写进生图面板并切过去。 */
  usePromptInGen: (prompt: string) => void;
  /** 对挂起的工具调用给出决定；Approve all 只降级本 turn。 */
  resolveApproval: (decision: ApprovalDecision) => Promise<void>;
  /** 落一笔：抬笔时整笔发送，颜色取当前调色板选择（null = 擦除）。 */
  paintStroke: (cells: StrokeCell[]) => Promise<void>;
  /** 油漆桶点一下；颜色取当前调色板选择（null = 浸回透明）。 */
  fillCell: (x: number, y: number) => Promise<void>;
  /** 结构与帧操作。frameHint 是新文档到达后要选中的帧序。 */
  runEditorOps: (ops: EditorOperation[], frameHint?: number) => Promise<number | null>;
  addFrame: () => Promise<void>;
  duplicateFrame: () => Promise<void>;
  deleteFrame: () => Promise<void>;
  moveFrame: (delta: number) => Promise<void>;
  /** 回退一步编辑器改动：撤销栈见底就什么都不做。 */
  undoEdit: () => Promise<void>;
}

const EMPTY_MODELS: ModelsView = { active_id: "", entries: [] };
const EMPTY_MCP: McpServersView = { entries: [] };

const LANG_KEY = "aipixel.lang";

/** 启动时读本地记忆；没有记忆就中文。存储不可用（隐身模式 / node 测试）时同样中文。 */
function storedLang(): Lang {
  try {
    const raw = globalThis.localStorage?.getItem(LANG_KEY);
    return raw === "en" || raw === "zh" ? raw : "zh";
  } catch {
    return "zh";
  }
}

/** 与 Rust `PixelizeOptions::default()` 一致；改了 Rust 要同步这里。 */
const DEFAULT_OPTIONS: PixelizeOptions = {
  max_colors: 32,
  snap_tolerance: 12,
  expand_palette: true,
  dither: false,
  alpha_threshold: 128,
  fit: "contain",
};

const INITIAL_DOCK_DRAFT: DockDraft = {
  prompt: "",
  size: "1024x1024",
  genPath: null,
  genSource: "none",
  genFrame: "",
  spot: "new_frame",
  durationMs: 83,
  options: DEFAULT_OPTIONS,
  idea: "",
  visionPath: null,
  videoPath: null,
  videoCount: 4,
  briefCount: 8,
  quantizePath: null,
  // 帧 id 由文档决定，loadDocument 之后由坞按真实帧校正。
  tweenFrom: "",
  tweenTo: "",
  tweenCount: 4,
  tweenMode: "migrate",
  tweenOrder: "scan",
  tweenEase: true,
};

function dataUrl(mediaType: string, dataBase64: string): string {
  return `data:${mediaType};base64,${dataBase64}`;
}

function stripDataUrl(url: string): { mediaType: string; data: string } {
  const match = /^data:([^;]+);base64,(.*)$/s.exec(url);
  if (!match) return { mediaType: "image/png", data: "" };
  return { mediaType: match[1], data: match[2] };
}

function baseName(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

/** kind -> 命令。vision_brief / prompt_refine 不落文档，走各自的动作。 */
function dispatchWorkflow(
  id: string,
  kind: DockKind,
  params: WorkflowParams,
): Promise<WorkflowOutcome> {
  switch (kind) {
    case "image_gen":
      return bridge.workflowImageGen(id, params as ImageGenParams);
    case "frame_tween":
      return bridge.workflowTween(id, params as TweenParams);
    case "video_frames":
      return bridge.workflowVideoFrames(id, params as VideoFramesParams);
    default:
      return Promise.reject(new Error(`workflow ${kind} has no direct runner`));
  }
}

/** Tauri 的 invoke 拒绝时给的是字符串而不是 Error，统一成一句能看的话。 */
function workflowError(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

let unlisten: (() => void) | null = null;
let booting: Promise<void> | null = null;
let snapshotSeq = 0;

/** 撤销栈上限：再老的笔触就别指望了，省得内存和「撤销到天边」一起失控。 */
const UNDO_LIMIT = 40;
/**
 * 下一次 document_updated 若是编辑器自己触发的，就把改前的文档压进撤销栈。
 * 为什么不让模型改动也进栈：一轮 agent 跑下来事件几十条，会把这些笔触挤没。
 */
let undoCapture = false;

function pushUndo(stack: PixelDocument[], doc: PixelDocument): PixelDocument[] {
  const next = [...stack, doc];
  return next.length > UNDO_LIMIT ? next.slice(next.length - UNDO_LIMIT) : next;
}

export const useStore = create<StoreState & StoreActions>()((setState, getState) => {
  /** 键控失败：措辞跟着当前界面语言走，Rust 的原文当 {error} 追在后面。 */
  function failKey(key: TKey, vars?: TVARS) {
    setState({ notice: { text: translate(getState().lang, key, vars), isError: true }, running: false });
  }

  /** 一段本机成功提示（载入、保存）。不是错误，所以不进 fail 那条路。 */
  function noteKey(key: TKey, vars?: TVARS) {
    setState({ notice: { text: translate(getState().lang, key, vars), isError: false } });
  }

  /** document_updated 的统一落点：文档、撤销栈、帧选择意图一起结算。 */
  function applyDocument(
    document: PixelDocument,
    revision: number,
    frameHint: number | null,
  ) {
    const state = getState();
    const stale = revision < state.pngRevision;
    const next: Partial<StoreState> = {
      document,
      revision,
      // 撤销栈只吃编辑器自己那次改动前的快照，见 undoCapture 的说明。
      undoStack:
        undoCapture && state.document
          ? pushUndo(state.undoStack, state.document)
          : state.undoStack,
      pendingFrameIndex: null,
      pngRevision: stale ? state.pngRevision : revision,
    };
    if (frameHint !== null) {
      // 结构操作后帧表已变：按预期位置选帧，越界夹到末帧。
      const index = Math.max(0, Math.min(frameHint, document.frames.length - 1));
      const frame = document.frames[index];
      next.frameIndex = index;
      if (frame) next.active = { ...state.active, frame: frame.id };
    }
    undoCapture = false;
    setState(next);
    if (frameHint !== null) {
      const id = getState().activeId;
      const active = getState().active;
      if (id) void bridge.setActive(id, active);
    }
    void getState().refreshPng();
  }

  /** agent-event 路由：文档事件驱动画布，其余折叠进对话条目。 */
  async function ensureListener() {
    if (unlisten) return;
    unlisten = await bridge.listenAgentEvents((raw: AgentEvent) => {
      const state = getState();
      if (raw.kind === "approval_request") {
        setState({
          pendingApproval: { callId: raw.call_id, name: raw.name, input: raw.input },
        });
        return;
      }
      if (raw.kind === "document_updated") {
        applyDocument(raw.document, raw.revision, state.pendingFrameIndex);
        return;
      }
      if (raw.kind === "completed" || raw.kind === "error" || raw.kind === "interrupted") {
        setState({
          entries: sealTranscript(reduceEvent(state.entries, raw, state.lang)),
          running: false,
          // 一轮收尾，挂着没批的调用跟着作废——别让下一轮还看见这张票。
          pendingApproval: null,
        });
        void getState().refreshSessions();
        return;
      }
      if (raw.kind === "usage") {
        setState({ usage: { input: raw.input_tokens, output: raw.output_tokens } });
        return;
      }
      setState({ entries: reduceEvent(state.entries, raw, state.lang) });
    });
  }

  async function loadDocument(id: string) {
    // 换会话就把工作流面板的中间产物倒掉：上一条会话的提示词不属于这一条。
    setState({
      refined: null,
      vision: null,
      probe: null,
      videoBrief: null,
      outcome: null,
      outcomeError: null,
      catalogReady: false,
      refinedDraft: "",
      // 撤销栈是当前会话的笔迹，换会话不跟着走；挂着的审批同理。
      undoStack: [],
      pendingApproval: null,
      pendingFrameIndex: null,
    });
    try {
      const messages = await bridge.agentHistory(id);
      setState({ entries: historyToTranscript(messages, getState().lang) });
    } catch {
      // 历史读不到就从空对话开始，不阻塞文档加载
    }
    const snapshot = await bridge.documentSnapshot(id);
    setState({
      document: snapshot.document,
      revision: snapshot.revision,
      pngRevision: snapshot.revision,
      frameIndex: 0,
    });
    const firstLayer = snapshot.document?.layers[0]?.id ?? "L0";
    const firstFrame = snapshot.document?.frames[0]?.id ?? "F0";
    const active: ActiveContext = {
      layer: firstLayer,
      frame: firstFrame,
      color: getState().active.color ?? null,
    };
    setState({ active });
    await bridge.setActive(id, active);
    await getState().refreshPng();
    await getState().refreshWorkflows();
  }

  return {
    booted: false,
    models: EMPTY_MODELS,
    sessions: [],
    activeId: null,
    document: null,
    revision: 0,
    pngUrl: null,
    pngRevision: -1,
    frameIndex: 0,
    entries: emptyTranscript(),
    running: false,
    usage: null,
    attachments: [],
    permission: "auto",
    active: { layer: "L0", frame: "F0", color: null },
    busy: false,
    pendingApproval: null,
    pendingFrameIndex: null,
    undoStack: [],
    notice: null,
    settingsOpen: false,
    mcpServers: EMPTY_MCP,
    mcpOpen: false,
    mcpBusy: false,
    workflows: [],
    catalogReady: false,
    kind: "image_gen",
    workflowBusy: false,
    outcome: null,
    outcomeError: null,
    refined: null,
    refineTarget: "image_gen",
    vision: null,
    probe: null,
    videoBrief: null,
    refinedDraft: "",
    dockDraft: INITIAL_DOCK_DRAFT,
    composeRequest: null,
    lang: storedLang(),

    boot: async () => {
      if (booting) return booting;
      booting = (async () => {
        await ensureListener();
        let models: ModelsView;
        try {
          models = await bridge.listModels();
        } catch (error) {
          failKey("store.read_models_failed", { error: String(error) });
          models = EMPTY_MODELS;
        }
        setState({ models });
        let mcpServers: McpServersView;
        try {
          mcpServers = await bridge.listMcpServers();
        } catch (error) {
          failKey("store.read_mcp_failed", { error: String(error) });
          mcpServers = EMPTY_MCP;
        }
        setState({ mcpServers });
        let sessions: SessionInfo[] = [];
        try {
          sessions = await bridge.listSessions();
        } catch (error) {
          failKey("store.read_sessions_failed", { error: String(error) });
        }
        if (sessions.length === 0) {
          const created = await bridge.createSession();
          sessions = [created];
        }
        sessions.sort((a, b) => a.id.localeCompare(b.id));
        const first = sessions[sessions.length - 1];
        if (first) {
          setState({ sessions, activeId: first.id });
          await loadDocument(first.id);
        }
        setState({ booted: true });
      })();
      return booting;
    },

    selectSession: async (id) => {
      if (getState().running) await getState().interrupt();
      setState({
        activeId: id,
        entries: emptyTranscript(),
        usage: null,
        attachments: [],
        frameIndex: 0,
      });
      await loadDocument(id);
      await getState().refreshSessions();
    },

    createSession: async (width, height) => {
      const document = width && height ? blankDocument(width, height) : undefined;
      const info = await bridge.createSession(document);
      setState({
        sessions: [...getState().sessions, info].sort((a, b) => a.id.localeCompare(b.id)),
        activeId: info.id,
        entries: emptyTranscript(),
        usage: null,
        attachments: [],
        frameIndex: 0,
      });
      await loadDocument(info.id);
    },

    removeSession: async (id) => {
      await bridge.dropSession(id);
      const rest = getState().sessions.filter((s) => s.id !== id);
      setState({ sessions: rest });
      if (getState().activeId === id) {
        if (rest.length > 0) {
          await getState().selectSession(rest[rest.length - 1].id);
        } else {
          await getState().createSession();
        }
      }
    },

    bindSessionModel: async (modelId) => {
      const id = getState().activeId;
      if (!id) return;
      const info = await bridge.bindModel(id, modelId);
      setState({
        sessions: getState().sessions.map((s) => (s.id === id ? info : s)),
      });
      // 能力跟着会话绑的模型走，换绑之后哪些工作流跑得动就变了。
      await getState().refreshWorkflows();
    },

    setPermissionMode: async (mode) => {
      const id = getState().activeId;
      setState({ permission: mode });
      if (!id) return;
      await bridge.setPermission(id, mode);
    },

    send: async (text) => {
      const id = getState().activeId;
      if (!id) {
        failKey("store.no_session");
        return;
      }
      const trimmed = text.trim();
      const attachments = getState().attachments;
      if (!trimmed && attachments.length === 0) return;
      const payload: Attachment[] = attachments.map((a) => ({
        role: a.role,
        media_type: a.mediaType,
        data_base64: a.dataBase64,
      }));
      setState({
        entries: pushUserMessage(getState().entries, trimmed, attachments),
        attachments: [],
        running: true,
        notice: null,
      });
      try {
        await bridge.sendMessage(id, trimmed, payload);
      } catch (error) {
        failKey("store.send_failed", { error: String(error) });
      }
    },

    interrupt: async () => {
      const id = getState().activeId;
      if (!id) return;
      try {
        await bridge.interrupt(id);
      } catch (error) {
        failKey("store.interrupt_failed", { error: String(error) });
      }
    },

    upsertModel: async (config) => {
      const models = await bridge.upsertModel(config);
      setState({ models });
      // 改的是当前会话绑的那个模型，能力勾选就得重新反映到目录上。
      const bound = getState().sessions.find((s) => s.id === getState().activeId);
      if (!bound || bound.model_id === config.id) {
        await getState().refreshWorkflows();
      }
    },

    removeModel: async (id) => {
      const models = await bridge.removeModel(id);
      setState({ models });
    },

    activateModel: async (id) => {
      const models = await bridge.setActiveModel(id);
      setState({ models });
    },

    refreshMcp: async () => {
      try {
        setState({ mcpServers: await bridge.listMcpServers() });
      } catch (error) {
        failKey("store.read_mcp_failed", { error: String(error) });
      }
    },

    openMcp: () => setState({ mcpOpen: true }),

    closeMcp: () => setState({ mcpOpen: false }),

    upsertMcpServer: async (config) => {
      setState({ mcpBusy: true });
      try {
        setState({ mcpServers: await bridge.upsertMcpServer(config) });
      } catch (error) {
        failKey("store.save_mcp_failed", { error: String(error) });
      } finally {
        setState({ mcpBusy: false });
      }
    },

    removeMcpServer: async (name) => {
      setState({ mcpBusy: true });
      try {
        setState({ mcpServers: await bridge.removeMcpServer(name) });
      } catch (error) {
        failKey("store.remove_mcp_failed", { error: String(error) });
      } finally {
        setState({ mcpBusy: false });
      }
    },

    connectMcpServer: async (name) => {
      setState({ mcpBusy: true });
      try {
        setState({ mcpServers: await bridge.connectMcpServer(name) });
      } catch (error) {
        // 失败原因由 Rust 带回视图；这里只负责把视图刷出来让用户看见。
        setState({ mcpServers: await bridge.listMcpServers() });
        failKey("store.connect_mcp_failed", { error: String(error) });
      } finally {
        setState({ mcpBusy: false });
      }
    },

    disconnectMcpServer: async (name) => {
      setState({ mcpBusy: true });
      try {
        setState({ mcpServers: await bridge.disconnectMcpServer(name) });
      } catch (error) {
        failKey("store.disconnect_mcp_failed", { error: String(error) });
      } finally {
        setState({ mcpBusy: false });
      }
    },

    attachReferenceImages: async (paths) => {
      const pending: PendingAttachment[] = [];
      for (const path of paths) {
        try {
          const attachment = await bridge.readImageContext(path);
          pending.push({
            key: `${path}:${pending.length}`,
            role: attachment.role,
            name: baseName(path),
            mediaType: attachment.media_type,
            dataBase64: attachment.data_base64,
            previewUrl: dataUrl(attachment.media_type, attachment.data_base64),
          });
        } catch (error) {
          failKey("store.read_image_failed", { name: baseName(path), error: String(error) });
        }
      }
      if (pending.length > 0) setState({ attachments: [...getState().attachments, ...pending] });
    },

    attachSnapshot: async () => {
      const id = getState().activeId;
      if (!id) return;
      try {
        const url = await bridge.pngUrl(id);
        const { mediaType, data } = stripDataUrl(url);
        if (!data) {
          failKey("store.empty_snapshot");
          return;
        }
       snapshotSeq += 1;
       setState({
         attachments: [
           ...getState().attachments,
           {
              key: `snapshot:${getState().revision}:${snapshotSeq}`,
              role: "snapshot",
              name: translate(getState().lang, "store.snapshot_name"),
              mediaType,
              dataBase64: data,
              previewUrl: url,
            },
          ],
        });
      } catch (error) {
        failKey("store.snapshot_failed", { error: String(error) });
      }
    },

    removeAttachment: (key) => {
      setState({ attachments: getState().attachments.filter((a) => a.key !== key) });
    },

    setActiveLayer: (layerId) => {
      const id = getState().activeId;
      if (!id) return;
      const active = { ...getState().active, layer: layerId };
      setState({ active });
      void bridge.setActive(id, active);
    },

    setActiveFrame: (index) => {
      const id = getState().activeId;
      const doc = getState().document;
      if (!id || !doc) return;
      const frame = doc.frames[index];
      if (!frame) return;
      const active = { ...getState().active, frame: frame.id };
      setState({ active, frameIndex: index });
      void bridge.setActive(id, active);
      void getState().refreshPng();
    },

    setActiveColor: (color) => {
      const id = getState().activeId;
      if (!id) return;
      const active = { ...getState().active, color };
      setState({ active });
      void bridge.setActive(id, active);
    },

    refreshDocument: async () => {
      const id = getState().activeId;
      if (!id) return;
      await loadDocument(id);
      await getState().refreshSessions();
    },

    refreshPng: async () => {
      const id = getState().activeId;
      if (!id) return;
      const requested = getState().revision;
      const frameIndex = getState().frameIndex;
      try {
        const url = await bridge.pngUrl(id, frameIndex > 0 ? frameIndex : undefined);
        if (getState().revision === requested) setState({ pngUrl: url });
      } catch (error) {
        failKey("store.render_failed", { error: String(error) });
      }
    },

    openAip: async (path) => {
      const id = getState().activeId;
      if (!id) return;
      setState({ busy: true });
      try {
      const document = await bridge.aipLoad(path);
      await bridge.syncDocument(id, document);
      // 新文档和旧笔迹无关，撤销栈清空；否则一撤销就退回上一个文件。
      setState({ undoStack: [], pendingFrameIndex: null });
      noteKey("store.loaded", { name: baseName(path) });
        await getState().refreshDocument();
      } catch (error) {
        failKey("store.open_aip_failed", { error: String(error) });
      } finally {
        setState({ busy: false });
      }
    },

    saveAip: async (path) => {
      const id = getState().activeId;
      if (!id) return;
      setState({ busy: true });
      try {
        await bridge.aipSave(id, path);
        noteKey("store.saved", { name: baseName(path) });
      } catch (error) {
        failKey("store.save_failed", { error: String(error) });
      } finally {
        setState({ busy: false });
      }
    },

    readAipText: async () => {
      const id = getState().activeId;
      if (!id) return null;
      try {
        return await bridge.aipText(id);
      } catch (error) {
        failKey("store.read_aip_failed", { error: String(error) });
        return null;
      }
    },

    clearNotice: () => setState({ notice: null }),

    setLang: (lang) => {
      try {
        globalThis.localStorage?.setItem(LANG_KEY, lang);
      } catch {
        // 存储不可用就只切这一趟：设置里的选择当场就生效。
      }
      setState({ lang });
    },

    openSettings: () => setState({ settingsOpen: true }),

    closeSettings: () => setState({ settingsOpen: false }),

    refreshWorkflows: async () => {
      const id = getState().activeId;
      if (!id) {
        setState({ workflows: [], catalogReady: false });
        return;
      }
      try {
        const workflows = await bridge.workflowCatalog(id);
        setState({ workflows, catalogReady: true });
      } catch {
        // 会话刚建、模型还没绑好时会失败。不弹通知栏：每次切会话都弹就成噪音了。
        setState({ workflows: [], catalogReady: false });
      }
    },

    setKind: (kind) => {
      // 换工作流就作废上一条回执：不同 kind 的 detail 字段含义不一样，混着看会误读。
      setState({ kind, outcome: null, outcomeError: null });
    },

    setRefineTarget: (target) => setState({ refineTarget: target }),

    runWorkflow: async (kind, params) => {
      const id = getState().activeId;
      if (!id) {
        failKey("store.no_session");
        return null;
      }
      setState({ workflowBusy: true, outcomeError: null });
      try {
        const outcome = await dispatchWorkflow(id, kind, params);
        setState({
          outcome,
          outcomeError: null,
          entries: pushNotice(getState().entries, renderUiText(getState().lang, outcome.summary), false),
        });
        return outcome;
      } catch (error) {
        const message = workflowError(error);
        setState({
          outcome: null,
          outcomeError: message,
          entries: pushNotice(getState().entries, message, true),
        });
        return null;
      } finally {
        setState({ workflowBusy: false });
      }
    },

    runPixelize: async (params) => {
      const id = getState().activeId;
      if (!id) {
        failKey("store.no_session");
        return null;
      }
      setState({ workflowBusy: true, outcomeError: null });
      try {
        const outcome = await bridge.workflowPixelize(id, params);
        setState({
          outcome,
          outcomeError: null,
          entries: pushNotice(getState().entries, renderUiText(getState().lang, outcome.summary), false),
        });
        return outcome;
      } catch (error) {
        const message = workflowError(error);
        setState({
          outcome: null,
          outcomeError: message,
          entries: pushNotice(getState().entries, message, true),
        });
        return null;
      } finally {
        setState({ workflowBusy: false });
      }
    },

    refinePrompt: async (idea) => {
      const id = getState().activeId;
      if (!id) {
        failKey("store.no_session");
        return;
      }
      if (idea.trim() === "") {
        failKey("store.refine_empty");
        return;
      }
      setState({ workflowBusy: true, outcomeError: null, refined: null });
      try {
        // 宽高传 0：Rust 会拿画布的真实尺寸补，提示词里的比例才和画布对得上。
        const refined = await bridge.promptRefine(
          id,
          idea,
          0,
          0,
        getState().refineTarget,
      );
      setState({ refined, refinedDraft: refined.prompt, outcomeError: null });
      } catch (error) {
        setState({ outcomeError: workflowError(error) });
      } finally {
        setState({ workflowBusy: false });
      }
    },

    briefReference: async (path) => {
      const id = getState().activeId;
      if (!id) {
        failKey("store.no_session");
        return;
      }
      setState({ workflowBusy: true, outcomeError: null, vision: null });
      try {
        const vision = await bridge.visionBrief(id, path);
        setState({ vision, outcomeError: null });
      } catch (error) {
        setState({ outcomeError: workflowError(error) });
      } finally {
        setState({ workflowBusy: false });
      }
    },

    briefVideo: async (path, count) => {
      const id = getState().activeId;
      if (!id) {
        failKey("store.no_session");
        return;
      }
      setState({ workflowBusy: true, outcomeError: null, videoBrief: null });
      try {
        const videoBrief = await bridge.videoBrief(id, { path, count });
        setState({ videoBrief, outcomeError: null });
      } catch (error) {
        setState({ outcomeError: workflowError(error) });
      } finally {
        setState({ workflowBusy: false });
      }
    },

    probeVideo: async (path) => {
      setState({ workflowBusy: true, outcomeError: null, probe: null });
      try {
        const probe = await bridge.videoProbe(path);
        setState({ probe, outcomeError: null });
      } catch (error) {
        setState({ outcomeError: workflowError(error) });
      } finally {
        setState({ workflowBusy: false });
      }
    },

    clearWorkflowResult: () =>
      setState({
        outcome: null,
        outcomeError: null,
        refined: null,
        vision: null,
        probe: null,
        videoBrief: null,
        refinedDraft: "",
      }),

    patchDraft: (patch) => setState({ dockDraft: { ...getState().dockDraft, ...patch } }),

    setRefinedPrompt: (text) => setState({ refinedDraft: text }),

    surfaceError: (message) =>
      setState({
        outcomeError: message,
        entries: pushNotice(getState().entries, message, true),
      }),

    requestCompose: (text) =>
      setState({ composeRequest: { text, nonce: Date.now() } }),

    usePromptInGen: (prompt) =>
      setState({
        kind: "image_gen",
        outcome: null,
        outcomeError: null,
        dockDraft: { ...getState().dockDraft, prompt },
      }),

    resolveApproval: async (decision) => {
      const id = getState().activeId;
      const pending = getState().pendingApproval;
      if (!id || !pending) return;
      // 乐观清票：Button 点了就别让人再点，决定已经在路上了。
      setState({ pendingApproval: null });
      try {
        await bridge.resolveApproval(id, pending.callId, decision);
      } catch (error) {
        // 多半是这一轮已经翻页（新一轮开始 / 被中断），票自然作废，不是死锁。
        failKey("store.approval_failed", { error: String(error) });
      }
    },

    paintStroke: async (cells) => {
      const id = getState().activeId;
      const document = getState().document;
      const active = getState().active;
      if (!id || !document || cells.length === 0) return;
      undoCapture = true;
      try {
        await bridge.paintStroke(id, {
          layer: active.layer,
          frame: active.frame,
          cells,
          color: active.color ?? null,
        });
      } catch (error) {
        failKey("store.paint_failed", { error: String(error) });
      }
    },

    fillCell: async (x, y) => {
      const id = getState().activeId;
      const active = getState().active;
      if (!id) return;
      undoCapture = true;
      try {
        await bridge.fillCells(id, active.layer, active.frame, x, y, active.color ?? null);
      } catch (error) {
        failKey("store.fill_failed", { error: String(error) });
      }
    },

    runEditorOps: async (ops, frameHint) => {
      const id = getState().activeId;
      if (!id) return null;
      undoCapture = true;
      // 先把帧选择意图挂上：新文档还在路上，到了就按这个落点选帧。
      if (frameHint !== undefined) setState({ pendingFrameIndex: frameHint });
      try {
        return await bridge.applyEditorOps(id, ops);
      } catch (error) {
        setState({ pendingFrameIndex: null });
        failKey("store.edit_failed", { error: String(error) });
        return null;
      }
    },

    addFrame: async () => {
      const document = getState().document;
      if (!document) return;
      const current = document.frames[getState().frameIndex];
      // 时长继承当前帧：逐帧动画里新帧几乎总是延续同一节拍。
      await getState().runEditorOps(
        [
          {
            op: "create_frame",
            after: current?.id ?? null,
            duration_ms: current?.duration_ms ?? 100,
          },
        ],
        getState().frameIndex + 1,
      );
    },

    duplicateFrame: async () => {
      const document = getState().document;
      const current = document?.frames[getState().frameIndex];
      if (!current) return;
      // 复制帧插在源帧后面，所以新选中的是下一格。
      await getState().runEditorOps([{ op: "duplicate_frame", id: current.id }], getState().frameIndex + 1);
    },

    deleteFrame: async () => {
      const document = getState().document;
      const current = document?.frames[getState().frameIndex];
      if (!document || !current || document.frames.length <= 1) return;
      // 删掉第 i 帧后原第 i+1 帧顶上来，所以还选 i；本来就在末帧则夹到新末帧。
      await getState().runEditorOps([{ op: "delete_frame", id: current.id }], getState().frameIndex);
    },

    moveFrame: async (delta) => {
      const document = getState().document;
      const current = document?.frames[getState().frameIndex];
      if (!document || !current) return;
      const target = getState().frameIndex + delta;
      if (target < 0 || target >= document.frames.length) return;
      await getState().runEditorOps([{ op: "move_frame", id: current.id, to_index: target }], target);
    },

    undoEdit: async () => {
      const id = getState().activeId;
      const stack = getState().undoStack;
      const previous = stack[stack.length - 1];
      if (!id || !previous) return;
      setState({ undoStack: stack.slice(0, -1), pendingFrameIndex: null });
      try {
        await bridge.syncDocument(id, previous);
        // sync_document 不发 document_updated：状态和预览都得自己结算。
        const frameIndex = Math.max(0, Math.min(getState().frameIndex, previous.frames.length - 1));
        const frame = previous.frames[frameIndex];
        setState({
          document: previous,
          revision: previous.revision,
          pngRevision: previous.revision,
          frameIndex,
          active: frame ? { ...getState().active, frame: frame.id } : getState().active,
        });
        await getState().refreshPng();
      } catch (error) {
        failKey("store.undo_failed", { error: String(error) });
      }
    },

    refreshSessions: async () => {
      try {
        const sessions = await bridge.listSessions();
        sessions.sort((a, b) => a.id.localeCompare(b.id));
        setState({ sessions });
      } catch {
        // 会话列表刷新失败不影响主流程
      }
    },
  };
});

/** 空白文档：新建会话时指定画布尺寸用（与 Rust Document::new 的初始结构一致）。 */
export function blankDocument(width: number, height: number): PixelDocument {
  return {
    name: "untitled",
    width,
    height,
    palette: [],
    layers: [{ id: "L0", name: "Layer 1", visible: true, opacity: 255 }],
    frames: [{ id: "F0", duration_ms: 100 }],
    cels: { L0: { F0: { indices: new Array(width * height).fill(0) } } },
    revision: 0,
  };
}
