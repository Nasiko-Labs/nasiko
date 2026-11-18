//! In-process entry point for the platform's own LLM calls (orchestrator, MAF, selector).
//!
//! The orchestrator runs in the same process as the router, so it needs no token and no
//! traceparent: it calls [`PlatformLlm`] directly. Calls resolve to the platform defaults
//! (`DEFAULT_PROVIDER` / `DEFAULT_MODEL`, else the first provider with credentials), use
//! the platform key, and are written to `trace_usage` by the router — the single place
//! usage is recorded.

use std::sync::Arc;
use std::time::Instant;

use crate::LlmRouterCtx;
use crate::error::GatewayError;
use crate::ir::{ChatRequest, ChatResponse, EmbeddingsRequest, EmbeddingsResponse, Message};
use crate::providers::attempts::AttemptLog;
use crate::providers::execute_embeddings_once;
use crate::providers::fallback::{execute_chat_by, provider_deadline};
use crate::resolver::{ResolvedConfig, ResolvedEmbeddingConfig};
use crate::usage::{self, CallScope, UsageRecord};

/// Who a platform call is billed to and where its usage row lands.
#[derive(Debug, Clone)]
pub struct PlatformCall {
    /// The run owner (`trace_usage.user_id`).
    pub user_id: String,
    /// `trace_usage.agent_name`, e.g. `maf`, `maf:step2`, `orchestrator:selector`.
    pub component: String,
    /// `trace_usage.trace_id` / `session_id`; `None` writes a `direct:` row.
    pub session_id: Option<String>,
}

/// Platform LLM handle. Cheap to clone.
#[derive(Clone)]
pub struct PlatformLlm {
    ctx: Arc<LlmRouterCtx>,
}

impl PlatformLlm {
    pub fn new(ctx: LlmRouterCtx) -> Self {
        Self { ctx: Arc::new(ctx) }
    }

    /// The model a chat call without an explicit model runs on.
    pub fn default_model(&self) -> &str {
        &self.ctx.cfg.default_model
    }

    /// Provider and model an [`PlatformLlm::embed`] call for `requested` resolves to.
    pub fn resolve_embedding(&self, requested: &str) -> EmbeddingRoute {
        embedding_route(&self.ctx.cfg, requested)
    }

    /// Whether some platform provider can serve an embeddings call for `requested`.
    pub fn can_embed(&self, requested: &str) -> bool {
        let route: EmbeddingRoute = self.resolve_embedding(requested);
        embedding_model_fits(&route.model, &route.provider)
    }

    /// The provider platform calls go to.
    pub fn default_provider(&self) -> &str {
        &self.ctx.cfg.default_provider
    }

    /// Non-streaming chat on the platform default provider, with usage logged.
    pub async fn chat(
        &self,
        call: &PlatformCall,
        req: ChatRequest,
    ) -> Result<ChatResponse, GatewayError> {
        let resolved: ResolvedConfig = platform_chat_config(&self.ctx, req.model.as_deref())?;
        let started = Instant::now();
        let mut attempts = AttemptLog::default();
        let result = execute_chat_by(
            &self.ctx.http,
            &self.ctx.cfg,
            &resolved,
            &req,
            provider_deadline(&self.ctx.cfg),
            &mut attempts,
        )
        .await;
        let scope = self.scope(call, "direct_llm");
        usage::spawn_log_failed(
            &self.ctx.db,
            &self.ctx.pricing,
            &self.ctx.ledger,
            &scope,
            attempts.take_failed(),
        );
        let (resp, (provider, model)) = result?;
        let finish_reason: Option<String> =
            resp.choices.first().and_then(|c| c.finish_reason.clone());
        self.log(
            call,
            "direct_llm",
            provider,
            model,
            resp.usage.clone(),
            started,
            finish_reason,
        )
        .await;
        Ok(resp)
    }

