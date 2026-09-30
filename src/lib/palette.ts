// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 配色范围：调色板在这里就是「允许用哪些颜色」的边界。
// 换预设不是换着玩——Rust 的 set_palette 会按就近色把已有像素重映射进新范围，
// 画面留住，颜色归队。所以预设收的是几套人名级别的经典色板，取色也得跟眼睛站一起。

import type { Rgba } from "./types";

/** 一套配色预设。name 是专有名词（Sweetie 16 / PICO-8），两种语言下都不改写。 */
export interface PalettePreset {
  id: string;
  name: string;
  /** 全是不带 alpha 的六位小写 hex，和 to_hex 的拼法一致，好和文档调色板比对。 */
  colors: string[];
}

/** 预设按「自由度」从高到低排：越靠后越像风格化限制，不是越靠后越好。 */
export const PALETTE_PRESETS: PalettePreset[] = [
  {
    id: "gray8",
    name: "Gray 8",
    colors: [
      "#000000",
      "#242424",
      "#484848",
      "#6c6c6c",
      "#909090",
      "#b4b4b4",
      "#d8d8d8",
      "#ffffff",
    ],
  },
  {
    id: "sweetie16",
    name: "Sweetie 16",
    colors: [
      "#1a1c2c",
      "#5d275d",
      "#b13e53",
      "#ef7d57",
      "#ffcd75",
      "#a7f070",
      "#38b764",
      "#257179",
      "#29366f",
      "#3b5dc9",
      "#41a6f6",
      "#73eff7",
      "#f4f4f4",
      "#94b0c2",
      "#566c86",
      "#333c57",
    ],
  },
  {
    id: "db16",
    name: "DawnBringer 16",
    colors: [
      "#140c1c",
      "#442434",
      "#30346d",
      "#4e4a4e",
      "#854c30",
      "#346524",
      "#d04648",
      "#757161",
      "#597dce",
      "#d27d2c",
      "#8595a1",
      "#6daa2c",
      "#d2aa99",
      "#6dc2ca",
      "#dad45e",
      "#deeed6",
    ],
  },
  {
    id: "pico8",
    name: "PICO-8",
    colors: [
      "#000000",
      "#1d2b53",
      "#7e2553",
      "#008751",
      "#ab5236",
      "#5f574f",
      "#c2c3c7",
      "#fff1e8",
      "#ff004d",
      "#ffa300",
      "#ffec27",
      "#00e436",
      "#29adff",
      "#83769c",
      "#ff77a8",
      "#ffccaa",
    ],
  },
  {
    id: "gameboy",
    name: "Game Boy",
    colors: ["#0f380f", "#306230", "#8bac0f", "#9bbc0f"],
  },
  {
    id: "onebit",
    name: "1-bit",
    colors: ["#000000", "#ffffff"],
  },
];

/** 解析 hex 字面量（# 可省，六位或八位）。坏了给 null，不抛。 */
export function parseHex(hex: string): Rgba | null {
  const text = hex.trim().replace(/^#/, "");
  if (text.length !== 6 && text.length !== 8) return null;
  const byte = (from: number) => {
    const value = Number.parseInt(text.slice(from, from + 2), 16);
    return Number.isNaN(value) ? null : value;
  };
  const r = byte(0);
  const g = byte(2);
  const b = byte(4);
  const a = text.length === 8 ? byte(6) : 255;
  if (r === null || g === null || b === null || a === null) return null;
  return { r, g, b, a };
}

/** 颜色 -> 文档调色板用的 hex 串。alpha 满的就六位，和 Rust 的 to_hex 对齐。 */
export function rgbaToHex(color: Rgba): string {
  const part = (value: number) => value.toString(16).padStart(2, "0");
  const base = `#${part(color.r)}${part(color.g)}${part(color.b)}`;
  return color.a === 255 ? base : `${base}${part(color.a)}`;
}

/** 文档调色板 -> hex 串列表。索引 0 是透明，所以这里比 palette 少一项，别拿去当下标用。 */
export function hexesOf(document: { palette: Rgba[] }): string[] {
  return document.palette.map(rgbaToHex);
}

/**
 * redmean 距离：人眼对绿色的差特别敏感，对暗部的红特别迟钝。
 * 直接用 RGB 欧氏距离会把深红和深灰判成近邻，皮肤和木头的层次就糊了。
 */
function redmeanDistance(a: Rgba, b: Rgba): number {
  const rMean = (a.r + b.r) / 2;
  const dr = a.r - b.r;
  const dg = a.g - b.g;
  const db = a.b - b.b;
  const weightR = 2 + rMean / 256;
  const weightG = 4;
  const weightB = 2 + (255 - rMean) / 256;
  return dr * dr * weightR + dg * dg * weightG + db * db * weightB;
}

/**
 * 在候选色里找最近的一个，返回候选自己的拼写（大小写跟着候选走，UI 高亮才对得上）。
 * 完全同色就直接返回那一项，省得让用户觉得「选了的色被偷偷改了」。
 */
export function nearestHex(candidates: string[], hex: string): string | null {
  const target = parseHex(hex);
  if (!target) return null;
  let best: string | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const candidate of candidates) {
    const color = parseHex(candidate);
    if (!color) continue;
    if (
      color.r === target.r &&
      color.g === target.g &&
      color.b === target.b &&
      color.a === target.a
    ) {
      return candidate;
    }
    const distance = redmeanDistance(color, target);
    // 严格小于：同距时留在前面的那一项，结果才稳定可复现。
    if (distance < bestDistance) {
      bestDistance = distance;
      best = candidate;
    }
  }
  return best;
}

/**
 * 拼出实际生效的调色板：范围底子加上最多一个「任意颜色」。
 * 任意颜色已经落在底子里就先去重再追加——set_palette 收到重复色会浪费索引，
 * 而「最多一个」正是这个函数的形状：调用方只有一个槽位可填。
 */
export function paletteWithCustom(base: string[], custom: string | null): string[] {
  if (!custom) return [...base];
  return [...base.filter((hex) => hex.toLowerCase() !== custom.toLowerCase()), custom];
}
