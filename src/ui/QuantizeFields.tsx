// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 量化选项控件：工作流坞的量化面板、批量工作台的量化参数共用这一套。
// 抽出来是为了让两处的调色板 / 抖动 / 适配手感完全一致，而不是各写一份。

import type { ReactNode } from "react";
import { Segmented, Slider, Switch } from "antd";
import { Sliders } from "lucide-react";

import { useT } from "../lib/t";
import type { FitMode, PixelizeOptions } from "../lib/types";

export function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="dock-field">
      <span className="dock-field-label">{label}</span>
      {children}
    </div>
  );
}

export function QuantizeFields({
  value,
  onChange,
}: {
  value: PixelizeOptions;
  onChange: (next: PixelizeOptions) => void;
}) {
  const t = useT();
  return (
    <details className="dock-advanced">
      <summary>
        <Sliders size={12} />
        {t("dock.advanced")}
      </summary>
      <Field label={t("dock.max_colors", { count: value.max_colors })}>
        {/* 中位切分上限：位图里最多提取几种主色。给得小，颗粒感和色块都更「像素画」。 */}
        <Slider
          min={2}
          max={64}
          value={value.max_colors}
          onChange={(next) => onChange({ ...value, max_colors: next })}
        />
      </Field>
      <Field label={t("dock.alpha_cutoff", { count: value.alpha_threshold })}>
        {/* 半透明判官：alpha 低于它的输出像素直接算透明（索引 0），
            所以调低它能把 PNG 里朦胧的边缘救回来。 */}
        <Slider
          min={0}
          max={255}
          value={value.alpha_threshold}
          onChange={(next) => onChange({ ...value, alpha_threshold: next })}
        />
      </Field>
      <Field label={t("dock.snap_tolerance", { count: value.snap_tolerance })}>
        {/* 复用旧色的容差：新主色离画布已有颜色多近就直接并进去，不新增。
            拉到 0 等于不复用，每次量化都会把调色板撑大一轮。 */}
        <Slider
          min={0}
          max={128}
          value={value.snap_tolerance}
          onChange={(next) => onChange({ ...value, snap_tolerance: next })}
        />
      </Field>
      <div className="dock-flag">
        {/* 有序抖动（Bayer 4x4）：渐变色区的救命稻草，代价是画面起网格纹理。 */}
        <Switch
          size="small"
          checked={value.dither}
          onChange={(next) => onChange({ ...value, dither: next })}
        />
        <span>{t("dock.dither")}</span>
      </div>
      <div className="dock-flag">
        {/* 允许新增色板项：关掉之后找不到近似色的主色会被并进已有颜色，
            调色板永远不涨——适合已经定好色数的项目。 */}
        <Switch
          size="small"
          checked={value.expand_palette}
          onChange={(next) => onChange({ ...value, expand_palette: next })}
        />
        <span>{t("dock.expand_palette")}</span>
      </div>
      <Field label={t("dock.fit")}>
        <Segmented
          size="small"
          block
          value={value.fit}
          options={[
            { label: t("fit.contain"), value: "contain" },
            { label: t("fit.stretch"), value: "stretch" },
          ]}
          onChange={(next) => onChange({ ...value, fit: next as FitMode })}
        />
      </Field>
    </details>
  );
}
