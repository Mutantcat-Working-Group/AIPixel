// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 风格预设：一句「按 Game Boy 那样画」，其实是把色数和技法一起钉死了。
//!
//! 这类约束看着像口味，其实是硬指标：1-bit 只剩一个色加透明，Game Boy 只剩四级绿，
//! PICO-8 只剩十六色。不提前说清楚，模型会用自己那套 3-5 级色阶去画，
//! 于是「复古掌机味」变成「现代渐变小图」——用户一眼就能看出不对，却说不出哪里不对。
//!
//! 触发词刻意收得保守：只有把风格名字说出口才算。像「抖动」这种词既是风格也是
//! 通用技法，一句话里出现不等于要把整轮锁成抖动风格，所以只认「抖动递色」这类
//! 完整说法。宁可漏判，不可误判——误判是把整张图的配色改掉。
//!
//! 后半张表是另一族：用户说「精细一点」时并不想换配色，他想让同一张图多出信息。
//! 这族词（realistic / fine / painterly / cel / noir / neon）锁的都是「怎么收尾」，
//! 所以规则写到最后一道流程上：定型、上色之后还要不要再走一遍细节。它们和色数
//! 预设并不冲突——写实允许 5-7 级色阶，精细只要求在原有色阶里多加结构。
//!
//! 这族风格还多带两件行李：知识条目（`quality_knowledge_ids`）和收尾预设
//! （`quality_preset_ids`）。理由很实际——「画得写实一点」这句话里，
//! 风格规矩说了色数与材质怎么分光，却不说「最后一两个像素放在哪里」。
//! 过去那句话只换来一段风格规矩，模型照旧把该放细节的地方留白，
//! 用户回过头来还是那句「不咋写实」。配套的那几条预设（`presets`）各管一段，
//! 由 `plan` 在分流时一起上路，用户在界面上看得见。
//!
//! 质量词最容易被一句话反着说：「不要太多细节」「少点特效」。所以命中前先过一道
//! 否定闸门：触发词前面贴着「不要」「少点」「no 」这类说法时，这一击不算。
//! 反向漏判的代价只是少带一段规矩，反向误判的代价是按用户不要的方向硬画。

use serde_json::Value;

/// 默认质量档：用户这句话一个风格都没点名时，照什么把一张画画完。
///
/// 这是最常见的一条路（「画只猫」「做个行走图」），也正是过去最吃亏的一条路：
/// 其余十三条风格都只在被点名时才上场，于是这条没人管的路反而不带任何渲染规矩，
/// 模型按自己的老习惯铺两坨色、点两个眼睛就收工——用户抱怨「生成得一点不写实」，
/// 十个里有八个是走这条路来的。
///
/// 现在它和命名风格共用一条链路：提示词路由段和生图提示词后缀都念这一份文案，
/// 改只有一个地方要改。措辞刻意写成「一张画画完是什么意思」而不是又一条禁令，
/// 模型对着一串否定词容易把手缩回去，最后什么都没画。
pub const DEFAULT_QUALITY_TIER: &str = "the user named no style, so this is what a finished \
drawing means here: ONE light direction honoured by every object, and when the user named none \
no invented key light either - tops a half step lighter and undersides a half step darker off \
the viewer's angle, with the cast shadow pooled beneath the form; 4-5 hue-shifted ramp steps per \
material at 64px and up, 3 under 32px, the darkest step placed just past the terminator and \
never riding the silhouette edge; one consistent solid outline, one pixel wide, in the darkened \
hue of each shape and never pure black, around every shape; occlusion darkened one step wherever \
two forms meet; half a step of the background colour bounced back under every lifted form; a \
tight 1-2px contact shadow where the subject meets the ground, then a dithered falloff that \
thins to nothing; two or three signature details placed last (a 2x2 pupil with a one-pixel \
highlight, a pink inner ear, a lighter pad, a tapering tail); every curve that meets a \
contrasting background drawn through the aa* family or blend() at 32px and up, never stair-stepped; \
one closing polish pass that adds only what the form already implies, then one self-review pass \
that fixes what the first pass actually missed. Stop the moment the subject reads at 1x - a \
crowded drawing is as wrong as a bare one, and a second redraw is wrong too.";

