// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! Provider 传输层：把协议无关的 `ChatRequest` 翻译成 Anthropic Messages 或 OpenAI 兼容
//! Chat Completions 请求，并把 SSE 流归一成 `LlmEvent`。只支持用户自带 base_url + api_key，
//! 无内置服务器 / 登录 / 计费。

use super::models::{ChatRequest, ContentBlock, LlmEvent, Message, ModelConfig, Protocol, Role};
use async_trait::async_trait;
use futures_util::Stream;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

/// TCP 连接阶段的超时。
///
/// `reqwest::Client::new()` 一个超时都不带：网关收了连接却迟迟不响应时，
/// `.send().await` 能挂到天荒地老。重试状态发出去之后界面就再也不动，
/// 用户看到的正是「正在重试 1/5」之后的一片死寂。连接超时兜住最常卡的
/// 那一段；响应头那一段由 runner 的 `timeout` 收口。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// 所有 provider 共用的 HTTP 客户端。
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// 没配 Max tokens 时的输出上限兜底。真值在 `limits`，这里只留一个别名，
/// 免得两处数字各自漂移。
pub const DEFAULT_MAX_TOKENS: u32 = crate::limits::FALLBACK_MAX_TOKENS;

/// 按字符截断：provider 回的错误报文多为中文，按字节切会正好切在汉字
/// 三个字节的中间，Rust 直接 panic，挂掉的还是 async 任务——前端只看到
/// 一个永不返回的请求。给用户看的报文截到一眼读完就够。
pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}

/// 只给有值的可选字段占位。
///
/// `json!` 宏对 `Option::None` 会老实写成 `null`。不少中转（以及按严格 schema
/// 校验的 vLLM 部署）把 `"temperature": null` 当非法值，整条请求 400 拒掉。
/// 用户视角是「我什么都没改，就是没填温度，现在发不出消息了」。可选字段一律
/// 「有值才写进 body」，缺省交给端点自己的默认值。
fn set_some<T: serde::Serialize>(body: &mut Value, key: &str, value: Option<T>) {
    if let Some(v) = value {
        body[key] = serde_json::to_value(v).unwrap_or(Value::Null);
    }
}

/// 这个模型要不要把上一轮的推理内容原样带回。
///
/// DeepSeek 的推理系（reasoner / r1 / v3 / v4）在官方文档里写明了必须回传
/// `reasoning_content`，否则模型不认这段历史，下一轮会当新问题重想一遍。
/// 通义千问 QwQ、智谱 GLM 的思考版、Kimi K2 思考版同理。
/// OpenAI、Anthropic、Gemini 不认这个字段，回传反而会被端点拒掉，所以不放行。
///
/// 认模型名走 `limits::resolve_alias`：中转站把 `deepseek-v4.1-flash` 简写成
/// `deepseek-flash` 时，这条判定也必须跟着认出来。认不出的代价很具体——
/// 推理历史被丢掉，续写时模型当新问题重想一遍，用户看到的就是通篇重写。
pub fn echoes_reasoning(model: &str) -> bool {
    let name = crate::limits::resolve_alias(model);
    const MARKS: &[&str] = &[
        "deepseek-reasoner",
        "deepseek-r1",
        "deepseek-v3",
        "deepseek-v4",
        "qwq",
        "glm-4.5",
        "glm-4.6",
        "glm-z1",
        "kimi-k2-thinking",
        "kimi-thinking",
        "hunyuan-t1",
        "ernie-x1",
    ];
    MARKS.iter().any(|m| crate::limits::name_matches(&name, m))
}

/// 从 4xx 报错里抠出 provider 允许的输出上限。
///
/// 表里的花名认不全，猜大了 provider 会当场拒掉整轮请求。这种错不必让用户手改
/// 配置：它报的数就是上限，降下来重发一次就好。输出短了也不丢人——runner 的
/// 续写机制会把几截拼成一条完整回复。
pub fn token_cap_from_error(err: &ProviderError) -> Option<u32> {
    let ProviderError::Http { status, body } = err else {
        return None;
    };
    if !(400..500).contains(status) {
        return None;
    }
    let low = body.to_lowercase();
    // 先确认这是「输出上限」相关的抱怨，再在附近找它允许多少。
    let (anchor, keyword) = [
        "max_tokens",
        "max tokens",
        "max_completion_tokens",
        "max_output_tokens",
        "maxoutputtokens",
        "output token",
    ]
    .iter()
    .filter_map(|k| low.find(k).map(|at| (at, *k)))
    .min_by_key(|(at, _)| *at)?;
    // 只看报错里紧跟其后的那一小段：再往远就是另一段话了。
    let window: String = low[anchor + keyword.len()..].chars().take(160).collect();
    // 取最靠前的那句「允许多少」。泛泛地捡窗口里第一个数，捡到的多半是
    // 我们自己发过去的请求值，等于什么都没问出来。
    let mut best: Option<(usize, u32)> = None;
    for phrase in CAP_PHRASES {
        let Some(at) = window.find(phrase) else {
            continue;
        };
        let Some(n) = first_number(&window[at + phrase.len()..]) else {
            continue;
        };
        if !(MIN_TOKEN_CAP..=MAX_TOKEN_CAP).contains(&n) {
            continue;
        }
        if best.is_none_or(|(pos, _)| at < pos) {
            best = Some((at, n));
        }
    }
    best.map(|(_, n)| n)
}

/// 「它允许多少」的常见说法。英文为主，国内中转站的中文报错也见得到。
const CAP_PHRASES: &[&str] = &[
    "at most",
    "no more than",
    "not exceed",
    "cannot exceed",
    "less than or equal",
    "max allowed",
    "maximum allowed",
    "maximum",
    "allowed is",
    "allowed value",
    "limit is",
    "limit of",
    "up to",
    "<=",
    "上限为",
    "最大值",
    "最多",
    "不超过",
];

/// 抠出来的数得像个输出上限才算数，不然一条裹着 URL 的报错能把任何数字喂进来。
const MIN_TOKEN_CAP: u32 = 256;
const MAX_TOKEN_CAP: u32 = 1_000_000;

/// 窗口里第一段连续数字。
fn first_number(text: &str) -> Option<u32> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let digits: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<LlmEvent, ProviderError>> + Send>>;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("config error: {0}")]
    Config(String),
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn request(&self, req: &ChatRequest) -> Result<EventStream, ProviderError>;
    fn max_tokens(&self) -> u32 {
        DEFAULT_MAX_TOKENS
    }
}

/// 构造对应协议的 provider。
pub fn build_provider(config: &ModelConfig) -> Arc<dyn LlmProvider> {
    let client = http_client();
    match config.protocol {
        Protocol::Anthropic => Arc::new(AnthropicProvider {
            client,
            base_url: trim_trailing_slash(&config.base_url),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        }),
        Protocol::OpenAiCompat => Arc::new(OpenAiCompatProvider {
            client,
            base_url: trim_trailing_slash(&config.base_url),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        }),
    }
}

