// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 内置提示词预设：用户要的不是一个新画风，是「这一张按什么规矩收尾」。
//!
//! 画风（`artstyle`）管的是色数和描边那类硬指标——钉了 Game Boy 就只有四级绿，
//! 谁来说情都不改。这里管的是另一半：同样四级绿，可以画成一张干瘪的示意图，
//! 也可以画成有形体、有材质、有收尾的一张图。用户抱怨「不咋写实」「细节不够」
//! 说的都是这一半，而过去能拧的螺丝只有画风那一个，于是写实被做成了第十四种画风，
//! 和 1-bit、PICO-8 摆在同一排——用户想要的是「细节」，看到的却是「换套配色」。
//!
//! 所以预设单独一层，规则只讲收尾：形体怎么算明暗、材质怎么分光、收尾加什么、
//! 深度怎么让平面分开、柔边从哪里来、肌理长在 ramp 里还是 ramp 外。它们和画风不冲突：
//! 画风点名时色数/描边/抖动听画风的，其余规矩照常上路（见 `plan` 里的让位措辞），
//! 重合的那两个 id（realistic / fine）直接由画风段顶替，不发两遍。
//!
//! 预设可以叠加（`MAX_STACKED`）：细节这件事本来就是乘法——「写实渲染」讲整张图按什么
//! 规矩收尾，「微细结构」讲最后一两个像素点在哪里，「闭塞接触」讲暗部怎么攒起来。
//! 三者各管一段，合起来才是一张写实的图；只让选一个，用户就得在「像照片」和「有细节」
//! 之间二选一，而那正是过去最常被抱怨的地方。上限三个是有意的：规矩之间会互相稀释，
//! 模型的输出预算也只有一个。
//!
//! 触发只认界面：这里没有 `classify_text`。一句话里出现「电影感」不等于要把整轮
//! 锁成电影打光——它可能只是在描述题材。宁可只在下拉里点一下，不可凭一个字
//! 就把一整套渲染规矩按在这一轮上。

/// 一次最多同时生效几个预设。三个已经覆盖「形体 + 材质 + 收尾」，再多不是更细，
/// 是让模型同时照看七八套彼此重叠的约束——最后哪一套都没照看到。
/// 界面（`src/lib/presets.ts` 的 `MAX_STACKED_PRESETS`）和后端入参都按这个数收口。
pub const MAX_STACKED: usize = 3;

/// 一个内置提示词预设。`rules` 是直接下发给模型的渲染规矩，英文与其余提示词一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// 稳定标识。界面、节点入参、前后端比对都念它。
    pub id: &'static str,
    /// 中文界面名。
    pub label_zh: &'static str,
    /// 英文界面名。
    pub label_en: &'static str,
    /// 下发给模型的规矩正文。讲「怎么收尾」，不重复画风的色数/描边/抖动条款。
    pub rules: &'static str,
}

