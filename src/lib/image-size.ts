// 生图尺寸：模型只会给一整张位图，而画布常常是 64x48 这类小规格。
// 尺寸给得跟画布同比例，像素才不浪费在「反正要被裁掉的边」上。

/** 常见模型的固定档位。多数服务只认这几个，或只认 32 的整数倍。 */
export const SIZE_OPTIONS = [
  "512x512",
  "768x768",
  "1024x1024",
  "1024x576",
  "576x1024",
];

export type SizeMode = "preset" | "canvas" | "custom";

export interface SizeDraft {
  sizeMode: SizeMode;
  /** sizeMode 为 preset 时的档位。 */
  size: string;
  /** sizeMode 为 custom 时的手敲宽高。 */
  sizeW: number;
  sizeH: number;
}

/** 生图端点的尺寸粒度：不是 32 的倍数，不少兼容实现直接 400。 */
const SIZE_STEP = 32;
const SIZE_MIN = 256;
const SIZE_MAX = 1536;
const SIZE_LONG = 1024;

function snap(value: number): number {
  if (!Number.isFinite(value)) return SIZE_MIN;
  const stepped = Math.round(value / SIZE_STEP) * SIZE_STEP;
  return Math.min(SIZE_MAX, Math.max(SIZE_MIN, stepped));
}

/**
 * 按画布宽高比折一个尺寸：长边顶到 1024，两边各自 snap 到 32 的倍数。
 * 给定的宽高不合法时按方形走，别折出个 0。
 */
export function canvasAspectSize(width: number, height: number): string {
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
    return `${SIZE_LONG}x${SIZE_LONG}`;
  }
  const scale = SIZE_LONG / Math.max(width, height);
  return `${snap(width * scale)}x${snap(height * scale)}`;
}

/**
 * 把面板上的三种尺寸口径折成发给模型的 "WxH"。
 * canvas 口径要知道当前画布多大，所以文档尺寸是必需参数。
 */
export function resolveImageSize(
  draft: SizeDraft,
  canvasWidth: number,
  canvasHeight: number,
): string {
  switch (draft.sizeMode) {
    case "canvas":
      return canvasAspectSize(canvasWidth, canvasHeight);
    case "custom":
      return `${snap(draft.sizeW)}x${snap(draft.sizeH)}`;
    default:
      return draft.size;
  }
}
