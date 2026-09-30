// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 本轮分流：把「这句话到底要什么」一次说清楚，只说一次。
//!
//! 分流由四件事合成，每件都是用户一句话就能钉死、模型却最容易猜错的方向：
//! 参照定性（只借画风还是照着实临摹）、成品意图（瓦片 / 角色 / 场景 / 图标 / 图案 / 道具）、
//! 风格预设（1-bit、Game Boy、PICO-8…）、以及知识库这一轮该带哪几条。
//!
//! 收口成一个 `TurnPlan` + 一个可纠正工具（`pixel_plan`）而不是四个工具：
//! 四张卡片只会把对话刷满，而用户真正想知道的只有一句「这一轮被理解成了什么」。
//! 定性的位置仍然在 Rust：模型把预算全花在思考、一个工具都不调的场面真实存在，
//! 那时候约束必须已经在路上，不能等它开口。

use super::models::ToolSpec;
use super::{
    artstyle, intent, knowledge, references, Attachment, AttachmentRole, ReferenceMode,
    ReferenceUpdate,
};
use serde_json::{json, Value};

/// 工具名。语义归本模块，规格由 `spec()` 发出去。
pub const PLAN_TOOL: &str = "pixel_plan";

/// 显式「这一项不限」的说法。模型想清掉上一轮的预设时会这么写。
const NONE_WORDS: &[&str] = &["none", "null", "clear", "auto", "unset", "any"];

/// 这一轮的分流结果。跟着 turn 走：开 turn 时按用户原话定一次，
/// 模型调 `pixel_plan` 纠正时就地改写，之后的每一发请求都带着新结论上路。
#[derive(Debug, Clone, Default)]
pub struct TurnPlan {
    /// 与附件清单一一对齐，快照位恒 `None`。
    pub reference_modes: Vec<Option<ReferenceMode>>,
    pub intent: Option<intent::Intent>,
    /// 定意图时命中的那个说法。节点上要显示，用户看一眼就知道判据是什么。
    pub intent_hit: Option<String>,
    pub style: Option<artstyle::ArtStyle>,
    pub style_hit: Option<String>,
    /// 这一轮是在已经画好的画布上动刀。与成品意图平级的另一根轴：
    /// 「把这只猫的动作改一下」要的成品仍是 sprite，操作却是改。
    pub editing: bool,
    /// 判「改」时命中的那个说法，和 intent_hit 一个用法。
    pub editing_hit: Option<String>,
    pub knowledge_ids: Vec<String>,
}

impl TurnPlan {
    /// 按用户原话定这一轮的分流。`text` 同时用于参照定性、意图、风格和知识检索。
    /// `canvas_has_pixels` 为假时改画轴整条不发：空画布上「改」没有落点，
    /// 「画只猫，加个项圈」词面上是改、实际得从零画，那时候带着
    /// 「不许 clear()、不许重画」的禁令上路，只会让模型对着一张空网格犯难。
    pub fn from_text(text: &str, attachments: &[Attachment], canvas_has_pixels: bool) -> Self {
        let base_mode = references::classify_text(text).0;
        let (intent, intent_hit) = match intent::classify_text(text) {
            Some((intent, hit)) => (Some(intent), Some(hit)),
            None => (None, None),
        };
        let (style, style_hit) = match artstyle::classify_text(text) {
            Some((style, hit)) => (Some(style), Some(hit)),
            None => (None, None),
        };
        let (editing, editing_hit) = if canvas_has_pixels {
            match intent::classify_edit(text) {
                Some(hit) => (true, Some(hit)),
                None => (false, None),
            }
        } else {
            (false, None)
        };
        let mut knowledge_ids = knowledge::matched_ids(text, knowledge::DEFAULT_LIMIT);
        if editing {
            knowledge_ids = collapse_to_refine(knowledge_ids);
        }
        TurnPlan {
            reference_modes: attachments
                .iter()
                .map(|item| (item.role == AttachmentRole::Reference).then_some(base_mode))
                .collect(),
            intent,
            intent_hit,
            style,
            style_hit,
            editing,
            editing_hit,
            knowledge_ids,
        }
    }

