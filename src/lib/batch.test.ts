// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 批量工作台的纯函数：事件流怎么折成快照、什么情况下才放行开始。
// 这里不碰 Tauri，所以流的每一步都能在原地断言。

import { describe, expect, it } from "vitest";

import {
  batchPercent,
  canRunBatch,
  canSaveRecipe,
  DEFAULT_BATCH_RECIPE,
  DEFAULT_PIXELIZE_OPTIONS,
  EMPTY_BATCH_RUN,
  MAX_RECIPE_NAME_CHARS,
  MAX_RECIPES,
  reduceBatchEvent,
  recipeImportNotables,
  recipeNameProblem,
  tallyRecipeImport,
  scanMatchesKind,
} from "./batch";
import type { BatchRecipeEntry, BatchScan, RecipeImportRow } from "./types";

function scan(over: Partial<BatchScan> = {}): BatchScan {
  return {
    kind: "quantize",
    dir: "/tmp/in",
    count: 3,
    truncated: false,
    files: ["/tmp/in/a.png", "/tmp/in/b.png", "/tmp/in/c.png"],
    ...over,
  };
}

describe("reduceBatchEvent", () => {
  it("opens a run on started and throws away the previous log", () => {
    const first = reduceBatchEvent(EMPTY_BATCH_RUN, { kind: "started", total: 4 });
    expect(first.running).toBe(true);
    expect(first.total).toBe(4);
    expect(first.rows).toEqual([]);

    // 上一趟的余行不该带进下一趟：Started 是新一轮的起点。
    const dirty = {
      ...EMPTY_BATCH_RUN,
      rows: [{ file: "old.png", state: "ok", note: "" } as const],
    };
    const second = reduceBatchEvent(dirty, { kind: "started", total: 2 });
    expect(second.rows).toEqual([]);
  });

  it("appends one row per progress event and counts it as done", () => {
    let run = reduceBatchEvent(EMPTY_BATCH_RUN, { kind: "started", total: 3 });
    run = reduceBatchEvent(run, {
      kind: "progress",
      index: 0,
      total: 3,
      file: "a.png",
      state: "ok",
      note: "12 colors, 3 new",
    });
    run = reduceBatchEvent(run, {
      kind: "progress",
      index: 1,
      total: 3,
      file: "b.png",
      state: "error",
      note: "cannot read: broken",
    });
    expect(run.rows.map((row) => row.file)).toEqual(["a.png", "b.png"]);
    expect(run.rows[1].state).toBe("error");
    expect(run.done).toBe(2);
    expect(run.running).toBe(true);
  });

  it("seals the summary on done and keeps the rows for inspection", () => {
    let run = reduceBatchEvent(EMPTY_BATCH_RUN, { kind: "started", total: 2 });
    run = reduceBatchEvent(run, {
      kind: "progress",
      index: 0,
      total: 2,
      file: "a.png",
      state: "ok",
      note: "",
    });
    run = reduceBatchEvent(run, {
      kind: "done",
      batch_kind: "quantize",
      ok: 1,
      skipped: 1,
      failed: 0,
      output_dir: "/tmp/out",
    });
    expect(run.running).toBe(false);
    expect(run.rows).toHaveLength(1);
    expect(run.summary).toEqual({
      ok: 1,
      skipped: 1,
      failed: 0,
      outputDir: "/tmp/out",
    });
  });

  it("keeps batch_kind on the wire while the snapshot speaks camelCase", () => {
    // Rust 那边叫 batch_kind，折叠出来的快照只取计数与输出目录，不按 kind 分支。
    const done = {
      kind: "done",
      batch_kind: "export",
      ok: 0,
      skipped: 0,
      failed: 2,
      output_dir: "/tmp/out",
    } as const;
    const run = reduceBatchEvent(EMPTY_BATCH_RUN, done);
    expect(run.summary?.outputDir).toBe("/tmp/out");
  });
});

describe("canRunBatch", () => {
  it("refuses to run twice at once", () => {
    const running = { ...EMPTY_BATCH_RUN, running: true, total: 3, done: 1 };
    expect(canRunBatch(scan(), running)).toBe(false);
  });

  it("needs a scan with something in it", () => {
    expect(canRunBatch(null, EMPTY_BATCH_RUN)).toBe(false);
    expect(canRunBatch(scan({ count: 0 }), EMPTY_BATCH_RUN)).toBe(false);
    expect(canRunBatch(scan({ count: 2 }), EMPTY_BATCH_RUN)).toBe(true);
  });
});

