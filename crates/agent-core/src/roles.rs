// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 会话里的模型分工。
//!
//! 「只用用户自带模型」经常意味着用户手上不止一个模型：一个会聊天、一个会出图、
//! 一个读得懂图、少数读得懂视频。会话只绑一个模型的话，为了跑一次生图得把会话
//! 改绑过去、跑完再改回来——没人受得了这个。这里把会话的模型按角色拆开：
//! 主模型管 agent 主循环，另外三个角色各可以另绑一个；没配就回落主模型，
//! 单模型用户什么都不用配，多模型用户才能各尽其才。
//!
//! 这层只做纯逻辑：不发请求、不碰 Tauri，方便单测。

use serde::{Deserialize, Serialize};

use super::workflows::WorkflowKind;

/// 会话里的一种模型分工。Chat 就是会话那把主引擎，另外三个是可选的分身。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// agent 主循环：聊天、工具调用、落笔。
    Chat,
    /// 生图模型：出位图，再量化上画布。
    ImageGen,
    /// 识图模型：把参考图读成结构化简报。
    Vision,
    /// 读视频模型：把一段素材读成运动简报。
    Video,
}

impl ModelRole {
    /// 序列化给前端的稳定标识。
    pub fn id(self) -> &'static str {
        match self {
            ModelRole::Chat => "chat",
            ModelRole::ImageGen => "image_gen",
            ModelRole::Vision => "vision",
            ModelRole::Video => "video",
        }
    }

    pub fn all() -> [ModelRole; 4] {
        [
            ModelRole::Chat,
            ModelRole::ImageGen,
            ModelRole::Vision,
            ModelRole::Video,
        ]
    }

    /// 这条工作流该找哪个角色要结果。纯本机的流程不吃模型，归主模型。
    pub fn for_workflow(kind: WorkflowKind) -> ModelRole {
        match kind {
            WorkflowKind::Agent
            | WorkflowKind::PromptRefine
            | WorkflowKind::FrameTween
            | WorkflowKind::VideoFrames => ModelRole::Chat,
            WorkflowKind::ImageGen => ModelRole::ImageGen,
            WorkflowKind::VisionBrief => ModelRole::Vision,
            WorkflowKind::VideoBrief => ModelRole::Video,
        }
    }

    /// 这个角色对应的能力名；主模型没有专属能力，它的能力说了算的是主循环。
    pub fn capability(self) -> Option<&'static str> {
        match self {
            ModelRole::Chat => None,
            ModelRole::ImageGen => Some("image_gen"),
            ModelRole::Vision => Some("vision"),
            ModelRole::Video => Some("video"),
        }
    }

    /// 除了主模型之外还能另绑一个的角色。Chat 就是主模型本身，没有再绑一说。
    pub fn is_detachable(self) -> bool {
        self.capability().is_some()
    }
}

/// 一条角色实际由谁干活。UI 拿它显示「生图由 X 提供」，
/// 也拿它解释为什么某条流程灰着。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleBinding {
    pub role: ModelRole,
    /// 实际干活的模型 id。回落主模型时是主模型的 id。
    pub model_id: String,
    pub model_label: String,
    /// 这个角色有没有单独绑过；false 表示正在用主模型凑。
    pub detached: bool,
}

