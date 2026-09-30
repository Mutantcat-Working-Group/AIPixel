// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 常用模型的输出上限对照表。
//!
//! 「Max tokens」填得太小是像素画 agent 最隐蔽的坑：一段分镜 Lua 动辄五六百行，
//! 加上推理模型的思考过程，4096 个 token 连一半都装不下。用户不一定知道该填多少，
//! 所以这里按模型名给一个像样的默认值，命中不了就回落 `FALLBACK_MAX_TOKENS`。
//!
//! 匹配只看裸模型名最后一段：`openai/gpt-4o`、`accounts/fireworks/models/claude-sonnet-4-5`
//! 都是常见写法。分隔符必须是 `-` 或到头，不然 `gpt-4.1` 会被 `gpt-4` 抢走。
//!
//! 同一个模型在不同平台上的马甲也归这里管：先 `canonical_name` 抹平 `.` `_` `-`
//! 的手滑，再照 `ALIASES` 认那几个省掉版本号的简名。认不出的代价不只是上限填错——
//! `providers::echoes_reasoning` 会把它判成非推理模型，推理历史一丢，模型下一轮
//! 就把整个问题重新想一遍。那正是「每次续写都是重写」的病根。

/// 表里查不到这个模型时的默认输出上限。
///
/// 取 32768 而不是 8192：现在连小模型都普遍支持到 32k 以上，给少了用户只会
/// 看到半截代码。用户显式填过的值永远优先，这里只兜底。
pub const FALLBACK_MAX_TOKENS: u32 = 32768;

/// 同一个模型在不同平台上的简名。左边是别名叫，右边是表里的正式名。
///
/// 平台之间从来不商量命名：官方叫 `deepseek-v4.1-flash`，中转站可能写成
/// `deepseek-v4-1-flash`（分隔符手滑），也可能干脆只留 `deepseek-flash`
/// （版本整段省掉）。前一种 `canonical_name` 就能对上，后一种只能照表认亲。
const ALIASES: &[(&str, &str)] = &[("deepseek-flash", "deepseek-v4.1-flash")];

/// 一条对照记录：模型名前缀 + 常用输出上限。
struct Limit {
    pattern: &'static str,
    max_tokens: u32,
}

