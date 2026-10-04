// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 应用状态：会话表、模型配置、对话条目、权威文档与画布预览。
// Rust 主循环只通过 agent-event 说话；所有副作用都收敛成 bridge 调用。

import { create } from "zustand";

import { renderUiText, translate, type Lang, type TVARS, type TKey } from "./i18n";
import * as bridge from "./bridge";
import type { ExportFormat } from "./bridge";
import { PALETTE_PRESETS, nearestHex, parseHex, rgbaToHex } from "./palette";
import {
  emptyTranscript,
  historyToTranscript,
  pushPendingAssistant,
  pushSideNotice,
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
  SessionListChanged,
  EditorOperation,
  DocPatch,
  DockKind,
  DockDraft,
  ImageGenParams,
  ImageSupport,
  McpServerConfig,
  McpServersView,
  McpServerStatusView,
  Message,
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
  SessionShadow,
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
import { createCoalescer, type Coalescer } from "./coalesce";

import { MAX_STACKED_PRESETS, normalizePresetStack, uniquePresetIds } from "./presets";

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
  /**
   * pngUrl 到底属于哪一帧。切帧时帧层是同步重画的，而权威 PNG 要等后端一趟
   * 往返；这期间旧的 PNG 还挂在 <img> 上，和新帧层叠在一起就是一张花脸。
   * 帧号对不上就让权威图让位，等新的回来再出场。-1 表示手上没有图。
   */
  pngFrame: number;
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
  /** 撤销过又还没被新改动作废的快照。空表示没有可重做的一步。 */
  redoStack: PixelDocument[];
  notice: { text: string; isError: boolean } | null;
  settingsOpen: boolean;
  /** 连接/保存进行中：期间按钮全灭，防止连点把服务器打爆。 */
  mcpBusy: boolean;
  /** 界面语言。默认中文，用户可在设置里改成英语；只影响这一层，不回灌 Rust。 */
  lang: Lang;
  /** 运行护栏当前值；为 null 表示还没读过 Rust。 */
  loopLimits: LoopLimits | null;
  /** MCP 总开关。true = 模型看得见用户自配的外部工具。 */
  mcpEnabled: boolean;
  /** 本程序当 MCP 服务端的运行快照；null = 还没读过 Rust。 */
  mcpServerStatus: McpServerStatusView | null;
  /** 服务端起停进行中：期间开关与端口全灭，防止连点把监听器打爆。 */
  mcpServerBusy: boolean;
  /** 设置弹窗停在哪一页：模型 / 行为护栏 / 关于。 */
  settingsTab: SettingsTab;
  /** 会话输入区里钉住的画风 id；null = 让模型按这句话自己判断。
  *  只在发消息那一刻读一次，不进会话状态——它是「这一句的偏好」，不是会话属性。 */
  styleOverride: string | null;
  /** 会话输入区里点的内置提示词预设 id 集合（写实渲染、微细结构…）；空 = 不限。
   *  与画风那柄同等级、同寿命：同样只在这一句上生效，同样不进会话状态。
   *  分管另一半——画风钉色数和描边，预设讲这张图按什么规矩收尾。
   *  能叠几条（上限 `presets.ts` 的 MAX_STACKED_PRESETS）：细节这件事是乘法，
   *  「写实渲染」管整张图按什么规矩收尾，「微细结构」管最后一两个像素放哪里，
   *  两条一起才凑得成一张写实的图。 */
  presetOverrides: string[];
  /** 切走后仍在跑的会话现场。键是会话 id，只在「后台确实有回合」时才存在。
   *
   * 会话之间在 Rust 是并发的，切走不打断；而进行中的 token 只活在事件流里，
   * 主循环要等轮次收尾才整段落库。影子就是那一段的暂存区。 */
  sessionShadows: Record<string, SessionShadow>;
  /** 新建会话弹窗开着。开机一条会话都没有时弹一次，用户关掉就再不自动弹。 */
  createPromptOpen: boolean;
  /** 工具块展开状态，按工具调用 id 记。跨会话重载也不丢：用户摊开的 JSON 不该
  * 因为切走再回来就自己合上。 */
  /** 当前会话的 .aip 落盘路径。null = 还没存过，关窗时要给用户一个「存哪儿」。 */
  projectPath: string | null;
  /** 当前会话有没有没存进 .aip 的改动。关窗问的就是这一笔账。 */
  projectDirty: boolean;
  /** 每个会话一份落盘账本。切走再切回来，不至于忘了自己还没存。 */
  projectLedger: Record<string, { path: string | null; dirty: boolean }>;
  /** 关窗问询弹窗开着。true = Rust 把窗口按住了，就等一个答复。 */
  closeGuardOpen: boolean;
  toolOpen: Record<string, boolean>;
  }

