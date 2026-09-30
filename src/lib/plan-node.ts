// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 本轮分流节点（`pixel_plan`）的入参解析：把模型送来的那团 JSON 摆成几行字。

// 一个节点要回答四件事：参照图怎么参照、要的是哪类成品、风格预设锁没锁、
// 这一轮带了哪几条知识。四件事挤在一段裸 JSON 里，用户得自己数括号；
// 这里把它们读成「标签: 值（判据）」的行，读不出形状就原样退回 JSON。

// 解析必须容错：入参是模型手写的，字符串、对象、数组、按图号索引的对象
// 都可能出现，甚至是下一版加了新字段的形状。读不出形状不抛错——
// 一行报错比一段难看 JSON 更糟，用户要的是看见判词，不是看见异常。

/** 行里一个「这一项不限」的说法。和 Rust 侧 `plan::NONE_WORDS` 对齐。 */
const NONE_WORDS = ["none", "null", "clear", "auto", "unset", "any"];

/** 一行：左边一个标签，右边一列值，外加写下这件事的原话。 */
export type PlanRow = {
  /** 标签的文案键。 */
  labelKey: string;
  /** 标签里的变量，比如「第 {n} 张参照」的图号。 */
  vars?: Record<string, string | number>;
  /** 值的文案键。键不在字典里（模型自造的说法）就显示 raw。 */
  keys: string[];
  /** 与 keys 一一对应的原文。 */
  raws: string[];
  /** 判据：用户原话里定下这件事的那个说法。 */
  because?: string;
};

function asObject(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function text(value: unknown): string | null {
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

function isNone(raw: string): boolean {
  return NONE_WORDS.includes(raw.toLowerCase());
}

/** `{"id": "icon", "because": "图标"}` 和光秃秃 `"icon"` 是同一种写法。 */
function readId(value: unknown): { id: string; because?: string } | null {
  const obj = asObject(value);
  const id = obj ? text(obj.id) : text(value);
  if (!id) return null;
  const because = obj ? text(obj.because) ?? undefined : undefined;
  return because ? { id, because } : { id };
}

/** 成品意图、风格预设这种单项：标签一个，值一个，判据可有可无。 */
function readOne(labelKey: string, namespace: string, value: unknown): PlanRow | null {
  const read = readId(value);
  if (!read) return null;
  const row: PlanRow = { labelKey, keys: [], raws: [] };
  if (isNone(read.id)) {
    // 「不限」也是一种结论：上一轮锁了 Game Boy，这一轮用户没说风格，
    // 模型得有办法把预设摘掉，摘掉这件事本身要说出来。
    row.keys.push("plan.any");
  } else {
    row.keys.push(`${namespace}.${read.id.toLowerCase()}`);
  }
  row.raws.push(read.id);
  if (read.because) row.because = read.because;
  return row;
}

/** 图号：模型可以显式带 `index`，也可以只给列表位置，显式的那个赢。
 * 自动分流发的入参一定带 index，模型自己纠正常常不带，两条路都得对上。 */
function readIndex(obj: Record<string, unknown> | null, fallback: number): number {
  if (!obj) return fallback;
  const raw = obj.index;
  const n = typeof raw === "number" ? raw : typeof raw === "string" ? Number(raw) : Number.NaN;
  return Number.isInteger(n) && n > 0 ? n : fallback;
}

/** 参照定性：每张参考图各一行，行里带图号，好和出图清单对得上。 */
function readReference(value: unknown, index: number): PlanRow | null {
  const obj = asObject(value);
  const mode = obj ? text(obj.mode) : text(value);
  if (!mode) return null;
  const row: PlanRow = {
    labelKey: "plan.reference_one",
    vars: { n: readIndex(obj, index) },
    keys: [],
    raws: [mode],
  };
  row.keys.push(isNone(mode) ? "plan.any" : `plan.mode.${mode.toLowerCase()}`);
  const because = obj ? text(obj.because) : null;
  if (because) row.because = because;
  return row;
}

function readReferences(value: unknown): PlanRow[] {
  if (Array.isArray(value)) {
    return value
      .map((entry, i) => readReference(entry, i + 1))
      .filter((row): row is PlanRow => row !== null);
  }
  const obj = asObject(value);
  if (!obj) return [];
  // 按图号索引的对象：{"1": "full"}，图号就是键。
  const numbered = Object.keys(obj).filter((key) => /^\d+$/.test(key));
  if (numbered.length > 0) {
    return numbered
      .map((key) => readReference(obj[key], Number(key)))
      .filter((row): row is PlanRow => row !== null);
  }
  // 一条光秃秃的纠正：{"mode": "style"}，图号按位置推成 1。
  const single = readReference(obj, 1);
  return single ? [single] : [];
}

/** 知识条目：一行说完，值是一串标题。 */
function readKnowledge(value: unknown): PlanRow | null {
  const ids = Array.isArray(value)
    ? value.map(text).filter((id): id is string => id !== null)
    : [text(value)].filter((id): id is string => id !== null);
  if (ids.length === 0) return null;
  return {
    labelKey: "plan.knowledge",
    keys: ids.map((id) => `knowledge.${id.toLowerCase()}`),
    raws: ids,
  };
}

/** 把节点入参读成行。读不出任何一行就返回空数组，调用方照原样显示 JSON。 */
export function parsePlanRows(input: unknown): PlanRow[] {
  const obj = asObject(input);
  if (!obj) return [];
  const rows: PlanRow[] = [];
  if ("references" in obj) rows.push(...readReferences(obj.references));
  if ("intent" in obj) {
    const row = readOne("plan.intent", "intent", obj.intent);
    if (row) rows.push(row);
  }
  if ("style" in obj) {
    const row = readOne("plan.style", "style", obj.style);
    if (row) rows.push(row);
  }
  if ("knowledge" in obj) {
    const row = readKnowledge(obj.knowledge);
    if (row) rows.push(row);
  }
  return rows;
}
