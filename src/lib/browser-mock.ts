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
import { PALETTE_PRESETS, nearestHex, parseHex, rgbaToHex } from "./palette";
import { compositeFrame } from "./render";
import type {
  LoopLimits,
  McpServerView,
  McpServersView,
  NamedPalette,
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

/**
 * 预览文档里的配色范围库。和 Rust 的 `builtin_palettes()` 同一批 id，
 * 这样预览里打开的面板、能改的范围、新建的副本都跟真机一个形状。
 */
const PREVIEW_PALETTES: NamedPalette[] = PALETTE_PRESETS.map((preset) => ({
  id: preset.id,
  name: preset.name,
  colors: preset.colors.flatMap((hex) => {
    const parsed = parseHex(hex);
    return parsed ? [parsed] : [];
  }),
  builtin: true,
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
    layers: [
      { id: "L0", name: "Layer 1", visible: true, opacity: 255, palette_id: "sweetie16", locked: false },
    ],
    palettes: PREVIEW_PALETTES,
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

function makeSession(id: string, revision: number): SessionInfo {
  // 预览里摆两条会话，title 和 order 两项才都测得到：改名和拖动排序都指着它们。
  const title = sessionTitles[id] ?? null;
  const roles = (["chat", "image_gen", "vision", "video"] as const).map((role) => ({
    role,
    model_id: MODEL.id,
    model_label: MODEL.label,
    detached: false,
  }));
  return {
    id,
    model_id: MODEL.id,
    model_label: MODEL.label,
    roles,
    width: WIDTH,
    height: HEIGHT,
    revision,
    title,
    // 排序位从 1 起排，和 Rust 一致；0 留给还没落位的。
    order: sessionOrders[id] ?? 0,
  };
}

/** 预览的会话簿：改名、排序、新建、删除动的都是这本账。 */
const previewSessions: string[] = ["p1", "p2"];
const sessionTitles: Record<string, string | null> = {};
const sessionOrders: Record<string, number> = { p1: 1, p2: 2 };
let nextSessionSeq = 1;

/** 按排序位列出会话，同值时拿 id 兜底：和 Rust 的 session_list 一个规矩。 */
function listSessions(revision: number): SessionInfo[] {
  return [...previewSessions]
    .sort((a, b) => {
      const byOrder = (sessionOrders[a] ?? 0) - (sessionOrders[b] ?? 0);
      if (byOrder !== 0) return byOrder;
      return a < b ? -1 : a > b ? 1 : 0;
    })
    .map((sessionId) => makeSession(sessionId, revision));
}

/** 删号连名带位一起清：留在簿里的空号会把侧栏拖成鬼影。 */
function dropSession(id: string): void {
  const at = previewSessions.indexOf(id);
  if (at >= 0) previewSessions.splice(at, 1);
  delete sessionTitles[id];
  delete sessionOrders[id];
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

/** cel 下标 0 是透明格，requantize 的重映射表要拿它当第一位。 */
const TRANSPARENT_SLOT: Rgba = { r: 0, g: 0, b: 0, a: 0 };

/** Rust 的 next_id：从 0 往上找第一个没占用的编号，同一个文档里 id 稳定可复现。 */
function nextId(prefix: string, existing: string[]): string {
  for (let i = 0; i < existing.length + 2; i += 1) {
    const candidate = `${prefix}${i}`;
    if (!existing.includes(candidate)) return candidate;
  }
  return `${prefix}${existing.length}`;
}

/** Rust 的 palettes::slugify：ascii 字母数字留下，中文空格标点一律折叠成连字符。 */
function slugify(name: string): string {
  let out = "";
  let lastDash = false;
  for (const ch of name.toLowerCase()) {
    if ((ch >= "a" && ch <= "z") || (ch >= "0" && ch <= "9")) {
      out += ch;
      lastDash = false;
    } else if (out !== "" && !lastDash) {
      out += "-";
      lastDash = true;
    }
  }
  const trimmed = out.replace(/^-+|-+$/g, "");
  return trimmed === "" ? "palette" : trimmed;
}

/** Rust 的 unique_palette_id：撞名从 -2 往后数，不用随机，导入导出才稳。 */
function uniquePaletteId(base: string): string {
  const taken = (id: string) => doc.palettes.some((item) => item.id === id);
  if (!taken(base)) return base;
  for (let suffix = 2; suffix <= 200; suffix += 1) {
    const candidate = `${base}-${suffix}`;
    if (!taken(candidate)) return candidate;
  }
  return `${base}-${doc.palettes.length + 1}`;
}

/** 色进文档调色板就复用下标，没有才追加。返回的是 cel 里的下标（比 palette 下标多 1）。 */
function internColor(color: Rgba): number {
  const hex = rgbaToHex(color);
  const found = doc.palette.findIndex((item) => rgbaToHex(item) === hex);
  if (found >= 0) return found + 1;
  doc.palette.push(color);
  return doc.palette.length;
}

/**
 * 换配色范围时把这一层的像素就地收进新范围，行为对齐 Rust 的 requantize_layer：
 * 每个旧色先在「旧调色板」里取到 RGB，再在新范围里找最近色，最后才把新色补进文档
 * 调色板。顺序反了就会拿已经替换过的色当参照，一层比一层偏。
 */
function requantizeLayer(layerId: string, range: Rgba[]): void {
  if (range.length === 0) return;
  // 旧 RGB 快照必须先取，后面 internColor 会动 doc.palette。
  const targets: Rgba[] = [TRANSPARENT_SLOT];
  for (const color of doc.palette) {
    const hex = rgbaToHex(color);
    const snapped = nearestHex(range.map(rgbaToHex), hex) ?? hex;
    targets.push(parseHex(snapped) ?? color);
  }
  const remap = targets.map((color) => internColor(color));
  const frames = doc.cels[layerId];
  if (!frames) return;
  for (const cel of Object.values(frames)) {
    if (!cel) continue;
    for (let i = 0; i < cel.indices.length; i += 1) {
      const next = remap[cel.indices[i]];
      if (next !== undefined) cel.indices[i] = next;
    }
  }
}
let revision = 1;
let LIMITS: LoopLimits = {
  max_continuations: 20,
  max_retries: 5,
  max_reasoning_continuations: 2,
};

/** MCP 总开关在浏览器预览里的状态。默认开着，和 Rust 的默认值一致。 */
let MCP_ON = true;

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

/** 假后端在聊天里播一段「思考 -> 分流 -> 落笔 -> 答复」的演示流，纯浏览器里能直接看成色。
 * 事件类型与 Rust 主循环广播的一致（含工具调用与结果），折叠块的高度因此能量到真实值。 */
function demoTurn(): void {
  const planInput = {
    reference_mode: "style",
    intent: "tile-map",
    art_style: "16-bit platformer",
    mood: "cozy",
    knowledge: ["color ramp", "tile seam"],
  };
  const frames = [
    { at: 0, kind: "reasoning", text: "先看画布结构和需要改动的区域。" },
    { at: 420, kind: "reasoning", text: "\n中间那格改成橙色，其余保持原样。" },
    { at: 520, kind: "tool_call", id: "demo-c1", name: "pixel_plan", input: planInput },
    {
      at: 620,
      kind: "tool_result",
      id: "demo-c1",
      name: "pixel_plan",
      summary: "意图：瓦片地图 · 风格：16-bit platformer",
      is_error: false,
    },
    {
      at: 700,
      kind: "tool_call",
      id: "demo-c2",
      name: "pixel_apply_operations",
      input: {
        layers: [
          {
            name: "L0",
            operations: [
              { op: "rect", x: 8, y: 5, w: 4, h: 4, color: "3" },
              { op: "pixel", x: 9, y: 6, color: "2" },
            ],
          },
        ],
      },
    },
    {
      at: 840,
      kind: "tool_result",
      id: "demo-c2",
      name: "pixel_apply_operations",
      summary: "已写入 1 个图层、2 个操作",
      is_error: false,
    },
    {
      at: 1000,
      kind: "token",
      text: [
        "## 这一轮做了什么",
        "",
        "- 图层 `L0`：把中间 4x4 填成调色板 3 号色，中心点提亮",
        "- 瓦片底图 1x1 生效，边界像素留在格内",
        "",
        "**配色**沿用 `sweetie16`，1. 暖橙 `#f2a03d` 2. 亮黄 `#f7e26b`",
        "",
        "```lua",
        "rectfill(8, 5, 11, 8, pal(3))",
        "```",
      ].join("\n"),
    },
    { at: 1400, kind: "completed", turns: 1 },
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
    case "agent_mcp_enabled":
      return MCP_ON;
    case "agent_set_mcp_enabled":
      // 浏览器预览里真假端点都没有，但只要记住开关本身：
      // 设置界面的反馈、以及「关了之后工具清单变不变」都指着这个值。
      MCP_ON = Boolean(payload.enabled);
      return MCP_ON;
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
      return listSessions(revision);
    case "session_create": {
      const requested = payload.document as PixelDocument | null;
      if (requested) {
        doc = requested;
        revision += 1;
      }
      const fresh = `s-${nextSessionSeq++}`;
      previewSessions.push(fresh);
      sessionOrders[fresh] = previewSessions.length;
      return makeSession(fresh, revision);
    }
    case "session_bind_model":
    case "session_bind_role":
    case "session_clear_role":
      return makeSession(String(payload.id ?? "p1"), revision);
    case "session_drop":
      dropSession(String(payload.id ?? ""));
      return null;
    case "session_rename": {
      const target = String(payload.id ?? "p1");
      // 空白名当取消：侧栏改回默认编号，别留一个空标题。
      const title = (payload.title as string | null) ?? "";
      sessionTitles[target] = title.trim() === "" ? null : title;
      return makeSession(target, revision);
    }
    case "session_reorder":
      // 按新次序整批改写排序位，id 不在簿里就当没发生过。
      {
        const ids = (payload.ids as string[] | undefined) ?? [];
        ids.forEach((sessionId, index) => {
          if (!previewSessions.includes(sessionId)) return;
          sessionOrders[sessionId] = index + 1;
        });
        return null;
      }
    case "model_set_active":
    case "model_upsert":
      return { active_id: MODEL.id, entries: [MODEL] };
    case "model_remove":
      return { active_id: "", entries: [] };
    case "model_fetch_models":
      return FETCHABLE;
    case "model_probe_image": {
      // 浏览器预览没有真端点：按模型名演一遍三种结论，够看清按钮的三种反馈。
      const probe = payload as unknown as { model?: string };
      if (!probe.model) return { state: "unknown", reason: "no_model" };
      if (/image|draw|paint|flux|sd|dall|seedream|imagen/.test(probe.model)) {
        return { state: "yes", transport: "images" };
      }
      return { state: "no", reason: "text_only" };
    }
    case "agent_sync_document":
      revision += 1;
      return { revision };
    case "editor_paint_stroke": {
      // 落笔：把这一笔的格子写进当前 cel；橡皮的颜色是 null，也就是回到透明格。
      // 形状必须和真机一致：bridge 发的是 { id, stroke: { layer, frame, cells, color } }，
      // Rust 那边也是 stroke: StrokeRequest。mock 读平铺字段的话 strokes 会全落在
      // undefined 上——预览里画一笔看着有反馈，实际一格都没改。
      const stroke = payload.stroke as unknown as { layer: string; frame: string; cells: { x: number; y: number }[]; color: string | null };
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
      // 编辑器直接下的一整批结构操作。假后端不能整批吞掉：界面不动，预览就成了
      //「功能看着有、实际没接上」。真机由 Rust 的 apply_batch 执行，这里逐条照它的
      // 规矩演一遍：帧、图层、两套调色板（存储 palette 与命名范围 palettes）都做。
     for (const op of (payload.ops ?? []) as Record<string, unknown>[]) {
       const id = op.id as string | undefined;
        if (op.op === "create_frame") {
          const durationMs = typeof op.duration_ms === "number" ? op.duration_ms : 100;
          if (durationMs < 1 || durationMs > 60000) continue;
          const after = op.after as string | null | undefined;
          // 点了名又找不到那一帧，Rust 整个批次回滚；预览里就近跳过这一条。
          if (after != null && !doc.frames.some((frame) => frame.id === after)) continue;
          const newId = nextId("F", doc.frames.map((frame) => frame.id));
          const pos = after
            ? doc.frames.findIndex((frame) => frame.id === after) + 1
            : doc.frames.length;
          doc.frames.splice(Math.min(pos, doc.frames.length), 0, {
            id: newId,
            duration_ms: durationMs,
          });
          for (const layer of doc.layers) {
            const frames = doc.cels[layer.id] ?? {};
            frames[newId] = { indices: new Array(doc.width * doc.height).fill(0) };
            doc.cels[layer.id] = frames;
          }
        }
        if (op.op === "delete_frame" && id) {
          // 最后一帧是文档的骨头，删不得。
          if (doc.frames.length <= 1) continue;
          if (!doc.frames.some((frame) => frame.id === id)) continue;
          doc.frames = doc.frames.filter((frame) => frame.id !== id);
          for (const frames of Object.values(doc.cels)) delete frames[id];
        }
        if (op.op === "duplicate_frame" && id) {
          const src = doc.frames.findIndex((frame) => frame.id === id);
          if (src < 0) continue;
          const newId = nextId("F", doc.frames.map((frame) => frame.id));
          doc.frames.splice(src + 1, 0, { id: newId, duration_ms: doc.frames[src].duration_ms });
          for (const layer of doc.layers) {
            const frames = doc.cels[layer.id] ?? {};
            // 深复制：共用一个数组的话，改一格会同时改两帧。
            const copied = frames[id];
            frames[newId] = {
              indices: copied ? copied.indices.slice() : new Array(doc.width * doc.height).fill(0),
            };
            doc.cels[layer.id] = frames;
          }
        }
        if (op.op === "move_frame" && id) {
          const from = doc.frames.findIndex((frame) => frame.id === id);
          if (from < 0) continue;
          const to = Math.max(0, Math.min(Number(op.to_index ?? 0), doc.frames.length - 1));
          const [frame] = doc.frames.splice(from, 1);
          doc.frames.splice(to, 0, frame);
        }
        if (op.op === "move_layer" && id) {
          const from = doc.layers.findIndex((item) => item.id === id);
          if (from < 0) continue;
          const to = Math.max(0, Math.min(Number(op.to_index ?? 0), doc.layers.length - 1));
          const [layer] = doc.layers.splice(from, 1);
          doc.layers.splice(to, 0, layer);
        }
        if (op.op === "rename_layer" && id) {
          const layer = doc.layers.find((item) => item.id === id);
          if (layer && typeof op.name === "string") layer.name = op.name;
        }
        if (op.op === "set_layer_properties" && id) {
          const layer = doc.layers.find((item) => item.id === id);
          if (layer) {
            if (typeof op.visible === "boolean") layer.visible = op.visible;
            if (typeof op.opacity === "number") layer.opacity = op.opacity;
          }
        }
        if (op.op === "set_frame_duration" && id) {
          const frame = doc.frames.find((item) => item.id === id);
          if (frame && typeof op.duration_ms === "number") frame.duration_ms = op.duration_ms;
        }
        if (op.op === "create_layer") {
          // 位置、名字、id 都按 Rust 的规矩来：插在 after 之后，没点名就落栈顶；
          // id 取第一个没占用的编号。配色范围和锁跟新邻居继承，行为对齐。
          const after = op.after as string | null | undefined;
          if (after != null && !doc.layers.some((l) => l.id === after)) continue;
          const pos = after
            ? doc.layers.findIndex((l) => l.id === after) + 1
            : doc.layers.length;
          const neighbor = doc.layers[Math.max(0, pos - 1)];
          // 邻居的配色得真在库里，指向一个不存在的 id 会开天窗。
          const inherit = doc.palettes.some((p) => p.id === neighbor?.palette_id)
            ? neighbor.palette_id
            : (doc.palettes[0]?.id ?? "sweetie16");
          const newId = nextId("L", doc.layers.map((l) => l.id));
          doc.layers.splice(Math.min(pos, doc.layers.length), 0, {
            id: newId,
            name: typeof op.name === "string" ? op.name : `Layer ${doc.layers.length + 1}`,
            visible: true,
            opacity: 255,
            palette_id: inherit,
            locked: neighbor?.locked ?? false,
          });
          const cels: Record<string, { indices: number[] }> = {};
          for (const frame of doc.frames) {
            cels[frame.id] = { indices: new Array(doc.width * doc.height).fill(0) };
          }
          doc.cels[newId] = cels;
        }
        if (op.op === "delete_layer" && id) {
          // 最后一层不删：Rust 直接报错，预览里也照样端着。
          if (doc.layers.length > 1) {
            doc.layers = doc.layers.filter((l) => l.id !== id);
            delete doc.cels[id];
          }
        }
        // ---- 文档调色板（像素实际存的那种）----
        if (op.op === "add_palette_colors") {
          for (const hex of (op.colors as string[] | undefined) ?? []) {
            const color = parseHex(hex);
            if (color) internColor(color);
          }
        }
        if (op.op === "set_palette") {
          const next = ((op.colors as string[] | undefined) ?? [])
            .map(parseHex)
            .filter((color): color is Rgba => color !== null);
          // 空配色等于把整幅擦透明，那是毁画面不是换风格，跟 Rust 一样端着。
          if (next.length === 0) continue;
          // 旧下标 -> 新下标：透明与归不进新色板的都回透明格。
          const remap = doc.palette.map((old) => {
            const snapped = nearestHex(
              next.map(rgbaToHex),
              rgbaToHex(old),
            );
            const index =
              snapped === null ? -1 : next.findIndex((item) => rgbaToHex(item) === snapped);
            return index + 1;
          });
          for (const frames of Object.values(doc.cels)) {
            for (const cel of Object.values(frames)) {
              if (!cel) continue;
              for (let i = 0; i < cel.indices.length; i += 1) {
                const nextIndex = remap[cel.indices[i] - 1];
                cel.indices[i] = typeof nextIndex === "number" ? nextIndex : 0;
              }
            }
          }
          doc.palette = next;
        }
        // ---- 命名配色范围（每层各认领的那套边界）----
        if (op.op === "create_palette") {
          const name = typeof op.name === "string" ? op.name : "";
          const from = typeof op.from === "string" ? op.from : undefined;
          const explicit = typeof op.id === "string" ? op.id : undefined;
          if (explicit && doc.palettes.some((p) => p.id === explicit)) continue;
          // 点名了 id 就以它为准：Rust 在 fork 与新建两条支路上都会盖掉自动命名。
          const idOf = (fallback: string) => explicit ?? fallback;
          let palette: NamedPalette;
          if (from) {
            const src = doc.palettes.find((p) => p.id === from);
            if (!src) continue;
            if (src.builtin) {
              // 内置预设走副本：原套一个色都不动，副本 id 照 Rust 的命名来。
              palette = {
                id: idOf(uniquePaletteId(`${slugify(src.name)}-copy`)),
                name: `${src.name} copy`,
                colors: src.colors.map((color) => ({ ...color })),
                builtin: false,
              };
            } else {
              palette = {
                id: idOf(uniquePaletteId(slugify(name || src.name))),
                name: name || src.name,
                colors: src.colors.map((color) => ({ ...color })),
                builtin: false,
              };
            }
          } else {
            palette = {
              id: idOf(uniquePaletteId(slugify(name || "palette"))),
              name: name || "palette",
              colors: [],
              builtin: false,
            };
          }
          for (const hex of (op.colors as string[] | undefined) ?? []) {
            const color = parseHex(hex);
            if (!color) continue;
            if (palette.colors.some((item) => rgbaToHex(item) === rgbaToHex(color))) continue;
            palette.colors.push(color);
          }
          doc.palettes.push(palette);
          // 指名的那层跟着换过去，换范围要把像素就地收进新范围。
          if (typeof op.layer === "string" && op.layer !== "") {
            const target = doc.layers.find((l) => l.id === op.layer);
            if (target && palette.colors.length > 0) {
              target.palette_id = palette.id;
              requantizeLayer(target.id, palette.colors);
            }
          }
        }
        if (op.op === "delete_palette" && id) {
          const target = doc.palettes.find((p) => p.id === id);
          // 内置的删不得，还被某层指着的也删不得：静默改指会让人莫名换色板。
          if (!target || target.builtin || doc.layers.some((l) => l.palette_id === id)) continue;
          doc.palettes = doc.palettes.filter((p) => p.id !== id);
        }
        if (op.op === "rename_palette" && id) {
          const target = doc.palettes.find((p) => p.id === id);
          const name = typeof op.name === "string" ? op.name.trim() : "";
          if (!target || target.builtin || name === "") continue;
          target.name = name;
        }
        if (op.op === "add_palette_color" && id) {
          const target = doc.palettes.find((p) => p.id === id);
          const color = parseHex(typeof op.color === "string" ? op.color : "");
          if (!target || target.builtin || !color) continue;
          if (target.colors.some((item) => rgbaToHex(item) === rgbaToHex(color))) continue;
          target.colors.push(color);
        }
        if (op.op === "remove_palette_color" && id) {
          const target = doc.palettes.find((p) => p.id === id);
          const index = Number(op.index ?? -1);
          if (!target || target.builtin) continue;
          // 范围里总得留一个色，最后一个是不让删的。
          if (index < 0 || index >= target.colors.length || target.colors.length <= 1) continue;
          target.colors.splice(index, 1);
        }
        if (op.op === "set_layer_palette") {
          const palette = doc.palettes.find((p) => p.id === op.palette_id);
          const target =
            typeof op.layer === "string" ? doc.layers.find((l) => l.id === op.layer) : undefined;
          if (!target || !palette || palette.colors.length === 0) continue;
          target.palette_id = palette.id;
          requantizeLayer(target.id, palette.colors);
        }
        if (op.op === "set_layer_locked") {
          const target =
            typeof op.layer === "string" ? doc.layers.find((l) => l.id === op.layer) : undefined;
          if (target && typeof op.locked === "boolean") target.locked = op.locked;
        }
      }
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
