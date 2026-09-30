// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 常见颜色名表：hex + 中英文名。给界面把 hex 翻成色名当 tooltip，也给取色器当建议色。
// 与 Rust 的 `crates/agent-core/src/colornames.rs` 同源：那边喂模型，这边喂眼睛，
// 两张表必须指同样的色，否则「模型说红」和「界面上那颗红」会对不上。

/** 一个带名字的颜色。hex 全是不带 alpha 的六位小写。 */
export interface NamedColor {
  hex: string;
  en: string;
  zh: string;
}

/** 按色相环走一遍，同色系内由浅到深。 */
export const NAMED_COLORS: NamedColor[] = [
  { hex: "#000000", en: "black", zh: "黑" },
  { hex: "#1a1a1a", en: "ink", zh: "墨色" },
  { hex: "#333333", en: "charcoal", zh: "炭灰" },
  { hex: "#666666", en: "gray", zh: "灰" },
  { hex: "#999999", en: "silver", zh: "银灰" },
  { hex: "#cccccc", en: "ash", zh: "石灰" },
  { hex: "#ffffff", en: "white", zh: "白" },
  { hex: "#8b0000", en: "dark red", zh: "深红" },
  { hex: "#c0392b", en: "brick red", zh: "砖红" },
  { hex: "#e74c3c", en: "red", zh: "红" },
  { hex: "#ff6b6b", en: "coral red", zh: "珊瑚红" },
  { hex: "#ff9999", en: "salmon pink", zh: "鲑红" },
  { hex: "#ffccd5", en: "light pink", zh: "浅粉" },
  { hex: "#ffb6c1", en: "pink", zh: "粉红" },
  { hex: "#ff69b4", en: "hot pink", zh: "洋红" },
  { hex: "#c71585", en: "magenta", zh: "品红" },
  { hex: "#8e44ad", en: "purple", zh: "紫" },
  { hex: "#6c3483", en: "deep purple", zh: "深紫" },
  { hex: "#9b59b6", en: "violet", zh: "蓝紫" },
  { hex: "#d7bde2", en: "lilac", zh: "淡紫" },
  { hex: "#4a235a", en: "aubergine", zh: "茄紫" },
  { hex: "#16a085", en: "teal", zh: "青绿" },
  { hex: "#1abc9c", en: "turquoise", zh: "绿松石" },
  { hex: "#48c9b0", en: "light teal", zh: "浅青绿" },
  { hex: "#0077be", en: "azure", zh: "天青" },
  { hex: "#2e86c1", en: "blue", zh: "蓝" },
  { hex: "#3498db", en: "sky blue", zh: "天蓝" },
  { hex: "#85c1e9", en: "light blue", zh: "浅蓝" },
  { hex: "#1b4f72", en: "navy", zh: "藏青" },
  { hex: "#0b3d91", en: "deep blue", zh: "深蓝" },
  { hex: "#2874a6", en: "steel blue", zh: "钢蓝" },
  { hex: "#5dade2", en: "cornflower", zh: "矢车菊蓝" },
  { hex: "#cce5ff", en: "pale blue", zh: "极浅蓝" },
  { hex: "#145a32", en: "forest green", zh: "森绿" },
  { hex: "#1e8449", en: "green", zh: "绿" },
  { hex: "#27ae60", en: "leaf green", zh: "叶绿" },
  { hex: "#52be80", en: "light green", zh: "浅绿" },
  { hex: "#a9dfbf", en: "mint", zh: "薄荷绿" },
  { hex: "#7dcea0", en: "sage", zh: "鼠尾草绿" },
  { hex: "#f4d03f", en: "yellow", zh: "黄" },
  { hex: "#f7dc6f", en: "cream yellow", zh: "奶黄" },
  { hex: "#d4ac0d", en: "mustard", zh: "芥末黄" },
  { hex: "#b7950b", en: "olive", zh: "橄榄绿" },
  { hex: "#e67e22", en: "orange", zh: "橙" },
  { hex: "#d35400", en: "burnt orange", zh: "焦橙" },
  { hex: "#f0b27a", en: "peach", zh: "桃" },
  { hex: "#ca6f1e", en: "carrot", zh: "胡萝卜" },
  { hex: "#a04000", en: "rust", zh: "锈色" },
  { hex: "#873600", en: "brown", zh: "棕" },
  { hex: "#6e2c00", en: "dark brown", zh: "深棕" },
  { hex: "#935116", en: "light brown", zh: "浅棕" },
  { hex: "#c39b6d", en: "tan", zh: "浅褐" },
  { hex: "#e8c39e", en: "sand", zh: "沙色" },
  { hex: "#f5e6ca", en: "beige", zh: "米白" },
  { hex: "#dc7633", en: "terracotta", zh: "陶土" },
  { hex: "#7b5133", en: "walnut", zh: "胡桃木" },
  { hex: "#a9714b", en: "oak", zh: "橡木" },
  { hex: "#fdebd0", en: "linen", zh: "亚麻" },
  { hex: "#fad7a0", en: "apricot", zh: "杏黄" },
  { hex: "#fd79b8", en: "rose", zh: "玫红" },
  { hex: "#e84393", en: "fuchsia", zh: "品红" },
  { hex: "#6d4c41", en: "umber", zh: "棕褐" },
  { hex: "#f8c471", en: "skin light", zh: "浅肤" },
  { hex: "#e0ac69", en: "skin", zh: "肤色" },
  { hex: "#c68642", en: "skin tan", zh: "小麦肤" },
  { hex: "#8d5524", en: "skin deep", zh: "深肤" },
  { hex: "#f2d7d5", en: "blush", zh: "腮红" },
  { hex: "#d98880", en: "rosewood", zh: "玫木" },
  { hex: "#cd6155", en: "cheek", zh: "颊彩" },
  { hex: "#a93226", en: "blood red", zh: "血红" },
  { hex: "#641e16", en: "dark blood", zh: "暗血红" },
  { hex: "#fdfefe", en: "snow", zh: "雪白" },
  { hex: "#aab7b8", en: "stone", zh: "石灰" },
  { hex: "#707b7c", en: "slate", zh: "石板灰" },
  { hex: "#424949", en: "graphite", zh: "石墨" },
  { hex: "#b2babb", en: "fog", zh: "雾灰" },
  { hex: "#d5dbdb", en: "pearl", zh: "珍珠灰" },
  { hex: "#f9e79f", en: "pale yellow", zh: "鹅黄" },
  { hex: "#f5b041", en: "amber", zh: "琥珀" },
  { hex: "#9c640c", en: "gold brown", zh: "金棕" },
  { hex: "#f1c40f", en: "gold", zh: "金" },
  { hex: "#bf9000", en: "dark gold", zh: "暗金" },
  { hex: "#d5d8dc", en: "silver pale", zh: "白银" },
  { hex: "#909497", en: "iron", zh: "铁灰" },
  { hex: "#5d6d7e", en: "steel", zh: "钢色" },
  { hex: "#2c3e50", en: "dark steel", zh: "深钢蓝" },
  { hex: "#34495e", en: "gunmetal", zh: "炮铜" },
  { hex: "#99a3a4", en: "pewter", zh: "白镴" },
  { hex: "#58d68d", en: "spring green", zh: "春绿" },
  { hex: "#28b463", en: "emerald", zh: "翠绿" },
  { hex: "#239b56", en: "jade", zh: "玉色" },
  { hex: "#196f3d", en: "pine", zh: "松绿" },
  { hex: "#5499c7", en: "denim", zh: "牛仔蓝" },
  { hex: "#21618c", en: "cobalt", zh: "钴蓝" },
  { hex: "#154360", en: "midnight", zh: "午夜蓝" },
  { hex: "#a9cce3", en: "ice blue", zh: "冰蓝" },
  { hex: "#76448a", en: "plum", zh: "梅紫" },
  { hex: "#5b2c6f", en: "grape", zh: "葡紫" },
  { hex: "#bb8fce", en: "lavender", zh: "薰衣草" },
  { hex: "#e6b0aa", en: "dusty rose", zh: "灰玫" },
  { hex: "#ec7063", en: "brick light", zh: "浅砖红" },
  { hex: "#e59866", en: "terracotta light", zh: "浅陶土" },
  { hex: "#abebc6", en: "seafoam", zh: "海沫绿" },
  { hex: "#76d7c4", en: "aqua", zh: "水绿" },
  { hex: "#45b39d", en: "pine teal", zh: "松绿青" },
  { hex: "#17a589", en: "cyan", zh: "青" },
  { hex: "#f7f9f9", en: "paper", zh: "纸白" },
];

