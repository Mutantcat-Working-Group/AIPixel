import { useEffect, useRef, useState } from "react";
import { Button, Tooltip } from "antd";
import { Eye, EyeOff, FileCode2, Layers } from "lucide-react";

import { useStore } from "../lib/store";
import type { Rgba } from "../lib/types";

const MAX_PREVIEW_HEIGHT = 240;

function cssColor(color: Rgba): string {
  return `rgba(${color.r}, ${color.g}, ${color.b}, ${(color.a / 255).toFixed(3)})`;
}

/** 整数倍放大：像素画不能被插值糊掉，倍率向下取整，格子才始终是方的。 */
function integerScale(width: number, height: number, budget: number): number {
  return Math.max(1, Math.floor(Math.min(budget / width, MAX_PREVIEW_HEIGHT / height)));
}

export default function DocumentPanel() {
  const document = useStore((s) => s.document);
  const pngUrl = useStore((s) => s.pngUrl);
  const active = useStore((s) => s.active);
  const frameIndex = useStore((s) => s.frameIndex);
  const revision = useStore((s) => s.revision);
  const [aipText, setAipText] = useState<string | null>(null);
  const [aipOpen, setAipOpen] = useState(false);
  const [budget, setBudget] = useState(300);
  const checkerRef = useRef<HTMLDivElement>(null);

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

  async function toggleAipText() {
    if (aipOpen) {
      setAipOpen(false);
      return;
    }
    const text = await useStore.getState().readAipText();
    setAipText(text);
    setAipOpen(true);
  }

  const scale = document
    ? integerScale(document.width, document.height, budget)
    : 1;

  return (
    <aside className="panel doc">
      <div className="doc-preview">
        <div className="checker" ref={checkerRef}>
          {pngUrl && document ? (
            <img
              src={pngUrl}
              alt="canvas preview"
              width={document.width * scale}
              height={document.height * scale}
            />
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
          <div className="doc-section-title">Frames</div>
          <div className="frame-strip">
            {document?.frames.map((frame, index) => (
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
            <Tooltip title="Eraser (index 0, transparent)">
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
