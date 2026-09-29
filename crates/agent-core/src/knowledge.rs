//! 自带知识库：明文检索，把这一轮真用得上的那几条摆进系统提示词。
//!
//! 做法是「关键词命中加权」而不是向量检索：每条知识自带中英触发词，命中越多、
//! 词越长，排得越前。不需要 embedding、不需要模型，桌面端零依赖零延迟，
//! 而且检索结果可复现、可解释——用户问「为什么突然说到抖动」时，
//! 答案是能指着条目给他看的。
//!
//! 和 colornames / glossary 两张全量表的区别在这儿：那两张每轮都进提示词，
//! 因为每句话都可能提到一个颜色名或术语；知识库只进命中的那几条，
//! 因为「行走图的落地要点」放进一句「画个图标」的轮次里纯属占预算。

/// 一条知识。`keywords` 是中英触发词（含别称），命中任意一个都算数。
pub struct KnowledgeEntry {
    /// 稳定标识，界面和调试日志都念它。
    pub id: &'static str,
    /// 一句话标题，模型扫条目时先看这个。
    pub title: &'static str,
    pub keywords: &'static [&'static str],
    /// 正文。写给模型的落地要点，一两句话说完，不做教科书。
    pub body: &'static str,
}

/// 默认检出条数。再多就挤压通用规则的预算了。
pub const DEFAULT_LIMIT: usize = 4;

/// 检查预算：条目正文注入系统提示词的字符上限，防某一轮把上下文吃干。
pub const DEFAULT_BUDGET: usize = 2600;

