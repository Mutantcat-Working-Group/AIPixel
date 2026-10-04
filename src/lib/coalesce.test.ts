// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it, vi } from "vitest";

import { createCoalescer } from "./coalesce";

describe("事件合并器", () => {
  it("一个任务里堆的几条，一次 drain 按顺序全部处理完", async () => {
    const seen: number[] = [];
    const bus = createCoalescer<number>((item) => seen.push(item));

    for (const item of [1, 2, 3, 4]) bus.push(item);
    // 定时器没到：一条都不该被提前处理。
    expect(seen).toEqual([]);
    expect(bus.pending()).toBe(4);

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(seen).toEqual([1, 2, 3, 4]);
    expect(bus.pending()).toBe(0);
  });

  it("flush 不等定时器，立刻结算", () => {
    const seen: number[] = [];
    const bus = createCoalescer<number>((item) => seen.push(item));

    bus.push(7);
    bus.push(8);
    bus.flush();
    expect(seen).toEqual([7, 8]);
    // flush 之后再 push 还能照常排上：唤醒对齐那条路会连着用。
    bus.push(9);
    bus.flush();
    expect(seen).toEqual([7, 8, 9]);
  });

  it("一条事件处理炸了，同批剩下的照旧处理", () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      const seen: number[] = [];
      const bus = createCoalescer<number>((item) => {
        if (item === 2) throw new Error("bad payload");
        seen.push(item);
      });

      bus.push(1);
      bus.push(2);
      bus.push(3);
      bus.flush();
      expect(seen).toEqual([1, 3]);
    } finally {
      errors.mockRestore();
    }
  });

  it("处理期间又进来的事件排下一轮，不吃成死循环", () => {
    const handled: number[] = [];
    let pushed = false;
    let pendingWhileHandling = -1;

    const bus = createCoalescer<number>((item) => {
      handled.push(item);
      // 处理第 1 条的当口又塞一条新的：队列已经被整批换走，这条只能进下一轮，
      // 不会让这次 drain 一条套一条递归到停不下来。
      if (item === 1 && !pushed) {
        pushed = true;
        pendingWhileHandling = bus.pending();
        bus.push(99);
      }
    });

    bus.push(1);
    bus.flush();
    expect(pendingWhileHandling).toBe(0);

    expect(handled).toEqual([1, 99]);
    expect(bus.pending()).toBe(0);
  });
});
