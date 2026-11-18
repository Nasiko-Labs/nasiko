//! Usage-preserving bridge between rig's tools and the shared provider transport.

use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Utc};
use futures::{StreamExt, stream::BoxStream};
use nasiko_llm_router::{
    ir::{ChatRequest, Message as WireMessage, ToolCallDelta, Usage},
    providers::{OpenAiProvider, ProviderClient, ProviderError},
    resolver::ResolvedConfig,
};
use rig::{
    OneOrMany,
    completion::message::{ToolCall, ToolFunction},
    completion::{
        AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
    },
    providers::openai,
};
use serde::Serialize;
use serde_json::json;

use crate::react_loop::{ApiKeySource, OrchestratorConfig};

/// What a chat turn shows when the orchestrator has no key to call its model with.
pub(crate) const NO_KEY_MESSAGE: &str = "No OpenAI API key for the orchestrator. Save an OpenAI \
    key in LLM router, or ask your admin to set OPENAI_API_KEY.";

/// Longest provider message carried into a failure the user reads.
const PROVIDER_MESSAGE_MAX_CHARS: usize = 300;

/// Disjoint token counts for one completion, not a running session total.
#[derive(Debug, Clone, Serialize)]
pub struct CallUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub total_tokens: u64,
    pub model: String,
    pub provider: Option<String>,
    pub started_at: DateTime<Utc>,
    pub streaming: bool,
    pub estimated: bool,
}

impl CallUsage {
    pub(crate) fn estimated(&self, input: u64, output: u64) -> Self {
        Self {
            input_tokens: input,
            output_tokens: output,
            total_tokens: input.saturating_add(output),
            estimated: true,
            ..self.clone()
        }
    }
}

#[derive(Clone)]
pub(crate) struct UsageModel {
    provider: Arc<dyn ProviderClient>,
    config: ResolvedConfig,
    identity: Option<String>,
    wording: FailureWording,
}

/// Turns a provider failure into what the user reads: whose key was refused and where to fix
/// it, or the provider's status and its own short message, never its raw body.
#[derive(Clone, Copy)]
struct FailureWording {
    key_source: ApiKeySource,
    provider_is_openai: bool,
}

impl FailureWording {
    fn describe(self, error: ProviderError) -> CompletionError {
        let ProviderError::Status {
            status, message, ..
        } = &error
        else {
            return CompletionError::ProviderError(error.to_string());
        };
        tracing::warn!(status, provider_message = %message, "orchestrator model call failed");
        let provider = if self.provider_is_openai {
            "OpenAI"
        } else {
            "The model provider"
        };
        let text = match (status, self.key_source) {
            (401 | 403, ApiKeySource::Saved) => {
                format!("{provider} rejected your saved API key. Update it in LLM router.")
            }
            (401 | 403, ApiKeySource::Server) => format!(
                "{provider} rejected this server's API key. Ask your admin to check OPENAI_API_KEY."
            ),
            _ => match short_provider_message(message) {
                Some(said) => format!("The model provider returned HTTP {status}: {said}"),
                None => format!("The model provider returned HTTP {status}."),
            },
        };
        CompletionError::ProviderError(text)
    }
}

/// The provider's own one-line explanation from an OpenAI-shaped error body
/// (`{"error":{"message":…}}` or `{"message":…}`), shortened; `None` when the body has none.
fn short_provider_message(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let said = value
        .pointer("/error/message")
        .or_else(|| value.get("message"))?
        .as_str()?
        .trim();
    if said.is_empty() {
        return None;
    }
    let mut short: String = said.chars().take(PROVIDER_MESSAGE_MAX_CHARS).collect();
    if short.len() < said.len() {
        short.push('…');
    }
    Some(short)
}

/// What the user reads for a failed model call: the provider failure's own text, without
/// rig's `ProviderError: ` variant prefix.
pub(crate) fn failure_text(error: &CompletionError) -> String {
    match error {
        CompletionError::ProviderError(text) => text.clone(),
        other => other.to_string(),
    }
}

