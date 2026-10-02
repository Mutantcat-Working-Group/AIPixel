// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only

/** 一次按键想表达的意思。null = 这事不归快捷键管。 */
export type EditShortcut = "undo" | "redo" | null;

/**
 * 绘制快捷键的裁决表：Cmd+Z（Ctrl+Z）反悔，Cmd+Shift+Z 或 Ctrl+Y 重做。
 * 抽成纯函数是为了能离开 DOM 单测——键盘组合的分支比看上去多：平台修饰键
 * 不统一（Mac 的 meta 和 Windows 的 ctrl）、反悔和重做只差一个 shift、
 * 还有「正在敲字」这一整类要让开的情况。
 *
 * @param key      event.key，小写化由调用方负责
 * @param mods     meta / ctrl / shift / alt 四个键的状态
 * @param typing  目标是不是正在敲字的输入框或富文本
 */
export function editShortcut(
  key: string,
  mods: { meta: boolean; ctrl: boolean; shift: boolean; alt: boolean },
  typing: boolean,
): EditShortcut {
  // Alt 是输入法切字/切半角的键，抢它的组合键会把中文输入搅乱。
  if (mods.alt) return null;
  // Mac 的 meta 和 Windows 的 ctrl 都算主修饰键，缺一个就当普通敲字。
  if (!mods.meta && !mods.ctrl) return null;
  // 正在敲字：这一下是用户要撤销自己打的文字，不是画面。
  if (typing) return null;
  if (key === "y") return "redo";
  if (key !== "z") return null;
  return mods.shift ? "redo" : "undo";
}

/**
 * 目标是不是「正在敲字」的那类元素。输入框、文本域、下拉、富文本都算。
 * 用 closest 而不是看 tagName：antd 的 Select、ColorPicker 会把按键事件
 * 派到内部的 input 上，只看最外层 div 会漏。
 */
export function isTypingTarget(target: EventTarget | null): boolean {
  if (typeof HTMLElement === "undefined") return false;
  const node = target instanceof HTMLElement ? target : null;
  if (!node) return false;
  if (node.isContentEditable) return true;
  const tag = node.tagName;
  return (
    tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || tag === "OPTION"
  );
}
