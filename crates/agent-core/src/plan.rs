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

use super::craft;
use super::models::ToolSpec;
use super::presets;
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
/// 界面钉住风格时写进判据位的那几个字。四处渲染共用一份，措辞不会跑。
const PINNED_HIT: &str = "pinned in the composer";

/// 画风联动的收尾预设写进判据位的那几个字。和 `PINNED_HIT` 分开是有必要的：
/// 一个是用户亲手点的，一个是风格自带的行李。写成同一句，模型会以为用户
/// 明确点过它，将来想按自己的判断省掉它时就少了理由。
const COMPANION_HIT: &str = "riding along with the art style";

/// 这一轮的分流结果。跟着 turn 走：开 turn 时按用户原话定一次，
/// 模型调 `pixel_plan` 纠正时就地改写，之后的每一发请求都带着新结论上路。
#[derive(Debug, Clone, Default)]
pub struct TurnPlan {
    /// 与附件清单一一对齐，快照位恒 `None`。
    pub reference_modes: Vec<Option<ReferenceMode>>,
    pub intent: Option<intent::Intent>,
    /// 定意图时命中的那个说法。节点上要显示，用户看一眼就知道判据是什么。
    pub intent_hit: Option<String>,
    /// 用户这句话是不是在要图。成品没定性时它接手兜底：一句「画只猫」
    /// 既没有「瓦片」也没有「行走图」，触发词一个都不中，可它确实在要一张
    /// 成品。没有这个前提，那一轮的分流段只能整段留空。
    pub art_requested: bool,
    pub style: Option<artstyle::ArtStyle>,
    pub style_hit: Option<String>,
    /// 这个风格是用户在界面上钉住的，不是从他这句话里读出来的。两者的区别只在
    /// 措辞：钉住的那一份写成「你自己选的」，模型才知道这句话没说错——它不需要
    /// 因为「用户没提风格却带着一段风格规矩」而判自己判错、去调 pixel_plan 消掉它。
    pub style_pinned: bool,
    /// 这一轮是在已经画好的画布上动刀。与成品意图平级的另一根轴：
    /// 「把这只猫的动作改一下」要的成品仍是 sprite，操作却是改。
    pub editing: bool,
    /// 判「改」时命中的那个说法，和 intent_hit 一个用法。
    pub editing_hit: Option<String>,
    /// 用户在界面上点的内置提示词预设（写实渲染、电影打光…），最多叠
    /// `presets::MAX_STACKED` 个。与画风平级但分管另一半月：画风钉色数和描边，
    /// 预设讲这张图按什么规矩收尾。没有 `classify_text`——只认下拉里那几下
    /// 点击，一句话里出现「电影感」不算。
    ///
    /// 给 Vec 而不是 Option，是因为细节这件事是乘法：「写实渲染」讲整张图按什么
    /// 规矩收尾，「微细结构」讲最后一两个像素点在哪里，「闭塞接触」讲暗部怎么攒
    /// 起来。三条各管一段，合起来才是一张写实的图。只让选一个，用户就得在
    /// 「像照片」和「有细节」之间二选一——而那正是最常被抱怨的地方。
    pub presets: Vec<&'static presets::Preset>,
    /// `presets` 是画风自带的行李，不是用户在界面上点的。两者对提示词的区别只在
    /// 判据位写什么：亲手点的那份写成「你自己选的」，自带的那份写清楚是跟着
    /// 画风来的——模型将来想省掉一条时，省哪条、凭什么省，全看这一句。
    pub presets_auto: bool,
    pub knowledge_ids: Vec<String>,
    /// 本轮用户原话。分流之外还有第二个用途：颜色名表和术语表按它裁剪，
    /// 不然一百多条色名每发请求都全量上路。
    query: String,
}

/// 画风该带上的收尾预设。没有风格时一条都不带：那一轮走默认质量档
/// （`artstyle::DEFAULT_QUALITY_TIER`），两套收尾规矩同时上身，
/// 模型只会在两段打架的要求里猜权重。
///
/// 认不出的 id 直接跳过（`filter_map`）： artstyle 那边的测试已经守住
/// 「每个 id 都在预设表里」，这里再写一遍报错只是重复。
fn companion_presets(style: Option<artstyle::ArtStyle>) -> Vec<&'static presets::Preset> {
    let Some(style) = style else {
        return Vec::new();
    };
    style
        .quality_preset_ids()
        .iter()
        .filter_map(|id| presets::parse(id))
        .collect()
}

