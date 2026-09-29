/** 纯浏览器里的假后端。
 *
 * 只在一个前提下开工：`window.__TAURI_INTERNALS__` 不存在，也就是没被 Tauri 的 webview
 * 包着。真机（开发期或打包后）一律原样放行，这里的数据一丝都渗不进真会话。
 * 用途是在普通浏览器里看界面、截图、量样式，不必先等一遍原生构建。
 *
 * 编辑命令按真机的路子假扮：改一份内存里的文档，再往 `agent-event` 发一条
 * `document_updated`，画布因此真的会变——和 Rust 唯一的区别只是没有落盘。
 *
 * `?mock=empty`：一条模型都没配，用来验顶栏的「未设置模型」。
 */

import { emit } from "@tauri-apps/api/event";
import { mockIPC } from "@tauri-apps/api/mocks";

import { AGENT_EVENT_CHANNEL } from "./bridge";
import { publishLocal } from "./local-bus";
import { rgbaToHex } from "./palette";
import { compositeFrame } from "./render";
import type {
  LoopLimits,
  McpServerView,
  McpServersView,
  ModelView,
  PixelDocument,
  Rgba,
  SessionInfo,
  WorkflowEntry,
} from "./types";

const WIDTH = 20;
const HEIGHT = 16;

/** 24 色调色板。字符 '1'..'9'、'a'..'n' 依次对应第 1..24 号色，'.' 是透明格。 */
const HEX = [
  "#14110f",
  "#f7e26b",
  "#f2a03d",
  "#fffdf0",
  "#8a5a2b",
  "#2f6690",
  "#3f8efc",
  "#8ec7e8",
  "#3fa34d",
  "#a3d977",
  "#d64550",
  "#f2a7bb",
  "#6c4ab6",
  "#b39ddb",
  "#565264",
  "#a0a0a0",
  "#d6d6d6",
  "#1b1b22",
  "#2e2a35",
  "#463f5c",
  "#7a6a9b",
  "#06d6a0",
  "#ffd166",
  "#ef476f",
];

const PALETTE: Rgba[] = HEX.map((hex) => ({
  r: parseInt(hex.slice(1, 3), 16),
  g: parseInt(hex.slice(3, 5), 16),
  b: parseInt(hex.slice(5, 7), 16),
  a: 255,
}));

const SLOT = "123456789abcdefghijklmnopqrstuvwxyz";

/** 一颗居中的爱心，10x8，'.' 处透明。帧 2、3 只是往下沉一格，预览里看得出在跳。 */
const HEART = [
  "..11.11..",
  ".12212221.",
  ".12222221.",
  ".12222221.",
  "..1222221.",
  "...122221.",
  "....12221.",
  ".....121..",
];

function makeCel(dropY: number, sparkle: boolean): { indices: number[] } {
  const indices = new Array<number>(WIDTH * HEIGHT).fill(0);
  const put = (x: number, y: number, ch: string) => {
    if (x < 0 || y < 0 || x >= WIDTH || y >= HEIGHT) return;
    indices[y * WIDTH + x] = SLOT.indexOf(ch) + 1;
  };
  HEART.forEach((row, ry) => {
    for (let rx = 0; rx < row.length; rx += 1) {
      const ch = row[rx];
      if (ch !== ".") put(rx + 5, ry + 2 + dropY, ch);
    }
  });
  if (sparkle) {
    put(16, 2, "4");
    put(17, 3, "4");
    put(16, 4, "4");
  }
  return { indices };
}

function makeDocument(): PixelDocument {
  return {
    name: "preview",
    width: WIDTH,
    height: HEIGHT,
    palette: PALETTE,
    layers: [{ id: "L0", name: "Layer 1", visible: true, opacity: 255 }],
    frames: [
      { id: "F0", duration_ms: 120 },
      { id: "F1", duration_ms: 120 },
      { id: "F2", duration_ms: 120 },
    ],
    cels: {
      L0: {
        F0: makeCel(0, false),
        F1: makeCel(1, false),
        F2: makeCel(2, true),
      },
    },
    revision: 1,
  };
}

const MODEL: ModelView = {
  id: "m1",
  label: "本地示例模型",
  protocol: "open_ai_compat",
  base_url: "https://api.example.com/v1",
  model: "gpt-4o-mini",
  max_tokens: 16384,
  temperature: 0.7,
  disable_thinking: null,
  capabilities: { vision: true, image_gen: false, video: false, reasoning: true },
  has_api_key: true,
};

const MCP_TRANSPORT = {
  kind: "http" as const,
  command: "",
  args: [],
  env_keys: [],
  url: "http://127.0.0.1:8765/mcp",
  header_keys: ["authorization"],
};

function makeSession(revision: number): SessionInfo {
  const roles = (["chat", "image_gen", "vision", "video"] as const).map((role) => ({
    role,
    model_id: MODEL.id,
    model_label: MODEL.label,
    detached: false,
  }));
  return {
    id: "p1",
    model_id: MODEL.id,
    model_label: MODEL.label,
    roles,
    width: WIDTH,
    height: HEIGHT,
    revision,
  };
}

