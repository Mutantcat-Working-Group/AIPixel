// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import {
  useEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import {
  Button,
  ColorPicker,
  Input,
  InputNumber,
  Segmented,
  Select,
  Slider,
  Switch,
  Tooltip,
} from "antd";
import {
  ArrowLeft,
  ArrowDown,
  ArrowUp,
  ArrowRight,
  Brush,
  Copy,
  Eraser,
  Eye,
  EyeOff,
  FileCode2,
  Ghost,
  Layers,
  PaintBucket,
  Palette,
  Pause,
  Play,
  Plus,
  Pencil,
  Trash2,
  Undo2,
  X,
} from "lucide-react";

import { useStore } from "../lib/store";
import { useT } from "../lib/t";
import { appendStroke, lineCells } from "../lib/stroke";
import { compositeFrame } from "../lib/render";
import { parseHex, rgbaToHex } from "../lib/palette";
import { NAMED_COLORS, colorName } from "../lib/colornames";
import FrameThumb from "./FrameThumb";
import CanvasSizeModal from "./CanvasSizeModal";
import { openContextMenu, type ContextMenuItem } from "./ContextMenu";
import type {
  EditorTool,
  InkColor,
  Layer,
  PixelDocument,
  StrokeCell,
} from "../lib/types";

const MAX_PREVIEW_HEIGHT = 240;
/** 洋葱皮的浓度：看得见上一帧的轮廓，但抢不走当前帧的注意力。 */
const ONION_ALPHA = 0.24;

/** 整数倍放大：像素画不能被插值糊掉，倍率向下取整，格子才始终是方的。 */
function integerScale(
  width: number,
  height: number,
  budget: number,
  maxHeight: number,
): number {
  return Math.max(1, Math.floor(Math.min(budget / width, maxHeight / height)));
}

/** 指针位置 -> 格子坐标。用实测矩形换算，CSS 缩放图片后格子也不会对不齐。 */
function cellFromEvent(
  event: ReactMouseEvent<HTMLDivElement>,
  document: PixelDocument,
): StrokeCell | null {
  const rect = event.currentTarget.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0) return null;
  const x = Math.floor(((event.clientX - rect.left) / rect.width) * document.width);
  const y = Math.floor(((event.clientY - rect.top) / rect.height) * document.height);
  if (x < 0 || y < 0 || x >= document.width || y >= document.height) return null;
  return { x, y };
}

/** 离屏烘焙位：把「要贴进帧层的那一帧」先落在这儿。窗口里可复用。 */
function stageBuffer(
  ref: { current: HTMLCanvasElement | null },
  width: number,
  height: number,
): HTMLCanvasElement {
  // window.document：组件里的 document 是像素文档，把 DOM 的那个遮蔽掉了。
  const canvas = ref.current ?? window.document.createElement("canvas");
  ref.current = canvas;
  // 只在尺寸变化时改宽高：改宽高会清空画布，每帧都写等于白擦一遍。
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }
  return canvas;
}

function stagePixels(
  canvas: HTMLCanvasElement,
  pixels: Uint8ClampedArray<ArrayBuffer>,
  width: number,
  height: number,
): void {
  const ctx = canvas.getContext("2d");
  if (ctx) ctx.putImageData(new ImageData(pixels, width, height), 0, 0);
}

