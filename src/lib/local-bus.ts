// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
/** 页面内事件总线：Tauri 事件桥不可用时的兜底通道。
 *
 * 为什么需要它：`listen()` 内部读 `window.__TAURI_INTERNALS__.transformCallback`，
 * 缺了就抛。而 store 的启动流程是 await 着订阅的，一抛后面读模型、建会话全断，
 * 界面停在半个初始化状态——用户看到的就是「发一句话下去没反应」。
 * 有总线兜底，订阅至少不会散，浏览器预览里也能把事件送到该送的地方。
 */

type Handler = (payload: unknown) => void;

const channels = new Map<string, Set<Handler>>();

/** 订阅一条通道。返回退订函数，重复调用是安全的。 */
export function subscribeLocal(channel: string, handler: Handler): () => void {
  const handlers = channels.get(channel) ?? new Set<Handler>();
  handlers.add(handler);
  channels.set(channel, handlers);
  return () => {
    handlers.delete(handler);
    if (handlers.size === 0) channels.delete(channel);
  };
}

/** 往一条通道广播。没有订阅者就静默丢掉，不算错误。 */
export function publishLocal(channel: string, payload: unknown): void {
  const handlers = channels.get(channel);
  if (!handlers) return;
  // 复制一份再发：回调里退订不该让本轮还没收到的订阅者漏掉。
  for (const handler of [...handlers]) handler(payload);
}

export function hasLocalSubscribers(channel: string): boolean {
  return (channels.get(channel)?.size ?? 0) > 0;
}
