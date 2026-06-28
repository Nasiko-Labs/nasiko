//! OpenAI inbound parser — an identity transform, since the IR is OpenAI shape.

use serde_json::Value;

use super::{ChatStreamRenderer, InboundParser};
use crate::error::GatewayError;
use crate::ir::{ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse};

/// Parses/renders the agent-facing OpenAI wire format. The agent's OpenAI SDK speaks
/// this directly, so parsing is `serde_json::from_value` and rendering is `to_value`.
pub struct OpenAiInbound;

impl InboundParser for OpenAiInbound {
    fn parse_chat(&self, body: Value) -> Result<ChatRequest, GatewayError> {
        serde_json::from_value(body)
            .map_err(|e| GatewayError::BadRequest(format!("invalid chat request: {e}")))
    }

    fn render_chat_response(&self, resp: ChatResponse) -> Value {
        // IR structs are always serializable; a failure here is a programming error.
        serde_json::to_value(resp).expect("ChatResponse must serialize")
    }

    fn chat_stream_renderer(&self) -> Box<dyn ChatStreamRenderer> {
        Box::new(OpenAiStreamRenderer)
    }

    fn parse_embeddings(&self, body: Value) -> Result<EmbeddingsRequest, GatewayError> {
        serde_json::from_value(body)
            .map_err(|e| GatewayError::BadRequest(format!("invalid embeddings request: {e}")))
    }

    fn render_embeddings(&self, resp: EmbeddingsResponse) -> Value {
        serde_json::to_value(resp).expect("EmbeddingsResponse must serialize")
    }
}

/// Identity streaming renderer: each IR chunk is one `data: <json>` event; the stream
/// terminates with `data: [DONE]` (OpenAI's contract).
struct OpenAiStreamRenderer;

impl ChatStreamRenderer for OpenAiStreamRenderer {
    fn render(&mut self, chunk: ChatChunk) -> Vec<String> {
        let json = serde_json::to_value(&chunk).expect("ChatChunk must serialize");
        vec![format!("data: {json}\n\n")]
    }

    fn finish(&mut self) -> Vec<String> {
        vec!["data: [DONE]\n\n".to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_valid_chat_request() {
        let req = OpenAiInbound
            .parse_chat(json!({
                "model": "gpt-4o",
                "messages": [{ "role": "user", "content": "hi" }]
            }))
            .unwrap();
        assert_eq!(req.messages.len(), 1);
    }

    #[test]
    fn malformed_chat_request_is_bad_request() {
        // `messages` is required.
        let err = OpenAiInbound
            .parse_chat(json!({ "model": "gpt-4o" }))
            .unwrap_err();
        assert!(matches!(err, GatewayError::BadRequest(_)));
        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn render_chat_response_is_identity_shape() {
        let resp: ChatResponse = serde_json::from_value(json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        }))
        .unwrap();
        let v = OpenAiInbound.render_chat_response(resp);
        assert_eq!(v["choices"][0]["message"]["content"], "ok");
        assert_eq!(v["usage"]["total_tokens"], 2);
    }
}