/// 归一 base_url：去掉首尾空白，再去掉右斜杠。
///
/// 从输入框拷过来的地址常带一个尾部换行或空格，`url::Url` 解析阶段不会报错，
/// 但拼出来的 host 是空串，最后只给一句看不出原因的「builder error」。
/// 空白和斜杠都在这里收口，两个 provider 都受益。
fn trim_trailing_slash(s: &str) -> String {
    s.trim().trim_end_matches('/').trim().to_string()
}

/// 拉一份 provider 的模型清单，给设置界面让用户挑着填。
///
/// 只认 `GET {base_url}/models`：Anthropic 走 `x-api-key`，OpenAI 兼容走 `Bearer`。
/// base_url 里带不带 `/v1` 都试一遍——不少中转只挂在裸域名上，写死一段路径就 404。
pub async fn list_models(config: &ModelConfig) -> Result<Vec<String>, ProviderError> {
    if config.api_key.trim().is_empty() {
        return Err(ProviderError::Config("missing api key".into()));
    }
    let base = trim_trailing_slash(config.base_url.trim());
    if base.is_empty() {
        return Err(ProviderError::Config("missing base url".into()));
    }
    let client = http_client();
    let mut last_err: Option<ProviderError> = None;
    for url in model_list_urls(&base) {
        let mut req = client.get(&url);
        req = match config.protocol {
            Protocol::Anthropic => req
                .header("x-api-key", &config.api_key)
                .header("anthropic-version", "2023-06-01"),
            Protocol::OpenAiCompat => {
                req.header("authorization", format!("Bearer {}", config.api_key))
            }
        };
        // 模型列表以前一样裸奔：端点接了连接却迟迟不回，设置里点「获取模型」
        // 就是一个永远转的圈，而且第一个候选 URL 挂住还堵着后面那次尝试。
        let resp = match super::http::send(req, super::http::ONESHOT_TIMEOUT).await {
            Ok(resp) => resp,
            Err(e) => {
                last_err = Some(e);
                continue;
            }
        };
        let status = resp.status();
        let body = super::http::read_text(
            resp,
            super::http::TEXT_BODY_CAP,
            super::http::ONESHOT_TIMEOUT,
            400,
        )
        .await
        .unwrap_or_default();
        if !status.is_success() {
            last_err = Some(ProviderError::Http {
                status: status.as_u16(),
                body,
            });
            continue;
        }
        return Ok(extract_model_ids(&body));
    }
    Err(last_err.unwrap_or_else(|| ProviderError::Config("no model list endpoint".into())))
}

