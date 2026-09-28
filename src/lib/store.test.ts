import { describe, expect, it, vi } from "vitest";

import { briefToText, probeSummary, videoBriefToText } from "./dock-format";
import { DEFAULT_BATCH_RECIPE, EMPTY_BATCH_RUN } from "./batch";
import { blankDocument, useStore } from "./store";
import type {
  BatchRecipe,
  BatchRecipeEntry,
  BatchScan,
  RecipeImportReport,
  Layer,
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

/** 两图层两帧的文档：帧时长与图层排序的边界都要有两个元素才测得出来。 */
function seedDocument(): PixelDocument {
  const base = blankDocument(8, 8);
  const second: Layer = { id: "L1", name: "Layer 2", visible: true, opacity: 255 };
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

    useStore.getState().usePromptInGen("一只乌鸦起飞", "/tmp/crow.png");

    const draft = useStore.getState().dockDraft;
    expect(useStore.getState().kind).toBe("image_gen");
    expect(draft.prompt).toBe("一只乌鸦起飞");
    // 只看过描述等于把原图扔了：垫图必须是刚挑的那张示例图。
    expect(draft.genSource).toBe("file");
    expect(draft.genPath).toBe("/tmp/crow.png");
  });

  it("不带参考图时，原来选好的垫图方式原样留着", () => {
    useStore.getState().patchDraft({ genSource: "frame", genFrame: "F1", genPath: null });

    useStore.getState().usePromptInGen("把头饰画大一点");

    const draft = useStore.getState().dockDraft;
    expect(draft.genSource).toBe("frame");
    expect(draft.genFrame).toBe("F1");
    expect(draft.genPath).toBeNull();
  });
});
