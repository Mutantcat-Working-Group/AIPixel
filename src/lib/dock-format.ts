// 工作流坞里「给人看 / 给模型看」的几段纯文本。
// 抽出来单测：坞组件只管渲染，这里的措辞错了会直接误导用户或模型。

import type { ProbeSource, VideoBrief, VideoProbe, VisionBrief } from "./types";

/**
 * 视觉简报压成一段提示词：标签用 snake_case，模型比中文标签更买账。
 * palette 是生成简报时被要求「由亮到暗」排的，但收这段话的 agent 不知道，
 * 不标注它就会把顺序当成随意罗列，丢掉明暗信息。
 */
export function briefToText(brief: VisionBrief): string {
  const rows: [string, string][] = [
    ["subject", brief.subject],
    ["silhouette", brief.silhouette],
    ["pose", brief.pose_notes],
    ["proportions", brief.proportions],
    ["craft", brief.craft_notes],
  ];
  const lines = rows
    .filter(([, text]) => text.trim() !== "")
    .map(([label, text]) => `${label}: ${text}`);
  if (brief.palette.length > 0) {
    lines.push(`palette (light to dark): ${brief.palette.join(", ")}`);
  }
  return lines.join("\n");
}

/**
 * 运动简报压成一段提示词。key_poses 是有顺序的，用 " | " 连起来，
 * 不然模型分不清哪个姿态在前。
 */
export function videoBriefToText(brief: VideoBrief): string {
  const rows: [string, string][] = [
    ["subject", brief.subject],
    ["motion", brief.motion],
    ["key_poses", brief.key_poses.join(" | ")],
    ["timing", brief.timing],
    ["craft", brief.craft_notes],
  ];
  const lines = rows
    .filter(([, text]) => text.trim() !== "")
    .map(([label, text]) => `${label}: ${text}`);
  if (brief.palette.length > 0) {
    lines.push(`palette (light to dark): ${brief.palette.join(", ")}`);
  }
  return lines.join("\n");
}

/** 探针摘要。目录来源没有 fps / duration 可言，文案得跟着 source 换。 */
export function probeSummary(probe: VideoProbe, source: ProbeSource): string {
  if (source === "directory") {
    const count = probe.frame_count ?? 0;
    const size = probe.width && probe.height ? ` at ${probe.width}x${probe.height}` : "";
    return `${count} still(s)${size}`;
  }
  const size = probe.width && probe.height ? `${probe.width}x${probe.height}` : "unknown size";
  const fps = probe.fps === null ? "unknown fps" : `${probe.fps.toFixed(2)} fps`;
  const duration =
    probe.duration_s === null ? "unknown length" : `${probe.duration_s.toFixed(1)}s`;
  const audio = probe.has_audio ? " with audio" : "";
  return `${size}, ${fps}, ${duration}${audio}`;
}