/// 只收「日常真会用到的」。冷门老模型漏了不要紧，回落值够用。
const LIMITS: &[Limit] = &[
    // ---------- Anthropic ----------
    Limit {
        pattern: "claude-opus-4-5",
        max_tokens: 64000,
    },
    Limit {
        pattern: "claude-sonnet-4-5",
        max_tokens: 64000,
    },
    Limit {
        pattern: "claude-haiku-4-5",
        max_tokens: 64000,
    },
    Limit {
        pattern: "claude-opus-4-1",
        max_tokens: 32000,
    },
    Limit {
        pattern: "claude-opus-4",
        max_tokens: 32000,
    },
    Limit {
        pattern: "claude-sonnet-4",
        max_tokens: 64000,
    },
    Limit {
        pattern: "claude-haiku-4",
        max_tokens: 32000,
    },
    Limit {
        pattern: "claude-3-7-sonnet",
        max_tokens: 64000,
    },
    Limit {
        pattern: "claude-3-5-sonnet",
        max_tokens: 8192,
    },
    Limit {
        pattern: "claude-3-5-haiku",
        max_tokens: 8192,
    },
    Limit {
        pattern: "claude-3-opus",
        max_tokens: 4096,
    },
    Limit {
        pattern: "claude-3-sonnet",
        max_tokens: 4096,
    },
    Limit {
        pattern: "claude-3-haiku",
        max_tokens: 4096,
    },
    Limit {
        pattern: "claude-2.1",
        max_tokens: 4096,
    },
    // ---------- OpenAI ----------
    Limit {
        pattern: "gpt-5",
        max_tokens: 128000,
    },
    Limit {
        pattern: "gpt-5-mini",
        max_tokens: 128000,
    },
    Limit {
        pattern: "gpt-5-nano",
        max_tokens: 128000,
    },
    Limit {
        pattern: "gpt-4.1",
        max_tokens: 32768,
    },
    Limit {
        pattern: "gpt-4.1-mini",
        max_tokens: 32768,
    },
    Limit {
        pattern: "gpt-4.1-nano",
        max_tokens: 32768,
    },
    Limit {
        pattern: "gpt-4o",
        max_tokens: 16384,
    },
    Limit {
        pattern: "gpt-4o-mini",
        max_tokens: 16384,
    },
    Limit {
        pattern: "chatgpt-4o-latest",
        max_tokens: 16384,
    },
    Limit {
        pattern: "gpt-4-turbo",
        max_tokens: 4096,
    },
    Limit {
        pattern: "gpt-4-32k",
        max_tokens: 4096,
    },
    Limit {
        pattern: "gpt-4",
        max_tokens: 8192,
    },
    Limit {
        pattern: "o1",
        max_tokens: 100000,
    },
    Limit {
        pattern: "o1-mini",
        max_tokens: 100000,
    },
    Limit {
        pattern: "o1-pro",
        max_tokens: 100000,
    },
    Limit {
        pattern: "o3",
        max_tokens: 100000,
    },
    Limit {
        pattern: "o3-mini",
        max_tokens: 100000,
    },
    Limit {
        pattern: "o4-mini",
        max_tokens: 100000,
    },
    // ---------- Gemini ----------
    Limit {
        pattern: "gemini-2.5-pro",
        max_tokens: 65536,
    },
    Limit {
        pattern: "gemini-2.5-flash",
        max_tokens: 65536,
    },
    Limit {
        pattern: "gemini-2.0-flash",
        max_tokens: 8192,
    },
    Limit {
        pattern: "gemini-1.5-pro",
        max_tokens: 8192,
    },
    Limit {
        pattern: "gemini-1.5-flash",
        max_tokens: 8192,
    },
    // ---------- DeepSeek ----------
    Limit {
        pattern: "deepseek-v4.1-flash",
        max_tokens: 65536,
    },
    Limit {
        pattern: "deepseek-v4.1-pro",
        max_tokens: 65536,
    },
    Limit {
        pattern: "deepseek-v4-flash",
        max_tokens: 65536,
    },
    Limit {
        pattern: "deepseek-v4-pro",
        max_tokens: 65536,
    },
    Limit {
        pattern: "deepseek-v4",
        max_tokens: 65536,
    },
    Limit {
        pattern: "deepseek-reasoner",
        max_tokens: 65536,
    },
    Limit {
        pattern: "deepseek-chat",
        max_tokens: 32768,
    },
    Limit {
        pattern: "deepseek-v3",
        max_tokens: 65536,
    },
    Limit {
        pattern: "deepseek-r1",
        max_tokens: 65536,
    },
    // ---------- Qwen ----------
    Limit {
        pattern: "qwen3",
        max_tokens: 32768,
    },
    Limit {
        pattern: "qwq",
        max_tokens: 32768,
    },
    Limit {
        pattern: "qwen-max",
        max_tokens: 32768,
    },
    Limit {
        pattern: "qwen-plus",
        max_tokens: 32768,
    },
    Limit {
        pattern: "qwen-turbo",
        max_tokens: 8192,
    },
    Limit {
        pattern: "qwen-long",
        max_tokens: 8192,
    },
    Limit {
        pattern: "qwen-vl",
        max_tokens: 8192,
    },
    Limit {
        pattern: "qwen2.5",
        max_tokens: 8192,
    },
    // ---------- 智谱 GLM ----------
    Limit {
        pattern: "glm-4.6",
        max_tokens: 32768,
    },
    Limit {
        pattern: "glm-4.5",
        max_tokens: 32768,
    },
    Limit {
        pattern: "glm-4-plus",
        max_tokens: 8192,
    },
    Limit {
        pattern: "glm-4-air",
        max_tokens: 8192,
    },
    Limit {
        pattern: "glm-4v",
        max_tokens: 8192,
    },
    Limit {
        pattern: "glm-4",
        max_tokens: 8192,
    },
    // ---------- Kimi / Moonshot ----------
    Limit {
        pattern: "kimi-k2",
        max_tokens: 32768,
    },
    Limit {
        pattern: "kimi-latest",
        max_tokens: 32768,
    },
    Limit {
        pattern: "moonshot-v1",
        max_tokens: 8192,
    },
    // ---------- 豆包 ----------
    Limit {
        pattern: "doubao-seed",
        max_tokens: 32768,
    },
    Limit {
        pattern: "doubao-1.5-pro",
        max_tokens: 12288,
    },
    Limit {
        pattern: "doubao-pro",
        max_tokens: 4096,
    },
    // ---------- MiniMax ----------
    Limit {
        pattern: "minimax-m2",
        max_tokens: 32768,
    },
    Limit {
        pattern: "minimax-m1",
        max_tokens: 8192,
    },
    Limit {
        pattern: "minimax-text",
        max_tokens: 8192,
    },
    // ---------- Llama 系 ----------
    Limit {
        pattern: "llama-3.3",
        max_tokens: 32768,
    },
    Limit {
        pattern: "llama-3.1",
        max_tokens: 32768,
    },
];