pub(crate) struct RawCompletion {
    pub usage: Option<CallUsage>,
}

pub(crate) enum CompletionEvent {
    Text(String),
    Tool(ToolCall),
    Usage(CallUsage),
}

impl UsageModel {
    /// The user's saved key (`config.api_key`) → OpenAI-compatible endpoint. Otherwise the
    /// router's platform default provider (`DEFAULT_PROVIDER`, else the first provider with
    /// credentials). The platform `OPENAI_API_KEY` is deliberately not a user key: a Vertex
    /// deployment that adds it only for Whisper must keep chatting on Vertex.
    pub fn from_config(config: &OrchestratorConfig) -> Result<Self, String> {
        let explicit_key: Option<String> = config
            .api_key
            .clone()
            .filter(|k: &String| !k.trim().is_empty());
        match explicit_key {
            Some(api_key) => Ok(Self::openai(config, api_key)),
            None => Self::platform(config),
        }
    }

    fn platform(config: &OrchestratorConfig) -> Result<Self, String> {
        let gateway = nasiko_llm_router::config::GatewayConfig::from_env();
        let provider: String = gateway.default_provider.clone();
        // The orchestrator is the platform's own call: its key, not the one agents fall back to,
        // which the Free plane leaves empty.
        let api_key: String = gateway.service_key_for(&provider).to_string();
        if api_key.is_empty() {
            if provider == "openai" {
                return Err(NO_KEY_MESSAGE.to_string());
            }
            return Err(format!(
                "no LLM credentials: set OPENAI_API_KEY or credentials for provider '{provider}'"
            ));
        }
        let model: String = if config.model.trim().is_empty() {
            gateway.default_model.clone()
        } else {
            config.model.clone()
        };
        let http = reqwest::Client::new();
        let client = nasiko_llm_router::providers::provider_for(&provider, None, &http, &gateway)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            provider: Arc::from(client),
            config: Self::resolved(provider.clone(), model, api_key),
            wording: FailureWording {
                key_source: config.api_key_source,
                provider_is_openai: provider == "openai",
            },
            identity: Some(provider),
        })
    }

    fn openai(config: &OrchestratorConfig, api_key: String) -> Self {
        let base = config
            .base_url
            .clone()
            .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
            .unwrap_or_else(|| "https://api.openai.com/v1".into());
        let fallback: String = nasiko_llm_router::openai_model::direct_openai_fallback();
        let model: String = nasiko_llm_router::openai_model::resolve_direct_chat_model(
            &config.model,
            &base,
            &fallback,
        );
        // OpenAI-compatible wire format does not establish the hosting provider.
        let identity = reqwest::Url::parse(&base)
            .ok()
            .filter(|url| url.host_str() == Some("api.openai.com"))
            .map(|_| "openai".to_owned());
        Self {
            provider: Arc::new(OpenAiProvider::new(reqwest::Client::new(), base)),
            config: Self::resolved("openai".into(), model, api_key),
            wording: FailureWording {
                key_source: config.api_key_source,
                provider_is_openai: identity.is_some(),
            },
            identity,
        }
    }

    fn resolved(provider: String, model: String, api_key: String) -> ResolvedConfig {
        ResolvedConfig {
            provider,
            litellm_model: model.clone(),
            model,
            api_key,
            fallback_models: vec![],
            temperature: None,
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
            // Compression is a per-agent opt-in read from `agents.compress_enabled`.
            // This is the orchestrator's own call against the provider, made from
            // `OrchestratorConfig` with no agent row behind it, so nothing opted in.
            compress_enabled: false,
            // No agent row either, so no agent to key OpenAI's prompt cache by.
            agent_id: String::new(),
        }
    }

    pub fn call_identity(&self, streaming: bool) -> CallUsage {
        CallUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            total_tokens: 0,
            model: self.config.model.clone(),
            provider: self.identity.clone(),
            started_at: Utc::now(),
            streaming,
            estimated: false,
        }
    }

    pub async fn stream_request(
        &self,
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<CompletionEvent, CompletionError>>, CompletionError> {
        let request = wire_request(request)?;
        let identity = self.call_identity(true);
        let span = call_span(&identity);
        let wording = self.wording;
        let upstream = self
            .provider
            .chat_stream(&request, &self.config)
            .await
            .map_err(|error| wording.describe(error))?;
        Ok(Box::pin(async_stream::stream! {
            futures::pin_mut!(upstream);
            let mut tools = BTreeMap::<i64, ToolFragments>::new();
            let mut usage = None;
            let mut failure = None;
            while let Some(chunk) = upstream.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => { failure = Some(wording.describe(error)); break; }
                };
                if let Some(raw) = chunk.usage {
                    // Providers may repeat cumulative snapshots; never sum them.
                    if let Some(reported) = normalized_usage(raw, &identity) {
                        // Retain received counts on the span even if this stream is dropped.
                        record_usage(&span, &reported);
                        usage = Some(reported);
                    }
                }
                for choice in chunk.choices.into_iter().filter(|choice| choice.index == 0) {
                    if let Some(text) = choice.delta.content { yield Ok(CompletionEvent::Text(text)); }
                    for delta in choice.delta.tool_calls.unwrap_or_default() {
                        tools.entry(delta.index).or_default().append(delta);
                    }
                }
            }
            if let Some(usage) = usage {
                record_usage(&span, &usage);
                yield Ok(CompletionEvent::Usage(usage));
            }
            if let Some(error) = failure { yield Err(error); return; }
            for tool in tools.into_values() {
                match tool.finish() {
                    Ok(tool) => yield Ok(CompletionEvent::Tool(tool)),
                    Err(error) => { yield Err(error); return; }
                }
            }
        }))
    }
}

