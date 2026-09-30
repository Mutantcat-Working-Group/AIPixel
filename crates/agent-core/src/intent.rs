// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 用户意图：这一句话要的是哪一类成品。
//!
//! 「画一只猫」和「做一套草地瓦片」要的东西根本不是一回事：前者是单张立绘，
//! 后者是一组能在网格上重复的边角转场。以前只有一套通用 craft 规则，
//! 模型只能自己猜，猜错的方向是整张白画——瓦片被画成一张地图，图标被画成带背景的小场景。
//!
//! 和参照定性一样分两段接力：Rust 先按关键词从用户原话里读一个默认意图，
//! 写进 system prompt 的 TURN ROUTING 段和一条 function_call 节点；
//! 模型读原话觉得判错了，再调 `pixel_plan` 纠正。缺了第一段，
//! 「光思考不调工具」的模型根本不知道这一轮的服务对象是什么。

use serde_json::Value;

/// 成品类型。不细分到题材（那是模型的事），只分到「要怎么组织画面」这一层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// 可平铺的瓦片集：格子边界、边角转场、整套共享一个光向。
    Tilemap,
    /// 角色 / 精灵：剪影先行，逐帧保持辨识度。
    Sprite,
    /// 场景 / 背景：分层、纵深、地平线统一。
    Scene,
    /// 图标 / emblem：小尺寸读形，无边框少颜色。
    Icon,
    /// 图案 / 材质 / 纹理：无缝循环，重复单元小。
    Pattern,
    /// 道具 / 物品：单件、透明底、可辨轮廓。
    Prop,
}

impl Intent {
    pub fn id(self) -> &'static str {
        match self {
            Intent::Tilemap => "tilemap",
            Intent::Sprite => "sprite",
            Intent::Scene => "scene",
            Intent::Icon => "icon",
            Intent::Pattern => "pattern",
            Intent::Prop => "prop",
        }
    }

    /// 从模型入参里认意图。别名防手滑；认不出的由调用方报错，
    /// 绝不静默按「什么都不限定」处理——那等于把意图分流整段删掉。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "tilemap" | "tileset" | "tile" | "tiles" | "map tiles" => Some(Intent::Tilemap),
            "sprite" | "character" | "sprites" => Some(Intent::Sprite),
            "scene" | "background" | "backdrop" | "landscape" => Some(Intent::Scene),
            "icon" | "emblem" | "badge" => Some(Intent::Icon),
            "pattern" | "texture" | "seamless" | "material" => Some(Intent::Pattern),
            "prop" | "item" | "object" | "weapon" => Some(Intent::Prop),
            _ => None,
        }
    }

    /// 这一类成品对模型的硬约束。写进 system prompt 的 TURN ROUTING 段，
    /// 也写进工具结果：两处措辞必须一致，不然模型看到的和回灌的说法会漂移。
    pub fn rules(self) -> &'static str {
        match self {
            Intent::Tilemap => "the deliverable is a SET OF TILES, not one picture: work on one cell grid (8/16/32px), make every tile edge-welding so rows repeat without seams, provide base plus edge and corner variants, and share ONE light direction and ONE ramp per material across the whole set",
            Intent::Sprite => "the deliverable is ONE SUBJECT on a transparent background: silhouette first, then shading; keep proportions generous rather than cramped, keep the signature detail clear of the body outline, and when it animates keep every frame recognizable so the loop wraps",
            Intent::Scene => "the deliverable is a SCENE: split it into far, mid and near planes, keep ONE horizon line and ONE ground angle, push distance with value and saturation instead of adding colors, and give every object the same light direction",
            Intent::Icon => "the deliverable is an ICON read at small size: one clean silhouette, few colors, no scene, no border and no drop shadow unless the user asks; check it at 1x, and keep the shape inside the canvas with one pixel of breathing room",
            Intent::Pattern => "the deliverable is a seamless PATTERN or material: keep the repeat cell small (8-32px), make opposite edges exact complements, and never place a single weighted element dead centre; verify by repeating the cell twice in each axis",
            Intent::Prop => "the deliverable is ONE ITEM on a transparent background: centre it, fill the canvas consistently, shade from one light, and leave the silhouette readable at a glance with no scene context",
        }
    }
}

