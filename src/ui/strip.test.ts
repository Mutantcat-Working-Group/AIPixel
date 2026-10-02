// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { describe, expect, it } from "vitest";

import {
  stripEdgesOf,
  stripReach,
  stripScrollFor,
  stripScrollable,
  thumbBox,
  thumbDragScroll,
  type StripMetrics,
} from "./strip";
// 样式表按原文读进来：这一组断言钉的是「一行放不下往右滚」的合同，
// jsdom 不排版面，折没折行它测不出来，只能把规矩落到文本上比对。
import styles from "../styles.css?raw";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import HStrip from "./HStrip";

/** 七帧的帧条：内容 422、可见 269、还能往右滚 153。 */
const FRAMES: StripMetrics = { scrollWidth: 422, clientWidth: 269, scrollLeft: 0 };

describe("stripReach", () => {
  it("只认真正宽出去的部分", () => {
    expect(stripReach(FRAMES)).toBe(153);
    expect(stripReach({ scrollWidth: 269, clientWidth: 269, scrollLeft: 0 })).toBe(0);
  });

  it("亚像素溢出不算溢出，不然 1px 的滑块会常驻", () => {
    expect(stripScrollable({ scrollWidth: 269.4, clientWidth: 269, scrollLeft: 0 })).toBe(false);
  });
});

describe("stripEdgesOf", () => {
  it("起手在左头：只能往右", () => {
    expect(stripEdgesOf(FRAMES)).toEqual({ canLeft: false, canRight: true });
  });

  it("滚到底只剩往左", () => {
    expect(stripEdgesOf({ ...FRAMES, scrollLeft: 153 })).toEqual({ canLeft: true, canRight: false });
  });

  it("不溢出的条子两端都不亮", () => {
    expect(stripEdgesOf({ scrollWidth: 200, clientWidth: 269, scrollLeft: 0 })).toEqual({
      canLeft: false,
      canRight: false,
    });
  });
});

describe("thumbBox", () => {
  it("滑块长度等于可见部分占全部的比", () => {
    // 269/422 的 200px 轨道，滑块该占一半多一点。
    const box = thumbBox(FRAMES, 200, 24);
    expect(Math.round(box.width)).toBe(Math.round((200 * 269) / 422));
    expect(box.left).toBe(0);
  });

  it("滑块随滚动走到行程尽头", () => {
    const end = thumbBox({ ...FRAMES, scrollLeft: 153 }, 200, 24);
    // 行程 = 轨道 - 滑块宽，滚到底必须正好走完，不能留缝也不能出头。
    expect(Math.round(end.left + end.width)).toBe(200);
  });

  it("帧再多滑块也不细过下限", () => {
    expect(thumbBox({ scrollWidth: 4220, clientWidth: 269, scrollLeft: 0 }, 200, 24).width).toBe(24);
  });

  it("轨道比下限还窄时滑块不越过轨道", () => {
    expect(thumbBox(FRAMES, 10, 24)).toEqual({ left: 0, width: 10 });
  });

  it("不溢出时滑块铺满轨道（其实那时整根条都不渲染）", () => {
    expect(thumbBox({ scrollWidth: 100, clientWidth: 269, scrollLeft: 0 }, 200, 24)).toEqual({
      left: 0,
      width: 200,
    });
  });
});

describe("thumbDragScroll", () => {
  it("指针位移按行程比例换算成 scrollLeft", () => {
    const travel = 200 - thumbBox(FRAMES, 200, 24).width;
    // 拖着走满全程，scrollLeft 正好到 153。
    expect(thumbDragScroll(FRAMES, travel, travel)).toBeCloseTo(153, 5);
  });

  it("位移夹在两端之间，不会把内容甩出去", () => {
    expect(thumbDragScroll(FRAMES, 120, -500)).toBe(0);
    expect(thumbDragScroll(FRAMES, 120, 5000)).toBe(153);
  });

  it("没有行程时拖动不改变滚动位置", () => {
    expect(thumbDragScroll({ scrollWidth: 100, clientWidth: 269, scrollLeft: 0 }, 0, 80)).toBe(0);
  });
});

