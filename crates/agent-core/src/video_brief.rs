//! 视频运动简报：让读视频的模型先把一段视频「读」成结构化文字，再交给 agent 主循环落笔。
//!
//! 与 `vision.rs` 的分工：读图简报回答「画的是什么」，这里回答「怎么动」。
//! 像素画里的运动只有三个抓手——形体怎么变、时序怎么排、颜色怎么跟随，
//! 所以字段就围着这三件事设计，别的都不问。
//!
//! 为什么不像 `video_frames` 那样直接抽帧量化：
//! 1. 抽帧的产物是位图，位图没有「这段是预备、这段是击发」这种运动语义。
//! 2. 一段 3 秒的动画按 12fps 抽就是 36 帧，逐帧量化会瞬间堆出用户并不想要的帧数。
//! 3. 简报可编辑：用户把「三段挥锤改成两段」改完再发去画，比重新剪辑视频便宜。
//!
//! 输入是若干张静帧（已缩成小图）按顺序组成的图片块；模型要看的是变化，
//! 所以系统提示词必须交代「这些是同一段视频按时间顺序取的帧」。

use super::models::{Attachment, ChatRequest, Message, ModelConfig};
use super::one_shot;
use super::providers::ProviderError;
use super::video::{ProbeSource, VideoProbe};
use serde::{Deserialize, Serialize};

/// 一次最多喂给模型多少帧。再多不增加运动语义，只是把请求撑大。
pub const MAX_BRIEF_FRAMES: usize = 12;

/// 喂给模型的静帧缩到多长。参考图简报用原图（模型要看清轮廓），
/// 但运动简报靠「帧与帧的差别」说话，256px 足够看出差别，还能省下大量 token。
pub const BRIEF_THUMB_MAX_DIM: u32 = 256;

/// 从 `total` 张抽出的帧里挑最多 `MAX_BRIEF_FRAMES` 个下标，**均匀铺满整段**。
///
/// 抽帧本身已经是按时长均匀采样的，但如果用户一次抽得比上限多，
/// 取前 N 张就只覆盖片段开头——那是把「整段动作」读成「起手式」。
/// 这里按步长在整个下标区间上取点，保证简报看得见结尾。
/// `total` 为 0 时返回空，调用方据此在发请求前就报错。
pub fn brief_frame_indices(total: usize) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    let wanted = total.min(MAX_BRIEF_FRAMES);
    if wanted == total {
        return (0..total).collect();
    }
    // 步长取小数再加 0.5，相当于在每段里取中点：避开首帧（常是黑场或静止帧）。
    let step = total as f64 / wanted as f64;
    (0..wanted)
        .map(|i| (i as f64 * step + step * 0.5) as usize)
        .collect()
}

/// 一段视频的结构化运动简报。字段都是短文本，给人看也给 agent 看。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoBrief {
    /// 一句话说清这段视频在表现什么动作。
    pub subject: String,
    /// 形体怎么变：哪些部位在动，各自的幅度和方向。
    pub motion: String,
    /// 关键姿态的有序列表，模型观察到的每个转折点一条。
    pub key_poses: Vec<String>,
    /// 节奏与持续：总帧数建议、哪些帧是预备/极限/收势，以及帧间隔。
    pub timing: String,
    /// 主色到暗色的有序色板，hex 字符串。
    pub palette: Vec<String>,
    /// 落到像素画上的具体建议：循环怎么接、该简化的细节、必须读得清的轮廓。
    pub craft_notes: String,
    /// 模型原文，解析不全时至少留个痕迹。
    pub raw: String,
}

