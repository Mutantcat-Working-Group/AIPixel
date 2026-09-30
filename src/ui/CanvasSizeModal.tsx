// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import type { ReactNode } from "react";

import { Button, InputNumber, Modal } from "antd";

/** 像素画的常用起步尺寸。点一下就把 W/H 填好，用户仍然能手改。 */
export const SIZE_PRESETS: { w: number; h: number; label: string }[] = [
  { w: 16, h: 16, label: "16×16" },
  { w: 32, h: 32, label: "32×32" },
  { w: 48, h: 48, label: "48×48" },
  { w: 64, h: 64, label: "64×64" },
  { w: 96, h: 96, label: "96×96" },
  { w: 128, h: 128, label: "128×128" },
  { w: 32, h: 16, label: "32×16" },
  { w: 64, h: 32, label: "64×32" },
  { w: 128, h: 64, label: "128×64" },
  { w: 320, h: 180, label: "320×180" },
];

export interface CanvasSizeModalProps {
  open: boolean;
  width: number;
  height: number;
  title: string;
  okText: string;
  cancelText: string;
  widthLabel: string;
  heightLabel: string;
  presetsLabel: string;
  /** 已经插值好的读数：宽高与格子数由调用方按自己的语言拼好再传进来。 */
  readout: string;
  hint?: ReactNode;
  onChange: (width: number, height: number) => void;
  onOk: () => void;
  onCancel: () => void;
}

/**
 * 选画布宽高的弹窗。新建会话与「左上角 WxH 改尺寸」共用一份：
 * 两处的输入项、预览、预设本就该一模一样，拆成两个只会各自长歪。
 * 文案由调用方给：新建会话和改尺寸说的话不一样（一个讲起步，一个讲裁剪）。
 */
export default function CanvasSizeModal(props: CanvasSizeModalProps) {
  const { open, width, height } = props;
  return (
    <Modal
      title={props.title}
      open={open}
      okText={props.okText}
      cancelText={props.cancelText}
      onOk={props.onOk}
      onCancel={props.onCancel}
      destroyOnHidden
      width={460}
      className="size-modal"
    >
      <div className="size-modal-body">
        <div className="size-preview" style={{ aspectRatio: `${width} / ${height}` }} aria-hidden />
        <div className="size-fields">
          <label className="size-field">
            <span className="k">{props.widthLabel}</span>
            <InputNumber
              min={1}
              max={1024}
              value={width}
              onChange={(v) => props.onChange(v ?? 1, height)}
            />
          </label>
          <span className="size-times">×</span>
          <label className="size-field">
            <span className="k">{props.heightLabel}</span>
            <InputNumber
              min={1}
              max={1024}
              value={height}
              onChange={(v) => props.onChange(width, v ?? 1)}
            />
          </label>
        </div>
      </div>

      <div className="size-readout">{props.readout}</div>

      <div className="size-presets">
        <span className="size-presets-label">{props.presetsLabel}</span>
        <div className="size-preset-row">
          {SIZE_PRESETS.map((preset) => (
            <Button
              key={preset.label}
              size="small"
              type={preset.w === width && preset.h === height ? "primary" : "default"}
              onClick={() => props.onChange(preset.w, preset.h)}
            >
              {preset.label}
            </Button>
          ))}
        </div>
      </div>

      {props.hint ? <p className="size-hint">{props.hint}</p> : null}
    </Modal>
  );
}
