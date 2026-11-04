//! OpenAI inbound parser — an identity transform, since the IR is OpenAI shape.

use serde_json::Value;

use super::{ChatStreamRenderer, InboundParser};
use crate::error::GatewayError;
use crate::ir::{
    ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse, Usage,
};

/// Parses/renders the agent-facing OpenAI wire format. The agent's OpenAI SDK speaks
/// this directly, so parsing is `serde_json::from_value` and rendering is `to_value`.
pub struct OpenAiInbound;

impl InboundParser for OpenAiInbound {
    fn parse_chat(&self, body: Value) -> Result<ChatRequest, GatewayError> {
        serde_json::from_value(body)
            .map_err(|e| GatewayError::BadRequest(format!("invalid chat request: {e}")))
    }

    fn render_chat_response(&self, mut resp: ChatResponse) -> Value {
        resp.usage = resp.usage.map(Usage::into_cache_inclusive);
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
    fn render(&mut self, mut chunk: ChatChunk) -> Vec<String> {
        chunk.usage = chunk.usage.map(Usage::into_cache_inclusive);
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

    fn anthropic_served_usage() -> Value {
        json!({ "prompt_tokens": 4, "completion_tokens": 75, "total_tokens": 79,
                "cache_read_input_tokens": 10431, "cache_creation_input_tokens": 0 })
    }

    fn expected_openai_usage() -> Value {
        json!({ "prompt_tokens": 10435, "completion_tokens": 75, "total_tokens": 10510,
                "cache_creation_input_tokens": 0,
                "prompt_tokens_details": { "cached_tokens": 10431 } })
    }

    #[test]
    fn a_reply_served_from_another_providers_cache_reports_openai_usage() {
        let resp: ChatResponse = serde_json::from_value(json!({
            "id": "chatcmpl-msg_1", "object": "chat.completion", "model": "claude-haiku-5-5",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
            "usage": anthropic_served_usage()
        }))
        .unwrap();

        let v = OpenAiInbound.render_chat_response(resp);

        assert_eq!(v["usage"], expected_openai_usage());
    }

    #[test]
    fn the_streamed_usage_chunk_reports_openai_usage_too() {
        let chunk: ChatChunk = serde_json::from_value(json!({
            "id": "chatcmpl-msg_1", "object": "chat.completion.chunk", "model": "claude-haiku-5-5",
            "choices": [], "usage": anthropic_served_usage()
        }))
        .unwrap();

        let frames = OpenAiStreamRenderer.render(chunk);

        let data: Value =
            serde_json::from_str(frames[0].strip_prefix("data: ").unwrap().trim()).unwrap();
        assert_eq!(data["usage"], expected_openai_usage());
    }
}
