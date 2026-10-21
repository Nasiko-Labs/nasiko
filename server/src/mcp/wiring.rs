//! Deploy-time MCP gateway credential wiring.
//!
//! Mints the per-agent gateway credential and injects it as `MCP_GATEWAY_TOKEN`
//! so the agent can configure its MCP client once at startup
//! (`Authorization: Bearer $MCP_GATEWAY_TOKEN` against `MCP_GATEWAY_URL`, which
//! `McpInjector` injects at the runtime layer). Called at every deploy/restart
//! path, right next to `llm_router::wiring::inject_agent_llm_env` — redeploying
//! rotates the credential (the old plaintext only lived in the env being
//! replaced).
//!
//! Best-effort like the LLM wiring: a mint failure is logged and the deploy
//! proceeds — the agent then simply gets 401s from `/api/mcp` instead of being
//! undeployable. Minting is unconditional (not gated on
//! `MCP_GATEWAY_PUBLIC_URL`) so flipping the gateway on later only requires a
//! restart, not a config-ordering dance.

use std::collections::HashMap;

use uuid::Uuid;

/// Mint (rotate) the agent's gateway credential and set `MCP_GATEWAY_TOKEN`.
///
/// Generic over the executor so a caller still inside the transaction that
/// created `agent_id` (e.g. `agents::upload::upload_and_deploy`) can pass
/// `&mut *tx` — a separate pool connection can't see that row until commit,
/// which otherwise fails the mint's `agent_gateway_tokens_agent_id_fkey`.
pub async fn inject_agent_gateway_token(
    db: impl sqlx::PgExecutor<'_>,
    env_vars: &mut HashMap<String, String>,
    agent_id: Uuid,
) {
    match nasiko_mcp_gateway::agent_tokens::mint(db, agent_id).await {
        Ok(token) => {
            env_vars.insert("MCP_GATEWAY_TOKEN".into(), token);
        }
        Err(e) => {
            tracing::error!(%agent_id, error = %e, "failed to mint MCP gateway token; agent's /api/mcp calls will be rejected");
        }
    }
}