    /// One embeddings call on `provider`/`model` (platform key, no fallback).
    ///
    /// Without an explicit provider the model picks it (`embedding_route`), so embeddings
    /// no longer ride the chat provider.
    pub async fn embed(
        &self,
        call: &PlatformCall,
        provider: Option<&str>,
        req: EmbeddingsRequest,
    ) -> Result<EmbeddingsResponse, GatewayError> {
        let requested: String = req
            .model
            .clone()
            .ok_or_else(|| GatewayError::BadRequest("embedding model required".into()))?;
        let route: EmbeddingRoute = match provider {
            Some(p) => EmbeddingRoute {
                provider: p.to_string(),
                model: requested,
            },
            None => embedding_route(&self.ctx.cfg, &requested),
        };
        let provider: String = route.provider;
        let model: String = route.model;
        let mut req = req;
        req.model = Some(model.clone());
        let api_key = self.ctx.cfg.service_key_for(&provider).to_string();
        if api_key.is_empty() {
            return Err(GatewayError::BadRequest(format!(
                "no platform credentials for provider '{provider}'"
            )));
        }
        let resolved = ResolvedEmbeddingConfig {
            provider: provider.clone(),
            model: model.clone(),
            api_key,
            custom_endpoint: None,
            platform_paid: true,
        };
        let started = Instant::now();
        let mut attempts = AttemptLog::default();
        let result = execute_embeddings_once(
            &self.ctx.http,
            &self.ctx.cfg,
            &resolved,
            &req,
            &mut attempts,
        )
        .await;
        let scope = self.scope(call, "embedding");
        usage::spawn_log_failed(
            &self.ctx.db,
            &self.ctx.pricing,
            &self.ctx.ledger,
            &scope,
            attempts.take_failed(),
        );
        let resp = result?;
        self.log(
            call,
            "embedding",
            provider,
            model,
            resp.usage.clone(),
            started,
            None,
        )
        .await;
        Ok(resp)
    }

    /// Speech to text on the platform default provider, with usage logged.
    ///
    /// OpenAI uses its `/audio/transcriptions` endpoint (`whisper-1` unless `req.model`);
    /// Gemini and Vertex send the audio inline to a chat model (`DEFAULT_MODEL` unless
    /// `req.model`). Other providers have no audio input and return `BadRequest`.
    pub async fn transcribe(
        &self,
        call: &PlatformCall,
        req: TranscribeRequest,
    ) -> Result<Transcript, GatewayError> {
        if req.audio.is_empty() {
            return Err(GatewayError::BadRequest("audio is empty".into()));
        }
        if req.audio.len() > MAX_TRANSCRIBE_BYTES {
            return Err(GatewayError::BadRequest(format!(
                "audio is {} bytes; the limit is {MAX_TRANSCRIBE_BYTES}",
                req.audio.len()
            )));
        }
        let provider: &str = self.default_provider();
        let has_openai_key: bool = !self.ctx.cfg.service_key_for("openai").is_empty();
        match transcription_route_for(provider, has_openai_key) {
            Some(TranscriptionRoute::Whisper) => self.transcribe_whisper(call, req).await,
            Some(TranscriptionRoute::ChatAudio) => {
                let chat: ChatRequest = transcription_chat_request(&req);
                let resp: ChatResponse = self.chat(call, chat).await?;
                let text: String = resp
                    .choices
                    .first()
                    .and_then(|c| c.message.text())
                    .unwrap_or_default();
                Ok(Transcript {
                    text: text.trim().to_string(),
                })
            }
            None => Err(GatewayError::BadRequest(format!(
                "provider '{provider}' does not support transcription"
            ))),
        }
    }

