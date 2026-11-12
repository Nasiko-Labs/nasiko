//! The one place the platform's own deploy-time env is assembled.
//!
//! Every path that starts an agent workload (seed, import, upload, update, rollback, restart,
//! admin deploy, startup reconcile, GitHub clone) calls here instead of wiring credentials
//! itself, so a new platform credential reaches all of them at once and no path can quietly miss
//! one. What the platform injects, besides the LLM router wiring (`llm_router::wiring`) and
//! `MCP_GATEWAY_TOKEN` (`mcp::wiring`):
//!
//! | Name | Meaning |
//! | --- | --- |
//! | `NASIKO_STATE_URL` | Base URL of the agent state API (`{server}/api/agent-state`) |
//! | `NASIKO_AGENT_TOKEN` | The agent's state-scoped credential for that API |
//!
//! # State mode
//!
//! A deploy of a different image resets the agent's `state_mode` to `memory` (the new image may
//! not register a task store; one that does re-registers when it starts). A deploy of the same
//! image (restart, startup recovery) keeps it, so a durable agent is durable again as soon as it
//! is back. The caller says which it is with [`StateModeOnDeploy`].
//!
//! # Platform keys win
//!
//! The platform's keys are removed from the caller's env before anything is injected, so a user
//! secret or an uploaded `env` field of the same name cannot stand in for a credential - not even
//! when minting fails and the platform has nothing to put in its place.
//!
//! # Best-effort
//!
//! Like the wiring it composes, a failure here is logged and the deploy proceeds: an agent that
//! cannot reach the state API falls back to in-memory state, which is slower to recover but not
//! undeployable.

use std::collections::HashMap;

use uuid::Uuid;

use crate::state::AppState;

/// Base URL of the agent state API, as the agent container must dial it.
pub const STATE_URL_VAR: &str = "NASIKO_STATE_URL";
/// The agent's state-scoped credential.
pub const STATE_TOKEN_VAR: &str = "NASIKO_AGENT_TOKEN";
const GATEWAY_TOKEN_VAR: &str = "MCP_GATEWAY_TOKEN";

/// Keys only the platform may set; stripped from caller env before injection.
const RESERVED_KEYS: [&str; 3] = [GATEWAY_TOKEN_VAR, STATE_URL_VAR, STATE_TOKEN_VAR];

/// Path of the MCP gateway under the server's base URL, as `MCP_GATEWAY_PUBLIC_URL` names it.
const MCP_GATEWAY_PATH: &str = "/api/mcp";
/// Path the agent state router is mounted at.
const AGENT_STATE_PATH: &str = "/api/agent-state";

/// What a deploy does to the agent's recorded `state_mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateModeOnDeploy {
    /// Back to `memory`: the image may have changed, so a task store is no longer a given.
    Reset,
    /// Leave it: the same image comes back and will find its durable state as before.
    Keep,
}

/// The agent's full deploy env: its secrets and platform defaults (`AppState::agent_env`), then
/// `caller_env` on top (an upload's own `env` field), then the platform's keys on top of both.
pub async fn build(
    state: &AppState,
    agent_id: Uuid,
    owner_id: Uuid,
    caller_env: HashMap<String, String>,
    state_mode: StateModeOnDeploy,
) -> HashMap<String, String> {
    let mut env = state.agent_env(agent_id).await;
    env.extend(caller_env);
    apply_platform_env(state, &mut env, agent_id, owner_id, state_mode).await;
    env
}

/// Removes every key only the platform may set, so a value the caller put under one of those
/// names never reaches the agent.
fn strip_reserved_keys(env: &mut HashMap<String, String>) {
    for key in RESERVED_KEYS {
        env.remove(key);
    }
}

/// Sets all of the platform's keys on `env`: the credentials (`apply_credentials`) and the LLM
/// router wiring (`apply_llm_wiring`).
///
/// `owner_id` is the agent's real owner (the LLM router JWT's `owner_id` claim), never the user
/// who happened to trigger this deploy.
pub async fn apply_platform_env(
    state: &AppState,
    env: &mut HashMap<String, String>,
    agent_id: Uuid,
    owner_id: Uuid,
    state_mode: StateModeOnDeploy,
) {
    apply_credentials(state, env, agent_id, state_mode).await;
    apply_llm_wiring(env, agent_id, owner_id);
}

/// Sets the platform's credential keys on `env`, replacing anything the caller put under those
/// names: the gateway token and the state API address and token. Rotates the agent's gateway
/// and state credentials; see `agent_tokens::ROTATION_GRACE_SECS` for how long the previous ones
/// keep working.
///
/// Alone (without the LLM wiring) for an agent whose model access is configured directly, such
/// as the internal seed, which must not gain router wiring it never had.
pub async fn apply_credentials(
    state: &AppState,
    env: &mut HashMap<String, String>,
    agent_id: Uuid,
    state_mode: StateModeOnDeploy,
) {
    strip_reserved_keys(env);
    crate::mcp::wiring::inject_agent_gateway_token(&state.db, env, agent_id).await;
    inject_agent_state(state, &state.db, env, agent_id, state_mode).await;
}

