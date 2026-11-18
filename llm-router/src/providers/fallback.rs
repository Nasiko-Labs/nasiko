//! Ordered fallback execution.
//!
//! Try the primary resolved config, then each `fallback_models` entry in order; the
//! first success wins, and only after all are exhausted — or the chain's shared
//! `provider_timeout_secs` deadline passes (a 504) — do we surface an error. Each
//! call returns the **effective** provider/model actually used, so usage logging
//! records the fallback when one fires.
//!
//! Key selection (decision §6.5): a fallback whose provider matches the primary reuses
//! the resolved key; a cross-provider fallback uses that provider's platform key (see
//! [`GatewayConfig::platform_key_for`]). A fallback with no usable key is skipped.
//!
//! **Chat only.** Embeddings used to run through here too, which is exactly how an agent
//! could end up with vectors from two different models in one corpus. They now take a
//! single attempt in [`crate::providers::execute_embeddings_once`].

use std::borrow::Cow;
use std::time::Duration;

use futures::stream::BoxStream;
use serde_json::Value;
use tokio::time::Instant;

use super::attempts::AttemptLog;
use super::{KeyOwner, ProviderClient, ProviderError, provider_for};
use crate::config::GatewayConfig;
use crate::error::GatewayError;
use crate::ir::{ChatChunk, ChatRequest, ChatResponse};
use crate::resolver::ResolvedConfig;

/// The provider/model actually used for a call.
type Effective = (String, String);

/// Cap on how many distinct parameters we'll adjust-and-retry against one model before
/// giving up on it. A backstop against a provider that keeps rejecting params; real
/// mismatches resolve in one or two fixes.
const MAX_PARAM_FIXES: usize = 4;

/// One parameter adjustment already made against the current model: the param, and the
/// value it was set to (`Value::Null` for a drop). The value is part of the record
/// because a repair is allowed to revisit a param but never to repeat a value.
type ParamFix = (String, Value);

/// Run a non-streaming chat with ordered fallbacks. Returns the response and the
/// effective `(provider, model)`. The whole chain shares one deadline
/// ([`within_provider_deadline`]), and each attempt gets a fair share of what is left of it
/// ([`attempt_budget`]).
pub async fn execute_chat(
    http: &reqwest::Client,
    cfg: &GatewayConfig,
    primary: &ResolvedConfig,
    req: &ChatRequest,
) -> Result<(ChatResponse, Effective), GatewayError> {
    let mut attempts = AttemptLog::default();
    execute_chat_by(
        http,
        cfg,
        primary,
        req,
        provider_deadline(cfg),
        &mut attempts,
    )
    .await
}

/// [`execute_chat`] against a deadline the caller already holds, so a request that runs
/// more than one chain (the compact-tools re-send) spends one budget, not one per chain.
/// Each attempt that failed, the one the deadline cut off included, is left in `attempts`
/// for the caller to bill.
pub async fn execute_chat_by(
    http: &reqwest::Client,
    cfg: &GatewayConfig,
    primary: &ResolvedConfig,
    req: &ChatRequest,
    deadline: Instant,
    attempts: &mut AttemptLog,
) -> Result<(ChatResponse, Effective), GatewayError> {
    within_provider_deadline(
        cfg,
        deadline,
        chat_attempts(http, cfg, primary, req, deadline, attempts),
    )
    .await
}

/// When a provider call starting now must have answered: `provider_timeout_secs` away.
pub fn provider_deadline(cfg: &GatewayConfig) -> Instant {
    Instant::now() + Duration::from_secs(cfg.provider_timeout_secs)
}

/// Run a streaming chat with ordered fallbacks (fallback applies to the *initial*
/// connect failure; once bytes flow, mid-stream errors end the stream). Returns the
/// chunk stream and the effective `(provider, model)`. The deadline covers establishing
/// the stream, never the chunks that follow. Failed attempts are left in `attempts`, as for
/// [`execute_chat_by`].
pub async fn execute_chat_stream(
    http: &reqwest::Client,
    cfg: &GatewayConfig,
    primary: &ResolvedConfig,
    req: &ChatRequest,
    attempts: &mut AttemptLog,
) -> Result<
    (
        BoxStream<'static, Result<ChatChunk, ProviderError>>,
        Effective,
    ),
    GatewayError,
> {
    let deadline = provider_deadline(cfg);
    within_provider_deadline(
        cfg,
        deadline,
        chat_stream_attempts(http, cfg, primary, req, deadline, attempts),
    )
    .await
}

/// Bound a whole attempt chain — every fallback and parameter-fix retry — by `deadline`.
/// Each attempt is already held to its share ([`attempt_budget`]); this is the backstop, so a
/// chain can never outlast the calling agent's own timeout and have the agent retry while the
/// router still waits.
async fn within_provider_deadline<T>(
    cfg: &GatewayConfig,
    deadline: Instant,
    attempts: impl Future<Output = Result<T, GatewayError>>,
) -> Result<T, GatewayError> {
    let timed_out = || {
        GatewayError::UpstreamTimeout(format!(
            "the fallback chain ran out of time: no provider answered within {}s across all \
             attempts (LLM_PROVIDER_TIMEOUT_SECS)",
            cfg.provider_timeout_secs
        ))
    };
    // A spent budget sends nothing: polled once, the chain would start an attempt only for
    // the deadline to drop it, and that attempt would be billed as a timed-out call.
    if Instant::now() >= deadline {
        return Err(timed_out());
    }
    tokio::time::timeout_at(deadline, attempts)
        .await
        .unwrap_or_else(|_| Err(timed_out()))
}

/// A fair share of what is left before `deadline` for the attempt about to run, with
/// `attempts_left` attempts (this one included) still to go. Recomputed per attempt, so a
/// model that fails fast leaves its unused time to the next, and a stalled one is cut off
/// before it can starve the fallbacks.
fn attempt_budget(deadline: Instant, attempts_left: usize) -> Duration {
    let left = u32::try_from(attempts_left.max(1)).unwrap_or(u32::MAX);
    deadline.saturating_duration_since(Instant::now()) / left
}

/// Run one provider call, giving up on it once `budget` has passed.
///
/// The last attempt's budget can equal the client's read timeout, and the two timers then
/// race: a read timeout surfaces as a bare transport error. A transport error that arrives
/// only once the budget is spent is that same stall, so it is reported as the timeout.
async fn within_attempt_budget<T>(
    model: &str,
    budget: Duration,
    call: impl Future<Output = Result<T, ProviderError>>,
) -> Result<T, ProviderError> {
    let timed_out = || {
        ProviderError::TimedOut(format!(
            "model {model} did not answer within {:.1}s",
            budget.as_secs_f64()
        ))
    };
    let started = Instant::now();
    match tokio::time::timeout(budget, call).await {
        Ok(Err(ProviderError::Transport(_))) if started.elapsed() >= budget => Err(timed_out()),
        Ok(result) => result,
        Err(_) => Err(timed_out()),
    }
}

async fn chat_attempts(
    http: &reqwest::Client,
    cfg: &GatewayConfig,
    primary: &ResolvedConfig,
    req: &ChatRequest,
    deadline: Instant,
    log: &mut AttemptLog,
) -> Result<(ChatResponse, Effective), GatewayError> {
    let attempts = build_attempts(primary, cfg);
    let mut failures = Failures::default();
    let total = attempts.len();
    for (i, attempt) in attempts.iter().enumerate() {
        log_request("chat", attempt, i, total);
        let provider = match provider_for(
            &attempt.provider,
            attempt.custom_endpoint.as_ref(),
            http,
            cfg,
        ) {
            Ok(p) => p,
            Err(e) => {
                failures.note_gateway(e);
                continue;
            }
        };
        // Working copies so a parameter-fix retry can mutate the request + config.
        let mut work_req = Cow::Borrowed(req);
        let mut work_cfg = attempt.clone();
        let mut applied: Vec<ParamFix> = Vec::new();
        loop {
            log.start(&attempt.provider, &attempt.model, attempt.platform_paid);
            let budget = attempt_budget(deadline, total - i);
            match within_attempt_budget(&attempt.model, budget, provider.chat(&work_req, &work_cfg))
                .await
            {
                Ok(resp) => {
                    log.settle();
                    log_response("chat", attempt, &resp);
                    return Ok((resp, (attempt.provider.clone(), attempt.model.clone())));
                }
                Err(e) => {
                    if names_a_param(&*provider, &e)
                        && try_fix_param(
                            &*provider,
                            &e,
                            work_req.to_mut(),
                            &mut work_cfg,
                            &mut applied,
                        )
                    {
                        log.settle();
                        log_param_fix(attempt, &applied.last().unwrap().0, &e);
                        continue;
                    }
                    log.fail(&e);
                    if !ends_the_chain(&e) {
                        warn_attempt(attempt, &e, i, total);
                    }
                    if let Some(stop) = failures.note(attempt, e) {
                        return Err(stop);
                    }
                    break;
                }
            }
        }
    }
    Err(failures.into_error())
}