/// 剥掉 `vendor/` 一类的路径前缀，只留模型本名。
fn bare_name(model: &str) -> &str {
    let name = model.trim();
    let tail = name.rsplit('/').next().unwrap_or(name);
    tail.trim()
}

/// 这个模型的常用输出上限。表里没有就回落 `FALLBACK_MAX_TOKENS`。
///
/// 长 pattern 优先匹配，避免 `gpt-4o-mini` 输给 `gpt-4o`。
pub fn ceiling_for(model: &str) -> u32 {
    let name = resolve_alias(model);
    if name.is_empty() {
        return FALLBACK_MAX_TOKENS;
    }
    // 逐条比，命中的最长者赢：这样表里的先后顺序就不影响结果。
    let mut best: Option<&Limit> = None;
    for row in LIMITS {
        if row.pattern.is_empty() {
            continue;
        }
        if name_matches(&name, row.pattern) {
            let longer = best
                .map(|b| row.pattern.len() > b.pattern.len())
                .unwrap_or(true);
            if longer {
                best = Some(row);
            }
        }
    }
    best.map(|b| b.max_tokens).unwrap_or(FALLBACK_MAX_TOKENS)
}

/// 归一到可以比较的形式：剥掉厂商路径前缀、小写，`.` 和 `_` 都换成 `-`。
///
/// `deepseek-v4.1-flash`、`deepseek-v4-1-flash`、`DeepSeek_V4.1_Flash` 是同一个
/// 东西，分开记三条就等着漏。归一之后分隔符只剩 `-`，前缀匹配也只需要考虑
/// 一种边界。
pub fn canonical_name(model: &str) -> String {
    let bare = bare_name(model);
    let mut out = String::with_capacity(bare.len());
    for ch in bare.chars() {
        match ch {
            '.' | '_' => out.push('-'),
            other => out.extend(other.to_lowercase()),
        }
    }
    out
}

/// 认过别名再交表：同一个模型换几个马甲也查到同一份上限。
pub fn resolve_alias(model: &str) -> String {
    let canonical = canonical_name(model);
    for (alias, target) in ALIASES {
        let alias = canonical_name(alias);
        let rest = if canonical == alias {
            Some("")
        } else {
            canonical
                .strip_prefix(alias.as_str())
                .and_then(|r| r.strip_prefix('-'))
        };
        if let Some(rest) = rest {
            let target = canonical_name(target);
            // 简名后面挂着日期之类的后缀时，把后缀一起搬过去：
            // `deepseek-flash-20260101` 该变成 `deepseek-v4-1-flash-20260101`。
            return if rest.is_empty() {
                target
            } else {
                format!("{target}-{rest}")
            };
        }
    }
    canonical
}

