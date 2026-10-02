// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 核心流程集成测试：文档 -> 类型化操作 -> Lua 着色器 -> RLE 读回 -> .aip 往返。
//! 这些用例锁住 agent 主循环依赖的几条契约，改动底层时先跑这里。

use pixel_core::aip;
use pixel_core::context;
use pixel_core::document::{Document, Rgba};
use pixel_core::ops::{self, PixelOperation};
use pixel_core::rle;
use pixel_core::shader::{self, ShaderBudget};
use pixel_core::sheet;

fn blank() -> Document {
    Document::new("test", 16, 16).expect("16x16 within limits")
}

#[test]
fn ops_create_structure_then_set_pixels() {
    let mut doc = blank();
    let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());

    let rev = ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::CreateLayer {
                after: Some(layer.clone()),
                name: Some("fx".into()),
                id: None,
                palette_id: None,
                locked: None,
            },
            PixelOperation::CreateFrame {
                after: None,
                duration_ms: 90,
                id: None,
            },
            PixelOperation::AddPaletteColors {
                colors: vec!["#FF004D".into(), "#29ADFF".into()],
            },
            PixelOperation::SetPixels {
                layer: layer.clone(),
                frame: frame.clone(),
                cells: vec![
                    ops::PixelCell {
                        x: 0,
                        y: 0,
                        color: "#FF004D".into(),
                    },
                    ops::PixelCell {
                        x: 1,
                        y: 0,
                        color: "#FF004D".into(),
                    },
                ],
            },
        ],
    )
    .expect("batch applies");

    assert_eq!(doc.layers.len(), 2);
    assert_eq!(doc.layers[1].name, "fx");
    assert_eq!(doc.frames.len(), 2);
    assert_eq!(doc.frames[1].duration_ms, 90);
    assert!(rev > 0);
    assert_eq!(
        doc.cel(&layer, &frame).unwrap().get(doc.width, 0, 0),
        Some(1)
    );
    assert_eq!(
        doc.cel(&layer, &frame).unwrap().get(doc.width, 1, 0),
        Some(1)
    );
    // 第二帧是新 cel，第一帧的像素不应泄漏过去
    let second = doc.frames[1].id.clone();
    assert_eq!(
        doc.cel(&layer, &second).unwrap().get(doc.width, 0, 0),
        Some(0)
    );
}

#[test]
fn ops_reject_out_of_range_pixels_and_bad_layer() {
    let mut doc = blank();
    let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    let bad_frame = doc.frames[0].id.clone();
    assert!(ops::apply_one(
        &mut doc,
        &PixelOperation::SetPixels {
            layer: layer.clone(),
            frame,
            cells: vec![ops::PixelCell {
                x: 99,
                y: 0,
                color: "#FFFFFF".into(),
            }],
        },
    )
    .is_err());
    assert!(ops::apply_one(
        &mut doc,
        &PixelOperation::SetPixels {
            layer: "nope".into(),
            frame: bad_frame,
            cells: vec![],
        },
    )
    .is_err());
}

#[test]
fn shader_draws_with_loops_and_palette_helpers() {
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::AddPaletteColors {
            colors: vec!["#FF004D".into()],
        }],
    )
    .unwrap();
    let outcome = shader::run_shader(
        &mut doc,
        &layer,
        r##"
        pal(1)
        for y = 2, 9 do
          for x = 2, 9 do
            pset(x, y, pal(1))
          end
        end
        line(0, 0, 15, 15, "#FFFFFF")
        "##,
        false,
        &ShaderBudget::default(),
    )
    .expect("shader runs");

    assert_eq!(outcome.frames_rendered, 1);
    let cel = doc.cel(&layer, &doc.frames[0].id).unwrap();
    // 对角线穿过 (5,5)，所以那里应是白线 intern 出来的索引 2；取格内非对角线点验证方块填充。
    assert_eq!(
        cel.get(doc.width, 3, 5),
        Some(1),
        "square fill keeps the palette color"
    );
    assert_eq!(
        cel.get(doc.width, 0, 0),
        Some(2),
        "line interns a palette color"
    );
    assert_eq!(
        cel.get(doc.width, 10, 2),
        Some(0),
        "outside both shapes stays transparent"
    );
}

#[test]
fn color_helpers_compose_in_every_direction() {
    // 真模型踩过这个坑：先 `local red = hex('#e74c3c')` 再 `mix(red, '#000000', .35)`
    // 直接报 bad color 1 —— hex() 是唯一交索引的助手，别的全说 hex 串，白烧一个来回。
    // 颜色在 Lua 侧必须是一种能到处传的值，怎么串都行。
    let mut doc = blank();
    let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    let script = r##"
    local red    = hex('#e74c3c')
    local redDk  = mix(red, '#000000', 0.35)
    local redLt  = mix(red, '#ffffff', 0.3)
    local stem   = hex('#6e2c00')
    local leaf   = hex('#27ae60')
    local leafDk = mix(leaf, '#000000', 0.3)
    local ghost  = alpha(red, 0.5)
    canvas.clear(nil)
    for x = 0, 15 do
      canvas.pset(x, 1, redDk)
      canvas.pset(x, 2, redLt)
      canvas.pset(x, 3, stem)
      canvas.pset(x, 4, leafDk)
      canvas.pset(x, 5, ghost)
      canvas.pset(x, 6, red)
    end
    canvas.pset(0, 8, 'transparent')
    "##;
    shader::run_shader(&mut doc, &layer, script, false, &ShaderBudget::default())
        .expect("颜色助手要能随便串");
    let cel = doc.cel(&layer, &frame).unwrap();
    let mut seen = Vec::new();
    for x in 0..16 {
        for y in 1..7 {
            let idx = cel.get(doc.width, x, y).unwrap();
            assert_ne!(idx, 0, "({x},{y}) 该着色");
            let color = doc.color_of(idx).unwrap();
            if !seen.contains(&color) {
                seen.push(color);
            }
        }
        assert_eq!(cel.get(doc.width, x, 8).unwrap(), 0, "'transparent' 该擦除");
    }
    // hex() 现在交 hex 串，吃进 mix 之后逐个都还得是不同色号。
    assert_eq!(
        seen.len(),
        6,
        "六个助手结果各是一个色号，实际 {}：{seen:?}",
        seen.len()
    );
}

#[test]
fn stamp_legend_takes_a_color_helper_result() {
    // legend 收 "#hex" 串；hex() 以前交索引，写进 legend 就废了。
    let mut doc = blank();
    let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    shader::run_shader(
        &mut doc,
        &layer,
        r##"
        stamp({
            'aab',
            'abb',
        }, {a = hex('#FF004D'), b = hex(mix('#FF004D', '#000000', 0.5))}, 0, 0)
        "##,
        false,
        &ShaderBudget::default(),
    )
    .expect("legend 里放颜色助手的结果要能跑");
    let cel = doc.cel(&layer, &frame).unwrap();
    let a = cel.get(doc.width, 0, 0).unwrap();
    let b = cel.get(doc.width, 2, 0).unwrap();
    assert_ne!(a, 0);
    assert_ne!(b, 0);
    assert_ne!(a, b, "混过的那一格得是另一个色号");
}

#[test]
fn pal_out_of_range_names_what_to_do() {
    // 新文档一个色都没有，模型顺手就写 pal(1) 然后死在这儿。
    // 报错必须自己说清出路，不然它只会换一个数字再撞一次墙。
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    let err = shader::run_shader(
        &mut doc,
        &layer,
        "canvas.pset(1, 1, pal(1))",
        false,
        &ShaderBudget::default(),
    )
    .expect_err("空调色板上 pal(1) 必须报错");
    let msg = err.to_string();
    assert!(msg.contains("out of range"), "报错要说越界：{msg}");
    assert!(
        msg.contains("add_palette_colors"),
        "报错要指一条出路：{msg}"
    );
    assert!(msg.contains("#RRGGBB"), "报错要指出字符串也收：{msg}");
}

#[test]
fn shader_animate_clears_each_frame_and_moves_with_phase() {
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec!["#29ADFF".into()],
            },
            PixelOperation::CreateFrame {
                after: None,
                duration_ms: 80,
                id: None,
            },
            PixelOperation::CreateFrame {
                after: None,
                duration_ms: 80,
                id: None,
            },
            PixelOperation::CreateFrame {
                after: None,
                duration_ms: 80,
                id: None,
            },
        ],
    )
    .unwrap();

    let outcome = shader::run_shader(
        &mut doc,
        &layer,
        r#"
        local r = 1 + math.floor(phase * 3)
        circfill(canvas.width / 2, canvas.height / 2, r, pal(1))
        "#,
        true,
        &ShaderBudget::default(),
    )
    .expect("shader runs");

    assert_eq!(outcome.frames_rendered, 4);
    let counts: Vec<usize> = doc
        .frames
        .iter()
        .map(|f| {
            doc.cel(&layer, &f.id)
                .unwrap()
                .indices
                .iter()
                .filter(|i| **i == 1)
                .count()
        })
        .collect();
    assert!(
        counts.windows(2).all(|w| w[0] < w[1]),
        "每帧半径递增: {counts:?}"
    );
    assert!(counts[0] > 0, "第一帧就有内容: {counts:?}");
    assert!(
        counts.windows(2).all(|w| w[0] < w[1]),
        "每帧半径递增: {counts:?}"
    );
    assert!(counts[0] > 0, "第一帧就有内容: {counts:?}");
}

#[test]
fn shader_budget_aborts_runaway_loops() {
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    let err = shader::run_shader(
        &mut doc,
        &layer,
        "while true do end",
        false,
        &ShaderBudget {
            max_instructions: 200_000,
            max_seconds: 5,
        },
    )
    .expect_err("infinite loop must be cut off");
    assert!(matches!(err, shader::ShaderError::Budget(_)), "got {err:?}");
}

