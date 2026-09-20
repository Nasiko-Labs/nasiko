//! `POST /v1/route` — DronaHQ contract (v1.0): tier hint → provider cascade → unified reply.
//!
//! Default path uses platform provider keys and the contract tier cascade (agnostic).
//! When the caller presents an agent JWT and `agents.tier_cascade = false`, the handler
//! instead resolves that agent's `llm_config` / BYOK provider (console-selected) and
//! calls only that destination — so NVIDIA BYOK sticks.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::LlmRouterCtx;
use crate::auth::verify_agent_jwt;
use crate::config::GatewayConfig;
use crate::ir::{ChatRequest, Message, Usage};
use crate::providers::{self, ProviderError};
use crate::resolver::{PgRegistry, RegistryStore, RequestHint, ResolvedConfig, resolve};
use crate::routing::tier::{
    ContractTier, TierCandidate, cascade_for, cheapest_candidate, estimate_prompt_tokens,
    projected_completion_tokens, resolve_tier,
};
use crate::usage::{self, UsageRecord};

const CONTRACT_VERSION: &str = "1.0";

const TASK_TYPES: &[&str] = &[
    "summarize",
    "translate",
    "reason",
    "code",
    "chat",
    "extract",
];

/// Process-local reply cache for `allow_cache=true` (DronaHQ contract).
#[derive(Clone)]
struct CachedRoute {
    provider: String,
    model: String,
    content: String,
    usage: RouteUsage,
    tier_used: String,
}

fn route_response_cache() -> &'static Mutex<HashMap<u64, CachedRoute>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, CachedRoute>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn route_cache_key(tier: ContractTier, messages: &[Message], task_type: &str) -> u64 {
    let mut h = DefaultHasher::new();
    tier.as_str().hash(&mut h);
    task_type.hash(&mut h);
    for m in messages {
        m.role.hash(&mut h);
        if let Some(t) = m.text() {
            t.hash(&mut h);
        }
    }
    h.finish()
}
#[derive(Debug, Deserialize)]
pub struct RouteBody {
    pub task_type: Option<String>,
    pub complexity: Option<i32>,
    pub messages: Option<Vec<RouteMessage>>,
    pub budget_tokens: Option<i64>,
    #[serde(default = "default_allow_cache")]
    pub allow_cache: bool,
}

fn default_allow_cache() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct RouteMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct RouteOk {
    pub provider: String,
    pub model: String,
    pub content: String,
    pub usage: RouteUsage,
    pub cache_hit: bool,
    pub tier_used: String,
    pub latency_ms: u64,
    pub trace_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RouteUsage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
}

/// `POST /v1/route`
pub async fn route(
    State(ctx): State<LlmRouterCtx>,
    headers: HeaderMap,
    body: Json<Value>,
) -> Response {
    let started = Instant::now();
    let trace_id = {
        let full = Uuid::new_v4().simple().to_string();
        full[..16].to_string()
    };
    let _session = headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let parsed: RouteBody = match serde_json::from_value(body.0) {
        Ok(b) => b,
        Err(e) => {
            return err_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                &format!("invalid JSON body: {e}"),
                None,
            );
        }
    };

    let task_type = parsed
        .task_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("")
        .to_ascii_lowercase();
    if task_type.is_empty() || !TASK_TYPES.contains(&task_type.as_str()) {
        return err_response(
            StatusCode::BAD_REQUEST,
            "bad_request",
            &format!("task_type must be one of: {}", TASK_TYPES.join(", ")),
            None,
        );
    }

    let messages = match &parsed.messages {
        Some(m) if !m.is_empty() => m,
        _ => {
            return err_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "messages must be a non-empty array",
                None,
            );
        }
    };

    let ir_messages: Vec<Message> = messages
        .iter()
        .map(|m| Message {
            role: m.role.clone(),
            content: m.content.clone(),
            name: None,
            tool_call_id: None,
            tool_calls: None,
            extra: Map::new(),
        })
        .collect();

    let prompt_tokens = estimate_prompt_tokens(&ir_messages);

    if let Some(budget) = parsed.budget_tokens {
        if budget < prompt_tokens {
            return err_response(
                StatusCode::BAD_REQUEST,
                "budget_exceeded",
                "budget_tokens smaller than estimated prompt",
                None,
            );
        }
        let proj_c = projected_completion_tokens(prompt_tokens, Some(budget));
        let cheap = cheapest_candidate();
        if budget < prompt_tokens + 1 {
            return err_response(
                StatusCode::BAD_REQUEST,
                "budget_exceeded",
                "even cheapest tier exceeds budget_tokens",
                Some(cheap.provider),
            );
        }
        let _ = cheap;
        // Carry the projected (capped) completion budget into provider calls.
        let max_tokens = Some(proj_c.max(1));
        return route_after_budget(
            ctx,
            headers,
            parsed,
            ir_messages,
            prompt_tokens,
            max_tokens,
            task_type,
            started,
            trace_id,
        )
        .await;
    }

    route_after_budget(
        ctx,
        headers,
        parsed,
        ir_messages,
        prompt_tokens,
        None,
        task_type,
        started,
        trace_id,
    )
    .await
}