impl CompletionModel for UsageModel {
    type Response = RawCompletion;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let identity = self.call_identity(false);
        let span = call_span(&identity);
        let response = self
            .provider
            .chat(&wire_request(request)?, &self.config)
            .await
            .map_err(|error| self.wording.describe(error))?;
        let usage = response
            .usage
            .and_then(|raw| normalized_usage(raw, &identity));
        if let Some(usage) = &usage {
            record_usage(&span, usage);
        }
        let choice = response
            .choices
            .into_iter()
            .find(|choice| choice.index == 0)
            .ok_or_else(|| CompletionError::ResponseError("missing completion choice".into()))?;
        let mut contents = Vec::new();
        if let Some(text) = choice.message.text() {
            contents.push(AssistantContent::text(text));
        }
        for tool in choice.message.tool_calls.unwrap_or_default() {
            contents.push(AssistantContent::ToolCall(
                ToolFragments {
                    id: tool.id,
                    name: tool.function.name,
                    arguments: tool.function.arguments,
                }
                .finish()?,
            ));
        }
        let choice = OneOrMany::many(contents)
            .map_err(|error| CompletionError::ResponseError(error.to_string()))?;
        Ok(CompletionResponse {
            choice,
            raw_response: RawCompletion { usage },
        })
    }
}

fn wire_request(request: CompletionRequest) -> Result<ChatRequest, CompletionError> {
    let mut messages = Vec::<openai::Message>::new();
    if let Some(preamble) = &request.preamble {
        messages.push(openai::Message::system(preamble));
    }
    for message in request
        .chat_history
        .iter()
        .cloned()
        .chain(std::iter::once(request.prompt_with_context()))
    {
        let converted: Vec<openai::Message> = message.try_into()?;
        messages.extend(converted);
    }
    let messages: Vec<WireMessage> = serde_json::from_value(serde_json::to_value(messages)?)?;
    let tools = if request.tools.is_empty() {
        None
    } else {
        Some(serde_json::from_value(serde_json::to_value(
            request
                .tools
                .into_iter()
                .map(openai::ToolDefinition::from)
                .collect::<Vec<_>>(),
        )?)?)
    };
    Ok(ChatRequest {
        model: None,
        messages,
        tool_choice: tools.as_ref().map(|_| json!("auto")),
        tools,
        temperature: request.temperature,
        max_tokens: request
            .max_tokens
            .map(|value| value.min(i64::MAX as u64) as i64),
        stream: None,
        extra: request
            .additional_params
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default(),
    })
}

