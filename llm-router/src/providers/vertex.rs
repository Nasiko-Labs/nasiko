//! Vertex AI spoke: one ADC token, two wire formats picked by model id.
//!
//! - Bare Gemini ids (`gemini-3.8-flash`, `gemini-4-carbon`) go to the native
//!   `publishers/google/models/{m}:generateContent` path ([`GeminiProvider`]).
//! - Publisher-prefixed MaaS ids (`deepseek-ai/deepseek-v4.1-flash-maas`,
//!   `openai/gpt-oss-120b-maas`, `google/gemini-4-carbon`) go to the OpenAI-compatible
//!   `endpoints/openapi/chat/completions` path ([`OpenAiProvider`]) with the ADC token
//!   as bearer key.
//!
//! Embeddings always use the native `:predict` path.

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;

use super::gemini::GeminiProvider;
use super::openai::OpenAiProvider;
use super::vertex_auth::TokenSource;
use super::{ProviderClient, ProviderError};
use crate::ir::{ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse};
use crate::resolver::{ResolvedConfig, ResolvedEmbeddingConfig};

/// Which Vertex wire format serves `model`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexWire {
    GeminiNative,
    OpenApi,
}

/// Publisher-prefixed ids use the OpenAI-compatible endpoint; bare ids are native Gemini.
pub fn wire_for(model: &str) -> VertexWire {
    if model.contains('/') {
        VertexWire::OpenApi
    } else {
        VertexWire::GeminiNative
    }
}

pub struct VertexProvider {
    gemini: GeminiProvider,
    openapi: OpenAiProvider,
    tokens: Arc<TokenSource>,
}

impl VertexProvider {
    pub fn new(
        http: reqwest::Client,
        native_base: String,
        openapi_base: String,
        tokens: Arc<TokenSource>,
    ) -> Self {
        Self {
            gemini: GeminiProvider::new(http.clone(), native_base).with_adc(tokens.clone()),
            openapi: OpenAiProvider::new(http, openapi_base),
            tokens,
        }
    }

    /// The config with the ADC access token in place of the `"adc"` marker key.
    async fn with_token(&self, cfg: &ResolvedConfig) -> Result<ResolvedConfig, ProviderError> {
        let token: String = self.tokens.token().await?;
        let mut out: ResolvedConfig = cfg.clone();
        out.api_key = token;
        Ok(out)
    }
}

/// Vertex authenticates only with the platform's ADC identity. A user-secret config would
/// spend platform GCP credentials while skipping usage billing, so it is refused.
fn require_platform_paid(platform_paid: bool) -> Result<(), ProviderError> {
    if platform_paid {
        Ok(())
    } else {
        Err(ProviderError::Status {
            status: 400,
            message: "vertex uses platform ADC credentials only; per-user API key secrets are not supported for provider 'vertex'".into(),
            retryable: false,
        })
    }
}

#[async_trait]
impl ProviderClient for VertexProvider {
    async fn chat(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<ChatResponse, ProviderError> {
        require_platform_paid(cfg.platform_paid)?;
        match wire_for(&cfg.model) {
            VertexWire::GeminiNative => self.gemini.chat(req, cfg).await,
            VertexWire::OpenApi => self.openapi.chat(req, &self.with_token(cfg).await?).await,
        }
    }

    async fn chat_stream(
        &self,
        req: &ChatRequest,
        cfg: &ResolvedConfig,
    ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
        require_platform_paid(cfg.platform_paid)?;
        match wire_for(&cfg.model) {
            VertexWire::GeminiNative => self.gemini.chat_stream(req, cfg).await,
            VertexWire::OpenApi => {
                self.openapi
                    .chat_stream(req, &self.with_token(cfg).await?)
                    .await
            }
        }
    }

    async fn embeddings(
        &self,
        req: &EmbeddingsRequest,
        cfg: &ResolvedEmbeddingConfig,
    ) -> Result<EmbeddingsResponse, ProviderError> {
        require_platform_paid(cfg.platform_paid)?;
        self.gemini.embeddings(req, cfg).await
    }

    fn droppable_param(&self, err: &ProviderError) -> Option<String> {
        self.openapi
            .droppable_param(err)
            .or_else(|| self.gemini.droppable_param(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_secret_configs_are_refused() {
        assert!(require_platform_paid(true).is_ok());
        assert!(matches!(
            require_platform_paid(false),
            Err(ProviderError::Status {
                status: 400,
                retryable: false,
                ..
            })
        ));
    }

    #[test]
    fn bare_gemini_ids_use_native_wire() {
        assert_eq!(wire_for("gemini-3.8-flash"), VertexWire::GeminiNative);
        assert_eq!(wire_for("gemini-4-carbon"), VertexWire::GeminiNative);
    }

    #[test]
    fn publisher_prefixed_ids_use_openapi_wire() {
        assert_eq!(
            wire_for("deepseek-ai/deepseek-v4.1-flash-maas"),
            VertexWire::OpenApi
        );
        assert_eq!(wire_for("openai/gpt-oss-120b-maas"), VertexWire::OpenApi);
        assert_eq!(wire_for("google/gemini-4-carbon"), VertexWire::OpenApi);
    }
}