    /// 有参照图才发分流节点。纯快照的一轮不发：节点是给参照定性看的，
    /// 一张参考都没有还摆一张卡片，只能是噪声。
    pub fn has_references(&self) -> bool {
        self.reference_modes.iter().any(Option::is_some)
    }

    /// function_call 节点的入参。形状与模型自己调工具时送来的保持一致，
    /// 这样「自动分流」和「模型纠正」在对话里长得是同一种东西。
    pub fn node_input(&self) -> Value {
        let references: Vec<Value> = self
            .reference_modes
            .iter()
            .enumerate()
            .filter_map(|(i, mode)| {
                mode.map(|mode| json!({ "index": i + 1, "mode": mode.as_str() }))
            })
            .collect();
        let mut input = json!({ "references": references });
        if let Some(intent) = self.intent {
            input["intent"] = json!({ "id": intent.id(), "because": self.intent_hit });
        }
        if let Some(style) = self.style {
            input["style"] = json!({ "id": style.id(), "because": self.style_hit });
        }
        if !self.knowledge_ids.is_empty() {
            input["knowledge"] = json!(self.knowledge_ids);
        }
        if self.editing {
            input["edit"] = json!({ "mode": "refine", "because": self.editing_hit });
        }
        input
    }

    /// 写进系统提示词的本轮规则。没有定出任何东西就返回空串：
    /// 一段写着「什么都不限」的标题，比没有更占预算。
    /// 知识段由 `knowledge::prompt_section` 另给，那里有自己的条数/预算控制。
    pub fn prompt_sections(&self) -> String {
        let mut out = String::new();
        if let Some(intent) = self.intent {
            out.push_str(&format!(
                "- deliverable: {} (the prompt says \"{}\"): {}\n",
                intent.id(),
                self.intent_hit.as_deref().unwrap_or(""),
                intent.rules()
            ));
        }
        if let Some(style) = self.style {
            out.push_str(&format!(
                "- art style: {} (the prompt says \"{}\"): {}\n",
                style.id(),
                self.style_hit.as_deref().unwrap_or(""),
                style.rules()
            ));
        }
        if self.has_references() {
            out.push_str("- reference modes: ");
            for (i, mode) in self.reference_modes.iter().enumerate() {
                if let Some(mode) = mode {
                    out.push_str(&format!("image {} = {}. ", i + 1, mode.as_str()));
                }
            }
            out.push_str("The per-image rules are restated in the attachment list below; obey them as written.\n");
        }
        if self.editing {
            // 改画是本轮的元规则，排在 deliverable 之前：它决定「动还是不动手」，
            // 而成品类型只决定「画成什么样」。少了这一条，模型会照 intent 那一句
            // 把主体从头再画一遍，用户在编辑器里的手笔和已确认的姿态一起被吃掉。
            out.push_str(&format!(
                "- operation: EDIT what is already on the canvas (the prompt says \"{}\"). Read the current grid first and keep the approved silhouette, pose, proportions, pixel positions and palette. Never open the script with clear() and never redraw the whole subject: change information inside the existing pixels, one small pass at a time, and only add new structure where the user named something that is not there yet. If you were about to start over, stop and edit instead.\n",
                self.editing_hit.as_deref().unwrap_or("")
            ));
        }
        if out.is_empty() {
            return out;
        }
        format!(
            "TURN ROUTING (read from the user's own words before this turn; it is binding unless it is clearly wrong - then call {PLAN_TOOL} ONCE to correct it before any drawing tool, and never otherwise):\n{out}"
        )
    }

