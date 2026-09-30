// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 常见颜色名表：hex + 中英文名 + 别称。
//!
//! 解决的是「用户说蓝，模型不知道该填哪个 hex」这件事。表按人眼能叫出名字的
//! 粒度收：同色系收敛到一两个代表色，不收商业色卡的几百个变体，
//! 否则模型在近邻之间反复横跳，用户也没法预期。
//!
//! 两个用途：给提示词一张对照表；给界面把 hex 翻成色名当 tooltip。

/// 一个带名字的颜色。hex 是不带透明通道的六位小写，和 `Rgba::to_hex` 对齐。
pub struct NamedColor {
    pub hex: &'static str,
    pub en: &'static str,
    pub zh: &'static str,
    /// 别称：方言、行话、行业写法都算。命中任意一个都指向这一色。
    pub aliases: &'static [&'static str],
}

/// 按色相环走一遍，同色系内由浅到深，模型按名字扫表时不那么容易漏。
pub const NAMED_COLORS: &[NamedColor] = &[
    NamedColor {
        hex: "#000000",
        en: "black",
        zh: "黑",
        aliases: &["纯黑", "黑色", "墨黑"],
    },
    NamedColor {
        hex: "#1a1a1a",
        en: "ink",
        zh: "墨色",
        aliases: &["近黑", "碳黑"],
    },
    NamedColor {
        hex: "#333333",
        en: "charcoal",
        zh: "炭灰",
        aliases: &["深灰", "炭黑"],
    },
    NamedColor {
        hex: "#666666",
        en: "gray",
        zh: "灰",
        aliases: &["中灰", "灰色", "银灰"],
    },
    NamedColor {
        hex: "#999999",
        en: "silver",
        zh: "银灰",
        aliases: &["浅灰", "银色"],
    },
    NamedColor {
        hex: "#cccccc",
        en: "ash",
        zh: "石灰",
        aliases: &["淡灰", "青灰"],
    },
    NamedColor {
        hex: "#ffffff",
        en: "white",
        zh: "白",
        aliases: &["纯白", "白色", "雪白"],
    },
    NamedColor {
        hex: "#8b0000",
        en: "dark red",
        zh: "深红",
        aliases: &["暗红", "褐红"],
    },
    NamedColor {
        hex: "#c0392b",
        en: "brick red",
        zh: "砖红",
        aliases: &["砖红色", "红砖"],
    },
    NamedColor {
        hex: "#e74c3c",
        en: "red",
        zh: "红",
        aliases: &["红色", "正红", "朱红"],
    },
    NamedColor {
        hex: "#ff6b6b",
        en: "coral red",
        zh: "珊瑚红",
        aliases: &["珊瑚色", "亮红"],
    },
    NamedColor {
        hex: "#ff9999",
        en: "salmon pink",
        zh: "鲑红",
        aliases: &["三文鱼色", "粉红"],
    },
    NamedColor {
        hex: "#ffccd5",
        en: "light pink",
        zh: "浅粉",
        aliases: &["淡粉", "粉"],
    },
    NamedColor {
        hex: "#ffb6c1",
        en: "pink",
        zh: "粉红",
        aliases: &["粉色", "樱花粉"],
    },
    NamedColor {
        hex: "#ff69b4",
        en: "hot pink",
        zh: "洋红",
        aliases: &["艳粉", "桃红"],
    },
    NamedColor {
        hex: "#c71585",
        en: "magenta",
        zh: "品红",
        aliases: &["洋红", "红紫"],
    },
    NamedColor {
        hex: "#8e44ad",
        en: "purple",
        zh: "紫",
        aliases: &["紫色", "正紫"],
    },
    NamedColor {
        hex: "#6c3483",
        en: "deep purple",
        zh: "深紫",
        aliases: &["暗紫", "茄紫"],
    },
    NamedColor {
        hex: "#9b59b6",
        en: "violet",
        zh: "蓝紫",
        aliases: &["紫罗兰", "藤紫"],
    },
    NamedColor {
        hex: "#d7bde2",
        en: "lilac",
        zh: "淡紫",
        aliases: &["丁香紫", "浅紫"],
    },
    NamedColor {
        hex: "#4a235a",
        en: "aubergine",
        zh: "茄紫",
        aliases: &["紫黑", "暗紫", "茄皮紫"],
    },
    NamedColor {
        hex: "#16a085",
        en: "teal",
        zh: "青绿",
        aliases: &["水鸭色", "蓝绿"],
    },
    NamedColor {
        hex: "#1abc9c",
        en: "turquoise",
        zh: "绿松石",
        aliases: &["松石绿", "青"],
    },
    NamedColor {
        hex: "#48c9b0",
        en: "light teal",
        zh: "浅青绿",
        aliases: &["淡青"],
    },
    NamedColor {
        hex: "#0077be",
        en: "azure",
        zh: "天青",
        aliases: &["青蓝", "蔚蓝"],
    },
    NamedColor {
        hex: "#2e86c1",
        en: "blue",
        zh: "蓝",
        aliases: &["蓝色", "正蓝"],
    },
    NamedColor {
        hex: "#3498db",
        en: "sky blue",
        zh: "天蓝",
        aliases: &["天空蓝", "淡蓝"],
    },
    NamedColor {
        hex: "#85c1e9",
        en: "light blue",
        zh: "浅蓝",
        aliases: &["淡蓝", "粉蓝"],
    },
    NamedColor {
        hex: "#1b4f72",
        en: "navy",
        zh: "藏青",
        aliases: &["海军蓝", "深蓝"],
    },
    NamedColor {
        hex: "#0b3d91",
        en: "deep blue",
        zh: "深蓝",
        aliases: &["暗蓝", "宝蓝"],
    },
    NamedColor {
        hex: "#2874a6",
        en: "steel blue",
        zh: "钢蓝",
        aliases: &["铁灰蓝"],
    },
    NamedColor {
        hex: "#5dade2",
        en: "cornflower",
        zh: "矢车菊蓝",
        aliases: &["长春花蓝"],
    },
    NamedColor {
        hex: "#cce5ff",
        en: "pale blue",
        zh: "极浅蓝",
        aliases: &["白蓝", "雾蓝"],
    },
    NamedColor {
        hex: "#145a32",
        en: "forest green",
        zh: "森绿",
        aliases: &["森林绿", "深绿"],
    },
    NamedColor {
        hex: "#1e8449",
        en: "green",
        zh: "绿",
        aliases: &["绿色", "正绿", "草绿"],
    },
    NamedColor {
        hex: "#27ae60",
        en: "leaf green",
        zh: "叶绿",
        aliases: &["叶绿色"],
    },
    NamedColor {
        hex: "#52be80",
        en: "light green",
        zh: "浅绿",
        aliases: &["淡绿"],
    },
    NamedColor {
        hex: "#a9dfbf",
        en: "mint",
        zh: "薄荷绿",
        aliases: &["薄荷色", "嫩绿"],
    },
    NamedColor {
        hex: "#7dcea0",
        en: "sage",
        zh: "鼠尾草绿",
        aliases: &["灰绿", "草灰绿"],
    },
    NamedColor {
        hex: "#f4d03f",
        en: "yellow",
        zh: "黄",
        aliases: &["黄色", "正黄", "明黄"],
    },
    NamedColor {
        hex: "#f7dc6f",
        en: "cream yellow",
        zh: "奶黄",
        aliases: &["奶油色", "淡黄"],
    },
    NamedColor {
        hex: "#d4ac0d",
        en: "mustard",
        zh: "芥末黄",
        aliases: &["土黄", "暗黄"],
    },
    NamedColor {
        hex: "#b7950b",
        en: "olive",
        zh: "橄榄绿",
        aliases: &["橄榄色", "军绿"],
    },
    NamedColor {
        hex: "#e67e22",
        en: "orange",
        zh: "橙",
        aliases: &["橙色", "橘", "橘黄"],
    },
    NamedColor {
        hex: "#d35400",
        en: "burnt orange",
        zh: "焦橙",
        aliases: &["深橙", "橘红"],
    },
    NamedColor {
        hex: "#f0b27a",
        en: "peach",
        zh: "桃",
        aliases: &["蜜桃色", "橘粉"],
    },
    NamedColor {
        hex: "#ca6f1e",
        en: "carrot",
        zh: "胡萝卜",
        aliases: &["橘黄"],
    },
    NamedColor {
        hex: "#a04000",
        en: "rust",
        zh: "锈色",
        aliases: &["铁锈红", "赭石"],
    },
    NamedColor {
        hex: "#873600",
        en: "brown",
        zh: "棕",
        aliases: &["棕色", "褐色"],
    },
    NamedColor {
        hex: "#6e2c00",
        en: "dark brown",
        zh: "深棕",
        aliases: &["暗棕", "咖啡"],
    },
    NamedColor {
        hex: "#935116",
        en: "light brown",
        zh: "浅棕",
        aliases: &["淡棕", "驼色"],
    },
    NamedColor {
        hex: "#c39b6d",
        en: "tan",
        zh: "浅褐",
        aliases: &["奶茶", "沙色"],
    },
    NamedColor {
        hex: "#e8c39e",
        en: "sand",
        zh: "沙色",
        aliases: &["米色", "杏色"],
    },
    NamedColor {
        hex: "#f5e6ca",
        en: "beige",
        zh: "米白",
        aliases: &["奶油白", "象牙"],
    },
    NamedColor {
        hex: "#dc7633",
        en: "terracotta",
        zh: "陶土",
        aliases: &["赤陶色"],
    },
    NamedColor {
        hex: "#7b5133",
        en: "walnut",
        zh: "胡桃木",
        aliases: &["木色", "深木"],
    },
    NamedColor {
        hex: "#a9714b",
        en: "oak",
        zh: "橡木",
        aliases: &["浅木色"],
    },
    NamedColor {
        hex: "#fdebd0",
        en: "linen",
        zh: "亚麻",
        aliases: &["麻白", "米黄"],
    },
    NamedColor {
        hex: "#fad7a0",
        en: "apricot",
        zh: "杏黄",
        aliases: &["浅橘"],
    },
    NamedColor {
        hex: "#fd79b8",
        en: "rose",
        zh: "玫红",
        aliases: &["玫瑰色"],
    },
    NamedColor {
        hex: "#e84393",
        en: "fuchsia",
        zh: "品红",
        aliases: &["桃红"],
    },
    NamedColor {
        hex: "#6d4c41",
        en: "umber",
        zh: "棕褐",
        aliases: &["生褐", "深褐"],
    },
    NamedColor {
        hex: "#f8c471",
        en: "skin light",
        zh: "浅肤",
        aliases: &["亮肤色"],
    },
    NamedColor {
        hex: "#e0ac69",
        en: "skin",
        zh: "肤色",
        aliases: &["肉色", "皮肤色", "中性肤色"],
    },
    NamedColor {
        hex: "#c68642",
        en: "skin tan",
        zh: "小麦肤",
        aliases: &["深肤", "晒黑肤色"],
    },
    NamedColor {
        hex: "#8d5524",
        en: "skin deep",
        zh: "深肤",
        aliases: &["深肤色"],
    },
    NamedColor {
        hex: "#f2d7d5",
        en: "blush",
        zh: "腮红",
        aliases: &["淡粉白"],
    },
    NamedColor {
        hex: "#d98880",
        en: "rosewood",
        zh: "玫木",
        aliases: &["暗粉棕"],
    },
    NamedColor {
        hex: "#cd6155",
        en: "cheek",
        zh: "颊彩",
        aliases: &["红棕", "赭红"],
    },
    NamedColor {
        hex: "#a93226",
        en: "blood red",
        zh: "血红",
        aliases: &["血"],
    },
    NamedColor {
        hex: "#641e16",
        en: "dark blood",
        zh: "暗血红",
        aliases: &["深血红"],
    },
    NamedColor {
        hex: "#fdfefe",
        en: "snow",
        zh: "雪白",
        aliases: &["冷白"],
    },
    NamedColor {
        hex: "#aab7b8",
        en: "stone",
        zh: "石灰",
        aliases: &["石色", "岩灰"],
    },
    NamedColor {
        hex: "#707b7c",
        en: "slate",
        zh: "石板灰",
        aliases: &["青灰"],
    },
    NamedColor {
        hex: "#424949",
        en: "graphite",
        zh: "石墨",
        aliases: &["铅灰"],
    },
    NamedColor {
        hex: "#b2babb",
        en: "fog",
        zh: "雾灰",
        aliases: &["烟灰"],
    },
    NamedColor {
        hex: "#d5dbdb",
        en: "pearl",
        zh: "珍珠灰",
        aliases: &["月白"],
    },
    NamedColor {
        hex: "#f9e79f",
        en: "pale yellow",
        zh: "鹅黄",
        aliases: &["极淡黄"],
    },
    NamedColor {
        hex: "#f5b041",
        en: "amber",
        zh: "琥珀",
        aliases: &["琥珀色", "蜜色"],
    },
    NamedColor {
        hex: "#9c640c",
        en: "gold brown",
        zh: "金棕",
        aliases: &["深金"],
    },
    NamedColor {
        hex: "#f1c40f",
        en: "gold",
        zh: "金",
        aliases: &["金色", "黄金", "太阳金"],
    },
    NamedColor {
        hex: "#bf9000",
        en: "dark gold",
        zh: "暗金",
        aliases: &["旧金"],
    },
    NamedColor {
        hex: "#d5d8dc",
        en: "silver pale",
        zh: "白银",
        aliases: &["亮银"],
    },
    NamedColor {
        hex: "#909497",
        en: "iron",
        zh: "铁灰",
        aliases: &["金属灰", "锡色"],
    },
    NamedColor {
        hex: "#5d6d7e",
        en: "steel",
        zh: "钢色",
        aliases: &["钢灰"],
    },
    NamedColor {
        hex: "#2c3e50",
        en: "dark steel",
        zh: "深钢蓝",
        aliases: &["钢蓝黑"],
    },
    NamedColor {
        hex: "#34495e",
        en: "gunmetal",
        zh: "炮铜",
        aliases: &["枪铁灰"],
    },
    NamedColor {
        hex: "#99a3a4",
        en: "pewter",
        zh: "白镴",
        aliases: &["锡灰"],
    },
    NamedColor {
        hex: "#58d68d",
        en: "spring green",
        zh: "春绿",
        aliases: &["嫩绿", "新绿"],
    },
    NamedColor {
        hex: "#28b463",
        en: "emerald",
        zh: "翠绿",
        aliases: &["祖母绿"],
    },
    NamedColor {
        hex: "#239b56",
        en: "jade",
        zh: "玉色",
        aliases: &["翡翠"],
    },
    NamedColor {
        hex: "#196f3d",
        en: "pine",
        zh: "松绿",
        aliases: &["深翠绿"],
    },
    NamedColor {
        hex: "#5499c7",
        en: "denim",
        zh: "牛仔蓝",
        aliases: &["丹宁蓝"],
    },
    NamedColor {
        hex: "#21618c",
        en: "cobalt",
        zh: "钴蓝",
        aliases: &["深宝蓝"],
    },
    NamedColor {
        hex: "#154360",
        en: "midnight",
        zh: "午夜蓝",
        aliases: &["深夜蓝"],
    },
    NamedColor {
        hex: "#a9cce3",
        en: "ice blue",
        zh: "冰蓝",
        aliases: &["极浅蓝"],
    },
    NamedColor {
        hex: "#76448a",
        en: "plum",
        zh: "梅紫",
        aliases: &["李紫"],
    },
    NamedColor {
        hex: "#5b2c6f",
        en: "grape",
        zh: "葡紫",
        aliases: &["深紫罗兰"],
    },
    NamedColor {
        hex: "#bb8fce",
        en: "lavender",
        zh: "薰衣草",
        aliases: &["淡紫"],
    },
    NamedColor {
        hex: "#e6b0aa",
        en: "dusty rose",
        zh: "灰玫",
        aliases: &["暗粉"],
    },
    NamedColor {
        hex: "#ec7063",
        en: "brick light",
        zh: "浅砖红",
        aliases: &["亮砖红"],
    },
    NamedColor {
        hex: "#e59866",
        en: "terracotta light",
        zh: "浅陶土",
        aliases: &["浅赤陶"],
    },
    NamedColor {
        hex: "#abebc6",
        en: "seafoam",
        zh: "海沫绿",
        aliases: &["浅湖绿"],
    },
    NamedColor {
        hex: "#76d7c4",
        en: "aqua",
        zh: "水绿",
        aliases: &["水色", "湖绿"],
    },
    NamedColor {
        hex: "#45b39d",
        en: "pine teal",
        zh: "松绿青",
        aliases: &["深湖绿"],
    },
    NamedColor {
        hex: "#17a589",
        en: "cyan",
        zh: "青",
        aliases: &["蓝绿", "青色"],
    },
    NamedColor {
        hex: "#f7f9f9",
        en: "paper",
        zh: "纸白",
        aliases: &["绢白"],
    },
];

