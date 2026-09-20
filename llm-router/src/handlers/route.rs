//! `POST /v1/route` — DronaHQ contract (v1.0): tier hint → provider cascade → unified reply.
//!
//! Default path uses platform provider keys and the contract tier cascade (agnostic).
//! When the caller presents an agent JWT and `agents.tier_cascade = false`, the handler
//! instead resolves that agent's `llm_config` / BYOK provider (console-selected) and
//! calls only that destination — so NVIDIA BYOK sticks.

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
use crate::ir::{ChatRequest, Message};
use crate::providers::{self, ProviderError};
use crate::resolver::{PgRegistry, RequestHint, ResolvedConfig, resolve};
use crate::routing::tier::{
    ContractTier, TierCandidate, cascade_for, cheapest_candidate, estimate_prompt_tokens,
    projected_completion_tokens, resolve_tier,
};

const CONTRACT_VERSION: &str = "1.0";

const TASK_TYPES: &[&str] = &[
    "summarize",
    "translate",
    "reason",
    "code",
    "chat",
    "extract",
];

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

#[derive(Debug, Serialize)]
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

    // Optional shared secret for unauthenticated billable cascade (set NASIKO_ROUTE_TOKEN).
    // Agent JWT still unlocks the tier_cascade=false (BYOK) path below.
    if let Err(resp) = require_route_credential(&headers, &ctx.cfg) {
        return resp;
    }

    // Stick-to-selected-provider: agent JWT + agents.tier_cascade = false.
    if let Some((agent_id, owner_id)) = optional_agent(&headers, &ctx.cfg) {
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
                tracing::warn!(
                    target: "nasiko::llm_router::route",
                    %agent_id,
                    error = %e,
                    "route: tier_cascade lookup failed — using platform cascade"
                );
            }
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
                let _ = parsed.allow_cache;
                return ok_response(RouteOk {
                    provider: candidate.provider.to_string(),
                    model: candidate.model.to_string(),
                    content,
                    usage: RouteUsage {
                        prompt_tokens: p,
                        completion_tokens: c,
                        total_tokens: total,
                        cost_usd: cost,
                    },
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

fn require_route_credential(headers: &HeaderMap, cfg: &GatewayConfig) -> Result<(), Response> {
    let expected = cfg.route_token.trim();
    if expected.is_empty() {
        return Ok(());
    }
    // Agent JWT satisfies auth (selected-provider path / trusted agent).
    if optional_agent(headers, cfg).is_some() {
        return Ok(());
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
    let hint = RequestHint {
        provider: None,
        model: None,
    };
    let mut resolved = match resolve(&store, &ctx.cache, &ctx.cfg, agent_id, owner_id, hint).await {
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
                latency_ms: started.elapsed().as_millis() as u64,
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
    // Ollama is local — empty platform key is fine (provider sends a dummy bearer).
    if provider == "ollama" {
        return true;
    }
    !cfg.platform_key_for(provider).is_empty()
}

fn platform_resolved(
    candidate: &TierCandidate,
    cfg: &GatewayConfig,
    max_tokens: Option<i64>,
) -> ResolvedConfig {
    let provider = candidate.provider.to_string();
    let model = candidate.model.to_string();
    ResolvedConfig {
        litellm_model: format!("{provider}/{model}"),
        provider,
        model,
        api_key: cfg.platform_key_for(candidate.provider).to_string(),
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

    #[test]
    fn rejects_empty_task_type_shape() {
        let raw = json!({"messages":[{"role":"user","content":"hi"}]});
        let parsed: RouteBody = serde_json::from_value(raw).unwrap();
        assert!(parsed.task_type.is_none());
    }
}