const CATALOG: Omit<WorkflowEntry, "readiness" | "served_by_label" | "served_by_detached">[] = [
  {
    kind: "agent",
    id: "agent",
    title: "Agent Draw",
    summary: "Chat to draw. Runs one sandboxed Lua script per edit.",
    needs: { vision: false, image_gen: false, video: false },
    output: "Edited canvas document",
  },
  {
    kind: "image_gen",
    id: "image_gen",
    title: "Image Generate",
    summary: "Model renders a bitmap, then it is quantized onto the canvas grid.",
    needs: { vision: false, image_gen: true, video: false },
    output: "One new cel of pixelized art",
  },
  {
    kind: "vision_brief",
    id: "vision_brief",
    title: "Reference Brief",
    summary: "A vision model reads your reference into a structured brief, then draws.",
    needs: { vision: true, image_gen: false, video: false },
    output: "Brief text plus a drawn canvas",
  },
  {
    kind: "video_frames",
    id: "video_frames",
    title: "Video Frames",
    summary: "Pull key frames out of a clip and quantize each one onto its own frame.",
    needs: { vision: false, image_gen: false, video: false },
    output: "A frame sequence drawn from video stills",
  },
  {
    kind: "video_brief",
    id: "video_brief",
    title: "Video Brief",
    summary: "A video model reads a clip into a motion brief you can edit, then draws.",
    needs: { vision: false, image_gen: false, video: true },
    output: "Motion brief text plus a drawn canvas",
  },
  {
    kind: "frame_tween",
    id: "frame_tween",
    title: "In-between",
    summary: "Generate tween frames between two existing frames, on this machine.",
    needs: { vision: false, image_gen: false, video: false },
    output: "New frames inserted before the end frame",
  },
  {
    kind: "prompt_refine",
    id: "prompt_refine",
    title: "Prompt Refine",
    summary: "Turn a plain sentence into a structured pixel-art prompt you can edit.",
    needs: { vision: false, image_gen: false, video: false },
    output: "A refined prompt you can send or keep editing",
  },
];

/** `model_fetch_models` 回给「获取」按钮的假清单，够挑一个填进表单就行。 */
const FETCHABLE = [
  "gpt-4o",
  "gpt-4o-mini",
  "gpt-4.1",
  "gpt-5-mini",
  "claude-sonnet-4-5",
  "claude-haiku-4-5",
  "deepseek-chat",
  "qwen-max",
];

let doc = makeDocument();
let revision = 1;
let LIMITS: LoopLimits = {
  max_continuations: 20,
  max_retries: 5,
  max_reasoning_continuations: 2,
};

function mcpList(): McpServersView {
  const server: McpServerView = {
    name: "example-tools",
    transport: MCP_TRANSPORT,
    auto_connect: true,
    connected: true,
    tools: [
      {
        name: "example-tools__palette_swap",
        server: "example-tools",
        tool: "palette_swap",
        description: "Swap two palette slots across the document.",
      },
    ],
    last_error: null,
  };
  return { entries: [server] };
}

function catalog(): WorkflowEntry[] {
  return CATALOG.map((info) => ({
    ...info,
    readiness: { state: "ready" as const },
    served_by_label: MODEL.label,
    served_by_detached: false,
  }));
}

/** 走一遍真正的合成代码出 PNG，预览里的画布和导出的才是同一个东西。 */
function pngFor(frameIndex: number): string {
  const canvas = document.createElement("canvas");
  canvas.width = doc.width;
  canvas.height = doc.height;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";
  const pixels = compositeFrame(doc, Math.max(0, Math.min(frameIndex, doc.frames.length - 1)));
  ctx.putImageData(new ImageData(pixels, doc.width, doc.height), 0, 0);
  return canvas.toDataURL("image/png");
}

/** 文档变了就往真机同一条通道报一声，前端的路由一行都不用改。 */
function fire(channel: string, payload: unknown): void {
  // 事件桥能用就走 Tauri（真机 / mockIPC 装了 invoke 的时候）；
  // 用不了就落页面内总线，至少订阅方还在。
  const internals = (window as unknown as { __TAURI_INTERNALS__?: { invoke?: unknown } })
    .__TAURI_INTERNALS__;
  if (typeof internals?.invoke === "function") {
    void emit(channel, payload);
    return;
  }
  publishLocal(channel, payload);
}

function broadcast(): void {
  fire(AGENT_EVENT_CHANNEL, { kind: "document_updated", revision, document: doc });
}

/** 假后端在聊天里播一段「思考 -> 答复」的演示流，纯浏览器里能直接看成色。 */
function demoTurn(): void {
  const frames = [
    { at: 0, kind: "reasoning", text: "先看画布结构和需要改动的区域。" },
    { at: 420, kind: "reasoning", text: "\n中间那格改成橙色，其余保持原样。" },
    { at: 840, kind: "token", text: "我来把这格涂成橙色。" },
    { at: 1300, kind: "completed", turns: 1 },
  ] as const;
  for (const frame of frames) {
    setTimeout(() => fire(AGENT_EVENT_CHANNEL, frame), frame.at);
  }
}

