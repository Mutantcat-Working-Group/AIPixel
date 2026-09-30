// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 工作流坞里「这条流程由谁跑」的判定。
// 单独给流程绑了模型就说「由 X 跑」；没绑、拿会话主模型兜底的必须讲明白是主模型跑的，
// 不能让用户以为背后还藏了个专属模型。抽帧、补间、量化全程本机，谁都不提。

import type { TKey, TVARS } from "./i18n";
import type { DockKind } from "./types";

/** 有模型在背后跑的流程，加 agent 主循环。 */
const MODEL_BACKED: ReadonlySet<DockKind> = new Set<DockKind>([
  "agent",
  "prompt_refine",
  "image_gen",
  "vision_brief",
  "video_brief",
]);

export type T = (key: TKey, vars?: TVARS) => string;

/** 这条流程实际由谁跑：detached=true 是单独绑的，false 是主模型兜底。 */
export interface ServedBy {
  label: string;
  detached: boolean;
}

export function isModelBacked(kind: DockKind): boolean {
  return MODEL_BACKED.has(kind);
}

export function servedLineText(served: ServedBy, t: T): string {
  return t(served.detached ? "roles.served" : "roles.served_fallback", {
    model: served.label,
  });
}
