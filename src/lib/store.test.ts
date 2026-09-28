import { describe, expect, it, vi } from "vitest";

import { briefToText, probeSummary, videoBriefToText } from "./dock-format";
import { DEFAULT_BATCH_RECIPE, EMPTY_BATCH_RUN } from "./batch";
import { blankDocument, useStore, type WorkflowParams } from "./store";
import type {
  BatchRecipe,
  BatchRecipeEntry,
  BatchScan,
  ModelRole,
  RecipeImportReport,
  RoleBinding,
  SessionInfo,
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

    expect(invokeCalls).toEqual([
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

    expect(invokeCalls).toEqual([{ cmd: "workflow_tween", args: { id: "doc-01", params } }]);
  });

  it("video_frames 落到 workflow_video_frames，路径与抽帧数原样传", async () => {
    reset();
    const params: WorkflowParams = { path: "/tmp/clip.mp4", count: 0, duration_ms: 250 };

    await useStore.getState().runWorkflow("video_frames", params);

    expect(invokeCalls).toEqual([
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

    expect(invokeCalls).toEqual([
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
