//! OpenRouter provider — wire-compatible with OpenAI's Chat Completions API, so this
//! is ≈passthrough (same shape as [`super::openai::OpenAiProvider`]). The only
//! OpenRouter-specific behavior is the optional `HTTP-Referer`/`X-Title` attribution
//! headers (app ranking on openrouter.ai; harmless to omit) and the model id, which is
//! a full `vendor/model` string (e.g. `anthropic/claude-sonnet-4.6`) rather than a bare
//! provider-native id. With `TOKEN_CACHE_MARKERS` on, a Claude model also gets the router's own
//! cache markers in the slots the agent left free ([`add_cache_markers`]), and every model gets a
//! `session_id` that pins the agent to one upstream provider ([`add_session_id`]).

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use super::sse::sse_data_stream;
use super::{ProviderClient, ProviderError};
use crate::ir::{
    CACHE_CONTROL, ChatChunk, ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse,
    Message,
};
use crate::resolver::{ResolvedConfig, ResolvedEmbeddingConfig};

pub struct OpenRouterProvider {
    http: reqwest::Client,
    /// API base, e.g. `https://openrouter.ai/api/v1` (overridable for tests).
    base: String,
    /// Optional `HTTP-Referer` attribution header. Empty ⇒ omitted.
    http_referer: String,
    /// Optional `X-Title` attribution header. Empty ⇒ omitted.
    x_title: String,
    /// Add the router's own cache markers ([`add_cache_markers`]) and session id
    /// ([`add_session_id`]).
    cache_markers: bool,
}

/// OpenRouter accepts at most this many explicit `cache_control` breakpoints for Claude.
const MAX_CACHE_MARKERS: usize = 4;

impl OpenRouterProvider {
    pub fn new(http: reqwest::Client, base: String, http_referer: String, x_title: String) -> Self {
        Self {
            http,
            base,
            http_referer,
            x_title,
            cache_markers: false,
        }
    }

    /// Fill the cache-marker slots an agent leaves free (`TOKEN_CACHE_MARKERS`).
    pub fn with_cache_markers(mut self, enabled: bool) -> Self {
        self.cache_markers = enabled;
        self
    }

    /// The request as OpenRouter receives it: the resolved model and parameters, plus the
    /// router's own cache markers and session id when they are on.
    fn outbound(&self, req: &ChatRequest, cfg: &ResolvedConfig, stream: bool) -> ChatRequest {
        let mut out = req.clone();
        out.model = Some(cfg.model.clone()); // C4: resolved model is authoritative
        out.temperature = cfg.temperature.or(req.temperature);
        if let Some(mt) = cfg.max_tokens.or(req.max_tokens) {
            out.max_tokens = Some(mt);
        }
        out.stream = Some(stream);
        if self.cache_markers {
            add_cache_markers(&mut out);
            add_session_id(&mut out, cfg);
        }
        out
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
        let out = self.outbound(req, cfg, false);

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
        let mut out = self.outbound(req, cfg, true);
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
        cfg: &ResolvedEmbeddingConfig,
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
        if !matches!(code, "unsupported_value" | "unsupported_parameter") {
            return None;
        }
        let param = error.get("param").and_then(|p| p.as_str())?;
        Some(param.to_string())
    }
}

