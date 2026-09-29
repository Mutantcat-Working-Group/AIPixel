//! 风格预设：一句「按 Game Boy 那样画」，其实是把色数和技法一起钉死了。
//!
//! 这类约束看着像口味，其实是硬指标：1-bit 只剩一个色加透明，Game Boy 只剩四级绿，
//! PICO-8 只剩十六色。不提前说清楚，模型会用自己那套 3-5 级色阶去画，
//! 于是「复古掌机味」变成「现代渐变小图」——用户一眼就能看出不对，却说不出哪里不对。
//!
//! 触发词刻意收得保守：只有把风格名字说出口才算。像「抖动」这种词既是风格也是
//! 通用技法，一句话里出现不等于要把整轮锁成抖动风格，所以只认「抖动递色」这类
//! 完整说法。宁可漏判，不可误判——误判是把整张图的配色改掉。

use serde_json::Value;

/// 风格预设。色数上限和技法是两条腿：只报色数，模型照样会画渐变。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtStyle {
    /// 1-bit：一个色 + 透明。
    Mono1,
    /// 四级绿的掌机味。
    GameBoy,
    /// 红白机限定色板。
    Nes,
    /// PICO-8 十六色。
    Pico8,
    /// DOS / CGA 时代的四色。
    Cga,
    /// 抖动递色：两级色阶加图案过渡，不引中间色。
    Dither,
    /// 粉彩奶油味。
    Pastel,
    /// 高位色：连续色调，允许细过渡。
    HiBit,
}

impl ArtStyle {
    pub fn id(self) -> &'static str {
        match self {
            ArtStyle::Mono1 => "mono1",
            ArtStyle::GameBoy => "gameboy",
            ArtStyle::Nes => "nes",
            ArtStyle::Pico8 => "pico8",
            ArtStyle::Cga => "cga",
            ArtStyle::Dither => "dither",
            ArtStyle::Pastel => "pastel",
            ArtStyle::HiBit => "hibit",
        }
    }

    /// 别名防手滑。认不出的由调用方报错，绝不静默退回「不限风格」。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "mono1" | "1bit" | "1-bit" | "monochrome" | "mono" | "black and white" => {
                Some(ArtStyle::Mono1)
            }
            "gameboy" | "game boy" | "gb" | "dmg" => Some(ArtStyle::GameBoy),
            "nes" | "famicom" | "fc" => Some(ArtStyle::Nes),
            "pico8" | "pico-8" | "pico 8" => Some(ArtStyle::Pico8),
            "cga" | "ega" | "dos" => Some(ArtStyle::Cga),
            "dither" | "dithered" | "ordered dither" | "bayer" => Some(ArtStyle::Dither),
            "pastel" | "cream" | "soft colors" => Some(ArtStyle::Pastel),
            "hibit" | "hi-bit" | "high color" | "highcolour" => Some(ArtStyle::HiBit),
            _ => None,
        }
    }

    /// 这一类风格对模型的硬约束。写进 TURN ROUTING 段和工具结果，措辞两处一致。
    pub fn rules(self) -> &'static str {
        match self {
            ArtStyle::Mono1 => "ONE colour plus transparency: build form with silhouette and dither DENSITY (sparse to solid), introduce no second hue, no anti-aliasing and no outline in a lighter grey; a mid grey count as a second colour and breaks the preset",
            ArtStyle::GameBoy => "a four-step green ramp from darkest to lightest plus nothing else: use those four steps for shadow, base, light and highlight, keep every material on the same four steps, no hue shift, no extra accent colour",
            ArtStyle::Nes => "the NES hardware palette only - saturated primaries plus a few neutrals, no gradients inside a colour, hard pixel edges and no smooth blends; pick indices from the console set and stay inside them",
            ArtStyle::Pico8 => "the PICO-8 sixteen-colour palette and nothing outside it: that palette already ships its own ramps, so reuse its steps instead of generating new ones, and keep outlines to one of its darks",
            ArtStyle::Cga => "four flat colours, cyan-magenta-white-black or a two-hue DOS set: large untextured areas, hard 1px edges, no dithering inside large shapes and no anti-aliasing",
            ArtStyle::Dither => "two or three steps per ramp and NO in-between colour: make every tonal transition from an ordered pattern (Bayer 4x4, or a checkerboard at small scales) between adjacent steps, keep the pattern regular, and never dither across an outline",
            ArtStyle::Pastel => "high value, low saturation, warm-cool separation: lift every shadow off black into a tinted dark, cap saturation well below full, let highlights read cream rather than white, no true black and no hot accent anywhere - flat and gentle beats dramatic",
            ArtStyle::HiBit => "continuous tone is allowed: blend with mix()/hsv() for soft edges and fine gradients, more colour steps are acceptable, but keep ONE light direction and ramps that still step rather than smear; no second light source and no colour outside the ramps",
        }
    }
}

impl std::fmt::Display for ArtStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