impl std::fmt::Display for Intent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// 触发词表。顺序即同分时的兜底顺序：越靠前的越「具体」。
/// 中文别称放前面，因为主语言是中文；英文直译和惯用语跟在后面。
const TABLES: &[(Intent, &[&str])] = &[
    (
        Intent::Tilemap,
        &[
            "瓦片",
            "瓷砖",
            "地块",
            "图块",
            "拼贴地",
            "tilemap",
            "tileset",
            "tile set",
            "tiles",
        ],
    ),
    (
        Intent::Sprite,
        &[
            "角色",
            "精灵",
            "人物",
            "立绘",
            "小人",
            "动物",
            "怪物",
            "npc",
            "sprite",
            "character",
            "walk",
            "run cycle",
        ],
    ),
    (
        Intent::Scene,
        &[
            "场景",
            "背景",
            "风景",
            "景色",
            "地图",
            "关卡",
            "界面底",
            "scene",
            "background",
            "backdrop",
            "landscape",
        ],
    ),
    (
        Intent::Icon,
        &["图标", "icon", "头像", "徽章", "标志", "emblem", "badge"],
    ),
    (
        Intent::Pattern,
        &[
            "无缝", "图案", "花纹", "纹样", "材质", "纹理", "平铺", "贴图", "pattern", "texture",
            "seamless", "tileable",
        ],
    ),
    (
        Intent::Prop,
        &[
            // 只收到「成品类别」这一层：「剑」「宝箱」是题材，也可能是在说图标的题材
            // （「一把剑的图标」要的是图标，不是道具），所以具体物件不进这里。
            "道具", "物品", "装备", "武器", "prop", "item", "weapon",
        ],
    ),
];

/// 从用户原话读意图，顺带把命中的词还回来（节点上要显示）。
/// 最早出现的说法赢：一句话同时说到「画个角色」和「做成瓦片」时，
/// 用户心里那一件通常先说出来。一词不命中就返回 `None`，交给通用规则。
pub fn classify_text(text: &str) -> Option<(Intent, String)> {
    let needle = text.to_lowercase();
    let mut best: Option<(usize, Intent, &str)> = None;
    for (intent, signals) in TABLES {
        for signal in *signals {
            if let Some(at) = find(&needle, signal) {
                if best.is_none_or(|(where_, _, _)| at < where_) {
                    best = Some((at, *intent, signal));
                }
            }
        }
    }
    best.map(|(_, intent, word)| (intent, word.to_string()))
}

/// 短英文词必须整词命中。「nos」里藏着「os」、「happiness」里藏着「nes」，
/// 不拦这一下，一句毫不相干的话就能把配色风格整个带偏。
fn find(haystack: &str, signal: &str) -> Option<usize> {
    let whole = signal.len() <= 4 && signal.is_ascii();
    let mut from = 0usize;
    while let Some(off) = haystack[from..].find(signal) {
        let at = from + off;
        let end = at + signal.len();
        if !whole || boundary(haystack, at, end) {
            return Some(at);
        }
        from = end;
    }
    None
}

/// 修改类触发词。与成品类型不是一回事：成品说「要画个什么东西」，
/// 这里说的是「在已经画好的东西上动刀」。
///
/// 为什么单列一轴而不是塞进 `Intent`：用户说「把这只猫的动作改一下」时，
/// 成品仍然是 sprite（说清楚是个照旧），但操作是改。两件事都要告诉模型，
/// 而 `Intent` 只有一个坑位。塞进去就只能二选一——选 sprite，
/// 模型照着「剪影先行」把猫重画一遍，用户的手笔和已确认的姿态全没了；
/// 选 modify，又丢掉「这是个角色」这条约束。
///
/// 刻意不收「重画」「重新画」「重做」：那些话要的就是覆盖，
/// 归到这一轴来正好和愿望相反。
const EDIT_WORDS: &[&str] = &[
    // 中文：改、调、加、换、删都是改，但都要带上下文，光一个字会误伤
    // （「更加」里藏着「加」，「交换」里藏着「换」）。
    "优化",
    "细化",
    "润色",
    "完善",
    "修改",
    "改动",
    "更改",
    "改改",
    "改成",
    "改色",
    "换色",
    "换个颜色",
    "调整",
    "微调",
    "调亮",
    "调暗",
    "加上",
    "添加",
    "加个",
    "加一",
    "加条",
    "添加一个",
    "换成",
    "换个",
    "去掉",
    "删掉",
    // 英文：短词走整词匹配，长词允许子串，和成品那边一个规矩。
    "refine",
    "polish",
    "improve",
    "tweak",
    "adjust",
    "modify",
    "edit",
    "add a",
    "add some",
    "make it",
    "change the",
];

