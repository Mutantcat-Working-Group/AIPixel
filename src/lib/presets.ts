// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
/**
 * 内置提示词预设 id。和 Rust `presets::ALL` 的 id 一一对应；两边的集合由
 * `presets.test.ts` 用源码比对守住，任一侧加了预设测试就红。
 *
 * 这些不是画风：画风管色数、描边、抖动那类硬指标（见 `artstyle.ts`），预设管
 * 「同一套色数下这张图按什么规矩收尾」——形体明暗怎么算、材质怎么分光、柔边
 * 从哪里来。用户抱怨「不咋写实」「细节不够」时，能拧的是这一颗螺丝。
 *
 * 同样只放 id 和收窄判断，不重复 Rust 的规矩正文：那是直接下发给模型的英文提示词，
 * 在前端留一份副本只会两边不一致。
 */
export const PRESET_IDS = [
  "realistic",
  "cinematic",
  "material",
  "polish",
  "depth",
  "soft",
  "texture",
  "form",
  "silhouette",
  "weathered",
  // ---------- 细节六条 ----------
  // 后六条专门伺候细节：微细结构讲最后一两个像素放在哪里，闭塞接触讲暗部怎么
  // 攒起来，明度结构讲去色之后还读不读得懂，色彩层级讲谁当那个最艳的点，
  // 反射透光讲材质怎么从一块平色里分出来，构图焦点讲细节预算往哪儿花。
  // 用户抱怨「不咋写实」「细节不够」时，缺的往往是这六条里的两三条，
  // 而不是第一组里的某一条。
  "microdetail",
  "occlusion",
  "value",
  "hierarchy",
  "reflection",
  "composition",
] as const;

export type PresetId = (typeof PRESET_IDS)[number];

/**
 * 一次最多同时生效几条。和 Rust `presets::MAX_STACKED` 是同一个数，两边各守一遍：
 * Rust 在命令入参里拒收超额的，前端在这里就不该让第四条选上——选上又被拒，
 * 用户看到的是一次没头没尾的失败。
 *
 * 三条是有意的：细节这件事是乘法，但规矩之间也在互相稀释，「写实渲染」+
 * 「微细结构」+「闭塞接触」三样各管一段就够凑成一张写实的图。
 */
export const MAX_STACKED_PRESETS = 3;

/** 收窄成预设 id。用户在下拉里选中的一定是这里的一个值。 */
export function isPresetId(value: string): value is PresetId {
  return (PRESET_IDS as readonly string[]).includes(value);
}

/** 认得过的 id、去掉重复，顺序照旧。不收上限——超没超要由调用方说了算。 */
export function uniquePresetIds(stack: readonly string[]): string[] {
  const out: string[] = [];
  for (const id of stack) {
    if (isPresetId(id) && !out.includes(id)) out.push(id);
  }
  return out;
}

/** 收窄成一份能直接发出去的清单：认得过、不重复、不超上限。 */
export function normalizePresetStack(stack: readonly string[]): string[] {
  return uniquePresetIds(stack).slice(0, MAX_STACKED_PRESETS);
}

/**
 * 套餐：一次铺好一整套收尾规矩。用户嘴里说的是「画得写实一点」，
 * 心里想的是三四件事一起发生（形体怎么算、细节放哪、暗部怎么攒）——
 * 让他自己在十六条里挑三条，多半挑不出那三四件事。
 *
 * 点一下 = 替换当前选择，不合并。合并很容易顶到三条上限，
 * 用户看到的是第四条莫名其妙没进去。
 *
 * 组合刻意和 `artstyle::quality_preset_ids` 的那份保持一致：
 * 用户在画风下拉里选了「写实」，Rust 已经自带微细结构 + 闭塞接触；
 * 这里再点一次写实套餐，铺开的是同一套，不会出现两处结论对不上。
 */
export const PRESET_BUNDLES = [
  { id: "realistic", presetIds: ["realistic", "microdetail", "occlusion"] },
  { id: "detail", presetIds: ["microdetail", "polish", "value"] },
  { id: "scene", presetIds: ["cinematic", "depth", "composition"] },
] as const;

export type PresetBundleId = (typeof PRESET_BUNDLES)[number]["id"];

/** 下拉里套餐那一项的值前缀。没有这个前缀的就是普通预设 id。 */
export const BUNDLE_PREFIX = "bundle:";

/** 收窄成套餐 id。认不出的由调用方当作普通预设处理。 */
export function isPresetBundleId(value: string): value is PresetBundleId {
  return PRESET_BUNDLES.some((bundle) => bundle.id === value);
}

/** 套餐铺开之后的预设清单。认不出的套餐 id 返回 null，让调用方自己决定。 */
export function bundlePresetIds(bundle: string): readonly string[] | null {
  const hit = PRESET_BUNDLES.find((item) => item.id === bundle);
  return hit ? hit.presetIds : null;
}

/**
 * 把 Select 交上来的选择展开成能发出去的预设清单：点过套餐就按套餐铺开，
 * 没点过就原样收窄（去重、过滤、砍到上限）。
 *
 * 单独一个函数而不是塞在 onChange 里，是为了让「点套餐到底换来什么」
 * 这件事有测试守着：合并还是替换、超上限怎么收口，都是用户看得见的行为。
 */
export function expandPresetChoice(values: readonly string[]): string[] {
  const bundles = values.filter((value) => value.startsWith(BUNDLE_PREFIX));
  const plain = normalizePresetStack(
    values.filter((value) => !value.startsWith(BUNDLE_PREFIX)),
  );
  if (bundles.length === 0) return plain;
  // 一次点几个套餐就照最后一个算：那几个选项挨在一起，用户是想换一套，
  // 不是想两套叠一起。上一个赢，行为可预期。
  const last = bundlePresetIds(bundles[bundles.length - 1].slice(BUNDLE_PREFIX.length));
  return last ? normalizePresetStack(last) : plain;
}
