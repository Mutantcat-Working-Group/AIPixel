// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { useEffect } from "react";

import { editShortcut, isTypingTarget } from "../lib/shortcuts";
import { useStore } from "../lib/store";

/**
 * 撤销 / 重做的键盘入口：Cmd+Z（Ctrl+Z）反悔，Cmd+Shift+Z 或 Ctrl+Y 重做。
 *
 * 一条笔画下去画布就变，点错一下要靠鼠标去戳右上角那两个小方块太远，满手
 * 颜料的时候尤其别扭。和别的绘图工具同一个手感才是对的。
 *
 * 组合键怎么判、什么情况要让开，都在 lib/shortcuts 里写清了也能单测；这里
 * 只负责把裁决结果接到 store 上。
 */
export default function useEditShortcuts(): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const action = editShortcut(
        event.key.toLowerCase(),
        {
          meta: event.metaKey,
          ctrl: event.ctrlKey,
          shift: event.shiftKey,
          alt: event.altKey,
        },
        isTypingTarget(event.target),
      );
      if (action === null) return;
      // 没有可走的那一步就别吞按键：让浏览器和系统按自己的规矩处理。
      const state = useStore.getState();
      if (action === "undo" && state.undoStack.length === 0) return;
      if (action === "redo" && state.redoStack.length === 0) return;
      event.preventDefault();
      if (action === "undo") void state.undoEdit();
      else void state.redoEdit();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);
}