    async fn transcribe_whisper(
        &self,
        call: &PlatformCall,
        req: TranscribeRequest,
    ) -> Result<Transcript, GatewayError> {
        let api_key: String = self.ctx.cfg.service_key_for("openai").to_string();
        if api_key.is_empty() {
            return Err(GatewayError::BadRequest(
                "no platform credentials for provider 'openai'".into(),
            ));
        }
        let model: String = req
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| WHISPER_MODEL.to_string());
        let format: String = audio_format(&req.mime_type);
        let part = reqwest::multipart::Part::bytes(req.audio)
            .file_name(format!("audio.{format}"))
            .mime_str(&req.mime_type)
            .map_err(|e| GatewayError::BadRequest(format!("invalid audio mime type: {e}")))?;
        let form = reqwest::multipart::Form::new()
            .text("model", model.clone())
            .text("response_format", "json")
            .part("file", part);
        let url: String = format!(
            "{}/audio/transcriptions",
            self.ctx.cfg.openai_api_base.trim_end_matches('/')
        );
        let started = Instant::now();
        let res = self
            .ctx
            .http
            .post(&url)
            .bearer_auth(api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| GatewayError::Upstream(format!("transcription request failed: {e}")))?;
        let status = res.status();
        if !status.is_success() {
            let body: String = res.text().await.unwrap_or_default();
            return Err(GatewayError::Upstream(format!(
                "transcription API {status}: {body}"
            )));
        }
        let body: serde_json::Value = res
            .json()
            .await
            .map_err(|e| GatewayError::Upstream(format!("invalid transcription response: {e}")))?;
        let text: String = body
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        // Whisper bills per audio minute and reports no tokens: the row records the call.
        self.log(
            call,
            "transcription",
            "openai".to_string(),
            model,
            None,
            started,
            None,
        )
        .await;
        Ok(Transcript { text })
    }

    fn scope(&self, call: &PlatformCall, operation_type: &'static str) -> CallScope {
        CallScope {
            owner_id: call.user_id.clone(),
            agent_id: call.component.clone(),
            operation_type,
            streaming: false,
            flow_id: call.session_id.clone(),
            attribution_source: None,
            request_bytes: None,
            routing_metadata: None,
        }
    }

    /// Writes the usage row before returning, so a caller that reads spend back (MAF's
    /// `platform_spend`) sees this call. The write runs as its own task: a caller dropped
    /// mid-await cannot roll back the record of a call that was already paid for.
    #[allow(clippy::too_many_arguments)]
    async fn log(
        &self,
        call: &PlatformCall,
        operation_type: &'static str,
        provider: String,
        model: String,
        usage: Option<crate::ir::Usage>,
        started: Instant,
        finish_reason: Option<String>,
    ) {
        let cached_tokens: Option<i64> = usage.as_ref().and_then(|u| u.cache_read_input_tokens);
        let db = self.ctx.db.clone();
        let pricing = self.ctx.pricing.clone();
        let ledger = self.ctx.ledger.clone();
        let record = UsageRecord {
            owner_id: call.user_id.clone(),
            agent_id: call.component.clone(),
            operation_type,
            provider,
            model,
            usage,
            cached_tokens,
            reasoning_tokens: None,
            latency_ms: started.elapsed().as_millis() as i64,
            streaming: false,
            finish_reason,
            flow_id: call.session_id.clone(),
            attribution_source: None,
            platform_paid: true,
            routing_metadata: None,
            compress_metadata: None,
            brevity_metadata: None,
            compact_tools_metadata: None,
            compress_bytes: None,
            compact_tools_bytes: None,
            request_bytes: None,
            failure: None,
        };
        let write = tokio::spawn(async move {
            usage::log_usage(db, pricing.as_ref(), ledger.as_ref(), record).await
        });
        match write.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(error = %e, "platform usage write failed (swallowed)"),
            Err(e) => tracing::warn!(error = %e, "platform usage write task failed (swallowed)"),
        }
    }
}

/// Largest audio upload [`PlatformLlm::transcribe`] accepts. Gemini caps an inline
/// request at 20 MB and base64 grows the bytes by a third, so 14 MB of raw audio fits.
pub const MAX_TRANSCRIBE_BYTES: usize = 14 * 1024 * 1024;

const WHISPER_MODEL: &str = "whisper-1";

const TRANSCRIBE_PROMPT: &str = "Transcribe the speech in this audio verbatim. Reply with \
     only the transcript text: no preamble, no quotes, no timestamps. If there is no \
     speech, reply with nothing.";