export interface StoreActions {
  setStyleOverride: (style: string | null) => void;
  setPresetOverrides: (presets: string[]) => void;
  /** 摊开/收起某条工具调用；autoOpen 是这一条的默认姿态（分流节点默认摊开）。 */
  toggleToolOpen: (id: string, autoOpen: boolean) => void;
  /** 当前会话动过了：记一笔「没存」。存过的路（openAip / saveAip）自己会清。 */
  markProjectDirty: () => void;
  /** 对关窗问询的答复。quit = true 才放行退出，false 只是收起弹窗接着用。 */
  answerClose: (quit: boolean) => void;
  boot: () => Promise<void>;
  setLang: (lang: Lang) => void;
  selectSession: (id: string) => Promise<void>;
  createSession: (width?: number, height?: number, title?: string) => Promise<void>;
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
  /** 摊开 / 收起新建会话弹窗。空会话状态下这个弹窗就是新建的唯一入口。 */
  openCreatePrompt: () => void;
  closeCreatePrompt: () => void;
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
  /** 读一次 MCP 总开关。 */
  refreshMcpEnabled: () => Promise<void>;
  /** 开/关 MCP。Rust 当场作用到活着的会话，回值才是生效的那份。 */
  setMcpEnabled: (enabled: boolean) => Promise<void>;
  /** 读一次本程序 MCP 服务端的运行快照。 */
  refreshMcpServer: () => Promise<void>;
  /** 开/关服务端。回值才算生效：bind 失败时 Rust 会把原因带回来。 */
  setMcpServerEnabled: (enabled: boolean) => Promise<void>;
  /** 换监听端口；0 = 让内核挑。改端口会自动重起监听。 */
  setMcpServerPort: (port: number) => Promise<void>;
  /** 先停再起，读配置不变。端口被占的僵局靠它解。 */
  restartMcpServer: () => Promise<void>;
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
  /** 重拉整份文档；刚导入过外来文件时传 authoritative，让低号文档也能盖掉本地。 */
  refreshDocument: (authoritative?: boolean) => Promise<void>;
  syncSelection: () => Promise<void>;
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
  /** 从零起一套；不给名字就沿用 Rust 的兜底命名。 */
  createPalette: (name: string, colors: string[], layerId?: string) => Promise<void>;
  /** 删掉一套自定义范围。内置的、还被引用的，Rust 会拒。 */
  deletePalette: (id: string, fallback?: string | null) => Promise<void>;
  renamePalette: (id: string, name: string) => Promise<void>;
  /** 往范围里添一个颜色。已经有这个色就当无事发生。 */
  addPaletteColor: (id: string, color: string) => Promise<void>;
  /** 从范围里拿掉一个颜色；画面不动，只是以后不许再用了。 */
  /** 删范围里的一个颜色。带 replacement 时画面上用了它的像素跟着改写过去。 */
  removePaletteColor: (id: string, index: number, replacement?: string | null) => Promise<void>;
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
  /** 把撤销掉的画面找回来：重做栈见底就什么都不做。 */
  redoEdit: () => Promise<void>;
  /** 改画布宽高：左上角锚定，原有像素留住，新区域透明。 */
  resizeCanvas: (width: number, height: number) => Promise<void>;
  /** 铺纸娃娃白膜：整列帧按部件分区画人形剪影，落撤销栈，一步 undo 回来。 */
  layPaperdollBase: (layer?: string) => Promise<void>;
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

/** 「新建会话」弹窗的免打扰标记。关过一次就再也不自动弹：用户读过一遍了，
 * 每回开机都问一遍不是贴心，是骚扰。空会话下侧栏那个 + 永远在那儿。 */
const CREATE_PROMPT_KEY = "aipixel.create_prompt_dismissed";

function readCreatePromptDismissed(): boolean {
  try {
    return globalThis.localStorage?.getItem(CREATE_PROMPT_KEY) === "1";
  } catch {
    // 存不了就当关过：宁可少弹一次，也不能每回开机都糊一脸。
    return true;
  }
}

function writeCreatePromptDismissed(): void {
  try {
    globalThis.localStorage?.setItem(CREATE_PROMPT_KEY, "1");
  } catch {
    // 隐身模式下写不进去，这次会话不弹就是了，别让它把界面卡住。
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
/**
 * 撤销/重做排队链：同一时刻只跑一档「把整份文档拍回后端」。
 * 定义在模块级而不是 store 里，是为了切会话、重开窗口也共用同一条链。
 */
let restoreChain: Promise<unknown> = Promise.resolve();
// 批量跑在独立通道上，与 agent-event 各听一条，互不打扰。
let batchUnlisten: (() => void) | null = null;
// 会话簿变化（外部 MCP 建/删/改名、导入、改尺寸）常住一条通道。
let sessionUnlisten: (() => void) | null = null;
// 关窗问询也常住一条：Rust 按住窗口时只有它能应答，注销了就等于没人应答。
let closeUnlisten: (() => void) | null = null;
let booting: Promise<void> | null = null;
let snapshotSeq = 0;

/** 撤销栈上限：再老的笔触就别指望了，省得内存和「撤销到天边」一起失控。 */
const UNDO_LIMIT = 40;
/**
 * 撤销栈的字节预算。快照存的是整个文档（number[] 索引网格，约 4 字节/格），
 * 一张 1024x1024、30 层 60 帧的怪物画布单份就要 256MB，40 份能把内存吃到 10GB，
 * 不拦就是 OOM 崩溃。低于预算时照旧保留满 40 步，只有真的大文档才自动少存几份，
 * 最少也留 1 步——一步都没有的话整个撤销就废了。
 */
const UNDO_BYTE_BUDGET = 256 * 1024 * 1024;
/**
 * 预算读数口子。做成可变对象是给测试留的缝：把额度一压，用小文档就能验
 * 「超预算先丢最老、至少留一步」这套裁剪，不必真造几百 MB 的数组。
 */
export const undoBudget = { bytes: UNDO_BYTE_BUDGET };

/** 文档体积的备忘：同一份快照会被反复估算（每压一次栈就全量过一遍），不可变，可放心缓存。 */
const docBytesCache = new WeakMap<PixelDocument, number>();

function estimateDocBytes(doc: PixelDocument): number {
  const cached = docBytesCache.get(doc);
  if (cached !== undefined) return cached;
  let cells = 0;
  for (const frames of Object.values(doc.cels)) {
    for (const cel of Object.values(frames)) cells += cel.indices.length;
  }
  const bytes = cells * 4;
  docBytesCache.set(doc, bytes);
  return bytes;
}
/**
 * 一次成功的编辑器改动落下的两笔账：改前的文档进撤销栈，重做栈作废。
 * 为什么不让模型改动也进栈：一轮 agent 跑下来事件几十条，会把这些笔触挤没。
 *
 * 记账点选在这一次 invoke 的落点，不在 document_updated 里认领：模型那边
 * 随时也在广播 document_updated，插在这一笔来回之间的那条会被旗子当成用户
 * 笔触吃掉——用户这一下笔白撤，模型的改动还占一格。快照在调用前取，
 * 谁先广播都不影响它记的是「动手之前」。
 */
function recordUndo(state: StoreState, before: PixelDocument): Partial<StoreState> {
  return {
    undoStack: pushDocStack(state.undoStack, before),
    redoStack: [],
  };
}

/**
 * 给当前会话记一笔落盘账。账本和「当前会话看到的那份」同步写：
 * 只写一份的话，切走再切回来就丢账——A 会话没存的东西，
 * 不该因为用户去 B 看了一眼就变成已保存。
 */
function bumpBill(patch: Partial<{ path: string | null; dirty: boolean }>): void {
  const state = useStore.getState();
  const id = state.activeId;
  if (!id) return;
  const current = state.projectLedger[id] ?? { path: null, dirty: false };
  const next = { ...current, ...patch };
  useStore.setState({
    projectLedger: { ...state.projectLedger, [id]: next },
    projectPath: next.path,
    projectDirty: next.dirty,
  });
}

/**
 * 落空时把上一笔账退回去：撤销栈顶如果还是这一笔就摘掉，重做栈还原成动手之前。
 * 不然一次点空的画笔也会白吃一步撤销，还把本来能重做的那步一起作废。
 */
function rollbackUndo(before: PixelDocument, redoBefore: PixelDocument[]): Partial<StoreState> {
  const stack = useStore.getState().undoStack;
  if (stack[stack.length - 1] !== before) return {};
  return { undoStack: stack.slice(0, -1), redoStack: redoBefore };
}

/**
 * 跑一次会改画布的编辑器动作：先把改前文档记进撤销栈，落空再把账退回去。
 * before 为空说明文档快照还没到手（刚开会话、正在切换），这账记不了，
 * 但动作照跑——后端手上的画布未必是空的。
 */
async function withCanvasUndo<T>(before: PixelDocument | null, run: () => Promise<T>): Promise<T> {
  if (!before) return run();
  const redoBefore = useStore.getState().redoStack;
  useStore.setState(recordUndo(useStore.getState(), before));
  try {
    return await run();
  } catch (error) {
    useStore.setState(rollbackUndo(before, redoBefore));
    throw error;
  }
}

/**
 * 多久收不到任何 agent 事件就当「卡住了」提醒用户。
 * 思考模型静默一分钟以上很常见（长推理、大图），所以门槛比那更宽：
 * 只在真的很久没动静时打扰，而且只是提醒，替用户做不了主。
 */
export const STALL_SECONDS = 1800;

const STALL_MS = STALL_SECONDS * 1000;

/**
 * 离开多久再回来才算「睡了一觉」：短于这个时长（切了个窗口、看了一眼浏览器）
 * 不碰静默表，只有真的离开过（休眠、长时间切走）才对齐各路计时。
 */
const WAKE_ABSENCE_MS = 60_000;

let stallTimer: ReturnType<typeof setTimeout> | null = null;
/**
 * 第几趟 loadDocument 了。同一时刻只认最后一次。
 *
 * 用户连点两个会话、或者在画布上连着改两下的时候，先发起的那趟往返可能
 * 后到。它一落地就把晚那一趟的结果盖掉：画布上出现的是一张不属于当前会话
 * 的「幽灵画布」，帧条、缩略图、授权栈也全都跟着错位。用一个自增序号把过期
 * 的结果挡在门外，比在每个调用点加旗子可靠——调用点多，旗子一定会漏。
 */
let loadSeq = 0;

/**
 * 会话簿通道的合并器。
 *
 * 外部 MCP 批量建画布、导入整包 .aip 时，session-event 一条接一条地来，每条
 * 都要把整份列表拍进 store 顺带渲染一次侧栏。收进同一个任务再统一结算，整批
 * 风暴只留一次渲染。语义不变：处理顺序还是到达顺序，最新一条说了算。
 */
let sessionCoalescer: Coalescer<SessionListChanged> | null = null;

/**
 * 预览图刷新合并：同一时刻只留一趟在飞的往返，途中再来的一律并成「补一发」，
 * 而且两趟之间至少隔 `PNG_MIN_INTERVAL_MS`。
 *
 * 光靠「在飞时不重发」还不够：后端一趟只要 20ms，模型一秒发六十条广播时，
 * 每一趟刚落地就又接上新的一趟，预览往返照样连成一条不断的队。加个最小间隔，
 * 让人眼来不及看的那几张（后端栅格化过、IPC 传过、webview 解码过，全白干）
 * 直接在合并里消失，只留首尾两张。
 */
const PNG_MIN_INTERVAL_MS = 96;

let pngBusy = false;
let pngAgain = false;
let pngTail: Promise<void> = Promise.resolve();
/** 下一趟最早能出发的时刻（墙上毫秒）。 */
let pngReadyAt = 0;
/** 等最小间隔时挂的那个闹钟，同一时刻只挂一个。 */
let pngTimer: ReturnType<typeof setTimeout> | null = null;

/** 唤醒对齐只挂一次：重复挂等于每醒一次多做一遍无用功。 */
let wakeSyncArmed = false;

/** 文档快照栈统一往栈尾压一份，超限就丢最老的那份。 */
function pushDocStack(stack: PixelDocument[], doc: PixelDocument): PixelDocument[] {
  const next = [...stack, doc];
  const byCount = next.length > UNDO_LIMIT ? next.slice(next.length - UNDO_LIMIT) : next;
  let drop = 0;
  let bytes = byCount.reduce((sum, entry) => sum + estimateDocBytes(entry), 0);
  while (byCount.length - drop > 1 && bytes > undoBudget.bytes) {
    bytes -= estimateDocBytes(byCount[drop]);
    drop += 1;
  }
  return drop > 0 ? byCount.slice(drop) : byCount;
}

export const useStore = create<StoreState & StoreActions>()((setState, getState) => {
  /**
   * 撤销/重做的串行闸门。
   *
   * 连点两下撤销时，第二次会在第一次的 sync_document 还没落地时就把「动手之前」
   * 的当前文档又压一遍重做栈，而那两份 IPC 并发，谁后到就把文档定成谁：两步撤销
   * 可能只退一步，重做栈里还多出一份重复的当前文档，重做一下就越过了中间态。
   * 排队之后每一档都等前一档真落地，栈里的账和画布才一一对得上。
   */
  const queueRestore = <T,>(task: () => Promise<T>): Promise<T> => {
    const settled = restoreChain.then(task, task);
    restoreChain = settled.then(
      () => undefined,
      () => undefined,
    );
    return settled;
  };

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

  /**
   * 预览图刷新：同一时刻只留一趟在飞的往返，途中又来的一律并成「回来补一发」。
   *
   * document_updated 一条笔一次广播，MCP 批量画图时一秒里就是几十条：每条都
   * 发起一趟 document_png_url，后端得把整张画布重新栅格化几十遍，队列一堵，
   * 预览反而迟迟不翻页。中间的截图根本来不及显示，把往返并成最新那一张，
   * 省下的往返正是界面不卡的来源。用户主动的动作（撤销、切帧）照旧走
   * refreshPng 原路，不在这个合并里。
   */
  function refreshPngSoon(): Promise<void> {
    // 离上一趟还不到最小间隔：先记一笔，等窗口到点再补发，不立刻扑上去。
    const wait = pngReadyAt - Date.now();
    if (wait > 0) {
      pngAgain = true;
      if (pngTimer === null) {
        pngTimer = setTimeout(() => {
          pngTimer = null;
          void refreshPngSoon();
        }, wait);
      }
      return pngTail;
    }
    if (pngBusy) {
      pngAgain = true;
      return pngTail;
    }
    pngBusy = true;
    // 这一趟把「途中攒的」一并算上：标记就此销账，等这一趟回来再看有没有新的。
    pngAgain = false;
    // 打的是出发时刻的记号，不是回来时刻：后端要是自己慢（大画布栅格化），
    // 别把它的耗时又叠进间隔里，那会让预览明显迟钝。
    pngReadyAt = Date.now() + PNG_MIN_INTERVAL_MS;
    // refreshPng 失败自己会招呼用户；这里接着链往下走，别让一次失败
    // 把后面排队的补发一起噎死。
    pngTail = getState().refreshPng().catch(() => {});
    return pngTail.then(() => {
      pngBusy = false;
      // 处理这一趟的功夫又来了一批：按最新的 revision 补一发，而不是逐条补。
      if (pngAgain) {
        pngAgain = false;
        return refreshPngSoon();
      }
    });
  }

  /**
   * 休眠唤醒 / 长时间离开再回来的对齐。
   *
   * 系统睡过去期间 JS 定时器全冻着，醒来那一瞬间：过期的静默表、攒了一路的
   * 会话簿事件、停在睡前读数的计时器一起扑上来，界面看到的就是「刚醒来卡
   * 半秒，然后什么都跳一遍」。焦点一回来就把各路的表对齐——攒着的事件立刻
   * 结算，静默表重新打表；睡过去的时长不算在「模型不回话」的账上。
   */
  function armWakeSync() {
    if (wakeSyncArmed || typeof window === "undefined") return;
    wakeSyncArmed = true;
    let awayAt: number | null = null;
    const thaw = () => {
      // 离开过再回来才算「醒」：一直在前台的回合不碰这些表，
      // 免得用户眨个眼回来看到自己没等过的红字。
      const absence = awayAt === null ? 0 : Date.now() - awayAt;
      awayAt = null;
      if (absence < WAKE_ABSENCE_MS) return;
      sessionCoalescer?.flush();
      if (!getState().running) return;
      clearStallWatch();
      if (getState().stalled) setState({ stalled: false });
      touchStallWatch();
    };
    window.addEventListener("blur", () => {
      awayAt = Date.now();
    });
    window.addEventListener("focus", thaw);
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) thaw();
    });
  }

  /** 等这一回合把占用交回来。
   *
   * 中断不是一句话就生效的：Rust 那边要走完手头那个检查点才收手（在途请求卡住时
   * 也在百来毫秒内，读流那个循环每 120ms 轮一次取消标志）。这期间 `running` 还是
   * 真，后端的回合占用也还压在上一轮手上——这时候紧跟着发新消息，只会得到一句
   * 「这个会话还在忙」，用户看到的就是「点了重试原地不动」。
   *
   * 订阅而不是轮询：收尾事件一到就放手，不用白等。等不到也不死等，超时返回假，
   * 由调用方决定是把丑话说清楚还是硬发。 */
  function waitForTurnRelease(timeoutMs = 8000): Promise<boolean> {
    if (!getState().running) return Promise.resolve(true);
    return new Promise<boolean>((resolve) => {
      let settled = false;
      let unsub = () => {};
      const finish = (released: boolean) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        unsub();
        resolve(released);
      };
      const timer = setTimeout(() => finish(false), timeoutMs);
      unsub = useStore.subscribe((state) => {
        if (!state.running) finish(true);
      });
    });
  }

