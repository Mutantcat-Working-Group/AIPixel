// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
/**
 * 画风预设 id。和 Rust `ArtStyle::id()` 一一对应；两边的集合由
 * `artstyle.test.ts` 用源码比对守住，任一侧加了预设测试就红。
 * 这里只放 id 和收窄判断，不重复 Rust 的规则正文——那是以提示词形式下发的。
 */
export const ART_STYLE_IDS = [
  "mono1",
  "gameboy",
  "nes",
  "pico8",
  "cga",
  "dither",
  "pastel",
  "hibit",
  "realistic",
  "fine",
  "painterly",
  "cel",
  "noir",
  "neon",
] as const;

export type ArtStyleId = (typeof ART_STYLE_IDS)[number];

/** 收窄成画风 id。用户在下拉里选中的一定是这里的一个值。 */
export function isArtStyleId(value: string): value is ArtStyleId {
  return (ART_STYLE_IDS as readonly string[]).includes(value);
}
