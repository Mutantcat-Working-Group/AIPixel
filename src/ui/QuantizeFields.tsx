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
        <Slider
          min={2}
          max={64}
          value={value.max_colors}
          onChange={(next) => onChange({ ...value, max_colors: next })}
        />
      </Field>
      <Field label={t("dock.alpha_cutoff", { count: value.alpha_threshold })}>
        <Slider
          min={0}
          max={255}
          value={value.alpha_threshold}
          onChange={(next) => onChange({ ...value, alpha_threshold: next })}
        />
      </Field>
      <Field label={t("dock.snap_tolerance", { count: value.snap_tolerance })}>
        <Slider
          min={0}
          max={128}
          value={value.snap_tolerance}
          onChange={(next) => onChange({ ...value, snap_tolerance: next })}
        />
      </Field>
      <div className="dock-flag">
        <Switch
          size="small"
          checked={value.dither}
          onChange={(next) => onChange({ ...value, dither: next })}
        />
        <span>{t("dock.dither")}</span>
      </div>
      <div className="dock-flag">
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
