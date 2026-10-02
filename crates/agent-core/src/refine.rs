// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 提示词微调：把一句大白话改写成可以用来生图或画像素的结构化提示词。
//!
//! 这是唯一一条「先出文本、由用户过目、再决定跑不跑」的工作流。其他流程都是
//! 跑完才看到结果，这条必须相反：改写后的提示词是要给用户逐行改的，
//! 不能自动往下执行。所以这里只产文本，不碰文档、不调工具。

use super::models::{Message, ModelConfig};
use super::one_shot;
use super::pins;
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
    /// 这一句点名的画风 id（输入区那个下拉）。None = 没点名。
    ///
    /// 微调这条链路过去完全不带它：用户在输入区选了「写实渲染」，点一下微调，
    /// 出来的九行提示词里一条渲染规矩都没有——选的那一套半点没上车，而用户盯着
    /// 九行字很难看出少的是哪一段。所以这里必须收下它，并按 plan 同一条让位规则
    /// 拼进提示词（与画风 id 重合的预设由画风段顶替，不发两遍）。
    #[serde(default)]
    pub style: Option<String>,
    /// 同时生效的收尾预设 id。形式和主循环那边一模一样：可以叠几条、上限
    /// `presets::MAX_STACKED`、认不出的 id 当场报错。细节这件事是乘法。
    #[serde(default)]
    pub presets: Vec<String>,
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
///
/// 后头的 FINISH CONTRACT 段是这一句点名的收尾规矩（画风 + 叠加预设）。它决定
/// palette / lighting / detail / outline 那几行怎么写——而用户改的正是那几行。
/// 认不出的 id 在这里报 `Err`：静默丢掉的后果是用户选了「写实渲染」、拿到的九行
/// 里没有任何渲染规矩，比报错难查得多。
pub fn refine_system_prompt(req: &RefineRequest) -> Result<String, String> {
    let head = format!(
        "You rewrite a rough idea into a pixel art prompt for a {}x{} canvas. The result will drive {}.",
        req.width, req.height, target_label(req.target)
    );
    // 钉在这里就收紧过一次：认不出的 id 直接往上抛，不进提示词。
    let style = pins::pinned_style(req.style.as_deref())?;
    let presets = pins::pinned_presets(&req.presets)?;
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
    lines.extend(finish_contract(req.target, style, &presets));
    Ok(lines.join("\n"))
}