fn normalized_usage(mut raw: Usage, identity: &CallUsage) -> Option<CallUsage> {
    let prompt = raw.prompt_tokens?;
    let output = raw.completion_tokens.or_else(|| {
        // Flat cache fields are already disjoint; subtract them from the full
        // total as well when output is absent. Nested OpenAI cache is inclusive.
        let cache = raw
            .cache_read_input_tokens
            .unwrap_or(0)
            .checked_add(raw.cache_creation_input_tokens.unwrap_or(0))?;
        raw.total_tokens?.checked_sub(prompt)?.checked_sub(cache)
    })?;
    let counts = [
        Some(prompt),
        Some(output),
        raw.total_tokens,
        raw.cache_read_input_tokens,
        raw.cache_creation_input_tokens,
        raw.prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens),
    ];
    if counts.into_iter().flatten().any(|count| count < 0) {
        return None;
    }
    if raw.cache_read_input_tokens.is_none()
        && raw
            .prompt_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens)
            .is_some_and(|cached| cached > prompt)
    {
        return None;
    }
    raw.normalize_openai_details();
    let input = raw.prompt_tokens? as u64;
    let read = raw.cache_read_input_tokens.unwrap_or(0) as u64;
    let creation = raw.cache_creation_input_tokens.unwrap_or(0) as u64;
    let output = output as u64;
    Some(CallUsage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: read,
        cache_creation_tokens: creation,
        total_tokens: input
            .saturating_add(output)
            .saturating_add(read)
            .saturating_add(creation),
        ..identity.clone()
    })
}

#[derive(Default)]
struct ToolFragments {
    id: String,
    name: String,
    arguments: String,
}

impl ToolFragments {
    fn append(&mut self, delta: ToolCallDelta) {
        if let Some(id) = delta.id {
            self.id = id;
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name {
                self.name.push_str(&name);
            }
            if let Some(arguments) = function.arguments {
                self.arguments.push_str(&arguments);
            }
        }
    }
    fn finish(self) -> Result<ToolCall, CompletionError> {
        if self.id.is_empty() || self.name.is_empty() {
            return Err(CompletionError::ResponseError(
                "tool call missing id or name".into(),
            ));
        }
        Ok(ToolCall {
            id: self.id,
            function: ToolFunction {
                name: self.name,
                arguments: serde_json::from_str(&self.arguments)?,
            },
        })
    }
}

fn call_span(usage: &CallUsage) -> tracing::Span {
    tracing::info_span!("gen_ai.orchestrator.chat", otel.kind = "client", gen_ai.operation.name = "chat",
        gen_ai.request.model = %usage.model, gen_ai.provider.name = usage.provider.as_deref().unwrap_or("unknown"),
        nasiko.usage.prompt_convention = "exclusive",
        gen_ai.usage.input_tokens = tracing::field::Empty, gen_ai.usage.output_tokens = tracing::field::Empty,
        gen_ai.usage.cache_read_input_tokens = tracing::field::Empty, gen_ai.usage.cache_creation_input_tokens = tracing::field::Empty,
        gen_ai.usage.total_tokens = tracing::field::Empty)
}

