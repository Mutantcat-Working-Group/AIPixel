import { describe, expect, it } from "vitest";

import { parsePlanRows, type PlanRow } from "./plan-node";

// 这个解析器面对的是模型手写的入参：形状由它定，容错由我们扛。
// 四种键各自一条用例，三种容器各一条，加「不限」和「读不出来」——
// 读不出来的那条尤其重要：调用方靠空数组才知道该退回裸 JSON。

const keysOf = (row: PlanRow) => row.keys;

describe("parsePlanRows", () => {
  it("reads all four keys in one node input", () => {
    const rows = parsePlanRows({
      references: [{ index: 1, mode: "style", because: "照这个画风换色" }, { index: 2, mode: "full" }],
      intent: { id: "tilemap", because: "瓦片地图" },
      style: { id: "gameboy", because: "GameBoy 配色" },
      knowledge: ["walk-cycle", "tilemap"],
    });
    expect(rows.map((row) => row.labelKey)).toEqual([
      "plan.reference_one",
      "plan.reference_one",
      "plan.intent",
      "plan.style",
      "plan.knowledge",
    ]);
    expect(rows[0].vars).toEqual({ n: 1 });
    expect(rows[0].because).toBe("照这个画风换色");
    expect(keysOf(rows[1])).toEqual(["plan.mode.full"]);
    expect(keysOf(rows[2])).toEqual(["intent.tilemap"]);
    expect(keysOf(rows[3])).toEqual(["style.gameboy"]);
    expect(keysOf(rows[4])).toEqual(["knowledge.walk-cycle", "knowledge.tilemap"]);
    // 知识行不带判据：模型只报标题，原话在检索时就已经用掉了。
    expect(rows[4].because).toBeUndefined();
  });

  it("falls back to position when a reference carries no index", () => {
    const rows = parsePlanRows({ references: [{ mode: "style" }, "full"] });
    expect(rows.map((row) => row.vars?.n)).toEqual([1, 2]);
    expect(keysOf(rows[1])).toEqual(["plan.mode.full"]);
  });

  it("accepts a numbered object for references", () => {
    const rows = parsePlanRows({ references: { 1: "style", 2: "full" } });
    expect(rows.map((row) => row.vars?.n)).toEqual([1, 2]);
  });

  it("accepts a bare object as one reference entry", () => {
    const rows = parsePlanRows({ references: { mode: "full", because: "照着实临摹" } });
    expect(rows).toHaveLength(1);
    expect(rows[0].vars).toEqual({ n: 1 });
    expect(rows[0].because).toBe("照着实临摹");
  });

  it("accepts a bare string for intent and style", () => {
    const rows = parsePlanRows({ intent: "icon", style: "PICO8" });
    expect(keysOf(rows[0])).toEqual(["intent.icon"]);
    // 大小写无所谓：交给字典去认，认不出就显示原文。
    expect(keysOf(rows[1])).toEqual(["style.pico8"]);
  });

  it("treats an explicit none as 'unset' rather than a missing key", () => {
    const rows = parsePlanRows({ intent: "none", style: { id: "clear" } });
    expect(keysOf(rows[0])).toEqual(["plan.any"]);
    expect(keysOf(rows[1])).toEqual(["plan.any"]);
  });

  it("returns nothing when the input has no readable shape", () => {
    expect(parsePlanRows(null)).toEqual([]);
    expect(parsePlanRows("style")).toEqual([]);
    expect(parsePlanRows([])).toEqual([]);
    expect(parsePlanRows({})).toEqual([]);
    expect(parsePlanRows({ references: [], intent: null, style: "" })).toEqual([]);
  });

  it("skips garbage entries but keeps the readable ones", () => {
    const rows = parsePlanRows({
      references: [{ because: "没有 mode" }, "style", null, 7],
      intent: {},
      style: "dither",
    });
    expect(keysOf(rows[0])).toEqual(["plan.mode.style"]);
    expect(keysOf(rows[1])).toEqual(["style.dither"]);
  });
});
