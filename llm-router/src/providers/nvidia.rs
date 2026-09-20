//! NVIDIA NIM provider — OpenAI Chat Completions–compatible wire format
//! (`https://integrate.api.nvidia.com/v1`). Bearer auth with `nvapi-…` keys.
//!
//! Model ids are catalog names (e.g. `meta/llama-3.3-70b-instruct`,
//! `nvidia/llama-3.1-nemotron-nano-8b-v1`). Embeddings depend on the model;
//! unsupported models return provider 4xx (not a hard 501).

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError};
use crate::ir::{ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse};
use crate::resolver::ResolvedConfig;

pub struct NvidiaProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://integrate.api.nvidia.com/v1` (overridable for tests).
    base: String,
}

impl NvidiaProvider {
    pub fn new(http: reqwest::Client, base: String) -> Self {
        Self { http, base }
    }

    fn status_error(status: reqwest::StatusCode, body: String) -> ProviderError {
        ProviderError::Status {
            status: status.as_u16(),
            message: body,
            retryable: status.as_u16() == 429 || status.is_server_error(),
        }
    }

    fn post(&self, path: &str, api_key: &str) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}{path}", self.base))
            .bearer_auth(api_key)
    }
}

#[async_trait]
impl ProviderClient for NvidiaProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        let mut out = req.clone();
        out.model = Some(cfg.model.clone());
        out.temperature = cfg.temperature.or(req.temperature);
        if let Some(mt) = cfg.max_tokens.or(req.max_tokens) {
            out.max_tokens = Some(mt);
        }
        out.stream = Some(false);

        let resp = self
            .post("/chat/completions", &cfg.api_key)
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
        parsed.model = cfg.model.clone();
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
        if let Some(mt) = cfg.max_tokens.or(req.max_tokens) {
            out.max_tokens = Some(mt);
        }
        out.stream = Some(true);

        let resp = self
            .post("/chat/completions", &cfg.api_key)
            .json(&out)
            .send()
            .await
            .map_err(|e| ProviderError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::status_error(status, body));
        }

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
            .post("/embeddings", &cfg.api_key)
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
    use futures::StreamExt;
    use serde_json::json;

    fn resolved(model: &str) -> ResolvedConfig {
        ResolvedConfig {
            provider: "nvidia".into(),
            model: model.into(),
            litellm_model: format!("nvidia/{model}"),
            api_key: "nvapi-test".into(),
            fallback_models: vec![],
            temperature: Some(0.2),
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            is_coding_agent: false,
        }
    }

    fn provider(base: String) -> NvidiaProvider {
        NvidiaProvider::new(reqwest::Client::new(), base)
    }

    #[tokio::test]
    async fn chat_overrides_model_and_reports_bare_id() {
        let mut server = mockito::Server::new_async().await;
        let provider_body = json!({
            "id": "chatcmpl-nvidia-1",
            "object": "chat.completion",
            "model": "meta/llama-3.3-70b-instruct",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "hello from nim" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8 }
        });
        let m = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "meta/llama-3.3-70b-instruct",
                "temperature": 0.2,
                "stream": false
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(provider_body.to_string())
            .create_async()
            .await;

        let provider = provider(server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "whatever",
            "messages": [{ "role": "user", "content": "hi" }],
            "temperature": 0.9
        }))
        .unwrap();
        let resp = provider
            .chat(&req, &resolved("meta/llama-3.3-70b-instruct"))
            .await
            .unwrap();

        m.assert_async().await;
        assert_eq!(resp.model, "meta/llama-3.3-70b-instruct");
        assert_eq!(
            resp.choices[0].message.text().as_deref(),
            Some("hello from nim")
        );
    }

    #[tokio::test]
    async fn chat_stream_yields_delta_chunks() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"id\":\"s1\",\"object\":\"chat.completion.chunk\",\"model\":\"meta/llama-3.3-70b-instruct\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"s1\",\"object\":\"chat.completion.chunk\",\"model\":\"meta/llama-3.3-70b-instruct\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let m = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({ "stream": true })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let provider = provider(server.url());
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let mut stream = provider
            .chat_stream(&req, &resolved("meta/llama-3.3-70b-instruct"))
            .await
            .unwrap();

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.choices[0].delta.content.as_deref(), Some("Hi"));
        assert!(stream.next().await.is_some());
        assert!(stream.next().await.is_none());
        m.assert_async().await;
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
        let provider = provider(server.url());
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let err = provider
            .chat(&req, &resolved("meta/llama-3.3-70b-instruct"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ProviderError::Status {
                retryable: true,
                ..
            }
        ));
    }
}
