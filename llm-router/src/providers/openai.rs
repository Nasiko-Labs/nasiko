//! OpenAI provider — the simplest spoke: the agent already speaks OpenAI, so this is
//! ≈passthrough. We override the model with the resolved one (C4), apply the param
//! precedence (resolved wins when set, else the request's), force non-streaming on the
//! non-stream path, call the provider, and report the bare resolved model.
//!
//! Streaming is implemented in step 7; until then `chat_stream` returns an error.

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::json;

use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError};
use crate::ir::{ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse};
use crate::resolver::ResolvedConfig;

pub struct OpenAiProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://api.openai.com/v1` (overridable for tests).
    base: String,
}

impl OpenAiProvider {
    pub fn new(http: reqwest::Client, base: String) -> Self {
        Self { http, base }
    }

    /// Map a non-2xx provider response to a [`ProviderError`]. 429 and 5xx are
    /// retryable (transient); other 4xx are request-shape errors and are not.
    fn status_error(status: reqwest::StatusCode, body: String) -> ProviderError {
        ProviderError::Status {
            status: status.as_u16(),
            message: body,
            retryable: status.as_u16() == 429 || status.is_server_error(),
        }
    }
}

#[async_trait]
impl ProviderClient for OpenAiProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        let mut out = req.clone();
        out.model = Some(cfg.model.clone()); // C4: resolved model is authoritative
        out.temperature = cfg.temperature.or(req.temperature); // resolved wins when set
        // OpenAI deprecated `max_tokens`; newer models reject it. Emit the current
        // `max_completion_tokens` param instead (via passthrough; the named field is
        // cleared so it doesn't also serialize).
        out.max_tokens = None;
        if let Some(mt) = cfg.max_tokens.or(req.max_tokens) {
            out.extra
                .insert("max_completion_tokens".to_string(), json!(mt));
        }
        out.stream = Some(false);

        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base))
            .bearer_auth(&cfg.api_key)
            .json(&out)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::status_error(status, body));
        }

        let mut parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        parsed.model = cfg.model.clone(); // report the bare resolved model id
        Ok(parsed)
    }

    async fn chat_stream(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        let mut out = req.clone();
        out.model = Some(cfg.model.clone());
        out.temperature = cfg.temperature.or(req.temperature);
        // See `chat` — OpenAI wants `max_completion_tokens`, not the deprecated `max_tokens`.
        out.max_tokens = None;
        if let Some(mt) = cfg.max_tokens.or(req.max_tokens) {
            out.extra
                .insert("max_completion_tokens".to_string(), json!(mt));
        }
        out.stream = Some(true);
        // Ask OpenAI to emit a final usage chunk (off by default when streaming).
        out.extra
            .insert("stream_options".to_string(), json!({ "include_usage": true }));

        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base))
            .bearer_auth(&cfg.api_key)
            .json(&out)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::status_error(status, body));
        }

        // OpenAI chunks are already in IR shape; parse each, normalize the model.
        let model = cfg.model.clone();
        let data = sse_data_stream(resp.bytes_stream());
        let stream = async_stream::stream! {
            futures::pin_mut!(data);
            while let Some(item) = data.next().await {
                match item {
                    Err(e) => { yield Err(e); return; }
                    Ok(payload) => {
                        if payload.trim() == "[DONE]" {
                            break;
                        }
                        // Skip unparseable lines (comments/keep-alives) rather than failing.
                        if let Ok(mut chunk) = serde_json::from_str::<ChatChunk>(&payload) {
                            chunk.model = model.clone();
                            yield Ok(chunk);
                        }
                    }
                }
            }
        };
        Ok(Box::pin(stream))
    }

    async fn embeddings(
        &self,
        req: &EmbeddingsRequest,
        cfg: &ResolvedConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        let mut out = req.clone();
        out.model = Some(cfg.model.clone());

        let resp = self
            .http
            .post(format!("{}/embeddings", self.base))
            .bearer_auth(&cfg.api_key)
            .json(&out)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::status_error(status, body));
        }

        let mut parsed: EmbeddingsResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;
        parsed.model = cfg.model.clone();
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resolved(model: &str, temperature: Option<f64>) -> ResolvedConfig {
        ResolvedConfig {
            provider: "openai".into(),
            model: model.into(),
            litellm_model: format!("openai/{model}"),
            api_key: "sk-test".into(),
            fallback_models: vec![],
            temperature,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
        }
    }

    #[tokio::test]
    async fn chat_overrides_model_applies_precedence_and_reports_bare_model() {
        let mut server = mockito::Server::new_async().await;
        let provider_body = json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            // OpenAI may echo a dated model; we must report the bare resolved one.
            "model": "gpt-4o-mini-2024-07-18",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hello" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
        });
        let m = server
            .mock("POST", "/chat/completions")
            // outbound must carry the RESOLVED model (request's "gpt-4o" discarded),
            // the resolved temperature (0.2, not the request's 0.9), and stream:false.
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "gpt-4o-mini", "temperature": 0.2, "stream": false,
                // request's max_tokens must be emitted as max_completion_tokens
                "max_completion_tokens": 256
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(provider_body.to_string())
            .create_async()
            .await;

        let provider = OpenAiProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o",
            "messages": [{ "role": "user", "content": "hi" }],
            "temperature": 0.9,
            "max_tokens": 256
        }))
        .unwrap();
        let resp = provider.chat(&req, &resolved("gpt-4o-mini", Some(0.2))).await.unwrap();

        m.assert_async().await;
        assert_eq!(resp.model, "gpt-4o-mini"); // bare resolved id, not the echoed dated one
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("hello"));
        assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(resp.usage.unwrap().total_tokens, Some(7));
    }

    #[tokio::test]
    async fn embeddings_overrides_model_and_parses() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/embeddings")
            .match_body(mockito::Matcher::PartialJson(json!({ "model": "text-embedding-3-small" })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "object": "list",
                    "data": [{ "object": "embedding", "embedding": [0.5, 0.6], "index": 0 }],
                    "model": "text-embedding-3-small-v2",
                    "usage": { "prompt_tokens": 2, "total_tokens": 2 }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let provider = OpenAiProvider::new(reqwest::Client::new(), server.url());
        let req: crate::ir::EmbeddingsRequest =
            serde_json::from_value(json!({ "model": "whatever", "input": "hi" })).unwrap();
        let resp = provider.embeddings(&req, &resolved("text-embedding-3-small", None)).await.unwrap();
        assert_eq!(resp.model, "text-embedding-3-small"); // bare resolved
        assert_eq!(resp.data[0].embedding[0], 0.5);
    }

    #[tokio::test]
    async fn server_error_is_retryable() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(503)
            .with_body("overloaded")
            .create_async()
            .await;
        let provider = OpenAiProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let err = provider.chat(&req, &resolved("gpt-4o-mini", None)).await.unwrap_err();
        assert!(matches!(err, ProviderError::Status { retryable: true, .. }));
    }

    #[tokio::test]
    async fn chat_stream_parses_sse_chunks_and_overrides_model() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}\n\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\n\n",
            "data: [DONE]\n\n",
        );
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({ "stream": true, "stream_options": { "include_usage": true } })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let provider = OpenAiProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "gpt-4o", "stream": true,
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        let stream = provider.chat_stream(&req, &resolved("gpt-4o-mini", None)).await.unwrap();
        let chunks: Vec<ChatChunk> = stream.filter_map(|r| async { r.ok() }).collect().await;

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].model, "gpt-4o-mini"); // normalized to resolved
        assert_eq!(chunks[0].choices[0].delta.content.as_deref(), Some("Hel"));
        assert_eq!(chunks[1].choices[0].delta.content.as_deref(), Some("lo"));
        assert_eq!(chunks[2].choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(chunks[2].usage.as_ref().unwrap().total_tokens, Some(3));
    }

    #[tokio::test]
    async fn client_error_is_not_retryable() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(400)
            .with_body("bad request")
            .create_async()
            .await;
        let provider = OpenAiProvider::new(reqwest::Client::new(), server.url());
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let err = provider.chat(&req, &resolved("gpt-4o-mini", None)).await.unwrap_err();
        assert!(matches!(err, ProviderError::Status { retryable: false, status: 400, .. }));
    }
}
