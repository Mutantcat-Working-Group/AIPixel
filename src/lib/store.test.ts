// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";

import { briefToText, probeSummary, videoBriefToText } from "./dock-format";
import { DEFAULT_BATCH_RECIPE, EMPTY_BATCH_RUN } from "./batch";
import { AGENT_EVENT_CHANNEL } from "./bridge";
import { STALL_SECONDS, blankDocument, undoBudget, useStore, type WorkflowParams } from "./store";
import { publishLocal } from "./local-bus";
import type {
  BatchRecipe,
  BatchRecipeEntry,
  BatchScan,
  DocPatch,
  ModelRole,
  RecipeImportReport,
  RoleBinding,
  SessionInfo,
  Layer,
  ModelConfig,
  ModelView,
  PixelDocument,
  VideoBrief,
  VideoProbe,
  VisionBrief,
} from "./types";

// Rust 后端只活在桌面进程里，这里把 invoke 整个接住：桥接层的每个函数最终都落到
// 这一条命令调用上，所以侧栏那四个编辑动作到底往 Rust 发了什么，看它就行。
const invokeCalls = vi.hoisted(
  () => [] as Array<{ cmd: string; args: Record<string, unknown> }>,
);

/** 按命令预制返回值：配方簿的读要走这条，编辑器操作继续吃默认的 1。 */
const invokeResults = vi.hoisted(() => ({}) as Record<string, unknown>);

/** 按命令预制失败：读不到配方簿那条路要靠它。 */
const invokeErrors = vi.hoisted(() => ({}) as Record<string, string>);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => {
    invokeCalls.push({ cmd, args: args ?? {} });
    if (invokeErrors[cmd]) return Promise.reject(new Error(invokeErrors[cmd]));
    // 编辑器操作统一回报一个新 revision，真实环境由 Rust 校验 op。
    return Promise.resolve(invokeResults[cmd] ?? 1);
  },
}));

/** ops 已经落到最后一条 editor_apply_ops 调用里；proto/JSON 字段名一字不改。 */
function lastOps(): Array<Record<string, unknown>> {
  const calls = invokeCalls.filter((call) => call.cmd === "editor_apply_ops");
  const last = calls[calls.length - 1];
  return (last?.args.ops as Array<Record<string, unknown>>) ?? [];
}

/** 往 agent-event 通道上发一条带会话归属的事件，和 Rust 的载荷同形。 */
function publishAgent(sessionId: string, event: Record<string, unknown>): void {
  publishLocal(AGENT_EVENT_CHANNEL, { session_id: sessionId, event });
}

/** 把一份文档打包成「第一次广播」那种全量增量。真机上后端没有基准可比，
 * 第一次就是这么发的；测试里拿它把前端那份文档整体对齐到目标状态。 */
function fullPatch(doc: PixelDocument): DocPatch {
  const cels: DocPatch["cels"] = [];
  for (const [layerId, frames] of Object.entries(doc.cels)) {
    for (const [frameId, cel] of Object.entries(frames)) {
      cels.push([layerId, frameId, cel.indices]);
    }
  }
  return {
    name: doc.name,
    width: doc.width,
    height: doc.height,
    palette: doc.palette,
    layers: doc.layers,
    frames: doc.frames,
    palettes: doc.palettes,
    revision: doc.revision,
    cels,
    dropped: [],
  };
}

/** 两图层两帧的文档：帧时长与图层排序的边界都要有两个元素才测得出来。 */
function seedDocument(): PixelDocument {
  const base = blankDocument(8, 8);
  const second: Layer = {
    id: "L1",
    name: "Layer 2",
    visible: true,
    opacity: 255,
    palette_id: "sweetie16",
    locked: false,
  };
  return {
    ...base,
    layers: [...base.layers, second],
    frames: [...base.frames, { id: "F1", duration_ms: 250 }],
    cels: { ...base.cels, L1: { F0: { indices: new Array(64).fill(0) }, F1: { indices: new Array(64).fill(0) } } },
  };
}

describe("编辑器结构动作（store -> bridge）", () => {
  it("writes the current frame duration and clamps to Rust's range", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: seedDocument(),
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: null },
    });
    invokeCalls.length = 0;

    await useStore.getState().setFrameDuration(430);
    expect(lastOps()).toEqual([
      { op: "set_frame_duration", id: "F0", duration_ms: 430 },
    ]);

    invokeCalls.length = 0;
    // 上下界各夹一次：0 归 1，999999 归 60000，与 MAX_FRAME_DURATION_MS 对齐。
    await useStore.getState().setFrameDuration(0);
    expect(lastOps()).toEqual([
      { op: "set_frame_duration", id: "F0", duration_ms: 1 },
    ]);
    await useStore.getState().setFrameDuration(999_999);
    expect(lastOps()).toEqual([
      { op: "set_frame_duration", id: "F0", duration_ms: 60_000 },
    ]);
  });

  it("skips the round trip when the duration is already in place", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: seedDocument(),
      frameIndex: 1,
      active: { layer: "L0", frame: "F1", color: null },
    });
    invokeCalls.length = 0;

    await useStore.getState().setFrameDuration(250);
    expect(lastOps()).toEqual([]);
  });

  it("does nothing structural without an open document", async () => {
    useStore.setState({ activeId: null, document: null, frameIndex: 0 });
    invokeCalls.length = 0;

    await useStore.getState().setFrameDuration(300);
    expect(lastOps()).toEqual([]);
  });

  it("toggles layer visibility without touching opacity", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: seedDocument(),
      active: { layer: "L1", frame: "F0", color: null },
    });
    invokeCalls.length = 0;

    await useStore.getState().setLayerVisible("L1", false);
    expect(lastOps()).toEqual([{ op: "set_layer_properties", id: "L1", visible: false }]);
  });

  it("clamps layer opacity to 0..255 and rounds to whole steps", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: seedDocument(),
      active: { layer: "L0", frame: "F0", color: null },
    });
    invokeCalls.length = 0;

    await useStore.getState().setLayerOpacity("L0", 128.6);
    expect(lastOps()).toEqual([{ op: "set_layer_properties", id: "L0", opacity: 129 }]);

    invokeCalls.length = 0;
    await useStore.getState().setLayerOpacity("L0", -40);
    expect(lastOps()).toEqual([{ op: "set_layer_properties", id: "L0", opacity: 0 }]);

    invokeCalls.length = 0;
    await useStore.getState().setLayerOpacity("L0", 900);
    expect(lastOps()).toEqual([{ op: "set_layer_properties", id: "L0", opacity: 255 }]);
  });

  it("moves the active layer one step later in draw order", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: seedDocument(),
      active: { layer: "L0", frame: "F0", color: null },
    });
    invokeCalls.length = 0;

    await useStore.getState().moveLayer(1);
    expect(lastOps()).toEqual([{ op: "move_layer", id: "L0", to_index: 1 }]);
  });

  it("treats an out-of-range layer move as a no-op", async () => {
    const doc = seedDocument();
    useStore.setState({
      activeId: "doc-01",
      document: doc,
      // L1 已在最上：再往上没有位置了。
      active: { layer: "L1", frame: "F0", color: null },
    });
    invokeCalls.length = 0;

    await useStore.getState().moveLayer(1);
    expect(lastOps()).toEqual([]);

    useStore.setState({ active: { layer: "L0", frame: "F0", color: null } });
    await useStore.getState().moveLayer(-1);
    expect(lastOps()).toEqual([]);
  });

  it("swaps the whole palette through one set_palette op", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: seedDocument(),
      active: { layer: "L0", frame: "F0", color: null },
    });
    invokeCalls.length = 0;

    await expect(useStore.getState().setPaletteColors(["#1a1c2c", "#ffcd75"])).resolves.toBe(
      true,
    );
    expect(lastOps()).toEqual([{ op: "set_palette", colors: ["#1a1c2c", "#ffcd75"] }]);
  });

  it("refuses an empty palette instead of wiping the canvas", async () => {
    useStore.setState({ activeId: "doc-01", document: seedDocument() });
    invokeCalls.length = 0;

    // 空配色等于把整幅擦透明：不发车，并如实告诉调用方没落成。
    await expect(useStore.getState().setPaletteColors([])).resolves.toBe(false);
    expect(lastOps()).toEqual([]);
  });

  it("hands an export straight to Rust with the picked path", async () => {
    useStore.setState({ activeId: "doc-01", document: seedDocument() });
    invokeCalls.length = 0;

    await useStore.getState().exportDocument("gif", "/tmp/out.gif");
    expect(invokeCalls.at(-1)).toEqual({
      cmd: "document_export",
      args: { id: "doc-01", format: "gif", path: "/tmp/out.gif", columns: 0, frame: null },
    });
  });

  it("carries the frame number only for a single-frame export", async () => {
    useStore.setState({ activeId: "doc-01", document: seedDocument(), frameIndex: 2 });
    invokeCalls.length = 0;

    await useStore.getState().exportDocument("frame", "/tmp/out.png", { frame: 2 });
    expect(invokeCalls.at(-1)?.args).toEqual({
      id: "doc-01",
      format: "frame",
      path: "/tmp/out.png",
      columns: 0,
      frame: 2,
    });
  });

  it("inserts a new layer right after the active one", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: seedDocument(),
      active: { layer: "L0", frame: "F0", color: null },
    });
    invokeCalls.length = 0;

    await useStore.getState().addLayer();
    expect(lastOps()).toEqual([{ op: "create_layer", after: "L0" }]);
  });

  it("deletes the named layer, or the active one when none is named", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: seedDocument(),
      active: { layer: "L1", frame: "F0", color: null },
    });
    invokeCalls.length = 0;

    await useStore.getState().deleteLayer("L0");
    expect(lastOps()).toEqual([{ op: "delete_layer", id: "L0" }]);

    invokeCalls.length = 0;
    await useStore.getState().deleteLayer();
    expect(lastOps()).toEqual([{ op: "delete_layer", id: "L1" }]);
  });

  it("keeps the last layer: no delete op leaves the document", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: blankDocument(8, 8),
      active: { layer: "L0", frame: "F0", color: null },
    });
    invokeCalls.length = 0;

    await useStore.getState().deleteLayer();
    expect(lastOps()).toEqual([]);
  });
});

