// Copyright (C) 2026 Mutantcat Working Group
// SPDX-License-Identifier: GPL-3.0-only
//! 出网请求的公共护栏：总时限、body 上限、按上限读流。
//!
//! 三条规矩在每一条出网链路上都必须成立，缺一条的症状都不是报错，而是
//! 「转圈永远不停」——端点接上连接之后慢慢送、或者干脆再也不送，前端等的
//! 是一个永远不会 resolve 的 Future。reqwest 的 `.send()` 不带总时限，
//! `.json()` / `.text()` / `.bytes()` 也都不带上限：故障端点吐几个 G 的
//! 无用数据就能把进程内存吃穿。
//!
//! 这些护栏原先散落各处：一次性请求路径有一份，生图、模型列表、MCP 三条
//! 路径一份都没有。后果是「设置里点获取模型卡住不动」「探测能力转圈到
//! 天荒地老」，而同一条链路上的聊天一直好端端的——用户只会当成软件不稳。
//! 收在一处，四处共用。

use std::time::Duration;

use futures_util::StreamExt;
use tokio::time::timeout;

use super::providers::ProviderError;

/// 「结论很短」的请求的总时限：读参考图、读视频简报、提示词微调、模型列表。
pub const ONESHOT_TIMEOUT: Duration = Duration::from_secs(90);

/// 生图请求的总时限。出图比聊天慢得多，但再慢也有个尽头。
pub const IMAGE_TIMEOUT: Duration = Duration::from_secs(180);

/// 非二进制 body 的上限（含错误报文）。读到头也就够了，再多是端点在耍流氓。
pub const TEXT_BODY_CAP: usize = 256 * 1024;

/// MCP 请求的总时限相对宽松：工具服务器可能是用户自己起的慢脚本。
pub const MCP_TIMEOUT: Duration = Duration::from_secs(120);

/// MCP body 的上限。工具回执里可能有整段文本，但要有个头。
pub const MCP_BODY_CAP: usize = 8 * 1024 * 1024;

/// MCP 会话收尾的上限。DELETE 会话纯属礼貌性动作，答不答都不影响结果，
/// 等满 120s 只会让界面看起来卡死。
pub const MCP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// 发车：先过总时限。
///
/// 连接超时只在 TCP 阶段兜底；响应头回来之后端点照样能把连接挂住慢慢送，
/// 所以这里再收一道口。超时按网络故障交回，调用方照实报错——用户看到的是
/// 「没成」，而不是一个再也不动的转圈。
pub(crate) async fn send(
    req: reqwest::RequestBuilder,
    limit: Duration,
) -> Result<reqwest::Response, ProviderError> {
    match timeout(limit, req.send()).await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(ProviderError::Network(e.to_string())),
        Err(_) => Err(ProviderError::Network(stalled(limit))),
    }
}

/// `read_capped` 交回的失败，把「撞了上限」和「流本身出错」分开摆。
///
/// 原先靠嗅探报错文案里的 "exceeded" 区分，脆在两头：调用方一旦按自己的对象
/// 改写措辞（生图要说位图上限、MCP 要说回执上限），判定就跟着失效，而失效是
/// 静默的——用户只看到一条笼统的读取失败，看不出真正原因是超限。
#[derive(Debug)]
pub(crate) enum BodyFailure {
    /// 读满了上限。带上限和实际读到的量：只说「超限」不说差多少，排查时没用。
    TooLarge { cap: usize, seen: usize },
    /// 流本身出错，或读超时。
    Failed(ProviderError),
}

impl BodyFailure {
    /// 是不是撞了上限。调用方据此换成自己的措辞。
    pub(crate) fn too_large(&self) -> bool {
        matches!(self, BodyFailure::TooLarge { .. })
    }

    /// 折回 `ProviderError`：撞上限也按解码失败交回，上层好按可重试错误对待。
    pub(crate) fn into_error(self) -> ProviderError {
        match self {
            BodyFailure::TooLarge { cap, seen } => ProviderError::Decode(format!(
                "response body exceeded {cap} bytes (read {seen}), refusing to buffer it"
            )),
            BodyFailure::Failed(e) => e,
        }
    }
}

/// 按上限读整个 body，一边读一边对上限。
///
/// 先整段读进来再量长度等于没限：故障端点吐几个 G，内存先被吃穿，然后才被
/// 告知「太大了」。读满即停，流式读取本身不占额外内存。
pub(crate) async fn read_capped(
    resp: reqwest::Response,
    cap: usize,
    limit: Duration,
) -> Result<Vec<u8>, BodyFailure> {
    let read = async {
        let mut stream = resp.bytes_stream();
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    if buf.len() >= cap {
                        return Err(BodyFailure::TooLarge {
                            cap,
                            seen: buf.len(),
                        });
                    }
                }
                Err(e) => return Err(BodyFailure::Failed(ProviderError::Decode(e.to_string()))),
            }
        }
        Ok(buf)
    };
    match timeout(limit, read).await {
        Ok(Ok(buf)) => Ok(buf),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(BodyFailure::Failed(ProviderError::Network(stalled(limit)))),
    }
}