/// redmean 距离：人眼对绿色差最敏感，对暗部的红最迟钝，
/// 与 pixel-core 的 color_distance 同口径，两处算出来的「最近色」才对得上。
fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let (ar, ag, ab) = (a.0 as f64, a.1 as f64, a.2 as f64);
    let (br, bg, bb) = (b.0 as f64, b.1 as f64, b.2 as f64);
    let r_mean = (ar + br) / 2.0;
    let dr = ar - br;
    let dg = ag - bg;
    let db = ab - bb;
    let weight_r = 2.0 + r_mean / 256.0;
    let weight_g = 4.0;
    let weight_b = 2.0 + (255.0 - r_mean) / 256.0;
    dr * dr * weight_r + dg * dg * weight_g + db * db * weight_b
}

fn parse_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let text = hex.trim().trim_start_matches('#');
    if text.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(text, 16).ok()?;
    Some((
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    ))
}

/// 找最近的有名颜色。同距时留在前面的那一项，结果稳定可复现。
pub fn nearest_named(hex: &str) -> Option<&'static NamedColor> {
    let target = parse_rgb(hex)?;
    let mut best: Option<&'static NamedColor> = None;
    let mut best_distance = f64::INFINITY;
    for entry in NAMED_COLORS {
        let Some(rgb) = parse_rgb(entry.hex) else {
            continue;
        };
        let d = distance(target, rgb);
        if d < best_distance {
            best_distance = d;
            best = Some(entry);
        }
    }
    best
}

