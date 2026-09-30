// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 工作流目录与能力模型。
//!
//! 「只用用户自带模型」意味着不同模型能做的事差别很大：有的只会聊天，有的能读图，
//! 有的能直接生图，少数能读视频。与其在提示词里让模型猜，不如把流程显式分成几类，
//! 每类声明自己需要的能力；UI 只展示当前模型跑得动的那几个。
//!
//! 这层是纯逻辑，不发请求、不碰文档，方便测试，也方便 Tauri 层直接序列化给前端。

use serde::{Deserialize, Serialize};

pub use super::models::Capabilities;

/// 七条工作流。三条吃模型专有能力（生图、读图、读视频），其余只要有会话模型就能跑；
/// 其中视频抽帧与补间是纯本机的，量化同理，不在能力目录里、单独挂在坞的末尾。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowKind {
    /// 默认：文本 agent 主循环，用 Lua 沙箱和类型化操作落笔。
    Agent,
    /// 生图模型出图 -> 量化成像素文档（支持示例图垫图）。
    ImageGen,
    /// 读图模型把参考图读成结构化简报，再交给 agent 主循环。
    VisionBrief,
    /// 抽帧得到关键帧序列，逐帧量化。纯本机，不请求模型。
    VideoFrames,
    /// 读视频模型把一段视频读成结构化运动简报，再交给 agent 主循环。
    VideoBrief,
    /// 在两个已有帧之间插帧（本机计算，不吃模型额度）。
    FrameTween,
    /// 提示词微调：把一句大白话改写成结构化生图提示词。
    PromptRefine,
}

impl WorkflowKind {
    pub fn needs_vision(self) -> bool {
        matches!(self, WorkflowKind::VisionBrief)
    }

    pub fn needs_image_gen(self) -> bool {
        matches!(self, WorkflowKind::ImageGen)
    }

    pub fn needs_video(self) -> bool {
        // 只有「模型真的要看视频」才算。video_frames 是 ffprobe 抽帧后逐帧量化，
        // 全程在本机，把它挂在 capabilities.video 上会让没有读视频模型的用户
        // 白丢一条能用的工作流，还会收到「去设置里开启读视频」这种错建议。
        matches!(self, WorkflowKind::VideoBrief)
    }

    /// 纯本机、不请求模型的工作流。
    pub fn is_local(self) -> bool {
        matches!(self, WorkflowKind::FrameTween | WorkflowKind::VideoFrames)
    }

    pub fn id(self) -> &'static str {
        match self {
            WorkflowKind::Agent => "agent",
            WorkflowKind::ImageGen => "image_gen",
            WorkflowKind::VisionBrief => "vision_brief",
            WorkflowKind::VideoFrames => "video_frames",
            WorkflowKind::VideoBrief => "video_brief",
            WorkflowKind::FrameTween => "frame_tween",
            WorkflowKind::PromptRefine => "prompt_refine",
        }
    }
}

/// 一条工作流的展示信息。字段用 String 而非 &'static str：
/// 这个结构既要 Serialize（发给前端）也要 Deserialize（测试与将来读回配置），
/// 借用生命周期过不了 Deserialize。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowInfo {
    pub kind: WorkflowKind,
    pub id: String,
    pub title: String,
    pub summary: String,
    pub needs: Capabilities,
    /// 跑完会得到什么，给 UI 当结果说明。
    pub output: String,
}

impl WorkflowInfo {
    fn new(
        kind: WorkflowKind,
        title: &str,
        summary: &str,
        needs: Capabilities,
        output: &str,
    ) -> Self {
        WorkflowInfo {
            kind,
            id: kind.id().to_string(),
            title: title.to_string(),
            summary: summary.to_string(),
            needs,
            output: output.to_string(),
        }
    }
}

/// 全部工作流。顺序即 UI 展示顺序：常用的在前。
pub fn catalog() -> Vec<WorkflowInfo> {
    vec![
        WorkflowInfo::new(
            WorkflowKind::Agent,
            "Agent Draw",
            "Chat to draw. Runs one sandboxed Lua script per edit.",
            Capabilities::default(),
            "Edited canvas document",
        ),
        WorkflowInfo::new(
            WorkflowKind::ImageGen,
            "Image Generate",
            "Model renders a bitmap, then it is quantized onto the canvas grid.",
            Capabilities {
                image_gen: true,
                ..Default::default()
            },
            "One new cel of pixelized art",
        ),
        WorkflowInfo::new(
            WorkflowKind::VisionBrief,
            "Reference Brief",
            "A vision model reads your reference into a structured brief, then draws.",
            Capabilities {
                vision: true,
                ..Default::default()
            },
            "Brief text plus a drawn canvas",
        ),
        WorkflowInfo::new(
            WorkflowKind::VideoFrames,
            "Video Frames",
            "Pull key frames out of a clip and quantize each one onto its own frame.",
            Capabilities::default(),
            "A frame sequence drawn from video stills",
        ),
        WorkflowInfo::new(
            WorkflowKind::VideoBrief,
            "Video Brief",
            "A video model reads a clip into a motion brief you can edit, then draws.",
            Capabilities {
                video: true,
                ..Default::default()
            },
            "Motion brief text plus a drawn canvas",
        ),
        WorkflowInfo::new(
            WorkflowKind::FrameTween,
            "In-between",
            "Generate tween frames between two existing frames, on this machine.",
            Capabilities::default(),
            "New frames inserted before the end frame",
        ),
        WorkflowInfo::new(
            WorkflowKind::PromptRefine,
            "Prompt Refine",
            "Turn a plain sentence into a structured pixel-art prompt you can edit.",
            Capabilities::default(),
            "A refined prompt you can send or keep editing",
        ),
    ]
}

