//! A discovered A2A agent's routed calls, kept as one chat session per agent.
//!
//! The router is the only record of such an agent's conversation (see
//! `nasiko_llm_router::turns`), so each call it routes becomes a turn of a session the agent
//! owns, and the agent shows up in Sessions the way a coding agent does. One session per agent
//! for now; splitting it (by idle time, or by an id the agent sends) can come later without
//! changing what a turn is.
//!
//! Every write happens on its own task: [`TurnRecorder::record`] is called on the routed
//! request's path and returns at once, so recording never adds latency to the agent's call. A
//! failed write is logged and dropped, like usage logging.

use nasiko_llm_router::turns::{RecordedTurn, TurnRecorder};
use sqlx::PgPool;
use uuid::Uuid;

use super::external_turn::persist_external_turn;
use super::models::{ExternalTurn, MessageUsage};

/// Keeps each discovered agent's routed calls as turns of its one chat session.
pub struct DiscoveredAgentSessions {
    db: PgPool,
}

impl DiscoveredAgentSessions {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }
}

impl TurnRecorder for DiscoveredAgentSessions {
    fn record(&self, turn: RecordedTurn) {
        let db = self.db.clone();
        tokio::spawn(async move {
            if let Err(error) = persist(&db, &turn).await {
                tracing::warn!(%error, agent_id = %turn.agent_id, "discovered-agent turn not recorded");
            }
        });
    }
}

/// The session a discovered agent's turns go to: one per agent, stable across restarts.
pub fn session_id_for(agent_id: Uuid) -> String {
    const NAMESPACE: Uuid = Uuid::from_u128(0x3f0c_51e2_8d7a_4b6e_9c1d_0a2b_7e64_f915);
    Uuid::new_v5(&NAMESPACE, agent_id.as_bytes()).to_string()
}

async fn persist(db: &PgPool, turn: &RecordedTurn) -> anyhow::Result<()> {
    let agent_id: Uuid = turn.agent_id.parse()?;
    let owner_id: Uuid = turn.owner_id.parse()?;
    let session_id = session_id_for(agent_id);
    let mut tx = db.begin().await?;
    // Titled after the agent, owned by the agent's owner — who the router billed for the call.
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title, created_at, updated_at) \
         SELECT $1, $2, a.id, COALESCE(NULLIF(a.display_name, ''), a.name), $4, $4 \
         FROM agents a WHERE a.id = $3 AND a.discovered_on IS NOT NULL \
         ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(&session_id)
    .bind(owner_id)
    .bind(agent_id)
    .bind(turn.started_at)
    .execute(&mut *tx)
    .await?;
    let external = ExternalTurn {
        turn_id: Uuid::new_v4().to_string(),
        user_content: turn.user_text.clone(),
        assistant_content: turn.assistant_text.clone(),
        assistant_usage: Some(MessageUsage {
            input_tokens: turn.input_tokens.map(saturating_i32),
            output_tokens: turn.output_tokens.map(saturating_i32),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            model: Some(turn.model.clone()),
            duration_ms: Some(saturating_i32(turn.latency_ms)),
            cost_usd: None,
            estimated: None,
            trace_id: None,
        }),
        assistant_metadata: None,
    };
    persist_external_turn(&mut tx, &session_id, &external, turn.started_at, true).await?;
    tx.commit().await?;
    Ok(())
}

fn saturating_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value < 0 { i32::MIN } else { i32::MAX })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_agent_has_one_stable_session() {
        let agent = Uuid::new_v4();
        assert_eq!(session_id_for(agent), session_id_for(agent));
        assert_ne!(session_id_for(agent), session_id_for(Uuid::new_v4()));
    }
}
