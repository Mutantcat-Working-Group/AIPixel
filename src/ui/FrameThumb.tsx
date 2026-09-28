import { useEffect, useRef } from "react";

import { compositeFrame } from "../lib/render";
import type { PixelDocument } from "../lib/types";

// 缩略图盒子的上限：再大的文档也在这个盒子里等比缩小，帧条不会被某一帧撑爆。
const THUMB_MAX_WIDTH = 46;
const THUMB_MAX_HEIGHT = 34;

/** 缩略图盒子的另一档尺寸：窄容器（比如工作流坞）里用，格子才不会被内容顶宽。 */
const THUMB_COMPACT_WIDTH = 30;
const THUMB_COMPACT_HEIGHT = 22;

/**
 * 帧条里的一格缩略图。画在文档尺寸的画布上，缩放交给 CSS 的 pixelated：
 * 文档多大都不糊，也不必给每种尺寸各算一套放大倍率。
 * 内容和 Rust 的权威渲染逐位同源（compositeFrame），缩略图看到的帧就是导出的帧。
 */
export default function FrameThumb({
  document: doc,
  index,
  compact = false,
}: {
  document: PixelDocument;
  index: number;
  /** 窄容器里改用小盒子；比例照旧按文档最长边算，缩略图永远和导出的帧同源。 */
  compact?: boolean;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const maxWidth = compact ? THUMB_COMPACT_WIDTH : THUMB_MAX_WIDTH;
  const maxHeight = compact ? THUMB_COMPACT_HEIGHT : THUMB_MAX_HEIGHT;
  const ratio = Math.min(maxWidth / doc.width, maxHeight / doc.height);
  // 极小文档也留个巴掌大的盒子，拇指点得到。
  const cssWidth = Math.max(12, Math.round(doc.width * ratio));
  const cssHeight = Math.max(12, Math.round(doc.height * ratio));

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    canvas.width = doc.width;
    canvas.height = doc.height;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.imageSmoothingEnabled = false;
    ctx.putImageData(new ImageData(compositeFrame(doc, index), doc.width, doc.height), 0, 0);
  }, [doc, index]);

  return (
    <canvas
      ref={canvasRef}
      className="frame-thumb"
      style={{ width: cssWidth, height: cssHeight }}
    />
  );
}