/// 从用户原话读「这一轮是在改已有的画」。命中即返回那个词（界面上当判据念）。
/// 一词不中就返回 `None`——那是从零画，该走成品类型那一轴。
pub fn classify_edit(text: &str) -> Option<String> {
    let needle = text.to_lowercase();
    EDIT_WORDS
        .iter()
        .filter_map(|word| find(&needle, word).map(|at| (at, *word)))
        .min_by_key(|(at, _)| *at)
        .map(|(_, word)| word.to_string())
}

/// 工具入参里的「这一轮改不改已有画面」。布尔、字符串、`{"mode": ...}` 都认，
/// 认不出的值报错——猜反了就是「该改的时候重画、该重画的时候改」。
pub fn parse_edit(value: &serde_json::Value) -> Result<bool, String> {
    use serde_json::Value;
    let raw = match value {
        Value::Bool(b) => return Ok(*b),
        Value::Number(n) => return Ok(n.as_f64().unwrap_or(0.0) != 0.0),
        Value::String(s) => s.as_str(),
        Value::Object(map) => map
            .get("mode")
            .or_else(|| map.get("id"))
            .or_else(|| map.get("edit"))
            .and_then(Value::as_str)
            .ok_or("pixel_plan: an edit flag needs a 'mode'")?,
        other => return Err(format!("pixel_plan: cannot read an edit flag from {other}")),
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" | "refine" | "edit" | "modify" | "keep" => Ok(true),
        "false" | "no" | "off" | "0" | "none" | "null" | "clear" | "release" | "fresh"
        | "redraw" => Ok(false),
        other => Err(format!(
            "pixel_plan: '{other}' does not say whether to edit; use true or false"
        )),
    }
}

fn boundary(haystack: &str, at: usize, end: usize) -> bool {
    let before = haystack[..at].chars().next_back();
    let after = haystack[end..].chars().next();
    let loose = |c: char| !c.is_alphanumeric();
    before.is_none_or(loose) && after.is_none_or(loose)
}

/// 工具入参里的意图字段。`{ "intent": "scene" }` 和
/// `{ "intent": { "id": "scene", "because": "..." } }` 都认，
/// 因为模型两种写法都会用；认不出的值报错，不猜。
pub fn parse_value(value: &Value) -> Result<(Intent, Option<String>), String> {
    let (raw, because) = match value {
        Value::String(s) => (s.as_str(), None),
        Value::Object(map) => {
            let raw = map
                .get("id")
                .or_else(|| map.get("intent"))
                .and_then(Value::as_str)
                .ok_or("pixel_plan: an intent needs an 'id'")?;
            let because = map
                .get("because")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            (raw, because)
        }
        other => return Err(format!("pixel_plan: cannot read an intent from {other}")),
    };
    let intent = Intent::parse(raw).ok_or_else(|| {
        format!("pixel_plan: '{raw}' is not an intent; use tilemap, sprite, scene, icon, pattern or prop")
    })?;
    Ok((intent, because))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chinese_and_english_both_route() {
        assert_eq!(
            classify_text("做一套草地瓦片").map(|(i, _)| i.id()),
            Some("tilemap")
        );
        assert_eq!(
            classify_text("build me a tileset").map(|(i, _)| i.id()),
            Some("tilemap")
        );
        assert_eq!(
            classify_text("画一个游戏图标").map(|(i, _)| i.id()),
            Some("icon")
        );
        assert_eq!(
            classify_text("一把剑的图标").map(|(i, _)| i.id()),
            Some("icon")
        );
    }

    #[test]
    fn nothing_recognizable_stays_empty() {
        assert!(classify_text("把颜色调亮一点").is_none());
    }

    #[test]
    fn the_first_thing_said_wins() {
        // 「画个角色，背景做成瓦片」——说出口的顺序就是心里那一件。
        let (intent, _) = classify_text("画个角色，背景做成瓦片").unwrap();
        assert_eq!(intent, Intent::Sprite);
    }

    #[test]
    fn short_words_do_not_match_inside_other_words() {
        assert!(classify_text("thursday nos paint").is_none());
        assert!(classify_text("full happiness").is_none());
    }

    #[test]
    fn the_hit_word_comes_back_for_the_node() {
        let (_, hit) = classify_text("帮我做个无缝图案").unwrap();
        assert_eq!(hit, "无缝");
    }

    #[test]
    fn nonsense_intents_are_refused_by_name() {
        let err = parse_value(&json!("vibe")).unwrap_err();
        assert!(err.contains("vibe"), "{err}");
        assert_eq!(parse_value(&json!("scene")).unwrap().0, Intent::Scene);
        assert_eq!(
            parse_value(&json!({"id": "prop", "because": "一把剑"}))
                .unwrap()
                .0,
            Intent::Prop
        );
    }
}