type Args = Record<string, unknown>;

function handler(cmd: string, raw?: unknown): unknown {
  const payload: Args = (raw ?? {}) as Args;
  const id = String(payload.id ?? "p1");
  switch (cmd) {
    case "agent_list_models":
      return emptyModels ? { active_id: "", entries: [] } : { active_id: MODEL.id, entries: [MODEL] };
    case "agent_loop_limits":
      return LIMITS;
    case "agent_set_loop_limits":
      LIMITS = { ...(payload.limits as typeof LIMITS) };
      return LIMITS;
    case "agent_document":
      return { id, revision, document: doc };
    case "agent_history":
      return [];
    case "agent_set_active":
    case "agent_set_permission":
    case "agent_interrupt":
    case "agent_resolve_approval":
      return null;
    case "agent_send_message":
      demoTurn();
      return null;
    case "session_list":
      return [makeSession(revision)];
    case "session_create": {
      const requested = payload.document as PixelDocument | null;
      if (requested) {
        doc = requested;
        revision += 1;
      }
      return makeSession(revision);
    }
    case "session_bind_model":
    case "session_bind_role":
    case "session_clear_role":
      return makeSession(revision);
    case "session_drop":
      return null;
    case "model_set_active":
    case "model_upsert":
      return { active_id: MODEL.id, entries: [MODEL] };
    case "model_remove":
      return { active_id: "", entries: [] };
    case "model_fetch_models":
      return FETCHABLE;
    case "agent_sync_document":
      revision += 1;
      return { revision };
    case "editor_paint_stroke": {
      // 落笔：把这一笔的格子写进当前 cel；橡皮的颜色是 null，也就是回到透明格。
      const stroke = payload as unknown as { layer: string; frame: string; cells: { x: number; y: number }[]; color: string | null };
      const cel = doc.cels[stroke.layer]?.[stroke.frame];
      if (cel) {
        const slot = stroke.color ? doc.palette.findIndex((c) => rgbaToHex(c) === stroke.color) : 0;
        const index = slot >= 0 ? slot + 1 : 1;
        for (const cell of stroke.cells) {
          if (cell.x < 0 || cell.y < 0 || cell.x >= doc.width || cell.y >= doc.height) continue;
          cel.indices[cell.y * doc.width + cell.x] = stroke.color ? index : 0;
        }
      }
      revision += 1;
      broadcast();
      return revision;
    }
    case "editor_fill": {
      const fill = payload.fill as { layer: string; frame: string; x: number; y: number; color: string | null };
      const cel = doc.cels[fill.layer]?.[fill.frame];
      if (cel) {
        const slot = fill.color ? doc.palette.findIndex((c) => rgbaToHex(c) === fill.color) : 0;
        const index = slot >= 0 ? slot + 1 : 1;
        if (fill.x >= 0 && fill.y >= 0 && fill.x < doc.width && fill.y < doc.height) {
          cel.indices[fill.y * doc.width + fill.x] = fill.color ? index : 0;
        }
      }
      revision += 1;
      broadcast();
      return revision;
    }
    case "editor_apply_ops":
      revision += 1;
      broadcast();
      return revision;
    case "mcp_list":
      return mcpList();
    case "mcp_upsert":
    case "mcp_connect":
    case "mcp_disconnect":
    case "mcp_remove":
      return mcpList();
    case "workflow_catalog":
      return catalog();
    case "document_png_url":
      return pngFor(Number(payload.frame ?? 0));
    case "batch_recipes_list":
      return [];
    case "batch_recipe_save":
    case "batch_recipe_delete":
    case "batch_recipe_export":
    case "batch_recipe_import":
    case "batch_scan":
    case "batch_run":
      return null;
    case "document_export":
    case "aip_save":
    case "aip_load":
      return null;
    case "aip_text":
      return "AIP1\n";
    default:
      if (cmd.startsWith("plugin:")) return null;
      throw new Error(`浏览器预览没有这条假数据：${cmd}`);
  }
}

let emptyModels = false;

/** 装好假后端。不在 Tauri 里就什么都不做，一句提示都不打。 */
export function installBrowserMock(): void {
  if (typeof window === "undefined") return;
  const internals = (window as unknown as { __TAURI_INTERNALS__?: { invoke?: unknown } })
    .__TAURI_INTERNALS__;
  // 是不是真机，看 invoke 在不在，而不是 internals 这个对象存不存在：
  // 只认对象的话，碰上被谁抢先注入的半成品 internals 会两头落空——
  // mock 不装，真机的命令又调不通。
  if (typeof internals?.invoke === "function") return;
  emptyModels = new URLSearchParams(window.location.search).get("mock") === "empty";
  mockIPC(handler, { shouldMockEvents: true });
}