/// 把这一轮会话的模型分工写成系统提示词里的一段。
///
/// 为什么非要进提示词：主循环那一发请求只有主模型在看，它得先知道生图 / 读图 /
/// 读视频这几段到底谁在跑。单模型兼任时它要在同一轮里把每一段做完；另绑了专职
/// 模型时它只要把交接输入备齐再调那个工具，别顺手把同事的活也编了。少了这段，
/// 多模型会话里主模型经常自己脑补一张位图，而真正的生图模型压根没被叫上；单模型
/// 会话则容易在「还有别的模型会接手」的错觉里一步都不做。
pub fn prompt_section(bindings: &[RoleBinding]) -> String {
    // 名字以真实绑定为准；绑定还没建好时退回落款说法，段落照样成型。
    let label = |role: ModelRole| -> String {
        bindings
            .iter()
            .find(|binding| binding.role == role)
            .map(|binding| binding.model_label.clone())
            .unwrap_or_else(|| "the main model".to_string())
    };
    let is_detached = |role: ModelRole| -> bool {
        bindings
            .iter()
            .any(|binding| binding.role == role && binding.detached)
    };

    let mut out = String::from(
        "MODEL ROLES - who runs which stage this session: the pipeline can run on one model or on several; read the bindings before you plan a turn.\n",
    );
    out.push_str(&format!(
        "- chat = {}: owns the agent main loop - read the request, route the turn, call tools, write the Lua scripts and operations, close with one short sentence.\n",
        label(ModelRole::Chat)
    ));
    out.push_str(&format!(
        "- image_gen = {}: owns raster generation - turn the positive and negative prompt lists into a bitmap that the editor then quantizes onto the canvas.\n",
        label(ModelRole::ImageGen)
    ));
    out.push_str(&format!(
        "- vision = {}: owns reading a reference image into a structured brief.\n",
        label(ModelRole::Vision)
    ));
    out.push_str(&format!(
        "- video = {}: owns reading a video into a motion brief.\n",
        label(ModelRole::Video)
    ));

    let detached: Vec<ModelRole> = ModelRole::all()
        .into_iter()
        .filter(|role| is_detached(*role))
        .collect();
    if detached.is_empty() {
        out.push_str(
            "Every stage above is you: there is no separate model to hand off to, so complete each stage yourself, in order, inside the turn - write the lists, call the tool, and never skip a stage assuming another model will pick it up.\n",
        );
    } else {
        let names = detached
            .iter()
            .map(|role| format!("{} ({})", role.id(), label(*role)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "Stages served by a separate model: {names}. Those stages run on their own engine, so your job for one of them is to prepare the exact input it needs and then call the tool that runs it - for image_gen that means the positive and negative prompt lists plus size and reference mode, and for a reference read it means the file plus what you need back. Do not hand-write an approximation of a detached stage's output, and do not announce that you are about to call it.\n",
        ));
        let shared: Vec<&str> = ModelRole::all()
            .into_iter()
            .filter(|role| *role != ModelRole::Chat && !detached.contains(role))
            .map(|role| role.id())
            .collect();
        if !shared.is_empty() {
            out.push_str(&format!(
                "The stages still shared with the main model ({}) you run yourself, in order, exactly as if every role were one model.\n",
                shared.join(", ")
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_workflow_lands_on_exactly_one_role() {
        assert_eq!(
            ModelRole::for_workflow(WorkflowKind::Agent),
            ModelRole::Chat
        );
        assert_eq!(
            ModelRole::for_workflow(WorkflowKind::PromptRefine),
            ModelRole::Chat
        );
        assert_eq!(
            ModelRole::for_workflow(WorkflowKind::FrameTween),
            ModelRole::Chat
        );
        assert_eq!(
            ModelRole::for_workflow(WorkflowKind::VideoFrames),
            ModelRole::Chat
        );
        assert_eq!(
            ModelRole::for_workflow(WorkflowKind::ImageGen),
            ModelRole::ImageGen
        );
        assert_eq!(
            ModelRole::for_workflow(WorkflowKind::VisionBrief),
            ModelRole::Vision
        );
        assert_eq!(
            ModelRole::for_workflow(WorkflowKind::VideoBrief),
            ModelRole::Video
        );
    }

    #[test]
    fn only_the_three_capability_roles_can_be_detached() {
        for role in ModelRole::all() {
            assert_eq!(
                role.is_detachable(),
                role.capability().is_some(),
                "{role:?} detachable must follow having a capability"
            );
        }
        assert!(!ModelRole::Chat.is_detachable());
    }

    #[test]
    fn capability_names_match_what_readiness_reports() {
        // missing_for 报的就是这三个名字，两边对不上用户会看到对不上的建议。
        assert_eq!(ModelRole::ImageGen.capability(), Some("image_gen"));
        assert_eq!(ModelRole::Vision.capability(), Some("vision"));
        assert_eq!(ModelRole::Video.capability(), Some("video"));
    }

    #[test]
    fn prompt_section_names_every_role_and_who_serves_it() {
        let bindings = vec![
            RoleBinding {
                role: ModelRole::Chat,
                model_id: "m1".into(),
                model_label: "主脑".into(),
                detached: false,
            },
            RoleBinding {
                role: ModelRole::ImageGen,
                model_id: "m2".into(),
                model_label: "画匠".into(),
                detached: true,
            },
            RoleBinding {
                role: ModelRole::Vision,
                model_id: "m1".into(),
                model_label: "主脑".into(),
                detached: false,
            },
            RoleBinding {
                role: ModelRole::Video,
                model_id: "m1".into(),
                model_label: "主脑".into(),
                detached: false,
            },
        ];
        let text = prompt_section(&bindings);
        for role in ["chat", "image_gen", "vision", "video"] {
            assert!(text.contains(role), "缺角色 {role}");
        }
        assert!(text.contains("画匠"), "专职生图模型没露面");
        assert!(
            text.contains("image_gen (画匠)"),
            "没点名哪个角色另绑了模型"
        );
        assert!(
            text.contains("Do not hand-write an approximation"),
            "没警告别自己编专职阶段的输出"
        );
        assert!(
            !text.contains("Every stage above is you"),
            "已经有分工了，不该再说全是一个人扛"
        );
    }

    #[test]
    fn prompt_section_tells_a_solo_model_to_cover_every_stage_itself() {
        let bindings: Vec<RoleBinding> = ModelRole::all()
            .into_iter()
            .map(|role| RoleBinding {
                role,
                model_id: "m1".into(),
                model_label: "独苗".into(),
                detached: false,
            })
            .collect();
        let text = prompt_section(&bindings);
        assert!(
            text.contains("Every stage above is you"),
            "单模型会话必须明说没人接手"
        );
        assert!(
            !text.contains("Stages served by a separate model"),
            "没绑专职模型就别提分工交接"
        );
    }

    #[test]
    fn prompt_section_survives_an_empty_binding_list() {
        // 会话还没绑好、老会话没记分工时也不能空手：退回落款说法，段落仍然成型。
        let text = prompt_section(&[]);
        assert!(text.contains("the main model"));
        assert!(text.contains("Every stage above is you"));
    }
}
