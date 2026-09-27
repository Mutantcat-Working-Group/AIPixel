// 应用状态：会话表、模型配置、对话条目、权威文档与画布预览。
// Rust 主循环只通过 agent-event 说话；所有副作用都收敛成 bridge 调用。

import { create } from "zustand";

import * as bridge from "./bridge";
import {
  emptyTranscript,
  historyToTranscript,
  pushUserMessage,
  reduceEvent,
  sealTranscript,
} from "./transcript";
import type {
  ActiveContext,
  Attachment,
  AgentEvent,
  ModelConfig,
  ModelsView,
  PendingAttachment,
  PermissionMode,
  PixelDocument,
  SessionInfo,
  TranscriptEntry,
  Usage,
} from "./types";

export interface DocumentSnapshot {
  document: PixelDocument | null;
  revision: number;
  pngUrl: string | null;
  /** 已经渲染过 PNG 的 revision，用来丢弃过期的异步结果。 */
  pngRevision: number;
  frameIndex: number;
}

interface StoreState extends DocumentSnapshot {
  booted: boolean;
  models: ModelsView;
  sessions: SessionInfo[];
  activeId: string | null;
  entries: TranscriptEntry[];
  running: boolean;
  usage: Usage | null;
  attachments: PendingAttachment[];
  permission: PermissionMode;
  active: ActiveContext;
  busy: boolean;
  notice: { text: string; isError: boolean } | null;
  settingsOpen: boolean;
}

export interface StoreActions {
  boot: () => Promise<void>;
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
}

const EMPTY_MODELS: ModelsView = { active_id: "", entries: [] };

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

let unlisten: (() => void) | null = null;
let booting: Promise<void> | null = null;
let snapshotSeq = 0;

export const useStore = create<StoreState & StoreActions>()((setState, getState) => {
  function fail(message: string) {
    setState({ notice: { text: message, isError: true }, running: false });
  }

  /** agent-event 路由：文档事件驱动画布，其余折叠进对话条目。 */
  async function ensureListener() {
    if (unlisten) return;
    unlisten = await bridge.listenAgentEvents((event: AgentEvent) => {
      const state = getState();
      if (event.kind === "document_updated") {
        const stale = event.revision < state.pngRevision;
        setState({
          document: event.document,
          revision: event.revision,
          pngRevision: stale ? state.pngRevision : event.revision,
        });
        void getState().refreshPng();
        return;
      }
      if (event.kind === "usage") {
        setState({ usage: { input: event.input_tokens, output: event.output_tokens } });
        return;
      }
      if (event.kind === "completed" || event.kind === "error" || event.kind === "interrupted") {
        setState({
          entries: sealTranscript(reduceEvent(state.entries, event)),
          running: false,
        });
        void getState().refreshSessions();
        return;
      }
      setState({ entries: reduceEvent(state.entries, event) });
    });
  }

  async function loadDocument(id: string) {
    try {
      const messages = await bridge.agentHistory(id);
      setState({ entries: historyToTranscript(messages) });
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
    notice: null,
    settingsOpen: false,

    boot: async () => {
      if (booting) return booting;
      booting = (async () => {
        await ensureListener();
        let models: ModelsView;
        try {
          models = await bridge.listModels();
        } catch (error) {
          fail(`读取模型配置失败：${String(error)}`);
          models = EMPTY_MODELS;
        }
        setState({ models });
        let sessions: SessionInfo[] = [];
        try {
          sessions = await bridge.listSessions();
        } catch (error) {
          fail(`读取会话列表失败：${String(error)}`);
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
        fail("还没有会话");
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
        fail(`发送失败：${String(error)}`);
      }
    },

    interrupt: async () => {
      const id = getState().activeId;
      if (!id) return;
      try {
        await bridge.interrupt(id);
      } catch (error) {
        fail(`中断失败：${String(error)}`);
      }
    },

    upsertModel: async (config) => {
      const models = await bridge.upsertModel(config);
      setState({ models });
    },

    removeModel: async (id) => {
      const models = await bridge.removeModel(id);
      setState({ models });
    },

    activateModel: async (id) => {
      const models = await bridge.setActiveModel(id);
      setState({ models });
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
          fail(`读取图片失败 ${baseName(path)}：${String(error)}`);
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
          fail("画布快照为空");
          return;
        }
       snapshotSeq += 1;
       setState({
         attachments: [
           ...getState().attachments,
           {
              key: `snapshot:${getState().revision}:${snapshotSeq}`,
              role: "snapshot",
              name: "canvas snapshot",
              mediaType,
              dataBase64: data,
              previewUrl: url,
            },
          ],
        });
      } catch (error) {
        fail(`截图失败：${String(error)}`);
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
        fail(`渲染画布失败：${String(error)}`);
      }
    },

    openAip: async (path) => {
      const id = getState().activeId;
      if (!id) return;
      setState({ busy: true });
      try {
        const document = await bridge.aipLoad(path);
        await bridge.syncDocument(id, document);
        setState({ notice: { text: `已载入 ${baseName(path)}`, isError: false } });
        await getState().refreshDocument();
      } catch (error) {
        fail(`打开 .aip 失败：${String(error)}`);
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
        setState({ notice: { text: `已保存 ${baseName(path)}`, isError: false } });
      } catch (error) {
        fail(`保存失败：${String(error)}`);
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
        fail(`读取 .aip 文本失败：${String(error)}`);
        return null;
      }
    },

    clearNotice: () => setState({ notice: null }),

    openSettings: () => setState({ settingsOpen: true }),

    closeSettings: () => setState({ settingsOpen: false }),

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
