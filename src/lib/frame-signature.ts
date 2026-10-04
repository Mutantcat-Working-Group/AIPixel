// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 帧缩略图的重绘判据：签名没变的帧，合成一次都不该再合成。

import type { Cel, PixelDocument } from "./types";

/** cel 的身份号。同一个对象永远同一个号，换了新对象就是新号。 */
const celIds = new WeakMap<Cel, number>();
let nextCelId = 0;

function celId(cel: Cel): number {
  let id = celIds.get(cel);
  if (id === undefined) {
    nextCelId += 1;
    id = nextCelId;
    celIds.set(cel, id);
  }
  return id;
}

/**
 * 文档级那半段签名：尺寸、调色板、图层的可见性与不透明度。
 * 这些一变，每一帧的合成结果都跟着变，所以按文档对象缓存一份共用。
 */
const headers = new WeakMap<PixelDocument, string>();

function documentHeader(doc: PixelDocument): string {
  const cached = headers.get(doc);
  if (cached !== undefined) return cached;
  // 调色板比的是颜色值不是数组引用：整份文档每次广播都是新对象，
  // 比引用等于每次都判「变了」，缓存也就白做了。
  const palette = doc.palette.map((c) => `${c.r},${c.g},${c.b},${c.a}`).join(";");
  const layers = doc.layers.map((l) => `${l.id}:${l.visible ? 1 : 0}:${l.opacity}`).join(",");
  const value = `${doc.width}x${doc.height}|${palette}|${layers}`;
  headers.set(doc, value);
  return value;
}

/**
 * 一帧的重绘判据：两份签名相等，合出来的就是同一张图。
 *
 * 为什么能拿 cel 的「引用」当像素的指纹：文档只从 `mergePatch` 那条路进来，
 * 而它对没动的 cel 原样沿用旧对象、只给改过的那几格换新对象。于是「这一帧
 * 各层挂着的 cel 还是不是原来那几个」恰好就是「这一帧动了没有」。
 *
 * 换来的是：一回合几十次广播里改的常常只有当前那一帧，其余帧不必每广播一次
 * 就重合成一遍。256x256 八层文档单帧合成是五十万次循环，帧条三十格就是
 * 一千五百万次，白干的部分正是界面发卡的来源。
 */
export function frameSignature(doc: PixelDocument, index: number): string {
  const frame = doc.frames[index];
  if (!frame) return `${documentHeader(doc)}|`;
  const cels = doc.layers
    .map((layer) => {
      const cel = doc.cels[layer.id]?.[frame.id];
      return cel ? celId(cel) : "-";
    })
    .join(",");
  return `${documentHeader(doc)}|${frame.id}|${cels}`;
}