/// 色名注脚：`红 / red`，认不出就给 hex 本身。
pub fn describe(hex: &str) -> String {
    match nearest_named(hex) {
        Some(entry) => format!("{} / {}", entry.zh, entry.en),
        None => hex.to_string(),
    }
}

/// 给提示词的对照表：`中 / EN #hex`。一段一名，模型按名字扫过去就能落 hex。
pub fn prompt_table() -> String {
    let mut out = String::from(
        "COLOR NAMES - map user color words to hex; pick the closest name, never a near-miss:\n",
    );
    for entry in NAMED_COLORS {
        out.push_str(&format!(
            "{} / {} {}{}\n",
            entry.zh,
            entry.en,
            entry.hex,
            if entry.aliases.is_empty() {
                String::new()
            } else {
                format!(" (aka {})", entry.aliases.join(", "))
            }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hexes_are_unique() {
        let mut seen: Vec<&str> = NAMED_COLORS.iter().map(|c| c.hex).collect();
        let before = seen.len();
        seen.sort();
        seen.dedup();
        // 重复色名会让模型在同一组 hex 之间犹豫，属于表坏了。
        assert_eq!(before, seen.len(), "duplicate hex in NAMED_COLORS");
    }

    #[test]
    fn names_have_both_languages() {
        for entry in NAMED_COLORS {
            assert!(!entry.en.is_empty(), "{} has no English name", entry.hex);
            assert!(!entry.zh.is_empty(), "{} has no Chinese name", entry.hex);
        }
    }

    #[test]
    fn nearest_named_snaps_to_the_expected_family() {
        let orange = nearest_named("#e67e22").expect("orange");
        assert_eq!(orange.hex, "#e67e22");
        // 三位 hex 不在表里，但不该崩。
        assert!(nearest_named("#abc").is_none());
        assert!(parse_rgb("#abc").is_none());
        // 差半个色阶也该归到同一族。
        let near = nearest_named("#e5711c").expect("near orange");
        assert_eq!(near.zh, "橙");
    }

    #[test]
    fn prompt_table_mentions_hex_and_both_names() {
        let table = prompt_table();
        assert!(table.contains("#e67e22"));
        assert!(table.contains("orange"));
        assert!(table.contains("橙"));
    }
}
