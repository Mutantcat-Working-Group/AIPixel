import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Button, Segmented, Tooltip } from "antd";
import {
  ArrowLeft,
  ArrowRight,
  Brush,
  Copy,
  Eye,
  EyeOff,
  FileCode2,
  Layers,
  PaintBucket,
  Plus,
  Trash2,
  Undo2,
} from "lucide-react";

import { useStore } from "../lib/store";
import { appendStroke, lineCells } from "../lib/stroke";
import type {
  EditorTool,
  InkColor,
  PixelDocument,
  Rgba,
  StrokeCell,
} from "../lib/types";

const MAX_PREVIEW_HEIGHT = 240;

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
  const checkerRef = useRef<HTMLDivElement>(null);
  // 一笔笔画的临时状态全在 ref 里：pointermove 不该触发 React 渲染。
  const strokeRef = useRef<StrokeCell[]>([]);
  const lastCellRef = useRef<StrokeCell | null>(null);
  const paintingRef = useRef(false);
  const paintLayerRef = useRef<HTMLCanvasElement>(null);

  const scale = document
    ? integerScale(document.width, document.height, budget)
    : 1;

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
  // active.color 可空：归一成 ink，画笔与预览都吃这一份。
  const ink: InkColor = active.color ?? null;

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
                    <Brush size={13} /> Brush
                  </span>
                ),
              },
              {
                value: "fill",
                label: (
                  <span className="tool-label">
                    <PaintBucket size={13} /> Fill
                  </span>
                ),
              },
            ]}
          />
          <span className="grow" />
          <Tooltip title={undoDepth > 0 ? `Undo the last edit (${undoDepth} left)` : "Nothing to undo"}>
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
              style={{ width: document.width * scale, height: document.height * scale }}
            >
              <img
                src={pngUrl}
                alt="canvas preview"
                width={document.width * scale}
                height={document.height * scale}
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
          <span>{document?.name ?? "empty"}</span>
          <span>
            {document ? `${document.width}x${document.height}` : "--"}
          </span>
          <span>rev {revision}</span>
        </div>
      </div>

      <div className="panel-body">
        <div className="doc-section">
          <div className="doc-section-title">
            <Layers size={12} />
            Layers
          </div>
          {document?.layers.map((layer) => (
            <button
              type="button"
              key={layer.id}
              className={`layer-row ${active.layer === layer.id ? "active" : ""}`}
              onClick={() => useStore.getState().setActiveLayer(layer.id)}
            >
              {layer.visible ? <Eye size={13} /> : <EyeOff size={13} />}
              <span className="layer-id">{layer.id}</span>
              <span className="grow">{layer.name}</span>
            </button>
          ))}
        </div>

        <div className="doc-section">
          <div className="doc-section-title">
            Frames
            <span className="grow" />
            <Tooltip title="New frame after this one">
              <Button
                size="small"
                type="text"
                icon={<Plus size={13} />}
                onClick={() => void useStore.getState().addFrame()}
              />
            </Tooltip>
            <Tooltip title="Duplicate this frame">
              <Button
                size="small"
                type="text"
                icon={<Copy size={13} />}
                onClick={() => void useStore.getState().duplicateFrame()}
              />
            </Tooltip>
            <Tooltip title="Delete this frame">
              <Button
                size="small"
                type="text"
                icon={<Trash2 size={13} />}
                disabled={frames.length <= 1}
                onClick={() => void useStore.getState().deleteFrame()}
              />
            </Tooltip>
            <Tooltip title="Move this frame earlier">
              <Button
                size="small"
                type="text"
                icon={<ArrowLeft size={13} />}
                disabled={firstFrame}
                onClick={() => void useStore.getState().moveFrame(-1)}
              />
            </Tooltip>
            <Tooltip title="Move this frame later">
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
                className={`frame-chip ${index === frameIndex ? "active" : ""}`}
                onClick={() => useStore.getState().setActiveFrame(index)}
              >
                {frame.id} · {frame.duration_ms}ms
              </button>
            ))}
          </div>
        </div>

        <div className="doc-section">
          <div className="doc-section-title">Palette</div>
          <div className="palette-grid">
            <Tooltip title="Transparent (index 0). With the brush it erases.">
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
                <Tooltip key={`${hex}-${index}`} title={`${hex} · index ${index + 1}`}>
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
          <Tooltip title="Inspect the .aip v2 text for this document">
            <Button size="small" type="text" icon={<FileCode2 size={13} />} onClick={toggleAipText}>
              {aipOpen ? "Hide .aip" : "Show .aip"}
            </Button>
          </Tooltip>
          {aipOpen && aipText !== null ? <pre className="aip-text">{aipText}</pre> : null}
        </div>
      </div>
    </aside>
  );
}