/// Points the agent's LLM SDK at the LLM router, when one is configured.
fn apply_llm_wiring(env: &mut HashMap<String, String>, agent_id: Uuid, owner_id: Uuid) {
    crate::llm_router::wiring::inject_agent_llm_env(env, agent_id, Some(owner_id));
}

/// Gives the agent its state API address and credential, and, for a deploy that
/// [`StateModeOnDeploy::Reset`]s it, marks the agent `memory` until it registers a task store
/// again.
///
/// The mode is reset here, with the credential, because this runs for every deploy. Resetting
/// early errs toward `memory`, the conservative reading, if the deploy then fails.
async fn inject_agent_state(
    state: &AppState,
    mint_db: impl sqlx::PgExecutor<'_>,
    env: &mut HashMap<String, String>,
    agent_id: Uuid,
    state_mode: StateModeOnDeploy,
) {
    let configured = state.config.mcp_gateway_public_url.as_deref();
    let Some(url) = configured.and_then(agent_state_url) else {
        tracing::warn!(
            %agent_id,
            "MCP_GATEWAY_PUBLIC_URL is unset or does not end in {MCP_GATEWAY_PATH}: the agent \
             gets no {STATE_URL_VAR} and keeps its state in memory"
        );
        return;
    };
    if state_mode == StateModeOnDeploy::Reset
        && let Err(e) = crate::agent_state::reset_state_mode(&state.db, agent_id).await
    {
        tracing::error!(%agent_id, error = %e, "failed to reset agent state mode on deploy");
    }
    match crate::agent_state::mint_state_token(mint_db, agent_id).await {
        Ok(token) => {
            env.insert(STATE_URL_VAR.into(), url);
            env.insert(STATE_TOKEN_VAR.into(), token);
        }
        Err(e) => {
            tracing::error!(%agent_id, error = %e, "failed to mint agent state token; the agent's state API calls will be rejected");
        }
    }
}

/// `MCP_GATEWAY_PUBLIC_URL` is the server's externally reachable `/api/mcp` URL, so the
/// server's base is what is left once that path is taken off. `None` when it does not end in
/// that path: guessing a base would hand agents an address that does not exist.
fn agent_state_url(gateway_public_url: &str) -> Option<String> {
    let base = gateway_public_url
        .trim_end_matches('/')
        .strip_suffix(MCP_GATEWAY_PATH)?;
    Some(format!("{base}{AGENT_STATE_PATH}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_url_replaces_the_gateway_path_on_the_same_base() {
        assert_eq!(
            agent_state_url("http://nasiko-server.ns.svc.cluster.local:8080/api/mcp").as_deref(),
            Some("http://nasiko-server.ns.svc.cluster.local:8080/api/agent-state")
        );
        assert_eq!(
            agent_state_url("https://cp.example.com/api/mcp/").as_deref(),
            Some("https://cp.example.com/api/agent-state")
        );
    }

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// What a caller (an agent secret or an upload's `env`) might put under the platform's names.
    fn hostile_caller_env() -> HashMap<String, String> {
        env_of(&[
            (GATEWAY_TOKEN_VAR, "ngt_forged"),
            (STATE_URL_VAR, "http://attacker.test/api/agent-state"),
            (STATE_TOKEN_VAR, "nst_forged"),
            ("OPENAI_MODEL", "gpt-4.1"),
            ("MY_SECRET", "kept"),
        ])
    }

    #[test]
    fn a_reserved_key_never_keeps_the_callers_value() {
        let mut env = hostile_caller_env();
        strip_reserved_keys(&mut env);
        for key in RESERVED_KEYS {
            assert!(
                !env.contains_key(key),
                "{key} must not come from the caller"
            );
        }
    }

    #[test]
    fn keys_the_platform_does_not_reserve_stay_the_callers() {
        let mut env = hostile_caller_env();
        strip_reserved_keys(&mut env);
        assert_eq!(env["OPENAI_MODEL"], "gpt-4.1");
        assert_eq!(env["MY_SECRET"], "kept");
    }

    #[test]
    fn a_gateway_url_that_is_not_the_mcp_path_gives_no_state_url() {
        assert_eq!(agent_state_url("http://host.docker.internal:8080"), None);
        assert_eq!(agent_state_url("https://cp.example.com/mcp"), None);
        assert_eq!(agent_state_url(""), None);
    }
}