/// 知识库。按「运动 -> 上色 -> 造型 -> 场景 -> 工具词」排，同组相邻，
/// 模型扫起来快；检索同分时按这个顺序兜底，结果稳定可复现。
pub const ENTRIES: &[KnowledgeEntry] = &[
    KnowledgeEntry {
        id: "walk-cycle",
        title: "Walk and run cycles",
        keywords: &["行走", "走路", "步态", "奔跑", "跑动", "walk", "walking", "walk cycle", "run cycle", "gait"],
        body: "Drive every limb from ONE phase variable. A 4-beat walk spreads the four legs over 0 / 0.25 / 0.5 / 0.75; a trot uses two diagonal pairs a half cycle apart. Lift a foot only while it swings forward - lift = max(0, sin(2pi*phase)) - and keep it planted while it travels back. The body bobs at twice the step frequency and the head counter-bobs a little. Close the loop: the last frame has to flow back into the first.",
    },
    KnowledgeEntry {
        id: "frame-timing",
        title: "Frame timing and animation rhythm",
        keywords: &["帧率", "时长", "动画", "动效", "补间", "frame duration", "timing", "fps", "tween", "animation"],
        body: "8-12 fps is the pixel-art norm. Hold key and contact poses longer and pass through in-betweens quickly; set each frame's duration instead of relying on a uniform rate, and keep the total loop divisible so the repeat is invisible. On small canvases, fewer frames with longer holds read better than many 60ms frames.",
    },
    KnowledgeEntry {
        id: "tilemap",
        title: "Tilesets and tilemaps",
        keywords: &["瓦片", "地图", "地块", "瓷砖", "平铺", "tilemap", "tile map", "tileset", "tile set", "tiles", "无缝平铺"],
        body: "Work on a cell grid (8/16/32px) and make every tile edge-welding so rows repeat without seams. Ship a small variant set - base plus two or three edge and corner transitions - instead of one big picture. Share ONE light direction and ONE ramp per material across the whole set, then draw at 1x and verify by repeating the tile twice in each axis.",
    },
    KnowledgeEntry {
        id: "dithering",
        title: "Dithering and ordered patterns",
        keywords: &["抖动", "网点", "棋盘", "递色", "dither", "dithering", "ordered", "bayer", "checkerboard"],
        body: "Alternate two ramp steps in a regular pattern instead of adding an in-between color. Ordered (Bayer 4x4) keeps texture and direction and fits marble, skin and sky; noise dithering reads organic but dirties up close. Dither at most a third to a half of an area, and never across an outline.",
    },
    KnowledgeEntry {
        id: "ramps",
        title: "Value ramps and hue shifting",
        keywords: &["色阶", "过渡", "渐变", "明暗", "暗部", "亮部", "ramp", "ramps", "shading", "value steps", "gradient", "hue shift"],
        body: "Build 3-5 steps from core shadow to highlight, hue-shifted (cool violet-blue shadows, warm highlights) rather than just adding white and black. Place the darkest step just past the terminator, not on the object's edge. Generate steps with hsv()/mix() so the rhythm stays even, and give each material its own ramp.",
    },
    KnowledgeEntry {
        id: "outlines",
        title: "Outline strategy",
        keywords: &["勾线", "描边", "轮廓线", "线稿", "outline", "outlines", "line art", "hard edge"],
        body: "Pick ONE strategy for the whole drawing - solid dark, darker only on the shadowed side, or none - and keep it consistent. Hue-shift the outline toward the surface color instead of using pure black on a saturated body, or it eats the silhouette. Keep outlines 1px at 32px and above; below 16px skip them entirely.",
    },
    KnowledgeEntry {
        id: "pixel-clusters",
        title: "Pixel clusters and curve rhythm",
        keywords: &["像素簇", "孤立像素", "锯齿", "毛刺", "阶梯", "pixel cluster", "single pixel", "jaggies", "curve rhythm"],
        body: "Details read as 2x2-plus clusters; one stray pixel reads as dirt. The fix is a regular step rhythm, not an extra color: 45 degrees = one pixel per row, about 22.6 = 2-pixel runs, about 30 = evenly spaced. When a curve looks lumpy, re-space the runs instead of smoothing them.",
    },
    KnowledgeEntry {
        id: "circles",
        title: "Circles and ellipses at low resolution",
        keywords: &["圆形", "圆角", "球体", "弧线", "circle", "circles", "ellipse", "sphere", "round shape"],
        body: "A pixel circle is never symmetric in both axes - use an odd diameter on the axis that must read as round and accept a slightly flatter other axis. For radius under 5, hand-tune the placement instead of computing it. Shade a circle by cutting the lit side, not by adding a radial gradient.",
    },
    KnowledgeEntry {
        id: "palette",
        title: "Palette design",
        keywords: &["调色板", "配色", "色板", "配色方案", "palette", "color scheme", "swatch", "colors"],
        body: "Decide the palette before drawing, then spend only its steps: one dominant hue family, one accent, and a neutral ramp. Give each material its own ramp. Tiny canvases (under 32px) hold 4-6 colors; 64px and up can carry 12-20 without turning to noise.",
    },
    KnowledgeEntry {
        id: "lighting",
        title: "One light source",
        keywords: &["光源", "光照", "受光", "背光", "阴影", "投影", "cast shadow", "light source", "lighting"],
        body: "Fix ONE light direction (default top-left) and honour it on every object in the scene. The lit side takes the highlight, the shadow side a darker ramp step, and the darkest core shadow sits just past where the form turns away. Cast shadows must agree with that same direction in both length and lean.",
    },
    KnowledgeEntry {
        id: "silhouette",
        title: "Silhouette first",
        keywords: &["剪影", "外形", "读形", "辨识度", "silhouette", "shape read", "readability"],
        body: "Draw the shape flat in one colour before shading anything. If it does not read as one blob, shading will not save it - restate the proportions or the pose. Keep the iconic detail (ears, horn, weapon, hat) clear of the body so it survives the outline.",
    },
    KnowledgeEntry {
        id: "isometric",
        title: "Isometric and 2.5D grids",
        keywords: &["等轴", "等距", "立体地图", "斜视", "isometric", "iso", "axonometric", "dimetric"],
        body: "Keep ONE grid angle for the whole sheet: true isometric is 2:1 pixels, dimetric sits near 30 degrees. Give the three visible faces three distinct steps of the same ramp so a cube reads without an outline. Never mix angles, and keep verticals exactly vertical.",
    },
    KnowledgeEntry {
        id: "perspective",
        title: "Depth, planes and parallax",
        keywords: &["透视", "灭点", "纵深", "视差", "perspective", "vanishing", "depth", "parallax", "planes"],
        body: "Split the scene into far, mid and near planes, then push distance with value and saturation: lift shadows and desaturate far, saturate and darken near. Keep one horizon and one ground angle. Parallax layers should overlap at least a third of their height so the gap does not read as a seam.",
    },
    KnowledgeEntry {
        id: "seamless",
        title: "Seamless and repeatable textures",
        keywords: &["无缝", "可平铺", "重复图案", "seamless", "tileable", "repeat", "repeating"],
        body: "Make opposite edges exact complements: draw each element five times offset by the cell size, or draw at 2x and downsample. Keep the repeat cell small (8-32px) so the eye reads texture rather than a poster, and never put a single weighted element dead centre.",
    },
    KnowledgeEntry {
        id: "materials",
        title: "Material logic: metal, wood, cloth",
        keywords: &["金属", "木头", "木纹", "布料", "织物", "材质", "metal", "wood", "grain", "cloth", "fabric", "material"],
        body: "Metal takes a hard thin specular and a dark core shadow; wood keeps its grain direction constant and takes a soft core shadow; cloth folds wide with big mid tones and no hard specular. Give each material one ramp and never let two material ramps cross halfway.",
    },
    KnowledgeEntry {
        id: "terrain",
        title: "Terrain and ground planes",
        keywords: &["地形", "草地", "地面", "岩石", "沙漠", "沙地", "雪地", "terrain", "ground", "grass", "rock", "sand", "snow"],
        body: "Break the ground plane into two or three bands of the same ramp, then vary only the top edge - a straight boundary line reads as a cut-out. Add a darker contact band where anything sits on the ground, and keep pebble and clump shapes in one size family.",
    },
    KnowledgeEntry {
        id: "water",
        title: "Water, foam and reflections",
        keywords: &["水面", "湖水", "海面", "海浪", "波浪", "泡沫", "倒影", "water", "waves", "sea", "lake", "foam", "reflection", "ripple"],
        body: "Water takes the sky's key colours, not its own hue: mirror the darkest and lightest sky steps and keep the mid tone. Foam reads as clusters along the contact edge, not as white blobs; reflections are vertically squashed copies of the object, broken by horizontal wobble.",
    },
    KnowledgeEntry {
        id: "foliage",
        title: "Foliage and plants",
        keywords: &["树叶", "植物", "树丛", "森林", "花草", "foliage", "leaves", "leaf", "tree", "trees", "bush", "bushes", "plant"],
        body: "Cluster leaves into overlapping lobes of two or three sizes rather than drawing individual blades; vary only the outer edge. Keep the light direction on every lobe, and let the trunk or branch show through the gaps so the mass does not turn into one flat blob.",
    },
    KnowledgeEntry {
        id: "sky",
        title: "Sky, clouds and space",
        keywords: &["天空", "星空", "晚霞", "云朵", "云层", "sky", "clouds", "cloud", "stars", "space", "sunset"],
        body: "A sky is a vertical ramp with no hard edges; put the light source's warmth into the horizon band and the cool end at the top. Clouds take cumulus lobes with a flat shaded base and a lit top; at night keep only two star brightnesses so the field stays even.",
    },
    KnowledgeEntry {
        id: "proportions",
        title: "Character proportions at small sizes",
        keywords: &["人物", "人体", "角色", "立绘", "比例", "character", "proportions", "figure", "hero"],
        body: "At 32px and under a character is five to six heads tall with no neck detail and mitten hands; under 16px reduce to three heads and read the pose from a single limb line. Spend the detail budget on the eyes and the silhouette - everything else is an accent.",
    },
    KnowledgeEntry {
        id: "eyes",
        title: "Eyes and facial read",
        keywords: &["眼睛", "眼部", "瞳孔", "眼神", "脸部", "表情", "eye", "eyes", "pupil", "face", "expression"],
        body: "On most sprites the eye white is a 2x3 to 4x6 block and the pupil a 2x2 core. Put the highlight in the upper left for the default light. Do not outline the eye against the face - a dark pupil on light fur reads better than a black socket.",
    },
    KnowledgeEntry {
        id: "motion",
        title: "Anticipation, follow-through and smear frames",
        keywords: &["预备", "跟随", "残影", "挤压", "拉伸", "动势", "anticipation", "follow-through", "smear frame", "squash", "stretch", "motion"],
        body: "Fast motion needs anticipation (a compressed pose before the extension), follow-through (limbs continuing after the body stops) and, above roughly 3 pixels per frame, a smear frame - an elongated blob between poses. Without them, fast animation reads as teleporting.",
    },
    KnowledgeEntry {
        id: "glow",
        title: "Glow, neon and light effects",
        keywords: &["发光", "光晕", "霓虹", "激光", "辉光", "glow", "neon", "laser", "bloom", "light beam"],
        body: "Glow is built from the OUTSIDE in: tint the background two steps toward the light colour, then place the hot core last - never paint the core first and bleed it. Keep the halo colour-shifted toward the source hue, and let the darkest part of the scene sit right next to the light so contrast stays.",
    },
    KnowledgeEntry {
        id: "rim-light",
        title: "Rim light and backlight",
        keywords: &["轮廓光", "边缘光", "逆光", "rim light", "backlight", "edge light", "silhouette light"],
        body: "A rim light is a thin 1px band of the lightest ramp step on the shadow side of the form, and it must follow the same light direction as everything else. Use it to separate a dark subject from a dark background; drop it where the subject already contrasts with its backdrop.",
    },
    KnowledgeEntry {
        id: "contrast",
        title: "Contrast and visual hierarchy",
        keywords: &["对比", "主次", "焦点", "突出", "醒目", "contrast", "hierarchy", "focal point", "emphasis"],
        body: "Decide the one focal point and give it the strongest value contrast; push everything else one or two steps flatter. Reserve the darkest dark and lightest light for that spot. If everything is crisp, nothing reads first.",
    },
    KnowledgeEntry {
        id: "symmetry",
        title: "Symmetry and mirroring",
        keywords: &["对称", "镜像", "左右对称", "symmetry", "mirror", "mirrored", "symmetric"],
        body: "Perfect symmetry reads as stiff - mirror the body, then break it by turning the head, shifting the nearer arm, or changing one leg phase. For genuinely symmetric objects (icons, crowns, shields) draw half and mirror it, then add one asymmetric accent so it does not look stamped.",
    },
    KnowledgeEntry {
        id: "small-sprite",
        title: "Drawing at 8-16 pixels",
        keywords: &["小图", "小尺寸", "迷你", "16x16", "16像素", "32x32", "tiny", "small sprite", "small canvas"],
        body: "Below 16px every pixel is load-bearing: one silhouette, two to four interior shades, no outline, and the detail budget goes to the eyes or the single iconic feature. Halve the canvas in your head and design the shape at that size before placing pixels.",
    },
    KnowledgeEntry {
        id: "upscale",
        title: "Scaling and crisp edges",
        keywords: &["放大", "缩放", "模糊", "倍率", "upscale", "scale", "nearest neighbor", "crisp", "pixel perfect"],
        body: "Scale pixel art with nearest neighbour at integer factors or it blurs into mush. Integer doubling keeps every pixel a square block; non-integer factors make uneven pixel sizes - avoid them or accept the wobble. The editor's zoom view scales up; the export does not bake in any smoothing.",
    },
    KnowledgeEntry {
        id: "pixel-font",
        title: "Pixel text and glyphs",
        keywords: &["像素字", "字体", "文字", "字形", "font", "text", "glyph", "lettering", "typography"],
        body: "A pixel font needs one consistent stem width (usually 1-2px), one x-height and no curve finer than 2px; draw only the characters the sheet needs and check them at 1x, never at zoom. Keep the baseline on one row for the whole set and leave one empty row between lines.",
    },
    KnowledgeEntry {
        id: "texture",
        title: "Noise, scanlines and retro texture",
        keywords: &["噪点", "颗粒", "肌理", "老电视", "扫描线", "做旧", "noise", "grain", "scanline", "texture", "dirty"],
        body: "Texture is a threshold pattern, not random smearing: noise(x,y,scale) with a low scale gives long streaks, a high scale gives speckle. Add texture last and at low density (10-20% of pixels) so the underlying form survives; never let it cross a ramp boundary.",
    },
    KnowledgeEntry {
        id: "ui-kit",
        title: "Pixel UI panels and frames",
        keywords: &["界面", "面板", "按钮", "边框", "血条", "进度条", "panel", "button", "frame", "border"],
        body: "Pixel UI works on nine-slice logic: four corners unchanged, edges stretched, middle filled. Keep a 1px inner highlight top-left and a 1px dark line bottom-right, one flat fill between them, and leave one transparent pixel of padding so neighbouring frames do not touch.",
    },
];