export default function DocumentPanel() {
  const t = useT();
  const document = useStore((s) => s.document);
  const pngUrl = useStore((s) => s.pngUrl);
  const active = useStore((s) => s.active);
  const frameIndex = useStore((s) => s.frameIndex);
  const revision = useStore((s) => s.revision);
  const lang = useStore((s) => s.lang);
  const undoDepth = useStore((s) => s.undoStack.length);
  const [tool, setTool] = useState<EditorTool>("brush");
  const [aipText, setAipText] = useState<string | null>(null);
  const [aipOpen, setAipOpen] = useState(false);
  const [budget, setBudget] = useState(300);
  const [playing, setPlaying] = useState(false);
  const [playFrame, setPlayFrame] = useState(0);
  const [onion, setOnion] = useState(false);
 // 瓦片底图：1 = 单幅，2 / 3 = 把画面平铺开，看瓦片接缝。
  const [tileMode, setTileMode] = useState<1 | 2 | 3>(1);
  // 配色区正在伺候哪一层。null = 跟着激活层走；用户在色板区分区里另挑过
  // 一层时钉住，方便不切激活层也能给底层换范围。
  const [scopeLayerId, setScopeLayerId] = useState<string | null>(null);
// 取色盘正在挑的草稿值：拖动过程中只预览，落文档等松手（onChangeComplete）。
const [swatchDraft, setSwatchDraft] = useState<string | null>(null);
// 手输十六进制的草稿。取色盘拖不出「我就要这个 #hex」，而像素行当里 hex 是通行证。
const [hexDraft, setHexDraft] = useState("");
  // 配色区的行内输入：null = 收起，"new" = 新建一套，"rename" = 给当前套改名。
  // 和图层/会话改名同一套：ref 是权威，失焦提交时 state 已经清了。
  const [scopeEditing, setScopeEditing] = useState<"new" | "rename" | null>(null);
  const [scopeDraftName, setScopeDraftName] = useState("");
  const scopeEditingRef = useRef<"new" | "rename" | null>(null);
  const checkerRef = useRef<HTMLDivElement>(null);
  // 一笔笔画的临时状态全在 ref 里：pointermove 不该触发 React 渲染。
  const strokeRef = useRef<StrokeCell[]>([]);
  const lastCellRef = useRef<StrokeCell | null>(null);
  const paintingRef = useRef(false);
  // 瓦片容器。帧层和笔迹层每个瓦片一份，按容器现查：
  // 瓦片数一变 DOM 就重建，缓存句柄会指向已经摘掉的画布。
  const tilesRef = useRef<HTMLDivElement>(null);
  // 洋葱皮的上一帧先落在离屏画布上，再压低透明度贴到底层，
  // 因为 putImageData 不吃 globalAlpha。
  const ghostRef = useRef<HTMLCanvasElement | null>(null);
  // 当前帧也占一个离屏位：合成要的是 drawImage 的 source-over，
  // putImageData 会把刚铺好的幽灵整层覆盖掉。
  const frameBufRef = useRef<HTMLCanvasElement | null>(null);
  // 左上角 WxH 点开的改尺寸弹窗。新建会话那扇窗是「起步」，这扇窗是「改」，
  // 说的话不一样，其余控件共用 CanvasSizeModal。
  const [sizeOpen, setSizeOpen] = useState(false);
  const [sizeDraft, setSizeDraft] = useState<{ width: number; height: number }>({
    width: 64,
    height: 64,
  });

  // 开窗那一刻才取现文档宽高：这层组件不会随换会话重建，
  // 停在旧会话的草稿值会被带到新会话里。
  const openSizeModal = () => {
    setSizeDraft({
      width: document?.width ?? 64,
      height: document?.height ?? 64,
    });
    setSizeOpen(true);
  };

  // 瓦片平铺按份数等比缩：预览区不用滚动也能看全接缝，格子仍旧是整数倍。
  const scale = document
    ? integerScale(
        document.width,
        document.height,
        budget / tileMode,
        MAX_PREVIEW_HEIGHT / tileMode,
      )
    : 1;
  // 只有中间那一格接收指针：其余瓦片是同一幅画的回声，点它们等于点中间。
  const centerIndex = Math.floor((tileMode - 1) / 2);

  /** 容器里某一类画布的全部句柄：帧层每个瓦片一份，播放与洋葱皮要一起动。 */
  function tileCanvases(selector: string): HTMLCanvasElement[] {
    const root = tilesRef.current;
    return root ? Array.from(root.querySelectorAll<HTMLCanvasElement>(selector)) : [];
  }

  /**
   * 把某一帧（可选带上一帧的幽灵）画到帧画布上。画布尺寸就是文档尺寸，
   * 放大交给 CSS 和 image-rendering: pixelated，和 <img> 走同一条放大路径。
   */
  function paintFrame(index: number, ghostOf: number | null) {
    if (!document) return;
    const { width, height } = document;
    const wantGhost = ghostOf !== null && ghostOf >= 0 && ghostOf !== index;
    // 幽灵和当前帧都得各自落在离屏画布上，再交给 drawImage 合成。putImageData
    // 是不合成、直接覆盖的：先在帧画布上铺好幽灵、再用 putImageData 贴当前帧，
    // 幽灵会被整层擦掉（只剩当前帧透空的地方漏出一点），洋葱皮看着就像坏了。
    // 改走 source-over：先铺 24% 的幽灵，再把当前帧整个压上去，才是
    // 「上一帧淡淡地垫在下面」。九宫格也只烘焙两帧，回声格照样贴。
    const ghost = stageBuffer(ghostRef, width, height);
    if (wantGhost) {
      stagePixels(ghost, compositeFrame(document, ghostOf as number), width, height);
    }
    const current = stageBuffer(frameBufRef, width, height);
    stagePixels(current, compositeFrame(document, index), width, height);
    for (const canvas of tileCanvases("canvas.frame-layer")) {
      const ctx = canvas.getContext("2d");
      if (!ctx) continue;
      ctx.clearRect(0, 0, width, height);
      ctx.imageSmoothingEnabled = false;
      if (wantGhost) {
        ctx.globalAlpha = ONION_ALPHA;
        ctx.drawImage(ghost, 0, 0);
        ctx.globalAlpha = 1;
      }
      ctx.drawImage(current, 0, 0);
    }
  }

  // 播放：帧号在前端自己走，不每帧都去 Rust 取一次图。停下来才把帧号同步回
  // store，让帧条、高亮和 png 对齐。时长极端短的帧也兜个底，不然 setTimeout
  // 会被压成一锅粥。
  // playFrame 刻意不进依赖：它每次播放都在变，把它算进去会让计时器不断重置。
  // 单行注释放这儿盖不住依赖数组上的告警，用块级注释把整个 effect 罩住。
  /* eslint-disable react-hooks/exhaustive-deps */
  useEffect(() => {
    if (!playing || !document) return;
    if (document.frames.length < 2) {
      // 单帧文档没什么可播，但「播放中」这个状态仍会把权威 png 藏掉
      // （.canvas-stack[data-playing="true"] .canvas-img）。没人接管帧层的
      // 话，画布就只剩一块透明——删帧删到只剩一格时正是这条路。补画当前帧。
      paintFrame(Math.min(playFrame, document.frames.length - 1), null);
      return;
    }
    const frames = document.frames;
    let index = playFrame < frames.length ? playFrame : 0;
    let timer = 0;
    const show = (i: number) => {
      paintFrame(i, onion ? i - 1 : null);
      setPlayFrame(i);
    };
    const tick = () => {
      show(index);
      const hold = Math.max(16, frames[index].duration_ms);
      timer = window.setTimeout(() => {
        index = (index + 1) % frames.length;
        tick();
      }, hold);
    };
    tick();
    return () => window.clearTimeout(timer);
  }, [playing, document, onion]);
  /* eslint-enable react-hooks/exhaustive-deps */

  // 不播放时，切帧、洋葱皮开关、文档改动（document_updated / 落笔 / 撤销）
  // 都要把当前帧重新画回来。
  //
  // 这里以前是「只清不画」：洋葱皮开着才 paintFrame，关着就只调
  // clearFrameLayer()。于是点帧条、AI 改完画面之后，帧层被擦成一块透明，
  // 剩下的只有底层那张 <img>——而 img 的 src 要等 refreshPng 那趟往返，
  // 期间画布看着就是空的；img 又只在 revision 不变时才被采信，切帧后
  // 高亮、缩略图和主画布三者对不上，用户看到的就是「选帧之后画布空了」。
  // 帧层本就是覆盖层，唯一正确的动作是按当前帧号重绘：洋葱皮要就带上
  // 一帧，不要就自己画自己，永远别留一块没人写的画布。
  //
  // revision 也必须是触发条件：document 引用不变而内容改了（后端原地改的
  // 文档对象、或者同一快照被复用）时，useStore((s) => s.document) 认不出
  // 变化，重绘就整段跳过。revision 每次 document_updated 都涨，拿它当
  // 第二把钥匙，画布才一定跟得上后端。
  /* eslint-disable react-hooks/exhaustive-deps */
  useEffect(() => {
    if (playing || !document) return;
    paintFrame(frameIndex, onion && frameIndex > 0 ? frameIndex - 1 : null);
  }, [playing, onion, frameIndex, document, revision]);
  /* eslint-enable react-hooks/exhaustive-deps */

  // 预览区可用宽度决定放大倍率，侧栏变窄（<1180px）时倍率要跟着缩。
  useEffect(() => {
    const node = checkerRef.current;
    if (!node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver((entries) => {
      const width = entries[0]?.contentRect.width ?? 0;
      if (width > 0) setBudget(Math.floor(width));
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (aipOpen) return;
    setAipText(null);
  }, [aipOpen]);

  /** 落笔过程的即时反馈：笔迹画在 PNG 上层的透明画布里，抬笔后由新 PNG 接手。 */
  function drawStroke(cells: StrokeCell[], color: InkColor) {
    // 笔迹同步画到每个瓦片：平铺着改图时，回声格也得跟着动。
    for (const canvas of tileCanvases("canvas.paint-layer")) {
      const ctx = canvas.getContext("2d");
      if (!ctx) continue;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      if (cells.length === 0) continue;
      // 擦除也画出痕迹：把格子压暗一块，用户才知道正在擦哪儿。
      ctx.fillStyle = color ?? "rgba(8, 10, 14, 0.4)";
      for (const cell of cells) ctx.fillRect(cell.x * scale, cell.y * scale, scale, scale);
    }
  }

  function flushStroke() {
    if (!paintingRef.current) return;
    paintingRef.current = false;
    const cells = strokeRef.current;
    strokeRef.current = [];
    lastCellRef.current = null;
    drawStroke([], ink);
    // ink 已经是这一笔该落的颜色：橡皮在取 ink 时就被归一成透明。
    void useStore.getState().paintStroke(cells, ink);
  }

  function onPointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (!document) return;
    // 播放中点一下就是「停下」，不再落笔：帧在跳，笔会落到另一帧上。
    if (playing) {
      stopPlay();
      return;
    }
    const cell = cellFromEvent(event, document);
    if (!cell) return;
    if (tool === "fill") {
      // 油漆桶按下就生效，没有「墨迹」要预览。
      void useStore.getState().fillCell(cell.x, cell.y, ink);
      return;
    }
    event.currentTarget.setPointerCapture(event.pointerId);
    paintingRef.current = true;
    strokeRef.current = [cell];
    lastCellRef.current = cell;
    drawStroke([cell], ink);
  }

  function onPointerMove(event: ReactMouseEvent<HTMLDivElement>) {
    if (!paintingRef.current || !document) return;
    const cell = cellFromEvent(event, document);
    const last = lastCellRef.current;
    if (!cell || !last) return;
    if (cell.x === last.x && cell.y === last.y) return;
    // 两次采样之间补线：指针划得快，格子不许断。
    strokeRef.current = appendStroke(strokeRef.current, lineCells(last, cell));
    lastCellRef.current = cell;
    drawStroke(strokeRef.current, ink);
  }

  /** 停播，并把最后停住的那一帧交回 store：高亮、帧条和 png 才有处可去。 */
  function stopPlay() {
    if (playFrame !== frameIndex) void useStore.getState().setActiveFrame(playFrame);
    setPlaying(false);
  }

  /** 播放中直接点帧条：停播并跳过去，别让本地帧号和 store 各走各路。 */
  function jumpToFrame(index: number) {
    setPlaying(false);
    setPlayFrame(index);
    useStore.getState().setActiveFrame(index);
  }

  function togglePlay() {
    if (playing) {
      stopPlay();
      return;
    }
    if (!document || document.frames.length < 2) return;
    // 从当前帧接着播，不从第 0 帧重新来。
    setPlayFrame(frameIndex);
    setPlaying(true);
  }
  async function toggleAipText() {
    if (aipOpen) {
      setAipOpen(false);
      return;
    }
    const text = await useStore.getState().readAipText();
    setAipText(text);
    setAipOpen(true);
  }

  const frames = document?.frames ?? [];
  const firstFrame = frameIndex <= 0;
  const lastFrame = frameIndex >= frames.length - 1;
  // 图层列表反过来显示：最上层在最前面。合成时 layers[0] 画在最底下，
  // 所以 vec 里越靠后越盖得住，箭头上移 = to_index + 1。
  const layersTopFirst = [...(document?.layers ?? [])].reverse();
  const layerCount = document?.layers.length ?? 0;
  const activeLayer = document?.layers.find((layer) => layer.id === active.layer) ?? null;
  const activeLayerIndex = document?.layers.findIndex((layer) => layer.id === active.layer) ?? -1;
  // vec 里越靠后越盖得住：末位就是最上层，首位就是最底层。
  const topLayer = activeLayerIndex < 0 || activeLayerIndex >= layerCount - 1;
  const bottomLayer = activeLayerIndex <= 0;
  const opacityPercent = Math.round(((activeLayer?.opacity ?? 255) / 255) * 100);

  // 图层重命名：就地改，回车落库、Esc 收手，失焦当取消。
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  // renamingRef 是权威：失焦提交时会随 Input 卸载一起触发，那时 state 已经清了，
  // 闭包里读到的仍是旧值，名字照样落库。以 ref 为准，取消先把 ref 抹掉。
  const renamingRef = useRef<string | null>(null);
  const beginRename = (layerId: string, currentName: string) => {
    renamingRef.current = layerId;
    setRenamingId(layerId);
    setRenameDraft(currentName);
  };
  const commitRename = () => {
    const layerId = renamingRef.current;
    renamingRef.current = null;
    setRenamingId(null);
    if (layerId) void useStore.getState().renameLayer(layerId, renameDraft);
  };
  const cancelRename = () => {
    renamingRef.current = null;
    setRenamingId(null);
    setRenameDraft("");
  };

  // 落笔色统一成 ink：橡皮恒为透明，画笔跟随调色板选择。
  const ink: InkColor = tool === "eraser" ? null : (active.color ?? null);
  const canPlay = frames.length > 1;
  // 播放时高亮跟着本地帧号走，store 的 frameIndex 还停在开播那一帧。
  const shownFrame = playing ? playFrame : frameIndex;
  const currentFrame = frames[frameIndex];
  /** 选色即回画笔：刚挑的颜色总得有个工具把它落下去。 */
  function pickColor(color: InkColor) {
    if (tool === "eraser") setTool("brush");
    useStore.getState().setActiveColor(color);
  }

  // ---------- 配色范围 ----------
  // 每层各认领一套范围，改的是「这一层允许用哪些颜色」。scopeLayerId 只是
  // 「用户正在伺候哪一层」的便签，空 = 跟着激活层走；点图层行尾的色片就钉过去。
  const scopeLayer = scopeLayerId
    ? document?.layers.find((layer) => layer.id === scopeLayerId) ?? activeLayer
    : activeLayer;
  const scope =
    document?.palettes.find((item) => item.id === scopeLayer?.palette_id) ?? null;
  const scopeHexes = scope ? scope.colors.map(rgbaToHex) : [];
  // 复制内置预设时的名字后缀：用户得看得出手上这「一套」是抄来的。
  const copySuffix = t("palette.copy_suffix");
  const scopeInUse = scope
    ? (document?.layers ?? []).some((layer) => layer.palette_id === scope.id)
    : false;
  // 取色建议：当前范围全量排在前面，再从色名表补一批常见色，补到二十来个收手，
  // 不然预设条会长到看不见框。
  const suggestHexes = [...scopeHexes];
  for (const item of NAMED_COLORS) {
    if (suggestHexes.length >= 24) break;
    if (!suggestHexes.includes(item.hex)) suggestHexes.push(item.hex);
  }
  /** swatch 提示：色名 + 与当前墨色的关系。 */
  function swatchTip(hex: string, index: number): string {
    if (active.color === hex) return t("palette.swatch_current", { hex: colorName(hex, lang) });
    return t("palette.swatch", { hex: colorName(hex, lang), index: index + 1 });
  }

  /** 换范围：换的是这一层的边界，已有像素由 Rust 按就近色归队。 */
  function pickScope(paletteId: string) {
    if (!scopeLayer || paletteId === "" || paletteId === scopeLayer.palette_id) return;
    void useStore.getState().setLayerPalette(scopeLayer.id, paletteId);
  }

  /** 往范围里加色。内置预设改不得：复制一份副本再往里加，副本顺带接到这一层上。 */
  async function addScopeColor(hex: string) {
    const layer = scopeLayer;
    if (!layer || hex === "") return;
    if (scopeHexes.some((item) => item.toLowerCase() === hex.toLowerCase())) return;
    if (scope?.builtin) {
      // 内置预设改不得，可「复制一份再往里加」不能拆成两条命令跑：文档是异步事件
      // 推回来的，第二条命令要先回读才知道副本 id 是谁，那一下回读可能还没到，颜色
      // 就悄悄丢了。拧成一条 create_palette，from 出副本、colors 加色、layer 把这一
      // 层接过去，像素按就近色归队，一个空档都不留。
      const revision = await useStore.getState().runEditorOps([
        {
          op: "create_palette",
          name: `${scope.name}${copySuffix}`,
          from: layer.palette_id,
          colors: [hex],
          layer: layer.id,
        },
      ]);
      if (revision === null) return;
    } else {
      await useStore.getState().addPaletteColor(layer.palette_id, hex);
    }
    // 挑完即用：新颜色不当当前墨，这一下就白挑了。
    pickColor(hex);
    setHexDraft("");
  }

  /** 手输的十六进制入表。加色那一步（含内置预设 fork）addScopeColor 里已经办了。 */
  function commitHex() {
    const text = hexDraft.trim();
    if (text === "") return;
    // 少了井号也认：从截图、博客、别人色板里抠出来的 hex 常常不带。
    const parsed = parseHex(text.startsWith("#") ? text : `#${text}`);
    if (!parsed) {
      useStore.getState().warnKey("palette.hex_bad");
      return;
    }
    void addScopeColor(rgbaToHex(parsed));
    setHexDraft("");
  }

  /** 从范围里去掉一个颜色，画面上的像素由 Rust 就近归队。 */
  function removeScopeColor(index: number) {
    if (!scope || scope.builtin || scope.colors.length <= 1) return;
    void useStore.getState().removePaletteColor(scope.id, index);
  }

  function openScopeEdit(mode: "new" | "rename") {
    if (mode === "rename" && (!scope || scope.builtin)) return;
    scopeEditingRef.current = mode;
    setScopeEditing(mode);
    // 新建默认以当前套为底子：十有八九只是想改改手上这套，不是从零搭。
    setScopeDraftName(mode === "new" ? `${scope?.name ?? ""}${copySuffix}`.trim() : scope?.name ?? "");
  }

  function cancelScopeEdit() {
    scopeEditingRef.current = null;
    setScopeEditing(null);
    setScopeDraftName("");
  }

  async function commitScopeEdit() {
    const mode = scopeEditingRef.current;
    scopeEditingRef.current = null;
    setScopeEditing(null);
    const name = scopeDraftName.trim();
    setScopeDraftName("");
    if (!scopeLayer || name === "") return;
    if (mode === "new") await useStore.getState().createPalette(name, scopeHexes, scopeLayer.id);
    else if (mode === "rename" && scope && !scope.builtin) void useStore.getState().renamePalette(scope.id, name);
  }

  function deleteScope() {
    if (!scope || scope.builtin || scopeInUse) return;
    void useStore.getState().deletePalette(scope.id);
  }

  /** 图层行尾的配色小片：一眼看出这一层认领哪套范围，点一下就把配色区切过去。 */
  function layerScopeChip(layer: Layer) {
    const item = document?.palettes.find((entry) => entry.id === layer.palette_id) ?? null;
    const dots = item ? item.colors.slice(0, 4).map(rgbaToHex) : [];
    return (
      <Tooltip
        title={
          <>
            <div>{item?.name ?? layer.palette_id}</div>
            <div>{layer.locked ? t("palette.lock_on") : t("palette.lock_off")}</div>
          </>
        }
      >
        <span
          className="layer-scope-chip"
          onClick={(event) => {
            // 点色片就是「我要改这一层」：既当选中，也把配色区的伺候对象钉过去。
            event.stopPropagation();
            setScopeLayerId(layer.id);
            void useStore.getState().setActiveLayer(layer.id);
          }}
        >
          {dots.map((hex, index) => (
            <i key={`${hex}-${index}`} style={{ background: hex }} />
          ))}
        </span>
      </Tooltip>
    );
  }

  // ---------- 右键菜单 ----------
  // 菜单跟着鼠标所在的位置出现：画布问「现在在干什么」，图层行问「这一层怎么了」，
  // 帧格问「这一帧怎么排」。三处各报各的事，菜单本体不认识任何业务。

  /** 画布：工具、撤销、播放、洋葱皮、瓦片份数，都是档位，勾在哪就是现在哪一档。 */
  function openCanvasMenu(event: ReactMouseEvent<HTMLDivElement>) {
    const tile = (mode: 1 | 2 | 3): ContextMenuItem => ({
      key: `tile-${mode}`,
      label: `${t("menu.tile")} ${mode}x${mode}`,
      checked: tileMode === mode,
      onSelect: () => setTileMode(mode),
    });
    openContextMenu(event, [
      {
        key: "brush",
        label: t("doc.brush"),
        icon: <Brush size={13} />,
        checked: tool === "brush",
        onSelect: () => setTool("brush"),
      },
      {
        key: "fill",
        label: t("doc.fill"),
        icon: <PaintBucket size={13} />,
        checked: tool === "fill",
        onSelect: () => setTool("fill"),
      },
      {
        key: "eraser",
        label: t("doc.eraser"),
        icon: <Eraser size={13} />,
        checked: tool === "eraser",
        onSelect: () => setTool("eraser"),
      },
      {
        key: "undo",
        label: t("menu.undo"),
        icon: <Undo2 size={13} />,
        disabled: undoDepth === 0,
        onSelect: () => void useStore.getState().undoEdit(),
      },
      {
        key: "onion",
        label: t("menu.onion"),
        icon: <Ghost size={13} />,
        checked: onion,
        disabled: !canPlay,
        onSelect: () => setOnion((value) => !value),
      },
      {
        key: "play",
        label: playing ? t("menu.pause") : t("doc.play"),
        icon: playing ? <Pause size={13} /> : <Play size={13} />,
        disabled: !canPlay,
        onSelect: () => togglePlay(),
      },
      tile(1),
      tile(2),
      tile(3),
    ]);
  }

  /** 图层行：单层的事全在这一间，带着是哪一层。 */
  function openLayerMenu(event: ReactMouseEvent<HTMLDivElement>, layerId: string) {
    const layers = document?.layers ?? [];
    const index = layers.findIndex((layer) => layer.id === layerId);
    const layer = index >= 0 ? layers[index] : null;
    if (!layer) return;
    const move = (delta: number) => {
      const target = index + delta;
      // 到头了不硬挪：to_index 越界的话 Rust 会夹到末尾，看起来就像点坏了。
      if (target < 0 || target >= layers.length) return;
      void useStore.getState().runEditorOps([{ op: "move_layer", id: layerId, to_index: target }]);
    };
    openContextMenu(event, [
      {
        key: "rename",
        label: t("menu.layer_rename"),
        icon: <Pencil size={13} />,
        onSelect: () => beginRename(layerId, layer.name),
      },
      {
        key: "scope",
        label: t("menu.layer_scope"),
        icon: <Palette size={13} />,
        onSelect: () => {
          setScopeLayerId(layerId);
          void useStore.getState().setActiveLayer(layerId);
        },
      },
      {
        key: "visible",
        label: layer.visible ? t("menu.layer_hide") : t("menu.layer_show"),
        icon: layer.visible ? <EyeOff size={13} /> : <Eye size={13} />,
        onSelect: () => void useStore.getState().setLayerVisible(layerId, !layer.visible),
      },
      {
        key: "up",
        label: t("menu.layer_up"),
        icon: <ArrowUp size={13} />,
        disabled: index >= layers.length - 1,
        onSelect: () => move(1),
      },
      {
        key: "down",
        label: t("menu.layer_down"),
        icon: <ArrowDown size={13} />,
        disabled: index <= 0,
        onSelect: () => move(-1),
      },
      {
        key: "delete",
        label: t("menu.layer_delete"),
        icon: <Trash2 size={13} />,
        danger: true,
        disabled: layers.length <= 1,
        onSelect: () => void useStore.getState().runEditorOps([{ op: "delete_layer", id: layerId }]),
      },
    ]);
  }

  /** 帧格：从这儿播、复制、删掉、前后挪。帧的顺序就是播放的顺序，值得单独一间菜单。 */
  function openFrameMenu(event: ReactMouseEvent<HTMLButtonElement>, index: number) {
    const items: ContextMenuItem[] = [
      {
        key: "play",
        label: t("menu.play_from"),
        icon: <Play size={13} />,
        disabled: !canPlay,
        onSelect: () => {
          setPlayFrame(index);
          setPlaying(true);
        },
      },
      { key: "dup", label: t("doc.duplicate_frame"), icon: <Copy size={13} />, onSelect: () => void useStore.getState().duplicateFrame() },
      { key: "del", label: t("doc.delete_frame"), icon: <Trash2 size={13} />, danger: true, disabled: frames.length <= 1, onSelect: () => void useStore.getState().deleteFrame() },
      { key: "earlier", label: t("doc.move_earlier"), icon: <ArrowLeft size={13} />, disabled: index <= 0, onSelect: () => void useStore.getState().moveFrame(-1) },
      { key: "later", label: t("doc.move_later"), icon: <ArrowRight size={13} />, disabled: index >= frames.length - 1, onSelect: () => void useStore.getState().moveFrame(1) },
    ];
    openContextMenu(event, items);
  }

  return (
    <aside className="panel doc">
      <div className="doc-preview">
        <div className="canvas-tools">
          <Segmented
            size="small"
            value={tool}
            onChange={(value) => setTool(value as EditorTool)}
            options={[
              {
                value: "brush",
                label: (
                  <span className="tool-label">
                    <Brush size={13} /> {t("doc.brush")}
                  </span>
                ),
              },
              {
                value: "fill",
                label: (
                  <span className="tool-label">
                    <PaintBucket size={13} /> {t("doc.fill")}
                  </span>
                ),
              },
              {
                value: "eraser",
                label: (
                  <span className="tool-label">
                    <Eraser size={13} /> {t("doc.eraser")}
                  </span>
                ),
              },
            ]}
          />
          {/* 瓦片底图：平铺开才看得出接缝。选项格号就是倍率，不用再多解释。 */}
          <Tooltip title={t("doc.tile_tip")}>
            <Segmented
              size="small"
              value={tileMode}
              onChange={(value) => setTileMode(value as 1 | 2 | 3)}
              options={[
                { value: 1, label: <span className="tile-label">1x1</span> },
                { value: 2, label: <span className="tile-label">2x2</span> },
                { value: 3, label: <span className="tile-label">3x3</span> },
              ]}
            />
          </Tooltip>
          <Tooltip title={canPlay ? t("doc.play") : t("doc.play_need_frames")}>
            <Button
              size="small"
              type="text"
              icon={playing ? <Pause size={14} /> : <Play size={14} />}
              disabled={!canPlay}
              onClick={togglePlay}
            />
          </Tooltip>
          <Tooltip title={t("doc.onion")}>
            <Button
              size="small"
              type="text"
              icon={<Ghost size={14} />}
              className={onion ? "tool-on" : ""}
              disabled={!canPlay}
              onClick={() => setOnion((value) => !value)}
            />
          </Tooltip>
          <span className="grow" />
          <Tooltip
            title={
              undoDepth > 0 ? t("doc.undo", { count: undoDepth }) : t("doc.undo_none")
            }
          >
            <Button
              size="small"
              type="text"
              icon={<Undo2 size={14} />}
              disabled={undoDepth === 0}
              onClick={() => void useStore.getState().undoEdit()}
            />
          </Tooltip>
        </div>
        <div className="checker" ref={checkerRef}>
          {pngUrl && document ? (
            // 瓦片平铺：每格都是同一幅画，帧层与笔迹层各有一份，
            // 只有中间那一格接管指针——回声格是用来看的，不是用来点的。
            <div
              ref={tilesRef}
              className="canvas-tiles"
              style={{
                gridTemplateColumns: `repeat(${tileMode}, ${document.width * scale}px)`,
              }}
            >
              {Array.from({ length: tileMode * tileMode }, (_, tile) => {
                const col = tile % tileMode;
                const row = Math.floor(tile / tileMode);
                const live = col === centerIndex && row === centerIndex;
                return (
                  <div
                    key={tile}
                    className="canvas-stack"
                    data-live={live ? "true" : undefined}
                    data-playing={playing ? "true" : undefined}
                    style={{
                      width: document.width * scale,
                      height: document.height * scale,
                    }}
                  >
                    <img
                      src={pngUrl}
                      className="canvas-img"
                      alt={live ? t("doc.canvas_alt") : ""}
                      width={document.width * scale}
                      height={document.height * scale}
                    />
                    <canvas
                      className="frame-layer"
                      width={document.width}
                      height={document.height}
                    />
                    <canvas
                      className="paint-layer"
                      width={document.width * scale}
                      height={document.height * scale}
                    />
                    {live ? (
                      <div
                        className="canvas-overlay"
                        onPointerDown={onPointerDown}
                        onPointerMove={onPointerMove}
                        onPointerUp={flushStroke}
                        onPointerCancel={flushStroke}
                        onContextMenu={openCanvasMenu}
                      />
                    ) : null}
                  </div>
                );
              })}
            </div>
          ) : null}
        </div>
        <div className="doc-meta">
          <span>{document?.name ?? t("doc.empty")}</span>
          <Tooltip title={t("doc.size_edit")}>
            <span
              className="doc-size"
              role="button"
              tabIndex={0}
              aria-label={t("doc.size_edit")}
              data-disabled={document ? undefined : "true"}
              onClick={() => document && openSizeModal()}
              onKeyDown={(event) => {
                if (event.key !== "Enter" && event.key !== " ") return;
                event.preventDefault();
                if (document) openSizeModal();
              }}
            >
              {document ? `${document.width}x${document.height}` : "--"}
            </span>
          </Tooltip>
          <span>{t("doc.rev", { rev: revision })}</span>
        </div>
      </div>

      <div className="panel-body">
        <div className="doc-section">
          <div className="doc-section-title">
            <Layers size={12} />
            {t("doc.layers")}
            <span className="grow" />
            <Tooltip title={t("doc.new_layer")}>
              <Button
                size="small"
                type="text"
                icon={<Plus size={13} />}
                disabled={!document}
                onClick={() => void useStore.getState().addLayer()}
              />
            </Tooltip>
            <Tooltip title={t("doc.delete_layer")}>
              <Button
                size="small"
                type="text"
                icon={<Trash2 size={13} />}
                disabled={layerCount <= 1}
                onClick={() => void useStore.getState().deleteLayer(active.layer)}
              />
            </Tooltip>
            <Tooltip title={t("doc.layer_up")}>
              <Button
                size="small"
                type="text"
                icon={<ArrowUp size={13} />}
                disabled={topLayer}
                onClick={() => void useStore.getState().moveLayer(1)}
              />
            </Tooltip>
            <Tooltip title={t("doc.layer_down")}>
              <Button
                size="small"
                type="text"
                icon={<ArrowDown size={13} />}
                disabled={bottomLayer}
                onClick={() => void useStore.getState().moveLayer(-1)}
              />
            </Tooltip>
          </div>
          {/* 最上层排在最前面，和 Aseprite / Photoshop 的图层列表同一约定：
              箭头向上就是往栈顶走，方向不需要在脑子里换算一次。 */}
          {layersTopFirst.map((layer) => (
            <div
              key={layer.id}
              className={`layer-row ${active.layer === layer.id ? "active" : ""}`}
              onContextMenu={(event) => openLayerMenu(event, layer.id)}
            >
              <Tooltip title={t("doc.layer_visible")}>
                <button
                  type="button"
                  className={`layer-eye ${layer.visible ? "" : "off"}`}
                  aria-label={t("doc.layer_visible")}
                  onClick={() =>
                    void useStore.getState().setLayerVisible(layer.id, !layer.visible)
                  }
                >
                  {layer.visible ? <Eye size={13} /> : <EyeOff size={13} />}
                </button>
              </Tooltip>
              {/* 原来是个 <button>：改名要往里塞输入框和改名按钮，
                  套 button 不合法，整块改成 div。 */}
              <div
                className="layer-name"
                onClick={() => useStore.getState().setActiveLayer(layer.id)}
              >
                {renamingId === layer.id ? (
                  <Input
                    autoFocus
                    size="small"
                    className="layer-rename"
                    onClick={(event) => event.stopPropagation()}
                    value={renameDraft}
                    onChange={(event) => setRenameDraft(event.target.value)}
                    onPressEnter={commitRename}
                    onBlur={() => commitRename()}
                    onDoubleClick={(event) => event.stopPropagation()}
                    onKeyDown={(event) => {
                      // Esc 走 keydown：preventDefault 拦下的是「清空」不是交给窗口的快捷键。
                      if (event.key === "Escape") {
                        event.preventDefault();
                        event.stopPropagation();
                        cancelRename();
                      }
                    }}
                  />
                ) : (
                  <>
                    <span className="layer-id">{layer.id}</span>
                    <span
                      className="grow layer-label"
                      onDoubleClick={() => beginRename(layer.id, layer.name)}
                    >
                      {layer.name}
                    </span>
                    <Tooltip title={t("doc.layer_rename")}>
                      <button
                        type="button"
                        className="layer-rename-btn"
                        aria-label={t("doc.layer_rename")}
                        onClick={(event) => {
                          event.stopPropagation();
                          beginRename(layer.id, layer.name);
                        }}
                      >
                        <Pencil size={11} />
                      </button>
                    </Tooltip>
                    {/* 行尾一小片配色：这一层认领的是哪套范围，锁没锁，hover 才细说。 */}
                    {layerScopeChip(layer)}
                  </>
                )}
              </div>
            </div>
          ))}
          {activeLayer ? (
            <div className="layer-opacity">
              <span className="layer-opacity-label">{t("doc.layer_opacity")}</span>
              <Slider
                className="layer-opacity-slider"
                min={0}
                max={255}
                value={activeLayer.opacity}
                tooltip={{ formatter: (value) => `${Math.round(((value ?? 0) / 255) * 100)}%` }}
                onChange={(value) =>
                  void useStore.getState().setLayerOpacity(activeLayer.id, value)
                }
              />
              <span className="layer-opacity-value">{opacityPercent}%</span>
            </div>
          ) : null}
        </div>

        <div className="doc-section">
          <div className="doc-section-title">
            {t("doc.frames")}
            <span className="grow" />
            <Tooltip title={t("doc.new_frame")}>
              <Button
                size="small"
                type="text"
                icon={<Plus size={13} />}
                onClick={() => void useStore.getState().addFrame()}
              />
            </Tooltip>
            <Tooltip title={t("doc.duplicate_frame")}>
              <Button
                size="small"
                type="text"
                icon={<Copy size={13} />}
                onClick={() => void useStore.getState().duplicateFrame()}
              />
            </Tooltip>
            <Tooltip title={t("doc.delete_frame")}>
              <Button
                size="small"
                type="text"
                icon={<Trash2 size={13} />}
                disabled={frames.length <= 1}
                onClick={() => void useStore.getState().deleteFrame()}
              />
            </Tooltip>
            <Tooltip title={t("doc.move_earlier")}>
              <Button
                size="small"
                type="text"
                icon={<ArrowLeft size={13} />}
                disabled={firstFrame}
                onClick={() => void useStore.getState().moveFrame(-1)}
              />
            </Tooltip>
            <Tooltip title={t("doc.move_later")}>
              <Button
                size="small"
                type="text"
                icon={<ArrowRight size={13} />}
                disabled={lastFrame}
                onClick={() => void useStore.getState().moveFrame(1)}
              />
            </Tooltip>
          </div>
          <div className="frame-strip">
            {frames.map((frame, index) => (
              <button
                type="button"
                key={frame.id}
                className={`frame-chip ${index === shownFrame ? "active" : ""}`}
                onClick={() => jumpToFrame(index)}
                onContextMenu={(event) => openFrameMenu(event, index)}
              >
                {document ? <FrameThumb document={document} index={index} /> : null}
                <span className="frame-chip-id">{frame.id}</span>
                <span className="frame-chip-ms">{frame.duration_ms}ms</span>
              </button>
            ))}
          </div>
          <div className="frame-duration">
            <span className="frame-duration-label">{t("doc.frame_duration")}</span>
            <InputNumber
              size="small"
              className="frame-duration-input"
              min={1}
              max={60000}
              step={10}
              value={currentFrame?.duration_ms ?? 100}
              onChange={(next) => void useStore.getState().setFrameDuration(next ?? 100)}
            />
            <span className="frame-duration-unit">ms</span>
          </div>
        </div>

        <div className="doc-section">
          <div className="doc-section-title">
            {t("palette.title")}
            <span className="grow" />
            <Tooltip title={t("palette.new_hint")}>
              <Button
                size="small"
                type="text"
                icon={<Plus size={13} />}
                disabled={!scopeLayer}
                onClick={() => openScopeEdit("new")}
              />
            </Tooltip>
          </div>

          {/* 改的是谁：配色范围每层独立，动表之前先说清楚伺候的是哪一层。 */}
          <div className="scope-row">
            <span className="scope-label">{t("palette.layer")}</span>
            <Select
              size="small"
              className="scope-layer-select"
              value={scopeLayer?.id ?? undefined}
              disabled={!document || document.layers.length < 2}
              onChange={(value) => setScopeLayerId(value === activeLayer?.id ? null : value)}
              options={(document?.layers ?? []).map((layer) => ({
                value: layer.id,
                label: layer.name,
              }))}
            />
          </div>

          {/* 配色锁：锁上就只许用范围里的颜色，越界的 Rust 就近归队；AI 也改不了这张表。 */}
          <div className="scope-row">
            <span className="scope-label">{t("palette.lock")}</span>
            <Switch
              size="small"
              className="scope-lock-switch"
              checked={scopeLayer?.locked ?? false}
              disabled={!scopeLayer}
              onChange={(value) => {
                if (scopeLayer) void useStore.getState().setLayerLocked(scopeLayer.id, value);
              }}
            />
            <Tooltip title={scopeLayer?.locked ? t("palette.lock_on") : t("palette.lock_off")}>
              <span className="scope-state">
                {scopeLayer?.locked ? t("palette.state_locked") : t("palette.state_open")}
              </span>
            </Tooltip>
          </div>

          {/* 哪一套：内置的是只读的，想改就复制一份，改名和删除只对自建的开。 */}
          <div className="scope-row">
            <Select
              size="small"
              className="scope-select"
              value={scope?.id ?? undefined}
              onChange={pickScope}
              options={(document?.palettes ?? []).map((item) => ({
                value: item.id,
                label: (
                  <span className="scope-option">
                    <span className="scope-option-name">{item.name}</span>
                    <span className={`scope-tag ${item.builtin ? "builtin" : "custom"}`}>
                      {item.builtin ? t("palette.builtin_tag") : t("palette.custom_tag")}
                    </span>
                  </span>
                ),
              }))}
            />
            {scope && !scope.builtin ? (
              <>
                <Tooltip title={t("palette.rename")}>
                  <Button
                    size="small"
                    type="text"
                    icon={<Pencil size={13} />}
                    onClick={() => openScopeEdit("rename")}
                  />
                </Tooltip>
                <Tooltip title={scopeInUse ? t("palette.delete_used") : t("palette.delete")}>
                  <Button
                    size="small"
                    type="text"
                    danger
                    icon={<Trash2 size={13} />}
                    disabled={scopeInUse}
                    onClick={deleteScope}
                  />
                </Tooltip>
              </>
            ) : (
              <Tooltip title={t("palette.scope_hint")}>
                <span className="scope-state">{t("palette.builtin_tag")}</span>
              </Tooltip>
            )}
          </div>
          {scopeEditing !== null ? (
            <Input
              autoFocus
              size="small"
              className="scope-rename"
              value={scopeDraftName}
              placeholder={t("palette.new_placeholder")}
              onChange={(event) => setScopeDraftName(event.target.value)}
              onPressEnter={() => void commitScopeEdit()}
              onBlur={() => void commitScopeEdit()}
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  event.stopPropagation();
                  cancelScopeEdit();
                }
              }}
            />
          ) : null}

          <div className="palette-grid">
            <Tooltip title={t("doc.transparent")}>
              <button
                type="button"
                className={`swatch eraser ${active.color === null ? "active" : ""}`}
                onClick={() => pickColor(null)}
              />
            </Tooltip>
            {scopeHexes.map((hex, index) => (
              <span className="swatch-cell" key={`${hex}-${index}`}>
                <Tooltip title={swatchTip(hex, index)}>
                  <button
                    type="button"
                    className={`swatch ${active.color === hex ? "active" : ""}`}
                    style={{ background: hex }}
                    onClick={() => pickColor(hex)}
                  />
                </Tooltip>
                {/* 自建的范围才让拆色：内置的是一个色都不动，删了别人还怎么用。 */}
                {scope && !scope.builtin ? (
                  <Tooltip title={t("palette.remove_color")}>
                    <button
                      type="button"
                      className="swatch-remove"
                      aria-label={t("palette.remove_color")}
                      onClick={() => removeScopeColor(index)}
                    >
                      <X size={10} />
                    </button>
                  </Tooltip>
                ) : null}
              </span>
            ))}
            {/* 「任意颜色」：一套里只摆得下这一个入口，但从这儿进去的颜色
                一颗一颗落在表里，等于无限加色。 */}
            <Tooltip
              title={
                <>
                  <div>{t("palette.add_color")}</div>
                  <div>{t("palette.add_color_tip")}</div>
                </>
              }
            >
              <div className={`swatch-slot ${swatchDraft !== null ? "active" : ""}`}>
                <ColorPicker
                  format="hex"
                  allowClear
                  showText={false}
                  value={swatchDraft ?? undefined}
                  presets={[
                    { label: t("palette.scope"), colors: scopeHexes },
                    {
                      label: t("palette.suggest"),
                      colors: NAMED_COLORS.slice(0, 18).map((item) => item.hex),
                    },
                  ]}
                  onChange={(value) => setSwatchDraft(value.toHexString())}
                  onChangeComplete={(value) => {
                    const hex = value.toHexString();
                    // 拖完松手才落文档：拖动中途一路加色，撤销栈会糊成一锅粥。
                    setSwatchDraft(null);
                    void addScopeColor(hex);
                  }}
                  onClear={() => setSwatchDraft(null)}
               />
             </div>
           </Tooltip>
         </div>
          {/* 手输十六进制：拖拽取色应对不了「我要的就是这一串 hex」，
              配色行当里这个通行证必须留。回车即加，加完顺手当当前墨。 */}
          <div className="palette-hex-row">
            <Input
              size="small"
              className="palette-hex-input"
              placeholder="#RRGGBB"
              maxLength={8}
              value={hexDraft}
              onChange={(event) => setHexDraft(event.target.value)}
              onPressEnter={() => void commitHex()}
            />
            <Button
              size="small"
              type="primary"
              disabled={hexDraft.trim() === ""}
              onClick={() => void commitHex()}
            >
              {t("palette.hex_add")}
            </Button>
          </div>
        </div>

        <div className="doc-section">
          <Tooltip title={t("doc.aip_tooltip")}>
            <Button size="small" type="text" icon={<FileCode2 size={13} />} onClick={toggleAipText}>
              {aipOpen ? t("doc.hide_aip") : t("doc.show_aip")}
            </Button>
          </Tooltip>
          {aipOpen && aipText !== null ? <pre className="aip-text">{aipText}</pre> : null}
        </div>
      </div>

      <CanvasSizeModal
        open={sizeOpen}
        width={sizeDraft.width}
        height={sizeDraft.height}
        title={t("doc.resize_title")}
        okText={t("doc.resize_ok")}
        cancelText={t("doc.resize_cancel")}
        widthLabel={t("sidebar.width")}
        heightLabel={t("sidebar.height")}
        presetsLabel={t("sidebar.presets")}
        readout={t("sidebar.size_readout", {
          width: sizeDraft.width,
          height: sizeDraft.height,
          cells: sizeDraft.width * sizeDraft.height,
        })}
        hint={t("doc.resize_hint")}
        onChange={(width, height) => setSizeDraft({ width, height })}
        onOk={() => {
          setSizeOpen(false);
          void useStore.getState().resizeCanvas(sizeDraft.width, sizeDraft.height);
        }}
        onCancel={() => setSizeOpen(false)}
      />
    </aside>
  );
}
