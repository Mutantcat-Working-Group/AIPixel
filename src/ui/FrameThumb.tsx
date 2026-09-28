import { useEffect, useRef } from "react";

import { compositeFrame } from "../lib/render";
import type { PixelDocument } from "../lib/types";

// 缩略图盒子的上限：再大的文档也在这个盒子里等比缩小，帧条不会被某一帧撑爆。
const THUMB_MAX_WIDTH = 46;
const THUMB_MAX_HEIGHT = 34;

/**
 * 帧条里的一格缩略图。画在文档尺寸的画布上，缩放交给 CSS 的 pixelated：
 * 文档多大都不糊，也不必给每种尺寸各算一套放大倍率。
 * 内容和 Rust 的权威渲染逐位同源（compositeFrame），缩略图看到的帧就是导出的帧。
 */
export default function FrameThumb({
  document: doc,
  index,
}: {
  document: PixelDocument;
  index: number;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const ratio = Math.min(THUMB_MAX_WIDTH / doc.width, THUMB_MAX_HEIGHT / doc.height);
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
