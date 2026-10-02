// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 文档增量：`DocumentUpdated` 事件只推元数据加变化过的 cel。
//!
//! 为什么不再整份推：256x256、8 层、30 帧的文档是 1560 万个格子，序列化成
//! JSON 约 47 MB。一轮 agent 有几十次工具调用，每次都推整份就是把上 GB 的
//! JSON 灌进 IPC，webview 直接卡死。而前端真正要的只有调色板、图层、帧这些
//! 元数据（洋葱皮另外单独取一个 cel，主画布走 PNG 预览），像素本身并不需要
//! 每次全量到位。
//!
//! 但前端仍须持有一份完整文档：撤销时它要把整份回传给后端 `sync_document`。
//! 所以增量的语义是「合并进已有文档」而不是「替换」——`cels` 里是全量替换的
//! 那几个 cel，`dropped` 是结构操作删掉的那几个，前端据此把本地文档补齐。

use std::collections::BTreeMap;

use super::document::{Document, Rgba};
use serde::{Deserialize, Serialize};

/// 一份相对上一次广播的文档增量。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocPatch {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub palette: Vec<Rgba>,
    pub layers: Vec<super::document::Layer>,
    pub frames: Vec<super::document::Frame>,
    pub palettes: Vec<super::document::NamedPalette>,
    pub revision: u64,
    /// 整块换掉的 cel：`(layer_id, frame_id, indices)`。
    /// 新建的、被写过的都在这儿。配色范围换套不另立门户——那边先把这一层的
    /// 索引就近重映射一遍（ops::SetLayerPalette -> requantize_layer），索引
    /// 没动的 cel 画面也没动，照旧不必过桥。
    pub cels: Vec<(String, String, Vec<u16>)>,
    /// 删掉的 cel：`(layer_id, frame_id)`。删图层、删帧都落在这里。
    pub dropped: Vec<(String, String)>,
}

impl DocPatch {
    /// 第一次广播：没有基准可比，元数据和全部 cel 一次给全。
    pub fn full(doc: &Document) -> Self {
        let mut cels = Vec::new();
        for (layer_id, frames) in &doc.cels {
            for (frame_id, cel) in frames {
                cels.push((layer_id.clone(), frame_id.clone(), cel.indices.clone()));
            }
        }
        Self {
            name: doc.name.clone(),
            width: doc.width,
            height: doc.height,
            palette: doc.palette.clone(),
            layers: doc.layers.clone(),
            frames: doc.frames.clone(),
            palettes: doc.palettes.clone(),
            revision: doc.revision,
            cels,
            dropped: Vec::new(),
        }
    }

    /// 从改动前后的两份文档算出增量。
    ///
    /// 按 (layer, frame) 逐个比 `indices`：Vec<u16> 相等是一次 memcmp，
    /// 比序列化整个文档便宜两三个数量级。宁可多比一遍，也不能靠「哪个工具
    /// 写过的」来记——漏记一处，前端那份文档就永久缺一个 cel，而且不报错。
    pub fn diff(before: &Document, after: &Document) -> Self {
        let mut cels = Vec::new();
        let mut dropped = Vec::new();
        for (layer_id, frames) in &after.cels {
            let old = before.cels.get(layer_id);
            for (frame_id, cel) in frames {
                let unchanged = old
                    .and_then(|f| f.get(frame_id))
                    .is_some_and(|prev| prev.indices == cel.indices);
                if !unchanged {
                    cels.push((layer_id.clone(), frame_id.clone(), cel.indices.clone()));
                }
            }
        }
        for (layer_id, frames) in &before.cels {
            let now = after.cels.get(layer_id);
            for frame_id in frames.keys() {
                if !now.is_some_and(|f| f.contains_key(frame_id)) {
                    dropped.push((layer_id.clone(), frame_id.clone()));
                }
            }
        }
        Self {
            name: after.name.clone(),
            width: after.width,
            height: after.height,
            palette: after.palette.clone(),
            layers: after.layers.clone(),
            frames: after.frames.clone(),
            palettes: after.palettes.clone(),
            revision: after.revision,
            cels,
            dropped,
        }
    }