/// FINISH CONTRACT 段：把这一句点名的画风与收尾预设翻译成 palette / lighting /
/// detail / outline 那几行要遵守的规矩。
///
/// 什么都没点的时候一行都不给。这不是偷懒：这条链路的产品是可逐行改的中间文本，
/// 用户没要写实就不该替他写实；主循环那头有自己的默认质量档，这里再挂一份，
/// 同一套规矩就在一次对话里出现两遍。
///
/// 画风在前、预设在后的顺序照 plan：画风管色数与描边那类硬指标，先定下来，
/// 预设的规矩才好往里填。与画风 id 重合的那条预设整段不发（realistic 画风自带
/// 同一条），同一套要求发两遍只会让模型自己猜哪遍算数。
fn finish_contract(
    target: RefineTarget,
    style: Option<super::artstyle::ArtStyle>,
    presets: &[&'static super::presets::Preset],
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(style) = style {
        out.push(String::new());
        out.push("FINISH CONTRACT (how this picture has to be finished):".to_string());
        out.push(format!("art style: {}", style.id()));
        out.push(style.rules().to_string());
    }
    for preset in presets {
        // 顶替的那几个不出现在合同里：画风段已经写了同一条。
        if style.map(|s| s.id()) == Some(preset.id) {
            continue;
        }
        if out.is_empty() {
            out.push(String::new());
            out.push("FINISH CONTRACT (how this picture has to be finished):".to_string());
        }
        out.push(format!("finish preset: {}", preset.id));
        out.push(preset.rules.to_string());
    }
    if out.is_empty() {
        return out;
    }
    // 合同的两种读法：写 Lua 的那条链里，helper 名（aa* / blend / pget）是真家伙，
    // detail 行可以直接点名；生图那条链里一个工具都没有，同一个名字写进提示词
    // 只会让模型去描一段代码，所以这里明说要把它翻成人话。
    out.push(String::new());
    match target {
        RefineTarget::Shader => out.push(
            "The contract above is written for the Lua drawing sandbox and its helpers are real: \
the palette, lighting, detail and outline lines must already obey it, and the detail line may \
name a helper where one is called for. Where your own default disagrees with the contract, the \
contract wins."
                .to_string(),
        ),
        RefineTarget::ImageGen => out.push(
            "The contract above is a rendering goal, not a tool list: translate it into plain \
picture words in the lines above - no helper names, no function calls, no code of any kind, \
because the consumer is an image model with no tools. Where your own default disagrees with the \
contract, the contract wins."
                .to_string(),
        ),
    }
    out
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
    // 提示词先拼出来再发：认不出的画风 / 预设在这一步就拦下，不花一次请求钱。
    let system = refine_system_prompt(req).map_err(ProviderError::Config)?;
    let text =
        one_shot::chat_once(config, &system, vec![Message::user_text(idea)], Some(900)).await?;
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
            style: None,
            presets: Vec::new(),
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
            disable_thinking: None,
            capabilities: Default::default(),
        }
    }

    #[test]
    fn the_prompt_names_the_canvas_and_the_downstream_target() {
        let text = refine_system_prompt(&request()).expect("没钉风格不该报错");
        assert!(text.contains("32x32"), "{text}");
        assert!(text.contains("pixel_run_shader"), "{text}");
    }

    #[test]
    fn the_image_gen_target_names_a_different_downstream() {
        let mut req = request();
        req.target = RefineTarget::ImageGen;
        let text = refine_system_prompt(&req).expect("没钉风格不该报错");
        assert!(text.contains("image generation"), "{text}");
        assert!(!text.contains("pixel_run_shader"), "{text}");
    }

    #[test]
    fn every_declared_label_is_asked_for() {
        let text = refine_system_prompt(&request()).expect("没钉风格不该报错");
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

    /// 这条链路过去整段不带收尾规矩：用户在输入区选了「写实渲染」，点一下微调，
    /// 出来的九行里一条渲染规矩都没有。选过的画风必须真的落到提示词上。
    #[test]
    fn a_pinned_style_reaches_the_prompt_as_a_finish_contract() {
        let mut req = request();
        req.style = Some("realistic".into());
        let text = refine_system_prompt(&req).expect("认得出的画风不该报错");
        assert!(text.contains("FINISH CONTRACT"), "{text}");
        assert!(text.contains("art style: realistic"), "{text}");
        assert!(
            text.contains("5-7 ramp steps per material"),
            "画风规矩正文要跟着来：{text}"
        );
    }

    /// 什么都没点的时候一段都不给：这条链路的产品是给人逐行改的中间文本，
    /// 用户没要写实就不该替他写实，主循环那头自带默认质量档。
    #[test]
    fn nothing_pinned_adds_no_contract_section_at_all() {
        let text = refine_system_prompt(&request()).expect("没钉风格不该报错");
        assert!(!text.contains("FINISH CONTRACT"), "{text}");
        assert!(!text.contains("default quality tier"), "{text}");
    }

    /// 预设叠几条都上车，且都由用户点的那些规矩组成——它们是乘法关系，
    /// 少一条用户就看得出成品变了。
    #[test]
    fn stacked_presets_ride_along_in_the_order_they_were_picked() {
        let mut req = request();
        req.presets = vec!["microdetail".into(), "occlusion".into()];
        let text = refine_system_prompt(&req).expect("认得出的预设不该报错");
        assert!(text.contains("finish preset: microdetail"), "{text}");
        assert!(text.contains("finish preset: occlusion"), "{text}");
        assert!(
            text.find("microdetail").expect("microdetail")
                < text.find("occlusion").expect("occlusion"),
            "顺序要照用户点的来：{text}"
        );
    }

    /// 与画风 id 重合的那条只发一遍：同一套要求出现两遍，模型会自己猜哪遍算数。
    #[test]
    fn a_preset_that_repeats_the_style_is_not_sent_twice() {
        let mut req = request();
        req.style = Some("realistic".into());
        req.presets = vec!["realistic".into(), "microdetail".into()];
        let text = refine_system_prompt(&req).expect("认得出的组合不该报错");
        assert_eq!(
            text.matches("art style: realistic").count(),
            1,
            "画风段只该有一段：{text}"
        );
        assert_eq!(
            text.matches("finish preset: realistic").count(),
            0,
            "重合的那条由画风段顶替，不重复发：{text}"
        );
        assert!(text.contains("finish preset: microdetail"), "{text}");
    }

    /// 生图那条链上一个工具都没有，helper 名写过去只会让模型去描一段代码。
    /// 同一个合同在两条链里读法不同，这个差别必须说出来。
    #[test]
    fn the_contract_tells_the_image_gen_chain_to_drop_helper_names() {
        let mut shader_req = request();
        shader_req.style = Some("realistic".into());
        let shader = refine_system_prompt(&shader_req).expect("不该报错");
        assert!(
            shader.contains("written for the Lua drawing sandbox"),
            "写脚本的那条链要点明 helper 是真家伙：{shader}"
        );

        let mut image_req = shader_req;
        image_req.target = RefineTarget::ImageGen;
        let image = refine_system_prompt(&image_req).expect("不该报错");
        assert!(
            image.contains("plain picture words"),
            "生图那条链要把合同翻成人话：{image}"
        );
        assert!(
            image.contains("no tools"),
            "要让模型知道对面没有工具：{image}"
        );
    }

    /// 认不出的 id 当场报错，绝不静默丢掉：静默丢掉的后果是用户以为规矩上了路，
    /// 而九行提示词里看不出少的是哪一段。
    #[test]
    fn an_unknown_style_or_preset_is_refused_instead_of_dropped() {
        let mut req = request();
        req.style = Some("photoshop".into());
        assert!(refine_system_prompt(&req).is_err());
        req.style = None;
        req.presets = vec![
            "realistic".into(),
            "microdetail".into(),
            "occlusion".into(),
            "polish".into(),
        ];
        let err = refine_system_prompt(&req).unwrap_err();
        assert!(err.contains("too many prompt presets"), "{err}");
    }
}
