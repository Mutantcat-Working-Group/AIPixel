// 应用状态：会话表、模型配置、对话条目、权威文档与画布预览。
// Rust 主循环只通过 agent-event 说话；所有副作用都收敛成 bridge 调用。

import { create } from "zustand";

import { renderUiText, translate, type Lang, type TVARS, type TKey } from "./i18n";
import * as bridge from "./bridge";
import type { ExportFormat } from "./bridge";
import { PALETTE_PRESETS, hexesOf, nearestHex, parseHex, rgbaToHex } from "./palette";
import {
  emptyTranscript,
  historyToTranscript,
  pushPendingAssistant,
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
  BatchKind,
  BatchRecipe,
  BatchRecipeEntry,
  RecipeImportReport,
  BatchScan,
  EditorOperation,
  DockKind,
  DockDraft,
  ImageGenParams,
  ImageSupport,
  McpServerConfig,
  McpServersView,
  NamedPalette,
  ModelConfig,
  ModelsView,
  ModelRole,
  PixelizeParams,
  PendingAttachment,
  PendingApproval,
 PermissionMode,
  Protocol,
  PixelDocument,
  PixelizeOptions,
  RefinedPrompt,
  RefineTarget,
  SessionInfo,
  SettingsTab,
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
  InkColor,
  LoopLimits,
} from "./types";

import {
  canRunBatch,
  DEFAULT_BATCH_RECIPE,
  EMPTY_BATCH_RUN,
  reduceBatchEvent,
  recipeNameProblem,
  scanMatchesKind,
  type BatchRun,
} from "./batch";

/**
 * 内置配色范围的界面镜像，色值与 Rust 的 `builtin_palettes()` 逐字一致。
 * id 也照抄：图层认领用的是同一套 id，两边对不上就指不到地方。
 * 只在「文档里没带 palettes」时兜底（早期 .aip、浏览器 preview）。
 */
const BUILTIN_PALETTES: NamedPalette[] = PALETTE_PRESETS.map((preset) => ({
  id: preset.id,
  name: preset.name,
  colors: preset.colors.flatMap((hex) => {
    const parsed = parseHex(hex);
    // 内置色写错是不可能的事；真写错了就少一个色，也别让整个文档打不开。
    return parsed ? [parsed] : [];
  }),
  builtin: true,
}));

export interface DocumentSnapshot {
  document: PixelDocument | null;
  revision: number;
  pngUrl: string | null;
  /** 已经渲染过 PNG 的 revision，用来丢弃过期的异步结果。 */
  pngRevision: number;
  frameIndex: number;
}

/**
 * 会话排序：先按用户在侧边栏摆好的顺序位，同值时退回 id 字典序保证确定性。
 * Rust 本身已按 order 返回，这里再兜一次是防着新建会话还没落排序位、
 * 以及浏览器预览里没有真后端的情况。
 */