    /// 回灌给模型的结论。第一行就是界面上那条摘要，所以保持一行说完。
    pub fn outcome_text(&self) -> String {
        let mut out = String::from("turn routing for this turn:\n");
        if self.editing {
            out.push_str(&format!(
                "- operation: edit existing pixels (the prompt says \"{}\")\n",
                self.editing_hit.as_deref().unwrap_or("")
            ));
        }
        match (self.intent, &self.intent_hit) {
            (Some(intent), Some(hit)) => out.push_str(&format!(
                "- deliverable: {} (the prompt says \"{hit}\")\n",
                intent.id()
            )),
            (Some(intent), None) => out.push_str(&format!("- deliverable: {}\n", intent.id())),
            _ => {}
        }
        match (self.style, &self.style_hit) {
            (Some(style), Some(hit)) => out.push_str(&format!(
                "- art style: {} (the prompt says \"{hit}\")\n",
                style.id()
            )),
            (Some(style), None) => out.push_str(&format!("- art style: {}\n", style.id())),
            _ => {}
        }
        if !self.knowledge_ids.is_empty() {
            out.push_str(&format!(
                "- knowledge notes: {}\n",
                self.knowledge_ids.join(", ")
            ));
        }
        for (i, mode) in self.reference_modes.iter().enumerate() {
            if let Some(mode) = mode {
                out.push_str(&format!(
                    "- reference image {} = {}: {}\n",
                    i + 1,
                    mode.as_str(),
                    references::rules(*mode)
                ));
            }
        }
        if out.lines().count() == 1 {
            out.push_str("- nothing was recognized; fall back on the general craft rules.\n");
        }
        out
    }
}

/// 模型通过工具给出的纠正。`references` 是参照定性，`intent` / `style` 为
/// `None` 表示这一轮没提这一项；带一个 `None` 值（`"none"`）表示要清掉它。
#[derive(Debug, Clone, Default)]
pub struct PlanUpdates {
    pub references: Vec<ReferenceUpdate>,
    pub intent: Option<Option<intent::Intent>>,
    pub style: Option<Option<artstyle::ArtStyle>>,
    /// 这一轮是改还是重画。`Some(true)` 改、`Some(false)` 重画。
    pub edit: Option<bool>,
    pub because: Vec<String>,
}

/// 解析工具的入参。入参写成什么样全看模型心情，所以容器层什么都接；
/// 真正不能容的只有两样——下标重复、值不认识，因为那会让分流表和附件对不上。
pub fn parse_updates(input: &Value) -> Result<PlanUpdates, String> {
    let mut out = PlanUpdates::default();
    // references 的容器兼容全在 references 里（数组 / 数字键对象 / 裸对象 / 裸字符串），
    // 这里直接转交：本工具的入参就是它那份入参，外加 intent 和 style 两个键。
    // 但只在入参真的在说参照时才转交。模型经常只纠正一件事——{"intent":"scene"}
    // 就是这么一条——硬转交会先撞上「缺 mode」，真正的错误（不认识 scene）反而
    // 被盖住，模型照着这句摸不着头脑的话再改一遍，还是错。
    if talks_about_references(input) {
        out.references = references::parse_updates(input)?;
    }
    if let Some(value) = input.get("intent") {
        if let Some(because) = cleared(value) {
            out.intent = Some(None);
            out.because.push(because);
        } else {
            let (intent, because) = intent::parse_value(value)?;
            out.intent = Some(Some(intent));
            if let Some(because) = because {
                out.because.push(because);
            }
        }
    }
    if let Some(value) = input.get("style") {
        if let Some(because) = cleared(value) {
            out.style = Some(None);
            out.because.push(because);
        } else {
            let (style, because) = artstyle::parse_value(value)?;
            out.style = Some(Some(style));
            if let Some(because) = because {
                out.because.push(because);
            }
        }
    }
    if let Some(value) = input.get("edit") {
        // 布尔、字符串、{"mode": ...} 都由 intent 那边认——同一套入参形状，
        // 不该在两处定义两种语义。认不出的值直接报错：猜反了就是
        // 「该改的时候重画、该重画的时候改」，两种都是不可逆的损失。
        let edit = intent::parse_edit(value)?;
        out.edit = Some(edit);
        // because 可能是兄弟键（{"edit": true, "because": "..."}）也可能是内嵌的
        // （{"edit": {"mode": "refine", "because": "..."}}），两种都读得到：
        // 用户看着节点才知道「改」是被模型按哪句话定的。
        let because = value
            .get("because")
            .or_else(|| input.get("because"))
            .and_then(Value::as_str);
        if let Some(because) = because {
            let because = because.trim().to_string();
            if !because.is_empty() {
                out.because.push(because);
            }
        }
    }
    Ok(out)
}

