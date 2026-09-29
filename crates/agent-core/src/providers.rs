//! Provider 传输层：把协议无关的 `ChatRequest` 翻译成 Anthropic Messages 或 OpenAI 兼容
//! Chat Completions 请求，并把 SSE 流归一成 `LlmEvent`。只支持用户自带 base_url + api_key，
//! 无内置服务器 / 登录 / 计费。

use super::models::{ChatRequest, ContentBlock, LlmEvent, Message, ModelConfig, Protocol, Role};
use async_trait::async_trait;
use futures_util::Stream;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// 没配 Max tokens 时的输出上限兜底。真值在 `limits`，这里只留一个别名，
/// 免得两处数字各自漂移。
pub const DEFAULT_MAX_TOKENS: u32 = crate::limits::FALLBACK_MAX_TOKENS;

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
    let client = reqwest::Client::new();
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

fn trim_trailing_slash(s: &str) -> String {
    s.trim_end_matches('/').to_string()
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
    let client = reqwest::Client::new();
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
        let resp = match req.send().await {
            Ok(resp) => resp,
            Err(e) => {
                last_err = Some(ProviderError::Network(e.to_string()));
                continue;
            }
        };
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
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
}

// ---------------- SSE ----------------

/// 原始 SSE 流，产出每个 event 的 `data:` 载荷（String）。
struct SseStream {
    inner: Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
    buf: String,
    pending: VecDeque<String>,
    inner_done: bool,
}

impl SseStream {
    fn drain_frames(&mut self) {
        while let Some(pos) = self.buf.find("\n\n") {
            let frame: String = self.buf[..pos].to_string();
            self.buf.drain(..pos + 2);
            if let Some(data) = frame_data(&frame) {
                self.pending.push_back(data);
            }
        }
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
                    let s = String::from_utf8_lossy(&chunk);
                    this.buf.push_str(&s);
                    this.buf = this.buf.replace("\r\n", "\n").replace('\r', "\n");
                    this.drain_frames();
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(ProviderError::Network(e.to_string()))));
                }
                Poll::Ready(None) => {
                    this.inner_done = true;
                    if !this.buf.is_empty() {
                        let frame = std::mem::take(&mut this.buf);
                        if let Some(data) = frame_data(&frame) {
                            this.pending.push_back(data);
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
    names: HashMap<usize, (String, String)>, // index -> (id, name)
    started: std::collections::HashSet<usize>,
    stop: String,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
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
                        this.push_done();
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
                                this.push_done();
                            }
                        }
                        Err(e) => return Poll::Ready(Some(Err(e))),
                    }
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => {
                    this.push_done();
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl ParsedStream {
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
                    acc.names.insert(idx, (id.clone(), name.clone()));
                    acc.started.insert(idx);
                    vec![LlmEvent::ToolUseStart {
                        index: idx,
                        id,
                        name,
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
                    if !acc.started.contains(&index) {
                        if let (Some(id), Some(name)) = (id, name) {
                            acc.names.insert(index, (id.to_string(), name.to_string()));
                            acc.started.insert(index);
                            out.push(LlmEvent::ToolUseStart {
                                index,
                                id: id.to_string(),
                                name: name.to_string(),
                            });
                        }
                    }
                    if let Some(args) = tc["function"].get("arguments").and_then(|s| s.as_str()) {
                        if !args.is_empty() {
                            out.push(LlmEvent::ToolInputDelta {
                                index,
                                json_partial: args.to_string(),
                            });
                        }
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
            "temperature": req.temperature,
            "stream": true,
            "system": req.system,
            "messages": to_anthropic_messages(&req.messages),
            "tools": req.tools.iter().map(|t| json!({
                "name": t.name,
                "description": t.description,
                "input_schema": t.schema,
            })).collect::<Vec<_>>(),
        });
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
        let mut messages = vec![json!({"role": "system", "content": req.system})];
        messages.extend(to_openai_messages(&req.messages, req.echo_reasoning));
        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
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
        // 关思考的两种写法都发：vLLM / 通义 / LongCat 走 chat_template_kwargs，
        // 智谱一系走 thinking.type。认不出的端点当未知字段忽略，代价为零；
        // 认出来的立刻从「想半天」切到「直接调工具」。
        if req.disable_thinking {
            body["thinking"] = json!({"type": "disabled"});
            body["chat_template_kwargs"] = json!({"enable_thinking": false});
        }
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
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::Http {
            status: code,
            body: text,
        });
    }
    Ok(SseStream {
        inner: Box::pin(resp.bytes_stream()),
        buf: String::new(),
        pending: VecDeque::new(),
        inner_done: false,
    })
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
                out.push(json!({"role": "user", "content": blocks}));
            }
            Role::User | Role::Assistant => {
                let blocks: Vec<Value> = m.content.iter().filter_map(anthropic_block).collect();
                let role = match m.role {
                    Role::Assistant => "assistant",
                    _ => "user",
                };
                out.push(json!({"role": role, "content": blocks}));
                i += 1;
            }
        }
    }
    out
}

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
                let text = if text.is_empty() {
                    Value::Null
                } else {
                    json!(text)
                };
                let mut msg = json!({"role": "assistant", "content": text});
                if echo_reasoning && !reasoning.is_empty() {
                    msg["reasoning_content"] = json!(reasoning);
                }
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
