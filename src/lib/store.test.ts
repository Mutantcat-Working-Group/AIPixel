import { describe, expect, it, vi } from "vitest";

import { briefToText, probeSummary, videoBriefToText } from "./dock-format";
import { blankDocument, useStore } from "./store";
import type { Layer, PixelDocument, VideoBrief, VideoProbe, VisionBrief } from "./types";

// Rust 后端只活在桌面进程里，这里把 invoke 整个接住：桥接层的每个函数最终都落到
// 这一条命令调用上，所以侧栏那四个编辑动作到底往 Rust 发了什么，看它就行。
const invokeCalls = vi.hoisted(
  () => [] as Array<{ cmd: string; args: Record<string, unknown> }>,
);

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => {
    invokeCalls.push({ cmd, args: args ?? {} });
    // 编辑器操作统一回报一个新 revision，真实环境由 Rust 校验 op。
    return Promise.resolve(1);
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