/// 模型列表端点候选：先按用户填的 base_url 试，域名不同再退回裸域名试一次。
pub(crate) fn model_list_urls(base: &str) -> Vec<String> {
    let mut out = vec![format!("{base}/models")];
    if let Some(origin) = origin_of(base) {
        let candidate = format!("{origin}/models");
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// 从 base_url 里取 `scheme://host`。探测模型列表要打 `${origin}/models`，
/// 而用户填的 base_url 常带版本路径（.../v1、.../v4）：照整个 URL 拼会变成
/// /v1/models 之外的四不像，只取源是唯一对两头都成立的做法。
fn origin_of(base: &str) -> Option<String> {
    let (scheme, rest) = base.split_once("://")?;
    let host = rest.split('/').next().filter(|h| !h.is_empty())?;
    Some(format!("{scheme}://{host}"))
}

/// 从 `GET /models` 的响应里抠出 id 列表，去重排序。
/// Anthropic 和 OpenAI 两边的字段名都是 `data[].id`，所以一份解析吃两头。
fn extract_model_ids(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = value
        .get("data")
        .and_then(|d| d.as_array())
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("id").and_then(|id| id.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 拿得出一份 OpenAI 兼容的清单。
    #[test]
    fn reads_the_openai_list_shape() {
        let body = r#"{"object":"list","data":[{"id":"gpt-4o-mini"},{"id":"gpt-4o"}]}"#;
        assert_eq!(
            extract_model_ids(body),
            vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()]
        );
    }

    /// Anthropic 多一个 display_name，字段位置不一样但不影响取 id。
    #[test]
    fn reads_the_anthropic_list_shape() {
        let body = r#"{"data":[{"type":"model","id":"claude-sonnet-4-5","display_name":"Claude Sonnet 4.5"}],"has_more":false}"#;
        assert_eq!(
            extract_model_ids(body),
            vec!["claude-sonnet-4-5".to_string()]
        );
    }

    /// 没填温度时请求体里连字段都不该有：`null` 会让严格校验 schema 的端点
    /// 整条 400，症状是「我什么都没改，就是突然发不出消息了」。
    #[test]
    fn an_unset_temperature_is_absent_rather_than_null() {
        let req = ChatRequest {
            system: "你是像素助手".into(),
            messages: vec![Message::user_text("画只猫")],
            tools: Vec::new(),
            max_tokens: 1024,
            temperature: None,
            disable_thinking: false,
            echo_reasoning: false,
        };
        let body = openai_body("some-model", &req);
        assert!(
            body.get("temperature").is_none(),
            "没填温度就不该发这个字段，实际 body 是 {}",
            serde_json::to_string(&body).unwrap()
        );
        // 填了就得真发出去：缺字段和发错值一样是故障。f32 转 f64 会带尾差，
        // 按浮点数比大小而不是比相等。
        let filled = ChatRequest {
            temperature: Some(0.7),
            ..req
        };
        let sent = openai_body("m", &filled)["temperature"].as_f64();
        assert_eq!(sent.map(|v| (v * 10.0).round() as i64), Some(7));
    }

    /// 只剩推理的 assistant 回合不许进请求：推理过网就被丢掉，剩一条
    /// `content: null` 且没有工具调用的空消息，端点只当它是坏请求。
    #[test]
    fn an_assistant_message_with_only_reasoning_never_reaches_the_request() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Reasoning {
                text: "先在脑子里过一遍".into(),
            }],
        }];
        assert!(
            to_openai_messages(&messages, false).is_empty(),
            "推理被端点字典挡掉之后，这条消息就该整条消失"
        );
        let kept = to_openai_messages(&messages, true);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["reasoning_content"], json!("先在脑子里过一遍"));
    }

    /// Anthropic 要求 user/assistant 严格交替。续写时「只吐推理」的半截会被
    /// 推成紧挨着的第二条 user 指令，逐条发出去就是 400。相邻同角色必须并成一轮。
    #[test]
    fn anthropic_never_sees_two_user_turns_in_a_row() {
        let messages = vec![
            Message::user_text("画一只五帧橘猫"),
            Message::user_text("接着写"),
            Message::assistant(vec![ContentBlock::Text {
                text: "画完了".into(),
            }]),
        ];
        let out = to_anthropic_messages(&messages);
        assert_eq!(out.len(), 2, "两条相邻 user 必须并成一整轮，实际 {out:?}");
        assert_eq!(out[0]["role"], json!("user"));
        assert_eq!(out[1]["role"], json!("assistant"));
    }

    /// 同理，只剩推理的 assistant 回合推平就是空 content 数组，Anthropic 直接拒。
    #[test]
    fn anthropic_drops_a_turn_that_would_carry_no_blocks_at_all() {
        let messages = vec![
            Message::user_text("画猫"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Reasoning {
                    text: "想想怎么画".into(),
                }],
            },
            Message::user_text("改一下"),
        ];
        let out = to_anthropic_messages(&messages);
        assert_eq!(out.len(), 1, "空 assistant 该整条消失，实际 {out:?}");
        assert_eq!(out[0]["role"], json!("user"));
        let blocks = out[0]["content"].as_array().expect("content 是块数组");
        assert_eq!(blocks.len(), 2, "两条 user 的正文都要留下");
    }

    /// 中转偶尔会把同一个模型在不同端点各报一次，去重排序后再给用户挑。
    #[test]
    fn dedups_and_sorts_ids() {
        let body = r#"{"data":[{"id":"qwen-plus"},{"id":"deepseek-chat"},{"id":"qwen-plus"}]}"#;
        assert_eq!(
            extract_model_ids(body),
            vec!["deepseek-chat".to_string(), "qwen-plus".to_string()]
        );
    }

    /// 端点通了但不是模型清单（比如返回了一页 HTML），就当没拉到，别把噪声当选项。
    #[test]
    fn returns_nothing_when_the_payload_is_not_a_model_list() {
        assert!(extract_model_ids("<html>login</html>").is_empty());
        assert!(extract_model_ids(r#"{"object":"list"}"#).is_empty());
        assert!(extract_model_ids(r#"{"data":[]}"#).is_empty());
    }

    /// base_url 带路径时补一个裸域名候选，让只认域名的中转也能通。
    #[test]
    fn offers_a_bare_origin_candidate_when_the_base_url_has_a_path() {
        assert_eq!(
            model_list_urls("https://api.example.com/v1"),
            vec![
                "https://api.example.com/v1/models".to_string(),
                "https://api.example.com/models".to_string(),
            ]
        );
    }

    /// 裸域名只试一次，别把同一个地址请求两遍。
    #[test]
    fn keeps_one_candidate_when_the_base_url_is_bare() {
        assert_eq!(
            model_list_urls("https://api.example.com"),
            vec!["https://api.example.com/models".to_string()]
        );
    }

    /// 推理系的模型要把上一轮的思考原样带回，不然下一轮会被当成新问题重想一遍。
    #[test]
    fn only_the_reasoning_families_echo_their_thoughts_back() {
        for model in [
            "deepseek-reasoner",
            "deepseek-r1",
            "deepseek-v3",
            // 用户手里那台就是 v4 系的 flash：续写时最吃这一口。
            "deepseek-v4.1-flash",
            "deepseek-v4-pro",
            "qwq-plus",
            "glm-4.6",
            "kimi-k2-thinking",
            "hunyuan-t1",
            // 带目录前缀的中转也得认。
            "vllm/deepseek-v4.1-flash",
        ] {
            assert!(echoes_reasoning(model), "{model} 该回传推理内容");
        }
        // 简名 `deepseek-flash` 也是推理系：认不出的代价是模型当新问题重想一遍。
        assert!(echoes_reasoning("deepseek-flash"));
        assert!(echoes_reasoning("deepseek-v4-1-flash"));
        assert!(echoes_reasoning("deepseek-flash-20260101"));
        // 这些端点不认 reasoning_content，硬塞会被拒掉，或者白丢一半上下文。
        for model in [
            "gpt-4o",
            "gpt-5",
            "o3",
            "claude-sonnet-4-5",
            "gemini-2.5-pro",
            "deepseek-chat",
            "qwen-max",
            "grok-4",
        ] {
            assert!(!echoes_reasoning(model), "{model} 不该回传推理内容");
        }
    }

    fn http(status: u16, body: &str) -> ProviderError {
        ProviderError::Http {
            status,
            body: body.into(),
        }
    }

    /// 把若干 OpenAI SSE 数据块依次喂给解析器，收集所有事件（跨块累加 ToolCall 状态）。
    fn openai_stream(chunks: &[&str]) -> Vec<LlmEvent> {
        let mut acc = ToolAcc::default();
        let mut out = vec![];
        for c in chunks {
            out.extend(openai_parse(c, &mut acc).expect("chunk parses"));
        }
        out
    }

    fn tool_starts(events: &[LlmEvent]) -> Vec<(String, String)> {
        events
            .iter()
            .filter_map(|e| match e {
                LlmEvent::ToolUseStart { id, name, .. } => Some((id.clone(), name.clone())),
                _ => None,
            })
            .collect()
    }

    fn tool_args(events: &[LlmEvent]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                LlmEvent::ToolInputDelta { json_partial, .. } => Some(json_partial.as_str()),
                _ => None,
            })
            .collect()
    }

    /// 中转把 id 和 name 拆进不同分片：老代码会静默丢掉整条 tool call，表现为"模型
    /// 要画却没触发任何工具"。现在按分片累加，到齐才发 start，args 一个字节都不丢。
    #[test]
    fn a_tool_call_split_across_chunks_still_assembles() {
        let ev = openai_stream(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"pixel_run_shader"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"x\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}"#,
        ]);
        assert_eq!(
            tool_starts(&ev),
            vec![("call_1".to_string(), "pixel_run_shader".to_string())]
        );
        assert_eq!(tool_args(&ev), "{\"x\":1}");
        // start 必须早于任何 arguments 分片，否则 runner 会把它丢进不存在的槽。
        let start = ev
            .iter()
            .position(|e| matches!(e, LlmEvent::ToolUseStart { .. }))
            .unwrap();
        let first_arg = ev
            .iter()
            .position(|e| matches!(e, LlmEvent::ToolInputDelta { .. }))
            .unwrap();
        assert!(
            start < first_arg,
            "ToolUseStart 必须早于任何 arguments 分片"
        );
    }

    /// 更极端：arguments 比 name 先到。先缓存，name 到齐发 start 时按序补发。
    #[test]
    fn arguments_arriving_before_the_name_are_buffered_not_dropped() {
        let ev = openai_stream(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_9","function":{"arguments":"{\"a\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"draw","arguments":"7}"}}]}}]}"#,
        ]);
        assert_eq!(
            tool_starts(&ev),
            vec![("call_9".to_string(), "draw".to_string())]
        );
        assert_eq!(tool_args(&ev), "{\"a\":7}");
    }

    /// 正常一片到齐的老路子不能回归。
    #[test]
    fn a_tool_call_in_one_chunk_still_assembles() {
        let ev = openai_stream(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"apply","arguments":"{}"}}]}}]}"#,
        ]);
        assert_eq!(
            tool_starts(&ev),
            vec![("c".to_string(), "apply".to_string())]
        );
        assert_eq!(tool_args(&ev), "{}");
    }

    /// 更刁钻的一批中转：整条流一个 id 都不发，只有 name 和 arguments。
    /// 老逻辑要求 id 到齐才登记 start，工具会被整条吞掉——症状正是
    /// 「模型把预算全花在思考上了，一个工具都没调」。现在名字到了就按
    /// index 造 id 顶上，调用必须发得出去，参数一个字节都不能丢。
    #[test]
    fn a_provider_that_never_sends_ids_still_fires_the_tool() {
        let ev = openai_stream(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"pixel_plan","arguments":"{\"a\""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":1}"}}]}}]}"#,
        ]);
        assert_eq!(
            tool_starts(&ev),
            vec![("call-0".to_string(), "pixel_plan".to_string())]
        );
        assert_eq!(tool_args(&ev), "{\"a\":1}");
    }

    /// 收摊前没来得及发 start 的半条调用，flush 必须发生在 Done 之前。
    /// 排在 Done 后面的 ToolUseStart 会被 runner 当成下一回合的开头丢掉，
    /// 工具白发。这里连 name 都没到的纯 arguments 碎片，宁可丢也不瞎编工具名。
    #[test]
    fn a_half_arrived_tool_call_is_flushed_before_done() {
        let mut acc = ToolAcc::default();
        let mut out = VecDeque::new();
        let slot = acc.calls.entry(0).or_default();
        slot.name = "pixel_run_shader".into();
        slot.pre_args = "{\"shape\":\"ellipse\"}".into();
        let orphan = acc.calls.entry(1).or_default();
        orphan.pre_args = "{\"junk\":true}".into(); // 连名字都没有，该丢。
        acc.flush_unstarted(&mut out);

        let starts = tool_starts(out.iter().cloned().collect::<Vec<_>>().as_slice());
        assert_eq!(
            starts,
            vec![("call-0".to_string(), "pixel_run_shader".to_string())],
            "没名字的那条不许被编出来"
        );
        let args = tool_args(out.iter().cloned().collect::<Vec<_>>().as_slice());
        assert_eq!(args, "{\"shape\":\"ellipse\"}");
    }

    #[test]
    fn a_provider_that_names_its_ceiling_gets_clamped_to_it() {
        // 中转站嫌我们给的 max_tokens 太大，顺手告诉我们它允许多少。
        assert_eq!(
            token_cap_from_error(&http(
                400,
                r#"{"error":{"message":"max_tokens is too large: 65536, maximum allowed is 8192","type":"invalid_request_error"}}"#
            )),
            Some(8192)
        );
        // 我们自己发过去的那个数不能被误当成上限。
        assert_eq!(
            token_cap_from_error(&http(
                400,
                "max_tokens must be at most 16384 but 65536 was requested"
            )),
            Some(16384)
        );
        assert_eq!(
            token_cap_from_error(&http(400, "max_tokens <= 32768")),
            Some(32768)
        );
        assert_eq!(
            token_cap_from_error(&http(400, "max_tokens 上限为 32768")),
            Some(32768)
        );
        assert_eq!(
            token_cap_from_error(&http(400, "supports up to 64000 output tokens")),
            None,
            "没提 max_tokens 就不关输出上限的事"
        );
    }

    #[test]
    fn an_unrelated_complaint_says_nothing_about_tokens() {
        assert_eq!(token_cap_from_error(&http(401, "bad api key")), None);
        assert_eq!(token_cap_from_error(&http(404, "no such model")), None);
        assert_eq!(
            token_cap_from_error(&http(500, "internal error near max_tokens")),
            None,
            "5xx 是服务端自己的毛病，跟上限无关"
        );
        assert_eq!(
            token_cap_from_error(&http(400, "max_tokens: invalid")),
            None,
            "一句含糊的抱怨里没有可用的数"
        );
        assert_eq!(
            token_cap_from_error(&ProviderError::Network("connection reset".into())),
            None
        );
        assert_eq!(
            token_cap_from_error(&ProviderError::Config("missing api key".into())),
            None
        );
    }

    /// 回传与否只看开关：开着才补这个字段，塞错端点是会被整包拒掉的。
    #[test]
    fn reasoning_content_is_sent_only_when_the_model_asks_for_it() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Reasoning {
                    text: "先看看画布".into(),
                },
                ContentBlock::Text {
                    text: "我来画".into(),
                },
            ],
        }];

        let with = to_openai_messages(&messages, true);
        assert_eq!(
            with[0]["reasoning_content"],
            json!("先看看画布"),
            "回传时要把推理原样带上"
        );

        let without = to_openai_messages(&messages, false);
        assert!(
            without[0].get("reasoning_content").is_none(),
            "不该出现这个字段"
        );
        // 两边的正文都在：推理是附带信息，不是正文的替代品。
        assert_eq!(with[0]["content"], json!("我来画"));
        assert_eq!(without[0]["content"], json!("我来画"));
    }

    // ---- SseStream 字节级重组 ----

    /// 拿若干字节分片攒一条真实会来分片的 SSE 流。
    fn sse_from_chunks(chunks: Vec<Vec<u8>>) -> SseStream {
        let inner = futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|c| Ok::<_, reqwest::Error>(bytes::Bytes::from(c))),
        );
        SseStream {
            inner: Box::pin(inner),
            buf: Vec::new(),
            pending: VecDeque::new(),
            inner_done: false,
            held_cr: false,
        }
    }

    async fn drain_sse(stream: SseStream) -> (Vec<String>, Vec<ProviderError>) {
        use futures_util::StreamExt as _;
        let mut stream = stream;
        let mut data = Vec::new();
        let mut errs = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(d) => data.push(d),
                Err(e) => errs.push(e),
            }
        }
        (data, errs)
    }

    /// 汉字正好从三个字节的中间下刀。按分片做 `from_utf8_lossy` 的老逻辑会把
    /// 残缺字节换成 U+FFFD：帧拼回来了，内容却是乱的——重试再多次也只是再拿
    /// 一段乱码。现在逐字节缓冲，凑齐整帧才一次性解码。
    #[tokio::test]
    async fn a_multibyte_character_split_across_chunks_is_not_mangled() {
        let frame = "data: {\"content\":\"橘猫行走图\"}\n\n".as_bytes();
        // 每个可能的切点都试一遍：三字节的任何一刀都不该出乱码。
        for cut in 1..frame.len() {
            let (head, tail) = frame.split_at(cut);
            let (data, errs) = drain_sse(sse_from_chunks(vec![head.to_vec(), tail.to_vec()])).await;
            assert!(errs.is_empty(), "在 {cut} 处分片被判成非法 UTF-8");
            assert_eq!(
                data,
                vec!["{\"content\":\"橘猫行走图\"}".to_string()],
                "在 {cut} 处分片把汉字切坏了"
            );
        }
    }

    /// `\r\n` 行尾的帧边界。分片正好切在 `\r` 和 `\n` 之间时，一看见 `\r` 就
    /// 改写成 `\n` 的老逻辑会把帧空行判错，整帧丢掉——模型说完了这句话，界面
    /// 一个字都不显示，然后干等超时。
    #[tokio::test]
    async fn crlf_frame_boundaries_split_across_chunks_still_parse() {
        let raw = b"data: one\r\n\r\ndata: two\r\n\r\n";
        for cut in 1..raw.len() {
            let (head, tail) = raw.split_at(cut);
            let (data, errs) = drain_sse(sse_from_chunks(vec![head.to_vec(), tail.to_vec()])).await;
            assert!(errs.is_empty(), "在 {cut} 处分片被当成非法 UTF-8");
            assert_eq!(
                data,
                vec!["one".to_string(), "two".to_string()],
                "在 {cut} 处切开漏掉了帧"
            );
        }
    }

    /// 200 但内容不是 SSE 的响应（整段 JSON 的错误页、HTML 登录跳转页都这样）
    /// 永远等不到空行。不设上限就能把缓冲吃到底；撞限要报成解码失败，好让上层
    /// 按可重试错误走，而不是静默返回一段空流、再空转掉全部重试次数。
    #[tokio::test]
    async fn an_endpoint_that_never_sends_a_frame_boundary_fails_instead_of_buffering_forever() {
        let blob = vec![b'x'; 4 * 1024 * 1024];
        let (_data, errs) =
            drain_sse(sse_from_chunks(vec![blob.clone(), blob.clone(), blob])).await;
        let first = errs.first().expect("撞到上限必须报错");
        assert!(
            matches!(first, ProviderError::Decode(_)),
            "对面没在说 SSE 是解码层面的失败，不是网络失败"
        );
        assert!(
            first.to_string().contains("probably not speaking SSE"),
            "报错要说清楚对面没在说 SSE: {first}"
        );
    }
}

