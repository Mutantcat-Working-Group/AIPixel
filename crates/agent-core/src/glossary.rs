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
        en: "pixel cluster",
        zh: "像素簇",
        aliases: &["簇", "成组像素"],
        note: "单个像素读不出形状，2x2 以上的簇才有存在感；细节按簇摆，别撒孤点。",
    },
    GlossaryEntry {
        en: "banding",
        zh: "色带",
        aliases: &["色带断层", "条纹"],
        note: "渐变跨色阶时出现的硬边条；用抖动接两个色阶而不是补一个中间色。",
    },
    GlossaryEntry {
        en: "cel shading",
        zh: "色块着色",
        aliases: &["赛璐璐上色", "平涂"],
        note: "只用色块表现明暗、不加过渡的卡通上色；像素画的基本盘。",
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
    let mut out = String::from(
        "ART VOCABULARY - user wording to craft terms; use the technique when it fits:\n",
    );
    for entry in GLOSSARY {
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
}