  /** 影子代次计数器。只为「异步种子是否还有效」服务，别当序列号读。 */
  let shadowGen = 0;

  /**
   * 把台前这一回合的现场整份抄成后台影子。
   *
   * 没在跑也照抄：这一份就是用户眼前那串对话。缺了它，切回来只能读 Rust 历史，
   * 而历史里那些「发出去但没能落库」的轮次（发送失败、中断在半路）会整段消失。
   */
  function stashShadow(id: string) {
    const state = getState();
    const shadow: SessionShadow = {
      entries: state.entries,
      running: state.running,
      runStartedAt: state.runStartedAt,
      runElapsedMs: state.runElapsedMs,
      stalled: state.stalled,
      usage: state.usage,
      pendingApproval: state.pendingApproval,
      lastEventAt: state.running ? Date.now() : null,
      generation: 0,
      sealed: !state.running,
      seeded: true,
    };
    setState({ sessionShadows: { ...state.sessionShadows, [id]: shadow } });
  }

  /** 摘除某会话的影子并交出来；从来没有过就返回 null（这一条没在后台跑过）。 */
  function takeShadow(id: string): SessionShadow | null {
    const shadow = getState().sessionShadows[id] ?? null;
    if (!shadow) return null;
    const next = { ...getState().sessionShadows };
    delete next[id];
    setState({ sessionShadows: next });
    return shadow;
  }

