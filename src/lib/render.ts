// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 帧合成：把 cels 里的调色板索引压成一帧 RGBA。
// 语义逐位对齐 Rust 的 `pixel-core/src/png.rs::composite_pixel`——播放预览和
// 洋葱皮看到的必须和权威渲染同一个东西，否则用户预览的动画不是他导出的动画。

import type { PixelDocument } from "./types";

/**
 * 合成一帧。返回 `width * height * 4` 的 RGBA，索引 0 与隐藏图层都当透明。
 * 越界 / 空帧给一块全透明，调用方不用各自判空。
 */
export function compositeFrame(
  doc: PixelDocument,
  frameIndex: number,
  // 具体写成 <ArrayBuffer>：ImageData 的构造只收这一种，写宽了过不了 tsc。
  // buffer 尺寸一起钉死，putImageData 才不会抱怨长度不齐。
): Uint8ClampedArray<ArrayBuffer> {
  const { width, height } = doc;
  const out = new Uint8ClampedArray(new ArrayBuffer(width * height * 4));
  // 累加器里放「未预乘」的 RGB 与 alpha，和 Rust 的 [f32; 4] 一致：
  // alpha 得单独留着，混色时的权重用的是它而不是压暗后的分量。
  const acc = new Float32Array(width * height * 4);
  const frame = doc.frames[frameIndex];
  if (!frame) return out;

  for (const layer of doc.layers) {
    if (!layer.visible) continue;
    const cel = doc.cels[layer.id]?.[frame.id];
    if (!cel) continue;
    const layerOpacity = layer.opacity / 255;
    const indices = cel.indices;
    for (let p = 0; p < indices.length && p < width * height; p++) {
      const index = indices[p];
      // 索引 0 是透明格，不是调色板第 0 号色。
      if (index === 0) continue;
      const color = doc.palette[index - 1];
      if (!color) continue;
      const a = (color.a / 255) * layerOpacity;
      if (a <= 0) continue;
      const o = p * 4;
      const outA = a + acc[o + 3] * (1 - a);
      if (outA <= 0) continue;
      const src: [number, number, number] = [color.r, color.g, color.b];
      for (let c = 0; c < 3; c++) {
        acc[o + c] = (src[c] * a + acc[o + c] * acc[o + 3] * (1 - a)) / outA;
      }
      acc[o + 3] = outA;
    }
  }

  // acc 里 RGB 是 0..255，只有 alpha 是 0..1（和 Rust 的 [f32; 4] 同一套量纲），
  // 所以只有第四个通道需要换算回字节。
  for (let p = 0; p < acc.length; p += 4) {
    out[p] = acc[p];
    out[p + 1] = acc[p + 1];
    out[p + 2] = acc[p + 2];
    out[p + 3] = Math.round(acc[p + 3] * 255);
  }
  return out;
}
