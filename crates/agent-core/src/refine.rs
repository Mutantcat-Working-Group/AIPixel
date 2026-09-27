//! 提示词微调：把一句大白话改写成可以用来生图或画像素的结构化提示词。
//!
//! 这是唯一一条「先出文本、由用户过目、再决定跑不跑」的工作流。其他流程都是
//! 跑完才看到结果，这条必须相反：改写后的提示词是要给用户逐行改的，
//! 不能自动往下执行。所以这里只产文本，不碰文档、不调工具。

use super::models::{Message, ModelConfig};
use super::one_shot;
use super::providers::ProviderError;
use serde::{Deserialize, Serialize};

/// 微调出来的提示词是给哪条链路用的。两条链路要的东西不一样：
/// 写 Lua 脚本的 agent 要的是「结构与配色」；生图模型要的是「主体与镜头」。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefineTarget {
    #[default]
    Shader,
    ImageGen,
}

/// 微调请求。目标画布是必填的：不知道最终尺寸，模型无从谈比例。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefineRequest {
    pub idea: String,
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_target")]
    pub target: RefineTarget,
}

fn default_target() -> RefineTarget {
    RefineTarget::Shader
}

/// 微调结果。`prompt` 是可直接用的成品；`raw` 留原文，方便用户回头看模型加了什么。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefinedPrompt {
    pub prompt: String,
    pub raw: String,
}

pub fn target_label(target: RefineTarget) -> &'static str {
    match target {
        RefineTarget::Shader => "pixel_run_shader",
        RefineTarget::ImageGen => "image generation",
    }
}

/// 微调用的系统提示词。要求固定顺序的几行短文本，而不是一篇文章：
/// 用户接下来要逐行改，行太长、行数太多就没法改了。
/// 用行数组拼而不是长字符串字面量，是为了让每条规则在源码里可读且可单独改。
pub fn refine_system_prompt(req: &RefineRequest) -> String {
    let head = format!(
        "You rewrite a rough idea into a pixel art prompt for a {}x{} canvas. The result will drive {}.",
        req.width, req.height, target_label(req.target)
    );
    let mut lines = vec![head];
    lines.extend(
        [
            "",
            "Reply with ONLY these lines, each starting with the label exactly as written and a colon. No bullets, no numbering, no commentary before or after.",
            "subject: what is drawn, in one sentence.",
            "silhouette: the outer contour and main mass blocks, readable at this canvas size.",
            "palette: 4-8 colours from light to dark as #RRGGBB, comma separated.",
            "lighting: one light source and where it hits.",
            "pose: stance, facing, weight, focal point.",
            "proportions: how the parts relate, expressed in canvas cells (for example \"head takes about 3 of 16 rows\").",
            "detail: the 2-4 signature details to keep, and what to drop.",
            "outline: one outline strategy and one only.",
            "constraints: what must stay simple and clean at this size.",
            "",
            "RULES:",
            "- Keep each line under 25 words.",
            "- Invent nothing about the user's idea; expand it into pixel-art decisions only.",
            "- Every line must be actionable, not descriptive.",
        ]
        .iter()
        .map(|s| (*s).to_string()),
    );
    lines.join("\n")
}

/// 微调一条想法。空想法直接报错，不发请求。
pub async fn refine(
    config: &ModelConfig,
    req: &RefineRequest,
) -> Result<RefinedPrompt, ProviderError> {
    let idea = req.idea.trim();
    if idea.is_empty() {
        return Err(ProviderError::Config(
            "refine needs a non-empty idea".into(),
        ));
    }
    if req.width == 0 || req.height == 0 {
        return Err(ProviderError::Config(
            "refine needs the target canvas size".into(),
        ));
    }
    let text = one_shot::chat_once(
        config,
        &refine_system_prompt(req),
        vec![Message::user_text(idea)],
        Some(900),
    )
    .await?;
    Ok(RefinedPrompt {
        prompt: normalise_labels(&text),
        raw: text,
    })
}