/// Audio to transcribe.
#[derive(Debug, Clone)]
pub struct TranscribeRequest {
    pub audio: Vec<u8>,
    /// Upload content type, e.g. `audio/webm;codecs=opus`.
    pub mime_type: String,
    /// Model override; `None` uses `whisper-1` (OpenAI) or `DEFAULT_MODEL` (Gemini/Vertex).
    pub model: Option<String>,
}

/// Transcribed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub text: String,
}

/// How a provider turns audio into text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptionRoute {
    /// OpenAI multipart `/audio/transcriptions`.
    Whisper,
    /// Inline `input_audio` part on a chat model.
    ChatAudio,
}

fn transcription_route(provider: &str) -> Option<TranscriptionRoute> {
    match provider {
        "openai" => Some(TranscriptionRoute::Whisper),
        "gemini" | "vertex" => Some(TranscriptionRoute::ChatAudio),
        _ => None,
    }
}

/// Route for the chat default `provider`; a provider without audio input (Anthropic,
/// OpenRouter) falls back to Whisper when the platform has an OpenAI key, so the chat
/// default never disables voice input.
fn transcription_route_for(provider: &str, has_openai_key: bool) -> Option<TranscriptionRoute> {
    transcription_route(provider).or(has_openai_key.then_some(TranscriptionRoute::Whisper))
}

/// `input_audio.format` for a content type: `audio/webm;codecs=opus` → `webm`,
/// `audio/mpeg` → `mp3`; anything unparseable → `webm` (what browsers record).
fn audio_format(mime_type: &str) -> String {
    let subtype: &str = mime_type
        .split(';')
        .next()
        .and_then(|m| m.trim().strip_prefix("audio/"))
        .unwrap_or("")
        .trim();
    match subtype {
        "" => "webm".to_string(),
        "mpeg" | "mpga" => "mp3".to_string(),
        "x-wav" | "wave" => "wav".to_string(),
        "x-m4a" | "mp4" => "m4a".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

/// Chat request asking the model to transcribe `req.audio`, at temperature 0.
fn transcription_chat_request(req: &TranscribeRequest) -> ChatRequest {
    use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
    let content = serde_json::json!([
        { "type": "text", "text": TRANSCRIBE_PROMPT },
        {
            "type": "input_audio",
            "input_audio": {
                "data": BASE64.encode(&req.audio),
                "format": audio_format(&req.mime_type),
            },
        },
    ]);
    ChatRequest {
        model: req.model.clone().filter(|m| !m.is_empty()),
        messages: vec![Message {
            role: "user".into(),
            content: Some(content),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }],
        tools: None,
        tool_choice: None,
        temperature: Some(0.0),
        max_tokens: None,
        stream: None,
        extra: Default::default(),
    }
}

#[cfg(test)]
mod transcribe_tests {
    use super::*;

    fn req(mime: &str) -> TranscribeRequest {
        TranscribeRequest {
            audio: vec![1, 2, 3],
            mime_type: mime.into(),
            model: None,
        }
    }

    #[test]
    fn route_per_provider() {
        assert_eq!(
            transcription_route("openai"),
            Some(TranscriptionRoute::Whisper)
        );
        assert_eq!(
            transcription_route("vertex"),
            Some(TranscriptionRoute::ChatAudio)
        );
        assert_eq!(
            transcription_route("gemini"),
            Some(TranscriptionRoute::ChatAudio)
        );
        assert_eq!(transcription_route("anthropic"), None);
        assert_eq!(transcription_route("openrouter"), None);
    }

    #[test]
    fn audio_format_from_mime() {
        assert_eq!(audio_format("audio/webm;codecs=opus"), "webm");
        assert_eq!(audio_format("audio/ogg"), "ogg");
        assert_eq!(audio_format("audio/mpeg"), "mp3");
        assert_eq!(audio_format("audio/x-wav"), "wav");
        assert_eq!(audio_format("application/octet-stream"), "webm");
        assert_eq!(audio_format(""), "webm");
    }

    #[test]
    fn chat_request_carries_prompt_and_inline_audio() {
        let chat = transcription_chat_request(&req("audio/ogg"));
        assert_eq!(chat.temperature, Some(0.0));
        assert!(chat.model.is_none(), "no override leaves DEFAULT_MODEL");
        let parts = chat.messages[0]
            .content
            .as_ref()
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(parts[0]["text"], TRANSCRIBE_PROMPT);
        assert_eq!(parts[1]["input_audio"]["data"], "AQID");
        assert_eq!(parts[1]["input_audio"]["format"], "ogg");
    }

    #[test]
    fn blank_model_override_is_ignored() {
        let mut r = req("audio/webm");
        r.model = Some(String::new());
        assert!(transcription_chat_request(&r).model.is_none());
    }
}

/// Platform-default destination for a chat call: `model` when given, else `DEFAULT_MODEL`,
/// on `DEFAULT_PROVIDER` with the platform's own key (`GatewayConfig::service_key_for`).
fn platform_chat_config(
    ctx: &LlmRouterCtx,
    model: Option<&str>,
) -> Result<ResolvedConfig, GatewayError> {
    let provider: String = ctx.cfg.default_provider.clone();
    let model: String = model
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| ctx.cfg.default_model.clone());
    let api_key = ctx.cfg.service_key_for(&provider).to_string();
    if api_key.is_empty() {
        return Err(GatewayError::BadRequest(format!(
            "no platform credentials for provider '{provider}'"
        )));
    }
    Ok(ResolvedConfig {
        litellm_model: format!("{provider}/{model}"),
        provider,
        model,
        api_key,
        fallback_models: Vec::new(),
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
        compress_enabled: false,
        agent_id: String::new(),
    })
}

/// Provider + model one platform embeddings call runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingRoute {
    pub provider: String,
    pub model: String,
}

