// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
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

/// 知识库。按「技法 -> 素材 -> 生物 -> 器物 -> 场景 -> 工具词」排，同组相邻，
/// 模型扫起来快；检索同分时按这个顺序兜底，结果稳定可复现。
/// 生物和器物两组的条目是「怎么把一样东西画对」的要点：用户说「画只猫」时，
/// 只给上色规则是不够的——模型知道怎么铺色阶，照样能把猫画成四条腿的毯子。
pub const ENTRIES: &[KnowledgeEntry] = &[
    KnowledgeEntry {
        id: "walk-cycle",
        title: "Walk and run cycles",
        keywords: &["行走", "走路", "步态", "奔跑", "跑动", "walk", "walking", "walk cycle", "run cycle", "gait"],
        body: "Drive every limb from ONE phase variable. A 4-beat walk spreads the four legs over 0 / 0.25 / 0.5 / 0.75; a trot uses two diagonal pairs a half cycle apart. Lift a foot only while it swings forward - lift = max(0, sin(2pi*phase)) - and keep it planted while it travels back. The body bobs at twice the step frequency and the head counter-bobs a little. Close the loop: the last frame has to flow back into the first.",
    },
    KnowledgeEntry {
        id: "locomotion",
        title: "Gaits: walk, trot, canter, gallop, flight",
        keywords: &[
            "奔跑", "跑动", "飞奔", "奔驰", "小跑", "奔跑动画", "跑动动画", "步伐",
            "飞行", "飞翔", "滑翔", "翱翔", "跳跃", "跳起", "跃起", "扑过去",
            "run", "running", "runner", "run cycle", "gallop", "canter", "trot",
            "sprint", "dash", "fly", "flight", "flying", "glide", "hover",
            "leap", "jump", "hop",
        ],
        body: "Name the gait first, then drive every limb from that gait's phase table. WALK: legs at 0 / 0.25 / 0.5 / 0.75 with three feet down at any moment. TROT: two diagonal pairs half a cycle apart plus a one-frame suspension between strides. CANTER: near hind, then the diagonal pair, then the leading foreleg (0 / 0.2 / 0.4 / 0.6). GALLOP: the near pair lands, the far pair a beat later, then a long pose with all four legs gathered - contact 0, gather 0.35, extend 0.6, suspension 0.75. A gallop reads as running only because that gathered suspension frame exists: skip it and the motion reads as a sped-up walk. Gait rules for all four: plant the foot while it sweeps back and lift it only on the forward swing (lift = max(0, sin(2*pi*phase))), and mirror the amplitude between the near and far pair or the far legs skate. FLIGHT: wing down at 0, catching the air 0.2-0.4, top of the upstroke 0.6, folded 0.8. HOPS and LEAPS squash the body to three quarters height at 0, stretch toward the target at 0.5, land and squash again at 1.",
    },
    KnowledgeEntry {
        id: "frame-timing",
        title: "Frame timing and animation rhythm",
        keywords: &["帧率", "时长", "动画", "动效", "补间", "frame duration", "timing", "fps", "tween", "animation"],
        body: "8-12 fps is the pixel-art norm. Hold key and contact poses longer and pass through in-betweens quickly; set each frame's duration instead of relying on a uniform rate, and keep the total loop divisible so the repeat is invisible. On small canvases, fewer frames with longer holds read better than many 60ms frames.",
    },
    KnowledgeEntry {
        id: "sprite-sheet",
        title: "Sprite-sheet and sequence layout",
        keywords: &[
            "序列帧",
            "帧序列",
            "排列",
            "排版",
            "摆帧",
            "图集",
            "雪碧图",
            "sprite sheet",
            "spritesheet",
            "contact sheet",
            "sequence layout",
            "frame strip",
        ],
        body: "A multi-frame deliverable inherits the rhythm of its motion: keep subject placement and palette byte-identical across the strip so only the moving limbs change, park the key and contact poses at the ends of the row and space the in-betweens evenly between them, then hold each key pose one duration longer than its briefs. Keep the ground line, the light direction and the body outline the same on every frame - a limb that drifts one pixel mid-loop reads as a glitch, not as motion. Read the frames back before finishing: a tool result shows the active frame only, so a limb that wanders out of the body on frame three is invisible unless you look at all of them.",
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
        id: "smooth-curves",
        title: "Smooth curves, rounds and soft edges",
        keywords: &[
            "平滑", "曲线", "弧线", "圆润", "圆滑", "柔边", "抗锯齿", "收边", "流畅", "平滑曲线",
            "smooth", "curve", "curves", "rounded", "round", "bezier", "spline",
            "anti-alias", "anti-aliased", "soft edge", "feather",
        ],
        body: "A hard-edged helper rounds geometry to whole cells, so a 46 degree diagonal breaks into steps and an r=5 circle only has six directions to go. Draw the detail layer with the aa* family - same coordinates, real coverage per cell: aaline/aaseg for one segment, aacurve (aaquad) for a quadratic through a control point, aacubic (aabez) for a cubic, aapoly/aapath and aapolyfill/aafill for a point table (a triangle is three points), aacircle, aaellipse, aarect, and blend(x,y,c,a) for one already-placed pixel. Lay the flat base passes with the hard-edged fills first and curve over them: an aa* stroke blends with what is underneath instead of punching a hole. Keep aa* off sprites under 32px and off outlines, where the hard edge reads better; on a palette-locked layer every blended colour snaps back into the range, so use dither(x,y,c,a) when a soft edge must stay inside a tiny range.",
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
        keywords: &["透视", "灭点", "纵深", "视差", "景深", "空气透视", "远景", "近景", "perspective", "vanishing", "depth", "parallax", "planes", "atmospheric"],
        body: "Split the scene into far, mid and near planes, then push distance with value and saturation: lift shadows and desaturate far, saturate and darken near. Atmospheric perspective is the same lever per pixel - anything further away loses contrast, drifts toward the sky colour and loses detail, and the nearest plane keeps the darkest dark and the sharpest edge. Keep one horizon and one ground angle. Parallax layers should overlap at least a third of their height so the gap does not read as a seam.",
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
        body: "Every material takes light differently and that difference is the whole read: metal takes a hard 1px specular band, an angular tail and a very dark core shadow; wood keeps its grain direction constant and takes a soft core shadow; cloth folds into wide mid tones with no specular at all and dithers its deepest creases; stone is matte with occlusion darkening every pit and one bright top edge per facet; leather takes a broad mid highlight and a worn edge; skin and fur are diffuse and glow warm at thin parts; glass is transmission plus a dark line at the liquid surface and a bright rim on the far side; foliage glows where thin leaves are backlit. Give each material ONE ramp, never let two material ramps cross halfway, and shade a form built from two materials as two separate passes.",
    },
    KnowledgeEntry {
        id: "form-shading",
        title: "Shading a form from its geometry",
        keywords: &[
            "立体", "体积", "体积感", "明暗交界", "受光面", "背光面", "法线", "转折", "立体感",
            "form shading", "volume", "normal", "terminator", "light side",
        ],
        body: "Shade from the FORM, not from a preset offset. On a sphere or ellipse the normal at (x,y) is ((x-cx)/rx, (y-cy)/ry) normalized, and lambert is that dotted with the light vector; cut the ramp on lambert instead of filling one value. The sharpest ramp step - the terminator - belongs where the normal turns away from the light, at roughly 50-60 degrees from it, NEVER on the silhouette edge. On a cylinder keep the terminator parallel to the long axis; on a box give each visible face one flat value and let the crease do the work. A curved form never gets fewer than three steps, and its shadow side keeps one mid step instead of going to black: shaded to black it reads as a hole, so save the darkest step for the core shadow just past the terminator.",
    },
    KnowledgeEntry {
        id: "specular",
        title: "Speculars and highlights",
        keywords: &[
            "高光", "反光", "镜面", "光泽", "亮部", "刺眼",
            "specular", "highlight", "gloss", "shiny", "reflective", "sheen",
        ],
        body: "A specular belongs to the LIGHT, not the surface: it stays where the light is even when the object turns. Keep it one to two pixels at 32px and place it on the lit side just inside the terminator, never centred on the lit face. Hardness is the material - metal is a sharp band with an angular tail, wet surfaces and glass a tight bright core, glazed ceramic and plastic a broad blob, cloth, fur and skin nothing at all. Tie it to one value brighter than anything else in the drawing, and never let two speculars compete inside one silhouette.",
    },
    KnowledgeEntry {
        id: "subsurface",
        title: "Subsurface scattering: thin organic parts",
        keywords: &[
            "次表面", "透光", "半透明", "逆光", "耳朵", "薄膜", "通透", "蜡质",
            "subsurface", "translucent", "backlit", "transmission", "thin",
        ],
        body: "Thin organic parts glow when light passes through them: an ear, a tail tip, a leaf, a wing membrane and a nose bridge all shift one step toward the light colour at their thin edges, strongest where the light exits on the far side. It is the biggest realism win available on fur and foliage and it costs one line - where the form is thin, mix toward the light colour instead of toward the shadow. Keep a warm inner glow inside the shadow side of a backlit subject and darken only its outer edge: a translucent mass shaded flat reads as cardboard. Skin, wax, marble and paper carry it too; metal and stone do not.",
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
    KnowledgeEntry {
        id: "icon",
        title: "Icons, logos and marks",
        keywords: &[
            "图标",
            "图示",
            "标志",
            "app图标",
            "应用图标",
            "icon",
            "icons",
            "logo",
            "logotype",
            "favicon",
            "pictogram",
            "glyph mark",
        ],
        body: "An icon is one silhouette that has to survive at 16 pixels: settle the outer shape in a single fill first, then keep at most two or three interior masses, each readable on its own. Keep the iconic feature (leaf, blade, eye, corner) clear of the silhouette edge so a rounded mask or a crop cannot cut it. No outline at this size and no anti-aliasing inside the glyph - hard edges and 2x2 minimum clusters are what stay legible. One hue family plus one accent carries the whole mark, the accent belongs to the focal mass, and the shape has to read against both a light and a dark backdrop, so never spend the darkest dark inside the silhouette unless the backdrop is guaranteed light.",
    },
    KnowledgeEntry {
        id: "refine",
        title: "Refinement and realism",
        keywords: &[
            "细节",
            "写实",
            "细腻",
            "丰富",
            "润色",
            "优化",
            "细化",
            "detail",
            "detailed",
            "refine",
            "refinement",
            "polish",
            "realistic",
            "richer",
        ],
        body: "A refinement request means add to what is already on the canvas, never redraw it from scratch. Read the current grid first, keep the approved silhouette and pose, and never open a refinement script with clear(). Add information inside the existing structures: sub-divide a ramp step with a dither band, add a rim light on the shadow side, deepen contact shadows, tighten the cluster rhythm. Keep the palette the same unless the user asks for more colors, and end by reading the canvas back - a script that changes nothing is skipped as a replay, so change the script or finish.",
    },
    KnowledgeEntry {
        id: "realism",
        title: "Realism inside a pixel grid",
        keywords: &[
            "写实", "逼真", "拟真", "照片级", "真实感", "有质感", "立体感",
            "realism", "realistic", "photoreal", "photorealistic", "lifelike", "convincing",
        ],
        body: "Realism on a grid is information, not blur: keep every edge on the cell and spend the extra colour on form. Give each material its own 5-7 step ramp hue-shifted both ways, cut the value bands from the form itself (a normal dotted with the light vector) instead of a preset offset, and place the sharpest step - the terminator - where the form turns away from the light, never on the silhouette edge. Put one anti-aliased pixel where a curve meets a contrasting background (aaline/aacurve, or blend on a single pixel), darken one step past the ramp wherever two forms meet, and mix a half step of the ground colour back into every underside. Keep speculars to one or two in the whole canvas and drop detail with distance: fewer clusters and less contrast far away. Nothing leaves the grid and no second light source ever appears.",
    },
    KnowledgeEntry {
        id: "color-harmony",
        title: "Hue relationships and color schemes",
        keywords: &[
            "配色方案",
            "色调",
            "色系",
            "邻近色",
            "对比色",
            "冷暖",
            "互补色",
            "harmony",
            "hue scheme",
            "complementary",
            "analogous",
            "color scheme",
        ],
        body: "Pick the hue relationship before the first pixel: analogous (a 30-60 degree band) reads calm and unified, complementary (opposite hues) reads punchy but needs one side clearly dominant, and a triad reads busy unless two thirds of the canvas is neutral. Warm hues advance and cool hues recede, so spend the warm accent on the focal point and keep the cool hues in the background. One hue family should own more than half the canvas.",
    },
    KnowledgeEntry {
        id: "canvas-scale",
        title: "Matching the drawing to the canvas size",
        keywords: &[
            "画布大小",
            "画布尺寸",
            "画布多大",
            "尺寸",
            "分辨率",
            "canvas size",
            "resolution",
            "grid size",
            "how big",
        ],
        body: "Read width and height from the canvas context and derive EVERY coordinate from them - a script with literal constants tuned for 64px blasts off the edge of a 24px canvas. Under 16 pixels the silhouette is the whole picture and outlines hurt; at 32-64 spend it on two or three signature details plus a 3-step ramp; at 96 and above loops and math pay for themselves and you can carry 4-5 ramp steps per material. Use canvas.scale(k) for a 64px-design constant, canvas.grid(cols, rows) for a cell grid, and canvas.cx / canvas.cy instead of a hand-computed centre.",
    },
    KnowledgeEntry {
        id: "quadruped",
        title: "Quadrupeds: cats, dogs and four-legged bodies",
        keywords: &[
            "四足", "走兽", "兽", "猛兽", "猫", "狗", "兔", "狐狸", "狼", "熊", "鹿",
            "quadruped", "beast", "four legs", "cat", "dog", "fox", "wolf", "bear", "deer",
            "rabbit",
        ],
        body: "A side-view quadruped is ONE body mass plus four separate legs: two verticals under the shoulder, two under the hip, all four landing on the same ground line. The spine sags slightly in the middle, the chest sits higher than the belly, and the head rides level with the back rather than above it. Keep a visible gap between the near and far pair so the stance reads three-dimensional, shade the far legs a step darker, and give the tail its own curve with a phase lag.",
    },
    KnowledgeEntry {
        id: "cat",
        title: "Cats and small felines",
        keywords: &[
            "猫", "猫咪", "橘猫", "小猫", "狸花猫", "黑猫", "白猫",
            "cat", "cats", "kitten", "kitty", "tabby", "feline",
        ],
        body: "A cat is round, never angular: the head is close to a circle with two triangles on top, the body an ellipse about three heads long, and the legs short enough that the belly nearly clears the ground. Ears take a darker inner triangle with a light rim on the lit side; the muzzle reads as a lighter wedge under the nose; whiskers are two or three single pixels, not lines. Stripes run along the body long axis in two or three tapering bands, and the paws take a lighter pad plus a dark contact shadow where they land.",
    },
    KnowledgeEntry {
        id: "dog",
        title: "Dogs and canines",
        keywords: &["狗", "犬", "小狗", "柴犬", "dog", "dogs", "puppy", "hound", "canine"],
        body: "Read a dog by the muzzle and the ear: a long squared snout, a nostril dot at its tip, and one ear either flopped past the jaw or perked as a triangle. The chest is deeper than a cat, the legs longer, and the tail a straight or gently curved taper whose tip swings to read a wag. Keep the neck line straight from shoulder to skull - a curved back reads as a wolf or a hound instead.",
    },
    KnowledgeEntry {
        id: "horse",
        title: "Horses and long-legged runners",
        keywords: &["马", "马匹", "骏马", "horse", "horses", "pony", "stallion"],
        body: "A horse is leg: the body mass is short and deep, the neck arches up to a small head, and the four legs are about two thirds of the total height with a joint at mid-length. Hooves read as a single dark block at the foot, the mane as a row of small triangles along the neck crest, and the tail as a long tapering mass from the rump. Below 32 pixels keep each leg to two straight lines with a knee bump and let the motion live in the hooves.",
    },
    KnowledgeEntry {
        id: "bird",
        title: "Birds and wings",
        keywords: &[
            "鸟", "飞鸟", "小鸟", "麻雀", "鹰", "乌鸦", "翅膀", "羽",
            "bird", "birds", "wing", "wings", "feather", "eagle", "crow", "sparrow",
        ],
        body: "A bird in profile is a teardrop body with a triangular beak at the front and a fan of tail feathers at the back, and the eye sits high and forward close to the beak. A folded wing reads as two or three overlapping bands along the body with the longest feather on the bottom; a spread wing is a long tapering blade with a notched tip. Legs are two thin lines to a three-toed foot, and a perched bird is mostly silhouette plus one belly shade.",
    },
    KnowledgeEntry {
        id: "fish",
        title: "Fish and aquatic creatures",
        keywords: &["鱼", "小鱼", "金鱼", "鲨鱼", "fish", "fishes", "shark", "goldfish", "whale"],
        body: "A fish is a lens shape: pointed at the nose, widest just behind the head, pinched at the tail stalk. The tail fin is a triangle whose fluke follows the current, dorsal and ventral fins are small triangles on the top and bottom edge, and the gill line is a single curved stroke just behind the head. Scales read as two or three rows of a repeating arc near the back, not over the whole body; the belly is always a lighter step than the back.",
    },
    KnowledgeEntry {
        id: "insect",
        title: "Insects, spiders and small creatures",
        keywords: &[
            "虫", "昆虫", "蚂蚁", "蜜蜂", "蝴蝶", "蜘蛛", "蜗牛", "甲虫",
            "insect", "bug", "ant", "bee", "butterfly", "spider", "snail", "beetle",
        ],
        body: "An insect is three clear masses - head, thorax, abdomen - joined by narrow segments, with six thin legs in two pairs and two antennae. Butterfly and moth wings are two large overlapping blades with a repeating spot or band pattern; a beetle is a single rounded shell split by one centre line. Keep the segments readable as separate clusters at even 12 pixels, and shade the shell with a hard specular since it is chitin, not fur.",
    },
    KnowledgeEntry {
        id: "fur",
        title: "Fur, feathers and hair masses",
        keywords: &[
            "毛发", "皮毛", "绒毛", "羽毛", "头发", "鬃毛",
            "fur", "hair", "pelt", "fluff", "mane", "down",
        ],
        body: "Hair is a mass first and strands second: settle the whole silhouette of the mass in one fill, then break ONLY its outer edge with two or three pixel notches that follow the flow direction. Never draw individual strands - they read as noise and cost a loop per hair. Give the mass one ramp and let a rim light trace the lit side of its contour; let the shape cross the body outline to sell volume.",
    },
    KnowledgeEntry {
        id: "pig-boar",
        title: "Pigs, boars and barrel bodies",
        keywords: &[
            "猪",
            "小猪",
            "母猪",
            "野猪",
            "山猪",
            "豚",
            "pig",
            "pigs",
            "piglet",
            "boar",
            "hog",
            "swine",
        ],
        body: "A pig is a barrel with a wedge on the front: one deep ellipse, wide at the shoulder and pinched at the rump, carried on four short straight legs that plant on one ground line. The head reads as a snout disc plus two ear triangles that flop toward the eyes, and the snout itself is a lighter oval with two nostril dots. A boar is the same body angularised - a higher shoulder hump, a squarer face and two tusks rising from the lower jaw. Shade the body in long horizontal bands so the barrel reads as round rather than as a rectangle, keep the belly nearly scraping the ground, and give the tail a tight curl.",
    },
    KnowledgeEntry {
        id: "cow-buffalo",
        title: "Cattle, oxen and horned grazers",
        keywords: &[
            "牛",
            "奶牛",
            "公牛",
            "水牛",
            "黄牛",
            "耕牛",
            "牦牛",
            "犀牛",
            "cow",
            "cows",
            "cattle",
            "ox",
            "bull",
            "buffalo",
            "bison",
            "yak",
            "rhino",
            "rhinoceros",
            "steer",
            "calf",
        ],
        body: "Cattle read from the barrel plus the head line: a deep rounded body, a straight back, a long face with a wide wet muzzle, and two ears set wide and flat. Horns are the silhouette signal - two curved tapering blades from the top corners of the skull, one-pixel strokes with a bright lit edge on the upper side. A buffalo or bison trades the long face for a heavy shoulder hump and a beard mass under the jaw; a rhino trades horns for one or two nose horns plus folded skin over the neck. Udder and hooves are the only ventral detail worth pixels, and the tail is a thin line to a tuft.",
    },
    KnowledgeEntry {
        id: "sheep-goat",
        title: "Sheep, goats and wool masses",
        keywords: &[
            "羊",
            "绵羊",
            "山羊",
            "羔羊",
            "羊群",
            "羊驼",
            "sheep",
            "lamb",
            "lambs",
            "goat",
            "goats",
            "ram",
            "ewe",
            "alpaca",
            "llama",
            "wool",
            "flock",
        ],
        body: "A sheep is a cloud on four pegs: settle the wool as ONE cluster of overlapping rounded lobes in two or three sizes and let the dark face and legs read against it - shading inside the wool is a waste, because its value lives entirely in the silhouette edge. A goat is the angular cousin: a straight nose line, a beard mass under the jaw, two horns swept back in parallel ridges and ears half-flopped. An alpaca or llama is the same wool mass on a long neck, taller than it is deep. Hooves are two dark blocks on the ground line and the tail a short nub.",
    },
    KnowledgeEntry {
        id: "chicken-fowl",
        title: "Chickens, ducks and small fowl",
        keywords: &[
            "鸡",
            "公鸡",
            "母鸡",
            "小鸡",
            "鸭",
            "鸭子",
            "鹅",
            "家禽",
            "chicken",
            "chickens",
            "hen",
            "rooster",
            "chick",
            "poultry",
            "duck",
            "ducks",
            "duckling",
            "goose",
            "geese",
            "fowl",
        ],
        body: "A fowl reads from the beak, the comb and the tail in profile: a rounded body carried low, a small head with a triangular beak, and a tail of two or three long arcs that fan up in a rooster and hang down in a hen. The comb and wattle are the signature coloured accents - two or three notches along the skull line and one small drop under the beak. A duck is the same body lengthened, with a flat squared bill, a higher chest line on the water and a curled tail feather. Legs are two thin lines to forward-pointing toes, and at small sizes the whole subject is silhouette plus one belly shade plus the comb accent.",
    },
    KnowledgeEntry {
        id: "dragon",
        title: "Dragons and winged serpents",
        keywords: &[
            "龙",
            "巨龙",
            "飞龙",
            "神龙",
            "东方龙",
            "龙族",
            "dragon",
            "dragons",
            "wyvern",
            "drake",
            "whelp",
        ],
        body: "A dragon is a quadruped body plus a serpentine neck plus one pair of bat wings, and the three masses must read separately: keep the body with four legs under it, draw the neck as a tapering S-curve that carries its own width, and spread the wing from three or four finger bones holding membrane. Membrane takes a translucent mid value with bright ribs and a dark line along each bone, never a flat fill. Horns sweep back from the skull in two or three sizes, the tail is long and tapering with small spikes, and the silhouette lives on the wing spread and the neck curve. Shade the body as a scaled quadruped with a hard specular - it is scale, not fur.",
    },
    KnowledgeEntry {
        id: "reptile-amphibian",
        title: "Snakes, turtles, frogs and cold blood",
        keywords: &[
            "蛇",
            "毒蛇",
            "蟒蛇",
            "青蛇",
            "龟",
            "乌龟",
            "海龟",
            "甲鱼",
            "蛙",
            "青蛙",
            "蟾蜍",
            "蜥蜴",
            "壁虎",
            "鳄鱼",
            "蝾螈",
            "snake",
            "snakes",
            "serpent",
            "viper",
            "python",
            "cobra",
            "turtle",
            "tortoise",
            "frog",
            "frogs",
            "toad",
            "lizard",
            "gecko",
            "crocodile",
            "alligator",
            "newt",
            "salamander",
        ],
        body: "Cold-blooded bodies read from the belly line: a snake is a tapering S-curve whose head is barely wider than the neck, with belly scales alternating two values in a regular checkerboard down its length; a turtle is a domed shell polygon of two or three plate tones with a small head and four paddles poking out, the shell always lighter on top than under; a frog is a wide low body with two huge eyes riding the top line and each hind leg folded as one bent shape; lizards and crocodiles run low with legs splayed wide and a long tapering tail whose ridges read as a row of small triangles. Wet skin takes one bright specular dot on the crown and one along the backbone.",
    },
    KnowledgeEntry {
        id: "fantasy-beast",
        title: "Fantasy creatures, monsters and undead",
        keywords: &[
            "史莱姆",
            "魔物",
            "怪物",
            "恶魔",
            "魔鬼",
            "小鬼",
            "兽人",
            "半兽人",
            "哥布林",
            "地精",
            "幽灵",
            "鬼魂",
            "亡灵",
            "骷髅",
            "骨架",
            "僵尸",
            "丧尸",
            "蝙蝠",
            "老鼠",
            "slime",
            "blob",
            "monster",
            "monsters",
            "demon",
            "devil",
            "imp",
            "goblin",
            "orc",
            "ghost",
            "spirit",
            "skeleton",
            "skull",
            "undead",
            "zombie",
            "bat",
            "rat",
            "mouse",
            "worm",
        ],
        body: "A fantasy subject reads from one invented law stated once and kept: a slime is a domed gel mass with a two-pixel highlight on the crown, two dark eyes and a flat contact shadow, and it takes no hard outline because gel has none; a skeleton is segments plus joints - bones as 1px-outlined tubes, a ribcage of three or four arcs, the skull placed last as the brightest mass in the drawing; a ghost is a tapered sheet whose bottom edge breaks into two or three wavy notches, its eyes the only dark; orcs and goblins are hunched humanoids with a heavy jaw, ears dragged past the chin and one armour accent; demons take the horn set and a tail that reads as its own curve. Whatever the law, it gets ONE light direction, and the silhouette rather than the anatomy does the read.",
    },
    KnowledgeEntry {
        id: "blade",
        title: "Blades, weapons and held tools",
        keywords: &[
            "剑", "刀", "匕首", "斧", "斧头", "锤", "长矛", "法杖", "武器", "兵器",
            "sword", "blade", "dagger", "knife", "axe", "hammer", "spear", "staff", "weapon",
        ],
        body: "Draw a weapon along its long axis as three separate masses - blade, guard, grip - and never let them share one fill. The blade is a long tapering quad with a lighter centre line and a bright edge on the lit side; the guard and pommel are solid blocks that read as the weightiest part; the grip is darker than the blade and set on a diagonal so the hand finds it. Metal takes the sharpest contrast on the sheet: a thin specular highlight no wider than one pixel and a hard dark core shadow.",
    },
    KnowledgeEntry {
        id: "armor",
        title: "Shields, armor and heraldry",
        keywords: &[
            "盾", "盾牌", "铠甲", "盔甲", "头盔", "徽章", "纹章",
            "shield", "armor", "armour", "helmet", "crest", "heraldry", "emblem",
        ],
        body: "A shield is one silhouette with a charged centre: draw the outline shape first, then place the emblem as a smaller mass inside it with clear margin. Metal armor reads as overlapping plates - each plate takes its own highlight and shadow along the same light - and a helmet is a dome plus a visor slot. Keep the rim one step lighter than the face so the silhouette survives against a busy background.",
    },
    KnowledgeEntry {
        id: "potion",
        title: "Potions, bottles and glass",
        keywords: &[
            "药水", "瓶子", "玻璃", "水晶", "水杯", "容器", "烧瓶",
            "potion", "bottle", "flask", "glass", "vial", "jar", "goblet",
        ],
        body: "Glass reads by what it does to the shape behind it plus three marks: a narrow highlight on the lit side, a dark contact line along the liquid surface, and a bright rim where the body turns away. A potion is a neck, a shoulder and a body; the cork is a separate small mass. Liquid fills only the lower two thirds and takes its own brighter ramp, and a glow inside a bottle is built from the outside in with the core painted last.",
    },
    KnowledgeEntry {
        id: "container",
        title: "Chests, crates and boxes",
        keywords: &[
            "宝箱", "箱子", "木箱", "盒子", "背包", "袋子", "柜子",
            "chest", "crate", "box", "barrel", "sack", "bag", "backpack",
        ],
        body: "A container is a lid plus a body plus a lock, three masses stacked on one vertical axis, and the lid overhangs the body slightly so the joint reads as a line. Wood takes its grain direction along the long axis and a soft core shadow; iron bands are horizontal wraps that catch one highlight. The lock or clasp is the focal point - it takes the brightest highlight and the darkest shadow in the whole object.",
    },
    KnowledgeEntry {
        id: "treasure",
        title: "Coins, gems and treasure",
        keywords: &[
            "金币", "银币", "宝石", "钻石", "珠宝", "宝藏", "钱",
            "coin", "coins", "gem", "gems", "jewel", "diamond", "treasure", "gold",
        ],
        body: "A gem is a flat-topped shape with facets: a bright top facet, a mid tone on one side, and the darkest step on the opposite facet, all sharing one light. A coin is an ellipse with a raised rim and a symbol in the middle, and its value reads from the rim being one step brighter than the face. Give treasure one hot specular and let the darkest dark of the scene sit directly beside it so the sparkle has contrast to work against.",
    },
    KnowledgeEntry {
        id: "small-object",
        title: "Keys, torches, books and handheld props",
        keywords: &[
            "钥匙", "火把", "火炬", "书本", "书", "卷轴", "灯笼", "灯", "蜡烛", "号角",
            "key", "torch", "Lantern", "book", "scroll", "candle", "lamp", "horn",
        ],
        body: "Small props read from one iconic silhouette plus one telling detail: a key is a ring plus a shaft plus one tooth, a book is a cover plus a visible page block on one edge, and a torch is a shaft plus a flame mass that is wider than the shaft. Fire and light sources are drawn from the outside in - tint the surrounding air two steps toward the flame colour first, then place the hot core last. Keep the prop centred and let its shadow fall in one direction.",
    },
    KnowledgeEntry {
        id: "interior",
        title: "Rooms, furniture and household props",
        keywords: &[
            "家具",
            "桌子",
            "茶几",
            "椅子",
            "凳子",
            "床",
            "柜子",
            "橱柜",
            "衣柜",
            "书架",
            "房间",
            "室内",
            "屋内",
            "卧室",
            "客厅",
            "厨房",
            "饭厅",
            "壁炉",
            "灶台",
            "民居",
            "内饰",
            "interior",
            "indoors",
            "room",
            "furniture",
            "table",
            "chair",
            "stool",
            "bed",
            "shelf",
            "shelves",
            "cabinet",
            "cupboard",
            "wardrobe",
            "fireplace",
            "hearth",
            "stove",
            "oven",
            "bedroom",
            "kitchen",
        ],
        body: "An interior is boxes on one ground line under one light: settle the floor, the back wall and every furniture mass as flat filled rectangles first, then cut openings and details as smaller masses that touch the wall edge only at the bottom. One eye-level line carries furniture tops, seat height and shelves - a second level breaks the whole room. Wood takes its grain along the long axis and a dark contact band where it meets the floor; cloth beds, curtains and cushions fold into wide mid tones with no specular at all. Keep the darkest value inside the room and let the window or the lamp be the single brightest accent.",
    },
    KnowledgeEntry {
        id: "fabric-banner",
        title: "Flags, banners and hanging cloth",
        keywords: &[
            "旗帜",
            "旗子",
            "军旗",
            "国旗",
            "彩旗",
            "横幅",
            "标语",
            "帷幔",
            "布幔",
            "挂布",
            "桌布",
            "飘带",
            "绶带",
            "banner",
            "flag",
            "pennant",
            "standard",
            "curtain",
            "drape",
            "tapestry",
        ],
        body: "Hanging cloth reads from its folds, and a fold is two long parallel edges around a dark core - never a smudge: settle the whole cloth as one silhouette first, then cut three or four spaced vertical fold lines, each taking the same law (lit face, one mid, dark core) so the eye reads a ripple rather than random stripes. A flag carries its charge as a smaller mass with clear margin near the hoist side, and the free edge takes the wider ripple. Cloth takes no specular, never touches the silhouette of whatever it hangs from, and keeps every fold edge near-vertical - only a named wind tilts them.",
    },
    KnowledgeEntry {
        id: "tree",
        title: "Trees, trunks and wood",
        keywords: &[
            "树", "树木", "大树", "树干", "松树", "棕榈", "枯木",
            "tree", "trees", "trunk", "pine", "palm", "log", "stump",
        ],
        body: "A tree is a trunk plus a canopy mass: the trunk tapers as it rises, splits into two or three limbs that reach INTO the canopy, and takes a vertical grain. The canopy is a cluster of overlapping lobes of two or three sizes with only the outer edge varied. Conifers are stacked triangles with a flat bottom; broadleaf trees are round masses with notches. Keep one light direction on every lobe and let branch tips read as 2x2 clusters.",
    },
    KnowledgeEntry {
        id: "rock",
        title: "Rocks, stones and crystals",
        keywords: &[
            "石头", "岩石", "石块", "鹅卵石", "水晶", "矿石", "悬崖", "冰块",
            "rock", "rocks", "stone", "stones", "boulder", "crystal", "ore", "cliff", "ice",
        ],
        body: "A rock is a closed polygon with no parallel sides and no sharp 90 degree corners; shade it with two or three flat facets meeting at one bright top edge. Stones on the ground belong to one size family with the smaller ones clustered near the large one. Ice and crystal are the exception: they take a translucent mid tone with bright interior lines rather than a solid shadow, and their edges are lighter than the interior.",
    },
    KnowledgeEntry {
        id: "building",
        title: "Buildings, houses and architecture",
        keywords: &[
            "房子", "房屋", "建筑", "屋子", "塔楼", "城墙", "桥", "门", "窗",
            "house", "building", "tower", "wall", "bridge", "door", "window", "hut",
        ],
        body: "Architecture reads from one-box massing plus roof plus openings: settle the volumes as flat filled shapes first, then cut windows and doors as darker rectangles that never touch the wall edge except at the bottom. Roofs are the lightest plane because they face the sky, and walls take their shade from the same light direction. Keep every vertical exactly vertical and every roofline on one shared angle - mixed angles break the whole structure.",
    },
    KnowledgeEntry {
        id: "food",
        title: "Food, fruit and consumables",
        keywords: &[
            "食物", "水果", "苹果", "面包", "肉", "蘑菇", "料理", "果实",
            "food", "fruit", "apple", "bread", "meat", "mushroom", "berry", "pie",
        ],
        body: "Food reads from shape plus one appetite cue: a round fruit takes a bright specular and a small stem, bread reads as a domed top with two or three slash marks, and meat is a rounded mass with a bone tip. Mushrooms are a cap plus a stem with the cap the lighter plane. Keep food within one warm hue family and give it one soft shadow under it so it sits on the surface instead of floating.",
    },
    KnowledgeEntry {
        id: "vehicle",
        title: "Vehicles, ships and mounts",
        keywords: &[
            "车", "马车", "船", "飞船", "坦克", "载具", "飞行器",
            "vehicle", "cart", "wagon", "ship", "boat", "tank", "airship", "mount",
        ],
        body: "A vehicle is a chassis mass plus a propulsion mass plus one or two wheels or a hull line, and its wheels and windows share one line across the body. A ship reads from the hull silhouette plus a mast or smokestack plus a flag; a cart reads from two wheels plus a bed plus a shaft. Shade the chassis in two long planes rather than per-part, and keep any text or emblem level with the body, not rotated.",
    },
];

/// 检出这一轮用得上的知识条目。明文命中加权：词越长越算数，
/// 得分相同时保持库内顺序，所以同样的提问永远得到同样的结果。
/// 命中判据走 `terms`：拉丁触发词整词算，中文子串算。
pub fn retrieve(query: &str, limit: usize) -> Vec<&'static KnowledgeEntry> {
    let needle = query.to_lowercase();
    let mut scored: Vec<(usize, &KnowledgeEntry)> = ENTRIES
        .iter()
        .filter_map(|entry| {
            let score = entry
                .keywords
                .iter()
                .map(|kw| {
                    let lowered = kw.to_lowercase();
                    if super::terms::find_lower(&needle, &lowered).is_some() {
                        kw.chars().count()
                    } else {
                        0
                    }
                })
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
    render(retrieve(query, limit), budget)
}

/// 按条目 id 列表摆知识段。分流在那里定的 id 列表就是真相——「这一轮改画」
/// 会把 `refine` 硬塞进去，那句话就算一个字都没提到技法也照样带上路。
/// 认不出的 id 直接跳过：宁可少一条，也不要在系统提示词里留一行空标题。
pub fn section_from_ids(ids: &[String], budget: usize) -> String {
    let hits: Vec<&'static KnowledgeEntry> = ids
        .iter()
        .filter_map(|id| ENTRIES.iter().find(|entry| entry.id == id))
        .collect();
    render(hits, budget)
}

fn render(hits: Vec<&'static KnowledgeEntry>, budget: usize) -> String {
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

    /// 奔跑、飞行的循环能不能像，全看有没有把步态相位表这一路带出去。
    /// 半个字不提 gaits 的提问命中不了它，正是命中不了才对。
    #[test]
    fn a_run_request_surfaces_the_gait_phase_table() {
        for query in ["画5帧橘猫奔跑", "a five frame run cycle of a cat"] {
            let named: Vec<&str> = retrieve(query, 4).iter().map(|e| e.id).collect();
            assert!(
                named.contains(&"locomotion"),
                "'{query}' 没带上步态相位表：{named:?}"
            );
        }
        assert!(!retrieve("画一只静止的杯子", 4)
            .iter()
            .map(|e| e.id)
            .collect::<Vec<_>>()
            .contains(&"locomotion"));
    }

    #[test]
    fn nothing_relevant_returns_nothing() {
        assert!(retrieve("你好", 4).is_empty());
        assert!(prompt_section("你好", 4, DEFAULT_BUDGET).is_empty());
    }

    #[test]
    fn a_latin_trigger_word_never_rides_inside_a_bigger_word() {
        // ant 钻进 elephant、hair 钻进 chair、cat 和 log 钻进 catalog、
        // ship 钻进 spaceship、ore 钻进 more。这些过去全都命中过，
        // 命中一条错的，系统提示词里就多一段斩钉截铁的错手艺。
        for query in [
            "draw an elephant",
            "a wooden chair",
            "a catalog page",
            "a spaceship",
            "make it more detailed",
        ] {
            let named: Vec<&str> = retrieve(query, 4).iter().map(|e| e.id).collect();
            for wrong in ["insect", "fur", "cat", "foliage", "vehicle", "rock"] {
                assert!(
                    !named.contains(&wrong),
                    "'{query}' 命中了 {wrong}: {named:?}"
                );
            }
        }
        // 整词和复数照旧命中，别把检索改死了。
        assert!(retrieve("draw a cat", 4).iter().any(|e| e.id == "cat"));
        assert!(retrieve("two cats", 4).iter().any(|e| e.id == "cat"));
        assert!(retrieve("a pirate ship", 4)
            .iter()
            .any(|e| e.id == "vehicle"));
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
        // 「优化一下细节」是这个工具里最高频的追问：用户看完第一版就让改细节、
        // 写实一点。这类话里一个画种名词都没有，检索不到东西就等于每轮细化
        // 都在裸画——而细化恰恰是最不能从 clear() 开始的轮次。
        assert_eq!(retrieve("优化一下细节", 4)[0].id, "refine");
        assert_eq!(retrieve("细节再写实一点", 4)[0].id, "refine");
        assert!(retrieve("把颜色再丰富一些", 4)
            .iter()
            .any(|e| e.id == "refine"));

        // 用户嘴里除了技法，说得最多的是「画只猫」「来把剑」。生物和器物两组检索
        // 不到，模型就只能凭训练语料里的平均认知画——猫画成四条腿的毯子、剑画成
        // 一根发光的棍子，都是这么来的。
        assert_eq!(retrieve("画一只坐着的小猫", 4)[0].id, "cat");
        assert_eq!(retrieve("画一把长剑", 4)[0].id, "blade");
        assert_eq!(retrieve("一棵松树", 4)[0].id, "tree");
        let two = retrieve("宝箱和金币", 4);
        let named: Vec<&str> = two.iter().map(|e| e.id).collect();
        assert!(named.contains(&"container"), "{named:?}");
        assert!(named.contains(&"treasure"), "{named:?}");
        // 按 id 出段：分流里硬塞的 refine 也要能落到提示词里。
        let forced = section_from_ids(&["refine".to_string(), "cat".to_string()], DEFAULT_BUDGET);
        assert!(forced.contains("Refinement and realism"), "{forced}");
        assert!(forced.contains("Cats and small felines"), "{forced}");
        assert!(section_from_ids(&["no-such-entry".to_string()], DEFAULT_BUDGET).is_empty());

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

    /// 写实度那组新条目要真能被检索到，而且拉丁触发词不许误命中。
    /// 「生成的不够写实」是最高频的追评，这几条检索不到，就等于每轮细化
    /// 都在裸画——而细化恰恰最吃材质响应和取带规则。
    #[test]
    fn the_realism_entries_surface_and_stay_word_safe() {
        assert!(
            retrieve("画一只写实的橘猫，皮毛要透光", 4)
                .iter()
                .any(|e| e.id == "subsurface"),
            "透光该命中 subsurface"
        );
        assert!(
            retrieve("金属高光再亮一点", 4)
                .iter()
                .any(|e| e.id == "specular"),
            "高光该命中 specular"
        );
        assert!(
            retrieve("立体感不够，明暗交界太生硬", 4)
                .iter()
                .any(|e| e.id == "form-shading"),
            "立体感该命中 form-shading"
        );
        // 远景/空气透视走 perspective，不再因为没有一个词叫「写实」就整组隐身。
        assert!(
            retrieve("加点空气透视，远景虚一点", 4)
                .iter()
                .any(|e| e.id == "perspective"),
            "空气透视该命中 perspective"
        );
        // 拉丁触发词整词算：thinking / something 里的 thin 不算命中。
        for query in [
            "thinking about the shading",
            "something is off",
            "within reach",
        ] {
            let named: Vec<&str> = retrieve(query, 4).iter().map(|e| e.id).collect();
            assert!(
                !named.contains(&"subsurface"),
                "'{query}' 误命中 subsurface: {named:?}"
            );
        }
        // 一次四条长条目全命中也压得住预算：宁可少带一条，不把上下文吃干。
        let text = prompt_section("写实 材质 透光 光影 立体 远景", 4, DEFAULT_BUDGET);
        assert!(
            text.chars().count() <= DEFAULT_BUDGET,
            "{}",
            text.chars().count()
        );
        assert!(text.contains("Material logic"), "{text}");
    }

    /// 曲线和写实两条新条目要真能被检索到——「平滑一点」「再写实一点」是
    /// 追评里最高频的两句，检索不到就等于这两句白说。
    #[test]
    fn the_curve_and_realism_entries_surface() {
        assert_eq!(retrieve("尾巴画条平滑的曲线", 4)[0].id, "smooth-curves");
        assert_eq!(
            retrieve("a smooth curve for the tail", 4)[0].id,
            "smooth-curves"
        );
        assert!(retrieve("整体再写实一点", 4)
            .iter()
            .any(|e| e.id == "realism"));
        assert!(retrieve("make it more photoreal", 4)
            .iter()
            .any(|e| e.id == "realism"));
    }

    /// 动物、器物、图标、帧排版这几组新条目要真能被原话捞出来。
    /// 用户嘴里最高频的就是「画只 X」和「画个图标」——这两类检索不到，
    /// 模型就按训练语料里的平均认知画，猫画成四条腿的毯子就是这么来的。
    #[test]
    fn the_animal_and_prop_entries_surface() {
        for (query, id) in [
            ("画一头在院子里散步的奶牛", "cow-buffalo"),
            ("画几只山羊站在悬崖上", "sheep-goat"),
            ("画一只打鸣的公鸡", "chicken-fowl"),
            ("画一条盘着的蛇", "reptile-amphibian"),
            ("画一只青蛙蹲在荷叶上", "reptile-amphibian"),
            ("画一只探头出来的野猪", "pig-boar"),
            ("画一条盘在山洞里的龙", "dragon"),
            ("画一只史莱姆怪物", "fantasy-beast"),
            ("画一个客厅的室内场景", "interior"),
            ("画一面挂在城墙上的旗帜", "fabric-banner"),
            ("给我画一个 app 图标", "icon"),
            ("画一棵松树", "tree"),
        ] {
            let hits = retrieve(query, 4);
            assert!(
                hits.iter().any(|e| e.id == id),
                "'{query}' 没捞出 {id}：{:?}",
                hits.iter().map(|e| e.id).collect::<Vec<_>>()
            );
        }
        // 序列帧那句话词最长，得压过其它命中排第一。
        assert_eq!(retrieve("把序列帧排列成一排", 4)[0].id, "sprite-sheet");
    }

    /// 新条目的拉丁触发词不许误命中，中文子串照旧认。
    /// 这条是门口的保安：加条目最容易顺手把一个常见英文词收进来，
    /// 之后每句带 chair 的画图题都被塞一段室内规矩。
    #[test]
    fn the_new_entries_stay_word_safe() {
        // 这几个全是「触发词只是某个英文词的半截」的陷阱：整词判定天生该拦下。
        for (query, wrong) in [
            ("a dragonfly over the pond", "dragon"),
            ("read the whole catalog", "pig-boar"),
            ("a pirated copy of it", "fantasy-beast"),
            ("a man with a long goatee", "sheep-goat"),
        ] {
            let named: Vec<&str> = retrieve(query, 4).iter().map(|e| e.id).collect();
            assert!(
                !named.contains(&wrong),
                "'{query}' 误命中 {wrong}：{named:?}"
            );
        }
        // 整词照旧命中，别把检索改死。
        assert!(retrieve("draw a wooden chair", 4)
            .iter()
            .any(|e| e.id == "interior"));
        assert!(retrieve("two goats on a hill", 4)
            .iter()
            .any(|e| e.id == "sheep-goat"));
        assert_eq!(retrieve("画一把长剑", 4)[0].id, "blade");
    }
}
