//! [`LlmBackend`] for the orchestrator, served by the in-process LLM router.
//!
//! The orchestrator and router share this process, so the orchestrator's calls skip HTTP,
//! tokens and traceparents and go straight to [`PlatformLlm`]. The router picks the
//! provider from platform config and writes `trace_usage`.

use async_trait::async_trait;
use nasiko_llm_router::error::GatewayError;
use nasiko_llm_router::inbound::{InboundFormat, inbound_for};
use nasiko_llm_router::ir::{ChatRequest, EmbeddingsRequest, EmbeddingsResponse};
use nasiko_llm_router::platform::{PlatformCall, PlatformLlm, TranscribeRequest, Transcript};
use nasiko_orchestrator::llm_backend::{LlmBackend, LlmCallContext, LlmError};

pub struct RouterBackend {
    llm: PlatformLlm,
}

impl RouterBackend {
    pub fn new(llm: PlatformLlm) -> Self {
        Self { llm }
    }
}

fn platform_call(call: &LlmCallContext) -> PlatformCall {
    PlatformCall {
        user_id: call.user_id.to_string(),
        component: call.component.clone(),
        session_id: call.session_id.clone(),
    }
}

/// Keeps the upstream status of a provider rejection (`GatewayError::Provider`) so callers
/// can tell "request rejected" from "provider unreachable".
pub(crate) fn llm_error(e: GatewayError) -> LlmError {
    let status: Option<u16> = match &e {
        GatewayError::Provider { status, .. } => Some(status.as_u16()),
        _ => None,
    };
    LlmError {
        status,
        message: e.to_string(),
    }
}

/// Vectors from an OpenAI-shaped embeddings response, in input order.
pub(crate) fn vectors(resp: EmbeddingsResponse, expected: usize) -> Result<Vec<Vec<f32>>, String> {
    let mut data = resp.data;
    data.sort_by_key(|d| d.index);
    if data.len() != expected {
        return Err(format!(
            "expected {expected} embeddings, got {}",
            data.len()
        ));
    }
    data.into_iter()
        .map(|d| {
            serde_json::from_value::<Vec<f32>>(d.embedding)
                .map_err(|e| format!("embedding is not a float array: {e}"))
        })
        .collect()
}

#[async_trait]
impl LlmBackend for RouterBackend {
    async fn chat(
        &self,
        call: &LlmCallContext,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, LlmError> {
        let inbound = inbound_for(InboundFormat::OpenAi);
        let req: ChatRequest = inbound.parse_chat(body).map_err(|e| LlmError {
            status: Some(400),
            message: e.to_string(),
        })?;
        let resp = self
            .llm
            .chat(&platform_call(call), req)
            .await
            .map_err(llm_error)?;
        Ok(inbound.render_chat_response(resp))
    }

    async fn embed(
        &self,
        call: &LlmCallContext,
        model: &str,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, String> {
        let req = EmbeddingsRequest {
            model: Some(model.to_string()),
            input: serde_json::json!(texts),
            extra: Default::default(),
        };
        let resp = self
            .llm
            .embed(&platform_call(call), None, req)
            .await
            .map_err(|e| e.to_string())?;
        vectors(resp, texts.len())
    }

    fn embedding_identity(&self, model: &str) -> String {
        let route = self.llm.resolve_embedding(model);
        format!("{}/{}", route.provider, route.model)
    }

    fn can_embed(&self, model: &str) -> bool {
        self.llm.can_embed(model)
    }

    async fn transcribe(
        &self,
        call: &LlmCallContext,
        audio: Vec<u8>,
        mime_type: &str,
    ) -> Result<String, String> {
        let req = TranscribeRequest {
            audio,
            mime_type: mime_type.to_string(),
            model: std::env::var("TRANSCRIPTION_MODEL")
                .ok()
                .filter(|m| !m.trim().is_empty()),
        };
        let transcript: Transcript = self
            .llm
            .transcribe(&platform_call(call), req)
            .await
            .map_err(|e| e.to_string())?;
        Ok(transcript.text)
    }

    fn provider(&self) -> String {
        self.llm.default_provider().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(items: serde_json::Value) -> EmbeddingsResponse {
        serde_json::from_value(serde_json::json!({"data": items, "model": "m"})).unwrap()
    }

    #[test]
    fn vectors_sorted_by_index() {
        let r = resp(serde_json::json!([
            {"embedding": [2.0], "index": 1},
            {"embedding": [1.0], "index": 0}
        ]));
        assert_eq!(vectors(r, 2).unwrap(), vec![vec![1.0], vec![2.0]]);
    }

    #[test]
    fn provider_rejection_keeps_status() {
        let e = llm_error(GatewayError::Provider {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: "response_format unavailable".into(),
        });
        assert_eq!(e.status, Some(400));
        assert!(e.message.contains("response_format"));
        assert_eq!(
            llm_error(GatewayError::Upstream("down".into())).status,
            None
        );
    }

    #[test]
    fn vectors_count_mismatch_errors() {
        let r = resp(serde_json::json!([{"embedding": [1.0], "index": 0}]));
        assert!(vectors(r, 2).is_err());
    }
}
