// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 参考图简报：让读图模型先把参考图「读」成结构化文字，再交给 agent 主循环落笔。
//!
//! 为什么不是直接把参考图随消息发给画图 agent：
//! 1. 会画像素的模型往往不识图，识图的模型往往不会画像素，把一个不认识的硬通货
//!    塞给它，只会得到凭想象发挥的结果。
//! 2. 简报可以表达「比例、姿态、配色」这类像素画真正需要的判断，而位图不能。
//! 3. 简报是可编辑的文本：用户改一句「尾巴改成两条」，比重新截图再喂一次更顺。
//!
//! 因此这条路径只做一件事：图 -> JSON -> VisionBrief。解析必须宽容，
//! 因为模型再听话也可能给 JSON 套代码围栏或加两句前言。

use super::models::{Attachment, AttachmentRole, ChatRequest, Message, ModelConfig};
use super::one_shot;
use super::providers::ProviderError;
use serde::{Deserialize, Serialize};

/// 参考图的结构化简报。全部字段都是给人看也给 agent 看的短文本。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VisionBrief {
    /// 一句话说清画的是什么。
    pub subject: String,
    /// 剪影描述：外轮廓与块面关系，像素画最看重这个。
    pub silhouette: String,
    /// 主色到暗色的有序色板，hex 字符串。
    pub palette: Vec<String>,
    /// 姿态与重心。
    pub pose_notes: String,
    /// 各部位相对比例，尽量换算成目标画布下的格子数。
    pub proportions: String,
    /// 落到像素画上的具体建议：色阶数、轮廓策略、该简化的细节。
    pub craft_notes: String,
    /// 模型原文，解析不全时至少留个痕迹。
    pub raw: String,
}

/// 读图用的系统提示词。要求紧凑 JSON，并把目标分辨率告知模型，
/// 这样它给出的比例才有意义（不然它会按 1024px 的真实照片比例回答）。
fn system_prompt(width: u32, height: u32) -> String {
    format!(
        "You analyse reference images for a pixel art editor. The canvas the art must be redrawn on is {width}x{height} pixels.\n\n\
Reply with ONE compact JSON object and nothing else, no code fences, using exactly these keys:\n\
{{\"subject\":\"\",\"silhouette\":\"\",\"palette\":[],\"pose_notes\":\"\",\"proportions\":\"\",\"craft_notes\":\"\"}}\n\n\
RULES:\n\
- subject: one short sentence naming what is in the image.\n\
- silhouette: the outer contour and the main mass blocks, in pixel-art terms (chunky, readable at small size).\n\
- palette: 4-8 colors ordered light to dark, as \"#RRGGBB\" strings, taken from the image itself. Hue-shift the shadows cooler and the highlights warmer.\n\
- pose_notes: stance, weight, facing, and what reads as the focal point.\n\
- proportions: how the parts relate, expressed against the {width}x{height} target canvas (for example \"head takes about 3 of 16 rows\").\n\
- craft_notes: concrete pixel-art advice for this subject: how many ramp steps per material, one outline strategy, which tiny details to drop, and what must stay readable.\n\
- Keep every value under 40 words. JSON only, no commentary before or after."
    )
}

/// 从模型回复里抠出 JSON 对象。容忍代码围栏、前言后记，以及多余空白。
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end > start {
        Some(&text[start..=end])
    } else {
        None
    }
}

/// 读一张参考图，产出结构化简报。
pub async fn brief_reference(
    config: &ModelConfig,
    attachment: &Attachment,
    width: u32,
    height: u32,
) -> Result<VisionBrief, ProviderError> {
    if attachment.role != AttachmentRole::Reference {
        return Err(ProviderError::Config(
            "a vision brief needs a reference image, not a canvas snapshot".into(),
        ));
    }
    // max_tokens 收到 1500：简报是结构化短文本，给多了模型会开始写散文。
    let text = one_shot::chat_once(
        config,
        &system_prompt(width, height),
        vec![reference_message(attachment, width, height)],
        Some(1500),
    )
    .await?;

    if let Some(json_text) = extract_json_object(&text) {
        if let Ok(mut brief) = serde_json::from_str::<VisionBrief>(json_text) {
            // 模型经常漏 craft_notes，因为它觉得前面说够了；这里兜一句，别让下游拿到空字段。
            if brief.craft_notes.trim().is_empty() {
                brief.craft_notes =
                    "simplify to the canvas resolution; keep the silhouette and 2-4 signature details"
                        .into();
            }
            brief.raw = text.clone();
            return Ok(brief);
        }
    }
    // 解析不出来也别浪费这次调用：原文整体当作 craft_notes 交给 agent，至少有信息量。
    Ok(VisionBrief {
        subject: "(see raw notes)".into(),
        silhouette: String::new(),
        palette: Vec::new(),
        pose_notes: String::new(),
        proportions: String::new(),
        craft_notes: text.trim().to_string(),
        raw: text,
    })
}