#[test]
fn rle_window_encodes_runs_and_legend() {
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    let frame = doc.frames[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec!["#FF004D".into()],
            },
            PixelOperation::SetPixels {
                layer: layer.clone(),
                frame: frame.clone(),
                cells: (0..10)
                    .map(|x| ops::PixelCell {
                        x,
                        y: 3,
                        color: "#FF004D".into(),
                    })
                    .collect(),
            },
        ],
    )
    .unwrap();

    let view = rle::render_window(&doc, &layer, &frame, Some((0, 0, 16, 16)), 4096).expect("view");
    assert_eq!(view.rows.len(), 16);
    assert_eq!(view.rows[3], "10a6.", "十个同色 + 六个透明折叠成两个 run");
    assert_eq!(view.rows[0], "16.", "整行透明也折叠成一个 run");
    assert_eq!(
        view.legend
            .symbol_of(Some(Rgba::parse_hex("#FF004D").unwrap())),
        'a'
    );
}

/// 区域读回的一行都不能少。agent 修局部（比如把一只猫的耳朵抬两个像素）
/// 全指望这个视图：区域要几行就得给几行，少一行那段像素就成了未知。
#[test]
fn a_partial_region_view_returns_every_row_of_the_region() {
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    let frame = doc.frames[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec!["#FF004D".into()],
            },
            PixelOperation::SetPixels {
                layer: layer.clone(),
                frame: frame.clone(),
                cells: (3..8)
                    .map(|x| ops::PixelCell {
                        x,
                        y: 7,
                        color: "#FF004D".into(),
                    })
                    .collect(),
            },
        ],
    )
    .unwrap();

    // 16x16 的画布只读 (2,3) 起 5x7 一块。
    let view =
        rle::render_window(&doc, &layer, &frame, Some((2, 3, 5, 7)), 4096).expect("region view");
    assert_eq!(view.rows.len(), 7, "区域多高就要几行");
    assert_eq!(view.width, 5);
    assert_eq!(view.height, 7);
    assert_eq!(view.x, 2);
    assert_eq!(view.y, 3);
    // 画在 y=7（区域内第 4 行）：区域外的 x=7 被裁掉，剩四个同色
    // 加左侧一个透明，两个 run 数清楚。
    assert_eq!(view.rows[4], ".4a");
    assert_eq!(view.rows[0], "5.", "没画过的行整行透明");
}

#[test]
fn aip_round_trip_keeps_structure_and_pixels() {
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    let frame = doc.frames[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec!["#123456".into(), "#ABCDEF".into()],
            },
            PixelOperation::RenameLayer {
                id: layer.clone(),
                name: "art".into(),
            },
            PixelOperation::SetPixels {
                layer: layer.clone(),
                frame: frame.clone(),
                cells: vec![
                    ops::PixelCell {
                        x: 1,
                        y: 1,
                        color: "#123456".into(),
                    },
                    ops::PixelCell {
                        x: 2,
                        y: 2,
                        color: "#ABCDEF".into(),
                    },
                ],
            },
        ],
    )
    .unwrap();

    let text = context::to_aip(&doc).expect("export");
    assert!(text.contains("@layers"), "aip 文本必须声明图层");
    let back = aip::import_any(&text).expect("import");
    assert_eq!(back.width, 16);
    assert_eq!(back.height, 16);
    assert_eq!(back.layers[0].name, "art");
    assert_eq!(back.palette.len(), doc.palette.len());
    assert_eq!(
        back.cel(&back.layers[0].id, &back.frames[0].id)
            .unwrap()
            .get(back.width, 2, 2),
        Some(2),
    );
}

#[test]
fn aip_rejects_malformed_input() {
    assert!(aip::import_any("not an aip file at all").is_err());
    assert!(aip::import_any("@layers\nL0 | base\n@frames\nF0\n").is_err());
}

#[test]
fn document_json_round_trips_across_the_tauri_boundary() {
    // Tauri 命令靠 serde_json 在前后端之间搬文档：cels 的键必须能序列化成
    // JSON 对象键，且回读后图层/帧/像素/revision 一个都不丢。
    let mut doc = blank();
    let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec!["#112233".into()],
            },
            PixelOperation::CreateFrame {
                after: None,
                duration_ms: 120,
                id: None,
            },
            PixelOperation::SetPixels {
                layer: layer.clone(),
                frame: frame.clone(),
                cells: vec![
                    ops::PixelCell {
                        x: 0,
                        y: 0,
                        color: "#112233".into(),
                    },
                    ops::PixelCell {
                        x: 15,
                        y: 15,
                        color: "#112233".into(),
                    },
                ],
            },
        ],
    )
    .unwrap();

    let json = serde_json::to_value(&doc).expect("document serializes for the webview");
    assert!(json["cels"]["L0"].is_object(), "cels are keyed by layer id");
    assert!(
        json["cels"]["L0"]["F1"].is_object(),
        "every layer x frame has a cel"
    );

    let back: Document = serde_json::from_value(json).expect("document deserializes back");
    assert_eq!(back.revision, doc.revision);
    assert_eq!(back.frames.len(), 2);
    assert_eq!(back.frames[1].duration_ms, 120);
    assert_eq!(back.palette.len(), 1);
    let second = back.frames[1].id.clone();
    assert_eq!(
        back.cel(&layer, &frame).unwrap().get(back.width, 15, 15),
        Some(1)
    );
    assert_eq!(
        back.cel(&layer, &second).unwrap().get(back.width, 0, 0),
        Some(0)
    );
}

#[test]
fn document_rejects_oversized_canvas() {
    assert!(Document::new("huge", 2048, 16).is_err());
}

/// 前端把编辑器操作按 serde 内部 tag 发过来。tag 名必须和 TypeScript 侧
/// `EditorOperation` 的 `op` 字段逐字一致：改一边忘了另一边，图层改名
/// 就会在前端「成功」、在后端静默失败。
#[test]
fn editor_operations_deserialize_from_the_webview_wire_form() {
    let renamed: PixelOperation = serde_json::from_value(serde_json::json!({
        "op": "rename_layer",
        "id": "L0",
        "name": "主角层",
    }))
    .expect("rename_layer arrives with its snake_case tag");
    match renamed {
        PixelOperation::RenameLayer { id, name } => {
            assert_eq!(id, "L0");
            assert_eq!(name, "主角层");
        }
        other => panic!("rename_layer 解成了别的变体：{other:?}"),
    }

    // 没给 name 的 set_layer_properties 必须能解：字段是可选的，缺了就是不动。
    let partial: PixelOperation = serde_json::from_value(serde_json::json!({
        "op": "set_layer_properties",
        "id": "L0",
        "visible": false,
    }))
    .expect("set_layer_properties tolerates a missing opacity");
    assert!(matches!(
        partial,
        PixelOperation::SetLayerProperties {
            visible: Some(false),
            opacity: None,
            ..
        }
    ));

    // 帧与层的四个操作认 `frame`/`layer` 当主键：文档里就是这么写的，
    // 模型照文档发就得通。以前只认 `id`，一次报错白烧一整轮来回。
    for (json, expect) in [
        (
            serde_json::json!({"op": "set_frame_duration", "frame": "F0", "duration_ms": 80}),
            "帧时长",
        ),
        (
            serde_json::json!({"op": "delete_frame", "frame": "F0"}),
            "删帧",
        ),
        (
            serde_json::json!({"op": "move_frame", "frame": "F0", "to_index": 0}),
            "移帧",
        ),
        (
            serde_json::json!({"op": "rename_layer", "layer": "L0", "name": "x"}),
            "图层改名",
        ),
    ] {
        let parsed: PixelOperation = serde_json::from_value(json)
            .unwrap_or_else(|e| panic!("{expect} 用 frame/layer 字段必须能解：{e}"));
        assert!(
            matches!(
                parsed,
                PixelOperation::SetFrameDuration { .. }
                    | PixelOperation::DeleteFrame { .. }
                    | PixelOperation::MoveFrame { .. }
                    | PixelOperation::RenameLayer { .. }
            ),
            "{expect} 解成了别的变体"
        );
    }
}

