use async_stream::stream;
use futures::StreamExt;
use reqwest::Client;
use std::time::Instant;

use crate::llm_backend::{LlmCallContext, SharedLlmBackend};
use crate::models::*;

/// Provider abstraction for different LLM backends.
#[derive(Clone)]
pub struct LLMProvider {
    client: Client,
    api_key: String,
    base_url: String,
    /// When set, non-streaming calls go through the in-process LLM router, which picks
    /// the provider and records usage.
    routed: Option<(SharedLlmBackend, LlmCallContext)>,
    /// Model sent on the direct OpenAI path when the requested one belongs to another
    /// provider (task models resolve from `DEFAULT_MODEL` before the backend is known).
    openai_model: String,
    /// True when `base_url` is `api.openai.com`; custom compatible endpoints keep the requested model.
    official_openai: bool,
}

// Shared with the chat-path orchestrator (`nasiko_react_agent`) so both normalize alike.
pub use nasiko_llm_router::openai_model::{
    OPENAI_CHAT_FALLBACK, direct_openai_fallback, direct_openai_model, is_foreign_model,
    is_official_openai, resolve_direct_chat_model,
};

/// OpenAI embedding model used when the configured one belongs to another provider.
pub const OPENAI_EMBEDDING_FALLBACK: &str = "text-embedding-3-small";

/// Embedding model for a direct (saved-key) call to `base_url`: official OpenAI gets an
/// OpenAI model (`EMBEDDING_MODEL=gemini-embedding-001` would 404 there); custom endpoints
/// keep `requested`. Callers pass the result to the vector store so cache keys match the
/// vectors actually produced.
pub fn direct_embedding_model(requested: &str, base_url: &str) -> String {
    if is_official_openai(base_url) && is_foreign_model(requested) {
        OPENAI_EMBEDDING_FALLBACK.to_owned()
    } else {
        requested.to_owned()
    }
}

impl LLMProvider {
    // `chat_completion`/`chat_completion_stream`
    /// hardcode a `/v1` segment; a caller-supplied `OPENAI_BASE_URL` that
    /// already ends in `/v1` (a common way to write it, and how
    /// typical deployment env files have it) would otherwise double up into
    /// `.../v1/v1/chat/completions`, which 404s.
    pub fn new(client: Client, api_key: String, base_url: String) -> Self {
        Self {
            client,
            api_key,
            official_openai: is_official_openai(&base_url),
            base_url: nasiko_config::openai_base_url_without_v1(&base_url).to_string(),
            routed: None,
            openai_model: direct_openai_fallback(),
        }
    }

    /// The request as sent to OpenAI directly, with a foreign model swapped out.
    fn direct_request(&self, request: &ChatCompletionRequest) -> ChatCompletionRequest {
        let mut req: ChatCompletionRequest = request.clone();
        if !self.official_openai {
            return req;
        }
        let model: String = direct_openai_model(&req.model, &self.openai_model);
        if model != req.model {
            tracing::info!(requested = %req.model, sent = %model, "non-OpenAI model on direct OpenAI path; substituted");
            req.model = model;
        }
        req
    }

    /// Send non-streaming calls through `backend` under `call`.
    pub fn with_backend(mut self, backend: SharedLlmBackend, call: LlmCallContext) -> Self {
        self.routed = Some((backend, call));
        self
    }

    /// True when calls go through the router (which then writes usage itself).
    pub fn is_routed(&self) -> bool {
        self.routed.is_some()
    }

    pub fn from_env(client: Client) -> Self {
        let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".into());
        Self::new(client, api_key, base_url)
    }

    /// The same endpoint with another key, for a caller who saved their own.
    pub fn with_api_key(mut self, api_key: String) -> Self {
        self.api_key = api_key;
        self
    }

