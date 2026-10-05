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

/// 检索预算：条目正文注入系统提示词的字符上限，防某一轮把上下文吃干。
///
/// 上限要压得住闲聊轮，又不能小到把「奔跑」「瓦片」这种真·技法连坐丢掉。
/// 一条「画 5 帧橘猫奔跑」会同时够到 `cat` + `walk-cycle` + `locomotion` +
/// `quadruped`，随知识库扩充，四条正文合计已接近四千四百字符，所以给到
/// 5200——够一整轮把姿态相位表、四足解剖和落地的行为准则一并带上，
/// 再多就该收手了。加了新条目、正文写长之后这条要跟着重算，否则被挤掉的
/// 恰是检索分最高的那几条。
pub const DEFAULT_BUDGET: usize = 5200;

/// 行为准则三条：规格锁、收工自检、别画成程序化假图案。
///
/// 它们不是「提到这个词才给」的技法，是动笔的每一轮都要守住的护栏，所以不占
/// 检索那 `DEFAULT_LIMIT` 个名额——让护栏插队的话，「画 5 帧奔跑」那一轮就
/// 该拿不到步态相位表了。护栏自己一段下发，见 `discipline_section`。
pub const DISCIPLINE_IDS: [&str; 3] = ["spec-lock", "quality-gate", "fake-patterns"];

/// 行为准则单独一份字符预算。三条正文加起来约两千二百字符，2600 够整段带走，
/// 又不至于把系统提示词顶到别处去。
pub const DISCIPLINE_BUDGET: usize = 2600;

