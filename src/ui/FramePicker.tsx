import { Tooltip } from "antd";

import FrameThumb from "./FrameThumb";
import { useT } from "../lib/t";
import type { PixelDocument } from "../lib/types";

/**
 * 坞里的帧选择：一格一帧、缩略图在上序号在下，横向滚动。
 *
 * 这里刻意不放成下拉或分段按钮：抽帧和补间动辄产出几十帧，侧边栏只有
 * 330px 宽，别的形态一多就挤成一团。缩略图是帧的唯一直观标识——
 * 用户要选的是「第几帧长什么样」，不是帧号。
 * 渲染与编辑器帧条同源（compositeFrame），所以缩略图就是帧本身。
 */
export default function FramePicker({
  document: doc,
  value,
  onChange,
}: {
  document: PixelDocument | null;
  value: string;
  onChange: (frameId: string) => void;
}) {
  const t = useT();
  if (!doc) return null;

  return (
    <div className="frame-picker">
      {doc.frames.map((frame, index) => (
        <Tooltip
          key={frame.id}
          title={t("dock.frame_chip", { index: index + 1, ms: frame.duration_ms })}
        >
          <button
            type="button"
            className={`frame-pick ${frame.id === value ? "active" : ""}`}
            aria-pressed={frame.id === value}
            onClick={() => onChange(frame.id)}
          >
            <FrameThumb document={doc} index={index} compact />
            <span className="frame-pick-id">{index + 1}</span>
          </button>
        </Tooltip>
      ))}
    </div>
  );
}