/// 生图提示词出门前挂上的收尾规矩。
///
/// 生图模型那头没有我们的沙箱规矩，唯一能约束它的就是这段文字（`artstyle::decorate_image_prompt`
/// 里有同样一段说明）。这里多出来的是预设：这一句在输入区叠了「写实渲染 + 微细结构 +
/// 闭塞接触」时，那三条规矩同样得跟着出门——它们各管一段，少一条用户就看得出成品变了。
///
/// 放在 `plan` 而不是别处，是因为「画风与预设重合时谁顶替谁」这条让位规则只有这一处：
/// 路由段（`TurnPlan::routing_section`）用它决定哪些预设不出现在结论里，这里用它决定
/// 哪些预设不上提示词。两处各写一遍，迟早出现「路由段说不发、生图提示词却发了」的那种
/// 对不上。artstyle 那头刻意不认识预设（见 `quality_preset_ids` 的说明），所以两边的
/// 缝合只能在知道两者的这一层做。
pub fn image_prompt(
    prompt: &str,
    style: Option<artstyle::ArtStyle>,
    presets: &[&'static presets::Preset],
) -> String {
    let stacked: Vec<&&'static presets::Preset> = presets
        .iter()
        .filter(|preset| style.map(|s| s.id()) != Some(preset.id))
        .collect();
    if stacked.is_empty() {
        return artstyle::decorate_image_prompt(prompt, style);
    }
    // 名字列出来、规矩接在后面：名字是给用户复盘「这一句到底带了几条」用的，
    // 规矩给模型。先报名字再接正文，模型才知道哪几段是一套。
    let names = stacked
        .iter()
        .map(|preset| preset.id)
        .collect::<Vec<_>>()
        .join(", ");
    let rules = stacked
        .iter()
        .map(|preset| preset.rules)
        .collect::<Vec<_>>()
        .join(" ");
    let tail = format!(
        "Finish the picture to these rules too, all of them, because each covers a different job: [{names}] {rules}"
    );
    let body = prompt.trim();
    match style {
        // 预设在路上、画风没点名：默认质量档整段不上。这也正是路由段的让位规矩
        // （见 `routing_section` 里那段说明）：预设本身就是质量档，两套收尾要求
        // 同时上身，模型只会在两段打架的话里猜权重。
        None => format!("{body}\n{tail}"),
        Some(style) => format!("{body}\n{}\n{tail}", artstyle::style_suffix(Some(style))),
    }
}

impl TurnPlan {
    /// 按用户原话定这一轮的分流。`text` 同时用于参照定性、意图、风格和知识检索。
    /// `canvas_has_pixels` 为假时改画轴整条不发：空画布上「改」没有落点，
    /// 「画只猫，加个项圈」词面上是改、实际得从零画，那时候带着
    /// 「不许 clear()、不许重画」的禁令上路，只会让模型对着一张空网格犯难。
    pub fn from_text(text: &str, attachments: &[Attachment], canvas_has_pixels: bool) -> Self {
        Self::from_text_with_style(text, attachments, canvas_has_pixels, None)
    }

    /// 带界面预设的分流入口。`preset` 是用户在输入区点的内置提示词预设。
    /// 与画风那一层完全平行：两者都压过原话判定，一个管配色硬度，一个管收尾规矩。
    pub fn from_text_pinned(
        text: &str,
        attachments: &[Attachment],
        canvas_has_pixels: bool,
        pinned: Option<artstyle::ArtStyle>,
        presets: Vec<&'static presets::Preset>,
    ) -> Self {
        let mut plan = Self::from_text_with_style(text, attachments, canvas_has_pixels, pinned);
        // 界面点了预设才整份换掉：用户在收尾下拉里做过选择的时候，那个选择就是
        // 这一轮的全部意愿，不该再被画风自带的行李掺一手。
        // 选「不限」（空）时保留画风自带的那些——那是风格的一部分，不是推荐位，
        // 用户把推荐位清空不等于要把「写实」这两个字换来的细节规矩一起扔掉。
        if !presets.is_empty() {
            plan.presets = presets;
            plan.presets_auto = false;
        }
        plan
    }

    /// 带界面钉住风格的分流入口。`pinned` 不为空时它压过用户原话里的判定：
    /// 那一下点击是一个明确的意愿，比一句话里偶然出现的风格词更该听。
    /// 其余轴（成品、改画、参照、知识）照旧按原话走，一点不受影响。
    pub fn from_text_with_style(
        text: &str,
        attachments: &[Attachment],
        canvas_has_pixels: bool,
        pinned: Option<artstyle::ArtStyle>,
    ) -> Self {
        let query = text.to_string();
        let base_mode = references::classify_text(text).0;
        let (intent, intent_hit) = match intent::classify_text(text) {
            Some((intent, hit)) => (Some(intent), Some(hit)),
            None => (None, None),
        };
        let (style, style_hit, style_pinned) = match pinned {
            // 钉住的风格没有「用户原话里的那个说法」，判据写给界面：节点上显示
            // 出来用户才知道这一条是从哪儿来的。
            Some(style) => (Some(style), Some(PINNED_HIT.to_string()), true),
            None => match artstyle::classify_text(text) {
                Some((style, hit)) => (Some(style), Some(hit), false),
                None => (None, None, false),
            },
        };
        let (editing, editing_hit) = if canvas_has_pixels {
            match intent::classify_edit(text) {
                Some(hit) => (true, Some(hit)),
                None => (false, None),
            }
        } else {
            (false, None)
        };
        let art_requested = intent::asks_for_artwork(text);
        let mut knowledge_ids = knowledge::matched_ids(text, knowledge::DEFAULT_LIMIT);
        // 风格联动：质量族风格把它配套的知识条目插到队首。用户一句「厚涂方式画个
        // 罐子」只说了风格没说技法，纯靠触发词检索会一条知识都不带，模型手里
        // 只剩一段干规矩——那正是「说了风格还是画得平」的来路。
        // 去重后 truncate 回 DEFAULT_LIMIT：插队可以，加预算不行。
        let linked = style
            .into_iter()
            .flat_map(|s| s.quality_knowledge_ids().iter().copied())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !linked.is_empty() {
            let mut merged = linked;
            for id in knowledge_ids {
                if !merged.contains(&id) {
                    merged.push(id);
                }
            }
            merged.truncate(knowledge::DEFAULT_LIMIT);
            knowledge_ids = merged;
        }
        if editing {
            knowledge_ids = collapse_to_refine(knowledge_ids);
        }
        // 画风自带的收尾预设。界面上点过的预设由 `from_text_pinned` 整份顶掉，
        // 所以这里只按画风算——那是风格的一部分，不是又一层推荐。
        let companions = companion_presets(style);
        let presets_are_auto = !companions.is_empty();
        TurnPlan {
            reference_modes: attachments
                .iter()
                .map(|item| (item.role == AttachmentRole::Reference).then_some(base_mode))
                .collect(),
            intent,
            intent_hit,
            art_requested,
            style,
            style_hit,
            style_pinned,
            // 画风自带的收尾预设。「画得写实一点」这句话里，风格规矩说了色数、
            // 材质怎么分光、接触阴影怎么落，唯独不说「最后一两个像素放在哪里」。
            // 过去那句话只换来一段风格规矩，模型照旧把该放细节的地方留白。
            // 配套的那几条各管一段，在这里一起上路——用户在界面上点过预设时，
            // 由 `from_text_pinned` 整份换掉，走不到这一行。
            presets: companions,
            presets_auto: presets_are_auto,
            editing,
            editing_hit,
            knowledge_ids,
            query,
        }
    }

    /// 本轮用户原话。供颜色名表 / 术语表按需裁剪，见 `prompt::PromptExtras::query`。
    pub fn prompt_query(&self) -> &str {
        &self.query
    }

    /// 这一轮会不会往画布上动笔。判据和 `craft::required` 同源：要图、改画、
    /// 成品意图已定都算，纯问答不算——护栏跟着动笔走，不跟着触发词走。
    /// 用在行为准则三条的下发上，见 `knowledge::discipline_section`。
    pub fn draws(&self) -> bool {
        craft::required(self.editing, self.intent, self.art_requested)
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
        if !self.presets.is_empty() {
            // 一条一行给数组而不是单个对象：界面那头是复选，钉几条就在节点上
            // 显示几条。形状和模型自己纠正时送过来的保持一致。
            input["presets"] = json!(self
                .presets
                .iter()
                .map(|preset| json!({
                    "id": preset.id,
                    // 亲手点的和画风自带的分两种判据：用户想弄清「这条是谁加的」，
                    // 节点上写得含糊，他就得去猜。
                    "because": if self.presets_auto { COMPANION_HIT } else { PINNED_HIT },
                }))
                .collect::<Vec<_>>());
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
            // 措辞分两种：「你自己钉的」是一条明确意愿，模型不该当误判消掉；
            // 「你这句话说的」才谈得上纠正。合起来写，这条区别就没了。
            let because = if self.style_pinned {
                format!("{PINNED_HIT} - treat it as binding for this turn")
            } else {
                format!(
                    "the prompt says \"{}\"",
                    self.style_hit.as_deref().unwrap_or("")
                )
            };
            out.push_str(&format!(
                "- art style: {} ({}): {}\n",
                style.id(),
                because,
                style.rules()
            ));
        }
        // 内置提示词预设紧挨着画风段：两层是用户在同一排下拉里做的选择，
        // 分开太远会让模型把预设当成一句可有可无的建议。
        //
        // 两件事在这里当场说完，不留给模型猜：一是让位——画风点了名，色数、
        // 描边、抖动那几条硬指标归画风，预设只管辖「怎么收尾」；二是顶替——
        // id 重合时（画风里选了写实、预设里又点写实渲染）画风段已经在路上，
        // 再发一遍纯属重复预算。
        // 与画风 id 重合的那几条先挑掉：画风段已经在路上，再发一遍纯属重复预算，
        // 还会让模型以为这一轮挂了两套规矩。挑完再排让位措辞，单数复数才说得准。
        let stacked: Vec<&presets::Preset> = self
            .presets
            .iter()
            .copied()
            .filter(|preset| self.style.map(|style| style.id()) != Some(preset.id))
            .collect();
        if !stacked.is_empty() {
            let deference = match self.style {
                Some(style) => format!(
                    " The locked art style above still owns the colour count, the outline \
strategy and the dithering rules - obey those first; {} {} how the drawing is \
finished, not which palette it uses (\"{}\").",
                    if stacked.len() == 1 {
                        "this preset"
                    } else {
                        "these presets"
                    },
                    // 动词跟着主语走：单复数只换一半，读出来就是一句错话，
                    // 而提示词里的每个错话都在消耗模型对整套规矩的信任。
                    if stacked.len() == 1 {
                        "governs"
                    } else {
                        "govern"
                    },
                    style.id()
                ),
                None => String::new(),
            };
            for preset in stacked {
                out.push_str(&format!(
                    "- finish preset: {} ({}): {}{}\n",
                    preset.id,
                    if self.presets_auto {
                        COMPANION_HIT
                    } else {
                        PINNED_HIT
                    },
                    preset.rules,
                    deference
                ));
            }
        }
        // 用户确实在要图、而这句话一个风格都没点名时，成品兜底（intent 也没中
        // 才需要）和默认质量档一起上路。条件里刻意没有「intent 也没中」：
        // 说过成品词的轮次一样拿不到风格约束，而它们才是最常走的一批——
        // 「行走图」「瓦片地图」过去有成品类别、没有任何质量要求，模型交出来
        // 的就是两坨色加两个眼睛。风格一旦有名（原话说的或界面钉的），
        // 风格规矩整段顶替默认档，两套绝不同时上身。
        //
        // 改画的轮次同样要质量档。「优化一下细节」这种话既不含成品触发词也不含
        // 风格词，过去它只拿到「不许 clear()」一条禁令：模型知道别重画，却不知
        // 道该往哪边改，于是把暗部糊成一团就交差——用户说的「越改越糙」就是这么
        // 来的。反过来，成品兜底绝不能在改画轮次上路：它写着「居中、铺满画布」，
        // 而改画的元规则是保留已有像素，两段同时出现就是对着干。
        // 预设本身就是质量档：点了写实渲染还再挂一份默认档，两套收尾规矩当场打架。
        // 叠几条都一样：只要有一条预设在路上，默认质量档整段不上——两套收尾规矩
        // 同时上身，模型只会在两段打架的要求里猜权重。
        if self.style.is_none() && self.presets.is_empty() && (self.art_requested || self.editing) {
            if self.intent.is_none() && !self.editing {
                out.push_str(
                "- deliverable: ONE finished artwork on a transparent background (the prompt named no category). \
Silhouette first, then exactly one light direction, then shading inside that silhouette. \
Unless the user asked otherwise, centre the subject and size it to fill the canvas, keep the whole \
silhouette on-canvas, and no scene, no frame and no props around it. Stop when the shape reads at \
1x: do not scatter extra detail, and leave no stray pixels outside the subject.\n",
            );
            }
            // 默认质量档：这一句没点名风格，就照「一张画画完是什么意思」收尾。
            // 它是用户最常走的一条路，也是过去唯一一条不带任何渲染规矩的路。
            out.push_str(&format!(
                "- default quality tier: {}\n",
                artstyle::DEFAULT_QUALITY_TIER
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
            (Some(style), Some(hit)) if self.style_pinned => out.push_str(&format!(
                "- art style: {} (pinned, so it holds even though this sentence never said it)\n",
                style.id()
            )),
            (Some(style), Some(hit)) => out.push_str(&format!(
                "- art style: {} (the prompt says \"{hit}\")\n",
                style.id()
            )),
            (Some(style), None) => out.push_str(&format!("- art style: {}\n", style.id())),
            _ => {}
        }
        for preset in &self.presets {
            // 顶替的那几个不出现在结论里：画风段已经写了同一条，再说一遍
            // 只会让用户以为挂了两套规矩。
            if self.style.map(|style| style.id()) != Some(preset.id) {
                out.push_str(&format!("- finish preset: {}\n", preset.id));
            }
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
        description: "Correct how this turn's request has been read, when the routing written for it is clearly wrong for what the user asked. Call it ONCE, before any drawing tool, and never otherwise. references: style = borrow ONLY that image's palette, ramps, light direction, outline and dithering technique and draw the subject the user described; full = reproduce that image's subject, composition, proportions and palette. intent: what the deliverable is - tilemap, sprite, scene, icon, pattern or prop. style: a locked style preset - mono1, gameboy, nes, pico8, cga, dither, pastel, hibit, realistic, fine, painterly, cel, noir or neon; send \"none\" to release a preset. edit: true = this turn edits the pixels already on the canvas (keep the approved silhouette, pose and palette, then add information inside them), false = the user wants a fresh drawing from scratch. Reference numbers are the 1-based positions in the attachment list.".into(),
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
                        {"type": "string", "enum": ["mono1", "gameboy", "nes", "pico8", "cga", "dither", "pastel", "hibit", "realistic", "none"]},
                        {
                            "type": "object",
                            "properties": {
                                "id": {"type": "string", "enum": ["mono1", "gameboy", "nes", "pico8", "cga", "dither", "pastel", "hibit", "realistic", "none"]},
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
    fn a_plain_chat_line_still_routes_nothing() {
        let plan = TurnPlan::from_text("画一只猫", &[], true);
        assert!(plan.intent.is_none());
        // 要图就算动笔：行为准则三条跟着它走，见 `draws`。
        assert!(plan.draws());
        // 「画一只猫」一个类别词都没中，可它确实在要一张成品：成品段由
        // 兜底接手，不再是空段。这正是「行走图」没进触发词之前那批请求
        // 的真实处境——不补这一段，模型拿不到任何成品约束。
        assert!(plan.prompt_sections().contains("ONE finished artwork"));

        let plan = TurnPlan::from_text("这段配色什么意思", &[], true);
        assert!(!plan.art_requested);
        assert!(!plan.draws(), "纯问答轮次不该带上三轮护栏");
        assert!(plan.prompt_sections().is_empty(), "没定出东西就别占标题");
    }

    /// 护栏跟着动笔走，不跟着触发词走。「别画成程序化假图案」这种话没人会
    /// 主动说，等触发词命中就等于永远不发——所以改画那一轮也照样下发。
    #[test]
    fn guardrails_follow_the_brush_not_the_trigger_words() {
        assert!(
            TurnPlan::from_text("优化一下细节", &[], true).draws(),
            "改画也算动笔：规格锁和收工自检对改画同样要紧"
        );
        assert!(
            TurnPlan::from_text("画5帧橘猫奔跑", &[], false).draws(),
            "要图那一轮更该带上规格锁"
        );
    }

    #[test]
    fn an_animation_line_routes_as_a_sprite_cycle() {
        let plan = TurnPlan::from_text("绘制一个五帧的橘猫行走图", &[], false);
        assert_eq!(plan.intent.map(|i| i.id()), Some("sprite"));
        let sections = plan.prompt_sections();
        assert!(sections.contains("TURN ROUTING"), "{sections}");
        assert!(sections.contains("deliverable"), "{sections}");
    }

    #[test]
    fn the_fallback_deliverable_carries_the_craft_basics() {
        let sections = TurnPlan::from_text("画一只猫", &[], true).prompt_sections();
        for word in [
            "ONE finished artwork",
            "transparent background",
            "Silhouette first",
            "exactly one light direction",
            "centre the subject",
        ] {
            assert!(sections.contains(word), "成品兜底段少了一条硬规矩：{word}");
        }
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

    /// 改画轮次要拿到质量档，但绝不能顺手拿到成品兜底。
    ///
    /// 「把腿改长一点」既不含成品触发词也不含风格词，过去它只拿到一条
    /// 「不许 clear()」：模型知道别重画，却不知道该往哪边改，新长出来那段腿
    /// 的暗部于是糊成一团。成品兜底那句写着「居中、铺满画布」，和改画的
    /// 保留规则对着干，所以这一段只能给质量档。
    #[test]
    fn an_edit_request_gets_the_quality_tier_but_not_the_centring_order() {
        let sections = TurnPlan::from_text("把腿改得长一点", &[], true).prompt_sections();
        assert!(sections.contains("default quality tier"), "{sections}");
        assert!(
            !sections.contains("centre the subject and size it to fill the canvas"),
            "改画轮次拿到居中铺满的指令，模型会把用户手改过的画面重排一遍：{sections}"
        );
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

    #[test]
    fn a_pinned_style_overrides_what_the_sentence_said() {
        // 界面钉了写实，话里偏偏说的是单色：钉住的那份赢，不然用户锁了等于没锁。
        let plan = TurnPlan::from_text_with_style(
            "画一只单色猫",
            &[],
            false,
            Some(artstyle::ArtStyle::Realistic),
        );
        assert_eq!(plan.style, Some(artstyle::ArtStyle::Realistic));
        assert!(plan.style_pinned);
        assert_eq!(plan.style_hit.as_deref(), Some(PINNED_HIT));
        let sections = plan.prompt_sections();
        assert!(
            sections
                .contains("realistic (pinned in the composer - treat it as binding for this turn)"),
            "{sections}"
        );
        assert!(
            plan.outcome_text().contains(
                "realistic (pinned, so it holds even though this sentence never said it)"
            ),
            "{}",
            plan.outcome_text()
        );
    }

    #[test]
    fn an_unpinned_turn_keeps_reading_the_style_out_of_the_sentence() {
        // 没有钉住就照旧从原话里判：别让新参数把老路改坏。
        let plan = TurnPlan::from_text_with_style("画一只写实的猫", &[], false, None);
        assert_eq!(plan.style, Some(artstyle::ArtStyle::Realistic));
        assert!(!plan.style_pinned);
        assert_ne!(plan.style_hit.as_deref(), Some(PINNED_HIT));
    }

    /// 成品词一命中（行走图、瓦片、角色…），默认质量档不能跟着消失。
    ///
    /// 这是最常见的一条路，也是过去最亏的一条路：默认档原本挂在「intent 与
    /// style 都没中」这个几乎没人走的分支里，于是凡是说了成品词的轮次——
    /// 模型拿得到成品约束，拿不到任何质量要求——铺两坨色、点两个眼睛就收工，
    /// 用户看到的正是「一点都不写实」那批图。
    #[test]
    fn the_default_quality_tier_reaches_an_art_request_that_named_a_deliverable() {
        let sections =
            TurnPlan::from_text("绘制一个五帧的橘猫行走图", &[], false).prompt_sections();
        assert!(sections.contains("sprite"), "{sections}");
        for lever in [
            "default quality tier",
            "contact shadow",
            "aa* family",
            "reads at 1x",
        ] {
            assert!(sections.contains(lever), "默认档漏了 {lever}：{sections}");
        }
    }

    /// 风格档与默认档不能同时上身：两套规矩打架，模型只会在中间猜权重。
    #[test]
    fn a_named_style_replaces_the_default_quality_tier() {
        let sections = TurnPlan::from_text("画得写实一点", &[], true).prompt_sections();
        assert!(sections.contains("art style: realistic"), "{sections}");
        assert!(
            !sections.contains("default quality tier"),
            "风格档和默认档同时上了路：{sections}"
        );
        // 界面上钉住的风格同样压过默认档。
        let pinned =
            TurnPlan::from_text_with_style("画一只猫", &[], true, Some(artstyle::ArtStyle::Cel));
        let pinned_sections = pinned.prompt_sections();
        assert!(
            pinned_sections.contains("art style: cel"),
            "{pinned_sections}"
        );
        assert!(
            !pinned_sections.contains("default quality tier"),
            "{pinned_sections}"
        );
    }

    /// 一句「画得写实一点」同时钉死风格和知识：风格讲怎么收尾，知识讲为什么
    /// 这么收尾。分开检索时，只说到风格没说到技法的句子会一条知识都不带。
    #[test]
    fn a_quality_style_links_its_knowledge_entries() {
        // 空画布上这一句就是新建：四件套一次带齐。
        let plan = TurnPlan::from_text("画得写实一点", &[], false);
        for id in ["realism", "form-shading", "subsurface", "specular"] {
            assert!(
                plan.knowledge_ids.contains(&id.to_string()),
                "写实该连带 {id}，实际带上的是 {:?}",
                plan.knowledge_ids
            );
        }
        // 上限照旧：风格知识是插队进来的，不是给预算加座。
        assert!(
            plan.knowledge_ids.len() <= knowledge::DEFAULT_LIMIT,
            "{:?}",
            plan.knowledge_ids
        );
        // 色数预设的规矩本身自足，不联动通用笔记。
        let gb = TurnPlan::from_text("按gameboy风格画个史莱姆", &[], false);
        assert!(
            !gb.knowledge_ids.iter().any(|id| id == "realism"),
            "{:?}",
            gb.knowledge_ids
        );
        // 改画那一轮的 refine 仍然占着头一条：风格知识是插到它后面，不是把它顶掉。
        let edit = TurnPlan::from_text("把这只猫改得写实一点", &[], true);
        assert_eq!(
            edit.knowledge_ids.first().map(String::as_str),
            Some("refine"),
            "{:?}",
            edit.knowledge_ids
        );
        assert!(
            edit.knowledge_ids.contains(&"realism".to_string()),
            "{:?}",
            edit.knowledge_ids
        );
    }

    // ---------- 内置提示词预设（收尾规矩） ----------

    /// 预设只认两处：界面上那一下点击，和画风自带的行李。一句话里出现「电影感」
    /// 不等于要把整轮锁成电影打光——它可能只是在描述题材。所以预设表没有
    /// classify_text，原话里那些字一个都不碰；能从话里长出来的只有画风，
    /// 以及画风配套的那一两条。
    #[test]
    fn a_preset_is_only_pinned_from_the_composer_or_a_style() {
        let plan = TurnPlan::from_text("画一只有电影感的猫", &[], false);
        assert!(plan.presets.is_empty(), "原话里的说法不该点着预设");
        // 老入口一律不带预设：四处调用点（含测试）不用跟着改。
        let old = TurnPlan::from_text_with_style("画一只电影打光的猫", &[], true, None);
        assert!(old.presets.is_empty());
        assert!(!old.presets_auto);
    }

    /// 画风自带的收尾预设。「画得写实一点」这句话里，风格规矩说了色数、材质分光、
    /// 接触阴影，唯独不说「最后一两个像素放在哪里」——那正是用户盯着图放大看时
    /// 判断「细不细」的东西。配套的微细结构与闭塞接触必须一起上路。
    #[test]
    fn a_quality_style_brings_its_companion_presets() {
        let plan = TurnPlan::from_text("画得写实一点", &[], false);
        assert_eq!(plan.style.map(|s| s.id()), Some("realistic"));
        let ids: Vec<&str> = plan.presets.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["microdetail", "occlusion"], "{ids:?}");
        assert!(plan.presets_auto, "画风自带的要标出来源");
        let sections = plan.prompt_sections();
        assert!(
            sections.contains("finish preset: microdetail"),
            "{sections}"
        );
        assert!(sections.contains("finish preset: occlusion"), "{sections}");
        assert!(
            sections.contains("riding along with the art style"),
            "{sections}"
        );
    }

    /// 界面上点过的预设整份顶掉画风自带的：那是用户亲手做的选择，
    /// 掺一手推荐只会让他以为自己点的没生效。
    #[test]
    fn composer_presets_replace_the_styles_companions() {
        let plan = TurnPlan::from_text_pinned(
            "画得写实一点",
            &[],
            false,
            Some(artstyle::ArtStyle::Realistic),
            vec![presets::parse("polish").unwrap()],
        );
        let ids: Vec<&str> = plan.presets.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["polish"], "{ids:?}");
        assert!(!plan.presets_auto);
    }

    /// 界面选了「不限」（空）时画风自带的照走：那是风格的一部分，
    /// 用户清空推荐位不等于把「写实」换来的细节规矩一起扔掉。
    #[test]
    fn an_empty_composer_choice_keeps_the_companions() {
        let plan = TurnPlan::from_text_pinned(
            "画得写实一点",
            &[],
            false,
            Some(artstyle::ArtStyle::Fine),
            vec![],
        );
        let ids: Vec<&str> = plan.presets.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["microdetail"], "{ids:?}");
        assert!(plan.presets_auto);
    }

    /// 节点入参把来源写清楚：亲手点的和画风自带的不是一回事，用户想弄清
    /// 「这条是谁加的」，判据位写得含糊他就得去猜。
    #[test]
    fn the_node_input_names_the_source_of_every_preset() {
        let auto = TurnPlan::from_text("画得写实一点", &[], false);
        assert_eq!(
            auto.node_input()["presets"][0]["because"],
            json!("riding along with the art style")
        );
        let pinned = TurnPlan::from_text_pinned(
            "画一只猫",
            &[],
            false,
            None,
            vec![presets::parse("soft").unwrap()],
        );
        assert_eq!(
            pinned.node_input()["presets"][0]["because"],
            json!(PINNED_HIT)
        );
    }

    /// 画风自带预设时默认质量档整段不上：两套收尾规矩同时上身，
    /// 模型只会在两段打架的要求里猜权重。
    #[test]
    fn companions_also_replace_the_default_quality_tier() {
        let sections = TurnPlan::from_text("画得写实一点", &[], false).prompt_sections();
        assert!(!sections.contains("default quality tier"), "{sections}");
    }

    /// 色数预设与 Cel 不带配套：它们的规矩本身就是自足的质量约束。
    #[test]
    fn a_self_contained_style_still_gets_the_default_tier() {
        let sections = TurnPlan::from_text("按gameboy风格画个史莱姆", &[], false).prompt_sections();
        // 色数预设的规矩本身就是完整的质量约束：风格段上路，一条预设都不搭。
        assert!(sections.contains("art style: gameboy"), "{sections}");
        assert!(!sections.contains("finish preset:"), "{sections}");
    }

    /// 钉了预设就真的上路：这是这条链路存在的全部理由。用户点了写实渲染，
    /// 规矩正文必须出现在这一轮的提示词里，而且判据写着来自界面——
    /// 不写的话模型会以为这句话说过「写实」，从而判自己判错。
    #[test]
    fn a_pinned_preset_reaches_the_prompt() {
        let plan = TurnPlan::from_text_pinned(
            "画一只猫",
            &[],
            false,
            None,
            vec![presets::parse("realistic").unwrap()],
        );
        let sections = plan.prompt_sections();
        assert!(sections.contains("finish preset: realistic"), "{sections}");
        assert!(sections.contains("form normal dotted with"), "{sections}");
        assert!(sections.contains("pinned in the composer"), "{sections}");
        // 分流节点上也要看得见：结论里同样有一行。
        let outcome = plan.outcome_text();
        assert!(outcome.contains("finish preset: realistic"), "{outcome}");
    }

    /// 预设本身就是质量档：点了写实渲染还再挂一份默认档，两套收尾规矩当场打架。
    /// 过去「不咋写实」这条路上默认档是唯一的质量来源，现在预设接管它。
    #[test]
    fn a_preset_replaces_the_default_quality_tier() {
        let sections = TurnPlan::from_text_pinned(
            "画一只猫",
            &[],
            true,
            None,
            vec![presets::parse("polish").unwrap()],
        )
        .prompt_sections();
        assert!(!sections.contains("default quality tier"), "{sections}");
    }

    /// 画风与预设同时挂着时不打架：色数、描边、抖动听画风的，其余收尾规矩照常上路。
    /// 让位的措辞必须写明这件事，否则模型在两套要求之间只能猜权重。
    #[test]
    fn a_style_and_a_preset_coexist_with_the_style_owning_the_palette() {
        let plan = TurnPlan::from_text_pinned(
            "画一只猫",
            &[],
            true,
            Some(artstyle::ArtStyle::GameBoy),
            vec![presets::parse("realistic").unwrap()],
        );
        let sections = plan.prompt_sections();
        assert!(sections.contains("art style: gameboy"), "{sections}");
        assert!(sections.contains("finish preset: realistic"), "{sections}");
        // 让位段：画风的色数/描边/抖动归它，预设只管怎么收尾。
        assert!(
            sections.contains("still owns the colour count"),
            "没写明画风管配色硬度：{sections}"
        );
        assert!(
            sections.contains("this preset governs how the drawing is finished"),
            "一条预设时让位措辞要用单数：{sections}"
        );
        assert!(!sections.contains("default quality tier"), "{sections}");
    }

    /// id 重合时画风段顶替预设段，不发两遍：选了 Game Boy 又在预设里点一次 Game Boy
    /// （界面 id 重合的那两个），重复发一遍纯属烧预算，还会让模型以为挂了两套。
    #[test]
    fn an_overlapping_id_is_not_sent_twice() {
        let plan = TurnPlan::from_text_pinned(
            "画一只猫",
            &[],
            true,
            Some(artstyle::ArtStyle::Realistic),
            vec![presets::parse("realistic").unwrap()],
        );
        let sections = plan.prompt_sections();
        assert_eq!(
            sections.matches("art style: realistic").count(),
            1,
            "{sections}"
        );
        assert!(!sections.contains("finish preset:"), "{sections}");
        assert_eq!(
            plan.outcome_text().matches("finish preset:").count(),
            0,
            "{}",
            plan.outcome_text()
        );
    }

    /// 节点入参上带 preset 行，界面上才看得见这一轮多了什么。
    #[test]
    fn the_node_input_carries_the_preset_row() {
        let plan = TurnPlan::from_text_pinned(
            "画一只猫",
            &[],
            false,
            None,
            vec![presets::parse("soft").unwrap()],
        );
        let input = plan.node_input();
        assert_eq!(input["presets"][0]["id"], json!("soft"));
        assert_eq!(input["presets"][0]["because"], json!(PINNED_HIT));
        // 没带预设的轮次不摆这一行：空着的行比没有更占地方。
        assert!(TurnPlan::from_text("画一只猫", &[], false)
            .node_input()
            .get("presets")
            .is_none());
    }

    /// 叠加是这条链路新长出来的腿：三条同时上路，三条的规矩正文都得在，
    /// 并且让位措辞跟着改成复数。少任何一条，用户点了就等于没点。
    #[test]
    fn several_pinned_presets_all_reach_the_prompt() {
        let plan = TurnPlan::from_text_pinned(
            "画一只猫",
            &[],
            true,
            // 带一个画风才看得到让位措辞：没画风时那段不上路，复数单数无从验证。
            // 特意挑 HiBit——它不是预设里的 id，不会被重合顶替挑掉。
            Some(artstyle::ArtStyle::HiBit),
            vec![
                presets::parse("realistic").unwrap(),
                presets::parse("microdetail").unwrap(),
                presets::parse("occlusion").unwrap(),
            ],
        );
        let sections = plan.prompt_sections();
        for id in ["realistic", "microdetail", "occlusion"] {
            assert!(
                sections.contains(&format!("finish preset: {id}")),
                "{id} 的规矩没上路：{sections}"
            );
        }
        // 三条以上用复数：措辞单复数不分，模型会以为只剩一条在管事。
        assert!(
            sections.contains("these presets govern how the drawing is finished"),
            "{sections}"
        );
        // 结论段一行一条，用户才数得清自己钉了几条。
        let outcome = plan.outcome_text();
        assert_eq!(outcome.matches("finish preset:").count(), 3, "{outcome}");
        // 默认质量档照旧不上：有一条在路上就接管了。
        assert!(!sections.contains("default quality tier"), "{sections}");
    }

    /// 叠几条都不能把画风的硬指标顶掉：重合的那条被画风段顶替，其余的照常上路。
    #[test]
    fn a_stack_still_defers_to_the_locked_style() {
        let plan = TurnPlan::from_text_pinned(
            "画一只猫",
            &[],
            true,
            Some(artstyle::ArtStyle::Realistic),
            vec![
                presets::parse("realistic").unwrap(),
                presets::parse("microdetail").unwrap(),
            ],
        );
        let sections = plan.prompt_sections();
        // realistic 由画风段顶替，只剩 microdetail 一条 preset 段。
        assert_eq!(sections.matches("finish preset:").count(), 1, "{sections}");
        assert!(
            sections.contains("finish preset: microdetail"),
            "{sections}"
        );
        // 只剩一条，让位措辞回到单数。
        assert!(
            sections.contains("this preset governs how the drawing is finished"),
            "{sections}"
        );
    }

    // ---------- 生图提示词挂收尾规矩 ----------

    /// 什么都没叠的时候，生图提示词照旧只挂风格段或默认档——这一段是老路，
    /// 不能因为新增了预设就换个形状。
    #[test]
    fn an_unstacked_image_prompt_keeps_its_old_shape() {
        let plain = image_prompt("  a frog  ", None, &[]);
        assert!(plain.starts_with("a frog"), "{plain}");
        assert!(plain.contains("default quality tier"), "{plain}");
    }

    /// 输入区叠的收尾预设必须跟着生图提示词出门。
    ///
    /// 生图模型那头没有我们的沙箱规矩，一个字都不知道用户在界面上选过什么。
    /// 而用户点「写实渲染 + 微细结构 + 闭塞接触」时，那三条各管一段，少一条
    /// 成品就变了——不挂在这里，那三下点击在生图这条路上等于没点。
    #[test]
    fn stacked_presets_reach_the_image_prompt() {
        let out = image_prompt(
            "a frog",
            None,
            &[
                presets::parse("microdetail").unwrap(),
                presets::parse("occlusion").unwrap(),
            ],
        );
        assert!(out.contains("[microdetail, occlusion]"), "{out}");
        assert!(
            out.contains("fur direction in 1px strokes"),
            "microdetail 的规矩正文要带上：{out}"
        );
        assert!(
            out.contains("ONE dedicated occlusion pass"),
            "occlusion 的规矩正文要带上：{out}"
        );
    }

    /// 与画风重合的那条只发一遍：同一套要求出现两遍，模型会自己猜哪遍算数。
    /// 路由段是同一条让位规则，两边必须一致。
    #[test]
    fn a_preset_repeating_the_style_is_dropped_from_the_image_prompt() {
        let out = image_prompt(
            "a frog",
            Some(artstyle::ArtStyle::Realistic),
            &[
                presets::parse("realistic").unwrap(),
                presets::parse("microdetail").unwrap(),
            ],
        );
        assert!(
            out.contains("art style: realistic") || out.contains("art style"),
            "{out}"
        );
        assert_eq!(
            out.matches("microdetail").count(),
            1,
            "microdetail 只出现在预设清单里：{out}"
        );
        assert!(
            !out.contains("finish preset: realistic"),
            "重合的那条由画风段顶替：{out}"
        );
    }

    /// 预设在路上时默认质量档整段不上：预设本身就是质量档，两套收尾要求同时上身，
    /// 模型只会在两段打架的话里猜权重。这也和路由段的让位条件一字不差。
    #[test]
    fn the_default_tier_yields_to_stacked_presets() {
        let out = image_prompt("a frog", None, &[presets::parse("polish").unwrap()]);
        assert!(
            !out.contains("default quality tier"),
            "预设在路上就不该再挂默认档：{out}"
        );
        assert!(out.contains("polish"), "{out}");
        // 画风点名时风格段照旧：它管的是另一半（色数、描边），不受预设影响。
        let styled = image_prompt(
            "a frog",
            Some(artstyle::ArtStyle::GameBoy),
            &[presets::parse("polish").unwrap()],
        );
        assert!(styled.contains("four-step green ramp"), "{styled}");
    }
}
