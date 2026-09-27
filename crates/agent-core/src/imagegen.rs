//! 生图模型：让模型直接产出一张位图，再由 pixelize 量化到画布网格。
//!
//! 这条路径和 agent 主循环是两套完全不同的接法：agent 让模型写 Lua 脚本落像素，
//! 而生图模型只会回图。两者不该混在一个循环里，否则模型既写不好脚本也生不好图。
//!
//! 现实里「OpenAI 兼容」这个帽子底下有两种生图方式：
//! 1. `POST {base}/images/generations`，最早也最直接，OpenAI / Together / SiliconFlow 走这条。
//! 2. `POST {base}/chat/completions` 带 `modalities: ["text","image"]`，
//!    Gemini 图像模型和 OpenRouter 走这条，顺带还能吃参考图做垫图。
//!
//! 同一个 base_url 下两种都可能存在，所以这里按顺序试，只在「端点不存在」时降级；
//! 其余错误（鉴权、参数、内容策略）必须原样抛出去，否则用户看到的会是莫名其妙的二次失败。

use super::models::{Attachment, ModelConfig, Protocol};
use super::providers::ProviderError;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub const MAX_IMAGE_BYTES: usize = 24 * 1024 * 1024;

/// 一次生图的产出：原始字节 + media type + 实际走通的传输方式。
#[derive(Debug, Clone)]
pub struct GeneratedImage {
    pub media_type: String,
    pub bytes: Vec<u8>,
    /// "images" 或 "chat_modalities"，回给 UI 便于排查端点问题。
    pub transport: &'static str,
    /// 模型顺带返回的文本（有些实现会解释自己画了什么），没有则为空。
    pub note: String,
}

/// 生图请求参数。
#[derive(Debug, Clone, Default)]
pub struct ImageGenParams {
    /// 生图提示词。为空时报错，不要把空 prompt 发给计费接口。
    pub prompt: String,
    /// 只在使用 chat_modalities 传输时才有效的尺寸提示，如 "1024x1024"。
    pub size: Option<String>,
    /// 参考图（垫图）。只有 chat_modalities 传输支持。
    pub reference: Option<Attachment>,
}

#[async_trait]
pub trait ImageGenerator: Send + Sync {
    async fn generate(&self, params: &ImageGenParams) -> Result<GeneratedImage, ProviderError>;
}

/// Anthropic Messages 协议不返回位图，直接说明白，别让用户以为是网络问题。
struct UnsupportedGenerator;

#[async_trait]
impl ImageGenerator for UnsupportedGenerator {
    async fn generate(&self, _params: &ImageGenParams) -> Result<GeneratedImage, ProviderError> {
        Err(ProviderError::Config(
            "the Anthropic Messages protocol returns text only; switch this model's protocol to an OpenAI-compatible image endpoint (for example Gemini image models or OpenRouter) to generate images".into(),
        ))
    }
}

/// OpenAI 兼容端点：先试 images/generations，端点不存在再退回 chat modalities。
struct OpenAiCompatGenerator {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

#[async_trait]
impl ImageGenerator for OpenAiCompatGenerator {
    async fn generate(&self, params: &ImageGenParams) -> Result<GeneratedImage, ProviderError> {
        if params.prompt.trim().is_empty() {
            return Err(ProviderError::Config("empty image prompt".into()));
        }
        match self.via_images(params).await {
            Ok(img) => Ok(img),
            // 端点不存在才降级。400 说明请求本身有问题，重试也是同样失败。
            Err(ProviderError::Http {
                status: 404 | 405 | 501,
                ..
            }) => self.via_chat(params).await,
            Err(e) => Err(e),
        }
    }
}

impl OpenAiCompatGenerator {
    async fn via_images(&self, params: &ImageGenParams) -> Result<GeneratedImage, ProviderError> {
        let mut body = json!({
            "model": self.model,
            "prompt": params.prompt,
            "n": 1,
        });
        if let Some(size) = &params.size {
            // 有些实现只认一种尺寸，参数不合法会 400；只在用户显式指定时才带。
            body["size"] = json!(size);
        }
        let value = self.post("images/generations", &body).await?;
        let first = value
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|d| d.first())
            .ok_or_else(|| {
                ProviderError::Decode(format!(
                    "images/generations returned no data entry: {}",
                    short(&value)
                ))
            })?;
        if let Some(b64) = first.get("b64_json").and_then(|v| v.as_str()) {
            let bytes = pixel_core::decode::decode_base64(b64).map_err(ProviderError::Decode)?;
            let media_type = first
                .get("media_type")
                .and_then(|v| v.as_str())
                .unwrap_or("image/png")
                .to_string();
            return Ok(GeneratedImage {
                media_type,
                bytes,
                transport: "images",
                note: first
                    .get("revised_prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        if let Some(url) = first.get("url").and_then(|v| v.as_str()) {
            let (media_type, bytes) = self.fetch_image(url).await?;
            return Ok(GeneratedImage {
                media_type,
                bytes,
                transport: "images",
                note: String::new(),
            });
        }
        Err(ProviderError::Decode(format!(
            "images/generations entry had neither b64_json nor url: {}",
            short(first)
        )))
    }

    async fn via_chat(&self, params: &ImageGenParams) -> Result<GeneratedImage, ProviderError> {
        let mut user_content: Vec<Value> = vec![json!({"type": "text", "text": params.prompt})];
        if let Some(reference) = &params.reference {
            user_content.push(json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{};base64,{}", reference.media_type, reference.data_base64),
                },
            }));
        }
        let mut body = json!({
            "model": self.model,
            "stream": false,
            "modalities": ["text", "image"],
            "messages": [{"role": "user", "content": user_content}],
        });
        if let Some(size) = &params.size {
            // Gemini 系接受 image_config.aspect_ratio；这里是通用兼容层，只做提示性字段。
            body["image_config"] = json!({"aspect_ratio": aspect_from_size(size)});
        }
        let value = self.post("chat/completions", &body).await?;
        let message = value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .and_then(|c| c.get("message"))
            .ok_or_else(|| {
                ProviderError::Decode(format!(
                    "chat/completions returned no choice: {}",
                    short(&value)
                ))
            })?;
        let note = message
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string();
        // 各家字段名不统一：images[]（Gemini）、content 里的 image_url 块（OpenRouter）。
        let entry = message
            .get("images")
            .and_then(|i| i.as_array())
            .and_then(|i| i.first());
        if let Some(entry) = entry {
            let url = entry
                .get("image_url")
                .and_then(|u| u.get("url"))
                .and_then(|u| u.as_str());
            if let Some(url) = url {
                let (media_type, bytes) = self.fetch_image(url).await?;
                return Ok(GeneratedImage {
                    media_type,
                    bytes,
                    transport: "chat_modalities",
                    note,
                });
            }
        }
        Err(ProviderError::Decode(format!(
            "chat/completions carried no image part (expected message.images[0].image_url.url): {}",
            short(message)
        )))
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value, ProviderError> {
        if self.api_key.trim().is_empty() {
            return Err(ProviderError::Config("missing api key".into()));
        }
        let resp = self
            .client
            .post(format!("{}/{path}", self.base_url))
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
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
        resp.json()
            .await
            .map_err(|e| ProviderError::Decode(e.to_string()))
    }

