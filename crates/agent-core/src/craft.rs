// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 提示词工程：动笔之前，先让模型把「这一张到底要画什么、不要什么」明确写成
//! 正向提示词和逆向提示词，之后的生图全程以这两份清单为约束。
//!
//! 位置排在整个执行流的第二段：分流（Rust 按用户原话定性）→ 提示词（模型自己写）
//! → 生图（模型照着清单画）。定性只在 Rust 做，写清单必须由模型做——只有它知道
//! 这一张画具体要落到哪些像素决策上；而 Rust 负责把这一步变成硬流程：
//! 没写过清单就动笔，第一次挡下并要求补写，第二次按用户原话兜底合一支基线放行，
//! 保证再倔的模型也画得出图，同时把「这单没走提示词流程」如实告诉用户。

use super::intent::Intent;
use super::models::ToolSpec;
use super::tools::{IMAGE_GEN_TOOL, SHADER_TOOL};
use serde_json::{json, Value};

/// 工具名。语义归本模块，规格由 `spec()` 发出去。
pub const PROMPT_TOOL: &str = "pixel_prompt";

/// 模型不肯先写清单时，按用户原话兜底的逆向提示词。
const FALLBACK_NEGATIVE: &str = "blobs and smudges where a shape was owed; stray single pixels; \
wrong proportions or a mangled silhouette; palette colors outside one ramp; narration instead of \
pixels; a redraw of what is already on the canvas when the user asked to edit";

/// 这一轮的提示词清单。两个字段都允许为空字符串——只要有一边写了就有约束价值。
#[derive(Debug, Clone, Default)]
pub struct CraftedPrompt {
    pub positive: String,
    pub negative: String,
}

impl CraftedPrompt {
    pub fn is_empty(&self) -> bool {
        self.positive.trim().is_empty() && self.negative.trim().is_empty()
    }
}

/// 哪些调用算「生图」。结构操作（建帧、建图层、调色板增删）不算——那是搭台，
/// 不是画画；真正往画布上落 pixels 的几条才算，它们都必须先有清单。
pub fn is_drawing_tool(name: &str) -> bool {
    matches!(
        name,
        SHADER_TOOL | IMAGE_GEN_TOOL | "pixel_pixelize_image" | "pixel_tween_frames"
    )
}

/// 这一轮要不要走提示词流程。要图的话、改画的话、成品意图已定的话都算；
/// 纯问答一句都不算——没人在那种轮次里往画布上落像素。
pub fn required(editing: bool, intent: Option<Intent>, art_requested: bool) -> bool {
    art_requested || editing || intent.is_some()
}

/// 解析入参。模型写键名全看心情，常见几种都接；一边缺省就只约束另一边。
pub fn parse(input: &Value) -> Result<CraftedPrompt, String> {
    let positive = read_list(
        input,
        &["positive", "positive_prompt", "prompt", "pos", "keep"],
    );
    let negative = read_list(
        input,
        &["negative", "negative_prompt", "avoid", "avoid_list", "neg"],
    );
    let crafted = CraftedPrompt {
        positive: positive.join("; "),
        negative: negative.join("; "),
    };
    if crafted.is_empty() {
        return Err(format!(
            "{PROMPT_TOOL}: send at least one of 'positive' / 'negative' - the lists this turn's \
             drawing will be judged against"
        ));
    }
    Ok(crafted)
}

/// 先把这几个键当字符串读，读不到再当数组读——两种容器模型都常用。
fn read_list(input: &Value, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let Some(value) = input.get(*key) else {
            continue;
        };
        match value {
            Value::String(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return Vec::new();
                }
                return vec![trimmed.to_string()];
            }
            Value::Array(items) => {
                return items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            _ => {}
        }
    }
    Vec::new()
}

