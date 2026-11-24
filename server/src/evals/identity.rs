//! The caller's `eval` coding-agent identity: every model call an eval makes (rewriting,
//! screening, the replayed agent) goes through the router as this agent, with a token scoped to
//! one attempt and one of the caller's LLM configs (`nasiko_llm_router::auth::EvalScope`).

use anyhow::{Context, Result};
use nasiko_llm_router::auth::{EvalScope, mint_eval_token, parse_algorithm};
use nasiko_llm_router::config::GatewayConfig;
use sqlx::PgPool;
use uuid::Uuid;

use crate::catalog::routes::{coding_agent_name, coding_agent_spec};

const EVAL_INTEGRATION: &str = "eval";

/// The caller's live `eval` identity, created on first use (one per user, by the unique index
/// on `(owner_id, coding_agent_integration_id)`).
pub async fn ensure(db: &PgPool, owner_id: Uuid) -> Result<Uuid> {
    if let Some(id) = existing(db, owner_id).await? {
        return Ok(id);
    }
    let (base_name, display_name) =
        coding_agent_spec(EVAL_INTEGRATION).context("the eval integration is not registered")?;
    let (username, email): (String, String) =
        sqlx::query_as("SELECT username, email FROM users WHERE id = $1")
            .bind(owner_id)
            .fetch_one(db)
            .await
            .context("eval identity owner lookup")?;
    let name = coding_agent_name(&username, base_name)
        .context("the account's username cannot form an agent name")?;
    sqlx::query(
        r#"INSERT INTO agents
               (name, display_name, description, owner_id, tags, metadata,
                coding_agent_integration_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT DO NOTHING"#,
    )
    .bind(&name)
    .bind(format!("{display_name} ({email})"))
    .bind("Replay evals: rewriting, screening and replaying coding tasks")
    .bind(owner_id)
    .bind(vec!["local", "coding-agent"])
    .bind(serde_json::json!({"source": "nasiko-evals", "integration_id": EVAL_INTEGRATION}))
    .bind(EVAL_INTEGRATION)
    .execute(db)
    .await
    .context("eval identity insert")?;
    existing(db, owner_id)
        .await?
        .context("the eval identity's name is taken by another agent")
}

async fn existing(db: &PgPool, owner_id: Uuid) -> Result<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM agents WHERE owner_id = $1 AND coding_agent_integration_id = $2 \
         AND deleted_at IS NULL",
    )
    .bind(owner_id)
    .bind(EVAL_INTEGRATION)
    .fetch_optional(db)
    .await?)
}

/// A router token for `agent_id` (the owner's eval identity), filed under `attempt` and running
/// on `config_id`, valid for `ttl_secs`.
pub fn attempt_token(
    agent_id: Uuid,
    owner_id: Uuid,
    attempt: &str,
    config_id: Uuid,
    ttl_secs: u64,
) -> Result<String> {
    let cfg = GatewayConfig::from_env();
    anyhow::ensure!(
        !cfg.agent_jwt_secret.is_empty(),
        "AGENT_JWT_SECRET is not configured"
    );
    Ok(mint_eval_token(
        &agent_id.to_string(),
        &owner_id.to_string(),
        &cfg.agent_jwt_secret,
        ttl_secs,
        parse_algorithm(&cfg.agent_jwt_algorithm),
        &EvalScope {
            attempt: attempt.to_string(),
            llm_config_id: Some(config_id),
        },
    )?)
}
