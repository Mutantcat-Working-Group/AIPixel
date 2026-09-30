// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";

import { briefToText, probeSummary, videoBriefToText } from "./dock-format";
import { DEFAULT_BATCH_RECIPE, EMPTY_BATCH_RUN } from "./batch";
import { AGENT_EVENT_CHANNEL } from "./bridge";
import { STALL_SECONDS, blankDocument, useStore, type WorkflowParams } from "./store";
import { publishLocal } from "./local-bus";
import type {
  BatchRecipe,
  BatchRecipeEntry,
  BatchScan,
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
  it("落笔失败就缴械，紧跟着模型那次改动不该被塞进撤销栈", async () => {
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
    publishAgent("doc-01", { kind: "document_updated", revision: 5, document: doc });
    expect(useStore.getState().undoStack).toEqual([doc]);

    // 失败的那一笔不产新文档，旗子却还悬着：下一个到达的 document_updated
    // 会把它当成编辑器笔触吃掉，用户自己随后那一下笔反而没了撤销。
    invokeErrors["editor_paint_stroke"] = "stroke rejected";
    try {
      await useStore.getState().paintStroke([{ x: 2, y: 2 }]);
      expect(useStore.getState().undoStack).toEqual([doc]);
      // 模型自己的一次改动（跑完 Lua 脚本）：现在不该再进撤销栈。
      publishAgent("doc-01", { kind: "document_updated", revision: 6, document: doc });
      expect(useStore.getState().undoStack).toEqual([doc]);
    } finally {
      delete invokeErrors["editor_paint_stroke"];
    }
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
    await useStore.getState().retry();
    expect(invokeCalls.filter((call) => call.cmd === "agent_send_message")).toEqual([
      { cmd: "agent_send_message", args: { id: "doc-01", text: "画一只八帧橘猫行走图", attachments: [], modelId: null } },
    ]);
    expect(useStore.getState().stalled).toBe(false);
  });

  it("发不出去就把表撤了，不留一个到点自爆的计时器", async () => {
    vi.useFakeTimers();
    useStore.setState({ activeId: null, lang: "zh", entries: [], notice: null });

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
      document: stranger,
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
    publishAgent("doc-01", { kind: "document_updated", revision: 5, document: painted });

    const state = useStore.getState();
    expect(state.revision).toBe(5);
    expect(state.document).toEqual(painted);
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