    /// Whether there is any key to send. Without one every call is refused upstream, so a caller
    /// can say so before making it.
    pub fn has_api_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    /// Streaming chat completion — returns a pinned stream of parsed chunks.
    pub async fn chat_completion_stream(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChunkStream, ProviderError> {
        let request: &ChatCompletionRequest = &self.direct_request(request);
        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(request)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                body,
            });
        }

        let mut byte_stream = response.bytes_stream();

        let chunk_stream = stream! {
            let mut buffer = String::new();

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        yield Err(ProviderError::Network(e.to_string()));
                        return;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim_end_matches('\r').to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            return;
                        }

                        match serde_json::from_str::<ChatCompletionChunk>(data) {
                            Ok(chunk) => {
                                yield Ok(chunk);
                            }
                            Err(e) => {
                                yield Err(
                                    ProviderError::Parse(
                                        format!("failed to parse chunk: {} — raw: {}", e, data)
                                    )
                                );
                                return;
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(chunk_stream))
    }

    /// Non-streaming chat completion — returns full response with usage details.
    pub async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<CompletionResult, ProviderError> {
        let start = Instant::now();

        if let Some((backend, call)) = &self.routed {
            let body: serde_json::Value =
                serde_json::to_value(request).map_err(|e| ProviderError::Parse(e.to_string()))?;
            let reply: serde_json::Value =
                backend.chat(call, body).await.map_err(|e| match e.status {
                    Some(status) => ProviderError::Api {
                        status,
                        body: e.message,
                    },
                    None => ProviderError::Network(e.message),
                })?;
            let resp: ChatCompletionResponse =
                serde_json::from_value(reply).map_err(|e| ProviderError::Parse(e.to_string()))?;
            return Ok(completion_result(
                resp,
                start,
                backend.provider(),
                request.model.clone(),
            ));
        }

        let request: &ChatCompletionRequest = &self.direct_request(request);
        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(request)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status: status.as_u16(),
                body,
            });
        }

        let resp: ChatCompletionResponse = response
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        Ok(completion_result(
            resp,
            start,
            "openai".to_string(),
            request.model.clone(),
        ))
    }
}

fn completion_result(
    resp: ChatCompletionResponse,
    start: Instant,
    provider: String,
    model: String,
) -> CompletionResult {
    {
        let latency_ms = start.elapsed().as_millis() as i32;

        let content = resp
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();

        let finish_reason = resp.choices.first().and_then(|c| c.finish_reason.clone());

        let usage = resp.usage.clone().unwrap_or(CompletionUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        });

        CompletionResult {
            content,
            finish_reason,
            usage,
            latency_ms,
            provider,
            model,
        }
    }
}

/// Result from a non-streaming completion call.
#[derive(Debug, Clone)]
pub struct CompletionResult {
    pub content: String,
    pub finish_reason: Option<String>,
    pub usage: CompletionUsage,
    pub latency_ms: i32,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("network error: {0}")]
    Network(String),
    #[error("API error (HTTP {status}): {body}")]
    Api { status: u16, body: String },
    #[error("parse error: {0}")]
    Parse(String),
}

impl ProviderError {
    /// Whether the provider refused the key itself (`401`/`403`), as opposed to the request.
    pub fn is_rejected_key(&self) -> bool {
        matches!(
            self,
            Self::Api {
                status: 401 | 403,
                ..
            }
        )
    }
}

#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn only_official_openai_endpoint_swaps_models() {
        assert!(is_official_openai("https://api.openai.com/v1"));
        assert!(is_official_openai("https://API.openai.com"));
        assert!(!is_official_openai("https://api.deepseek.com/v1"));
        assert!(!is_official_openai("http://localhost:4000"));
        assert!(!is_official_openai(
            "https://api.openai.com.proxy.example/v1"
        ));
        assert!(!is_official_openai(
            "https://proxy.example/api.openai.com/v1"
        ));
    }

    #[test]
    fn openai_models_pass_through() {
        assert_eq!(direct_openai_model("gpt-6-luna", "x"), "gpt-6-luna");
        assert_eq!(direct_openai_model("o4-mini", "x"), "o4-mini");
    }

    #[test]
    fn foreign_models_are_swapped() {
        for m in [
            "gemini-2.5-flash",
            "claude-haiku-4-5",
            "openai/gpt-4o",
            "deepseek-v4-pro",
            "",
        ] {
            assert_eq!(direct_openai_model(m, "gpt-6-luna"), "gpt-6-luna", "{m}");
        }
    }
}