/// 检出这一轮用得上的知识条目。明文命中加权：词越长越算数，
/// 得分相同时保持库内顺序，所以同样的提问永远得到同样的结果。
pub fn retrieve(query: &str, limit: usize) -> Vec<&'static KnowledgeEntry> {
    let needle = query.to_lowercase();
    let mut scored: Vec<(usize, &KnowledgeEntry)> = ENTRIES
        .iter()
        .filter_map(|entry| {
            let score = entry
                .keywords
                .iter()
                .filter(|kw| needle.contains(&kw.to_lowercase()))
                .map(|kw| kw.chars().count())
                .sum::<usize>();
            (score > 0).then_some((score, entry))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().take(limit).map(|(_, e)| e).collect()
}

/// 默认配置下的知识段。给「只管问，不管带几条」的调用方用。
pub fn section(query: &str) -> String {
    prompt_section(query, DEFAULT_LIMIT, DEFAULT_BUDGET)
}

/// 默认配置下的条目 id，界面上念得出「这轮参考了哪几条」。
pub fn ids(query: &str) -> Vec<String> {
    matched_ids(query, DEFAULT_LIMIT)
}

/// 检索结果的条目 id，界面上念得出「这轮参考了哪几条」。
pub fn matched_ids(query: &str, limit: usize) -> Vec<String> {
    retrieve(query, limit)
        .into_iter()
        .map(|entry| entry.id.to_string())
        .collect()
}

/// 摆进系统提示词的知识段。没有命中就返回空串——一段空标题比没有更糟。
/// `budget` 是字符上限，够几条就几条，剩下的等命中词更明确的那一轮。
pub fn prompt_section(query: &str, limit: usize, budget: usize) -> String {
    let hits = retrieve(query, limit);
    if hits.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "Relevant craft notes for this request (retrieved from the built-in knowledge base; \
         apply the parts that fit and ignore the rest):\n",
    );
    for entry in hits {
        let block = format!("- {}: {}\n", entry.title, entry.body);
        if out.chars().count() + block.chars().count() > budget {
            break;
        }
        out.push_str(&block);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_section_is_one_call_away() {
        assert!(section("画一个八帧的行走图").contains("Walk and run cycles"));
        assert!(ids("无缝瓦片地图").contains(&"tilemap".to_string()));
    }

    #[test]
    fn chinese_and_english_both_find_the_entry() {
        assert_eq!(retrieve("画一个八帧的行走图", 4)[0].id, "walk-cycle");
        assert_eq!(retrieve("draw a walk cycle", 4)[0].id, "walk-cycle");
        assert_eq!(retrieve("无缝瓦片地图", 4)[0].id, "tilemap");
    }

    #[test]
    fn a_longer_more_specific_keyword_outranks_an_incidental_one() {
        // 「抖动」和「行走」都命中，但这句话问的是步态，权重更高。
        let hits = retrieve("画一只会抖动身体的行走生物", 4);
        assert_eq!(hits[0].id, "walk-cycle");
        assert!(hits.iter().any(|e| e.id == "dithering"));
    }

    #[test]
    fn nothing_relevant_returns_nothing() {
        assert!(retrieve("你好", 4).is_empty());
        assert!(prompt_section("你好", 4, DEFAULT_BUDGET).is_empty());
    }

    #[test]
    fn the_section_never_exceeds_its_budget() {
        // 五条全命中也压得住：宁可少带一条，也不把上下文吃干。
        let section = prompt_section("行走图 瓦片 抖动 圆形 配色", 4, DEFAULT_BUDGET);
        assert!(
            section.chars().count() <= DEFAULT_BUDGET,
            "{}",
            section.chars().count()
        );
        assert!(section.contains("Walk and run cycles"));
    }

    #[test]
    fn ids_are_unique_and_every_entry_is_usable() {
        let mut ids: Vec<&str> = ENTRIES.iter().map(|e| e.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "条目 id 撞号了");
        for entry in ENTRIES {
            assert!(!entry.body.is_empty(), "{} 没有正文", entry.id);
            let mut kw: Vec<&str> = entry.keywords.to_vec();
            kw.sort_unstable();
            let n = kw.len();
            kw.dedup();
            assert_eq!(kw.len(), n, "{} 有重复触发词", entry.id);
        }
    }
}