describe("撤销栈只记编辑器自己那一下", () => {
  it("落笔失败就把账退回去，模型那次改动也不该被塞进撤销栈", async () => {
    const doc = seedDocument();
    // boot 是 store 唯一挂事件监听的地方（真实进程里开机就挂）。它一路上要读
    // 六七样东西，这里把可能抛的那几样预制好，别让它在半路炸掉。
    const stubs = {
      agent_list_models: { active_id: "", entries: [] },
      agent_history: [],
      batch_recipes_list: [],
      session_list: [],
      session_create: {
        id: "s1",
        model_id: "m1",
        model_label: "test",
        roles: [],
        width: 8,
        height: 8,
        revision: 1,
        title: null,
        order: 0,
      },
      agent_document: { id: "s1", revision: 1, document: doc },
    };
    Object.assign(invokeResults, stubs);
    try {
      await useStore.getState().boot();
    } finally {
      for (const key of Object.keys(stubs)) delete invokeResults[key];
    }

    useStore.setState({
      activeId: "doc-01",
      document: doc,
      revision: 4,
      pngRevision: 4,
      undoStack: [],
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: "#ffffff" },
    });

    // 正面例子先立规矩：成功的那一笔，撤销栈吃到的是改前的快照。
    await useStore.getState().paintStroke([{ x: 1, y: 1 }]);
    expect(useStore.getState().undoStack).toEqual([doc]);

    // 落空的那一笔当场退账：撤销栈顶那一格得摘回去，不然一次点空的画笔
    // 也会白吃一步撤销。
    invokeErrors["editor_paint_stroke"] = "stroke rejected";
    try {
      await useStore.getState().paintStroke([{ x: 2, y: 2 }]);
      expect(useStore.getState().undoStack).toEqual([doc]);
      // 模型自己的一次改动（跑完 Lua 脚本）：现在不该再进撤销栈。
      publishAgent("doc-01", { kind: "document_updated", revision: 6, patch: fullPatch(doc) });
      expect(useStore.getState().undoStack).toEqual([doc]);
      expect(useStore.getState().redoStack).toEqual([]);
    } finally {
      delete invokeErrors["editor_paint_stroke"];
    }
  });

  it("撤销栈超字节预算先丢最老，裁到一步也得留住那一步", async () => {
    const doc = seedDocument();
    useStore.setState({
      activeId: "doc-01",
      document: doc,
      revision: 4,
      pngRevision: 4,
      undoStack: [],
      redoStack: [],
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: "#ffffff" },
    });

    // 一份快照 = 3 个 8x8 的 cel = 768 字节。连点五笔把栈压满（3840），
    // 预算压到 1600：丢到两份 1536 就打住，最老的三份先走、最新的留下。
    const original = undoBudget.bytes;
    undoBudget.bytes = 1600;
    try {
      for (let i = 0; i < 5; i++) {
        await useStore.getState().paintStroke([{ x: i, y: 0 }]);
      }
      expect(useStore.getState().undoStack).toEqual([doc, doc]);
      // 预算再砍到比一份还小：也得留一步，一步都没有的话撤销整个废掉。
      undoBudget.bytes = 100;
      await useStore.getState().paintStroke([{ x: 7, y: 7 }]);
      expect(useStore.getState().undoStack).toEqual([doc]);
    } finally {
      undoBudget.bytes = original;
    }
  });

  it("撤销把当前文档挪进重做栈，重做再原样挪回来", async () => {
    const first = seedDocument();
    const second: PixelDocument = { ...first, revision: 5 };
    useStore.setState({
      activeId: "doc-01",
      document: second,
      revision: 5,
      pngRevision: 5,
      undoStack: [first],
      redoStack: [],
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: "#ffffff" },
    });
    invokeCalls.length = 0;

    await useStore.getState().undoEdit();
    // 撤销栈见底，画面退回改前那一版；退掉的那版挪进了重做栈。
    expect(useStore.getState().document).toBe(first);
    expect(useStore.getState().undoStack).toEqual([]);
    expect(useStore.getState().redoStack).toEqual([second]);
    // 拍回走的是整份同步，不是编辑器 op：撤销要的是回到那一版，不是改哪几笔。
    const syncCall = invokeCalls.filter((call) => call.cmd === "agent_sync_document").at(-1);
    expect(syncCall?.args.document).toBe(first);

    await useStore.getState().redoEdit();
    // 重做把画面拍回来，这一格又回到撤销栈里，两步可以来回踩。
    expect(useStore.getState().document).toBe(second);
    expect(useStore.getState().undoStack).toEqual([first]);
    expect(useStore.getState().redoStack).toEqual([]);

    // 再动一笔，重做栈作废：重做的画面已经被新笔触盖掉，没有「找回来」可言。
    await useStore.getState().paintStroke([{ x: 3, y: 3 }]);
    expect(useStore.getState().redoStack).toEqual([]);
    expect(useStore.getState().undoStack).toEqual([first, second]);
  });

  it("拍回失败时两个栈原样还回去，不凭空少一步", async () => {
    const first = seedDocument();
    const second: PixelDocument = { ...first, revision: 5 };
    useStore.setState({
      activeId: "doc-01",
      document: second,
      revision: 5,
      pngRevision: 5,
      undoStack: [first],
      redoStack: [],
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: "#ffffff" },
    });
    invokeErrors["agent_sync_document"] = "sync rejected";
    try {
      await useStore.getState().undoEdit();
      expect(useStore.getState().undoStack).toEqual([first]);
      expect(useStore.getState().redoStack).toEqual([]);
    } finally {
      delete invokeErrors["agent_sync_document"];
    }
  });

  it("拍回旧快照后选中跟着夹过去，后端收到同一条 active", async () => {
    // 当前文档两层两帧，手里停在 L1 的 F1 上；撤销栈顶那份更早，只剩 L0 的 F0。
    const older: PixelDocument = { ...blankDocument(8, 8), revision: 3 };
    const current: PixelDocument = { ...seedDocument(), revision: 5 };
    useStore.setState({
      activeId: "doc-01",
      document: current,
      revision: 5,
      pngRevision: 5,
      undoStack: [older],
      redoStack: [],
      frameIndex: 1,
      active: { layer: "L1", frame: "F1", color: "#ffffff" },
    });
    invokeCalls.length = 0;

    await useStore.getState().undoEdit();
    // 帧和层都不在新文档里，双双夹到首个；快照本身原样拍回。
    expect(useStore.getState().document).toBe(older);
    expect(useStore.getState().frameIndex).toBe(0);
    expect(useStore.getState().active).toEqual({ layer: "L0", frame: "F0", color: "#ffffff" });
    // 关键一步：选中回传。sync_document 只在选中「不存在」时才修，而这里夹出来的
    // 帧名层名在新文档里都真实存在，不补这一条模型下一步就画到用户没在看的帧上。
    const activeCall = invokeCalls.find((call) => call.cmd === "agent_set_active");
    expect(activeCall?.args.id).toBe("doc-01");
    expect(activeCall?.args.active).toEqual(useStore.getState().active);

    // 重做走同一条拍回路径，选中也得回传：回到两层两帧后帧号仍是 0、层仍是 L0。
    await useStore.getState().redoEdit();
    expect(useStore.getState().document).toBe(current);
    expect(useStore.getState().undoStack).toEqual([older]);
    expect(useStore.getState().redoStack).toEqual([]);
    const redoActiveCall = invokeCalls.filter((call) => call.cmd === "agent_set_active").at(-1);
    expect(redoActiveCall?.args.active).toEqual(useStore.getState().active);
    expect(useStore.getState().active).toEqual({ layer: "L0", frame: "F0", color: "#ffffff" });
  });
});

describe("blankDocument", () => {
  it("builds the same baseline structure Rust Document::new produces", () => {
    const doc = blankDocument(16, 8);
    expect(doc.width).toBe(16);
    expect(doc.height).toBe(8);
    expect(doc.layers).toHaveLength(1);
    expect(doc.layers[0].id).toBe("L0");
    expect(doc.frames).toHaveLength(1);
    expect(doc.frames[0].id).toBe("F0");
    expect(doc.palette).toEqual([]);
    expect(doc.revision).toBe(0);
  });

  it("fills the single cel with transparent index 0", () => {
    const doc = blankDocument(4, 3);
    expect(doc.cels.L0.F0.indices).toHaveLength(12);
    expect(doc.cels.L0.F0.indices.every((index) => index === 0)).toBe(true);
  });
});

function brief(over: Partial<VisionBrief> = {}): VisionBrief {
  return {
    subject: "a fox blacksmith",
    silhouette: "round ears over a square apron",
    palette: ["#2b1d18", "#c9603a", "#f2d3a8"],
    pose_notes: "leaning forward, hammer raised",
    proportions: "head one third of the height",
    craft_notes: "hard edges, no antialiasing",
    raw: "subject: a fox blacksmith",
    ...over,
  };
}

describe("briefToText", () => {
  it("keeps only the fields the model filled in", () => {
    const text = briefToText(brief({ proportions: "", craft_notes: "  " }));
    expect(text).toBe(
      [
        "subject: a fox blacksmith",
        "silhouette: round ears over a square apron",
        "pose: leaning forward, hammer raised",
        "palette (light to dark): #2b1d18, #c9603a, #f2d3a8",
      ].join("\n"),
    );
  });

  it("drops the palette line when nothing was read", () => {
    expect(briefToText(brief({ palette: [] }))).not.toContain("palette:");
  });
});

function probe(over: Partial<VideoProbe> = {}): VideoProbe {
  return {
    width: 640,
    height: 360,
    duration_s: 3.5,
    fps: 12.0,
    frame_count: 42,
    codec: "h264",
    has_audio: false,
    ...over,
  };
}

