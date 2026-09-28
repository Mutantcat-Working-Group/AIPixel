// 批量工作台的纯逻辑：recipe 默认值、事件流折叠成可渲染快照。
// 这些函数不碰 Tauri、不碰 React，方便单测；副作用都收到 store 的 action 里。

import type {
  BatchEvent,
  BatchItemState,
  BatchRecipeEntry,
  BatchRecipe,
  BatchScan,
  RecipeImportRow,
  PixelizeOptions,
} from "./types";

export const DEFAULT_PIXELIZE_OPTIONS: PixelizeOptions = {
  max_colors: 32,
  snap_tolerance: 12,
  expand_palette: true,
  dither: false,
  alpha_threshold: 128,
  fit: "contain",
};

export const DEFAULT_BATCH_RECIPE: BatchRecipe = {
  kind: "quantize",
  input_dir: "",
  output_dir: "",
  options: { ...DEFAULT_PIXELIZE_OPTIONS },
  match_source_size: true,
  target_w: 64,
  target_h: 64,
  export_format: "png",
};

export interface BatchRow {
  file: string;
  state: BatchItemState;
  note: string;
}

/** 一次批量运行的可渲染快照。由事件流折叠而来，Idle 时是 EMPTY。 */
export interface BatchRun {
  running: boolean;
  total: number;
  done: number;
  rows: BatchRow[];
  summary: { ok: number; skipped: number; failed: number; outputDir: string } | null;
  error: string | null;
}

export const EMPTY_BATCH_RUN: BatchRun = {
  running: false,
  total: 0,
  done: 0,
  rows: [],
  summary: null,
  error: null,
};

export function reduceBatchEvent(run: BatchRun, raw: BatchEvent): BatchRun {
  switch (raw.kind) {
    case "started":
      return { running: true, total: raw.total, done: 0, rows: [], summary: null, error: null };
    case "progress": {
      // 追加一行而不是替换整表：进度是流式的，最新的就在表尾，用户能实时看到跑过谁。
      const rows = [...run.rows, { file: raw.file, state: raw.state, note: raw.note }];
      return { ...run, rows, done: rows.length };
    }
    case "done":
      return {
        ...run,
        running: false,
        summary: {
          ok: raw.ok,
          skipped: raw.skipped,
          failed: raw.failed,
          outputDir: raw.output_dir,
        },
      };
    default:
      return run;
  }
}

/** 该不该放行「开始」：没扫到文件、或正在跑，都不许点。 */
export function canRunBatch(scan: BatchScan | null, run: BatchRun): boolean {
  return !run.running && !!scan && scan.count > 0;
}

/** 配方名上限、配方簿容量上限。与 Rust 侧一一对应，前端先拦一遍省一次往返。 */
export const MAX_RECIPE_NAME_CHARS = 40;
export const MAX_RECIPES = 50;

/** 这个名字为什么存不下来；null = 可以存。 */
export type RecipeNameProblem = "empty" | "long" | "chars" | "full";

/**
 * 存之前先问一遍。和 Rust 侧 `validate_recipe_name` 是同一条线，
 * 但 Rust 才是最终关口：这里拦的是手滑，不是恶意。
 */
export function recipeNameProblem(
  name: string,
  book: BatchRecipeEntry[],
): RecipeNameProblem | null {
  const trimmed = name.trim();
  if (trimmed === "") return "empty";
  if ([...trimmed].length > MAX_RECIPE_NAME_CHARS) return "long";
  if (/[\u0000-\u001f\u007f-\u009f\u2028\u2029/\\]/.test(trimmed)) return "chars";
  // 同名覆盖永远放行：簿子满了也能改旧配方，这才叫改配方而不是加配方。
  if (book.some((entry) => entry.name === trimmed)) return null;
  if (book.length >= MAX_RECIPES) return "full";
  return null;
}

export function canSaveRecipe(name: string, book: BatchRecipeEntry[]): boolean {
  return recipeNameProblem(name, book) === null;
}

/** 导入回执的点算：三条路各落了多少条，拼一句总结就够了。 */
export interface RecipeImportTally {
  imported: number;
  renamed: number;
  skipped: number;
}

export function tallyRecipeImport(rows: RecipeImportRow[]): RecipeImportTally {
  const tally: RecipeImportTally = { imported: 0, renamed: 0, skipped: 0 };
  for (const row of rows) tally[row.state] += 1;
  return tally;
}

/** 只有改名和跳过值得占一行：顺顺利利进来的那一堆不需要逐条复述。 */
export function recipeImportNotables(rows: RecipeImportRow[]): RecipeImportRow[] {
  return rows.filter((row) => row.state !== "imported");
}

/** 进度百分比 0..100；没有 total 时归零，交给 UI 当 indeterminate。 */
export function batchPercent(run: BatchRun): number {
  if (run.total === 0) return 0;
  return Math.min(100, Math.round((run.done / run.total) * 100));
}

/** 换 kind 时要不要重扫。目录没变也能复用旧扫描——但要按新 kind 才算数。 */
export function scanMatchesKind(scan: BatchScan | null, kind: BatchRecipe["kind"]): boolean {
  return !!scan && scan.kind === kind;
}