/// 把模型可能加的前言后记、围栏、多余空行清掉，只留标签行。
/// 保序：用户要按行改，顺序不能变。
fn normalise_labels(text: &str) -> String {
    const LABELS: [&str; 9] = [
        "subject",
        "silhouette",
        "palette",
        "lighting",
        "pose",
        "proportions",
        "detail",
        "outline",
        "constraints",
    ];
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim().trim_start_matches("- ").trim();
        // 代码围栏和无关散文一律丢掉：它们不属于提示词。
        if trimmed.starts_with("```") {
            continue;
        }
        let Some((label, rest)) = trimmed.split_once(':') else {
            continue;
        };
        // 模型爱给标签加 ** 强调，也可能写成 `- **label**:`
        let label = label.trim().trim_matches('*').trim().to_ascii_lowercase();
        if !LABELS.contains(&label.as_str()) {
            continue;
        }
        let rest = rest.trim().trim_matches('*').trim();
        if rest.is_empty() {
            continue;
        }
        out.push(format!("{label}: {rest}"));
    }
    if out.is_empty() {
        return text.trim().to_string();
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::super::models::Protocol;
    use super::*;

    fn request() -> RefineRequest {
        RefineRequest {
            idea: "a knight standing in the rain".into(),
            width: 32,
            height: 32,
            target: RefineTarget::Shader,
        }
    }

    fn offline_config() -> ModelConfig {
        ModelConfig {
            id: "m".into(),
            label: "m".into(),
            protocol: Protocol::OpenAiCompat,
            base_url: "https://example.invalid/v1".into(),
            api_key: "key".into(),
            model: "m".into(),
            max_tokens: None,
            temperature: None,
            capabilities: Default::default(),
        }
    }

    #[test]
    fn the_prompt_names_the_canvas_and_the_downstream_target() {
        let text = refine_system_prompt(&request());
        assert!(text.contains("32x32"), "{text}");
        assert!(text.contains("pixel_run_shader"), "{text}");
    }

    #[test]
    fn the_image_gen_target_names_a_different_downstream() {
        let mut req = request();
        req.target = RefineTarget::ImageGen;
        let text = refine_system_prompt(&req);
        assert!(text.contains("image generation"), "{text}");
        assert!(!text.contains("pixel_run_shader"), "{text}");
    }

    #[test]
    fn every_declared_label_is_asked_for() {
        let text = refine_system_prompt(&request());
        for label in [
            "subject",
            "silhouette",
            "palette",
            "lighting",
            "pose",
            "proportions",
            "detail",
            "outline",
            "constraints",
        ] {
            assert!(text.contains(label), "{label} missing from the prompt");
        }
    }

    #[test]
    fn prose_and_fences_around_the_answer_are_dropped() {
        let messy = "Sure, here you go:\n```\n- subject: a knight in armour\nsome run-on sentence\n- **palette**: #112233, #445566\n```\nHope that helps!";
        let clean = normalise_labels(messy);
        assert_eq!(
            clean,
            "subject: a knight in armour\npalette: #112233, #445566"
        );
    }

    #[test]
    fn label_order_survives_even_when_the_model_shuffles_it() {
        let messy = "pose: standing\nsubject: a knight";
        assert_eq!(normalise_labels(messy), "pose: standing\nsubject: a knight");
    }

    #[test]
    fn an_answer_with_no_labels_at_all_falls_back_to_the_whole_text() {
        // 完全不按格式回答时，把原文交给用户比给个空字符串诚实。
        let clean = normalise_labels("just draw something cool");
        assert_eq!(clean, "just draw something cool");
    }

    #[test]
    fn empty_values_do_not_survive_as_blank_labels() {
        assert_eq!(
            normalise_labels("subject:   \npose: standing"),
            "pose: standing"
        );
    }

    #[tokio::test]
    async fn a_blank_idea_never_reaches_the_endpoint() {
        let mut req = request();
        req.idea = "   ".into();
        let err = refine(&offline_config(), &req).await.unwrap_err();
        assert!(matches!(err, ProviderError::Config(_)), "{err}");
    }

    #[tokio::test]
    async fn a_zero_canvas_is_rejected_before_any_request() {
        let mut req = request();
        req.width = 0;
        let err = refine(&offline_config(), &req).await.unwrap_err();
        assert!(matches!(err, ProviderError::Config(_)), "{err}");
    }
}