describe("probeSummary", () => {
  it("reads like a clip when ffprobe answered", () => {
    expect(probeSummary(probe(), "ffprobe")).toBe("640x360, 12.00 fps, 3.5s");
  });

  it("counts stills instead of timing for a directory of frames", () => {
    expect(probeSummary(probe({ width: null, height: null }), "directory")).toBe("42 still(s)");
  });

  it("says what it does not know instead of printing 0", () => {
    expect(probeSummary(probe({ fps: null, duration_s: null }), "none")).toBe(
      "640x360, unknown fps, unknown length",
    );
  });

  it("mentions audio only when there is audio", () => {
    expect(probeSummary(probe({ has_audio: true }), "ffprobe")).toContain("with audio");
  });
});

function motion(over: Partial<VideoBrief> = {}): VideoBrief {
  return {
    subject: "a crow taking off",
    motion: "a two-step hop, then wings open",
    key_poses: ["crouched, weight forward", "wings half open", "fully airborne"],
    timing: "hop on 1, lift on 3, full wing on 5",
    palette: ["#1b1b22", "#3d4a6b", "#e8e2d0"],
    craft_notes: "hard edges, no motion blur",
    raw: "subject: a crow taking off",
    ...over,
  };
}

describe("videoBriefToText", () => {
  it("keeps poses in order, joined by a pipe so the model reads a sequence", () => {
    const text = videoBriefToText(motion());
    expect(text).toBe(
      [
        "subject: a crow taking off",
        "motion: a two-step hop, then wings open",
        "key_poses: crouched, weight forward | wings half open | fully airborne",
        "timing: hop on 1, lift on 3, full wing on 5",
        "craft: hard edges, no motion blur",
        "palette (light to dark): #1b1b22, #3d4a6b, #e8e2d0",
      ].join("\n"),
    );
  });

  it("drops empty rows and the pose line when there are no poses", () => {
    const text = videoBriefToText(
      motion({ motion: "", timing: "  ", key_poses: [], palette: [] }),
    );
    expect(text).toBe(
      ["subject: a crow taking off", "craft: hard edges, no motion blur"].join("\n"),
    );
  });
});

describe("批量配方簿（存、取、删）", () => {
  function entry(name: string, recipe: Partial<BatchRecipe> = {}): BatchRecipeEntry {
    return { name, recipe: { ...DEFAULT_BATCH_RECIPE, ...recipe } };
  }

  function scanOf(dir: string): BatchScan {
    return { kind: "quantize", dir, count: 2, truncated: false, files: [] };
  }

  it("存的是当前 recipe，不是簿子里旧的那份", async () => {
    invokeResults["batch_recipes_list"] = [entry("旧配方", { target_w: 16 })];
    useStore.setState({
      recipe: { ...DEFAULT_BATCH_RECIPE, target_w: 96, kind: "export" },
      recipeName: "新配方",
      recipeBook: [],
    });
    invokeCalls.length = 0;

    await useStore.getState().saveRecipeAs();

    const saved = invokeCalls.find((call) => call.cmd === "batch_recipe_save");
    expect(saved?.args.name).toBe("新配方"); // 前后空白已经裁掉
    expect((saved?.args.recipe as BatchRecipe).target_w).toBe(96);
    // 存完把输入框清空，簿子里新出现的那一条才是「存好了」的确认。
    expect(useStore.getState().recipeName).toBe("");
    expect(useStore.getState().recipeBook[0].name).toBe("旧配方");
  });

  it("名字不合法时一个命令都不发", async () => {
    useStore.setState({ recipeName: "   " });
    invokeCalls.length = 0;

    await useStore.getState().saveRecipeAs();

    expect(invokeCalls.filter((call) => call.cmd === "batch_recipe_save")).toEqual([]);
  });

  it("挑一条配方盖上来，同时作废旧扫描与旧明细", () => {
    useStore.setState({
      recipe: DEFAULT_BATCH_RECIPE,
      recipeBook: [entry("sword-32", { target_w: 48, match_source_size: false })],
      recipeName: "",
      scan: scanOf("/old/input"),
      run: { ...EMPTY_BATCH_RUN, rows: [{ file: "a.png", state: "ok", note: "" }] },
    });

    useStore.getState().applyRecipe("sword-32");

    const { recipe, recipeName, scan, run } = useStore.getState();
    expect(recipe.target_w).toBe(48);
    expect(recipe.match_source_size).toBe(false);
    // 选择框要显示刚取的那一条，否则看不出现在用的是谁。
    expect(recipeName).toBe("sword-32");
    expect(scan).toBeNull();
    expect(run).toEqual(EMPTY_BATCH_RUN);
  });

  it("老配方缺字段时按默认补齐，不让默认值把已有参数盖掉", () => {
    useStore.setState({
      recipe: DEFAULT_BATCH_RECIPE,
      // 早先版本存下的半条配方：只有目录与种类。
      recipeBook: [
        {
          name: "半条",
          recipe: { kind: "quantize", input_dir: "/old", output_dir: "/out" } as BatchRecipe,
        },
      ],
    });

    useStore.getState().applyRecipe("半条");

    const recipe = useStore.getState().recipe;
    expect(recipe.input_dir).toBe("/old");
    expect(recipe.options).toEqual(DEFAULT_BATCH_RECIPE.options);
    expect(recipe.export_format).toBe("png");
  });

  it("删配方；删的正是当前这条时把输入框一起清掉", async () => {
    invokeResults["batch_recipes_list"] = [];
    useStore.setState({
      recipeBook: [entry("甲"), entry("乙")],
      recipeName: "乙",
    });
    invokeCalls.length = 0;

    await useStore.getState().removeRecipe("乙");

    expect(
      invokeCalls.some((call) => call.cmd === "batch_recipe_delete" && call.args.name === "乙"),
    ).toBe(true);
    expect(useStore.getState().recipeName).toBe("");
    expect(useStore.getState().recipeBook).toEqual([]);
  });

  it("删的是另一条时输入框原样留着", async () => {
    invokeResults["batch_recipes_list"] = [entry("甲")];
    useStore.setState({ recipeBook: [entry("甲"), entry("乙")], recipeName: "甲" });

    await useStore.getState().removeRecipe("乙");

    expect(useStore.getState().recipeName).toBe("甲");
    expect(useStore.getState().recipeBook.map((item) => item.name)).toEqual(["甲"]);
  });

  it("导出把点名的配方和路径一起交给 Rust，提示念落盘后的路径", async () => {
    invokeResults["batch_recipe_export"] = "/tmp/team/剑士.aipr";
    useStore.setState({ recipeBook: [entry("甲"), entry("乙")], recipeBusy: false });
    invokeCalls.length = 0;

    await useStore
      .getState()
      .exportRecipes([entry("甲"), entry("乙")], "/tmp/team/剑士.json");

    const call = invokeCalls.find((item) => item.cmd === "batch_recipe_export");
    expect((call?.args.entries as BatchRecipeEntry[]).map((item) => item.name)).toEqual(["甲", "乙"]);
    expect(call?.args.path).toBe("/tmp/team/剑士.json");
    // Rust 补了后缀，所以提示里是 .aipr 那个路径。
    expect(useStore.getState().notice).toEqual({
      text: "已导出 2 条配方到 /tmp/team/剑士.aipr",
      isError: false,
    });
    expect(useStore.getState().recipeBusy).toBe(false);
  });

  it("导入用回执里的簿子刷新视图，逐条交代也留下来", async () => {
    invokeResults["batch_recipe_import"] = {
      rows: [
        { name: "甲", final_name: "甲", state: "imported", note: "" },
        { name: "甲", final_name: "甲 (2)", state: "renamed", note: "" },
        { name: "#3", final_name: "", state: "skipped", note: "missing field `kind`" },
      ],
      entries: [entry("甲"), entry("甲 (2)")],
    } as RecipeImportReport;
    useStore.setState({ recipeBook: [entry("旧")], recipeImport: null, notice: null });
    invokeCalls.length = 0;

    await useStore.getState().importRecipes("/tmp/team/team.aipr");

    const call = invokeCalls.find((item) => item.cmd === "batch_recipe_import");
    expect(call?.args.path).toBe("/tmp/team/team.aipr");
    expect(useStore.getState().recipeBook.map((item) => item.name)).toEqual(["甲", "甲 (2)"]);
    expect(useStore.getState().recipeImport?.rows).toHaveLength(3);
    expect(useStore.getState().notice).toBeNull();
  });

  it("导入失败只清回执，不动本机簿子", async () => {
    invokeErrors["batch_recipe_import"] = "cannot read /tmp/x.aipr";
    useStore.setState({
      recipeBook: [entry("甲")],
      recipeImport: { rows: [], entries: [] },
      notice: null,
    });

    await useStore.getState().importRecipes("/tmp/x.aipr");

    expect(useStore.getState().recipeBook.map((item) => item.name)).toEqual(["甲"]);
    expect(useStore.getState().recipeImport).toBeNull();
    expect(useStore.getState().notice?.isError).toBe(true);
  });

  it("读不到配方簿只当没有，不给用户弹错误", async () => {
    invokeErrors["batch_recipes_list"] = "config dir unreadable";
    useStore.setState({ recipeBook: [entry("甲")], entries: [] });

    await useStore.getState().loadRecipes();

    // 读不到就是没有，别让一条坏配方挡住批量工作台。
    expect(useStore.getState().recipeBook).toEqual([]);
    // 但痕迹要留：通知栏多一条，比无声无息什么都没发生好。
    expect(useStore.getState().notice?.isError).toBe(true);
  });
});

describe("把提示词送进生图面板（识图 -> 生图）", () => {
  it("带着参考图时，垫图源切到这张磁盘图", () => {
    useStore.getState().patchDraft({ genSource: "none", genPath: null });
    invokeCalls.length = 0;

    useStore.getState().fillGenPrompt("一只乌鸦起飞", "/tmp/crow.png");

    const draft = useStore.getState().dockDraft;
    expect(useStore.getState().kind).toBe("image_gen");
    expect(draft.prompt).toBe("一只乌鸦起飞");
    // 只看过描述等于把原图扔了：垫图必须是刚挑的那张示例图。
    expect(draft.genSource).toBe("file");
    expect(draft.genPath).toBe("/tmp/crow.png");
  });

  it("不带参考图时，原来选好的垫图方式原样留着", () => {
    useStore.getState().patchDraft({ genSource: "frame", genFrame: "F1", genPath: null });

    useStore.getState().fillGenPrompt("把头饰画大一点");

    const draft = useStore.getState().dockDraft;
    expect(draft.genSource).toBe("frame");
    expect(draft.genFrame).toBe("F1");
    expect(draft.genPath).toBeNull();
  });
});

