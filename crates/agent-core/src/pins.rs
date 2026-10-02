// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 界面钉上来的画风与收尾预设：把一串字符串解析成提示词要用的那一份。
//!
//! 「这一句照什么规矩画」有三个入口：主循环（`agent_send_message`）、提示词微调
//! （`prompt_refine`）、工作流坞手动生图（`workflow_image_gen`）。三条问的是同一个
//! 问题，答案曾经分散在三处各自实现：认不出的 id 一处报错一处静默忽略，去重、上限、
//! 「不限」的表示法也各写一遍。用户在输入区选了「写实渲染」、微调出来的成品却不带这条
//! 规矩，查的就是这种地方——补了一处，另外两处还在原地。
//!
//! 所以解析只留这一份。语义刻意严：认不出的 id 一律报错，绝不静默退回「不限」——
//! 那会让用户以为规矩上了路，其实这一轮什么都没多带，比报错难查得多。

use super::artstyle::ArtStyle;
use super::presets::{self, Preset};

/// 下拉里「不限」的那一项。前端点它会送 null，但别处（mock、别的壳、以后加的面板）
/// 可能照字面送过来：那是同一个意思，不是错误。
const RELEASE_WORDS: [&str; 4] = ["auto", "none", "auto style", "不限"];

/// 解析一句「这一句锁什么画风」。
///
/// 返回 `None` 表示没锁：这一句该按自己的话走分类，拿不到风格时由调用方决定
/// 兜底（生图那条路挂默认质量档，主循环那条让 `plan` 自己判）。
pub fn pinned_style(raw: Option<&str>) -> Result<Option<ArtStyle>, String> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty());
    let Some(raw) = raw else { return Ok(None) };
    if RELEASE_WORDS.contains(&raw.to_ascii_lowercase().as_str()) {
        return Ok(None);
    }
    ArtStyle::parse(raw)
        .map(Some)
        .ok_or_else(|| format!("unknown art style: {raw}"))
}

/// 解析同时生效的收尾预设。可以叠几条（上限 `presets::MAX_STACKED`）：细节这件事是
/// 乘法，「写实渲染」讲整张图怎么收尾，「微细结构」讲最后一两个像素点在哪里，
/// 两条一起才是一张写实的图。
///
/// 超出上限当场报错而不是悄悄丢掉：用户点了第四条又没反应，他会以为这条也上了路，
/// 然后盯着没变化的图猜原因。重复的 id 直接吃掉——同一个下拉里选两遍同一条，
/// 没有任何新信息，报错只是在罚他的手滑。
pub fn pinned_presets<I, S>(raw: I) -> Result<Vec<&'static Preset>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out: Vec<&'static Preset> = Vec::new();
    for raw in raw {
        let raw = raw.as_ref().trim();
        if raw.is_empty() || RELEASE_WORDS.contains(&raw.to_ascii_lowercase().as_str()) {
            continue;
        }
        let preset = presets::parse(raw).ok_or_else(|| format!("unknown prompt preset: {raw}"))?;
        if out.iter().any(|p| p.id == preset.id) {
            continue;
        }
        if out.len() >= presets::MAX_STACKED {
            return Err(format!(
                "too many prompt presets: at most {} at a time",
                presets::MAX_STACKED
            ));
        }
        out.push(preset);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 没锁的时候是 None，不是「默认风格」：锁没锁是两回事，混成一个就再也分不出
    /// 「用户说随便」和「用户说不限」。
    #[test]
    fn nothing_pinned_reads_as_none() {
        assert_eq!(pinned_style(None), Ok(None));
        assert_eq!(pinned_style(Some("")), Ok(None));
        assert_eq!(pinned_style(Some("   ")), Ok(None));
    }

    /// 「不限」那几个字是同一个意思的不同写法，一律放行：为下拉里的一个「不限」
    /// 把整句话打回来，用户收到的是「发送失败」四个字，无从查起。
    #[test]
    fn the_release_words_mean_unpinned_in_any_casing() {
        for word in ["auto", "AUTO", "none", "None"] {
            assert_eq!(pinned_style(Some(word)), Ok(None), "{word} 该放行");
        }
    }

    #[test]
    fn a_known_style_parses_and_an_unknown_one_is_refused() {
        assert_eq!(pinned_style(Some(" gameboy ")), Ok(Some(ArtStyle::GameBoy)));
        let err = pinned_style(Some("photoshop")).unwrap_err();
        assert!(
            err.contains("unknown art style"),
            "报错要说清是哪个 id 认不出：{err}"
        );
    }

    #[test]
    fn presets_stack_in_order_and_drop_the_release_words() {
        let got = pinned_presets(vec![
            " realistic ".to_string(),
            "auto".to_string(),
            "microdetail".to_string(),
        ])
        .expect("三个以内不该报错");
        assert_eq!(
            got.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec!["realistic", "microdetail"],
            "认不出能不能点的词要吃掉，不能把整句话打回来"
        );
    }

    /// 同一条选两遍是手滑，报错只会罚他；吃掉它就是正确答案。
    #[test]
    fn picking_the_same_preset_twice_is_forgiven() {
        let got =
            pinned_presets(vec!["polish".to_string(), "polish".to_string()]).expect("重复 id");
        assert_eq!(got.len(), 1);
    }

    /// 上限就是上限：第四条必须当场说清，而不是默默丢掉一条用户点过的规矩。
    #[test]
    fn the_fourth_preset_is_refused_outright() {
        let err = pinned_presets(vec![
            "realistic".to_string(),
            "microdetail".to_string(),
            "occlusion".to_string(),
            "polish".to_string(),
        ])
        .unwrap_err();
        assert!(
            err.contains("too many prompt presets"),
            "超额要报出来：{err}"
        );
        assert_eq!(
            pinned_presets(vec![
                "realistic".to_string(),
                "microdetail".to_string(),
                "occlusion".to_string(),
            ])
            .expect("三条正好是上限")
            .len(),
            3
        );
    }

    #[test]
    fn an_unknown_preset_id_never_falls_back_to_unpinned() {
        let err = pinned_presets(vec!["photoshop".to_string()]).unwrap_err();
        assert!(
            err.contains("unknown prompt preset"),
            "认不出要报错，静默忽略会让用户以为规矩上了路：{err}"
        );
    }
}