  /** 丢掉某会话的影子（删会话、切回前台之后不再需要它）。 */
  function dropShadow(id: string) {
    if (!getState().sessionShadows[id]) return;
    const next = { ...getState().sessionShadows };
    delete next[id];
    setState({ sessionShadows: next });
  }

  /**
   * 把一条后台会话的事件折进它的影子。
   *
   * 与前台路径同构，但一个字节都不碰台前状态：document_updated 在这里直接扔掉
   * （它改的是后台那份画布，前台画的是另一幅），token / 工具块 / 收尾照单全收。
   */
  function foldBackgroundEvent(sessionId: string, raw: AgentEvent) {
    const state = getState();
    const shadow = state.sessionShadows[sessionId];
    if (!shadow) {
      // 头一回见到这条会话的事件：多半是 MCP 在外部起的一圈，台前从没握过它。
      // 先把历史读来打底，这一条交给种子接住——直接折进空列表会丢掉开头。
      void seedShadow(sessionId, raw);
      return;
    }
    const now = Date.now();
    const base: SessionShadow = { ...shadow, lastEventAt: now, stalled: false };
    let next = base;
    if (shadow.sealed && raw.kind !== "completed" && raw.kind !== "error" && raw.kind !== "interrupted") {
      // 收过尾的会话又来事件：只可能是新开了一圈。把启动态重新点亮，
      // 否则切回来会看见一个「明明还在画，却显示已停下」的界面。
      next = { ...base, running: true, runStartedAt: now, runElapsedMs: null, sealed: false };
    }
    if (raw.kind === "completed" || raw.kind === "error" || raw.kind === "interrupted") {
      const elapsed =
        next.runStartedAt === null ? next.runElapsedMs : now - next.runStartedAt;
      next = {
        ...next,
        entries: sealTranscript(reduceEvent(next.entries, raw, state.lang)),
        running: false,
        runStartedAt: null,
        runElapsedMs: elapsed,
        sealed: true,
        // 一圈收尾，挂着没批的调用跟着作废——别让切回去时看见一张废票。
        pendingApproval: null,
      };
    } else if (raw.kind === "usage") {
      next = { ...next, usage: { input: raw.input_tokens, output: raw.output_tokens } };
    } else if (raw.kind === "approval_request") {
      next = {
        ...next,
        pendingApproval: { callId: raw.call_id, name: raw.name, input: raw.input },
      };
    } else if (raw.kind !== "document_updated") {
      next = { ...next, entries: reduceEvent(next.entries, raw, state.lang) };
    }
    setState({ sessionShadows: { ...getState().sessionShadows, [sessionId]: next } });
    // 后台这一圈收尾了，侧栏的模型名、耗时都可能变：照前台的样子刷新一次。
    if (next.sealed && !shadow.sealed) void getState().refreshSessions();
  }

  /**
   * 给一条从没在前台亮过相的会话打底：读 Rust 历史当前几条，再接住头一条事件。
   *
   * 历史要到 async 边界之后才回来，这期间新事件会陆续折进影子——它们属于还没落库
   * 的这一圈，历史里没有，所以最后是「历史在前、折进来的在后」拼起来。
   * 两种情况下不拼：影子被切回前台或会话被删（代次变了），以及这一圈已经在路上
   * 跑完了（历史里已经有它，再拼一遍就是同一批消息显示两回）。
   */
  async function seedShadow(sessionId: string, firstEvent: AgentEvent) {
    const generation = (shadowGen += 1);
    const state = getState();
    if (state.sessionShadows[sessionId]) return;
    setState({
      sessionShadows: {
        ...getState().sessionShadows,
        [sessionId]: {
          entries: [],
          running: true,
          runStartedAt: null,
          runElapsedMs: null,
          stalled: false,
          usage: null,
          pendingApproval: null,
          lastEventAt: Date.now(),
          generation,
          sealed: false,
          seeded: false,
        },
      },
    });
    foldBackgroundEvent(sessionId, firstEvent);
    let messages: Message[];
    try {
      messages = await bridge.agentHistory(sessionId);
    } catch {
      // 读不到就从这一圈开始，不硬撑：后台会话看不到更早的几句，也比没有强。
      return;
    }
    // 形状不对（旧版本 Runtime、载荷被截断）就当没有历史：这一圈的对话还在，
    // 别为一个不相干的历史把整份现场掀掉。
    if (!Array.isArray(messages)) return;
    const current = getState().sessionShadows[sessionId];
    if (!current || current.generation !== generation) return;
    const history = historyToTranscript(messages, getState().lang);
    setState({
      sessionShadows: {
        ...getState().sessionShadows,
        [sessionId]: current.sealed
          ? { ...current, entries: history, seeded: true }
          : { ...current, entries: [...history, ...current.entries], seeded: true },
      },
    });
  }

  /**
   * session-event 路由：会话簿变了。补的是「外部改了，界面不动」这一整类问题。
   *
   * 在这之前，前端只在**自己**动作之后才 refreshSessions。外部进程通过 MCP
   * 建一个画布、删一个、改个名，Rust 那边账都记完了，可这边一条消息都没收到，
   * 侧栏和画面纹丝不动——用户看着自己刚点过「创建」而什么都没发生，
   * 只会原样再发一次，于是外部那边堆起一串重复画布。
   */
  async function ensureSessionListener() {
    if (sessionUnlisten) return;
    // 合并器只建一次：重复建会把先到的监听事件漏给上一个队列。
    sessionCoalescer ??= createCoalescer<SessionListChanged>((raw) => {
      // 形状不对就当没这条：旧版本 Runtime 没这个通道，或者载荷被截断了。
      if (!raw || !Array.isArray(raw.sessions)) return;
      const sessions = sortSessions(raw.sessions);
      const state = getState();
      const alive = new Set(sessions.map((session) => session.id));

      // 活动会话被外部删掉了：不能继续挂在一个不存在的 id 上，
      // 否则用户往画布上画的每一笔都落进空气，还以为是自己点错了地方。
      let activeId = state.activeId;
      let orphaned = false;
      if (activeId !== null && !alive.has(activeId)) {
        // 取剩下里 order 最大的那条（= 最新的），和开机时的取法一致。
        activeId = sessions.length ? sessions[sessions.length - 1].id : null;
        orphaned = true;
      }
      setState({ sessions, activeId });
      // 死掉的会话（外部删的）影子一并收走：它的事件不会再来了，
      // 留着就是一份永远停在「跑着」的假现场。
      for (const stale of Object.keys(state.sessionShadows)) {
        if (!alive.has(stale)) dropShadow(stale);
      }

      // Rust 点名要我们看这个：外部新建/导入的画布。必须真的切过去，
      // 只在侧栏加一条的话，用户根本不知道新画布在哪儿。
      const focus = typeof raw.focus === "string" ? raw.focus : null;
      if (focus && alive.has(focus) && focus !== state.activeId) {
        void getState().selectSession(focus);
        return;
      }
      // 没被点名但当前这条已经死了：得把它的画面撤掉。
      // 一条都不剩就是空状态，「当前没有会话」本身是个正常状态。
      if (orphaned) {
        if (activeId === null) {
          setState({ document: null, entries: emptyTranscript(), frameIndex: 0 });
        } else {
          void getState().selectSession(activeId);
        }
      }
    });
    sessionUnlisten = await bridge.listenSessionList((raw) => {
      // 进队列，不在回调里当场结算：一整批会话簿变化合成一次渲染。
      sessionCoalescer?.push(raw);
    });
  }

