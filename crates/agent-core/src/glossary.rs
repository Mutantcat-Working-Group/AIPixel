// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 美术概念/专有名词表：中英双语 + 别称 + 一句白话解释。
//!
//! 用户嘴里的「色阶」「节奏」「剪影」和模型训练语料里的叫法常常对不上，
//! 尤其国内教程和海外教程同一件事两套名字。这张表的作用就是把两边接起来：
//! 模型读到「勾线」知道去搜 outline，听到「残影帧」知道那是 smear frame。
//! 每条只给一句解释，够模型对齐语义即可，不做教科书。

/// 一个术语。`en` 是模型最认的英文写法，`zh` 是界面与中文用户那边的名字。
pub struct GlossaryEntry {
    pub en: &'static str,
    pub zh: &'static str,
    /// 别称：行业写法与个人习惯叫法，命中任意一个都指这一条。
    pub aliases: &'static [&'static str],
    /// 一句白话：怎么用它，别用来干什么。
    pub note: &'static str,
}

/// 按「上色 -> 造型 -> 运动」分组，模型按组找概念比按字母表找更快落地。
pub const GLOSSARY: &[GlossaryEntry] = &[
    GlossaryEntry {
        en: "dithering",
        zh: "抖动",
        aliases: &["递色", "混色抖动"],
        note: "两种颜色按规律交替排列，远看像中间色；像素画里比多引入一个色阶更省。",
    },
    GlossaryEntry {
        en: "ordered dithering",
        zh: "有序抖动",
        aliases: &["Bayer 抖动", "规则抖动"],
        note: "用固定的 Bayer 矩阵网点交错两色，纹理均匀、方向感强，适合大理石皮肤和天空。",
    },
    GlossaryEntry {
        en: "noise dithering",
        zh: "噪声抖动",
        aliases: &["随机抖动"],
        note: "按随机阈值散点交错两色，比有序抖动有机，但靠近了容易显脏。",
    },
    GlossaryEntry {
        en: "dither threshold",
        zh: "抖动阈值",
        aliases: &["抖动比例"],
        note: "两色各占多少的比例阈值；调它等于调「混出多深的中间色」。",
    },
    GlossaryEntry {
        en: "anti-aliasing",
        zh: "抗锯齿",
        aliases: &["AA", "柔边"],
        note: "曲线和背景交界处用一两个过渡色收边，去掉毛刺；小尺寸和勾线上别用，会糊。",
    },
    GlossaryEntry {
        en: "jaggies",
        zh: "锯齿",
        aliases: &["阶梯感", "毛刺"],
        note: "低分辨率下斜线呈阶梯的缺陷；靠固定步长节奏而不是多加色来解决。",
    },
    GlossaryEntry {
        en: "doubles",
        zh: "双像素",
        aliases: &["双格", "成对像素", "双像素线"],
        note: "同一段边上并排两个像素，让线条多占一倍面积；该用来校准粗细，不是把轮廓整体拉粗。",
    },
    GlossaryEntry {
        en: "pixel cluster",
        zh: "像素簇",
        aliases: &["簇", "成组像素"],
        note: "单个像素读不出形状，2x2 以上的簇才有存在感；细节按簇摆，别撒孤点。",
    },
    GlossaryEntry {
        en: "orphan pixel",
        zh: "孤立像素",
        aliases: &["孤点", "散点", "噪音点"],
        note: "四周都是别的内容的单个像素，只读成脏点；删掉，或者并进相邻的像素簇。",
    },
    GlossaryEntry {
        en: "crt screen",
        zh: "老显示器",
        aliases: &["CRT", "显像管", "老电视", "模拟信号"],
        note: "老电视会把相邻像素糊在一起；网上那些干净截图不等于当年主机上的样子。",
    },
    GlossaryEntry {
        en: "bezier curve",
        zh: "贝塞尔曲线",
        aliases: &["平滑曲线", "曲线", "弧线", "圆滑", "二次曲线", "三次曲线", "bezier", "spline"],
        note: "用控制点描述的平滑曲线；落到整数格上会变台阶，细节级的曲线改用 aacurve / aacubic 按真实覆盖率画。",
    },
    GlossaryEntry {
        en: "feathering",
        zh: "羽化",
        aliases: &["软化边缘", "收边", "柔化", "feather", "soften"],
        note: "边缘按覆盖率逐步过渡到背景色，一两个像素就够；曲线细节用 aaline / blend，轮廓线上别用。",
    },
    GlossaryEntry {
        en: "banding",
        zh: "色带",
        aliases: &["色带断层", "条纹"],
        note: "渐变跨色阶时出现的硬边条；用抖动接两个色阶而不是补一个中间色。",
    },
    GlossaryEntry {
        en: "pillow shading",
        zh: "枕形阴影",
        aliases: &["枕头阴影", "一圈圈加暗", "平行条纹阴影"],
        note: "从轮廓往外一圈圈往里加暗的坏习惯：看着鼓胀发平；阴影该跟着形体和真实光源走。",
    },
    GlossaryEntry {
        en: "cel shading",
        zh: "色块着色",
        aliases: &["赛璐璐上色", "平涂"],
        note: "只用色块表现明暗、不加过渡的卡通上色；像素画的基本盘。",
    },
    GlossaryEntry {
        en: "realism",
        zh: "写实",
        aliases: &["逼真", "照片级", "拟真", "真实感", "realistic", "photoreal"],
        note: "在像素格内靠色阶、形体光影和柔边增加信息量，不是把边缘画糊、也不是挪出格外。",
    },
    GlossaryEntry {
        en: "ramp",
        zh: "色阶",
        aliases: &["阶梯", "明度区间"],
        note: "同一材质从亮到暗的一组递进色；每个材质 3-5 阶，暗部偏冷、亮部偏暖。",
    },
    GlossaryEntry {
        en: "hue shift",
        zh: "色相偏移",
        aliases: &["色相推移", "偏色"],
        note: "明部和暗部不只是改明度，还各偏一点色相；比纯加白加黑通透得多。",
    },
    GlossaryEntry {
        en: "value",
        zh: "明度",
        aliases: &["明暗", "明度值"],
        note: "颜色亮暗本身；大关系先看明度，再看色相。",
    },
    GlossaryEntry {
        en: "saturation",
        zh: "饱和",
        aliases: &["纯度", "彩度"],
        note: "颜色鲜艳程度；远小画面降一点饱和更像实物，全高饱容易糖分超标。",
    },
    GlossaryEntry {
        en: "hsv",
        zh: "HSV 色彩模型",
        aliases: &["色相饱和度明度", "HSB", "hsv()", "色相饱和明度"],
        note: "色相定是什么颜色、饱和定多艳、明度定多亮；写脚本调色时拧明度做色阶、微调色相做冷暖，别拿十六进制硬抠。",
    },
    GlossaryEntry {
        en: "contrast",
        zh: "对比",
        aliases: &["明暗对比", "对比度", "反差", "高反差"],
        note: "相邻区域亮暗与色相的差距；把最强的一档对比放在主体上，视线自己就会跑过去。",
    },
    GlossaryEntry {
        en: "colour temperature",
        zh: "色温与冷暖",
        aliases: &["冷暖", "暖色", "冷色", "环境色"],
        note: "冷暖是比较出来的，同一块灰挨着红发冷、挨着蓝发暖；桌面地面会把自身的颜色反弹到物体的接触面上。",
    },
    GlossaryEntry {
        en: "bead count",
        zh: "每色用珠数",
        aliases: &["色号统计", "用豆量", "配线量"],
        note: "拼豆、刺绣、钻石画这类实体媒介的交付清单：每种颜色各需多少颗，照着备料才不会中途断色。",
    },
    GlossaryEntry {
        en: "lightness check",
        zh: "明度检查",
        aliases: &["黑白检查", "灰度检查", "去色检查", "desaturate"],
        note: "把整幅图去成灰色看一眼：只剩明度还读得出形体，才算立得住。",
    },
    GlossaryEntry {
        en: "core shadow",
        zh: "核心阴影",
        aliases: &["明暗交界线", "闭塞阴影"],
        note: "明暗交界处最窄的一条最暗色，比普通暗部重，是体积感的来源。",
    },
    GlossaryEntry {
        en: "terminator",
        zh: "明暗交界",
        aliases: &["受光边界", "阴阳界"],
        note: "受光面和背光面的分界；一条线，不是一个过渡区。",
    },
    GlossaryEntry {
        en: "form normal",
        zh: "法线",
        aliases: &["法向量", "表面朝向", "normal", "normals"],
        note: "表面上一点的朝向；椭圆上就是 ((x-cx)/rx, (y-cy)/ry)，跟光照向量点乘就能切色阶。",
    },
    GlossaryEntry {
        en: "cast shadow",
        zh: "投影",
        aliases: &["落地影", "投影区"],
        note: "物体挡光投出的影子；方向必须和光源一致，别省。",
    },
    GlossaryEntry {
        en: "ambient occlusion",
        zh: "环境光遮蔽",
        aliases: &["AO", "接触阴影"],
        note: "物体接触面缝隙里进不去光的那一层；只放在贴得最紧的地方。",
    },
    GlossaryEntry {
        en: "bounce light",
        zh: "环境反光",
        aliases: &["反弹光", "二次光", "bounced light"],
        note: "光打在周围物体上再弹回来的那一层；阴影里最暖最亮的一条，别让它变成新光源。",
    },
    GlossaryEntry {
        en: "rim light",
        zh: "轮廓光",
        aliases: &["边缘光", "反光"],
        note: "沿着受光轮廓的一圈亮色，把主体从背景里削出来。",
    },
    GlossaryEntry {
        en: "highlight",
        zh: "高光",
        aliases: &["亮部", "Highlight"],
        note: "光源直射那一点的最亮色；一处足矣，撒多了变成噪点。",
    },
    GlossaryEntry {
        en: "midtone",
        zh: "中间调",
        aliases: &["固有色层"],
        note: "固有色所在的层；整幅画信息量最大的一层，别让暗部吃掉它。",
    },
    GlossaryEntry {
        en: "matte",
        zh: "哑光材质",
        aliases: &["哑光", "无光泽", "漫反射材质", "matte surface"],
        note: "只吃漫反射、完全没有镜面的材质（布、土、石头、皮肤）；靠核心阴影和接触阴影出成果，别加高光。",
    },
    GlossaryEntry {
        en: "glossy",
        zh: "光泽材质",
        aliases: &["光泽", "半光", "光滑面", "glossy surface"],
        note: "镜面弱而散的材质，一段亮带加一条渐变尾巴；釉面、塑料、湿表面都算。",
    },
    GlossaryEntry {
        en: "metal",
        zh: "金属材质",
        aliases: &["金属", "镜面金属", "metallic"],
        note: "镜面又硬又窄、核心阴影几乎压到最暗的材质；亮带贴着光源转，尾迹带折角。",
    },
    GlossaryEntry {
        en: "outline",
        zh: "勾线",
        aliases: &["描边", "轮廓线"],
        note: "沿形体外缘走一圈色线；同一幅画只用一种策略：全勾、只勾背光面或不勾。",
    },
    GlossaryEntry {
        en: "selective outline",
        zh: "选择性勾线",
        aliases: &["局部勾线"],
        note: "只在背光和贴边处勾线，受光面留空；比全勾干净且更透气。",
    },
    GlossaryEntry {
        en: "limited palette",
        zh: "限制色板",
        aliases: &["限定色", "锁色"],
        note: "只用固定几个颜色作画；风格感的来源，比颜色多少重要。",
    },
    GlossaryEntry {
        en: "palette lock",
        zh: "色板锁定",
        aliases: &["配色锁", "颜色范围"],
        note: "图层只允许用范围表内的颜色，取色一律就近归队；本作每个图层独立。",
    },
    GlossaryEntry {
        en: "color index",
        zh: "颜色索引",
        aliases: &["色号", "索引色"],
        note: "调色板序号，0 恒为透明；结构操作按索引传，别传 hex 字符串。",
    },
    GlossaryEntry {
        en: "tile",
        zh: "瓦片",
        aliases: &["贴片", "拼接块"],
        note: "可无缝重复的最小图元；检查能不能平铺比单看好不好看重要。",
    },
    GlossaryEntry {
        en: "tileable",
        zh: "可平铺",
        aliases: &["无缝拼接", "四方连续"],
        note: "左右上下能接上不断裂；接缝处要把像素对齐着延续过去。",
    },
    GlossaryEntry {
        en: "sprite sheet",
        zh: "精灵图",
        aliases: &["雪碧图", "序列帧图"],
        note: "把多帧排进一张图；导出后给引擎切帧用。",
    },
    GlossaryEntry {
        en: "isometric",
        zh: "等轴",
        aliases: &["2:1 像素", "斜 45 度"],
        note: "两轴同一角度的斜投影；线条只走固定几个方向，不能混。",
    },
    GlossaryEntry {
        en: "parallax",
        zh: "视差",
        aliases: &["多层滚动", "远景差速"],
        note: "不同层按不同速度滚动出的纵深；层数不必多，速度差要明显。",
    },
    GlossaryEntry {
        en: "silhouette",
        zh: "剪影",
        aliases: &["外轮廓", "黑影"],
        note: "只靠外形识别角色；涂黑就能认出来才算过关。",
    },
    GlossaryEntry {
        en: "line art",
        zh: "线稿",
        aliases: &["清线", "勾线稿"],
        note: "只画轮廓和内部结构的干净线稿；像素画里先剪影后内部。",
    },
    GlossaryEntry {
        en: "hatching",
        zh: "排线",
        aliases: &["交叉线", "阴影线"],
        note: "用一组平行线表现暗部；像素画中抖动的方向版。",
    },
    GlossaryEntry {
        en: "sub-pixel",
        aliases: &["亚像素"],
        zh: "子像素",
        note: "1px 级的移动；把小数部分按比例画成两个格子的深浅，不要直接四舍五入。",
    },
    GlossaryEntry {
        en: "pixel crawl",
        zh: "像素游走",
        aliases: &["沸点", "蠕动"],
        note: "相邻帧像素乱跳导致的闪烁；静态纹理不要逐帧全随机。",
    },
    GlossaryEntry {
        en: "keyframe",
        zh: "关键帧",
        aliases: &["关键姿势", "原画"],
        note: "决定动作性格的那几帧；先摆关键帧，中间帧后补。",
    },
    GlossaryEntry {
        en: "inbetween",
        zh: "中间帧",
        aliases: &["补间", "中割"],
        note: "两张关键帧之间的过渡帧；帧动画里手画的，不是插值算的。",
    },
    GlossaryEntry {
        en: "breakdown",
        zh: "过渡帧",
        aliases: &["拆分帧"],
        note: "关键帧之间给运动定方向的那张；不是匀速的一半，是曲线的起点。",
    },
    GlossaryEntry {
        en: "hold",
        zh: "停格",
        aliases: &["定格", "保持"],
        note: "刻意让某一帧多停几拍；节奏的来源，动作不能帧帧都动。",
    },
    GlossaryEntry {
        en: "ease in/out",
        zh: "缓入缓出",
        aliases: &["加减速", "缓动"],
        note: "起步慢、到位慢、中间快；万物皆可用 smoothstep 重映射相位。",
    },
    GlossaryEntry {
        en: "smoothstep",
        zh: "平滑步进",
        aliases: &["平滑曲线"],
        note: "p*p*(3-2p)，把线性映射成头尾缓的曲线，最常用的缓动基元。",
    },
    GlossaryEntry {
        en: "overshoot",
        zh: "过冲",
        aliases: &["回弹", "变形"],
        note: "落点前多冲 10-20% 再收回；打击和落地靠它读出重量。",
    },
    GlossaryEntry {
        en: "smear frame",
        zh: "残影帧",
        aliases: &["速度线帧", "拉伸帧"],
        note: "极快动作不补帧，把形状沿轨迹拉成一道残影；比中间帧更带劲。",
    },
    GlossaryEntry {
        en: "onion skin",
        zh: "洋葱皮",
        aliases: &["透光台", "叠影"],
        note: "同时看到前后帧半透明轮廓；本作预览工具有这个开关。",
    },
    GlossaryEntry {
        en: "walk cycle",
        zh: "行走循环",
        aliases: &["走帧", "走路动画"],
        note: "四足侧视 4-8 帧，对角步态，末帧要接回首帧。",
    },
    GlossaryEntry {
        en: "run cycle",
        zh: "奔跑循环",
        aliases: &["跑帧"],
        note: "6-12 帧，有腾空帧；比走帧身体起伏大一倍，尾巴滞后更大。",
    },
    GlossaryEntry {
        en: "idle loop",
        zh: "待机循环",
        aliases: &["呼吸动画"],
        note: "2-4 帧的呼吸起伏；幅度小、周期长，角色活起来最省钱的办法。",
    },
    GlossaryEntry {
        en: "secondary animation",
        zh: "次级动画",
        aliases: &["跟随动作", "附属动画"],
        note: "尾巴、头发、披风这类附属物滞后主动作一点、幅度小一点。",
    },
    GlossaryEntry {
        en: "overlap action",
        zh: "重叠动作",
        aliases: &["延迟跟随"],
        note: "各部件的动作不完全同步，靠相位差产生弹性。",
    },
    GlossaryEntry {
        en: "anticipation",
        zh: "预备动作",
        aliases: &["蓄力", "预备"],
        note: "出招前先反向收一下；不蓄力的动作像被弹了一下。",
    },
    GlossaryEntry {
        en: "squash and stretch",
        zh: "挤压拉伸",
        aliases: &["压缩拉伸", "S&S"],
        note: "受力时沿运动方向压扁、反方向拉长；体积守恒，别只压不弹。",
    },
    GlossaryEntry {
        en: "frame duration",
        zh: "帧时长",
        aliases: &["驻帧时间", "帧率"],
        note: "一帧停留的毫秒数；默认约 83ms（12FPS），逐帧动画的节奏都靠它。",
    },
    GlossaryEntry {
        en: "cel",
        zh: "赛璐珞层",
        aliases: &["图层帧", "Cel"],
        note: "图层 x 帧上的那一格像素；本作 Canvas 里一个 cel 就是一份索引网格。",
    },
    GlossaryEntry {
        en: "loop wrap",
        zh: "循环闭合",
        aliases: &["收尾相接", "首尾相连"],
        note: "末帧和首帧能接上不跳；相位驱动动画必须取模。",
    },
    GlossaryEntry {
        en: "ping-pong loop",
        zh: "来回循环",
        aliases: &["乒乓循环"],
        note: "播到末尾再倒着播；省帧但容易显得黏。",
    },
    GlossaryEntry {
        en: "turn around",
        zh: "转身",
        aliases: &["三视图", "模型表"],
        note: "同一角色多个角度的成套设定；检查剪影一致性用。",
    },
];