/// Default embedding model per built-in provider.
fn default_embedding_model(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("text-embedding-3-small"),
        "gemini" => Some("gemini-embedding-001"),
        "vertex" => Some("text-embedding-005"),
        _ => None,
    }
}

/// Whether `provider` serves embedding `model` (name-prefix heuristic).
fn embedding_model_fits(model: &str, provider: &str) -> bool {
    let m: String = model.trim().to_ascii_lowercase();
    match provider {
        "openai" => m.starts_with("text-embedding-3") || m.starts_with("text-embedding-ada"),
        "gemini" => m.starts_with("gemini-embedding") || m.starts_with("text-embedding-004"),
        "vertex" => {
            m.starts_with("gemini-embedding")
                || m.starts_with("text-embedding-00")
                || m.starts_with("text-multilingual-embedding")
        }
        // anthropic and openrouter expose no embeddings API.
        _ => false,
    }
}

/// Built-in providers with an embeddings API, in fallback order.
const EMBEDDING_PROVIDERS: [&str; 3] = ["openai", "gemini", "vertex"];

/// Picks the embedding provider from the model, not the chat provider.
///
/// 1. Default provider serves the model → default provider.
/// 2. Another keyed provider serves it → that provider.
/// 3. Default provider can embed → its own default embedding model.
/// 4. Default cannot embed (anthropic, openrouter) → first keyed embedding provider
///    with its default model.
pub fn embedding_route(cfg: &crate::GatewayConfig, requested: &str) -> EmbeddingRoute {
    let default: &str = cfg.default_provider.as_str();
    if embedding_model_fits(requested, default) {
        return EmbeddingRoute {
            provider: default.to_string(),
            model: requested.to_string(),
        };
    }
    for p in EMBEDDING_PROVIDERS {
        if p != default && embedding_model_fits(requested, p) && !cfg.service_key_for(p).is_empty()
        {
            return EmbeddingRoute {
                provider: p.to_string(),
                model: requested.to_string(),
            };
        }
    }
    if let Some(model) = default_embedding_model(default) {
        tracing::info!(
            requested,
            provider = default,
            model,
            "embedding model swapped to provider default"
        );
        return EmbeddingRoute {
            provider: default.to_string(),
            model: model.to_string(),
        };
    }
    for p in EMBEDDING_PROVIDERS {
        if !cfg.service_key_for(p).is_empty()
            && let Some(model) = default_embedding_model(p)
        {
            tracing::info!(
                requested,
                provider = p,
                model,
                "default provider cannot embed; using keyed embedding provider"
            );
            return EmbeddingRoute {
                provider: p.to_string(),
                model: model.to_string(),
            };
        }
    }
    tracing::warn!(
        requested,
        provider = default,
        "no platform provider can serve embeddings"
    );
    EmbeddingRoute {
        provider: default.to_string(),
        model: requested.to_string(),
    }
}