describe("stripScrollFor", () => {
  it("把不在视野里的格子带回来", () => {
    // 第七格在 336px 处，条子只看见 0..269：得往右滚。
    expect(stripScrollFor(FRAMES, 336, 56)).toBeGreaterThan(0);
  });

  it("已经在视野里的格子不挪条子", () => {
    expect(stripScrollFor(FRAMES, 120, 56)).toBe(0);
  });

  it("夹在最大滚动量里，不会把条子拉过头", () => {
    expect(stripScrollFor(FRAMES, 400, 56)).toBe(153);
  });
});

/**
 * 按选择器从样式表里揪出一条规则，拆成「属性 -> 值」。没揪到就是空对象，
 * 断言自然失败——比继续往下走到一个查不到原因的 undefined 好。
 */
function ruleOf(sheet: string, selector: string): Record<string, string> {
  const hit = new RegExp(`(?:^|\\n)\\s*\\.${selector}\\s*\\{([^}]*)\\}`).exec(sheet);
  if (!hit) return {};
  const out: Record<string, string> = {};
  for (const decl of hit[1].split(";")) {
    const at = decl.indexOf(":");
    if (at < 0) continue;
    out[decl.slice(0, at).trim()] = decl.slice(at + 1).trim();
  }
  return out;
}

/**
 * 帧条为什么必须「一行放不下就往右滚」：这条路真踩过坑。
 *
 * 帧条一度写着 flex-wrap: wrap，八帧以上就折成两三行，底下的小节被顶出视野，
 * 帧条自己也看不全，用户只会得出「横向滚动根本没做」的结论。而折行是布局
 * 结果，jsdom 排不出来，所以合同得钉在这里，三样任缺一样都会退回换行：
 *   1. className 必须落在 HStrip 里那个真的会滚的元素上。落在外面那层
 *      (.hstrip) 上的话，.hstrip-scroll 的 overflow-x 永远不生效，帧多了
 *      一样折行，而且页面上看不出哪儿错了。
 *   2. 公共类 .hstrip-scroll 要把 nowrap、overflow-x: auto、overflow-y: hidden
 *      一项不落地带上：这三行是「一行 + 横滚」本身，少一项就退回换行或竖滚。
 *   3. 一格帧不许被压缩（flex: none）：能缩的话它先缩成牙签，而不是滚出视野。
 */
describe("帧条横滚的合同", () => {
  it("className 落在真正会滚的那一层上", () => {
    // 文件是 .ts，用 createElement 免得为两个断言另开一间 .tsx。
    // children 摊在 props 里而不是当第三个参数：React 18 的类型把「带子节点」
    // 和「不带子节点」分成两个重载，摊里面才落得准。
    const html = renderToStaticMarkup(
      createElement(
        HStrip,
        {
          className: "frame-strip",
          label: "帧",
          children: createElement("button", { type: "button", className: "frame-chip" }),
        },
      ),
    );
    // 两个类必须同时挂在滚动元素上：hstrip-scroll 是横滚那份名声的来源，
    // frame-strip 是间距与吸附的来源，缺哪个都没法用。
    expect(html).toContain('class="hstrip-scroll frame-strip"');
    expect(html).toContain('class="frame-chip"');
  });

  it("帧条不换行、横滚竖关，一格帧的尺寸钉死", () => {
    // 横滚的声明归公共类 .hstrip-scroll：调用方（帧条、图层条）只留间距。
    const scroller = ruleOf(styles, "hstrip-scroll");
    expect(scroller["flex-wrap"]).toBe("nowrap");
    expect(scroller["overflow-x"]).toBe("auto");
    expect(scroller["overflow-y"]).toBe("hidden");
    // 格子不设 flex: none 时，条子宁可挤压格子也不滚，帧缩成一条缝。
    expect(ruleOf(styles, "frame-chip")["flex"]).toBe("none");
  });
});
