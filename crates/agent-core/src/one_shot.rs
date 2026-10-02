// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 一次性非流式对话：POST 一个 chat 请求，拿回整段文本。
//!
//! `providers.rs` 的 `LlmProvider` 是流式的，因为 agent 主循环要边出字边打断。
//! 但参考图读简报、提示词微调、生图模型的 chat 变体都属于「必须拿到完整结果才能解析」，
//! 用流式等于把 SSE 缓存回来再做一次拼接，没必要。这里单独收口。
//!
//! 消息转换复用 providers 里的函数，保证同一份 `Message` 在两条路径上翻译一致。

use super::http::{ONESHOT_TIMEOUT, TEXT_BODY_CAP};
use super::models::{ChatRequest, Message, ModelConfig, Protocol};
use super::providers::{ProviderError, DEFAULT_MAX_TOKENS};
use serde_json::{json, Value};

/// 发一次不带工具的对话，返回助手的全部文本。
///
/// `system` 为空时仍会作为系统消息发出——两家协议都允许空 system，
/// 比条件分支更省心，也不会因为漏发而改变模型行为。
pub async fn chat_once(
    config: &ModelConfig,
    system: &str,
    messages: Vec<Message>,
    max_tokens: Option<u32>,
) -> Result<String, ProviderError> {
    if config.api_key.trim().is_empty() {
        return Err(ProviderError::Config("missing api key".into()));
    }
    let req = oneshot_request(config, system, messages, max_tokens);
    let client = super::providers::http_client();
    match config.protocol {
        Protocol::Anthropic => {
            let body = anthropic_oneshot_body(config, &req);
            let resp = post_json(
                &client,
                &format!("{}/messages", trim(&config.base_url)),
                &config.api_key,
                Protocol::Anthropic,
                &body,
            )
            .await?;
            let value: Value = super::http::read_json(resp, TEXT_BODY_CAP, ONESHOT_TIMEOUT).await?;
            // content 是块数组；只取 text 块，tool_use / thinking 与本路径无关。
            let text = value
                .get("content")
                .and_then(|c| c.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if text.trim().is_empty() {
                return Err(ProviderError::Decode(format!(
                    "response carried no text content: {}",
                    one_line(&value)
                )));
            }
            Ok(text)
        }
        Protocol::OpenAiCompat => {
            let body = openai_oneshot_body(config, &req);
            let resp = post_json(
                &client,
                &format!("{}/chat/completions", trim(&config.base_url)),
                &config.api_key,
                Protocol::OpenAiCompat,
                &body,
            )
            .await?;
            let value: Value = super::http::read_json(resp, TEXT_BODY_CAP, ONESHOT_TIMEOUT).await?;
            let text = value
                .get("choices")
                .and_then(|c| c.as_array())
                // 第一个 choice 未必带 content：中转偶尔在它前面塞一个只填了
                // finish_reason 的空壳。取第一个真有文字的，别把空壳当答案。
                .and_then(|c| {
                    c.iter()
                        .find(|choice| {
                            !choice
                                .pointer("/message/content")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .trim()
                                .is_empty()
                        })
                        .or_else(|| c.first())
                })
                .and_then(|c| c.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string();
            if text.trim().is_empty() {
                // 有的兼容层把内容放在 reasoning_content，或返回数组形式，都不常见；
                // 直接报错比猜更诚实。
                return Err(ProviderError::Decode(format!(
                    "response carried no text content: {}",
                    one_line(&value)
                )));
            }
            Ok(text)
        }
    }
}

/// 把「模型配置 + 本次调用」翻译成一份 `ChatRequest`。
///
/// 单独拆出来是为了能测：关思考这条默认值一旦丢在 body 拼装那一步（确实丢过），
/// 症状是侧道任务把预算烧在推理上、或者只回一段 reasoning 然后报「没有文本内容」，
/// 只有把请求体摊开断言才看得见。
fn oneshot_request(
    config: &ModelConfig,
    system: &str,
    messages: Vec<Message>,
    max_tokens: Option<u32>,
) -> ChatRequest {
    ChatRequest {
        system: system.to_string(),
        messages,
        tools: Vec::new(),
        max_tokens: max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        temperature: config.temperature,
        // 单轮补写也可能挂着一截 reasoning，回灌口径跟着模型走。
        echo_reasoning: super::providers::echoes_reasoning(&config.model),
        // 补写是接着断点往下写，思考只会把断点重复一遍。用户没显式开思考时，
        // 一次性路径一律按「关」处理：额度要留给结论本身。
        disable_thinking: config.disable_thinking.unwrap_or(true),
    }
}

/// Anthropic 协议的一次性请求体。
///
/// 抽成纯函数的理由和 `providers::openai_body` 一样：「有值才写」这条规矩破防时
/// 症状不是崩溃，而是没填温度的用户突然发不出请求——`Option::None` 会被 `json!`
/// 老实写成 `null`，严格校验的端点整条 400 拒掉。摊开断言才看得出来。
fn anthropic_oneshot_body(config: &ModelConfig, req: &ChatRequest) -> Value {
    let mut body = json!({
        "model": config.model,
        "max_tokens": req.max_tokens,
        "stream": false,
        "system": req.system,
        "messages": super::providers::to_anthropic_messages(&req.messages),
    });
    if let Some(t) = config.temperature {
        body["temperature"] = json!(t);
    }
    // Anthropic 默认就不思考；这一句是给「模型配置里开着思考」的情形兜底的。
    if req.disable_thinking {
        body["thinking"] = json!({"type": "disabled"});
    }
    body
}

/// OpenAI 兼容协议的一次性请求体。与 `providers::openai_body` 的差别只在
/// `stream: false` 和不带 `stream_options`——非流式要这两个纯属多余。
fn openai_oneshot_body(config: &ModelConfig, req: &ChatRequest) -> Value {
    let mut messages = vec![json!({"role": "system", "content": req.system})];
    messages.extend(super::providers::to_openai_messages(
        &req.messages,
        req.echo_reasoning,
    ));
    let mut body = json!({
        "model": config.model,
        "max_tokens": req.max_tokens,
        "stream": false,
        "messages": messages,
    });
    if let Some(t) = config.temperature {
        body["temperature"] = json!(t);
    }
    // 关思考在流式路径上早就下了（providers::openai_body），一次性路径漏了它：
    // 读参考图、读视频简报、提示词微调的 max_tokens 都只有几百到一千五，模型
    // 把额度花在思考上，回来的就只有一段推理和一条「没有文本内容」的报错。
    if req.disable_thinking {
        super::providers::silence_thinking(&mut body);
    }
    body
}

async fn post_json(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    protocol: Protocol,
    body: &Value,
) -> Result<reqwest::Response, ProviderError> {
    let mut req = client.post(url).header("content-type", "application/json");
    req = match protocol {
        Protocol::Anthropic => req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        Protocol::OpenAiCompat => req.header("authorization", format!("Bearer {api_key}")),
    };
    // 超时和上限都收在 `http` 模块里，与生图、模型列表、MCP 三条链路同一份规矩。
    let resp = super::http::send(req.json(body), ONESHOT_TIMEOUT).await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ProviderError::Http {
            status: status.as_u16(),
            body: super::http::read_text(resp, TEXT_BODY_CAP, ONESHOT_TIMEOUT, 400).await?,
        });
    }
    Ok(resp)
}

