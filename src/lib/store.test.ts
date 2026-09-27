import { describe, expect, it } from "vitest";

import { briefToText, probeSummary, videoBriefToText } from "./dock-format";
import { blankDocument } from "./store";
import type { VideoBrief, VideoProbe, VisionBrief } from "./types";

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
        "palette: #2b1d18, #c9603a, #f2d3a8",
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
        "palette: #1b1b22, #3d4a6b, #e8e2d0",
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