/// Shared body after budget validation (keeps max_tokens projection in one place).
async fn route_after_budget(
    ctx: LlmRouterCtx,
    headers: HeaderMap,
    parsed: RouteBody,
    ir_messages: Vec<Message>,
    prompt_tokens: i64,
    max_tokens: Option<i64>,
    task_type: String,
    started: Instant,
    trace_id: String,
) -> Response {
    let header_tier = headers.get("x-nasiko-tier").and_then(|v| v.to_str().ok());
    let mut tier = resolve_tier(header_tier, parsed.complexity, parsed.budget_tokens);

    if matches!(task_type.as_str(), "reason" | "code") && tier == ContractTier::Cheap {
        if parsed.complexity.unwrap_or(3) >= 4 {
            tier = ContractTier::Balanced;
        }
    }

    // Stub replies need no credentials (local shape checks only).
    if ctx.cfg.route_stub {
        return stub_ok(
            &ir_messages,
            tier,
            prompt_tokens,
            max_tokens,
            parsed.allow_cache,
            started,
            trace_id,
        );
    }

    let agent = optional_agent(&headers, &ctx.cfg);

    // Fail closed: require agent JWT or NASIKO_ROUTE_TOKEN (stub mode skips this).
    if let Err(resp) = require_route_credential(&headers, &ctx.cfg, agent.as_ref()) {
        return resp;
    }

    // Stick-to-selected-provider: agent JWT + agents.tier_cascade = false.
    if let Some((agent_id, owner_id)) = agent.clone() {
        match agent_wants_cascade(&ctx.db, &agent_id).await {
            Ok(false) => {
                return route_selected_provider(
                    &ctx,
                    &agent_id,
                    &owner_id,
                    &ir_messages,
                    tier,
                    parsed.budget_tokens,
                    prompt_tokens,
                    started,
                    trace_id,
                )
                .await;
            }
            Ok(true) => {
                tracing::debug!(
                    target: "nasiko::llm_router::route",
                    %agent_id,
                    "route: tier_cascade=true — using platform cascade"
                );
            }
            Err(e) => {
                // Fail closed: never silently burn platform keys when BYOK-off is set
                // but the flag cannot be read (DB outage / missing migration).
                tracing::error!(
                    target: "nasiko::llm_router::route",
                    %agent_id,
                    error = %e,
                    "route: tier_cascade lookup failed — refusing cascade"
                );
                return err_response(
                    StatusCode::BAD_GATEWAY,
                    "cascade_lookup_failed",
                    "could not read agents.tier_cascade; refusing to fall open to platform cascade",
                    None,
                );
            }
        }
    }

    if parsed.allow_cache {
        let key = route_cache_key(tier, &ir_messages, &task_type);
        if let Ok(guard) = route_response_cache().lock()
            && let Some(hit) = guard.get(&key).cloned()
        {
            let latency_ms = started.elapsed().as_millis() as u64;
            spawn_route_usage(
                &ctx,
                agent.as_ref(),
                hit.provider.clone(),
                hit.model.clone(),
                hit.usage.prompt_tokens,
                hit.usage.completion_tokens,
                hit.usage.total_tokens,
                latency_ms as i64,
                true,
            );
            return ok_response(RouteOk {
                provider: hit.provider,
                model: hit.model,
                content: hit.content,
                usage: hit.usage,
                cache_hit: true,
                tier_used: hit.tier_used,
                latency_ms,
                trace_id,
            });
        }
    }

    let cascade = cascade_for(tier);
    let keyed: Vec<TierCandidate> = cascade
        .iter()
        .copied()
        .filter(|c| has_platform_key(&ctx.cfg, c.provider))
        .collect();

    if keyed.is_empty() {
        return err_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_provider",
            &format!("no platform key configured for tier '{tier}' cascade"),
            None,
        );
    }

    // Prefer projected completion budget when present; else derive from raw budget.
    let max_tokens = max_tokens.or_else(|| {
        parsed
            .budget_tokens
            .map(|b| projected_completion_tokens(prompt_tokens, Some(b)).max(1))
    });
    let mut last_provider_err: Option<(String, ProviderError)> = None;
    let mut saw_rate_limit = false;

    for candidate in &keyed {
        let resolved = platform_resolved(candidate, &ctx.cfg, max_tokens);
        let client = match providers::provider_for(candidate.provider, &ctx.http, &ctx.cfg) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    target: "nasiko::llm_router::route",
                    provider = candidate.provider,
                    error = %e,
                    "route: failed to construct provider client; trying next"
                );
                continue;
            }
        };

        let chat_req = ChatRequest {
            model: Some(candidate.model.to_string()),
            messages: ir_messages.clone(),
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens,
            stream: Some(false),
            extra: Map::new(),
        };

        tracing::info!(
            target: "nasiko::llm_router::route",
            %trace_id,
            tier = %tier,
            provider = candidate.provider,
            model = candidate.model,
            task_type = %task_type,
            allow_cache = parsed.allow_cache,
            "route: attempting provider"
        );

        match client.chat(&chat_req, &resolved).await {
            Ok(resp) => {
                let content = resp
                    .choices
                    .first()
                    .and_then(|c| c.message.text())
                    .unwrap_or_default();
                let usage = resp.usage.unwrap_or_default();
                let p = usage.prompt_tokens.unwrap_or(prompt_tokens);
                let c = usage.completion_tokens.unwrap_or(0);
                let total = usage.total_tokens.unwrap_or(p + c);
                let cost = round6(candidate.estimate_cost_usd(p, c));
                let latency_ms = started.elapsed().as_millis() as u64;
                let usage = RouteUsage {
                    prompt_tokens: p,
                    completion_tokens: c,
                    total_tokens: total,
                    cost_usd: cost,
                };
                if parsed.allow_cache {
                    let key = route_cache_key(tier, &ir_messages, &task_type);
                    if let Ok(mut guard) = route_response_cache().lock() {
                        if guard.len() >= 256 {
                            guard.clear();
                        }
                        guard.insert(
                            key,
                            CachedRoute {
                                provider: candidate.provider.to_string(),
                                model: candidate.model.to_string(),
                                content: content.clone(),
                                usage: usage.clone(),
                                tier_used: tier.as_str().to_string(),
                            },
                        );
                    }
                }
                spawn_route_usage(
                    &ctx,
                    agent.as_ref(),
                    candidate.provider.to_string(),
                    candidate.model.to_string(),
                    p,
                    c,
                    total,
                    latency_ms as i64,
                    /* platform_paid */ true,
                );
                return ok_response(RouteOk {
                    provider: candidate.provider.to_string(),
                    model: candidate.model.to_string(),
                    content,
                    usage,
                    cache_hit: false,
                    tier_used: tier.as_str().to_string(),
                    latency_ms,
                    trace_id,
                });
            }
            Err(e) => {
                tracing::warn!(
                    target: "nasiko::llm_router::route",
                    %trace_id,
                    provider = candidate.provider,
                    model = candidate.model,
                    error = %e,
                    "route: provider failed; considering cascade"
                );
                if let ProviderError::Status { status: 429, .. } = &e {
                    saw_rate_limit = true;
                }
                last_provider_err = Some((candidate.provider.to_string(), e));
            }
        }
    }

    if let Some((provider, err)) = last_provider_err {
        if saw_rate_limit {
            if let ProviderError::Status { status: 429, .. } = &err {
                return err_response(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limited",
                    &err.to_string(),
                    Some(&provider),
                );
            }
        }
        return err_response(
            StatusCode::BAD_GATEWAY,
            "provider_error",
            &err.to_string(),
            Some(&provider),
        );
    }

    err_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "no_provider",
        "no provider satisfied the tier cascade",
        None,
    )
}