// ---------------- SSE ----------------

/// 无帧边界时的缓冲上限。一条 200 但内容不是 SSE 的响应（整段 JSON 的代理
/// 最常这样）永远等不来空行，不设限就能把缓冲吃到底；正常 SSE 一帧只有
/// 几百字节，8MB 已是极端宽松。
const SSE_FRAME_CAP: usize = 8 * 1024 * 1024;

/// 原始 SSE 流，产出每个 event 的 `data:` 载荷（String）。
///
/// 缓冲是字节级的：网络分片会从汉字三个字节的中间下刀，按分片做
/// `from_utf8_lossy` 会把残缺字节变成 U+FFFD、后面的字节跟着乱码，
/// 交出一段「格式正确但内容是错」的输出——重试也救不回来。只有凑齐
/// 整帧才一次性解码，非法字节原样报错，不悄悄替换。
struct SseStream {
    inner: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
    buf: Vec<u8>,
    pending: VecDeque<String>,
    inner_done: bool,
    /// 上一个分片以 `\r` 收尾。它可能是独立的 CR 行尾，也可能是 CRLF 的
    /// 前半截，要等下一个字节才能定，先把解码按住不并进缓冲。
    held_cr: bool,
}

impl SseStream {
    /// 把新分片按字节并入，行尾统一成 `\n`。
    fn absorb(&mut self, chunk: &[u8]) {
        for &b in chunk {
            if self.held_cr {
                self.held_cr = false;
                self.buf.push(b'\n');
                if b != b'\n' {
                    self.push_byte(b);
                }
            } else {
                self.push_byte(b);
            }
        }
    }