/// The error a chain reports when no attempt answered: an input refused by name, else the
/// first attempt's, because the primary's is usually the one the caller can act on. One
/// exception: when that first error is a bare 502 and the last attempt ran out of time, the
/// timeout (504) describes the chain better.
#[derive(Default)]
struct Failures {
    first: Option<GatewayError>,
    rejected: Option<GatewayError>,
    last_timeout: Option<String>,
}

impl Failures {
    fn note_gateway(&mut self, err: GatewayError) {
        self.last_timeout = None;
        self.first.get_or_insert(err);
    }

    fn note(&mut self, attempt: &ResolvedConfig, err: ProviderError) -> Option<GatewayError> {
        let ends_chain = ends_the_chain(&err);
        let rejected = matches!(err, ProviderError::Rejected(_));
        let key_owner = KeyOwner::of(attempt.platform_paid, attempt.custom_endpoint.is_some());
        let err = super::rejected_key_error(&err, &attempt.provider, key_owner)
            .unwrap_or_else(|| GatewayError::from(err));
        self.last_timeout = match &err {
            GatewayError::UpstreamTimeout(message) => Some(message.clone()),
            _ => None,
        };
        if ends_chain {
            tracing::warn!(
                target: "nasiko::llm_router::fallback",
                provider = %attempt.provider, model = %attempt.model, status = %err.status(),
                "llm attempt failed on the request itself; not re-sending it to a fallback"
            );
            return Some(err);
        }
        if rejected {
            self.rejected.get_or_insert(err);
        } else {
            self.first.get_or_insert(err);
        }
        None
    }

    fn into_error(self) -> GatewayError {
        match (self.rejected.or(self.first), self.last_timeout) {
            (Some(GatewayError::Upstream(_)), Some(timeout)) => {
                GatewayError::UpstreamTimeout(timeout)
            }
            (Some(first), _) => first,
            (None, _) => GatewayError::Upstream("no provider attempts".to_string()),
        }
    }
}

/// Whether the request itself is bad, so no fallback could answer it either: 400, 413, 415
/// or 422, unless it blames the model (Bedrock's unknown model id) or is Gemini refusing the
/// API key, both of which a fallback can fix.
fn ends_the_chain(err: &ProviderError) -> bool {
    match err {
        ProviderError::Status {
            status: status @ (400 | 413 | 415 | 422),
            message,
            ..
        } => {
            !message.to_ascii_lowercase().contains("model")
                && !super::is_bad_key_400(*status, message)
        }
        _ => false,
    }
}

async fn chat_stream_attempts(
    http: &reqwest::Client,
    cfg: &GatewayConfig,
    primary: &ResolvedConfig,
    req: &ChatRequest,
    deadline: Instant,
    log: &mut AttemptLog,
) -> Result<
    (
        BoxStream<'static, Result<ChatChunk, ProviderError>>,
        Effective,
    ),
    GatewayError,
> {
    let attempts = build_attempts(primary, cfg);
    let mut failures = Failures::default();
    let total = attempts.len();
    for (i, attempt) in attempts.iter().enumerate() {
        log_request("chat_stream", attempt, i, total);
        let provider = match provider_for(
            &attempt.provider,
            attempt.custom_endpoint.as_ref(),
            http,
            cfg,
        ) {
            Ok(p) => p,
            Err(e) => {
                failures.note_gateway(e);
                continue;
            }
        };
        // The initial connect (status check) happens before any chunk flows, so a
        // parameter-rejection surfaces here and is safe to fix-and-retry — same as `chat`.
        let mut work_req = Cow::Borrowed(req);
        let mut work_cfg = attempt.clone();
        let mut applied: Vec<ParamFix> = Vec::new();
        loop {
            log.start(&attempt.provider, &attempt.model, attempt.platform_paid);
            let budget = attempt_budget(deadline, total - i);
            match within_attempt_budget(
                &attempt.model,
                budget,
                provider.chat_stream(&work_req, &work_cfg),
            )
            .await
            {
                Ok(stream) => {
                    log.settle();
                    tracing::info!(
                        target: "nasiko::llm_router::provider",
                        provider = %attempt.provider,
                        model = %attempt.model,
                        "provider response ← chat_stream established (body streamed as SSE chunks)"
                    );
                    return Ok((stream, (attempt.provider.clone(), attempt.model.clone())));
                }
                Err(e) => {
                    if names_a_param(&*provider, &e)
                        && try_fix_param(
                            &*provider,
                            &e,
                            work_req.to_mut(),
                            &mut work_cfg,
                            &mut applied,
                        )
                    {
                        log.settle();
                        log_param_fix(attempt, &applied.last().unwrap().0, &e);
                        continue;
                    }
                    log.fail(&e);
                    if !ends_the_chain(&e) {
                        warn_attempt(attempt, &e, i, total);
                    }
                    if let Some(stop) = failures.note(attempt, e) {
                        return Err(stop);
                    }
                    break;
                }
            }
        }
    }
    Err(failures.into_error())
}

fn names_a_param(provider: &dyn ProviderClient, err: &ProviderError) -> bool {
    provider.droppable_param(err).is_some() || provider.repairable_param(err).is_some()
}

/// Log the outbound request to an upstream LLM — provider + model only (never the
/// prompt/messages or the api key). Emitted per attempt so fallbacks are visible.
fn log_request(op: &str, attempt: &ResolvedConfig, i: usize, total: usize) {
    tracing::info!(
        target: "nasiko::llm_router::provider",
        op,
        provider = %attempt.provider,
        model = %attempt.model,
        attempt = i + 1,
        of = total,
        "provider request → dispatching {op} to upstream LLM (provider/model)"
    );
}

/// Log that the upstream LLM answered, and how large the answer was. Never the body itself: it is
/// the conversation (the model's reply, its tool calls and their arguments), and logs — exported
/// as span events when OTLP is on — are no place to keep one.
fn log_response<T: serde::Serialize>(op: &str, attempt: &ResolvedConfig, resp: &T) {
    tracing::debug!(
        target: "nasiko::llm_router::provider",
        op,
        provider = %attempt.provider,
        model = %attempt.model,
        response_bytes = serde_json::to_vec(resp).map_or(0, |body| body.len()),
        "provider response ← {op}"
    );
}

fn warn_attempt(attempt: &ResolvedConfig, err: &ProviderError, i: usize, total: usize) {
    let more = i + 1 < total;
    tracing::warn!(
        provider = %attempt.provider, model = %attempt.model, error = %err,
        "llm attempt failed{}", if more { "; trying fallback" } else { "; exhausted" }
    );
}

/// Try to recover from `err` by adjusting a rejected parameter and retrying the *same*
/// model. Two repairs, in order: drop a param the provider says it doesn't accept, or
/// set one it says it needs at a specific value. Either way the param is recorded in
/// `applied`, which caps the total and guards the loop. Returns `true` when the caller
/// should retry.
pub(crate) fn try_fix_param(
    provider: &dyn ProviderClient,
    err: &ProviderError,
    req: &mut ChatRequest,
    cfg: &mut ResolvedConfig,
    applied: &mut Vec<ParamFix>,
) -> bool {
    if applied.len() >= MAX_PARAM_FIXES {
        return false;
    }
    // Guard against a provider that re-reports the same param: if we already adjusted it
    // (or the adjustment is a no-op), retrying would just fail identically — give up.
    if let Some(param) = provider.droppable_param(err) {
        if applied.iter().any(|(p, _)| p == &param) || !strip_param(req, cfg, &param) {
            return false;
        }
        applied.push((param, Value::Null));
        return true;
    }
    let Some((param, value)) = provider.repairable_param(err) else {
        return false;
    };
    // A repair, unlike a drop, may legitimately revisit a param it already set: a model
    // that rejects the first remedy names a second one. What it must never do is cycle —
    // a model whose two remedies contradict each other (refusing function tools at its
    // default effort *and* refusing the `'none'` that remedy demands) would otherwise
    // ping-pong until the cap. Re-sending a value this attempt already tried is that
    // cycle, so stop at the point it is first provable rather than four calls later.
    if applied.iter().any(|(p, v)| p == &param && v == &value) {
        return false;
    }
    if !apply_param(req, cfg, &param, value.clone()) {
        return false;
    }
    applied.push((param, value));
    true
}

