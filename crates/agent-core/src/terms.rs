// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 触发词命中判据：拉丁词按整词算，中文按子串算。
//!
//! 意图、风格、参照、知识库四张表过去各写各的 `contains`，也各写各的一道
//! 整词闸门，闸门门槛还都是「长度不超 4」——于是 `canvas is` 里的 `as is`、
//! `lifestyle` 里的 `style`、`elephant` 里的 `ant`、`chair` 里的 `hair`
//! 全都畅通无阻。子串命中一条错的，代价不是少带一条笔记，是往系统提示词里
//! 塞一段斩钉截铁的错规矩：模型照着 "an insect is three clear masses"
//! 去画大象，而用户只觉得 AI 莫名聊到昆虫。
//!
//! 中文没有空格，子串是唯一可用的判据（「行走」就该在「行走图」里命中），
//! 所以边界只对拉丁词设。宽容只给复数：`tiles` 命中 `tile`，但 `tilemaps`
//! 不命中 `tile`——宁可少带一条笔记，也不要塞错一条。

/// 小写后的文本里，`at` 这一下 `term` 算不算命中。
///
/// `haystack` 与 `term` 都必须已经小写过；`haystack[at..]` 必须以 `term`
/// 开头（调用方用 `match_indices` 找到的偏移）。中文触发词直接算命中。
pub fn matches_at(haystack: &str, at: usize, term: &str) -> bool {
    if !term.is_ascii() {
        return true;
    }
    // 前面贴着字母数字，说明这是另一个词的尾巴：`lifestyle` 里的 `style`。
    if haystack[..at]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return false;
    }
    let words: Vec<&str> = term
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect();
    let (Some(first), Some(last)) = (words.first(), words.last()) else {
        return false;
    };
    // 多词触发词之间的连接符是什么都行（空格、连字符），中间那个词信任首尾：
    // 表里都是「as is」「walk cycle」这种，为它在判据里再写一个小解析器不划算。
    let head = run_at(haystack, at);
    if !run_matches(&head, first) {
        return false;
    }
    if words.len() == 1 {
        return true;
    }
    // 多词触发词只看尾巴：后面什么都没有，或者紧跟的不是字母数字，
    // 那就是天然的边界（`walk cycle` 结尾、`1-bit 小猫` 的连字符后面是中文）。
    let tail = run_at(haystack, at + term.len());
    if tail.is_empty() {
        return true;
    }
    run_matches(&tail, last)
}

/// `at` 处那一串字母数字（中文触发词用不到，拉丁侧专用）。
fn run_at(haystack: &str, at: usize) -> String {
    haystack[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// 一个字母数字串对上触发词里的一个词：相等，或只差一个复数后缀。
fn run_matches(run: &str, word: &str) -> bool {
    run == word || matches!(run.strip_prefix(word), Some("s") | Some("es") | Some("ies"))
}

/// 小写后的文本里找一个触发词，整词才算。要的还是最早那一下。
pub fn find_lower(haystack: &str, term_lower: &str) -> Option<usize> {
    haystack
        .match_indices(term_lower)
        .map(|(at, _)| at)
        .find(|at| matches_at(haystack, *at, term_lower))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_latin_term_riding_inside_a_bigger_word_does_not_count() {
        // 这几句过去全都命中过：ant 钻进 elephant，hair 钻进 chair，
        // style 钻进 lifestyle，tree 钻进 street，as is 钻进 canvas is。
        for (text, term) in [
            ("draw an elephant", "ant"),
            ("a wooden chair", "hair"),
            ("a lifestyle scene", "style"),
            ("a street scene", "tree"),
            ("the canvas is empty", "as is"),
            ("an atlas is open", "as is"),
            ("a spaceship", "ship"),
            ("a monkey", "key"),
            ("download it", "down"),
        ] {
            assert!(
                find_lower(text, term).is_none(),
                "'{term}' should not hit inside '{text}'"
            );
        }
    }

    #[test]
    fn a_latin_term_still_matches_a_whole_word_and_its_plural() {
        assert_eq!(find_lower("draw a walk cycle", "walk cycle"), Some(7));
        assert_eq!(find_lower("draw a walk-cycle", "walk"), Some(7));
        assert_eq!(find_lower("lots of tiles", "tile"), Some(8));
        assert_eq!(find_lower("three crates", "crate"), Some(6));
        assert_eq!(find_lower("two cats", "cat"), Some(4));
        assert_eq!(find_lower("style please", "style"), Some(0));
        // 三词触发词也吃得下，中间那个词信任首尾。
        assert_eq!(find_lower("use look and feel", "look and feel"), Some(4));
        assert_eq!(find_lower("refine it", "refine"), Some(0));
        assert_eq!(find_lower("as is", "as is"), Some(0));
    }

    #[test]
    fn chinese_terms_stay_substring_matches() {
        // 中文没有词边界，「行走」就该在「行走图」里命中。
        // 六个汉字打头，一个三字节：「行走」落在第 18 字节上。
        assert_eq!(find_lower("画一个八帧的行走图", "行走"), Some(18));
        assert_eq!(find_lower("画一个八帧的行走图", "step"), None);
    }
}