    /// 合并进一份已有文档，得到新的完整文档。
    ///
    /// 真机上的合并发生在前端（TS 侧 `mergePatch`），这里是与它逐步对齐的
    /// 镜像：撤销链路上撤回到旧文档时，两边算出同一份结果才不会各画各的。
    ///
    /// cel 只许长在元数据点名的层与帧里。光靠 `dropped` 收场不够稳：事件桥还没
    /// 挂上监听、窗口藏起来那一阵的丢包，都会让一条 dropped 永远缺席，而层号
    /// 帧号是会复用的——下一层拿回 "L0" 时，旧像素就跟着新层一起显形。按元数据
    /// 把门管死，任何一次漏报都能在下一条增量里自愈。
    pub fn apply(&self, current: &Document) -> Document {
        use std::collections::HashSet;
        let live_layers: HashSet<&str> = self.layers.iter().map(|l| l.id.as_str()).collect();
        let live_frames: HashSet<&str> = self.frames.iter().map(|f| f.id.as_str()).collect();
        let mut cels: BTreeMap<String, BTreeMap<String, super::document::Cel>> = BTreeMap::new();
        for (layer_id, frames) in &current.cels {
            if !live_layers.contains(layer_id.as_str()) {
                continue;
            }
            let kept = frames
                .iter()
                .filter(|(frame_id, _)| live_frames.contains(frame_id.as_str()))
                .map(|(frame_id, cel)| (frame_id.clone(), cel.clone()))
                .collect();
            cels.insert(layer_id.clone(), kept);
        }
        for (layer_id, frame_id) in &self.dropped {
            if let Some(frames) = cels.get_mut(layer_id) {
                frames.remove(frame_id);
            }
        }
        for (layer_id, frame_id, indices) in &self.cels {
            cels.entry(layer_id.clone()).or_default().insert(
                frame_id.clone(),
                super::document::Cel {
                    indices: indices.clone(),
                },
            );
        }
        Document {
            name: self.name.clone(),
            width: self.width,
            height: self.height,
            palette: self.palette.clone(),
            layers: self.layers.clone(),
            frames: self.frames.clone(),
            cels,
            palettes: self.palettes.clone(),
            revision: self.revision,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{apply_batch, PixelCell, PixelOperation};

    fn doc() -> Document {
        Document::new("t", 4, 4).expect("4x4 is within limits")
    }

    fn painted() -> Document {
        let mut d = doc();
        apply_batch(
            &mut d,
            &[PixelOperation::AddPaletteColors {
                colors: vec!["#FF004D".into()],
            }],
        )
        .expect("adding one color is fine");
        apply_batch(
            &mut d,
            &[PixelOperation::SetPixels {
                layer: "L0".into(),
                frame: "F0".into(),
                cells: vec![
                    PixelCell {
                        x: 0,
                        y: 0,
                        color: "#FF004D".into(),
                    },
                    PixelCell {
                        x: 1,
                        y: 0,
                        color: "#FF004D".into(),
                    },
                ],
            }],
        )
        .expect("painting two cells is fine");
        d
    }

    /// 什么都没动：增量必须是空的。非空的增量会让前端每回合都重画一遍画布。
    #[test]
    fn an_unchanged_document_yields_an_empty_patch() {
        let d = painted();
        let patch = DocPatch::diff(&d, &d);
        assert!(patch.cels.is_empty(), "没有变化就不该带 cel: {patch:?}");
        assert!(patch.dropped.is_empty());
    }

    /// 只吐写过的那个 cel：同一帧没被动过的部分一个字节都不该过桥。
    #[test]
    fn only_the_touched_cel_travels() {
        let mut d = painted();
        let before = d.clone();
        apply_batch(
            &mut d,
            &[PixelOperation::SetPixels {
                layer: "L0".into(),
                frame: "F0".into(),
                cells: vec![PixelCell {
                    x: 0,
                    y: 1,
                    color: "#FF004D".into(),
                }],
            }],
        )
        .expect("painting one more cell is fine");
        let patch = DocPatch::diff(&before, &d);
        assert_eq!(patch.cels.len(), 1, "只该有一个 cel 变了: {patch:?}");
        assert_eq!(patch.cels[0].0, "L0");
        assert_eq!(patch.cels[0].1, "F0");
    }

    /// 删帧要报在 dropped 里：少了它，前端的帧条删了、cel 还挂着，
    /// 下一次合并又会把它塞回来。
    #[test]
    fn a_deleted_frame_reports_its_cels_as_dropped() {
        let mut d = painted();
        apply_batch(
            &mut d,
            &[PixelOperation::CreateFrame {
                after: None,
                duration_ms: 83,
                id: None,
            }],
        )
        .expect("one more frame is fine");
        let before = d.clone();
        apply_batch(&mut d, &[PixelOperation::DeleteFrame { id: "F1".into() }])
            .expect("deleting a frame is fine");
        let patch = DocPatch::diff(&before, &d);
        assert_eq!(patch.dropped.len(), 1, "删掉的那一帧要报出来: {patch:?}");
        assert_eq!(patch.dropped[0].1, "F1");
    }

    /// full + apply 必须是恒等变换：这是第一条事件，前端全靠它拿到完整文档。
    #[test]
    fn a_full_patch_round_trips_the_whole_document() {
        let d = painted();
        let empty = Document::new("t", 1, 1).expect("1x1 is within limits");
        let merged = DocPatch::full(&d).apply(&empty);
        assert!(same_document(&merged, &d), "合并回去必须和原文一模一样");
    }

    /// diff + apply 也必须是恒等变换：撤销量大时这条是撤销链路的安全带。
    #[test]
    fn a_diff_patch_round_trips_the_whole_document() {
        let mut d = painted();
        let before = d.clone();
        apply_batch(
            &mut d,
            &[PixelOperation::AddPaletteColors {
                colors: vec!["#29ADFF".into()],
            }],
        )
        .expect("one more color is fine");
        apply_batch(
            &mut d,
            &[PixelOperation::SetPixels {
                layer: "L0".into(),
                frame: "F0".into(),
                cells: vec![PixelCell {
                    x: 2,
                    y: 2,
                    color: "#29ADFF".into(),
                }],
            }],
        )
        .expect("painting is fine");
        let merged = DocPatch::diff(&before, &d).apply(&before);
        assert!(same_document(&merged, &d), "合并回去必须和原文一模一样");
    }

    /// 空 cel 也要能过桥：全透明的帧照样是一份数据，漏了它前端会缺一块。
    #[test]
    fn an_all_transparent_cel_still_travels() {
        let d = doc();
        let patch = DocPatch::full(&d);
        assert_eq!(patch.cels.len(), 1, "新建文档也有一个空 cel");
        assert_eq!(patch.cels[0].2, vec![0u16; 16]);
    }

    /// 带着 dropped 的那条增量丢了（事件桥还没挂上监听、窗口藏着那一阵），
    /// 下一条增量也得自己把死层收尸。
    ///
    /// 不收的话层号一复用就出事：`next_id` 会把 "L0" 还给新层，前端那份旧 L0
    /// 的像素会在新层上显形——用户看到的是「刚建的图层里莫名有上一任的画」，
    /// 而且怎么擦都擦不干净，因为后端那边新层本来就是空的。
    #[test]
    fn a_patch_after_a_lost_dropped_entry_still_prunes_dead_cels() {
        let mut d = doc();
        apply_batch(
            &mut d,
            &[PixelOperation::AddPaletteColors {
                colors: vec!["#FF004D".into()],
            }],
        )
        .expect("adding one color is fine");
        apply_batch(
            &mut d,
            &[PixelOperation::CreateLayer {
                after: None,
                name: None,
                palette_id: None,
                locked: None,
                id: None,
            }],
        )
        .expect("one more layer is fine");
        apply_batch(
            &mut d,
            &[PixelOperation::SetPixels {
                layer: "L0".into(),
                frame: "F0".into(),
                cells: vec![PixelCell {
                    x: 0,
                    y: 0,
                    color: "#FF004D".into(),
                }],
            }],
        )
        .expect("painting on the old L0 is fine");
        let before = d.clone();

        // 后端删掉 L0：文档只剩 L1，这一层的 cel 整棵树跟着走。
        apply_batch(&mut d, &[PixelOperation::DeleteLayer { id: "L0".into() }])
            .expect("deleting a layer is fine");
        let lost = DocPatch::diff(&before, &d);
        assert!(
            lost.dropped.iter().any(|(layer, _)| layer == "L0"),
            "删层要把这一层的 cel 报出来: {:?}",
            lost.dropped
        );

        // 前端只收到后面这条「什么都没改」：元数据里已经没有 L0 了，
        // 合并必须照元数据收尸，而不是等一条可能永远不来的 dropped。
        let second = DocPatch::diff(&d, &d);
        let merged = second.apply(&before);
        assert!(
            !merged.cels.contains_key("L0"),
            "死层的 cel 不该活着: {:?}",
            merged.cels.keys().collect::<Vec<_>>()
        );
        assert!(merged.cels.contains_key("L1"), "活着的层一个都不能少");

        // 层号是会复用的：再拿回 "L0" 的新层读到空白，不是上一任的笔迹。
        apply_batch(
            &mut d,
            &[PixelOperation::CreateLayer {
                after: None,
                name: None,
                palette_id: None,
                locked: None,
                id: None,
            }],
        )
        .expect("recreating the layer number is fine");
        let reused = DocPatch::diff(&d, &d);
        assert_eq!(
            reused.layers.iter().filter(|l| l.id == "L0").count(),
            1,
            "删掉的层号确实会被复用"
        );
        let after_reuse = reused.apply(&merged);
        let inherited = after_reuse
            .cels
            .get("L0")
            .and_then(|frames| frames.get("F0"))
            .is_some_and(|cel| cel.indices.contains(&1));
        assert!(
            !inherited,
            "复用的层号不许读到上一任的像素: {:?}",
            after_reuse.cels.get("L0")
        );
    }

    /// Document 没实现 PartialEq（它是主数据，不该为一个断言补派生），
    /// 两轮恒等性都改成按 JSON 比：序列化形式一致就等于内容一致。
    fn same_document(a: &Document, b: &Document) -> bool {
        serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
    }
}