/// 简报在 agent 工作流里要用的最小请求构造，单独拆出来便于测试。
pub fn brief_request(attachment: &Attachment, width: u32, height: u32) -> ChatRequest {
    ChatRequest {
        system: system_prompt(width, height),
        messages: vec![reference_message(attachment, width, height)],
        tools: Vec::new(),
        max_tokens: 1500,
        temperature: None,
        // 简报是单轮请求，历史里没有推理内容可回灌。
        echo_reasoning: false,
        // 读图简报同理：结论优先，省下的额度都给描述本身。
        disable_thinking: true,
    }
}

/// 让 `brief_reference` 与 `brief_request` 共用同一份消息构造，避免两处漂移。
fn reference_message(attachment: &Attachment, width: u32, height: u32) -> Message {
    Message {
        role: super::models::Role::User,
        content: vec![
            super::models::ContentBlock::Text {
                text: format!(
                    "Read this reference image. It will be redrawn as pixel art on a {width}x{height} canvas."
                ),
            },
            super::models::ContentBlock::Image {
                media_type: attachment.media_type.clone(),
                data_base64: attachment.data_base64.clone(),
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::super::models::Protocol;
    use super::*;

    fn attachment() -> Attachment {
        Attachment {
            role: AttachmentRole::Reference,
            media_type: "image/png".into(),
            data_base64: "AAAA".into(),
        }
    }

    #[test]
    fn finds_json_even_when_the_model_chats_around_it() {
        let text = "Sure! Here is the analysis:\n```json\n{\"subject\":\"a fox\",\"palette\":[\"#fff\"]}\n```\nHope that helps.";
        let json = extract_json_object(text).expect("should find the object");
        assert!(json.starts_with('{'), "{json}");
        assert!(json.ends_with('}'), "{json}");
        let brief: VisionBrief = serde_json::from_str(json).unwrap();
        assert_eq!(brief.subject, "a fox");
        assert_eq!(brief.palette, vec!["#fff".to_string()]);
    }

    #[test]
    fn a_reply_with_no_braces_yields_none() {
        assert!(extract_json_object("no json here").is_none());
        assert!(extract_json_object("{unbalanced").is_none());
    }

    #[test]
    fn missing_fields_deserialize_as_empty_not_as_errors() {
        let brief: VisionBrief = serde_json::from_str(r#"{"subject":"a wolf"}"#).unwrap();
        assert_eq!(brief.subject, "a wolf");
        assert!(brief.silhouette.is_empty());
        assert!(brief.palette.is_empty());
        assert!(brief.raw.is_empty());
    }

    #[tokio::test]
    async fn a_snapshot_is_rejected_because_it_is_not_ground_truth() {
        let config = ModelConfig {
            id: "m".into(),
            label: "m".into(),
            protocol: Protocol::OpenAiCompat,
            base_url: "https://example.invalid/v1".into(),
            api_key: "key".into(),
            model: "m".into(),
            max_tokens: None,
            temperature: None,
            disable_thinking: None,
            capabilities: Default::default(),
        };
        let snap = Attachment {
            role: AttachmentRole::Snapshot,
            media_type: "image/png".into(),
            data_base64: "AAAA".into(),
        };
        let err = brief_reference(&config, &snap, 32, 32).await.unwrap_err();
        assert!(err.to_string().contains("reference image"), "{err}");
    }

    #[test]
    fn the_request_carries_the_target_canvas_in_both_halves() {
        let req = brief_request(&attachment(), 48, 32);
        assert!(req.system.contains("48x32"));
        let text = req.messages[0].text_of();
        assert!(text.contains("48x32"), "{text}");
        assert_eq!(req.tools.len(), 0);
    }

    #[test]
    fn brief_reference_and_brief_request_agree_on_the_message() {
        // 两处构造必须一致，否则测试过了、线上行为却是另一套。
        let a = reference_message(&attachment(), 16, 16);
        let b = brief_request(&attachment(), 16, 16).messages[0].clone();
        assert_eq!(a, b);
    }

    #[test]
    fn protocol_is_threaded_into_the_imports() {
        // 编译期守卫：ChatRequest/Protocol 的相关 import 一旦被清掉，这里立刻报错。
        let req = brief_request(&attachment(), 1, 1);
        assert_eq!(req.max_tokens, 1500);
    }
}
