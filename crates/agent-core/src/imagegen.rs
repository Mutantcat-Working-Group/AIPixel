// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 生图模型：让模型直接产出一张位图，再由 pixelize 量化到画布网格。
//!
//! 这条路径和 agent 主循环是两套完全不同的接法：agent 让模型写 Lua 脚本落像素，
//! 而生图模型只会回图。两者不该混在一个循环里，否则模型既写不好脚本也生不好图。
//!
//! 现实里「OpenAI 兼容」这个帽子底下有三种接法：
//! 1. `POST {base}/images/generations`，最早也最直接，OpenAI / Together / SiliconFlow 走这条；
//!    不吃垫图。
//! 2. `POST {base}/images/edits`（multipart），OpenAI 系的改图入口，把参考图真字节
//!    连同 prompt 一起发过去。DALL·E 2 / gpt-image-1 及多数兼容实现都认这个形态。
//! 3. `POST {base}/chat/completions` 带 `modalities: ["text","image"]`，
//!    Gemini 图像模型和 OpenRouter 走这条，顺带还能吃参考图做垫图。
//!
//! 同一个 base_url 下多种都可能存在，所以这里按顺序试，只在「端点不存在」时降级；
//! 其余错误（鉴权、参数、内容策略）必须原样抛出去，否则用户看到的会是莫名其妙的二次失败。
//! 但有一条铁律：带垫图的请求绝不退到不吃垫图的端点。用户选「改这一帧」「照这一帧
//! 再长一帧」时，垫图就是指令的真值，静默丢掉的后果是模型对着提示词自由发挥，
//! 而界面毫无异常——那是最坏的一种失败。

use super::models::{Attachment, AttachmentRole, ModelConfig, Protocol};
use super::providers::ProviderError;
use async_trait::async_trait;
use pixel_core::document::Document;
use serde::{Deserialize, Serialize};
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
    /// 尺寸提示，如 "1024x1024"。images/generations 与 images/edits 原样传；
    /// chat modalities 没有 size 字段，折成 image_config.aspect_ratio 做提示。
    pub size: Option<String>,
    /// 参考图（垫图）。有它就走只吃垫图的端点（images/edits 或 chat modalities），
    /// 绝不被静默丢弃。
    pub reference: Option<Attachment>,
}

/// 一张位图落到文档的哪个位置。供工作流坞与 agent 工具共用：
/// 两边问的是同一个问题，答案也该是同一个枚举。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LandSpot {
    /// 默认落在激活 cel。单张参考图的量化不该顺手多出一帧来。
    #[default]
    ActiveCel,
    /// 新建一帧再落进去（逐帧生图 / 视频抽帧 / 「照这一帧再长一帧」都走这条）。
    NewFrame,
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