/// 知识库。按「技法 -> 素材 -> 生物 -> 器物 -> 场景 -> 工具词」排，同组相邻，
/// 模型扫起来快；检索同分时按这个顺序兜底，结果稳定可复现。
/// 生物和器物两组的条目是「怎么把一样东西画对」的要点：用户说「画只猫」时，
/// 只给上色规则是不够的——模型知道怎么铺色阶，照样能把猫画成四条腿的毯子。
pub const ENTRIES: &[KnowledgeEntry] = &[
    KnowledgeEntry {
        id: "walk-cycle",
        title: "Walk and run cycles",
        // 「行走图」必须在这里显式列一遍：它是用户最高频的原话，而
        // `rpmaker-sheet` 也认这三个字。两边都命中时按词长加权打平，
        // 由库内顺序兜底——本条目在前，所以纯步态提问仍归步态。
        keywords: &["行走", "行走图", "走路", "步态", "奔跑", "跑动", "walk", "walking", "walk cycle", "run cycle", "gait"],
        body: "Drive every limb from ONE phase variable. A 4-beat walk spreads the four legs over 0 / 0.25 / 0.5 / 0.75; a trot uses two diagonal pairs a half cycle apart. Lift a foot only while it swings forward - lift = max(0, sin(2pi*phase)) - and keep it planted while it travels back. The body bobs at twice the step frequency and the head counter-bobs a little. Close the loop: the last frame has to flow back into the first. An EIGHT-frame walk is the generous default because it can hold all four classic cartoon poses - contact, down, pass, up - twice per cycle; short-limbed chibi sprites and tiny canvases do fine with four to six, while long full-length limbs need the eight or the motion reads as a shuffle. For a believable realistic walk, drive the eight keys from real footage rather than feel: screen-record or step through a walk, cut it to eight keys, and keep the four phases - contact (heel lands, limbs at the extreme, body lowest), down (foot flattens, body still low), pass (legs cross, body highest) and swing (lead leg reaches forward off the ground) - then mirror those four for the second half. The head should travel a TRIANGLE wave, not a sine: the pass frame rises faster than the contact frame drops, and uniform sine motion reads mechanical. Arms swing opposite to the legs and opposite-side limbs share one momentum, so the right arm's swing tracks the left leg. There is no need to redraw every direction - mirror the side view for the other side and re-draw only the asymmetric details.",
    },
    KnowledgeEntry {
        id: "anim-catalog",
        title: "Action catalogue and animation specs",
        keywords: &[
            "动作表", "动作列表", "动作库", "技能", "攻击", "施法", "死亡", "受伤", "动作帧数", "循环吗",
            "action list", "action catalogue", "action catalog", "animation set", "animation spec",
            "attack", "cast", "hurt", "die", "interact", "crouch", "roll", "swim",
        ],
        body: "Take frame counts and rates from the action catalogue, not from feel: idle 2-4 frames at 6-8 fps, walk 4-6 at 8-12, run 4-6 at 12-16, jump 4-6, fall 2-4, attack 3-6 at 12-16, hurt 2-3, die 4-6 at 6-10, cast 4-8, interact 2-4, climb 4-6, crouch 2-3, roll 4-6, swim 4-6. Slow actions take fewer frames held longer; fast actions take more frames held briefly. One action per row in the strip, every frame the same canvas size, the subject anchored inside one shared box so only the moving parts shift. Name them <asset>_<action>_<number> and state for each action whether it loops or plays once.",
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
        body: "Name the gait first, then drive every limb from that gait's phase table. WALK: legs at 0 / 0.25 / 0.5 / 0.75 with three feet down at any moment. TROT: two diagonal pairs half a cycle apart plus a one-frame suspension between strides. CANTER: near hind, then the diagonal pair, then the leading foreleg (0 / 0.2 / 0.4 / 0.6). GALLOP: the near pair lands, the far pair a beat later, then a long pose with all four legs gathered - contact 0, gather 0.35, extend 0.6, suspension 0.75. A gallop reads as running only because that gathered suspension frame exists: skip it and the motion reads as a sped-up walk. Gait rules for all four: plant the foot while it sweeps back and lift it only on the forward swing (lift = max(0, sin(2*pi*phase))), and mirror the amplitude between the near and far pair or the far legs skate. FLIGHT: wing down at 0, catching the air 0.2-0.4, top of the upstroke 0.6, folded 0.8. HOPS and LEAPS squash the body to three quarters height at 0, stretch toward the target at 0.5, land and squash again at 1. An upright CHARACTER RUN is a walk with four edits, so derive it from the finished walk instead of animating it from scratch: lean the head and torso forward with the head slightly down, crank the stride and the bounce, and speed the playback up - the lean plus the down-tilt is what actually sells it.",
    },
    KnowledgeEntry {
        id: "run-and-gun",
        title: "Side-view run 'n gun: layering, jump and shoot-on-move",
        keywords: &[
            "跑射", "横版射击", "射击游戏角色", "持枪", "举枪", "开枪", "边跑边射", "移动射击",
            "跳跃动作", "落地动作", "受击", "掩体",
            "run and gun", "run 'n gun", "run-and-gun", "shooter", "contra", "gun sprite",
            "shoot while moving", "aiming", "jump animation", "landing", "gun arm",
        ],
        body: "A side-view shooter character is built as a STACK of shared parts, not as one animation per action. Draw the base dummy at 8x16 and settle walk and run first - they set the expression and the standard of fluidity everything else is cut from. Then cut LEGS and TORSO into separate layers so the legs keep cycling through walk, run and jump independently of the top half; the shooting pose is 3 frames reused as an UPPER-BODY overlay, stamped onto every frame of every animation that allows linear motion (walk, run, jump), and the shoulder anchoring and bounce must follow the leg layer underneath or the two halves tear apart. HOLDING the gun is the cheap version: delete the arms from a finished frame and redraw them around the weapon, since the shoulder position and the bounce are already correct. JUMP stays minimal for input response: one tucked pose is the floor, and an 'up' pose for the ascent plus a 'down' pose for the descent is the sweet spot - the apex swap doubles as a timing cue. NO anticipation crouch: it adds input delay, so the jump fires instantly. LAND recycles the crouch pose plus one extra frame with the head and arms dipped to sell the weight, and that dip is optional if you want no loss of momentum. Provide both male and female builds by re-sketching the dummy rather than recolouring it, and let long hair animate on run and jump - but not on run-and-shoot, where the rhythmic run bounce should own the silhouette.",
    },
    KnowledgeEntry {
        id: "shmup",
        title: "Shoot 'em up sprite and screen design",
        keywords: &[
            "射击游戏", "横版射击游戏", "纵版射击", "弹幕", "飞机", "战机", "飞船", "敌机",
            "僚机", "子弹", "弹幕游戏",
            "shmup", "shoot 'em up", "shoot em up", "bullet hell", "danmaku",
            "spaceship", "aircraft", "plane sprite", "star fighter",
        ],
        body: "In a shoot 'em up, readability beats realism. The player hitbox is a tiny circle kept at the same relative pixel across every animation frame; the player's own bullets and the enemy bodies get generous bounds so aiming always feels fair; enemy bullets get small centre hitboxes. The small player box plus the buffer is the whole feel of a tight dodge, and the player hitbox must never stick out past the sprite's own pixels. The player craft is usually about 48x32 and animated with the SAME trick for every movement option: one neutral level-flight frame plus two banked frames each way, driven by input press time - a tap shows the shallow roll briefly, a hold shows the full bank and stays there, and the release rolls back through the shallow position. Illustrate ROLL for vertical movement and keep the nose level: pitching distorts the pixel clusters and breaks the always-straight-forward firing line, which level design assumes. NEVER put inertia on player movement in a shmup - tap should move a smidge and settle, hold should move flat and fast, because inertia adds a handicap the level design has to fight. Give ground units cheap motion (moving treads, wobbling wheels, a one-pixel bob) and only spend extra frames on an air unit that actually changes altitude on screen. Detachable options, pods or escorts around the ship are not just firepower - they add screen presence, which is how the player keeps track of position while reading bullet patterns.",
    },
    KnowledgeEntry {
        id: "combat-idle",
        title: "Idle fighting stance and combat breathing",
        keywords: &[
            "待机", "待机动画", "战斗待机", "格斗站姿", "战斗姿势", "起手式", "备战", "呼吸",
            "晃动", "待机循环", "抱架",
            "idle stance", "fighting stance", "combat idle", "guard stance", "breathing idle",
        ],
        body: "A fighting idle is a bounce, not a still: the character holds the guard pose while every part rides a small up-down breath cycle anchored to the torso. Build an eight-frame loop - the figure rises 1px (legs straighten, knees shift in, shoulders, elbows and fists rise, the inner fist travelling the most), holds the top for one frame as the heels lift, then drops back through two frames of knee bend before the legs decompress - and time the rise slower than the fall (frames 1-4 at 100ms, 5-8 at 50ms) so gravity reads instead of a floating bob. A boxer's stance (hands guarding the face, neck tucked into the shoulders, knees softly bent) suits a punch-heavy character; a karate side-stance with a lower centre of gravity and a wider base suits a kicking one. Clothes and hair add sub-movement on top of the same bounce - fabric follows the body one frame late, the loose hem and hair lag behind - but keep the base forms solid and the folds minimal, because a jumbled silhouette in motion reads worse than a simple one.",
    },
    KnowledgeEntry {
        id: "melee-attacks",
        title: "Melee attack animation and weapon weight",
        keywords: &[
            "近战", "近战攻击", "挥砍", "劈砍", "出拳", "拳击", "直拳", "刺拳", "勾拳", "踢击",
            "回旋踢", "前踢", "攻击动画", "打击感", "挥剑", "重击", "大锤", "长矛",
            "melee", "punch", "jab", "cross punch", "kick", "round kick", "front kick",
            "sword swing", "attack animation", "hit stop",
        ],
        body: "A melee attack is four to six phases, and the weapon's weight is the whole story: anticipation (1 frame of wind-up - longer means heavier but adds input delay), smear (the fast travel, drawn as an elongated trail in simple colours so it stays legible), rebound (only when the weapon strikes the ground - one brief bounce), follow-through (full extension, held longer to sell commitment and weight), recover (one frame pulling back toward idle) and overshoot (the idle pose shifted 1px backwards, so every hit snaps back with energy). Never put a smear on the anticipation or recover frames - it muddies the forward strike. Frame timing in milliseconds, by weapon: short sword 100/50/50/50/100/50 (400ms total) for a fast, wide sweep with minimal delay; spear 200/50/50/50/150/50 (550ms) for a longer hold on the wind-up and extension that buys reach and commitment; hammer 250/50/50/50/300/100 (800ms), the slowest wind-up and recovery of the set, backed by screen shake and the rebound. Punches follow the same shape: a jab fires straight from the guard with no telegraph and stays fast enough to chain, while a cross loads the fists and swings the elbows out for one or two wind-up frames before the hips twist into it. Kicks chamber the knee first - a front kick snaps the leg out from a raised knee with the chamber only implied in the smear, and a round kick adds a load frame and a spring frame before the hip-driven snap. In an eight-direction set keep the weapon in the same hand in every direction, and bend the pose per direction so the hitbox stays balanced rather than copying one swing around the circle.",
    },
    KnowledgeEntry {
        id: "frame-timing",
        title: "Frame timing and animation rhythm",
        keywords: &["帧率", "时长", "动画", "动效", "补间", "frame duration", "timing", "fps", "tween", "animation"],
        body: "8-12 fps is the pixel-art norm. Hold key and contact poses longer and pass through in-betweens quickly; set each frame's duration instead of relying on a uniform rate, and keep the total loop divisible so the repeat is invisible. On small canvases, fewer frames with longer holds read better than many 60ms frames. Two anchor numbers worth starting from: 120 ms for idle and walk, 60 ms for run, shoot, land and dust - that is the classic 8-bit action split, and slowing the walk while the run stays fast is what makes the speed difference read.",
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
        id: "sheet-layouts",
        title: "Layout templates and character facings",
        keywords: &[
            "朝向", "四个方向", "四向", "八向", "侧视", "排版模板", "素材模板", "几行几帧",
            "sheet layout", "layout template", "facing", "facings", "direction", "directions",
            "4-direction", "8-direction", "side view", "side only", "rows", "tile sheet", "ui sheet",
        ],
        body: "Layout is a contract, not a look. Character facings: 4-direction for a top-down RPG (down/up/left/right, one row each), 8-direction for directional movers, side-only for a platformer (one row mirrored), single for a portrait or card. This tool holds ONE strip per canvas, so one action per session and each facing its own session - never stack two actions in one strip. Template rows: 4-direction, 4 rows of 3-4 frames; RPG Maker character sheet, 4 rows of 3 columns (VX/Ace/MV/MZ) or 4 rows of 4 columns (XP), one row per facing; RPG Maker battle charset, 5 rows of 4-6; platformer, 5 rows of 4-6. Tiles: one cell basic, a 3x3 nine-piece autotile or 5x3 (byte 47), animated N frames in a row. Items: one cell, or 4-8 frames for a pickup spin. UI: a 3-cell strip or a 3x3 nine-slice. Every cell the same size and the subject anchored in one shared box so only the moving parts shift.",
    },
    // RPG Maker 角色行走图。它和别的排版不一样的地方是：网格不是建议而是契约，
    // 引擎按行列号直接切图，列数错了整张图错位一行。触发词里带上各代引擎的
    // 简称——用户说的是「给 RM 用的」而不是「四行三列」。
    KnowledgeEntry {
        id: "rpmaker-sheet",
        title: "RPG Maker character sheets: 4 rows of 3 or 4",
        keywords: &[
            "rpg maker", "rpgmaker", "rmmv", "rmmz", "rmvx", "vx ace", "vxace", "rmxp", "rm2k", "rm 2000",
            "角色行走图", "人物行走图", "行走图素材", "四向行走图", "4行3列", "四行三列", "4行4列", "四行四列",
            "3列4行", "三列四行", "纸娃娃", "白膜", "捏人", "换装", "单角色文件",
            "character sheet", "walker", "walking sprite", "paper doll", "character generator", "charset",
        ],
        body: "An RPG Maker character sheet is a FIXED grid, not a strip: 4 rows top to bottom (down, left, right, up) and either 3 columns (VX Ace, MV, MZ) or 4 columns (XP). Cell size by generation: XP 32x32 so one character is 128x128; VX/Ace 32x32 so one character is 96x128; MV/MZ 48x48 so one character is 144x192, a full sheet is 576x384 holding 8 characters 4 across and 2 down, and a single-character file starts with $ and is just that 144x192 block. Frame roles inside a row are fixed: index 0 is the stand and must read as the exact middle between the two steps; a 3-column row uses index 1 and 2 as the step extremes and the engine plays 0-1-0-2, while a 4-column row cycles 0-1-2-3 with index 2 a real passing pose, never a copy of index 0. Every cell the same size, the feet on ONE ground line sitting 2-3 pixels above the cell bottom, the body centred left-right, and one pixel of transparent margin around every limb so neighbouring cells never fuse. The sheet shares ONE palette across all 8 characters because the engine has no per-character palette slot: quantise hard, dither instead of anti-aliasing, transparent background, and a 1-pixel dark outline is the only thing that keeps a 32 px character readable over a tile. The character walks in place - never scroll the ground under it.",
    },
    // 纸娃娃白膜。RGB（调色板）与描边都由用户或套装决定，这里管的是「先铺底稿」
    // 这一整套做法：底稿的部件分区就是将来换色的边界，边界不定下来，
    // 后面每一次上色都在赌轮廓会不会跑。
    KnowledgeEntry {
        id: "topdown",
        title: "Top-down and four-direction sprites",
        keywords: &[
            "俯视", "俯视角", "上帝视角", "顶视", "四方向", "4方向", "八方向", "8方向",
            "上下左右四个方向", "多方向", "朝向", "地图角色",
            "top down", "top-down", "topdown", "bird's eye", "birds eye", "overhead",
            "4-direction", "four direction", "8-direction", "eight direction", "overworld",
        ],
        body: "Draw the character ONCE facing down and derive the other three directions from it - never sketch each direction from nothing, or the head size and shoulder width drift. UP never shows a face: it is the back of the head plus hair, and any weapon or pack rides on the back and must not swap shoulders. LEFT and RIGHT are mirrors, so draw one, flip it, then re-draw any asymmetric detail (a strap, a scar, a held item) instead of leaving it flipped. The head turns and the shoulders turn with it, and the body stays the same height in every direction or the sprite bobs as the player walks. There is no foreshortening from overhead: depth is the sprite's vertical POSITION on the canvas, not its scale, so a character standing further away is drawn higher up, not smaller. An EIGHT-direction set is 36 unique frames for one action, so build it from a DUMMY first - animate the bare anatomy until the motion is right, then paint the costume on top - and respect asymmetric details across every direction instead of mirroring them. Cut the armour and equipment into their own layers over the base body so a weapon or shield can be swapped without redrawing the character, and dial the arm swing down once a heavy item is added: the equipment has weight, and the run taking that weight is what sells it. A sword swing is six frames timed 100, 50, 50, 50, 100, 50 ms - a slow wind-up and a slow recovery around four fast swing frames - and the shoulders lead the blade. Judge the loop at full play speed, not frame by frame: sub-pixel flicker and clusters that merge into noise only show up in motion, and the fix is usually to drop a colour or simplify the cluster rather than to redraw the silhouette.",
    },
    KnowledgeEntry {
        id: "paperdoll-base",
        title: "Paper-doll base (blank base) and swappable parts",
        keywords: &[
            "白膜", "纸娃娃", "纸人", "底稿", "底图", "部件", "分块", "分区", "换色区", "模板",
            "paper doll", "base mesh", "underdrawing", "swappable parts", "dress up", "parts",
        ],
        body: "Lay the blank base BEFORE any colour: one flat mid-value silhouette per cell in the exact walk pose, with the part regions - hair, face, torso, front arm, back arm, front leg, back leg, shoes, weapon - laid down as separate flat fields. That is what every paper-doll generator does, and it is why their parts stay interchangeable: recolour inside a region and the outline never moves, and a base built on the same anchors accepts parts from any other base. Keep the SAME anchors across all cells - shoulder, hip, knee and foot placement - and move limbs by rotating around those anchors, never by redrawing the figure, so part boundaries run continuously from cell to cell and a swapped part leaves no seam. Hold one flat value per region until the base is complete, then shade; add light and shadow with the SAME part boundaries so a later recolour still lands inside its region. Leave one transparent pixel of margin around every limb so parts never fuse, and keep the base pose generous - a cramped stand pose leaves no room for the clothing the user will swap in later.",
    },
    KnowledgeEntry {
        id: "target-platform",
        title: "Target platform and export constraints",
        keywords: &[
            "目标平台", "引擎", "游戏引擎", "平台限制", "导给",
            "engine", "platform", "console", "retro console", "unity", "godot", "rpg maker",
            "rpgmaker", "rmmv", "rmmz", "rmvx", "vx ace", "rmxp", "rm2k",
            "gamemaker", "game maker", "gdevelop", "nes", "snes",
        ],
        body: "Read the destination off the request and honour it. Unity: 32-128px, power-of-two sizes, clean alpha edges. Godot: 16-64px, the import filter set to Nearest. RPG Maker: MV/MZ use 48x48 cells in a 4-row-by-3-column character sheet (144x192 per character, 8 to a 576x384 sheet); VX/Ace use 32x32 cells in the same 4x3 sheet at 96x128; XP uses 32x32 cells in a 4-row-by-4-column sheet at 128x128; keep tilesets A1-A5 grouped by function. GameMaker: watch the sprite origin, usually bottom-centre for a character. Web: 8-64px, keep the file small and let the consumer slice the plain sheet. Retro console: three or four colours per sprite plus transparent, 16x16 or 32x32 cells, and attribute clashing decides the palette per 8 or 16 pixel row. When the user names no engine, deliver the plain sheet plus a manifest that records the size and palette, and let them slice it.",
    },
    KnowledgeEntry {
        id: "tilemap",
        title: "Tilesets and tilemaps",
        keywords: &["瓦片", "地图", "地块", "瓷砖", "平铺", "程序化", "值噪声", "配比", "砖块", "砖墙", "砖块尺寸", "手绘瓦片", "tilemap", "tile map", "tileset", "tile set", "tiles", "无缝平铺", "brick", "brick pattern"],
        body: "Procedural beats hand-drawn: lay the base value with value noise, smooth it with cosine interpolation, then quantise hard into the ramp - anything freehand reads as decoration. Drive the noise at TWO scales, or one octave alone comes out either blotchy or flat: place the large nodes every 8-16 px so the big regions read as terrain, and run a second, finer pass every 3-6 px for the subtle variation inside them. Colour it by probability: about 70% of cells the base value, 15% one step lighter, 10% one step darker, and at most 5% sparse features (pebble, crack, tuft) with no single feature over 1% of the tile. Make every edge weld: fill two pixels in from each border with the wrap rule and hold edge contrast to within one ramp step so the seam disappears. Keep the tile strictly quantised - one stray value announces the grid. Never draw a black outline on a tile. Then verify: repeat the tile 2x2, 4x4 and 6x6 and read the result, and confirm the rotations and mirrors read as different tiles. Work on a cell grid of 8, 16 or 32, keep ONE light direction and ONE ramp per material across the whole set, and author it at exactly 1x. Hand-authored tiles obey a short rulebook: distribute visual weight evenly so no region dominates and the repeat stays hidden; repeat the same key clusters and never let two key clusters touch except corner to corner, or they clump into noise; keep the colour count low, because a busy texture next to another busy texture exhausts the eye and negative space is the friend; and mirror a tile to get an orientation variant before drawing a new one. Build a set in layers - keep the full-bleed repeating texture on the base, shave its sides and corners off for the connection tiles, and put textures that do not fill the whole tile on their own layer so a few pieces can be combined instead of baked into every variant. For a 16px brick tile with a 1px grout, the only brick sizes that divide evenly are 15, 7, 3 and 1px - including when the pattern is tilted 45 degrees - so pick from those or the courses break.",
    },
    KnowledgeEntry {
        id: "sprite-scale",
        title: "Sprite size in tile units and native resolution",
        keywords: &[
            "精灵尺寸", "角色多大", "角色尺寸", "精灵大小", "瓦片单位", "原生分辨率", "游戏分辨率",
            "屏幕分辨率", "像素完美缩放", "分辨率怎么选",
            "sprite size", "tile unit", "native resolution", "screen resolution",
            "pixel perfect scaling", "reference resolution",
        ],
        body: "Size the sprite in TILE UNITS, not in free pixels: one tile wide by two tiles tall is the classic top-down character, and using whole multiples of the tile keeps the sprite snug in the grid and makes the collision box a simple tile expression. The frame is a frame, not a bounding box - leave a little air so adjacent sprites do not show gaps, but not so much that the character looks lost in it. In a top-down view the sprite may overlap the tile above it (depth runs down the screen, so lower means nearer) but should never overlap along the X axis, where the eye reads overlap as an error. At small sizes the head wants a third to half of the total height, because the face is what the player connects to; chasing realistic proportions on a 16-24px sprite just yields a stick figure. Four directions drawn properly can carry eight-direction movement - the 16-bit consoles did exactly that and it still reads well; add the four diagonal frames only when the sprite is large and detailed enough for the missing angles to be noticeable. The walk and the run can share one pose set: build eight run frames, then make the four-frame walk by dropping the full-stride frames and playing what is left slower. Pick a native resolution that multiplies cleanly into 1920x1080 - 320x180 (6x), 480x270 (4x) and 640x360 (3x) are the safe picks - because an arbitrary one leaves borders at full screen or breaks pixel perfection; higher native resolutions read as more zoomed-out and suit fast scrolling, lower ones read chunkier and suit a retro feel.",
    },
    KnowledgeEntry {
        id: "sand-terrain",
        title: "Sand terrain and recycling wall tiles",
        keywords: &[
            "沙子", "沙滩", "沙漠", "沙地", "沙纹", "土墙", "石墙", "墙面瓦片", "墙体",
            "遗迹", "废墟",
            "sand", "desert", "sand texture", "wall tile", "cliff", "ruins",
        ],
        body: "Sand is not noise: build it from long angled calligraphic S-curves that sweep across the tile and keep the waves parallel-ish, then break a few of them with irregularity so the field does not look printed. Watch the density - if every wiggly line connects into one net, the sand reads as noodles instead of drifts; leave flat space between the curves. A wall or cliff set is built the economical way: start from the FRONT-facing wall texture and make it loop on all four sides, then rotate that same texture to 45 degrees and trim pieces of it into the linking bottom tile and the wall-top tile, mirror the angled tile and swap one colour for the opposite wall face, and reach for a triangle tile only where a terrace has to meet the ground. Add a universal column or corner piece instead of four bespoke corners - it doubles as decoration. Every drop shadow in the set stays on one or two faces only, follows the single light source, and never runs longer than one tile regardless of how tall the wall looks; long shadows fight the sprites layered on top of them, and a short consistent shadow reads fine even though it is not physically accurate.",
    },
    KnowledgeEntry {
        id: "tiny-tiles",
        title: "Tiny 8x8 tiles and minimal-palette readability",
        keywords: &[
            "8x8", "8x8瓦片", "8像素瓦片", "微型像素", "极小尺寸", "小瓦片", "微型科幻",
            "nes配色", "红白机配色", "描边可读性", "8位瓦片",
            "tiny pixels", "tiny tiles", "8px tile", "minimal palette", "nes palette",
        ],
        body: "An 8x8 tile framework - 'tiny pixels' - keeps production fast while still reading. Design characters as a bare 8px-tall figure and let the outline grow it one or two pixels larger, so the whole cast shares the grid; a hero gets a full 6-frame run, most other characters do fine with 4, and a dash or rocket boost is 4 frames at 50ms. At this size one pixel of movement is a whole stride, so a couple of frames carry a full walk and a one- or two-pixel shift is enough to turn a head or change a stance; never pad the cycle with frames that move nothing. Build the environment mainly by reusing the RULES, not the tiles: a building is a body tile plus roof pieces whose width, height and depth can be added or removed, and a handful of balanced textures lays out a whole city. Keep ambient occlusion and drop shadows on their OWN layer rather than baked into the base texture, because a free shadow layer lets the same tiles be restacked in new combinations for variation; a reliable 8x8 layer order bottom-to-top is dirt, grass, shadows, trees and rocks, cliffs, and a second shadow pass. When the palette is as tight as the NES set, turn OUTLINES ON - usually the opposite of the advice for larger art - because the outline is what separates the sprite from a busy background. Faces stop being faces: keep one forward-facing idle for every direction and just flip the run, since the character turns to camera when it stops, and accept that sprites sitting slightly large against small buildings is a convention that keeps level design compact. All the tiny assets are usually separate layers - a gun, a muzzle flare, a dust puff - so a weapon or an effect can be swapped without redrawing the body, and a large character is best broken into layered pieces that can be posed independently.",
    },
    KnowledgeEntry {
        id: "fake-patterns",
        title: "Patterns that make procedural work look fake",
        keywords: &[
            "太规律", "一看就假", "一看就是假", "太假", "很假", "假的明显", "有规律",
            "几何图案", "四方格", "居中十字", "正弦波", "水平条纹",
            "对称装饰", "斜向裂缝", "噪点太碎", "雪花点", "规则感",
            "fake pattern", "too regular", "looks fake", "geometric pattern", "centered cross",
            "symmetric motif", "straight crack", "salt and pepper", "procedural",
        ],
        body: "Procedural terrain looks fake for exactly one reason: regularity the eye can predict. Forbidden in repeatable ground - a centred cross, star or plus, four equal quadrants, regular sine waves, horizontal stripe layers, symmetric corner motifs, straight diagonal cracks, and salt-and-pepper noise or evenly scattered bright dots. Replace each with its irregular cousin: off-centre single features, Voronoi cells of 8-15 uneven pieces, low-frequency value noise, diagonal or broken colour blobs, one asymmetric accent, short random-walk cracks that never cross the tile. Near-white is out on ground and underground unless asked for - use one step lighter, warm ochre or muted grey. A decorative tile built out of architecture (brick, floorboard, roof) may keep its geometry, because there the pattern IS the material.",
    },
    KnowledgeEntry {
        id: "tile-edges",
        title: "Tile edges and transition pieces",
        keywords: &[
            "瓦片边缘", "瓦片衔接", "接缝", "过渡瓦片", "地块过渡", "角块", "边缘块",
            "tile edge", "edges", "corner", "corners", "transition", "bitmask", "autotile", "welding",
        ],
        body: "Ship the minimum nine-piece set - centre, four straight edges, four corners - welded so the centre band repeats and the corners close the loop, instead of one big picture. Drive grime and terrain transitions with a bitmask over the eight neighbours (four orthogonals plus four diagonals; byte 47, autotile 5x3) rather than a manual table of every combination. Cell size follows the engine: 16 for NES / SNES and RPG Maker 2000, 32 as the general middle, 48 for RPG Maker MV/MZ, 64 for HD sets. SIDE-VIEW is a different minimum: 8x8 cells, a 3x3 structure for floors, ceilings and walls, plus a 4x4 diamond shape holding the INNER corners that close L-shaped formations - twelve tiles cover a whole stage that has no slopes, and the inner-corner pieces are the ones people forget. Target the native resolution the tiles were drawn for rather than the monitor: 320x180 is 16:9, scales pixel-perfect into 1080p at 6x, and keeps characters and 8x8 tiles in a readable ratio. The whole set shares one light direction, one ramp per material and one outline rule, and is authored at exactly 1x - drawing big and downscaling smears the edges.",
    },
    KnowledgeEntry {
        id: "dithering",
        title: "Dithering and ordered patterns",
        keywords: &[
            "抖动", "网点", "棋盘", "递色", "交织抖动", "风格化抖动", "随机抖动", "网点太多",
            "dither", "dithering", "ordered", "bayer", "checkerboard", "interlaced dither",
            "random dither", "stylized dither",
        ],
        body: "Alternate two ramp steps in a regular pattern instead of adding an in-between color. A dither eases the transition between two colors and, when the pattern is coarse enough, doubles as texture - but a checkerboard's job is to taper the ENDS and EDGES of an opaque field, so a dithered area that covers half the sprite has stopped buffering and become a texture field, and a new palette step would serve better. The common forms: 50/50 (checkerboard, the base pattern), ordered Bayer 4x4 (keeps texture and direction, fits marble, skin and sky), interlaced (two dither regions weave together at their border and build a gradient), stylized (little shapes embedded in the pattern, reads as decoration), and random/noise (adds single-pixel noise and is usually avoided outside very small doses - the reason a 25% dither is dangerous is the lone pixels it scatters). The lower the contrast between the two colors, the gentler the dither, so dither 50/50 between close neighbours and something sparser between far ones, never across an outline, and never so much that the pattern reveals the grid.",
    },
    KnowledgeEntry {
        id: "ramps",
        title: "Value ramps and hue shifting",
        keywords: &[
            "色阶", "过渡", "渐变", "明暗", "暗部", "亮部", "直线色阶", "共用色阶",
            "借色", "借色相", "色相替换", "颜色不够用", "没有浅色",
            "ramp", "ramps", "shading", "value steps", "gradient", "hue shift", "straight ramp",
            "hue substitution", "borrow a hue", "run out of colours",
        ],
        body: "Build 3-5 steps from core shadow to highlight, hue-shifted (cool violet-blue shadows, warm highlights) rather than just adding white and black. A STRAIGHT ramp changes value only and reads boring; bend the highlights toward one hue and the shadows toward another so the ramp carries subtle color contrast on top of the value change. Ramps may SHARE steps: the darkest and the lightest usually belong to every ramp in the palette, and a near-neutral mid-tone can bridge two ramps in place of two separate colors. Place the darkest step just past the terminator, not on the object's edge. Generate steps with hsv()/mix() so the rhythm stays even, give each material its own ramp, and remember value changes apparent thickness - a mid-grey line reads thinner than a black one of the same width. A tight palette forces HUE SUBSTITUTION: when the ramp has no lighter step of the local hue, borrow the nearest available hue for the highlight (green lit by a yellow step) and the nearest dark cool hue for the shadow (green shadowed by a blue step). Keep the direction - warm hues up, cool hues down - because flipping it (a brighter cool highlight and a darker warm shadow) reads plainly wrong even though both are technically inside the palette.",
    },
    KnowledgeEntry {
        id: "palette-control",
        title: "Palette control: saturation, value range, eyeburn",
        keywords: &[
            "过饱和", "太艳", "太鲜艳", "刺眼", "辣眼睛", "晃眼", "颜色跳出来", "融不进画面",
            "色阶跨度不够", "灰蒙蒙", "对比不够", "共用颜色", "共用暗部", "共用亮部",
            "中性色过渡", "桥梁色",
            "saturation", "oversaturated", "too saturated", "eyeburn", "off-ramp",
            "value range", "neutral bridge", "shared ramp color",
        ],
        body: "A small palette is kept for two reasons: COHESION, because fewer colors reappear across the whole piece and tie it together, and CONTROL, because changing one color moves a whole ramp instead of 200 micro-relationships. Spend the steps deliberately and run three checks. SATURATION stays low - colors emitted as light burn the eye far faster than pigment does, and the usual beginner failure is a palette where every step is fully saturated so the picture is uncomfortable to look at. VALUE spreads across the whole range - a palette that only holds mid-tones cannot make contrast no matter how many hues it has, and a low-contrast palette is the second most common failure. Every color sits ON its ramp - a step whose saturation jumps or whose hue clashes with its neighbours punches through the picture and looks pasted on top; that is EYEBURN, and it is a value/hue relationship problem, not a matter of taste. Ramps are allowed to share their extremes, and a near-neutral mid-tone is the usual bridge that lets one color serve two ramps.",
    },
    KnowledgeEntry {
        id: "outlines",
        title: "Outline strategy",
        keywords: &["勾线", "描边", "轮廓线", "线稿", "outline", "outlines", "line art", "hard edge"],
        body: "Pick ONE strategy for the whole drawing and keep it consistent, and the default is a solid outline on every shape: users read an outlined sprite as finished and an un-outlined one as an unfinished fill. Hue-shift the outline toward the surface color instead of using pure black on a saturated body, or it eats the silhouette - take the local hue a few steps darker rather than inventing a black. Keep outlines 1px at 32px and above; below 16px skip them entirely, or drop them everywhere only when the user asked for no outline, fog or backlight. The eight styles worth naming, drawn from one mushroom: single-pixel black (the animation-safe default), double-pixel, a dark shade of the surface colour, the region's own colour, an outline lit by the light direction, sel-out (outline AA toward a known background colour), broken or dashed, and none at all. Pick one per drawing and hold it on every frame.",
    },
    KnowledgeEntry {
        id: "pixel-discipline",
        title: "Pixel discipline and colour budget",
        keywords: &[
            "硬边", "硬像素", "色数", "色板限制", "配色预算", "统一光向", "严禁", "禁止出现",
            "anti-aliasing", "antialiasing", "gradient fill", "colour budget", "color budget",
            "palette limit", "budget", "restrictions",
        ],
        body: "What breaks a pixel drawing is mostly habit carried over from vector art: anti-aliasing on the silhouette, sub-pixel placement, gradient fills, local transparency, blur, bezier handles, and any colour outside the palette. Fix the light direction once for the whole project - default top-left - and never flip it halfway. The colour budget is per image, not per shape: 3 for minimalist, 4-6 under 32px, 3 per sprite plus transparent on NES, 15 for SNES-style, 31 for a modern look, 47 for a dense illustration. Sizes that stay clean: 8, 16, 24, 32, 48, 64, 96, 128, 256. Draw at exactly 1x - resampling afterwards turns clusters into mush.",
    },
    KnowledgeEntry {
        id: "style-tiers",
        title: "Console and house style tiers",
        keywords: &[
            "风格分级", "画风分级", "哪种风格", "8位机", "八位机", "8位风格", "八位风格",
            "红白机", "FC风格", "16位机", "16位风格", "超任", "sfc", "掌机风格", "独立游戏风",
            "极简风格", "高密度", "影视级", "印刷级", "大尺寸像素", "高分辨率像素", "风格定级",
            "style tier", "nes style", "snes style", "modern pixel", "minimalist style",
            "dense detail", "high-res pixel", "cinematic pixel", "console style", "which style",
        ],
        body: "Name the tier before the first pixel and it fixes size, colour budget and shading tier at once. NES CLASSIC: 16x16, up to 32x32 for large sprites, 4-8 colours per sprite, flat or 2-tone shading, hard edges. SNES RETRO: 32x32-64x64, 8-16 colours, 3-tone. MODERN PIXEL: 32x32-64x64, 16-32 colours, full ramps, dithering allowed. MINIMALIST: 8x8-16x16, 2-4 colours, flat only. DENSE DETAIL: 64x64-128x128, 24-48 colours, 4-5 ramp steps per material. HIGH-RES PIXEL: 256x256 and up, 64-96 colours. CINEMATIC PIXEL: 1024x1024 and up, 128-256 colours, print quality. The shading tier follows the tier, never personal taste: flat for minimalist, 2-tone at NES, 3-tone at SNES, dithered when the palette forces it, selective outline throughout. Read the tier off the words used - 红白机 or fc means NES, 超任 or 16位机 means SNES, 独立游戏 means Modern Pixel, 大头像 or 海报 means the top tiers.",
    },
    KnowledgeEntry {
        id: "spec-lock",
        title: "Lock the spec before drawing",
        keywords: &[
            "先定规格", "规格锁", "先把尺寸和配色定下来", "别改设定", "统一规范", "执行纪律",
            "别跳步", "不要臆测", "别提前画", "每件都重读规格", "规格", "做到一半",
            "改设定", "不要跳步", "别跑偏",
            "spec lock", "locked spec", "execution discipline", "stay in spec", "spec drift",
            "lock the spec", "no speculative",
        ],
        body: "Write the lock once, then draw only inside it. The lock is five lines: canvas size in pixels, the palette as literal hex values, the art style, the target platform, and the animation list with frame counts. Put it in writing before the first pixel and re-read it before every asset - a fresh generation pass remembers none of the earlier lines, so a colour invented on frame three and a size that drifts on frame five are the two failures that show up in every review. Never widen the lock mid-run: if a colour or a size is missing, add it to the lock first, then draw. Never pre-draw what a later step will need, never bundle two assets into one pass, and finish each asset completely before starting the next.",
    },
    KnowledgeEntry {
        id: "quality-gate",
        title: "Self-check before calling anything done",
        keywords: &[
            "自检", "交付前检查", "检查清单", "验收", "核对一下", "检查一遍", "别跳过检查",
            "quality check", "checklist", "validate", "validation", "self check", "pre-delivery",
            "before shipping",
        ],
        body: "Before declaring an asset finished, read the canvas back and check it against the lock: every colour inside the palette, no anti-aliasing and no sub-pixel edge, every frame the same size, frame counts matching the declared list, transparency set, and nothing stray outside the declared boundary. Then read the frames themselves - a tool result shows the active frame only, so a limb that wandered out of the body on frame three stays invisible until they are opened. Failing a check means fixing it in place; never ship a failed asset and never skip a check because the drawing looked fine on screen. Quantise, then clean stray pixels, then validate, then pack, then export - in that order, and no step is optional.",
    },
    KnowledgeEntry {
        id: "asset-classes",
        title: "Asset classes and deliverables",
        keywords: &[
            "资产类型", "素材分类", "交付物", "素材清单", "分类一下", "这类素材",
            "分类", "素材", "清单",
            "asset class", "deliverable", "asset breakdown", "categories", "category",
        ],
        body: "Six deliverable classes, each with its own contract. CHARACTERS: 32-64px, side or three-quarter view, a pose readable in four frames. TILES: follow the tilemap and tile-edge rules. ITEMS: centred with a one or two pixel margin, silhouette readable at a glance, no cast shadow. UI: nine-slice the frame - corners keep their exact size, edges repeat, centre stretches - 1px hard borders and a 2px minimum hit area. EFFECTS: 4-8 frames, transparent background, no outline, brightest at the start and dissolved by the end. BACKGROUNDS: 96-160px, far planes take less contrast, no single focal subject. Decide the class from the user's words first: a well-drawn thing in the wrong class is unusable.",
    },
    KnowledgeEntry {
        id: "set-consistency",
        title: "One set, one scale, one style",
        keywords: &[
            "成套素材", "一套素材", "素材组", "同一组", "放在一起", "拼在一起", "整套",
            "比例不统一", "尺寸混了", "硬贴上去", "同一种画风", "风格统一", "统一画风",
            "asset set", "one set", "consistent set", "same scale", "style consistency",
            "cohesive", "matching set", "set of assets",
        ],
        body: "Assets that appear together must share one scale, one style and one outline rule - the wizard stays 32x32 because it belongs beside other 32x32 pieces, and mixing sizes into one screen reads as something pasted on afterwards. Two habits keep a set cohesive: re-draw a subject rather than rescale it (auto-shrinking loses the eyes, breaks the outline and turns the beard into mush), and make the style choices on purpose, then hold them on every piece - a deliberate choice repeated twenty times reads as authored, while the same drawing made four different ways reads as assembled. Before drawing the second item of a set, ask what the first one established and repeat it, and record the shared canvas size, shared palette and shared outline rule in the lock so a later pass cannot drift from them.",
    },
    KnowledgeEntry {
        id: "pixel-clusters",
        title: "Pixel clusters and curve rhythm",
        keywords: &[
            "像素簇", "孤立像素", "噪点", "杂点", "锯齿", "毛刺", "阶梯",
            "pixel cluster", "pixel clusters", "single pixel", "lone pixel", "noise pixel",
            "jaggies", "curve rhythm",
        ],
        body: "A cluster is a continuous run of pixels of the exact same colour, and it is the unit the drawing is built from - the border of one cluster shapes the cluster beside it, so rearranging a cluster changes the picture more than recolouring it does. The aim is as FEW clusters as possible and no one-pixel clusters at all. Pixels that touch only diagonally are a WEAK connection: they technically join, but treat them as a seam to avoid unless the shape demands it. Details read as 2x2-plus clusters; one stray pixel reads as dirt. LONE PIXELS are the exception, justified for exactly three jobs: a specular highlight dot, texture, and a small but essential detail on a very small sprite (an eye, a beak, a star, a bubble). A lone pixel of a DIFFERENT color that directly buffers an edge is not noise either - it is anti-aliasing, and counts as part of the cluster it touches. When a lone pixel is carrying a real detail, the fix is to absorb it into a small shape (a 2x2, an L, a T) rather than to delete it; when it is not, delete it and merge the neighbours. Everything else is noise: fix a lumpy curve by re-spacing its runs into a regular step rhythm (45 degrees = one pixel per row, about 22.6 = 2-pixel runs, about 30 = evenly spaced) instead of smoothing it with extra color, and remember that single pixels expose the grid by revealing the resolution.",
    },
    KnowledgeEntry {
        id: "line-quality",
        title: "Line weight: doubles, jaggies and step rhythm",
        keywords: &[
            "双像素", "像素拐角", "线条粗细", "台阶长度", "线条不干净", "断线", "锯齿", "阶梯",
            "doubles", "double pixel", "line weight", "step rhythm", "line quality", "jaggies",
        ],
        body: "Two pixels forming an L at a corner make a DOUBLE, and that corner reads thicker, darker and harder than the rest of the line - for a uniform 1px outline, delete the extra corner pixel. JAGGIES come from uneven step lengths: a clean straight line runs 2-2-2-2 while a lumpy one runs 1-3-2-1-4. Read the border as a STAIRCASE and count the pixels in each step: on a correct curve the run lengths rise smoothly toward the horizontal and fall smoothly toward the vertical, usually in a geometric progression (5-3-2-1-1-2-3-5). The run lengths are allowed to change fast - what is not allowed is the direction of the change reversing in the middle of the curve, which is exactly what a jaggy is: a step that suddenly shrinks and then grows again. Fix it by PUSHING PIXELS to restore the steady rise or fall, never by adding colour - a 1-3-2-1 sequence is a misstep, not a shading problem. Doubles are not automatically a defect - running the entire outline in doubles is a bold style of its own, and one deliberately broken line can model a brow ridge.",
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
        id: "aa",
        title: "Anti-aliasing discipline",
        keywords: &[
            "抗锯齿", "柔化", "外部抗锯齿", "内部抗锯齿", "过渡像素", "中间色像素", "阶梯柔化",
            "加多了", "抗锯齿过多", "抗锯齿太少", "抗锯齿带", "糊了", "边缘模糊",
            "anti-aliasing", "antialiasing", "aa pixel", "external aa", "internal aa",
            "soft pixel", "aa banding", "over-anti-aliasing",
        ],
        body: "AA puts a pixel whose value sits between a shape and its neighbour on every step of an edge. Three rules carry it: add it SELECTIVELY - one or two pixels where a curve meets a contrasting background; NEVER let it change the shape, because extra pixels in the corners quietly redraw the outline; and judge it at real size, never at 8x zoom. Length has to match the step it buffers: too little AA (one lone pixel on a long step) only blunts the corner without smoothing it, and too much turns the crisp edge into a blur - a 4-pixel step wants a roughly 4-pixel taper, not a single dot and not a smeared fringe. AA BANDING is the specific failure where the AA segments line up with the edge they are buffering, so the buffer becomes a second parallel outline and exposes the grid; stagger the starts of the AA runs instead of stacking them evenly. Inner AA lives inside the shape, outer AA lives on the background and dies the moment that background changes, so prefer inner AA on anything that may be moved onto a different background. Value is the only thing that has to be right, so an AA pixel's hue is free to pick. Most sprites need no AA at all, and where it is used as a style choice the beard and the hair are the places that earn it.",
    },
    KnowledgeEntry {
        id: "sel-out",
        title: "Sel-out needs a known background",
        keywords: &[
            "选择性描边", "选择性勾勒", "断线描边", "断线轮廓", "破轮廓", "破碎轮廓",
            "描边断开", "描边蹭背景", "背景色抗锯齿", "蹭背景",
            "sel-out", "selout", "selective outlining", "broken outline", "broken outlines",
        ],
        body: "Sel-out (selective outlining, also called broken outlines) is anti-aliasing an outline toward a background color - it is really a kind of EXTERNAL AA, which is why it only works when the background is known and stays that color, such as a game scene that is consistently dark or a fixed HUD panel. It is NOT shading an outline by the light source: a full outline with light variation is normal shading and keeps its solid line. Breaking a solid outline into dashed runs exposes jaggies worse than the solid outline does, because the gaps become new steps along the silhouette, so sel-out buys background blending at the cost of edge quality. Default to a solid outline; reach for sel-out only when the user asked for it or the deliverable pins a known backdrop, and then replace the outline pixels with a step toward that backdrop color (never black), kept to the side that faces the background.",
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
        body: "Decide the palette before drawing and spend only its steps: one dominant hue family, one accent, one neutral ramp, and a ramp per material. Build by ROLE rather than by taste: every hue that carries form needs at least three steps (highlight / base / shadow), because a hue with only one or two steps can be filled but not shaded. Keep neighbouring steps visibly apart - roughly 30 or more in RGB distance - since two near-identical colours read as one and waste a slot, which is the 'too many similar colours' failure. Cover warm, cool and neutral families, hold both saturated and desaturated entries, keep background colours duller than the foreground, and give characters two skin ranges (a lit one and a shadowed one) instead of one flat tone. Building from a reference: sample the dominant colours, cut them down to the target count, check that the steps still separate, then show the palette before drawing into it. Tiny canvases (under 32px) hold 4-6 colors; 64px and up can carry 12-20 without turning to noise.",
    },
    KnowledgeEntry {
        id: "palette-library",
        title: "Curated palette library",
        keywords: &[
            "调色板库", "经典配色", "配色库", "用现成的配色", "色板套用",
            "pico-8", "pico8", "db32", "endesga", "resurrect", "sweetie", "nuclear blaze",
            "mushi", "curated palette", "named palette", "open palette", "famous palette",
        ],
        body: "A curated palette often beats an invented one: PICO-8 (16), DB32 (32), Endesga 32, Sweetie 16, Nuclear Blaze 8, Mushi 8, Resurrect 64, Ink 5. Pick by feel: DB32 or Resurrect for general game art, Endesga for a colourful youthful look, PICO-8 for retro jams, Nuclear Blaze or Mushi for tight two-tone noir, Sweetie for cute palettes, Ink for a single-colour mono look. A locked palette is a constraint that buys discipline: no colour outside the set, and the light and dark steps come from neighbouring entries in the palette rather than from a hue shift. Recommend a named palette when the user asked for a house style but described no colours of their own, and record the palette name in the deliverable manifest.",
    },
    KnowledgeEntry {
        id: "style-recipes",
        title: "House styles with the hex already settled",
        keywords: &[
            "卡哇伊", "可爱风", "粉彩", "8位", "八位", "万圣节", "暗黑风", "乡村风", "单色方案",
            "色调配方", "现成配色", "风格配方", "配色速查", "8位配色", "八位配色", "配色配方",
            "粉彩配色", "可爱配色", "万圣配色", "暗黑配色", "乡村配色", "给一套现成",
            "来一套配色", "给个配色方案",
            "pastel", "kawaii", "cute palette", "8-bit", "halloween", "cottage",
            "nes palette", "style recipe", "house style", "monochrome palette",
        ],
        body: "Ready-made recipes, hex included, so a whole set stays consistent. PASTEL/KAWAII (5): peach #ffb7b2, mint #b5ead7, sky #c7ceea, cream #fff1e6, blush #ffd3b6, plus outline ink #2d3142. NES 8-BIT (4 of the console's 56): red #b0030a, blue #0078f8, yellow #fcbc3c, white #fcfcfc, plus black for outlines. COTTAGE (6): sage #b7c9a8, terracotta #d4825a, dusty rose #d8a7a1, butter #f5e1a4, soft brown #8b6f47, cream #fff8e7. HALLOWEEN DARK (5): near-black #0f0f0f, pumpkin #ff7518, wizard purple #5a3fff, blood #8b0000, candle yellow #ffd700. MONO (2): black and white, which forces confident silhouette decisions and is superb at 8x8 and 16x16. Carrying one recipe across every frame of a sheet is what makes the set look authored rather than assembled.",
    },
    KnowledgeEntry {
        id: "physical-media",
        title: "Beads, cross-stitch and diamond painting",
        keywords: &[
            "拼豆", "豆子", "拼豆板", "十字绣", "刺绣", "钻石画", "针织", "手工", "实体媒介", "图纸",
            "bead", "beads", "perler", "cross stitch", "embroidery", "diamond painting", "handcraft",
        ],
        body: "One bead, one stitch, one diamond holds one colour, so pixel art already IS the pattern - every rule above carries over with three changes. The palette becomes the beads or threads you can actually buy, so pick that set first and draw into it instead of treating a limited palette as optional. The bead board decides the size: a large square board is 29x29, so a 16x16 sprite sits comfortably while 32x32 needs four boards. Viewing distance does the dithering for you - a checkerboard reads as a mid tone from across the room, and one wrong bead still looks like a speck. Anti-aliasing converts worst of all and usually wants a clean step in an available colour. Hand-drawn sheets beat converted photos every time, and the finished deliverable carries a per-colour bead count: export the manifest and read its `palette.pixels` array, which lists each colour's cell count in palette order beside `palette.transparent`, so the shopping list is a read rather than a recount.",
    },
    KnowledgeEntry {
        id: "lighting",
        title: "One light source",
        keywords: &["光源", "光照", "受光", "背光", "阴影", "投影", "cast shadow", "light source", "lighting"],
        body: "Fix ONE light direction and honour it on every object in the scene. When the user named no direction, no lamp, no hour of day, do not invent one: read tops a half step lighter and undersides a half step darker off the viewer's angle, and pool the cast shadow on the ground beneath the form instead of leaning it toward an assumed source. The lit side takes the highlight, the shadow side a darker ramp step, and the darkest core shadow sits just past where the form turns away. Cast shadows must agree with that same direction in both length and lean. A small canvas rarely needs all seven parts of the basic model (highlight, midtone, terminator, shadow, bounce, occlusion, cast shadow) - pick by size and style. Material decides the read as much as direction does: matte spreads light evenly with no highlight at all, glossy takes a small sharp one, and metal takes strong contrast and reflects its surroundings.",
    },
    KnowledgeEntry {
        id: "pillow-shading",
        title: "Pillow shading",
        keywords: &[
            "枕形阴影", "枕头阴影", "枕头", "一圈圈加暗", "一圈圈", "往里加", "向内加",
            "由外向内", "像个抱枕", "显平", "显鼓", "中间亮一圈",
            "pillow shading", "pillow", "rings of dark",
        ],
        body: "Pillow shading darkens inward from the outline in concentric rings: the result reads flat and puffed, like a cushion. The reason it is wrong is NOT that the light comes from the viewer - a frontal light is legal - it is that the bands follow the flat 2D outline instead of the 3D form, so the object is lit as a silhouette rather than as a body. The fix is to light from the real direction and let the bands follow the surface: highlight on the lit side, shadow past the turn, core shadow where the form turns away, and step widths that change as the surface changes. One deliberate exception: a form facing the viewer straight on can legitimately have its brightest area in the middle, so long as the bands still describe the form rather than tracing the border.",
    },
    KnowledgeEntry {
        id: "banding",
        title: "Banding: hugging, fat pixels, skip-one, 45-degree",
        keywords: &[
            "色带", "条带", "带状", "等宽阴影带", "贴着轮廓", "平行条纹", "明暗层",
            "条纹感", "色带感", "网格显形", "肥像素", "肥线", "隔空对齐", "45度带",
            "banding", "hugging", "fat pixel", "fat pixels", "skip-one banding",
            "staircase banding", "parallel bands",
        ],
        body: "Banding is when pixels LINE UP: two runs that start or end on the same grid coordinate expose the grid, and the apparent resolution of the image drops even though every pixel is individually fine. Four named shapes cover most of it. HUGGING: a shade or AA run of constant width tracing an outline, so the band and the border expose each other. FAT PIXELS: 2x2 blocks or a thicker line forming a band, including the staircase. SKIP-ONE: two bands separated by a one-pixel gap still band, because the eye fills the gap in. 45-DEGREE: even a run only one pixel wide bands when every step lands on the same row/column rhythm as its neighbour. The fixes are all about breaking alignment rather than changing colors: vary the run length along the band, offset each band by a step or two from its neighbour, cut the band where the form turns away, and let the terminator follow the surface instead of the border. Banding is an alignment problem, so recoloring the band will not remove it.",
    },
    KnowledgeEntry {
        id: "form-first",
        title: "Draw the volume, not the flat shape",
        keywords: &[
            "立体感", "体积感", "球体感", "圆柱感", "不要平", "扁平", "机械感",
            "网格线", "块状像素", "厚像素", "一像素粗", "像划痕",
            "volume", "three-dimensional form", "3d form", "flat shading", "chunky pixels",
        ],
        body: "Shade the form you actually mean: an arm is a cylinder, a chest is a pair of spheres, a wing is one stretched membrane - light the 3D body, not the 2D outline it happens to draw. Two failures come directly from ignoring this: pillow shading (bands hugging the outline) and evenly spaced vertical bands pretending to be form shading. THE CHUNKY PIXELS RULE: a feature only one pixel thick cannot carry its own shading, so decide before drawing whether a stripe, a limb or a horn is a LINE (no volume, and that is fine) or a FORM (give it at least two pixels so it can take a lit side and a dark side) - a one-pixel-thick limb that receives shading reads as a scratch. Build the big masses first, exaggerate the defining features (ears, snout, weapon) rather than describing them at true scale, and design for readability: a smaller number of distinct, chunky shapes beats many near-identical thin ones.",
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
        body: "Everything rests on the 2-step (2:1) line: a true 30-degree isometric ground plane drawn in pixels resolves to a 2:1 step, about 26.5 degrees, and once you can read that rhythm the rest is bookkeeping. Keep ONE grid angle for the whole sheet and keep verticals exactly vertical. Draw a side circle from a skewed square plus a cross - the ellipse touches the square at the four cross tips. Keep one 2:1 line on its own layer as a ruler to check alignment, because at this angle you cannot trust your eye. Give the three visible faces three distinct steps of the same ramp so a cube reads without an outline. Cube corner style is a real choice: wide corners look better but need clever overlapping to tile, sharp 2:1 corners tile perfectly but read as hard. A cuboid need not be a cube - the top face just has to match its neighbours so tiles of different height stack - and every face texture has to loop on all four of its sides, not just the ground. Even dimensions are your friend; 36x36 is a common isometric tile. Build units from simple geometric solids - a cube for a torso, a sphere for a shoulder, a cylinder for a limb - then push and pull them into the silhouette, and at 32x32 be willing to OMIT parts that will not read: two pixels is a whole hand, so the skill is abstraction, not detail. Work from the body outward, give the torso a tank-like jut and shoulders that sit higher than the head for weight, and keep the optics to one or two bright pixels. In an isometric mech, the upper leg is barely visible and the groin piece is often dropped, but the FEET are prominent: make them ski-like or talon-like for a stable footprint rather than copying a humanoid foot. On the environment side, a cube tile's diamond top has 2px corners, so adjacent tiles must overlap the top row for a flush fit (or squash the diamond by 1px to trade the look for a 1px corner); water sits about half a cube lower to read as a pit, and a single texture minus its highlights can carry a seamless 16-frame loop.",
    },
    KnowledgeEntry {
        id: "perspective",
        title: "Depth, planes and parallax",
        keywords: &["透视", "灭点", "纵深", "景深", "空气透视", "远景", "近景", "perspective", "vanishing", "depth", "planes", "atmospheric"],
        body: "Split the scene into far, mid and near planes, then push distance with value and saturation: lift shadows and desaturate far, saturate and darken near. Atmospheric perspective is the same lever per pixel - anything further away loses contrast, drifts toward the sky colour and loses detail, and the nearest plane keeps the darkest dark and the sharpest edge. Keep one horizon and one ground angle. Parallax layers should overlap at least a third of their height so the gap does not read as a seam.",
    },
    KnowledgeEntry {
        id: "parallax",
        title: "Layered parallax scrolling backgrounds",
        keywords: &[
            "视差", "视差滚动", "多层视差", "背景滚动", "滚动背景", "分层滚动", "滚动", "卷轴",
            "横版背景", "滚动速度", "视差层", "每层速度", "像素完美移动", "每帧像素", "循环帧数",
            "parallax", "parallax scrolling", "scrolling background", "layer scroll", "scroll speed",
            "pixels per frame", "infinite scroll",
        ],
        body: "Build a scrolling background as four to six separate layers at increasing depth, each moving at its own constant speed in pixels per frame (ppf). Keep each layer's distance identical for every step of that layer no matter the playback rate - convert to a per-second rate only at the very end - or the layers slide out of sync. Pixel-perfect scrolling means the layer moves by the SAME whole number of pixels every frame, so choose a canvas dimension that divides into as many whole numbers as possible and is not a prime-looking width: a 96px-wide loop divides by 1, 2, 3, 4, 6, 8, 12, 16, 24, 32 and 48, which is what gives a stack of layers clean increasing speeds instead of jittery fractional ones. When a layer is authored as a looping strip, the total frame count of the loop equals the canvas width in pixels, and the number of times the image repeats across that strip equals the pixels moved per frame - so a 3-screen-wide band scrolls 3 ppf. Start the furthest background at 1 ppf and step each layer closer to the viewer up to the next perfect rate; a rate slower than 1 ppf needs the image to repeat twice inside the same screen and still tends to look jittery, so avoid it when you can. Push depth with the same atmospheric lever used in a still: going back one layer, drop saturation, drop contrast and lift brightness (about S -20, C -15, B +15 in a typical editor), and leave the near layer untouched so contrast still reads. Loop each layer seamlessly and match feature LENGTH to how obvious the repeat is - a mountain range or landmark has to be wide because a repeated silhouette shows, while grass and trees survive high repetition. Keep layers balanced with no single dominant point of interest so the seam is hard to find, and let any constant animation riding on the parallax loop in a frame count that DIVIDES the total frame count, or the whole loop tears. After one or two screen-lengths of the same tile, drop in a landmark or a variation to break the repeat, and let optional vertical camera movement reveal a little more of the layer.",
    },
    KnowledgeEntry {
        id: "landscape-bg",
        title: "Landscape backgrounds and atmospheric depth",
        keywords: &[
            "风景", "风景画", "场景背景", "背景绘制", "背景图", "自然风景", "远景背景", "地平线",
            "山谷", "沙漠", "森林背景", "草原",
            "landscape", "scenery", "background art", "background scene", "horizon",
            "valley", "desert background", "forest background", "sky gradient",
        ],
        body: "Paint a landscape background as horizontal COLOUR BANDS first, then detail - the illusion of depth comes from the colour choice far more than from the shapes. Set the horizon, then split ground and sky into receding bands and run atmospheric perspective on them: the nearest plane is the most saturated and has the strongest light/shadow contrast, and every plane further back drops saturation, rises in lightness (in daylight) and shifts its hue toward the sky colour - so under a blue sky the near grass is the warmest and each receding plane goes bluer. Keep the budget tight: a whole 4:3 scene at 192x144 with sky, two mountain layers, ground, clouds and a few props fits in about 15 colours by REUSING them across layers, including reusing the far sky tone as the haze on the distant mountains. Match texture scale to distance: the near plane gets 1px-wide short vertical blade clusters, the next plane only one or two pixel tufts, and the far plane stops depicting individual blades at all. Cliff notes for the common variants - valley: warm near grass shifting blue; desert: thick haze pales the sky, near orange sand to light yellow to light blue at the horizon with violet distant shadows; forest: the vantage point sits under the canopy so the light/distance ramp is compressed and even the far trees keep a few branch pixels.",
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
            "高光", "反光", "镜面", "光泽", "亮部", "金属", "刺眼",
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
        body: "Water takes the sky's key colours, not its own hue: mirror the darkest and lightest sky steps and keep the mid tone. Foam reads as clusters along the contact edge, not as white blobs; reflections are vertically squashed copies of the object, broken by horizontal wobble. Build an animated water tile from wavy interconnected blobs drawn with single-pixel lines: start one blob, branch lines from it until they reconnect into a network, break a few lines so the flow keeps moving, then add sparse highlights and a crude drop shadow a couple of pixels below each bright line. Animate it as TWO full tiles cut back and forth; a hard cut suits a retro look, but a third frame that blends the two textures at 50% opacity smooths the loop, and the timing must be neither so fast it reads as noise nor so slow it reads as choppy - only two frames can be convincing at the right speed.",
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
        id: "human-anatomy",
        title: "Realistic human anatomy and head models",
        keywords: &[
            "人体", "人体比例", "八头身", "六头身", "头身比", "解剖", "人体结构", "真人比例",
            "写实人体", "关节", "骨架", "面部比例", "五官比例", "三庭", "三庭五眼", "脸型比例",
            "anatomy", "human anatomy", "head model", "eight head", "8 head", "six head",
            "figure proportions", "realistic figure", "facial proportions",
        ],
        body: "Two head models cover almost everything. The eight-head model is the realistic adult measure and needs a large canvas - a male lands near 29x96, and a female is slightly shorter only because the head unit is smaller. The six-head model is the stocky, doll-like stand-in that fits a small sprite; the same build at six heads is about 29x78. Under 32px you cannot hold an eight-head figure at all, so drop to six heads or fewer and read the pose from one limb line. Build the proportions as a colour-coded dummy first, one segment at a time, and check the shared alignment lines: elbow level with the belly button, wrist meeting the groin, knee midway between hip and ankle, shoulder width about two heads. Nothing drifts more than one pixel between views, and at this scale a single pixel of drift is a visible change, so measure instead of eyeballing. Hair, armour and loose clothing join only after the dummy holds together, and the fastest route to a believable large walk is to rotoscope a real video down to eight frames rather than inventing the motion. The head follows a fixed standard model that you stylise from rather than invent: the eyes sit halfway between the top of the skull and the chin, the face divides into three equal bands from the hairline to the chin, the bottom of the nose is halfway between the eyes and the chin, and the mouth sits one third to one half of the way from the nose to the chin. Spacing in an eye-width: the gap between the two eyes and the width of the base of the nose both equal about one eye, and the mouth is as wide as the distance between the pupils. The ears run from the brow line down to the bottom of the nose, and the neck is about one half to two thirds of a head wide. Draw in three passes - a wireframe to lock proportions, rough shapes to find the volume, then the final form - and start each pose from a single rough gesture stroke down the spine before the wireframe, because that stroke carries the energy the finished figure must keep. Copy real photos and figurine references instead of guessing.",
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
        body: "On most sprites the eye white is a 2x3 to 4x6 block and the pupil a 2x2 core. The wet eye takes one sharp highlight dot because the material is reflective, and it sits on the side the light comes from - with no light named, put it on the upper side. Do not outline the eye against the face - a dark pupil on light fur reads better than a black socket.",
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
        id: "vfx",
        title: "Impact, explosion and particle effects",
        keywords: &[
            "爆炸", "爆炸特效", "烟雾", "浓烟", "尘土", "扬尘", "火星", "火花", "碎片",
            "撞击", "命中", "打中", "受击", "打击", "打击感", "闪光", "闪白", "拖尾", "尾迹",
            "弹道", "子弹", "枪口火光",
            "血迹", "粘液", "闪电", "电流", "特效", "粒子",
            "explosion", "explode", "smoke", "dust", "impact", "hit flash", "muzzle flash",
            "vfx", "fx", "particle", "projectile", "bullet", "trail", "debris",
            "splatter", "blood", "goo", "lightning", "electricity", "spark", "sparks",
        ],
        body: "An effect is a short PHASE LIST, not one puff: an explosion runs charge, a one-to-two-frame flash that blows the silhouette out to white, a fast bloom of hot core to mid to dark smoke, then debris and a settling dust ring - keep the hot core to a single colour and let the outer rings carry the ramp. The frames just before a hit stay empty, and the dust lands AFTER the foot or the body does; the ground briefly loses its outline where the impact is. Bullets and rockets are 2-4 pixel streaks plus a trail that fades over about three frames, never a round dot sliding across the canvas. Blood, goo and sparks must not eat the victim's silhouette - splatter outward from the wound and keep one bright specular so the fluid is not a flat stain. A flame is the same phase idea read upwards: keep one S-shaped main flame that whips and tapers as it rises, then let it break into smaller S-shaped particles near the tip, and animate the main shape like a flag flipped vertical. Six frames on a 100ms beat with the flickering highlight on a 50ms beat is a reliable campfire; cheap, dynamic, and readable at almost any size.",
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
        body: "Decide the one focal point and give it the strongest value contrast; push everything else one or two steps flatter. Reserve the darkest dark and lightest light for that spot. If everything is crisp, nothing reads first. Saturation is seasoning, not the main course: keep most of the canvas quiet and let full saturation live only where the eye should land - a drawing that is saturated everywhere reads as noise. Value outranks hue every single time, so check it the cheap way - put a black layer over the finished drawing with its blend mode set to Colour: the greyscale version still has to read, or the picture is being carried by colour alone.",
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
        body: "Below 16px every pixel is load-bearing: one silhouette, two to four interior shades, no outline, and the detail budget goes to the eyes or the single iconic feature. Halve the canvas in your head and design the shape at that size before placing pixels. The classic first exercise is 16x16 with a 4-colour palette (a mug, then a skull, a sword, a face): the small canvas and the tiny palette are the point, because every placement becomes a deliberate choice instead of an accident. Keep the pencil at exactly 1 pixel wide - anything wider destroys the discipline the size is teaching - and when something will not fit, abstract it down to a single pixel and try again rather than enlarging the canvas.",
    },
    KnowledgeEntry {
        id: "upscale",
        title: "Scaling and crisp edges",
        keywords: &["放大", "缩放", "模糊", "倍率", "upscale", "scale", "nearest neighbor", "crisp", "pixel perfect"],
        body: "Scale pixel art with nearest neighbour at integer factors or it blurs into mush. Integer doubling keeps every pixel a square block; non-integer factors make uneven pixel sizes - avoid them or accept the wobble. NEVER scale by a partial percentage: 107% breaks the pixels unevenly and the result is a mess, while 200% makes every pixel exactly 2x2 and stays sharp, so partial resizes are simply off the table. Scale for display only, with nearest neighbour, and keep an editable source file (.ase / .aip) alongside every export so a later edit reopens the original instead of a rescaled copy. The editor's zoom view scales up; the export does not bake in any smoothing.",
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
        id: "crt-screen",
        title: "The CRT screen: what the blur was hiding",
        keywords: &[
            "老显示器", "老式屏幕", "老屏幕", "老主机", "老机子", "复古屏幕", "显像管", "糊屏",
            "屏幕模糊", "糊掉", "糊一点", "模糊的边", "大屁股电视", "显示器糊", "模拟信号",
            "crt", "crt screen", "screen blur", "retro screen", "blurred pixels", "analog screen",
        ],
        body: "Retro games were drawn for CRT sets that smeared neighbouring pixels together: a waterfall of blue-white stripes and a checkerboard-dithered sky came out of the television as flowing water and a smooth gradient. Two consequences. Collecting references, remember that the clean screenshots online show pixels nobody ever saw - stripes, jaggies and hard dithering were normal on the hardware, and some artists designed straight into that blur. And drawing, borrow the trick rather than the artefact: a dither pattern can stand in for a mid tone the hardware was going to soften anyway. Not every retro game was authored this way, so it is a choice, not a rule.",
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
        id: "final-checks",
        title: "Before calling it done: 1x read, flip, desaturate",
        keywords: &[
            "最终检查", "收尾检查", "交付自查", "最后一眼", "过一眼", "1x 检查", "原尺寸看",
            "缩小看", "翻转看", "镜像检查", "去色检查", "灰度检查", "明度检查",
            "final check", "final pass", "flip check", "mirror check", "grayscale check",
            "desaturate", "read at 1x",
        ],
        body: "Three cheap self-checks catch most misses, and each is a READ, not a redraw. 1X READ: judge the result at actual pixel size, never at extreme zoom - if the subject, the silhouette and the eye line do not read there, added detail will not save them, and the fix is a bigger shape rather than more pixels. MIRROR: flip the image horizontally in your head (or draw it, if a mirror helper is available); asymmetry, a heavy corner and a limb colliding with the body show up instantly when flipped and are almost invisible when not. VALUE / DESATURATE: drop the color and look at the greys - if the value steps bunch into one tone the drawing has no contrast no matter how many hues it uses, and if two materials collapse into the same grey, separate their VALUES instead of adding another hue. Then stop: a piece that reads at 1x is done, and a second polish round on a readable drawing adds noise.",
    },
    KnowledgeEntry {
        id: "drawing-process",
        title: "The four passes: silhouette, line art, flat colour, refine",
        keywords: &[
            "分几遍", "四遍", "起稿", "铺剪影", "勾线稿", "平涂", "先画什么", "从哪儿开始",
            "绘画流程", "作画流程", "第一步", "上色步骤",
            "drawing process", "four pass", "passes", "block in", "blocking in",
            "where do i start", "how do i start",
        ],
        body: "Work in four passes and finish each before the next starts. 1 SILHOUETTE: one flat colour, nothing but the outer shape - if it does not read as one blob, stop and restate the proportions rather than shading it. 2 LINE ART: cut that shape into its major regions (brim, face, beard, staff) at one consistent weight. 3 FLAT COLOUR: one local colour per region, ignoring light entirely. 4 REFINE: shading, highlight and the small details last. The silhouette barely changes between pass one and pass four, and that stability is the whole point of drawing it first. Other starts are legitimate too - construction lines for proportion, rough colour blocks for atmosphere, drawing big and shrinking for detail - but whatever you pick, the first pass stays about the big shape only, and there is no single correct order. For a SCENE, work BACK TO FRONT and keep the layer count low: lay the far plane (sky, mountains) first, then the mid ground, then the near silhouette, because the foundation has to exist before anything sits on it - that is what makes the colours and the relative scale of the objects easy to judge.",
    },
    KnowledgeEntry {
        id: "cluster-sketching",
        title: "Cluster sketching: start from blobs, not from lines",
        keywords: &[
            "色块起稿", "色簇起稿", "色块铺底", "簇起稿", "直接上色", "块面起稿", "不打线稿",
            "cluster sketch", "cluster sketching", "block in colour", "blob sketch", "mass sketch",
            "paint first", "colour first", "no lineart",
        ],
        body: "Instead of lines then fill, sketch straight in colour and refine in shrinking steps: 1 BIG CLUSTERS - a messy gestural version of the whole picture, choosing only the colours and the mood, no detail at all; a 2- or 3-pixel brush is fine here, or outline the cluster and bucket-fill it. 2 REFINE - go smaller one step at a time, working back to front (sky, then mountains, then the building, then the near silhouette) so the foundation is settled before things sit on top of it. 3 FIX JAGGIES AND ADD DETAIL - walk the cluster borders looking for step-length missteps, push pixels to repair them, and add contrast, light and small details as you go. Keep layers few - sky, mid, near is usually enough; more layers than that makes the picture messy rather than controllable. This technique suits organic and painterly subjects best: nature, plants, water, mountains, backgrounds; it is the wrong tool for a 16px sprite where the silhouette has to be deliberate. Keep the working size in the 64-128 range - under 64 there is no room for the blobs, over 128 it turns into ordinary digital painting.",
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
        id: "temperature",
        title: "Temperature is relative, and the environment rewrites colour",
        keywords: &[
            "冷暖", "冷色", "暖色", "环境色", "色温", "反射上来的颜色", "桌面颜色", "旁边的影响",
            "相对冷暖", "同一种颜色不同感觉", "反光", "底部带红", "带上周围", "受环境影响",
            "红色桌面上", "光从哪来", "光源色",
            "warm and cool", "colour temperature", "color temperature", "relative warmth",
            "environment tint", "environmental colour", "bounced tint",
        ],
        body: "Warm and cool are relative, not properties of a hue: the same grey looks cool beside red and warm beside blue, and the same red is the warm half against one neighbour and the cool half against another. Judge temperature by comparison, never in isolation. The surroundings rewrite colour far harder than most people expect - a yellow ball resting on a red table picks red up into its underside, because light bounces off the table into the shadow. So tint the shadow side toward the environment and let that bounced hue be the warmest note in the shadow. The light source changes it again: white, warm and cold light give three different readings of one object.",
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
        body: "Read width and height from the canvas context and derive EVERY coordinate from them - a script with literal constants tuned for 64px blasts off the edge of a 24px canvas. Under 16 pixels the silhouette is the whole picture and outlines hurt; at 32-64 spend it on two or three signature details plus a 3-step ramp; at 96 and above loops and math pay for themselves and you can carry 4-5 ramp steps per material. Use canvas.scale(k) for a 64px-design constant, canvas.grid(cols, rows) for a cell grid, and canvas.cx / canvas.cy instead of a hand-computed centre. Colour budget follows the canvas: 4 at 8x8, 8 at 16x16, 16 at 24 and 32, 24 at 48x48, 32 at 64x64, 48 at 128x128, 64 at 256, 96 at 512, 128 at 1024, 256 at 2048 - more colours on a small canvas buys noise, not detail. Standard sizes worth knowing by use: 8x8 for icons and small props, 16x16 for classic-game props and small characters, 32x32 for a character whose face has to read, 64x64 for portraits and detail-heavy characters, 29x29 for one large square bead board, 128 and up for scenes. One project keeps ONE ratio: mixing sizes looks like assets pasted together from different games. Climb the ladder rather than jumping: 16x16 with 4 colours, then 32x32 with 12 colours and pixel-perfect 1px lines plus a first outline, then 48x48 with 16 colours, and only then scenes around 100x64. Stay on a rung until the pixels stop feeling like a puzzle, and skip animation entirely until static pieces are comfortable.",
    },
    KnowledgeEntry {
        id: "starter-subjects",
        title: "First subjects with size and colour count settled",
        keywords: &[
            "入门", "新手", "练习", "练手", "画什么", "题材", "创意", "点子", "灵感", "想画",
            "心形", "笑脸", "披萨", "机器人", "骑士", "巫师", "第一个作品",
            "beginner", "starter", "practice", "ideas", "idea", "what to draw",
            "first sprite", "heart", "smiley", "pizza", "robot", "knight", "wizard",
        ],
        body: "Subjects whose size and colour count are already settled - a first piece should be a decision, not a blank page: heart 11x11 in 2 colours, smiley 15x15 in 3, apple and ghost at 4, robot face 8x8 in 3, walking knight 16x16 in 5, wizard 24x32 in 7, dragon 32x32 in 8, pizza 24x24 in 6. Three habits carry every one of them: exaggerate the single most recognisable feature, mirror half the canvas when the subject is genuinely symmetric, and keep one flat backing colour across a sticker-style set. Small canvases are the training ground - at 10x10 a couple of pixels is a whole expression change, and that weight is easy to miss on a big sheet.",
    },
    KnowledgeEntry {
        id: "quadruped",
        title: "Quadrupeds: cats, dogs and four-legged bodies",
        keywords: &[
            "四足", "走兽", "兽", "猛兽", "猫", "狗", "兔", "狐狸", "狼", "熊", "鹿",
            "quadruped", "beast", "four legs", "cat", "dog", "fox", "wolf", "bear", "deer",
            "rabbit",
        ],
        body: "A side-view quadruped is ONE body mass plus four separate legs: two verticals under the shoulder, two under the hip, all four landing on the same ground line. The spine sags slightly in the middle, the chest sits higher than the belly, and the head rides level with the back rather than above it. Keep a visible gap between the near and far pair so the stance reads three-dimensional, shade the far legs a step darker, and give the tail its own curve with a phase lag. In a quadruped walk the four legs run a four-beat cycle, and the back leg on one side follows the front leg of the OPPOSITE side by about a quarter cycle. Both diagonal legs must travel the same distance or the body stretches and contracts like an accordion, and the hip and the shoulder undulate in opposite but equal amounts so the back keeps a balanced rhythm. Break the anatomy into the same colour-coded parts as a biped, animate one pair of legs at a time, then the body, head and tail - and remember that a dog at speed does not walk, it bounds with the left and right legs working in near-parallel.",
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
        body: "A bird in profile is a teardrop body with a triangular beak at the front and a fan of tail feathers at the back, and the eye sits high and forward close to the beak. A folded wing reads as two or three overlapping bands along the body with the longest feather on the bottom; a spread wing is a long tapering blade with a notched tip. Legs are two thin lines to a three-toed foot, and a perched bird is mostly silhouette plus one belly shade. Animating a wing needs a marked joint: draw the wing from the FRONT first as a wireframe, mark the shoulder/elbow fold so the wing is not a rubbery sheet, then animate only the wings before adding body bob. The extremes - wings fully up and fully down - are the keyframes, so paint those first and add the smear between them. Eight frames describe the full arc, but fewer read fine at small sizes; thicken the leading edge and show a few tail feathers once the motion works.",
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
        body: "A tree is a trunk plus a canopy mass: the trunk tapers as it rises, splits into two or three limbs that reach INTO the canopy, and takes a vertical grain. The canopy is a cluster of overlapping lobes of two or three sizes with only the outer edge varied. Conifers are stacked triangles with a flat bottom; broadleaf trees are round masses with notches. Keep one light direction on every lobe and let branch tips read as 2x2 clusters. The quick modular route to a leafy tree: build ONE leaf bundle out of a repeated geometric unit (a rhombus, a 2x2 square, a circle), make a darker and a lighter variant on the same ramp, and layer the three according to one light source. Consistent leaf form and even distribution are what hide the repeat - a single odd-sized cluster draws the eye - and 4-5 colours per bundle is plenty; if the bundle only looks right after heavy touch-ups, the bundle itself is the problem. A conifer is built the other way: lay a stick skeleton, then draw the branches from top to bottom so the lower ones are overlapped, reflect the side branches to the opposite side before shading, and split them into light, mid and dark with the upper branches casting small shadows on the ones below.",
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
            "房子", "房屋", "建筑", "屋子", "塔楼", "城墙", "桥", "门", "窗", "城堡", "堡垒",
            "尖塔", "屋顶", "石墙", "砖墙",
            "house", "building", "tower", "wall", "bridge", "door", "window", "hut",
            "castle", "fortress", "spire", "roof", "brick wall",
        ],
        body: "Architecture reads from one-box massing plus roof plus openings: settle the volumes as flat filled shapes first, then cut windows and doors as darker rectangles that never touch the wall edge except at the bottom. Roofs are the lightest plane because they face the sky, and walls take their shade from the same light direction. Keep every vertical exactly vertical and every roofline on one shared angle - mixed angles break the whole structure. A wall tile is designed exactly like a ground tile, but the CONTRAST against the neighbouring top and sides is what sells the rise: the top of a wall is much brighter than its sides, and a brick laid with its long narrow face out means the bricks on a side wall are shorter than on a top wall. Use a column or a universal corner piece instead of drawing four bespoke corner variants - the pay-off is not worth the extra tiles, and the column doubles as decoration. A wall set needs drop shadows to read, but keep every shadow short and no longer than one tile regardless of how tall the structure looks, cast mainly to one side and slightly downward, or long shadows fight the sprites layered over them. For a fantasy castle, start from a roof-capped tower - it sets the palette and the light direction - then block the whole silhouette as a rough plan and keep reusing those colours; the compact repetition of towers, turrets and spires over one shared angle is what reads as a castle rather than a pile of boxes.",
    },
    KnowledgeEntry {
        id: "city-building",
        title: "Top-down city building and town blocks",
        keywords: &[
            "城市", "城市建造", "城镇", "小镇", "街区", "郊区", "俯视建筑", "俯视房屋",
            "道路", "公路", "马路", "路口", "校园", "教堂",
            "city builder", "city", "town", "urban", "suburb", "road", "street",
            "rooftop", "church",
        ],
        body: "A top-down town is assembled from buildings that all sit on the SAME 16x16 tile foundation as the terrain, so every structure snaps to the grid the roads and lawns already use. A building may break the TOP edge of its footprint by a few pixels - that overhang is what lets the roof or a chimney overlap the tile behind it and quietly adds depth - but it must NEVER break the bottom or the sides of the footprint, because that is where the layering order is decided and a broken side collides with the neighbouring tile. Build one roof-first and work downward: block the roof and walls as flat masses, cut the windows and doorways as darker openings, drop shadows under the eaves and inside every recess from the one light direction, and only then outline. Skip a straight black outline for a line one or two steps darker than the pixels it touches, so the asset reads clean against any ground. Finish by fleshing out the property around the foundation and casting its drop shadow onto the ground, and size that property on the same 16x16 grid so lawns, driveways and parking lots line up with the map. Lay the town out as a grid of blocks joined by roads, where a road is just a terrain tile carrying a traffic line, and keep every roof, wall and ground plane lit from the same direction. The same recipe scales from a single cottage to a school, office, church or civic building - only the massing and the number of stops change.",
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
    KnowledgeEntry {
        id: "spreadsheet-index",
        title: "Index grids: spreadsheets and numbered cells",
        keywords: &[
            "电子表格", "表格", "编号", "索引色", "数字编码", "网格编号", "填格子", "色号表",
            "spreadsheet", "excel", "sheets", "numbered grid", "index numbers", "csv",
            "grid of numbers",
        ],
        body: "A pixel sheet also works as a grid of INDEX numbers instead of colours, and that buys three things: recolouring the whole image is one conditional-format rule per colour, the per-colour pixel count comes from a single COUNTIF (which is also the bead and embroidery bill), and a mirror is `=H1` dragged sideways. Four ways to fill the grid: by hand, with a number palette plus rules, by pasting an image, or by importing a pattern CSV. Keep one cell per pixel, one row of cells per canvas row, and always deliver a numbered grid with a legend - a legend-free number sheet is unreadable.",
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

/// 动笔那一轮的行为准则段。规格锁、收工自检、别画成程序化假图案三条全数下发：
/// 用户提不提醒都改成它们的形状——「别画成程序化假图案」这种话没人会主动说，
/// 可它恰恰是程序化瓦片一眼假的全部原因。
///
/// 纯问答轮次不要这一段：三轮护栏压在一句「你好」上面，只会把闲聊也变成施工队。
/// 所以调用方判「这一轮会不会动笔」，判据在 `plan::TurnPlan::draws`。
pub fn discipline_section() -> String {
    let ids: Vec<String> = DISCIPLINE_IDS.iter().map(|id| (*id).to_string()).collect();
    let hits = ids
        .iter()
        .filter_map(|id| ENTRIES.iter().find(|entry| entry.id == id))
        .collect();
    render_titled(
        hits,
        DISCIPLINE_BUDGET,
        "Working discipline for every drawing in this project (binding whether or not this \
         sentence mentions it):\n",
    )
}

fn render(hits: Vec<&'static KnowledgeEntry>, budget: usize) -> String {
    render_titled(
        hits,
        budget,
        "Relevant craft notes for this request (retrieved from the built-in knowledge base; \
         apply the parts that fit and ignore the rest):\n",
    )
}

/// 摆条目：标题一行，一条一行。`budget` 按字符数封顶，够几条就几条，
/// 装不下的整条舍弃而不是截半——半条规矩比没给更容易被模型照错那一半执行。
fn render_titled(hits: Vec<&'static KnowledgeEntry>, budget: usize, title: &str) -> String {
    if hits.is_empty() {
        return String::new();
    }
    let mut out = String::from(title);
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

    /// 行为准则三条是动笔才有的护栏：整段在，而且和检索条目各走各的预算。
    /// 互不挤占才有意义——护栏插队的话，「画 5 帧奔跑」那一轮就带不上步态相位表，
    /// 而不是反过来多一条护栏。
    #[test]
    fn the_discipline_trio_rides_its_own_budget() {
        let section = discipline_section();
        for title in [
            "Lock the spec before drawing",
            "Self-check before calling anything done",
            "Patterns that make procedural work look fake",
        ] {
            assert!(section.contains(title), "准则段少了「{title}」：{section}");
        }
        // 三条 id 都在库里：改坏了名字，护栏会静悄悄地整段消失，
        // 而消失的护栏不报错，只是在成品里露出马脚。
        for id in DISCIPLINE_IDS {
            assert!(
                ENTRIES.iter().any(|entry| entry.id == id),
                "知识库里没有 {id}"
            );
        }
        assert!(
            section.chars().count() <= DISCIPLINE_BUDGET,
            "准则段超预算了"
        );
        // 检索那一路照旧满员：护栏不占名额，这一条是这段设计成立的前提。
        assert!(retrieve("画5帧橘猫奔跑", DEFAULT_LIMIT)
            .iter()
            .any(|e| e.id == "locomotion"));
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

    /// 规范层那几条新条目要真能被原话捞出来。用户嘴上说的都是
    /// 「画个 UI 面板给 unity」「来个带攻击动作的角色」这种带契约的话，
    /// 这几条检索不到就等于白说——而它们恰好是决定成败的那一半。
    #[test]
    fn the_spec_layer_entries_surface() {
        for (query, id) in [
            ("画一套可平铺的草地瓦片，要无缝", "tilemap"),
            ("给我的 unity 项目导出这套序列帧", "target-platform"),
            ("瓦片边缘怎么衔接才不露缝", "tile-edges"),
            ("画一个 app 图标，注意色数", "pixel-discipline"),
            ("帮我列一下这套素材的交付分类", "asset-classes"),
            ("用 db32 的配色画一个角色", "palette-library"),
            ("配上 idle 和 attack 的动作，各多少帧", "anim-catalog"),
            ("角色要上下左右四个朝向，每个朝向一套行走", "sheet-layouts"),
        ] {
            let hits = retrieve(query, 4);
            assert!(
                hits.iter().any(|e| e.id == id),
                "'{query}' 没捞出 {id}：{:?}",
                hits.iter().map(|e| e.id).collect::<Vec<_>>()
            );
        }
        // 「平滑曲线」是 smooth-curves 的主场，别因为 pixel-discipline 也收了
        // 抗锯齿就被抢走——同分时靠库内顺序兜底，那太脆。
        assert_eq!(
            retrieve("尾巴画条平滑的抗锯齿曲线", 4)[0].id,
            "smooth-curves"
        );
        // 长条目全命中也要压得住预算。
        let text = prompt_section(
            "瓦片 无缝 平铺 边缘 引擎 unity db32 色数 交付 分类 动作表",
            8,
            DEFAULT_BUDGET,
        );
        assert!(
            text.chars().count() <= DEFAULT_BUDGET,
            "{}",
            text.chars().count()
        );
    }

    /// MakeBead 教程吸进来的那几条要真能被原话捞出来。用户嘴上说的是
    /// 「怎么开始画」「加抗锯齿加多了」「像枕头一样」这种口语，
    /// 捞不到就等于教程只活在了源码里。
    #[test]
    fn the_craft_process_entries_surface() {
        for (query, id) in [
            ("新手入门不知道从哪儿开始画", "drawing-process"),
            ("先画剪影再平涂最后细化，分几遍比较好", "drawing-process"),
            ("阴影一圈圈往里加，看起来像个枕头", "pillow-shading"),
            ("抗锯齿加多了，形状都被改变了", "aa"),
            ("线条拐角有双像素，粗细不均匀", "line-quality"),
            ("这条曲线画在老屏幕上会不会糊掉", "crt-screen"),
            ("16位风格参考图，原版当年是 CRT 显示的", "crt-screen"),
            ("画一个拼豆图纸，要统计每种颜色的数量", "physical-media"),
            ("用十字绣的方式做一个像素图", "physical-media"),
            ("卡哇伊风格的配色，给一套现成的", "style-recipes"),
            ("万圣节暗黑配色，五个颜色", "style-recipes"),
            ("第一次画像素画，练手画点什么好", "starter-subjects"),
            ("在电子表格里用编号填格子", "spreadsheet-index"),
            // Cure《The Pixel Art Tutorial》里几块过去没单独成条的规则。
            ("描边想断开来蹭背景色", "sel-out"),
            ("阴影等宽贴着轮廓，网格都显出来了", "banding"),
            ("分不清该画成线还是画成体块，块状像素怎么处理", "form-first"),
            ("颜色太艳了有点刺眼，灰阶跨度也不够", "palette-control"),
            ("交付前把它翻转看看，再去色核对明暗", "final-checks"),
            // Pedro Medeiros 两篇与 Slynyrd Pixelblog 60 吸进来的规则。
            ("色块起稿怎么画，不打线稿行不行", "cluster-sketching"),
            ("受限调色板要做色相替换，高光该借哪个色", "ramps"),
            ("小画布上的孤立像素该不该删掉", "pixel-clusters"),
            // 半成品不能按百分比缩放，只能整倍放大。
            ("把像素图放大 107% 可不可以", "upscale"),
            // 横版跑射角色的上下半身分层与移动射击。
            ("横版射击游戏角色，边跑边开枪怎么分层", "run-and-gun"),
            ("跑射角色跳跃动作和落地动作怎么做", "run-and-gun"),
            ("横版瓦片的角块和接缝怎么处理", "tile-edges"),
            // saint11 教程合集里单独成篇的几块：特效、俯视方向。
            ("爆炸特效的火光和烟雾怎么分帧", "vfx"),
            ("打中敌人的时候要有一帧闪光", "vfx"),
            ("俯视四方向的地图角色怎么保证不跑形", "topdown"),
            ("上下左右的行走图朝向要各画一遍吗", "topdown"),
            // Slynyrd Pixelblog 58/61/62/63 吸进来的规则。
            ("横版射击的背景要分层滚动，每层速度不一样", "parallax"),
            ("卷轴背景每一层滚动速度怎么定", "parallax"),
            ("画一个山谷风景背景，要有纵深", "landscape-bg"),
            ("沙漠场景背景的颜色怎么分带", "landscape-bg"),
            ("画一个弹幕射击游戏的飞机", "shmup"),
            ("8x8 瓦片怎么做微型像素城市", "tiny-tiles"),
            // 俯视八方向角色与等轴机甲的姿势/分层规则。
            ("八方向角色跑步，剑和盾要保持左右手一致", "topdown"),
            ("32x32 等轴机甲怎么起形", "isometric"),
            // Slynyrd Pixelblog 49/50/52/53 吸进来的人体、格斗与近战规则。
            ("格斗待机怎么做出呼吸感", "combat-idle"),
            ("挥剑动画的帧时长怎么排", "melee-attacks"),
            ("勾拳和前踢的动作怎么拆", "melee-attacks"),
            ("八头身人体比例怎么起稿", "human-anatomy"),
            ("写实人物走路的八帧怎么排", "walk-cycle"),
            // 手绘瓦片、水波动画、针叶树与城堡这几条扩充规则。
            ("手绘瓦片重复的时候怎么藏接缝", "tilemap"),
            ("16px 的砖墙按几像素砌才不破", "tilemap"),
            ("水面瓦片怎么做两帧循环动画", "water"),
            ("松树的枝条从上往下画还是从下往上", "tree"),
            ("等轴斜圆和 2:1 标尺线怎么画", "isometric"),
            ("城堡从哪个部分开始起稿", "building"),
            ("沙子地砖的纹理怎么做才不生硬", "sand-terrain"),
            ("墙面瓦片怎么从正面墙复用", "sand-terrain"),
            ("角色尺寸按几个瓦片单位算", "sprite-scale"),
            ("原生分辨率选多少才能像素完美", "sprite-scale"),
            // Slynyrd Pixelblog 17/23/47/51 吸进来的面部比例、像素完美滚动、
            // 微型分层与俯视城镇规则。
            ("三庭五眼怎么分，五官位置怎么定", "human-anatomy"),
            ("写实人物的面部比例怎么起稿", "human-anatomy"),
            ("视差背景每层每帧移动多少个像素才平滑", "parallax"),
            ("视差滚动的循环帧数和画布宽度怎么对应", "parallax"),
            ("8x8 瓦片的阴影要不要单独分一层", "tiny-tiles"),
            ("画一个俯视的小镇，房子沿着街道排", "city-building"),
            ("城市建造游戏的道路和街区怎么规划", "city-building"),
            ("俯视建筑的屋顶能不能超出地基一点", "city-building"),
        ] {
            let hits = retrieve(query, 4);
            assert!(
                hits.iter().any(|e| e.id == id),
                "'{query}' 没捞出 {id}：{:?}",
                hits.iter().map(|e| e.id).collect::<Vec<_>>()
            );
        }
    }

    /// 行为准则那几条要真能被原话捞出来，还不能抢走配色库的主场：
    /// 「红白机风格」该定到 NES 分级，「8位配色」却还是该去 style-recipes
    /// 取现成 hex。这两句只差两个字，混一条就把配方冲没了。
    #[test]
    fn the_discipline_entries_surface_and_stay_in_their_lane() {
        for (query, id) in [
            ("红白机风格，角色 16x16", "style-tiers"),
            ("16位机风格来一套", "style-tiers"),
            ("先把尺寸和配色定下来再画", "spec-lock"),
            ("规格别做到一半又改", "spec-lock"),
            ("画个草地瓦片，别一看就是假的", "fake-patterns"),
            ("程序化生成的石头路太规律了", "fake-patterns"),
            ("交付前自己检查一遍", "quality-gate"),
            ("冷暖到底怎么判断", "temperature"),
            ("放在红桌面上球的底部带红色", "temperature"),
        ] {
            let hits = retrieve(query, 4);
            assert!(
                hits.iter().any(|e| e.id == id),
                "'{query}' 没捞出 {id}：{:?}",
                hits.iter().map(|e| e.id).collect::<Vec<_>>()
            );
        }
        // 分级只管尺寸和色数，hex 配方还是 style-recipes 的主场。
        for (query, preferred) in [
            ("8位配色", "style-recipes"),
            ("粉彩配色，给一套现成的", "style-recipes"),
            ("用 db32 的配色画一个角色", "palette-library"),
            ("优化一下细节", "refine"),
            ("画一个 app 图标，注意色数", "pixel-discipline"),
        ] {
            assert_eq!(retrieve(query, 4)[0].id, preferred, "{query}");
        }
    }

    /// 这些新条目的触发词最容易踩两种坑：抢走老条目的主场，或者被
    /// 常见英文词的半截误命中。前者会让「优化一下细节」这种高频追问
    /// 丢掉 refine，后者会把一段枕形阴影规矩塞进一句闲聊。
    #[test]
    fn the_craft_process_entries_stay_in_their_lane() {
        // refine、form-shading、specular、smooth-curves 的主场一个字都没提。
        for (query, preferred) in [
            ("优化一下细节", "refine"),
            ("细节再写实一点", "refine"),
            ("立体感不够，明暗交界太生硬", "form-shading"),
            ("金属高光再亮一点", "specular"),
            ("尾巴画条平滑的抗锯齿曲线", "smooth-curves"),
        ] {
            assert_eq!(retrieve(query, 4)[0].id, preferred, "{query}");
        }
        // 术语新条目不许把「画一只猫」这种安静提问带偏。
        let named: Vec<&str> = retrieve("画一只坐着的小猫", 4)
            .iter()
            .map(|e| e.id)
            .collect();
        for wrong in [
            "starter-subjects",
            "style-recipes",
            "crt-screen",
            "physical-media",
        ] {
            assert!(
                !named.contains(&wrong),
                "'画一只坐着的小猫' 误命中 {wrong}：{named:?}"
            );
        }
        // 拉丁触发词整词算：lifestyle 里的 style 不该把配方条目拖进来。
        let styled: Vec<&str> = retrieve("a lifestyle scene", 4)
            .iter()
            .map(|e| e.id)
            .collect();
        assert!(!styled.contains(&"style-recipes"), "{styled:?}");
    }

    /// RPG Maker 角色行走图和纸娃娃白膜这两条要真能被原话捞出来。
    /// 「引擎按行列号直接切图」是这两张图的契约：列数错了整张图错位一行，
    /// 所以「4行3列」这句话必须精确落到 rpmaker-sheet，而不是落进
    /// sheet-layouts 拿到一段泛泛的排版规矩。
    #[test]
    fn the_rpmaker_and_paperdoll_entries_surface() {
        // 「行走图」是 RPMaker 那一行的原话，得压过同认这两个字的 sheet-layouts。
        let hits = retrieve("给 RPG Maker MV 做一个四向行走图，4行3列", 4);
        let named: Vec<&str> = hits.iter().map(|e| e.id).collect();
        assert!(named.contains(&"rpmaker-sheet"), "{named:?}");
        // RPMaker 那张固定的行列网格必须排在泛泛的排版模板之前：
        // 「按行列号切图」的契约错了，整张图错位一行，排版模板救不回来。
        let sheet_rank = named.iter().position(|&id| id == "sheet-layouts");
        assert!(
            named.iter().position(|&id| id == "rpmaker-sheet") < sheet_rank,
            "{named:?}"
        );
        // 纯步态提问还是归步态——「行走图」两个字两边都认，这条断言是门口的保安。
        assert_eq!(retrieve("画一个八帧的行走图", 4)[0].id, "walk-cycle");
        // 白膜要连着「底稿」「部件」才不被 RPMaker 那张网格抢走：白膜只是
        // 铺底稿的做法之一，「先铺白膜底稿，部件分区」才是它的主场。
        let base = retrieve("先铺白膜底稿，部件分区，再上色", 4);
        let base_ids: Vec<&str> = base.iter().map(|e| e.id).collect();
        assert_eq!(base[0].id, "paperdoll-base", "{base_ids:?}");
        // 换装类提问也该带上来。
        assert!(retrieve("做个能换装的纸娃娃角色", 4)
            .iter()
            .any(|e| e.id == "paperdoll-base"));
    }
}