/// Remove `param` from both the request and the resolved config so the retry omits it.
/// Named params (`temperature`, `max_tokens`) live on both structs; anything else is a
/// passthrough field in the request's `extra` map. `max_completion_tokens` is the wire
/// alias OpenAI uses for `max_tokens`. Returns whether anything was actually removed.
fn strip_param(req: &mut ChatRequest, cfg: &mut ResolvedConfig, param: &str) -> bool {
    match param {
        "temperature" => {
            let had = req.temperature.is_some() || cfg.temperature.is_some();
            req.temperature = None;
            cfg.temperature = None;
            had
        }
        "max_tokens" | "max_completion_tokens" => {
            let had = req.max_tokens.is_some() || cfg.max_tokens.is_some();
            req.max_tokens = None;
            cfg.max_tokens = None;
            had
        }
        // The provider adds a cache key from `cfg.agent_id` (`TOKEN_CACHE_MARKERS`), so the
        // key is not in `extra` to remove; clearing the id stops the retry adding it again.
        "prompt_cache_key" | "session_id" => {
            let had = req.extra.remove(param).is_some() || !cfg.agent_id.is_empty();
            cfg.agent_id.clear();
            had
        }
        other => req.extra.remove(other).is_some(),
    }
}

/// Set `param` so the retry carries it — the mirror of [`strip_param`], and it has to be
/// a mirror: the named params live on both structs, and writing one into the request's
/// flattened `extra` map instead would serialize it *alongside* the field derived from
/// the struct (a `max_tokens` repair would emit the clamped value and the original
/// `max_completion_tokens`). The resolved config wins on the wire, so both are set.
/// Returns whether the value actually changed — an unchanged one means the retry would
/// fail identically.
fn apply_param(req: &mut ChatRequest, cfg: &mut ResolvedConfig, param: &str, value: Value) -> bool {
    match param {
        "temperature" => {
            let Some(t) = value.as_f64() else {
                return false;
            };
            let changed = cfg.temperature != Some(t) || req.temperature != Some(t);
            req.temperature = Some(t);
            cfg.temperature = Some(t);
            changed
        }
        "max_tokens" | "max_completion_tokens" => {
            let Some(n) = value.as_i64() else {
                return false;
            };
            let changed = cfg.max_tokens != Some(n) || req.max_tokens != Some(n);
            req.max_tokens = Some(n);
            cfg.max_tokens = Some(n);
            changed
        }
        other => {
            if req.extra.get(other) == Some(&value) {
                return false;
            }
            req.extra.insert(other.to_string(), value);
            true
        }
    }
}

fn log_param_fix(attempt: &ResolvedConfig, param: &str, err: &ProviderError) {
    tracing::warn!(
        target: "nasiko::llm_router::provider",
        provider = %attempt.provider, model = %attempt.model, param, error = %err,
        "upstream rejected parameter; adjusting it and retrying the same model"
    );
}

/// Build the ordered attempt list: the primary, then each usable fallback.
pub(crate) fn build_attempts(primary: &ResolvedConfig, cfg: &GatewayConfig) -> Vec<ResolvedConfig> {
    let mut attempts = vec![ResolvedConfig {
        fallback_models: Vec::new(),
        ..primary.clone()
    }];

    for entry in &primary.fallback_models {
        let (provider, model) = split_prefixed(entry, &primary.provider);
        // Same provider ⇒ reuse the resolved key (may be a per-user secret). A
        // cross-provider fallback can't use that key, so fall back to the platform
        // key for that provider.
        let api_key = if provider == primary.provider {
            primary.api_key.clone()
        } else {
            cfg.platform_key_for(&provider).to_string()
        };
        if api_key.is_empty() {
            tracing::warn!(%entry, "skipping fallback: no api key available");
            continue;
        }
        // Same-provider fallbacks reuse the primary's key (and payer); cross-provider
        // fallbacks always use the platform key.
        let platform_paid = if provider == primary.provider {
            primary.platform_paid
        } else {
            true
        };
        // A same-provider fallback inherits the primary's endpoint, so a custom
        // provider's own fallback still targets it in the same dialect; a
        // cross-provider fallback uses the built-in base URL (and a custom
        // cross-provider name has no platform key, so it is already skipped by the
        // `is_empty` guard above).
        let custom_endpoint = if provider == primary.provider {
            primary.custom_endpoint.clone()
        } else {
            None
        };
        attempts.push(ResolvedConfig {
            provider,
            model,
            litellm_model: entry.clone(),
            api_key,
            fallback_models: Vec::new(),
            temperature: primary.temperature,
            max_tokens: primary.max_tokens,
            has_llm_config: primary.has_llm_config,
            // Fallback attempts are never pinned — pinning disables fallbacks upstream.
            pinned_model: None,
            // Tier overrides don't apply to fallback attempts.
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid,
            custom_endpoint,
            is_coding_agent: primary.is_coding_agent,
            is_discovered_agent: primary.is_discovered_agent,
            compress_enabled: primary.compress_enabled,
            agent_id: primary.agent_id.clone(),
        });
    }
    attempts
}

