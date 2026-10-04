// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only

/**
 * 把连续到达的事件收拢成一次处理。
 *
 * Tauri 每条事件都是各自独立的任务直接送到回调里，回调里每条都 setState 就是
 * 一次独立的 React 提交：MCP 一口气建十个画布、唤醒后补发一整批会话簿变更时，
 * 界面不是卡成幻灯片，就是半天才跟上账。React 18 只把同一个任务里的 setState
 * 合并成一次渲染，所以让同一条通道的事件排一会儿队、攒进同一个任务再统一处理，
 * 一整串风暴就塌成一次提交，中间的旧状态反正也来不及显示。
 */
export interface Coalescer<T> {
  /** 收进一条事件。同一条通道的调用顺序就是处理顺序。 */
  push(item: T): void;
  /** 不等定时器，立刻把队列处理干净（休眠唤醒、测试里用）。 */
  flush(): void;
  /** 排队还没处理的条数，诊断用。 */
  pending(): number;
}

export function createCoalescer<T>(
  handle: (item: T) => void,
  options: { delayMs?: number } = {},
): Coalescer<T> {
  const delayMs = options.delayMs ?? 0;
  let queue: T[] = [];
  let timer: ReturnType<typeof setTimeout> | null = null;
  let draining = false;

  const drain = () => {
    timer = null;
    draining = true;
    try {
      // 处理期间新到的事件排进下一轮 drain：一次 drain 不吃成停不下来的循环，
      // 而两轮之间浏览器能腾出手来画一帧。
      while (queue.length > 0) {
        const batch = queue;
        queue = [];
        for (const item of batch) {
          try {
            handle(item);
          } catch (error) {
            // 一条坏载荷不该把同批剩下的几条一起拖下水：单独兜住继续走。
            console.error("[coalescer] 事件处理失败，已跳过", error);
          }
        }
      }
    } finally {
      draining = false;
    }
  };

  return {
    push(item: T) {
      queue.push(item);
      if (timer !== null) return;
      timer = setTimeout(drain, delayMs);
    },
    flush() {
      if (draining) return;
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      drain();
    },
    pending() {
      return queue.length;
    },
  };
}