/// 模型文档里 `duplicate_frame {frame, after?}` 和 `create_layer {palette_id?, locked?}`
/// 都带可选字段，落地行为必须和文档说的一致：after 说了插哪就插哪，
/// palette_id/locked 说了就照办，没说才继承邻居。
#[test]
fn optional_target_fields_of_frame_and_layer_ops_land_where_documented() {
    let mut doc = blank();
    for _ in 0..2 {
        ops::apply_batch(
            &mut doc,
            &[PixelOperation::CreateFrame {
                after: None,
                duration_ms: 100,
                id: None,
            }],
        )
        .expect("frame created");
    }
    let order: Vec<String> = doc.frames.iter().map(|f| f.id.clone()).collect();
    assert_eq!(order, vec!["F0", "F1", "F2"], "先备好三帧");

    // after 指到 F2：副本必须落在 F2 之后，而不是默认的源帧后面。
    let dup: PixelOperation = serde_json::from_value(serde_json::json!({
        "op": "duplicate_frame",
        "frame": "F0",
        "after": "F2",
    }))
    .expect("duplicate_frame with after parses");
    ops::apply_batch(&mut doc, &[dup]).expect("duplicate applies");
    let after_dup: Vec<String> = doc.frames.iter().map(|f| f.id.clone()).collect();
    assert_eq!(
        after_dup,
        vec!["F0", "F1", "F2", "F3"],
        "after 指定 F2 之后，副本就该是第四帧"
    );
    assert_eq!(
        doc.cels
            .get(&doc.layers[0].id)
            .and_then(|f| f.get("F3"))
            .expect("副本带自己的 cel")
            .indices
            .len(),
        usize::try_from(doc.width * doc.height).expect("16x16 fits usize"),
        "副本必须真带一格内容"
    );

    // 没给 after 就还是老规矩：紧跟在源帧后面。
    ops::apply_batch(
        &mut doc,
        &[serde_json::from_value::<PixelOperation>(serde_json::json!({
            "op": "duplicate_frame",
            "frame": "F3",
        }))
        .expect("duplicate_frame without after parses")],
    )
    .expect("duplicate applies");
    let ids: Vec<String> = doc.frames.iter().map(|f| f.id.clone()).collect();
    assert_eq!(ids[ids.len() - 2], "F3", "缺省落点仍在源帧后面");

    // after 指向源帧自己：就是「照着这一帧改」的默认动作，
    // 落点必须是源帧后面，不能插到它前面去。
    ops::apply_batch(
        &mut doc,
        &[serde_json::from_value::<PixelOperation>(serde_json::json!({
            "op": "duplicate_frame",
            "frame": "F0",
            "after": "F0",
        }))
        .expect("duplicate_frame with after=source parses")],
    )
    .expect("duplicate applies");
    let ids: Vec<String> = doc.frames.iter().map(|f| f.id.clone()).collect();
    assert_eq!(ids[0], "F0", "源帧还在第一位");
    assert_eq!(ids[1], "F5", "副本紧跟源帧，插在它后面");

    // 建层就报一套范围和锁，不该再靠第二句 set_layer_palette 补。
    let created: PixelOperation = serde_json::from_value(serde_json::json!({
        "op": "create_layer",
        "after": doc.layers[0].id.clone(),
        "name": "固定两色层",
        "palette_id": "onebit",
        "locked": true,
    }))
    .expect("create_layer with palette_id and locked parses");
    ops::apply_batch(&mut doc, &[created]).expect("create applies");
    let fresh = doc.layers.last().expect("新层在栈里");
    assert_eq!(fresh.name, "固定两色层");
    assert_eq!(fresh.palette_id, "onebit", "显式指定的范围要照办");
    assert!(fresh.locked, "显式上锁要照办");

    // 一个层只能指着一个范围：拼错的 id 必须当场报错，
    // 而不是建出一层指向空气、等后面才炸。
    let bad: PixelOperation = serde_json::from_value(serde_json::json!({
        "op": "create_layer",
        "palette_id": "onebitt",
    }))
    .expect("create_layer parses");
    assert!(
        ops::apply_batch(&mut doc, &[bad]).is_err(),
        "不存在的 palette_id 必须被拒"
    );
    let count = doc.layers.len();
    let err = ops::apply_batch(
        &mut doc,
        &[serde_json::from_value::<PixelOperation>(serde_json::json!({
            "op": "create_layer",
            "palette_id": "still-not-here",
        }))
        .expect("create_layer parses")],
    );
    assert!(err.is_err(), "拼错的范围 id 不许悄悄建层");
    assert_eq!(doc.layers.len(), count, "失败要整批回滚，不留下半层");
}

#[test]
fn exported_animation_lands_on_disk_and_reads_back() {
    use image::AnimationDecoder;
    use std::io::Cursor;

    // 两帧、两种颜色：导出的东西必须同时带对帧数和像素，只带对一样都不算能用。
    let mut doc = blank();
    ops::apply_one(
        &mut doc,
        &PixelOperation::CreateFrame {
            after: None,
            duration_ms: 90,
            id: None,
        },
    )
    .unwrap();
    ops::apply_one(
        &mut doc,
        &PixelOperation::AddPaletteColors {
            colors: vec!["#FF004D".into(), "#29ADFF".into()],
        },
    )
    .unwrap();
    // id 先取出来：&mut doc 之后不能再借 doc 读字段。
    let layer = doc.layers[0].id.clone();
    let first = doc.frames[0].id.clone();
    let second = doc.frames[1].id.clone();
    ops::apply_one(
        &mut doc,
        &PixelOperation::SetPixels {
            layer: layer.clone(),
            frame: first,
            cells: vec![ops::PixelCell {
                x: 0,
                y: 0,
                color: "#FF004D".into(),
            }],
        },
    )
    .unwrap();
    ops::apply_one(
        &mut doc,
        &PixelOperation::SetPixels {
            layer,
            frame: second,
            cells: vec![ops::PixelCell {
                x: 0,
                y: 0,
                color: "#29ADFF".into(),
            }],
        },
    )
    .unwrap();
    assert_ne!(doc.frames[0].id, doc.frames[1].id, "两帧必须是不同帧");

    // 走真实的写入路径：命令层就是 std::fs::write(bytes)，这里不能绕过。
    let path = std::env::temp_dir().join(format!("aipixel-flow-{}.gif", std::process::id()));
    let bytes = sheet::encode_gif(&doc).expect("gif encodes");
    std::fs::write(&path, &bytes).expect("gif lands on disk");

    let from_disk = std::fs::read(&path).expect("gif reads back");
    std::fs::remove_file(&path).ok();

    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(from_disk)).expect("valid gif");
    let frames = decoder
        .into_frames()
        .collect_frames()
        .expect("frames decode");
    assert_eq!(frames.len(), 2);
    assert_eq!(
        frames[0].buffer().get_pixel(0, 0).0,
        [0xFF, 0x00, 0x4D, 0xFF]
    );
    assert_eq!(
        frames[1].buffer().get_pixel(0, 0).0,
        [0x29, 0xAD, 0xFF, 0xFF]
    );

    // 精灵表同理：落盘、回读、按行找回原来的帧。
    let sheet_path = std::env::temp_dir().join(format!("aipixel-sheet-{}.png", std::process::id()));
    let sheet_bytes =
        pixel_core::png::encode_png(&sheet::spritesheet(&doc, 1)).expect("sheet encodes");
    std::fs::write(&sheet_path, &sheet_bytes).expect("sheet lands on disk");
    let loaded = image::load_from_memory(&sheet_bytes).expect("sheet parses");
    assert_eq!(loaded.to_rgba8().dimensions(), (16, 32));
    assert_eq!(
        loaded.to_rgba8().get_pixel(0, 0).0,
        [0xFF, 0x00, 0x4D, 0xFF]
    );
    assert_eq!(
        loaded.to_rgba8().get_pixel(0, 16).0,
        [0x29, 0xAD, 0xFF, 0xFF]
    );
    std::fs::remove_file(&sheet_path).ok();
}

#[test]
fn set_palette_remaps_painted_pixels_to_nearest_color() {
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    let frame = doc.frames[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec!["#FF0000".into(), "#0000FF".into()],
            },
            PixelOperation::SetPixels {
                layer: layer.clone(),
                frame: frame.clone(),
                cells: (0..4)
                    .map(|x| ops::PixelCell {
                        x,
                        y: 0,
                        color: "#FF0000".into(),
                    })
                    .chain((0..4).map(|x| ops::PixelCell {
                        x,
                        y: 1,
                        color: "#0000FF".into(),
                    }))
                    .collect(),
            },
        ],
    )
    .unwrap();

    // 换到灰阶两色：红色就近变成白，蓝色就近变成黑，画面不丢成透明。
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::SetPalette {
            colors: vec!["#000000".into(), "#FFFFFF".into()],
        }],
    )
    .unwrap();

    assert_eq!(doc.palette.len(), 2);
    let cel = doc.cel(&layer, &frame).unwrap();
    assert_eq!(cel.indices[0], 2, "红色最近的是白（索引 2）");
    assert_eq!(cel.indices[16], 1, "蓝色最近的是黑（索引 1）");

    // 合成回位图看最终颜色：就近映射必须落到肉眼可辨的结果上。
    let flat = pixel_core::png::flatten(&doc);
    assert_eq!(flat.get_pixel(0, 0).0, [0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(flat.get_pixel(0, 1).0, [0x00, 0x00, 0x00, 0xFF]);
}

// ---- 命名配色范围 ----

/// 每层各自认领一套范围，锁着的那层越界颜色要被拉回范围里。
#[test]
fn locked_layer_clamps_colors_into_its_own_range() {
    let mut doc = blank();
    let (l0, f0) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::CreateLayer {
                after: Some(l0.clone()),
                name: Some("mono".into()),
                id: None,
                palette_id: None,
                locked: None,
            },
            PixelOperation::SetLayerPalette {
                layer: "L1".into(),
                palette_id: "onebit".into(),
            },
            PixelOperation::SetLayerLocked {
                layer: "L1".into(),
                locked: true,
            },
        ],
    )
    .expect("batch applies");

    // 两层原先是同一套范围；现在 L1 锁在黑白二色上，L0 还开着。
    assert_eq!(doc.layer("L1").unwrap().palette_id, "onebit");
    assert_eq!(doc.layer("L0").unwrap().palette_id, "sweetie16");

    for layer in ["L0", "L1"] {
        ops::apply_batch(
            &mut doc,
            &[PixelOperation::SetPixels {
                layer: layer.into(),
                frame: f0.clone(),
                cells: vec![ops::PixelCell {
                    x: 0,
                    y: 0,
                    color: "#88ff00".into(),
                }],
            }],
        )
        .expect("paint applies");
    }

    // 开着的层原样落笔；锁住的层被拉回黑或白。
    assert_eq!(
        doc.cel("L0", &f0).unwrap().indices[0],
        doc.palette_index_of(Rgba::rgb(0x88, 0xff, 0x00)).unwrap(),
        "未锁的层不许动颜色"
    );
    let clamped = doc.palette[doc.cel("L1", &f0).unwrap().indices[0] as usize - 1];
    let range = &doc.layer_palette("L1").unwrap().colors;
    assert!(
        range.contains(&clamped),
        "锁住的层必须用范围里的颜色，实际落了 {} ",
        clamped.to_hex()
    );
}

#[test]
fn layer_ranges_are_independent() {
    let mut doc = blank();
    let (l0, f0) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::SetLayerPalette {
                layer: l0.clone(),
                palette_id: "gameboy".into(),
            },
            PixelOperation::SetLayerLocked {
                layer: l0.clone(),
                locked: true,
            },
            PixelOperation::CreateLayer {
                after: Some(l0.clone()),
                name: Some("free".into()),
                id: None,
                palette_id: None,
                locked: None,
            },
            PixelOperation::SetLayerPalette {
                layer: "L1".into(),
                palette_id: "pico8".into(),
            },
        ],
    )
    .expect("batch applies");

    assert_eq!(doc.layer("L0").unwrap().palette_id, "gameboy");
    assert_eq!(doc.layer("L1").unwrap().palette_id, "pico8");
    assert!(doc.layer("L0").unwrap().locked);
    // 紧挨着插进来的层连着「锁不锁」一起继承，换范围时才把锁解开。
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::SetLayerLocked {
            layer: "L1".into(),
            locked: false,
        }],
    )
    .expect("unlock applies");
    assert!(!doc.layer("L1").unwrap().locked);
    assert!(doc.layer("L0").unwrap().locked, "另一层的锁不许被顺手改掉");
    let _ = f0;
}

