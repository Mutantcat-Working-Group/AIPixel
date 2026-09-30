// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 尺寸口径的规矩：预设原样透传，自定义与跟随画布都要 snap 到 32 的倍数，
// 且夹在 256..1536 之间——不少兼容实现不认这个粒度之外的尺寸，直接 400。

import { describe, expect, it } from "vitest";

import { canvasAspectSize, resolveImageSize, SIZE_OPTIONS } from "./image-size";

describe("resolveImageSize", () => {
  it("预设档位原样发出去，不加工", () => {
    const draft = {
      sizeMode: "preset" as const,
      size: "1024x576",
      sizeW: 999,
      sizeH: 999,
    };
    expect(resolveImageSize(draft, 64, 64)).toBe("1024x576");
  });

  it("自定义尺寸按 32 对齐并夹在区间内", () => {
    const draft = {
      sizeMode: "custom" as const,
      size: "",
      sizeW: 700,
      sizeH: 100,
    };
    // 700 -> 704，100 被抬到下限 256。
    expect(resolveImageSize(draft, 64, 64)).toBe("704x256");
  });

  it("跟随画布时方形画布得方形尺寸", () => {
    const draft = {
      sizeMode: "canvas" as const,
      size: "",
      sizeW: 64,
      sizeH: 48,
    };
    expect(resolveImageSize(draft, 64, 64)).toBe("1024x1024");
  });

  it("跟随画布时宽画布得宽尺寸，比例不变", () => {
    const draft = {
      sizeMode: "canvas" as const,
      size: "",
      sizeW: 0,
      sizeH: 0,
    };
    expect(resolveImageSize(draft, 320, 180)).toBe("1024x576");
  });

  it("画布尺寸不可信时退回方形，不折出个 0", () => {
    expect(canvasAspectSize(0, 0)).toBe("1024x1024");
    expect(canvasAspectSize(-4, 10)).toBe("1024x1024");
    expect(canvasAspectSize(Number.NaN, 10)).toBe("1024x1024");
  });

  it("预设档位本身都合法", () => {
    for (const option of SIZE_OPTIONS) {
      expect(option).toMatch(/^\d+x\d+$/);
    }
  });
});