/// 给提示词的术语表：一行一条，模型扫一遍就能把用户的叫法对上自己的概念。
pub fn prompt_table() -> String {
    render(
        "ART VOCABULARY - user wording to craft terms; use the technique when it fits:\n",
        GLOSSARY.iter().collect(),
    )
}

/// 术语表的按需裁剪。和颜色表的区别是这里不做「泛泛而谈就全给」的例外：
/// 这张表的作用是把用户嘴里的说法翻成模型认得的技法名，用户没提技法时
/// 核心条目就是全部所需；而颜色表裁得太狠会让模型以为「只准用这几个色」。
pub fn prompt_table_for(query: &str) -> String {
    let head = "ART VOCABULARY - user wording to craft terms; use the technique when it fits:\n";
    render(head, select(query))
}

/// 核心术语：每张像素画都要用上的那些。同类只留代表（`dithering` 留下，
/// `ordered dithering` / `noise dithering` 等点名再现身），理由同颜色表：
/// 同义词堆一排只会让模型在近邻之间犹豫。
const CORE_EN: &[&str] = &[
    "dithering",
    "anti-aliasing",
    "pixel cluster",
    "cel shading",
    "ramp",
    "hue shift",
    "core shadow",
    "cast shadow",
    "highlight",
    "outline",
    "selective outline",
    "limited palette",
    "palette lock",
    "silhouette",
    "sub-pixel",
    "keyframe",
    "frame duration",
    "smear frame",
    "walk cycle",
    "run cycle",
    "loop wrap",
    "squash and stretch",
    "tile",
    "tileable",
    "isometric",
];

