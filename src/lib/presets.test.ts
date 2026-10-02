import { describe, expect, it } from "vitest";

import {
  BUNDLE_PREFIX,
  MAX_STACKED_PRESETS,
  PRESET_BUNDLES,
  PRESET_IDS,
  bundlePresetIds,
  expandPresetChoice,
  isPresetBundleId,
  normalizePresetStack,
  uniquePresetIds,
} from "./presets";
import { en, zh } from "./i18n";

/** 整个 agent-core 的源码原文：glob 在构建期展开，跑测试时就是一堆字符串。 */
const RUST_SOURCES = import.meta.glob("../../crates/*/src/*.rs", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

/**
 * Rust 是预设 id 的产地，前端下拉要按同一份清单摆选项。和画风那条同一个理由：
 * Rust 加了预设而 i18n 没补键，界面上那个选项就会露出 `preset.xxx` 原文；反过来
 * i18n 里多了键而 Rust 不认识，用户选了下拉只会拿到一句 `unknown prompt preset`。
 *
 * 这里比画风那条宽松一点：不要求出现顺序一致，只要集合相同。画风那个下拉靠
 * 顺序给人一个「从最硬到最软」的梯度，预设这边只是七个并列的收尾规矩，
 * 顺序没有语义，钉死只会让两边无谓地同步改动。
 */
function presetIdsFromRust(): string[] {
  const src = RUST_SOURCES["../../crates/agent-core/src/presets.rs"];
  if (src === undefined) throw new Error("presets.rs did not come through the raw glob");
  const ids = [...src.matchAll(/^\s+id: "([a-z0-9]+)",$/gm)].map((m) => m[1]);
  return [...new Set(ids)].sort();
}

/** Rust 那侧的界面标签。预设有两本字典（zh/en），名字漂了下拉和节点就对不上。 */
function presetLabelsFromRust(): Record<string, [string, string]> {
  const src = RUST_SOURCES["../../crates/agent-core/src/presets.rs"];
  if (src === undefined) throw new Error("presets.rs did not come through the raw glob");
  // 只认 `Preset {` 之后紧跟的三连，别把结构体定义里的字段声明也抓进来。
  const out: Record<string, [string, string]> = {};
  for (const m of src.matchAll(
    /Preset\s*\{\s*id: "([a-z0-9]+)",\s*label_zh: "([^"]+)",\s*label_en: "([^"]+)",/g,
  )) {
    out[m[1]] = [m[2], m[3]];
  }
  return out;
}

describe("prompt preset id parity", () => {
  it("lists exactly the ids rust knows", () => {
    expect(presetIdsFromRust().length).toBeGreaterThan(0);
    expect([...PRESET_IDS].sort()).toEqual(presetIdsFromRust());
  });

  it("names every preset in both dictionaries", () => {
    const missing = (dict: Record<string, string>) =>
      PRESET_IDS.filter((id) => !(`preset.${id}` in dict));
    expect(missing(zh)).toEqual([]);
    expect(missing(en)).toEqual([]);
  });

  it("shows the same label rust ships", () => {
    const labels = presetLabelsFromRust();
    for (const id of PRESET_IDS) {
      const [zhLabel, enLabel] = labels[id];
      expect(zhLabel, `preset.${id} 的 Rust 三连没读到`).toBeDefined();
      expect(zh[`preset.${id}`]).toBe(zhLabel);
      expect(en[`preset.${id}`]).toBe(enLabel);
    }
  });

  it("names every bundle button in both dictionaries", () => {
    // 套餐 id 不进 Rust 的预设表，但两本字典仍要逐键对上：漏一个键，
    // 按钮上就会露出 `preset_bundle.realistic` 这种原文，像没做完的半成品。
    for (const id of PRESET_BUNDLES.map((bundle) => bundle.id)) {
      expect(zh[`preset_bundle.${id}`]).toBeTruthy();
      expect(en[`preset_bundle.${id}`]).toBeTruthy();
    }
    expect(zh["preset_bundle.group"]).toBeTruthy();
    expect(en["preset_bundle.group"]).toBeTruthy();
  });
});

describe("preset stack narrowing", () => {
  it("keeps the order the user clicked in", () => {
    // 叠起来的几条是有先后的：用户先点「写实渲染」再点「微细结构」，
    // 说明在他心里前者是主规矩。顺序不该被排序抹平。
    expect(uniquePresetIds(["material", "realistic", "microdetail"])).toEqual([
      "material",
      "realistic",
      "microdetail",
    ]);
  });

  it("drops repeats and unknown ids but keeps everything it knows", () => {
    expect(uniquePresetIds(["realistic", "realistic", "photoshop", "occlusion"])).toEqual([
      "realistic",
      "occlusion",
    ]);
    expect(uniquePresetIds(["", "auto", "realistic"])).toEqual(["realistic"]);
    // "auto" 是界面里的「不限预设」，不是一个预设：不认得，就该被收窄掉。
    expect(uniquePresetIds(["auto"])).toEqual([]);
  });

  it("does not trim on its own: that call belongs to the caller", () => {
    const wanted = Array.from({ length: 4 }, (_, i) => PRESET_IDS[i]);
    // uniquePresetIds 只负责去重和过滤，报不报「叠满了」要调用方看着办——
    // 它裁完了，调用方就没法告诉用户多了哪一条，只剩一次没头没尾的失败。
    expect(uniquePresetIds(wanted)).toHaveLength(4);
    expect(normalizePresetStack(wanted)).toHaveLength(MAX_STACKED_PRESETS);
    expect(normalizePresetStack(uniquePresetIds(wanted))).toEqual(
      uniquePresetIds(wanted).slice(0, MAX_STACKED_PRESETS),
    );
  });

  it("never returns more than the cap, and survives junk input", () => {
    expect(normalizePresetStack(["realistic", "nope", "polish", "occlusion", "value"])).toEqual([
      "realistic",
      "polish",
      "occlusion",
    ]);
    expect(normalizePresetStack([])).toEqual([]);
    expect(normalizePresetStack(["nope"])).toEqual([]);
    expect(normalizePresetStack(PRESET_IDS)).toHaveLength(MAX_STACKED_PRESETS);
  });
});

describe("finish bundles", () => {
  it("spreads a bundle into the exact presets it stands for", () => {
    expect(expandPresetChoice([`${BUNDLE_PREFIX}realistic`])).toEqual([
      "realistic",
      "microdetail",
      "occlusion",
    ]);
  });

  it("replaces the current pick instead of merging into it", () => {
    // 合并很容易顶到三条上限，用户看到的是第四条莫名其妙没进去——
    // 他明明只点了一个套餐。
    expect(expandPresetChoice(["material", `${BUNDLE_PREFIX}detail`])).toEqual([
      "microdetail",
      "polish",
      "value",
    ]);
  });

  it("lets the last bundle win when several get clicked", () => {
    // 下拉底部那几个按钮挨在一起，连着点两下是「想换一套」，
    // 不是「想两套叠一起」。上一个赢，行为才可预期。
    expect(expandPresetChoice([`${BUNDLE_PREFIX}realistic`, `${BUNDLE_PREFIX}scene`])).toEqual([
      "cinematic",
      "depth",
      "composition",
    ]);
  });

  it("falls back to plain picks when the bundle id is unknown", () => {
    expect(expandPresetChoice([`${BUNDLE_PREFIX}nope`, "realistic"])).toEqual(["realistic"]);
    expect(expandPresetChoice(["nope"])).toEqual([]);
  });

  it("keeps plain clicking exactly as it was before bundles existed", () => {
    // 没点套餐时，展开只是收窄：去重、过滤、砍上限，顺序照旧。
    expect(expandPresetChoice(["realistic", "realistic", "occlusion", "bogus"])).toEqual([
      "realistic",
      "occlusion",
    ]);
    expect(expandPresetChoice([])).toEqual([]);
    expect(expandPresetChoice(["auto"])).toEqual([]);
  });

  it("carries only real preset ids, within the stack cap", () => {
    // 套餐最终要变成能发给 Rust 的清单：id 必须认得、条数不超上限，
    // 否则点一下套餐换来的是一次 unknown prompt preset 的报错。
    for (const bundle of PRESET_BUNDLES) {
      expect(bundle.presetIds.length).toBeLessThanOrEqual(MAX_STACKED_PRESETS);
      for (const id of bundle.presetIds) {
        expect(PRESET_IDS).toContain(id);
      }
    }
    expect(isPresetBundleId("realistic")).toBe(true);
    expect(isPresetBundleId("realistic-render")).toBe(false);
    expect(bundlePresetIds("nope")).toBeNull();
    expect(bundlePresetIds("scene")).toEqual(["cinematic", "depth", "composition"]);
  });
});