    fn push_byte(&mut self, b: u8) {
        if b == b'\r' {
            self.held_cr = true;
        } else {
            self.buf.push(b);
        }
    }

    /// 把已就绪的帧从缓冲里摘出来。帧按空行切；迟迟等不到空行又一直涨，
    /// 说明对面没在说 SSE，直接断流报错，别把内存吃穿。
    fn drain_frames(&mut self) -> Result<(), ProviderError> {
        while let Some(pos) = self.buf.windows(2).position(|w| w == b"\n\n") {
            let frame: Vec<u8> = self.buf[..pos].to_vec();
            self.buf.drain(..pos + 2);
            let text = decode_frame(&frame)?;
            if let Some(data) = frame_data(&text) {
                self.pending.push_back(data);
            }
        }
        if self.buf.len() > SSE_FRAME_CAP {
            return Err(ProviderError::Decode(format!(
                "SSE frame exceeded {} bytes without a frame boundary; the endpoint is probably not speaking SSE",
                SSE_FRAME_CAP
            )));
        }
        Ok(())
    }
}

/// 整帧一次性解码。切帧已经保证字符完整，这里再撞上非法 UTF-8 就是对面
/// 真的在发脏数据，报错比悄悄替换成 U+FFFD 诚实。
fn decode_frame(frame: &[u8]) -> Result<String, ProviderError> {
    match std::str::from_utf8(frame) {
        Ok(s) => Ok(s.to_string()),
        Err(e) => Err(ProviderError::Decode(format!(
            "SSE frame was not valid UTF-8: {e}"
        ))),
    }
}