fn require_route_credential(
    headers: &HeaderMap,
    cfg: &GatewayConfig,
    agent: Option<&(String, String)>,
) -> Result<(), Response> {
    // Agent JWT satisfies auth (selected-provider path / trusted agent).
    if agent.is_some() {
        return Ok(());
    }
    let expected = cfg.route_token.trim();
    if expected.is_empty() {
        return Err(err_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "set NASIKO_ROUTE_TOKEN or present a valid agent JWT",
            None,
        ));
    }
    let header = headers
        .get("x-nasiko-route-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let bearer = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
        .unwrap_or("");
    if header == expected || bearer == expected {
        return Ok(());
    }
    Err(err_response(
        StatusCode::UNAUTHORIZED,
        "unauthorized",
        "NASIKO_ROUTE_TOKEN required (or a valid agent JWT)",
        None,
    ))
}

fn optional_agent(headers: &HeaderMap, cfg: &GatewayConfig) -> Option<(String, String)> {
    let authz = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    verify_agent_jwt(authz, cfg).ok()
}

async fn agent_wants_cascade(db: &sqlx::PgPool, agent_id: &str) -> Result<bool, sqlx::Error> {
    let Ok(uuid) = Uuid::parse_str(agent_id) else {
        return Ok(true);
    };
    let flag: Option<bool> =
        sqlx::query_scalar("SELECT tier_cascade FROM agents WHERE id = $1 AND deleted_at IS NULL")
            .bind(uuid)
            .fetch_optional(db)
            .await?;
    Ok(flag.unwrap_or(true))
}

