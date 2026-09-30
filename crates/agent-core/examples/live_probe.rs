//! 一次性联调探针：拿用户在设置里配的那套模型真跑一轮，看 agent 主循环
//! 到底调没调工具、画没画出东西。凭据只从环境变量读，不落任何文件。
//!
//! 跑法（凭据只从环境变量进，别写进任何文件）：
//!   cargo run -p agent-core --example live_probe
//! 可调环境变量：
//!   AIPIXEL_BASE_URL / AIPIXEL_API_KEY / AIPIXEL_MODEL / AIPIXEL_PROMPT
//!   AIPIXEL_W / AIPIXEL_H / AIPIXEL_MAX_TOKENS / AIPIXEL_DISABLE_THINKING
//!
//! 逐条工具调用落在 $TMPDIR/aipixel-probe-trace.jsonl；退出码非 0 表示这一轮没画出东西。

use std::collections::BTreeSet;
use std::sync::Arc;

use agent_core::AgentEvent;
use pixel_core::document::Document;
use tokio::sync::mpsc;

fn env(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

#[tokio::main]
async fn main() {
    let base_url = env("AIPIXEL_BASE_URL", "");
    let api_key = env("AIPIXEL_API_KEY", "");
    let model = env("AIPIXEL_MODEL", "deepseek-v4.1-flash");
    let prompt = env("AIPIXEL_PROMPT", "画一个5帧的闭环的橘猫行走图");
    let width: u32 = env("AIPIXEL_W", "64").parse().unwrap_or(64);
    let height: u32 = env("AIPIXEL_H", "64").parse().unwrap_or(64);
    let max_tokens: Option<u32> = std::env::var("AIPIXEL_MAX_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok());
    let disable_thinking = std::env::var("AIPIXEL_DISABLE_THINKING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(false);
    if base_url.is_empty() || api_key.is_empty() {
        eprintln!("需要 AIPIXEL_BASE_URL 与 AIPIXEL_API_KEY");
        std::process::exit(2);
    }
    eprintln!("== probe: model={model} max_tokens={max_tokens:?} canvas={width}x{height}");

    let config = agent_core::ModelConfig {
        id: "probe".into(),
        label: "probe".into(),
        protocol: agent_core::Protocol::OpenAiCompat,
        base_url,
        api_key,
        model,
        max_tokens,
        temperature: None,
        disable_thinking: if disable_thinking { Some(true) } else { None },
        capabilities: Default::default(),
    };
    let doc = Document::new("probe", width, height).expect("document");
    let session = Arc::new(agent_core::AgentSession::new("probe-1", config, doc));

    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let started = std::time::Instant::now();
    let trace_path = std::env::temp_dir().join("aipixel-probe-trace.jsonl");
    let trace_file = std::fs::File::create(&trace_path).ok();
    let trace = std::sync::Mutex::new(trace_file);
    let record = |line: String| {
        use std::io::Write;
        if let Some(f) = trace.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{line}");
        }
    };
    let runner = Arc::clone(&session);
    let handle = tokio::spawn(async move { runner.run_turn(prompt, Vec::new(), tx).await });

    let mut tools: Vec<String> = Vec::new();
    let mut text_chars = 0usize;
    let mut reasoning_chars = 0usize;
    while let Some(ev) = rx.recv().await {
        match ev {
            AgentEvent::Status { message } => eprintln!("[status] {message:?}"),
            AgentEvent::Token { text } => text_chars += text.len(),
            AgentEvent::Reasoning { text } => reasoning_chars += text.len(),
            AgentEvent::ToolCall { name, input, .. } => {
                let brief: String = serde_json::to_string(&input)
                    .unwrap_or_default()
                    .chars()
                    .take(160)
                    .collect();
                eprintln!("[tool] {name} {brief}");
                record(format!(
                    "{{\"tool\":{:?},\"input\":{}}}",
                    name,
                    serde_json::to_string(&input).unwrap_or_default()
                ));
                tools.push(name);
            }
            AgentEvent::ToolResult {
                name,
                summary,
                is_error,
                ..
            } => eprintln!(
                "[result] {name} {} {summary}",
                if is_error { "ERR" } else { "ok" }
            ),
            AgentEvent::DocumentUpdated { revision, .. } => eprintln!("[doc] revision={revision}"),
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
            } => eprintln!("[usage] in={input_tokens:?} out={output_tokens:?}"),
            AgentEvent::Completed { turns } => eprintln!("[completed] turns={turns}"),
            AgentEvent::Interrupted => eprintln!("[interrupted]"),
            AgentEvent::Error { message } => eprintln!("[error] {message:?}"),
            AgentEvent::ApprovalRequest { name, call_id, .. } => {
                let _ = session.resolve_approval(&call_id, agent_core::ApprovalDecision::Approve);
                eprintln!("[approval] {name} -> auto-approve");
            }
        }
    }
    let _ = handle.await;

    let mut filled = 0usize;
    let mut colors: BTreeSet<u16> = BTreeSet::new();
    session.with_document(|doc| {
        for layer in &doc.layers {
            for frame in &doc.frames {
                if let Some(cel) = doc.cel(&layer.id, &frame.id) {
                    for idx in &cel.indices {
                        if *idx != 0 {
                            filled += 1;
                            colors.insert(*idx);
                        }
                    }
                }
            }
        }
    });
    eprintln!(
        "== trace: {:.1}s tool_calls={} distinct_tools={} text_chars={text_chars} reasoning_chars={reasoning_chars}",
        started.elapsed().as_secs_f32(),
        tools.len(),
        tools.iter().collect::<BTreeSet<_>>().len(),
    );
    eprintln!(
        "== canvas: filled_px={filled} distinct_colors={}",
        colors.len()
    );
    dump_canvas(session.as_ref());

    if tools.is_empty() {
        eprintln!("!! 一个工具都没调");
    }
    if filled == 0 {
        eprintln!("!! 画布是空的");
    }
}

/// 把每一帧按调色板字符打一遍，再整张导成 PNG：肉眼判断「像不像」的唯一办法。
fn dump_canvas(session: &agent_core::AgentSession) {
    session.with_document(|doc| {
        let palette = doc
            .palettes
            .first()
            .map(|p| p.colors.clone())
            .unwrap_or_default();
        for (fi, frame) in doc.frames.iter().enumerate() {
            let Some(cel) = doc.cel(&doc.layers[0].id, &frame.id) else {
                continue;
            };
            let ramp = " .:-=+*#%@";
            eprintln!("-- frame {fi} ({})", frame.id);
            for y in 0..doc.height {
                let row: String = (0..doc.width)
                    .map(|x| {
                        let idx = cel.indices[(y * doc.width + x) as usize];
                        if idx == 0 {
                            return ' ';
                        }
                        let Some(rgb) = palette.get((idx - 1) as usize) else {
                            return ramp.as_bytes()[ramp.len() - 1] as char;
                        };
                        // 简单亮度映射：按 RGB 算个灰度挑字符，肉眼看得出形状就够。
                        let lum =
                            (rgb.r as u32 * 299 + rgb.g as u32 * 587 + rgb.b as u32 * 114) / 1000;
                        let slot = (lum as usize * (ramp.len() - 1)) / 255;
                        ramp.as_bytes()[slot] as char
                    })
                    .collect();
                eprintln!("|{row}|");
            }
        }
        // 拼一张横向 spritesheet 存到临时目录，方便直接看。
        let img = pixel_core::sheet::spritesheet(doc, doc.frames.len().max(1) as u32);
        if let Ok(bytes) = pixel_core::png::encode_png(&img) {
            let path = std::env::temp_dir().join("aipixel-probe-sheet.png");
            let _ = std::fs::write(&path, bytes);
            eprintln!("-- sheet: {}", path.display());
        }
    });
}
