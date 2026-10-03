// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import type { ReactNode } from "react";

import { Button, Checkbox, Input, InputNumber, Modal, Tooltip } from "antd";

import { isSheetGrid } from "../lib/sheet";

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

/** RPG Maker 角色行走图的固定网格。行恒为 4，列 3（VX / Ace / MV / MZ）
 *  或列 4（XP），格子按代际分别是 48px 与 32px——引擎是按行列号直接切图的，
 *  所以这几个尺寸不是「建议」而是契约：差一格，导出的图在游戏里错位一行。 */
export const SHEET_PRESETS: { w: number; h: number; label: string; cell: string }[] = [
  { w: 144, h: 192, label: "144×192", cell: "MV/MZ" },
  { w: 96, h: 128, label: "96×128", cell: "VX/Ace" },
  { w: 128, h: 128, label: "128×128", cell: "XP" },
  { w: 72, h: 128, label: "72×128", cell: "24px 格" },
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
  sheetPresetsLabel: string;
  /** 白膜勾选那一栏。给了 label 才显示：改尺寸用不到它。 */
  paperdollLabel?: string;
  paperdollTip?: string;
  paperdollNeedSheet?: string;
  paperdoll?: boolean;
  /** 已经插值好的读数：宽高与格子数由调用方按自己的语言拼好再传进来。 */
  readout: string;
  /** 会话名那一栏。给了 label 才显示：改尺寸用不到名字。 */
  nameLabel?: string;
  namePlaceholder?: string;
  name?: string;
  hint?: ReactNode;
  onChange: (width: number, height: number) => void;
  onPaperdollChange?: (checked: boolean) => void;
  onNameChange?: (name: string) => void;
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
      {props.nameLabel ? (
        <label className="size-name">
          <span className="k">{props.nameLabel}</span>
          {/* 可留空：空名由后端给默认编号 s1、s2……，顶上还提示着最大宽度。
              所以 placeholder 就把「不填会怎样」说清，别只说「请输入」。 */}
          <Input
            autoFocus
            value={props.name ?? ""}
            placeholder={props.namePlaceholder}
            onChange={(event) => props.onNameChange?.(event.target.value)}
            onPressEnter={props.onOk}
          />
        </label>
      ) : null}
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
        <span className="size-presets-label">{props.sheetPresetsLabel}</span>
        <div className="size-preset-row">
          {SHEET_PRESETS.map((preset) => (
            // 档位名贴在尺寸后面：同一个「144x192」，MV/MZ 和 XP 是两代引擎，
            // 选错了等于白铺。字号小一档，只作脚注不作标题。
            <Button
              key={preset.label}
              size="small"
              type={preset.w === width && preset.h === height ? "primary" : "default"}
              onClick={() => props.onChange(preset.w, preset.h)}
            >
              {preset.label} <em className="size-preset-note">{preset.cell}</em>
            </Button>
          ))}
        </div>
      </div>

      {props.paperdollLabel ? (
        // 白膜勾选：只在新建会话出现。尺寸不是角色行走图网格时置灰——
        // 后端会拒，但让用户点完才吃一张红字提示，不如在这儿就说明白。
        <Tooltip title={isSheetGrid(width, height) ? (props.paperdollTip ?? "") : (props.paperdollNeedSheet ?? "")}>
          <span className="size-paperdoll">
            <Checkbox
              checked={props.paperdoll ?? false}
              disabled={!isSheetGrid(width, height)}
              onChange={(event) => props.onPaperdollChange?.(event.target.checked)}
            >
              {props.paperdollLabel}
            </Checkbox>
          </span>
        </Tooltip>
      ) : null}

      {props.hint ? <p className="size-hint">{props.hint}</p> : null}
    </Modal>
  );
}