/// OpenAI 兼容端点：没垫图先试 images/generations，端点不存在再退回 chat modalities；
/// 带垫图走 images/edits，端点不存在退 chat modalities，总之垫图不丢。
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
        if params.reference.is_some() {
            // 垫图在，只走吃垫图的端点。images/edits 是 OpenAI 系的改图入口；
            // 端点不存在（404/405/501）退 chat modalities（Gemini / OpenRouter）。
            // 400 照旧抛出去：请求本身有问题，换个端点发也是同样失败。
            match self.via_edits(params).await {
                Ok(img) => Ok(img),
                Err(ProviderError::Http {
                    status: 404 | 405 | 501,
                    ..
                }) => self.via_chat(params).await,
                Err(e) => Err(e),
            }
        } else {
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
        self.parse_images_response(&value, "images").await
    }

    /// 改图入口：multipart 把垫图真字节连同 prompt 发给 images/edits。
    /// 响应与 images/generations 同构，共用解析；差别只在请求形态。
    async fn via_edits(&self, params: &ImageGenParams) -> Result<GeneratedImage, ProviderError> {
        let Some(reference) = &params.reference else {
            return Err(ProviderError::Config(
                "images/edits needs a reference image".into(),
            ));
        };
        let bytes = pixel_core::decode::decode_base64(&reference.data_base64)
            .map_err(ProviderError::Decode)?;
        let mut form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .text("prompt", params.prompt.clone())
            .text("n", "1");
        if let Some(size) = &params.size {
            form = form.text("size", size.clone());
        }
        // 后缀跟着 media type 走：不少实现按文件名判类型，一律 .png 会让 JPEG 垫图被拒。
        let file_name = format!("reference.{}", extension_for(&reference.media_type));
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name)
            .mime_str(&reference.media_type)
            .map_err(|e| ProviderError::Decode(format!("bad reference media type: {e}")))?;
        form = form.part("image", part);
        let value = self.post_multipart("images/edits", form).await?;
        self.parse_images_response(&value, "images_edits").await
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
        // 垫图在而端点不回图时，把「垫图随请求发出去了」讲清楚：用户要排查的是
        // 端点能力，而不是怀疑自己是不是漏填了什么。
        let hint = if params.reference.is_some() {
            "; a reference image was attached, so this endpoint took the request but cannot answer with an image"
        } else {
            ""
        };
        Err(ProviderError::Decode(format!(
            "chat/completions carried no image part (expected message.images[0].image_url.url){hint}: {}",
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

    /// multipart 变体。content-type 必须由 reqwest 从 Form 推（boundary 在里面），
    /// 所以不能和 post 共用同一条请求链。
    async fn post_multipart(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<Value, ProviderError> {
        if self.api_key.trim().is_empty() {
            return Err(ProviderError::Config("missing api key".into()));
        }
        let resp = self
            .client
            .post(format!("{}/{path}", self.base_url))
            .header("authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
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
    let client = super::providers::http_client();
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

/// 探测结论。三态而不是布尔：说「能」要有凭据，说「不能」要看到端点真的不回图，
/// 剩下的一律交给用户自己判断——自动开关能力这件事，猜错比不猜更糟。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSupport {
    /// 端点能出图；transport 是实际走通的那条通路（images / images_edits / chat_modalities）。
    Yes { transport: &'static str },
    /// 端点或模型不出图；reason 是给人看的短码（no_endpoint / text_only / protocol）。
    No { reason: &'static str },
    /// 探测本身没跑通（鉴权、网络、参数）。不是「不能」，只是「不知道」。
    Unknown { reason: String },
}

/// 这个端点/模型能不能出图。
///
/// 探测是用户主动点的，所以允许花一次请求；但绝不为了让端点回图而真发一张生图任务——
/// 空 prompt 就够：真的端点会因为缺 prompt 回 400，不存在的端点回 404/405/501。
/// images 端不在时再退一步问 chat/completions（Gemini 图像模型、OpenRouter 走这条），
/// 那一步会真发一句 ping，但只换 16 个 token，换来的是「模型只回文字」这种确定答案。
pub async fn probe_image_support(config: &ModelConfig) -> ImageSupport {
    if config.model.trim().is_empty() {
        return ImageSupport::Unknown {
            reason: "no_model".into(),
        };
    }
    if config.api_key.trim().is_empty() {
        return ImageSupport::Unknown {
            reason: "missing_key".into(),
        };
    }
    match config.protocol {
        Protocol::Anthropic => ImageSupport::No { reason: "protocol" },
        Protocol::OpenAiCompat => {
            OpenAiCompatGenerator {
                client: super::providers::http_client(),
                base_url: config.base_url.trim_end_matches('/').to_string(),
                api_key: config.api_key.clone(),
                model: config.model.clone(),
            }
            .probe()
            .await
        }
    }
}

impl OpenAiCompatGenerator {
    /// 揽活：空 prompt 打一次 images/generations。
    /// 端点存在却抱怨缺 prompt，就说明它认这个端点，能出图。
    async fn probe(&self) -> ImageSupport {
        match self
            .post("images/generations", &json!({"model": self.model, "n": 1}))
            .await
        {
            Ok(_) => ImageSupport::Yes {
                transport: "images",
            },
            Err(ProviderError::Http { status, body }) => match status {
                // 端点不在。有些兼容层把「没有这个端点」也报成 400，所以 400 要核一遍文案。
                404 | 405 | 501 => self.probe_chat().await,
                400 if complains_about_prompt(&body) => ImageSupport::Yes {
                    transport: "images",
                },
                401 | 403 => ImageSupport::Unknown {
                    reason: "auth".into(),
                },
                _ => ImageSupport::Unknown {
                    reason: short_text(&body),
                },
            },
            Err(e) => ImageSupport::Unknown {
                reason: short_text(&e.to_string()),
            },
        }
    }

    /// 退一步问 chat/completions：带上 modalities 让它出图。
    /// 回文字说明这个模型只会聊天；连端点都不在，就是这个 Base URL 没有生图这回事。
    async fn probe_chat(&self) -> ImageSupport {
        let body = json!({
            "model": self.model,
            "stream": false,
            "max_tokens": 16,
            "modalities": ["text", "image"],
            "messages": [{"role": "user", "content": "ping"}],
        });
        match self.post("chat/completions", &body).await {
            Ok(value) => {
                let has_image = value
                    .get("choices")
                    .and_then(|c| c.as_array())
                    .and_then(|c| c.first())
                    .and_then(|c| c.get("message"))
                    .and_then(|m| m.get("images"))
                    .is_some();
                if has_image {
                    ImageSupport::Yes {
                        transport: "chat_modalities",
                    }
                } else {
                    ImageSupport::No {
                        reason: "text_only",
                    }
                }
            }
            Err(ProviderError::Http { status, body }) => match status {
                404 | 405 | 501 => ImageSupport::No {
                    reason: "no_endpoint",
                },
                401 | 403 => ImageSupport::Unknown {
                    reason: "auth".into(),
                },
                _ => ImageSupport::Unknown {
                    reason: short_text(&body),
                },
            },
            Err(e) => ImageSupport::Unknown {
                reason: short_text(&e.to_string()),
            },
        }
    }
}

/// 端点抱怨「缺 prompt」才算作认了这个端点。带上 model 相关文案的 400
/// 多半是模型名不对，那种情况只能算不知道，别顺手把能力勾上。
fn complains_about_prompt(body: &str) -> bool {
    body.to_lowercase().contains("prompt")
}

/// 把文档里某一帧合成成一张 PNG，当作垫图真值交给生图模型。
///
/// 这是「改这一帧」和「照这一帧再长一帧」的共同底座：用户要改的是画布上活着的东西，
/// 不是磁盘上一张可能已经过期的导出图。合成按图层顺序走，所以隐藏层不进去，
/// 模型看到的和用户看到的一致。
///
/// 角色恒为 Reference：这是要模型照着改的真值，不是供参考的截图，
/// 一旦混成 Snapshot，模型会以为「只是给你看看」，导致它不敢动笔。
pub fn frame_reference(doc: &Document, frame_id: &str) -> Result<Attachment, String> {
    let index = doc
        .frames
        .iter()
        .position(|frame| frame.id == frame_id)
        .ok_or_else(|| format!("unknown frame: {frame_id}"))?;
    let image = pixel_core::png::composite_frame(doc, index as u32);
    let bytes = pixel_core::png::encode_png(&image)?;
    Ok(Attachment {
        role: AttachmentRole::Reference,
        media_type: "image/png".into(),
        data_base64: pixel_core::png::base64_encode(&bytes),
    })
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

/// 探测理由的截断。给用户看的原始报文，截到一眼能读完的长度就够。
fn short_text(text: &str) -> String {
    if text.len() > 160 {
        format!("{}...", &text[..160])
    } else {
        text.to_string()
    }
}

impl OpenAiCompatGenerator {
    /// images/generations 与 images/edits 的响应同构：`data[0]` 里要么 `b64_json` 要么 `url`。
    /// transport 原样写进产出，界面回执用它告诉用户图是从哪个端点来的。
    async fn parse_images_response(
        &self,
        value: &Value,
        transport: &'static str,
    ) -> Result<GeneratedImage, ProviderError> {
        let first = value
            .get("data")
            .and_then(|d| d.as_array())
            .and_then(|d| d.first())
            .ok_or_else(|| {
                ProviderError::Decode(format!(
                    "image endpoint returned no data entry: {}",
                    short(value)
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
                transport,
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
                transport,
                note: String::new(),
            });
        }
        Err(ProviderError::Decode(format!(
            "image entry had neither b64_json nor url: {}",
            short(first)
        )))
    }
}

/// media type 转文件后缀，给 multipart 的垫图命名用。
fn extension_for(media_type: &str) -> &'static str {
    match media_type {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "png",
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
            disable_thinking: None,
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
            disable_thinking: None,
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

    /// 2x2 文档，左下角一块红。小尺寸是 image crate 无条件支持的。
    fn red_corner_doc() -> Document {
        let mut doc = Document::new("t", 2, 2).unwrap();
        let red = pixel_core::Rgba::rgb(255, 0, 0);
        let index = doc.intern_color(red).unwrap();
        let width = doc.width;
        if let Some(cel) = doc.cel_mut("L0", "F0") {
            cel.set(width, 1, 1, index);
        }
        doc
    }

    /// 把附件里的 base64 PNG 解回像素，取 (x,y) 的 RGBA。
    fn pixel_of(attachment: &Attachment, x: u32, y: u32) -> [u8; 4] {
        let bytes = pixel_core::decode::decode_base64(&attachment.data_base64).unwrap();
        let (rgba, width, _) =
            pixel_core::decode::decode_image(&bytes, &attachment.media_type).unwrap();
        let at = ((y * width + x) * 4) as usize;
        [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]]
    }

    #[test]
    fn frame_reference_paints_only_the_asked_frame() {
        use pixel_core::ops::{self, PixelOperation};

        let mut doc = red_corner_doc();
        let green = pixel_core::Rgba::rgb(0, 255, 0);
        let index = doc.intern_color(green).unwrap();
        ops::apply_batch(
            &mut doc,
            &[PixelOperation::CreateFrame {
                after: Some("F0".into()),
                duration_ms: 100,
                id: None,
            }],
        )
        .expect("setup applies");
        // 新帧 F1：左上角一块绿。F0 保持只有右下角红。
        let width = doc.width;
        if let Some(cel) = doc.cel_mut("L0", "F1") {
            cel.set(width, 0, 0, index);
        }

        let first = super::frame_reference(&doc, "F0").unwrap();
        assert_eq!(first.media_type, "image/png");
        assert_eq!(first.role, AttachmentRole::Reference);
        // F0：右下是红的，左上是透明。
        assert_eq!(pixel_of(&first, 1, 1), [255, 0, 0, 255]);
        assert_eq!(pixel_of(&first, 0, 0)[3], 0);

        let second = super::frame_reference(&doc, "F1").unwrap();
        // F1：左上是绿的，右下回到透明——证明渲染的是这一帧而不是整份文档。
        assert_eq!(pixel_of(&second, 0, 0), [0, 255, 0, 255]);
        assert_eq!(pixel_of(&second, 1, 1)[3], 0);
    }

    #[test]
    fn frame_reference_names_the_frame_it_could_not_find() {
        let doc = red_corner_doc();
        let err = super::frame_reference(&doc, "F9").unwrap_err();
        assert!(err.contains("F9"), "{err}");
    }

    #[test]
    fn frame_reference_honors_layer_visibility() {
        use pixel_core::ops::{self, PixelOperation};

        let mut doc = red_corner_doc();
        ops::apply_batch(
            &mut doc,
            &[PixelOperation::CreateLayer {
                after: None,
                name: None,
                id: None,
                palette_id: None,
                locked: None,
            }],
        )
        .expect("setup applies");
        let green = pixel_core::Rgba::rgb(0, 255, 0);
        let index = doc.intern_color(green).unwrap();
        // L1 整帧铺绿，然后藏起来：模型该看见的只有 L0 那块红。
        let width = doc.width;
        let height = doc.height;
        if let Some(cel) = doc.cel_mut("L1", "F0") {
            for y in 0..height {
                for x in 0..width {
                    cel.set(width, x, y, index);
                }
            }
        }
        doc.layers[1].visible = false;

        let attachment = super::frame_reference(&doc, "F0").unwrap();
        assert_eq!(pixel_of(&attachment, 0, 0)[3], 0);
        assert_eq!(pixel_of(&attachment, 1, 1), [255, 0, 0, 255]);
    }

    // ---------- 传输路由：垫图绝不静默丢失 ----------

    fn mock_config(base_url: &str) -> ModelConfig {
        ModelConfig {
            id: "m".into(),
            label: "m".into(),
            protocol: Protocol::OpenAiCompat,
            base_url: base_url.into(),
            api_key: "key".into(),
            model: "mock-model".into(),
            max_tokens: None,
            temperature: None,
            disable_thinking: None,
            capabilities: Default::default(),
        }
    }

    /// 一次性本地 HTTP 服务器：按路径分发响应，并把每个请求原样记下来。
    /// 线程随测试进程退出；连接逐条处理，respond 完即关。
    struct Recorder {
        addr: String,
        requests: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    }

    impl Recorder {
        fn serve<F>(handler: F) -> Self
        where
            F: Fn(&str) -> (u16, String) + Send + 'static,
        {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let log = std::sync::Arc::clone(&requests);
            std::thread::spawn(move || {
                use std::io::Write;
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let request = read_request(&mut stream);
                    log.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(request.clone());
                    let text = String::from_utf8_lossy(&request).to_string();
                    let path = text
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("")
                        .to_string();
                    let (status, body) = handler(&path);
                    let response = format!(
                        "HTTP/1.1 {status} \r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }
            });
            Recorder {
                addr: format!("http://{addr}"),
                requests,
            }
        }

        fn base_url(&self) -> String {
            format!("{}/v1", self.addr)
        }

        fn count(&self) -> usize {
            self.requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len()
        }

        fn text_of(&self, index: usize) -> String {
            let requests = self
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            String::from_utf8_lossy(&requests[index]).to_string()
        }

        fn raw_of(&self, index: usize) -> Vec<u8> {
            self.requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)[index]
                .clone()
        }

        fn path_of(&self, index: usize) -> String {
            self.text_of(index)
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default()
                .to_string()
        }
    }

    /// 把一个请求连同 body 完整读出。multipart 的二进制部分也一并进 Vec<u8>。
    fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
        use std::io::Read;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break buf.len();
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let headers = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while buf.len() < header_end + content_length {
            let n = stream.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        buf
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    /// chat modalities 的回图响应：各家字段名不统一，这里用 Gemini 形状。
    fn chat_image_response() -> String {
        json!({"choices": [{"message": {
            "content": "mock image",
            "images": [{"image_url": {"url": "data:image/png;base64,AAAA"}}],
        }}]})
        .to_string()
    }

    #[tokio::test]
    async fn a_prompt_complaint_proves_the_image_endpoint_exists() {
        // 空 prompt 打过去，端点因为缺参数回 400：这就是「能出图」的凭据。
        let server = Recorder::serve(|path| match path {
            "/v1/images/generations" => (
                400,
                r#"{"error":{"message":"Missing required parameter: 'prompt'."}}"#.into(),
            ),
            _ => (404, "\"nope\"".into()),
        });
        let support = super::probe_image_support(&mock_config(&server.base_url())).await;
        assert_eq!(
            support,
            ImageSupport::Yes {
                transport: "images"
            },
            "端点抱怨缺 prompt 就是认了这个端点"
        );
        // 只发一次：探测不该继续往 chat 兜底，那段会真的发一次对话。
        assert_eq!(server.count(), 1);
        assert!(
            !server.text_of(0).to_lowercase().contains("\"prompt\""),
            "探测不能带真 prompt，否则会真的画一张图"
        );
    }

    #[tokio::test]
    async fn a_missing_image_endpoint_falls_back_to_chat_modalities() {
        // images 端点不在，chat 带 modalities 回了 images[]：Gemini 图像模型正是这个形状。
        let server = Recorder::serve(|path| match path {
            "/v1/chat/completions" => (200, chat_image_response()),
            _ => (404, "\"nope\"".into()),
        });
        let support = super::probe_image_support(&mock_config(&server.base_url())).await;
        assert_eq!(
            support,
            ImageSupport::Yes {
                transport: "chat_modalities"
            }
        );
        assert_eq!(server.path_of(0), "/v1/images/generations");
        assert_eq!(server.path_of(1), "/v1/chat/completions");
    }

    #[tokio::test]
    async fn a_text_only_model_is_reported_as_unsupported_not_unknown() {
        // chat 回了纯文字：确定这个模型不出图，不是探测失败。
        let server = Recorder::serve(|path| match path {
            "/v1/chat/completions" => (
                200,
                json!({"choices": [{"message": {"content": "pong"}}]}).to_string(),
            ),
            _ => (404, "\"nope\"".into()),
        });
        let support = super::probe_image_support(&mock_config(&server.base_url())).await;
        assert_eq!(
            support,
            ImageSupport::No {
                reason: "text_only"
            }
        );
    }

    #[tokio::test]
    async fn neither_endpoint_means_no_image_support() {
        // 两个端点都不在：这个 Base URL 没有生图这回事，结论是「不能」而不是「不知道」。
        let server = Recorder::serve(|_| (404, "\"nope\"".into()));
        let support = super::probe_image_support(&mock_config(&server.base_url())).await;
        assert_eq!(
            support,
            ImageSupport::No {
                reason: "no_endpoint"
            }
        );
    }

    #[tokio::test]
    async fn a_rejected_key_is_unknown_not_unsupported() {
        // 403 是鉴权问题，端点本身可能好得很：这种只能算不知道。
        let server = Recorder::serve(|_| {
            (
                403,
                r#"{"error":{"message":"model is not available in the current token plan"}}"#
                    .into(),
            )
        });
        let support = super::probe_image_support(&mock_config(&server.base_url())).await;
        assert!(
            matches!(support, ImageSupport::Unknown { .. }),
            "403 不能反推出端点不出图: {support:?}"
        );
    }

    #[tokio::test]
    async fn the_anthropic_protocol_is_reported_as_unsupported() {
        let mut config = mock_config("https://example.invalid/v1");
        config.protocol = Protocol::Anthropic;
        let support = super::probe_image_support(&config).await;
        assert_eq!(support, ImageSupport::No { reason: "protocol" });
    }

    #[tokio::test]
    async fn a_probe_without_a_model_cannot_decide() {
        let mut config = mock_config("https://example.invalid/v1");
        config.model = String::new();
        let support = super::probe_image_support(&config).await;
        assert_eq!(
            support,
            ImageSupport::Unknown {
                reason: "no_model".into()
            }
        );
    }

    #[tokio::test]
    async fn reference_travels_as_multipart_to_images_edits() {
        // 有 images/edits 就走它：垫图真字节进 multipart，transport 记 images_edits。
        let server = Recorder::serve(|path| match path {
            "/v1/images/edits" => (
                200,
                json!({"data": [{"b64_json": "AAAA", "media_type": "image/png"}]}).to_string(),
            ),
            _ => (404, "\"nope\"".into()),
        });
        let doc = red_corner_doc();
        let reference = super::frame_reference(&doc, "F0").unwrap();
        let gen = build_image_generator(&mock_config(&server.base_url()));
        let img = gen
            .generate(&ImageGenParams {
                prompt: "restyle this frame".into(),
                size: None,
                reference: Some(reference),
            })
            .await
            .expect("images/edits answers");
        assert_eq!(img.transport, "images_edits");
        assert_eq!(img.bytes, vec![0, 0, 0]);
        let request = server.text_of(server.count() - 1);
        // 字段名与 prompt 在 multipart 里是明文的；文件名后缀跟 media type 走。
        assert!(request.contains("name=\"image\""), "{request}");
        assert!(request.contains("filename=\"reference.png\""), "{request}");
        assert!(request.contains("restyle this frame"), "{request}");
        // PNG 签名必须原样出现在 body 里：证明发的是真字节而不是引用或描述。
        let raw = server.raw_of(server.count() - 1);
        let signature = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        assert!(
            raw.windows(signature.len()).any(|w| w == signature),
            "raw PNG bytes must reach the endpoint"
        );
    }

    #[tokio::test]
    async fn generations_that_ignore_references_are_skipped_when_a_reference_is_given() {
        // 兼容服务器最常见的形状：images/generations 好使，images/edits 没有。
        // 带垫图时绝不能图省事走 generations——那正是静默丢垫图的老路。
        // 必须绕去 chat modalities，让模型真的看见那一帧。
        let server = Recorder::serve(|path| match path {
            "/v1/images/generations" => (200, json!({"data": [{"b64_json": "Zm9v"}]}).to_string()),
            "/v1/chat/completions" => (200, chat_image_response()),
            _ => (404, "\"nope\"".into()),
        });
        let reference = super::frame_reference(&red_corner_doc(), "F0").unwrap();
        let gen = build_image_generator(&mock_config(&server.base_url()));
        let img = gen
            .generate(&ImageGenParams {
                prompt: "swap the red for green".into(),
                size: None,
                reference: Some(reference.clone()),
            })
            .await
            .expect("reference lands on chat modalities");
        assert_eq!(img.transport, "chat_modalities");
        let paths: Vec<String> = (0..server.count()).map(|i| server.path_of(i)).collect();
        assert!(
            !paths.iter().any(|p| p.contains("images/generations")),
            "a reference must never reach the endpoint that drops it: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.contains("images/edits")),
            "edits is tried first when it might exist: {paths:?}"
        );
        let chat_at = paths
            .iter()
            .position(|p| p.contains("chat/completions"))
            .expect("chat modalities takes over");
        let chat = server.text_of(chat_at);
        assert!(chat.contains("image_url"), "{chat}");
        assert!(chat.contains(&reference.data_base64), "{chat}");
    }

    #[tokio::test]
    async fn chat_modalities_is_the_last_reference_carrying_resort() {
        // edits 与 generations 都不在：垫图随 chat modalities 发出，仍然不丢。
        let server = Recorder::serve(|path| match path {
            "/v1/chat/completions" => (200, chat_image_response()),
            _ => (404, "\"nope\"".into()),
        });
        let reference = super::frame_reference(&red_corner_doc(), "F0").unwrap();
        let gen = build_image_generator(&mock_config(&server.base_url()));
        let img = gen
            .generate(&ImageGenParams {
                prompt: "shift the red block left".into(),
                size: None,
                reference: Some(reference.clone()),
            })
            .await
            .expect("chat modalities answers");
        assert_eq!(img.transport, "chat_modalities");
        let last = server.text_of(server.count() - 1);
        assert!(last.contains(&reference.data_base64), "{last}");
    }

    #[tokio::test]
    async fn no_reference_still_prefers_images_generations() {
        // 没垫图的请求保持老路：generations 先试，通了就不再用 chat，
        // 不少用户的 images/generations 是主力端点。
        let server = Recorder::serve(|path| match path {
            "/v1/images/generations" => (
                200,
                json!({"data": [{"b64_json": "AAAA", "revised_prompt": "brighter"}]}).to_string(),
            ),
            _ => (404, "\"nope\"".into()),
        });
        let gen = build_image_generator(&mock_config(&server.base_url()));
        let img = gen
            .generate(&params("a lone tree"))
            .await
            .expect("generations answers");
        assert_eq!(img.transport, "images");
        assert_eq!(img.note, "brighter");
        assert_eq!(
            server.count(),
            1,
            "no fallback when the first endpoint answers"
        );
        assert!(server.path_of(0).contains("images/generations"));
    }
}
