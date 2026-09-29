//! 参照模式：用户贴进来的参考图，是「只借画风」还是「照着实临摹」。
//!
//! 这两件事对提示词的约束完全相反：风格参照下，主体必须听用户的话，
//! 图里那只猫只是个用色样板；完全参照下，主体、构图、比例都得跟图走。
//! 以前只有一种说法（参考图就是视觉真值），于是「照这个画风画一只猫」
//! 也会被做成「把图里那只猫再画一遍」——用户要的是配色，得到的却是复刻。
//!
//! 合成轮次分流的四件事之一（意图 / 风格 / 知识库 / 参照定性）住在 `plan` 里，
//! 那个节点和工具也归它；本模块只管定性本身：怎么读、每种模式卡什么约束、
//! 模型送来的纠正怎么解。定性同样由两段接力完成——Rust 先按关键词定默认值，
//! 模型觉得判错了再调 `pixel_plan` 纠正。缺了第一段，模型把预算全花在思考上、
//! 一个工具都不调的时候（这种模型真存在），约束就根本不在路上。

use super::models::ReferenceMode;
use serde_json::Value;

/// 「照着实临摹」的说法。优先级高于风格：用户把「完全」两个字说出口时，
/// 他脑子里那张图就是要落到画布上的那一张，这时候讲画风就是理解错了。
const FULL_SIGNALS: &[&str] = &[
    // 中文：把「照原样」说出来的各种讲法。
    "完全参照",
    "完全按照",
    "完全照",
    "完全一样",
    "完全復刻",
    "完全复刻",
    "完全复现",
    "完全参考这",
    "完整参照",
    "完整复制",
    "一模一样",
    "一模一樣",
    "照着画",
    "照着这张",
    "照着这个",
    "照这张画",
    "照这个画",
    "照抄",
    "照搬",
    "复刻",
    "复现",
    "还原这张",
    "画成这个",
    "画成这样",
    "按这张画",
    "按这个画",
    "原图",
    "原样",
    "一比一",
    "1:1",
    // 英文：同一批意思的直译与惯用语。
    "exact copy",
    "copy this",
    "copy of this",
    "reproduce",
    "replicate",
    "identical",
    "same image",
    "same picture",
    "pixel-perfect",
    "trace this",
    "as-is",
    "as is",
];

/// 「只借画风」的说法。都是用户在说「照着它的样子上色 / 出这个味道」。
const STYLE_SIGNALS: &[&str] = &[
    "画风",
    "畫風",
    "风格",
    "風格",
    "风格化",
    "风格参照",
    "配色",
    "色彩",
    "色调",
    "色感",
    "用色",
    "上色",
    "着色",
    "笔触",
    "技法",
    "手法",
    "渲染方式",
    "光效",
    "光影",
    "氛围",
    "质感",
    "调性",
    "style",
    "palette",
    "coloring",
    "colouring",
    "color scheme",
    "colour scheme",
    "shading",
    "look and feel",
    "aesthetic",
    "mood",
    "rendering style",
    "same vibe",
];

/// 紧贴在说法前面的这几个字，把那句话变成否定：「不要照抄」不是完全参照。
/// 反向用也一样：用户说「不要抄它的构图，只要画风」，复刻那段就被否掉了。
const NEGATORS: &[&str] = &[
    "不要", "别", "不用", "不是", "并非", "无需", "不许", "no ", "not ", "don't", "avoid",
];

/// 往前看几个字找否定词。六字窗是中英否定词的公共长度上限：
/// 再往前的「不要」已经在说另一件事了，跟本句无关。
const NEGATION_WINDOW: usize = 6;

/// 模型通过工具给出的纠正。`index` 是附件序号（1 起算，与清单里显示的一致，
/// 用户和模型看到的是同一个号）。
#[derive(Debug, Clone)]
pub struct ReferenceUpdate {
    pub index: usize,
    pub mode: ReferenceMode,
    pub because: Option<String>,
}

/// 从一句话里读参照方式，顺带把命中的说法还回来（节点上要显示）。
pub fn classify_text(text: &str) -> (ReferenceMode, Option<String>) {
    // 英文信号一律小写比对；中文不受影响。
    let needle = text.to_lowercase();
    let fulls = hits(&needle, FULL_SIGNALS);
    let styles = hits(&needle, STYLE_SIGNALS);
    for (at, signal) in &fulls {
        if negated(&needle, *at) || swallowed(&needle, *at, signal, &styles) {
            continue;
        }
        return (ReferenceMode::Full, Some(signal.to_string()));
    }
    match styles.first() {
        Some((_, signal)) => (ReferenceMode::Style, Some(signal.to_string())),
        None => (ReferenceMode::Full, None),
    }
}