/// Add the router's own cache markers for a Claude model in the slots the agent left free:
/// first the end of the conversation, ahead of brevity's trailing directive, then the end of the
/// leading system messages. Each is a 5-minute marker. Other models cache on OpenRouter without
/// markers, and Gemini keeps only the last one, so they get none.
fn add_cache_markers(out: &mut ChatRequest) {
    if !out
        .model
        .as_deref()
        .is_some_and(|m| m.starts_with("anthropic/"))
    {
        return;
    }
    let mut free = MAX_CACHE_MARKERS.saturating_sub(out.cache_marker_count());
    // A 5-minute marker may not come before a 1-hour one, and system comes before every turn,
    // including a tool call's marker, which OpenRouter receives on the call itself.
    let turns_hold_long_marker = out.messages.iter().any(|m| {
        let parts = m
            .content
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        let calls = m
            .tool_calls
            .iter()
            .flatten()
            .filter_map(|call| call.extra.get(CACHE_CONTROL));
        m.role != "system"
            && parts
                .map(|part| &part[CACHE_CONTROL])
                .chain(calls)
                .any(|marker| marker["ttl"] == "1h")
    });

    if free > 0
        && let Some(last) = out
            .messages
            .iter_mut()
            .rev()
            .find(|m| !crate::brevity::is_directive(m))
        && mark_last_text(last)
    {
        free -= 1;
    }
    if free > 0
        && !turns_hold_long_marker
        && let Some(system) = out
            .messages
            .iter_mut()
            .take_while(|m| m.role == "system" && !crate::brevity::is_directive(m))
            .last()
    {
        mark_last_text(system);
    }
}

/// The body field OpenRouter keys sticky routing by. It reads `prompt_cache_key` when this is
/// absent.
const SESSION_ID: &str = "session_id";
const PROMPT_CACHE_KEY: &str = "prompt_cache_key";

/// Pin the agent to one upstream provider, so its calls reach the cache the first one wrote.
/// Without a key OpenRouter pins a conversation only after its first cache hit; with one it pins
/// from the first call, for 10 minutes after the last. Z.ai's cache affinity also follows it. A
/// manual `provider.order` would turn sticky routing off, so the router sends none. The agent's own
/// `session_id` or `prompt_cache_key` wins.
fn add_session_id(out: &mut ChatRequest, cfg: &ResolvedConfig) {
    if cfg.agent_id.is_empty()
        || out.extra.contains_key(SESSION_ID)
        || out.extra.contains_key(PROMPT_CACHE_KEY)
    {
        return;
    }
    out.extra
        .insert(SESSION_ID.to_string(), json!(cfg.agent_id));
}