/// 全部内置预设。清单给测试和界面用：少一个成员进来，覆盖断言就会红。
///
/// 分三组读：先六条讲「这张图按什么规矩收尾」（写实、打光、材质、精修、景深、柔边），
/// 再四条讲「画面站得住的那半边」（肌理、体积、剪影、做旧），最后六条专门伺候细节——
/// 微细结构、闭塞接触、明度结构、色彩层级、反射透光、构图焦点。前两组决定「像不像
/// 一张画完的画」，最后六组决定「细节到底从哪来」：用户说「不咋写实」「细节不够」
/// 时，真正缺的往往就是这六条里的某两条。
pub const ALL: [Preset; 16] = [
    Preset {
        id: "realistic",
        label_zh: "写实渲染",
        label_en: "Realistic render",
        rules: "Finish the picture the way a finished picture behaves: 5-7 hue-shifted ramp steps \
per material (3 under 32px), every pixel's value computed from its own form normal dotted with \
the ONE light direction instead of a preset offset, so the terminator is the sharpest step and \
sits just past where the form turns away from the light. Give each material its own light \
response - metal a 1px specular band, fur the widest diffuse with a warm glow where the form is \
thin, cloth no specular at all and dithered deepest creases, stone matte with every pit occluded \
one step. Close with the three things that sell realism: a tight 1-2px contact shadow where the \
subject meets the ground, one half-step of the background colour bounced back under every lifted \
form, and the signature detail (a 2x2 pupil with a one-pixel highlight, a pink inner ear, a \
lighter pad, a tapering tail) placed last. Every curve that meets a contrasting background goes \
through an aa* helper or blend() so its partial coverage falls out for free; nothing leaves the \
pixel grid.",
    },
    Preset {
        id: "cinematic",
        label_zh: "电影打光",
        label_en: "Cinematic light",
        rules: "Light the frame like a shot, not like a diagram: ONE hard key light with a \
direction the user can name, a fill at roughly a quarter of the key's value on the opposite \
side, and a rim light that traces the lit edge of the silhouette against the darker background. \
The rim is the one place where cinematic lighting meets the pixel grid: lay it with an aa* helper \
or blend() so its partial coverage falls out for free, and keep the hard edge where the rim \
itself is the subject. \
Push the contrast until the middle ramp step is the scarcest one on the canvas, keep the core \
shadow just past the terminator, and let the cast shadow run long and agree with the key in \
length and lean. Separate the planes by value banding - the near plane runs the full ramp, mid \
planes keep three steps, far planes keep two dithered with one step of the background colour \
mixed in - so the depth comes out of the palette already in use instead of a new colour. No \
second key light, and the vignette only if the user asked for one.",
    },
    Preset {
        id: "material",
        label_zh: "材质区分",
        label_en: "Material read",
        rules: "Split the subject by material before drawing a single pixel and give each \
material its own ramp and its own light response: metal takes a 1px specular band with an \
angular tail and a very dark core shadow; skin and fur take the widest diffuse and glow warm \
where the form is thin; cloth takes no specular at all and dithers its deepest creases; stone \
is matte with every pit occluded one step; leather takes a broad mid highlight and a worn edge; \
glass is transmission plus a dark line at the liquid surface and a bright rim on the far side. \
Draw each material as its own pass so one ramp never paints over another, and darken one step \
wherever two materials meet - the seam is the occlusion. A repeated material is one helper \
function called twice, never a second hand-written copy. Lay the thin parts - the specular \
band, the far-side rim on glass, the worn edge on leather - with an aa* helper or blend() so \
their partial coverage falls out for free, and hold the hard edge wherever the outline says so.",
    },
    Preset {
        id: "polish",
        label_zh: "细节精修",
        label_en: "Detail polish",
        rules: "After the form reads, add only what the form already implies: a 2x2 pupil with a \
one-pixel highlight, a pink inner ear, a lighter pad, a tapering tail, a worn edge, a 1px seam. \
Tighten every detail into 2x2-plus clusters (a lone pixel reads as dirt), redraw any curve that \
meets a contrasting background through an aa* helper or blend() instead of stair-stepping it, \
and sweep the silhouette once for stray single pixels outside the subject. Deepen the contact \
shadow and sub-divide one ramp step with a dither band rather than adding a colour. Stop the \
moment the subject still reads at 1x - a crowded drawing is as wrong as a bare one, and detail \
density scales with the canvas (under 32px this means silhouette plus two shades).",
    },
    Preset {
        id: "depth",
        label_zh: "景深层次",
        label_en: "Depth cueing",
        rules: "Cue depth with what is already in the palette: overlap near forms over far ones, \
scale contact shadows with the distance from the ground plane, and band the planes by value - \
the near plane runs the full ramp, mid planes keep three steps, far planes keep two dithered \
with one step of the background colour mixed in so the air sits between them. Keep ONE light \
direction across every plane, drop a horizon only where the deliverable calls for it, and hold \
the busiest texture in the near plane. Read the canvas back once at the end: if the far plane \
carries more detail than the near one, the depth cue is inverted. Lay the near plane's overlap \
seam with an aa* helper or blend() so the edge between two planes reads as air rather than a \
cut-out, and keep the hard edge where the outline strategy already owns it.",
    },
    Preset {
        id: "soft",
        label_zh: "柔边平滑",
        label_en: "Soft edges",
        rules: "Softness comes from coverage, never from new colours: lay every flat base pass \
with the hard-edged helpers, then spend the aa* family on the detail layer with the same \
coordinates - aaline/aaseg, aacurve (aaquad), aacubic (aabez), aapoly/aapath and \
aapolyfill (aafill), aacircle, aaellipse, aarect - and blend(x,y,c,a) for a single \
already-placed pixel. On a palette-locked layer every blended colour snaps back into the \
range, so use dither(x,y,c,a) when a soft edge has to stay inside a tight range. Keep aa* off \
sprites under 32px and along any outline, where the hard edge reads better, and keep the \
silhouette's own step rhythm intact - a soft edge is a finish, not a new contour.",
    },
    Preset {
        id: "texture",
        label_zh: "肌理质感",
        label_en: "Surface texture",
        rules: "Texture lives inside the ramp, never outside it: pick the texture from the \
material (noise() for organic surfaces such as fur, bark and stone; an ordered pattern for \
manufactured ones such as cloth weave, brick and metal plate) and hold the value band while \
you apply it. Follow the form with it - stretched along a limb, orthogonal on a wall, circular \
on a shoulder - so the surface wraps instead of sitting on top of the shape. Keep one texture \
per material, dither at most a third to a half of any area, and never carry a texture across \
an outline or into the smallest detail.",
    },
    Preset {
        id: "form",
        label_zh: "体积塑造",
        label_en: "Volumetric form",
        rules: "Make every pixel earn its value from the form underneath it rather than from a ramp \
offset picked once: read the form normal at that pixel - the dy of the ellipse it sits in, a distance \
from the light, or a cross product of two tangents along the contour - dot it with the ONE light \
direction, and lay that band through a pget test over the base pass so the silhouette, the stripes \
and the eyes already on top survive. The terminator is the sharpest step on the canvas and sits just \
past where the form turns away from the light, never riding the silhouette edge; the core shadow \
falls one step beyond it, and one rim light runs the lit contour against the darker ground. A rounded \
torso reads as one ball stretched sideways - which is what a torso is - while a flat plane splits into \
two or three value bands along its own grid angle, never into a gradient. Lay the rim and the \
terminator along any curve with an aa* helper or blend() so the partial coverage falls out for free, \
then check the frame once in one flat value: if the form does not read as a silhouette, the lighting \
is decoration.",
    },
    Preset {
        id: "silhouette",
        label_zh: "剪影可读",
        label_en: "Silhouette read",
        rules: "Pass the silhouette gate before spending one pixel on shading: fill the subject in one \
flat colour, read the outer contour at 1x, and confirm the pose, the proportions and the focal point \
survive that read on their own - a shape that needs its shading to be understood is a shape that was \
not drawn. Settle the whole silhouette out of filled shapes first (overlapping circles plus a rect to \
flatten a bottom is the normal way), then shade by RE-WRITING pixels that pass a pget test, because a \
second filled shape dropped over a finished one deletes the detail under it for good. Keep every \
detail inside the contour, tighten stray single pixels into 2x2 clusters or sweep them off, and hold \
the contour's own step rhythm - a 45-degree run is one pixel per row, while a small curve may instead \
be laid with aacurve or aapoly so it reads genuinely smooth. Nothing crosses the outline, no detail \
hangs off it, and the read has to hold at the canvas's own size rather than at a zoom.",
    },
    Preset {
        id: "weathered",
        label_zh: "磨损做旧",
        label_en: "Wear and age",
        rules: "Wear follows use, never noise: put the chips and the rubbed-clean patches where the \
subject was handled or hit - the contact points, the leading edge into the motion, the raised corners \
that catch first - and leave the sheltered inside of a curve clean. Paint loss is one ramp step \
showing through another, laid as 2x2 clusters with a dithered edge where it thins and never as a \
second colour; dirt and rust collect inside the occlusion you already darkened, one step below the \
step beside it; a worn edge is the light step broken by two or three nicks and then rounded. Each \
material wears its own way - metal pits and streaks along the rubbing direction, leather goes shiny \
where it flexes, cloth pills and frays at the hem, stone loses only its top edges. Hold the value band \
while you apply it so wear never opens a new ramp step, soften a nick that meets a contrasting \
background through the aa* family or blend(), and stop at two or three wear sites on a small canvas: \
weathering is what sells scale, and a fully weathered small sprite just reads as dirt.",
    },
    // ---------- 细节六条 ----------
    // 上面十条管「收尾」，这一组管「细节从哪里长出来」。分开的理由很实际：
    // 一条「写实渲染」把高色数、形体光影、接触阴影全说了，可它说得再全也替代不了
    // 「最后一两个像素该放在哪里」。后者才是用户盯着图放大看时判断「细不细」的东西。
    //
    // 这六条里有的会被画风自带着上路，那张映射表在 artstyle 的
    // `quality_preset_ids`：写实、精细自带微细结构，厚涂自带材质区分，等等。
    // 只有一份结论，就放在那一处；这里再抄一遍，迟早两边对不上。
    Preset {
        id: "microdetail",
        label_zh: "微细结构",
        label_en: "Micro detail",
        rules: "Spend the last pass on the details that only exist at one or two pixels, and put \
them where the eye already lands instead of spreading them evenly: fur direction in 1px strokes \
along the form, a seam or a stitch run where two panels meet, a rivet or a bolt as a 2x2 cluster \
with one lit pixel, scratches that follow the direction of the rub, freckles or scales held to one \
patch rather than the whole body, a weave or grain kept inside a single value band. Every one of \
them is a 2x2-or-larger cluster at 32px and up, so nothing floats and no lone pixel reads as dirt. \
Two or three sites is the whole budget on a small canvas - detail density scales with the canvas, \
and a crowded drawing is as wrong as a bare one. Lay a curved strand with aaline or aapoly and \
blend(x,y,c,a) for the single pixel it lands on, never a third ramp colour to pay for it, and never \
carry a texture across an outline or into the smallest feature.",
    },
    Preset {
        id: "occlusion",
        label_zh: "闭塞接触",
        label_en: "Occlusion contact",
        rules: "Darkness collects wherever two forms meet, and it is the cheapest way to make a \
flat drawing read as solid: run ONE dedicated occlusion pass after the shading is final - never as \
a second filled shape over it, because that deletes the detail underneath for good. Under a chin, \
inside an armpit, where a limb meets the belly, behind an ear, under a lip, along a ground contact: \
darken one step past the ramp with a pget test (if pget(x,y)==base then pset(x,y,dark) end), and \
keep the band tight - 1-2px on a 64px canvas, wider only where the gap between the forms is wider. \
The contact shadow where the subject meets the ground runs the same logic: a tight dark band, then \
a dithered falloff that thins to nothing, kept on its own pass so the outline never eats it. Then \
bounce half a step of the surface colour back into every lifted underside that faces a bright \
ground - a shadow that never gets light back is the most common reason a sprite looks pasted on. \
Soften an occluding edge that meets a contrasting background through blend() or the aa* family so \
it reads as air rather than a cut.",
    },
    Preset {
        id: "value",
        label_zh: "明度结构",
        label_en: "Value structure",
        rules: "Settle the value structure before the colours, because a drawing that only reads in \
colour does not read at all: group the canvas into three to five value bands, keep every material \
inside them, then prove it once by reading the frame in one flat value - a grayscale downsample or \
a 1-bit threshold - where the silhouette, the lit side and the subject/ground separation must \
survive on their own. The terminator is the sharpest step on the canvas and sits just past where \
the form turns away from the light, never riding the silhouette edge; the core shadow falls one \
step beyond it, and the mid step is the scarcest value on the canvas rather than the default fill. \
Separate overlapping forms by value first and hue second, so the picture still holds when the \
palette is desaturated. Keep the silhouette's own step rhythm, and lay the terminator along any \
curve with an aa* helper or blend() so its partial coverage falls out for free instead of \
stair-stepping.",
    },
    Preset {
        id: "hierarchy",
        label_zh: "色彩层级",
        label_en: "Colour hierarchy",
        rules: "Order the palette by job instead of by hue: the ground and the background take the \
desaturated low-chroma steps, the subject takes the mid and high chroma, and ONE accent colour is \
the only place on the canvas allowed to be the most saturated thing. Reuse the existing palette \
colors and hue-shift each ramp both ways - cooler shadows, warmer highlights - so depth comes out \
of the palette already in use rather than out of a new colour, and give every material its own \
ramp. Where the user named a small palette, that palette IS the hierarchy: decide which of its \
colours plays the accent and let the rest fall silent. Never let two accents fight, never rainbow \
a subject across hues to pass for detail, and soften an accent that meets a contrasting background \
through the aa* family or blend(x,y,c,a) rather than mixing a new in-between colour.",
    },
    Preset {
        id: "reflection",
        label_zh: "反射透光",
        label_en: "Reflection and translucency",
        rules: "Light that comes back off a surface is what separates a material from a flat patch \
of colour. A specular is one 1-2px band placed where the form turns toward the light, it moves \
with the light, it never exceeds two on the whole canvas, and it never lands on cloth, which takes \
no specular at all. A reflection is the mirrored form clipped to the surface it sits on, broken by \
that surface's own texture and darkened with distance from the reflecting edge; water breaks it \
into horizontal runs at the water's rhythm, metal keeps it sharp and darkens one step. \
Translucency is the thin part of a form - an ear, a wing, a leaf, a fin - glowing warm where the \
light passes through, laid as a lighter step inside the same ramp with a bright rim on the far \
side. Glass and crystal take transmission plus a dark line at the surface and a bright rim on the \
far side. Lay every specular and rim along a curve with an aa* helper or blend() so its partial \
coverage falls out for free, and hold the hard edge wherever the outline strategy already owns it.",
    },
    Preset {
        id: "composition",
        label_zh: "构图焦点",
        label_en: "Composition and focus",
        rules: "Compose the frame so the eye has one first stop and then a path: place the subject \
by design - centre it and let it fill most of the canvas unless the user named a spot - then spend \
the detail budget in descending order outward from the focal point, so the near plane carries the \
busiest texture while the far plane keeps two dithered steps. Leave deliberate negative space \
rather than filling every corner, and let the silhouette break that space into unequal parts \
instead of parking the subject dead centre of an empty frame. Read the frame once at thumbnail \
size: if the detail density sits away from the focal point, or the far plane reads busier than the \
near one, move the detail rather than adding more of it. Keep the horizon or the ground line only \
where the deliverable calls for it, and soften the seam where two planes overlap through the aa* \
family or blend() so the edge reads as air rather than a cut-out.",
    },
];