/// 读 JSON body。响应头回来之后端点照样能把连接挂住，body 阶段没有时限的话
/// 侧道任务会静默死掉。
pub(crate) async fn read_json(
    resp: reqwest::Response,
    cap: usize,
    limit: Duration,
) -> Result<serde_json::Value, ProviderError> {
    let buf = read_capped(resp, cap, limit)
        .await
        .map_err(BodyFailure::into_error)?;
    serde_json::from_slice(&buf).map_err(|e| ProviderError::Decode(e.to_string()))
}

/// 读文本 body，按字符截断后交给界面。
pub(crate) async fn read_text(
    resp: reqwest::Response,
    cap: usize,
    limit: Duration,
    shown: usize,
) -> Result<String, ProviderError> {
    let buf = read_capped(resp, cap, limit)
        .await
        .map_err(BodyFailure::into_error)?;
    Ok(super::providers::truncate_chars(
        &String::from_utf8_lossy(&buf),
        shown,
    ))
}

pub(crate) fn stalled(limit: Duration) -> String {
    format!(
        "no response within {}s, the endpoint looks stalled",
        limit.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用一份内存里的响应体造 `reqwest::Response`，省得真起一个本地服务器。
    fn response(payload: Vec<u8>) -> reqwest::Response {
        reqwest::Response::from(
            http::Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(reqwest::Body::from(payload))
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn a_small_json_body_still_decodes() {
        let value = read_json(
            response(br#"{"choices":[{"message":{"content":"hi"}}]}"#.to_vec()),
            TEXT_BODY_CAP,
            ONESHOT_TIMEOUT,
        )
        .await
        .expect("json decodes");
        assert_eq!(
            value.pointer("/choices/0/message/content"),
            Some(&serde_json::json!("hi"))
        );
    }

    #[tokio::test]
    async fn an_empty_body_reports_decode_rather_than_panicking() {
        let err = read_json(response(Vec::new()), TEXT_BODY_CAP, ONESHOT_TIMEOUT)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Decode(_)), "{err}");
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused_instead_of_buffered() {
        // `.json()` 不设上限，512KB 的垃圾也能整段吃进内存；cap 是 256KB，
        // 读满即交回解码失败。
        let err = read_json(
            response(vec![b'x'; TEXT_BODY_CAP + 4096]),
            TEXT_BODY_CAP,
            ONESHOT_TIMEOUT,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ProviderError::Decode(_)), "{err}");
        assert!(err.to_string().contains("exceeded"), "{err}");
    }

    /// 撞上限必须和流故障分开摆。生图和 MCP 都要按自己的对象重写措辞，
    /// 一旦分不开，用户看到的就是一条笼统的读取失败，看不出真正原因是超限。
    #[tokio::test]
    async fn too_large_is_distinct_from_a_broken_stream() {
        let capped = read_capped(
            response(vec![b'x'; TEXT_BODY_CAP + 16]),
            TEXT_BODY_CAP,
            ONESHOT_TIMEOUT,
        )
        .await
        .expect_err("读满必须失败");
        assert!(capped.too_large(), "撞上限要能认出来");
        let shown = capped.into_error().to_string();
        // 上限和实际读了多少都写进去：只说「超限」不说差多少，排查时没用。
        assert!(shown.contains("exceeded"), "{shown}");
        assert!(shown.contains(&TEXT_BODY_CAP.to_string()), "{shown}");

        // 同一份上限下读一个正常的小 body，不该被误判成超限。
        let fine = read_capped(response(b"{}".to_vec()), TEXT_BODY_CAP, ONESHOT_TIMEOUT)
            .await
            .expect("小 body 照样读得到");
        assert_eq!(fine, b"{}".to_vec());
    }

    #[tokio::test]
    async fn text_is_truncated_by_characters_not_bytes() {
        // 按字节切会在汉字三个字节的中间下刀：Rust 直接 panic，挂掉的还是 async 任务。
        let body = "生图失败：套餐额度不足".repeat(200);
        let text = read_text(
            response(body.into_bytes()),
            TEXT_BODY_CAP,
            ONESHOT_TIMEOUT,
            400,
        )
        .await
        .expect("text reads");
        assert!(text.chars().count() <= 403, "{}", text.chars().count());
        assert!(text.ends_with("..."), "{text}");
    }
}
