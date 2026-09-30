// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it, vi } from "vitest";

import { hasLocalSubscribers, publishLocal, subscribeLocal } from "./local-bus";

// 总线只在 Tauri 事件桥拿不到时兜底，所以这里考的是它作为兜底靠不靠得住：
// 该送到的一个不漏，退订了就真的不再送，空通道不炸。

describe("local bus", () => {
  // 每条用例用独一份的通道名：总线是模块级共享的，串名会互相看见对方留下的订阅。
  it("delivers to every subscriber of the channel", () => {
    const first = vi.fn();
    const second = vi.fn();
    const off = subscribeLocal("t-fanout", first);
    subscribeLocal("t-fanout", second);

    publishLocal("t-fanout", { kind: "completed", turns: 1 });

    expect(first).toHaveBeenCalledWith({ kind: "completed", turns: 1 });
    expect(second).toHaveBeenCalledTimes(1);
    off();
  });

  it("stops delivering once unsubscribed", () => {
    const handler = vi.fn();
    const off = subscribeLocal("t-unsub", handler);

    publishLocal("t-unsub", { kind: "usage" });
    off();
    off(); // 幂等：重复退订不能抛
    publishLocal("t-unsub", { kind: "usage" });

    expect(handler).toHaveBeenCalledTimes(1);
    expect(hasLocalSubscribers("t-unsub")).toBe(false);
  });

  it("keeps channels apart and tolerates an empty one", () => {
    const agent = vi.fn();
    const batch = vi.fn();
    subscribeLocal("t-channel-a", agent);
    subscribeLocal("t-channel-b", batch);

    publishLocal("t-channel-a", { kind: "token", text: "hi" });

    expect(agent).toHaveBeenCalledTimes(1);
    expect(batch).not.toHaveBeenCalled();
    expect(() => publishLocal("nobody-listens", { kind: "token" })).not.toThrow();
    expect(hasLocalSubscribers("t-channel-a")).toBe(true);
  });

  it("still reaches subscribers that unsubscribed earlier in the same round", () => {
    const seen: string[] = [];
    // 第一个回调里把自己退订：抄送名单是取完快照再发的，所以后来的订阅者照样收到。
    subscribeLocal("t-midround", () => {
      seen.push("first");
      off();
    });
    const off = subscribeLocal("t-midround", () => seen.push("second"));

    publishLocal("t-midround", { kind: "token", text: "x" });

    expect(seen).toEqual(["first", "second"]);
  });
});