/// 这一轮该带哪些术语。核心打底，命中的追加，顺序沿用原表（按上色->造型->运动
/// 分过组，乱序会让模型丢掉分组带来的语义）。
fn select(query: &str) -> Vec<&'static GlossaryEntry> {
    let lower = query.to_lowercase();
    let mut out: Vec<&GlossaryEntry> = GLOSSARY
        .iter()
        .filter(|entry| CORE_EN.contains(&entry.en))
        .collect();
    for entry in GLOSSARY.iter().filter(|entry| {
        std::iter::once(entry.en)
            .chain(std::iter::once(entry.zh))
            .chain(entry.aliases.iter().copied())
            .any(|name| super::terms::find_lower(&lower, &name.to_lowercase()).is_some())
    }) {
        if !out.iter().any(|kept| kept.en == entry.en) {
            out.push(entry);
        }
    }
    out
}

/// 表体。`entries` 的顺序即输出顺序，排的事调用方负责。
fn render(head: &str, entries: Vec<&GlossaryEntry>) -> String {
    let mut out = String::from(head);
    for entry in entries {
        out.push_str(&format!(
            "{} / {}{} - {}\n",
            entry.zh,
            entry.en,
            if entry.aliases.is_empty() {
                String::new()
            } else {
                format!(" (aka {})", entry.aliases.join(", "))
            },
            entry.note
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_names_are_unique() {
        let mut seen: Vec<&str> = GLOSSARY.iter().map(|e| e.en).collect();
        let before = seen.len();
        seen.sort();
        seen.dedup();
        // 同一条概念两个英文名会让模型分裂成两种做法。
        assert_eq!(before, seen.len(), "duplicate entry in GLOSSARY");
    }

    #[test]
    fn every_entry_carries_both_languages_and_a_note() {
        for entry in GLOSSARY {
            assert!(!entry.en.is_empty());
            assert!(!entry.zh.is_empty(), "{} has no Chinese name", entry.en);
            assert!(!entry.note.is_empty(), "{} has no note", entry.en);
        }
    }

    #[test]
    fn vocabularies_cover_the_craft_basics() {
        let table = prompt_table();
        for must in [
            "dithering",
            "smoothstep",
            "smear frame",
            "selective outline",
            "squash",
        ] {
            assert!(table.contains(must), "missing {must} in the glossary");
        }
    }

    #[test]
    fn every_core_term_really_is_in_the_glossary() {
        for en in CORE_EN {
            assert!(
                GLOSSARY.iter().any(|entry| entry.en == *en),
                "{en} 不在术语表里"
            );
        }
    }

    #[test]
    fn a_craft_word_pulls_its_entry_in() {
        // 用户嘴里的「过渡帧」要翻成模型认得的 breakdown。
        assert!(prompt_table_for("过渡帧要顺一点").contains("breakdown"));
        assert!(prompt_table_for("add some ambient occlusion").contains("ambient occlusion"));
    }

    #[test]
    fn the_core_terms_survive_a_quiet_query() {
        let full = prompt_table().chars().count();
        let quiet = prompt_table_for("画一只猫").chars().count();
        println!("glossary: full={full} trimmed={quiet}");
        assert!(quiet * 2 < full, "缩表没省下多少：{quiet} vs {full}");
        let table = prompt_table_for("画一只猫");
        assert!(table.contains("dithering"));
        assert!(table.contains("silhouette"));
        assert!(!table.contains("onion skin"), "没提洋葱皮就不该带上");
        assert!(table.lines().count() < prompt_table().lines().count());
    }

    #[test]
    fn a_latin_term_is_matched_whole() {
        // 整词才算：cancel 里的 cel 不该把赛璐珞层拖进这一轮。
        let table = prompt_table_for("cancel that");
        assert!(!table.contains("赛璐珞层"), "cel 被 cancel 误命中");
        assert!(!table.contains("cel -"), "cel 条目整行都不该出现");
    }
}