function parseRgb(hex: string): [number, number, number] | null {
  const text = hex.trim().replace(/^#/, "").slice(0, 6);
  if (text.length !== 6) return null;
  const part = (from: number) => Number.parseInt(text.slice(from, from + 2), 16);
  const r = part(0);
  const g = part(2);
  const b = part(4);
  if ([r, g, b].some((value) => Number.isNaN(value))) return null;
  return [r, g, b];
}

function distance(a: [number, number, number], b: [number, number, number]): number {
  const dr = a[0] - b[0];
  const dg = a[1] - b[1];
  const db = a[2] - b[2];
  return dr * dr + dg * dg + db * db;
}

/**
 * 按 RGB 欧氏距离找最近的有名颜色。同距时留在前面的那一项，和 Rust 同一套
 * 择优规则：结果稳定可复现，同一个 hex 永远得到同一个名字。
 */
export function nearestNamed(hex: string): NamedColor | null {
  const target = parseRgb(hex);
  if (!target) return null;
  let best: NamedColor | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (const entry of NAMED_COLORS) {
    const rgb = parseRgb(entry.hex);
    if (!rgb) continue;
    const score = distance(target, rgb);
    if (score < bestDistance) {
      bestDistance = score;
      best = entry;
    }
  }
  return best;
}

/** 色名注脚：中文界面上写「黑 / 黑 black」读着顺，英语界面上反过来。 */
export function colorName(hex: string, lang: "zh" | "en"): string {
  const hit = nearestNamed(hex);
  if (!hit) return hex.toLowerCase();
  return lang === "zh" ? `${hit.zh} · ${hit.en}` : `${hit.en} · ${hit.zh}`;
}

