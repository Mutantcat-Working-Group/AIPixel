// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 一次性非流式对话：POST 一个 chat 请求，拿回整段文本。
//!
//! `providers.rs` 的 `LlmProvider` 是流式的，因为 agent 主循环要边出字边打断。
//! 但参考图读简报、提示词微调、生图模型的 chat 变体都属于「必须拿到完整结果才能解析」，
//! 用流式等于把 SSE 缓存回来再做一次拼接，没必要。这里单独收口。
//!
//! 消息转换复用 providers 里的函数，保证同一份 `Message` 在两条路径上翻译一致。

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
    let req = ChatRequest {
        system: system.to_string(),
        messages,
        tools: Vec::new(),
        max_tokens: max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        temperature: config.temperature,
        // 单轮补写也可能挂着一截 reasoning，回灌口径跟着模型走。
        echo_reasoning: super::providers::echoes_reasoning(&config.model),
        // 补写是接着断点往下写，思考只会把断点重复一遍。
        disable_thinking: config.disable_thinking.unwrap_or(true),
    };
    let client = reqwest::Client::new();
    match config.protocol {
        Protocol::Anthropic => {
            let body = json!({
                "model": config.model,
                "max_tokens": req.max_tokens,
                "temperature": req.temperature,
                "stream": false,
                "system": req.system,
                "messages": super::providers::to_anthropic_messages(&req.messages),
            });
            let resp = post_json(
                &client,
                &format!("{}/messages", trim(&config.base_url)),
                &config.api_key,
                Protocol::Anthropic,
                &body,
            )
            .await?;
            let value: Value = resp
                .json()
                .await
                .map_err(|e| ProviderError::Decode(e.to_string()))?;
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
            let mut messages = vec![json!({"role": "system", "content": req.system})];
            messages.extend(super::providers::to_openai_messages(
                &req.messages,
                super::providers::echoes_reasoning(&config.model),
            ));
            let body = json!({
                "model": config.model,
                "max_tokens": req.max_tokens,
                "temperature": req.temperature,
                "stream": false,
                "messages": messages,
            });
            let resp = post_json(
                &client,
                &format!("{}/chat/completions", trim(&config.base_url)),
                &config.api_key,
                Protocol::OpenAiCompat,
                &body,
            )
            .await?;
            let value: Value = resp
                .json()
                .await
                .map_err(|e| ProviderError::Decode(e.to_string()))?;
            let text = value
                .get("choices")
                .and_then(|c| c.as_array())
                .and_then(|c| c.first())
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
    let resp = req
        .json(body)
        .send()
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::Http {
            status: status.as_u16(),
            body: text,
        });
    }
    Ok(resp)
}

fn trim(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

/// 把响应压成一行，塞进错误信息里；太长的 body 截断，避免刷屏。
fn one_line(value: &Value) -> String {
    let text = value.to_string();
    if text.len() > 400 {
        format!("{}...", &text[..400])
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_without_a_key_fails_before_any_network_call() {
        let config = ModelConfig {
            id: "m".into(),
            label: "m".into(),
            protocol: Protocol::OpenAiCompat,
            base_url: "https://example.invalid/v1".into(),
            api_key: "  ".into(),
            model: "gpt".into(),
            max_tokens: None,
            temperature: None,
            disable_thinking: None,
            capabilities: Default::default(),
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

    #[test]
    fn one_line_truncates_long_response_bodies() {
        let value = serde_json::json!({"error": "x".repeat(2000)});
        let text = one_line(&value);
        assert!(text.len() <= 403, "{}", text.len());
        assert!(text.ends_with("..."));
    }
}
