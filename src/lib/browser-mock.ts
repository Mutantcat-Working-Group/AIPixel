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

import { AGENT_EVENT_CHANNEL, CLOSE_EVENT_CHANNEL } from "./bridge";
import { publishLocal } from "./local-bus";
import { PALETTE_PRESETS, nearestHex, parseHex, rgbaToHex } from "./palette";
import { compositeFrame } from "./render";
import type {
  Attachment,
  DocPatch,
  MigrateOrder,
  LoopLimits,
  McpServerView,
  McpServerStatusView,
  McpServersView,
  NamedPalette,
  PixelizeParams,
  PixelizeOptions,
  ModelView,
  ModelsView,
  PixelDocument,
  RefineTarget,
  RefinedPrompt,
  Readiness,
  Rgba,
  SessionInfo,
  TweenParams,
  VideoBrief,
  VideoBriefParams,
  VideoFramesParams,
  VideoProbeResult,
  VisionBrief,
  WorkflowOutcome,
  ModelConfig,
  ModelRole,
  LandSpot,
  WorkflowEntry,
} from "./types";

const WIDTH = 20;
const HEIGHT = 16;

/** 和 Rust 的 pixel_core::document::MAX_PALETTE 对齐：调色板条目上限。 */
const MAX_PALETTE = 256;
/** 和 Rust 的 MAX_FRAME_DURATION_MS 对齐：单帧时长上限（毫秒）。 */
const MAX_FRAME_DURATION_MS = 60000;

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