#[test]
fn set_layer_palette_requantizes_only_that_layer() {
    let mut doc = blank();
    let (l0, f0) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::CreateLayer {
                after: Some(l0.clone()),
                name: Some("other".into()),
                id: None,
                palette_id: None,
                locked: None,
            },
            PixelOperation::SetPixels {
                layer: l0.clone(),
                frame: f0.clone(),
                cells: vec![ops::PixelCell {
                    x: 0,
                    y: 0,
                    color: "#ff004d".into(),
                }],
            },
            PixelOperation::SetPixels {
                layer: "L1".into(),
                frame: f0.clone(),
                cells: vec![ops::PixelCell {
                    x: 0,
                    y: 0,
                    color: "#ff004d".into(),
                }],
            },
            PixelOperation::SetLayerPalette {
                layer: l0.clone(),
                palette_id: "onebit".into(),
            },
        ],
    )
    .expect("batch applies");

    let range = doc.layer_palette("L0").unwrap().colors.clone();
    let moved = doc.palette[doc.cel("L0", &f0).unwrap().indices[0] as usize - 1];
    let kept = doc.palette[doc.cel("L1", &f0).unwrap().indices[0] as usize - 1];
    assert!(range.contains(&moved), "换范围的层要就地归队");
    assert_eq!(kept, Rgba::rgb(0xff, 0x00, 0x4d), "另一层一个色都不许动");
}

#[test]
fn builtin_presets_are_read_only() {
    let mut doc = blank();
    let l0 = doc.layers[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::SetLayerPalette {
            layer: l0.clone(),
            palette_id: "pico8".into(),
        }],
    )
    .expect("switch applies");

    let attempts: Vec<PixelOperation> = vec![
        PixelOperation::RenamePalette {
            id: "pico8".into(),
            name: "我的".into(),
        },
        PixelOperation::AddPaletteColor {
            id: "pico8".into(),
            color: "#123456".into(),
        },
        PixelOperation::RemovePaletteColor {
            id: "pico8".into(),
            index: 0,
            replacement: None,
        },
        PixelOperation::DeletePalette {
            id: "pico8".into(),
            fallback: None,
        },
    ];
    for op in attempts {
        let err = ops::apply_batch(&mut doc, &[op]).expect_err("内置预设动不得");
        assert!(
            matches!(err, ops::OperationError::BuiltinPalette(_)),
            "expected BuiltinPalette, got {err}"
        );
    }
    assert_eq!(doc.palette_by_id("pico8").unwrap().colors.len(), 16);
}

#[test]
fn editing_a_builtin_forks_it_and_leaves_the_original_alone() {
    let mut doc = blank();
    let l0 = doc.layers[0].id.clone();
    let before = doc.palette_by_id("pico8").unwrap().colors.clone();

    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::SetLayerPalette {
                layer: l0.clone(),
                palette_id: "pico8".into(),
            },
            PixelOperation::CreatePalette {
                name: "PICO-8 copy".into(),
                from: Some("pico8".into()),
                colors: vec!["#123456".into()],
                layer: Some(l0.clone()),
                id: None,
            },
        ],
    )
    .expect("batch applies");

    assert_eq!(
        doc.palette_by_id("pico8").unwrap().colors,
        before,
        "内置那套一个色都不该变"
    );
    let fork_id = doc.layer(&l0).unwrap().palette_id.clone();
    assert_ne!(fork_id, "pico8");
    let fork = doc.palette_by_id(&fork_id).unwrap();
    assert!(!fork.builtin);
    assert_eq!(fork.colors.len(), before.len() + 1);
    assert!(fork.colors.contains(&Rgba::rgb(0x12, 0x34, 0x56)));
}

#[test]
fn removing_a_palette_color_keeps_pixels_intact() {
    let mut doc = blank();
    let l0 = doc.layers[0].id.clone();
    let (f0, _) = (doc.frames[0].id.clone(), ());
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::CreatePalette {
                name: "我的配色".into(),
                from: None,
                colors: vec!["#ff0000".into(), "#00ff00".into(), "#0000ff".into()],
                layer: Some(l0.clone()),
                id: None,
            },
            PixelOperation::SetPixels {
                layer: l0.clone(),
                frame: f0.clone(),
                cells: vec![ops::PixelCell {
                    x: 3,
                    y: 3,
                    color: "#ff0000".into(),
                }],
            },
        ],
    )
    .expect("batch applies");
    let palette_id = doc.layer(&l0).unwrap().palette_id.clone();
    assert_eq!(doc.palette_by_id(&palette_id).unwrap().colors.len(), 3);

    ops::apply_batch(
        &mut doc,
        &[PixelOperation::RemovePaletteColor {
            id: palette_id.clone(),
            index: 1,
            replacement: None,
        }],
    )
    .expect("remove applies");

    // 范围少了一色，画面上的像素原样躺着。
    assert_eq!(doc.palette_by_id(&palette_id).unwrap().colors.len(), 2);
    let painted =
        doc.palette[doc.cel(&l0, &f0).unwrap().indices[(3 * 16 + 3) as usize] as usize - 1];
    assert_eq!(painted, Rgba::rgb(0xff, 0x00, 0x00));
}

#[test]
fn deleting_a_palette_in_use_is_refused() {
    let mut doc = blank();
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::CreatePalette {
            name: "临时的".into(),
            from: None,
            colors: vec!["#ff0000".into()],
            layer: None,
            id: None,
        }],
    )
    .expect("create applies");
    let id = doc.palettes.last().unwrap().id.clone();
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::SetLayerPalette {
            layer: "L0".into(),
            palette_id: id.clone(),
        }],
    )
    .expect("switch applies");

    let err = ops::apply_batch(
        &mut doc,
        &[PixelOperation::DeletePalette {
            id: id.clone(),
            fallback: None,
        }],
    )
    .expect_err("还被图层引用的删不掉");
    assert!(matches!(err, ops::OperationError::PaletteInUse(_, _)));
    assert!(doc.palette_by_id(&id).is_some());
}

/// 正被自家层引用的预设，给了接盘的那一套就删得掉：层先改指过去，预设才消失。
/// 缺了这条，用户一套自建范围都清不掉——当前层必然在引用它。
#[test]
fn deleting_a_palette_in_use_moves_its_layers_to_the_fallback() {
    let mut doc = blank();
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::CreatePalette {
            name: "临时的".into(),
            from: None,
            colors: vec!["#ff0000".into()],
            layer: None,
            id: None,
        }],
    )
    .expect("create applies");
    let id = doc.palettes.last().unwrap().id.clone();
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::SetLayerPalette {
                layer: "L0".into(),
                palette_id: id.clone(),
            },
            PixelOperation::CreateLayer {
                after: Some("L0".into()),
                name: Some("第二层".into()),
                palette_id: Some(id.clone()),
                locked: None,
                id: None,
            },
        ],
    )
    .expect("switch applies");

    ops::apply_batch(
        &mut doc,
        &[PixelOperation::DeletePalette {
            id: id.clone(),
            fallback: Some("sweetie16".into()),
        }],
    )
    .expect("接了盘就删得掉");

    assert!(doc.palette_by_id(&id).is_none(), "预设真的没了");
    for layer in &doc.layers {
        assert_eq!(
            layer.palette_id, "sweetie16",
            "引用过它的层都要落到接盘那套上"
        );
        assert!(doc.palette_by_id(&layer.palette_id).is_some());
    }
}

/// 删色带上接手色：画面用着被删色的像素跟着改写，不会因为索引前移悄悄变色。
#[test]
fn removing_a_palette_color_can_hand_its_pixels_to_a_replacement() {
    let mut doc = blank();
    let l0 = doc.layers[0].id.clone();
    let f0 = doc.frames[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::CreatePalette {
                name: "我的配色".into(),
                from: None,
                colors: vec!["#ff0000".into(), "#00ff00".into(), "#0000ff".into()],
                layer: Some(l0.clone()),
                id: None,
            },
            PixelOperation::SetPixels {
                layer: l0.clone(),
                frame: f0.clone(),
                cells: (0..4)
                    .map(|x| ops::PixelCell {
                        x,
                        y: 3,
                        color: "#ff0000".into(),
                    })
                    .collect(),
            },
        ],
    )
    .expect("batch applies");
    let palette_id = doc.layer(&l0).unwrap().palette_id.clone();

    ops::apply_batch(
        &mut doc,
        &[PixelOperation::RemovePaletteColor {
            id: palette_id.clone(),
            index: 0,
            replacement: Some("#00ff00".into()),
        }],
    )
    .expect("remove applies");

    assert_eq!(doc.palette_by_id(&palette_id).unwrap().colors.len(), 2);
    for x in 0..4 {
        let painted =
            doc.palette[doc.cel(&l0, &f0).unwrap().indices[(3 * 16 + x) as usize] as usize - 1];
        assert_eq!(painted, Rgba::rgb(0x00, 0xff, 0x00), "像素该跟着接手色走");
    }
}