/// Honor the agent's console-selected provider/model (BYOK / llm_config / pin).
async fn route_selected_provider(
    ctx: &LlmRouterCtx,
    agent_id: &str,
    owner_id: &str,
    ir_messages: &[Message],
    tier: ContractTier,
    budget_tokens: Option<i64>,
    prompt_tokens: i64,
    started: Instant,
    trace_id: String,
) -> Response {
    let store = PgRegistry::new(ctx.db.clone());
    route_selected_provider_with_store(
        ctx,
        &store,
        agent_id,
        owner_id,
        ir_messages,
        tier,
        budget_tokens,
        prompt_tokens,
        started,
        trace_id,
    )
    .await
}

async fn route_selected_provider_with_store(
    ctx: &LlmRouterCtx,
    store: &dyn RegistryStore,
    agent_id: &str,
    owner_id: &str,
    ir_messages: &[Message],
    tier: ContractTier,
    budget_tokens: Option<i64>,
    prompt_tokens: i64,
    started: Instant,
    trace_id: String,
) -> Response {
    let hint = RequestHint {
        provider: None,
        model: None,
    };
    let mut resolved = match resolve(store, &ctx.cache, &ctx.cfg, agent_id, owner_id, hint).await {
        Ok(r) => r,
        Err(e) => {
            return err_response(
                StatusCode::BAD_GATEWAY,
                "resolve_error",
                &e.to_string(),
                None,
            );
        }
    };

    // Compliance lock: same as chat_core — pinned model wins, no silent fallbacks.
    if let Some(pin) = resolved.pinned_model.clone() {
        tracing::info!(
            target: "nasiko::llm_router::route",
            %agent_id,
            pinned_model = %pin,
            "route: agent is pinned — using pinned model"
        );
        resolved.litellm_model = format!("{}/{}", resolved.provider, pin);
        resolved.model = pin;
        resolved.fallback_models.clear();
    }

    tracing::info!(
        target: "nasiko::llm_router::route",
        %trace_id,
        %agent_id,
        provider = %resolved.provider,
        model = %resolved.model,
        "route: tier_cascade=false — using selected provider"
    );

    let max_tokens = budget_tokens
        .map(|b| projected_completion_tokens(prompt_tokens, Some(b)).max(1))
        .or(resolved.max_tokens);

    let client = match providers::provider_for(&resolved.provider, &ctx.http, &ctx.cfg) {
        Ok(c) => c,
        Err(e) => {
            return err_response(
                StatusCode::BAD_GATEWAY,
                "provider_error",
                &e.to_string(),
                Some(&resolved.provider),
            );
        }
    };

    let chat_req = ChatRequest {
        model: Some(resolved.model.clone()),
        messages: ir_messages.to_vec(),
        tools: None,
        tool_choice: None,
        temperature: resolved.temperature,
        max_tokens,
        stream: Some(false),
        extra: Map::new(),
    };

    match client.chat(&chat_req, &resolved).await {
        Ok(resp) => {
            let content = resp
                .choices
                .first()
                .and_then(|c| c.message.text())
                .unwrap_or_default();
            let usage = resp.usage.unwrap_or_default();
            let p = usage.prompt_tokens.unwrap_or(prompt_tokens);
            let c = usage.completion_tokens.unwrap_or(0);
            let total = usage.total_tokens.unwrap_or(p + c);
            let cost = cost_usd_for(
                &ctx.db,
                &resolved.provider,
                &resolved.model,
                p,
                c,
            )
            .await;
            let latency_ms = started.elapsed().as_millis() as u64;
            spawn_route_usage(
                ctx,
                Some(&(agent_id.to_string(), owner_id.to_string())),
                resolved.provider.clone(),
                resolved.model.clone(),
                p,
                c,
                total,
                latency_ms as i64,
                resolved.platform_paid,
            );
            ok_response(RouteOk {
                provider: resolved.provider,
                model: resolved.model,
                content,
                usage: RouteUsage {
                    prompt_tokens: p,
                    completion_tokens: c,
                    total_tokens: total,
                    cost_usd: cost,
                },
                cache_hit: false,
                tier_used: format!("selected:{}", tier.as_str()),
                latency_ms,
                trace_id,
            })
        }
        Err(e) => err_response(
            StatusCode::BAD_GATEWAY,
            "provider_error",
            &e.to_string(),
            Some(&resolved.provider),
        ),
    }
}

