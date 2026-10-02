// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it } from "vitest";

import {
  PALETTE_PRESETS,
  hexesOf,
  nearestHex,
  paletteWithCustom,
  parseHex,
  rgbaToHex,
  swatchCommitOnClose,
} from "./palette";
import type { Rgba } from "./types";

describe("presets", () => {
  it("keeps every preset color parseable and duplicate-free", () => {
    for (const preset of PALETTE_PRESETS) {
      expect(preset.colors.length, preset.id).toBeGreaterThan(1);
      for (const hex of preset.colors) {
        expect(parseHex(hex), `${preset.id} ${hex}`).not.toBeNull();
      }
      const lowered = preset.colors.map((hex) => hex.toLowerCase());
      expect(new Set(lowered).size, preset.id).toBe(lowered.length);
    }
  });

  it("gives every preset a distinct id", () => {
    const ids = PALETTE_PRESETS.map((preset) => preset.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("hex round trip", () => {
  it("parses with or without the hash and keeps alpha only when it matters", () => {
    expect(parseHex("ff004d")).toEqual({ r: 255, g: 0, b: 77, a: 255 });
    expect(parseHex("#FF004D")).toEqual({ r: 255, g: 0, b: 77, a: 255 });
    expect(parseHex("#ff004d80")).toEqual({ r: 255, g: 0, b: 77, a: 128 });
    expect(parseHex("#fff")).toBeNull();
    expect(parseHex("nope")).toBeNull();
  });

  it("writes six digits for opaque colors and eight for translucent ones", () => {
    expect(rgbaToHex({ r: 0, g: 0, b: 0, a: 255 })).toBe("#000000");
    expect(rgbaToHex({ r: 171, g: 82, b: 54, a: 200 })).toBe("#ab5236c8");
  });

  it("reads a document palette as hexes in palette order", () => {
    const palette: Rgba[] = [
      { r: 255, g: 0, b: 0, a: 255 },
      { r: 0, g: 0, b: 255, a: 128 },
    ];
    expect(hexesOf({ palette })).toEqual(["#ff0000", "#0000ff80"]);
  });
});

describe("nearestHex", () => {
  const sweetie = PALETTE_PRESETS.find((preset) => preset.id === "sweetie16")!.colors;

  it("returns the candidate spelling when a color is already in range", () => {
    // 大小写不一致也认：返回候选自己的拼法，UI 高亮才对得上。
    expect(nearestHex(sweetie, "#1A1C2C")).toBe("#1a1c2c");
  });

  it("snaps out-of-range colors to the closest neighbor", () => {
    expect(nearestHex(["#000000", "#ffffff"], "#0a0a0a")).toBe("#000000");
    expect(nearestHex(["#000000", "#ffffff"], "#f2f2f2")).toBe("#ffffff");
    // 深红该归到红，不该被中灰截胡：欧氏距离会判错的那种。
    expect(nearestHex(["#b13e53", "#4e4a4e"], "#c03040")).toBe("#b13e53");
  });

  it("keeps the first candidate on ties so mapping is reproducible", () => {
    // #c0c0c0 和 #404040 到 #808080 的 redmean 距离严格相等，取靠前的那一项。
    expect(nearestHex(["#c0c0c0", "#404040"], "#808080")).toBe("#c0c0c0");
    expect(nearestHex(["#404040", "#c0c0c0"], "#808080")).toBe("#404040");
  });

  it("gives up on garbage instead of guessing", () => {
    expect(nearestHex(sweetie, "not-a-color")).toBeNull();
    expect(nearestHex([], "#ff004d")).toBeNull();
  });
});

describe("paletteWithCustom", () => {
  it("appends the custom color to the range", () => {
    expect(paletteWithCustom(["#000000", "#ffffff"], "#ff004d")).toEqual([
      "#000000",
      "#ffffff",
      "#ff004d",
    ]);
  });

  it("never grows a second custom slot", () => {
    const once = paletteWithCustom(["#000000"], "#ff004d");
    expect(paletteWithCustom(once, "#00e436")).toEqual(["#000000", "#ff004d", "#00e436"]);
    expect(paletteWithCustom(once, "#ff004d")).toEqual(["#000000", "#ff004d"]);
  });

  it("leaves the range alone when no custom color is set", () => {
    const base = ["#000000", "#ffffff"];
    expect(paletteWithCustom(base, null)).toEqual(base);
  });
});

describe("取色盘关弹层收尾", () => {
  it("没挑过就什么都不落", () => {
    expect(swatchCommitOnClose(null, null)).toBeNull();
    expect(swatchCommitOnClose("", "#ff004d")).toBeNull();
  });

  it("点预设、手输 hex 挑出来的色在关窗时落库", () => {
    expect(swatchCommitOnClose("#ff004d", null)).toBe("#ff004d");
  });

  it("拖完滑块那一轮已经落过，关窗不再落第二次", () => {
    expect(swatchCommitOnClose("#ff004d", "#ff004d")).toBeNull();
  });

  it("同一个色大小写写法和本轮落过的那个不同，也不重复落", () => {
    expect(swatchCommitOnClose("#FF004D", "#ff004d")).toBeNull();
  });

  it("拖完一色再拖另一色，落的是新色", () => {
    expect(swatchCommitOnClose("#00e436", "#ff004d")).toBe("#00e436");
  });
});