describe("会话模型分工（按角色另绑模型）", () => {
  /** 四个角色的分工快照：点名的角色自己带模型，其余蹭主模型。 */
  function rolesOf(...detached: ModelRole[]): RoleBinding[] {
    return (["chat", "image_gen", "vision", "video"] as ModelRole[]).map((role) => ({
      role,
      model_id: detached.includes(role) ? `m-${role}` : "m-chat",
      model_label: detached.includes(role) ? `${role}-model` : "chat-model",
      detached: detached.includes(role),
    }));
  }

  function session(roles: RoleBinding[]): SessionInfo {
    return {
      id: "s-1",
      model_id: "m-chat",
      model_label: "chat-model",
      roles,
    width: 64,
    height: 64,
    revision: 3,
    title: null,
    order: 1,
  };
}

  it("把角色和模型 id 一起交给 Rust，回执整条换掉旧会话", async () => {
    const rebound = session(rolesOf("image_gen"));
    invokeResults["session_bind_role"] = rebound;
    invokeResults["workflow_catalog"] = [];
    useStore.setState({ activeId: "s-1", sessions: [session(rolesOf())], workflows: [] });
    invokeCalls.length = 0;

    await useStore.getState().bindSessionRole("image_gen", "m-image_gen");

    const bound = invokeCalls.find((call) => call.cmd === "session_bind_role");
    expect(bound?.args).toEqual({ id: "s-1", role: "image_gen", modelId: "m-image_gen" });
    // 分会话整条换：分工快照不换的话，模型菜单里那条还指着旧模型。
    expect(useStore.getState().sessions).toEqual([rebound]);
    // 多了一个会生图的模型，之前灰着的流程可能就能跑了：重新问一遍目录。
    expect(invokeCalls.some((call) => call.cmd === "workflow_catalog")).toBe(true);
  });

  it("取消绑定时只点名角色，不带模型 id", async () => {
    const cleared = session(rolesOf());
    invokeResults["session_clear_role"] = cleared;
    invokeResults["workflow_catalog"] = [];
    useStore.setState({
      activeId: "s-1",
      sessions: [session(rolesOf("image_gen"))],
      workflows: [],
    });
    invokeCalls.length = 0;

    await useStore.getState().clearSessionRole("image_gen");

    const clearedCall = invokeCalls.find((call) => call.cmd === "session_clear_role");
    expect(clearedCall?.args).toEqual({ id: "s-1", role: "image_gen" });
    expect(useStore.getState().sessions).toEqual([cleared]);
  });

  it("没有会话时一个命令都不发", async () => {
    useStore.setState({ activeId: null, sessions: [] });
    invokeCalls.length = 0;

    await useStore.getState().bindSessionRole("vision", "m-vision");
    await useStore.getState().clearSessionRole("vision");

    expect(invokeCalls).toEqual([]);
  });
});

describe("runWorkflow / runPixelize 派发（kind -> Rust 命令 -> params）", () => {
  /** translateText 认不出的键回落 fallback，所以这里顺手验证回执原文不带回字典。 */
  const okOutcome = {
    revision: 4,
    summary: { key: "_test.summary", fallback: "done" },
  };

  /**
   * 只看派发了哪条工作流命令。落图成功后 store 会补一发 agent_document 去对齐
   * 选中——那是落地的收尾，不是派发，混在一起这几条用例就读不出来了。
   */
  function dispatched() {
    return invokeCalls.filter((call) => call.cmd !== "agent_document");
  }

  /** 每条用例都从同一起点出发：有会话、空对话流、四个命令都回 okOutcome。 */
  function reset(overrides: Record<string, unknown> = {}): void {
    for (const cmd of ["workflow_image_gen", "workflow_tween", "workflow_video_frames", "workflow_pixelize"]) {
      invokeResults[cmd] = okOutcome;
      delete invokeErrors[cmd];
    }
    useStore.setState({
      activeId: "doc-01",
      lang: "zh",
      entries: [],
      workflowBusy: false,
      outcome: null,
      outcomeError: null,
      notice: null,
      ...overrides,
    });
    invokeCalls.length = 0;
  }

  it("image_gen 落到 workflow_image_gen，params 一个字段不改", async () => {
    reset();
    const params: WorkflowParams = {
      prompt: "一只乌鸦起飞",
      size: "1024x1024",
      reference_frame: "F0",
      reference_path: null,
      spot: "new_frame",
      duration_ms: 120,
      options: null,
    };

    const outcome = await useStore.getState().runWorkflow("image_gen", params);

    expect(dispatched()).toEqual([
      { cmd: "workflow_image_gen", args: { id: "doc-01", params } },
    ]);
    expect(outcome).toEqual(okOutcome);
    expect(useStore.getState().outcome).toEqual(okOutcome);
    expect(useStore.getState().outcomeError).toBeNull();
    expect(useStore.getState().workflowBusy).toBe(false);
  });

  // 微调这条链路曾经两样都不带：用户在输入区选了「写实渲染」，点一下微调，
  // 出来的九行提示词里一条渲染规矩都没有。规矩不能只活在输入区。
  it("微调把这一句锁的画风与预设一起带上", async () => {
    reset();
    // 每次只看这一次调用：共享一份 invokeCalls，不清掉的话前面的调用会先撞上。
    invokeCalls.length = 0;
    await useStore.getState().refinePrompt("一只写实的乌鸦");
    expect(invokeCalls[0]).toMatchObject({
      cmd: "prompt_refine",
      args: { idea: "一只写实的乌鸦", width: 0, height: 0, style: null, presets: [] },
    });

    // 两把锁是平行的两根轴：换画风不动预设，摘画风也不动预设。
    useStore.getState().setStyleOverride("realistic");
    useStore.getState().setPresetOverrides(["microdetail", "occlusion"]);
    invokeCalls.length = 0;
    await useStore.getState().refinePrompt("一只写实的乌鸦");
    expect(invokeCalls[0]).toMatchObject({
      cmd: "prompt_refine",
      args: { style: "realistic", presets: ["microdetail", "occlusion"] },
    });

    useStore.getState().setStyleOverride(null);
    invokeCalls.length = 0;
    await useStore.getState().refinePrompt("一只写实的乌鸦");
    expect(invokeCalls[0]).toMatchObject({
      cmd: "prompt_refine",
      args: { style: null, presets: ["microdetail", "occlusion"] },
    });
    expect(useStore.getState().workflowBusy).toBe(false);
    // 两把锁要还回去：后续用例不少直接 useStore.setState，不从这里收，
    // 它们会带着这一组的偏好上路，红的还是不相干的那几条。
    useStore.setState({ styleOverride: null, presetOverrides: [] });
  });

  it("frame_tween 落到 workflow_tween，插值参数照搬", async () => {
    reset();
    const params: WorkflowParams = {
      from_frame: "F0",
      to_frame: "F1",
      count: 4,
      mode: "blend",
      duration_ms: 90,
    };

    await useStore.getState().runWorkflow("frame_tween", params);

    expect(dispatched()).toEqual([{ cmd: "workflow_tween", args: { id: "doc-01", params } }]);
  });

  it("video_frames 落到 workflow_video_frames，路径与抽帧数原样传", async () => {
    reset();
    const params: WorkflowParams = { path: "/tmp/clip.mp4", count: 0, duration_ms: 250 };

    await useStore.getState().runWorkflow("video_frames", params);

    expect(dispatched()).toEqual([
      { cmd: "workflow_video_frames", args: { id: "doc-01", params } },
    ]);
  });

  it("runPixelize 走 workflow_pixelize，与 runWorkflow 的另一条命令不混", async () => {
    reset();
    const params: WorkflowParams = {
      image_base64: "AAAA",
      media_type: "image/png",
      options: {
        max_colors: 16,
        snap_tolerance: 32,
        expand_palette: true,
        dither: false,
        alpha_threshold: 128,
        fit: "contain",
      },
    };

    const outcome = await useStore.getState().runPixelize(params);

    expect(dispatched()).toEqual([
      { cmd: "workflow_pixelize", args: { id: "doc-01", params } },
    ]);
    expect(outcome).toEqual(okOutcome);
  });

  it("没有直接 runner 的 kind 一条命令都不发，把原因写进 outcomeError", async () => {
    reset();

    for (const kind of ["quantize", "vision_brief", "video_brief", "prompt_refine"] as const) {
      const returned = await useStore
        .getState()
        .runWorkflow(kind, { prompt: "用不上" } as WorkflowParams);

      expect(returned).toBeNull();
      expect(useStore.getState().outcome).toBeNull();
      expect(useStore.getState().outcomeError).toContain("no direct runner");
      expect(useStore.getState().workflowBusy).toBe(false);
      const notice = useStore.getState().entries[useStore.getState().entries.length - 1];
      expect(notice).toMatchObject({ kind: "notice", isError: true });
    }

    // 这四个 kind 都得在前台自己点别的命令，一个 invoke 都不能漏给 Rust。
    expect(invokeCalls).toEqual([]);
  });

  it("后端拒绝时回执清空，原话进 outcomeError，busy 复位", async () => {
    reset();
    invokeErrors["workflow_image_gen"] = "生图模型不可用";

    const returned = await useStore
      .getState()
      .runWorkflow("image_gen", { prompt: "一只乌鸦起飞" });

    expect(returned).toBeNull();
    expect(useStore.getState().outcome).toBeNull();
    expect(useStore.getState().outcomeError).toBe("生图模型不可用");
    expect(useStore.getState().workflowBusy).toBe(false);
    const last = useStore.getState().entries[useStore.getState().entries.length - 1];
    expect(last).toMatchObject({ kind: "notice", isError: true, text: "生图模型不可用" });
  });

  it("没有会话时不发命令，只留一条报错通知", async () => {
    reset({ activeId: null });

    await useStore.getState().runWorkflow("image_gen", { prompt: "一只乌鸦起飞" });
    await useStore.getState().runPixelize({ image_base64: "AAAA" });

    expect(invokeCalls).toEqual([]);
    expect(useStore.getState().notice).toEqual({ text: "还没有会话", isError: true });
  });
});