impl Stream for SseStream {
    type Item = Result<String, ProviderError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(f) = this.pending.pop_front() {
                return Poll::Ready(Some(Ok(f)));
            }
            if this.inner_done {
                return Poll::Ready(None);
            }
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    this.absorb(&chunk);
                    if let Err(e) = this.drain_frames() {
                        return Poll::Ready(Some(Err(e)));
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(ProviderError::Network(e.to_string()))));
                }
                Poll::Ready(None) => {
                    this.inner_done = true;
                    if this.held_cr {
                        this.held_cr = false;
                        this.buf.push(b'\n');
                    }
                    if !this.buf.is_empty() {
                        let frame = std::mem::take(&mut this.buf);
                        match decode_frame(&frame) {
                            Ok(text) => {
                                if let Some(data) = frame_data(&text) {
                                    this.pending.push_back(data);
                                }
                            }
                            Err(e) => return Poll::Ready(Some(Err(e))),
                        }
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// 提取一个 SSE frame 里的 data 载荷（可能多行，以 "\n" 连接）。
fn frame_data(frame: &str) -> Option<String> {
    let mut lines: Vec<&str> = Vec::new();
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            lines.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// provider 特定的增量解析累积状态。
#[derive(Default)]
struct ToolAcc {
    /// index -> 流式 tool call 分片累加器。
    calls: HashMap<usize, StreamToolCall>,
    stop: String,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
}

impl ToolAcc {
    /// 流收摊时给「只到了一半」的 tool call 兜底，绝不晚于 Done。
    ///
    /// 两种漏网：name 到了 id 没到的（feed 里已经就地造 id 顶住），以及
    /// arguments 分片在最后一包里、SSE 紧跟着就 [DONE] 的。后者要是直接
    /// push_done，缓存在 pre_args 里的参数会跟着整条报废。
    /// 只有名字的才补发，连名字都没有的纯 arguments 碎片宁可丢掉——
    /// 瞎编工具名比丢掉一次调用危险得多。
    fn flush_unstarted(&mut self, out: &mut VecDeque<LlmEvent>) {
        let mut pending: Vec<usize> = self
            .calls
            .iter()
            .filter(|(_, c)| !c.started && !c.name.is_empty())
            .map(|(i, _)| *i)
            .collect();
        pending.sort_unstable();
        for idx in pending {
            if let Some(call) = self.calls.get_mut(&idx) {
                out.push_back(LlmEvent::ToolUseStart {
                    index: idx,
                    id: if call.id.is_empty() {
                        format!("call-{idx}")
                    } else {
                        call.id.clone()
                    },
                    name: call.name.clone(),
                });
                let args = std::mem::take(&mut call.pre_args);
                if !args.is_empty() {
                    out.push_back(LlmEvent::ToolInputDelta {
                        index: idx,
                        json_partial: args,
                    });
                }
                call.started = true;
            }
        }
    }
}

/// 一个流式 tool call 的分片累加器，OpenAI 与 Anthropic 两条 parser 共用。
///
/// 有的 OpenAI 兼容中转会把 id 和 function.name 拆进不同 SSE 分片，甚至 arguments 比
/// name 先到。老逻辑要求首片同时带 id+name 才登记 start，否则整条 tool call 被静默吞掉：
/// 模型明明要画，却一个工具都没触发。现在按分片累加——id/name 到齐才发 ToolUseStart，
/// start 之前先行到达的 arguments 先缓存并按序补发，绝不比 ToolUseStart 早一步。
#[derive(Default)]
struct StreamToolCall {
    id: String,
    name: String,
    started: bool,
    /// ToolUseStart 之前到达的 arguments 分片，start 时一次性按序补发。
    pre_args: String,
}

impl StreamToolCall {
    /// 累加 id/name 分片；首次两者齐备就发 `ToolUseStart`，并补发之前缓存的 arguments。
    fn feed(
        &mut self,
        index: usize,
        id: Option<&str>,
        name: Option<&str>,
        out: &mut Vec<LlmEvent>,
    ) {
        // 已 start 后中转可能重发 id/name，忽略即可。
        if self.started {
            return;
        }
        if let Some(id) = id {
            self.id.push_str(id);
        }
        if let Some(name) = name {
            self.name.push_str(name);
        }
        // 有的中转整条流都不发 tool_call.id。死等 id 的话工具会被整条吞掉——
        // 表现就是「模型把预算全花在思考上了，一个工具都没调」。名字到了就按
        // index 造一个本地 id 顶上：回传 assistant/tool 两条消息用的是同一个
        // id，闭环验证时端点自己也分不出真假，比丢掉整次工具调用便宜得多。
        if self.id.is_empty() && !self.name.is_empty() {
            self.id = format!("call-{index}");
        }
        if self.id.is_empty() || self.name.is_empty() {
            return; // 还没到齐，继续等下一片。
        }
        self.started = true;
        out.push(LlmEvent::ToolUseStart {
            index,
            id: self.id.clone(),
            name: self.name.clone(),
        });
        if !self.pre_args.is_empty() {
            out.push(LlmEvent::ToolInputDelta {
                index,
                json_partial: std::mem::take(&mut self.pre_args),
            });
        }
    }

    /// arguments 分片：started 之后直接转发，否则先缓存，绝不早于 ToolUseStart。
    fn feed_args(&mut self, index: usize, args: &str, out: &mut Vec<LlmEvent>) {
        if args.is_empty() {
            return;
        }
        if self.started {
            out.push(LlmEvent::ToolInputDelta {
                index,
                json_partial: args.to_string(),
            });
        } else {
            self.pre_args.push_str(args);
        }
    }
}

type ParseFn = fn(&str, &mut ToolAcc) -> Result<Vec<LlmEvent>, ProviderError>;

/// 把 SSE 数据流归一成 `LlmEvent`。
struct ParsedStream {
    sse: SseStream,
    parse: ParseFn,
    acc: ToolAcc,
    out: VecDeque<LlmEvent>,
    finished: bool,
    emitted_done: bool,
}

impl Stream for ParsedStream {
    type Item = Result<LlmEvent, ProviderError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(ev) = this.out.pop_front() {
                return Poll::Ready(Some(Ok(ev)));
            }
            if this.finished {
                return Poll::Ready(None);
            }
            match Pin::new(&mut this.sse).poll_next(cx) {
                Poll::Ready(Some(Ok(data))) => {
                    if data.trim() == "[DONE]" {
                        this.flush_then_done();
                        continue;
                    }
                    match (this.parse)(&data, &mut this.acc) {
                        Ok(events) => {
                            let mut has_done = false;
                            for ev in events {
                                if matches!(ev, LlmEvent::Done { .. }) {
                                    has_done = true;
                                }
                                this.out.push_back(ev);
                            }
                            if has_done {
                                this.flush_then_done();
                            }
                        }
                        Err(e) => return Poll::Ready(Some(Err(e))),
                    }
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => {
                    this.flush_then_done();
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl ParsedStream {
    /// 先补发漏掉的 tool call，再收摊。顺序不能颠倒：Done 一进队列，
    /// runner 就认为这一回合结束，排在它后面的 ToolUseStart 会被丢进
    /// 下一回合的开头，工具白发。
    fn flush_then_done(&mut self) {
        if !self.emitted_done {
            self.acc.flush_unstarted(&mut self.out);
        }
        self.push_done();
    }

    fn push_done(&mut self) {
        if !self.emitted_done {
            let reason = if self.acc.stop.is_empty() {
                "stop".to_string()
            } else {
                self.acc.stop.clone()
            };
            self.out.push_back(LlmEvent::Done {
                stop_reason: reason,
            });
            self.emitted_done = true;
        }
        self.finished = true;
    }
}

fn anthropic_parse(payload: &str, acc: &mut ToolAcc) -> Result<Vec<LlmEvent>, ProviderError> {
    let v: Value =
        serde_json::from_str(payload).map_err(|e| ProviderError::Decode(e.to_string()))?;
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let events = match ty {
        "message_start" => {
            if let Some(u) = v["message"]["usage"]["input_tokens"].as_u64() {
                acc.input_tokens = Some(u as u32);
            }
            vec![]
        }
        "content_block_start" => {
            let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            let block = &v["content_block"];
            match block.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    let slot = acc.calls.entry(idx).or_default();
                    slot.id = id;
                    slot.name = name;
                    slot.started = true;
                    vec![LlmEvent::ToolUseStart {
                        index: idx,
                        id: slot.id.clone(),
                        name: slot.name.clone(),
                    }]
                }
                _ => vec![],
            }
        }
        "content_block_delta" => {
            let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            let delta = &v["delta"];
            match delta.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text_delta" => match delta.get("text").and_then(|t| t.as_str()) {
                    Some(t) => vec![LlmEvent::Token(t.to_string())],
                    None => vec![],
                },
                "thinking_delta" | "reasoning_delta" => {
                    let text = delta
                        .get("thinking")
                        .or_else(|| delta.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    vec![LlmEvent::Reasoning(text.to_string())]
                }
                "input_json_delta" => match delta.get("partial_json").and_then(|t| t.as_str()) {
                    Some(part) => vec![LlmEvent::ToolInputDelta {
                        index: idx,
                        json_partial: part.to_string(),
                    }],
                    None => vec![],
                },
                _ => vec![],
            }
        }
        "message_delta" => {
            let mut out = vec![];
            if let Some(stop) = v["delta"]["stop_reason"].as_str() {
                acc.stop = stop.to_string();
            }
            if let Some(o) = v["usage"]["output_tokens"].as_u64() {
                acc.output_tokens = Some(o as u32);
            }
            if acc.input_tokens.is_some() || acc.output_tokens.is_some() {
                out.push(LlmEvent::Usage {
                    input_tokens: acc.input_tokens,
                    output_tokens: acc.output_tokens,
                });
            }
            out
        }
        "message_stop" => {
            let reason = if acc.stop.is_empty() {
                "stop".to_string()
            } else {
                acc.stop.clone()
            };
            vec![LlmEvent::Done {
                stop_reason: reason,
            }]
        }
        "error" => {
            let msg = v["error"]["message"]
                .as_str()
                .unwrap_or("provider error")
                .to_string();
            return Err(ProviderError::Decode(msg));
        }
        _ => vec![],
    };
    Ok(events)
}

fn openai_parse(payload: &str, acc: &mut ToolAcc) -> Result<Vec<LlmEvent>, ProviderError> {
    let v: Value =
        serde_json::from_str(payload).map_err(|e| ProviderError::Decode(e.to_string()))?;
    let mut out = vec![];
    if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
        let input = u
            .get("prompt_tokens")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        let output = u
            .get("completion_tokens")
            .and_then(|t| t.as_u64())
            .map(|t| t as u32);
        acc.input_tokens = input;
        acc.output_tokens = output;
        if input.is_some() || output.is_some() {
            out.push(LlmEvent::Usage {
                input_tokens: input,
                output_tokens: output,
            });
        }
    }
    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        if let Some(choice) = choices.first() {
            let delta = &choice["delta"];
            if let Some(reasoning) = delta
                .get("reasoning_content")
                .or_else(|| delta.get("reasoning"))
                .and_then(|t| t.as_str())
            {
                out.push(LlmEvent::Reasoning(reasoning.to_string()));
            }
            if let Some(content) = delta.get("content").and_then(|t| t.as_str()) {
                if !content.is_empty() {
                    out.push(LlmEvent::Token(content.to_string()));
                }
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tool_calls {
                    let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                    let id = tc.get("id").and_then(|s| s.as_str());
                    let name = tc["function"].get("name").and_then(|s| s.as_str());
                    let args = tc["function"].get("arguments").and_then(|s| s.as_str());
                    let slot = acc.calls.entry(index).or_default();
                    slot.feed(index, id, name, &mut out);
                    if let Some(args) = args {
                        slot.feed_args(index, args, &mut out);
                    }
                }
            }
            if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                acc.stop = reason.to_string();
            }
        }
    }
    Ok(out)
}

// ---------------- Anthropic ----------------

struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn request(&self, req: &ChatRequest) -> Result<EventStream, ProviderError> {
        if self.api_key.trim().is_empty() {
            return Err(ProviderError::Config("missing api key".into()));
        }
        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "stream": true,
            "system": req.system,
            "messages": to_anthropic_messages(&req.messages),
            "tools": req.tools.iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.schema,
            })).collect::<Vec<_>>(),
        });
        set_some(&mut body, "temperature", req.temperature);
        // Anthropic 默认就不思考，所以只在用户要求关的时候显式声明。
        // 空字段不能写 null：端点会把 null 当非法值整条拒掉。
        if req.disable_thinking {
            body["thinking"] = json!({"type": "disabled"});
        }
        let url = format!("{}/messages", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        ensure_ok(resp).await.map(|sse| {
            Box::pin(ParsedStream {
                sse,
                parse: anthropic_parse,
                acc: ToolAcc::default(),
                out: VecDeque::new(),
                finished: false,
                emitted_done: false,
            }) as EventStream
        })
    }
}

