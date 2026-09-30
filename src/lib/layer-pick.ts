// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 坞里图层选择的取值，和 frame-pick.ts 同一个道理：图层 id 属于文档，
// 换会话、删图层都会让旧选择失效。读取时推导必然合法的值，不用同步 effect 回写。

/**
 * 从候选图层里解析出一个合法图层 id。
 * @param layerIds 当前文档的图层 id，按图层顺序。
 * @param picked 用户此前的选择；空串表示「激活图层」，原样返回。
 * @returns 空串，或一个必然存在于 layerIds 里的 id。
 */
export function resolveLayerId(layerIds: readonly string[], picked: string | null): string {
  if (picked === null || picked === "") return "";
  return layerIds.includes(picked) ? picked : "";
}