const TABLES: &[(ArtStyle, &[&str])] = &[
    (
        ArtStyle::Mono1,
        &[
            "1位色",
            "1位图",
            "一位色",
            "单色",
            "单色素描",
            "纯黑白",
            "黑白",
            "灰阶画",
            "1-bit",
            "1bit",
            "one bit",
            "monochrome",
            "1-bit art",
        ],
    ),
    (
        ArtStyle::GameBoy,
        &[
            "掌机绿",
            "掌机色",
            "gameboy",
            "game boy",
            "gb风",
            "gb风格",
            "dmg",
        ],
    ),
    (
        ArtStyle::Nes,
        &["红白机", "fc风", "fc风格", "8位机色", "nes", "famicom"],
    ),
    (ArtStyle::Pico8, &["pico-8", "pico8", "pico 8", "pico8风格"]),
    (
        ArtStyle::Cga,
        &[
            "dos色",
            "dos风",
            "dos风格",
            "复古四色",
            "cga",
            "ega",
            "ms-dos",
            "msdos",
        ],
    ),
    (
        ArtStyle::Dither,
        &[
            "抖动递色",
            "递色过渡",
            "网点过渡",
            "有序抖动",
            "图案递色",
            "dithering",
            "dithered",
            "ordered dither",
            "bayer",
            "dither pattern",
        ],
    ),
    (
        ArtStyle::Pastel,
        &[
            "粉彩",
            "奶油色",
            "奶油风",
            "马卡龙",
            "粉嫩",
            "糖果色",
            "pastel",
            "rainbow pastel",
        ],
    ),
    (
        ArtStyle::HiBit,
        &[
            "高位色",
            "高位深",
            "色深高",
            "多色渐变",
            "细腻渐变",
            "水彩感",
            "hi-bit",
            "high color",
            "highcolour",
            "high color depth",
        ],
    ),
];

/// 从用户原话读风格，顺带把命中的说法还回来。最早出现的赢，
/// 同分时按上表顺序（越靠前越具体）。一词不命中返回 `None`，风格不限。
pub fn classify_text(text: &str) -> Option<(ArtStyle, String)> {
    let needle = text.to_lowercase();
    let mut best: Option<(usize, ArtStyle, &str)> = None;
    for (style, signals) in TABLES {
        for signal in *signals {
            if let Some(at) = find(&needle, signal) {
                if best.is_none_or(|(where_, _, _)| at < where_) {
                    best = Some((at, *style, signal));
                }
            }
        }
    }
    best.map(|(_, style, word)| (style, word.to_string()))
}

/// 短英文词整词匹配：「nes」藏在大半句英语里，「gb」藏在「rgb」里，
/// 不拦这一下，一句「happiness」就能把整张图改成红白机色板。
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

fn boundary(haystack: &str, at: usize, end: usize) -> bool {
    let before = haystack[..at].chars().next_back();
    let after = haystack[end..].chars().next();
    let loose = |c: char| !c.is_alphanumeric();
    before.is_none_or(loose) && after.is_none_or(loose)
}

/// 工具入参里的风格字段。`{ "style": "pastel" }` 与
/// `{ "style": { "id": "pastel", "because": "..." } }` 都认。
pub fn parse_value(value: &Value) -> Result<(ArtStyle, Option<String>), String> {
    let (raw, because) = match value {
        Value::String(s) => (s.as_str(), None),
        Value::Object(map) => {
            let raw = map
                .get("id")
                .or_else(|| map.get("style"))
                .and_then(Value::as_str)
                .ok_or("pixel_plan: a style needs an 'id'")?;
            let because = map
                .get("because")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            (raw, because)
        }
        other => return Err(format!("pixel_plan: cannot read a style from {other}")),
    };
    let style = ArtStyle::parse(raw).ok_or_else(|| {
        format!(
            "pixel_plan: '{raw}' is not a style; use mono1, gameboy, nes, pico8, cga, dither, pastel or hibit"
        )
    })?;
    Ok((style, because))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_named_preset_locks_the_style() {
        assert_eq!(
            classify_text("按gameboy风格画个史莱姆").map(|(s, _)| s.id()),
            Some("gameboy")
        );
        assert_eq!(
            classify_text("1-bit 小猫").map(|(s, _)| s.id()),
            Some("mono1")
        );
        assert_eq!(
            classify_text("用PICO-8的配色画").map(|(s, _)| s.id()),
            Some("pico8")
        );
    }

    #[test]
    fn no_style_named_means_no_style_locked() {
        assert!(classify_text("画一只橘猫").is_none());
        assert!(classify_text("画一只会抖动身体的行走生物").is_none());
    }

    #[test]
    fn short_words_do_not_match_inside_other_words() {
        assert!(classify_text("rgb colours only").is_none());
        assert!(classify_text("thursday dos shine").is_none());
        assert!(classify_text("with happiness").is_none());
    }

    #[test]
    fn nonsense_styles_are_refused_by_name() {
        let err = parse_value(&json!("vaporwave")).unwrap_err();
        assert!(err.contains("vaporwave"), "{err}");
        assert_eq!(parse_value(&json!("hibit")).unwrap().0, ArtStyle::HiBit);
    }

    #[test]
    fn every_rule_mentions_what_the_style_forbids() {
        for style in [
            ArtStyle::Mono1,
            ArtStyle::GameBoy,
            ArtStyle::Nes,
            ArtStyle::Pico8,
            ArtStyle::Cga,
            ArtStyle::Dither,
            ArtStyle::Pastel,
            ArtStyle::HiBit,
        ] {
            // 大小写都算：「NO in-between colour」里的强调大写也是「说了禁什么」。
            let rules = style.rules().to_ascii_lowercase();
            assert!(
                rules.len() > 60,
                "{style} rule too short to constrain anything"
            );
            assert!(
                rules.contains("no ") || rules.contains("nothing") || rules.contains("stay inside"),
                "{style}"
            );
        }
    }
}