#[test]
fn aip_round_trip_keeps_named_palettes() {
    let mut doc = blank();
    let l0 = doc.layers[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::CreateLayer {
                after: Some(l0.clone()),
                name: Some("第二层".into()),
                id: None,
                palette_id: None,
                locked: None,
            },
            PixelOperation::SetLayerPalette {
                layer: "L1".into(),
                palette_id: "gameboy".into(),
            },
            PixelOperation::SetLayerLocked {
                layer: "L1".into(),
                locked: true,
            },
            PixelOperation::CreatePalette {
                name: "我的配色".into(),
                from: None,
                colors: vec!["#ff0000".into(), "#00ff00".into()],
                layer: Some(l0.clone()),
                id: None,
            },
        ],
    )
    .expect("batch applies");

    let text = aip::dump_v2(&doc).expect("dump");
    assert!(text.contains("@palettes"), "{text}");
    let back = aip::parse_v2(&text).expect("parse");

    assert_eq!(back.palettes.len(), doc.palettes.len());
    assert_eq!(back.layer("L1").unwrap().palette_id, "gameboy");
    assert!(back.layer("L1").unwrap().locked);
    assert_eq!(
        back.layer(&l0).unwrap().palette_id,
        doc.layer(&l0).unwrap().palette_id
    );
    let mine = back.palettes.iter().find(|p| p.name == "我的配色").unwrap();
    assert_eq!(
        mine.colors,
        doc.palette_by_id(&doc.layer(&l0).unwrap().palette_id)
            .unwrap()
            .colors
    );
    assert!(back.palettes.iter().any(|p| p.id == "pico8" && p.builtin));
}

/// 63 个颜色正是从护栏缝里溜过去的那个数：符号表 62 个字符，索引 63
/// 已经没有符号可发。以前护栏写的是 `> 62 + 1`，63 色被放行，紧接着
/// `SYMBOLS[62]` 越界——用户点一次「导出 .aip」，整个进程连着画布一起没。
#[test]
fn exporting_sixty_three_colors_reports_instead_of_panicking() {
    let mut doc = blank();
    for i in 0..=(rle::SYMBOLS.len()) {
        doc.palette.push(Rgba::rgb(i as u8, 255 - i as u8, 128));
    }
    assert_eq!(
        doc.palette.len(),
        rle::SYMBOLS.len() + 1,
        "正好卡在护栏缝上"
    );

    let dumped = aip::dump_v2(&doc);
    assert!(dumped.is_err(), "63 色必须报错，而不是越界 panic");
    let message = dumped.unwrap_err().to_string();
    assert!(message.contains("too large"), "报错要说清原因：{message}");

    // 62 色是上限内的边界，必须照旧导出、照旧读得回来。
    doc.palette.pop();
    let text = aip::dump_v2(&doc).expect("62 色照旧能导出");
    assert!(text.contains("@palette"), "{text}");
    let back = aip::parse_v2(&text).expect("62 色往返");
    assert_eq!(back.palette.len(), doc.palette.len());
}

/// 老 .aip 没有 @palettes 段：读回来必须补上内置库，否则配色面板开天窗。
#[test]
fn old_aip_without_palettes_still_gets_builtin_defaults() {
    let doc = blank();
    let text = aip::dump_v2(&doc).expect("dump");
    // 手工把 @palettes 段剔掉，模拟旧版写出来的文件。
    let mut out = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        if line == "@palettes" {
            skipping = true;
            continue;
        }
        if skipping && !line.starts_with('@') {
            continue;
        }
        skipping = false;
        out.push(line);
    }
    let legacy = out.join("\n");
    assert!(!legacy.contains("@palettes"));

    let back = aip::parse_v2(&legacy).expect("parse");
    assert!(!back.palettes.is_empty(), "内置配色库要补回来");
    for layer in &back.layers {
        assert!(
            back.palette_by_id(&layer.palette_id).is_some(),
            "图层 {} 的范围不能悬空",
            layer.id
        );
    }
}

#[test]
fn shader_circle_near_edge_clamps_instead_of_freezing() {
    // 圆心贴着上边界、半径比画布还大：旧写法把 (2-10) 强转 u32 翻成 40 亿，
    // 双层循环直接卡死，整个会话卡在那里不动。
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    let outcome = shader::run_shader(
        &mut doc,
        &layer,
        r##"
        circfill(2, 2, 40, "#FF004D")
        circle(1, 1, 30, "#FFFFFF")
        rectfill(0, 12, 3, 15, "#00E436")
        "##,
        false,
        &ShaderBudget::default(),
    )
    .expect("越界圆不该把沙箱拖死");
    assert_eq!(outcome.frames_rendered, 1);
    let cel = doc.cel(&layer, &doc.frames[0].id.clone()).unwrap();
    assert!(cel.indices.iter().any(|i| *i != 0), "角上要留下颜色");
}

#[test]
fn shader_oversized_shape_stays_it_inside_canvas() {
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    shader::run_shader(
        &mut doc,
        &layer,
        r##"rectfill(4, 4, 11, 11, "#FF004D")"##,
        false,
        &ShaderBudget::default(),
    )
    .unwrap();
    let cel = doc.cel(&layer, &doc.frames[0].id.clone()).unwrap();
    assert_eq!(cel.indices.len(), 256, "16x16 一块都不能多画");
    assert!(
        cel.indices.iter().take(4).all(|i| *i == 0),
        "矩形外的像素保持透明"
    );
}

#[test]
fn shader_canvas_table_form_draws_too() {
    // 模型很爱写 canvas.pset / canvas.circfill：两种写法都要落地，
    // 不能让一次命名习惯的出入吃掉整个输出预算。
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    shader::run_shader(
        &mut doc,
        &layer,
        r##"
        local ink = '#FF004D'
        canvas.pset(2, 3, ink)
        canvas.circfill(8, 8, 3, ink)
        canvas.rect(0, 13, 5, 15, ink)
        pset(12, 12, ink)
        "##,
        false,
        &ShaderBudget::default(),
    )
    .expect("canvas.* 形式要能画");
    let frame = doc.frames[0].id.clone();
    let cel = doc.cel(&layer, &frame).unwrap();
    assert!(cel.indices.iter().any(|i| *i != 0));
    assert!(cel.get(doc.width, 12, 12).is_some() && cel.get(doc.width, 12, 12).unwrap() != 0);
}

#[test]
fn every_documented_api_name_resolves() {
    // 工具说明里承诺的每个接口都得真的注册。真模型实测里撞过两次，同一类毛病：
    // 文档写 ellipfill，实现只注册了 ellipsefill；文档写 width / height，实现只
    // 注册了 canvas_w / canvas_h。名字对不上在 Rust 这边一声不吭，只在模型那一侧
    // 静静地变成 nil，一个输出预算就这么烧掉了。这里逐个点名验，提示词以后加了
    // 新接口也顺手补进来。
    let mirrored = [
        // 绘图
        "pset",
        "pget",
        "line",
        "rect",
        "rectfill",
        "ellipse",
        "ellipfill",
        "circle",
        "circfill",
        "flood",
        "replace",
        "outline",
        "clear",
        "stamp",
        // 画布尺寸
        "width",
        "height",
    ];
    let bare = ["pal", "hex", "mix", "hsv", "alpha", "rand", "noise"];

    let mut script = String::from("local mirrored = {\n");
    for n in mirrored {
        script.push_str("  '");
        script.push_str(n);
        script.push_str("',\n");
    }
    script.push_str("}\nlocal bare = {\n");
    for n in bare {
        script.push_str("  '");
        script.push_str(n);
        script.push_str("',\n");
    }
    script.push_str(
        "}\nfor _, n in ipairs(mirrored) do\n  \
         if _G[n] == nil then error('missing ' .. n) end\n  \
         if canvas[n] == nil then error('missing canvas.' .. n) end\nend\n  \
         for _, n in ipairs(bare) do\n  \
         if _G[n] == nil then error('missing ' .. n) end\nend\n",
    );
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    shader::run_shader(&mut doc, &layer, &script, false, &ShaderBudget::default())
        .expect("提示词点名的接口必须全部可解析");
}

#[test]
fn ellipse_fill_aliases_draw_identically() {
    // ellipfill / ellipsefill / circlefill / ellipse(..., true) 是同一个填充实现的几个
    // 入口。别名不能只是「存在」，还得画出跟正门一样的东西：空画面对模型来说是
    // 一次静默失败，它没法从错误信息里看出自己换了个写法。
    let mut doc = Document::new("alias", 32, 32).expect("32x32 within limits");
    let layer = doc.layers[0].id.clone();
    let frame = doc.frames[0].id.clone();
    shader::run_shader(
        &mut doc,
        &layer,
        r##"
        local ink = '#FF004D'
        ellipfill(0, 0, 9, 19, ink)
        ellipsefill(11, 0, 20, 19, ink)
        ellipse(22, 0, 31, 19, ink, true)
        circlefill(15, 26, 5, ink)
        "##,
        false,
        &ShaderBudget::default(),
    )
    .expect("填充别名都要能画");
    let cel = doc.cel(&layer, &frame).unwrap();
    let mut bands = [0usize; 3];
    for (i, px) in cel.indices.iter().enumerate() {
        if *px == 0 {
            continue;
        }
        // 只统计三条色带所在的上 20 行，下面的 circlefill 不能混进来。
        if i / 32 > 19 {
            continue;
        }
        // 三条横向色带各占 10 列，中间留 1 列隔开，互不污染。
        let x = i % 32;
        match x {
            0..=9 => bands[0] += 1,
            11..=20 => bands[1] += 1,
            22..=31 => bands[2] += 1,
            _ => {}
        }
    }
    assert_eq!(
        bands[0], bands[1],
        "ellipfill 与 ellipsefill 画出来的不一样"
    );
    assert_eq!(
        bands[1], bands[2],
        "ellipsefill 与 ellipse(..., true) 画出来的不一样"
    );

    let corner = cel
        .indices
        .iter()
        .skip(22 * 32)
        .filter(|px| **px != 0)
        .count();
    assert!(corner > 0, "circlefill 什么都没画");
}