/// Split a `"provider/model"` id; an unprefixed entry inherits the primary provider.
fn split_prefixed(entry: &str, default_provider: &str) -> (String, String) {
    match entry.split_once('/') {
        Some((p, m)) => (p.to_string(), m.to_string()),
        None => (default_provider.to_string(), entry.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(platform_key: &str) -> GatewayConfig {
        GatewayConfig {
            platform_openai_api_key: platform_key.into(),
            ..Default::default()
        }
    }

    fn primary(provider: &str, fallbacks: Vec<&str>) -> ResolvedConfig {
        ResolvedConfig {
            agent_id: String::new(),
            provider: provider.into(),
            model: "primary-model".into(),
            litellm_model: format!("{provider}/primary-model"),
            api_key: "primary-key".into(),
            fallback_models: fallbacks.into_iter().map(String::from).collect(),
            temperature: Some(0.3),
            max_tokens: Some(100),
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
        }
    }

    /// A minimal tool-carrying request, the shape Claude Code sends every turn.
    fn chat_req() -> ChatRequest {
        serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [{
                "type": "function",
                "function": { "name": "Read", "parameters": { "type": "object" } }
            }]
        }))
        .expect("valid chat request")
    }

    /// The gpt-5.x rejection of function tools; `OpenAiProvider` classifies it purely
    /// from the error body, so no network is involved.
    fn reasoning_effort_rejection() -> ProviderError {
        ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Function tools with reasoning_effort are not supported for gpt-5.6 in /v1/chat/completions. To use function tools, use /v1/responses or set reasoning_effort to 'none'.",
                    "type": "invalid_request_error",
                    "param": "reasoning_effort",
                    "code": serde_json::Value::Null
                }
            })
            .to_string(),
            retryable: false,
        }
    }

    #[test]
    fn a_repairable_rejection_sets_the_param_and_retries_once() {
        let provider =
            crate::providers::OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        let err = reasoning_effort_rejection();
        let mut req = chat_req();
        let mut cfg = primary("openai", vec![]);
        let mut applied: Vec<ParamFix> = Vec::new();

        assert!(try_fix_param(
            &provider,
            &err,
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert_eq!(req.extra.get("reasoning_effort"), Some(&json!("none")));
        assert_eq!(
            applied,
            vec![("reasoning_effort".to_string(), json!("none"))]
        );
        // The tools the caller sent are untouched — the repair is additive.
        assert!(req.tools.is_some());

        // Re-reported by the provider ⇒ the retry would fail identically. Give up
        // rather than loop.
        assert!(!try_fix_param(
            &provider,
            &err,
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert_eq!(applied.len(), 1);
    }

    #[test]
    fn a_droppable_rejection_still_strips_the_param() {
        let provider =
            crate::providers::OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        let err = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Unsupported value: 'temperature' does not support 0.3 with this model.",
                    "param": "temperature",
                    "code": "unsupported_value"
                }
            })
            .to_string(),
            retryable: false,
        };
        let mut req = chat_req();
        let mut cfg = primary("openai", vec![]); // temperature: Some(0.3)
        let mut applied: Vec<ParamFix> = Vec::new();

        assert!(try_fix_param(
            &provider,
            &err,
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert_eq!(cfg.temperature, None);
        assert_eq!(
            applied,
            vec![("temperature".to_string(), serde_json::Value::Null)]
        );
        assert!(!req.extra.contains_key("reasoning_effort"));
    }

    #[test]
    fn an_unrecognized_error_is_never_fixed() {
        let provider =
            crate::providers::OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        let mut req = chat_req();
        let mut cfg = primary("openai", vec![]);
        let mut applied: Vec<ParamFix> = Vec::new();

        assert!(!try_fix_param(
            &provider,
            &ProviderError::Status {
                status: 401,
                message: json!({ "error": { "code": "invalid_api_key" } }).to_string(),
                retryable: false,
            },
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert!(applied.is_empty());
        assert!(req.extra.is_empty());
        assert_eq!(cfg.temperature, Some(0.3));
    }

    #[test]
    fn param_fixes_are_capped() {
        let provider =
            crate::providers::OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        let mut req = chat_req();
        let mut cfg = primary("openai", vec![]);
        let mut applied: Vec<ParamFix> = (0..MAX_PARAM_FIXES)
            .map(|i| (format!("param-{i}"), serde_json::Value::Null))
            .collect();

        assert!(!try_fix_param(
            &provider,
            &reasoning_effort_rejection(),
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert!(!req.extra.contains_key("reasoning_effort"));
    }

    #[test]
    fn a_rejected_router_cache_key_is_dropped_and_not_re_added() {
        // An OpenAI-compatible base that does not know `prompt_cache_key`. The key was added by
        // the provider, not the caller, so the repair has to stop the provider adding it.
        let provider =
            crate::providers::OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        let err = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Unrecognized request argument supplied: prompt_cache_key",
                    "type": "invalid_request_error",
                    "param": "prompt_cache_key",
                    "code": "unsupported_parameter"
                }
            })
            .to_string(),
            retryable: false,
        };
        let mut req = chat_req();
        let mut cfg = primary("openai", vec![]);
        cfg.agent_id = "agent-1".into();
        let mut applied: Vec<ParamFix> = Vec::new();

        assert!(try_fix_param(
            &provider,
            &err,
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert!(cfg.agent_id.is_empty());
        assert!(!try_fix_param(
            &provider,
            &err,
            &mut req,
            &mut cfg,
            &mut applied
        ));
    }

    #[test]
    fn same_provider_fallback_reuses_key_cross_provider_uses_platform() {
        let mut p = primary(
            "anthropic",
            vec!["anthropic/claude-haiku", "openai/gpt-4o-mini"],
        );
        p.agent_id = "agent-1".into();
        let attempts = build_attempts(&p, &cfg("sk-platform"));
        assert_eq!(attempts.len(), 3);
        // primary
        assert_eq!(attempts[0].model, "primary-model");
        assert!(attempts[0].fallback_models.is_empty());
        // same-provider fallback → reuse primary key
        assert_eq!(attempts[1].provider, "anthropic");
        assert_eq!(attempts[1].api_key, "primary-key");
        // cross-provider fallback → platform key + carried params
        assert_eq!(attempts[2].provider, "openai");
        assert_eq!(attempts[2].model, "gpt-4o-mini");
        assert_eq!(attempts[2].api_key, "sk-platform");
        assert_eq!(attempts[2].temperature, Some(0.3));
        assert_eq!(attempts[2].max_tokens, Some(100));
        // The same agent, so OpenAI keys its prompt cache as on a direct call.
        assert_eq!(attempts[2].agent_id, "agent-1");
    }

    #[test]
    fn cross_provider_fallback_skipped_without_platform_key() {
        let p = primary("anthropic", vec!["openai/gpt-4o-mini"]);
        let attempts = build_attempts(&p, &cfg("")); // no platform key
        assert_eq!(attempts.len(), 1); // fallback skipped
        assert_eq!(attempts[0].provider, "anthropic");
    }

    #[test]
    fn cross_provider_fallback_to_custom_name_is_skipped() {
        // A fallback naming a custom (non-built-in) provider gets no platform key
        // (platform_key_for returns "" for non-built-ins), so it is skipped rather
        // than mis-keyed with the OpenAI key.
        let p = primary("openai", vec!["my-gateway/llama-3.1-70b"]);
        let attempts = build_attempts(&p, &cfg("sk-platform"));
        assert_eq!(attempts.len(), 1); // custom-named fallback skipped
        assert_eq!(attempts[0].provider, "openai");
    }

    #[test]
    fn same_provider_custom_fallback_inherits_endpoint() {
        // A same-provider fallback for a custom provider reuses the primary key and
        // carries the primary's endpoint — URL *and* dialect — so the attempt still
        // targets it the same way.
        let mut p = primary("azure-prod", vec!["azure-prod/prod-gpt4o-mini"]);
        let dialect = crate::providers::ProviderDialect::AzureOpenAi {
            api_version: "2024-10-21".into(),
        };
        p.custom_endpoint = Some(crate::resolver::CustomEndpoint {
            base_url: "https://acme.openai.azure.com".into(),
            dialect: dialect.clone(),
        });
        let attempts = build_attempts(&p, &cfg("sk-platform"));
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[1].provider, "azure-prod");
        assert_eq!(attempts[1].api_key, "primary-key"); // same-provider ⇒ reuse key
        let endpoint = attempts[1]
            .custom_endpoint
            .as_ref()
            .expect("same-provider fallback keeps the endpoint");
        assert_eq!(endpoint.base_url, "https://acme.openai.azure.com");
        assert_eq!(endpoint.dialect, dialect);
    }

    #[tokio::test]
    async fn primary_failure_falls_back_to_openai_and_reports_effective() {
        // Primary Anthropic returns 401 (bad key); fallback OpenAI succeeds.
        let mut anthropic = mockito::Server::new_async().await;
        anthropic
            .mock("POST", "/messages")
            .with_status(401)
            .with_body("invalid x-api-key")
            .create_async()
            .await;
        let mut openai = mockito::Server::new_async().await;
        openai
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-4o-mini",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let cfg = GatewayConfig {
            anthropic_api_base: anthropic.url(),
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        let primary = ResolvedConfig {
            agent_id: String::new(),
            provider: "anthropic".into(),
            model: "claude-3-5-sonnet-20241022".into(),
            litellm_model: "anthropic/claude-3-5-sonnet-20241022".into(),
            api_key: "sk-bad".into(),
            fallback_models: vec!["openai/gpt-4o-mini".into()],
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
        };
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();

        let (resp, (provider, model)) = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req)
            .await
            .unwrap();
        assert_eq!(provider, "openai"); // effective = the fallback
        assert_eq!(model, "gpt-4o-mini");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn param_rejection_is_dropped_and_retried_on_same_model() {
        // gpt-5.5 rejects a non-default temperature with a 400 naming the param. The
        // executor must drop `temperature` and retry the SAME model (no fallback needed).
        let mut openai = mockito::Server::new_async().await;
        // First call carries temperature → 400 param rejection.
        let rejected = openai
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({ "temperature": 0.1 })))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "error": {
                        "message": "Unsupported value: 'temperature' does not support 0.1 with this model. Only the default (1) value is supported.",
                        "type": "invalid_request_error",
                        "param": "temperature",
                        "code": "unsupported_value"
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        // Retry without temperature → 200. (mockito matches the first mock whose body
        // matcher matches; the retry omits temperature so it falls through to this one.)
        let ok = openai
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-5.5",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        // Temperature comes from the resolved config (as tier routing sets it); no fallbacks.
        let primary = ResolvedConfig {
            agent_id: String::new(),
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            litellm_model: "openai/gpt-5.5".into(),
            api_key: "sk-x".into(),
            fallback_models: vec![],
            temperature: Some(0.1),
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
        };
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();

        let (resp, (provider, model)) = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req)
            .await
            .unwrap();
        rejected.assert_async().await;
        ok.assert_async().await;
        assert_eq!(provider, "openai"); // recovered on the same model, no fallback
        assert_eq!(model, "gpt-5.5");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn a_model_that_rejects_both_its_default_effort_and_none_recovers_on_the_third_call() {
        // The two-step exchange gpt-6-astra drives: it refuses function tools at its
        // default effort, then refuses the `'none'` that remedy asks for, naming the
        // values it does accept. Neither step is droppable — stripping the param just
        // reinstates the default — so the repair seam has to revisit the same param.
        let mut openai = mockito::Server::new_async().await;
        // Created first ⇒ matched first. The repair's own value, rejected in turn.
        let none_rejected = openai
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "reasoning_effort": "none" }),
            ))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "error": {
                        "message": "Unsupported value: 'reasoning_effort' does not support 'none' with this model. Supported values are: 'low', 'medium', 'high', and 'xhigh'.",
                        "type": "invalid_request_error",
                        "param": "reasoning_effort",
                        "code": "unsupported_value"
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let low_accepted = openai
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "reasoning_effort": "low" }),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-6-astra",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        // Carries no `reasoning_effort` at all ⇒ the original tool-carrying request.
        let tools_rejected = openai
            .mock("POST", "/chat/completions")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "error": {
                        "message": "Function tools with reasoning_effort are not supported for gpt-6-astra in /v1/chat/completions. To use function tools, use /v1/responses or set reasoning_effort to 'none'.",
                        "type": "invalid_request_error",
                        "param": "reasoning_effort",
                        "code": serde_json::Value::Null
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            ..Default::default()
        };
        let mut p = primary("openai", vec![]);
        p.model = "gpt-6-astra".into();
        p.temperature = None; // keep the exchange to the one param under test
        p.max_tokens = None;

        let (resp, (provider, model)) =
            execute_chat(&reqwest::Client::new(), &cfg, &p, &chat_req())
                .await
                .unwrap();
        tools_rejected.assert_async().await;
        none_rejected.assert_async().await;
        low_accepted.assert_async().await;
        assert_eq!(provider, "openai"); // recovered on the same model, no fallback
        assert_eq!(model, "gpt-6-astra");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("ok"));
    }

    #[test]
    fn two_contradictory_remedies_stop_at_the_cycle_rather_than_ping_ponging() {
        // gpt-6-astra refuses function tools at its default effort AND refuses the
        // 'none' that remedy demands, so the two repairs point at each other. Without a
        // value-level guard this burns every one of MAX_PARAM_FIXES before failing.
        let provider =
            crate::providers::OpenAiProvider::new(reqwest::Client::new(), "http://x".into());
        let none_unsupported = ProviderError::Status {
            status: 400,
            message: json!({
                "error": {
                    "message": "Unsupported value: 'reasoning_effort' does not support 'none' with this model. Supported values are: 'low', 'medium', 'high', and 'xhigh'.",
                    "param": "reasoning_effort",
                    "code": "unsupported_value"
                }
            })
            .to_string(),
            retryable: false,
        };
        let mut req = chat_req();
        let mut cfg = primary("openai", vec![]);
        let mut applied: Vec<ParamFix> = Vec::new();

        // none → rejected → low.
        assert!(try_fix_param(
            &provider,
            &reasoning_effort_rejection(),
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert_eq!(req.extra.get("reasoning_effort"), Some(&json!("none")));
        assert!(try_fix_param(
            &provider,
            &none_unsupported,
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert_eq!(req.extra.get("reasoning_effort"), Some(&json!("low")));

        // 'low' draws the tools rejection again, whose remedy is the 'none' already
        // tried — the cycle is now provable, so stop instead of swinging back.
        assert!(!try_fix_param(
            &provider,
            &reasoning_effort_rejection(),
            &mut req,
            &mut cfg,
            &mut applied
        ));
        assert_eq!(
            applied.len(),
            2,
            "two fixes, not the full budget of {MAX_PARAM_FIXES}"
        );
        assert_eq!(req.extra.get("reasoning_effort"), Some(&json!("low")));
    }

    #[tokio::test]
    async fn an_output_cap_above_the_model_ceiling_is_clamped_not_dropped() {
        // Claude Code asks for 64000 output tokens on every turn; gpt-4o-mini caps at
        // 16384. The rejection's code is `invalid_value`, which the droppable seam does
        // not read — the repair must clamp to the ceiling the message states, and must
        // write it to the named field rather than the flattened `extra` map, or the retry
        // would carry the clamp *and* the original `max_completion_tokens`.
        let mut openai = mockito::Server::new_async().await;
        let rejected = openai
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "max_completion_tokens": 64000 }),
            ))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "error": {
                        "message": "max_tokens is too large: 64000. This model supports at most 16384 completion tokens, whereas you provided 64000.",
                        "type": "invalid_request_error",
                        "param": "max_tokens",
                        "code": "invalid_value"
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let ok = openai
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "max_completion_tokens": 16384 }),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-4o-mini",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            ..Default::default()
        };
        let mut p = primary("openai", vec![]);
        p.model = "gpt-4o-mini".into();
        p.temperature = None;
        p.max_tokens = None; // the cap comes from the client, as Claude Code sends it
        let req: ChatRequest = serde_json::from_value(json!({
            "max_tokens": 64000,
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();

        let (resp, (_, model)) = execute_chat(&reqwest::Client::new(), &cfg, &p, &req)
            .await
            .unwrap();
        rejected.assert_async().await;
        ok.assert_async().await;
        assert_eq!(model, "gpt-4o-mini");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("ok"));
    }

    fn bedrock_primary(base: String) -> ResolvedConfig {
        let mut p = primary("my-bedrock", vec![]);
        p.model = "global.anthropic.claude-haiku-5-5".into();
        p.temperature = Some(0.3);
        p.custom_endpoint = Some(crate::resolver::CustomEndpoint {
            base_url: base,
            dialect: crate::providers::dialect::ProviderDialect::BedrockConverse,
        });
        p
    }

    async fn bedrock_refuses_temperature(
        server: &mut mockito::ServerGuard,
        path: &str,
        wording: &str,
    ) -> (mockito::Mock, mockito::Mock) {
        let rejected = server
            .mock("POST", path)
            .match_body(mockito::Matcher::PartialJson(
                json!({ "inferenceConfig": { "temperature": 0.3 } }),
            ))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(json!({ "message": wording }).to_string())
            .expect(1)
            .create_async()
            .await;
        let ok = server
            .mock("POST", path)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "output": { "message": { "role": "assistant", "content": [{ "text": "ok" }] } },
                    "stopReason": "end_turn",
                    "usage": { "inputTokens": 5, "outputTokens": 1, "totalTokens": 6 }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        (rejected, ok)
    }

    #[tokio::test]
    async fn bedrock_retries_without_a_temperature_its_claude_5_models_call_deprecated() {
        for wording in [
            "The model returned the following errors: `temperature` is deprecated for this model.",
            "This model doesn't support the temperature field.",
        ] {
            let mut bedrock = mockito::Server::new_async().await;
            let path = "/model/global.anthropic.claude-haiku-5-5/converse";
            let (rejected, ok) = bedrock_refuses_temperature(&mut bedrock, path, wording).await;
            let primary = bedrock_primary(bedrock.url());

            let (resp, _) = execute_chat(&reqwest::Client::new(), &cfg(""), &primary, &hi())
                .await
                .unwrap_or_else(|e| panic!("{wording}: {e}"));

            rejected.assert_async().await;
            ok.assert_async().await;
            assert_eq!(resp.choices[0].message.text().as_deref(), Some("ok"));
        }
    }

    #[tokio::test]
    async fn bedrock_streaming_retries_without_a_deprecated_temperature() {
        let mut bedrock = mockito::Server::new_async().await;
        let path = "/model/global.anthropic.claude-haiku-5-5/converse-stream";
        let (rejected, ok) = bedrock_refuses_temperature(
            &mut bedrock,
            path,
            "The model returned the following errors: `temperature` is deprecated for this model.",
        )
        .await;
        let primary = bedrock_primary(bedrock.url());

        let established = execute_chat_stream(
            &reqwest::Client::new(),
            &cfg(""),
            &primary,
            &hi(),
            &mut AttemptLog::default(),
        )
        .await;

        assert!(
            established.is_ok(),
            "stream was not retried without temperature"
        );
        rejected.assert_async().await;
        ok.assert_async().await;
    }

    #[tokio::test]
    async fn anthropic_deprecated_param_is_dropped_and_retried_on_same_model() {
        // claude-opus-4-8 rejects `temperature` with a 400 naming it in the message text
        // (Anthropic's shape). Temperature comes from the request, the agent is pinned
        // (no fallbacks) — the executor must still drop it and retry the SAME model.
        let mut anthropic = mockito::Server::new_async().await;
        let rejected = anthropic
            .mock("POST", "/messages")
            .match_body(mockito::Matcher::PartialJson(json!({ "temperature": 0.7 })))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "type": "error",
                    "error": {
                        "type": "invalid_request_error",
                        "message": "`temperature` is deprecated for this model."
                    }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let ok = anthropic
            .mock("POST", "/messages")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "msg_01", "type": "message", "role": "assistant",
                    "content": [{ "type": "text", "text": "ok" }],
                    "stop_reason": "end_turn",
                    "usage": { "input_tokens": 5, "output_tokens": 1 }
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;

        let cfg = GatewayConfig {
            anthropic_api_base: anthropic.url(),
            ..Default::default()
        };
        let primary = ResolvedConfig {
            agent_id: String::new(),
            provider: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            litellm_model: "anthropic/claude-opus-4-8".into(),
            api_key: "sk-ant".into(),
            fallback_models: vec![], // pinned → no fallbacks
            temperature: None,       // config sets none; the request carries it
            max_tokens: None,
            has_llm_config: true,
            pinned_model: Some("claude-opus-4-8".into()),
            tier1_model: None,
            tier2_model: None,
            tier3_model: None,
            platform_paid: true,
            custom_endpoint: None,
            is_coding_agent: false,
            is_discovered_agent: false,
            compress_enabled: false,
        };
        let req: ChatRequest = serde_json::from_value(json!({
            "temperature": 0.7,
            "messages": [{ "role": "user", "content": "hi" }]
        }))
        .unwrap();

        let (resp, (provider, model)) = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req)
            .await
            .unwrap();
        rejected.assert_async().await;
        ok.assert_async().await;
        assert_eq!(provider, "anthropic"); // recovered on the same pinned model
        assert_eq!(model, "claude-opus-4-8");
        assert_eq!(resp.choices[0].message.text().as_deref(), Some("ok"));
    }

    #[tokio::test]
    async fn all_attempts_exhausted_is_502() {
        let mut openai = mockito::Server::new_async().await;
        openai
            .mock("POST", "/chat/completions")
            .with_status(500)
            .with_body("boom")
            .expect_at_least(1)
            .create_async()
            .await;
        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        // primary openai + one openai fallback, both hit the failing mock
        let primary = ResolvedConfig {
            agent_id: String::new(),
            provider: "openai".into(),
            model: "gpt-4o".into(),
            litellm_model: "openai/gpt-4o".into(),
            api_key: "sk-x".into(),
            fallback_models: vec!["openai/gpt-4o-mini".into()],
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
        };
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let err = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req)
            .await
            .unwrap_err();
        assert!(matches!(err, GatewayError::Upstream(_)));
        assert_eq!(err.status(), axum::http::StatusCode::BAD_GATEWAY);
    }

    fn openai_chain(base: String, fallbacks: Vec<&str>) -> (GatewayConfig, ResolvedConfig) {
        let cfg = GatewayConfig {
            openai_api_base: base,
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        let mut primary = primary("openai", fallbacks);
        primary.model = "gpt-4o".into();
        primary.litellm_model = "openai/gpt-4o".into();
        primary.temperature = None;
        primary.max_tokens = None;
        (cfg, primary)
    }

    fn ok_body() -> String {
        json!({
            "id": "chatcmpl-1", "object": "chat.completion", "model": "gpt-4o-mini",
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })
        .to_string()
    }

    async fn model_mock(
        server: &mut mockito::ServerGuard,
        model: &str,
        status: usize,
        body: String,
        hits: usize,
    ) -> mockito::Mock {
        server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({ "model": model })))
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_body(body)
            .expect(hits)
            .create_async()
            .await
    }

    fn hi() -> ChatRequest {
        serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
            .unwrap()
    }

    #[tokio::test]
    async fn a_fallback_that_takes_forced_tool_choice_answers_after_claude_refuses_it() {
        let mut anthropic = mockito::Server::new_async().await;
        anthropic
            .mock("POST", "/messages")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                json!({"type": "error", "error": {"type": "invalid_request_error",
                    "message": "tool_choice: type \"tool\" and \"any\" are not supported for this model"}})
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let mut openai = mockito::Server::new_async().await;
        let answered = model_mock(&mut openai, "gpt-4o-mini", 200, ok_body(), 1).await;
        let cfg = GatewayConfig {
            anthropic_api_base: anthropic.url(),
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        let mut primary = primary("anthropic", vec!["openai/gpt-4o-mini"]);
        primary.model = "claude-sonnet-5-5".into();
        let mut req = chat_req();
        req.tool_choice = Some(json!("required"));

        let (_, (provider, _)) = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req)
            .await
            .unwrap();

        assert_eq!(provider, "openai");
        answered.assert_async().await;
    }

    #[tokio::test]
    async fn a_payload_error_ends_the_chain_and_is_what_the_caller_sees() {
        let mut openai = mockito::Server::new_async().await;
        let corrupt = json!({"error": {"message": "The file could not be processed."}}).to_string();
        let first = model_mock(&mut openai, "gpt-4o", 400, corrupt, 1).await;
        let fallback = model_mock(&mut openai, "gpt-4o-mini", 200, ok_body(), 0).await;
        let (cfg, primary) = openai_chain(openai.url(), vec!["openai/gpt-4o-mini"]);

        let err = execute_chat(&reqwest::Client::new(), &cfg, &primary, &hi())
            .await
            .unwrap_err();

        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(err.to_string().contains("could not be processed"), "{err}");
        first.assert_async().await;
        fallback.assert_async().await;
    }

    #[derive(Clone, Default)]
    struct LoggedMessages(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl tracing::field::Visit for LoggedMessages {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0.lock().unwrap().push(format!("{value:?}"));
            }
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LoggedMessages {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            event.record(&mut self.clone());
        }
    }

    impl LoggedMessages {
        /// Collect this thread's log messages until the guard drops. An idle second dispatcher
        /// is held with it: while only one scoped dispatcher exists, tracing takes a callsite's
        /// interest from the default of whichever thread reaches it first, so a parallel test
        /// with no subscriber would silence that callsite here.
        fn capture() -> (Self, (tracing::Dispatch, tracing::subscriber::DefaultGuard)) {
            use tracing_subscriber::prelude::*;
            let logged = Self::default();
            let idle = tracing::Dispatch::new(tracing_subscriber::registry());
            let guard = tracing::subscriber::set_default(
                tracing_subscriber::registry().with(logged.clone()),
            );
            (logged, (idle, guard))
        }

        fn any(&self, needle: &str) -> bool {
            self.0.lock().unwrap().iter().any(|m| m.contains(needle))
        }
    }

    #[tokio::test]
    async fn a_chain_that_stops_never_logs_that_it_is_trying_a_fallback() {
        let (logged, _capturing) = LoggedMessages::capture();
        let mut openai = mockito::Server::new_async().await;
        let corrupt = json!({"error": {"message": "The file could not be processed."}}).to_string();
        model_mock(&mut openai, "gpt-4o", 400, corrupt, 2).await;
        model_mock(&mut openai, "gpt-4o-mini", 200, ok_body(), 0).await;
        let (cfg, primary) = openai_chain(openai.url(), vec!["openai/gpt-4o-mini"]);
        let http = reqwest::Client::new();

        execute_chat(&http, &cfg, &primary, &hi())
            .await
            .unwrap_err();
        assert!(
            execute_chat_stream(&http, &cfg, &primary, &hi(), &mut AttemptLog::default())
                .await
                .is_err()
        );

        assert!(logged.any("not re-sending it to a fallback"));
        assert!(
            !logged.any("trying fallback"),
            "{:?}",
            logged.0.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn a_chain_that_moves_on_still_logs_that_it_is_trying_a_fallback() {
        let (logged, _capturing) = LoggedMessages::capture();
        let mut openai = mockito::Server::new_async().await;
        let missing =
            json!({"error": {"message": "The model `gpt-4o` does not exist."}}).to_string();
        model_mock(&mut openai, "gpt-4o", 404, missing, 1).await;
        model_mock(&mut openai, "gpt-4o-mini", 200, ok_body(), 1).await;
        let (cfg, primary) = openai_chain(openai.url(), vec!["openai/gpt-4o-mini"]);

        execute_chat(&reqwest::Client::new(), &cfg, &primary, &hi())
            .await
            .unwrap();

        assert!(logged.any("llm attempt failed; trying fallback"));
        assert!(!logged.any("not re-sending"));
    }

    #[tokio::test]
    async fn a_payload_error_ends_a_streaming_chain_too() {
        let mut openai = mockito::Server::new_async().await;
        let corrupt = json!({"error": {"message": "Invalid MIME type."}}).to_string();
        let first = model_mock(&mut openai, "gpt-4o", 400, corrupt, 1).await;
        let fallback = model_mock(&mut openai, "gpt-4o-mini", 200, ok_body(), 0).await;
        let (cfg, primary) = openai_chain(openai.url(), vec!["openai/gpt-4o-mini"]);

        let err = match execute_chat_stream(
            &reqwest::Client::new(),
            &cfg,
            &primary,
            &hi(),
            &mut AttemptLog::default(),
        )
        .await
        {
            Ok(_) => panic!("a payload error must not be answered by a fallback"),
            Err(err) => err,
        };

        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
        first.assert_async().await;
        fallback.assert_async().await;
    }

    #[tokio::test]
    async fn a_400_that_blames_the_model_still_lets_the_fallback_answer() {
        let mut openai = mockito::Server::new_async().await;
        let refusal =
            json!({"error": {"message": "The model `gpt-4o` does not exist or you do not have access to it."}})
                .to_string();
        model_mock(&mut openai, "gpt-4o", 400, refusal, 1).await;
        model_mock(&mut openai, "gpt-4o-mini", 200, ok_body(), 1).await;
        let (cfg, primary) = openai_chain(openai.url(), vec!["openai/gpt-4o-mini"]);

        let (_, (_, model)) = execute_chat(&reqwest::Client::new(), &cfg, &primary, &hi())
            .await
            .unwrap();

        assert_eq!(model, "gpt-4o-mini");
    }

    #[tokio::test]
    async fn a_rejected_key_says_whose_key_it_was_and_stays_a_502() {
        let mut openai = mockito::Server::new_async().await;
        let refused =
            json!({"error": {"message": "Incorrect API key provided: sk-..."}}).to_string();
        model_mock(&mut openai, "gpt-4o", 401, refused.clone(), 2).await;

        let (cfg, platform) = openai_chain(openai.url(), vec![]);
        let err = execute_chat(&reqwest::Client::new(), &cfg, &platform, &hi())
            .await
            .unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::BAD_GATEWAY);
        assert_eq!(
            err.to_string(),
            "Upstream LLM error: provider 'openai' rejected this server's API key (HTTP 401). \
             Ask your admin to check PLATFORM_OPENAI_API_KEY."
        );

        let (cfg, mut own_key) = openai_chain(openai.url(), vec![]);
        own_key.platform_paid = false;
        let err = execute_chat(&reqwest::Client::new(), &cfg, &own_key, &hi())
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Upstream LLM error: provider 'openai' rejected the API key from your LLM config \
             (HTTP 401). Update the key in LLM router."
        );
        assert!(!err.to_string().contains("Incorrect API key"));
    }

    #[test]
    fn a_gemini_bad_key_400_does_not_end_the_chain() {
        let bad_key = ProviderError::Status {
            status: 400,
            message: "API key not valid. Please pass a valid API key.".into(),
            retryable: false,
        };
        assert!(!ends_the_chain(&bad_key));
    }

    #[tokio::test]
    async fn a_missing_model_still_lets_the_fallback_answer() {
        let mut openai = mockito::Server::new_async().await;
        let gone = json!({"error": {"message": "This model has been deprecated."}}).to_string();
        model_mock(&mut openai, "gpt-4o", 404, gone, 1).await;
        model_mock(&mut openai, "gpt-4o-mini", 200, ok_body(), 1).await;
        let (cfg, primary) = openai_chain(openai.url(), vec!["openai/gpt-4o-mini"]);

        let (_, (_, model)) = execute_chat(&reqwest::Client::new(), &cfg, &primary, &hi())
            .await
            .unwrap();

        assert_eq!(model, "gpt-4o-mini");
    }

    #[tokio::test]
    async fn an_input_refused_by_name_beats_a_later_server_error() {
        let mut openai = mockito::Server::new_async().await;
        model_mock(&mut openai, "gpt-4o-mini", 502, "bad gateway".into(), 1).await;
        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        let mut primary = primary("anthropic", vec!["openai/gpt-4o-mini"]);
        primary.temperature = None;
        let req: ChatRequest = serde_json::from_value(json!({
            "messages": [{ "role": "user", "content": [
                { "type": "text", "text": "what is this" },
                { "type": "input_audio", "input_audio": { "data": "UklGRg==", "format": "wav" } }
            ]}]
        }))
        .unwrap();

        let err = execute_chat(&reqwest::Client::new(), &cfg, &primary, &req)
            .await
            .unwrap_err();

        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(
            err.to_string()
                .starts_with("Anthropic does not accept this input"),
            "{err}"
        );
    }

    /// A listener that accepts every connection and never answers, and how many it took.
    async fn stalled_listener() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let connections = Arc::new(AtomicUsize::new(0));
        let seen = connections.clone();
        tokio::spawn(async move {
            // Hold every socket open and silent.
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                seen.fetch_add(1, Ordering::SeqCst);
                held.push(socket);
            }
        });
        (base, connections)
    }

    /// A provider that accepts the connection and never answers. Each of the three attempts
    /// gets its share of the 1 s deadline, so all three are tried and the chain still ends
    /// near 1 s, as a 504 that says which model did not answer.
    #[tokio::test]
    async fn a_stalled_provider_is_bounded_once_across_all_attempts() {
        use std::sync::atomic::Ordering;

        let (base, connections) = stalled_listener().await;
        let cfg = GatewayConfig {
            openai_api_base: base,
            platform_openai_api_key: "sk-platform".into(),
            provider_timeout_secs: 1,
            ..Default::default()
        };
        let mut primary = primary("openai", vec!["openai/gpt-4o-mini", "openai/gpt-4o"]);
        primary.temperature = None;
        primary.max_tokens = None;
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        // The read timeout is longer than the deadline: the shares, not it, end each attempt.
        let http = crate::provider_client(std::time::Duration::from_secs(5));

        let started = std::time::Instant::now();
        let mut attempts = AttemptLog::default();
        let err = execute_chat_by(
            &http,
            &cfg,
            &primary,
            &req,
            provider_deadline(&cfg),
            &mut attempts,
        )
        .await
        .unwrap_err();
        let elapsed = started.elapsed();

        assert!(
            err.to_string().contains("did not answer within"),
            "expected a per-attempt timeout, got {err}"
        );
        assert_eq!(err.status(), axum::http::StatusCode::GATEWAY_TIMEOUT);
        assert!(
            elapsed < std::time::Duration::from_millis(1300),
            "{elapsed:?}"
        );
        assert_eq!(connections.load(Ordering::SeqCst), 3);
        let classes: Vec<_> = attempts
            .take_failed()
            .into_iter()
            .map(|f| f.failure.class)
            .collect();
        assert_eq!(classes, vec!["timeout"; 3]);
    }

    /// With one attempt, its share is the whole deadline, the same as the client's read
    /// timeout. Whichever timer fires first, the caller gets a 504 that names the model.
    #[tokio::test]
    async fn a_single_stalled_attempt_is_a_timeout_even_when_the_read_timeout_fires() {
        let (base, _) = stalled_listener().await;
        let cfg = GatewayConfig {
            openai_api_base: base,
            provider_timeout_secs: 1,
            ..Default::default()
        };
        let mut primary = primary("openai", vec![]);
        primary.temperature = None;
        primary.max_tokens = None;
        let http = crate::provider_client(std::time::Duration::from_secs(1));

        let err = execute_chat(&http, &cfg, &primary, &hi())
            .await
            .unwrap_err();

        assert_eq!(
            err.status(),
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            "{err}"
        );
        assert!(err.to_string().contains("did not answer within"), "{err}");
    }

    /// A stalled primary is cut off at its share, and the healthy fallback answers well
    /// inside the chain deadline.
    #[tokio::test]
    async fn a_stalled_primary_leaves_time_for_a_healthy_fallback() {
        let (stalled, _) = stalled_listener().await;
        let mut healthy = mockito::Server::new_async().await;
        let answered = healthy
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(ok_body())
            .expect(1)
            .create_async()
            .await;
        let cfg = GatewayConfig {
            openai_api_base: healthy.url(),
            platform_openai_api_key: "sk-platform".into(),
            provider_timeout_secs: 2,
            ..Default::default()
        };
        // The primary is a custom endpoint that stalls; the cross-provider fallback is
        // OpenAI on the healthy mock.
        let mut primary = primary("stall-gw", vec!["openai/backup"]);
        primary.custom_endpoint = Some(crate::resolver::CustomEndpoint {
            base_url: stalled,
            dialect: crate::providers::ProviderDialect::OpenAi,
        });
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let http = crate::provider_client(std::time::Duration::from_secs(5));

        let started = std::time::Instant::now();
        let (_, (provider, model)) = execute_chat(&http, &cfg, &primary, &req).await.unwrap();

        assert_eq!((provider.as_str(), model.as_str()), ("openai", "backup"));
        // Primary's share is half of 2 s; the fallback answers at once after it.
        assert!(
            started.elapsed() < std::time::Duration::from_millis(1600),
            "{:?}",
            started.elapsed()
        );
        answered.assert_async().await;
    }

    /// The compact-tools re-send runs a second chain against the first one's deadline, so
    /// a request whose first chain spent the budget gets no second budget.
    #[tokio::test]
    async fn a_second_chain_on_a_spent_deadline_ends_at_once() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let cfg = GatewayConfig {
            openai_api_base: base,
            provider_timeout_secs: 1,
            ..Default::default()
        };
        let primary = primary("openai", vec![]);
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let http = reqwest::Client::new();
        let deadline = provider_deadline(&cfg);

        let mut attempts = AttemptLog::default();
        let first = execute_chat_by(&http, &cfg, &primary, &req, deadline, &mut attempts).await;
        assert!(first.is_err());
        // The attempt the deadline cut off held the request at the provider: a failed call.
        let cut_off = attempts.take_failed();
        assert_eq!(cut_off.len(), 1);
        assert_eq!(cut_off[0].failure.class, "timeout");
        let started = std::time::Instant::now();
        let second = execute_chat_by(&http, &cfg, &primary, &req, deadline, &mut attempts).await;

        let err = second.unwrap_err();
        assert!(err.to_string().contains("across all attempts"), "{err}");
        assert!(
            attempts.take_failed().is_empty(),
            "a spent budget sends nothing"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_millis(200),
            "{:?}",
            started.elapsed()
        );
    }

    /// A primary that answers 503 and a fallback that succeeds are two calls: the failed one
    /// is left in the log for the caller to bill, the success is returned as before.
    #[tokio::test]
    async fn a_failed_primary_before_a_successful_fallback_is_logged_as_its_own_call() {
        let mut openai = mockito::Server::new_async().await;
        openai
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(
                json!({ "model": "primary-model" }),
            ))
            .with_status(503)
            .with_body("overloaded")
            .create_async()
            .await;
        openai
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::PartialJson(json!({ "model": "backup" })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "x", "object": "chat.completion", "created": 1, "model": "backup",
                    "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 3, "completion_tokens": 1, "total_tokens": 4 }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            ..Default::default()
        };
        let primary = primary("openai", vec!["openai/backup"]);
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let mut attempts = AttemptLog::default();

        let (resp, (_, model)) = execute_chat_by(
            &reqwest::Client::new(),
            &cfg,
            &primary,
            &req,
            provider_deadline(&cfg),
            &mut attempts,
        )
        .await
        .unwrap();

        assert_eq!(model, "backup");
        assert_eq!(resp.usage.and_then(|u| u.prompt_tokens), Some(3));
        let failed = attempts.take_failed();
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert_eq!(failed[0].model, "primary-model");
        assert_eq!(failed[0].failure.class, "status");
        assert_eq!(failed[0].failure.http_status, Some(503));
    }

    /// A chain where every attempt fails leaves one failed call per attempt, non-streaming
    /// (429 then 500) and streaming (a 503 before any chunk) alike.
    #[tokio::test]
    async fn an_exhausted_chain_logs_every_failed_attempt() {
        let mut openai = mockito::Server::new_async().await;
        for (model, status) in [("primary-model", 429), ("backup", 500)] {
            openai
                .mock("POST", "/chat/completions")
                .match_body(mockito::Matcher::PartialJson(json!({ "model": model })))
                .with_status(status)
                .with_body("upstream error")
                .create_async()
                .await;
        }
        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            ..Default::default()
        };
        let primary = primary("openai", vec!["openai/backup"]);
        let req: ChatRequest =
            serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hi" }] }))
                .unwrap();
        let http = reqwest::Client::new();

        let mut attempts = AttemptLog::default();
        let err = execute_chat_by(
            &http,
            &cfg,
            &primary,
            &req,
            provider_deadline(&cfg),
            &mut attempts,
        )
        .await
        .unwrap_err();
        // The primary's 429 is reported, not the fallback's later 500.
        assert_eq!(err.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        let statuses: Vec<_> = attempts
            .take_failed()
            .into_iter()
            .map(|f| (f.model, f.failure.http_status))
            .collect();
        assert_eq!(
            statuses,
            vec![
                ("primary-model".to_string(), Some(429)),
                ("backup".to_string(), Some(500))
            ]
        );

        let mut streamed = AttemptLog::default();
        let only_primary = ResolvedConfig {
            fallback_models: Vec::new(),
            ..primary
        };
        let mut stream_server = mockito::Server::new_async().await;
        stream_server
            .mock("POST", "/chat/completions")
            .with_status(503)
            .with_body("overloaded")
            .create_async()
            .await;
        let stream_cfg = GatewayConfig {
            openai_api_base: stream_server.url(),
            ..Default::default()
        };
        let streaming_req = ChatRequest {
            stream: Some(true),
            ..req
        };
        assert!(
            execute_chat_stream(
                &http,
                &stream_cfg,
                &only_primary,
                &streaming_req,
                &mut streamed
            )
            .await
            .is_err()
        );
        let failed = streamed.take_failed();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].failure.http_status, Some(503));
    }

    /// An audio part routed to Anthropic is refused by the spoke before any request
    /// leaves, and the caller gets a 400 naming why — not a 502 its SDK would retry.
    #[tokio::test]
    async fn input_the_provider_cannot_accept_is_400_and_never_sent() {
        let mut anthropic = mockito::Server::new_async().await;
        let never_called = anthropic
            .mock("POST", "/messages")
            .expect(0)
            .create_async()
            .await;
        let cfg = GatewayConfig {
            anthropic_api_base: anthropic.url(),
            ..Default::default()
        };
        let req: ChatRequest = serde_json::from_value(json!({ "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "what is the code?" },
                { "type": "input_audio", "input_audio": { "data": "UklGRg==", "format": "wav" } }
            ]
        }] }))
        .unwrap();
        let err = execute_chat(
            &reqwest::Client::new(),
            &cfg,
            &primary("anthropic", vec![]),
            &req,
        )
        .await
        .unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(
            err.to_string()
                .contains("Anthropic does not accept this input"),
            "{err}"
        );
        assert!(err.to_string().contains("input_audio"), "{err}");
        never_called.assert_async().await;
    }

    /// One OpenAI mock answering `primary-model` with `primary` and `backup` with
    /// `backup` (status, body), and the chain config pointing at it. Each mock expects
    /// exactly the given number of calls.
    async fn two_model_server(
        primary: (u16, &str, usize),
        backup: (u16, &str, usize),
    ) -> (mockito::ServerGuard, Vec<mockito::Mock>, GatewayConfig) {
        let mut openai = mockito::Server::new_async().await;
        let mut mocks = Vec::new();
        for (model, (status, body, calls)) in [("primary-model", primary), ("backup", backup)] {
            mocks.push(
                openai
                    .mock("POST", "/chat/completions")
                    .match_body(mockito::Matcher::PartialJson(json!({ "model": model })))
                    .with_status(status as usize)
                    .with_header("content-type", "application/json")
                    .with_body(body)
                    .expect(calls)
                    .create_async()
                    .await,
            );
        }
        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        (openai, mocks, cfg)
    }

    /// A request the provider refuses as malformed would be refused by every model: it is
    /// sent once, and its 400 comes back, however many fallbacks are configured.
    #[tokio::test]
    async fn a_bad_request_is_sent_once_however_many_fallbacks() {
        let refused = r#"{"error":{"message":"The file could not be processed."}}"#;
        let mut openai = mockito::Server::new_async().await;
        let only_once = openai
            .mock("POST", "/chat/completions")
            .with_status(400)
            .with_body(refused)
            .expect(1)
            .create_async()
            .await;
        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            ..Default::default()
        };
        let primary = primary("openai", vec!["openai/backup", "openai/backup-2"]);

        let mut attempts = AttemptLog::default();
        let err = execute_chat_by(
            &reqwest::Client::new(),
            &cfg,
            &primary,
            &hi(),
            provider_deadline(&cfg),
            &mut attempts,
        )
        .await
        .unwrap_err();

        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
        assert!(err.to_string().contains("could not be processed"), "{err}");
        assert_eq!(attempts.take_failed().len(), 1);
        only_once.assert_async().await;
    }

    /// A model the provider does not know is the model's fault, not the request's: the
    /// fallback answers. Bedrock says so with a 400, which must not end the chain either.
    #[tokio::test]
    async fn an_unknown_primary_model_falls_back() {
        for (status, body) in [
            (
                404,
                r#"{"error":{"message":"The model `primary-model` does not exist"}}"#,
            ),
            (
                400,
                r#"{"message":"The provided model identifier is invalid."}"#,
            ),
        ] {
            let (_server, mocks, cfg) =
                two_model_server((status, body, 1), (200, &ok_body(), 1)).await;
            let primary = primary("openai", vec!["openai/backup"]);

            let (_, (_, model)) = execute_chat(&reqwest::Client::new(), &cfg, &primary, &hi())
                .await
                .unwrap();

            assert_eq!(model, "backup", "after a {status}");
            for mock in mocks {
                mock.assert_async().await;
            }
        }
    }

    /// Input the primary's provider cannot take is refused before it is sent; a fallback on
    /// a provider that takes it answers.
    #[tokio::test]
    async fn input_the_primary_cannot_accept_goes_to_the_fallback() {
        let mut openai = mockito::Server::new_async().await;
        let answered = openai
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(ok_body())
            .expect(1)
            .create_async()
            .await;
        let cfg = GatewayConfig {
            openai_api_base: openai.url(),
            platform_openai_api_key: "sk-platform".into(),
            ..Default::default()
        };
        let req: ChatRequest = serde_json::from_value(json!({ "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "what is the code?" },
                { "type": "input_audio", "input_audio": { "data": "UklGRg==", "format": "wav" } }
            ]
        }] }))
        .unwrap();

        let (_, (provider, model)) = execute_chat(
            &reqwest::Client::new(),
            &cfg,
            &primary("anthropic", vec!["openai/backup"]),
            &req,
        )
        .await
        .unwrap();

        assert_eq!((provider.as_str(), model.as_str()), ("openai", "backup"));
        answered.assert_async().await;
    }

    /// When every model fails, the primary's error is the one reported: its 404 tells the
    /// caller what to fix, the fallback's later 500 does not.
    #[tokio::test]
    async fn an_exhausted_chain_reports_the_first_error() {
        let (_server, mocks, cfg) = two_model_server(
            (404, r#"{"error":{"message":"model not found"}}"#, 1),
            (500, "boom", 1),
        )
        .await;
        let primary = primary("openai", vec!["openai/backup"]);

        let err = execute_chat(&reqwest::Client::new(), &cfg, &primary, &hi())
            .await
            .unwrap_err();

        assert_eq!(err.status(), axum::http::StatusCode::NOT_FOUND);
        assert!(err.to_string().contains("model not found"), "{err}");
        for mock in mocks {
            mock.assert_async().await;
        }
    }
}
