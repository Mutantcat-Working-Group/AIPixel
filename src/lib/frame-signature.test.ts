// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it } from "vitest";

import { frameSignature } from "./frame-signature";
import { blankDocument } from "./store";
import type { PixelDocument, Rgba } from "./types";

const RED: Rgba = { r: 200, g: 40, b: 40, a: 255 };

/** 照 mergePatch 的语义改一格：只给动过的那帧换新 cel，其余原样沿用。 */
function withCel(doc: PixelDocument, frameId: string, indices: number[]): PixelDocument {
  const layerId = doc.layers[0].id;
  return {
    ...doc,
    cels: { ...doc.cels, [layerId]: { ...doc.cels[layerId], [frameId]: { indices } } },
  };
}

describe("frameSignature", () => {
  it("gives one document one signature per frame", () => {
    const doc = blankDocument(2, 1);
    expect(frameSignature(doc, 0)).toBe(frameSignature(doc, 0));
  });

  it("keeps untouched frames on their old signature", () => {
    const before = blankDocument(2, 1);
    const after = withCel(before, "F0", [1, 0]);
    // 动的是 F0，F0 要变号；别的帧照旧，缩略图这一步就能整个跳过去。
    expect(frameSignature(after, 0)).not.toBe(frameSignature(before, 0));
    expect(frameSignature(after, 1)).toBe(frameSignature(before, 1));
  });

  it("notices a palette edit even though no cel was replaced", () => {
    const before = blankDocument(2, 1);
    const after: PixelDocument = { ...before, palette: [RED] };
    expect(frameSignature(after, 0)).not.toBe(frameSignature(before, 0));
  });

  it("notices a layer being hidden or faded", () => {
    const before = blankDocument(2, 1);
    const layer = before.layers[0];
    const hidden: PixelDocument = { ...before, layers: [{ ...layer, visible: false }] };
    const faded: PixelDocument = { ...before, layers: [{ ...layer, opacity: 128 }] };
    expect(frameSignature(hidden, 0)).not.toBe(frameSignature(before, 0));
    expect(frameSignature(faded, 0)).not.toBe(frameSignature(before, 0));
  });

  it("reports a missing frame instead of throwing", () => {
    expect(frameSignature(blankDocument(2, 1), 9)).toContain("|");
  });
});