fn trim(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

/// 把响应压成一行，塞进错误信息里；太长的 body 截断，避免刷屏。
fn one_line(value: &Value) -> String {
    super::providers::truncate_chars(&value.to_string(), 400)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(protocol: Protocol) -> ModelConfig {
        ModelConfig {
            id: "m".into(),
            label: "m".into(),
            protocol,
            base_url: "https://example.invalid/v1".into(),
            api_key: "k".into(),
            model: "gpt".into(),
            max_tokens: None,
            temperature: None,
            disable_thinking: None,
            capabilities: Default::default(),
        }
    }

    #[test]
    fn a_request_without_a_key_fails_before_any_network_call() {
        let config = config(Protocol::OpenAiCompat);
        let config = ModelConfig {
            api_key: "  ".into(),
            ..config
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let err = runtime
            .block_on(chat_once(
                &config,
                "sys",
                vec![Message::user_text("hi")],
                None,
            ))
            .unwrap_err();
        assert!(matches!(err, ProviderError::Config(_)), "{err}");
    }

    /// 一次性路径的预算是按几百 token 出结论定的。如果关思考这条默认值在 body
    /// 拼装那一步丢掉（确实丢过一次：ChatRequest 上算了、body 里没写），读图简报、
    /// 提示词微调就会把额度花在推理上，端回一段 reasoning 加一条「没有文本内容」。
    #[test]
    fn a_oneshot_body_actually_silences_thinking_by_default() {
        let cfg = config(Protocol::OpenAiCompat);
        let req = oneshot_request(&cfg, "sys", vec![Message::user_text("hi")], Some(900));
        assert!(req.disable_thinking, "模型配置没提思考时也该关");
        let body = openai_oneshot_body(&cfg, &req);
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": false})
        );
        // 非流式的身份标识：少了它，这条请求会被端点当成流式打发。
        assert_eq!(body["stream"], json!(false));
    }

    /// 用户显式开了思考时，请求体里就不该出现关思考的字段——反着写同样会出事。
    #[test]
    fn an_explicitly_enabled_thinking_is_left_alone() {
        let cfg = ModelConfig {
            disable_thinking: Some(false),
            ..config(Protocol::OpenAiCompat)
        };
        let req = oneshot_request(&cfg, "sys", vec![Message::user_text("hi")], None);
        assert!(!req.disable_thinking);
        let body = openai_oneshot_body(&cfg, &req);
        assert!(body.get("thinking").is_none(), "{body}");
        assert!(body.get("chat_template_kwargs").is_none(), "{body}");
    }

    /// `Option::None` 会被 `json!` 老实写成 `null`，严格校验 schema 的端点会整条 400
    /// 拒掉。没填温度就必须不发这个字段。
    #[test]
    fn a_oneshot_body_omits_temperature_when_it_is_unset() {
        let cfg = config(Protocol::OpenAiCompat);
        let req = oneshot_request(&cfg, "sys", vec![Message::user_text("hi")], None);
        let body = openai_oneshot_body(&cfg, &req);
        assert!(body.get("temperature").is_none(), "{body}");
        let cfg = ModelConfig {
            temperature: Some(0.35),
            ..cfg
        };
        let req = oneshot_request(&cfg, "sys", vec![Message::user_text("hi")], None);
        let body = openai_oneshot_body(&cfg, &req);
        // 温度是 f32，经 f64 进 JSON 后末位会变，直接跟 f64 字面量比必然不等。
        assert_eq!(body["temperature"], json!(0.35f32 as f64));
    }

    /// Anthropic 协议同样要下关思考指令，否则同一份额度在另一条协议上照样烧掉。
    #[test]
    fn an_anthropic_oneshot_body_silences_thinking() {
        let cfg = config(Protocol::Anthropic);
        let req = oneshot_request(&cfg, "sys", vec![Message::user_text("hi")], Some(1500));
        let body = anthropic_oneshot_body(&cfg, &req);
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert_eq!(body["max_tokens"], json!(1500));
    }

    #[test]
    fn one_line_truncates_long_response_bodies() {
        // 报文按字节切会在汉字三个字节的中间下刀，直接 panic；中文必须覆盖。
        let value = serde_json::json!({"error": "模型当前套餐不可用，请升级后再试。".repeat(200)});
        let text = one_line(&value);
        assert!(text.chars().count() <= 403, "{}", text.chars().count());
        assert!(text.ends_with("..."));
    }
}
