//! Groq provider — OpenAI Chat Completions–compatible wire format
//! (`https://api.groq.com/openai/v1`), so this is ≈passthrough like
//! [`super::openai::OpenAiProvider`] / [`super::openrouter::OpenRouterProvider`].
//!
//! Groq has no embeddings API; [`GroqProvider::embeddings`] returns 501
//! (same posture as Anthropic). Model ids are Groq's bare names
//! (e.g. `llama-3.3-70b-versatile`, `openai/gpt-oss-120b`).

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::json;

use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError};
use crate::ir::{ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse};
use crate::resolver::ResolvedConfig;

pub struct GroqProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://api.groq.com/openai/v1` (overridable for tests).
    base: String,
}

impl GroqProvider {
    pub fn new(http: reqwest::Client, base: String) -> Self {
        Self { http, base }
    }

    /// 429 and 5xx are retryable (transient); other 4xx are request-shape errors.
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
impl ProviderClient for GroqProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        let mut out = req.clone();
        out.model = Some(cfg.model.clone()); // C4: resolved model is authoritative
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
        out.extra.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );

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
        _req: &EmbeddingsRequest,
        _cfg: &ResolvedConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        Err(ProviderError::Status {
            status: 501,
            message: "Groq has no embeddings API".to_string(),
            retryable: false,
        })
    }

    /// Groq speaks OpenAI's error envelope for unsupported params.
    fn droppable_param(&self, err: &ProviderError) -> Option<String> {
        let ProviderError::Status {
            status, message, ..
        } = err
        else {
            return None;
        };
        if *status != 400 {
            return None;
        }
        let body: serde_json::Value = serde_json::from_str(message).ok()?;
        let error = body.get("error")?;
        let code = error
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap_or_default();
        let param = error.get("param").and_then(|p| p.as_str())?;
        let droppable = matches!(code, "unsupported_value" | "unsupported_parameter")
            || (code == "invalid_value" && matches!(param, "max_tokens" | "max_completion_tokens"));
        if !droppable {
            return None;
        }
        Some(param.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;

    fn resolved(model: &str, temperature: Option<f64>) -> ResolvedConfig {
        ResolvedConfig {
            provider: "groq".into(),
            model: model.into(),
            litellm_model: format!("groq/{model}"),
            api_key: "gsk-test".into(),
            fallback_models: vec![],
            temperature,
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

    fn provider(base: String) -> GroqProvider {
        GroqProvider::new(reqwest::Client::new(), base)
    }

    #[tokio::test]
    async fn chat_overrides_model_and_reports_bare_id() {
        let mut server = mockito::Server::new_async().await;
        let provider_body = json!({
            "id": "chatcmpl-groq-1",
            "object": "chat.completion",
            "model": "llama-3.3-70b-versatile",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "hello from groq" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8 }
        });
        let m = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "llama-3.3-70b-versatile",
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
            .chat(&req, &resolved("llama-3.3-70b-versatile", Some(0.2)))
            .await
            .unwrap();

        m.assert_async().await;
        assert_eq!(resp.model, "llama-3.3-70b-versatile");
        assert_eq!(
            resp.choices[0].message.text().as_deref(),
            Some("hello from groq")
        );
        assert_eq!(resp.usage.unwrap().total_tokens, Some(8));
    }

    #[tokio::test]
    async fn chat_stream_yields_delta_chunks() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"id\":\"s1\",\"object\":\"chat.completion.chunk\",\"model\":\"llama-3.3-70b-versatile\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"s1\",\"object\":\"chat.completion.chunk\",\"model\":\"llama-3.3-70b-versatile\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
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
            .chat_stream(&req, &resolved("llama-3.3-70b-versatile", None))
            .await
            .unwrap();

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.model, "llama-3.3-70b-versatile");
        assert_eq!(first.choices[0].delta.content.as_deref(), Some("Hi"));
        let last = stream.next().await.unwrap().unwrap();
        assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));
        assert!(stream.next().await.is_none());
        m.assert_async().await;
    }

    #[tokio::test]
    async fn embeddings_are_not_supported() {
        let provider = provider("http://x".into());
        let req: EmbeddingsRequest =
            serde_json::from_value(json!({ "model": "x", "input": "hi" })).unwrap();
        let err = provider
            .embeddings(&req, &resolved("llama-3.3-70b-versatile", None))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ProviderError::Status {
                status: 501,
                retryable: false,
                ..
            }
        ));
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
            .chat(&req, &resolved("llama-3.3-70b-versatile", None))
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

    #[tokio::test]
    async fn client_error_is_not_retryable() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/chat/completions")
            .with_status(400)
            .with_body("bad request")
            .create_async()
            .await;
        let provider = provider(server.url());
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let err = provider
            .chat(&req, &resolved("llama-3.3-70b-versatile", None))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ProviderError::Status {
                retryable: false,
                status: 400,
                ..
            }
        ));
    }

    #[test]
    fn droppable_param_extracts_offending_field() {
        let provider = provider("http://x".into());
        let unsupported = ProviderError::Status {
            status: 400,
            message: json!({
                "error": { "message": "bad", "param": "temperature", "code": "unsupported_value" }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(
            provider.droppable_param(&unsupported).as_deref(),
            Some("temperature")
        );
        assert_eq!(
            provider.droppable_param(&ProviderError::Transport("x".into())),
            None
        );
    }
}