/// 清单还没写时插在系统提示词里的要求段。
pub fn required_section() -> String {
    format!(
        "PROMPT CRAFT - mandatory before any drawing tool this turn:\n\
         Call {PROMPT_TOOL} exactly once, before your first drawing tool, and write the brief this \
         turn's artwork will be drawn from:\n\
         - positive: everything the drawing MUST contain - subject and silhouette, proportions and \
         pose, palette ramps and light direction, the signature details, and for animation the \
         motion, timing and loop seam - distilled from the user's own words plus the routing \
        above, as one compact list.\n\
        Write SIZE and PLACEMENT relative to the canvas - fills most of the canvas width, \
        centred on canvas.cx / canvas.cy, one third up from the bottom - and never as an \
        absolute pixel box such as 16x16 or sits-at-x-20: a literal box becomes literal \
        coordinates and parks the subject in a corner of a larger canvas.\n\
        - negative: everything the drawing MUST NOT contain - the failure modes you would \
        otherwise produce on this canvas (smudged shapes, stray pixels, wrong proportions, \
        palette clashes, extra ramp colors, a redraw of approved pixels), as one compact list.\n\
        Then draw from those two lists: every pixel decision serves the positive list and avoids \
        the negative list. Do not restate the lists back to the user.\
        Quality tier: a named art style binds this turn, and when the user named none the \
        default quality tier binds. Fold its levers into the positive list you just wrote - how \
        many ramp steps to build, where the terminator sits under ONE light direction, how deep \
        occlusion darkens, and when to stop adding detail."
    )
}

/// 清单写定之后插在系统提示词里的约束段。它替换掉要求段——写过了就不再要求。
pub fn bound_section(crafted: &CraftedPrompt) -> String {
    let positive = if crafted.positive.trim().is_empty() {
        "(none written)"
    } else {
        crafted.positive.trim()
    };
    let negative = if crafted.negative.trim().is_empty() {
        "(none written)"
    } else {
        crafted.negative.trim()
    };
    format!(
        "CRAFTED PROMPTS FOR THIS TURN - written by {PROMPT_TOOL} and BINDING for every drawing \
         tool you call now:\n\
         POSITIVE (what this drawing must contain):\n{positive}\n\
        NEGATIVE (what this drawing must not contain):\n{negative}\n\
        Draw strictly inside these two lists: they are the brief for this turn's artwork, and a \
        drawing that ignores either side of it is wrong. If the user's words clearly contradict \
         a list, rewrite it with {PROMPT_TOOL} once before drawing again.\
        Any size or spot the positive list names is relative to the whole canvas: scale it off \
        canvas.cx / canvas.cy / canvas.scale so the subject lands centred and large on this \
        canvas, not at literal coordinates inside a corner."
    )
}

/// 写清单那一次调用的回填。结论要短，把「照着画、别复述」说死。
pub fn recorded_text(crafted: &CraftedPrompt) -> String {
    let positive = if crafted.positive.trim().is_empty() {
        "(none)"
    } else {
        crafted.positive.trim()
    };
    let negative = if crafted.negative.trim().is_empty() {
        "(none)"
    } else {
        crafted.negative.trim()
    };
    format!(
        "prompt lists recorded; they now bind every drawing tool this turn. Draw from them now - \
         POSITIVE: {positive} | NEGATIVE: {negative}. Do not restate the lists to the user."
    )
}

/// 动笔被挡下时的那一句。必须把「先写清单」和「写完就把画重新发一次」说清楚，
/// 不然模型会以为这一单被否了，转头去问用户。
pub fn craft_first_refusal() -> String {
    format!(
        "{PROMPT_TOOL} comes first: this turn has no prompt lists yet. Write the positive and \
         negative lists for this artwork and send them with {PROMPT_TOOL}, then resend the \
         drawing call - the drawing itself was not started and nothing was rejected."
    )
}

/// 模型第二次仍然跳过清单时，按用户原话兜底的那一支。用真话兜底：
/// 清单内容就是用户这句话本身，逆向那一侧用通用失败模式顶着。
pub fn fallback(user_text: &str, routing: &str) -> CraftedPrompt {
    let subject = user_text.trim();
    let trimmed_routing = routing.trim();
    let positive = if subject.is_empty() {
        trimmed_routing.to_string()
    } else if trimmed_routing.is_empty() {
        subject.to_string()
    } else {
        format!("{subject}\n{trimmed_routing}")
    };
    CraftedPrompt {
        positive,
        negative: FALLBACK_NEGATIVE.to_string(),
    }
}