/// 模型名算不算这个 pattern 的自家人：名字相同，或者跟着分隔符往后接版本号。
///
/// 入参会先过 `canonical_name`：`-` 和 `.` 都当分隔符，所以 `deepseek-v4` 认得下
/// `deepseek-v4.1-flash`，也认得下手滑写成 `deepseek-v4-1-flash` 的那个。分隔符
/// 不能省——`gpt-4` 不能抢走 `gpt-4o`，`qwen` 也不能抢走 `qwen2.5`。
pub(crate) fn name_matches(name: &str, pattern: &str) -> bool {
    let name = canonical_name(name);
    let pattern = canonical_name(pattern);
    if pattern.is_empty() {
        return false;
    }
    if name == pattern {
        return true;
    }
    // 归一之后只剩 `-` 一种分隔符：跟着它往后接的才算自家人。
    name.strip_prefix(pattern.as_str())
        .is_some_and(|rest| rest.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_model_gets_its_own_ceiling() {
        assert_eq!(ceiling_for("claude-sonnet-4-5-20250929"), 64000);
        assert_eq!(ceiling_for("gpt-4o-mini-2024"), 16384);
        // 厂商前缀和名字后缀都不该碍事。
        assert_eq!(ceiling_for("openai/gpt-4o"), 16384);
        assert_eq!(ceiling_for("deepseek-v4.1-flash"), 65536);
    }

    #[test]
    fn a_longer_pattern_wins_over_its_prefix() {
        assert_eq!(
            ceiling_for("gpt-4o-mini"),
            16384,
            "gpt-4o 抢不走 gpt-4o-mini"
        );
        assert_eq!(ceiling_for("gpt-4o"), 16384);
        assert_eq!(
            ceiling_for("gpt-4.1-nano"),
            32768,
            "gpt-4.1 抢不走 gpt-4.1-nano"
        );
    }

    #[test]
    fn an_unknown_model_falls_back_to_a_generous_ceiling() {
        assert_eq!(
            ceiling_for("some-vendor/my-secret-model"),
            FALLBACK_MAX_TOKENS
        );
        assert_eq!(ceiling_for("   "), FALLBACK_MAX_TOKENS);
        assert_eq!(ceiling_for(""), FALLBACK_MAX_TOKENS);
    }

    /// 小数点开头的变体也是自家人：deepseek-v4 得认下 v4.1 的 flash，
    /// 不然用户手里最新那个型号只差一个小数点就查不到自己的上限。
    #[test]
    fn a_dotted_variant_still_counts_as_the_same_family() {
        assert!(name_matches("deepseek-v4.1-flash", "deepseek-v4"));
        assert!(name_matches("glm-4.5-air", "glm-4.5"));
        // 分隔符手滑也认：`.` 和 `-` 混着写不该换个上限。
        assert!(name_matches("deepseek-v4-1-flash", "deepseek-v4"));
        assert!(name_matches("deepseek_v4.1_flash", "deepseek-v4"));
        // 但分隔符不能省：gpt-4 不能顺着 qwen2.5 摸过去。
        assert!(!name_matches("gpt-4o", "gpt-4"));
        assert!(!name_matches("qwen2.5", "qwen"));
        assert!(!name_matches("gpt-4o", "gpt-4o-mini"), "短的认不下长的");
        assert!(!name_matches("", "gpt-4"));
        assert!(!name_matches("gpt-4", ""), "空 pattern 谁都不认");
    }

    /// 同一个 flash 的三种写法必须拿到同一份上限、同一个推理判定。
    /// 这是用户手里真实存在的情况：同一个模型，三个平台三个叫法。
    #[test]
    fn one_model_written_three_ways_resolves_alike() {
        for name in [
            "deepseek-v4.1-flash",
            "deepseek-v4-1-flash",
            "DeepSeek-V4-1-Flash",
            "vendor/models/deepseek_v4.1_flash",
            "deepseek-flash",
        ] {
            assert_eq!(ceiling_for(name), 65536, "{name} 该拿到 65536 的上限");
            assert_eq!(resolve_alias(name), resolve_alias("deepseek-v4.1-flash"));
        }
        // 简名认到别的型号头上不算本事，得认准自家那一支。
        assert_eq!(resolve_alias("deepseek-flash"), "deepseek-v4-1-flash");
    }

    #[test]
    fn an_unknown_short_name_is_left_alone() {
        // 没登记的简名不硬猜，原样交表，查不到就回落。
        assert_eq!(resolve_alias("deepseek-pro"), "deepseek-pro");
        assert_eq!(ceiling_for("deepseek-pro"), FALLBACK_MAX_TOKENS);
    }

    #[test]
    fn the_fallback_is_not_a_toy_number() {
        // 一段像样的分镜脚本：几百行 Lua 加思考过程，烧掉这么多 token 很平常。
        // 兜底要是连这都装不下，用户看到的就是半截代码，还会以为是模型笨。
        let script_tokens: u32 = "16000".parse().expect("16000 is a plain number");
        assert!(
            FALLBACK_MAX_TOKENS >= script_tokens,
            "兜底 {} 连一段分镜脚本都装不下",
            FALLBACK_MAX_TOKENS
        );
    }
}
