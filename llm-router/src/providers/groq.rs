//! Groq provider — Groq serves an OpenAI-compatible Chat Completions API at
//! `https://api.groq.com/openai/v1`, so this is ≈passthrough (same shape as
//! [`super::openai::OpenAiProvider`]). Groq runs open-weight models on its LPU
//! hardware, so the draw is latency rather than a distinct wire format.
//!
//! Two Groq-specific behaviors are worth knowing:
//!
//! 1. **No embeddings.** Groq serves chat completions only — there is no
//!    `/embeddings` route. [`GroqProvider::embeddings`] therefore fails fast with a
//!    terminal (non-retryable) error rather than issuing a request that would 404.
//!    Callers that need embeddings should resolve to a provider that has them; the
//!    error names Groq explicitly so the reason is obvious in logs.
//! 2. **Narrower parameter surface than OpenAI.** Groq rejects several OpenAI
//!    parameters outright (`logprobs`, `logit_bias`, `top_logprobs`, and `n > 1`)
//!    with a 400 naming the offending field. [`GroqProvider::droppable_param`]
//!    recognizes that shape so the executor can retry the same model without the
//!    parameter instead of surfacing a hard failure.
//!
//! Model ids are bare, provider-native strings (e.g. `llama-3.3-70b-versatile`),
//! not the `vendor/model` form OpenRouter uses.

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
        // Groq honors OpenAI's `stream_options.include_usage`, emitting a final
        // usage-only chunk. Without it the stream carries no token counts and
        // usage logging records zeros.
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

    /// Groq serves no embeddings route. Fail fast and terminally rather than POSTing
    /// to a path that does not exist: a 404 from the upstream would be classified as a
    /// non-retryable status error anyway, but the message would be Groq's generic
    /// not-found body rather than something a caller can act on.
    async fn embeddings(
        &self,
        _req: &EmbeddingsRequest,
        _cfg: &ResolvedConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        Err(ProviderError::Status {
            status: 501,
            message: "Groq does not provide an embeddings API; resolve embeddings to a \
                      provider that supports them (openai, gemini)"
                .to_string(),
            retryable: false,
        })
    }

    /// Groq returns OpenAI-shaped 400s naming the rejected field in `error.param`.
    /// Recognizing them lets the executor retry the same model without that parameter
    /// instead of failing the call — Groq's parameter surface is narrower than
    /// OpenAI's (no `logprobs`, `logit_bias`, `top_logprobs`, or `n > 1`), so a request
    /// written against OpenAI routinely trips one of these.
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
        let droppable = matches!(
            code,
            "unsupported_value" | "unsupported_parameter" | "decommissioned_model"
        ) || (code == "invalid_value"
            && matches!(param, "max_tokens" | "max_completion_tokens"));
        if !droppable {
            return None;
        }
        Some(param.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn chat_overrides_model_and_reports_resolved_model() {
        let mut server = mockito::Server::new_async().await;
        let provider_body = json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "model": "llama-3.3-70b-versatile",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hello" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
        });
        let m = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "llama-3.3-70b-versatile", "temperature": 0.2, "stream": false
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
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("hello"));
        assert_eq!(resp.usage.unwrap().total_tokens, Some(7));
    }

    #[tokio::test]
    async fn chat_sends_bearer_auth() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/chat/completions")
            .match_header("authorization", "Bearer gsk-test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-1", "object": "chat.completion", "model": "llama-3.3-70b-versatile",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hi" }, "finish_reason": "stop" }]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = provider(server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "whatever",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        provider
            .chat(&req, &resolved("llama-3.3-70b-versatile", None))
            .await
            .unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn stream_text_then_finish_and_usage() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}\n\n",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"x\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"x\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"model\":\"x\",\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        );
        let m = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "stream": true, "stream_options": { "include_usage": true }
            })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse)
            .create_async()
            .await;

        let provider = provider(server.url());
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "whatever",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        let stream = provider
            .chat_stream(&req, &resolved("llama-3.3-70b-versatile", None))
            .await
            .unwrap();
        let chunks: Vec<_> = stream.collect().await;

        m.assert_async().await;
        // 3 content/finish chunks + 1 usage-only chunk, all tagged with the resolved model.
        assert_eq!(chunks.len(), 4);
        for c in &chunks {
            assert_eq!(c.as_ref().unwrap().model, "llama-3.3-70b-versatile");
        }
        let usage = chunks[3].as_ref().unwrap().usage.as_ref().unwrap();
        assert_eq!(usage.total_tokens, Some(5));
    }

    #[tokio::test]
    async fn rate_limit_is_retryable_but_bad_key_is_terminal() {
        let mut server = mockito::Server::new_async().await;
        let req: ChatRequest = serde_json::from_value(json!({
            "model": "whatever",
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();

        let m429 = server
            .mock("POST", "/chat/completions")
            .with_status(429)
            .with_body("{\"error\":{\"message\":\"rate limit\"}}")
            .create_async()
            .await;
        let err = provider(server.url())
            .chat(&req, &resolved("llama-3.3-70b-versatile", None))
            .await
            .unwrap_err();
        m429.assert_async().await;
        assert!(err.retryable(), "429 must be retryable");

        let m401 = server
            .mock("POST", "/chat/completions")
            .with_status(401)
            .with_body("{\"error\":{\"message\":\"invalid api key\"}}")
            .create_async()
            .await;
        let err = provider(server.url())
            .chat(&req, &resolved("llama-3.3-70b-versatile", None))
            .await
            .unwrap_err();
        m401.assert_async().await;
        assert!(!err.retryable(), "401 must be terminal, not retried");
    }

    #[tokio::test]
    async fn embeddings_fail_fast_without_calling_upstream() {
        let mut server = mockito::Server::new_async().await;
        // Any request to the server would be an unexpected call; mockito fails the
        // assertion below if `embeddings` issued one.
        let never = server
            .mock("POST", "/embeddings")
            .expect(0)
            .with_status(404)
            .create_async()
            .await;

        let req: EmbeddingsRequest = serde_json::from_value(json!({
            "model": "whatever",
            "input": "hello"
        }))
        .unwrap();
        let err = provider(server.url())
            .embeddings(&req, &resolved("llama-3.3-70b-versatile", None))
            .await
            .unwrap_err();

        never.assert_async().await;
        assert!(!err.retryable(), "unsupported capability is terminal");
        match err {
            ProviderError::Status { status, .. } => assert_eq!(status, 501),
            other => panic!("expected a status error, got {other:?}"),
        }
    }

    #[test]
    fn droppable_param_recognizes_groq_rejections() {
        let p = provider("http://unused".into());

        let unsupported = ProviderError::Status {
            status: 400,
            message: json!({
                "error": { "code": "unsupported_parameter", "param": "logprobs" }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(p.droppable_param(&unsupported).as_deref(), Some("logprobs"));

        // A genuine bad request is not a capability mismatch — do not retry-drop.
        let genuine = ProviderError::Status {
            status: 400,
            message: json!({
                "error": { "code": "invalid_request_error", "param": "messages" }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(p.droppable_param(&genuine), None);

        // Non-400s carry no droppable param.
        let auth = ProviderError::Status {
            status: 401,
            message: "{}".into(),
            retryable: false,
        };
        assert_eq!(p.droppable_param(&auth), None);
    }
}
