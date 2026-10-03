// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
// 几处「删了就找不回来」的动作，动手之前必须挡一道：
// 会话（聊天记录加画布一起没）、模型定义（API Key 跟着没）、MCP 服务器（工具整箱没）、
// 关窗时不保存就退出（没写进 .aip 的改动当场没）。
// 判据是「动作只能长在确认控件里」——按源码原文比对，不碰 DOM。
// 删帧删图层不在此列：那些走编辑器 op，落撤销栈，一步 undo 就回来了。

import { describe, expect, it } from "vitest";

import { en, zh } from "../lib/i18n";

/** 界面源码原文：glob 在构建期展开，跑测试时就是一堆字符串。 */
const SOURCES = import.meta.glob("./*.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

function sourceOf(name: string): string {
  const hit = Object.entries(SOURCES).find(([path]) => path.endsWith("/" + name));
  if (!hit) throw new Error("没找到界面源码：" + name);
  return hit[1];
}

/**
 * 取 <Modal ...> 到配对 </Modal>（或 Popconfirm）之间那一整段。
 * 这两种控件在本仓都不嵌套，所以不必配平，先开后合就够。
 */
function guardBlock(source: string, guard: string): string {
  const open = source.indexOf("<" + guard);
  expect(open, guard + " 的开标签没找到").toBeGreaterThanOrEqual(0);
  const close = source.indexOf("</" + guard + ">", open);
  expect(close, guard + " 的合标签没找到").toBeGreaterThan(open);
  return source.slice(open, close);
}

/**
 * 破坏性动作表。call 是确认控件里实际动手那一句；
 * action 是动作名，用来在确认控件之外找漏网的调用点。
 */
const SITES = [
  {
    file: "SessionSidebar.tsx",
    guard: "Modal",
    action: "removeSession",
    call: "removeSession(target.id)",
    label: "sidebar.delete_title",
    ok: "sidebar.delete_ok",
    cancel: "sidebar.modal_cancel",
  },
  {
    file: "ModelSettingsModal.tsx",
    guard: "Popconfirm",
    action: "removeModel",
    call: "void removeModel(selected.id)",
    label: "settings.delete_confirm",
    ok: "settings.delete_ok",
    cancel: "settings.delete_cancel",
  },
  {
    // 关窗时不保存退出：没写进 .aip 的改动当场没了，所以「直接走」这个动作
    // 只能在问询弹窗里发生，弹窗之外不能再有第二个入口。
    file: "CloseGuardModal.tsx",
    guard: "Modal",
    action: "discardQuit",
    call: "discardQuit",
    label: "close.title",
    ok: "close.save_quit",
    cancel: "close.discard_quit",
  },
  {
    file: "McpSection.tsx",
    guard: "Popconfirm",
    action: "removeMcpServer",
    call: "void removeMcpServer(server.name)",
    label: "mcp.delete_confirm",
    ok: "mcp.delete_ok",
    cancel: "mcp.delete_cancel",
  },
] as const;

describe("删档动作只能长在确认控件里", () => {
  for (const site of SITES) {
    it(site.file + "：" + site.action + " 只从 " + site.guard + " 里动手", () => {
      const source = sourceOf(site.file);
      const block = guardBlock(source, site.guard);

      expect(block).toContain(site.call);
      // 确认控件之外不能再有第二个调用点：侧栏的删除按钮和右键菜单
      // 现在都只负责开窗，不直接删。
      const outside = source.replace(block, "");
      expect(outside, site.action + " 在确认控件之外还有调用点").not.toContain(
        site.action + "(",
      );
    });

    it(site.file + "：确认控件问清了后果，也给了取消", () => {
      const block = guardBlock(sourceOf(site.file), site.guard);
      expect(block).toContain(site.label);
      expect(block).toContain('t("' + site.ok + '")');
      expect(block).toContain('t("' + site.cancel + '")');
      // 都是危险操作，红钮不能少。红钮长在确定还是取消上不设限：
      // 关窗问询里危险的那下是「不保存退出」，它是取消键。
      expect(block).toMatch(/ButtonProps=\{\{[^}]*danger: true/);
    });

    it(site.label + " 两种语言都写了字", () => {
      expect(zh[site.label]).toBeTruthy();
      expect(en[site.label]).toBeTruthy();
    });
  }
});