#[cfg(test)]
mod embedding_route_tests {
    use super::*;
    use crate::GatewayConfig;

    fn cfg(default: &str, openai: &str, gemini: &str) -> GatewayConfig {
        GatewayConfig {
            default_provider: default.into(),
            platform_openai_api_key: openai.into(),
            platform_gemini_api_key: gemini.into(),
            ..Default::default()
        }
    }

    #[test]
    fn gemini_only_swaps_openai_embedding_model() {
        let r: EmbeddingRoute = embedding_route(&cfg("gemini", "", "g"), "text-embedding-3-small");
        assert_eq!(
            r,
            EmbeddingRoute {
                provider: "gemini".into(),
                model: "gemini-embedding-001".into()
            }
        );
    }

    #[test]
    fn gemini_model_kept_on_gemini() {
        let r: EmbeddingRoute = embedding_route(&cfg("gemini", "", "g"), "gemini-embedding-001");
        assert_eq!(r.provider, "gemini");
        assert_eq!(r.model, "gemini-embedding-001");
    }

    #[test]
    fn keyed_openai_serves_openai_model_when_chat_is_gemini() {
        let r: EmbeddingRoute =
            embedding_route(&cfg("gemini", "sk", "g"), "text-embedding-3-small");
        assert_eq!(
            r,
            EmbeddingRoute {
                provider: "openai".into(),
                model: "text-embedding-3-small".into()
            }
        );
    }

    /// Agents' platform key off (the Free plane): the platform still embeds on its own keys.
    #[test]
    fn routes_on_the_platforms_own_keys_when_agents_have_none() {
        let c: GatewayConfig = GatewayConfig {
            default_provider: "gemini".into(),
            generic_key_fallback: false,
            service_openai_api_key: "sk".into(),
            service_gemini_api_key: "g".into(),
            ..Default::default()
        };
        let r: EmbeddingRoute = embedding_route(&c, "text-embedding-3-small");
        assert_eq!(
            r,
            EmbeddingRoute {
                provider: "openai".into(),
                model: "text-embedding-3-small".into()
            }
        );
    }

    /// `PlatformLlm::can_embed` is `embedding_model_fits` over this route.
    fn can_embed(cfg: &GatewayConfig, requested: &str) -> bool {
        let r: EmbeddingRoute = embedding_route(cfg, requested);
        embedding_model_fits(&r.model, &r.provider)
    }

    #[test]
    fn anthropic_default_falls_back_to_keyed_gemini() {
        let c: GatewayConfig = cfg("anthropic", "", "g");
        let r: EmbeddingRoute = embedding_route(&c, "text-embedding-3-small");
        assert_eq!(
            r,
            EmbeddingRoute {
                provider: "gemini".into(),
                model: "gemini-embedding-001".into()
            }
        );
        assert!(can_embed(&c, "text-embedding-3-small"));
    }

    #[test]
    fn no_embedding_provider_keyed_cannot_embed() {
        assert!(!can_embed(
            &cfg("anthropic", "", ""),
            "text-embedding-3-small"
        ));
        assert!(!can_embed(
            &cfg("openrouter", "", ""),
            "gemini-embedding-001"
        ));
    }

    #[test]
    fn embedding_providers_reject_chat_only_providers() {
        assert!(!embedding_model_fits("text-embedding-3-small", "anthropic"));
        assert!(!embedding_model_fits(
            "text-embedding-3-small",
            "openrouter"
        ));
        for p in EMBEDDING_PROVIDERS {
            assert!(
                default_embedding_model(p).is_some(),
                "{p} needs a default model"
            );
        }
    }
}