/// Mark a message's last text, turning a string content into its one text part; `false` when it
/// was marked already or has no text to mark.
fn mark_last_text(m: &mut Message) -> bool {
    if let Some(Value::String(text)) = &m.content
        && !text.is_empty()
    {
        m.content = Some(json!([{ "type": "text", "text": text }]));
    }
    let Some(part) =
        m.content
            .as_mut()
            .and_then(Value::as_array_mut)
            .and_then(|parts| {
                parts.iter_mut().rev().find(|p| {
                    p["type"] == "text" && p["text"].as_str().is_some_and(|t| !t.is_empty())
                })
            })
    else {
        return false;
    };
    if part.get(CACHE_CONTROL).is_some() {
        return false;
    }
    part[CACHE_CONTROL] = json!({ "type": "ephemeral" });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resolved(model: &str, temperature: Option<f64>) -> ResolvedConfig {
        ResolvedConfig {
            agent_id: String::new(),
            compress_enabled: false,
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
            custom_endpoint: None,
            is_coding_agent: false,
            is_discovered_agent: false,
        }
    }

    fn provider(base: String) -> OpenRouterProvider {
        OpenRouterProvider::new(reqwest::Client::new(), base, String::new(), String::new())
    }

    /// The body OpenRouter receives for `req`.
    async fn sent_body(req: &ChatRequest) -> serde_json::Value {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(serde_json::Value::Null));
        let capture = std::sync::Arc::clone(&seen);
        let reply = json!({
            "id": "gen-1", "object": "chat.completion", "model": "anthropic/claude-haiku-5.5",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })
        .to_string();
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |request| {
                *capture.lock().unwrap() = serde_json::from_slice(request.body().unwrap()).unwrap();
                reply.clone().into_bytes()
            })
            .create_async()
            .await;

        provider(server.url())
            .chat(req, &resolved("anthropic/claude-haiku-5.5", None))
            .await
            .unwrap();
        seen.lock().unwrap().take()
    }

    #[tokio::test]
    async fn a_turn_that_skips_brevity_still_matches_the_cached_prefix_of_one_that_applied_it() {
        // OpenRouter forwards the trailing system message where it is, after the agent's last
        // marker, so the next turn's bytes match up to that marker.
        use crate::inbound::InboundParser;
        let cfg = crate::config::GatewayConfig {
            brevity_enabled: true,
            brevity_min_bytes: 0,
            ..Default::default()
        };
        let brevity_on = ResolvedConfig {
            compress_enabled: true,
            ..resolved("anthropic/claude-haiku-5.5", None)
        };
        let ephemeral = json!({ "type": "ephemeral" });
        let mut agent_request = json!({
            "model": "claude-haiku-5-5",
            "max_tokens": 1024,
            "system": [{ "type": "text", "text": "You are a coding agent.", "cache_control": ephemeral }],
            "tools": [{ "name": "read", "input_schema": { "type": "object" } }],
            "messages": [{ "role": "user", "content": [
                { "type": "text", "text": "Fix the failing test.", "cache_control": ephemeral }
            ]}]
        });
        let mut first = crate::inbound::AnthropicInbound
            .parse_chat(agent_request.clone())
            .unwrap();
        crate::brevity::apply(&mut first, &cfg, &brevity_on, None).unwrap();

        // The next turn is mid tool-loop, where brevity stands down.
        let history = agent_request["messages"].as_array_mut().unwrap();
        history.push(json!({ "role": "assistant", "content": [
            { "type": "tool_use", "id": "toolu_1", "name": "read", "input": {} }
        ]}));
        history.push(json!({ "role": "user", "content": [
            { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}", "cache_control": ephemeral }
        ]}));
        let mut second = crate::inbound::AnthropicInbound
            .parse_chat(agent_request)
            .unwrap();
        assert_eq!(
            crate::brevity::apply(&mut second, &cfg, &brevity_on, None),
            Err(crate::brevity::Skipped::ToolContinuation)
        );

        let first = sent_body(&first).await;
        let second = sent_body(&second).await;

        assert_eq!(first["tools"], second["tools"]);
        let (directive, before) = first["messages"].as_array().unwrap().split_last().unwrap();
        assert_eq!(directive["content"], crate::brevity::DIRECTIVE);
        let last_marked = before.last().unwrap();
        assert_eq!(last_marked["role"], "user");
        assert!(last_marked.to_string().contains("cache_control"));
        assert_eq!(
            before,
            &second["messages"].as_array().unwrap()[..before.len()]
        );
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

    /// The body OpenRouter receives for `req` on `model` with `TOKEN_CACHE_MARKERS` on.
    fn with_router_markers(req: &ChatRequest, model: &str) -> serde_json::Value {
        let out = provider("http://x".into())
            .with_cache_markers(true)
            .outbound(req, &resolved(model, None), false);
        serde_json::to_value(out).unwrap()
    }

    fn plain_request() -> ChatRequest {
        serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are a coding agent." },
                { "role": "user", "content": "Fix the failing test." }
            ]
        }))
        .unwrap()
    }

    fn marked(text: &str) -> serde_json::Value {
        json!([{ "type": "text", "text": text, "cache_control": { "type": "ephemeral" } }])
    }

    #[test]
    fn the_router_marks_the_end_of_the_conversation_then_of_system_on_claude() {
        let body = with_router_markers(&plain_request(), "anthropic/claude-haiku-5.5");

        assert_eq!(
            body["messages"][0]["content"],
            marked("You are a coding agent.")
        );
        assert_eq!(
            body["messages"][1]["content"],
            marked("Fix the failing test.")
        );
    }

    #[test]
    fn without_the_flag_the_router_adds_no_marker() {
        let out = provider("http://x".into()).outbound(
            &plain_request(),
            &resolved("anthropic/claude-haiku-5.5", None),
            false,
        );

        assert_eq!(out.cache_marker_count(), 0);
    }

    #[test]
    fn models_that_cache_without_markers_get_none_from_the_router() {
        for model in [
            "openai/gpt-5.5",
            "google/gemini-2.5-pro",
            "deepseek/deepseek-v3.2",
            "z-ai/glm-5.3",
        ] {
            let body = with_router_markers(&plain_request(), model);

            assert!(!body.to_string().contains("cache_control"), "{model}");
        }
    }

    #[test]
    fn the_routers_last_marker_goes_ahead_of_brevitys_directive() {
        let cfg = crate::config::GatewayConfig {
            brevity_enabled: true,
            brevity_min_bytes: 0,
            ..Default::default()
        };
        let brevity_on = ResolvedConfig {
            compress_enabled: true,
            ..resolved("anthropic/claude-haiku-5.5", None)
        };
        let mut req = plain_request();
        crate::brevity::apply(&mut req, &cfg, &brevity_on, None).unwrap();

        let body = with_router_markers(&req, "anthropic/claude-haiku-5.5");

        assert_eq!(
            body["messages"][1]["content"],
            marked("Fix the failing test.")
        );
        assert_eq!(body["messages"][2]["content"], crate::brevity::DIRECTIVE);
    }

    #[test]
    fn a_tool_result_ending_the_conversation_takes_the_last_marker() {
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "user", "content": "Fix the failing test." },
                { "role": "assistant", "content": null, "tool_calls": [{
                    "id": "toolu_1", "type": "function",
                    "function": { "name": "read", "arguments": "{}" }
                }]},
                { "role": "tool", "tool_call_id": "toolu_1", "content": "fn main() {}" }
            ]
        }))
        .unwrap();

        let body = with_router_markers(&req, "anthropic/claude-haiku-5.5");

        assert_eq!(body["messages"][2]["content"], marked("fn main() {}"));
    }

    #[test]
    fn an_agent_holding_all_four_markers_gets_none_from_the_router() {
        use crate::inbound::InboundParser;
        let ephemeral = json!({ "type": "ephemeral" });
        let req = crate::inbound::AnthropicInbound
            .parse_chat(json!({
                "model": "claude-haiku-5-5",
                "max_tokens": 1024,
                "system": "You are a coding agent.",
                "tools": [{ "name": "read", "input_schema": { "type": "object" }, "cache_control": ephemeral }],
                "messages": [
                    { "role": "user", "content": [
                        { "type": "text", "text": "Fix the failing test.", "cache_control": ephemeral }
                    ]},
                    { "role": "assistant", "content": [
                        { "type": "tool_use", "id": "toolu_1", "name": "read", "input": {}, "cache_control": ephemeral }
                    ]},
                    { "role": "user", "content": [
                        { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}", "cache_control": ephemeral },
                        { "type": "text", "text": "Then stop." }
                    ]}
                ]
            }))
            .unwrap();
        let model = "anthropic/claude-haiku-5.5";
        let without = provider("http://x".into()).outbound(&req, &resolved(model, None), false);

        // Neither the end of the conversation nor system is marked, but a fifth is rejected.
        let body = with_router_markers(&req, model);

        assert_eq!(without.cache_marker_count(), MAX_CACHE_MARKERS);
        assert_eq!(body, serde_json::to_value(without).unwrap());
    }

    #[test]
    fn no_five_minute_system_marker_goes_ahead_of_an_agents_one_hour_marker() {
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [
                { "role": "system", "content": "You are a coding agent." },
                { "role": "user", "content": [{
                    "type": "text", "text": "Read the repository map.",
                    "cache_control": { "type": "ephemeral", "ttl": "1h" }
                }]},
                { "role": "assistant", "content": "Done." },
                { "role": "user", "content": "Now fix the failing test." }
            ]
        }))
        .unwrap();

        let body = with_router_markers(&req, "anthropic/claude-haiku-5.5");

        assert_eq!(body["messages"][0]["content"], "You are a coding agent.");
        assert_eq!(
            body["messages"][3]["content"],
            marked("Now fix the failing test.")
        );
    }

    #[test]
    fn no_five_minute_system_marker_goes_ahead_of_an_agents_one_hour_tool_call_marker() {
        use crate::inbound::InboundParser;
        let req = crate::inbound::AnthropicInbound
            .parse_chat(json!({
                "model": "claude-haiku-5-5",
                "max_tokens": 1024,
                "system": "You are a coding agent.",
                "messages": [
                    { "role": "user", "content": "Fix the failing test." },
                    { "role": "assistant", "content": [{
                        "type": "tool_use", "id": "toolu_1", "name": "read", "input": {},
                        "cache_control": { "type": "ephemeral", "ttl": "1h" }
                    }]},
                    { "role": "user", "content": [
                        { "type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}" }
                    ]}
                ]
            }))
            .unwrap();

        let body = with_router_markers(&req, "anthropic/claude-haiku-5.5");

        assert_eq!(body["messages"][0]["content"], "You are a coding agent.");
        assert_eq!(body["messages"][3]["content"], marked("fn main() {}"));
    }

    const AGENT_ID: &str = "6f1c2a9e-0b7d-4e8a-9c21-7d4e5f6a8b90";

    fn for_agent(model: &str) -> ResolvedConfig {
        ResolvedConfig {
            agent_id: AGENT_ID.into(),
            ..resolved(model, None)
        }
    }

    #[test]
    fn the_router_pins_every_model_to_one_provider_by_the_agent_id() {
        for model in [
            "anthropic/claude-haiku-5.5",
            "z-ai/glm-5.3",
            "openai/gpt-5.5",
        ] {
            let out = provider("http://x".into())
                .with_cache_markers(true)
                .outbound(&plain_request(), &for_agent(model), false);

            assert_eq!(out.extra[SESSION_ID], AGENT_ID, "{model}");
        }
    }

    #[test]
    fn an_agents_own_session_id_or_prompt_cache_key_wins() {
        for (field, value) in [
            (SESSION_ID, "agent-session"),
            (PROMPT_CACHE_KEY, "agent-key"),
        ] {
            let mut req = plain_request();
            req.extra.insert(field.into(), json!(value));

            let out = provider("http://x".into())
                .with_cache_markers(true)
                .outbound(&req, &for_agent("z-ai/glm-5.3"), false);

            assert_eq!(out.extra[field], value);
            assert_eq!(out.extra.len(), 1, "{field}");
        }
    }

    #[test]
    fn no_session_id_without_the_flag_or_an_agent() {
        let flag_off = provider("http://x".into()).outbound(
            &plain_request(),
            &for_agent("z-ai/glm-5.3"),
            false,
        );
        let no_agent = provider("http://x".into())
            .with_cache_markers(true)
            .outbound(&plain_request(), &resolved("z-ai/glm-5.3", None), false);

        assert!(!flag_off.extra.contains_key(SESSION_ID));
        assert!(!no_agent.extra.contains_key(SESSION_ID));
    }

    #[tokio::test]
    async fn the_default_gateway_config_sends_the_routers_markers_and_session_id() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({
                "session_id": AGENT_ID,
                "messages": [
                    { "role": "system", "content": marked("You are a coding agent.") },
                    { "role": "user", "content": marked("Fix the failing test.") }
                ]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "gen-1", "object": "chat.completion", "model": "anthropic/claude-haiku-5.5",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let cfg = crate::config::GatewayConfig {
            openrouter_api_base: server.url(),
            ..Default::default()
        };
        let provider =
            crate::providers::provider_for("openrouter", None, &reqwest::Client::new(), &cfg)
                .unwrap();

        provider
            .chat(&plain_request(), &for_agent("anthropic/claude-haiku-5.5"))
            .await
            .unwrap();

        m.assert_async().await;
    }
}