/// 生图提示词出门前挂上的质量档。`None` 表示这一句没点名风格，走默认档。
///
/// 生成模型那头没有我们的沙箱规矩，唯一能约束它的就是这段文字。不挂的后果是它
/// 按自己的训练分布交差：动漫脸、过曝高光、塑料渐变——栅格化落进画布之后，
/// 再干净的网格也带着一股不属于像素画的味。
/// 风格命中时风格规矩压过默认档：两套同时上身，生图模型只会在两段打架的要求里
/// 猜权重，交给用户的是一张四不像。
/// 风格那一句话：点名了风格就是风格规矩，没点名就是默认质量档。
///
/// 单独拆出来是给「有预设时不上默认档」留的口子（见 `plan::image_prompt`）：
/// 那一支只要风格段不要默认档，而两段的措辞必须和这里一字不差——同一个要求
/// 换个说法，模型会当成两件事。
pub fn style_suffix(style: Option<ArtStyle>) -> String {
    match style {
        Some(style) => format!("Bind the image to this art style: {}", style.rules()),
        None => format!("Draw to this default quality tier: {DEFAULT_QUALITY_TIER}"),
    }
}

pub fn decorate_image_prompt(prompt: &str, style: Option<ArtStyle>) -> String {
    format!("{}\n{}", prompt.trim(), style_suffix(style))
}

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
    /// 写实：高色数、柔边、形体光影，但仍然钉在像素格上。
    Realistic,
    /// 精细：色阶照旧，逼你在收尾时再多放一层真实结构。
    Fine,
    /// 厚涂：不透明色块加宽中间调，笔触留在形里。
    Painterly,
    /// 赛璐璐：两三级硬边色带，形状干净，不糊。
    Cel,
    /// 黑色电影：大对比、暗调子，一只硬光源。
    Noir,
    /// 霓虹：满地暗部，只有会发光的东西亮。
    Neon,
}

/// 全部预设，按「越具体越靠前」排。清单给测试和界面用：
/// 少一个成员进来，两边的覆盖断言就会亮。
pub const ALL: [ArtStyle; 14] = [
    ArtStyle::Mono1,
    ArtStyle::GameBoy,
    ArtStyle::Nes,
    ArtStyle::Pico8,
    ArtStyle::Cga,
    ArtStyle::Dither,
    ArtStyle::Pastel,
    ArtStyle::HiBit,
    ArtStyle::Realistic,
    ArtStyle::Fine,
    ArtStyle::Painterly,
    ArtStyle::Cel,
    ArtStyle::Noir,
    ArtStyle::Neon,
];

/// 「不要 / 少点」这类前缀。质量词被它贴着脸时不算命中——用户这句话正在
/// 把那个方向划掉。判定只认「贴着」：离得远的否定词管不到这个触发词，
/// 「画一只没有条纹的猫，精细一点」里的「没有」是否定条纹，不是否定精细。
const NEGATIONS: &[&str] = &[
    // 中文：把方向划掉的各种讲法。
    "不要",
    "不用",
    "无需",
    "不需要",
    "别太",
    "别",
    "不",
    "少点",
    "少一点",
    "减少",
    "去掉",
    "没有",
    // 拉丁：同一批意思。
    "no ",
    "not ",
    "without ",
    "less ",
    "avoid ",
    "no more ",
    "not more ",
    "no extra ",
    "not too ",
    "too much ",
    "too many ",
];

/// 否定词和触发词之间允许夹一层程度词：「不要太写实」「不需要太多细节」
/// 都要算否定。只收副词性的那几个字，收得越宽越容易把好话判成反话。
const INTENSIFIERS: &str = "太一些很多加那么于得";

/// 命中位置之前那一小段。两种脚本都只往前看 32 字节，够盖住「不需要那么多」。
fn window_before(lowered: &str, at: usize) -> &str {
    let start = at.saturating_sub(32);
    lowered.get(start..at).unwrap_or(&lowered[..at])
}

/// `pre` 是命中位置之前的一小段。先按原样贴脸判，再容忍中间夹一层程度词。
fn negated(pre: &str) -> bool {
    if NEGATIONS.iter().any(|m| pre.ends_with(m)) {
        return true;
    }
    let trimmed = pre.trim_end_matches(|c| INTENSIFIERS.contains(c));
    trimmed != pre && NEGATIONS.iter().any(|m| trimmed.ends_with(m))
}

