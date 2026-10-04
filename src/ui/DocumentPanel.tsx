// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
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
  ChevronsLeft,
  ChevronsRight,
  Brush,
  Copy,
  Droplet,
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
  Circle,
  Minus,
  PenTool,
  Square,
  Triangle,
  Trash2,
  Undo2,
  Redo2,
  X,
  User,
} from "lucide-react";

import { useStore } from "../lib/store";
import { useT } from "../lib/t";
import { appendStroke, lineCells } from "../lib/stroke";
import {
  shapeCells,
  smoothPathCells,
  type ShapeKind,
} from "../lib/shapes";
import { compositeFrame } from "../lib/render";
import { parseHex, rgbaToHex, swatchCommitOnClose } from "../lib/palette";
import { NAMED_COLORS, colorName } from "../lib/colornames";
import { tryCapturePointer } from "../lib/pointer-capture";
import PaletteOpsModal, { type PaletteChoice, type PaletteOpRequest } from "./PaletteOpsModal";
import FrameThumb from "./FrameThumb";
import HStrip from "./HStrip";
import { stripScrollFor, type StripEdges } from "./strip";
import CanvasSizeModal from "./CanvasSizeModal";
import { isSheetGrid } from "../lib/sheet";
import { openContextMenu, type ContextMenuItem } from "./ContextMenu";
import type {
  EditorTool,
  InkColor,
  Layer,
  PixelDocument,
  StrokeCell,
} from "../lib/types";
import type { NamedPalette } from "../lib/types";

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

/**
 * 工具 -> 形状档位。画笔、橡皮、油漆桶是「点什么算什么」，没有形状；
 * 其余五个工具本身就是形状的名字。
 *
 * 平滑曲线虽然也是形状，但它不吃锚点：采样点自己就是输入。所以预览和
 * 拖动两处都要把它单独摘出来，别和「锚点 -> 松手」那一族混在一起。
 */
function shapeOf(tool: EditorTool): ShapeKind | null {
  switch (tool) {
    case "line":
    case "rect":
    case "ellipse":
    case "triangle":
    case "smooth":
      return tool;
    default:
      return null;
  }
}

/** 能吃「实心」开关的形状。直线只会描边，平滑曲线没有填充这一说。 */
function canSolid(tool: EditorTool): boolean {
  return tool === "line" || tool === "rect" || tool === "ellipse" || tool === "triangle";
}

/** 圆角只对矩形和三角形有意义：椭圆本来就滑，直线滑起来就不是直线了。 */
function canCorner(tool: EditorTool): boolean {
  return tool === "rect" || tool === "triangle";
}

/**
 * 工具条上的一枚工具：图标 + 悬浮提示，不要文字。
 *
 * 工具条在 1168px 宽的窗口里只剩两百多像素，「画笔 / 矩形 / 三角形」一排
 * 文字挤下去会折成三四行，把画布顶出视野。图标加提示既看得见是什么、
 * 又只占一格宽，鼠标停一下就知道名字。
 */