fn estimate_cost_usd(provider: &str, model: &str, prompt: i64, completion: i64) -> f64 {
    for tier in [
        ContractTier::Cheap,
        ContractTier::Balanced,
        ContractTier::Premium,
    ] {
        for cand in cascade_for(tier) {
            if cand.provider == provider && cand.model == model {
                return round6(cand.estimate_cost_usd(prompt, completion));
            }
        }
    }
    round6(((prompt as f64) * 0.00015 + (completion as f64) * 0.0006) / 1000.0)
}

/// Prefer live `model_pricing` rows (provider + model); fall back to the tier table /
/// gpt-4o-mini-ish rates so BYOK models (e.g. NVIDIA at $0) report correctly.
async fn cost_usd_for(
    db: &sqlx::PgPool,
    provider: &str,
    model: &str,
    prompt: i64,
    completion: i64,
) -> f64 {
    #[derive(sqlx::FromRow)]
    struct Row {
        input_price_per_1m: f64,
        output_price_per_1m: f64,
    }

    let row = sqlx::query_as::<_, Row>(
        r#"SELECT input_price_per_1m::float8 AS input_price_per_1m,
                  output_price_per_1m::float8 AS output_price_per_1m
           FROM model_pricing
           WHERE provider = $1
             AND model = $2
             AND effective_from <= now()
             AND (effective_until IS NULL OR effective_until > now())
           ORDER BY effective_from DESC
           LIMIT 1"#,
    )
    .bind(provider)
    .bind(model)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();

    if let Some(row) = row {
        return round6(
            (prompt as f64) * row.input_price_per_1m / 1_000_000.0
                + (completion as f64) * row.output_price_per_1m / 1_000_000.0,
        );
    }
    estimate_cost_usd(provider, model, prompt, completion)
}

