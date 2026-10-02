// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
/** 顶栏模型框与「没有可用模型」横幅的取值规则。
 *
 * 这两件事以前挤在同一个表达式里，代价是「还没有会话」被当成「没有模型」：
 * 用户明明在设置里配好了一个模型，只是在空会话状态下打开软件，顶栏写「未设置模型」，
 * 聊天区还挂一条让他再去设置的横幅——照着做也只会发现设置里早就有了。
 *
 * 会话是允许为空的（「当前没有会话，您可以创建一个」本身就是正常态），所以
 * 「没有会话」和「没有模型」必须是两个判断，不能互相顶着答到。
 */

/** 顶栏该显示哪个模型 id；一个都不该显示时返回 null。 */
export function resolveBoundModelId(
  /** 会话自己绑的模型；没有会话或会话没绑过就是 null。 */
  sessionModelId: string | null | undefined,
  /** 全局激活模型：设置里「设为激活」的那个，新会话默认拿它。 */
  activeModelId: string | null | undefined,
  /** 当前活在设置里的模型 id 列表，用来筛掉已经删掉的。 */
  ids: readonly string[],
): string | null {
  if (sessionModelId && ids.includes(sessionModelId)) return sessionModelId;
  if (activeModelId && ids.includes(activeModelId)) return activeModelId;
  return null;
}

/**
 * 要不要挂「还没有可用的模型」那条横幅。只有真的一个模型都没配才挂。
 *
 * 会话绑的模型不见了不归这里管：Rust 在删模型时会把会话改指到当前激活模型，
 * 顶栏也跟着回落，用户看到的是换了个模型名，而不是被教育一顿。
 */
export function needsModelBanner(modelCount: number): boolean {
  return modelCount === 0;
}

/**
 * 顶栏换模型该走哪条路。
 *
 * 有会话就只改这个会话的绑定；没有会话时这句话无处可绑，此时顶栏的下拉若是死的，
 * 用户在空态下就永远切不了默认模型——所以改走全局激活模型，让下一个新会话接着。
 */
export function modelChangeTarget(
  hasActiveSession: boolean,
): "session" | "active" {
  return hasActiveSession ? "session" : "active";
}
