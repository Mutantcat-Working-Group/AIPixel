import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Button, ColorPicker, InputNumber, Segmented, Select, Slider, Tooltip } from "antd";
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
  Pause,
  Play,
  Plus,
  Trash2,
  Undo2,
} from "lucide-react";

import { useStore } from "../lib/store";
import { useT } from "../lib/t";
import { appendStroke, lineCells } from "../lib/stroke";
import { compositeFrame } from "../lib/render";
import { PALETTE_PRESETS, hexesOf, paletteWithCustom } from "../lib/palette";
import FrameThumb from "./FrameThumb";
import type {
  EditorTool,
  InkColor,
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
  event: ReactPointerEvent<HTMLDivElement>,
  document: PixelDocument,
): StrokeCell | null {
  const rect = event.currentTarget.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0) return null;
  const x = Math.floor(((event.clientX - rect.left) / rect.width) * document.width);
  const y = Math.floor(((event.clientY - rect.top) / rect.height) * document.height);
  if (x < 0 || y < 0 || x >= document.width || y >= document.height) return null;
  return { x, y };
}

export default function DocumentPanel() {
  const t = useT();
  const document = useStore((s) => s.document);
  const pngUrl = useStore((s) => s.pngUrl);
  const active = useStore((s) => s.active);
  const frameIndex = useStore((s) => s.frameIndex);
  const revision = useStore((s) => s.revision);
  const undoDepth = useStore((s) => s.undoStack.length);
  const [tool, setTool] = useState<EditorTool>("brush");
  const [aipText, setAipText] = useState<string | null>(null);
  const [aipOpen, setAipOpen] = useState(false);
  const [budget, setBudget] = useState(300);
  const [playing, setPlaying] = useState(false);
  const [playFrame, setPlayFrame] = useState(0);
  const [onion, setOnion] = useState(false);
  // 配色范围：null = 还没挑预设，沿用文档当前的调色板。
  const [presetId, setPresetId] = useState<string | null>(null);
  // 「任意颜色」：一个预设最多一个槽位，改它只动这一格。
  const [customColor, setCustomColor] = useState<string | null>(null);
  // 取色器拖动的中间值：即时预览，松手才落文档；不然一次拖动就是一串撤销步。
  const [draftColor, setDraftColor] = useState<string | null>(null);
  // 瓦片底图：1 = 单幅，2 / 3 = 把画面平铺开，看瓦片接缝。
  const [tileMode, setTileMode] = useState<1 | 2 | 3>(1);
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
    // 幽灵只画一次，再贴到每个瓦片：九宫格也不该重算九遍上一帧。
    let ghost: HTMLCanvasElement | null = null;
    if (ghostOf !== null && ghostOf >= 0 && ghostOf !== index) {
      // window.document：组件里的 document 是像素文档，把 DOM 的那个遮蔽掉了。
      ghost = ghostRef.current ?? window.document.createElement("canvas");
      ghostRef.current = ghost;
      ghost.width = width;
      ghost.height = height;
      const ghostCtx = ghost.getContext("2d");
      if (ghostCtx) {
        ghostCtx.putImageData(new ImageData(compositeFrame(document, ghostOf), width, height), 0, 0);
      }
    }
    const frame = new ImageData(compositeFrame(document, index), width, height);
    for (const canvas of tileCanvases("canvas.frame-layer")) {
      const ctx = canvas.getContext("2d");
      if (!ctx) continue;
      ctx.clearRect(0, 0, width, height);
      ctx.imageSmoothingEnabled = false;
      if (ghost) {
        ctx.globalAlpha = ONION_ALPHA;
        ctx.drawImage(ghost, 0, 0);
        ctx.globalAlpha = 1;
      }
      ctx.putImageData(frame, 0, 0);
    }
  }

  function clearFrameLayer() {
    for (const canvas of tileCanvases("canvas.frame-layer")) {
      const ctx = canvas.getContext("2d");
      if (!ctx) continue;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
    }
  }

  // 播放：帧号在前端自己走，不每帧都去 Rust 取一次图。停下来才把帧号同步回
  // store，让帧条、高亮和 png 对齐。时长极端短的帧也兜个底，不然 setTimeout
  // 会被压成一锅粥。
  // playFrame 刻意不进依赖：它每次播放都在变，把它算进去会让计时器不断重置。
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!playing || !document || document.frames.length < 2) return;
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

  // 不播放时洋葱皮自己补上；关掉就把幽灵擦干净，别留一层残影。
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (playing || !document) return;
    if (onion && frameIndex > 0) paintFrame(frameIndex, frameIndex - 1);
    else clearFrameLayer();
  }, [playing, onion, frameIndex, document]);

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

  function onPointerMove(event: ReactPointerEvent<HTMLDivElement>) {
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
  // 落笔色统一成 ink：橡皮恒为透明，画笔跟随调色板选择。
  const ink: InkColor = tool === "eraser" ? null : (active.color ?? null);
  const canPlay = frames.length > 1;
  // 播放时高亮跟着本地帧号走，store 的 frameIndex 还停在开播那一帧。
  const shownFrame = playing ? playFrame : frameIndex;
  const currentFrame = frames[frameIndex];
  // 调色板即配色范围：挑过预设就用预设，没挑就沿用文档当前的调色板。
  const paletteHexes = document ? hexesOf(document) : [];
  const preset = PALETTE_PRESETS.find((item) => item.id === presetId) ?? null;
  const paletteBase = preset ? preset.colors : paletteHexes;

  /**
   * 换配色范围：预设 + 最多一个任意颜色整幅下发，Rust 按就近色重映射已有像素。
   * 挑预设时保留任意颜色槽——它跟着人在走，不属于哪一套预设。
   */
  async function applyPalette(nextPresetId: string | null, nextCustom: string | null) {
    const base = nextPresetId
      ? (PALETTE_PRESETS.find((item) => item.id === nextPresetId)?.colors ?? paletteBase)
      : paletteBase;
    const colors = paletteWithCustom(base, nextCustom);
    // 范围和文档现成的一致就别发车：空跑一趟 set_palette 只会白赚一步撤销。
    const unchanged =
      colors.length === paletteHexes.length &&
      colors.every((hex, index) => hex.toLowerCase() === paletteHexes[index]?.toLowerCase());
    if (!unchanged && !(await useStore.getState().setPaletteColors(colors))) return;
    // 没落成就别把选择记下来：界面说是新范围、文档还是旧范围，两下会打架。
    setPresetId(nextPresetId);
    setCustomColor(nextCustom);
    setDraftColor(null);
  }

  /** 选色即回画笔：刚挑的颜色总得有个工具把它落下去。 */
  function pickColor(color: InkColor) {
    if (tool === "eraser") setTool("brush");
    useStore.getState().setActiveColor(color);
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
          <span>
            {document ? `${document.width}x${document.height}` : "--"}
          </span>
          <span>{t("doc.rev", { rev: revision })}</span>
        </div>
      </div>

      <div className="panel-body">
        <div className="doc-section">
          <div className="doc-section-title">
            <Layers size={12} />
            {t("doc.layers")}
            <span className="grow" />
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
              <button
                type="button"
                className="layer-name"
                onClick={() => useStore.getState().setActiveLayer(layer.id)}
              >
                <span className="layer-id">{layer.id}</span>
                <span className="grow">{layer.name}</span>
              </button>
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
          <div className="doc-section-title">{t("doc.palette")}</div>
          {/* 配色范围：换预设就是换「允许用哪些颜色」，已有像素按就近色搬过去。 */}
          <Select
            size="small"
            className="palette-preset"
            value={presetId ?? undefined}
            allowClear
            placeholder={t("doc.palette_current")}
            options={PALETTE_PRESETS.map((item) => ({
              value: item.id,
              label: `${item.name} · ${item.colors.length}`,
            }))}
            onChange={(value) => void applyPalette(value ?? null, customColor)}
          />
          <div className="palette-grid">
            <Tooltip title={t("doc.transparent")}>
              <button
                type="button"
                className={`swatch eraser ${active.color === null ? "active" : ""}`}
                onClick={() => pickColor(null)}
              />
            </Tooltip>
            {paletteBase.map((hex, index) => {
              return (
                <Tooltip
                  key={`${hex}-${index}`}
                  title={t("doc.swatch", { hex, index: index + 1 })}
                >
                  <button
                    type="button"
                    className={`swatch ${active.color === hex ? "active" : ""}`}
                    style={{ background: hex }}
                    onClick={() => pickColor(hex)}
                  />
                </Tooltip>
              );
            })}
            {/* 「任意颜色」：一个预设只带得动一个，拖完松手才落文档，拖动中途不记撤销。 */}
            <Tooltip
              title={
                <>
                  <div>{t("doc.palette_custom")}</div>
                  <div>{t("doc.palette_custom_tip")}</div>
                </>
              }
            >
              <div
                className={`swatch-slot ${
                  customColor !== null && active.color === customColor ? "active" : ""
                }`}
              >
                <ColorPicker
                  format="hex"
                  disabledAlpha
                  allowClear
                  showText={false}
                  value={draftColor ?? customColor ?? undefined}
                  onChange={(value) => setDraftColor(value.toHexString())}
                  onChangeComplete={(value) => {
                    const hex = value.toHexString();
                    // 挑完即用：新颜色不当当前墨，这一下就白挑了。
                    pickColor(hex);
                    void applyPalette(presetId, hex);
                  }}
                  onClear={() => void applyPalette(presetId, null)}
                />
              </div>
            </Tooltip>
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
    </aside>
  );
}