// ---------------- OpenAI compatible ----------------

/// 组装 OpenAI 兼容协议的请求体。
///
/// 抽成纯函数是为了能测。「可选字段有值才写」这条规矩一旦破防，症状不是崩溃，
/// 而是「我什么都没改，就是没填温度，现在突然发不出消息了」——严格校验 schema
/// 的端点会把 `"temperature": null` 当非法值整条 400 拒掉。只有把 body 摊开
/// 逐字段查才看得出来，所以这里必须是可断言的那一个。
pub(crate) fn openai_body(model: &str, req: &ChatRequest) -> Value {
    let mut messages = vec![json!({"role": "system", "content": req.system})];
    messages.extend(to_openai_messages(&req.messages, req.echo_reasoning));
    // temperature 只准出现在 set_some 里：`json!` 会把 `Option::None` 老实写成
    // `null`，而 set_some 只覆盖「有值」那一种，补不回来。曾经就在这里写穿，
    // 结果是没填温度的用户全部发不出消息。
    let mut body = json!({
        "model": model,
        "max_tokens": req.max_tokens,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": messages,
        "tools": req.tools.iter().map(|t| json!({
            "type": "function",
            "function": {
                "name": t.name,
                "description": t.description,
                "parameters": t.schema,
            },
        })).collect::<Vec<_>>(),
    });
    set_some(&mut body, "temperature", req.temperature);
    if req.disable_thinking {
        silence_thinking(&mut body);
    }
    body
}

/// 关掉思考：两种端点方言都发一遍，认不出的那个当未知字段忽略，代价为零。
///
/// vLLM / 通义 / LongCat 走 `chat_template_kwargs`，智谱一系走 `thinking.type`。
/// 抽成共享函数是因为一次性路径（读参考图、读视频简报、提示词微调）也得下这条
/// 指令：它们的 max_tokens 是按「几百个 token 出结论」定的，模型要是把额度全
/// 花在思考上，回来的就只有一段推理和一条「没有文本内容」的报错。先前那两个
/// body 各写一遍，一次性那边还漏了下指令，症状就是侧道任务莫名变慢、莫名失败。
pub(crate) fn silence_thinking(body: &mut Value) {
    body["thinking"] = json!({"type": "disabled"});
    body["chat_template_kwargs"] = json!({"enable_thinking": false});
}

struct OpenAiCompatProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    async fn request(&self, req: &ChatRequest) -> Result<EventStream, ProviderError> {
        if self.api_key.trim().is_empty() {
            return Err(ProviderError::Config("missing api key".into()));
        }
        let body = openai_body(&self.model, req);
        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        ensure_ok(resp).await.map(|sse| {
            Box::pin(ParsedStream {
                sse,
                parse: openai_parse,
                acc: ToolAcc::default(),
                out: VecDeque::new(),
                finished: false,
                emitted_done: false,
            }) as EventStream
        })
    }
}

async fn ensure_ok(resp: reqwest::Response) -> Result<SseStream, ProviderError> {
    let status = resp.status();
    if !status.is_success() {
        let code = status.as_u16();
        return Err(ProviderError::Http {
            status: code,
            body: read_error_body(resp).await,
        });
    }
    // 200 但对面不是在说 SSE：整段 JSON 的错误页、HTML 登录跳转页、网关的
    // 占位响应都长这样。不查 content-type 的话，这些字节会被当成 SSE 慢慢找
    // 空行，永远找不到——最后靠 SSE_FRAME_CAP 撞限才报，而报出来的还是
    // 「解码失败」。五次重试全耗在上面，用户只看到一句看不出原因的报错。
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    // 空 content-type 不拦：有服务端确实不写这个头，但照样在吐 SSE。
    let looks_like_sse = content_type.is_empty()
        || content_type.contains("event-stream")
        || content_type.starts_with("text/plain");
    if !looks_like_sse {
        let body = read_error_body(resp).await;
        return Err(ProviderError::Decode(format!(
            "HTTP 200 came back with content-type `{}` instead of `text/event-stream`, and the body is not an SSE stream: {}",
            content_type,
            truncate_chars(body.trim(), 400)
        )));
    }
    Ok(SseStream {
        inner: Box::pin(resp.bytes_stream()),
        buf: Vec::new(),
        pending: VecDeque::new(),
        inner_done: false,
        held_cr: false,
    })
}