fn has_platform_key(cfg: &GatewayConfig, provider: &str) -> bool {
    // Ollama is opt-in — never probe the default localhost daemon unless enabled.
    if provider == "ollama" {
        return cfg.ollama_enabled;
    }
    !cfg.platform_key_for(provider).is_empty()
}

/// Fire-and-forget `token_usage` for `/v1/route`.
///
/// Anonymous `NASIKO_ROUTE_TOKEN` callers have no user UUID — [`usage::log_usage`]
/// skips those rows. Agent-JWT callers always attribute to the minting owner.
fn spawn_route_usage(
    ctx: &LlmRouterCtx,
    agent: Option<&(String, String)>,
    provider: String,
    model: String,
    prompt: i64,
    completion: i64,
    total: i64,
    latency_ms: i64,
    platform_paid: bool,
) {
    let (agent_id, owner_id) = match agent {
        Some((a, o)) => (a.clone(), o.clone()),
        None => (String::new(), String::new()),
    };
    usage::spawn_log(
        ctx.db.clone(),
        UsageRecord {
            owner_id,
            agent_id,
            operation_type: "route_llm",
            provider,
            model,
            usage: Some(Usage {
                prompt_tokens: Some(prompt),
                completion_tokens: Some(completion),
                total_tokens: Some(total),
            }),
            latency_ms,
            streaming: false,
            finish_reason: Some("stop".into()),
            flow_id: None,
            platform_paid,
        },
    );
}

fn platform_resolved(
    candidate: &TierCandidate,
    cfg: &GatewayConfig,
    max_tokens: Option<i64>,
) -> ResolvedConfig {
    let provider = candidate.provider.to_string();
    let model = candidate.model.to_string();
    let api_key = cfg.platform_key_for(candidate.provider).to_string();
    ResolvedConfig {
        litellm_model: format!("{provider}/{model}"),
        provider,
        model,
        api_key,
        fallback_models: vec![],
        temperature: None,
        max_tokens,
        has_llm_config: false,
        pinned_model: None,
        tier1_model: None,
        tier2_model: None,
        tier3_model: None,
        platform_paid: true,
        is_coding_agent: false,
    }
}

fn round6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

fn stub_ok(
    messages: &[Message],
    tier: ContractTier,
    prompt_tokens: i64,
    max_completion: Option<i64>,
    allow_cache: bool,
    started: Instant,
    trace_id: String,
) -> Response {
    let candidate = cascade_for(tier)
        .first()
        .copied()
        .unwrap_or_else(cheapest_candidate);
    // Match live routing: never invent more completion tokens than the projected budget.
    let mut completion_tokens = 80 + (prompt_tokens % 50);
    if let Some(cap) = max_completion {
        completion_tokens = completion_tokens.min(cap.max(1));
    }
    let cost = round6(candidate.estimate_cost_usd(prompt_tokens, completion_tokens));
    let user = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(|m| m.text())
        .unwrap_or_default();
    let snippet: String = user.chars().take(80).collect();
    ok_response(RouteOk {
        provider: candidate.provider.to_string(),
        model: candidate.model.to_string(),
        content: format!(
            "[STUB · {}/{}] Response to: {snippet}",
            candidate.provider, candidate.model
        ),
        usage: RouteUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            cost_usd: cost,
        },
        cache_hit: allow_cache && prompt_tokens % 20 == 0,
        tier_used: tier.as_str().to_string(),
        latency_ms: started.elapsed().as_millis() as u64,
        trace_id,
    })
}

fn ok_response(body: RouteOk) -> Response {
    let mut res = (StatusCode::OK, Json(body)).into_response();
    if let Ok(v) = HeaderValue::from_str(CONTRACT_VERSION) {
        res.headers_mut().insert("x-contract-version", v);
    }
    res
}

