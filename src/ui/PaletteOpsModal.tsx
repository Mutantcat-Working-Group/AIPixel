// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 配色范围上三件「会动画面」的事，落文档之前都要用户把话说清楚：
// 换范围（这一层像素按就近色重排）、删色（用到的像素得有个接手色）、
// 删整套（引用它的层得有个接盘范围）。三件事共用这一个弹窗：
// 都是先说清后果，再选一个接手对象，最后才动文档。

import { useState } from "react";
import { Modal, Radio, Select } from "antd";

import { useStore } from "../lib/store";
import { useT } from "../lib/t";
import { colorName } from "../lib/colornames";

/** 删整套时的接盘候选：文档里除自己以外的范围。 */
export interface PaletteChoice {
  id: string;
  name: string;
  /** 前几个色，候选里一眼看出是哪一套。 */
  dots: string[];
}

export type PaletteOpRequest =
  | {
      kind: "switch";
      layerId: string;
      layerName: string;
      fromName: string;
      toId: string;
      toName: string;
    }
  | {
      kind: "remove";
      paletteId: string;
      paletteName: string;
      index: number;
      /** 被删的那个色：画面里用到它的像素要改写。 */
      hex: string;
      /** 能当接手色的：当前范围里剩下的颜色。 */
      choices: string[];
    }
  | { kind: "delete"; paletteId: string; paletteName: string; choices: PaletteChoice[] };

export interface PaletteOpsModalProps {
  request: PaletteOpRequest | null;
  onClose: () => void;
  /** 换范围：这一层改指过去。 */
  onSwitch: (layerId: string, toId: string) => void;
  /**
   * 删色：原样带回这一问本身，调用方从里面拿 paletteId 和 index，
   * 顺手能拿到用户选的接手色。少一层参数拷贝，也少一个对不上的机会。
   */
  onRemove: (request: Extract<PaletteOpRequest, { kind: "remove" }>, replacement: string) => void;
  /** 删整套：引用它的层先改指哪一套。 */
  onDelete: (request: Extract<PaletteOpRequest, { kind: "delete" }>, fallbackId: string) => void;
}

export default function PaletteOpsModal(props: PaletteOpsModalProps) {
  const { request } = props;
  const t = useT();
  const lang = useStore((s) => s.lang);
  // 接手对象跟着这一问走：每次开窗都是新的一问，不能留着上一次的答案。
  const [replacement, setReplacement] = useState("");
  const [fallback, setFallback] = useState("");
  // 记下这一问。换了新的一问就在渲染里直接改接手对象（React 认可的「属性变则
  // 调状态」写法），不走 useEffect：那会先拿旧值渲染一帧再改，白闪一下。
  const [seen, setSeen] = useState<PaletteOpRequest | null>(null);
  if (request !== seen) {
    setSeen(request);
    if (request?.kind === "remove") setReplacement(request.choices[0] ?? "");
    if (request?.kind === "delete") setFallback(request.choices[0]?.id ?? "");
  }

  if (!request) return null;

  const title =
    request.kind === "switch"
      ? t("palette.switch_title")
      : request.kind === "remove"
        ? t("palette.remove_title")
        : t("palette.delete_title");
  const okText =
    request.kind === "delete" ? t("palette.delete_ok") : t("palette.confirm_ok");
  // 没有接手对象就不能确认：删色删到空范围、删整套删出悬空引用，都是静默毁画面。
  const blocked =
    request.kind === "remove" ? replacement === "" : request.kind === "delete" ? fallback === "" : false;

  return (
    <Modal
      open
      title={title}
      okText={okText}
      cancelText={t("palette.confirm_cancel")}
      okButtonProps={{ disabled: blocked }}
      onOk={() => {
        if (request.kind === "switch") props.onSwitch(request.layerId, request.toId);
        else if (request.kind === "remove" && replacement !== "") {
          props.onRemove(request, replacement);
        } else if (request.kind === "delete" && fallback !== "") {
          props.onDelete(request, fallback);
        }
      }}
      onCancel={props.onClose}
      destroyOnHidden
      width={460}
      className="palette-ops-modal"
    >
      {request.kind === "switch" ? (
        <p className="palette-ops-warn">
          {t("palette.switch_warn", {
            layer: request.layerName,
            from: request.fromName,
            to: request.toName,
          })}
        </p>
      ) : null}

      {request.kind === "remove" ? (
        <>
          <p className="palette-ops-warn">
            {t("palette.remove_warn", {
              name: request.paletteName,
              color: colorName(request.hex, lang),
            })}
          </p>
          <span className="palette-ops-label">{t("palette.remove_pick")}</span>
          {request.choices.length === 0 ? (
            <p className="palette-ops-empty">{t("palette.remove_no_choice")}</p>
          ) : (
            <Radio.Group
              className="palette-ops-choices"
              value={replacement}
              onChange={(event) => setReplacement(event.target.value)}
            >
              {request.choices.map((hex) => (
                <Radio.Button key={hex} value={hex} className="palette-ops-choice">
                  <i className="palette-ops-dot" style={{ background: hex }} />
                  {colorName(hex, lang)}
                </Radio.Button>
              ))}
            </Radio.Group>
          )}
        </>
      ) : null}

      {request.kind === "delete" ? (
        <>
          <p className="palette-ops-warn">
            {t("palette.delete_warn", { name: request.paletteName })}
          </p>
          <span className="palette-ops-label">{t("palette.delete_pick")}</span>
          <Select
            className="palette-ops-select"
            value={fallback === "" ? undefined : fallback}
            onChange={(value) => setFallback(value)}
            options={request.choices.map((choice) => ({
              value: choice.id,
              label: (
                <span className="palette-ops-option">
                  <span className="palette-ops-dots">
                    {choice.dots.map((hex, index) => (
                      <i key={`${hex}-${index}`} style={{ background: hex }} />
                    ))}
                  </span>
                  {choice.name}
                </span>
              ),
            }))}
          />
        </>
      ) : null}
    </Modal>
  );
}
