// 坞里帧选择的取值：选中的帧还合法就留着，否则退回兜底，再不行退回第一帧。
// 帧 id 属于文档，换文档、删帧都会让旧选择失效；与其用同步 effect 回写，
// 不如在读取时推导一个必然合法的值，渲染永远不落空、draft 不被悄悄改写。

/**
 * 从候选帧里解析出一个合法帧 id。
 * @param frameIds 当前文档的帧 id，按帧顺序。
 * @param picked 用户此前的选择，可能已失效（对应帧被删、换了文档）。
 * @param fallback picked 失效时优先退回的帧 id，同样允许失效；null 表示无偏好。
 * @returns 必然存在于 frameIds 里的 id；一帧都没有时返回空串。
 */
export function resolveFrameId(
  frameIds: readonly string[],
  picked: string,
  fallback: string | null,
): string {
  if (frameIds.length === 0) return "";
  if (frameIds.includes(picked)) return picked;
  if (fallback !== null && frameIds.includes(fallback)) return fallback;
  return frameIds[0];
}
