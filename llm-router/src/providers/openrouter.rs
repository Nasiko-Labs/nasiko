//! OpenRouter provider — wire-compatible with OpenAI's Chat Completions API, so this
//! is ≈passthrough (same shape as [`super::openai::OpenAiProvider`]). The only
//! OpenRouter-specific behavior is the optional `HTTP-Referer`/`X-Title` attribution
//! headers (app ranking on openrouter.ai; harmless to omit) and the model id, which is
//! a full `vendor/model` string (e.g. `anthropic/claude-sonnet-4.6`) rather than a bare
//! provider-native id.

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::json;

use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError};
use crate::ir::{ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse};
use crate::resolver::ResolvedConfig;

pub struct OpenRouterProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://openrouter.ai/api/v1` (overridable for tests).
    base: String,
    /// Optional `HTTP-Referer` attribution header. Empty ⇒ omitted.
    http_referer: String,
    /// Optional `X-Title` attribution header. Empty ⇒ omitted.
    x_title: String,
}

impl OpenRouterProvider {
    pub fn new(http: reqwest::Client, base: String, http_referer: String, x_title: String) -> Self {
        Self {
            http,
            base,
            http_referer,
            x_title,
        }
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
        let mut req = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(api_key);
        if !self.http_referer.is_empty() {
            req = req.header("HTTP-Referer", &self.http_referer);
        }
        if !self.x_title.is_empty() {
            req = req.header("X-Title", &self.x_title);
        }
        req
    }
}

#[async_trait]
impl ProviderClient for OpenRouterProvider {
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

    /// OpenRouter proxies many upstream vendors, so a rejected param can carry any of
    /// their error shapes. We recognize OpenAI's own shape (passed through verbatim by
    /// OpenRouter when the upstream is OpenAI-compatible) and otherwise decline —
    /// unrecognized shapes are not safe to guess at.
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
    use serde_json::json;

    fn resolved(model: &str, temperature: Option<f64>) -> ResolvedConfig {
        ResolvedConfig {
            provider: "openrouter".into(),
            model: model.into(),
            litellm_model: format!("openrouter/{model}"),
            api_key: "sk-or-test".into(),
            fallback_models: vec![],
            temperature,
            max_tokens: None,
            has_llm_config: false,
            pinned_model: None,
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
        }
    }

    fn provider(base: String) -> OpenRouterProvider {
        OpenRouterProvider::new(reqwest::Client::new(), base, String::new(), String::new())
    }

    #[tokio::test]
    async fn chat_overrides_model_and_reports_bare_vendor_model_string() {
        let mut server = mockito::Server::new_async().await;
        let provider_body = json!({
            "id": "gen-1",
            "object": "chat.completion",
            "model": "anthropic/claude-sonnet-4.6",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hello" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 }
        });
        let m = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "anthropic/claude-sonnet-4.6", "temperature": 0.2, "stream": false
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
            .chat(&req, &resolved("anthropic/claude-sonnet-4.6", Some(0.2)))
            .await
            .unwrap();

        m.assert_async().await;
        assert_eq!(resp.model, "anthropic/claude-sonnet-4.6");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("hello"));
        assert_eq!(resp.usage.unwrap().total_tokens, Some(7));
    }

    #[tokio::test]
    async fn sends_attribution_headers_when_configured() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/chat/completions")
            .match_header("http-referer", "https://nasiko.example")
            .match_header("x-title", "Nasiko")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "gen-1", "object": "chat.completion", "model": "openai/gpt-4o",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "hi" }, "finish_reason": "stop" }]
                })
                .to_string(),
            )
            .create_async()
            .await;

        let provider = OpenRouterProvider::new(
            reqwest::Client::new(),
            server.url(),
            "https://nasiko.example".into(),
            "Nasiko".into(),
        );
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        provider
            .chat(&req, &resolved("openai/gpt-4o", None))
            .await
            .unwrap();
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
            .chat(&req, &resolved("openai/gpt-4o", None))
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
            .chat(&req, &resolved("openai/gpt-4o", None))
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

    #[test]
    fn droppable_param_accepts_invalid_value_only_for_max_tokens() {
        let provider = provider("http://x".into());
        let too_large = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "max_tokens is too large: 32000",
                    "type": "invalid_request_error",
                    "param": "max_tokens",
                    "code": "invalid_value"
                }
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(
            provider.droppable_param(&too_large).as_deref(),
            Some("max_tokens")
        );

        let invalid_temperature = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {"code": "invalid_value", "param": "temperature"}
            })
            .to_string(),
            retryable: false,
        };
        assert_eq!(provider.droppable_param(&invalid_temperature), None);
    }
}
