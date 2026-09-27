//! 工作流目录与能力模型。
//!
//! 「只用用户自带模型」意味着不同模型能做的事差别很大：有的只会聊天，有的能读图，
//! 有的能直接生图，少数能读视频。与其在提示词里让模型猜，不如把流程显式分成几类，
//! 每类声明自己需要的能力；UI 只展示当前模型跑得动的那几个。
//!
//! 这层是纯逻辑，不发请求、不碰文档，方便测试，也方便 Tauri 层直接序列化给前端。

use serde::{Deserialize, Serialize};

pub use super::models::Capabilities;

/// 六条工作流。前四条覆盖「不同模型怎么做同一件事」，后两条是纯本地的编辑助手。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowKind {
    /// 默认：文本 agent 主循环，用 Lua 沙箱和类型化操作落笔。
    Agent,
    /// 生图模型出图 -> 量化成像素文档（支持示例图垫图）。
    ImageGen,
    /// 读图模型把参考图读成结构化简报，再交给 agent 主循环。
    VisionBrief,
    /// 读视频模型/抽帧得到关键帧序列，逐帧量化或作为参考。
    VideoFrames,
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
        matches!(self, WorkflowKind::VideoFrames)
    }

    /// 纯本机、不请求模型的工作流。
    pub fn is_local(self) -> bool {
        matches!(self, WorkflowKind::FrameTween)
    }

    pub fn id(self) -> &'static str {
        match self {
            WorkflowKind::Agent => "agent",
            WorkflowKind::ImageGen => "image_gen",
            WorkflowKind::VisionBrief => "vision_brief",
            WorkflowKind::VideoFrames => "video_frames",
            WorkflowKind::FrameTween => "frame_tween",
            WorkflowKind::PromptRefine => "prompt_refine",
        }
    }
}

/// 一条工作流的展示信息。`&'static str` 让它可以零成本各处传阅。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowInfo {
    pub kind: WorkflowKind,
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub needs: Capabilities,
    /// 跑完会得到什么，给 UI 当结果说明。
    pub output: &'static str,
}

/// 全部工作流。顺序即 UI 展示顺序：常用的在前。
pub fn catalog() -> Vec<WorkflowInfo> {
    vec![
        WorkflowInfo {
            kind: WorkflowKind::Agent,
            id: "agent",
            title: "Agent Draw",
            summary: "Chat to draw. Runs one sandboxed Lua script per edit.",
            needs: Capabilities::default(),
            output: "Edited canvas document",
        },
        WorkflowInfo {
            kind: WorkflowKind::ImageGen,
            id: "image_gen",
            title: "Image Generate",
            summary: "Model renders a bitmap, then it is quantized onto the canvas grid.",
            needs: Capabilities {
                vision: false,
                image_gen: true,
                video: false,
            },
            output: "One new cel of pixelized art",
        },
        WorkflowInfo {
            kind: WorkflowKind::VisionBrief,
            id: "vision_brief",
            title: "Reference Brief",
            summary: "A vision model reads your reference into a structured brief, then draws.",
            needs: Capabilities {
                vision: true,
                image_gen: false,
                video: false,
            },
            output: "Brief text plus a drawn canvas",
        },
        WorkflowInfo {
            kind: WorkflowKind::VideoFrames,
            id: "video_frames",
            title: "Video Frames",
            summary: "Pull key frames out of a clip, then draw each one on its own frame.",
            needs: Capabilities {
                vision: false,
                image_gen: false,
                video: true,
            },
            output: "A frame sequence drawn from video stills",
        },
        WorkflowInfo {
            kind: WorkflowKind::FrameTween,
            id: "frame_tween",
            title: "In-between",
            summary: "Generate tween frames between two existing frames, on this machine.",
            needs: Capabilities::default(),
            output: "New frames inserted before the end frame",
        },
        WorkflowInfo {
            kind: WorkflowKind::PromptRefine,
            id: "prompt_refine",
            title: "Prompt Refine",
            summary: "Turn a plain sentence into a structured pixel-art prompt you can edit.",
            needs: Capabilities::default(),
            output: "A refined prompt you can send or keep editing",
        },
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
    Blocked {
        missing: Vec<&'static str>,
    },
}

pub fn readiness(kind: WorkflowKind, caps: &Capabilities) -> Readiness {
    let missing = caps.missing_for(kind);
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
    fn agent_tween_and_refine_need_nothing() {
        for kind in [
            WorkflowKind::Agent,
            WorkflowKind::FrameTween,
            WorkflowKind::PromptRefine,
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
        assert_eq!(none.missing_for(WorkflowKind::ImageGen), vec!["image_gen"]);
        assert_eq!(none.missing_for(WorkflowKind::VisionBrief), vec!["vision"]);
        assert_eq!(none.missing_for(WorkflowKind::VideoFrames), vec!["video"]);

        let all = Capabilities {
            vision: true,
            image_gen: true,
            video: true,
        };
        for kind in [
            WorkflowKind::ImageGen,
            WorkflowKind::VisionBrief,
            WorkflowKind::VideoFrames,
        ] {
            assert!(all.missing_for(kind).is_empty(), "{kind:?} should be ready");
        }
    }

    #[test]
    fn a_bare_model_still_gets_the_three_always_on_workflows() {
        let list = available(&Capabilities::default());
        let ids: Vec<&str> = list.iter().map(|w| w.id).collect();
        assert_eq!(ids, vec!["agent", "frame_tween", "prompt_refine"]);
    }

    #[test]
    fn a_fully_capable_model_gets_everything_in_catalog_order() {
        let caps = Capabilities {
            vision: true,
            image_gen: true,
            video: true,
        };
        let list = available(&caps);
        assert_eq!(list.len(), catalog().len());
        assert_eq!(list[0].kind, WorkflowKind::Agent);
    }

    #[test]
    fn only_tween_is_local() {
        assert!(WorkflowKind::FrameTween.is_local());
        assert!(!WorkflowKind::Agent.is_local());
        assert!(!WorkflowKind::ImageGen.is_local());
    }

    #[test]
    fn kind_serializes_as_snake_case_for_the_frontend() {
        let value = serde_json::to_value(WorkflowKind::VisionBrief).unwrap();
        assert_eq!(value, serde_json::json!("vision_brief"));
        let value = serde_json::to_value(WorkflowKind::FrameTween).unwrap();
        assert_eq!(value, serde_json::json!("frame_tween"));
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
                missing: vec!["image_gen"]
            }
        );
        assert_eq!(readiness(WorkflowKind::Agent, &Capabilities::default()), Readiness::Ready);
    }
}
