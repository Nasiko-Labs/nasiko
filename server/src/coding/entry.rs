//! Shared entry point for running a coding-agent turn and returning it as the
//! standard A2A `message/stream` SSE.
//!
//! Both front doors use this: the agent proxy (`/api/agents/{id}`) and the
//! orchestrator dispatch (`/api/orchestrator/a2a`, the path the web UI uses).
//! Keeping the token mint + env wiring + turn run in one place is what makes a
//! coding agent behave identically however it is reached (NAS-C1). Each caller is
//! still responsible for its own pre-turn bookkeeping (session row, flow row,
//! user-message persistence) and passes in the `flow_id` the session token binds
//! to.

use std::collections::HashMap;

use axum::response::Response;
use uuid::Uuid;

use super::proxy::CodingAgentTarget;
use super::turn::{CodingTurnError, CodingTurnRequest};
use crate::state::AppState;

/// Mint the per-turn session token, wire the agent's model/MCP env, run the ACP
/// turn, and return its events as A2A SSE.
///
/// `flow_id` is the flow the caller already opened for this turn — the token is
/// bound to it so the gateway and router authorize the agent's calls off the
/// signed claims. Minting is best-effort: with no JWT secret or no user the turn
/// still runs, the agent's calls just fall back to the traceparent path.
#[allow(clippy::too_many_arguments)]
pub async fn run_coding_turn_sse(
    state: &AppState,
    agent_id: Uuid,
    target: &CodingAgentTarget,
    user_id: Option<Uuid>,
    context_id: &str,
    prompt: &str,
    flow_id: &str,
    task_id: &str,
) -> Result<Response, CodingTurnError> {
    let mut env = HashMap::new();
    let mut session_token = None;
    let mut session_token_id = None;

    if !state.config.agent_jwt_secret.is_empty()
        && let Some(uid) = user_id
    {
        match crate::sessions::identity::mint_session_token(
            &state.config,
            agent_id,
            uid,
            context_id,
            flow_id,
        ) {
            Ok(minted) => {
                // The agent's model calls read their router credential from env;
                // set the common provider keys so whichever the image uses carries
                // the session token.
                env.insert("OPENAI_API_KEY".to_string(), minted.token.clone());
                env.insert("ANTHROPIC_API_KEY".to_string(), minted.token.clone());
                env.insert("NASIKO_SESSION_TOKEN".to_string(), minted.token.clone());
                session_token_id = Some(super::revocation::RevokedTokenId::from_minted(&minted));
                session_token = Some(minted.token);

                // Point the agent's model SDK at the LLM router (authenticated by
                // the token above, billed to the turn's user), and — for OpenCode —
                // force the routed model + HITL permission config. Other kinds
                // select their own model and get nothing extra here.
                let gw = nasiko_llm_router::GatewayConfig::from_env();
                if !gw.llm_gateway_base_url.trim().is_empty() {
                    nasiko_llm_router::inject_llm_base_url(
                        &mut env,
                        &gw.llm_gateway_base_url,
                        nasiko_llm_router::InboundFormat::OpenAi,
                    );
                    nasiko_llm_router::inject_llm_base_url(
                        &mut env,
                        &gw.llm_gateway_base_url,
                        nasiko_llm_router::InboundFormat::Anthropic,
                    );
                    for (k, v) in super::proxy::routed_model_env(&target.image, target.hitl_enabled)
                    {
                        env.insert(k, v);
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, %agent_id, "coding-agent: session token mint failed; proceeding without")
            }
        }
    }

    let request = CodingTurnRequest {
        session_id: context_id.to_string(),
        spec: super::proxy::build_spec(agent_id, target),
        acp_command: target.acp_command.clone(),
        env,
        user_id,
        prompt: prompt.to_string(),
        session_token,
        session_token_id,
        hitl_enabled: target.hitl_enabled,
    };
    let events = super::turn::run_turn(state, request).await?;
    Ok(super::sse::turn_to_sse(
        events,
        task_id.to_string(),
        context_id.to_string(),
    ))
}
