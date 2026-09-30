// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it } from "vitest";

import { compositeFrame } from "./render";
import { blankDocument } from "./store";
import type { PixelDocument, Rgba } from "./types";

const RED: Rgba = { r: 200, g: 40, b: 40, a: 255 };
const HALF_BLUE: Rgba = { r: 30, g: 30, b: 200, a: 128 };

function withCel(doc: PixelDocument, frameId: string, indices: number[]): PixelDocument {
  return {
    ...doc,
    cels: { ...doc.cels, [doc.layers[0].id]: { ...doc.cels[doc.layers[0].id], [frameId]: { indices } } },
  };
}

describe("compositeFrame", () => {
  it("maps palette index 1 onto the first palette color and leaves 0 transparent", () => {
    const doc = withCel(blankDocument(2, 1), "F0", [1, 0]);
    const out = compositeFrame({ ...doc, palette: [RED] }, 0);
    expect([out[0], out[1], out[2], out[3]]).toEqual([200, 40, 40, 255]);
    // 索引 0 是透明格，不是调色板第 0 号色。
    expect([out[4], out[5], out[6], out[7]]).toEqual([0, 0, 0, 0]);
  });

  it("skips hidden layers entirely", () => {
    const base = blankDocument(1, 1);
    const doc: PixelDocument = {
      ...base,
      palette: [RED],
      cels: { [base.layers[0].id]: { F0: { indices: [1] } } },
      layers: [{ ...base.layers[0], visible: false }],
    };
    expect(Array.from(compositeFrame(doc, 0))).toEqual([0, 0, 0, 0]);
  });

  it("scales a layer's opacity against the color's own alpha", () => {
    const base = blankDocument(1, 1);
    const doc: PixelDocument = {
      ...base,
      palette: [HALF_BLUE],
      cels: { [base.layers[0].id]: { F0: { indices: [1] } } },
      layers: [{ ...base.layers[0], opacity: 128 }],
    };
    // 128/255 色 alpha 再乘 128/255 图层不透明度。
    expect(compositeFrame(doc, 0)[3]).toBe(Math.round((128 / 255) * (128 / 255) * 255));
  });

  it("composites upper layers over lower ones with source-over", () => {
    const base = blankDocument(1, 1);
    base.layers.push({
      id: "L1",
      name: "top",
      visible: true,
      opacity: 255,
      palette_id: "sweetie16",
      locked: false,
    });
    const doc: PixelDocument = {
      ...base,
      palette: [HALF_BLUE, RED],
      cels: {
        L0: { F0: { indices: [1] } },
        L1: { F0: { indices: [2] } },
      },
    };
    const out = compositeFrame(doc, 0);
    expect(out[3]).toBe(255);
    // 上半不透明红压住半透明蓝，落点应该明显偏红。
    expect(out[0]).toBeGreaterThan(out[2]);
  });

  it("returns fully transparent pixels for a frame that does not exist", () => {
    const doc = { ...blankDocument(2, 1), palette: [RED] };
    expect(Array.from(compositeFrame(doc, 9))).toEqual([0, 0, 0, 0, 0, 0, 0, 0]);
  });

  it("reads each frame separately so playback never shows the same pose twice", () => {
    const base = blankDocument(2, 1);
    base.frames.push({ id: "F1", duration_ms: 100 });
    let doc: PixelDocument = { ...base, palette: [RED] };
    doc = withCel(doc, "F0", [1, 0]);
    doc = withCel(doc, "F1", [0, 1]);
    expect(Array.from(compositeFrame(doc, 0))).toEqual([200, 40, 40, 255, 0, 0, 0, 0]);
    expect(Array.from(compositeFrame(doc, 1))).toEqual([0, 0, 0, 0, 200, 40, 40, 255]);
  });
});