pub fn info(kind: WorkflowKind) -> WorkflowInfo {
    catalog()
        .into_iter()
        .find(|w| w.kind == kind)
        .expect("every WorkflowKind has a catalog entry")
}

/// 当前模型跑不跑得动。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    Blocked { missing: Vec<String> },
}

pub fn readiness(kind: WorkflowKind, caps: &Capabilities) -> Readiness {
    let missing: Vec<String> = caps
        .missing_for(kind)
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    if missing.is_empty() {
        Readiness::Ready
    } else {
        Readiness::Blocked { missing }
    }
}

/// 当前模型可用的工作流，顺序与 `catalog()` 一致。
pub fn available(caps: &Capabilities) -> Vec<WorkflowInfo> {
    catalog()
        .into_iter()
        .filter(|w| readiness(w.kind, caps) == Readiness::Ready)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_chat_workflows_need_nothing() {
        for kind in [
            WorkflowKind::Agent,
            WorkflowKind::FrameTween,
            WorkflowKind::PromptRefine,
            WorkflowKind::VideoFrames,
        ] {
            assert_eq!(
                Capabilities::default().missing_for(kind),
                Vec::<&str>::new(),
                "{kind:?} should need no capability"
            );
        }
    }

    #[test]
    fn each_model_backed_workflow_names_exactly_what_it_needs() {
        let none = Capabilities::default();
        assert_eq!(
            none.missing_for(WorkflowKind::ImageGen),
            vec!["image_gen".to_string()]
        );
        assert_eq!(
            none.missing_for(WorkflowKind::VisionBrief),
            vec!["vision".to_string()]
        );
        assert_eq!(
            none.missing_for(WorkflowKind::VideoBrief),
            vec!["video".to_string()]
        );

        let all = Capabilities {
            vision: true,
            image_gen: true,
            video: true,
            ..Default::default()
        };
        for kind in [
            WorkflowKind::ImageGen,
            WorkflowKind::VisionBrief,
            WorkflowKind::VideoBrief,
        ] {
            assert!(all.missing_for(kind).is_empty(), "{kind:?} should be ready");
        }
    }

    #[test]
    fn a_bare_model_still_gets_every_workflow_that_needs_no_model_skill() {
        let list = available(&Capabilities::default());
        let ids: Vec<&str> = list.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["agent", "video_frames", "frame_tween", "prompt_refine"]
        );
    }

    #[test]
    fn a_fully_capable_model_gets_everything_in_catalog_order() {
        let caps = Capabilities {
            vision: true,
            image_gen: true,
            video: true,
            ..Default::default()
        };
        let list = available(&caps);
        assert_eq!(list.len(), catalog().len());
        assert_eq!(list[0].kind, WorkflowKind::Agent);
    }

    #[test]
    fn only_the_on_machine_workflows_count_as_local() {
        assert!(WorkflowKind::FrameTween.is_local());
        assert!(WorkflowKind::VideoFrames.is_local());
        assert!(!WorkflowKind::Agent.is_local());
        assert!(!WorkflowKind::ImageGen.is_local());
        assert!(!WorkflowKind::VideoBrief.is_local());
    }

    #[test]
    fn kind_serializes_as_snake_case_for_the_frontend() {
        let value = serde_json::to_value(WorkflowKind::VisionBrief).unwrap();
        assert_eq!(value, serde_json::json!("vision_brief"));
        let value = serde_json::to_value(WorkflowKind::FrameTween).unwrap();
        assert_eq!(value, serde_json::json!("frame_tween"));
        let value = serde_json::to_value(WorkflowKind::VideoBrief).unwrap();
        assert_eq!(value, serde_json::json!("video_brief"));
    }

    #[test]
    fn capabilities_defaults_every_flag_off_when_absent() {
        let caps: Capabilities = serde_json::from_str("{}").unwrap();
        assert_eq!(caps, Capabilities::default());
        assert!(!caps.any());
        let caps: Capabilities = serde_json::from_str(r#"{"vision":true}"#).unwrap();
        assert!(caps.any());
    }

    #[test]
    fn readiness_reports_the_blockers_by_name() {
        let r = readiness(WorkflowKind::ImageGen, &Capabilities::default());
        assert_eq!(
            r,
            Readiness::Blocked {
                missing: vec!["image_gen".to_string()]
            }
        );
        assert_eq!(
            readiness(WorkflowKind::Agent, &Capabilities::default()),
            Readiness::Ready
        );
    }
}