describe("静默提醒（模型半天不吭声）", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("一句发出去之后，太久没有事件就亮提醒，重发照原样再来一遍", async () => {
    vi.useFakeTimers();
    useStore.setState({
      activeId: "doc-01",
      lang: "zh",
      entries: [],
      lastQuery: null,
      attachments: [],
      notice: null,
      models: { active_id: "", entries: [] },
    });
    invokeCalls.length = 0;

    await useStore.getState().send("画一只八帧橘猫行走图");
    // 发出去立刻有占位节点兜底，但还不算卡住。
    expect(useStore.getState().entries.at(-1)).toMatchObject({ kind: "pending" });
    expect(useStore.getState().stalled).toBe(false);

    vi.advanceTimersByTime(STALL_SECONDS * 1000 - 1);
    expect(useStore.getState().stalled).toBe(false);
    vi.advanceTimersByTime(2);
    expect(useStore.getState().stalled).toBe(true);

    // 出路只有两个：重发刚才那句，或者中断。先看重发。
    invokeCalls.length = 0;
    // 这一轮假死了，回合占用还压在 Rust 手上：重试得先把它收掉才发得出去。
    const retrying = useStore.getState().retry();
    await flushUntil(() => invokeCalls.some((call) => call.cmd === "agent_interrupt"));
    expect(invokeCalls.some((call) => call.cmd === "agent_interrupt")).toBe(true);
    publishAgent("doc-01", { kind: "interrupted" });
    await retrying;
    expect(invokeCalls.filter((call) => call.cmd === "agent_send_message")).toEqual([
      { cmd: "agent_send_message", args: { id: "doc-01", text: "画一只八帧橘猫行走图", attachments: [], modelId: null, style: null, presets: [] } },
    ]);
    expect(useStore.getState().stalled).toBe(false);
  });

  /** 退让出微任务队列，直到条件成立或让够了次数。用来等一条异步链走到某一步。 */
  async function flushUntil(ready: () => boolean, hops = 200): Promise<void> {
    for (let i = 0; i < hops && !ready(); i++) await Promise.resolve();
  }

  it("上一轮赖着不走时不硬发：把话说明白，不留第二个占位节点", async () => {
    vi.useFakeTimers();
    useStore.setState({
      activeId: "doc-01",
      lang: "zh",
      entries: [],
      lastQuery: null,
      attachments: [],
      running: false,
      stalled: false,
      notice: null,
    });

    await useStore.getState().send("画一只八帧橘猫行走图");
    expect(useStore.getState().running).toBe(true);

    // 中断打了，回执始终不来：等不到交还就不能硬发，那样只是把用户消息重复一遍。
    invokeCalls.length = 0;
    const sending = useStore.getState().send("再画一只");
    await vi.advanceTimersByTimeAsync(9000);
    await sending;

    expect(invokeCalls.some((call) => call.cmd === "agent_send_message")).toBe(false);
    expect(useStore.getState().notice).toMatchObject({ isError: true });
    expect(useStore.getState().entries.filter((entry) => entry.kind === "user")).toHaveLength(1);
    // 占位节点跟着失败一起封口，不留一枚一直闪的光标。
    expect(useStore.getState().entries.filter((entry) => entry.kind === "pending")).toHaveLength(0);
  });

  it("回车再发一句：先收掉上一轮，用户那句话在对话里只有一条", async () => {
    vi.useFakeTimers();
    useStore.setState({
      activeId: "doc-01",
      lang: "zh",
      entries: [],
      lastQuery: null,
      attachments: [],
      running: false,
      stalled: false,
      notice: null,
    });

    await useStore.getState().send("画一只八帧橘猫行走图");
    invokeCalls.length = 0;

    const sending = useStore.getState().send("顺便把背景也画了");
    await flushUntil(() => invokeCalls.some((call) => call.cmd === "agent_interrupt"));
    publishAgent("doc-01", { kind: "interrupted" });
    await sending;

    expect(invokeCalls.filter((call) => call.cmd === "agent_send_message")).toEqual([
      { cmd: "agent_send_message", args: { id: "doc-01", text: "顺便把背景也画了", attachments: [], modelId: null, style: null, presets: [] } },
    ]);
    expect(useStore.getState().entries.filter((entry) => entry.kind === "user")).toHaveLength(2);
    expect(useStore.getState().entries.filter((entry) => entry.kind === "pending")).toHaveLength(1);
  });

  it("锁了画风和收尾规矩就跟着这一句过去，自动则不带", async () => {
    vi.useFakeTimers();
    useStore.setState({
      activeId: "doc-01",
      lang: "zh",
      entries: [],
      lastQuery: null,
      attachments: [],
      running: false,
      stalled: false,
      notice: null,
      styleOverride: null,
    });

    /** 发一句，等它落到 invoke 上。用不到真时间，发送链本身是同步落库的。 */
    async function sendOnce(text: string) {
      useStore.setState({ running: false });
      const done = useStore.getState().send(text);
      await flushUntil(() => invokeCalls.some((call) => call.cmd === "agent_send_message"));
      await done;
      return invokeCalls.at(-1);
    }

    expect(await sendOnce("画一只写实的猫")).toEqual({
      cmd: "agent_send_message",
      args: {
        id: "doc-01",
        text: "画一只写实的猫",
        attachments: [],
        modelId: null,
        style: null,
        presets: [],
      },
    });

    // 换锁：下一次发送带的就是新锁，旧的那份不会赖着。
    useStore.getState().setStyleOverride("realistic");
    expect(await sendOnce("画一只猫")).toEqual({
      cmd: "agent_send_message",
      args: {
        id: "doc-01",
        text: "画一只猫",
        attachments: [],
        modelId: null,
        style: "realistic",
        presets: [],
      },
    });

    useStore.getState().setStyleOverride(null);
    expect((await sendOnce("画一只猫"))?.args.style).toBe(null);

    // 收尾规矩那一柄是平行的另一根轴：两把锁可以同时挂，互不覆盖。
    useStore.getState().setPresetOverrides(["cinematic"]);
    useStore.getState().setStyleOverride("gameboy");
    expect(await sendOnce("画一只猫")).toMatchObject({
      cmd: "agent_send_message",
      args: { style: "gameboy", presets: ["cinematic"] },
    });

    // 只摘画风：规矩那一柄留在原处，跟着下一次发送过去。
    useStore.getState().setStyleOverride(null);
    expect(await sendOnce("画一只猫")).toMatchObject({
      cmd: "agent_send_message",
      args: { style: null, presets: ["cinematic"] },
    });

    useStore.getState().setPresetOverrides([]);
    expect((await sendOnce("画一只猫"))?.args.presets).toEqual([]);

    // 叠加是这套机制的全部意义：「写实渲染」讲整张图按什么规矩收尾，
    // 「微细结构」讲最后一两个像素放在哪里，「闭塞接触」讲暗部怎么攒起来。
    // 三条各管一段，一起上路才凑成一张写实的图。
    useStore.getState().setPresetOverrides(["realistic", "microdetail", "occlusion"]);
    expect(await sendOnce("画一只猫")).toMatchObject({
      args: { presets: ["realistic", "microdetail", "occlusion"] },
    });

    // 第四条绝不静默丢：清单收在前三条，但通知要亮——点上去没反应，
    // 用户会盯着没变化的图猜原因，而不是猜自己点多了。
    useStore.getState().setPresetOverrides([
      "realistic",
      "microdetail",
      "occlusion",
      "polish",
    ]);
    expect(useStore.getState().presetOverrides).toEqual([
      "realistic",
      "microdetail",
      "occlusion",
    ]);
    expect(useStore.getState().notice).not.toBe(null);

    // 重复 id 和认不出的 id 都在这里收窄：下拉里只认 PRESET_IDS，但从别处
    // 塞进来的脏值不该一路漏到 Rust 让整条发送失败。
    useStore.getState().setPresetOverrides(["realistic", "realistic", "photoshop"]);
    expect(await sendOnce("画一只猫")).toMatchObject({
      args: { presets: ["realistic"] },
    });

    useStore.getState().setPresetOverrides([]);
    await vi.runAllTimersAsync();
    // 静默看护的定时器跟着假时间走，不在这里收掉会漏进下一个用例。
    vi.useRealTimers();
  });

  it("发不出去就把表撤了，不留一个到点自爆的计时器", async () => {
    vi.useFakeTimers();
    useStore.setState({ activeId: null, lang: "zh", entries: [], notice: null, running: false, stalled: false });

    await useStore.getState().send("画一只八帧橘猫行走图");
    expect(useStore.getState().running).toBe(false);
    expect(useStore.getState().notice).toMatchObject({ isError: true });

    vi.advanceTimersByTime(STALL_SECONDS * 1000 + 5);
    expect(useStore.getState().stalled).toBe(false);
  });

  it("发不出去就把占位节点收掉，别留一枚一直闪的光标", async () => {
    invokeErrors["agent_send_message"] = "connection refused";
    try {
      useStore.setState({
        activeId: "doc-01",
        lang: "zh",
        entries: [],
        lastQuery: null,
        attachments: [],
        notice: null,
        models: { active_id: "", entries: [] },
      });

      await useStore.getState().send("画一只八帧橘猫行走图");
      expect(useStore.getState().running).toBe(false);
      expect(useStore.getState().entries.some((e) => e.kind === "pending")).toBe(false);
      // 用户那句话还得在：失败的是发送，不是他说过的话。
      expect(useStore.getState().entries.map((e) => e.kind)).toEqual(["user"]);
    } finally {
      delete invokeErrors["agent_send_message"];
    }
  });
});