#[test]
fn shader_hand_written_rows_place_every_pixel() {
    // 工具说明里给模型的那份手写行示例必须真能跑：示例就是契约。
    let mut doc = blank();
    let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    shader::run_shader(
        &mut doc,
        &layer,
        r##"
        stamp({
            '.hd.xx..',
            'hhxxxxx.',
            'xxxxxxxx',
            'xxxxxxxx',
            '.xxxxxx.',
            '..xxxx..',
            '...xx...',
            '........',
        }, {h='#FF6B6B', d='#B32D2D', x='#E23B3B'}, 0, 0)
        "##,
        false,
        &ShaderBudget::default(),
    )
    .expect("手写行示例要能跑");
    let cel = doc.cel(&layer, &frame).unwrap();
    // 高光与暗部相邻，主体实心，两侧 '.' 保持透明。
    let h = cel.get(doc.width, 1, 0).unwrap();
    let d = cel.get(doc.width, 2, 0).unwrap();
    let gap = cel.get(doc.width, 0, 0).unwrap();
    let body = cel.get(doc.width, 5, 2).unwrap();
    assert_eq!(gap, 0, "'.' 应该透明，而不是保留原像素");
    assert_ne!(h, 0, "高光点没画上");
    assert_ne!(body, 0, "主体没画上");
    assert_ne!(h, d, "高光和暗部得是两个不同色号");
    // 收成尖角：第 6 行只有两列着色，第 7 行全透明。
    let tip = (0..16)
        .filter(|x| cel.get(doc.width, *x, 6).unwrap() != 0)
        .count();
    assert_eq!(tip, 2, "尖端应该只有 2px，实际 {tip}");
    assert!(
        (0..16).all(|x| cel.get(doc.width, x, 7).unwrap() == 0),
        "全点行应该整行透明"
    );
}

#[test]
fn shader_hand_written_rows_reject_a_row_of_the_wrong_width() {
    // 行宽校验是手写行的安全网：中间漏一列必须报错并点名行号，而不是画歪。
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    let err = shader::run_shader(
        &mut doc,
        &layer,
        r##"
        stamp({'..xx', 'xxx', '..xx'}, {x='#FF004D'}, 0, 0)
        "##,
        false,
        &ShaderBudget::default(),
    )
    .expect_err("行宽不一致必须报错");
    let msg = err.to_string();
    assert!(msg.contains('2'), "错误要点名那一行：{msg}");
}

/// 位图落格也要过配色锁：锁着的层只收范围里的色，范围外的就近归队。
/// shader 和 ops 都过 `color_for_layer`，生图这条不过的话锁就是装饰。
#[test]
fn locked_layer_keeps_a_pixelized_bitmap_inside_its_range() {
    let mut doc = blank();
    let (l0, f0) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::CreateLayer {
                after: Some(l0.clone()),
                name: Some("mono".into()),
                id: None,
                palette_id: None,
                locked: None,
            },
            PixelOperation::SetLayerPalette {
                layer: "L1".into(),
                palette_id: "onebit".into(),
            },
            PixelOperation::SetLayerLocked {
                layer: "L1".into(),
                locked: true,
            },
        ],
    )
    .expect("batch applies");

    // 4x4 纯红位图，contain 铺满 16x16 画布。
    let mut rgba = Vec::with_capacity(4 * 4 * 4);
    for _ in 0..(4 * 4) {
        rgba.extend_from_slice(&[0xFF, 0x00, 0x00, 0xFF]);
    }
    let before = doc.palette.len();
    let report = pixel_core::pixelize::pixelize_into_cel(
        &mut doc,
        "L1",
        &f0,
        &rgba,
        4,
        4,
        &pixel_core::PixelizeOptions::default(),
    )
    .expect("pixelize lands");

    // 落在格子里的每一个色都得是 1-bit 范围里那两个之一；红归队到黑。
    let cel = doc.cel("L1", &f0).unwrap();
    let landed: std::collections::BTreeSet<u16> = cel.indices.iter().copied().collect();
    assert_eq!(landed.len(), 1, "纯红图归队后只剩一色，实际 {landed:?}");
    let color = doc.color_of(*landed.iter().next().unwrap()).unwrap();
    assert_eq!(
        (color.r, color.g, color.b),
        (0, 0, 0),
        "红色最近的 1-bit 色是黑"
    );
    assert!(
        !doc.palette.iter().any(|c| (c.r, c.g, c.b) == (255, 0, 0)),
        "锁着的层不许把范围外的颜色灌进文档调色板"
    );
    assert_eq!(
        report.palette_added + before,
        doc.palette.len(),
        "报告里的新增色数要和文档实际一致"
    );

    // 没锁的那层照旧可以扩色：同一张图落在 L0 上必须长出新颜色。
    let open_before = doc.palette.len();
    pixel_core::pixelize::pixelize_into_cel(
        &mut doc,
        "L0",
        &f0,
        &rgba,
        4,
        4,
        &pixel_core::PixelizeOptions::default(),
    )
    .expect("pixelize lands on the open layer");
    assert!(
        doc.palette.len() > open_before,
        "没锁的层应当能扩色（{} -> {}）",
        open_before,
        doc.palette.len()
    );
}

#[test]
fn aip_round_trip_keeps_colors_when_the_first_painted_color_is_not_palette_zero() {
    // 回归：@cel 段曾经按「索引在画面里首次出现的顺序」发符号，而 @palette 段
    // 按调色板位置发。两者一旦不一致，导出再导入就把颜色整体换掉，且写读写写
    // 字节稳定、没有任何报错。这里故意让 palette[0] 完全不登场，逼两套表分叉。
    let mut doc = blank();
    let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    // 偏偏只画最后一色（索引 4）：调色板越靠后、登场越晚，两套符号表分叉得
    // 越厉害。旧的按序发符号会把索引 4 发成 'a'，而 @palette 段里 'a' 是
    // palette[0]，于是整幅画换色。
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec![
                    "#111111".into(),
                    "#222222".into(),
                    "#778899".into(),
                    "#112233".into(),
                ],
            },
            PixelOperation::SetPixels {
                layer: layer.clone(),
                frame: frame.clone(),
                cells: vec![ops::PixelCell {
                    x: 0,
                    y: 0,
                    color: "#112233".into(),
                }],
            },
        ],
    )
    .unwrap();
    let original = doc
        .color_of(
            doc.cel(&layer, &frame)
                .unwrap()
                .get(doc.width, 0, 0)
                .unwrap(),
        )
        .unwrap();

    let text = context::to_aip(&doc).expect("export");
    let back = aip::import_any(&text).expect("import");

    // 调色板顺序和内容必须原样回来。
    assert_eq!(
        back.palette, doc.palette,
        "往返之后调色板内容变了，说明 @palette 段和 @cel 段用的不是同一套符号表"
    );
    let idx = back
        .cel(&back.layers[0].id, &back.frames[0].id)
        .unwrap()
        .get(back.width, 0, 0)
        .expect("那一格还在");
    assert_eq!(
        back.color_of(idx),
        Some(original),
        "同一个 cel 索引读回的颜色必须是导出的那个"
    );
}

#[test]
fn encode_row_never_merges_two_identical_colors_into_one_symbol() {
    // 调色板里出现重复颜色时，「先按颜色找到的符号」会把两种颜色都写成同一个
    // 符号，读回来整片串成第一种。
    let palette = vec![Rgba::rgb(10, 20, 30), Rgba::rgb(10, 20, 30)];
    let legend = rle::Legend::build(&palette, &[1, 2]);
    let row = rle::encode_row(&[1, 2], &legend);
    assert_eq!(row, "ab", "重复颜色也必须各占各的符号：{row}");
}

#[test]
fn legend_symbols_match_the_row_encoding() {
    // 图例和行编码必须走同一套「索引 -> 符号」。图例里全是 'a' 的话，
    // 模型读到的颜色跟画面整体错位，而写读写写字节完全稳定，看不出报错。
    let palette = vec![
        Rgba::parse_hex("#FF004D").unwrap(),
        Rgba::parse_hex("#00E436").unwrap(),
        Rgba::parse_hex("#29ADFF").unwrap(),
    ];
    let legend = rle::Legend::build(&palette, &[1, 2, 3]);
    let lines = legend.to_lines();
    assert_eq!(
        lines,
        vec![
            ". = transparent".to_string(),
            "a = #ff004d".to_string(),
            "b = #00e436".to_string(),
            "c = #29adff".to_string(),
        ],
        "图例符号必须跟行编码同源：{lines:?}"
    );
    // 反方向：行里出现的符号，图例里一定要有一行对应的颜色。
    let row = rle::encode_row(&[0, 1, 2, 3, 3, 2], &legend);
    // [0,1,2,3,3,2] -> . a b 2c b（3 和 3 并成一次重复）
    assert_eq!(row, ".ab2cb", "{row}");
    for ch in row.chars().filter(|c| c.is_ascii_alphabetic()) {
        assert!(
            lines.iter().any(|l| l.starts_with(ch)),
            "行里的符号 {ch} 在图例里没有对应颜色"
        );
    }
}

/// cel 比调色板长时（索引从 .aip 或者别的进程灌进来），行里会写 `?`。
/// 图例必须把它讲成「这个位置没有颜色」，而不是跟透明的 `None` 混作一谈：
/// 那等于告诉模型这片是空白，模型顺手就把它擦了。
#[test]
fn an_index_outside_the_palette_is_never_reported_as_transparent() {
    let palette = vec![Rgba::parse_hex("#FF004D").unwrap()];
    // 索引 3 超出了调色板长度 1，可 cel 里真有这一格。
    let legend = rle::Legend::build(&palette, &[1, 3]);

    assert_eq!(legend.dangling.len(), 1, "悬空索引必须登记在册");
    assert_eq!(
        legend.dangling[0],
        rle::Unmapped::OutsidePalette(3),
        "登记的是索引本身：这一格的颜色压根没定义"
    );
    let lines = legend.to_lines();
    assert_eq!(lines[0], ". = transparent", "透明那一行不许被挤掉");
    assert_eq!(lines[1], "a = #ff004d", "有颜色的那一行也不许被挤掉");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("? = unmapped") && l.contains('3')),
        "图例里必须有一行讲清 ? 是什么：{lines:?}"
    );

    // 行编码和图例同源：同一个索引 3 在行里也是 ?，不会是图例里的 c。
    assert_eq!(rle::encode_row(&[1, 3], &legend), "a?");
    for ch in "a?".chars() {
        assert!(
            !ch.is_ascii_alphabetic() || lines.iter().any(|l| l.starts_with(ch)),
            "行里的符号 {ch} 在图例里没有对应颜色"
        );
    }
}