/// 一张表里的全部命中，按出现先后排。取全部而不只取最早：要判断某个「完全参照」
/// 是不是被画风词吞掉的残句，只看最早那一个会漏掉后面的说法。
fn hits<'a>(haystack: &str, signals: &[&'a str]) -> Vec<(usize, &'a str)> {
    let mut out: Vec<(usize, &'a str)> = Vec::new();
    for signal in signals {
        for (at, _) in haystack.match_indices(signal) {
            out.push((at, signal));
        }
    }
    out.sort_by_key(|(at, _)| *at);
    out
}

/// 这个「完全参照」的说法是不是一句画风话的残句。「照这个画风」的前四个字恰好
/// 也是「照这个画」——不拦这一下，用户要的是配色，端上来的却是复刻。
/// 判法：画风词骑在它身上、或紧贴它尾上，这句「完全参照」就不算数。
fn swallowed(haystack: &str, at: usize, signal: &str, styles: &[(usize, &str)]) -> bool {
    let end = at + signal.len();
    styles
        .iter()
        .any(|(style_at, _)| *style_at >= at && *style_at <= end && !negated(haystack, *style_at))
}

/// 这个位置的说法前面有没有否定词。
fn negated(haystack: &str, at: usize) -> bool {
    let head: String = haystack[..at]
        .chars()
        .rev()
        .take(NEGATION_WINDOW)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    NEGATORS.iter().any(|word| head.contains(word))
}

/// 每种模式对模型的约束原文。写进出图清单，也写进工具结果：
/// 两处措辞必须一致，不然模型看到的和回灌给它的说法会漂移。
pub fn rules(mode: ReferenceMode) -> &'static str {
    match mode {
        ReferenceMode::Style => "take ONLY its palette, ramps, light direction, outline and dithering technique; the subject, composition, pose and proportions must come from the user's words, never from this image",
        ReferenceMode::Full => "reproduce its subject, composition, proportions and palette as closely as the canvas resolution allows, simplified into clean pixel art, and invent nothing that is not in the image",
    }
}

/// 解析模型的纠正入参。下标按出图清单里的 1-based 号（用户在清单里看到的就是它），
/// 原样带着——调用方按 1-based 用，别在这儿偷偷减一，减了反而要和清单对不上。
///
/// 入参写成什么样全看模型心情，所以容器层什么都接：数组、按号索引的对象、
/// 光秃秃一个 mode 都算。真正不能容的只有两样——号重复、号不存在，
/// 因为那会让模式表和附件对不上，比报错难查得多。
pub fn parse_updates(input: &Value) -> Result<Vec<ReferenceUpdate>, String> {
    let mut out: Vec<ReferenceUpdate> = Vec::new();
    let container = input.get("references").unwrap_or(input);
    let mut push = |value: &Value, fallback: usize| -> Result<(), String> {
        let (index, mode, because) = match value {
            Value::String(s) => (
                fallback,
                ReferenceMode::parse(s).ok_or_else(|| bad_mode(s))?,
                None,
            ),
            Value::Object(map) => {
                let index = map
                    .get("index")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize)
                    .unwrap_or(fallback);
                if index == 0 {
                    return Err(
                        "pixel_plan: 'index' is 1-based; 0 is not an image number".to_string()
                    );
                }
                let raw = map.get("mode").and_then(Value::as_str).ok_or_else(|| {
                    "pixel_plan: every entry needs a 'mode' of style or full".to_string()
                })?;
                let because = map
                    .get("because")
                    .and_then(Value::as_str)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                (
                    index,
                    ReferenceMode::parse(raw).ok_or_else(|| bad_mode(raw))?,
                    because,
                )
            }
            other => {
                return Err(format!(
                    "pixel_plan: cannot read a reference entry from {other}"
                ))
            }
        };
        if out.iter().any(|u| u.index == index) {
            return Err(format!(
                "pixel_plan: image {index} is listed twice; one mode per image"
            ));
        }
        out.push(ReferenceUpdate {
            index,
            mode,
            because,
        });
        Ok(())
    };

    match container {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                push(item, i + 1)?;
            }
        }
        Value::Object(map) => {
            let entries: Vec<(usize, &Value)> = map
                .iter()
                .filter_map(|(key, value)| key.parse::<usize>().ok().map(|i| (i, value)))
                .collect();
            if entries.is_empty() {
                // 没有数字键，就按「一条光秃秃的纠正」读：{"mode":"style"} 和
                // {"references":{"mode":"style"}} 都是这个意思，下标按位置推成 1。
                // 真的不像条目的，交给 push 报「缺 mode」——那比含糊说缺清单有用。
                push(container, 1)?;
                return Ok(out);
            }
            for (index, value) in entries {
                push(value, index)?;
            }
        }
        other => {
            return Err(format!(
                "pixel_plan: needs a 'references' list, or one entry per image number; got {other}"
            ))
        }
    }
    Ok(out)
}