describe("回合只由它自己结束，侧道失败不陪葬", () => {
  afterEach(() => {
    delete invokeErrors["document_png_url"];
    delete invokeErrors["editor_paint_stroke"];
    delete invokeErrors["agent_set_active"];
  });

  /** 一个还在跑的回合，末尾挂着占位节点：模型那半截话没说完。 */
  function liveTurn() {
    useStore.setState({
      activeId: "doc-01",
      document: seedDocument(),
      frameIndex: 0,
      revision: 7,
      pngRevision: 7,
      active: { layer: "L0", frame: "F0", color: null },
      lang: "zh",
      entries: [{ key: "p-live", kind: "pending", thinking: true }],
      notice: null,
      running: true,
      runStartedAt: Date.now(),
      stalled: false,
    });
  }

  it("模型画画途中渲染失败：报警，但回合照跑、占位节点照闪", async () => {
    liveTurn();
    invokeErrors["document_png_url"] = "webview gone";
    await useStore.getState().refreshPng();

    const state = useStore.getState();
    expect(state.running).toBe(true);
    expect(state.runStartedAt).not.toBeNull();
    // 没封口：光标接着闪，用户才知道模型还在画。
    expect(state.entries.some((e) => e.kind === "pending")).toBe(true);
    expect(state.notice?.isError).toBe(true);
    expect(state.notice?.text).toContain("渲染画布失败");
  });

  it("权威 PNG 记着自己属于哪一帧：切了帧，旧图就得从画布上让位", async () => {
    liveTurn();
    invokeResults["document_png_url"] = "data:image/png;base64,AAAA";
    useStore.setState({ activeId: "doc-01", document: useStore.getState().document, pngFrame: -1 });
    await useStore.getState().refreshPng();
    // 图到手时顺手记下它是哪一帧的，之后界面照这个判断该不该显示它。
    expect(useStore.getState().pngFrame).toBe(0);

    // 切到第二帧：帧层是同步重画的，而权威图还在后端路上、仍属于第 0 帧。
    // 两个帧号对不上，旧图再挂着就是新旧两张脸叠在一起。
    useStore.setState({ frameIndex: 1 });
    expect(useStore.getState().pngFrame).toBe(0);
    delete invokeResults["document_png_url"];
  });

  it("用户落笔失败也只报警：画崩了不关模型那一回合的事", async () => {
    liveTurn();
    invokeErrors["editor_paint_stroke"] = "cel locked";
    await useStore.getState().paintStroke([{ x: 1, y: 2 }], null);

    const state = useStore.getState();
    expect(state.running).toBe(true);
    expect(state.entries.some((e) => e.kind === "pending")).toBe(true);
    expect(state.notice?.isError).toBe(true);
  });

  it("选中项回传失败只嘟囔一声，切层本身照样切过来", async () => {
    liveTurn();
    invokeErrors["agent_set_active"] = "session gone";
    await useStore.getState().setActiveLayer("L1");

    const state = useStore.getState();
    expect(state.active.layer).toBe("L1");
    expect(state.running).toBe(true);
    expect(state.notice?.isError).toBe(true);
    expect(state.notice?.text).toContain("同步选中项失败");
  });
});

describe("切走之后，上一个会话残着的事件不能落到这一轮", () => {
  /** 一个停在原地的回合：模型那半截话没说完。 */
  function liveTurn(doc: PixelDocument) {
    useStore.setState({
      activeId: "doc-01",
      document: doc,
      frameIndex: 0,
      revision: 3,
      pngRevision: 3,
      active: { layer: "L0", frame: "F0", color: null },
      lang: "zh",
      entries: [{ key: "p-live", kind: "pending", thinking: true }],
      notice: null,
      running: true,
      runStartedAt: Date.now(),
      stalled: false,
    });
  }

  function texts(state: ReturnType<typeof useStore.getState>): string[] {
    return state.entries.flatMap((entry) => ("text" in entry ? [entry.text] : []));
  }

  /** 整段对话拼成一片：token 是逐片追加的，逐条比对会漏掉「被接在尾巴上」。 */
  function transcript(state: ReturnType<typeof useStore.getState>): string {
    return texts(state).join("");
  }

  it("本会话的事件照收，别会话的 token 不往这一轮对话里拼", () => {
    const doc = seedDocument();
    liveTurn(doc);
    // 正面例子兼哨兵：它生效才说明监听器真的挂着，反例才有意义。
    publishAgent("doc-01", { kind: "token", text: "我这句" });
    expect(transcript(useStore.getState())).toContain("我这句");

    publishAgent("doc-other", { kind: "token", text: "别家这句" });
    expect(transcript(useStore.getState())).not.toContain("别家这句");
    // 正面那边没被动：别家那句话既没接上来，也没把已有内容顶掉。
    expect(transcript(useStore.getState())).toBe("我这句");
  });

  it("别会话的 document_updated 盖不掉这一轮的画布", () => {
    const doc = seedDocument();
    liveTurn(doc);
    const stranger = blankDocument(4, 3);

    publishAgent("doc-other", {
      kind: "document_updated",
      revision: 99,
      patch: fullPatch(stranger),
    });

    const state = useStore.getState();
    expect(state.document).toBe(doc);
    expect(state.revision).toBe(3);
  });

  it("别会话的收尾事件不给这一轮封口", () => {
    const doc = seedDocument();
    liveTurn(doc);
    expect(useStore.getState().running).toBe(true);

    publishAgent("doc-other", { kind: "completed", turns: 1 });

    const state = useStore.getState();
    expect(state.running).toBe(true);
    expect(state.entries.some((entry) => entry.kind === "pending")).toBe(true);
    expect(state.runStartedAt).not.toBeNull();
  });
});

describe("新建会话与模型定义改动", () => {
  /** 一条会话概览：侧栏摆的名字和画布尺寸都从它身上读。 */
  function sessionOf(width: number, height: number, label = "一号模型"): SessionInfo {
    return {
      id: "s-2",
      model_id: "m1",
      model_label: label,
      roles: [],
      width,
      height,
      revision: 0,
      title: null,
      order: 2,
    };
  }

  /** 设置表单保存下来的一条模型定义；密钥留空，本机已存那把由 Rust 合并。 */
  function modelConfig(label: string): ModelConfig {
    return {
      id: "m1",
      label,
      protocol: "open_ai_compat",
      base_url: "https://example.invalid/v1",
      api_key: "",
      model: "test-model",
      max_tokens: 4096,
      temperature: null,
      disable_thinking: null,
      capabilities: { vision: false, image_gen: false, video: false, reasoning: false },
    };
  }

  function modelView(label: string): ModelView {
    // disable_thinking 在视图里是必填：表单那份是可选的，这里补成「不干预」。
    return { ...modelConfig(label), disable_thinking: null, has_api_key: true };
  }

  it("新建会话把用户选的宽高交给 Rust，并告知左上角 WxH 能改尺寸", async () => {
    const created = sessionOf(48, 96);
    invokeResults["session_create"] = created;
    invokeResults["agent_document"] = { id: "s-2", revision: 0, document: blankDocument(48, 96) };
    invokeResults["session_list"] = [created];
    invokeResults["workflow_catalog"] = [];
    useStore.setState({ activeId: null, sessions: [] });
    invokeCalls.length = 0;

    await useStore.getState().createSession(48, 96);

    const document = invokeCalls.find((call) => call.cmd === "session_create")?.args
      .document as { width: number; height: number } | undefined;
    expect(document?.width).toBe(48);
    expect(document?.height).toBe(96);
    const notice = useStore.getState().notice;
    expect(notice?.isError).toBe(false);
    expect(notice?.text).toContain("48");
    expect(notice?.text).toContain("96");
    expect(notice?.text).toContain("WxH");
  });

  it("没选宽高的自动补建不弹通知：删掉最后一个会话时不该吵用户", async () => {
    const created = sessionOf(64, 64);
    invokeResults["session_create"] = created;
    invokeResults["agent_document"] = { id: "s-2", revision: 0, document: blankDocument(64, 64) };
    invokeResults["session_list"] = [created];
    invokeResults["workflow_catalog"] = [];
    useStore.setState({ activeId: null, sessions: [], notice: null });
    invokeCalls.length = 0;

    await useStore.getState().createSession();

    expect(useStore.getState().notice).toBeNull();
  });

  // 会话名：填了就带到 Rust 去，空白名当没填。少了这一条，用户填的名字会被
  // 悄悄丢掉，侧栏只剩 s1、s2，或者反过来多出一个空名会话。
  it("新建会话把用户填的名字带给 Rust，空白名当没填", async () => {
    const created = sessionOf(32, 32);
    invokeResults["session_create"] = created;
    invokeResults["agent_document"] = { id: "s-2", revision: 0, document: blankDocument(32, 32) };
    invokeResults["session_list"] = [created];
    invokeResults["workflow_catalog"] = [];
    useStore.setState({ activeId: null, sessions: [], notice: null });
    invokeCalls.length = 0;

    await useStore.getState().createSession(32, 32, "橘猫");

    const sent = invokeCalls.find((call) => call.cmd === "session_create")?.args;
    expect(sent?.title).toBe("橘猫");
    expect(useStore.getState().activeId).toBe(created.id);

    invokeCalls.length = 0;
    await useStore.getState().createSession(32, 32, "   ");

    const blank = invokeCalls.find((call) => call.cmd === "session_create")?.args;
    expect(blank?.title).toBeNull();
  });

  it("改完模型定义要重拉会话列表，侧栏名字才跟着设置走", async () => {
    const renamed = sessionOf(64, 64, "新名字");
    invokeResults["model_upsert"] = { active_id: "m1", entries: [modelView("新名字")] };
    invokeResults["session_list"] = [renamed];
    invokeResults["workflow_catalog"] = [];
    useStore.setState({ activeId: "s-2", sessions: [sessionOf(64, 64)] });
    invokeCalls.length = 0;

    await useStore.getState().upsertModel(modelConfig("新名字"));

    expect(invokeCalls.some((call) => call.cmd === "session_list")).toBe(true);
    expect(useStore.getState().sessions.map((s) => s.model_label)).toEqual(["新名字"]);
  });

  it("删掉正绑着的模型定义也要重拉会话列表", async () => {
    const fallenBack = sessionOf(64, 64, "二号模型");
    invokeResults["model_remove"] = { active_id: "m2", entries: [modelView("二号模型")] };
    invokeResults["session_list"] = [fallenBack];
    invokeResults["workflow_catalog"] = [];
    useStore.setState({ activeId: "s-2", sessions: [sessionOf(64, 64)] });
    invokeCalls.length = 0;

    await useStore.getState().removeModel("m1");

    expect(invokeCalls.some((call) => call.cmd === "session_list")).toBe(true);
    expect(useStore.getState().sessions.map((s) => s.model_label)).toEqual(["二号模型"]);
  });

  it("左上角 WxH 改尺寸：把新宽高交给 Rust，再拉回一份文档", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: blankDocument(64, 64),
      notice: null,
    });
    invokeResults["editor_resize_canvas"] = 7;
    invokeResults["agent_document"] = {
      id: "doc-01",
      revision: 7,
      document: blankDocument(96, 48),
    };
    invokeResults["session_list"] = [{ ...sessionOf(96, 48), id: "doc-01" }];
    invokeCalls.length = 0;

    await useStore.getState().resizeCanvas(96, 48);

    const resize = invokeCalls.find((call) => call.cmd === "editor_resize_canvas");
    expect(resize?.args).toMatchObject({ id: "doc-01", width: 96, height: 48 });
    expect(useStore.getState().document?.width).toBe(96);
    expect(useStore.getState().document?.height).toBe(48);
    expect(useStore.getState().sessions[0]?.width).toBe(96);
  });

  it("尺寸没变就不惊动 Rust，也弹不出通知", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: blankDocument(64, 64),
      notice: null,
    });
    invokeCalls.length = 0;

    await useStore.getState().resizeCanvas(64, 64);

    expect(invokeCalls.some((call) => call.cmd === "editor_resize_canvas")).toBe(false);
    expect(useStore.getState().notice).toBeNull();
  });

  it("Rust 拒了这次改尺寸，界面要说清楚", async () => {
    useStore.setState({
      activeId: "doc-01",
      document: blankDocument(64, 64),
      notice: null,
    });
    invokeErrors["editor_resize_canvas"] = "boom";
    invokeCalls.length = 0;

    await useStore.getState().resizeCanvas(128, 128);

    expect(useStore.getState().notice?.isError).toBe(true);
    delete invokeErrors["editor_resize_canvas"];
  });
});