/// 这一项是不是在说「不限」。清空也是纠正：用户上一轮被钉成 Game Boy，
/// 这一轮说「照之前的画」时，模型得有办法把预设摘掉。
fn cleared(value: &Value) -> Option<String> {
    let raw = match value {
        Value::String(s) => s.as_str(),
        Value::Object(map) => map
            .get("id")
            .or_else(|| map.get("style"))
            .and_then(Value::as_str)?,
        _ => return None,
    };
    NONE_WORDS
        .contains(&raw.trim().to_ascii_lowercase().as_str())
        .then(|| raw.to_string())
}

/// 入参是不是在说参照。带 `references` 键、或是一条光秃秃的参照条目
/// （有 `mode`，或按图号索引的对象）都算。其余入参——只说意图或风格的——
/// 与参照无关，转交过去只会得到一句文不对题的报错。
fn talks_about_references(input: &Value) -> bool {
    if input.get("references").is_some() || input.get("mode").is_some() {
        return true;
    }
    match input {
        Value::Object(map) => map.keys().any(|key| key.parse::<usize>().is_ok()),
        _ => false,
    }
}

/// 工具规格。描述里把四件事都说到，模型才知道一个工具能改全部。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: PLAN_TOOL.into(),
        description: "Correct how this turn's request has been read, when the routing written for it is clearly wrong for what the user asked. Call it ONCE, before any drawing tool, and never otherwise. references: style = borrow ONLY that image's palette, ramps, light direction, outline and dithering technique and draw the subject the user described; full = reproduce that image's subject, composition, proportions and palette. intent: what the deliverable is - tilemap, sprite, scene, icon, pattern or prop. style: a locked palette preset - mono1, gameboy, nes, pico8, cga, dither, pastel or hibit; send \"none\" to release a preset. edit: true = this turn edits the pixels already on the canvas (keep the approved silhouette, pose and palette, then add information inside them), false = the user wants a fresh drawing from scratch. Reference numbers are the 1-based positions in the attachment list.".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "references": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "index": {"type": "integer", "description": "1-based position of the reference image in the attachment list"},
                            "mode": {"type": "string", "enum": ["style", "full"]},
                            "because": {"type": "string", "description": "the words in the user's prompt that decided this mode"}
                        },
                        "required": ["index", "mode"]
                    }
                },
                "intent": {
                    "description": "the deliverable type, or the string none to release it",
                    "oneOf": [
                        {"type": "string", "enum": ["tilemap", "sprite", "scene", "icon", "pattern", "prop", "none"]},
                        {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "enum": ["tilemap", "sprite", "scene", "icon", "pattern", "prop", "none"]},
                                "because": {"type": "string"}
                            },
                            "required": ["id"]
                        }
                    ]
                },
                "style": {
                    "description": "a locked palette preset, or the string none to release it",
                    "oneOf": [
                        {"type": "string", "enum": ["mono1", "gameboy", "nes", "pico8", "cga", "dither", "pastel", "hibit", "none"]},
                        {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "enum": ["mono1", "gameboy", "nes", "pico8", "cga", "dither", "pastel", "hibit", "none"]},
                                "because": {"type": "string"}
                            },
                            "required": ["id"]
                        }
                    ]
                },
                "edit": {
                    "description": "whether this turn edits the pixels already on the canvas, or draws from scratch; or the string none to release it",
                    "oneOf": [
                        {"type": "boolean"},
                        {"type": "string", "enum": ["refine", "keep", "edit", "fresh", "redraw", "none"]},
                        {
                            "type": "object",
                            "properties": {
                                "mode": {"type": "string", "enum": ["refine", "edit", "keep", "fresh", "redraw"]},
                                "because": {"type": "string", "description": "the words in the user's prompt that decided this"}
                            },
                            "required": ["mode"]
                        }
                    ]
                }
            }
        }),
    }
}

