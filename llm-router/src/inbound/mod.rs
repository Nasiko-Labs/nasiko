//! Inbound parsing — the front of the hub.
//!
//! An [`InboundParser`] turns a provider-SDK-shaped HTTP body into the canonical IR
//! and renders IR responses/chunks back into that SDK's wire shape. v1 ships only the
//! OpenAI parser (an identity transform, since the IR *is* OpenAI shape); Anthropic-
//! and Gemini-SDK inbound parsers are additive later behind this same trait.

use serde_json::Value;

use crate::error::GatewayError;
use crate::ir::{ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse};

pub mod anthropic;
pub mod gemini;
pub mod openai;

pub use anthropic::AnthropicInbound;
pub use gemini::GeminiInbound;
pub use openai::OpenAiInbound;

/// Which SDK/wire format a caller speaks — selects the [`InboundParser`] at request
/// time and the base-URL/key env-var names at deploy time (see `crate::inject`).
///
/// Independent of the *outbound* provider in `agents.llm_config`: this is about how the
/// agent's own code calls an LLM, not where we route it. v1 supports only [`OpenAi`];
/// [`Anthropic`]/[`Gemini`] land in P2.3/P2.4.
///
/// [`OpenAi`]: InboundFormat::OpenAi
/// [`Anthropic`]: InboundFormat::Anthropic
/// [`Gemini`]: InboundFormat::Gemini
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InboundFormat {
    #[default]
    OpenAi,
    Anthropic,
    Gemini,
}

impl InboundFormat {
    /// Map an agent's `inbound_format` column value to the enum; unknown/missing →
    /// [`OpenAi`](InboundFormat::OpenAi) (the backward-compatible default).
    pub fn from_label(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "anthropic" => InboundFormat::Anthropic,
            "gemini" => InboundFormat::Gemini,
            _ => InboundFormat::OpenAi,
        }
    }

    /// The destination-provider label this SDK surface implies, matching the provider
    /// strings used by [`crate::config::GatewayConfig::platform_key_for`] and
    /// [`crate::providers::provider_for`]. Used only as the passthrough provider hint when
    /// an agent has no `llm_config` — the request's own SDK is honored before the platform
    /// default (the model comes from the request body, this provider from the same call, so
    /// the pair is self-consistent).
    pub fn provider_label(self) -> &'static str {
        match self {
            InboundFormat::OpenAi => "openai",
            InboundFormat::Anthropic => "anthropic",
            InboundFormat::Gemini => "gemini",
        }
    }
}

/// Select the inbound parser for a wire format. Mirrors `providers::provider_for`: the
/// per-route choice of which SDK shape we parse/render lives behind this one seam.
pub fn inbound_for(format: InboundFormat) -> Box<dyn InboundParser> {
    match format {
        InboundFormat::OpenAi => Box::new(OpenAiInbound),
        InboundFormat::Anthropic => Box::new(AnthropicInbound),
        InboundFormat::Gemini => Box::new(GeminiInbound),
    }
}

/// Translates between a caller's wire format and the canonical IR. The render methods
/// produce the exact bytes the agent's SDK parses, so the wire contract is owned here
/// (never delegated to a provider client).
pub trait InboundParser: Send + Sync {
    fn parse_chat(&self, body: Value) -> Result<ChatRequest, GatewayError>;
    fn render_chat_response(&self, resp: ChatResponse) -> Value;
    /// Create a fresh, stateful renderer for one streaming response. Owning the SSE
    /// framing here lets each format speak its own wire protocol (OpenAI emits flat
    /// `data:` lines + `[DONE]`; Anthropic emits stateful `event:`/`data:` sequences
    /// with open/close content blocks).
    fn chat_stream_renderer(&self) -> Box<dyn ChatStreamRenderer>;
    fn parse_embeddings(&self, body: Value) -> Result<EmbeddingsRequest, GatewayError>;
    fn render_embeddings(&self, resp: EmbeddingsResponse) -> Value;
}

/// Renders canonical IR chunks into a caller's SSE wire bytes, statefully across one
/// response. `render` returns zero or more fully-framed SSE events for a chunk; `finish`
/// emits any terminal events (OpenAI `[DONE]`; Anthropic `message_delta`/`message_stop`)
/// once the upstream stream ends — including on client disconnect.
pub trait ChatStreamRenderer: Send {
    fn render(&mut self, chunk: ChatChunk) -> Vec<String>;
    fn finish(&mut self) -> Vec<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_format_is_openai() {
        assert_eq!(InboundFormat::default(), InboundFormat::OpenAi);
    }

    #[test]
    fn from_label_maps_known_and_defaults_unknown() {
        assert_eq!(InboundFormat::from_label("anthropic"), InboundFormat::Anthropic);
        assert_eq!(InboundFormat::from_label("GEMINI"), InboundFormat::Gemini);
        assert_eq!(InboundFormat::from_label("openai"), InboundFormat::OpenAi);
        assert_eq!(InboundFormat::from_label("cohere"), InboundFormat::OpenAi);
        assert_eq!(InboundFormat::from_label(""), InboundFormat::OpenAi);
    }

    #[test]
    fn inbound_for_openai_parses_an_openai_chat_body() {
        let parser = inbound_for(InboundFormat::OpenAi);
        let req = parser
            .parse_chat(json!({ "model": "gpt-4o", "messages": [{ "role": "user", "content": "hi" }] }))
            .unwrap();
        assert_eq!(req.messages.len(), 1);
    }
}