describe("会话改名与排序", () => {
  it("改名只点名 id 和新名字，排序把整串 id 交给 Rust", async () => {
    invokeResults["session_list"] = [];
    useStore.setState({ sessions: [] });
    invokeCalls.length = 0;

    await useStore.getState().renameSession("doc-01", "橘猫项目");
    await useStore.getState().reorderSessions(["b", "a", "c"]);

    const rename = invokeCalls.find((call) => call.cmd === "session_rename");
    expect(rename?.args).toMatchObject({ id: "doc-01", title: "橘猫项目" });
    const reorder = invokeCalls.find((call) => call.cmd === "session_reorder");
    expect(reorder?.args).toMatchObject({ ids: ["b", "a", "c"] });
  });

  it("改名失败只嘟囔一声，不动会话列表", async () => {
    invokeErrors["session_rename"] = "nope";
    invokeResults["session_list"] = [{ ...{ id: "doc-02" } } as SessionInfo];
    useStore.setState({ sessions: [] });
    invokeCalls.length = 0;

    await useStore.getState().renameSession("doc-02", "新名字");

    expect(useStore.getState().notice?.isError).toBe(true);
    delete invokeErrors["session_rename"];
  });
});

describe("删除会话要先问过，失败了还得留着", () => {
  it("删成功才点名 session_drop，并把会话从列表里摘掉", async () => {
    const a = { id: "doc-a", title: "橘猫" } as SessionInfo;
    const b = { id: "doc-b", title: "小狗" } as SessionInfo;
    useStore.setState({ sessions: [a, b], activeId: "doc-a" });
    invokeCalls.length = 0;
    // 删的是当前会话，所以收尾会切到剩下那条、顺手重拉一遍列表。
    // 预置返回值：不然 mock 会把编辑器操作的默认值 1 当成会话列表塞回来。
    invokeResults["session_list"] = [b];

    await useStore.getState().removeSession("doc-a");

    const drop = invokeCalls.find((call) => call.cmd === "session_drop");
    expect(drop?.args).toMatchObject({ id: "doc-a" });
    expect(useStore.getState().sessions.map((s) => s.id)).toEqual(["doc-b"]);
  });

  it("Rust 没删成：会话列表原样不动，通知说明原因，并且往上抛", async () => {
    // 确认弹窗靠这个 rethrow 停在原处：删失败不能装作成功把窗子关掉，
    // 不然用户以为删干净了，其实那一会话连聊天带画布还在盘上。
    const a = { id: "doc-a", title: "橘猫" } as SessionInfo;
    useStore.setState({ sessions: [a], activeId: "doc-a" });
    invokeCalls.length = 0;
    invokeErrors["session_drop"] = "会话还在跑";

    await expect(useStore.getState().removeSession("doc-a")).rejects.toThrow("会话还在跑");

    expect(useStore.getState().sessions.map((s) => s.id)).toEqual(["doc-a"]);
    expect(useStore.getState().notice?.isError).toBe(true);
    delete invokeErrors["session_drop"];
  });
});

describe("画布刷新靠 revision 翻倍", () => {
  it("落笔之后文档和 revision 一起涨，帧缩略图这种二线视图才跟得上", async () => {
    const doc = seedDocument();
    const stubs = {
      agent_list_models: { active_id: "", entries: [] },
      agent_history: [],
      batch_recipes_list: [],
      session_list: [],
    };
    Object.assign(invokeResults, stubs);
    try {
      await useStore.getState().boot();
    } finally {
      for (const key of Object.keys(stubs)) delete invokeResults[key];
    }

    useStore.setState({
      activeId: "doc-01",
      document: doc,
      revision: 4,
      pngRevision: 4,
    });
    invokeCalls.length = 0;

    await useStore.getState().paintStroke([{ x: 1, y: 1 }]);
    const stroke = invokeCalls.find((call) => call.cmd === "editor_paint_stroke");
    expect(stroke?.args).toMatchObject({
      id: "doc-01",
      stroke: { layer: "L0", frame: "F0", cells: [{ x: 1, y: 1 }] },
    });
    // Rust 广播回来的 revision 是新的：拿它当第二把钥匙的画布与缩略图才重绘。
    const painted: PixelDocument = {
      ...doc,
      // 像素住在 cel 里：第一层第一帧的第一格染上调色板 1 号色。
      cels: {
        ...doc.cels,
        L0: { F0: { indices: [1, ...doc.cels.L0.F0.indices.slice(1)] } },
      },
    };
    publishAgent("doc-01", { kind: "document_updated", revision: 5, patch: fullPatch(painted) });

    const state = useStore.getState();
    expect(state.revision).toBe(5);
    expect(state.document).toEqual(painted);
  });

  it("增量只带改过的 cel，没动的 cel 原样留着", async () => {
    const doc = seedDocument();
    useStore.setState({
      activeId: "doc-01",
      document: doc,
      revision: doc.revision,
      pngRevision: doc.revision,
    });
    // 后端 diff 出来的增量：元数据全给，cel 只报动过的那一格。
    const touched = [2, ...doc.cels.L0.F0.indices.slice(1)];
    publishAgent("doc-01", {
      kind: "document_updated",
      revision: 12,
      patch: {
        ...fullPatch({ ...doc, revision: 12 }),
        cels: [["L0", "F0", touched]],
      },
    });

    const state = useStore.getState();
    expect(state.revision).toBe(12);
    // 改过的那一格落下去了，别的层、别的帧一格都没丢——增量是合并不是替换。
    expect(state.document?.cels.L0.F0).toEqual({ indices: touched });
    expect(state.document?.cels.L1).toEqual(doc.cels.L1);
    expect(state.document?.cels.L0.F1).toEqual(doc.cels.L0.F1);
  });

  it("增量里的 dropped 把删掉的 cel 摘下去", async () => {
    const doc = seedDocument();
    useStore.setState({
      activeId: "doc-01",
      document: doc,
      revision: doc.revision,
      pngRevision: doc.revision,
    });
    // 后端那边删掉的 cel 只会出现在 dropped 里，不会再出现在 cels 里。
    const patch = fullPatch({ ...doc, revision: 13 });
    patch.cels = patch.cels.filter(([layerId, frameId]) => layerId !== "L1" || frameId !== "F1");
    publishAgent("doc-01", {
      kind: "document_updated",
      revision: 13,
      patch: { ...patch, dropped: [["L1", "F1"]] },
    });

    expect(useStore.getState().document?.cels.L1.F1).toBeUndefined();
    expect(useStore.getState().document?.cels.L1.F0).toBeDefined();
  });

  it("带着 dropped 的那条增量丢了，下一条也照元数据把死层收尸", () => {
    const doc = seedDocument();
    useStore.setState({
      activeId: "doc-01",
      document: doc,
      revision: doc.revision,
      pngRevision: doc.revision,
    });
    // 后端已经删掉 L1：元数据里只剩 L0 和它那一帧，dropped 却整条没送到
    // （事件桥还没挂上监听、窗口藏着那一阵都可能）。合并不能只会等 dropped，
    // 层号是会复用的——不收尸的话新层拿回 "L1" 时，旧像素跟着新层一起显形。
    const after: PixelDocument = {
      ...doc,
      layers: doc.layers.filter((layer) => layer.id !== "L1"),
      frames: doc.frames.filter((frame) => frame.id !== "F1"),
      cels: { L0: { F0: doc.cels.L0.F0 } },
    };
    publishAgent("doc-01", {
      kind: "document_updated",
      revision: 20,
      patch: { ...fullPatch({ ...after, revision: 20 }), cels: [], dropped: [] },
    });

    expect(useStore.getState().document?.cels.L1).toBeUndefined();
    expect(useStore.getState().document?.cels.L0.F1).toBeUndefined();
    expect(useStore.getState().document?.cels.L0.F0).toEqual(doc.cels.L0.F0);
  });

  it("选色归队只在本层配色范围里找，不拿文档自带调色板顶数", () => {
    const base = seedDocument();
    const ink = { r: 255, g: 119, b: 168, a: 255 };
    const doc: PixelDocument = {
      ...base,
      // 基础调色板里正摆着当前这支笔的色：拿它当归队候选，这支色原地不动，
      // 而它压根不在 L1 自己的范围（Game Boy 四色）里。
      palette: [ink, { r: 0, g: 0, b: 0, a: 255 }],
      layers: [base.layers[0], { ...base.layers[1], palette_id: "gameboy" }],
    };
    useStore.setState({
      activeId: "doc-01",
      document: doc,
      revision: 4,
      pngRevision: 4,
      frameIndex: 0,
      active: { layer: "L1", frame: "F1", color: "#ff77a8" },
    });

    publishAgent("doc-01", {
      kind: "document_updated",
      revision: 5,
      patch: fullPatch({ ...doc, revision: 5 }),
    });

    // 落回 L1 自己的四色里：各层配色范围独立，不该被文档级调色板牵着走。
    expect(["#0f380f", "#306230", "#8bac0f", "#9bbc0f"]).toContain(
      useStore.getState().active.color,
    );
  });
});

