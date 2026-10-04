// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { memo, useState } from "react";

import CanvasSizeModal from "./CanvasSizeModal";
import { useT } from "../lib/t";
import { useStore } from "../lib/store";

/**
 * 新建会话弹窗。开机头一回（一条会话都没有）会自动递上来，平时由侧栏那个
 * + 唤起来——两条路共用同一份输入与文案，所以做成独立组件挂在 App 上，
 * 而不是塞在侧栏里：开机那次侧栏本身也该是空的。
 *
 * 状态留在本组件内而不是每次打开都重置：用户上一回选的 64×64 和会话名草稿
 * 都该留着，新建连环开才顺手。
 */
function CreateSessionModal() {
  const t = useT();
  const open = useStore((s) => s.createPromptOpen);
  const createSession = useStore((s) => s.createSession);
  const closeCreatePrompt = useStore((s) => s.closeCreatePrompt);

  // 像素画最稳的起步尺寸；用户改过就跟着改，关窗不重置。
  const [width, setWidth] = useState(64);
  const [height, setHeight] = useState(64);
  // 留空交给 Rust 自动编号（s1、s2……）。
  const [name, setName] = useState("");
  // 建完就铺一版纸娃娃白膜：只在角色行走图网格上有意义，勾选与否都行。
  const [paperdoll, setPaperdoll] = useState(false);

  async function submit() {
    // 先关再建：建会话要等 Rust 落盘，窗子多开一瞬只是挡着新画布。
    const trimmed = name.trim();
    await createSession(width, height, trimmed === "" ? undefined : trimmed);
    setName("");
    // 白膜要等文档落地之后再铺：layPaperdollBase 认的是当前活跃会话，
    // 建会话的那一刻它还没切过去。铺失败 Rust 会带原因报出来，
    // 由 store 转述——这里不静默吞掉。
    if (paperdoll) {
      await useStore.getState().layPaperdollBase();
      setPaperdoll(false);
    }
  }

  return (
    <CanvasSizeModal
      open={open}
      width={width}
      height={height}
      title={t("sidebar.modal_title")}
      okText={t("sidebar.modal_ok")}
      cancelText={t("sidebar.modal_cancel")}
      widthLabel={t("sidebar.width")}
      heightLabel={t("sidebar.height")}
      presetsLabel={t("sidebar.presets")}
      sheetPresetsLabel={t("sidebar.sheet_presets")}
      paperdollLabel={t("sidebar.paperdoll")}
      // 勾选的悬浮说明与工具栏那颗小人图标共用一套文案：说的是同一件事，
      // 拆成两套只会各自长歪。
      paperdollTip={t("doc.paperdoll_tip")}
      paperdollNeedSheet={t("doc.paperdoll_need_sheet")}
      paperdoll={paperdoll}
      onPaperdollChange={setPaperdoll}
      readout={t("sidebar.size_readout", { width, height, cells: width * height })}
      hint={t("sidebar.size_hint")}
      nameLabel={t("sidebar.name_label")}
      namePlaceholder={t("sidebar.name_placeholder")}
      name={name}
      onNameChange={setName}
      onChange={(w, h) => {
        setWidth(w);
        setHeight(h);
      }}
      onOk={submit}
      onCancel={closeCreatePrompt}
    />
  );
}

/** 弹窗只在用户点「新建」时才有内容，memo 一层挡掉父组件本不必要的连锁渲染。 */
export default memo(CreateSessionModal);