/// 把探到的素材信息压成一句给模型看的话。没有时长/帧率就返回 None——
/// 硬编一个「大约几秒」比不说更糟，模型的 timing 会顺着这句瞎话继续编。
pub fn source_note(probe: &VideoProbe, source: ProbeSource) -> Option<String> {
    if matches!(source, ProbeSource::Directory) {
        return Some(format!(
            "{} stills in time order",
            probe.frame_count.unwrap_or(0)
        ));
    }
    let mut parts: Vec<String> = Vec::new();
    if let (Some(w), Some(h)) = (probe.width, probe.height) {
        parts.push(format!("{w}x{h}"));
    }
    if let Some(d) = probe.duration_s {
        parts.push(format!("{d:.1}s long"));
    }
    if let Some(f) = probe.fps {
        parts.push(format!("{f:.2} fps"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

/// 读视频用的系统提示词。和读图简报一样要求紧凑 JSON，
/// 但额外交代目标画布尺寸与帧序：不说是视频，模型会逐张描述成不相干的图。
fn system_prompt(width: u32, height: u32, count: usize, source: Option<&str>) -> String {
    // 探不到素材信息就不说：模型会自己估一个不确定的值；
    // 但它要是看见「0.9s long, 12.00 fps」，就会把 timing 按真实长度算。
    let source_line = match source {
        Some(note) => format!(" The source clip is {note}."),
        None => String::new(),
    };
    format!(
        "You analyse video for a pixel art editor. The canvas the art must be redrawn on is {width}x{height} pixels.\n\
You are given {count} stills taken in time order from ONE short video clip, first to last.{source_line}\n\n\
Reply with ONE compact JSON object and nothing else, no code fences, using exactly these keys:\n\
{{\"subject\":\"\",\"motion\":\"\",\"key_poses\":[],\"timing\":\"\",\"palette\":[],\"craft_notes\":\"\"}}\n\n\
RULES:\n\
- subject: one short sentence naming the action, not the objects.\n\
- motion: which parts move, how far, and in what direction. Compare the stills against each other; a part that never moves is not motion.\n\
- key_poses: 2-5 short strings in time order, each naming one distinct pose the clip passes through.\n\
- timing: how many pixel frames this would take at about 8-12 fps, which frames are anticipation / impact / settle, and whether it should loop.\n\
- palette: 4-8 colors ordered light to dark, as \"#RRGGBB\" strings, taken from the stills themselves. Hue-shift the shadows cooler and the highlights warmer.\n\
- craft_notes: concrete pixel-art advice for animating this subject: how to keep the silhouette readable at 1-2 pixels of movement, which details to drop, and how a loop should close.\n\
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

/// 按顺序读若干静帧，产出结构化运动简报。
///
/// 一帧都没有就直接返回配置错误，**在任何网络请求之前**：
/// 没图可看的时候发请求，只会拿到一句「我看不到任何东西」，
/// 用户却以为自己的模型不支持读视频。
pub async fn brief_video(
    config: &ModelConfig,
    frames: &[Attachment],
    width: u32,
    height: u32,
    source: Option<&str>,
) -> Result<VideoBrief, ProviderError> {
    if frames.is_empty() {
        return Err(ProviderError::Config(
            "a video brief needs at least one frame to look at".into(),
        ));
    }
    // max_tokens 收到 1500：和读图简报同级，结构化短文本给多了模型就开始写散文。
    let text = one_shot::chat_once(
        config,
        &system_prompt(width, height, frames.len(), source),
        vec![frames_message(frames, width, height)],
        Some(1500),
    )
    .await?;

    if let Some(json_text) = extract_json_object(&text) {
        if let Ok(mut brief) = serde_json::from_str::<VideoBrief>(json_text) {
            // key_poses 是这条流程的核心，模型却经常只给 motion 就收手；
            // 空着的话下游拿不到任何姿态序列，这里兜一句。
            if brief.key_poses.is_empty() {
                brief.key_poses =
                    vec!["(model named no distinct poses; read the motion line)".into()];
            }
            if brief.timing.trim().is_empty() {
                brief.timing = "about 8 frames at 12 fps; decide the loop by feel".into();
            }
            brief.raw = text.clone();
            return Ok(brief);
        }
    }
    // 解析不出来也别浪费这次调用：原文整体当作 craft_notes 交给 agent，至少有信息量。
    Ok(unparsed_brief(&text))
}

/// 模型没给出可解析 JSON 时的兜底简报：原文进 craft_notes，
/// key_poses 留一句提示，让下游知道「不是没动作，是没解析出来」。
fn unparsed_brief(text: &str) -> VideoBrief {
    VideoBrief {
        subject: "(see raw notes)".into(),
        motion: String::new(),
        key_poses: vec!["(unparsed reply)".into()],
        timing: String::new(),
        palette: Vec::new(),
        craft_notes: text.trim().to_string(),
        raw: text.to_string(),
    }
}

/// 简报在 agent 工作流里要用的最小请求构造，单独拆出来便于测试。
pub fn brief_request(
    frames: &[Attachment],
    width: u32,
    height: u32,
    source: Option<&str>,
) -> ChatRequest {
    ChatRequest {
        system: system_prompt(width, height, frames.len(), source),
        messages: vec![frames_message(frames, width, height)],
        tools: Vec::new(),
        max_tokens: 1500,
        temperature: None,
        // 简报是单轮请求，历史里没有推理内容可回灌。
        echo_reasoning: false,
        // 简报要的是一段文字，别让模型先思前想后把 1500 额度烧光。
        disable_thinking: true,
    }
}

/// 让 `brief_video` 与 `brief_request` 共用同一份消息构造，避免两处漂移。
fn frames_message(frames: &[Attachment], width: u32, height: u32) -> Message {
    let mut content = vec![super::models::ContentBlock::Text {
        text: format!(
            "These {n} images are stills from ONE video clip, in time order. \
Summarise the motion for a {width}x{height} pixel animation.",
            n = frames.len()
        ),
    }];
    for frame in frames {
        content.push(super::models::ContentBlock::Image {
            media_type: frame.media_type.clone(),
            data_base64: frame.data_base64.clone(),
        });
    }
    Message {
        role: super::models::Role::User,
        content,
    }
}

#[cfg(test)]
mod tests {
    use super::super::models::{AttachmentRole, Protocol};
    use super::*;

    fn frames(n: usize) -> Vec<Attachment> {
        (0..n)
            .map(|i| Attachment {
                role: AttachmentRole::Reference,
                media_type: "image/png".into(),
                data_base64: format!("AAAA{i}"),
            })
            .collect()
    }

    #[test]
    fn finds_json_even_when_the_model_chats_around_it() {
        let text = "Here you go:\n```json\n{\"subject\":\"a hammer swing\",\"key_poses\":[\"wind up\",\"strike\"]}\n```\nDone.";
        let json = extract_json_object(text).expect("should find the object");
        assert!(json.starts_with('{'), "{json}");
        assert!(json.ends_with('}'), "{json}");
        let brief: VideoBrief = serde_json::from_str(json).unwrap();
        assert_eq!(brief.subject, "a hammer swing");
        assert_eq!(
            brief.key_poses,
            vec!["wind up".to_string(), "strike".to_string()]
        );
    }

    #[test]
    fn a_reply_with_no_braces_yields_none() {
        assert!(extract_json_object("no json here").is_none());
        assert!(extract_json_object("{unbalanced").is_none());
    }

    #[test]
    fn missing_fields_deserialize_as_empty_not_as_errors() {
        let brief: VideoBrief = serde_json::from_str(r#"{"subject":"a hop"}"#).unwrap();
        assert_eq!(brief.subject, "a hop");
        assert!(brief.motion.is_empty());
        assert!(brief.key_poses.is_empty());
        assert!(brief.raw.is_empty());
    }

    #[tokio::test]
    async fn no_frames_means_no_network_call_at_all() {
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
        // 端点不可达：真发出去会是 Network 错误，Config 错误才能证明没发。
        let err = brief_video(&config, &[], 32, 32, None).await.unwrap_err();
        assert!(matches!(err, ProviderError::Config(_)), "{err}");
        assert!(err.to_string().contains("at least one frame"), "{err}");
    }

    #[test]
    fn the_request_carries_the_frame_count_and_canvas_in_the_system_prompt() {
        let req = brief_request(&frames(3), 48, 32, None);
        assert!(req.system.contains("48x32"));
        assert!(req.system.contains("3 stills"), "{}", req.system);
        let text = req.messages[0].text_of();
        assert!(text.contains("3 images"), "{text}");
        assert!(text.contains("48x32"), "{text}");
        assert_eq!(req.tools.len(), 0);
    }

    #[test]
    fn every_frame_lands_in_the_same_message_in_order() {
        let all = frames(4);
        let message = frames_message(&all, 16, 16);
        assert_eq!(message.role, super::super::models::Role::User);
        let images: Vec<&str> = message
            .content
            .iter()
            .filter_map(|b| match b {
                super::super::models::ContentBlock::Image { data_base64, .. } => {
                    Some(data_base64.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(images, vec!["AAAA0", "AAAA1", "AAAA2", "AAAA3"]);
        assert!(brief_request(&all, 16, 16, None).messages[0]
            .text_of()
            .contains("ONE video"));
    }

    #[test]
    fn brief_video_and_brief_request_agree_on_the_message() {
        // 两处构造必须一致，否则测试过了、线上行为却是另一套。
        let all = frames(2);
        let a = frames_message(&all, 24, 24);
        let b = brief_request(&all, 24, 24, Some("0.9s long, 12.00 fps")).messages[0].clone();
        assert_eq!(a, b);
    }

    #[test]
    fn an_unparsed_reply_still_arrives_as_usable_notes() {
        let brief = unparsed_brief("I think something moves, maybe.");
        assert_eq!(brief.subject, "(see raw notes)");
        // 原文不能丢：它是唯一的信息来源，也是用户判断「这模型不行」的依据。
        assert_eq!(brief.craft_notes, "I think something moves, maybe.");
        assert_eq!(brief.raw, brief.craft_notes);
        assert_eq!(brief.key_poses, vec!["(unparsed reply)".to_string()]);
        // 动作序列不能空：空 key_poses 会让简报读起来像张静止图。
        assert!(!brief.key_poses.is_empty());
    }

    #[test]
    fn picking_frames_spreads_over_the_whole_clip() {
        assert!(brief_frame_indices(0).is_empty());
        assert_eq!(brief_frame_indices(4), vec![0, 1, 2, 3]);
        // 24 抽 12：取每段中点，最后一帧也在内——否则整段动作被读成起手式。
        let picked = brief_frame_indices(24);
        assert_eq!(picked, vec![1, 3, 5, 7, 9, 11, 13, 15, 17, 19, 21, 23]);
        // 超过上限一律收到 MAX_BRIEF_FRAMES 张。
        assert_eq!(brief_frame_indices(500).len(), MAX_BRIEF_FRAMES);
        for (position, &index) in brief_frame_indices(500).iter().enumerate() {
            assert!(index < 500);
            assert!(index >= position, "indices must never go backwards");
        }
    }

    #[test]
    fn a_known_clip_length_reaches_the_model_but_a_guess_never_does() {
        let clip = VideoProbe {
            width: Some(640),
            height: Some(360),
            duration_s: Some(0.9),
            fps: Some(12.0),
            ..Default::default()
        };
        assert_eq!(
            source_note(&clip, ProbeSource::Ffprobe).as_deref(),
            Some("640x360, 0.9s long, 12.00 fps")
        );
        // 探到什么就说什么，探不到就别硬编：这句会直接决定 timing 字段的可信度。
        assert_eq!(
            source_note(&VideoProbe::default(), ProbeSource::Ffprobe),
            None
        );
        // 目录来源没有时长，但帧数是真信息。
        let stills = VideoProbe {
            frame_count: Some(9),
            ..Default::default()
        };
        assert_eq!(
            source_note(&stills, ProbeSource::Directory).as_deref(),
            Some("9 stills in time order")
        );

        let req = brief_request(
            &frames(2),
            32,
            32,
            source_note(&clip, ProbeSource::Ffprobe).as_deref(),
        );
        assert!(
            req.system
                .contains("The source clip is 640x360, 0.9s long, 12.00 fps."),
            "{}",
            req.system
        );
        let silent = brief_request(&frames(2), 32, 32, None);
        assert!(!silent.system.contains("source clip"), "{}", silent.system);
    }
}
