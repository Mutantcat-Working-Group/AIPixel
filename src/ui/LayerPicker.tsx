import { useT } from "../lib/t";
import { resolveLayerId } from "../lib/layer-pick";
import type { PixelDocument } from "../lib/types";

/**
 * 坞里的图层选择：一叠图层里挑一个当落点。
 *
 * 只有一层时不渲染：那种情况下选择器是纯噪音，落点就是那一层。
 * 空字符串等于「激活图层」，和 Rust 侧 `layer: Option<&str>` 的 None 对齐。
 */
export default function LayerPicker({
  document: doc,
  value,
  onChange,
}: {
  document: PixelDocument | null;
  value: string;
  onChange: (layerId: string) => void;
}) {
  const t = useT();
  if (!doc || doc.layers.length < 2) return null;
  const resolved = resolveLayerId(
    doc.layers.map((layer) => layer.id),
    value || null,
  );

  return (
    <div className="frame-picker">
      <button
        type="button"
        className={`frame-pick layer-pick ${resolved === "" ? "active" : ""}`}
        aria-pressed={resolved === ""}
        onClick={() => onChange("")}
      >
        <span className="layer-pick-id">{t("dock.active_layer")}</span>
      </button>
      {doc.layers.map((layer, index) => (
        <button
          key={layer.id}
          type="button"
          className={`frame-pick layer-pick ${resolved === layer.id ? "active" : ""}`}
          aria-pressed={resolved === layer.id}
          onClick={() => onChange(layer.id)}
        >
          <span className="layer-pick-name">{layer.name}</span>
          <span className="layer-pick-id">{index + 1}</span>
        </button>
      ))}
    </div>
  );
}
