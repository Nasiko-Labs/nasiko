//! Deploy-time LLM gateway wiring (Phase 2, P2.1).
//!
//! Mints an agent-identity JWT and injects the gateway base-URL + key into an agent
//! container's env vars so the agent's LLM SDK transparently routes through the LLM
//! router (`nasiko-llm-router`). Thin wrapper over
//! [`nasiko_llm_router::inject_llm_env`] applying the server's **warn + deploy** policy:
//! if the gateway isn't configured (`AGENT_JWT_SECRET` / `LLM_GATEWAY_BASE_URL` unset)
//! the injection is skipped and the agent still deploys — wiring is never deploy-blocking.

use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

/// Inject LLM gateway wiring into `env_vars` for `agent_id`, choosing the env-var set by
/// the agent's `inbound_format` column (which SDK its code speaks). `owner_id` becomes the
/// JWT `owner_id` claim for per-user secret resolution (`None`/nil ⇒ platform-key path).
///
/// Best-effort: a configuration failure is logged and the deploy proceeds. Deploy is
/// authoritative — when the gateway is configured, the injected `*_BASE_URL`/`*_API_KEY`
/// overwrite any pre-existing values.
pub async fn inject_agent_llm_env(
    db: &PgPool,
    env_vars: &mut HashMap<String, String>,
    agent_id: Uuid,
    owner_id: Option<Uuid>,
) {
    let inbound_format = fetch_inbound_format(db, agent_id).await;
    let cfg = nasiko_llm_router::GatewayConfig::from_env();
    let ctx = nasiko_llm_router::LlmInjectCtx {
        agent_id: agent_id.to_string(),
        owner_id: owner_id.map(|u| u.to_string()).unwrap_or_default(),
        inbound_format,
    };
    match nasiko_llm_router::inject_llm_env(env_vars, &ctx, &cfg) {
        Ok(()) => tracing::info!(%agent_id, ?inbound_format, "injected LLM gateway wiring"),
        Err(e) => {
            tracing::warn!(%agent_id, error = %e, "skipped LLM gateway wiring (not configured)")
        }
    }
}

/// Read `agents.inbound_format`; missing row / unknown value / query error → OpenAI
/// (the backward-compatible default).
async fn fetch_inbound_format(db: &PgPool, agent_id: Uuid) -> nasiko_llm_router::InboundFormat {
    let raw: Option<String> = sqlx::query_scalar("SELECT inbound_format FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
    raw.map(|s| nasiko_llm_router::InboundFormat::from_label(&s))
        .unwrap_or_default()
}