/// 改画的一轮把知识条目收敛成「refine 打头 + 检索命中的前 3 条」。
/// 三条都要读：refine 讲「怎么保留已有像素」，对象词（cat、tilemap）讲「画的是什么」，
/// 只留 refine 的话「优化一下这只猫」会丢掉猫该长什么样。
/// refine 占首位的理由是它是全库最长的一条，排到后面会被字符预算整段切掉。
fn collapse_to_refine(ids: Vec<String>) -> Vec<String> {
    let mut out = vec!["refine".to_string()];
    for id in ids.into_iter().filter(|id| id != "refine") {
        if out.len() >= 4 {
            break;
        }
        out.push(id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn attachment(role: AttachmentRole) -> Attachment {
        Attachment {
            role,
            media_type: "image/png".into(),
            data_base64: "AAAA".into(),
        }
    }

    #[test]
    fn one_turn_routes_everything_at_once() {
        let attachments = vec![attachment(AttachmentRole::Reference)];
        let plan = TurnPlan::from_text(
            "照这个画风做一个 1-bit 的草地瓦片行走图",
            &attachments,
            true,
        );
        assert_eq!(plan.reference_modes[0], Some(ReferenceMode::Style));
        assert_eq!(plan.intent.map(|i| i.id()), Some("tilemap"));
        assert_eq!(plan.style.map(|s| s.id()), Some("mono1"));
        assert!(plan.knowledge_ids.contains(&"tilemap".to_string()));
        assert!(plan.has_references());
    }

    #[test]
    fn the_routing_section_only_answers_what_was_recognized() {
        let plan = TurnPlan::from_text("画一只猫", &[], true);
        assert!(plan.prompt_sections().is_empty(), "没定出东西就别占标题");
        assert!(plan.intent.is_none());
    }

    #[test]
    fn a_snapshot_only_turn_routes_nothing_about_references() {
        // 「画风」是参照信号、不是风格预设信号，所以这里得说一个真在预设表里的名字。
        let plan = TurnPlan::from_text(
            "按 GameBoy 风格换色",
            &[attachment(AttachmentRole::Snapshot)],
            true,
        );
        assert!(!plan.has_references());
        assert!(plan.prompt_sections().contains("art style"));
        assert!(!plan.prompt_sections().contains("reference modes"));
    }

    #[test]
    fn the_node_shows_the_hit_words() {
        let plan = TurnPlan::from_text("按 GameBoy 风格画个图标", &[], true);
        let input = plan.node_input();
        assert_eq!(input["style"]["id"], "gameboy");
        assert_eq!(input["style"]["because"], "gameboy");
        assert_eq!(input["intent"]["id"], "icon");
    }

    #[test]
    fn a_corrected_routing_reads_back() {
        let updates = parse_updates(&json!({
            "references": [{"index": 1, "mode": "style"}],
            "intent": {"id": "scene", "because": "用户说要背景"},
            "style": "none"
        }))
        .unwrap();
        assert_eq!(updates.references[0].mode, ReferenceMode::Style);
        assert_eq!(updates.intent.unwrap().unwrap().id(), "scene");
        assert_eq!(updates.style, Some(None), "none 表示清掉风格预设");
    }

    #[test]
    fn correcting_one_thing_is_not_read_as_a_reference_entry() {
        // 模型最常做的就是只改一件事：这一轮不是图标是场景。那种入参里没有
        // references 也没有 mode，硬交给参照解析只会撞上「缺 mode」，
        // 真正的错误被盖住，模型照着那句再改一遍还是错。
        let updates = parse_updates(&json!({"intent": "scene"})).unwrap();
        assert!(updates.references.is_empty(), "{updates:?}");
        assert_eq!(updates.intent.unwrap().unwrap().id(), "scene");
        let updates = parse_updates(&json!({"style": "none"})).unwrap();
        assert!(updates.references.is_empty(), "{updates:?}");
        assert_eq!(updates.style, Some(None), "none 表示摘掉风格预设");
        // 按图号索引的裸对象仍然算在说参照。
        let updates = parse_updates(&json!({"1": "full"})).unwrap();
        assert_eq!(updates.references.len(), 1);
        assert_eq!(updates.references[0].mode, ReferenceMode::Full);
    }

    #[test]
    fn correction_errors_name_what_was_wrong() {
        let err = parse_updates(&json!({"intent": "vibe"})).unwrap_err();
        assert!(err.contains("vibe"), "{err}");
        let err =
            parse_updates(&json!({"references": [{"index": 0, "mode": "style"}]})).unwrap_err();
        assert!(err.contains("1-based"), "{err}");
        let err = parse_updates(&json!({"references": [{"index": 1, "mode": "x"}]})).unwrap_err();
        assert!(err.contains('x'), "{err}");
    }

    #[test]
    fn the_outcome_mentions_all_four_kinds() {
        let attachments = vec![attachment(AttachmentRole::Reference)];
        let plan = TurnPlan::from_text("照这个画风做一个 1-bit 行走图", &attachments, true);
        let text = plan.outcome_text();
        assert!(text.contains("reference image 1 = style"), "{text}");
        assert!(text.contains("art style: mono1"), "{text}");
        assert!(text.contains("knowledge notes:"), "{text}");
    }

    #[test]
    fn an_edit_request_carries_the_keep_rule() {
        let plan = TurnPlan::from_text("优化一下细节", &[], true);
        assert!(plan.editing);
        assert_eq!(plan.editing_hit.as_deref(), Some("优化"));
        let sections = plan.prompt_sections();
        assert!(
            sections.contains("EDIT what is already on the canvas"),
            "{sections}"
        );
        assert!(
            sections.contains("Never open the script with clear()"),
            "改画不许以清屏开局，这条是保命用的"
        );
        // refine 被硬塞进知识条目，并且占首位——它最长，靠后会被字符预算整段切掉。
        assert_eq!(
            plan.knowledge_ids.first().map(String::as_str),
            Some("refine")
        );
        let input = plan.node_input();
        assert_eq!(input["edit"]["mode"], "refine");
        assert_eq!(input["edit"]["because"], "优化");
        assert!(plan
            .outcome_text()
            .contains("operation: edit existing pixels"));
    }

    #[test]
    fn a_redraw_request_is_not_an_edit() {
        // 「重画」要的就是覆盖，收进改画轴正好和愿望相反。
        let plan = TurnPlan::from_text("重新画一只猫", &[], true);
        assert!(!plan.editing);
        assert!(plan.editing_hit.is_none());
        assert!(!plan.prompt_sections().contains("EDIT what is already"));
    }

    #[test]
    fn an_empty_canvas_never_routes_as_an_edit() {
        // 「画只猫，加个项圈」词面上是改，可空画布上没东西可改。
        // 少了这个门，提示词会带着「不许从头画」的禁令上路，而禁令的对象不存在。
        let plan = TurnPlan::from_text("画一只橘猫，加一个项圈", &[], false);
        assert!(
            !plan.editing,
            "空画布上不该出现改画轴：{}",
            plan.prompt_sections()
        );
    }

    #[test]
    fn an_edit_correction_reads_back() {
        let updates = parse_updates(&json!({"edit": true, "because": "用户说接着改"})).unwrap();
        assert_eq!(updates.edit, Some(true));
        assert_eq!(updates.because, vec!["用户说接着改".to_string()]);
        let updates = parse_updates(&json!({"edit": "fresh"})).unwrap();
        assert_eq!(updates.edit, Some(false));
        let updates = parse_updates(&json!({"edit": {"mode": "refine"}})).unwrap();
        assert_eq!(updates.edit, Some(true));
        let err = parse_updates(&json!({"edit": "kinda"})).unwrap_err();
        assert!(err.contains("kinda"), "{err}");
    }
}