/// 第 63 个颜色往后没有单字符符号可发。以前图例会照样写一行 `? = #xxxxxx`，
/// 于是好几种颜色共用同一个符号：模型照着 `?` 写一笔，整片像素悄悄收敛成
/// 一种色，写读写写的字节还全都稳定，没人看得出来。
#[test]
fn colors_without_a_symbol_are_named_by_hex_not_by_the_question_mark() {
    // 66 个颜色：前 62 个拿得到符号，后 4 个只能共用 `?`。
    let palette: Vec<Rgba> = (0..66u16)
        .map(|i| Rgba::rgb((i * 3) as u8, (i * 5) as u8, (i * 7) as u8))
        .collect();
    let used: Vec<u16> = (1..=66).collect();
    let legend = rle::Legend::build(&palette, &used);

    // 拿得到符号的那 62 个照旧逐色一行，一个都不许被 ? 顶掉。
    assert_eq!(legend.entries.len(), 1 + 62, "透明加 62 个有符号的颜色");
    assert!(
        legend.entries.iter().all(|(sym, _)| *sym != '?'),
        "图例里不该出现 ? 冒充某一种颜色"
    );
    // 剩下 4 个进悬空清单，而且带着各自的 hex。
    assert_eq!(legend.dangling.len(), 4);
    for (slot, index) in legend.dangling.iter().zip(63..=66u16) {
        match slot {
            rle::Unmapped::SymbolsExhausted { index: idx, color } => {
                assert_eq!(*idx, index);
                assert_eq!(color.to_hex(), palette[index as usize - 1].to_hex());
            }
            other => panic!("索引 {index} 该归到符号用尽，却是 {other:?}"),
        }
    }

    // 行里这些格子写 ?，可图例必须告诉模型 ? 不是颜色，并给出 hex。
    let row = rle::encode_row(&[63, 63, 64], &legend);
    assert_eq!(row, "2??", "每个没符号的索引都写成 ?：{row}");
    let text = legend.to_lines().join("\n");
    let third_color = palette[62].to_hex();
    assert!(
        text.contains("symbol limit") && text.contains(&third_color),
        "图例必须点明符号用尽并给出真实 hex：{text}"
    );
    assert!(
        !text.lines().any(|l| l.starts_with("? = #")),
        "? 不许再冒充某一种颜色：{text}"
    );
}
#[test]
fn truncated_quotes_degrade_instead_of_killing_the_process() {
    // 手写的 .aip 少了收尾引号时，解析器过去会在多字节内容上切出越界切片，
    // 整个进程带着 panic 消失，用户一次误触就丢掉整张画布。
    let broken = [
        "AIP 2",
        "@meta name=\"未闭合",
        "@palette",
        ". transparent",
        "a #111111",
        "@layers",
        "L0 \"第二层",
        "@frames",
        "F0 100",
        "@cel L0 F0 2x1 rle",
        "aa",
    ]
    .join("\n");

    let doc = aip::parse_v2(&broken).expect("缺引号的文件应当降级解析而不是 panic");
    // 引号后面的 key=value 被整段吞进名字，但图层本身和像素都还在。
    assert_eq!(doc.layers.len(), 1, "图层没解析出来：{:?}", doc.layers);
    assert!(doc.layers[0].name.contains("第二层"), "{:?}", doc.layers[0]);
    assert_eq!(doc.width, 2);
    let idx = doc.cel("L0", "F0").expect("cel 还在").get(2, 0, 0);
    assert_eq!(idx, Some(1), "颜色索引没读回来");
}

#[test]
fn cel_header_with_astronomical_dims_is_refused_before_it_allocates() {
    // cel 头的宽高是 Vec 分配的直接参数。过去这里不设防，写个 100000x100000
    // 就要在报错之前先把几十 GB 的索引数组开出来，进程直接被 OOM 打死。
    let mut huge = vec![
        "AIP 2".to_string(),
        "@palette".into(),
        ". transparent".into(),
        "a #111111".into(),
        "@layers".into(),
        "L0 \"only\"".into(),
        "@frames".into(),
        "F0 100".into(),
        "@cel L0 F0 100000x100000 rle".into(),
    ];
    // 真给满行数的话测试自己就得跑一年，所以只写一行：位置在 cel 头，不该走到分配。
    huge.push("a".repeat(100000));
    let outcome = aip::parse_v2(&huge.join("\n"));
    let message = outcome
        .expect_err("天文数字的 cel 头必须被拒绝")
        .to_string();
    assert!(message.contains("cel"), "{message}");
}

#[test]
fn duplicate_layer_ids_are_refused_instead_of_sharing_one_cel() {
    // 图层 id 重名时 cels 的 BTreeMap 会把两层合成一格：改一层、另一层跟着变，
    // 前端侧栏还会撞出重复的 React key。
    let broken = [
        "AIP 2",
        "@palette",
        ". transparent",
        "a #111111",
        "@layers",
        "L0 \"第一层\"",
        "L0 \"第二层\"",
        "@frames",
        "F0 100",
        "@cel L0 F0 2x1 rle",
        "aa",
    ]
    .join("\n");

    let message = aip::parse_v2(&broken)
        .expect_err("重名图层 id 必须拒绝")
        .to_string();
    assert!(message.contains("L0"), "{message}");
}

#[test]
fn cel_header_that_grows_an_existing_cel_is_refused() {
    // 同一个 cel 写两遍、第二遍尺寸更大：Cel::new 已按旧尺寸分配过，
    // 照旧写下去 copy_from_slice 会切出越界片，进程带着 panic 消失。
    let broken = [
        "AIP 2",
        "@palette",
        ". transparent",
        "a #111111",
        "@layers",
        "L0 \"only\"",
        "@frames",
        "F0 100",
        "@cel L0 F0 2x1 rle",
        "aa",
        "@cel L0 F0 4x2 rle",
        "aaaa",
        "aaaa",
    ]
    .join("\n");

    let message = aip::parse_v2(&broken)
        .expect_err("同一 cel 换尺寸必须拒绝")
        .to_string();
    assert!(message.contains("inconsistent"), "{message}");
}

#[test]
fn named_palette_line_without_a_closing_quote_reports_a_plain_error() {
    // 配色范围段缺引号时名字会吃掉后面的颜色表，剩下空 tokens。
    // 这种文件只能拒绝，但不能 panic：调用方要拿到的是 AipError。
    let broken = "p0 \"我的配色 custom #ff0000";
    let outcome = std::panic::catch_unwind(|| {
        aip::parse_v2(&format!(
            "AIP 2\n@palettes\n{broken}\n@palette\n. transparent\na #111111\n"
        ))
        .map(|doc| doc.palettes.len())
    });
    let parsed = outcome.expect("解析残缺引号不该 panic");
    assert!(
        parsed.is_err() || parsed.as_ref().unwrap().eq(&0),
        "{parsed:?}"
    );
}

#[test]
fn oversized_run_count_is_rejected_before_it_allocates() {
    // 一行 2x1 的 cel 写成 `100000000a`，解码器过去先按计数铺索引，
    // 校验行宽之前就把几百兆内存申请出去。现在必须先被行宽拦住。
    let text = [
        "AIP 2",
        "@meta name=\"x",
        "@palette",
        ". transparent",
        "a #111111",
        "@layers",
        "L0 \"Layer 1\" visible 255 palette=default unlocked",
        "@frames",
        "F0 100",
        "@cel L0 F0 2x1 rle",
        "100000000a",
    ]
    .join("\n");

    let parsed = aip::parse_v2(&text);
    assert!(parsed.is_err(), "超宽的 run 必须在分配内存之前被拒绝");
}

#[test]
fn batch_that_breaks_the_layer_limit_leaves_the_document_alone() {
    // 上限检查过去在循环外面，`?` 直接带着半个文档返回：129 次 CreateLayer
    // 走到第 129 个才失败，可文档里已经实实在在多了 128 层。
    let mut doc = blank();
    let ops: Vec<PixelOperation> = (0..200)
        .map(|_| PixelOperation::CreateLayer {
            after: None,
            name: None,
            id: None,
            palette_id: None,
            locked: None,
        })
        .collect();

    let outcome = ops::apply_batch(&mut doc, &ops);
    assert!(outcome.is_err(), "超过图层上限必须整批失败");
    assert_eq!(doc.layers.len(), 1, "失败之后文档不应该留下任何半成品图层");
}

#[test]
fn create_with_an_id_that_already_exists_is_refused() {
    // 显式 id 撞名会留下两条同 id 的帧/层，cel 按 id 索引时互相冲掉。
    let mut doc = blank();
    let existing = doc.layers[0].id.clone();

    assert!(
        ops::apply_one(
            &mut doc,
            &PixelOperation::CreateLayer {
                after: None,
                name: Some("重名层".into()),
                id: Some(existing.clone()),
                palette_id: None,
                locked: None,
            }
        )
        .is_err(),
        "同 id 的图层必须被拒绝"
    );
    assert!(
        ops::apply_one(
            &mut doc,
            &PixelOperation::CreateFrame {
                after: None,
                duration_ms: 100,
                id: Some("F0".into()),
            }
        )
        .is_err(),
        "同 id 的帧必须被拒绝"
    );
    assert_eq!(doc.layers.len(), 1);
    assert_eq!(doc.frames.len(), 1);
}

#[test]
fn emptying_the_palette_is_refused_instead_of_wiping_the_canvas() {
    // 空配色表让就近映射全程落空，索引全归 0，整幅画静默擦成透明。
    let mut doc = blank();
    let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec!["#ff0000".into()],
            },
            PixelOperation::SetPixels {
                layer: layer.clone(),
                frame: frame.clone(),
                cells: vec![ops::PixelCell {
                    x: 1,
                    y: 1,
                    color: "#ff0000".into(),
                }],
            },
        ],
    )
    .expect("setup");

    let outcome = ops::apply_one(&mut doc, &PixelOperation::SetPalette { colors: vec![] });
    assert!(outcome.is_err(), "清空调色板必须报错");
    assert_eq!(
        doc.cel(&layer, &frame).unwrap().get(doc.width, 1, 1),
        Some(1),
        "画上去的像素不该被抹掉"
    );
}