/// 工具规格。描述里把「先于任何生图工具」「一次」和两边的写法都说清楚，
/// 免得模型把它当成可选的草稿步骤。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: PROMPT_TOOL.into(),
        description: format!(
            "Write this turn's prompt lists BEFORE any drawing tool. Call it exactly once per \
             turn, ahead of the first call to {SHADER_TOOL} / {IMAGE_GEN_TOOL} / \
             pixel_pixelize_image / pixel_tween_frames. positive: one compact list of everything \
             the drawing must contain (subject, silhouette, proportions, palette, light \
             direction, signature details, motion for animation). negative: one compact list of \
             everything it must not contain (smudged shapes, stray pixels, wrong proportions, \
             palette clashes, redraws of approved pixels). The lists bind every drawing tool in \
             this turn."
        )
        .into(),
        schema: json!({
            "type": "object",
            "properties": {
                "positive": {
                    "description": "What the drawing must contain, as one compact list.",
                    "type": "string"
                },
                "negative": {
                    "description": "What the drawing must not contain, as one compact list.",
                    "type": "string"
                }
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::PLAN_TOOL;

    #[test]
    fn both_spellings_of_each_list_are_read() {
        let a = parse(&json!({"positive": "a cat", "negative": "no blobs"})).unwrap();
        assert_eq!(a.positive, "a cat");
        assert_eq!(a.negative, "no blobs");
        let b =
            parse(&json!({"positive_prompt": ["a cat", "orange"], "avoid": ["blobs"]})).unwrap();
        assert_eq!(b.positive, "a cat; orange");
        assert_eq!(b.negative, "blobs");
    }

    #[test]
    fn one_sided_lists_still_count() {
        let only_positive = parse(&json!({"positive": "a fox"})).unwrap();
        assert!(!only_positive.is_empty());
        assert_eq!(only_positive.negative, "");
        let only_negative = parse(&json!({"negative": "no dither"})).unwrap();
        assert!(!only_negative.is_empty());
        assert_eq!(only_negative.positive, "");
    }

    #[test]
    fn an_empty_payload_is_an_error_not_a_blank_brief() {
        let err = parse(&json!({})).unwrap_err();
        assert!(err.contains("at least one"), "{err}");
    }

    #[test]
    fn drawing_tools_need_the_step_structure_does_not() {
        assert!(is_drawing_tool(SHADER_TOOL));
        assert!(is_drawing_tool(IMAGE_GEN_TOOL));
        assert!(is_drawing_tool("pixel_pixelize_image"));
        assert!(is_drawing_tool("pixel_tween_frames"));
        assert!(!is_drawing_tool("pixel_apply_operations"));
        assert!(!is_drawing_tool("pixel_read_canvas"));
        assert!(!is_drawing_tool(PLAN_TOOL));
    }

    #[test]
    fn the_bound_section_carries_both_lists_and_the_rule() {
        let crafted = CraftedPrompt {
            positive: "a side-view fox, warm ramp".into(),
            negative: "no stray pixels".into(),
        };
        let section = bound_section(&crafted);
        assert!(section.contains("a side-view fox"), "{section}");
        assert!(section.contains("no stray pixels"), "{section}");
        assert!(section.contains(PROMPT_TOOL), "{section}");
    }

    /// 尺寸和落位必须写成画布相对量。实测里模型把「16x16」当成字面坐标，
    /// 在 32x32 的画布上画出了一个缩在左上角的 16px 图标——正是这条规则要堵的。
    #[test]
    fn the_prompt_step_demands_canvas_relative_size_not_a_pixel_box() {
        let ask = required_section();
        assert!(
            ask.contains("SIZE and PLACEMENT relative to the canvas"),
            "{ask}"
        );
        assert!(ask.contains("never as an"), "{ask}");
        assert!(
            ask.contains("absolute pixel box such as 16x16"),
            "没把实测翻车的那一种写法点出来，模型认不出是在说自己：{ask}"
        );
        // 写完之后那段同样得提醒：模型是写完清单立刻动笔的，眼前只有绑定段。
        let bound = bound_section(&CraftedPrompt::default());
        assert!(bound.contains("relative to the whole canvas"), "{bound}");
        assert!(bound.contains("canvas.scale"), "没给出折算手段：{bound}");
    }

    /// 正向清单要把质量档吸收进去。
    ///
    /// 过去渲染规矩只挂在「风格预设」名下，用户没点名风格的那一轮（恰恰是最常见
    /// 的一轮）拿到的清单就只剩主体和落位，生图模型照自己的训练分布交差——
    /// 用户嘴里「生成得一点不写实」，十个里有八个是这么来的。
    #[test]
    fn the_prompt_step_absorbs_the_quality_tier() {
        let ask = required_section();
        assert!(ask.contains("Quality tier"), "{ask}");
        assert!(
            ask.contains("default quality tier"),
            "没点名风格那一轮也得有档可依：{ask}"
        );
        assert!(ask.contains("ramp steps"), "色阶级数要写进清单：{ask}");
        assert!(ask.contains("ONE light direction"), "光向要写进清单：{ask}");
        assert!(
            ask.contains("stop adding detail"),
            "停手条件要写进清单：{ask}"
        );
    }
}
