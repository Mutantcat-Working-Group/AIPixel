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
        },
        PixelOperation::DeletePalette { id: "pico8".into() },
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
        &[PixelOperation::DeletePalette { id: id.clone() }],
    )
    .expect_err("还被图层引用的删不掉");
    assert!(matches!(err, ops::OperationError::PaletteInUse(_, _)));
    assert!(doc.palette_by_id(&id).is_some());
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
