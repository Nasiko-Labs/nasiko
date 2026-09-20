//! Azure OpenAI provider — deployment-style OpenAI API
//! (`https://{resource}.openai.azure.com/openai/deployments/{deployment}/…`).
//!
//! Auth uses the Azure `api-key` header (not Bearer). The resolved `model` is the
//! **deployment name**. `api-version` is appended as a query param
//! (`AZURE_OPENAI_API_VERSION`, default `2024-10-21`).

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::json;

use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError};
use crate::ir::{ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse};
use crate::resolver::ResolvedConfig;

pub struct AzureOpenAiProvider {
    http: reqwest::Client,
    /// Resource root, e.g. `https://my-resource.openai.azure.com` (no trailing path).
    base: String,
    /// Azure OpenAI `api-version` query value.
    api_version: String,
}

impl AzureOpenAiProvider {
    pub fn new(http: reqwest::Client, base: String, api_version: String) -> Self {
        Self {
            http,
            base,
            api_version,
        }
    }

    fn status_error(status: reqwest::StatusCode, body: String) -> ProviderError {
        ProviderError::Status {
            status: status.as_u16(),
            message: body,
            retryable: status.as_u16() == 429 || status.is_server_error(),
        }
    }

    fn deployment_url(&self, deployment: &str, path_tail: &str) -> String {
        let root = self.base.trim_end_matches('/');
        format!(
            "{root}/openai/deployments/{deployment}/{path_tail}?api-version={}",
            self.api_version
        )
    }

    fn post(&self, url: &str, api_key: &str) -> reqwest::RequestBuilder {
        self.http.post(url).header("api-key", api_key)
    }
}

#[async_trait]
impl ProviderClient for AzureOpenAiProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        let mut out = req.clone();
        // Deployment is in the URL; Azure still accepts `model` in the body for some
        // API versions — keep the resolved deployment id for consistency.
        out.model = Some(cfg.model.clone());
        out.temperature = cfg.temperature.or(req.temperature);
        out.max_tokens = None;
        if let Some(mt) = cfg.max_tokens.or(req.max_tokens) {
            out.extra
                .insert("max_completion_tokens".to_string(), json!(mt));
        }
        out.stream = Some(false);

        let url = self.deployment_url(&cfg.model, "chat/completions");
        let resp = self
            .post(&url, &cfg.api_key)
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
        out.max_tokens = None;
        if let Some(mt) = cfg.max_tokens.or(req.max_tokens) {
            out.extra
                .insert("max_completion_tokens".to_string(), json!(mt));
        }
        out.stream = Some(true);
        out.extra.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );

        let url = self.deployment_url(&cfg.model, "chat/completions");
        let resp = self
            .post(&url, &cfg.api_key)
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

        let url = self.deployment_url(&cfg.model, "embeddings");
        let resp = self
            .post(&url, &cfg.api_key)
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

    fn resolved(deployment: &str) -> ResolvedConfig {
        ResolvedConfig {
            provider: "azure".into(),
            model: deployment.into(),
            litellm_model: format!("azure/{deployment}"),
            api_key: "azure-test-key".into(),
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

    fn provider(base: String) -> AzureOpenAiProvider {
        AzureOpenAiProvider::new(reqwest::Client::new(), base, "2024-10-21".into())
    }

    #[tokio::test]
    async fn chat_uses_deployment_path_and_api_key_header() {
        let mut server = mockito::Server::new_async().await;
        let provider_body = json!({
            "id": "chatcmpl-azure-1",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "hello from azure" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8 }
        });
        let m = server
            .mock(
                "POST",
                mockito::Matcher::Regex(
                    r"^/openai/deployments/my-gpt4o/chat/completions\?api-version=2024-10-21$"
                        .into(),
                ),
            )
            .match_header("api-key", "azure-test-key")
            .match_body(mockito::Matcher::PartialJson(json!({
                "model": "my-gpt4o",
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
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();
        let resp = provider.chat(&req, &resolved("my-gpt4o")).await.unwrap();

        m.assert_async().await;
        assert_eq!(resp.model, "my-gpt4o");
        assert_eq!(
            resp.choices[0].message.text().as_deref(),
            Some("hello from azure")
        );
    }

    #[tokio::test]
    async fn chat_stream_yields_delta_chunks() {
        let mut server = mockito::Server::new_async().await;
        let sse = concat!(
            "data: {\"id\":\"s1\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"s1\",\"object\":\"chat.completion.chunk\",\"model\":\"gpt-4o\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        let m = server
            .mock(
                "POST",
                mockito::Matcher::Regex(
                    r"^/openai/deployments/my-gpt4o/chat/completions\?api-version=2024-10-21$"
                        .into(),
                ),
            )
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
            .chat_stream(&req, &resolved("my-gpt4o"))
            .await
            .unwrap();

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.model, "my-gpt4o");
        assert_eq!(first.choices[0].delta.content.as_deref(), Some("Hi"));
        assert!(stream.next().await.is_some());
        assert!(stream.next().await.is_none());
        m.assert_async().await;
    }

    #[tokio::test]
    async fn server_error_is_retryable() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock(
                "POST",
                mockito::Matcher::Regex(
                    r"^/openai/deployments/my-gpt4o/chat/completions\?api-version=2024-10-21$"
                        .into(),
                ),
            )
            .with_status(503)
            .with_body("overloaded")
            .create_async()
            .await;
        let provider = provider(server.url());
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let err = provider.chat(&req, &resolved("my-gpt4o")).await.unwrap_err();
        assert!(matches!(
            err,
            ProviderError::Status {
                retryable: true,
                ..
            }
        ));
    }
}