    /// 图可能是 data URL（多数）或 http URL（少数）；后者要真的去取。
    async fn fetch_image(&self, url: &str) -> Result<(String, Vec<u8>), ProviderError> {
        if url.starts_with("data:") {
            let (media_type, payload) =
                pixel_core::decode::split_data_url(url).map_err(ProviderError::Decode)?;
            let bytes =
                pixel_core::decode::decode_base64(payload).map_err(ProviderError::Decode)?;
            return Ok((media_type, bytes));
        }
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ProviderError::Http {
                status: status.as_u16(),
                body: format!("fetching image from {url}"),
            });
        }
        let media_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("image/png")
            .to_string();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(ProviderError::Decode(format!(
                "generated image is {} bytes; above the {} byte limit",
                bytes.len(),
                MAX_IMAGE_BYTES
            )));
        }
        Ok((media_type, bytes.to_vec()))
    }
}

pub fn build_image_generator(config: &ModelConfig) -> Arc<dyn ImageGenerator> {
    let client = reqwest::Client::new();
    match config.protocol {
        Protocol::Anthropic => Arc::new(UnsupportedGenerator),
        Protocol::OpenAiCompat => Arc::new(OpenAiCompatGenerator {
            client,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        }),
    }
}

/// 把 "1024x1024" 这类尺寸折成 aspect ratio；解析不了就不发 image_config。
fn aspect_from_size(size: &str) -> Option<String> {
    let (w, h) = size.split_once('x')?;
    let w: u32 = w.trim().parse().ok()?;
    let h: u32 = h.trim().parse().ok()?;
    if w == 0 || h == 0 {
        return None;
    }
    let g = gcd(w.max(h), w.min(h));
    Some(format!("{}:{}", w / g, h / g))
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn short(value: &Value) -> String {
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

    fn params(prompt: &str) -> ImageGenParams {
        ImageGenParams {
            prompt: prompt.to_string(),
            size: None,
            reference: None,
        }
    }

    #[tokio::test]
    async fn anthropic_explains_itself_instead_of_failing_opaquely() {
        let config = ModelConfig {
            id: "m".into(),
            label: "m".into(),
            protocol: Protocol::Anthropic,
            base_url: "https://api.anthropic.com/v1".into(),
            api_key: "key".into(),
            model: "claude".into(),
            max_tokens: None,
            temperature: None,
            capabilities: Default::default(),
        };
        let gen = build_image_generator(&config);
        let err = gen.generate(&params("a knight")).await.unwrap_err();
        let text = err.to_string();
        assert!(text.contains("Anthropic"), "{text}");
        assert!(text.contains("OpenAI-compatible"), "{text}");
    }

    #[tokio::test]
    async fn an_empty_prompt_never_reaches_the_endpoint() {
        let config = ModelConfig {
            id: "m".into(),
            label: "m".into(),
            protocol: Protocol::OpenAiCompat,
            base_url: "https://example.invalid/v1".into(),
            api_key: "key".into(),
            model: "m".into(),
            max_tokens: None,
            temperature: None,
            capabilities: Default::default(),
        };
        let gen = build_image_generator(&config);
        let err = gen.generate(&params("   ")).await.unwrap_err();
        assert!(matches!(err, ProviderError::Config(_)), "{err}");
    }

    #[test]
    fn aspect_ratio_reduces_common_sizes_and_rejects_nonsense() {
        assert_eq!(aspect_from_size("1024x1024").as_deref(), Some("1:1"));
        assert_eq!(aspect_from_size("1280x720").as_deref(), Some("16:9"));
        assert_eq!(aspect_from_size("720x1280").as_deref(), Some("9:16"));
        assert_eq!(aspect_from_size("1024 x 1024").as_deref(), Some("1:1"));
        assert_eq!(aspect_from_size("wide"), None);
        assert_eq!(aspect_from_size("0x10"), None);
    }

    #[test]
    fn generated_image_carries_its_transport_for_debugging() {
        let img = GeneratedImage {
            media_type: "image/png".into(),
            bytes: vec![1, 2, 3],
            transport: "images",
            note: String::new(),
        };
        assert_eq!(img.transport, "images");
        assert_eq!(img.bytes.len(), 3);
    }
}