function makeCel(
  dropY: number,
  sparkle: boolean,
  width: number,
  height: number,
): { indices: number[] } {
  const indices = new Array<number>(width * height).fill(0);
  const put = (x: number, y: number, ch: string) => {
    if (x < 0 || y < 0 || x >= width || y >= height) return;
    indices[y * width + x] = SLOT.indexOf(ch) + 1;
  };
  // 图案按画布尺寸居中：预览里换成 64x64 也不能让它缩在左上角。
  const heartWidth = Math.max(...HEART.map((row) => row.length));
  const originX = Math.max(0, Math.floor((width - heartWidth) / 2));
  const originY = Math.max(0, Math.floor((height - HEART.length) / 2));
  HEART.forEach((row, ry) => {
    for (let rx = 0; rx < row.length; rx += 1) {
      const ch = row[rx];
      if (ch !== ".") put(rx + originX, ry + originY + dropY, ch);
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
      { id: "F3", duration_ms: 120 },
      { id: "F4", duration_ms: 120 },
      { id: "F5", duration_ms: 120 },
      { id: "F6", duration_ms: 120 },
    ],
    cels: {
      L0: {
        F0: makeCel(0, false, WIDTH, HEIGHT),
        F1: makeCel(1, false, WIDTH, HEIGHT),
        F2: makeCel(2, true, WIDTH, HEIGHT),
        F3: makeCel(0, false, WIDTH, HEIGHT),
        F4: makeCel(1, false, WIDTH, HEIGHT),
        F5: makeCel(2, true, WIDTH, HEIGHT),
        F6: makeCel(0, true, WIDTH, HEIGHT),
      },
    },
    revision: 1,
  };
}

/**
 * 用户在新建弹窗里选了尺寸，预览要有一份对应的画布。
 * 预览里只维护一份文档：换尺寸就按新宽高重画一份，侧栏显示和画布渲染才一致。
 */
function makeDocumentSized(width: number, height: number): PixelDocument {
  const w = Math.max(8, width);
  const h = Math.max(8, height);
  return {
    ...makeDocument(),
    width: w,
    height: h,
    cels: {
      L0: {
        F0: makeCel(0, false, w, h),
        F1: makeCel(1, false, w, h),
        F2: makeCel(2, true, w, h),
      },
    },
  };
}

const SAMPLE_MODEL: ModelView = {
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

/** 带生图能力的第二份样例。只有它把 image_gen 开着，所以「生图」那条工作流在
 * 预览里既点得到「不会生图」的报错，也跑得通整条量化落盘。 */
const SAMPLE_IMAGE_MODEL: ModelView = {
  id: "m2",
  label: "本地示例生图模型",
  protocol: "open_ai_compat",
  base_url: "https://api.example.com/v1",
  model: "sample-image-1",
  max_tokens: 8192,
  temperature: 0.7,
  disable_thinking: null,
  capabilities: { vision: true, image_gen: true, video: false, reasoning: false },
  has_api_key: true,
};

/** 三项能力全开的第三份样例。前两份各缺一项能力，光靠它们，
 *  读视频那两条工作流在预览里只能演到「能力不够」就断了。 */
const SAMPLE_ALL_MODEL: ModelView = {
  id: "m3",
  label: "本地示例全能模型",
  protocol: "open_ai_compat",
  base_url: "https://api.example.com/v1",
  model: "sample-all-1",
  max_tokens: 32768,
  temperature: 0.7,
  disable_thinking: null,
  capabilities: { vision: true, image_gen: true, video: true, reasoning: true },
  has_api_key: true,
};

/**
 * 预览里的模型簿与默认模型。
 *
 * 早期这里是个常量：model_upsert 一律回硬编码的一份，改名、删模型、切默认
 * 在预览里全无反应，「设置里改了侧栏不跟」这条真机 bug 因此复现不出来。
 * 现在按真机的规矩记一份账：upsert 真的插入/更新，remove 真的删，
 * 会话按 model_id 取 label，删掉当前默认时让剩下的顶上。
 */
const previewModels: ModelView[] = [
  { ...SAMPLE_MODEL },
  { ...SAMPLE_IMAGE_MODEL },
  { ...SAMPLE_ALL_MODEL },
];
let activeModelId = SAMPLE_MODEL.id;
/** 密钥只在真机才配，预览里留一份空账本是让人看见「沿用旧的」这条路真的是空的。 */
const previewApiKeys: Record<string, string> = {};
// 样例模型自带一个占位密钥，has_api_key 才和 ModelView 上的初始值对得上。
previewApiKeys[SAMPLE_MODEL.id] = "preview-only-key";
previewApiKeys[SAMPLE_IMAGE_MODEL.id] = "preview-only-key";
previewApiKeys[SAMPLE_ALL_MODEL.id] = "preview-only-key";
/** 会话名 -> 模型 id。没记的会话跟着默认模型走，和 Rust 一个规矩。 */
const sessionModels: Record<string, string> = {};
/** 会话 id -> 角色 -> 单独绑过的模型 id。没记的角色正在蹭会话主模型。
 *  真机那边 image_gen / vision / video 三个角色各能另绑一个模型，
 *  预览不记这本账，设置界面的分工就在浏览器里演不出来。 */
const sessionRoles: Record<string, Partial<Record<ModelRole, string>>> = {};

/** 这个角色实际该用哪个模型：单独绑了用它，否则回落会话主模型。
 *  和 Rust 的 `model_for_role` 同一套回落，单模型用户拿到的永远是主模型。 */
function modelForRole(sessionId: string, role: ModelRole): ModelView {
  const bound = role === "chat" ? undefined : sessionRoles[sessionId]?.[role];
  if (bound !== undefined && previewModels.some((entry) => entry.id === bound)) {
    return modelView(bound);
  }
  return modelView(sessionModels[sessionId] ?? activeModelId);
}

function modelView(id: string | null | undefined): ModelView {
  return (
    previewModels.find((entry) => entry.id === id) ??
    previewModels.find((entry) => entry.id === activeModelId) ??
    previewModels[0] ??
    SAMPLE_MODEL
  );
}

function modelsView(): ModelsView {
  return {
    active_id: previewModels.some((entry) => entry.id === activeModelId) ? activeModelId : "",
    entries: previewModels.map((entry) => ({ ...entry })),
  };
}

/** 删掉 model_id 之后还挂在空号上的会话，落到当前默认模型上。 */
function repairOrphanedSessions(): void {
  for (const sessionId of Object.keys(sessionModels)) {
    if (!previewModels.some((entry) => entry.id === sessionModels[sessionId])) {
      delete sessionModels[sessionId];
    }
  }
  // 角色绑定同理：模型删了还挂着 detached=true 的话，侧栏会报一个不存在的模型名。
  for (const sessionId of Object.keys(sessionRoles)) {
    const overrides = sessionRoles[sessionId];
    for (const role of Object.keys(overrides) as ModelRole[]) {
      const bound = overrides[role];
      if (bound === undefined || !previewModels.some((entry) => entry.id === bound)) {
        delete overrides[role];
      }
    }
  }
}

const MCP_TRANSPORT = {
  kind: "http" as const,
  command: "",
  args: [],
  env_keys: [],
  url: "http://127.0.0.1:8765/mcp",
  header_keys: ["authorization"],
};

/**
 * 每个号各自的画布读数。mock 只有一份全局画布，可侧栏一行报的是「这个会话」
 * 的宽高和版本（真机 session_info 读的是各会话自己的文档）。不按号分开记账，
 * 新建一个 32x16 的会话就会把侧栏每一行都改成「32x16 版本 3」，看着像所有
 * 会话共用一张画布。写入点只有四处：广播、整份换文档、新建会话、按号取文档。
 */
const sessionDocs: Record<string, { width: number; height: number; revision: number }> = {};

/** 把当前画布的读数记到某个号名下；要记的尺寸不是当前画布时显式传一份。 */
function recordSessionDoc(id: string, size?: { width: number; height: number }): void {
  sessionDocs[id] = {
    width: size?.width ?? doc.width,
    height: size?.height ?? doc.height,
    revision,
  };
}

function makeSession(id: string, revision: number): SessionInfo {
  // 宽高按这个号自己的读数报，没记过账的号（初始的 p1/p2）回落当前画布。
  const readings = sessionDocs[id] ?? { width: doc.width, height: doc.height, revision };
  // 预览里摆两条会话，title 和 order 两项才都测得到：改名和拖动排序都指着它们。
  const title = sessionTitles[id] ?? null;
  // 跟着模型簿取 label：设置里改名、换默认模型、删模型，侧栏那一行都要跟着变，
  // 所以这里不能写死某个模型的 label。
  const model = modelView(sessionModels[id]);
  // 单独绑过的角色照绑的那个报，没绑的回落主模型、detached 是 false：
  // 界面上「由 X 跑」和「用主模型 X 跑」是两回事，别让用户以为藏了个模型。
  const roles = (["chat", "image_gen", "vision", "video"] as const).map((role) => ({
    role,
    model_id: modelForRole(id, role).id,
    model_label: modelForRole(id, role).label,
    detached: modelForRole(id, role).id !== model.id,
  }));
  return {
    id,
    model_id: model.id,
    model_label: model.label,
    roles,
    width: readings.width,
    height: readings.height,
    revision: readings.revision,
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
  delete sessionRoles[id];
  // 画布读数也跟着销：留着旧读数的话，同号再建时侧栏会报上一个号的宽高。
  delete sessionDocs[id];
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
/** 编辑器当前停在哪一帧：`document_png_url` 不点名帧时要合成它，跟真机一致。 */
let activeFrameId = doc.frames[0]?.id ?? "F0";
/** 编辑器当前停在哪一层。插帧、量化不点名层时拿它当默认，和 Rust 的
 * `session.active().layer` 同一个来源——预览少记这一笔，坞里「插帧到别的层」
 * 就复现不出来。 */
let activeLayerId = doc.layers[0]?.id ?? "L0";

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
  // 调色板满了真机回 PaletteFull 整笔回滚；mock 里退回最接近的已有色，
  // 比把下标写到 256 之外让整块索引越界要轻。
  if (doc.palette.length >= MAX_PALETTE) {
    const snapped = nearestHex(
      doc.palette.map(rgbaToHex),
      hex,
    );
    const slot =
      snapped === null ? -1 : doc.palette.findIndex((item) => rgbaToHex(item) === snapped);
    return slot >= 0 ? slot + 1 : 0;
  }
  doc.palette.push(color);
  return doc.palette.length;
}

/** 4 邻域浸染：把和起点同索引的一片格子换成新索引，越界就停。
 * 编辑器油漆桶和演示回合的 fill 共用，别写出两份。 */
function floodFill(
  indices: number[],
  width: number,
  height: number,
  sx: number,
  sy: number,
  index: number,
): void {
  if (sx < 0 || sy < 0 || sx >= width || sy >= height) return;
  const target = indices[sy * width + sx];
  if (target === index) return;
  // 入队即标记：同一点只会进栈一次。「填过才跳」的写法里，一个点会被四个
  // 邻居各塞一份才轮到它，线性扫描看着没后果，画布一大就是白扫几倍的量。
  const seen = new Uint8Array(width * height);
  seen[sy * width + sx] = 1;
  const stack: [number, number][] = [[sx, sy]];
  const push = (nx: number, ny: number) => {
    if (nx < 0 || ny < 0 || nx >= width || ny >= height) return;
    const at = ny * width + nx;
    if (seen[at] !== 0 || indices[at] !== target) return;
    seen[at] = 1;
    stack.push([nx, ny]);
  };
  while (stack.length > 0) {
    const point = stack.pop();
    if (!point) break;
    const [x, y] = point;
    if (x < 0 || y < 0 || x >= width || y >= height) continue;
    if (indices[y * width + x] !== target) continue;
    indices[y * width + x] = index;
    push(x - 1, y);
    push(x + 1, y);
    push(x, y - 1);
    push(x, y + 1);
  }
}

/** 演示回合的色值：数字串按 cel 下标（RLE 符号那一套），# 开头的才当颜色字面量。 */
function demoInk(value: unknown): number | null {
  if (typeof value !== "string") return null;
  const text = value.trim();
  if (text.startsWith("#")) {
    const ink = parseHex(text);
    return ink === null ? null : internColor(ink);
  }
  const slot = Number.parseInt(text, 10);
  if (!Number.isFinite(slot) || slot < 1) return null;
  return Math.min(slot, Math.max(doc.palette.length, 1));
}

function demoNumber(item: Record<string, unknown>, key: string, fallback: number): number {
  const value = item[key];
  return typeof value === "number" && Number.isFinite(value) ? Math.round(value) : fallback;
}

/** 演示回合单条像素操作。只认 mock 自己的简化词表，落点、越界、取色和真机同源。 */
function applyDemoOp(layerId: string, item: Record<string, unknown>): void {
  const cel = doc.cels[layerId]?.[activeFrameId];
  if (!cel) return;
  const w = doc.width;
  const h = doc.height;
  const index = demoInk(item.color) ?? 0;
  const x = demoNumber(item, "x", 0);
  const y = demoNumber(item, "y", 0);
  const kind = String(item.op ?? "");
  const put = (px: number, py: number) => {
    if (px < 0 || py < 0 || px >= w || py >= h) return;
    cel.indices[py * w + px] = index;
  };
  switch (kind) {
    case "pixel":
      put(x, y);
      return;
    case "rect":
    case "rectfill": {
      const rw = Math.max(0, demoNumber(item, "w", 1));
      const rh = Math.max(0, demoNumber(item, "h", 1));
      for (let dy = 0; dy < rh; dy += 1) {
        for (let dx = 0; dx < rw; dx += 1) put(x + dx, y + dy);
      }
      return;
    }
    case "frame": {
      const rw = Math.max(1, demoNumber(item, "w", 1));
      const rh = Math.max(1, demoNumber(item, "h", 1));
      for (let dx = 0; dx < rw; dx += 1) {
        put(x + dx, y);
        put(x + dx, y + rh - 1);
      }
      for (let dy = 0; dy < rh; dy += 1) {
        put(x, y + dy);
        put(x + rw - 1, y + dy);
      }
      return;
    }
    case "line": {
      const x1 = demoNumber(item, "x1", x);
      const y1 = demoNumber(item, "y1", y);
      // Bresenham：演示里的斜线也别漏格。
      let cx = x;
      let cy = y;
      const dx = Math.abs(x1 - x);
      const dy = Math.abs(y1 - y);
      const sx = x < x1 ? 1 : -1;
      const sy = y < y1 ? 1 : -1;
      let err = dx - dy;
      for (let guard = 0; guard <= dx + dy + 1; guard += 1) {
        put(cx, cy);
        if (cx === x1 && cy === y1) break;
        const e2 = 2 * err;
        if (e2 > -dy) {
          err -= dy;
          cx += sx;
        }
        if (e2 < dx) {
          err += dx;
          cy += sy;
        }
      }
      return;
    }
    case "fill":
      floodFill(cel.indices, w, h, x, y, index);
      return;
    case "clear": {
      const rw = Math.max(0, demoNumber(item, "w", 1));
      const rh = Math.max(0, demoNumber(item, "h", 1));
      for (let dy = 0; dy < rh; dy += 1) {
        for (let dx = 0; dx < rw; dx += 1) put(x + dx, y + dy);
      }
      return;
    }
    default:
      return;
  }
}

/** 演示回合的一整批图层操作：逐层逐条落笔，最后广播一次。 */
function applyDemoLayers(layers: readonly Record<string, unknown>[]): void {
  if (doc.layers.length === 0) return;
  for (const entry of layers) {
    const name = typeof entry.name === "string" ? entry.name : activeLayerId;
    // 点名一个不存在的层，真机整批回滚；预览里就近跳过，别把别的层的字擦了。
    if (!doc.cels[name]) continue;
    const ops = Array.isArray(entry.operations) ? entry.operations : [];
    for (const raw of ops) {
      applyDemoOp(name, (raw ?? {}) as Record<string, unknown>);
    }
  }
  revision += 1;
  broadcast();
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

/** 预览版的量化选项默认值，和 Rust `PixelizeOptions::default()` 一致。 */
const DEFAULT_QUANTIZE: PixelizeOptions = {
  max_colors: 32,
  snap_tolerance: 12,
  expand_palette: true,
  dither: false,
  alpha_threshold: 128,
  fit: "contain",
};

/** ---------- 预览里的生图链路 ----------
 *
 * 真机这一段是：模型回一张位图 -> pixel_core 解码 -> pixelize 量化落 cel -> 广播
 * document_updated。预览里没有真端点，但链路要真：按提示词合成一张确定性位图，
 * 再照 pixelize 的规矩量化落盘。少了这一段，浏览器里点「生成」只会弹一句
 * 「没有这条假数据」，生图链路通不通永远看不出来。 */

/** 同一句提示词必须落到同一张图，否则没法对比量化选项换来的差别。 */
function hash32(text: string): number {
  let h = 2166136261;
  for (let i = 0; i < text.length; i += 1) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** 由种子驱动的确定性伪随机数：步进顺序固定，与调用次数无关。 */
function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** hsl -> rgb，h 取值随意（自动归到 0..360）。只给合成图用，不上界面配色。 */
function hslToRgb(h: number, s: number, l: number): [number, number, number] {
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const hp = (((h % 360) + 360) % 360) / 60;
  const x = c * (1 - Math.abs((hp % 2) - 1));
  const rgb: [number, number, number] =
    hp < 1
      ? [c, x, 0]
      : hp < 2
        ? [x, c, 0]
        : hp < 3
          ? [0, c, x]
          : hp < 4
            ? [0, x, c]
            : hp < 5
              ? [x, 0, c]
              : [c, 0, x];
  const m = l - c / 2;
  return [
    Math.round((rgb[0] + m) * 255),
    Math.round((rgb[1] + m) * 255),
    Math.round((rgb[2] + m) * 255),
  ];
}

function mixRgb(
  a: [number, number, number],
  b: [number, number, number],
  k: number,
): [number, number, number] {
  const w = Math.max(0, Math.min(1, k));
  return [a[0] + (b[0] - a[0]) * w, a[1] + (b[1] - a[1]) * w, a[2] + (b[2] - a[2]) * w];
}

/** 逐像素噪声：给地面铺一层颗粒，纯色块量化之后看不出源图尺寸变化。 */
function grainAt(x: number, y: number, salt: number): number {
  const h = Math.imul(x * 374761393 + y * 668265263 + salt * 2246822519, 1274126177);
  return ((h ^ (h >>> 15)) & 0xff) / 255;
}

/** 合成图的上限：够量化就行，别在浏览器里为一张没人细看的图烧几百万像素。 */
const SYNTH_MAX = 256;

/** 面板发来的 "WxH"；解析不出来就跟着画布走，和真机忽略 size 一样。 */
function synthSize(size: string | null | undefined): { w: number; h: number } {
  const hit = /^\s*(\d{1,5})\s*[x×*]\s*(\d{1,5})\s*$/i.exec(size ?? "");
  const w = hit ? Number(hit[1]) : doc.width;
  const h = hit ? Number(hit[2]) : doc.height;
  const scale = SYNTH_MAX / Math.max(w, h, 1);
  return {
    w: Math.max(1, Math.round(w * Math.min(1, scale))),
    h: Math.max(1, Math.round(h * Math.min(1, scale))),
  };
}

/** 一张「天空 + 地面 + 主体剪影」的小景，构图随提示词散列摆动。 */
function synthBitmap(prompt: string, w: number, h: number): Uint8ClampedArray {
  const rand = mulberry32(hash32(prompt));
  const sky = hslToRgb(rand() * 360, 0.5, 0.7);
  const ground = hslToRgb(rand() * 360, 0.45, 0.3);
  const hero = hslToRgb(rand() * 360, 0.75, 0.55);
  const horizon = Math.max(2, Math.round(h * (0.4 + rand() * 0.25)));
  const sun = {
    x: Math.round(w * (0.15 + rand() * 0.7)),
    y: Math.round(h * (0.1 + rand() * 0.25)),
    r: Math.max(3, Math.round(Math.min(w, h) * 0.09)),
  };
  const box = {
    x: Math.round(w * (0.3 + rand() * 0.3)),
    w: Math.max(4, Math.round(w * 0.18)),
    h: Math.max(4, Math.round(h * 0.28)),
  };
  const darker = (t: [number, number, number]): [number, number, number] => [
    t[0] * 0.55,
    t[1] * 0.55,
    t[2] * 0.7,
  ];
  const salt = hash32(prompt) & 0xff;
  const buf = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y += 1) {
    for (let x = 0; x < w; x += 1) {
      let rgb = y < horizon ? mixRgb(sky, darker(sky), (y / horizon) * 0.55) : ground;
      if (Math.hypot(x - sun.x, (y - sun.y) * 1.3) < sun.r) {
        rgb = mixRgb(rgb, [255, 248, 214], 0.85);
      }
      if (x >= box.x && x < box.x + box.w && y >= horizon - box.h && y < horizon + box.h * 0.4) {
        rgb = mixRgb(hero, darker(hero), (y - (horizon - box.h)) / box.h);
      }
      const grain = (grainAt(x, y, salt) - 0.5) * 26;
      const o = (y * w + x) * 4;
      buf[o] = rgb[0] + grain;
      buf[o + 1] = rgb[1] + grain;
      buf[o + 2] = rgb[2] + grain;
      buf[o + 3] = 255;
    }
  }
  return buf;
}

/** redmean 加权欧氏距离：Rust 的 color_distance 在预览里的同一份实现。 */
function colorDistance(a: Rgba, b: Rgba): number {
  const rmean = (a.r + b.r) / 2;
  const dr = a.r - b.r;
  const dg = a.g - b.g;
  const db = a.b - b.b;
  return (((512 + rmean) * dr * dr) / 256) + 4 * dg * dg + (((767 - rmean) * db * db) / 256);
}

/** 最近两档的下标。空调调色板返回两个 -1，调用方得自己补色。 */
function twoNearest(palette: Rgba[], color: Rgba): [number, number] {
  let best = -1;
  let second = -1;
  let bestD = Number.POSITIVE_INFINITY;
  let secondD = Number.POSITIVE_INFINITY;
  for (let i = 0; i < palette.length; i += 1) {
    const d = colorDistance(palette[i], color);
    if (d < bestD) {
      second = best;
      secondD = bestD;
      best = i;
      bestD = d;
    } else if (d < secondD) {
      second = i;
      secondD = d;
    }
  }
  return [best, second];
}

const BAYER4 = [
  [0, 8, 2, 10],
  [12, 4, 14, 6],
  [3, 11, 1, 9],
  [15, 7, 13, 5],
];

/** 层的配色锁：范围内的原样，范围外就近归队（对齐 color_for_layer）。 */
function colorForLayer(layerId: string, color: Rgba): Rgba {
  const layer = doc.layers.find((item) => item.id === layerId);
  if (!layer?.locked) return color;
  const range = doc.palettes.find((palette) => palette.id === layer.palette_id)?.colors ?? [];
  if (range.length === 0) return color;
  if (range.some((item) => rgbaToHex(item) === rgbaToHex(color))) return color;
  const snapped = nearestHex(range.map(rgbaToHex), rgbaToHex(color));
  return snapped ? (parseHex(snapped) ?? color) : color;
}

interface BitmapTarget {
  layer: string;
  frame: string;
  options: PixelizeOptions;
}

/**
 * 位图量化落 cel，对齐 `pixel_core::pixelize::pixelize_into_cel`：
 * 逐格取覆盖块平均色 -> 分桶取主色（预览版的中位切分）-> 按 snap 容差决定是否
 * 扩文档调色板 -> 每格取最近档（dither 开时两档间抖）-> 层上锁就收进这一层的
 * 配色范围。返回的统计数直接进 outcome，前端通知栏照着念。
 */
function landBitmap(
  rgba: Uint8ClampedArray,
  srcW: number,
  srcH: number,
  target: BitmapTarget,
): { colors_used: number; palette_added: number } {
  const layer = doc.layers.find((item) => item.id === target.layer);
  if (!layer) throw new Error(`unknown layer: ${target.layer}`);
  const cel = doc.cels[target.layer]?.[target.frame];
  if (!cel) throw new Error(`unknown cel: ${target.layer}/${target.frame}`);
  const opts = target.options;
  const dw = doc.width;
  const dh = doc.height;
  // 目标矩形：contain 保比例居中，stretch 忽略宽高比铺满。
  let destX = 0;
  let destY = 0;
  let destW = dw;
  let destH = dh;
  if (opts.fit === "contain") {
    const scale = Math.min(dw / srcW, dh / srcH);
    destW = Math.max(1, Math.round(srcW * scale));
    destH = Math.max(1, Math.round(srcH * scale));
    destX = Math.floor((dw - destW) / 2);
    destY = Math.floor((dh - destH) / 2);
  }
  // 每个目标格先取覆盖块的平均色，整块透明的算透。
  const cells: (Rgba | null)[] = new Array(dw * dh).fill(null);
  for (let y = 0; y < destH; y += 1) {
    for (let x = 0; x < destW; x += 1) {
      const sx0 = Math.floor((x * srcW) / destW);
      const sx1 = Math.max(sx0 + 1, Math.floor(((x + 1) * srcW) / destW));
      const sy0 = Math.floor((y * srcH) / destH);
      const sy1 = Math.max(sy0 + 1, Math.floor(((y + 1) * srcH) / destH));
      let r = 0;
      let g = 0;
      let b = 0;
      let a = 0;
      let n = 0;
      for (let sy = sy0; sy < Math.min(sy1, srcH); sy += 1) {
        for (let sx = sx0; sx < Math.min(sx1, srcW); sx += 1) {
          const o = (sy * srcW + sx) * 4;
          r += rgba[o];
          g += rgba[o + 1];
          b += rgba[o + 2];
          a += rgba[o + 3];
          n += 1;
        }
      }
      if (n === 0) continue;
      cells[(y + destY) * dw + (x + destX)] = {
        r: r / n,
        g: g / n,
        b: b / n,
        a: a / n,
      };
    }
  }
  // 主色：每通道取高 4 位分桶，留样本最多的几桶。
  const buckets = new Map<number, Rgba>();
  const counts = new Map<number, number>();
  for (const cell of cells) {
    if (!cell || cell.a < opts.alpha_threshold) continue;
    const key = ((cell.r >> 4) << 8) | ((cell.g >> 4) << 4) | (cell.b >> 4);
    const seen = counts.get(key) ?? 0;
    counts.set(key, seen + 1);
    if (seen === 0) buckets.set(key, { ...cell });
  }
  const mains = [...counts.entries()]
    .sort((l, r) => r[1] - l[1])
    .slice(0, Math.max(1, opts.max_colors))
    .map(([key]) => buckets.get(key))
    .filter((color): color is Rgba => color !== undefined);
  // 目标调色板：文档已有色打底；容差内的近色不新增，不准扩张也一律归到旧色上。
  const palette: Rgba[] = doc.palette.map((color) => ({ ...color }));
  for (const main of mains) {
    const [nearest] = twoNearest(palette, main);
    if (nearest >= 0 && colorDistance(palette[nearest], main) <= opts.snap_tolerance) continue;
    if (!opts.expand_palette) continue;
    palette.push(main);
  }
  const before = doc.palette.length;
  const used = new Set<number>();
  for (let y = 0; y < dh; y += 1) {
    for (let x = 0; x < dw; x += 1) {
      const cell = cells[y * dw + x];
      if (!cell || cell.a < opts.alpha_threshold) {
        cel.indices[y * dw + x] = 0;
        continue;
      }
      if (layer.locked) {
        // 上锁的层不走目标调色板：先收进范围，再由 internColor 落进文档调色板。
        const index = internColor(colorForLayer(layer.id, cell));
        cel.indices[y * dw + x] = index;
        used.add(index);
        continue;
      }
      const [best, second] = twoNearest(palette, cell);
      let chosen = best;
      // dither：真实颜色按 best/(best+second) 的比例落第二档，差得越悬殊越不动。
      if (opts.dither && second >= 0 && second !== best) {
        const bestD = colorDistance(palette[best], cell);
        const secondD = colorDistance(palette[second], cell);
        const weight = bestD / Math.max(1, bestD + secondD);
        const gate = (BAYER4[y % 4][x % 4] + 0.5) / 16;
        if (gate < weight) chosen = second;
      }
      const index = internColor(palette[chosen] ?? cell);
      cel.indices[y * dw + x] = index;
      used.add(index);
    }
  }
  return {
    colors_used: used.size,
    palette_added: Math.max(0, doc.palette.length - before),
  };
}

/** 新建一帧再落图：和 Rust 的 LandSpot::NewFrame 一样，之后把激活帧挪过去。 */
function appendFrame(durationMs: number): string {
  const frameId = nextId("F", doc.frames.map((frame) => frame.id));
  const at = doc.frames.findIndex((frame) => frame.id === activeFrameId) + 1;
  doc.frames.splice(at < 0 ? doc.frames.length : at, 0, {
    id: frameId,
    duration_ms: durationMs,
  });
  for (const layer of doc.layers) {
    doc.cels[layer.id] ??= {};
    doc.cels[layer.id][frameId] = {
      indices: new Array(doc.width * doc.height).fill(0),
    };
  }
  activeFrameId = frameId;
  return frameId;
}

interface ImageGenRequest {
  prompt?: string;
  size?: string | null;
  // 画风与收尾预设：真机在 Rust 的 pins/plan 那一份挂到提示词上（让位、顶替、
  // 报错语义都在那里）。预览不真调模型，合成位图也不看这两项，所以这里只记形状——
  // 写成可选是为了让 store 发的对象能原样进来，验证的是「链路通不通」。
  style?: string | null;
  presets?: string[] | null;
  options?: PixelizeOptions | null;
  layer?: string | null;
  spot?: LandSpot | null;
  duration_ms?: number;
}

/** 生图命令的预览实现：两段进度、一帧合成、一次量化、一次广播。 */
function runImageGen(payload: ImageGenRequest): Promise<unknown> {
  const gen = payload ?? {};
  // 生图这条流程找 image_gen 角色要模型：单独绑了就用它，没绑才回落会话主模型。
  // 预览里少了这层回落，把生图模型分出去之后坞子绿了、跑起来还是报不会生图。
  const sessionModel = modelForRole(ownerSession, "image_gen");
  // 不会生图的模型照真机那句话回：预览里也得看得见这条报错长什么样。
  if (!sessionModel.capabilities.image_gen) {
    return Promise.reject(
      new Error(
        "this model is not marked as able to generate images; pick an image model for this session or turn on image generation in model settings",
      ),
    );
  }
  const prompt = (gen.prompt ?? "").trim();
  if (prompt === "") return Promise.reject(new Error("image generation needs a prompt"));
  fire(AGENT_EVENT_CHANNEL, {
    kind: "status",
    message: { key: "status.asking_image", fallback: "asking the model for an image" },
  });
  return new Promise((resolve) => {
    setTimeout(() => {
      const size = synthSize(gen.size);
      const bitmap = synthBitmap(prompt, size.w, size.h);
      const frameId = gen.spot === "new_frame" ? appendFrame(gen.duration_ms ?? 83) : activeFrameId;
      // 默认层取当前激活层，和 Rust 的 `active.layer` 同一个来源。
      // 点名了不存在的层要报错而不是悄悄换一层——真机在 land_image 入口就挡了。
      let layerId = activeLayerId;
      if (gen.layer) {
        if (!doc.layers.some((item) => item.id === gen.layer)) {
          throw new Error(`unknown layer: ${gen.layer}`);
        }
        layerId = gen.layer;
      }
      fire(AGENT_EVENT_CHANNEL, {
        kind: "status",
        message: {
          key: "status.quantizing",
          fallback: "quantizing the generated image onto the grid",
        },
      });
      const report = landBitmap(bitmap, size.w, size.h, {
        layer: layerId,
        frame: frameId,
        options: gen.options ?? DEFAULT_QUANTIZE,
      });
      revision += 1;
      broadcast();
      resolve({
        revision,
        summary: {
          key: "outcome.bitmap_landed",
          vars: {
            transport: "images",
            layer: layerId,
            frame: frameId,
            colors: report.colors_used,
            added: report.palette_added,
          },
          fallback:
            "images landed on layer {layer} frame {frame} ({colors} colors, {added} new)",
        },
        detail: {
          transport: "images",
          layer: layerId,
          frame: frameId,
          colors_used: report.colors_used,
          palette_added: report.palette_added,
        },
      });
    }, 280);
  });
}

let revision = 1;
let LIMITS: LoopLimits = {
  max_continuations: 20,
  max_retries: 5,
  max_reasoning_continuations: 2,
  max_tool_steps: 0,
  max_turns: 0,
};

/** ---------- 预览里的文件类工作流 ----------
 *
 * 真机这一段是：对话框选路径 -> Rust 读文件 ->（要模型的问模型）-> 量化或广播。
 * 预览没有磁盘也没有端点，但链路要真：路径按过滤器合成，位图按路径合成，
 * 之后每一步都复用上面的 landBitmap / internColor / broadcast。
 * 少了这一段，坞里六个面板一半一按就是「没有这条假数据」，通不通永远看不出来。 */

/** 单次插帧上限，同 tween.rs 的 MAX_TWEEN_FRAMES。 */
const MAX_TWEEN_FRAMES = 64;
/** 单次抽帧上限，同 agent-core video.rs 的 MAX_EXTRACT_FRAMES。 */
const MAX_EXTRACT_FRAMES = 256;
/** 参考图的最长边：够大看得出量化的差别，又不至于让预览卡在编解码上。 */
const REFERENCE_MAX = 256;

function smoothstep(t: number): number {
  return t * t * (3 - 2 * t);
}

/** 帧时长夹在 Rust 认的区间里：面板填 0 或超大值都该被挡下，
 * 而不是造出一份非法文档让后面的代码跟着遭殃。 */
function clampDuration(ms: number | undefined): number {
  return Math.max(1, Math.min(60000, Math.round(Number(ms ?? 83))));
}

function rgbHex(r: number, g: number, b: number): string {
  const part = (value: number) =>
    Math.max(0, Math.min(255, Math.round(value))).toString(16).padStart(2, "0");
  return `#${part(r)}${part(g)}${part(b)}`;
}

/** 由亮到暗的一串 hex。色板要递给人也递给模型当明暗信息，顺序不能倒。 */
function rampHexes(rand: () => number, steps: number): string[] {
  const hue = Math.floor(rand() * 360);
  const saturation = 0.55 + rand() * 0.3;
  return Array.from({ length: steps }, (_, index) => {
    const [r, g, b] = hslToRgb(hue, saturation, 0.9 - (index / Math.max(1, steps - 1)) * 0.68);
    return rgbHex(r, g, b);
  });
}

/** 路径去掉目录与后缀：简报里要点名这张参考图是什么。 */
function baseNameOf(path: string): string {
  const tail = path.split(/[/\\]/).filter(Boolean).pop() ?? "reference";
  return tail.replace(/\.[a-z0-9]+$/i, "") || "reference";
}

/** 对话框：预览没有真文件系统，按请求的后缀合成一个确定路径。
 * multiple 要数组、单选要单串：两条调用链各自这么断言。 */
function synthDialogPath(raw?: unknown): unknown {
  const options = (
    raw as
      | { options?: { multiple?: unknown; directory?: unknown; filters?: { extensions?: string[] }[] } }
      | undefined
  )?.options ?? {};
  if (options.directory) return "/tmp/aipixel-preview/stills";
  const extensions = (options.filters ?? []).flatMap((filter) => filter.extensions ?? []);
  const extension = extensions[0] ?? "png";
  // 后缀进 seed：图片、视频两条路拿到的是不同的文件，简报内容才不一样。
  const name = `preview-${(hash32(extensions.join(",")) & 0xffff).toString(16).padStart(4, "0")}.${extension}`;
  const path = `/tmp/aipixel-preview/${name}`;
  return options.multiple ? [path] : path;
}

/** 参考图的合成尺寸：按画布宽高比放大，量化时才看得出 contain 的效果。 */
function referenceSize(): { w: number; h: number } {
  const longest = Math.max(doc.width, doc.height, 1);
  const scale = Math.min(4, REFERENCE_MAX / longest);
  return {
    w: Math.max(8, Math.round(doc.width * scale)),
    h: Math.max(8, Math.round(doc.height * scale)),
  };
}

/** 位图过一遍浏览器自带的编码器出 base64（不带 data: 前缀）。
 * 预览里不引 png 编码库：canvas 就是那份编码器，导入链路本来也要用它解。 */
function pngBase64(rgba: Uint8ClampedArray, width: number, height: number): string {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("this browser has no 2d canvas context");
  // 走 createImageData + set 而不是 new ImageData：
  // createImageData 给的就是这块画布的缓冲，set 一份进去，不用管 ArrayBuffer 的版本差异。
  const image = ctx.createImageData(width, height);
  image.data.set(rgba);
  ctx.putImageData(image, 0, 0);
  const url = canvas.toDataURL("image/png");
  return url.slice(url.indexOf(",") + 1);
}

/** data: 前缀与裸 base64 都吃，解回 RGBA。量化那条链路的前半段：
 * 真机由 Rust 的 decode 做，这里交给浏览器，两条路都不猜格式。 */
async function decodeBase64(
  image: string,
  mediaType: string | null | undefined,
): Promise<{ rgba: Uint8ClampedArray; width: number; height: number }> {
  const source = image.startsWith("data:")
    ? image
    : `data:${mediaType ?? "image/png"};base64,${image}`;
  const blob = await (await fetch(source)).blob();
  const bitmap = await createImageBitmap(blob);
  const canvas = document.createElement("canvas");
  canvas.width = bitmap.width;
  canvas.height = bitmap.height;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("this browser has no 2d canvas context");
  ctx.drawImage(bitmap, 0, 0);
  const data = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
  bitmap.close();
  return { rgba: data.data, width: data.width, height: data.height };
}

/** 参考图：按路径合成一张确定性位图再编码。形状和真机的 read_reference 一致
 * （role / media_type / data_base64），简报、量化、聊天附件三条链路都接着走。 */
function referenceAttachment(path: string): Attachment {
  const size = referenceSize();
  const bitmap = synthBitmap(`reference:${path}`, size.w, size.h);
  return {
    role: "reference",
    media_type: "image/png",
    data_base64: pngBase64(bitmap, size.w, size.h),
  };
}

/** 素材探针：按路径散列确定值。看着像目录的给 directory 结论——
 * 前端 probeSummary 为目录来源单独一行文案，这一支得能走到。 */
function probeFor(path: string): VideoProbeResult {
  const rand = mulberry32(hash32(`probe:${path}`) ^ 0x51ed);
  const width = 320 + Math.floor(rand() * 5) * 160;
  const height = Math.round((width * 9) / 16);
  if (!/\.[a-z0-9]{2,5}$/i.test(path)) {
    return {
      probe: {
        width,
        height,
        duration_s: null,
        fps: null,
        frame_count: 4 + Math.floor(rand() * 9),
        codec: null,
        has_audio: false,
      },
      source: "directory",
    };
  }
  const fps = [24, 25, 30, 60][Math.floor(rand() * 4)];
  const duration = Math.round((1 + rand() * 9) * 10) / 10;
  return {
    probe: {
      width,
      height,
      duration_s: duration,
      fps,
      frame_count: Math.round(fps * duration),
      codec: "h264",
      has_audio: rand() > 0.5,
    },
    source: "ffprobe",
  };
}

/** 参考图简报：vision.rs 要的六个字段一个不少。预览要验的是「字段齐、
 * 色板是 hex、BriefView 与 briefToText 都不白屏」。 */
function referenceBrief(path: string): VisionBrief {
  const rand = mulberry32(hash32(`brief:${path}`));
  const subject = baseNameOf(path);
  return {
    subject: `${subject}: one centred figure, full body, three-quarter view`,
    silhouette: "rounded head over a tapered torso, two legs, no inner holes",
    palette: rampHexes(rand, 5),
    pose_notes: "weight on the back leg, front paw raised, facing canvas right",
    proportions: "head takes about a third of the height; limbs two cells wide",
    craft_notes: "one-pixel outline, five-step value ramp, no anti-aliasing",
    raw: `${subject} read as a reference for a ${doc.width}x${doc.height} canvas`,
  };
}

/** 运动简报：这边全是运动语义，没有剪影和比例，字段名和 video_brief.rs 对齐。 */
function motionBrief(path: string, poseCount: number): VideoBrief {
  const rand = mulberry32(hash32(`motion:${path}`));
  const subject = baseNameOf(path);
  const poses = ["contact", "passing", "extreme", "passing back", "contact again"];
  return {
    subject: `${subject}: one figure moving across the frame`,
    motion: "four-beat walk cycle; hips lead, shoulders counter-rotate",
    key_poses: poses.slice(0, Math.max(2, Math.min(poses.length, poseCount))),
    timing: "contact frames hold longest; passing frames are the fastest",
    palette: rampHexes(rand, 4),
    craft_notes: "keep the silhouette steady; only the limbs re-block",
    raw: `${poseCount} frame(s) sampled from ${subject}`,
  };
}

/** 提示词微调：按 refine.rs 要的九行标签产确定性内容。预览要验的是「九行齐、
 * 顺序对、色板由亮到暗」——RefinePanel 是逐行给人改的，少一行编辑框就是空的。 */
function refineIdea(idea: string, target: RefineTarget | undefined): RefinedPrompt {
  const rand = mulberry32(hash32(`refine:${idea}:${target ?? "shader"}`));
  const trimmed = idea.trim().replace(/\s+/g, " ");
  const slug = trimmed === "" ? "the subject" : trimmed.slice(0, 60);
  const lines = [
    `subject: ${slug}, centred on the ${doc.width}x${doc.height} canvas`,
    "silhouette: one readable mass with a clear gap from the background",
    `palette: ${rampHexes(rand, 5).join(", ")}`,
    "lighting: single key light from the upper left, shadow falls to the lower right",
    "pose: standing, facing the viewer, weight even, focal point on the head",
    `proportions: head about a third of the ${doc.height} rows, limbs two cells wide`,
    "detail: keep the eyes and the outline; drop textures and small props",
    "outline: one-pixel dark outline around every shape, no selective outlining",
    "constraints: no gradients, no anti-aliasing, no more than two dither patterns",
  ];
  const prompt = lines.join("\n");
  // raw 是模型原文：预览里就当模型一字不差地照格式答了。
  return { prompt, raw: prompt };
}

/** 接下来 count 个没用过的帧 id。先一次算完：帧还没插进文档，
 * 边插边算会以刚生成的那个当基准，号就跳着走。 */
function freshFrameIds(count: number): string[] {
  const taken = new Set(doc.frames.map((frame) => frame.id));
  let base = 0;
  for (const frame of doc.frames) {
    if (!frame.id.startsWith("F")) continue;
    const value = Number(frame.id.slice(1));
    if (Number.isInteger(value)) base = Math.max(base, value);
  }
  const ids: string[] = [];
  for (let offset = 0; offset < count; offset += 1) {
    let value = base + 1 + offset;
    while (taken.has(`F${value}`)) value += 1;
    taken.add(`F${value}`);
    ids.push(`F${value}`);
  }
  return ids;
}

/** 新建一帧插在 after 之后，每层补一张空 cel。 */
function insertFrameAfter(after: string, id: string, durationMs: number): void {
  const at = doc.frames.findIndex((frame) => frame.id === after) + 1;
  doc.frames.splice(at < 0 ? doc.frames.length : at, 0, { id, duration_ms: durationMs });
  for (const layer of doc.layers) {
    doc.cels[layer.id] ??= {};
    doc.cels[layer.id][id] = { indices: new Array(doc.width * doc.height).fill(0) };
  }
  activeFrameId = id;
}

/** 非预乘 rgba 插值：透明像素的 rgb 当 0，与不透明色混会自然得到半透明。 */
function blendRgba(a: Rgba, b: Rgba, t: number): Rgba {
  const lerp = (x: number, y: number) => Math.round(x + (y - x) * t);
  return { r: lerp(a.r, b.r), g: lerp(a.g, b.g), b: lerp(a.b, b.b), a: lerp(a.a, b.a) };
}

/** 确定性散列：同一格永远落到同一个键，翻面顺序因此可复现。 */
function scatterKey(x: number, y: number): number {
  let h = Math.imul(x + 1, 0x9e3779b1) ^ Math.imul(y + 1, 0xbf58476d);
  h ^= h >>> 15;
  h = Math.imul(h, 0xbf58476d);
  h ^= h >>> 13;
  return h >>> 0;
}

/** 翻面顺序：scan 保持扫描序，scatter 像溶解，radial 从变化区域的质心往外扩。 */
function orderPositions(
  positions: [number, number][],
  order: MigrateOrder | undefined,
): [number, number][] {
  const list = positions.slice();
  if (order === "scatter") {
    list.sort((l, r) => scatterKey(l[0], l[1]) - scatterKey(r[0], r[1]));
    return list;
  }
  if (order === "radial") {
    const count = Math.max(1, list.length);
    const cx = list.reduce((sum, [x]) => sum + x, 0) / count;
    const cy = list.reduce((sum, [, y]) => sum + y, 0) / count;
    const radius = ([x, y]: [number, number]) => (x - cx) ** 2 + (y - cy) ** 2;
    list.sort((l, r) => radius(l) - radius(r));
  }
  return list;
}

/** 插帧：tween.rs 的三种模式逐个对着写。新帧插在 to_frame 之前，
 * 所以 to 帧一个像素都不会被破坏。 */
function runTween(params: TweenParams): WorkflowOutcome {
  const fail = (message: string): never => {
    throw new Error(`tween failed: ${message}`);
  };
  const count = Math.round(Number(params.count ?? 4));
  if (!Number.isFinite(count) || count < 1) fail("tween needs at least 1 frame");
  if (count > MAX_TWEEN_FRAMES) {
    fail(`tween asked for ${count} frames; the limit is ${MAX_TWEEN_FRAMES}`);
  }
  const layerId = params.layer ?? activeLayerId;
  const frames = doc.cels[layerId];
  const from = params.from_frame;
  const to = params.to_frame;
  if (!frames?.[from]) fail(`unknown layer/frame: ${layerId}/${from}`);
  if (!frames?.[to]) fail(`unknown layer/frame: ${layerId}/${to}`);
  if (from === to) fail("from_frame and to_frame must differ");
  const mode = params.mode ?? "migrate";
  const ease = params.ease ?? true;
  const durationMs = clampDuration(params.duration_ms);

  const fromSnap = frames[from].indices.slice();
  const toSnap = frames[to].indices.slice();
  const changed: [number, number][] = [];
  for (let y = 0; y < doc.height; y += 1) {
    for (let x = 0; x < doc.width; x += 1) {
      if (fromSnap[y * doc.width + x] !== toSnap[y * doc.width + x]) changed.push([x, y]);
    }
  }

  // 先一次把号算完，再按顺序插——from 和 to 之间的那 count 个就是新建出来的。
  const inserted = freshFrameIds(count);
  let after = from;
  for (const id of inserted) {
    insertFrameAfter(after, id, durationMs);
    after = id;
  }

  const positions = mode === "migrate" ? orderPositions(changed, params.order) : [];
  let paletteAdded = 0;
  inserted.forEach((id, step) => {
    const cel = doc.cels[layerId][id];
    const t = (step + 1) / (count + 1);
    const eased = ease ? smoothstep(t) : t;
    if (mode === "copy") {
      cel.indices = fromSnap.slice();
      return;
    }
    if (mode === "blend") {
      cel.indices = fromSnap.map((a, index) => {
        const b = toSnap[index];
        if (a === b) return a;
        const ca = doc.palette[a - 1];
        const cb = doc.palette[b - 1];
        // 调色板越界时留着起点色：真机会报错，预览里让这一格不动比整批失败好看。
        if (!ca || !cb) return a;
        const before = doc.palette.length;
        const slot = internColor(blendRgba(ca, cb, eased));
        if (doc.palette.length !== before) paletteAdded += 1;
        return slot;
      });
      return;
    }
    const flip = Math.round(positions.length * eased);
    cel.indices = fromSnap.slice();
    for (let i = 0; i < flip; i += 1) {
      const [x, y] = positions[i];
      cel.indices[y * doc.width + x] = toSnap[y * doc.width + x];
    }
  });

  revision += 1;
  broadcast();
  return {
    revision,
    summary: {
      key: "outcome.tween_inserted",
      vars: {
        count: inserted.length,
        from,
        to,
        layer: layerId,
        changed: changed.length,
      },
      fallback:
        "{count} frame(s) inserted between {from} and {to} on layer {layer}; {changed} px changed",
    },
    detail: {
      created: inserted,
      changed_pixels: changed.length,
      palette_added: paletteAdded,
      layer: layerId,
      mode,
    },
  };
}

/** 量化：把用户给的位图解回 RGBA，落到点名（或当前）的 cel 上。
 * 和真机一样是本机计算，这里只因为要先解码才返回 Promise。 */
async function runPixelize(params: PixelizeParams): Promise<WorkflowOutcome> {
  const image = (params.image_base64 ?? "").trim();
  if (image === "") throw new Error("pixelize needs an image");
  const layerId = params.layer ?? activeLayerId;
  const frameId = params.frame ?? activeFrameId;
  if (!doc.layers.some((layer) => layer.id === layerId)) {
    throw new Error(`unknown layer: ${layerId}`);
  }
  if (!doc.cels[layerId]?.[frameId]) {
    throw new Error(`unknown cel: ${layerId}/${frameId}`);
  }
  const decoded = await decodeBase64(image, params.media_type);
  const options = params.options ?? DEFAULT_QUANTIZE;
  const report = landBitmap(decoded.rgba, decoded.width, decoded.height, {
    layer: layerId,
    frame: frameId,
    options,
  });
  revision += 1;
  broadcast();
  return {
    revision,
    summary: {
      key: "outcome.pixelize_landed",
      vars: {
        layer: layerId,
        frame: frameId,
        colors: report.colors_used,
        added: report.palette_added,
      },
      fallback: "quantized onto layer {layer} frame {frame} ({colors} colors, {added} new)",
    },
    detail: {
      layer: layerId,
      frame: frameId,
      colors_used: report.colors_used,
      palette_added: report.palette_added,
      fit: options.fit,
    },
  };
}

/** 抽帧：按路径合成 count 帧，逐帧量化到各自的新帧上。
 * 状态事件照 Rust 那句「正在读取第 x 帧」发——坞顶的状态条要能走到。 */
async function runVideoFrames(params: VideoFramesParams): Promise<WorkflowOutcome> {
  const path = (params.path ?? "").trim();
  if (path === "") throw new Error("video frames need a path");
  const probe = probeFor(path);
  const wanted = Math.max(0, Math.round(Number(params.count ?? 0)));
  const total = Math.min(wanted === 0 ? 3 + (hash32(path) % 3) : wanted, MAX_EXTRACT_FRAMES);
  if (total === 0) throw new Error("no frames came out of that source");
  const durationMs = clampDuration(params.duration_ms);
  const options = params.options ?? DEFAULT_QUANTIZE;
  const layerId = activeLayerId;
  const sourceWidth = Math.min(512, probe.probe.width ?? 256);
  const sourceHeight = Math.max(1, Math.round((sourceWidth * 9) / 16));
  let after: string | null = null;
  const landed: { frame: string; colors_used: number; palette_added: number }[] = [];
  for (let index = 0; index < total; index += 1) {
    fire(AGENT_EVENT_CHANNEL, {
      kind: "status",
      message: {
        key: "status.reading_frame",
        vars: { index: index + 1, total },
        fallback: "reading frame {index} of {total}",
      },
    });
    // 第一帧跟在当前帧后面（appendFrame 内部按活跃帧定位），
    // 其后每帧接在前一帧后面——和真机的 after 链一致。
    let frameId: string;
    if (after === null) {
      frameId = appendFrame(durationMs);
    } else {
      frameId = nextId("F", doc.frames.map((frame) => frame.id));
      insertFrameAfter(after, frameId, durationMs);
    }
    after = frameId;
    const bitmap = synthBitmap(`${path}#${index}`, sourceWidth, sourceHeight);
    const report = landBitmap(bitmap, sourceWidth, sourceHeight, {
      layer: layerId,
      frame: frameId,
      options,
    });
    landed.push({
      frame: frameId,
      colors_used: report.colors_used,
      palette_added: report.palette_added,
    });
  }
  revision += 1;
  broadcast();
  return {
    revision,
    summary: {
      key: "outcome.video_landed.none",
      vars: {
        count: landed.length,
        layer: layerId,
        from: landed[0].frame,
        to: landed[landed.length - 1].frame,
      },
      fallback: "{count} frame(s) landed on layer {layer} frames {from}-{to}",
    },
    detail: {
      source: baseNameOf(path),
      frames: landed.map((one) => one.frame),
      colors_used: landed.map((one) => one.colors_used),
      palette_added: landed.map((one) => one.palette_added),
    },
  };
}

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

/** 服务端那块的预览态。真机上端口是内核挑的，浏览器里挑不了，
 *  就钉一个固定值，只把开关、计数、错误这几条交互演出来。 */
let MCP_SERVER: McpServerStatusView = {
  enabled: false,
  running: false,
  port: 7815,
  endpoint: "",
  requests: 0,
  last_error: null,
};

/** 关就是停，开就是起。起不来时回一条错误，供界面显示。 */
function mcpServerApply(enabled: boolean): McpServerStatusView {
  MCP_SERVER = enabled
    ? {
        ...MCP_SERVER,
        enabled: true,
        running: true,
        endpoint: `http://127.0.0.1:${MCP_SERVER.port}/`,
        last_error: null,
      }
    : { ...MCP_SERVER, enabled: false, running: false, endpoint: "", last_error: null };
  return MCP_SERVER;
}

/** 一条工作流该找哪个角色要结果，和 Rust 的 `ModelRole::for_workflow` 对齐。 */
const WORKFLOW_ROLE: Record<string, ModelRole> = {
  agent: "chat",
  prompt_refine: "chat",
  frame_tween: "chat",
  video_frames: "chat",
  image_gen: "image_gen",
  vision_brief: "vision",
  video_brief: "video",
};

function catalog(): WorkflowEntry[] {
  // 就绪度跟着当前会话的模型能力走，和 Rust 的 readiness 一个规矩：没有 image_gen
  // 的模型，坞里生图那一行就该是灰的，还得提示去设置里配——否则预览里看不出这道闸。
  const model = modelForRole(ownerSession, "chat");
  // 能力按合起来算：生图模型单独绑了也算这个会话会生图，
  // 不然用户明明配了生图模型，面板里那条流程还是灰的。
  const caps = { vision: false, image_gen: false, video: false };
  for (const role of ["chat", "image_gen", "vision", "video"] as const) {
    const bound = modelForRole(ownerSession, role);
    caps.vision = caps.vision || bound.capabilities.vision;
    caps.image_gen = caps.image_gen || bound.capabilities.image_gen;
    caps.video = caps.video || bound.capabilities.video;
  }
  return CATALOG.map((info) => ({
    ...info,
    readiness: readinessFor(info.needs, caps),
    // 「由 X 跑」按这条流程真正找的那个角色取，回落主模型时 detached 是 false。
    served_by_label: modelForRole(ownerSession, WORKFLOW_ROLE[info.kind]).label,
    served_by_detached:
      WORKFLOW_ROLE[info.kind] !== "chat" &&
      modelForRole(ownerSession, WORKFLOW_ROLE[info.kind]).id !== model.id,
  }));
}

/** 能力缺口：模型不会的那几项。为空就是跑得动。 */
function missingCapabilities(needs: WorkflowEntry["needs"], caps: WorkflowEntry["needs"]): string[] {
  return (["vision", "image_gen", "video"] as const).filter(
    (flag) => needs[flag] && !caps[flag],
  );
}

function readinessFor(needs: WorkflowEntry["needs"], caps: WorkflowEntry["needs"]): Readiness {
  const missing = missingCapabilities(needs, caps);
  return missing.length === 0 ? { state: "ready" } : { state: "blocked", missing };
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
  // 通道是全局的，载荷必须说清是谁的事。mock 只有一份全局文档，事件就挂在
  // 最近动过它的那个会话名下——用户切走之后，上一段演示流会被前端原样丢弃，
  // 这正是真机的行为。
  const envelope =
    channel === AGENT_EVENT_CHANNEL ? { session_id: ownerSession, event: payload } : payload;
  // 事件桥能用就走 Tauri（真机 / mockIPC 装了 invoke 的时候）；
  // 用不了就落页面内总线，至少订阅方还在。
  const internals = (window as unknown as { __TAURI_INTERNALS__?: { invoke?: unknown } })
    .__TAURI_INTERNALS__;
  if (typeof internals?.invoke === "function") {
    void emit(channel, envelope);
    return;
  }
  publishLocal(channel, envelope);
}

/**
 * 深拷一份文档：数组逐层复制，拿到的人可以随便改，不与 mock 内部状态纠缠。
 *
 * 真机上文档过一次 IPC 就被序列化，天然人人一份；页面内总线不过序列化，递活引用
 * 会让前端持有 mock 的活数组——mock 这边一落笔，前端手里的「改前快照」就地跟着
 * 改写，撤销拍回来的就是当前画面。凡把文档递出 mock 的地方（快照、广播整份回灌）
 * 都经这里。
 */
function cloneDoc(source: PixelDocument): PixelDocument {
  const cels: PixelDocument["cels"] = {};
  for (const [layerId, frames] of Object.entries(source.cels ?? {})) {
    const copied: PixelDocument["cels"][string] = {};
    for (const [frameId, cel] of Object.entries(frames ?? {})) {
      copied[frameId] = { indices: (cel?.indices ?? []).slice() };
    }
    cels[layerId] = copied;
  }
  return {
    name: source.name,
    width: source.width,
    height: source.height,
    palette: (source.palette ?? []).slice(),
    layers: (source.layers ?? []).map((layer) => ({ ...layer })),
    frames: (source.frames ?? []).map((frame) => ({ ...frame })),
    cels,
    palettes: (source.palettes ?? []).map((palette) => ({
      ...palette,
      colors: (palette.colors ?? []).slice(),
    })),
    revision: source.revision,
  };
}

function broadcast(): void {
  // 载荷和真机一致：增量而非整份文档。mock 只有一份全局文档，没有第二个
  // 写入方，所以每次都发全量增量——语义上等于「把本地文档整个换掉」，
  // 真机那边则会 diff 出只变过的那几个 cel。
  // 顺手把这份画布的读数记到当前号名下：侧栏那一行的版本号要靠它自增，
  // 不然画了半天侧栏还挂在上一次开机的版本上。
  recordSessionDoc(ownerSession);
  const cels: DocPatch["cels"] = [];
  for (const [layerId, frames] of Object.entries(doc.cels)) {
    for (const [frameId, cel] of Object.entries(frames)) {
      // 每格复制一份再发：真机走 IPC，JSON 序列化天然是浅拷贝；页面内总线不过
      // 序列化，直接把活数组递过去。前端把这份 document 当「改前快照」压进撤销栈
      // 之后，下一次落笔会原地改写同一批数组——撤销栈里那份跟着变，点撤销时拍回
      // 来的就是当前画面，「撤销无反应」就是这么来的。
      cels.push([layerId, frameId, cel.indices.slice()]);
    }
  }
  fire(AGENT_EVENT_CHANNEL, {
    kind: "document_updated",
    revision,
    patch: {
      name: doc.name,
      width: doc.width,
      height: doc.height,
      palette: doc.palette,
      layers: doc.layers,
      frames: doc.frames,
      palettes: doc.palettes ?? [],
      revision,
      cels,
      dropped: [],
    },
  });
}

/** 演示流还挂着的定时器。停止键一到就全撤，再补一条 interrupted 事件——
 * 不这么办的话浏览器里点停止毫无反应，那一串 setTimeout 自顾自播完，
 * 前端的 running 收不了口，跟真机上停止键失灵是一个样子。 */
const demoTimers = new Set<ReturnType<typeof setTimeout>>();

function hold(frame: () => void, at: number): void {
  const timer = setTimeout(() => {
    demoTimers.delete(timer);
    frame();
  }, at);
  demoTimers.add(timer);
}

function stopDemoTurn(): void {
  for (const timer of demoTimers) clearTimeout(timer);
  demoTimers.clear();
  fire(AGENT_EVENT_CHANNEL, { kind: "interrupted" });
}

/** 假后端在聊天里播一段「思考 -> 分流 -> 落笔 -> 答复」的演示流，纯浏览器里能直接看成色。
 * 事件类型与 Rust 主循环广播的一致（含工具调用与结果），折叠块的高度因此能量到真实值。 */
function demoTurn(): void {
  // 入参形状照 Rust 的 TurnPlan::node_input() 抄：references / intent / style /
  // knowledge / presets。老早那版用的是 reference_mode、art_style 这些旧键，
  // 解析器一个都不认，预览里那一段就只能显示裸 JSON。
  const planInput = {
    references: [{ index: 1, mode: "style", because: "照这个画风换色" }],
    intent: { id: "tilemap", because: "瓦片地图" },
    style: { id: "pico8", because: "PICO-8 配色" },
    knowledge: ["tilemap", "seamless"],
    presets: [
      { id: "realistic", because: "pinned in the composer" },
      { id: "occlusion", because: "pinned in the composer" },
    ],
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
      summary: "成品：瓦片地图 · 风格：pico8 · 收尾：写实渲染、闭塞接触",
      is_error: false,
    },
    {
      at: 700,
      kind: "tool_call",
      id: "demo-c2",
      name: "pixel_apply_operations",
      input: {
        layers: DEMO_OPS,
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
    hold(() => fire(AGENT_EVENT_CHANNEL, frame), frame.at);
  }
  // 落笔排在 tool_call 之后、tool_result 之前：预览里画布先变，结果再说话。
  // 不接这一下的话，浏览器里那条「已写入 2 个操作」说完画布还是空的。
  hold(() => applyDemoLayers(DEMO_OPS), 800);
}

/** 演示回合的像素操作。词表是 mock 自己的一套简化形，不是模型契约里的
 * PixelOperation；真正要一致的是落笔的账：颜色怎么取下标、越界怎么算。 */
const DEMO_OPS = [
  {
    name: "L0",
    operations: [
      { op: "rect", x: 8, y: 5, w: 4, h: 4, color: "3" },
      { op: "pixel", x: 9, y: 6, color: "2" },
    ],
  },
];

type Args = Record<string, unknown>;

/** 演示文档挂在哪条会话名下。mock 没有真机的会话隔离，就用「最近动过它的那个
 * 会话」顶着，足够让前端按 activeId 分流的那条守卫在浏览器里也演练到。 */
let ownerSession = "p1";

function handler(cmd: string, raw?: unknown): unknown {
  // 指令过手即记账：纯浏览器里界面没有句柄能把 store 掏出来，「点了没反应」
  // 这类事只能靠回放指令和活文档定位。这份记录只活在内存里。
  if (mockTrace.length >= 60) mockTrace.shift();
  mockTrace.push({ cmd, args: raw ?? null });
  mirrorMockState();
  const payload: Args = (raw ?? {}) as Args;
  const id = String(payload.id ?? "p1");
  // 真机那边 agent_set_active 会同步编辑器停住的帧号，mock 也记一份：
  // document_png_url 不点名帧时才知道该合成哪一帧。
  if (cmd === "agent_set_active") {
    const active = payload.active as { frame?: unknown } | undefined;
    if (typeof active?.frame === "string") activeFrameId = active.frame;
    // 层也记一份：插帧和量化不点名层时默认落在它上面，和真机同一个来源。
    const layer = (active as { layer?: unknown } | undefined)?.layer;
    if (typeof layer === "string" && doc.layers.some((item) => item.id === layer)) {
      activeLayerId = layer;
    }
  }
  // model_* 的 id 是模型 id 不是会话 id，拿它盖归属会把事件发到一个不存在的
  // 会话上去，画面就再也不更新了。不带 id 的指令（app_close_reply 这类全局
  // 命令）同样不该抢归属：真机那边它压根不针对会话，mock 里把它盖回默认值
  // 会让后续广播发错人家，前端的 activeId 分流就当垃圾事件扔掉。
  if (payload.id != null && !cmd.startsWith("model_")) ownerSession = id;
  switch (cmd) {
    case "agent_list_models":
      return emptyModels ? { active_id: "", entries: [] } : modelsView();
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
    case "mcp_server_status":
      return mcpServerApply(MCP_SERVER.enabled);
    case "mcp_server_set_enabled":
      // 端口守卫：真机上允许给 0 让内核挑，浏览器里挑不了，
      // 所以只把合法区间收一下，保持预览态别乱跳。
      return mcpServerApply(Boolean(payload.enabled));
    case "mcp_server_set_port":
      // 端口单独存，重启时才跟着变。改完不像真机那样立刻重绑——
      // 预览里没有 socket，只有跟着 enabled 状态走。
      {
        const raw = Number(payload.port);
        const port = Number.isFinite(raw) ? Math.min(65535, Math.max(0, Math.floor(raw))) : 0;
        MCP_SERVER = { ...MCP_SERVER, port };
      }
      return mcpServerApply(MCP_SERVER.enabled);
    case "mcp_server_restart":
      // 重启在预览里就是把请求计数归零再按当前开关起一遍，
      // 够看清「停了能拉起来」这条反馈。
      MCP_SERVER = {
        ...MCP_SERVER,
        requests: 0,
        last_error: null,
      };
      return mcpServerApply(MCP_SERVER.enabled);
    case "agent_document":
      // 深拷一份再交出去：前端会把这份文档整份留在 state 里，也会把它当撤销
      // 快照压栈。递活引用的话，mock 这边一动笔，前端「改前快照」就地改写。
      // 按号记账：这份画布已经交给这个号了，侧栏再报它的宽高版本要照这个数。
      recordSessionDoc(id);
      return { id, revision, document: cloneDoc(doc) };
    case "agent_history":
      return [];
    case "agent_set_active":
    case "agent_set_permission":
    case "agent_resolve_approval":
      return null;
    case "agent_send_message":
      demoTurn();
      return null;
    case "agent_interrupt":
      stopDemoTurn();
      return null;
    case "session_list":
      return listSessions(revision);
    case "session_create": {
      const requested = payload.document as PixelDocument | null;
      const title = (payload.title as string | null | undefined) ?? null;
      if (requested) {
        // 用户选了尺寸：按新宽高重画一份，别把请求文档原样塞进来——那份只有一层一帧，
        // 直接换上会把预览里的三帧动画冲掉，看着像新建会话把内容弄丢了。
        doc = makeDocumentSized(requested.width, requested.height);
        activeFrameId = doc.frames[0]?.id ?? "F0";
        revision += 1;
      }
      // 编号规矩和 Rust 的 create_session 对齐（s1、s2 这样）：预览叫 s-1、
      // 真机叫 s1，同一个动作两种默认名，两侧对不上。
      const fresh = `s${nextSessionSeq++}`;
      previewSessions.push(fresh);
      // 新号自己的画布读数：不记这份，回落的就是刚被换掉的当前画布，
      // 而那个画布归 ownerSession，侧栏两行会读出同一个宽高。
      recordSessionDoc(fresh, { width: doc.width, height: doc.height });
      sessionOrders[fresh] = previewSessions.length;
      // 填了名字就记下：预览和真机在这里要一致，不然侧栏两种表现。
      sessionTitles[fresh] = title;
      return makeSession(fresh, revision);
    }
    case "session_bind_model":
      sessionModels[String(payload.id ?? "p1")] = String(payload.model_id ?? payload.modelId ?? activeModelId);
      // 改绑完的会话名跟着新模型走，不然侧栏要等下一次刷新才变。
      return makeSession(String(payload.id ?? "p1"), revision);
    case "session_bind_role": {
      // 桥里发的是 modelId，真机靠 camelCase 转 snake_case 才对上；预览照原样收。
      const target = String(payload.id ?? "p1");
      const role = String(payload.role ?? "image_gen") as ModelRole;
      const wanted = String(payload.model_id ?? payload.modelId ?? "");
      if (role === "chat") {
        throw new Error(
          "the chat role is the session model itself; rebind the session instead",
        );
      }
      if (!previewModels.some((entry) => entry.id === wanted)) {
        throw new Error(`model ${wanted} is not configured`);
      }
      const book = (sessionRoles[target] ??= {});
      book[role] = wanted;
      return makeSession(target, revision);
    }
    case "session_clear_role": {
      const target = String(payload.id ?? "p1");
      const role = String(payload.role ?? "image_gen") as ModelRole;
      if (role === "chat") {
        throw new Error(
          "the chat role is the session model itself; rebind the session instead",
        );
      }
      // 没绑过就是空操作，照样回当前分工，和 Rust 的 remove 一个语义。
      delete sessionRoles[target]?.[role];
      return makeSession(target, revision);
    }
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
      // 只有簿里的号才算数：点一个刚被删掉的定义，默认就跟着空下来。
      activeModelId = previewModels.some((entry) => entry.id === payload.id)
        ? String(payload.id)
        : "";
      return modelsView();
    case "model_upsert": {
      const config = payload.config as unknown as ModelConfig | undefined;
      if (!config?.id) return modelsView();
      const previous = previewModels.find((entry) => entry.id === config.id);
      // 表单不会把密钥回填出来：空着送过来就是「沿用旧的」，
      // 拿空串盖上去会让一份本来配好的模型突然变成没密钥。
      if (config.api_key !== "") previewApiKeys[config.id] = config.api_key;
      const view: ModelView = {
        id: config.id,
        label: config.label || config.model || previous?.label || config.id,
        protocol: config.protocol,
        base_url: config.base_url,
        model: config.model,
        max_tokens: config.max_tokens ?? null,
        temperature: config.temperature ?? null,
        disable_thinking: config.disable_thinking ?? null,
        capabilities: config.capabilities,
        has_api_key: (previewApiKeys[config.id] ?? "").trim() !== "",
      };
      if (previous) {
        previewModels.splice(previewModels.indexOf(previous), 1, view);
      } else {
        previewModels.push(view);
        // 簿里原本空着（?mock=empty 之外的常规路径）：新建的第一份自动当默认。
        if (activeModelId === "" || !previewModels.some((entry) => entry.id === activeModelId)) {
          activeModelId = view.id;
        }
      }
      return modelsView();
    }
    case "model_remove": {
      const at = previewModels.findIndex((entry) => entry.id === payload.id);
      if (at >= 0) previewModels.splice(at, 1);
      delete previewApiKeys[String(payload.id)];
      repairOrphanedSessions();
      // 删的正是当前默认：回落到簿里剩下的第一个，和 Rust 一致。
      if (activeModelId === String(payload.id)) {
        activeModelId = previewModels[0]?.id ?? "";
      }
      return modelsView();
    }
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
      // 整份替换，和 Rust 的 sync_document 同语义：外部文档（撤销拍回、导入 .aip）
      // 是权威，mock 手里的画布必须换成它，否则下一次渲染仍按旧像素出图，
      // 撤销看着就没生效。老逻辑只自增 revision 不收文档，撤销链路整段是空的。
      {
        const incoming = payload.document as PixelDocument | null;
        if (incoming && Array.isArray(incoming.layers) && Array.isArray(incoming.frames)) {
          // 深拷：前端交上来的那份随后还会留在它的 state / 撤销栈里被读写，
          // 与 mock 共享引用会两边互相改写。
          doc = cloneDoc(incoming);
          doc.revision = revision + 1;
          revision = doc.revision;
          // 整份换过画布，读数重新记账：导入 .aip、撤销拍回都会走到这里，
          // 不记账的话侧栏还挂着换文档之前的宽高版本。
          recordSessionDoc(ownerSession, { width: doc.width, height: doc.height });
          // 选中落在不存在的层/帧上时回落到首个，和 Rust 的同一步对齐。
          if (!doc.layers.some((layer) => layer.id === activeLayerId)) {
            activeLayerId = doc.layers[0]?.id ?? activeLayerId;
          }
          if (!doc.frames.some((frame) => frame.id === activeFrameId)) {
            activeFrameId = doc.frames[0]?.id ?? activeFrameId;
          }
        }
      }
      return { revision };
    case "editor_resize_canvas": {
      // 改画布宽高：左上角锚定，和 Rust 的 Cel::resize 同一套算法——装得下的格子
      // 原样搬过去，多出来的补透明（索引 0）。尺寸没动就不动账本。
      const width = Math.max(1, Math.min(1024, Math.round(Number(payload.width) || doc.width)));
      const height = Math.max(1, Math.min(1024, Math.round(Number(payload.height) || doc.height)));
      if (width !== doc.width || height !== doc.height) {
        for (const layerId of Object.keys(doc.cels)) {
          for (const frameId of Object.keys(doc.cels[layerId] ?? {})) {
            const cel = doc.cels[layerId][frameId];
            const grown = new Array<number>(width * height).fill(0);
            const rows = Math.min(height, doc.height);
            const cols = Math.min(width, doc.width);
            for (let y = 0; y < rows; y += 1) {
              const from = y * doc.width;
              for (let x = 0; x < cols; x += 1) grown[y * width + x] = cel.indices[from + x];
            }
            cel.indices = grown;
          }
        }
        doc.width = width;
        doc.height = height;
        revision += 1;
        broadcast();
      }
      return revision;
    }
    case "editor_paint_stroke": {
      // 落笔：把这一笔的格子写进当前 cel；橡皮的颜色是 null，也就是回到透明格。
      // 形状必须和真机一致：bridge 发的是 { id, stroke: { layer, frame, cells, color } }，
      // Rust 那边也是 stroke: StrokeRequest。mock 读平铺字段的话 strokes 会全落在
      // undefined 上——预览里画一笔看着有反馈，实际一格都没改。
      const stroke = payload.stroke as unknown as { layer: string; frame: string; cells: { x: number; y: number }[]; color: string | null };
      const cel = doc.cels[stroke.layer]?.[stroke.frame];
      if (cel) {
        // 颜色取索引和 Rust 的 color_index -> intern_color 同路：调色板里没有就
        // 追加。老逻辑找不到色静默落 index 1，画出来永远是调色板第一种色，
        // 「画笔改不了颜色」这类怪现象就从这儿来。
        let index = 0;
        let badInk = false;
        if (stroke.color != null) {
          const ink = parseHex(stroke.color);
          if (ink === null) badInk = true; // 非法颜色字面量，真机整笔回滚
          else index = internColor(ink);
        }
        if (!badInk) {
          for (const cell of stroke.cells) {
            if (cell.x < 0 || cell.y < 0 || cell.x >= doc.width || cell.y >= doc.height) continue;
            cel.indices[cell.y * doc.width + cell.x] = index;
          }
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
        let index = 0;
        let badInk = false;
        if (fill.color != null) {
          const ink = parseHex(fill.color);
          if (ink === null) badInk = true;
          else index = internColor(ink);
        }
        if (!badInk && fill.x >= 0 && fill.y >= 0 && fill.x < doc.width && fill.y < doc.height) {
          // 4 邻域浸染，只浸和起点同索引的格子，对齐 ops::flood_fill。
          // 老逻辑只写单格，预览里点一下填充只出一个像素点。
          floodFill(cel.indices, doc.width, doc.height, fill.x, fill.y, index);
        }
      }
      revision += 1;
      broadcast();
      return revision;
    }
    case "editor_apply_ops":
      // 编辑器直接下的一整批结构操作。假后端不能整批吞掉：界面不动，预览就成了
      // 「功能看着有、实际没接上」。真机由 Rust 的 apply_batch 执行，这里逐条照它的
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
          // 空名（或全是空格）真机回 EmptyName 整批回滚；这里和 rename_palette
          // 一样就地跳过，别把侧栏那一层塌成一条缝。
          const layer = doc.layers.find((item) => item.id === id);
          const name = typeof op.name === "string" ? op.name.trim() : "";
          if (layer && name !== "") layer.name = name;
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
          // 1..=60000 之外真机报 Dimension 整批回滚；跳过。
          if (frame && typeof op.duration_ms === "number") {
            const ms = Math.round(op.duration_ms);
            if (ms >= 1 && ms <= MAX_FRAME_DURATION_MS) frame.duration_ms = ms;
          }
        }
        if (op.op === "create_layer") {
          // 位置、名字、id 都按 Rust 的规矩来：插在 after 之后，没点名就落栈顶；
          // id 取第一个没占用的编号。配色范围和锁跟新邻居继承，除非显式点名——
          // Rust 那边 `create_layer {palette_id, locked}` 说了就照办，这里漏了这条
          // 分支的话，预览里模型显式指定的范围与锁会被悄悄继承掉，和真机不一样。
          const after = op.after as string | null | undefined;
          if (after != null && !doc.layers.some((l) => l.id === after)) continue;
          const pos = after
            ? doc.layers.findIndex((l) => l.id === after) + 1
            : doc.layers.length;
          const neighbor = doc.layers[Math.max(0, pos - 1)];
          // 邻居的配色得真在库里，指向一个不存在的 id 会开天窗。
          const named =
            typeof op.palette_id === "string" &&
            doc.palettes.some((p) => p.id === op.palette_id)
              ? (op.palette_id as string)
              : doc.palettes.some((p) => p.id === neighbor?.palette_id)
                ? neighbor.palette_id
                : (doc.palettes[0]?.id ?? "sweetie16");
          const newId = nextId("L", doc.layers.map((l) => l.id));
          doc.layers.splice(Math.min(pos, doc.layers.length), 0, {
            id: newId,
            name: typeof op.name === "string" ? op.name : `Layer ${doc.layers.length + 1}`,
            visible: true,
            opacity: 255,
            palette_id: named,
            locked:
              typeof op.locked === "boolean" ? op.locked : (neighbor?.locked ?? false),
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
          // 内置的删不得；被某层指着时要一个接盘的那一套，给了才改指过去再删。
          if (!target || target.builtin) continue;
          const holders = doc.layers.filter((l) => l.palette_id === id);
          if (holders.length > 0) {
            const fallback =
              typeof op.fallback === "string" && op.fallback !== "" ? op.fallback : null;
            if (!fallback || fallback === id) continue;
            if (!doc.palettes.some((p) => p.id === fallback)) continue;
            for (const layer of holders) layer.palette_id = fallback;
          }
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
          const replacementHex =
            typeof op.replacement === "string" && op.replacement !== "" ? op.replacement : null;
          const removedHex = rgbaToHex(target.colors[index]);
          // 被删色在文档调色板里的位置要先量好：等下接手色会被 intern 追加进去。
          const deadIndex = doc.palette.findIndex((item) => rgbaToHex(item) === removedHex);
          target.colors.splice(index, 1);
          // 没给接手色就是老语义：只动范围不动画面。
          const replacement = replacementHex ? parseHex(replacementHex) : null;
          // 被删色压根没进过文档调色板 = 画面上没人用它，像素那一步无事可做。
          // 注意 deadIndex 是 -1 时不能当下标 0 用：那是透明格，会把整个画面擦花。
          if (deadIndex < 0 || !replacement) continue;
          const dead = deadIndex + 1;
          const slot = internColor(replacement);
          // 只动认领了这套范围的层：别的层色板里也有同值颜色，那不是这个范围的事。
          const owners = doc.layers.filter((l) => l.palette_id === id).map((l) => l.id);
          for (const owner of owners) {
            const frames = doc.cels[owner];
            if (!frames) continue;
            for (const cel of Object.values(frames)) {
              if (!cel) continue;
              for (let i = 0; i < cel.indices.length; i += 1) {
                if (cel.indices[i] === dead) cel.indices[i] = slot;
              }
            }
          }
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
    case "read_image_context":
      // 对话框 -> 读图 -> 摘要这三步里，读图这一步在真机是纯本机计算。
      // 预览同样只本机：按路径合成一张 PNG 附上，量化面板马上能用。
      return referenceAttachment(String(payload.path ?? ""));
    case "prompt_refine":
      return refineIdea(
        String(payload.idea ?? ""),
        payload.target as RefineTarget | undefined,
      );
    case "video_probe":
      return probeFor(String(payload.path ?? ""));
    case "vision_brief": {
      // 读图简报要的是 vision 角色，不认能力照真机那句话拒绝——
      // 否则坞面板永远是绿的，切不到「没配视觉模型」那一支。
      const model = modelForRole(String(payload.id ?? ownerSession), "vision");
      if (!model.capabilities.vision) {
        throw new Error(
          "this model is not marked as able to read images; pick a vision model for this session or turn on vision in model settings",
        );
      }
      return referenceBrief(String(payload.path ?? ""));
    }
    case "video_brief": {
      const params = (payload.params ?? {}) as VideoBriefParams;
      const model = modelForRole(String(payload.id ?? ownerSession), "video");
      if (!model.capabilities.video) {
        throw new Error(
          "this model is not marked as able to read video; pick a video model for this session or turn on video in model settings",
        );
      }
      const wanted = params.count && params.count > 0 ? params.count : 4;
      return motionBrief(String(params.path ?? ""), wanted);
    }
    case "workflow_pixelize":
      return runPixelize((payload.params ?? {}) as PixelizeParams);
    case "workflow_tween":
      return runTween((payload.params ?? {}) as TweenParams);
    case "workflow_video_frames":
      return runVideoFrames((payload.params ?? {}) as VideoFramesParams);
    case "mcp_list":
      return mcpList();
    case "workflow_image_gen":
      // 生图：合成位图 -> 量化落 cel -> 广播。命令名、事件、回执都和真机同一个形状，
      // 预览里点「生成」才知道这条链路通不通。
      return runImageGen((payload.params ?? {}) as ImageGenRequest);
    case "mcp_upsert":
    case "mcp_connect":
    case "mcp_disconnect":
    case "mcp_remove":
      return mcpList();
    case "workflow_catalog":
      return catalog();
    case "document_png_url":
      // 真机那边：点名帧合那一帧，没点名合活跃帧（不是把所有帧横铺开）。
      // mock 照同一套语义来，不然浏览器里验不出「切帧后画布对不上」这类问题。
      if (typeof payload.frame === "number") return pngFor(payload.frame);
      return pngFor(Math.max(0, doc.frames.findIndex((f) => f.id === activeFrameId)));
    case "batch_recipes_list":
      return [];
    case "batch_recipe_save":
    case "batch_recipe_delete":
    case "batch_recipe_export":
    case "batch_recipe_import":
    case "batch_scan":
    case "batch_run":
      return null;
    // 文件对话框：真机上用户挑完给真路径，预览里按后缀合成一个。
    // 保存类那支故意还是 null（当取消）——导出/保存链路的取消分支要靠它走到。
    case "plugin:dialog|open":
      return synthDialogPath(payload);
    case "document_export":
    case "aip_save":
    case "aip_load":
      return null;
    case "aip_text":
      return "AIP1\n";
    // 关窗值守：浏览器里没人真来问，登记和答复都当空操作放行。
    case "app_close_guard":
    case "app_close_reply":
      return null;
    default:
      if (cmd.startsWith("plugin:")) return null;
      throw new Error(`浏览器预览没有这条假数据：${cmd}`);
  }
}

let emptyModels = false;


/** 最近过手的指令，给纯浏览器排查「点了没反应」用，只活在内存里。 */
const mockTrace: { cmd: string; args: unknown }[] = [];

/**
 * 把 mock 的后端状态写进 `<html data-aip-mock>`。
 *
 * 排查「点了没反应」得同时看清两件事：界面把什么指令发了出去、后端文档到底有
 * 没有变。可沙箱化世界里 `window` 是受限代理，挂在 window 上的调试钩子根本
 * 摸不到，而 DOM 是共用的——写在这儿，任何观测手段都读得到。
 */
function mirrorMockState(): void {
  const root = document.documentElement;
  if (!root) return;
  // 逐帧各报一份非空格数：落笔写偏到别的帧上，「画了没反应」和「画到别处去了」
  // 是两种完全不同的病，只看当前那一格分不出来。
  const filledByFrame: Record<string, number> = {};
  for (const [frameId, cel] of Object.entries(doc.cels[activeLayerId] ?? {})) {
    filledByFrame[frameId] = cel.indices.reduce(
      (sum: number, value: number) => sum + (value === 0 ? 0 : 1),
      0,
    );
  }
  root.dataset.aipMock = JSON.stringify({
    width: doc.width,
    height: doc.height,
    revision,
    layer: activeLayerId,
    frame: activeFrameId,
    filled: filledByFrame[activeFrameId] ?? 0,
    filledByFrame,
    paletteSize: doc.palette.length,
    layerPalettes: doc.layers.map((item) => item.palette_id),
    last: mockTrace[mockTrace.length - 1] ?? null,
    paint: mockTrace.filter((item) => item.cmd === "editor_paint_stroke").slice(-1),
    trace: mockTrace.map((item) => item.cmd),
  });
}

/** mock 的调试出口：回放指令、看活文档，只活在内存里。 */
function mockDebug(): unknown {
  return {
    commands: mockTrace.slice(),
    doc: JSON.parse(document.documentElement.dataset.aipMock ?? "{}"),
  };
}

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
  (window as unknown as { __AIP_MOCK__?: () => unknown }).__AIP_MOCK__ = mockDebug;
  // 两个只活在预览里的测试缝：emit 一条关窗问询（模拟 Rust 按住窗口）、
  // 广播一条 document_updated（顺着真链路置脏）。无头浏览器靠它们把关窗值守跑通。
  (window as unknown as { __AIP_MOCK_CLOSE__?: () => void }).__AIP_MOCK_CLOSE__ = () => {
    void fire(CLOSE_EVENT_CHANNEL, null);
  };
  (window as unknown as { __AIP_MOCK_DIRTY__?: () => void }).__AIP_MOCK_DIRTY__ = () => {
    broadcast();
  };
}
