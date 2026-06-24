use async_stream::stream;
use futures::StreamExt;
use reqwest::Client;
use std::time::Instant;
use uuid::Uuid;

use crate::usage::TokenUsageBuilder;

use super::models::*;

/// Provider abstraction for different LLM backends
#[derive(Clone)]
pub struct LLMProvider {
    client: Client,
    api_key: String,
    base_url: String,
}

impl LLMProvider {
    pub fn new(client: Client, api_key: String, base_url: String) -> Self {
        Self {
            client,
            api_key,
            base_url,
        }
    }

    pub fn from_env(client: Client) -> Self {
        let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".into());
        Self::new(client, api_key, base_url)
    }

    /// Make a streaming chat completion request and return a stream of chunks
    pub async fn chat_completion_stream(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChunkStream, ProviderError> {
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

                // Process complete SSE lines from buffer
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
                            Ok(chunk) => yield Ok(chunk),
                            Err(e) => {
                                yield Err(ProviderError::Parse(format!(
                                    "Failed to parse chunk: {} — raw: {}",
                                    e, data
                                )));
                                return;
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(chunk_stream))
    }

    /// Make a chat completion request and return response with full usage details
    pub async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<CompletionResult, ProviderError> {
        let start = Instant::now();

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

        Ok(CompletionResult {
            content,
            finish_reason,
            usage,
            latency_ms,
            provider: "openai".to_string(),
            model: request.model.clone(),
        })
    }
}

/// Result from a completion call
#[derive(Debug, Clone)]
pub struct CompletionResult {
    pub content: String,
    pub finish_reason: Option<String>,
    pub usage: CompletionUsage,
    pub latency_ms: i32,
    pub provider: String,
    pub model: String,
}

impl CompletionResult {
    /// Convert to CreateTokenUsage for database storage
    pub fn to_token_usage(
        &self,
        user_id: Uuid,
        operation_type: &str,
        request_id: Option<&str>,
        session_id: Option<&str>,
        agent_id: Option<Uuid>,
    ) -> crate::usage::CreateTokenUsage {
        let mut builder =
            TokenUsageBuilder::new(user_id, operation_type, &self.provider, &self.model)
                .tokens(self.usage.prompt_tokens, self.usage.completion_tokens)
                .latency_ms(self.latency_ms);

        if let Some(ref details) = self.usage.prompt_tokens_details {
            if let Some(cached) = details.cached_tokens {
                builder = builder.cached_tokens(cached);
            }
            if let Some(audio) = details.audio_tokens {
                builder = builder.audio_tokens(audio);
            }
        }

        if let Some(ref details) = self.usage.completion_tokens_details {
            if let Some(reasoning) = details.reasoning_tokens {
                builder = builder.reasoning_tokens(reasoning);
            }
            if let Some(accepted) = details.accepted_prediction_tokens
                && let Some(rejected) = details.rejected_prediction_tokens {
                    builder = builder.predicted_tokens(accepted, rejected);
                }
        }

        if let Some(reason) = &self.finish_reason {
            builder = builder.finish_reason(reason.clone());
        }

        if let Some(rid) = request_id {
            builder = builder.request_id(rid);
        }
        if let Some(sid) = session_id {
            builder = builder.session_id(sid);
        }
        if let Some(aid) = agent_id {
            builder = builder.agent_id(aid);
        }

        builder.build()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("API error (HTTP {status}): {body}")]
    Api { status: u16, body: String },
    #[error("Parse error: {0}")]
    Parse(String),
}