function sortSessions(list: SessionInfo[]): SessionInfo[] {
  return [...list].sort((a, b) => a.order - b.order || a.id.localeCompare(b.id));
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

/**
 * 批量工作台的状态。它不跟着会话走：一个文件夹进一个文件夹出，与哪条对话开着无关。
 * scan 描述「这个文件夹里有什么」，run 描述「这一趟跑到哪了」。
 */
export interface BatchState {
  /** 可序列化的 recipe：改完下次扫/跑都带着。 */
  recipe: BatchRecipe;
  /** 最近一次扫描结果；目录或 kind 一变就作废，宁可多扫一次也别拿着旧清单开跑。 */
  scan: BatchScan | null;
  scanBusy: boolean;
  run: BatchRun;
  /** 存在本机 recipes.json 里的配方簿：跑熟的 recipe 固化下来随取随用。 */
  recipeBook: BatchRecipeEntry[];
  /** 配方名输入框。存成功即清空，簿子里显示的才是真相。 */
  recipeName: string;
  /** 存/删配方的进行中；配方簿是本地小文件，但按钮该转还得转。 */
  recipeBusy: boolean;
  /** 最近一次导入的回执；没导入过就是 null。显示逐条交代，用户自己判断下一步。 */
  recipeImport: RecipeImportReport | null;
}

/** 会动文档的四条工作流入参。image_gen / frame_tween / video_frames 走 runWorkflow。 */
export type WorkflowParams = TweenParams | PixelizeParams | ImageGenParams | VideoFramesParams;

interface StoreState extends DocumentSnapshot, WorkflowState, BatchState {
  booted: boolean;
  models: ModelsView;
  /** 用户自配的 MCP 服务器；连上的才带工具清单。 */
  mcpServers: McpServersView;
  sessions: SessionInfo[];
  activeId: string | null;
  entries: TranscriptEntry[];
  running: boolean;
  /** 本轮对话开始的时间戳（ms）。运行中给聊天面板当计时起点，停下来就清。 */
  runStartedAt: number | null;
  /** 上一轮收尾时一共花了多久（ms）。停火了也不擦，好让用户回看这一圈的代价。 */
  runElapsedMs: number | null;
  /** 主循环还在跑，但很久没有新事件了。只是提醒，不动数据、不替你中断。 */
  stalled: boolean;
  usage: Usage | null;
  /** 上一句发出的话（含附件），中断/报错后用来一键重试。换会话即清空。 */
  lastQuery: { text: string; attachments: PendingAttachment[] } | null;
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
  /** 运行护栏当前值；为 null 表示还没读过 Rust。 */
  loopLimits: LoopLimits | null;
  /** MCP 总开关。true = 模型看得见用户自配的外部工具。 */
  mcpEnabled: boolean;
  /** 设置弹窗停在哪一页：模型 / 行为护栏 / 关于。 */
  settingsTab: SettingsTab;
}

export interface StoreActions {
  boot: () => Promise<void>;
  setLang: (lang: Lang) => void;
  selectSession: (id: string) => Promise<void>;
  createSession: (width?: number, height?: number) => Promise<void>;
  removeSession: (id: string) => Promise<void>;
  /** 改侧边栏显示名。空白名 Rust 当取消处理。 */
  renameSession: (id: string, title: string) => Promise<void>;
  /** 拖动排序后整批上报新次序；数组就是排好后的 id 列表。 */
  reorderSessions: (ids: string[]) => Promise<void>;
  bindSessionModel: (modelId: string) => Promise<void>;
  /** 给生图 / 识图 / 读视频之一另绑一个模型。 */
  bindSessionRole: (role: ModelRole, modelId: string) => Promise<void>;
  /** 取消某个角色的单独绑定，让它回落去用会话主模型。 */
  clearSessionRole: (role: ModelRole) => Promise<void>;
  setPermissionMode: (mode: PermissionMode) => Promise<void>;
  send: (text: string) => Promise<void>;
  /** 重发刚才没跑完的那句。没有可重试的就什么都不做。 */
  retry: () => Promise<void>;
  interrupt: () => Promise<void>;
  upsertModel: (config: ModelConfig) => Promise<void>;
  removeModel: (id: string) => Promise<void>;
  activateModel: (id: string) => Promise<void>;
  refreshLoopLimits: () => Promise<void>;
  saveLoopLimits: (limits: LoopLimits) => Promise<void>;
  /** 拉 provider 的模型清单，供设置界面挑一个填入；失败把原文抛回界面。 */
  fetchProviderModels: (params: {
    id?: string | null;
    baseUrl: string;
    apiKey: string;
    protocol: Protocol;
  }) => Promise<string[]>;
  refreshMcp: () => Promise<void>;
  openMcp: () => void;
  closeMcp: () => void;
  /** 读一次 MCP 总开关。 */
  refreshMcpEnabled: () => Promise<void>;
  /** 开/关 MCP。Rust 当场作用到活着的会话，回值才是生效的那份。 */
  setMcpEnabled: (enabled: boolean) => Promise<void>;
  /** 探测这个模型能不能出图；只回结论，不改任何设置。 */
  probeImage: (params: {
    id?: string | null;
    baseUrl: string;
    apiKey: string;
    protocol: Protocol;
    model: string;
  }) => Promise<ImageSupport>;
  /** 切设置页。 */
  setSettingsTab: (tab: SettingsTab) => void;
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
  /** 导出到本地文件；格式由 Rust 认领（gif / frame / strip / sheet / ase）。 */
  exportDocument: (
    format: ExportFormat,
    path: string,
    options?: { columns?: number; frame?: number },
  ) => Promise<void>;
  readAipText: () => Promise<string | null>;
  clearNotice: () => void;
  /** 一条不打断流程的小警告：措辞跟着界面语言走，不带 fail 那一堆副作用。 */
  warnKey: (key: TKey, vars?: TVARS) => void;
  /** 一条本机成功提示（画布建好了、盘存完了）。同样不带 fail 那一堆副作用。 */
  noteKey: (key: TKey, vars?: TVARS) => void;
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
  /**
   * 「用这条提示词生图」：写进生图面板并切过去。
   * referencePath 是「图也一起带过去」：识图读过的示例图垫到生图面板当参考图，
   * 模型既看得见描述、也看得见原图；不给就只搬提示词，不动垫图设置。
   */
  fillGenPrompt: (prompt: string, referencePath?: string | null) => void;
  /** 对挂起的工具调用给出决定；Approve all 只降级本 turn。 */
  resolveApproval: (decision: ApprovalDecision) => Promise<void>;
  /** 落一笔：抬笔时整笔发送，颜色取当前调色板选择（null = 擦除）。 */
  /** inkOverride 给橡皮用：无视当前选色，一律擦回透明。 */
  paintStroke: (cells: StrokeCell[], inkOverride?: InkColor) => Promise<void>;
  /** 油漆桶点一下；颜色取当前调色板选择（null = 浸回透明）。 */
  fillCell: (x: number, y: number, inkOverride?: InkColor) => Promise<void>;
  /** 结构与帧操作。frameHint 是新文档到达后要选中的帧序。 */
  runEditorOps: (ops: EditorOperation[], frameHint?: number) => Promise<number | null>;
  /** 换配色范围：整幅按就近色重映射进新调色板，画面留住、颜色归队。 */
  setPaletteColors: (colors: string[]) => Promise<boolean>;
  /** 复制一套现成的配色范围当起点（内置预设走副本），再把当前层指过去。 */
    forkPalette: (fromId: string, name: string, layerId?: string) => Promise<void>;
  /** 从零起一套；不给名字就沿用 Rust 的兜底命名。 */
    createPalette: (name: string, colors: string[], layerId?: string) => Promise<void>;
  /** 删掉一套自定义范围。内置的、还被引用的，Rust 会拒。 */
  deletePalette: (id: string) => Promise<void>;
  renamePalette: (id: string, name: string) => Promise<void>;
  /** 往范围里添一个颜色。已经有这个色就当无事发生。 */
  addPaletteColor: (id: string, color: string) => Promise<void>;
  /** 从范围里拿掉一个颜色；画面不动，只是以后不许再用了。 */
  removePaletteColor: (id: string, index: number) => Promise<void>;
  /** 把某一层指到另一套范围上；这一层的像素就地收进新范围。 */
  setLayerPalette: (layerId: string, paletteId: string) => Promise<void>;
  /** 配色锁。锁上=只许用范围内的颜色，AI 也不能越界。 */
  setLayerLocked: (layerId: string, locked: boolean) => Promise<void>;
  addFrame: () => Promise<void>;
  duplicateFrame: () => Promise<void>;
  deleteFrame: () => Promise<void>;
  moveFrame: (delta: number) => Promise<void>;
  /** 改当前帧的停留时长。逐帧动画的节拍全在帧时长上。 */
  setFrameDuration: (durationMs: number) => Promise<void>;
  /** 切图层显隐。隐藏只为看清底下，内容不丢，也不改选中。 */
  setLayerVisible: (layerId: string, visible: boolean) => Promise<void>;
  /** 图层不透明度 0..255，与 Rust Layer::opacity 同一量纲。 */
  setLayerOpacity: (layerId: string, opacity: number) => Promise<void>;
  /** 新建图层：插在当前层之后，配色范围和锁都跟新邻居走（Rust 的继承规则）。 */
  addLayer: () => Promise<void>;
  /** 删图层：只剩一层时 Rust 会拒，这是最后一道人情关。 */
  deleteLayer: (layerId?: string) => Promise<void>;
  /** 图层名：空名字等于没改，交给 Rust 的原名顶着。 */
  renameLayer: (layerId: string, name: string) => Promise<void>;
  /** 沿绘制顺序挪一格：delta +1 = 后绘制，盖在更多图层之上。 */
  moveLayer: (delta: number) => Promise<void>;
  /** 回退一步编辑器改动：撤销栈见底就什么都不做。 */
  undoEdit: () => Promise<void>;
  /** 改画布宽高：左上角锚定，原有像素留住，新区域透明。 */
  resizeCanvas: (width: number, height: number) => Promise<void>;
  /** 换批量种类。旧扫描立马作废：素材类型和语义都变了，留着只会误导。 */
  setBatchKind: (kind: BatchKind) => void;
  /** 改 recipe。换了输入目录同样作废旧扫描。 */
  patchBatchRecipe: (patch: Partial<BatchRecipe>) => void;
  pickBatchInput: (dir: string) => void;
  pickBatchOutput: (dir: string) => void;
  /** 扫一遍输入文件夹，只读，数清楚有几份对口素材。 */
  scanBatchInput: () => Promise<void>;
  /** 跑一趟。命令即刻返回，过程走 batch-event。 */
  runBatch: () => Promise<void>;
  /** 清掉上一趟的明细，方便盯着下一趟。 */
  resetBatch: () => void;
  /** 开机把配方簿读回来；读失败只留空簿。 */
  loadRecipes: () => Promise<void>;
  /** 改配方名输入框。 */
  patchRecipeName: (name: string) => void;
  /** 把当前 recipe 存进配方簿；同名覆盖。 */
  saveRecipeAs: () => Promise<void>;
  /** 从簿子里挑一条盖到当前 recipe；旧的扫描作废，那条配方描述的可能是另一个文件夹。 */
  applyRecipe: (name: string) => void;
  /** 删一条配方。 */
  removeRecipe: (name: string) => Promise<void>;
  /** 把点名几条配方写成 `.aipr`；path 由文件对话框给。 */
  exportRecipes: (entries: BatchRecipeEntry[], path: string) => Promise<void>;
  /** 从 `.aipr` 读配方并进来；path 由文件对话框给。 */
  importRecipes: (path: string) => Promise<void>;
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
  sizeMode: "preset",
  size: "1024x1024",
  sizeW: 1024,
  sizeH: 1024,
  genLayer: "",
  quantizeLayer: "",
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

/** 某一层的配色范围（面板里摆出来的那一排 swatch）落成 hex 表。 */
function layerScopeColors(document: PixelDocument | null, layerId: string): string[] {
  if (!document) return [];
  const layer =
    document.layers.find((item) => item.id === layerId) ?? document.layers[0] ?? null;
  if (!layer) return [];
  const scope = document.palettes.find((item) => item.id === layer.palette_id);
  return (scope ? scope.colors : document.palette).map(rgbaToHex);
}

/**
 * 当前层配色范围的第一个色，给「从没选过色」的情况当默认墨。
 * 画笔的 ink 是 active.color ?? null，而 null 在绘制链里就是「擦成透明」：
 * 不选色直接下笔会静默擦背景，用户只觉得「点了没反应」，还以为是画布坏了。
 */
function defaultInkHex(document: PixelDocument | null, layerId: string): string | null {
  const scope = layerScopeColors(document, layerId);
  return scope.length > 0 ? scope[0] : null;
}

/**
 * 换层/换文档之后手里那支笔该是什么色：原色还在新范围里就留着，
 * 不在就近归队；本来空着、或者压根归不进去，就用新范围的第一个色顶上。
 */
function inkForLayer(
  document: PixelDocument | null,
  layerId: string,
  // color 是可选的：没选过是 undefined，工具里显式清空是 null，两者一回事。
  current: string | null | undefined,
): string | null {
  // 「没选过」和「没色可用」都归一成 null，后面只跟一种空值打交道。
  const held = current ?? null;
  const scope = layerScopeColors(document, layerId);
  if (scope.length === 0) return held;
  if (held !== null && scope.includes(held)) return held;
  const snapped = held === null ? null : nearestHex(scope, held);
  return snapped ?? scope[0];
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
// 批量跑在独立通道上，与 agent-event 各听一条，互不打扰。
let batchUnlisten: (() => void) | null = null;
let booting: Promise<void> | null = null;
let snapshotSeq = 0;

/** 撤销栈上限：再老的笔触就别指望了，省得内存和「撤销到天边」一起失控。 */
const UNDO_LIMIT = 40;
/**
 * 下一次 document_updated 若是编辑器自己触发的，就把改前的文档压进撤销栈。
 * 为什么不让模型改动也进栈：一轮 agent 跑下来事件几十条，会把这些笔触挤没。
 */
let undoCapture = false;
/**
 * 松手失败就缴械。armed 是「下一次 document_updated 属于编辑器」的约定，
 * 可这一步没成功（Rust 拒了 ops、链路断了）时 document_updated 永远不会来，
 * 旗子就一直悬着：下一个到达的 document_updated 会把它当成编辑器改动吃掉，
 * 把模型的改动塞进撤销栈，用户自己随后那一下笔反而没了撤销。
 */
function disarmUndoCapture() {
  undoCapture = false;
}

/**
 * 多久收不到任何 agent 事件就当「卡住了」提醒用户。
 * 思考模型静默一分钟以上很常见（长推理、大图），所以门槛比那更宽：
 * 只在真的很久没动静时打扰，而且只是提醒，替用户做不了主。
 */
export const STALL_SECONDS = 90;

const STALL_MS = STALL_SECONDS * 1000;

let stallTimer: ReturnType<typeof setTimeout> | null = null;

function pushUndo(stack: PixelDocument[], doc: PixelDocument): PixelDocument[] {
  const next = [...stack, doc];
  return next.length > UNDO_LIMIT ? next.slice(next.length - UNDO_LIMIT) : next;
}

export const useStore = create<StoreState & StoreActions>()((setState, getState) => {
  /**
   * 回合级失败：发通知、封口占位节点、把 running 收掉。
   *
   * 只有「这一回合确实走不下去」时才走这里——目前就是发送入口那两处：没会话可发、
   * 消息没能递到 agent。其余一律走 flagKey。
   * 措辞跟着当前界面语言走，Rust 的原文当 {error} 追在后面。
   */
  function failKey(key: TKey, vars?: TVARS) {
    // 发出去的那一句可能还挂着占位节点（思考节点 / 「在处理」）。收尾时
    // 不封口的话，一个跑不起来的回合会在对话里留一枚一直闪的光标，
    // 用户会以为它还在等模型。
    // runStartedAt 只读一次：连着三次 getState() 拿不到同一个快照的类型窄化。
    const now = Date.now();
    const live = getState();
    const elapsed =
      live.running && live.runStartedAt !== null
        ? live.runElapsedMs ?? now - live.runStartedAt
        : live.runElapsedMs;
    setState({
      entries: sealTranscript(getState().entries),
      notice: { text: translate(getState().lang, key, vars), isError: true },
      running: false,
      runStartedAt: null,
      runElapsedMs: elapsed,
    });
  }

  /**
   * 侧道失败：只发一条红色通知，不封口、不动 running。
   *
   * 渲染刷新、侧栏改名、落笔、存盘、MCP 这一路动作随时可能在模型画画的途中失败
   * （多数由用户点出来，也可能由 document_updated 带出来）。这些失败跟这一回合能不
   * 不能继续没有关系，绝不能把 running 一起收掉——否则一个好好活着的回合会被误判成
   * 结束：占位节点封口、光标停闪，用户以为模型已经答完。
   * 回合的生死只由 agent 事件（completed / error / interrupted）和发送入口决定。
   */
  function flagKey(key: TKey, vars?: TVARS) {
    setState({ notice: { text: translate(getState().lang, key, vars), isError: true } });
  }

  /**
   * 把界面上的选中项回传后端。失败只嘟囔一声，不往外抛。
   *
   * 这一下失败意味着模型的下一步编辑会落在旧的选中上，值得让用户知道；但它跟回合
   * 死活无关，也不该把调用方（切层、选帧、document_updated 的收尾）一起带垮。
   */
  function syncActive(id: string, active: ActiveContext) {
    bridge.setActive(id, active).catch((error) => {
      flagKey("store.set_active_failed", { error: String(error) });
    });
  }

  /** 一段本机成功提示（载入、保存）。不是错误，所以不进 fail 那条路。 */
  function noteKey(key: TKey, vars?: TVARS) {
    setState({ notice: { text: translate(getState().lang, key, vars), isError: false } });
  }

  function clearStallWatch() {
    if (stallTimer !== null) {
      clearTimeout(stallTimer);
      stallTimer = null;
    }
  }

  /** 每有一条事件活着进来就重打表。还在吐字就不算卡，一个事件都不来才会响。 */
  function touchStallWatch() {
    clearStallWatch();
    stallTimer = setTimeout(() => {
      stallTimer = null;
      if (getState().running) setState({ stalled: true });
    }, STALL_MS);
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
    // 调色板就是配色范围：选中的色不在新调色板里，就近挪进去。
    // 不挪的话画笔会按字面量把色 intern 进调色板，用户挑的范围就悄悄失守了。
    let active = state.active;
    // color 是可选的：null 与「没选过」在这里都是一个意思，都就近归队。
    if (active.color != null) {
      const snapped = nearestHex(hexesOf(document), active.color);
      if (snapped !== null && snapped !== active.color) active = { ...active, color: snapped };
    } else {
      // 从没选过色：给笔尖顶上第一个色，别让画笔以「擦除」的形态开工。
      active = { ...active, color: defaultInkHex(document, active.layer) };
    }
    if (frameHint !== null) {
      // 结构操作后帧表已变：按预期位置选帧，越界夹到末帧。
      const index = Math.max(0, Math.min(frameHint, document.frames.length - 1));
      const frame = document.frames[index];
      next.frameIndex = index;
      if (frame) active = { ...active, frame: frame.id };
    }
    undoCapture = false;
    // 选色被就近挪过、或者结构操作换了帧：都得让 Rust 侧的 active 跟上，
    // 不然模型的下一步编辑还落在旧的选中上。
    const activeChanged =
      active.color !== state.active.color ||
      active.frame !== state.active.frame ||
      active.layer !== state.active.layer;
    setState({ ...next, active });
    if (activeChanged) {
      const id = getState().activeId;
      if (id) syncActive(id, active);
    }
    void getState().refreshPng();
  }

  /** agent-event 路由：文档事件驱动画布，其余折叠进对话条目。 */
  async function ensureListener() {
    if (unlisten) return;
    unlisten = await bridge.listenAgentEvents((raw: AgentEvent, sessionId: string) => {
      // 别人家会话的事件：切走之后上一个回合要等主循环下一轮轮询（百来毫秒）
      // 才真的停，这期间它还在往外广播。拦在这儿，它的 token 才不会拼进新
      // 对话尾巴，document_updated 才不会把新画布盖成旧画面，收尾事件也不
      // 会替新回合封口。必须早于 touchStallWatch——旧事件照样算「链路活着」
      // 的话，新回合真卡死就被它掩盖过去了。
      if (sessionId !== getState().activeId) return;
      const state = getState();
      // 事件一到就说明链路活着：取消卡住提醒，并给静默计时重新打表。
      touchStallWatch();
      if (state.stalled) setState({ stalled: false });
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
        clearStallWatch();
        // 收尾时把这一圈总共花了多久冻住：runStartedAt 一清，界面上就没得可看了。
        // 三种结局（跑完/报错/打断）都记，失败的那几圈往往才是要盯的那几圈。
        const elapsed =
          state.runStartedAt === null ? state.runElapsedMs : Date.now() - state.runStartedAt;
        setState({
          entries: sealTranscript(reduceEvent(state.entries, raw, state.lang)),
          running: false,
          runStartedAt: null,
          runElapsedMs: elapsed,
          stalled: false,
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

  /** batch-event 路由：事件流折叠成 run 快照，跑完投一条通知栏收尾。 */
  async function ensureBatchListener() {
    if (batchUnlisten) return;
    batchUnlisten = await bridge.listenBatchEvents((raw) => {
      setState({ run: reduceBatchEvent(getState().run, raw) });
      if (raw.kind !== "done") return;
      // 批量可能要跑一会儿，这期间用户很可能切去别的视图；通知栏是唯一不依赖所见视图的回执。
      const lang = getState().lang;
      const failing = raw.failed > 0;
      setState({
        notice: {
          text: translate(
            lang,
            failing ? "batch.done" : "batch.done_clean",
            { ok: raw.ok, skipped: raw.skipped, failed: raw.failed },
          ),
          isError: failing,
        },
      });
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
      // 上一条会话挑过的色留着；没挑过就落到本层配色范围的第一个色上。
      color: getState().active.color ?? defaultInkHex(snapshot.document, firstLayer),
    };
    setState({ active });
    syncActive(id, active);
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
    runStartedAt: null,
    runElapsedMs: null,
    stalled: false,
    usage: null,
    lastQuery: null,
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
    loopLimits: null,
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
    // MCP 默认开着：关掉要在设置里明确按一下，而不是因为一次读失败悄悄消失。
    mcpEnabled: true,
    settingsTab: "models",
    recipe: DEFAULT_BATCH_RECIPE,
    scan: null,
    scanBusy: false,
    run: EMPTY_BATCH_RUN,
    recipeBook: [],
    recipeName: "",
    recipeBusy: false,
    recipeImport: null,

    boot: async () => {
      if (booting) return booting;
      const attempt = (async () => {
        // 配方簿和批量通道一样与会话无关：开机读回来，用户随时能从簿子里挑一条。
        await getState().loadRecipes();
        await ensureListener();
        // 批量通道与会话无关，开机听上就行：用户随时可能从工作台起一趟。
        await ensureBatchListener();
        let models: ModelsView;
        try {
          models = await bridge.listModels();
        } catch (error) {
          flagKey("store.read_models_failed", { error: String(error) });
          models = EMPTY_MODELS;
        }
        setState({ models });
        await getState().refreshMcpEnabled();
        await getState().refreshLoopLimits();
        let mcpServers: McpServersView;
        try {
          mcpServers = await bridge.listMcpServers();
        } catch (error) {
          flagKey("store.read_mcp_failed", { error: String(error) });
          mcpServers = EMPTY_MCP;
        }
        setState({ mcpServers });
        let sessions: SessionInfo[] = [];
        try {
          sessions = await bridge.listSessions();
        } catch (error) {
          flagKey("store.read_sessions_failed", { error: String(error) });
        }
        if (sessions.length === 0) {
          const created = await bridge.createSession();
          sessions = [created];
        }
        sessions = sortSessions(sessions);
        const first = sessions[sessions.length - 1];
        if (first) {
          setState({ sessions, activeId: first.id });
          await loadDocument(first.id);
        }
        setState({ booted: true });
      })();
      booting = attempt;
      // 启动失败不能把这次尝试记成永久结论：一次文档读取抽风之后，若还挂着这个
      // reject，后面每次 boot() 都拿到同一张判死刑的票，界面永远停在未就绪。
      // 这期间若又有人重新 boot，别把新的那次一起作废。
      attempt.catch(() => {
        if (booting === attempt) booting = null;
      });
      return booting;
    },

    selectSession: async (id) => {
      if (getState().running) await getState().interrupt();
      setState({
        activeId: id,
      entries: emptyTranscript(),
      running: false,
      runStartedAt: null,
      runElapsedMs: null,
      stalled: false,
      usage: null,
      lastQuery: null,
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
        // 新会话按顺序位追加：Rust 会把它的 order 排在已有会话之后。
        sessions: sortSessions([...getState().sessions, info]),
        activeId: info.id,
        entries: emptyTranscript(),
        running: false,
        runStartedAt: null,
        runElapsedMs: null,
        stalled: false,
        usage: null,
        lastQuery: null,
        attachments: [],
      frameIndex: 0,
    });
      await loadDocument(info.id);
      // 只有从尺寸弹窗进来的才告知：删掉最后一个会话时那次自动补建不该吵用户。
      if (width && height) {
        noteKey("sidebar.created_hint", { width: info.width, height: info.height });
      }
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

    bindSessionRole: async (role, modelId) => {
      const id = getState().activeId;
      if (!id) return;
      const info = await bridge.bindSessionRole(id, role, modelId);
      setState({
        sessions: getState().sessions.map((s) => (s.id === id ? info : s)),
      });
      // 多了一个会生图 / 识图 / 读视频的模型，之前灰着的流程可能就能跑了。
      await getState().refreshWorkflows();
    },

    clearSessionRole: async (role) => {
      const id = getState().activeId;
      if (!id) return;
      const info = await bridge.clearSessionRole(id, role);
      setState({
        sessions: getState().sessions.map((s) => (s.id === id ? info : s)),
      });
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
      // 占位气泡要不要画成「思考节点」：看会话主模型有没有勾「会思考」。
      const state = getState();
      const session = state.sessions.find((s) => s.id === state.activeId);
      const model = state.models.entries.find((m) => m.id === session?.model_id);
      const thinking = model?.capabilities.reasoning ?? false;
      setState({
        entries: pushPendingAssistant(
          pushUserMessage(state.entries, trimmed, attachments),
          thinking,
        ),
        attachments: [],
        running: true,
        runStartedAt: Date.now(),
        // 新一轮开跑，上一圈的耗时就别赖在界面上挡事了。
        runElapsedMs: null,
        stalled: false,
        lastQuery: { text: trimmed, attachments },
        notice: null,
      });
      // 从这一刻起盯着静默：路上一个事件都不来的话，界面上会出现「可能卡住了」。
      touchStallWatch();
      try {
        await bridge.sendMessage(id, trimmed, payload);
      } catch (error) {
        clearStallWatch();
        failKey("store.send_failed", { error: String(error) });
      }
    },

    retry: async () => {
      const last = getState().lastQuery;
      if (!last) return;
      // 占位与运行态在 send 里统一布置；这里只把上次的字和附件还给发送入口。
      setState({ attachments: last.attachments });
      await getState().send(last.text);
    },

    interrupt: async () => {
      const id = getState().activeId;
      if (!id) return;
      try {
        await bridge.interrupt(id);
      } catch (error) {
        flagKey("store.interrupt_failed", { error: String(error) });
      }
    },

    upsertModel: async (config) => {
      const models = await bridge.upsertModel(config);
      setState({ models });
      // Rust 已经把活着的会话重绑到这份新配置上，侧栏的模型名就是从那读的：
      // 不重新拉一遍，用户在设置里改完名字看到的还是旧的那串。
      await getState().refreshSessions();
      const bound = getState().sessions.find((s) => s.id === getState().activeId);
      // 会话还挂在「未设置」上（没配模型就开了会话，或者模型被删了）：补绑到刚存好的定义，
      // 不然用户存完还得回顶栏手动挑一次，而挑之前发消息只会拿到 provider 的报错。
      if (bound && !models.entries.some((m) => m.id === bound.model_id)) {
        await getState().bindSessionModel(config.id);
        return;
      }
      // 改的是当前会话绑的那个模型，能力勾选就得重新反映到目录上。
      if (!bound || bound.model_id === config.id) {
        await getState().refreshWorkflows();
      }
    },

    removeModel: async (id) => {
      const models = await bridge.removeModel(id);
      setState({ models });
      // 删掉的若正是会话正绑着的定义，Rust 会让它落到当前生效模型上：侧栏要刷新，
      // 能力勾选也跟着变（换了个模型就等于换了一套能跑的流程）。
      await getState().refreshSessions();
      const bound = getState().sessions.find((s) => s.id === getState().activeId);
      if (bound && !models.entries.some((m) => m.id === bound.model_id)) {
        await getState().refreshWorkflows();
      }
    },

    activateModel: async (id) => {
      const models = await bridge.setActiveModel(id);
      setState({ models });
      // 切默认模型会把还没绑过模型的会话一起带过去：侧栏模型名与流程可用性都跟着变。
      await getState().refreshSessions();
      await getState().refreshWorkflows();
    },

    refreshLoopLimits: async () => {
      try {
        setState({ loopLimits: await bridge.loopLimits() });
      } catch {
        // 读不回来就沿用当前界面上的值：护栏是兜底，不该因为一次读失败把界面打空。
      }
    },

    saveLoopLimits: async (next) => {
      // 先按界面显示：Rust 会把越界的值夹回来，回值才是真正生效的那份。
      setState({ loopLimits: next });
      try {
        setState({ loopLimits: await bridge.setLoopLimits(next) });
      } catch (error) {
        flagKey("store.save_limits_failed", { error: String(error) });
        await getState().refreshLoopLimits();
      }
    },

    fetchProviderModels: async (params) => bridge.fetchModelList(params),

    refreshMcp: async () => {
      try {
        setState({ mcpServers: await bridge.listMcpServers() });
      } catch (error) {
        flagKey("store.read_mcp_failed", { error: String(error) });
      }
    },

    openMcp: () => setState({ mcpOpen: true }),

    closeMcp: () => setState({ mcpOpen: false }),

    refreshMcpEnabled: async () => {
      try {
        setState({ mcpEnabled: await bridge.mcpEnabled() });
      } catch {
        // 读不回来就按开着显示：用户看到的开关该和他上一次设的一致。
      }
    },

    setMcpEnabled: async (enabled) => {
      try {
        // 回值才算生效：Rust 可能拒掉（比如磁盘写不进去），界面上要跟着回值走。
        setState({ mcpEnabled: await bridge.setMcpEnabled(enabled) });
      } catch (error) {
        flagKey("store.set_mcp_enabled_failed", { error: String(error) });
        await getState().refreshMcpEnabled();
      }
    },

    probeImage: async (params) => bridge.probeImageCapability(params),

    setSettingsTab: (tab) => setState({ settingsTab: tab }),

    renameSession: async (id, title) => {
      try {
        const info = await bridge.renameSession(id, title);
        setState({
          sessions: sortSessions(
            getState().sessions.map((session) => (session.id === id ? info : session)),
          ),
        });
      } catch (error) {
        flagKey("store.rename_session_failed", { error: String(error) });
      }
    },

    reorderSessions: async (ids) => {
      // 先按新次序显示再上报：拖动已经是个明确意图，等后端反而会看着像卡住。
      const known = new Map(getState().sessions.map((session) => [session.id, session]));
      const ordered: SessionInfo[] = [];
      for (const id of ids) {
        const hit = known.get(id);
        // 拖进来的 id 可能已经不在了（删过会话），少一个不补位，剩下照原次序。
        if (hit) ordered.push(hit);
      }
      setState({ sessions: ordered });
      try {
        await bridge.reorderSessions(ids);
      } catch (error) {
        flagKey("store.reorder_sessions_failed", { error: String(error) });
      }
      await getState().refreshSessions();
    },

    upsertMcpServer: async (config) => {
      setState({ mcpBusy: true });
      try {
        setState({ mcpServers: await bridge.upsertMcpServer(config) });
      } catch (error) {
        flagKey("store.save_mcp_failed", { error: String(error) });
      } finally {
        setState({ mcpBusy: false });
      }
    },

    removeMcpServer: async (name) => {
      setState({ mcpBusy: true });
      try {
        setState({ mcpServers: await bridge.removeMcpServer(name) });
      } catch (error) {
        flagKey("store.remove_mcp_failed", { error: String(error) });
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
        flagKey("store.connect_mcp_failed", { error: String(error) });
      } finally {
        setState({ mcpBusy: false });
      }
    },

    disconnectMcpServer: async (name) => {
      setState({ mcpBusy: true });
      try {
        setState({ mcpServers: await bridge.disconnectMcpServer(name) });
      } catch (error) {
        flagKey("store.disconnect_mcp_failed", { error: String(error) });
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
          flagKey("store.read_image_failed", { name: baseName(path), error: String(error) });
        }
      }
      if (pending.length > 0) setState({ attachments: [...getState().attachments, ...pending] });
    },

    attachSnapshot: async () => {
      const id = getState().activeId;
      if (!id) return;
      try {
        // 带上当前帧号：缺省那一趟会把所有帧横向铺成一条 PNG，
        // 模型拿到的就是一幅被拉长的帧序列，看不出用户在改哪一帧。
        const url = await bridge.pngUrl(id, getState().frameIndex);
        const { mediaType, data } = stripDataUrl(url);
        if (!data) {
          flagKey("store.empty_snapshot");
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
        flagKey("store.snapshot_failed", { error: String(error) });
      }
    },

    removeAttachment: (key) => {
      setState({ attachments: getState().attachments.filter((a) => a.key !== key) });
    },

    setActiveLayer: (layerId) => {
      const id = getState().activeId;
      if (!id) return;
      // 配色范围跟着层走：换层之后手里那个色不一定还在新层的范围里，
      // 就近归队；空着就顶上第一个色。不然画笔会按字面量把色 intern 进
      // 调色板，用户锁好的范围悄悄失守。
      const active = {
        ...getState().active,
        layer: layerId,
        color: inkForLayer(getState().document, layerId, getState().active.color),
      };
      setState({ active });
      syncActive(id, active);
    },

    setActiveFrame: (index) => {
      const id = getState().activeId;
      const doc = getState().document;
      if (!id || !doc) return;
      const frame = doc.frames[index];
      if (!frame) return;
      const active = { ...getState().active, frame: frame.id };
      setState({ active, frameIndex: index });
      syncActive(id, active);
      void getState().refreshPng();
    },

    setActiveColor: (color) => {
      const id = getState().activeId;
      if (!id) return;
      const active = { ...getState().active, color };
      setState({ active });
      syncActive(id, active);
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
        // 第 0 帧也必须显式带上：后端收到 None 会把所有帧横向铺开，
        // 画布里就会出现一条被拉长的帧序列。
        const url = await bridge.pngUrl(id, frameIndex);
        if (getState().revision === requested) setState({ pngUrl: url });
      } catch (error) {
        flagKey("store.render_failed", { error: String(error) });
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
        flagKey("store.open_aip_failed", { error: String(error) });
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
        flagKey("store.save_failed", { error: String(error) });
      } finally {
        setState({ busy: false });
      }
    },

    exportDocument: async (format, path, options) => {
      const id = getState().activeId;
      if (!id) return;
      setState({ busy: true });
      try {
        await bridge.documentExport(id, format, path, options);
        noteKey("store.exported", { name: baseName(path) });
      } catch (error) {
        flagKey("store.export_failed", { error: String(error) });
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
        flagKey("store.read_aip_failed", { error: String(error) });
        return null;
      }
    },

    clearNotice: () => setState({ notice: null }),
    warnKey: flagKey,
    noteKey,

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
        flagKey("store.no_session");
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
        flagKey("store.no_session");
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
        flagKey("store.no_session");
        return;
      }
      if (idea.trim() === "") {
        flagKey("store.refine_empty");
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
        flagKey("store.no_session");
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
        flagKey("store.no_session");
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

    fillGenPrompt: (prompt, referencePath) =>
      setState({
        kind: "image_gen",
        outcome: null,
        outcomeError: null,
        dockDraft: {
          ...getState().dockDraft,
          prompt,
          // 带了参考图就把垫图源切到这张磁盘图：生图面板直接看得见它。
          // 图是空的就不动用户原来选好的垫图方式。
          ...(referencePath ? { genSource: "file", genPath: referencePath } : {}),
        },
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
        flagKey("store.approval_failed", { error: String(error) });
      }
    },

    paintStroke: async (cells, inkOverride) => {
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
          // 传进来的 override 是给橡皮的：null 明说「这一笔就是擦」。
          color: inkOverride !== undefined ? inkOverride : (active.color ?? null),
        });
      } catch (error) {
        disarmUndoCapture();
        flagKey("store.paint_failed", { error: String(error) });
      }
    },

    fillCell: async (x, y, inkOverride) => {
      const id = getState().activeId;
      const active = getState().active;
      if (!id) return;
      undoCapture = true;
      try {
        await bridge.fillCells(
          id,
          active.layer,
          active.frame,
          x,
          y,
          inkOverride !== undefined ? inkOverride : (active.color ?? null),
        );
      } catch (error) {
        disarmUndoCapture();
        flagKey("store.fill_failed", { error: String(error) });
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
        disarmUndoCapture();
        flagKey("store.edit_failed", { error: String(error) });
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

    setPaletteColors: async (colors) => {
      // 空配色等于把整幅擦透明，那是毁画面不是换风格，拦在门外。
      if (colors.length === 0) return false;
      const revision = await getState().runEditorOps([{ op: "set_palette", colors }]);
      return revision !== null;
    },

    forkPalette: async (fromId, name, layerId) => {
      // layerId 不传就跟着激活层走；配色区伺候的是别层时，显式把它带过来。
      const target = layerId ?? getState().active.layer;
      if (!target) return;
      // 改内置预设的唯一入口：Rust 落点是副本，原套一个色都不动。
      // 顺带把当前层指过去——用户改的就是眼前这一层的范围。
      await getState().runEditorOps([
        {
          op: "create_palette",
          name,
          from: fromId,
          colors: [],
          layer: target,
        },
      ]);
    },

    createPalette: async (name, colors, layerId) => {
      const target = layerId ?? getState().active.layer;
      if (!target) return;
      await getState().runEditorOps([
        { op: "create_palette", name, colors, layer: target },
      ]);
    },

    deletePalette: async (id) => {
      await getState().runEditorOps([{ op: "delete_palette", id }]);
    },

    renamePalette: async (id, name) => {
      const trimmed = name.trim();
      if (trimmed === "") return;
      await getState().runEditorOps([{ op: "rename_palette", id, name: trimmed }]);
    },

    addPaletteColor: async (id, color) => {
      const hex = color.trim();
      if (hex === "") return;
      await getState().runEditorOps([{ op: "add_palette_color", id, color: hex }]);
    },

    removePaletteColor: async (id, index) => {
      await getState().runEditorOps([{ op: "remove_palette_color", id, index }]);
    },

    setLayerPalette: async (layerId, paletteId) => {
      if (!paletteId) return;
      await getState().runEditorOps([{ op: "set_layer_palette", layer: layerId, palette_id: paletteId }]);
    },

    setLayerLocked: async (layerId, locked) => {
      await getState().runEditorOps([{ op: "set_layer_locked", layer: layerId, locked }]);
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

    setFrameDuration: async (durationMs) => {
      const state = getState();
      const current = state.document?.frames[state.frameIndex];
      if (!current) return;
      // 与 Rust MAX_FRAME_DURATION_MS 对齐：下限 1ms（GIF 那边自己会抬到 20ms）。
      const next = Math.max(1, Math.min(60000, Math.round(durationMs)));
      if (next === current.duration_ms) return;
      await getState().runEditorOps([
        { op: "set_frame_duration", id: current.id, duration_ms: next },
      ]);
    },

    setLayerVisible: async (layerId, visible) => {
      await getState().runEditorOps([{ op: "set_layer_properties", id: layerId, visible }]);
    },

    setLayerOpacity: async (layerId, opacity) => {
      const next = Math.max(0, Math.min(255, Math.round(opacity)));
      await getState().runEditorOps([{ op: "set_layer_properties", id: layerId, opacity: next }]);
    },

    addLayer: async () => {
      const document = getState().document;
      const activeLayerId = getState().active.layer;
      if (!document) return;
      // 插在当前层之后：新画的玩意儿多半就是要在眼前这层的上面。
      // after 传 null 时 Rust 落在栈顶，行为和这里一致，但显式贴在当前层后面，
      // 才能保证「新层挨着我刚画的那层」——图层排序时尤其疼。
      const activeIndex = document.layers.findIndex((layer) => layer.id === activeLayerId);
      const after = activeIndex >= 0 ? document.layers[activeIndex].id : null;
      await getState().runEditorOps([{ op: "create_layer", after }]);
    },

    deleteLayer: async (layerId) => {
      const document = getState().document;
      if (!document) return;
      // 没点名就删当前层；只剩一层时谁也删不动，Rust 那边也会拒。
      const target = layerId ?? getState().active.layer;
      if (document.layers.length <= 1 || !target) return;
      const index = document.layers.findIndex((layer) => layer.id === target);
      if (index < 0) return;
      // 删的是当前层，选中得挪走：Rust 会重排，前端不跟上就会指着一个不存在的层。
      const fallback = document.layers[index === 0 ? 1 : index - 1];
      await getState().runEditorOps([{ op: "delete_layer", id: target }]);
      if (target === getState().active.layer && fallback) {
        getState().setActiveLayer(fallback.id);
      }
    },

    renameLayer: async (layerId, name) => {
      const trimmed = name.trim();
      // 空白名一律当取消：不留一个看不见的空图层，也别为一个空串走一趟后端。
      if (trimmed === "") return;
      const current = getState().document?.layers.find((layer) => layer.id === layerId);
      if (!current || current.name === trimmed) return;
      await getState().runEditorOps([{ op: "rename_layer", id: layerId, name: trimmed }]);
    },

    moveLayer: async (delta) => {
      const state = getState();
      const document = state.document;
      const layerId = state.active.layer;
      if (!document || !layerId) return;
      const index = document.layers.findIndex((layer) => layer.id === layerId);
      if (index < 0) return;
      const target = index + delta;
      if (target < 0 || target >= document.layers.length) return;
      await getState().runEditorOps([{ op: "move_layer", id: layerId, to_index: target }]);
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
        flagKey("store.undo_failed", { error: String(error) });
      }
    },

    resizeCanvas: async (width, height) => {
      const id = getState().activeId;
      const nextWidth = Math.round(width);
      const nextHeight = Math.round(height);
      if (!id || nextWidth < 1 || nextHeight < 1) return;
      const current = getState().document;
      // 尺寸没动就别惊动后端：Rust 那边也会短路，这里挡掉只是省一趟往返。
      if (current && current.width === nextWidth && current.height === nextHeight) return;
      try {
        // Rust 收到就广播 document_updated，画布与预览都由那条事件接走；
        // 这里再主动拉一次文档，是防止事件被别的路径抢先时落下。
        await bridge.resizeCanvas(id, nextWidth, nextHeight);
        await loadDocument(id);
        // 侧栏那一行画着「宽x高」：改完不重拉，那一行会一直停在旧尺寸上。
        await getState().refreshSessions();
      } catch (error) {
        flagKey("store.resize_failed", { error: String(error) });
      }
    },

    refreshSessions: async () => {
      try {
        const sessions = await bridge.listSessions();
        setState({ sessions: sortSessions(sessions) });
      } catch {
        // 会话列表刷新失败不影响主流程
      }
    },

    // ---------- 批量工作台 ----------
    setBatchKind: (kind) => {
      setState((s) => ({ ...s, recipe: { ...s.recipe, kind }, scan: null, run: EMPTY_BATCH_RUN }));
    },

    patchBatchRecipe: (patch) => {
      setState((s) => {
        const recipe = { ...s.recipe, ...patch };
        // 输入目录一换，旧清单描述的就是另一个文件夹：作废，逼用户重扫。
        const stale = recipe.input_dir !== s.recipe.input_dir;
        return { ...s, recipe, scan: stale ? null : s.scan, run: stale ? EMPTY_BATCH_RUN : s.run };
      });
    },

    pickBatchInput: (dir) => getState().patchBatchRecipe({ input_dir: dir }),

    pickBatchOutput: (dir) => getState().patchBatchRecipe({ output_dir: dir }),

    scanBatchInput: async () => {
      const { recipe } = getState();
      if (recipe.input_dir.trim() === "") {
        flagKey("batch.need_dirs");
        return;
      }
      setState({ scanBusy: true });
      try {
        const scan = await bridge.scanBatch(recipe.input_dir, recipe.kind);
        // 新一次扫描意味着上一趟的明细作废：那批行属旧目录或旧尺度。
        setState({ scan, run: EMPTY_BATCH_RUN });
      } catch (error) {
        setState({ scan: null });
        flagKey("batch.scan_failed", { error: String(error) });
      } finally {
        setState({ scanBusy: false });
      }
    },

    runBatch: async () => {
      const { recipe, scan, run } = getState();
      if (recipe.input_dir.trim() === "" || recipe.output_dir.trim() === "") {
        flagKey("batch.need_dirs");
        return;
      }
      if (!scan || !scanMatchesKind(scan, recipe.kind) || !canRunBatch(scan, run)) {
        flagKey("batch.need_scan");
        return;
      }
      // 命令即刻返回、事件随后才来；先把按钮转上，免得连点开出两趟互踩。
      setState({ run: { ...EMPTY_BATCH_RUN, running: true, total: scan.count } });
      try {
        await bridge.runBatch(recipe);
      } catch (error) {
        setState({ run: { ...EMPTY_BATCH_RUN, error: String(error) } });
        flagKey("batch.run_failed", { error: String(error) });
      }
    },

    resetBatch: () => setState({ run: EMPTY_BATCH_RUN }),

    // ---------- 批量配方簿 ----------
    loadRecipes: async () => {
      try {
        setState({ recipeBook: await bridge.listBatchRecipes() });
      } catch (error) {
        // 配方簿是锦上添花：读不到就是没有，别为它弹错误。
        setState({ recipeBook: [] });
        flagKey("batch.read_recipes_failed", { error: String(error) });
      }
    },

    patchRecipeName: (name) => setState({ recipeName: name }),

    saveRecipeAs: async () => {
      const { recipeName, recipe, recipeBook } = getState();
      if (recipeNameProblem(recipeName, recipeBook) !== null) return; // UI 已经禁用了按钮
      setState({ recipeBusy: true });
      try {
        await bridge.saveBatchRecipe(recipeName.trim(), recipe);
        // 清空名字而不是留住它：簿子里新出现的这一条就是「存好了」的确认。
        setState({ recipeBook: await bridge.listBatchRecipes(), recipeName: "" });
      } catch (error) {
        flagKey("batch.recipe_save_failed", { error: String(error) });
      } finally {
        setState({ recipeBusy: false });
      }
    },

    applyRecipe: (name) => {
      const entry = getState().recipeBook.find((item) => item.name === name);
      if (!entry) return;
      setState({
        recipe: { ...DEFAULT_BATCH_RECIPE, ...entry.recipe },
        recipeName: name,
        // 这条配方记的可能是另一个文件夹：旧清单立刻作废，宁可重扫一次。
        scan: null,
        run: EMPTY_BATCH_RUN,
      });
    },

    removeRecipe: async (name) => {
      try {
        await bridge.deleteBatchRecipe(name);
        const recipeBook = await bridge.listBatchRecipes();
        // 删的正是当前这条，就把输入框一起清掉，免得名字还在、条目没了。
        setState({ recipeBook, recipeName: getState().recipeName === name ? "" : getState().recipeName });
      } catch (error) {
        flagKey("batch.recipe_delete_failed", { error: String(error) });
      }
    },

    exportRecipes: async (entries, path) => {
      setState({ recipeBusy: true });
      try {
        // 落盘路径由 Rust 定：它负责补 `.aipr` 后缀，所以提示里念的是最终路径。
        const written = await bridge.exportBatchRecipes(entries, path);
        noteKey("batch.recipe_exported", { count: entries.length, path: written });
      } catch (error) {
        flagKey("batch.recipe_export_failed", { error: String(error) });
      } finally {
        setState({ recipeBusy: false });
      }
    },

    importRecipes: async (path) => {
      setState({ recipeBusy: true });
      try {
        const report = await bridge.importBatchRecipes(path);
        // 回执自带合并后的整本簿子：刷新视图不必再问一次 Rust。
        setState({ recipeBook: report.entries, recipeImport: report });
      } catch (error) {
        setState({ recipeImport: null });
        flagKey("batch.recipe_import_failed", { error: String(error) });
      } finally {
        setState({ recipeBusy: false });
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
    layers: [
      // 新图层默认落在 Sweetie 16 上、而且是开着的：用户第一笔就能画出想要的颜色。
      { id: "L0", name: "Layer 1", visible: true, opacity: 255, palette_id: "sweetie16", locked: false },
    ],
    frames: [{ id: "F0", duration_ms: 100 }],
    cels: { L0: { F0: { indices: new Array(width * height).fill(0) } } },
    palettes: BUILTIN_PALETTES,
    revision: 0,
  };
}