describe("batchPercent", () => {
  it("is zero before anything is known so the bar can stay indeterminate", () => {
    expect(batchPercent(EMPTY_BATCH_RUN)).toBe(0);
  });

  it("rounds the ratio and never leaves 100", () => {
    const run = { ...EMPTY_BATCH_RUN, total: 3, done: 1, rows: [] };
    expect(batchPercent(run)).toBe(33);
    expect(batchPercent({ ...run, done: 3 })).toBe(100);
    expect(batchPercent({ ...run, done: 9 })).toBe(100);
  });
});

describe("scanMatchesKind", () => {
  it("only trusts a scan taken for the recipe at hand", () => {
    expect(scanMatchesKind(scan({ kind: "quantize" }), "quantize")).toBe(true);
    expect(scanMatchesKind(scan({ kind: "quantize" }), "export")).toBe(false);
    expect(scanMatchesKind(null, "quantize")).toBe(false);
  });
});

describe("defaults", () => {
  it("mirrors the Rust PixelizeOptions default so both sides start identically", () => {
    expect(DEFAULT_PIXELIZE_OPTIONS).toEqual({
      max_colors: 32,
      snap_tolerance: 12,
      expand_palette: true,
      dither: false,
      alpha_threshold: 128,
      fit: "contain",
    });
  });

  it("starts on quantize with an empty pair of folders", () => {
    expect(DEFAULT_BATCH_RECIPE.kind).toBe("quantize");
    expect(DEFAULT_BATCH_RECIPE.input_dir).toBe("");
    expect(DEFAULT_BATCH_RECIPE.output_dir).toBe("");
    expect(DEFAULT_BATCH_RECIPE.match_source_size).toBe(true);
    expect(DEFAULT_BATCH_RECIPE.export_format).toBe("png");
  });
});

describe("配方名（存之前先问一遍）", () => {
  const book: BatchRecipeEntry[] = [{ name: "sword-32", recipe: DEFAULT_BATCH_RECIPE }];

  it("空名字和只有空格的名字都算没起", () => {
    expect(recipeNameProblem("", book)).toBe("empty");
    expect(recipeNameProblem("   ", book)).toBe("empty");
    expect(canSaveRecipe("  ", book)).toBe(false);
  });

  it("按字符数算长度，中文名不吃亏", () => {
    expect(recipeNameProblem("字".repeat(MAX_RECIPE_NAME_CHARS), book)).toBeNull();
    expect(recipeNameProblem("字".repeat(MAX_RECIPE_NAME_CHARS + 1), book)).toBe("long");
  });

  it("斜杠和控制字符都拒掉：名字不能看着像路径", () => {
    expect(recipeNameProblem("a/b", book)).toBe("chars");
    expect(recipeNameProblem("a\\b", book)).toBe("chars");
    expect(recipeNameProblem("a\nb", book)).toBe("chars");
  });

  it("同名永远放行：簿子满了也能改旧配方", () => {
    const full: BatchRecipeEntry[] = Array.from({ length: MAX_RECIPES }, (_, index) => ({
      name: `配方${index}`,
      recipe: DEFAULT_BATCH_RECIPE,
    }));
    expect(recipeNameProblem("配方7", full)).toBeNull();
    expect(recipeNameProblem("新名字", full)).toBe("full");
    expect(canSaveRecipe("配方7", full)).toBe(true);
    expect(canSaveRecipe("新名字", full)).toBe(false);
  });
});

describe("配方导入回执", () => {
  const row = (
    name: string,
    state: RecipeImportRow["state"],
    finalName = "",
    note = "",
  ): RecipeImportRow => ({ name, final_name: finalName, state, note });

  it("按三条路分别点数，空回执归零", () => {
    expect(tallyRecipeImport([])).toEqual({ imported: 0, renamed: 0, skipped: 0 });
    expect(
      tallyRecipeImport([
        row("sword", "imported", "sword"),
        row("shield", "imported", "shield"),
        row("sword", "renamed", "sword (2)"),
        row("#3", "skipped", "", "missing field `kind`"),
      ]),
    ).toEqual({ imported: 2, renamed: 1, skipped: 1 });
  });

  it("只把改名和跳过拎出来：顺利进来的不逐条复述", () => {
    const rows = [
      row("sword", "imported", "sword"),
      row("sword", "renamed", "sword (2)"),
      row("bad/name", "skipped", "", "invalid name"),
    ];
    const notables = recipeImportNotables(rows);
    expect(notables.map((item) => item.name)).toEqual(["sword", "bad/name"]);
    expect(notables[0].final_name).toBe("sword (2)");
  });
});
