// 批量工作台的纯逻辑：recipe 默认值、事件流折叠成可渲染快照。
// 这些函数不碰 Tauri、不碰 React，方便单测；副作用都收到 store 的 action 里。

import type {
  BatchEvent,
  BatchItemState,
  BatchRecipe,
  BatchScan,
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

/** 进度百分比 0..100；没有 total 时归零，交给 UI 当 indeterminate。 */
export function batchPercent(run: BatchRun): number {
  if (run.total === 0) return 0;
  return Math.min(100, Math.round((run.done / run.total) * 100));
}

/** 换 kind 时要不要重扫。目录没变也能复用旧扫描——但要按新 kind 才算数。 */
export function scanMatchesKind(scan: BatchScan | null, kind: BatchRecipe["kind"]): boolean {
  return !!scan && scan.kind === kind;
}
