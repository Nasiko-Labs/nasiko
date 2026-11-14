//! Deploy-time LLM gateway wiring (Phase 2, P2.1).
//!
//! Mints an agent-identity JWT and injects the gateway base-URL + key into an agent
//! container's env vars so the agent's LLM SDK transparently routes through the LLM
//! router (`nasiko-llm-router`). Thin wrapper over
//! [`nasiko_llm_router::inject_llm_env`] applying the server's **warn + deploy** policy:
//! if the gateway isn't configured (`AGENT_JWT_SECRET` / `LLM_GATEWAY_BASE_URL` unset)
//! the injection is skipped and the agent still deploys — wiring is never deploy-blocking.

use std::collections::HashMap;

use uuid::Uuid;

/// Inject LLM gateway wiring into `env_vars` for `agent_id`: the base URL and key of every
/// SDK the router serves (OpenAI, Anthropic, Gemini). `owner_id` becomes the JWT `owner_id`
/// claim for per-user secret resolution (`None`/nil ⇒ platform-key path).
///
/// Best-effort: a configuration failure is logged and the deploy proceeds. Deploy is
/// authoritative — when the gateway is configured, the injected `*_BASE_URL`/`*_API_KEY`
/// overwrite any pre-existing values.
pub fn inject_agent_llm_env(
    env_vars: &mut HashMap<String, String>,
    agent_id: Uuid,
    owner_id: Option<Uuid>,
) {
    let cfg = nasiko_llm_router::GatewayConfig::from_env();
    let ctx = nasiko_llm_router::LlmInjectCtx {
        agent_id: agent_id.to_string(),
        owner_id: owner_id.map(|u| u.to_string()).unwrap_or_default(),
    };
    match nasiko_llm_router::inject_llm_env(env_vars, &ctx, &cfg) {
        Ok(()) => tracing::info!(%agent_id, "injected LLM gateway wiring"),
        Err(e) => {
            // `e` names the missing setting (AGENT_JWT_SECRET or LLM_GATEWAY_BASE_URL).
            tracing::warn!(
                %agent_id,
                error = %e,
                "skipped LLM gateway wiring: this agent's model calls won't go through the LLM \
                 router, so it needs its own provider key"
            )
        }
    }
}
