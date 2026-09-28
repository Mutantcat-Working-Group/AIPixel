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
}