/// 按 id 取预设。界面钉上来的 id 一定来自这里，认不出来的由调用方报错——
/// 静默忽略会让用户以为规矩上了路，其实这一轮什么都没多带。
pub fn parse(raw: &str) -> Option<&'static Preset> {
    let raw = raw.trim();
    ALL.iter().find(|preset| preset.id == raw)
}

/// 界面上「不限预设」的那个选项。和画风的「自动」同一个位置、同一个意思。
pub const NONE: &str = "auto";

#[cfg(test)]
mod tests {
    use super::*;

    /// id 是全链路的钥匙：重复一个，界面两个选项就指向同一套规矩。
    #[test]
    fn every_preset_has_a_unique_id() {
        let mut ids: Vec<&str> = ALL.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "预设 id 重复了");
    }

    /// 规矩正文就是这条预设存在的全部理由：空着的选项比没有这个选项更糟。
    #[test]
    fn every_preset_carries_rules_and_both_labels() {
        for preset in ALL {
            assert!(
                !preset.rules.trim().is_empty(),
                "{} 没有规矩正文",
                preset.id
            );
            assert!(
                preset.rules.len() > 200,
                "{} 的规矩太短，压不住场面",
                preset.id
            );
            assert!(
                !preset.label_zh.trim().is_empty(),
                "{} 没有中文名",
                preset.id
            );
            assert!(
                !preset.label_en.trim().is_empty(),
                "{} 没有英文名",
                preset.id
            );
        }
    }

    /// 规矩里不许出现画风的硬指标：色数上限、描边策略、抖动规则归画风管，
    /// 预设段重复一遍，模型只会在两套要求里猜权重。
    #[test]
    fn rules_stay_out_of_the_styles_territory() {
        for preset in ALL {
            let body = preset.rules.to_lowercase();
            for banned in [
                "colour count is",
                "palette is limited to",
                "one outline strategy",
                "never use an outline",
            ] {
                assert!(
                    !body.contains(banned),
                    "{} 的规矩管到画风的地盘了：{banned}",
                    preset.id
                );
            }
            assert!(
                body.contains("aa") || preset.id == "texture",
                "{} 没收边条款，柔边这条腿断了",
                preset.id
            );
        }
    }

    #[test]
    fn parse_round_trips_and_rejects_unknown_ids() {
        for preset in ALL {
            assert_eq!(parse(preset.id), Some(&preset));
            assert_eq!(parse(&format!(" {} ", preset.id)), Some(&preset));
        }
        assert!(parse("").is_none());
        assert!(parse("photoshop").is_none());
        assert_eq!(parse("auto"), None, "「不限」不是一个预设，调用方要自己认");
    }

    /// 叠加上限卡在中间：小于 2 等于没做叠加，等于总数则界面失去取舍。
    /// 三个是「形体 + 材质 + 收尾」刚好够用的那个数，再多就开始互相稀释。
    #[test]
    fn the_stack_cap_leaves_room_to_choose() {
        // 编译期常量比大小，运行时断言是句废话（clippy 就是这么说的）。
        // 但这条挡的是「后人把上限改成 1」：那一下子叠加就名存实亡，
        // 而改动本身不会有任何症状。押进 const 块，编译期就炸，语义不变。
        const { assert!(MAX_STACKED >= 2, "只让叠一个，叠加机制等于没做") };
        assert!(
            MAX_STACKED < ALL.len(),
            "上限等于总数，用户在界面里就没有取舍了"
        );
    }

    /// 新加的六条细节预设各自有活干：id 不能被前十条覆盖，规矩也不能互相抄。
    /// 这条守的是「加了等于没加」——六条近似的规矩只会把同一份预算花两遍。
    #[test]
    fn the_detail_presets_each_carry_their_own_job() {
        for id in [
            "microdetail",
            "occlusion",
            "value",
            "hierarchy",
            "reflection",
            "composition",
        ] {
            let preset = parse(id).unwrap_or_else(|| panic!("缺细节预设 {id}"));
            // 每条都要点到自己的核心词，抄了别人的规矩就会在这里露出来。
            let signature = match id {
                "microdetail" => "one or two pixels",
                "occlusion" => "two forms meet",
                "value" => "flat value",
                "hierarchy" => "ONE accent colour",
                "reflection" => "mirrored form",
                "composition" => "one first stop",
                other => panic!("未知细节预设 {other}"),
            };
            assert!(
                preset.rules.contains(signature),
                "{id} 的规矩里没有自己的核心词：{signature}"
            );
        }
    }
}