impl ArtStyle {
    /// 这一条风格该连带把哪些知识条目带上路。
    ///
    /// 用户一句「画得写实一点」同时钉死了风格和知识：风格规矩讲的是「怎么收尾」，
    /// 知识条目讲的是「为什么这么收尾、落到哪个像素决策」。分开检索靠的是触发词
    /// 命中，而「写实」这种两个字既在风格表里也在知识表里——两边都命中固然好，
    /// 一旦某句话只说到风格（「厚涂方式画个罐子」），知识条目就整段消失，
    /// 模型只剩一段干规矩。这里把联动写在 Rust 里，触发词再偏也漏不掉。
    ///
    /// 色数预设（mono1 / gameboy / nes…）不联动：那几条规矩本身就是完整约束，
    /// 再塞几条通用笔记只会挤掉成品意图的预算。
    pub fn quality_knowledge_ids(self) -> &'static [&'static str] {
        match self {
            // 写实四件套：为什么这么画（realism）、怎么算明暗（form-shading）、
            // 薄处透光（subsurface）、高光归谁管（specular）。
            ArtStyle::Realistic => &["realism", "form-shading", "subsurface", "specular"],
            // 精细拼的是密度与分组，附带的都是「别加糊」的规矩。
            ArtStyle::Fine => &["pixel-clusters", "materials", "canvas-scale"],
            ArtStyle::Painterly => &["materials", "ramps", "smooth-curves"],
            ArtStyle::Cel => &["lighting", "materials", "outlines"],
            ArtStyle::Noir => &["lighting", "contrast", "rim-light"],
            ArtStyle::Neon => &["glow", "contrast", "rim-light"],
            ArtStyle::HiBit => &["ramps", "smooth-curves", "color-harmony"],
            ArtStyle::Dither => &["dithering"],
            ArtStyle::Pastel => &["color-harmony"],
            ArtStyle::Mono1
            | ArtStyle::GameBoy
            | ArtStyle::Nes
            | ArtStyle::Pico8
            | ArtStyle::Cga => &[],
        }
    }

    /// 这一条风格该连带上路的收尾预设 id（写实渲染、微细结构、闭塞接触…，
    /// 正文在 `presets`）。返回 id 而不是 `Preset`，是为了不让本模块去依赖
    /// 预设模块：两边的集合由 `plan` 用 `presets::parse` 对上，
    /// 打错一个字的后果由那边的测试兜住。
    ///
    /// 只收各管一段、且不跟风格规矩打架的那几条：
    ///
    /// - 写实：微细结构讲「最后一两个像素放在哪」，闭塞接触讲「暗部怎么攒」。
    ///   风格规矩说了色数、材质分光、接触阴影与反弹光，唯独没说这两件事。
    /// - 精细：风格规矩本身就是「最后一遍只加结构」，配一条微细结构讲往哪儿加，
    ///   刚好补上它没说的那半句。
    /// - 厚涂配材质区分（每种材质自己的 ramp 与受光），高位色 / 黑色电影配
    ///   明度结构（去色之后还读不读得懂），霓虹配色彩层级（谁是那个最艳的点），
    ///   粉彩配柔边平滑，抖动递色配肌理质感（递色图案长在 ramp 里）。
    ///
    /// Cel 和五个色数预设一概留空。这不是偷懒：Cel 明令禁止环境光遮蔽、禁止
    ///   轮廓光、禁止第四级过渡，再塞一条闭塞接触就是让模型同时照看两套相反的
    ///   要求；而 1-bit、Game Boy 那几条规矩本身就是完整约束，塞进来的规矩只会在
    ///   同一个像素上打架。
    pub fn quality_preset_ids(self) -> &'static [&'static str] {
        match self {
            ArtStyle::Realistic => &["microdetail", "occlusion"],
            ArtStyle::Fine => &["microdetail"],
            ArtStyle::Painterly => &["material"],
            // 黑色电影不带明度结构：它的规矩里已经写着「不许出现中间灰，
            // 明度只分亮/半亮/暗/黑」——那就是明度结构本身，再发一遍是重复预算。
            ArtStyle::HiBit => &["value"],
            ArtStyle::Noir => &[],
            ArtStyle::Neon => &["hierarchy"],
            ArtStyle::Pastel => &["soft"],
            ArtStyle::Dither => &["texture"],
            ArtStyle::Cel
            | ArtStyle::Mono1
            | ArtStyle::GameBoy
            | ArtStyle::Nes
            | ArtStyle::Pico8
            | ArtStyle::Cga => &[],
        }
    }

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
            ArtStyle::Realistic => "realistic",
            ArtStyle::Fine => "fine",
            ArtStyle::Painterly => "painterly",
            ArtStyle::Cel => "cel",
            ArtStyle::Noir => "noir",
            ArtStyle::Neon => "neon",
        }
    }

    /// 别名防手滑。认不出的由调用方报错，绝不静默退回「不限风格」。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "mono1" | "1bit" | "1-bit" | "monochrome" | "mono" | "black and white" => {
                Some(ArtStyle::Mono1)
            }
            "sketch" | "pencil" => Some(ArtStyle::Mono1),
            "gameboy" | "game boy" | "gb" | "dmg" => Some(ArtStyle::GameBoy),
            "nes" | "famicom" | "fc" => Some(ArtStyle::Nes),
            "pico8" | "pico-8" | "pico 8" => Some(ArtStyle::Pico8),
            "cga" | "ega" | "dos" => Some(ArtStyle::Cga),
            "dither" | "dithered" | "ordered dither" | "bayer" => Some(ArtStyle::Dither),
            "pastel" | "cream" | "soft colors" => Some(ArtStyle::Pastel),
            "watercolor" | "watercolour" => Some(ArtStyle::Pastel),
            "hibit" | "hi-bit" | "high color" | "highcolour" => Some(ArtStyle::HiBit),
            "realistic" | "photo realistic" | "photoreal" | "photorealistic" | "逼真" | "拟真"
            | "照片级" => Some(ArtStyle::Realistic),
            "fine" | "finely detailed" | "detailed" | "high detail" | "more detail"
            | "detail rich" | "精细" => Some(ArtStyle::Fine),
            "painterly" | "painter" | "impasto" | "brushwork" | "厚涂" | "oil" | "oil painting"
            | "hand painted" => Some(ArtStyle::Painterly),
            "cel" | "cel shaded" | "cel shading" | "toon" | "flat shading" | "赛璐璐"
            | "赛璐珞" | "赛璐璐风" | "anime" | "cartoon" | "chibi" => Some(ArtStyle::Cel),
            "noir" | "film noir" | "high contrast" | "黑色电影" | "黑白电影" => {
                Some(ArtStyle::Noir)
            }
            "neon" | "cyberpunk" | "cyber punk" | "synthwave" | "霓虹" | "赛博朋克" => {
                Some(ArtStyle::Neon)
            }
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
            ArtStyle::Realistic => "colour count and softness are the point, the grid is not negotiable: 5-7 ramp steps per material, hue-shifted both ways, and the value band of every pixel computed from its own form (a normal dotted with the light vector) instead of a preset offset, so the terminator is the sharpest step and never rides the silhouette edge. Every curve that meets a contrasting background goes through an aa* helper or blend() so its partial coverage falls out for free instead of stair-stepping. Each material keeps its own light response - metal takes a 1px specular band, cloth takes none at all, fur takes the widest diffuse - and the drawing closes with a polish pass: a contact shadow where the subject meets the ground, one bounce of the ground colour under every lifted form, and the signature detail (pupil highlight, inner ear, pad, tapering tail) placed last. Nothing leaves the pixel grid, no second light source, no gradient inside the smallest detail, no more than two speculars in the whole canvas, and detail density still scales with the canvas: under 32px a realistic subject means silhouette plus two shades.",
            ArtStyle::Fine => "detail density is the deliverable and the colour count stays exactly where it was: after the silhouette and the shading passes are final, run ONE more pass that adds only real structure - the smallest readable material break, one contact shadow band, one rim light along the lit contour, hair or cloth direction in 1px strokes - and never a new ramp colour to pay for it. Every added pixel belongs to a 2x2 or larger cluster at 32px and up, so nothing floats: no lone pixel away from the cluster around it, no detail crossing the silhouette outline, no ornament the user did not ask for, and no second light source. Where a detail is finer than one pixel can hold, dither between two existing ramp steps instead of inventing a third colour. Stop the moment the subject still reads at 1x: a crowded drawing is as wrong as a bare one.",
            ArtStyle::Painterly => "opaque blocks of colour with visible mid-tones, not line art: 6-9 ramp steps per material, wide soft transitions laid with aapolyfill / aaellipse and mix() at partial coverage, and every block keeps its own value so shapes read as brushwork rather than flat fills. The grid still holds - each block lands on whole pixels and nothing smears past the silhouette. Texture sits inside the form: a few 2x2 strokes along the direction of the surface, never a noise field over the whole canvas, never a second light source. Outline strategy is one choice for the whole drawing - either none at all or one dark value on the outside contour only - and no detail smaller than a 2x2 cluster at 32px and up.",
            ArtStyle::Cel => "clean cel shading, no blur anywhere: exactly two or three value bands per material (base, shadow, highlight) with a HARD edge between them, each shadow drawn as its own filled pass - never dithered, never anti-aliased across the boundary, never a fourth step mixed in between. One light direction only, no core shadow past the terminator, no ambient occlusion beyond a single contact band, and no gradient anywhere. Outlines are optional but must stay one strategy for the whole drawing, and where used, one value only. Feathering a shadow edge, adding a rim light the light source does not justify, or mixing a fifth step is what turns cel into blur.",
            ArtStyle::Noir => "high contrast, deep values, ONE hard light source: 5-6 ramp steps that lean dark rather than saturated, a bright key shape against near-black, and everything outside the key reads as two or three shadow steps with the silhouette doing the work. Nothing may sit mid-grey: values separate into light, half-light, dark and black, and no colour outside the ramp except a single warm accent. Shadows hold their shape - no dithered falloff into black, no second light source, and no detail inside the darkest band. Light it like a photograph: one direction, one hard terminator, and cast shadows long enough to read.",
            ArtStyle::Neon => "dark values everywhere except the light that comes out of the subject: two or three near-black ground steps, two or three material steps, and one or two emissive values that appear only where something glows (a sign, an eye, a screen, magic) - the emissive colour is the brightest thing on the canvas and appears nowhere else. Glow is a rim effect: one bright line along the lit contour plus a half-step halo out to at most two pixels, never a field of blur and never a gradient across the subject. No daylight, no second light source, no white highlight outside the glow, no detail inside the darkest band, and the emissive band stays narrow or the picture turns into lava.",
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
            // 素描只用一个色加透明，跟 1-bit 是同一条约束：明度全交给抖动密度。
            "素描",
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
            // 刻意不收裸「水彩」：裸词会把 HiBit 名下的「水彩感」也一起拽过来，
            // 而那边要的是连续色调，不是清淡。只收完整说法。
            "水彩画",
            "水彩风",
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
    (
        ArtStyle::Realistic,
        &[
            "写实",
            "写实风格",
            "写实风",
            "超写实",
            "逼真",
            "拟真",
            "照片级",
            "真实感",
            "写实感",
            // 「跟照片一样」这类说法整句都不出现「写实」两个字，可它要的就是
            // 写实那一整套规矩（高色数、形体光影、柔边）。漏判等于这句话白说。
            "照片一样",
            "像照片",
            "照片质感",
            "摄影级",
            "写真",
            "相片级",
            "realistic",
            "photoreal",
            "photorealistic",
            "photo realistic",
        ],
    ),
    (
        ArtStyle::Fine,
        &[
            "精细",
            "精细一点",
            "精致",
            "细致",
            "细节丰富",
            "细节饱满",
            "细节",
            "细节多一点",
            "加点细节",
            "多一点细节",
            "细节足",
            "高清",
            "品质高一点",
            "精细度",
            // 「细一点」不像「细节」那么郑重，但说的是同一件事：多放结构。
            "细一点",
            "detailed",
            "finely detailed",
            "high detail",
            "more detail",
            "detail rich",
            "rich detail",
        ],
    ),
    (
        ArtStyle::Painterly,
        &[
            "厚涂",
            "厚涂风",
            "笔触感",
            "油画感",
            "油画",
            "油画风",
            "painterly",
            "impasto",
            "brushwork",
        ],
    ),
    (
        ArtStyle::Cel,
        &[
            "赛璐璐",
            "赛璐珞",
            "赛璐璐风",
            "赛璐璐风格",
            // 动漫 / 卡通 / 二次元是用户嘴里最高频的画风词，规矩是同一个：
            // 干净色带、两到三级明度、形状不糊。刻意不收「线稿」——那句话是在说
            // 「先勾线」，锁成硬边色带会把用户随后上色的路堵死。
            "动漫",
            "动画风",
            "动画风格",
            "二次元",
            "卡通",
            "cel",
            "cel shaded",
            "toon",
            "flat shading",
        ],
    ),
    (
        ArtStyle::Noir,
        &[
            "黑色电影",
            "黑白电影",
            "电影感",
            "大对比",
            "强对比",
            "noir",
            "film noir",
            "high contrast",
        ],
    ),
    (
        ArtStyle::Neon,
        &[
            "霓虹",
            "赛博朋克",
            "赛博风",
            "霓虹灯",
            "neon",
            "cyberpunk",
            "synthwave",
        ],
    ),
];