  /** 把增量合并进本地那份完整文档。没动过的 cel 保持引用，不深拷。
   *
   * 必须和 Rust 侧 `DocPatch::apply` 一步步对齐：漏掉 dropped 会让删掉的图层
   * 在界面上复活，漏掉 cels 会让改过的画面永远停在旧像素。第一次广播是全量，
   * `current` 是 null 也照合——cels 全由 patch 带上。
   *
   * 顺手按元数据把门管死：cel 只许长在 patch 带着的层与帧里。dropped 漏报一次
   * （事件桥还没挂上监听、窗口藏起来那一阵丢包都可能），光靠 dropped 就再也
   * 补不回来，而层号帧号是会复用的——下一次 create_layer 拿回 "L0" 时，那一层
   * 旧像素就跟着新层一起显形了。 */
  function mergePatch(current: PixelDocument | null, patch: DocPatch): PixelDocument {
    const liveLayers = new Set(patch.layers.map((layer) => layer.id));
    const liveFrames = new Set(patch.frames.map((frame) => frame.id));
    const cels: PixelDocument["cels"] = {};
    for (const [layerId, frames] of Object.entries(current?.cels ?? {})) {
      if (!liveLayers.has(layerId)) continue;
      const kept: PixelDocument["cels"][string] = {};
      for (const [frameId, cel] of Object.entries(frames)) {
        if (liveFrames.has(frameId)) kept[frameId] = cel;
      }
      cels[layerId] = kept;
    }
    for (const [layerId, frameId] of patch.dropped) {
      const frames = cels[layerId];
      if (frames) delete frames[frameId];
    }
    for (const [layerId, frameId, indices] of patch.cels) {
      (cels[layerId] ??= {})[frameId] = { indices };
    }
    return {
      name: patch.name,
      width: patch.width,
      height: patch.height,
      palette: patch.palette,
      layers: patch.layers,
      frames: patch.frames,
      cels,
      palettes: patch.palettes,
      revision: patch.revision,
    };
  }

