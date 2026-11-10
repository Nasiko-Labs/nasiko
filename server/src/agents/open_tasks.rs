//! A2A tasks an agent has promised more work on.
//!
//! The platform sees the task-shaped replies a framework agent returns on every path it forwards
//! them (the agent proxy, direct `agent_id` dispatch, the HITL resume). A non-terminal one
//! (`working`, `input-required`, ...) is recorded here; a terminal one clears it. The idle reaper
//! does not park an agent with a row younger than two hours, so it is not parked mid-task. Rows
//! older than `max(AGENT_OPEN_TASK_MAX_AGE_SECS, two hours)` are pruned: a task that never reports
//! back must not pin an agent awake forever.

use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// One task state seen in an agent's reply.
#[derive(Debug, PartialEq, Eq)]
pub struct Observed {
    pub task_id: String,
    pub state: String,
    pub terminal: bool,
}

const TERMINAL_STATES: [&str; 5] = ["completed", "canceled", "cancelled", "failed", "rejected"];

/// The task a JSON-RPC reply (or one streamed event) reports, if it reports one. Every A2A
/// dialect is read by [`nasiko_types::a2a::task_status_of`]; this adds only which states end a
/// task.
pub fn observe(rpc: &Value) -> Option<Observed> {
    let (task_id, state) = nasiko_types::a2a::task_status_of(rpc)?;
    Some(Observed {
        terminal: TERMINAL_STATES.contains(&state.as_str()),
        task_id,
        state,
    })
}

/// Note what an agent's reply says about its task: an open one is recorded, a finished one
/// clears its row, a reply with no task state does nothing. Awaited inline so the writes land in
/// response order; a database failure is logged, never surfaced, since it costs only the park
/// protection of that one task.
///
/// Called where the platform forwards an agent's reply: the agent proxy, direct `agent_id`
/// dispatch and the HITL resume. MAF legs stay out (MAF tracks its own step state), and so do the
/// chat orchestrator's ReAct legs (`A2aClient`) for now.
pub async fn note_response(db: &PgPool, agent_id: Uuid, rpc: &Value) {
    if let Some(observed) = observe(rpc) {
        note_observed(db, agent_id, &observed).await;
    }
}

/// [`note_response`] for a state already classified; a database failure is logged only.
pub async fn note_observed(db: &PgPool, agent_id: Uuid, observed: &Observed) {
    if let Err(e) = record(db, agent_id, observed).await {
        tracing::warn!(error = %e, %agent_id, "open tasks: recording a task state failed");
    }
}

/// Forget one task of an agent, for a task the agent no longer knows (a lost task).
pub async fn clear(db: &PgPool, agent_id: Uuid, task_id: &str) {
    let done = sqlx::query("DELETE FROM agent_open_tasks WHERE agent_id = $1 AND task_id = $2")
        .bind(agent_id)
        .bind(task_id)
        .execute(db)
        .await;
    if let Err(e) = done {
        tracing::warn!(error = %e, %agent_id, "open tasks: clearing a lost task failed");
    }
}

/// Record what an agent's reply said about a task.
pub async fn record(db: &PgPool, agent_id: Uuid, observed: &Observed) -> sqlx::Result<()> {
    if observed.terminal {
        sqlx::query("DELETE FROM agent_open_tasks WHERE agent_id = $1 AND task_id = $2")
            .bind(agent_id)
            .bind(&observed.task_id)
            .execute(db)
            .await?;
    } else {
        sqlx::query(
            "INSERT INTO agent_open_tasks (agent_id, task_id, state) VALUES ($1, $2, $3) \
             ON CONFLICT (agent_id, task_id) DO UPDATE SET state = EXCLUDED.state, updated_at = now()",
        )
        .bind(agent_id)
        .bind(&observed.task_id)
        .bind(&observed.state)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// Delete rows older than `max_age`; returns how many.
pub async fn prune(db: &PgPool, max_age: Duration) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "DELETE FROM agent_open_tasks WHERE updated_at <= now() - make_interval(secs => $1)",
    )
    .bind(max_age.as_secs_f64())
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_task_results() {
        let working =
            json!({"jsonrpc":"2.0","id":1,"result":{"id":"t1","status":{"state":"working"}}});
        assert_eq!(
            observe(&working),
            Some(Observed {
                task_id: "t1".into(),
                state: "working".into(),
                terminal: false
            })
        );
        let done =
            json!({"jsonrpc":"2.0","id":1,"result":{"id":"t1","status":{"state":"completed"}}});
        assert!(observe(&done).unwrap().terminal);
        let message = json!({"jsonrpc":"2.0","id":1,"result":{"kind":"message","parts":[]}});
        assert_eq!(observe(&message), None);
        let wrapped = json!({"jsonrpc":"2.0","id":1,"result":{"task":{"id":"t2","status":{"state":"input-required"}}}});
        assert_eq!(observe(&wrapped).unwrap().task_id, "t2");
        let status_update = json!({"jsonrpc":"2.0","id":1,"result":{"kind":"status-update","taskId":"t3","status":{"state":"canceled"},"final":true}});
        assert_eq!(
            observe(&status_update),
            Some(Observed {
                task_id: "t3".into(),
                state: "canceled".into(),
                terminal: true
            })
        );
    }

    #[test]
    fn a_1_0_status_update_and_task_state_prefix_are_read() {
        let done = json!({"result":{"statusUpdate":{"taskId":"t","status":{"state":"TASK_STATE_COMPLETED"},"final":true}}});
        assert_eq!(
            observe(&done),
            Some(Observed {
                task_id: "t".into(),
                state: "completed".into(),
                terminal: true
            })
        );
        let working = json!({"result":{"statusUpdate":{"taskId":"t","status":{"state":"TASK_STATE_WORKING"}}}});
        assert!(!observe(&working).unwrap().terminal);
        let wrapped = json!({"result":{"task":{"id":"t","status":{"state":"TASK_STATE_FAILED"}}}});
        assert!(observe(&wrapped).unwrap().terminal);
    }

    #[test]
    fn snake_case_states_normalize_and_stay_open() {
        let v = json!({"result":{"id":"t","status":{"state":"input_required"}}});
        let o = observe(&v).unwrap();
        assert_eq!(o.state, "input-required");
        assert!(!o.terminal);
        let v = json!({"result":{"id":"t","status":{"state":"auth_required"}}});
        assert!(!observe(&v).unwrap().terminal);
    }

    #[test]
    fn every_terminal_state_is_terminal_and_the_rest_are_not() {
        for s in ["completed", "canceled", "cancelled", "failed", "rejected"] {
            let v = json!({"result":{"id":"t","status":{"state":s}}});
            assert!(observe(&v).unwrap().terminal, "{s}");
        }
        for s in [
            "submitted",
            "working",
            "input-required",
            "auth-required",
            "unknown",
        ] {
            let v = json!({"result":{"id":"t","status":{"state":s}}});
            assert!(!observe(&v).unwrap().terminal, "{s}");
        }
    }

    #[test]
    fn an_artifact_chunk_names_no_task_state() {
        let v = json!({"result":{"kind":"artifact-update","taskId":"t","artifact":{"parts":[]}}});
        assert_eq!(observe(&v), None);
    }
}