/// 从用户原话读风格，顺带把命中的说法还回来。最早出现的赢，
/// 同分时按上表顺序（越靠前越具体）。一词不命中返回 `None`，风格不限。
///
/// 否定闸门只对质量词那族要紧：色数预设是专有名词，被「不要」修饰的概率很低，
/// 而「精细一点」前面三个字经常就是「我不需要」。统一过闸门，一处逻辑两族都用。
pub fn classify_text(text: &str) -> Option<(ArtStyle, String)> {
    let needle = text.to_lowercase();
    let mut best: Option<(usize, ArtStyle, &str)> = None;
    for (style, signals) in TABLES {
        for signal in *signals {
            let lowered = signal.to_lowercase();
            if let Some(at) = super::terms::find_lower(&needle, &lowered) {
                if negated(window_before(&needle, at)) {
                    continue;
                }
                if best.is_none_or(|(where_, _, _)| at < where_) {
                    best = Some((at, *style, signal));
                }
            }
        }
    }
    best.map(|(_, style, word)| (style, word.to_string()))
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
            "pixel_plan: '{raw}' is not a style; use mono1, gameboy, nes, pico8, cga, dither, pastel, hibit, realistic, fine, painterly, cel, noir or neon"
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
        for style in ALL {
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

    /// 质量词一族：用户没说配色，说的是「多放点信息」。命中后走的是一套
    /// 收尾规矩而不是色板，所以每种都要能单独被认出来。
    #[test]
    fn quality_words_lock_their_own_finishing_rules() {
        for (text, id) in [
            ("画精细一点", "fine"),
            ("这只猫再细致一些", "fine"),
            ("画得写实一点", "realistic"),
            ("加一点细节", "fine"),
            ("用厚涂的方式画", "painterly"),
            ("赛璐璐风格的猫", "cel"),
            ("黑色电影感", "noir"),
            ("霓虹灯下的街道", "neon"),
        ] {
            assert_eq!(classify_text(text).map(|(s, _)| s.id()), Some(id), "{text}");
        }
    }

    /// 否定闸门：用户正把这个方向划掉，那一击不能算。漏判只是少带一段规矩，
    /// 误判是往提示词里塞一段用户明确不要的东西。
    #[test]
    fn a_negated_quality_word_does_not_lock_anything() {
        assert!(classify_text("不要太写实").is_none());
        assert!(classify_text("我不需要太多细节").is_none());
        assert!(classify_text("少点细节").is_none());
        assert!(classify_text("no more detail please").is_none());
        assert!(classify_text("without painterly texture").is_none());
    }

    /// 新预设的 id 也要能原路解回来：界面下发和 pixel_plan 纠正走的是同一条
    /// parse，少一个别名就是用户选了之后模型那边不认。
    #[test]
    fn new_presets_round_trip_through_parse() {
        for id in ["fine", "painterly", "cel", "noir", "neon"] {
            assert_eq!(ArtStyle::parse(id).map(|s| s.id()), Some(id), "{id}");
        }
    }

    /// 「写实」是用户最常说的质量词。它命中的是一整套渲染规矩（高色数、柔边、
    /// 形体光影），漏判等于这句话白说。
    #[test]
    fn a_realism_request_locks_the_rendering_rules() {
        assert_eq!(
            classify_text("画得写实一点").map(|(s, _)| s.id()),
            Some("realistic")
        );
        assert_eq!(
            classify_text("photorealistic cat").map(|(s, _)| s.id()),
            Some("realistic")
        );
        // 同形词不该命中。
        assert!(classify_text("画一个计时器").is_none());
    }

    /// 规则里点名的每个 aa* 函数，必须在沙箱里真的注册着。
    ///
    /// 两边各守一头：pixel-core 那边逐个验函数注册（shader 测试里的
    /// `every_documented_drawing_helper_is_actually_registered`），这边验
    /// 「提示词只点名真有的」。名字写错（aabezier、aapolyline）不会报编译错，
    /// 只会让模型照着一个不存在的函数白跑一轮——那是拿用户的输出预算交学费。
    /// 新增 aa* 函数时两处都要加，这条测试就是逼你记得同步的那只手。
    #[test]
    fn every_helper_named_in_a_rule_is_a_real_sandbox_function() {
        const SANDBOX_HELPERS: &[&str] = &[
            // 平滑家族与别名。
            "aaline",
            "aaseg",
            "aacurve",
            "aaquad",
            "aacubic",
            "aabez",
            "aapoly",
            "aapath",
            "aapolyfill",
            "aafill",
            "aacircle",
            "aacirc",
            "aaellipse",
            "aarect",
            // 单格柔化与取色。
            "blend",
            "aablend",
            "dither",
            "mix",
            "hsv",
            "hex",
            "pal",
            // 规则偶尔也会点到硬边那几位。
            "pset",
            "pget",
            "line",
            "rect",
            "rectfill",
            "circle",
            "circfill",
            "circlefill",
            "ellipse",
            "ellipfill",
            "ellipsefill",
            "flood",
            "replace",
            "outline",
            "clear",
            "stamp",
        ];
        for style in ALL {
            let rules = style.rules().to_ascii_lowercase();
            // 只查 aa 起头且不止两个字母的词：`aa*` 这种通配写法不算点名。
            for word in rules
                .split(|c: char| !c.is_ascii_lowercase())
                .filter(|w| w.len() > 2 && w.starts_with("aa"))
            {
                assert!(
                    SANDBOX_HELPERS.contains(&word),
                    "{style} 点名了沙箱里没有的函数 `{word}`"
                );
            }
        }
    }

    /// 承诺「柔边」的风格必须点名真柔边的函数：aa* 家族或 blend()。
    ///
    /// 规则末尾不许挂着逗号。这些句子是整段拼进提示词的，一个悬空逗号
    /// 读起来就是「话没说完」，模型很可能顺着往下续一列新要求，把整条
    /// 渲染规矩带偏。
    #[test]
    fn no_rule_ends_on_a_dangling_comma() {
        for style in ALL {
            let rules = style.rules().trim_end();
            assert!(
                !rules.ends_with(','),
                "{} 的规矩末尾挂着个逗号，读着像没写完：{rules}",
                style.id()
            );
        }
    }

    /// 承诺「柔边」的风格必须点名真柔边的函数：aa* 家族或 blend()。
    ///
    /// 用户按「写实」要的是「斜线不碎成台阶、圆不是八边形」。那段规矩要是
    /// 只谈色阶不谈怎么收边，模型照旧 stair-step，用户就回来骂人。
    /// HiBit 走另一条路（靠 mix()/hsv() 出连续色调），所以只要求它点到一个
    /// 真函数，不挑剔是哪个。
    #[test]
    fn styles_that_promise_soft_edges_name_a_soft_helper() {
        let names_a_soft_helper = |rules: &str| {
            rules
                .split(|c: char| !c.is_ascii_lowercase())
                .any(|w| w.starts_with("aa") || w == "blend" || w == "aablend")
        };
        for style in [ArtStyle::Realistic, ArtStyle::Painterly] {
            let rules = style.rules().to_ascii_lowercase();
            assert!(
                names_a_soft_helper(&rules),
                "{style} 承诺了柔边，却没点名 aa* 或 blend：{rules}"
            );
        }
        let hibit = ArtStyle::HiBit.rules().to_ascii_lowercase();
        assert!(
            hibit.contains("mix") || hibit.contains("hsv"),
            "hibit 的连续色调没点名 mix/hsv：{hibit}"
        );
    }

    /// 默认质量档是没人点名风格那一轮的全部指望。每一根杠杆都得点名，
    /// 少一根模型就在那一处偷工：只有色阶没有光源就是灰的一坨，
    /// 有光源没有闭塞与接触阴影就是一张贴纸。
    #[test]
    fn the_default_quality_tier_names_every_lever_it_promises() {
        for lever in [
            "light direction",
            "hue-shifted ramp steps",
            "terminator",
            "occlusion",
            "bounced back",
            "contact shadow",
            "pupil",
            "inner ear",
            "aa* family",
            "polish pass",
            // 收手条件和开始条件一样重要：没有它，模型会把整张画布糊满细节。
            "reads at 1x",
        ] {
            assert!(DEFAULT_QUALITY_TIER.contains(lever), "默认档漏了 {lever}");
        }
        assert!(
            DEFAULT_QUALITY_TIER.contains("64px"),
            "档位没跟画布尺寸挂钩，小图会被按大图的标准涂糊"
        );
    }

    /// 生图提示词出门时质量档必须挂上，而两套档位绝不同时上身。
    /// 谁家常走这条路径：用户开了生图模型、又没提风格——那正是默认档该出场
    /// 的一次，晚了它就只能看着生成模型按训练分布交差。
    #[test]
    fn an_image_prompt_leaves_with_the_quality_bar_attached() {
        let plain = decorate_image_prompt("  a small frog  ", None);
        assert!(
            plain.starts_with("a small frog"),
            "原提示词被改动了：{plain}"
        );
        assert!(
            plain.contains("default quality tier"),
            "没点名风格时默认档没挂上：{plain}"
        );
        let styled = decorate_image_prompt("a small frog", Some(ArtStyle::Realistic));
        assert!(styled.contains("art style"), "风格规矩没挂上：{styled}");
        assert!(
            !styled.contains("default quality tier"),
            "风格档和默认档同时上身，生成模型要在两段打架的要求里猜权重：{styled}"
        );
    }

    /// 风格联动的每个 id 必须在知识库里真实存在。打错一个字的后果是静默的：
    /// `section_from_ids` 认不出的 id 直接跳过，提示词里既没有报错也没有那条笔记，
    /// 只有用户在抱怨「说了写实还是画得平」。
    #[test]
    fn every_linked_knowledge_id_is_a_real_entry() {
        for style in ALL {
            for id in style.quality_knowledge_ids() {
                assert!(
                    super::super::knowledge::ENTRIES
                        .iter()
                        .any(|entry| entry.id == *id),
                    "{} 链了知识库里不存在的条目 `{id}`",
                    style.id()
                );
            }
        }
    }

    /// 质量族风格才联动知识；色数预设不联动——那几条规矩本身就是完整约束。
    #[test]
    fn only_the_quality_family_links_knowledge() {
        assert_eq!(ArtStyle::Realistic.quality_knowledge_ids().len(), 4);
        assert_eq!(ArtStyle::Cel.quality_knowledge_ids().len(), 3);
        assert!(ArtStyle::GameBoy.quality_knowledge_ids().is_empty());
        assert!(ArtStyle::Cga.quality_knowledge_ids().is_empty());
    }

    /// 配套预设的 id 必须在预设表里真实存在。打错一个字的后果是静默的：
    /// `presets::parse` 认不出就跳过，提示词里既没有报错也没有那条规矩，
    /// 只有用户在抱怨「说了写实还是画得平」——和知识联动那一侧是同一个坑。
    #[test]
    fn every_companion_preset_id_is_a_real_preset() {
        for style in ALL {
            for id in style.quality_preset_ids() {
                assert!(
                    super::super::presets::parse(id).is_some(),
                    "{} 链了预设表里不存在的预设 `{id}`",
                    style.id()
                );
            }
        }
    }

    /// 每种风格的配套预设不许超过叠加上限。用户自己还能在界面上点，界面的点击
    /// 优先（见 `plan`），但万一界面没点，这一份就是全部——超了上限 Rust 会拒收，
    /// 用户看到的是一次没头没尾的失败。
    #[test]
    fn companions_stay_inside_the_stack_budget() {
        for style in ALL {
            assert!(
                style.quality_preset_ids().len() <= super::super::presets::MAX_STACKED,
                "{} 的配套预设超过 {} 条",
                style.id(),
                super::super::presets::MAX_STACKED
            );
        }
    }

    /// 规矩本身完整的风格不带配套：Cel 禁止环境光遮蔽和轮廓光，色数预设把
    /// 色板钉死，再塞一条预设只会让模型同时照看两套相反的要求。
    #[test]
    fn self_contained_styles_bring_no_companions() {
        for style in [
            ArtStyle::Cel,
            ArtStyle::Mono1,
            ArtStyle::GameBoy,
            ArtStyle::Nes,
            ArtStyle::Pico8,
            ArtStyle::Cga,
        ] {
            assert!(
                style.quality_preset_ids().is_empty(),
                "{} 的规矩已经是完整约束，不该再带配套预设",
                style.id()
            );
        }
    }

    /// 「写实」是用户最常说要细节的一个词。它必须自带微细结构——那一句管的是
    /// 「最后一两个像素放在哪里」，风格规矩里一个字都没提。
    #[test]
    fn a_realism_request_brings_the_micro_detail_preset() {
        assert!(
            ArtStyle::Realistic
                .quality_preset_ids()
                .contains(&"microdetail"),
            "写实没带微细结构，说了写实还是画得平"
        );
        assert!(
            ArtStyle::Fine.quality_preset_ids().contains(&"microdetail"),
            "精细没带微细结构"
        );
    }

    /// 用户嘴里最高频的几个画风词，一个都不许漏判。它们整句话里没有出现过
    /// 「风格」两个字，可要的就是那一套质量规矩。
    #[test]
    fn everyday_style_words_lock_the_quality_family() {
        for (text, id) in [
            ("动漫风格的小猫", "cel"),
            ("画个卡通人物", "cel"),
            ("二次元头像", "cel"),
            ("水彩画的风景", "pastel"),
            ("油画质感的静物", "painterly"),
            ("素描一只兔子", "mono1"),
            ("画得跟照片一样", "realistic"),
            ("这张照片质感太好了", "realistic"),
            ("再细一点", "fine"),
        ] {
            assert_eq!(classify_text(text).map(|(s, _)| s.id()), Some(id), "{text}");
        }
    }

    /// 「水彩感」是 HiBit 的老词（要连续色调），不能被 Pastel 名下的新词拽走。
    /// 中文按子串算，新词一不留神就会把老词的判据改掉——这类回归只有测试看得见。
    #[test]
    fn a_existing_word_keeps_its_original_style() {
        assert_eq!(
            classify_text("水彩感的风景").map(|(s, _)| s.id()),
            Some("hibit"),
            "watercolor feel 一直是 HiBit 的连续色调"
        );
        // 同分时靠上表顺序兜底，而「写实」这类老词的位置没动过。
        assert_eq!(
            classify_text("赛璐璐风格的猫").map(|(s, _)| s.id()),
            Some("cel")
        );
    }
}