fn bad_mode(raw: &str) -> String {
    format!("pixel_plan: '{raw}' is not a reference mode; use style or full")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_users_own_words_pick_the_mode() {
        // 「照这个画风」要的是用色，不是图里那只猫。
        assert_eq!(
            classify_text("照这个画风画一只柴犬").0,
            ReferenceMode::Style
        );
        assert_eq!(
            classify_text("沿用这张图的配色，主体我来定").0,
            ReferenceMode::Style
        );
        assert_eq!(
            classify_text("use only the palette of this image").0,
            ReferenceMode::Style
        );
        // 「完全照着」要的就是那张图本身。
        assert_eq!(classify_text("完全照着这张图画").0, ReferenceMode::Full);
        assert_eq!(
            classify_text("reproduce this image exactly").0,
            ReferenceMode::Full
        );
        // 一句没提：参考图仍是视觉真值，跟加模式之前的行为一致。
        assert_eq!(classify_text("画一只猫").0, ReferenceMode::Full);
    }

    #[test]
    fn complete_wins_over_style_when_both_are_said() {
        // 用户把「完全」说出口时，画风只是顺带提的。
        assert_eq!(
            classify_text("完全参照这张图，包括它的画风").0,
            ReferenceMode::Full
        );
    }

    #[test]
    fn a_negated_signal_does_not_count() {
        // 「不要照抄」是在拒绝复刻，剩下那句画风才是主导。
        let (mode, hit) = classify_text("不要照抄它的构图，只要这个画风");
        assert_eq!(mode, ReferenceMode::Style);
        assert_eq!(hit.as_deref(), Some("画风"));
        assert_eq!(
            classify_text("别完全复刻，配色参考一下就行").0,
            ReferenceMode::Style
        );
    }

    #[test]
    fn the_hit_word_is_reported_so_the_node_can_show_it() {
        let (_, hit) = classify_text("按这个画风来");
        assert_eq!(hit.as_deref(), Some("画风"));
    }

    #[test]
    fn every_container_shape_the_model_might_send_is_read() {
        // 数组带对象。
        let updates =
            parse_updates(&json!({"references": [{"index": 2, "mode": "style"}]})).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].index, 2);
        assert_eq!(updates[0].mode, ReferenceMode::Style);
        // 按下标索引的对象。
        let updates = parse_updates(&json!({"references": {"1": "full"}})).unwrap();
        assert_eq!(updates[0].index, 1);
        assert_eq!(updates[0].mode, ReferenceMode::Full);
        // 光秃秃一个 mode，下标按位置推。
        let updates = parse_updates(&json!({"references": [{"mode": "style"}]})).unwrap();
        assert_eq!(updates[0].index, 1);
        // 裸 mode 字符串。
        let updates = parse_updates(&json!({"references": ["style"]})).unwrap();
        assert_eq!(updates[0].index, 1);
        let updates = parse_updates(&json!({"mode": "style"})).unwrap();
        assert_eq!(updates[0].index, 1);
    }

    #[test]
    fn nonsense_modes_and_bad_numbers_are_refused_by_name() {
        let err =
            parse_updates(&json!({"references": [{"index": 1, "mode": "vibe"}]})).unwrap_err();
        assert!(err.contains("vibe"), "{err}");
        let err =
            parse_updates(&json!({"references": [{"index": 0, "mode": "style"}]})).unwrap_err();
        assert!(err.contains("1-based"), "{err}");
        let err = parse_updates(
            &json!({"references": [{"index": 1, "mode": "style"}, {"index": 1, "mode": "full"}]}),
        )
        .unwrap_err();
        assert!(err.contains("twice"), "{err}");
        let err = parse_updates(&json!({"references": 3})).unwrap_err();
        assert!(err.contains("references"), "{err}");
    }
}
