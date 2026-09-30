// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 真模型端到端自检。默认跳过，只有把 Provider 变量配齐才会真的发出请求。
//!
//! ```text
//! AIPIXEL_LIVE_KEY=... \
//! AIPIXEL_LIVE_BASE_URL=https://api.example.com/v1 \
//! AIPIXEL_LIVE_MODEL=example-model \
//! cargo test -p agent-core --test live_model -- --ignored --nocapture
//! ```
//!
//! 可选：`AIPIXEL_LIVE_PROTOCOL=anthropic`、`AIPIXEL_LIVE_VISION=1`、
//! `AIPIXEL_LIVE_REQ`（用户提示词）、`AIPIXEL_LIVE_W` / `_H`、`AIPIXEL_LIVE_MAXTOK`。
//! key 只从环境变量进来，不要写进任何文件，这个仓库是对外开源的。
//!
//! 跑完把 `/tmp/aipdump/final-<W>x<H>.png` 拿出来对着看，是判断提示词改动有没
//! 有生效最直接的办法。

use agent_core::models::{AgentEvent, Capabilities, ModelConfig, Protocol};
use agent_core::runner::AgentSession;
use pixel_core::document::Document;

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

#[tokio::test]
#[ignore = "真的会向第三方发请求，按文件头注释配齐变量再 --ignored 跑"]
async fn live_turn() {
    let (Some(key), Some(base_url), Some(model)) = (
        env("AIPIXEL_LIVE_KEY"),
        env("AIPIXEL_LIVE_BASE_URL"),
        env("AIPIXEL_LIVE_MODEL"),
    ) else {
        eprintln!("skip: 配齐 AIPIXEL_LIVE_KEY / _BASE_URL / _MODEL 才会真的发请求");
        return;
    };
    let cfg = ModelConfig {
        id: "live".into(),
        label: "live".into(),
        protocol: match env("AIPIXEL_LIVE_PROTOCOL").as_deref() {
            Some("anthropic") => Protocol::Anthropic,
            _ => Protocol::OpenAiCompat,
        },
        base_url,
        api_key: key,
        model,
        max_tokens: env("AIPIXEL_LIVE_MAXTOK").and_then(|v| v.parse().ok()),
        temperature: None,
        disable_thinking: Some(true),
        capabilities: Capabilities {
            vision: env("AIPIXEL_LIVE_VISION").is_some(),
            ..Default::default()
        },
    };
    let w: u32 = env("AIPIXEL_LIVE_W")
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    let h: u32 = env("AIPIXEL_LIVE_H")
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    let doc = Document::new("untitled", w, h).unwrap();
    let s = AgentSession::new("live-1", cfg, doc);
    let text =
        env("AIPIXEL_LIVE_REQ").unwrap_or_else(|| "帮我画一个16x16的红苹果像素图标".to_string());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    s.run_turn(text, Vec::new(), tx).await;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AgentEvent::ToolCall { name, input, .. } => {
                let text = match &input {
                    serde_json::Value::String(t) => t.clone(),
                    v => v.to_string(),
                };
                println!("== TOOL {name} ==\n{text}");
            }
            AgentEvent::ToolResult {
                name,
                summary,
                is_error,
                ..
            } => {
                println!("== RESULT {name} err={is_error} ==\n{}", summary);
            }
            AgentEvent::Token { text } => print!("{text}"),
            AgentEvent::Reasoning { text } => print!("[R]{text}"),
            AgentEvent::Error { message } => println!("== ERROR == {:?}", message),
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
            } => println!("== USAGE == in={input_tokens:?} out={output_tokens:?}"),
            AgentEvent::Completed { turns } => println!("== COMPLETED turns={turns} =="),
            _ => {}
        }
    }
    let final_doc = s.document();
    let nonempty = final_doc.layers.iter().filter(|l| {
        final_doc
            .frames
            .iter()
            .any(|f| final_doc.cel(&l.id, &f.id).is_some())
    });
    let mut painted = 0usize;
    for l in final_doc.layers.iter() {
        for f in final_doc.frames.iter() {
            if let Some(cel) = final_doc.cel(&l.id, &f.id) {
                painted += cel.indices.iter().filter(|i| **i != 0).count();
            }
        }
    }
    println!(
        "layers={} live_layers={} frames={} painted_px={}",
        final_doc.layers.len(),
        nonempty.count(),
        final_doc.frames.len(),
        painted
    );
    if let Ok(png) = pixel_core::png::document_to_png(&final_doc) {
        std::fs::create_dir_all("/tmp/aipdump").unwrap();
        std::fs::write(format!("/tmp/aipdump/final-{w}x{h}.png"), &png).unwrap();
        println!(
            "png saved /tmp/aipdump/final-{w}x{h}.png ({} bytes)",
            png.len()
        );
    }
}