function toolOption(value: EditorTool, tip: string, icon: ReactNode) {
  return {
    value,
    label: (
      <Tooltip title={tip}>
        <span className="tile-label" aria-label={tip}>
          {icon}
        </span>
      </Tooltip>
    ),
  };
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
  const pngFrame = useStore((s) => s.pngFrame);
  const active = useStore((s) => s.active);
  const frameIndex = useStore((s) => s.frameIndex);
  const revision = useStore((s) => s.revision);
  const lang = useStore((s) => s.lang);
  const undoDepth = useStore((s) => s.undoStack.length);
  const redoDepth = useStore((s) => s.redoStack.length);
  const [tool, setTool] = useState<EditorTool>("brush");
  // 形状的圆角量：短边比例，0 = 尖角。只对矩形/三角形有意义——
  // 椭圆本来就滑，直线滑起来就不是直线了。
  const [corner, setCorner] = useState(0);
  // 实心开关：形状落下来是实心一块还是只描边。这是个独立维度，和「选哪个
  // 工具」分开——早期把实心绑在油漆桶上，用户勾着填充拖矩形得到的是泼油漆，
  // 而不是一块实心矩形，界面上还看不出来为什么。
  const [solid, setSolid] = useState(false);
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
  // 取色盘正在挑的草稿值：拖动过程中只预览，落文档等松手或关弹层。
  const [swatchDraft, setSwatchDraft] = useState<string | null>(null);
  // 草稿的镜像副本：关弹层是异步回调，闭包里读 state 会读到关之前那一帧。
  const swatchDraftRef = useRef<string | null>(null);
  // 这一轮弹层里已经落过文档的那个色：同一色不许落第二次（拖动会和
  // 「关弹层收尾」撞车，antd 的 onChangeComplete 只由滑块松手触发，点预设
  // 和手输都不触发，所以收尾还得靠关弹层）。
  const swatchDoneRef = useRef<string | null>(null);
  // 手输十六进制的草稿。取色盘拖不出「我就要这个 #hex」，而像素行当里 hex 是通行证。
  const [hexDraft, setHexDraft] = useState<string>("");
  // 配色区的行内输入：null = 收起，"new" = 新建一套，"rename" = 给当前套改名。
  // 和图层/会话改名同一套：ref 是权威，失焦提交时 state 已经清了。
  const [scopeEditing, setScopeEditing] = useState<"new" | "rename" | null>(null);
  const [scopeDraftName, setScopeDraftName] = useState("");
  const scopeEditingRef = useRef<"new" | "rename" | null>(null);
  // 三件「会动画面」的事共用一个确认弹窗：换范围、删色、删整套。
  // null = 关着。弹窗开着的时候不许顺手再开一个，否则两问的答案会串。
  const [paletteOp, setPaletteOp] = useState<PaletteOpRequest | null>(null);
  const checkerRef = useRef<HTMLDivElement>(null);
  // 一笔笔画的临时状态全在 ref 里：pointermove 不该触发 React 渲染。
  const strokeRef = useRef<StrokeCell[]>([]);
  const lastCellRef = useRef<StrokeCell | null>(null);
  const paintingRef = useRef(false);
  // 形状工具的锚点（按下的那一格）和平滑曲线的采样点。放 ref 是因为
  // pointermove 不该触发 React 渲染：形状预览重算在 drawStroke 里就完成了。
  const shapeAnchorRef = useRef<StrokeCell | null>(null);
  const smoothPointsRef = useRef<StrokeCell[]>([]);
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
      // 时长是文档字段，缺失/非法时 setTimeout(fn, NaN) 会当成 0 立刻重入：
      // 一个 tick 套一个 tick，主线程被播放循环吃满，界面看着就是卡死。
      // 兜底取 83ms（≈12FPS，像素动画的通常节拍），并给帧号加一道越界守卫。
      const raw = frames[index]?.duration_ms;
      const hold = Number.isFinite(raw) ? Math.max(16, Math.floor(raw as number)) : 83;
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
      // 非有限值一概不认（回授里有 NaN 会一路传到倍率计算上），2px 内的抖动也放过：
      // 那是同一次布局的回授，不是用户真的拖了侧栏，逐个 setState 会和滚动条打架。
      setBudget((prev) => {
        const next = Math.floor(width);
        if (!Number.isFinite(next) || next <= 0) return prev;
        return Math.abs(next - prev) <= 2 ? prev : next;
      });
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  // 收起就把缓存清掉：aipOpen 由真转假的那次渲染里就地清，不进 effect——
  // effect 体内同步 setState 要多走一次提交，React 明确不推荐。
  const [aipSeenOpen, setAipSeenOpen] = useState(aipOpen);
  if (aipOpen !== aipSeenOpen) {
    setAipSeenOpen(aipOpen);
    if (!aipOpen) setAipText(null);
  }

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
    shapeAnchorRef.current = null;
    smoothPointsRef.current = [];
    drawStroke([], ink);
    // ink 已经是这一笔该落的颜色：橡皮在取 ink 时就被归一成透明。
    void useStore.getState().paintStroke(cells, ink);
  }

  /**
   * 形状档位下「从锚点到现在这一格」该出现哪些格子。
   *
   * 实心与否看油漆桶：用户勾着填充再拖矩形，要的就是一块实心；单独拖是描边。
   * 平滑曲线不吃锚点，采样点自己就是输入。
   */
  function shapePreview(anchor: StrokeCell, now: StrokeCell): StrokeCell[] {
    // 形状的几何全在 shapes.ts 里收口：这里只说明语义（填充与否、圆角多少），
    // 每加一种形状或一个参数都不用回来改这段 switch。
    const shape = shapeOf(tool);
    if (!shape || shape === "smooth") return [];
    return shapeCells(shape, anchor, now, { filled: solid, radius: corner });
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
    // 油漆桶按下就生效，没有「墨迹」要预览。早期它一身兼两职：没挂形状时
    // 泼油漆、挂了形状时变「实心形状」，而界面上一点提示都没有——用户看到
    // 的就是「填充时好时坏」。现在实心是独立开关，油漆桶只管泼油漆。
    if (tool === "fill") {
      void useStore.getState().fillCell(cell.x, cell.y, ink);
      return;
    }
    paintingRef.current = true;
    strokeRef.current = [cell];
    lastCellRef.current = cell;
    // 形状工具把这一格当锚点记下来：拖动途中每帧都从锚点重算整条形状，
    // 所以「预览 = 落笔结果」，松手时不需要再做第二次换算。
    shapeAnchorRef.current = cell;
    smoothPointsRef.current = [cell];
    drawStroke([cell], ink);
    // 指针捕获放最后，且不许它坏事：capture 只是让指针划出画布外还能继续收到
    // pointermove，属于锦上添花；而它对「这个 pointerId 不是活动指针」是会
    // 抛 NotFoundError 的（触屏拖动、合成事件、双指交替都会碰上）。放在这里
    // 之前，一笔下去整段函数当场中断——既没 preview 也没置 paintingRef，
    // pointerup 的 flushStroke 又直接 return，于是用户看到「点了没反应」。
    // 现在顺序反过来：先画上，再试着捕获；拿不到捕获也不过是划出界外不跟手。
    tryCapturePointer(event.currentTarget, event.pointerId);
  }

  function onPointerMove(event: ReactMouseEvent<HTMLDivElement>) {
    if (!paintingRef.current || !document) return;
    const cell = cellFromEvent(event, document);
    const last = lastCellRef.current;
    if (!cell || !last) return;
    if (cell.x === last.x && cell.y === last.y) return;
    // 形状工具：锚点不动，整条形状按「锚点 -> 现在这一格」重算，预览即结果。
    const shape = shapeOf(tool);
    if (shape && shape !== "smooth") {
      const anchor = shapeAnchorRef.current ?? cell;
      strokeRef.current = shapePreview(anchor, cell);
      lastCellRef.current = cell;
      drawStroke(strokeRef.current, ink);
      return;
    }
    // 平滑曲线：每个采样点都收着，预览直接给平滑后的结果。拖动时看到的就是
    // 最终会落文档的那条曲线，不会出现「预览是折线、落笔变弧线」的错觉。
    if (shape === "smooth") {
      const points = appendStroke(smoothPointsRef.current, lineCells(last, cell));
      smoothPointsRef.current = points;
      strokeRef.current = points.length >= 3 ? smoothPathCells(points) : points;
      lastCellRef.current = cell;
      drawStroke(strokeRef.current, ink);
      return;
    }
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
  const frameStripRef = useRef<HTMLDivElement | null>(null);
  const currentFrame = frames[frameIndex];
  // 帧条「还能往哪滚」：溢出来才亮翻页按钮，不然空着两个箭头只会显得碍眼。
  // 状态由 HStrip 回吐——竖滚轮翻成横滚、常驻滚动条、两端渐隐都归它管，
  // 这里只留「要不要亮箭头」这一件自己用得上的事。
  const [stripScroll, setStripScroll] = useState<StripEdges>({ canLeft: false, canRight: false });
  const onFrameEdges = useCallback((next: StripEdges) => {
    setStripScroll((prev) =>
      prev.canLeft === next.canLeft && prev.canRight === next.canRight ? prev : next,
    );
  }, []);

  // 帧条横向滚：切帧要把那一格滚进视野。播放时每帧都挪，不然用户看的是左边、
  // 放的是右边。手算 scrollLeft 而不叫 chip.scrollIntoView——那个连外层
  // panel-body 一起滚，画面会跟着帧条上下蹦。
  //   偏移量只认「格子和条子左上角差多少」，不碰 offsetLeft：帧条自己没
  // position:relative，芯片的 offsetLeft 会顺着 offsetParent 一路退到上了
  // position 的右栏容器，量出来的是绝对位置——首帧也能算出七百多像素，
  // 被浏览器夹到最右端，一开软件整条蹦到末尾只剩最后四帧在场，用户根本
  // 看不出这儿做了横向滚动。矩形差与定位层级无关，换布局也不漂。
  //   播放中不减速：83ms 一跳还要 smooth，条子永远追不上当前帧。
  useEffect(() => {
    const strip = frameStripRef.current;
    const chip = strip?.children[shownFrame];
    if (!strip || !(chip instanceof HTMLElement)) return;
    const target = stripScrollFor(
      {
        scrollWidth: strip.scrollWidth,
        clientWidth: strip.clientWidth,
        scrollLeft: strip.scrollLeft,
      },
      // 换算成内容坐标：视口差加上当前滚动量，才是格子在内容里的位置。
      chip.getBoundingClientRect().left - strip.getBoundingClientRect().left + strip.scrollLeft,
      chip.offsetWidth,
    );
    if (target === strip.scrollLeft) return;
    strip.scrollTo({ left: target, behavior: playing ? "auto" : "smooth" });
  }, [shownFrame, frames.length, playing]);

  /** 帧条按「一页」翻：一页就是一条条子的宽，翻过去看得见整屏新帧。 */
  function pageFrameStrip(dir: -1 | 1) {
    const strip = frameStripRef.current;
    if (!strip) return;
    strip.scrollBy({ left: dir * strip.clientWidth, behavior: "smooth" });
  }
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

  /** 色块上的右键：换个墨色、或者把这个色从范围里去掉。内置的一个都不动。 */
  function openSwatchMenu(event: ReactMouseEvent, hex: string, index: number) {
    const locked = !scope || scope.builtin || scope.colors.length <= 1;
    openContextMenu(event, [
      {
        key: "use",
        label: t("palette.menu_use", { name: colorName(hex, lang) }),
        checked: active.color === hex,
        onSelect: () => pickColor(hex),
      },
      {
        key: "remove",
        label: t("palette.remove_color"),
        danger: true,
        disabled: locked,
        onSelect: () => removeScopeColor(index),
      },
    ]);
  }

  /** 删整套时的接盘候选：除自己以外的所有范围，内置的排在前面当默认。 */
  function paletteFallbackChoices(selfId: string): PaletteChoice[] {
    const items: NamedPalette[] = document?.palettes ?? [];
    const sorted = [
      ...items.filter((item) => item.builtin && item.id !== selfId),
      ...items.filter((item) => !item.builtin && item.id !== selfId),
    ];
    return sorted.map((item) => ({
      id: item.id,
      name: item.name,
      dots: item.colors.slice(0, 4).map(rgbaToHex),
    }));
  }

  /** 换范围：换的是这一层的边界，已有像素会按就近色重排。先问再动。 */
  function pickScope(paletteId: string) {
    if (!scopeLayer || paletteId === "" || paletteId === scopeLayer.palette_id) return;
    const from = document?.palettes.find((item) => item.id === scopeLayer.palette_id);
    const to = document?.palettes.find((item) => item.id === paletteId);
    if (!from || !to) return;
    setPaletteOp({
      kind: "switch",
      layerId: scopeLayer.id,
      layerName: scopeLayer.name,
      fromName: from.name,
      toId: to.id,
      toName: to.name,
    });
  }

  /**
   * 抄一份内置预设：内置的一个色都动不得，可「复制一份再改」原来只藏在
   * 「往里加色时顺带 fork」里，用户想改个名字、删个色都没处下手。
   * 走 create_palette + from：副本带上当前层的引用，像素按就近色归队。
   */
  async function copyScope() {
    if (!scope || !scopeLayer) return;
    const name = `${scope.name}${copySuffix}`;
    // 一条命令跑完：文档是异步事件推回来的，拆成 create + rename 两条，
    // 第二条要先回读才知道副本 id 是谁，那一下回读可能还没到。
    const revision = await useStore.getState().runEditorOps([
      { op: "create_palette", name, from: scope.id, layer: scopeLayer.id },
    ]);
    if (revision === null) return;
    useStore.getState().noteKey("palette.copied", { name });
  }

  /** 往范围里加色。内置预设改不得：复制一份副本再往里加，副本顺带接到这一层上。 */
  async function addScopeColor(hex: string) {
    const layer = scopeLayer;
    if (!layer || hex === "") return;
    // 重名色要说出来：不管的话用户点一下没反应，会以为色块坏了。
    if (scopeHexes.some((item) => item.toLowerCase() === hex.toLowerCase())) {
      useStore.getState().warnKey("palette.duplicated");
      return;
    }
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
    useStore.getState().noteKey("palette.color_added", { color: colorName(hex, lang) });
  }

  /**
   * 取色盘的落库口子。滑块松手和关弹层收尾都走这儿。
   *
   * antd 的 `onChangeComplete` 只由滑块松手触发，点预设色块、在面板里手输
   * hex 都只发 `onChange`——不收这一刀，那两条路全是「点了没反应」。
   */
  function commitSwatch(hex: string) {
    swatchDraftRef.current = null;
    swatchDoneRef.current = hex;
    setSwatchDraft(null);
    void addScopeColor(hex);
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

  /**
   * 从范围里去掉一个颜色。颜色是按索引记在像素上的，删一格等于后面全往前挪一
   * 格，所以画面上用到它的像素必须有个接手色。少了这一问，静默毁画面。
   */
  function removeScopeColor(index: number) {
    if (!scope || scope.builtin || scope.colors.length <= 1) return;
    const hex = scopeHexes[index];
    if (hex === undefined) return;
    // 接手色只能是范围里剩下的：给个表外的颜色，等于偷偷越狱改配色表。
    const choices = scopeHexes.filter((_item, position) => position !== index);
    if (choices.length === 0) {
      useStore.getState().warnKey("palette.one_left");
      return;
    }
    setPaletteOp({
      kind: "remove",
      paletteId: scope.id,
      paletteName: scope.name,
      index,
      hex,
      choices,
    });
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

  /**
   * 删整套。有图层在用不再拦死：那是死循环（想删得先切走，切走要先删）。
   * 改成让用户指定一套接盘范围，引用的层先改指过去，再删。
   */
  function deleteScope() {
    if (!scope || scope.builtin) return;
    const choices = paletteFallbackChoices(scope.id);
    if (choices.length === 0) {
      // 整个文档只有这一套，删了就没人可指。只能靠「复制一份」破局。
      useStore.getState().warnKey("palette.delete_last");
      return;
    }
    setPaletteOp({
      kind: "delete",
      paletteId: scope.id,
      paletteName: scope.name,
      choices,
    });
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
    // 工具全列出来，不只三个：右键菜单是「这条画布现在能怎么画」的唯一入口，
    // 少一项用户就得回工具条找。勾在哪一项，就是现在手里拿的那个工具。
    const pick = (value: EditorTool, label: string, icon: ReactNode): ContextMenuItem => ({
      key: `tool-${value}`,
      label,
      icon,
      checked: tool === value,
      onSelect: () => setTool(value),
    });
    openContextMenu(event, [
      pick("brush", t("doc.brush"), <Brush size={13} />),
      pick("line", t("doc.shape_line"), <Minus size={13} />),
      pick("rect", t("doc.shape_rect"), <Square size={13} />),
      pick("ellipse", t("doc.shape_ellipse"), <Circle size={13} />),
      pick("triangle", t("doc.shape_triangle"), <Triangle size={13} />),
      pick("smooth", t("doc.shape_smooth"), <PenTool size={13} />),
      pick("fill", t("doc.fill"), <PaintBucket size={13} />),
      pick("eraser", t("doc.eraser"), <Eraser size={13} />),
      // 实心是形状的独立开关，跟着形状走：手拿画笔时它没意义。
      {
        key: "solid",
        label: t("doc.solid"),
        icon: <Droplet size={13} />,
        checked: solid,
        disabled: !canSolid(tool),
        onSelect: () => setSolid((value) => !value),
      },
      {
        key: "undo",
        label: t("menu.undo"),
        icon: <Undo2 size={13} />,
        disabled: undoDepth === 0,
        onSelect: () => void useStore.getState().undoEdit(),
      },
      {
        key: "redo",
        label: t("menu.redo"),
        icon: <Redo2 size={13} />,
        disabled: redoDepth === 0,
        onSelect: () => void useStore.getState().redoEdit(),
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
        {/* 第一行只放「工具」这一件事：八枚图标一个轴，永远不折行。
            早期这里把「笔/填充/橡皮」和「自由/线/矩形…」拆成两组，结果
            「画笔」和「自由」是同一个功能，「填充」在两处含义还不一样——
            用户看到的就是「三个画笔没区别、填充时好时坏」。现在归一：
            八个工具各占一格，实心和圆角是它们共用的开关。 */}
        <div className="canvas-tools core">
          <Segmented
            size="small"
            value={tool}
            onChange={(value) => setTool(value as EditorTool)}
            options={[
              toolOption("brush", t("doc.tool_brush"), <Brush size={14} />),
              toolOption("line", t("doc.shape_line"), <Minus size={14} />),
              toolOption("rect", t("doc.shape_rect"), <Square size={14} />),
              toolOption("ellipse", t("doc.shape_ellipse"), <Circle size={14} />),
              toolOption("triangle", t("doc.shape_triangle"), <Triangle size={14} />),
              toolOption("smooth", t("doc.shape_smooth"), <PenTool size={14} />),
              toolOption("fill", t("doc.tool_fill"), <PaintBucket size={14} />),
              toolOption("eraser", t("doc.tool_eraser"), <Eraser size={14} />),
            ]}
          />
          {/* 形态选项自成一组：实心开关 + 圆角档。右栏定宽 330px，八枚图标
              已经贴边，这组整块折到工具下一行（见 .tool-mods 的注），内部
              仍是一行——绝不并排挤压，一挤按钮就成细条，横向还滚出栏外。 */}
          <div className="tool-mods">
            {/* 实心开关：形状落下来是一块还是只描边。只对形状有意义，
                手拿画笔/橡皮/油漆桶时它置灰，不会让人以为点了没反应。 */}
            <Tooltip title={solid ? t("doc.solid_on") : t("doc.solid_off")}>
              <Button
                size="small"
                type="text"
                aria-label={solid ? t("doc.solid_on") : t("doc.solid_off")}
                icon={<Droplet size={14} />}
                className={solid ? "tool-on" : ""}
                disabled={!canSolid(tool)}
                onClick={() => setSolid((value) => !value)}
              />
            </Tooltip>
            {/* 圆角档：跟在形状后面，只在该圆角有意义的形状上出现。
                用户提过「基础图形太突兀，画细节要平滑曲线」——矩形和三角的尖角
                是突兀感的来源，这里把它们让成圆弧；椭圆和直线不给这个档。 */}
            {canCorner(tool) && (
              <Tooltip title={t("doc.corner_tip")}>
                <Segmented
                  size="small"
                  value={corner}
                  onChange={(value) => setCorner(value as number)}
                  options={[
                    { value: 0, label: <span className="tile-label">{t("doc.corner_sharp")}</span> },
                    { value: 0.2, label: <span className="tile-label">{t("doc.corner_small")}</span> },
                    { value: 0.35, label: <span className="tile-label">{t("doc.corner_mid")}</span> },
                    { value: 0.5, label: <span className="tile-label">{t("doc.corner_big")}</span> },
                  ]}
                />
              </Tooltip>
            )}
          </div>
        </div>
        {/* 第二行是「看着办」的档位：瓦片、播放、洋葱皮、撤销重做。它们
            不参与落笔，所以窄一点没关系，放不下了自然折到下一行。 */}
        <div className="canvas-tools">
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
              aria-label={playing ? t("menu.pause") : t("doc.play")}
              icon={playing ? <Pause size={14} /> : <Play size={14} />}
              disabled={!canPlay}
              onClick={togglePlay}
            />
          </Tooltip>
          <Tooltip title={t("doc.onion")}>
            <Button
              size="small"
              type="text"
              aria-label={t("doc.onion")}
              icon={<Ghost size={14} />}
              className={onion ? "tool-on" : ""}
              disabled={!canPlay}
              onClick={() => setOnion((value) => !value)}
            />
          </Tooltip>
          {/* 纸娃娃白膜：按 RPG Maker 角色表的固定网格，在当前图层每一帧上
              铺一版按部件分区的人形剪影。底稿是可撤销的编辑动作，所以不设
              确认弹窗；尺寸不是 4 行网格时按钮置灰，一句话说清要什么样的画布。 */}
          <Tooltip
            title={
              document && isSheetGrid(document.width, document.height)
                ? t("doc.paperdoll_tip")
                : t("doc.paperdoll_need_sheet")
            }
          >
            <Button
              size="small"
              type="text"
              aria-label={t("doc.paperdoll")}
              icon={<User size={14} />}
              disabled={!document || !isSheetGrid(document.width, document.height)}
              onClick={() => void useStore.getState().layPaperdollBase()}
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
              aria-label={
                undoDepth > 0 ? t("doc.undo", { count: undoDepth }) : t("doc.undo_none")
              }
              icon={<Undo2 size={14} />}
              disabled={undoDepth === 0}
              onClick={() => void useStore.getState().undoEdit()}
            />
          </Tooltip>
          <Tooltip
            title={
              redoDepth > 0 ? t("doc.redo", { count: redoDepth }) : t("doc.redo_none")
            }
          >
            <Button
              size="small"
              type="text"
              aria-label={
                redoDepth > 0 ? t("doc.redo", { count: redoDepth }) : t("doc.redo_none")
              }
              icon={<Redo2 size={14} />}
              disabled={redoDepth === 0}
              onClick={() => void useStore.getState().redoEdit()}
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
                      // 权威 PNG 属于哪一帧和现在停在那一帧对不上时先请下去：
                      // 帧层已经同步画好了新帧，旧图还挂在这儿就是两张脸叠着。
                      // 换帧、撤销、AI 落笔之后的往返窗口里全靠这一手兜住。
                      data-stale={pngFrame !== frameIndex ? "true" : undefined}
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
              aria-label={t("doc.new_layer")}
              icon={<Plus size={13} />}
              disabled={!document}
              onClick={() => void useStore.getState().addLayer()}
            />
          </Tooltip>
          <Tooltip title={t("doc.delete_layer")}>
            <Button
              size="small"
              type="text"
              aria-label={t("doc.delete_layer")}
              icon={<Trash2 size={13} />}
              disabled={layerCount <= 1}
              onClick={() => void useStore.getState().deleteLayer(active.layer)}
            />
          </Tooltip>
          <Tooltip title={t("doc.layer_up")}>
            <Button
              size="small"
              type="text"
              aria-label={t("doc.layer_up")}
              icon={<ArrowUp size={13} />}
              disabled={topLayer}
              onClick={() => void useStore.getState().moveLayer(1)}
            />
          </Tooltip>
          <Tooltip title={t("doc.layer_down")}>
            <Button
              size="small"
              type="text"
              aria-label={t("doc.layer_down")}
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
              aria-label={t("doc.new_frame")}
              icon={<Plus size={13} />}
              onClick={() => void useStore.getState().addFrame()}
            />
          </Tooltip>
          <Tooltip title={t("doc.duplicate_frame")}>
            <Button
              size="small"
              type="text"
              aria-label={t("doc.duplicate_frame")}
              icon={<Copy size={13} />}
              onClick={() => void useStore.getState().duplicateFrame()}
            />
          </Tooltip>
          <Tooltip title={t("doc.delete_frame")}>
            <Button
              size="small"
              type="text"
              aria-label={t("doc.delete_frame")}
              icon={<Trash2 size={13} />}
              disabled={frames.length <= 1}
              onClick={() => void useStore.getState().deleteFrame()}
            />
          </Tooltip>
          <Tooltip title={t("doc.move_earlier")}>
            <Button
              size="small"
              type="text"
              aria-label={t("doc.move_earlier")}
              icon={<ArrowLeft size={13} />}
              disabled={firstFrame}
              onClick={() => void useStore.getState().moveFrame(-1)}
            />
          </Tooltip>
          <Tooltip title={t("doc.move_later")}>
            <Button
              size="small"
              type="text"
              aria-label={t("doc.move_later")}
              icon={<ArrowRight size={13} />}
              disabled={lastFrame}
              onClick={() => void useStore.getState().moveFrame(1)}
            />
          </Tooltip>
            {/* 帧条横滚的明示：滚轮和触控板之外，还得有两个能点的箭头。
                用双尖括号而不是单箭头：左边那对 ← → 挪的是「这一帧的位置」，
                这对翻的是「这一条还能往哪看」，长得一样用户必然点错。 */}
            {/* 这两个箭头常驻、到边变灰：帧条一溢出不溢出的那一刻按钮忽隐忽现，
                用户根本记不住东西长在哪儿；要的就是「这一行右边还有东西」这件事
                在还没滚之前就看得见。 */}
            <Tooltip title={t("doc.strip_scroll_left")}>
              <Button
                size="small"
                type="text"
                aria-label={t("doc.strip_scroll_left")}
                icon={<ChevronsLeft size={13} />}
                disabled={!stripScroll.canLeft}
                onClick={() => pageFrameStrip(-1)}
              />
            </Tooltip>
            <Tooltip title={t("doc.strip_scroll_right")}>
              <Button
                size="small"
                type="text"
                aria-label={t("doc.strip_scroll_right")}
                icon={<ChevronsRight size={13} />}
                disabled={!stripScroll.canRight}
                onClick={() => pageFrameStrip(1)}
              />
            </Tooltip>
          </div>
          <HStrip
            scrollerRef={frameStripRef}
            className="frame-strip"
            label={t("doc.frames")}
            onEdges={onFrameEdges}
          >
            {frames.map((frame, index) => (
              <Tooltip
                key={frame.id}
                title={t("doc.frame_chip", {
                  index: index + 1,
                  ms: frame.duration_ms,
                  id: frame.id,
                })}
              >
                <button
                  type="button"
                  className={`frame-chip ${index === shownFrame ? "active" : ""}`}
                  onClick={() => jumpToFrame(index)}
                  onContextMenu={(event) => openFrameMenu(event, index)}
                >
                  {document ? <FrameThumb document={document} index={index} /> : null}
                  <span className="frame-chip-id">{index + 1}</span>
                  <span className="frame-chip-ms">{frame.duration_ms}ms</span>
                </button>
              </Tooltip>
            ))}
          </HStrip>
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
                aria-label={t("palette.new_hint")}
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
            {/* 右边这几颗只做「这一套本身能怎么摆弄」：自建的能改名能删，
                内置的一律改成「复制一份」。原来这里摆的是一枚「内置」小牌，
                跟收起的选择器里那个标签说的是同一句话，白占一行宽度；
                换成复制键，tooltip 里承诺的「点一下就复制一份再改」才算有处落地。 */}
            {scope?.builtin ? (
              <Tooltip title={t("palette.scope_hint")}>
                <Button
                  size="small"
                  type="text"
                  aria-label={t("palette.copy")}
                  icon={<Copy size={13} />}
                  onClick={() => void copyScope()}
                />
              </Tooltip>
            ) : scope ? (
              <>
                <Tooltip title={t("palette.rename")}>
                  <Button
                    size="small"
                    type="text"
                    aria-label={t("palette.rename")}
                    icon={<Pencil size={13} />}
                    onClick={() => openScopeEdit("rename")}
                  />
                </Tooltip>
                <Tooltip title={t("palette.delete")}>
                  <Button
                    size="small"
                    type="text"
                    aria-label={t("palette.delete")}
                    danger
                    icon={<Trash2 size={13} />}
                    onClick={deleteScope}
                  />
                </Tooltip>
              </>
            ) : null}
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
                aria-label={t("doc.transparent")}
                className={`swatch eraser ${active.color === null ? "active" : ""}`}
                onClick={() => pickColor(null)}
              />
            </Tooltip>
            {scopeHexes.map((hex, index) => (
              <span
                className="swatch-cell"
                key={`${hex}-${index}`}
                // 右键出菜单：改墨色、把这个色从范围里去掉。挂在格子上而不是色块上，
                // 连带那个小叉一起接管，整个格子右击都是这一套。
                onContextMenu={(event) => openSwatchMenu(event, hex, index)}
              >
                <Tooltip title={swatchTip(hex, index)}>
                  <button
                    type="button"
                    // 色块本身没有文字，读屏里报的是「未标记按钮」；把悬停那句
                    // （色名 + 序号）同时给它当名字，两边一句话对齐。
                    aria-label={swatchTip(hex, index)}
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
                      <X size={7} />
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
                  onChange={(value) => {
                    // 拖动途中只更新草稿：一路加色的话，撤销栈会糊成一锅粥。
                    swatchDraftRef.current = value.toHexString();
                    setSwatchDraft(value.toHexString());
                  }}
                  onChangeComplete={(value) => commitSwatch(value.toHexString())}
                  onClear={() => {
                    swatchDraftRef.current = null;
                    setSwatchDraft(null);
                  }}
                  onOpenChange={(open) => {
                    if (open) {
                      // 新开一轮：草稿和「这一轮落过谁」都清零，不然上一轮
                      // 的色会在这一轮被误判成重复而跳过。
                      swatchDraftRef.current = null;
                      swatchDoneRef.current = null;
                      setSwatchDraft(null);
                      return;
                    }
                    // 收尾：点预设、面板里手输都只发 onChange，不在这里落库
                    // 就白挑了。同一轮里落过的色不重复落第二次。
                    const draft = swatchDraftRef.current;
                    swatchDraftRef.current = null;
                    setSwatchDraft(null);
                    const commit = swatchCommitOnClose(draft, swatchDoneRef.current);
                    if (commit !== null) commitSwatch(commit);
                  }}
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
        sheetPresetsLabel={t("sidebar.sheet_presets")}
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
      <PaletteOpsModal
        request={paletteOp}
        onClose={() => setPaletteOp(null)}
        onSwitch={(layerId, toId) => {
          setPaletteOp(null);
          void useStore.getState().setLayerPalette(layerId, toId);
        }}
        onRemove={(request, replacement) => {
          // 先把要动的表和格子号取出来再关窗：关窗之后问的就是下一件事了。
          const { paletteId, index } = request;
          setPaletteOp(null);
          void useStore.getState().removePaletteColor(paletteId, index, replacement);
        }}
        onDelete={(request, fallbackId) => {
          const { paletteId } = request;
          setPaletteOp(null);
          void useStore.getState().deletePalette(paletteId, fallbackId);
        }}
      />
    </aside>
  );
}
