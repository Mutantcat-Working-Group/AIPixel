// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 数字输入加一个单位后缀。
//
// antd 的 InputNumber.addonAfter 已经标记弃用，继续用它每渲染一次就往控制台
// 刷一条 warning，用户在开发者工具里看见只会以为项目没维护。这里用 Space.Compact
// 跟一个静态后缀拼出同样的观感，不是新交互，只是不再刷屏。

import type { ReactNode } from "react";
import { InputNumber, Space } from "antd";

export function NumberWithUnit({
  value,
  onChange,
  min,
  max,
  unit,
  fallback,
}: {
  value: number;
  onChange: (next: number) => void;
  min?: number;
  max?: number;
  unit: ReactNode;
  /** 输入被清空时的回落值。各个字段原来填的是什么就传什么。 */
  fallback: number;
}) {
  return (
    <Space.Compact style={{ width: "100%" }}>
      <InputNumber
        size="small"
        style={{ width: "100%" }}
        min={min}
        max={max}
        value={value}
        onChange={(next) => onChange(next ?? fallback)}
      />
      <span className="number-unit">{unit}</span>
    </Space.Compact>
  );
}
