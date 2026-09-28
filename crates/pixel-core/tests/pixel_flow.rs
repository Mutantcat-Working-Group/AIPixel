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