describe("syncSelection 把选中对齐到后端", () => {
  // 生图 / 抽帧在文档末尾添了一帧，Rust 顺手把 active.frame 挪过去。前端不跟的话
  // 新帧画好了，帧条高亮和「接下来改哪儿」还留在上一格，用户会以为没生效。
  it("follows the frame the backend just moved to", async () => {
    const doc = blankDocument(4, 4);
    const grown = {
      ...doc,
      frames: [{ id: "F0", duration_ms: 100 }, { id: "F1", duration_ms: 100 }],
      cels: { L0: { F0: doc.cels.L0.F0, F1: doc.cels.L0.F0 } },
    };
    invokeCalls.length = 0;
    invokeResults["agent_document"] = {
      id: "doc-01",
      revision: 9,
      document: grown,
      active: { layer: "L0", frame: "F1", color: null },
    };
    useStore.setState({
      activeId: "doc-01",
      document: grown,
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: "#111111" },
    });

    await useStore.getState().syncSelection();

    const state = useStore.getState();
    expect(state.frameIndex).toBe(1);
    expect(state.active.frame).toBe("F1");
    // 用户挑的颜色不能因为对齐选中被抹掉。
    expect(state.active.color).toBe("#111111");
    // 拉的是会话自己的快照，不是重新读一遍历史；选中动过才补一张新 png。
    expect(invokeCalls.map((call) => call.cmd)).toEqual(["agent_document", "document_png_url"]);
    delete invokeResults["agent_document"];
  });

  it("stays put when the backend says the same frame", async () => {
    const doc = blankDocument(4, 4);
    invokeCalls.length = 0;
    invokeResults["agent_document"] = {
      id: "doc-01",
      revision: 9,
      document: doc,
      active: { layer: "L0", frame: "F0", color: null },
    };
    const pngBefore = useStore.getState().pngUrl;
    useStore.setState({
      activeId: "doc-01",
      document: doc,
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: null },
    });

    await useStore.getState().syncSelection();

    const state = useStore.getState();
    expect(state.frameIndex).toBe(0);
    expect(state.active.frame).toBe("F0");
    expect(state.pngUrl).toBe(pngBefore);
    delete invokeResults["agent_document"];
  });
});

describe("工具块的展开态按调用 id 记", () => {
  it("没记过就照默认姿态取反，分流节点默认摊开所以第一下是收起", () => {
    useStore.setState({ toolOpen: {} });

    useStore.getState().toggleToolOpen("call_plan", true);
    expect(useStore.getState().toolOpen).toEqual({ call_plan: false });
    useStore.getState().toggleToolOpen("call_plan", true);
    expect(useStore.getState().toolOpen).toEqual({ call_plan: true });

    useStore.getState().toggleToolOpen("call_ops", false);
    expect(useStore.getState().toolOpen).toEqual({ call_plan: true, call_ops: true });
  });

  // 切会话会把条目列表按 Rust 历史重建，EntryRow 整行重挂载。状态挂在 id 上，
  // 重建多少次用户的摊开/收起都还在，不会自己合上。
  it("rebuilds the entry list without forgetting the user's choice", () => {
    useStore.setState({ toolOpen: {} });
    useStore.getState().toggleToolOpen("call_ops", false);
    expect(useStore.getState().toolOpen).toEqual({ call_ops: true });

    useStore.getState().toggleToolOpen("call_ops", false);
    expect(useStore.getState().toolOpen).toEqual({ call_ops: false });
    // 换一个 id 不牵连前一个：各条工具块自己记自己的。
    useStore.getState().toggleToolOpen("call_read", false);
    expect(useStore.getState().toolOpen).toEqual({ call_ops: false, call_read: true });
    useStore.setState({ toolOpen: {} });
  });
});

describe("连点撤销/重做", () => {
  /** 往 agent_sync_document 上发货门：不放行，那趟拍回就一直悬着。 */
  function gateSync(): { release: () => void } {
    let release: () => void = () => {};
    const gate = new Promise<number>((resolve) => {
      release = () => resolve(1);
    });
    invokeResults["agent_sync_document"] = gate;
    return { release };
  }

  function syncDocs(): PixelDocument[] {
    return invokeCalls
      .filter((call) => call.cmd === "agent_sync_document")
      .map((call) => call.args.document as PixelDocument);
  }

  it("第二下等第一下真落地，两步撤销不会退成一步", async () => {
    const first = { ...seedDocument(), revision: 3 };
    const second = { ...seedDocument(), revision: 4 };
    const third = { ...seedDocument(), revision: 5 };
    const current = { ...seedDocument(), revision: 6 };
    useStore.setState({
      activeId: "doc-01",
      document: current,
      revision: 6,
      pngRevision: 6,
      undoStack: [first, second, third],
      redoStack: [],
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: "#ffffff" },
    });
    const gate = gateSync();
    try {
      // 连着点两下，中间不等：真实用户连点撤销就是这么点的。
      const firstClick = useStore.getState().undoEdit();
      const secondClick = useStore.getState().undoEdit();
      // 让出一次微任务队列：第一下已经发起拍回，第二下必须还悬在闸门外。
      await Promise.resolve();
      expect(syncDocs().map((doc) => doc.revision)).toEqual([5]);
      gate.release();
      await Promise.all([firstClick, secondClick]);
      // 按第三份、第二份的顺序拍过去，两步都算数。
      expect(syncDocs().map((doc) => doc.revision)).toEqual([5, 4]);
    } finally {
      delete invokeResults["agent_sync_document"];
    }
    // 文档停在第二份上；重做栈顶是刚退掉的那一份，再点一次redo只退回一步。
    expect(useStore.getState().document).toBe(second);
    expect(useStore.getState().undoStack).toEqual([first]);
    expect(useStore.getState().redoStack).toEqual([current, third]);

    await useStore.getState().redoEdit();
    expect(useStore.getState().document).toBe(third);
    expect(useStore.getState().redoStack).toEqual([current]);
  });

  it("拍回失败就把乐观换上的文档原样还回去", async () => {
    const first = { ...seedDocument(), revision: 3 };
    const current = { ...seedDocument(), revision: 6 };
    useStore.setState({
      activeId: "doc-01",
      document: current,
      revision: 6,
      pngRevision: 6,
      undoStack: [first],
      redoStack: [],
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: "#ffffff" },
    });
    invokeErrors["agent_sync_document"] = "sync rejected";
    try {
      await useStore.getState().undoEdit();
      expect(useStore.getState().document).toBe(current);
      expect(useStore.getState().undoStack).toEqual([first]);
      expect(useStore.getState().redoStack).toEqual([]);
    } finally {
      delete invokeErrors["agent_sync_document"];
    }
  });
});

describe("整份快照晚到，不能盖掉模型刚画的那一笔", () => {
  /** 当前会话里停着一份 revision 更高的文档：模型还在往里画。 */
  function live() {
    const doc = seedDocument();
    useStore.setState({
      activeId: "doc-01",
      document: doc,
      revision: 10,
      pngRevision: 10,
      undoStack: [],
      redoStack: [],
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: "#ffffff" },
    });
    return doc;
  }

  /** 一份号更小的旧快照：往返半路上模型又改过画布，它就是那个过时状态。 */
  function staleSnapshot() {
    return { id: "doc-01", revision: 7, document: { ...seedDocument(), revision: 7 } };
  }

  it("revision 更低的快照直接不收", async () => {
    const doc = live();
    invokeResults["agent_document"] = staleSnapshot();
    try {
      await useStore.getState().refreshDocument();
    } finally {
      delete invokeResults["agent_document"];
    }
    // 收下这份旧快照，模型刚并进本地的那一笔就被抹掉了，而后端广播基线已经
    // 推进过去，那一笔再也不会补发。
    expect(useStore.getState().document).toBe(doc);
    expect(useStore.getState().revision).toBe(10);
  });

  it("导入了外来文件时要认领低号文档", async () => {
    live();
    invokeResults["agent_document"] = staleSnapshot();
    try {
      await useStore.getState().refreshDocument(true);
    } finally {
      delete invokeResults["agent_document"];
    }
    expect(useStore.getState().revision).toBe(7);
  });
});

describe("号更小的增量也得照合", () => {
  it("撤销把计数器退回旧号之后，模型的增量不能当成过期丢掉", () => {
    const doc = seedDocument();
    useStore.setState({
      activeId: "doc-01",
      document: doc,
      revision: 10,
      pngRevision: 10,
      frameIndex: 0,
      active: { layer: "L0", frame: "F0", color: "#ffffff" },
    });
    // patch 是「相对上一次广播」的增量，不是第 N 版全量：号小不代表内容旧。
    const touched = [7, ...doc.cels.L0.F0.indices.slice(1)];
    publishAgent("doc-01", {
      kind: "document_updated",
      revision: 6,
      patch: {
        ...fullPatch({ ...doc, revision: 6 }),
        cels: [["L0", "F0", touched]],
      },
    });

    const state = useStore.getState();
    expect(state.revision).toBe(6);
    expect(state.pngRevision).toBe(6);
    expect(state.document?.cels.L0.F0).toEqual({ indices: touched });
  });
});