fn record_usage(span: &tracing::Span, usage: &CallUsage) {
    span.record("gen_ai.usage.input_tokens", usage.input_tokens);
    span.record("gen_ai.usage.output_tokens", usage.output_tokens);
    span.record(
        "gen_ai.usage.cache_read_input_tokens",
        usage.cache_read_tokens,
    );
    span.record(
        "gen_ai.usage.cache_creation_input_tokens",
        usage.cache_creation_tokens,
    );
    span.record("gen_ai.usage.total_tokens", usage.total_tokens);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_llm_router::{
        ResolvedEmbeddingConfig,
        ir::{ChatChunk, ChatResponse, EmbeddingsRequest, EmbeddingsResponse},
        providers::ProviderError,
    };
    use rig::completion::Message;
    use std::sync::Mutex;

    fn refused(status: u16, body: &str) -> ProviderError {
        ProviderError::Status {
            status,
            message: body.into(),
            retryable: false,
        }
    }

    fn openai_wording(key_source: ApiKeySource) -> FailureWording {
        FailureWording {
            key_source,
            provider_is_openai: true,
        }
    }

    #[test]
    fn a_blank_key_is_no_key_and_says_where_to_add_one() {
        let error = UsageModel::from_config(&OrchestratorConfig {
            api_key: Some("  ".into()),
            ..Default::default()
        })
        .err()
        .unwrap();
        assert_eq!(error, NO_KEY_MESSAGE);
        assert!(error.contains("LLM router") && error.contains("OPENAI_API_KEY"));
    }

    #[test]
    fn a_refused_key_says_whose_key_and_never_echoes_the_body() {
        let body = r#"{"error":{"message":"Incorrect API key provided: sk-abc"}}"#;
        let saved = failure_text(&openai_wording(ApiKeySource::Saved).describe(refused(401, body)));
        assert_eq!(
            saved,
            "OpenAI rejected your saved API key. Update it in LLM router."
        );
        let server =
            failure_text(&openai_wording(ApiKeySource::Server).describe(refused(403, body)));
        assert_eq!(
            server,
            "OpenAI rejected this server's API key. Ask your admin to check OPENAI_API_KEY."
        );
        assert!(!saved.contains("sk-abc") && !server.contains("sk-abc"));
    }

    #[test]
    fn another_status_carries_the_providers_short_message_or_just_the_status() {
        let wording = openai_wording(ApiKeySource::Server);
        let missing_model = r#"{"error":{"message":"The model `gpt-4.0` does not exist","code":"model_not_found"}}"#;
        assert_eq!(
            failure_text(&wording.describe(refused(404, missing_model))),
            "The model provider returned HTTP 404: The model `gpt-4.0` does not exist"
        );
        assert_eq!(
            failure_text(&wording.describe(refused(502, "<html>bad gateway</html>"))),
            "The model provider returned HTTP 502."
        );
        let long = format!(r#"{{"message":"{}"}}"#, "x".repeat(400));
        let text = failure_text(&wording.describe(refused(400, &long)));
        assert!(text.ends_with('…') && text.chars().count() < 360, "{text}");
    }

    #[test]
    fn a_transport_failure_keeps_its_own_text() {
        let wording = openai_wording(ApiKeySource::Saved);
        assert_eq!(
            failure_text(&wording.describe(ProviderError::Transport("connection refused".into()))),
            "transport error: connection refused"
        );
    }

    struct FakeProvider {
        response: serde_json::Value,
        chunks: Vec<serde_json::Value>,
        fail: bool,
        requests: Mutex<Vec<serde_json::Value>>,
    }

    #[async_trait::async_trait]
    impl ProviderClient for FakeProvider {
        async fn chat(
            &self,
            request: &ChatRequest,
            _: &ResolvedConfig,
        ) -> Result<ChatResponse, ProviderError> {
            self.requests
                .lock()
                .unwrap()
                .push(serde_json::to_value(request).unwrap());
            Ok(serde_json::from_value(self.response.clone()).unwrap())
        }
        async fn chat_stream(
            &self,
            request: &ChatRequest,
            _: &ResolvedConfig,
        ) -> Result<BoxStream<'static, Result<ChatChunk, ProviderError>>, ProviderError> {
            self.requests
                .lock()
                .unwrap()
                .push(serde_json::to_value(request).unwrap());
            let mut chunks: Vec<_> = self
                .chunks
                .iter()
                .cloned()
                .map(|chunk| Ok(serde_json::from_value(chunk).unwrap()))
                .collect();
            if self.fail {
                chunks.push(Err(ProviderError::Transport("disconnected".into())));
            }
            Ok(Box::pin(futures::stream::iter(chunks)))
        }
        async fn embeddings(
            &self,
            _: &EmbeddingsRequest,
            _: &ResolvedEmbeddingConfig,
        ) -> Result<EmbeddingsResponse, ProviderError> {
            unreachable!("completion adapter does not embed")
        }
    }

    #[test]
    fn saved_key_resolves_empty_and_foreign_models_on_official_host_only() {
        let build = |model: &str, base: &str| -> String {
            let m: UsageModel = UsageModel::from_config(&OrchestratorConfig {
                api_key: Some("test-only".into()),
                base_url: Some(base.into()),
                model: model.into(),
                ..Default::default()
            })
            .unwrap();
            m.call_identity(false).model
        };
        let fallback: String = nasiko_llm_router::openai_model::direct_openai_fallback();
        assert_eq!(build("", "https://api.openai.com/v1"), fallback);
        assert_eq!(
            build("gemini-4-hydrogen", "https://api.openai.com/v1"),
            fallback
        );
        assert_eq!(
            build("gemini-4-hydrogen", "https://llm.example.com/v1"),
            "gemini-4-hydrogen"
        );
        assert_eq!(
            build("gpt-6.1-sol", "https://api.openai.com/v1"),
            "gpt-6.1-sol"
        );
    }

    fn model(chunks: Vec<serde_json::Value>, fail: bool) -> (UsageModel, Arc<FakeProvider>) {
        let mut model = UsageModel::from_config(&OrchestratorConfig {
            api_key: Some("test-only".into()),
            base_url: Some("https://api.openai.com/v1".into()),
            ..Default::default()
        })
        .unwrap();
        let fake = Arc::new(FakeProvider {
            response: json!({"id":"c", "model":"test", "choices":[{"index":0,"message":{"role":"assistant","content":"answer"}}], "usage": raw_usage()}),
            chunks,
            fail,
            requests: Mutex::new(vec![]),
        });
        model.provider = fake.clone();
        (model, fake)
    }

    fn raw_usage() -> serde_json::Value {
        json!({"prompt_tokens":4732,"completion_tokens":110,"total_tokens":4842,"prompt_tokens_details":{"cached_tokens":3968}})
    }
    fn usage_chunk() -> serde_json::Value {
        json!({"id":"c","model":"test","choices":[],"usage":raw_usage()})
    }
    fn delta(value: serde_json::Value) -> serde_json::Value {
        json!({"id":"c","model":"test","choices":[{"index":0,"delta":value}]})
    }
    fn assert_usage(usage: &CallUsage) {
        assert_eq!(
            (
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_tokens,
                usage.cache_creation_tokens
            ),
            (764, 110, 3968, 0)
        );
        assert_eq!(usage.total_tokens, 4842);
        assert!(!usage.estimated);
    }

    #[tokio::test]
    async fn buffered_completion_preserves_request_and_cache_usage() {
        let (model, fake) = model(vec![], false);
        let response = model
            .completion_request(Message::user("question"))
            .preamble("stable prefix".into())
            .temperature(0.2)
            .max_tokens(42)
            .tools(vec![rig::completion::ToolDefinition {
                name: "call_agent".into(),
                description: "delegate".into(),
                parameters: json!({"type":"object"}),
            }])
            .send()
            .await
            .unwrap();
        assert_usage(response.raw_response.usage.as_ref().unwrap());
        let requests = fake.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["messages"][0]["role"], "system");
        assert_eq!(requests[0]["messages"][1]["role"], "user");
        assert_eq!(requests[0]["tools"][0]["function"]["name"], "call_agent");
        assert_eq!(requests[0]["temperature"], 0.2);
        assert_eq!(requests[0]["max_tokens"], 42);
    }

    #[tokio::test]
    async fn terminal_usage_is_once_and_tools_keep_fragmented_ids_and_arguments() {
        let chunks = vec![
            delta(json!({"content":"working"})),
            delta(
                json!({"tool_calls":[{"index":0,"id":"tool-a","function":{"name":"first","arguments":"{\"a\":"}}]}),
            ),
            delta(
                json!({"tool_calls":[{"index":1,"id":"tool-b","function":{"name":"second","arguments":"{}"}}, {"index":0,"function":{"arguments":"1}"}}]}),
            ),
            json!({"id":"c","model":"test","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
            usage_chunk(),
            usage_chunk(),
        ];
        let (model, _) = model(chunks, false);
        let events: Vec<_> = model
            .stream_request(model.completion_request("question").build())
            .await
            .unwrap()
            .collect()
            .await;
        assert!(matches!(&events[0], Ok(CompletionEvent::Text(text)) if text == "working"));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Ok(CompletionEvent::Usage(_))))
                .count(),
            1
        );
        match &events[1] {
            Ok(CompletionEvent::Usage(usage)) => assert_usage(usage),
            _ => panic!("usage precedes tools"),
        }
        match &events[2] {
            Ok(CompletionEvent::Tool(tool)) => {
                assert_eq!(tool.id, "tool-a");
                assert_eq!(tool.function.arguments, json!({"a":1}));
            }
            _ => panic!("first tool"),
        }
        match &events[3] {
            Ok(CompletionEvent::Tool(tool)) => assert_eq!(tool.id, "tool-b"),
            _ => panic!("second tool"),
        }
    }

    #[tokio::test]
    async fn failure_preserves_received_usage_but_does_not_invent_missing_usage() {
        for reported in [false, true] {
            let (model, _) = model(
                if reported {
                    vec![usage_chunk()]
                } else {
                    vec![]
                },
                true,
            );
            let events: Vec<_> = model
                .stream_request(model.completion_request("q").build())
                .await
                .unwrap()
                .collect()
                .await;
            assert!(events.last().unwrap().is_err());
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, Ok(CompletionEvent::Usage(_))))
                    .count(),
                usize::from(reported)
            );
        }
    }

    #[test]
    fn zero_all_cache_missing_and_invalid_counts_are_distinct() {
        let (model, _) = model(vec![], false);
        let identity = model.call_identity(false);
        for (prompt, cache, output) in [(0, 0, 0), (100, 100, 5), (100, 0, 5)] {
            let raw = serde_json::from_value(json!({"prompt_tokens":prompt,"completion_tokens":output,"prompt_tokens_details":{"cached_tokens":cache}})).unwrap();
            let usage = normalized_usage(raw, &identity).unwrap();
            assert_eq!(usage.input_tokens, prompt - cache);
            assert_eq!(usage.total_tokens, prompt + output);
        }
        for raw in [
            json!({}),
            json!({"prompt_tokens":-1,"completion_tokens":1}),
            json!({"prompt_tokens":1,"completion_tokens":1,"prompt_tokens_details":{"cached_tokens":2}}),
        ] {
            assert!(normalized_usage(serde_json::from_value(raw).unwrap(), &identity).is_none());
        }
    }

    #[tokio::test]
    async fn simultaneous_streams_do_not_share_usage() {
        let (first, _) = model(vec![usage_chunk()], false);
        let mut different = usage_chunk();
        different["usage"] = json!({"prompt_tokens":10,"completion_tokens":2});
        let (second, _) = model(vec![different], false);
        let (a, b) = tokio::join!(
            first.stream_request(first.completion_request("a").build()),
            second.stream_request(second.completion_request("b").build())
        );
        let (a, b): (Vec<_>, Vec<_>) = tokio::join!(a.unwrap().collect(), b.unwrap().collect());
        match &a[0] {
            Ok(CompletionEvent::Usage(usage)) => assert_usage(usage),
            _ => panic!("first usage"),
        }
        match &b[0] {
            Ok(CompletionEvent::Usage(usage)) => assert_eq!(usage.total_tokens, 12),
            _ => panic!("second usage"),
        }
    }
}