#[test]
fn cel_set_refuses_x_past_the_row_width() {
    // 只查扁平下标的话，x 超过 width 的点会被写进下一行，右边溢出悄悄串到下面。
    let mut doc = blank();
    let (w, h) = (doc.width, doc.height);
    let cel = doc.cel_mut("L0", "F0").unwrap();
    assert!(!cel.set(w, w, 0, 1), "越列写入必须失败");
    assert!(!cel.set(w, 0, h, 1), "越行写入必须失败");
    assert!(cel.set(w, 0, 0, 1), "合法格子照样能写");
    assert_eq!(cel.get(w, 0, 0), Some(1));
}

#[test]
fn a_failed_animate_run_restores_the_frames_it_already_cleared() {
    // 逐帧执行先清 cel 再跑脚本，报错的那一帧之前清空的格子必须还原，
    // 否则画面凭空少半截，而 revision 也没动，前端还以为「什么都没发生」。
    let mut doc = blank();
    let layer = doc.layers[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec!["#29ADFF".into()],
            },
            PixelOperation::CreateFrame {
                after: None,
                duration_ms: 80,
                id: None,
            },
        ],
    )
    .unwrap();
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::SetPixels {
            layer: layer.clone(),
            frame: "F0".into(),
            cells: vec![ops::PixelCell {
                x: 3,
                y: 3,
                color: "#29ADFF".into(),
            }],
        }],
    )
    .unwrap();

    let outcome = shader::run_shader(
        &mut doc,
        &layer,
        // 第一帧画得动，第二帧踩一脚内置名让脚本炸掉。
        "if frame_index == 0 then pset(1, 1, pal(1)) else local line = 1 line() end",
        true,
        &ShaderBudget::default(),
    );
    assert!(outcome.is_err(), "脚本报错必须向上抛");

    let cel = doc.cel(&layer, "F0").expect("cel 还在");
    assert_eq!(
        cel.get(doc.width, 3, 3),
        Some(1),
        "失败之后原先画好的像素必须还在"
    );
    assert_eq!(
        cel.get(doc.width, 1, 1),
        Some(0),
        "失败那一帧临时画的点不该留着"
    );
}

// ---- 平滑绘制（抗锯齿）经真 Lua 跑一遍 ----

fn aa_doc(size: u32) -> (Document, String, String) {
    let doc = Document::new("aa", size, size).expect("square within limits");
    let (layer, frame) = (doc.layers[0].id.clone(), doc.frames[0].id.clone());
    (doc, layer, frame)
}

#[test]
fn aa_lua_softens_a_diagonal_with_intermediate_colors() {
    let (mut doc, layer, _frame) = aa_doc(24);
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec!["#000000".into(), "#FFFFFF".into()],
            },
            // 先糊一层黑底：斜线要掺的正是黑与白之间的灰。
            PixelOperation::SetPixels {
                layer: layer.clone(),
                frame: "F0".into(),
                cells: (0..24)
                    .flat_map(|y| {
                        (0..24).map(move |x| ops::PixelCell {
                            x,
                            y,
                            color: "#000000".into(),
                        })
                    })
                    .collect(),
            },
        ],
    )
    .unwrap();
    let before = doc.palette.len();
    let outcome = shader::run_shader(
        &mut doc,
        &layer,
        "aaline(2, 3, 21, 20, '#FFFFFF')",
        false,
        &ShaderBudget::default(),
    )
    .expect("aaline runs");
    assert!(outcome.opaque_pixels > 0, "柔线要画出东西来");
    // 掺出来的灰必须真的进了调色板：像素画的色阶就是这么长出来的。
    assert!(
        doc.palette.len() > before,
        "半覆盖格该掺出新色阶：{} -> {}",
        before,
        doc.palette.len()
    );
    // 45° 附近的斜线，两行交替出现才叫柔；纯 Bresenham 是同一行串成一条。
    let rows: Vec<u32> = doc
        .cel(&layer, "F0")
        .unwrap()
        .indices
        .iter()
        .enumerate()
        .filter(|(_, i)| **i != 0)
        .map(|(i, _)| (i as u32) / doc.width)
        .collect();
    assert!(rows.windows(2).any(|w| w[0] != w[1]), "斜线至少要穿过两行");
}

#[test]
fn aa_lua_shapes_and_point_tables_all_paint() {
    let (mut doc, layer, _frame) = aa_doc(32);
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::AddPaletteColors {
            colors: vec!["#29ADFF".into()],
        }],
    )
    .unwrap();
    let script = r##"
      aacurve(2, 28, 30, 28, 16, 2, '#FF004D')
      aacubic(2, 2, 30, 2, 10, 12, 22, 20, '#FF004D')
      aapoly({{x=4,y=4},{x=14,y=4},{x=9,y=14}}, '#00E436')
      aapolyfill({{4,20},{14,20},{9,29}}, '#00E436')
      aacircle(24, 10, 4, '#FFEC27', true)
      aaellipse(18, 14, 28, 24, '#FFEC27')
      aarect(2, 16, 7, 23, '#FF004D', true)
      blend(31, 31, '#FFFFFF', 0.5)
      dither(15, 15, '#FFFFFF', 0.5)
    "##;
    let outcome = shader::run_shader(&mut doc, &layer, script, false, &ShaderBudget::default())
        .expect("aa shapes run");
    assert!(outcome.opaque_pixels > 20, "每个形状都该留下痕迹");

    let cel = doc.cel(&layer, "F0").unwrap();
    let at = |x: u32, y: u32| cel.get(doc.width, x, y).unwrap_or(0);
    // 圆心那一格必须是实心圆留下的一笔，填充描边两不落空。
    assert!(at(24, 10) != 0, "aacircle 实心圆要在圆心落色");
    // aarect 的角上那格也必须在：填充矩形最老实，它没了说明整条链断了。
    assert!(at(4, 19) != 0, "aarect 填充要在矩形里落色");
    // dither 只按阈值落色，50% 覆盖率下一半的格会亮——这里不断言具体哪一格，
    // 只确认整幅图确实被这一串形状动过。
    assert!(cel.indices.iter().any(|i| *i != 0), "整幅图要留下痕迹");
}

#[test]
fn aa_lua_point_tables_accept_every_writing_style() {
    let (mut doc, layer, _frame) = aa_doc(32);
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::AddPaletteColors {
            colors: vec!["#29ADFF".into()],
        }],
    )
    .unwrap();
    let script = r##"
      aapoly({{x=2,y=30},{x=16,y=2},{x=30,y=30}}, '#FF004D')
      aapoly({{2,16},{30,16}}, '#FF004D')
      aapoly({8,4,24,20}, '#FF004D')
      aapolyfill({{10,26},{14,26},{12,30}}, '#FF004D')
    "##;
    let outcome = shader::run_shader(&mut doc, &layer, script, false, &ShaderBudget::default())
        .expect("every point table shape parses");
    assert!(outcome.opaque_pixels > 5);
}

#[test]
fn aa_lua_respects_the_palette_lock() {
    let (mut doc, layer, _frame) = aa_doc(24);
    let palette_id = doc.palettes[0].id.clone();
    ops::apply_batch(
        &mut doc,
        &[
            PixelOperation::AddPaletteColors {
                colors: vec!["#000000".into(), "#FFFFFF".into()],
            },
            PixelOperation::SetLayerPalette {
                layer: layer.clone(),
                palette_id: palette_id.clone(),
            },
            PixelOperation::SetLayerLocked {
                layer: layer.clone(),
                locked: true,
            },
        ],
    )
    .unwrap();
    shader::run_shader(
        &mut doc,
        &layer,
        "aaline(1, 1, 22, 22, '#00FF00')",
        false,
        &ShaderBudget::default(),
    )
    .expect("shrunk to the locked range");
    // 上着锁的层只肯落范围里的颜色：掺出来的灰就近归队，不该出现范围之外的颜色
    // ——那正是上锁要挡的事。落进来的只可能是范围里那几个色，天然有上界。
    let range = doc
        .palette_by_id(&palette_id)
        .expect("palette still there")
        .colors
        .clone();
    let mut seen: Vec<Rgba> = Vec::new();
    for index in doc
        .cel(&layer, "F0")
        .expect("cel")
        .indices
        .iter()
        .filter(|i| **i != 0)
    {
        let color = doc.color_of(*index).expect("index in range");
        assert!(
            range.contains(&color),
            "锁着的层落出了范围之外的色 {}",
            color.to_hex()
        );
        if !seen.contains(&color) {
            seen.push(color);
        }
    }
    assert!(!seen.is_empty(), "这条线至少要画上几格");
}

#[test]
fn aa_lua_clips_geometry_that_flies_off_the_canvas() {
    let (mut doc, layer, _frame) = aa_doc(16);
    ops::apply_batch(
        &mut doc,
        &[PixelOperation::AddPaletteColors {
            colors: vec!["#29ADFF".into()],
        }],
    )
    .unwrap();
    let script = r##"
      aacircle(400, 400, 90, '#FF004D', true)
      aaellipse(-300, -300, -100, -100, '#FF004D')
      aarect(900, 900, 1000, 1000, '#FF004D')
      aacurve(500, 500, 900, 900, 700, 700, '#FF004D')
      aaline(-50, -50, -10, -10, '#FF004D')
    "##;
    // 半径写飞、坐标写飞都不许把沙箱拖死，也不许往画布外写色。
    shader::run_shader(&mut doc, &layer, script, false, &ShaderBudget::default())
        .expect("canvas-external geometry is clipped, not fatal");
    let cel = doc.cel(&layer, "F0").unwrap();
    assert!(
        cel.indices.iter().all(|i| *i == 0),
        "全在画布外的几何一格都不该落"
    );
}

#[test]
fn aa_lua_bad_arguments_report_the_usual_cause() {
    let (mut doc, layer, _frame) = aa_doc(16);
    let err = shader::run_shader(
        &mut doc,
        &layer,
        "aapoly({{x=2,y=2}, 9}, '#FF004D')",
        false,
        &ShaderBudget::default(),
    );
    let text = err.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(text.contains("point"), "点表写错要把话说清楚： {text}");
}
