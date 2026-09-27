import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Button, InputNumber, Segmented, Slider, Tooltip } from "antd";
import {
  ArrowLeft,
  ArrowDown,
  ArrowUp,
  ArrowRight,
  Brush,
  Copy,
  Eye,
  EyeOff,
  FileCode2,
  Download,
  FileImage,
  Ghost,
  Grid2x2,
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
import { documentExport } from "../lib/bridge";
import { save } from "@tauri-apps/plugin-dialog";
import type {
  EditorTool,
  InkColor,
  PixelDocument,
  Rgba,
  StrokeCell,
} from "../lib/types";

const MAX_PREVIEW_HEIGHT = 240;
/** 洋葱皮的浓度：看得见上一帧的轮廓，但抢不走当前帧的注意力。 */
const ONION_ALPHA = 0.24;

function cssColor(color: Rgba): string {
  return `rgba(${color.r}, ${color.g}, ${color.b}, ${(color.a / 255).toFixed(3)})`;
}

/** 整数倍放大：像素画不能被插值糊掉，倍率向下取整，格子才始终是方的。 */
function integerScale(width: number, height: number, budget: number): number {
  return Math.max(1, Math.floor(Math.min(budget / width, MAX_PREVIEW_HEIGHT / height)));
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
  const [exporting, setExporting] = useState<"gif" | "sheet" | null>(null);
  const checkerRef = useRef<HTMLDivElement>(null);
  // 一笔笔画的临时状态全在 ref 里：pointermove 不该触发 React 渲染。
  const strokeRef = useRef<StrokeCell[]>([]);
  const lastCellRef = useRef<StrokeCell | null>(null);
  const paintingRef = useRef(false);
  const paintLayerRef = useRef<HTMLCanvasElement>(null);
  const frameLayerRef = useRef<HTMLCanvasElement>(null);
  // 洋葱皮的上一帧先落在离屏画布上，再压低透明度贴到底层，
  // 因为 putImageData 不吃 globalAlpha。
  const ghostRef = useRef<HTMLCanvasElement | null>(null);

  const scale = document
    ? integerScale(document.width, document.height, budget)
    : 1;

  /**
   * 把某一帧（可选带上一帧的幽灵）画到帧画布上。画布尺寸就是文档尺寸，
   * 放大交给 CSS 和 image-rendering: pixelated，和 <img> 走同一条放大路径。
   */
  function paintFrame(index: number, ghostOf: number | null) {
    const canvas = frameLayerRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx || !document) return;
    const { width, height } = document;
    ctx.clearRect(0, 0, width, height);
    ctx.imageSmoothingEnabled = false;
    if (ghostOf !== null && ghostOf >= 0 && ghostOf !== index) {
      // window.document：组件里的 document 是像素文档，把 DOM 的那个遮蔽掉了。
      const ghost = ghostRef.current ?? window.document.createElement("canvas");
      ghostRef.current = ghost;
      ghost.width = width;
      ghost.height = height;
      const ghostCtx = ghost.getContext("2d");
      if (ghostCtx) {
        ghostCtx.putImageData(new ImageData(compositeFrame(document, ghostOf), width, height), 0, 0);
        ctx.globalAlpha = ONION_ALPHA;
        ctx.drawImage(ghost, 0, 0);
        ctx.globalAlpha = 1;
      }
    }
    ctx.putImageData(new ImageData(compositeFrame(document, index), width, height), 0, 0);
  }

  function clearFrameLayer() {
    const canvas = frameLayerRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
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
    const canvas = paintLayerRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    if (cells.length === 0) return;
    // 擦除也画出痕迹：把格子压暗一块，用户才知道正在擦哪儿。
    ctx.fillStyle = color ?? "rgba(8, 10, 14, 0.4)";
    for (const cell of cells) ctx.fillRect(cell.x * scale, cell.y * scale, scale, scale);
  }

  function flushStroke() {
    if (!paintingRef.current) return;
    paintingRef.current = false;
    const cells = strokeRef.current;
    strokeRef.current = [];
    lastCellRef.current = null;
    drawStroke([], ink);
    void useStore.getState().paintStroke(cells);
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
      void useStore.getState().fillCell(cell.x, cell.y);
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

  /**
   * 导出动画到磁盘。位置先问用户，编码全在 Rust 侧：
   * 合成的原料是调色板索引而不是位图像素，所以结果和画布看到的是同一份东西。
   */
  async function exportAs(format: "gif" | "sheet") {
    const id = useStore.getState().activeId;
    if (!id || !document) return;
    const name = document.name || "untitled";
    const target = await save(
      format === "gif"
        ? {
            defaultPath: `${name}.gif`,
            filters: [{ name: t("doc.export_gif"), extensions: ["gif"] }],
          }
        : {
            defaultPath: `${name}-sheet.png`,
            filters: [{ name: t("doc.export_sheet"), extensions: ["png"] }],
          }
    );
    if (typeof target !== "string") return;
    setExporting(format);
    try {
      await documentExport(id, format, target);
    } finally {
      setExporting(null);
    }
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
  // active.color 可空：归一成 ink，画笔与预览都吃这一份。
  const ink: InkColor = active.color ?? null;
  const canPlay = frames.length > 1;
  // 播放时高亮跟着本地帧号走，store 的 frameIndex 还停在开播那一帧。
  const shownFrame = playing ? playFrame : frameIndex;
  const currentFrame = frames[frameIndex];

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
            ]}
          />
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
            <div
              className="canvas-stack"
              data-playing={playing ? "true" : undefined}
              style={{ width: document.width * scale, height: document.height * scale }}
            >
              <img
                src={pngUrl}
                className="canvas-img"
                alt={t("doc.canvas_alt")}
                width={document.width * scale}
                height={document.height * scale}
              />
              <canvas
                className="frame-layer"
                ref={frameLayerRef}
                width={document.width}
                height={document.height}
              />
              <canvas
                className="paint-layer"
                ref={paintLayerRef}
                width={document.width * scale}
                height={document.height * scale}
              />
              <div
                className="canvas-overlay"
                onPointerDown={onPointerDown}
                onPointerMove={onPointerMove}
                onPointerUp={flushStroke}
                onPointerCancel={flushStroke}
              />
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
                {frame.id} · {frame.duration_ms}ms
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
          <div className="palette-grid">
            <Tooltip title={t("doc.transparent")}>
              <button
                type="button"
                className={`swatch eraser ${active.color === null ? "active" : ""}`}
                onClick={() => useStore.getState().setActiveColor(null)}
              />
            </Tooltip>
            {document?.palette.map((color, index) => {
              const hex = `#${[color.r, color.g, color.b]
                .map((channel) => channel.toString(16).padStart(2, "0"))
                .join("")}`;
              return (
                <Tooltip
                  key={`${hex}-${index}`}
                  title={t("doc.swatch", { hex, index: index + 1 })}
                >
                  <button
                    type="button"
                    className={`swatch ${active.color === hex ? "active" : ""}`}
                    style={{ background: cssColor(color) }}
                    onClick={() => useStore.getState().setActiveColor(hex)}
                  />
                </Tooltip>
              );
            })}
          </div>
        </div>

        <div className="doc-section">
          <div className="doc-section-title">
            <Download size={12} />
            {t("doc.export")}
          </div>
          <div className="export-row">
            <Button
              size="small"
              icon={<FileImage size={13} />}
              loading={exporting === "gif"}
              disabled={exporting !== null}
              onClick={() => void exportAs("gif")}
            >
              {t("doc.export_gif")}
            </Button>
            <Button
              size="small"
              icon={<Grid2x2 size={13} />}
              loading={exporting === "sheet"}
              disabled={exporting !== null}
              onClick={() => void exportAs("sheet")}
            >
              {t("doc.export_sheet")}
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
    </aside>
  );
}