fn err_response(status: StatusCode, code: &str, message: &str, provider: Option<&str>) -> Response {
    let mut err = json!({
        "code": code,
        "message": message,
    });
    if let Some(p) = provider {
        err["provider"] = json!(p);
    }
    let mut res = (status, Json(json!({ "error": err }))).into_response();
    if let Ok(v) = HeaderValue::from_str(CONTRACT_VERSION) {
        res.headers_mut().insert("x-contract-version", v);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::{AgentConfigResult, ConfigCache, LLMConfig};
    use async_trait::async_trait;
    use jsonwebtoken::Algorithm;
    use std::sync::Arc;
    use std::time::Duration;

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";
    const OWNER: &str = "22222222-2222-2222-2222-222222222222";
    const SECRET: &str = "gateway-secret";

    fn cfg_auth(token: &str) -> GatewayConfig {
        GatewayConfig {
            agent_jwt_secret: SECRET.into(),
            route_token: token.into(),
            ..Default::default()
        }
    }

    fn jwt() -> String {
        crate::auth::mint_agent_token(AGENT, OWNER, SECRET, 3600, Algorithm::HS256).unwrap()
    }

    #[test]
    fn rejects_empty_task_type_shape() {
        let raw = json!({"messages":[{"role":"user","content":"hi"}]});
        let parsed: RouteBody = serde_json::from_value(raw).unwrap();
        assert!(parsed.task_type.is_none());
    }

    #[test]
    fn credential_rejects_when_token_unset() {
        let cfg = cfg_auth("");
        let err = require_route_credential(&HeaderMap::new(), &cfg, None).unwrap_err();
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn credential_accepts_matching_route_token_header() {
        let cfg = cfg_auth("secret-route");
        let mut headers = HeaderMap::new();
        headers.insert("x-nasiko-route-token", "secret-route".parse().unwrap());
        assert!(require_route_credential(&headers, &cfg, None).is_ok());
    }

    #[test]
    fn credential_accepts_agent_jwt() {
        let cfg = cfg_auth("");
        let agent = (AGENT.to_string(), OWNER.to_string());
        assert!(require_route_credential(&HeaderMap::new(), &cfg, Some(&agent)).is_ok());
    }

    #[test]
    fn ollama_gated_off_by_default() {
        let cfg = GatewayConfig::default();
        assert!(!has_platform_key(&cfg, "ollama"));
        let enabled = GatewayConfig {
            ollama_enabled: true,
            ..Default::default()
        };
        assert!(has_platform_key(&enabled, "ollama"));
    }

    #[test]
    fn filters_cascade_to_keyed_providers_only() {
        let cfg = GatewayConfig {
            platform_groq_api_key: "gsk".into(),
            ollama_enabled: false,
            ..Default::default()
        };
        let keyed: Vec<_> = cascade_for(ContractTier::Cheap)
            .iter()
            .copied()
            .filter(|c| has_platform_key(&cfg, c.provider))
            .collect();
        assert!(keyed.iter().all(|c| c.provider == "groq"));
        assert!(!keyed.is_empty());
    }

    #[test]
    fn stub_clamps_completion_to_budget() {
        let msgs = vec![Message {
            role: "user".into(),
            content: Some(json!("hi")),
            name: None,
            tool_call_id: None,
            tool_calls: None,
            extra: Map::new(),
        }];
        let resp = stub_ok(
            &msgs,
            ContractTier::Cheap,
            10,
            Some(5),
            false,
            Instant::now(),
            "trace".into(),
        );
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn stub_route_ok_without_credentials() {
        let cfg = GatewayConfig {
            route_stub: true,
            agent_jwt_secret: SECRET.into(),
            ..Default::default()
        };
        let ctx = LlmRouterCtx {
            db: sqlx::PgPool::connect_lazy("postgres://u:p@127.0.0.1:5999/none").unwrap(),
            http: reqwest::Client::new(),
            cfg: Arc::new(cfg),
            cache: Arc::new(ConfigCache::new(Duration::from_secs(30))),
            router_cache: Arc::new(crate::routing::NoopCache),
            tier_registry: Arc::new(crate::routing::StaticTierRegistry),
            cell_store: Arc::new(crate::routing::InMemoryCellStore::new()),
        };
        let body = json!({
            "task_type": "chat",
            "messages": [{"role":"user","content":"hello"}]
        });
        let resp = route(axum::extract::State(ctx), HeaderMap::new(), Json(body)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = jwt();
    }

    #[tokio::test]
    async fn live_route_rejects_unauthenticated() {
        let cfg = GatewayConfig {
            route_stub: false,
            route_token: String::new(),
            agent_jwt_secret: SECRET.into(),
            platform_openai_api_key: "sk-test".into(),
            ..Default::default()
        };
        let ctx = LlmRouterCtx {
            db: sqlx::PgPool::connect_lazy("postgres://u:p@127.0.0.1:5999/none").unwrap(),
            http: reqwest::Client::new(),
            cfg: Arc::new(cfg),
            cache: Arc::new(ConfigCache::new(Duration::from_secs(30))),
            router_cache: Arc::new(crate::routing::NoopCache),
            tier_registry: Arc::new(crate::routing::StaticTierRegistry),
            cell_store: Arc::new(crate::routing::InMemoryCellStore::new()),
        };
        let body = json!({
            "task_type": "chat",
            "messages": [{"role":"user","content":"hello"}]
        });
        let resp = route(axum::extract::State(ctx), HeaderMap::new(), Json(body)).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    struct ByokStore {
        config: LLMConfig,
    }

    #[async_trait]
    impl RegistryStore for ByokStore {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            Ok(Some(AgentConfigResult {
                config: Some(self.config.clone()),
                agent_pinned_model: None,
                is_coding_agent: false,
            }))
        }
        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn tier_cascade_false_uses_selected_byok_provider() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "id": "chatcmpl-x",
                    "object": "chat.completion",
                    "model": "gpt-4o-mini",
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": "byok-ok" },
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 3,
                        "completion_tokens": 2,
                        "total_tokens": 5
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let cfg = GatewayConfig {
            agent_jwt_secret: SECRET.into(),
            openai_api_base: server.url(),
            platform_openai_api_key: "sk-platform".into(),
            // Distinct groq key so cascade would prefer groq if wrongly taken.
            platform_groq_api_key: "gsk-cascade".into(),
            ..Default::default()
        };
        let ctx = LlmRouterCtx {
            db: sqlx::PgPool::connect_lazy("postgres://u:p@127.0.0.1:5999/none").unwrap(),
            http: reqwest::Client::new(),
            cfg: Arc::new(cfg),
            cache: Arc::new(ConfigCache::new(Duration::from_secs(30))),
            router_cache: Arc::new(crate::routing::NoopCache),
            tier_registry: Arc::new(crate::routing::StaticTierRegistry),
            cell_store: Arc::new(crate::routing::InMemoryCellStore::new()),
        };
        let store = ByokStore {
            config: LLMConfig {
                provider: "openai".into(),
                model: Some("gpt-4o-mini".into()),
                fallback_models: vec![],
                temperature: None,
                max_tokens: None,
                api_key_secret_name: None,
                pinned: false,
                pinned_model: None,
                tier1_model: None,
                tier2_model: None,
                tier3_model: None,
            },
        };
        let msgs = vec![Message {
            role: "user".into(),
            content: Some(json!("hi")),
            name: None,
            tool_call_id: None,
            tool_calls: None,
            extra: Map::new(),
        }];
        let resp = route_selected_provider_with_store(
            &ctx,
            &store,
            AGENT,
            OWNER,
            &msgs,
            ContractTier::Cheap,
            None,
            4,
            Instant::now(),
            "tr".into(),
        )
        .await;
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "body={}",
            String::from_utf8_lossy(&body)
        );
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["provider"], "openai");
        assert_eq!(v["model"], "gpt-4o-mini");
        assert_eq!(v["content"], "byok-ok");
        assert!(v["tier_used"].as_str().unwrap().starts_with("selected:"));
    }
}