  /** document_updated 的统一落点：文档、帧选择意图、选色归队一起结算。 */
  function applyDocument(
    patch: DocPatch,
    revision: number,
    frameHint: number | null,
  ) {
    const state = getState();
    const document = mergePatch(state.document, patch);
    const next: Partial<StoreState> = {
    document,
    revision,
    pendingFrameIndex: null,
    pngRevision: revision,
    };
    // revision 往回走是正常事：撤销把整份旧文档拍回后端，计数器跟着退回旧号，
    // 导入 .aip 更是直接换一份低号文档。而 patch 从来不是「第 N 版全量」，是
    // 「相对上一次广播」的增量，所以号再小也得照合——丢掉它，后端基线已经推进
    // 过去，这一笔就永远补不回来了。
    // 过期异步渲染的去重也不归这儿管：那由 refreshPng 自己按 revision 认领。
    // 原来那把「号小就不更新 pngRevision」的锁，只会让它一直停在比文档高的
    // 位置，之后每份增量都被误判成过期，纯属自己给自己埋雷。
    // 调色板就是配色范围：选中的色不在新调色板里，就近挪进去。
    // 不挪的话画笔会按字面量把色 intern 进调色板，用户挑的范围就悄悄失守了。
    // 归队的候选是「当前层的配色范围」，不是文档自带的那份基础调色板：
    // 各层的范围各自独立，拿基础调色板去就近，归回来的色很可能压根不在本层
    // 范围里，配色锁等于对模型这一回合的改动敞开着。
    let active = state.active;
    // color 是可选的：null 与「没选过」在这里都是一个意思，都就近归队。
    if (active.color != null) {
      const snapped = nearestHex(layerScopeColors(document, active.layer), active.color);
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
    // 选色被就近挪过、或者结构操作换了帧：都得让 Rust 侧的 active 跟上，
    // 不然模型的下一步编辑还落在旧的选中上。
    const activeChanged =
      active.color !== state.active.color ||
      active.frame !== state.active.frame ||
      active.layer !== state.active.layer;
    setState({ ...next, active });
    // 模型这一笔也落在画布上：盘上那份 .aip 同样旧了。用户视角里
    // 「AI 画完」和「自己画的」都是待存的改动，都得问过再走。
    bumpBill({ dirty: true });
    if (activeChanged) {
      const id = getState().activeId;
      if (id) syncActive(id, active);
    }
    // 一笔一次广播：把预览往返并成最新那一张，别让一次批量画布
    // 把后端渲染排成人龙——那时的卡顿全淤在这一趟趟 IPC 上。
    void refreshPngSoon();
  }

  /**
   * 把一份整文档拍回当前画布。
   * sync_document 只落在后端：它既不广播 document_updated，也不换预览地址，
   * 所以文档、revision、帧选择、预览图都得自己推——漏一个界面就和后端两张皮。
   *
   * 已知语义（有意不加锁）：这里的 revision 用的是快照里那个旧号，比后端
   * 计数器小。模型那一回合要是刚好在这之后广播 document_updated，它的 patch
   * 会叠在这份被拍回的文档上——撤销只撤掉"用户动手之前"的状态，
   * 撤不掉模型并行写进去的东西。想让它看得见，得上一个"这一笔没撤掉"的提示，
   * 而不是把撤销通道和模型通道用锁串起来：那会让模型一次普通的画完
   * 就顶掉用户连着点的好几步撤销。
   */
  async function restoreDocument(id: string, target: PixelDocument) {
    await bridge.syncDocument(id, target);
    const frameIndex = Math.max(0, Math.min(getState().frameIndex, target.frames.length - 1));
    const frame = target.frames[frameIndex];
    // 帧号可能是被夹过来的：撤销回到一份帧数更少的快照时，手里那个帧名
    // 已经不在新文档里。后端的 sync_document 只在选中「不存在」时才修，
    // 而这里换成的帧名在新文档里是存在的——两边就此指向不同的帧，
    // 模型下一步的工具调用就落到用户没在看的那个帧上。层同理。
    // 所以拍回之后要把选中原样回传，和 setActiveFrame / selectSession 一个规矩。
    const previous = getState().active;
    const layer = target.layers.some((one) => one.id === previous.layer)
      ? previous.layer
      : (target.layers[0]?.id ?? previous.layer);
    const active = frame ? { ...previous, layer, frame: frame.id } : previous;
    setState({
      document: target,
      revision: target.revision,
      pngRevision: target.revision,
      frameIndex,
      active,
    });
    // 画笔、填充、撤销全从这条漏斗过：画布变了，盘上那份 .aip 就旧了。
    bumpBill({ dirty: true });
    syncActive(id, active);
    await getState().refreshPng();
  }

  /**
   * 关窗值守：Rust 按住窗口时会广播一条问询，这里把弹窗亮起来。
   * 没脏就直接放行——问一句「你要不要存」而答案永远是「没什么可存」，
   * 纯属拿一个问题换一次多余的点击。
   */
  async function ensureCloseGuard() {
    if (closeUnlisten) return;
    closeUnlisten = await bridge.listenCloseRequest(() => {
      if (!useStore.getState().projectDirty) {
        // 没什么可丢的：答复就是放行，弹窗都不用亮。
        bridge.closeReply(true).catch(() => {
          // 答复递不回去（前端正被销毁）也别崩：让系统自己收尾。
        });
        return;
      }
      useStore.setState({ closeGuardOpen: true });
    });
    // 订阅上了才登记守门员：两条指令换序的话，登记之后、订阅之前那一下
    // 关窗会问出一个没人听的问。
    await bridge.closeGuardReady(true).catch(() => {
      // 浏览器预览里没有这条命令：值守照样挂着，只是没人真来问。
    });
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
    if (sessionId !== getState().activeId) {
      // 但绝不是扔掉。会话之间是并发的：切走只是不给它镜头，不是给它判死刑。
      // 折进影子，用户切回来时这一圈的 token、工具块、收尾一样都不少。
      foldBackgroundEvent(sessionId, raw);
        return;
      }
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
        applyDocument(raw.patch, raw.revision, state.pendingFrameIndex);
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

  /**
   * 把某条会话的文档拉进界面。switching 表示「换会话」，不只是刷新当前这条。
   *
   * 换会话和不换会话的区别只有一个，但正是幽灵画布的来处：换会话时旧画面
   * 必须在等新文档之前就撤掉。不撤的话，新文档还在半路上，画布里挂的仍是
   * 上一条会话的像素——用户点进来先看见一幅画，再看着它变成另一幅。
   */
  /**
   * @param authoritative 快照是否无条件盖掉本地文档。
   * 只有「刚往后端灌过一份外来文档」时才为真（导入 .aip）：那份文档的 revision
   * 比本地旧，但它就是要取而代之。
   */
  async function loadDocument(
    id: string,
    switching = false,
    authoritative = false,
    keepEntries = false,
  ) {
    const seq = (loadSeq += 1);
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
      // 撤销栈是当前会话的笔迹，换会话不跟着走。
      undoStack: [],
      redoStack: [],
      // 有影子的这一条另说：后台会话也会举手要审批（foldBackgroundEvent 里
      // 接），selectSession 刚把它恢复出来，这里再抹一次，用户就永远看不到
      // 那张审批票，回合挂在那儿谁也不动。和 entries 一样，影子说了算。
      pendingApproval: keepEntries ? getState().pendingApproval : null,
      pendingFrameIndex: null,
      closeGuardOpen: false,
    });
    // 落盘账跟着会话走：账本里有就请回来，没有的就是一份新账（还没存过）。
    // 放在默认清零之后、读文档之前，好让「打开 .aip → 记账为已存」覆盖它。
    const bill = getState().projectLedger[id] ?? { path: null, dirty: false };
    setState({ projectPath: bill.path, projectDirty: bill.dirty });
    if (switching) {
      // revision 一起归零：会话之间的 revision 没有可比性，带着旧值会让
      // 后续 document_updated 被 `revision < pngRevision` 误判成过期事件。
      setState({ document: null, revision: 0, pngRevision: -1, pngUrl: null });
      // 会话一换，旧图连「属于哪一帧」都不算数了：留着会让新会话第一眼
      // 拿上一幅画顶着，直到 refreshPng 那趟往返回来。
      setState({ pngFrame: -1 });
    }
    try {
      const messages = await bridge.agentHistory(id);
      if (seq !== loadSeq) return;
      // 影子里已经握着这一圈的现场就别盖：Rust 的历史只到上一轮收尾，
      // 拿它盖上去，正在跑的那半段就整段消失了——用户看到的就是「说着话人呢」。
      if (!keepEntries) setState({ entries: historyToTranscript(messages, getState().lang) });
    } catch {
      // 历史读不到就从空对话开始，不阻塞文档加载
    }
    const snapshot = await bridge.documentSnapshot(id);
    if (seq !== loadSeq) return;
    // 快照还在半路上时模型可能又画了一笔：那条 document_updated 早已并进本地文档，
    // 而快照是它之前的状态。照单全收就把刚画上去的东西从本地抹掉，而后端广播基线
    // 已经推进过去，那一笔再也不会补发——用户看到的就是「模型说画完了，画布没动」。
    // 按 revision 认领，谁新留谁；导入 .aip 那一路是主动换文档，不受这条约束。
    if (!authoritative && snapshot.revision < getState().revision) return;
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
    pngFrame: -1,
    frameIndex: 0,
    entries: emptyTranscript(),
    sessionShadows: {},
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
    redoStack: [],
    notice: null,
    settingsOpen: false,
    mcpServers: EMPTY_MCP,
    mcpBusy: false,
    loopLimits: null,
    mcpServerStatus: null,
    mcpServerBusy: false,
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
    styleOverride: null,
    presetOverrides: [],
    createPromptOpen: false,
    toolOpen: {},
    recipe: DEFAULT_BATCH_RECIPE,
    scan: null,
    scanBusy: false,
    run: EMPTY_BATCH_RUN,
    recipeBook: [],
    recipeName: "",
    recipeBusy: false,
    recipeImport: null,
    projectPath: null,
    projectDirty: false,
    projectLedger: {},
    closeGuardOpen: false,

    toggleToolOpen: (id, autoOpen) => {
      // 没记过就照默认姿态取反：分流节点默认摊开，第一下按是收起。
      const now = getState().toolOpen[id] ?? autoOpen;
      setState({ toolOpen: { ...getState().toolOpen, [id]: !now } });
    },

    boot: async () => {
      if (booting) return booting;
      const attempt = (async () => {
        // 配方簿和批量通道一样与会话无关：开机读回来，用户随时能从簿子里挑一条。
        await getState().loadRecipes();
        // 唤醒对齐挂在开机这一次上：之后休眠唤醒、切窗口回来都靠它。
        armWakeSync();
        await ensureListener();
        // 批量通道与会话无关，开机听上就行：用户随时可能从工作台起一趟。
        await ensureBatchListener();
        // 会话簿通道也在读簿子之前挂：早一步挂上，外部在开机这半秒里建的
        // 会话才不会掉进「读完了但还没人听」的那道缝里。
        await ensureSessionListener();
        // 关窗值守：开机就登记，之后每一下关闭都先问前端。
        await ensureCloseGuard();
        let models: ModelsView;
        try {
          models = await bridge.listModels();
        } catch (error) {
          flagKey("store.read_models_failed", { error: String(error) });
          models = EMPTY_MODELS;
        }
        setState({ models });
        await getState().refreshMcpEnabled();
        await getState().refreshMcpServer();
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
        sessions = sortSessions(sessions);
        const first = sessions[sessions.length - 1];
        if (first) {
          // 可能已经有人把活动会话定下来了：外部 MCP 在这半秒里创建画布时
          // 会带着 focus 广播过来，那一下正是最该跟着变的时候。此处再拿
          // 开机默认值去盖，用户就又回到「界面没反应」的老问题上了。
          const activeId = getState().activeId ?? first.id;
          setState({ sessions, activeId });
          await loadDocument(activeId);
        } else {
          // 一条都没有就空着。「当前没有会话，您可以创建」是个正常状态，
          // 开机悄悄补一个的话，侧栏里永远躺着个没打开过的 s1。
          setState({ sessions: [], activeId: null, document: null });
          // 但空状态下也别让用户自己发现「原来得点那个 +」：开机头一回
          // 主动把新建弹窗递上去。关过一次就把这个标记关掉，之后再不打扰。
          if (!readCreatePromptDismissed()) {
            setState({ createPromptOpen: true });
          }
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
      // 切会话不等于叫停。会话之间在 Rust 是并发的，把镜头让出去就行：上一回合
      // 接着画，它的 token 折进影子，切回来时原样接上。以前这里是先 interrupt，
      // 于是「切出去看一眼别的会话」就等于把这一笔作废——用户看到的正是
      // 「消息记录缺了一半，而且再也没有然后了」。
      const live = getState();
      if (live.activeId && live.activeId !== id) stashShadow(live.activeId);
      const shadow = takeShadow(id);
      setState({
        activeId: id,
        // 有影子就用影子的现场：对话、跑没跑、计时器，一样都不许被 loadDocument
        // 那份历史重放盖掉。没影子才是这一条真的从没跑过，照旧从空对话开始。
        entries: shadow?.entries ?? emptyTranscript(),
        running: shadow?.running ?? false,
        runStartedAt: shadow?.runStartedAt ?? null,
        runElapsedMs: shadow?.runElapsedMs ?? null,
        stalled:
          shadow?.running === true &&
          shadow.lastEventAt !== null &&
          Date.now() - shadow.lastEventAt > STALL_MS,
        usage: shadow?.usage ?? null,
        pendingApproval: shadow?.pendingApproval ?? null,
        lastQuery: null,
        attachments: [],
        frameIndex: 0,
      });
      // 后台这一圈还在跑：静默计时接着打表，别让切回来的人看到「已经停了」。
      if (shadow?.running) touchStallWatch();
      else clearStallWatch();
      await loadDocument(id, true, false, Boolean(shadow));
      await getState().refreshSessions();
    },

    createSession: async (width, height, title) => {
      const document = width && height ? blankDocument(width, height) : undefined;
      // 空字符串当没填：用户清空输入框不该得到一个空名会话。
      const info = await bridge.createSession(document, (title ?? "").trim() || null);
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
        // 不管弹窗是开机自动递上来的还是用户自己点 + 开的，建成了都该收掉：
        // 留着窗子挡着新画布，跟用户对着干。
        createPromptOpen: false,
    });
      await loadDocument(info.id, true);
      // 新工程从没存过：账本里划一笔「待存」，关窗时才问得到用户。
      bumpBill({ path: null, dirty: true });
      // 只有从尺寸弹窗进来的才告知：别的方式来这儿就安静建，不刷通知。
      if (width && height) {
        noteKey("sidebar.created_hint", { width: info.width, height: info.height });
      }
    },

    removeSession: async (id) => {
      try {
        await bridge.dropSession(id);
      } catch (error) {
        // Rust 那头没删成：会话还在盘上。消息必须给用户看见，不然点了「删除」
        // 什么也没发生，只会以为界面卡了。往上抛，好让确认弹窗停在原处等下一回。
        flagKey("store.drop_session_failed", { error: String(error) });
        throw error;
      }
      const rest = getState().sessions.filter((s) => s.id !== id);
      setState({ sessions: rest });
      if (getState().activeId === id) {
        if (rest.length > 0) {
          await getState().selectSession(rest[rest.length - 1].id);
        } else {
          // 最后一个也删了：停在「当前没有会话」，不悄悄补一个顶数。
          setState({
            activeId: null,
            document: null,
            entries: emptyTranscript(),
            running: false,
            runStartedAt: null,
            runElapsedMs: null,
            stalled: false,
            usage: null,
            lastQuery: null,
            attachments: [],
            pngUrl: null,
            revision: 0,
            pngRevision: -1,
            pngFrame: -1,
            frameIndex: 0,
            pendingFrameIndex: null,
            busy: false,
            undoStack: [],
            redoStack: [],
            pendingApproval: null,
          });
          // running 一收，静默计时也得当场撤。留着的话它过半小时自己响，
          // 在没有会话的空界面上弹一条「模型好像卡住了」。
          clearStallWatch();
        }
      }
      // 影子跟着会话一起走。放在最后：上面切走的那一步会把台前现场
      // 暂存进影子表，早一步丢就会被它又写回来一份死会话的假现场。
      dropShadow(id);
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

    openCreatePrompt: () => {
      // 空会话状态下这个弹窗就是新建的唯一入口，所以它随时可能被 + 唤起来；
      // 主动打开不算「被骚扰」，不清免打扰标记。
      setState({ createPromptOpen: true });
    },

    closeCreatePrompt: () => {
      // 关窗 = 免打扰。不是「这回想再缓缓」，而是「以后别再问」。
      // 空会话下侧栏那个 + 一直都在，想建随时建得到。
      writeCreatePromptDismissed();
      setState({ createPromptOpen: false });
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
      // 上一轮还赖着：先收掉再发。用户在停顿横幅上点重试、或者直接回车再发
      // 一句，走的是同一条路。不收的话 Rust 的回合占用压在上一轮手上，这句话
      // 过去只会撞「这个会话还在忙」，界面看着就是按了没反应。
      if (getState().running) {
        await getState().interrupt();
        if (!(await waitForTurnRelease())) {
          // 等不到也把话说清楚：占位气泡一个都不画，别在对话里留一堆重复消息。
          failKey("agent.busy");
          return;
        }
      }
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
      // 说出去的话也是工程的一部分：.aip 只有文档，但用户嘴里「这个会话」
      // 是连聊天记录一起算的。发过一句就记上待存，关窗时才问得到人。
      getState().markProjectDirty();
      try {
        // 风格锁定跟着这一句走：用户在输入区钉了画风，模型这一回合就得按它画，
        // 话里没提也不能改主意。
        // 预设同理，且是几条一起走：点了「写实渲染 + 微细结构 + 闭塞接触」，
        // 这一句就照这三条规矩收尾。两者都是「这一句的偏好」，一起送过去，
        // Rust 那侧按先让位、再顶替的顺序拼进提示词——画风在时色数听画风的，
        // 其余收尾规矩照常上路；与画风 id 重合的那条由画风段顶替，不发两遍。
        await bridge.sendMessage(
          id,
          trimmed,
          payload,
          undefined,
          getState().styleOverride,
          getState().presetOverrides,
        );
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

    refreshMcpServer: async () => {
      try {
        setState({ mcpServerStatus: await bridge.mcpServerStatus() });
      } catch (error) {
        // 读不回来不嚷嚷：这一块只影响设置页，主界面照常跑。
        flagKey("store.read_mcp_server_failed", { error: String(error) });
      }
    },

    setMcpServerEnabled: async (enabled) => {
      setState({ mcpServerBusy: true });
      try {
        setState({ mcpServerStatus: await bridge.mcpServerSetEnabled(enabled) });
      } catch (error) {
        flagKey("store.set_mcp_server_enabled_failed", { error: String(error) });
        await getState().refreshMcpServer();
      } finally {
        setState({ mcpServerBusy: false });
      }
    },

    setMcpServerPort: async (port) => {
      setState({ mcpServerBusy: true });
      try {
        setState({ mcpServerStatus: await bridge.mcpServerSetPort(port) });
      } catch (error) {
        flagKey("store.set_mcp_server_port_failed", { error: String(error) });
        await getState().refreshMcpServer();
      } finally {
        setState({ mcpServerBusy: false });
      }
    },

    restartMcpServer: async () => {
      setState({ mcpServerBusy: true });
      try {
        setState({ mcpServerStatus: await bridge.mcpServerRestart() });
      } catch (error) {
        flagKey("store.restart_mcp_server_failed", { error: String(error) });
        await getState().refreshMcpServer();
      } finally {
        setState({ mcpServerBusy: false });
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
      // ids 只该来自侧栏那一次拖拽。万一调用方给了残缺列表（别的流程、
      // 或者两份列表错开了），没被点名的会话不能凭空从界面上消失，按原序附在尾部，
      // 等 refreshSessions 拿回后端次序再收敛。
      const kept = new Set(ordered.map((session) => session.id));
      const stragglers = getState().sessions.filter((session) => !kept.has(session.id));
      setState({ sessions: [...ordered, ...stragglers] });
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
        // 必须往外抛：表单要靠这个失败把编辑态留住，不然关了窗用户以为存上了。
        throw error;
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

    refreshDocument: async (authoritative = false) => {
      const id = getState().activeId;
      if (!id) return;
      await loadDocument(id, false, authoritative);
      await getState().refreshSessions();
    },

    /**
     * 落图之后把选中对齐到后端。
     *
     * 生图、抽帧、量化都可能在文档里添一帧，Rust 顺手把 active.frame 挪到那一帧
     * ——不挪的话它自己下一步的编辑还留在旧帧。可这条消息只顺着 document_updated
     * 过来，事件里没有帧号，前端那份选中就还指着旧帧：新帧明明画好了，帧条高亮、
     * 主画布和「接下来改哪儿」三样全对不上，用户以为没生效。
     * 拉一趟快照按 active 里的帧 id 反查下标，权和 Rust 只留一份。
     */
    syncSelection: async () => {
      const id = getState().activeId;
      if (!id) return;
      try {
        const snapshot = await bridge.documentSnapshot(id);
        const active = snapshot.active;
        if (!active) return;
        const index = snapshot.document?.frames.findIndex((f) => f.id === active.frame) ?? -1;
        if (index < 0) return;
        const state = getState();
        if (
          state.frameIndex === index &&
          state.active.layer === active.layer &&
          state.active.frame === active.frame
        ) {
          return;
        }
        setState({
          frameIndex: index,
          active: { ...state.active, layer: active.layer, frame: active.frame },
        });
        await getState().refreshPng();
      } catch {
        // 拉不到就维持原样：画布已经由 document_updated 刷新过了，这一步只是让
        // 选中跟得上，失败不该把一次成功的落图说成失败。
      }
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
        // 帧号跟着图一起落库：切帧的瞬间帧层已经画上新的一帧，而这张图还是
        // 旧帧的，两层叠着就是花脸。记下来，界面照着把旧图请下去。
        if (getState().revision === requested) setState({ pngUrl: url, pngFrame: frameIndex });
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
        // 文件解析还在半路上用户切了会话：这份文档是照着打开时那条会话
        // 弄来的，这时候灌进去等于拿它覆盖现在这条画布。原样撤掉，招呼一声。
        if (getState().activeId !== id) {
          flagKey("store.open_aip_switched");
          return;
        }
        await bridge.syncDocument(id, document);
        // 新文档和旧笔迹无关，撤销栈清空；否则一撤销就退回上一个文件。
        setState({ undoStack: [], redoStack: [], pendingFrameIndex: null });
        // 这份文档正是从 path 读来的：它就是当前会话的落盘点，账上记平。
        bumpBill({ path, dirty: false });
        noteKey("store.loaded", { name: baseName(path) });
        // 刚把外来文件灌进后端：这份文档的号比本地旧，但就是要它盖掉本地，
        // 所以这一趟得无条件认领。
        await getState().refreshDocument(true);
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
        bumpBill({ path, dirty: false });
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

    markProjectDirty: () => bumpBill({ dirty: true }),

    answerClose: (quit) => {
      // 答复只递一句话：Rust 那头已经按住窗口，它要的只是「放不放」。
      // 取消只是收起弹窗接着用，别顺手把账改平了。
      setState({ closeGuardOpen: false });
      bridge.closeReply(quit).catch((error) => {
        flagKey("store.close_reply_failed", { error: String(error) });
      });
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

    setStyleOverride: (style) => setState({ styleOverride: style }),

    setPresetOverrides: (presets) => {
      // 上限之外的一律不收，但绝不静默收：用户点了第四条又没反应，他会以为
      // 这条也上了路，然后盯着没变化的图猜原因。antd 那头先把超额项置灰，
      // 这里是第二道闸，顺手兜住「从别处塞进来的超额清单」。
      const wanted = uniquePresetIds(presets);
      setState({ presetOverrides: normalizePresetStack(wanted) });
      if (wanted.length > MAX_STACKED_PRESETS) {
        flagKey("store.preset_stack_full", { max: MAX_STACKED_PRESETS });
      }
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
          entries: pushSideNotice(getState().entries, renderUiText(getState().lang, outcome.summary), false),
        });
        // 三条会改画布的链路（生图、插帧、抽帧）跑完对齐一次选中：
        // 添了新帧的话帧条高亮和下一次落点都得跟上，不然用户以为没生效。
        if (kind === "image_gen" || kind === "frame_tween" || kind === "video_frames") {
          await getState().syncSelection();
        }
        return outcome;
      } catch (error) {
        const message = workflowError(error);
        setState({
          outcome: null,
          outcomeError: message,
          entries: pushSideNotice(getState().entries, message, true),
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
          entries: pushSideNotice(getState().entries, renderUiText(getState().lang, outcome.summary), false),
        });
        await getState().syncSelection();
        return outcome;
      } catch (error) {
        const message = workflowError(error);
        setState({
          outcome: null,
          outcomeError: message,
          entries: pushSideNotice(getState().entries, message, true),
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
        // 画风与预设跟着这一句走，和主循环送的是同一份：微调这条链过去完全不
        // 带它们，用户在输入区选了「写实渲染」，点一下微调，出来的九行提示词里
        // 一条渲染规矩都没有。
        const refined = await bridge.promptRefine(
          id,
          idea,
          0,
          0,
          getState().refineTarget,
          getState().styleOverride,
          getState().presetOverrides,
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
        entries: pushSideNotice(getState().entries, message, true),
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
      try {
        await withCanvasUndo(document, () =>
          bridge.paintStroke(id, {
            layer: active.layer,
            frame: active.frame,
            cells,
            // 传进来的 override 是给橡皮的：null 明说「这一笔就是擦」。
            color: inkOverride !== undefined ? inkOverride : (active.color ?? null),
          }),
        );
      } catch (error) {
        flagKey("store.paint_failed", { error: String(error) });
      }
    },

    fillCell: async (x, y, inkOverride) => {
      const id = getState().activeId;
      const active = getState().active;
      const document = getState().document;
      if (!id || !document) return;
      try {
        await withCanvasUndo(document, () =>
          bridge.fillCells(
            id,
            active.layer,
            active.frame,
            x,
            y,
            inkOverride !== undefined ? inkOverride : (active.color ?? null),
          ),
        );
      } catch (error) {
        flagKey("store.fill_failed", { error: String(error) });
      }
    },

    runEditorOps: async (ops, frameHint) => {
      const id = getState().activeId;
      const document = getState().document;
      if (!id) return null;
      // 先把帧选择意图挂上：新文档还在路上，到了就按这个落点选帧。
      if (frameHint !== undefined) setState({ pendingFrameIndex: frameHint });
      try {
        return await withCanvasUndo(document, () => bridge.applyEditorOps(id, ops));
      } catch (error) {
        setState({ pendingFrameIndex: null });
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

    createPalette: async (name, colors, layerId) => {
      const target = layerId ?? getState().active.layer;
      if (!target) return;
      await getState().runEditorOps([
        { op: "create_palette", name, colors, layer: target },
      ]);
    },

    deletePalette: async (id, fallback) => {
      // 还有层在引用时带 fallback：Rust 会把那些层改指过去再删，不再死循环。
      await getState().runEditorOps([{ op: "delete_palette", id, fallback: fallback ?? null }]);
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

    removePaletteColor: async (id, index, replacement) => {
      // 带 replacement：画面上用了被删色的像素跟着改写，不靠索引前移碰运气。
      await getState().runEditorOps([
        { op: "remove_palette_color", id, index, replacement: replacement ?? null },
      ]);
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

    /**
    * 撤销一步：把「动手之前」的文档拍回画布，当前文档挪进重做栈。
    * 账先结再拍回——连点撤销时每一步都得当场落下，不能等后端回话；
    * 拍失败了原样还回去，不然栈会凭空少一步，用户点着点着就没了。
    * 文档也得当场换掉：不等后端回话的话，连点第二下时 document 还是改前那份，
    * 它会被当成「当前画面」再压一遍重做栈——重做一下就越过了中间态。
    */
    undoEdit: async () => {
      const id = getState().activeId;
      const stack = getState().undoStack;
      const previous = stack[stack.length - 1];
      const current = getState().document;
      if (!id || !previous || !current) return;
      const undoBack = stack;
      const redoBack = getState().redoStack;
      setState({
        document: previous,
        undoStack: stack.slice(0, -1),
        redoStack: pushDocStack(getState().redoStack, current),
        pendingFrameIndex: null,
      });
      try {
        await queueRestore(() => restoreDocument(id, previous));
      } catch (error) {
        setState({ document: current, undoStack: undoBack, redoStack: redoBack });
        flagKey("store.undo_failed", { error: String(error) });
      }
    },

    /** 重做一步：把刚才撤销掉的画面拍回来，当前文档退回撤销栈。 */
    redoEdit: async () => {
      const id = getState().activeId;
      const stack = getState().redoStack;
      const next = stack[stack.length - 1];
      const current = getState().document;
      if (!id || !next || !current) return;
      const undoBack = getState().undoStack;
      const redoBack = stack;
      setState({
        document: next,
        redoStack: stack.slice(0, -1),
        undoStack: pushDocStack(getState().undoStack, current),
        pendingFrameIndex: null,
      });
      try {
        await queueRestore(() => restoreDocument(id, next));
      } catch (error) {
        setState({ document: current, undoStack: undoBack, redoStack: redoBack });
        flagKey("store.redo_failed", { error: String(error) });
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

    layPaperdollBase: async (layer) => {
      const id = getState().activeId;
      const document = getState().document;
      if (!id || !document) return;
      // 没点名就铺当前层：白膜是底稿，落在用户此刻盯着的那层上最顺。
      const target = layer ?? getState().active.layer;
      if (!target) return;
      try {
        // 和编辑器 op 走同一条账：改前文档进撤销栈。白膜整层覆盖，
        // 是可撤销的编辑动作，所以不用 confirm-guard 里那种确认弹窗。
        await withCanvasUndo(document, () => bridge.layPaperdollBase(id, target));
        await loadDocument(id);
      } catch (error) {
        // 尺寸不是角色行走图网格时 Rust 会带原因拒掉：原样转述给用户，
        // 「画布必须是 144x192 这类 4 行网格」这句话只有后端说得准。
        flagKey("store.paperdoll_failed", { error: String(error) });
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