/// 读一段响应 body，读到 `ERROR_BODY_CAP` 为止。
///
/// `resp.text()` 不设底：端点把连接挂着慢慢吐坏 HTML 时，这个 await 就是
/// 永远。读满上限立刻收手，反正给用户看的只是前四百来个字符。
///
/// 响应 body 以上的传输不由这里设超时——那属于客户端层。调用方若需要整体
/// 时限，在 `timeout()` 里包住本函数。
async fn read_error_body(resp: reqwest::Response) -> String {
    let cap = 256 * 1024;
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                buf.extend_from_slice(&bytes);
                if buf.len() >= cap {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

// ---------------- message conversion ----------------

fn anthropic_block(b: &ContentBlock) -> Option<Value> {
    Some(match b {
        ContentBlock::Text { text } => json!({"type": "text", "text": text}),
        ContentBlock::Image {
            media_type,
            data_base64,
        } => json!({
            "type": "image",
            "source": {"type": "base64", "media_type": media_type, "data": data_base64},
        }),
        ContentBlock::ToolUse { id, name, input } => json!({
            "type": "tool_use", "id": id, "name": name, "input": input,
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": is_error,
        }),
        // Anthropic 要求 thinking block 带签名，回灌会报错；直接丢弃推理块。
        ContentBlock::Reasoning { .. } => return None,
    })
}

/// 内部消息 -> Anthropic Messages 的 messages 数组。
///
/// 两步重排都是为了 Anthropic 的结构要求：System 不进数组（它只有顶层
/// `system` 字段，塞进数组会被当成普通 user turn 重复一遍）；连续的
/// tool_result 必须合并进同一个 user turn——Anthropic 没有 tool 角色，
/// 拆成多条消息会触发 "tool_result blocks must immediately follow"。
pub(crate) fn to_anthropic_messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let m = &messages[i];
        match m.role {
            Role::System => {
                i += 1;
            }
            Role::Tool => {
                // 连续 tool_result 合并进一个 user turn。
                let mut blocks = Vec::new();
                while i < messages.len() && messages[i].role == Role::Tool {
                    for b in &messages[i].content {
                        if let Some(v) = anthropic_block(b) {
                            blocks.push(v);
                        }
                    }
                    i += 1;
                }
                push_anthropic_turn(&mut out, "user", blocks);
            }
            Role::User | Role::Assistant => {
                let role = match m.role {
                    Role::Assistant => "assistant",
                    _ => "user",
                };
                let blocks: Vec<Value> = m.content.iter().filter_map(anthropic_block).collect();
                push_anthropic_turn(&mut out, role, blocks);
                i += 1;
            }
        }
    }
    out
}

/// 追一轮内容；一个块都凑不出来就整轮不发。
///
/// 两件事一起办：
///
/// 一、已经有一轮同角色的就并进去。Anthropic 要求 user/assistant 严格交替，
/// 相邻同角色一律 400 "roles must alternate"。这种排列在消息簿里很实在：
/// 续写时「只吐推理没动笔」的半截会被推成紧挨着的第二条 user 指令
/// （见 runner::push_resume），而工具结果在协议里本就记在 user 名下，
/// 所以 [user, tool] 也得并，不能只盯着相邻的两条 user 消息。
///
/// 二、空的 content 数组整轮不发。Anthropic 拒收没有任何 content block 的
/// 回合，而推理块过网前会被丢掉（`anthropic_block` 对 Reasoning 返回 None），
/// 只剩推理的 assistant 回合滤完必然为空。发出去换来一整轮 400，
/// 不如让它消失——调用方只在没有 tool_use/tool_result 的回合上走这条路，
/// 所以消失也拆不散配对。
fn push_anthropic_turn(out: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    let joins = out
        .last()
        .is_some_and(|m| m["role"] == json!(role) && m["content"].is_array());
    if joins {
        if let Some(arr) = out.last_mut().and_then(|m| m["content"].as_array_mut()) {
            arr.extend(blocks);
            return;
        }
    }
    out.push(json!({"role": role, "content": blocks}));
}

/// 内部消息 -> OpenAI Chat Completions 的 messages 数组。
///
/// System 在这里被丢掉（调用方已经把它放在请求顶层）；带图的 user 消息
/// 必须换成 content 数组形式——字符串 content 表达不了图片，
/// 照字符串发过去，参考图会被端点静默丢弃，模型只能对着文字猜。
pub(crate) fn to_openai_messages(messages: &[Message], echo_reasoning: bool) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m.role {
            Role::System => {}
            Role::User => {
                let has_image = m
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Image { .. }));
                if has_image {
                    let content: Vec<Value> = m
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(json!({"type": "text", "text": text})),
                            ContentBlock::Image {
                                media_type,
                                data_base64,
                            } => Some(json!({
                                "type": "image_url",
                                "image_url": {"url": format!("data:{media_type};base64,{data_base64}")},
                            })),
                            _ => None,
                        })
                        .collect();
                    out.push(json!({"role": "user", "content": content}));
                } else {
                    out.push(json!({"role": "user", "content": m.text_of()}));
                }
            }
            Role::Assistant => {
                let text = m.text_of();
                // 推理模型的连续对话要靠这段：上一轮想过什么不带回去，模型就
                // 当新问题重想一遍，续写出来的东西自然就是把开头再念一次。
                let reasoning: String = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Reasoning { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let tool_calls: Vec<Value> = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolUse { id, name, input } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": input.to_string()},
                        })),
                        _ => None,
                    })
                    .collect();
                // 什么都没带才丢。推理回传是要留下的：DeepSeek 一系要靠它
                // 才认这段历史（见 `echoes_reasoning`），把它和空消息一起
                // 扔掉，模型下轮就当新问题重想一遍。
                // 而「推理说了全部」又正好被端点字典挡掉时，`content: null`
                // 加没有 tool_calls 会被严格校验的端点整包 400——那才该整条消失。
                // 它不带 tool_use，所以消失也拆不散 tool_result 的配对。
                let echoes = echo_reasoning && !reasoning.is_empty();
                if text.is_empty() && tool_calls.is_empty() && !echoes {
                    continue;
                }
                let text = if text.is_empty() {
                    Value::Null
                } else {
                    json!(text)
                };
                let mut msg = json!({"role": "assistant", "content": text});
                if echo_reasoning && !reasoning.is_empty() {
                    msg["reasoning_content"] = json!(reasoning);
                }
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = json!(tool_calls);
                }
                out.push(msg);
            }
            Role::Tool => {
                for b in &m.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = b
                    {
                        let mut c = content.clone();
                        if *is_error {
                            c = format!("ERROR: {c}");
                        }
                        out.push(
                            json!({"role": "tool", "tool_call_id": tool_use_id, "content": c}),
                        );
                    }
                }
            }
        }
    }
    out
}
