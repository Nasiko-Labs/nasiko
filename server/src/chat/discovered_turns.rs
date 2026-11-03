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
//!
//! Each turn is also tied to the trace of its call (the router's `gen_ai.chat` span), in
//! `session_traces` and on the reply's row, as the agent proxy ties a proxied call: that is how
//! Sessions finds a session's traces when no span carries a `session.id`.

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
        let trace_id = trace_id_of(&turn.span);
        tokio::spawn(async move {
            if let Err(error) = persist(&db, &turn, trace_id.as_deref()).await {
                tracing::warn!(%error, agent_id = %turn.agent_id, "discovered-agent turn not recorded");
            }
        });
    }
}

/// The id of the trace `span` belongs to, when it is recorded at all (`None` without an
/// OpenTelemetry layer, as in tests).
fn trace_id_of(span: &tracing::Span) -> Option<String> {
    use opentelemetry::trace::TraceContextExt as _;
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;
    let context = span.context();
    let span_context = context.span().span_context().clone();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
}

/// The session a discovered agent's turns go to: one per agent, stable across restarts.
pub fn session_id_for(agent_id: Uuid) -> String {
    const NAMESPACE: Uuid = Uuid::from_u128(0x3f0c_51e2_8d7a_4b6e_9c1d_0a2b_7e64_f915);
    Uuid::new_v5(&NAMESPACE, agent_id.as_bytes()).to_string()
}

async fn persist(db: &PgPool, turn: &RecordedTurn, trace_id: Option<&str>) -> anyhow::Result<()> {
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
            trace_id: trace_id.map(str::to_string),
        }),
        assistant_metadata: None,
    };
    persist_external_turn(&mut tx, &session_id, &external, turn.started_at, true).await?;
    if let Some(trace_id) = trace_id {
        sqlx::query(
            "INSERT INTO session_traces (session_id, trace_id, agent_id, agent_name) \
             SELECT $1, $2, a.id, a.name FROM agents a WHERE a.id = $3 \
             ON CONFLICT (session_id, trace_id) DO NOTHING",
        )
        .bind(&session_id)
        .bind(trace_id)
        .bind(agent_id)
        .execute(&mut *tx)
        .await?;
    }
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
    fn a_turn_is_tied_to_the_trace_of_its_request() {
        use opentelemetry::trace::TracerProvider as _;
        use tracing_subscriber::layer::SubscriberExt as _;

        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry().with(
            tracing_opentelemetry::OpenTelemetryLayer::new(provider.tracer("test")),
        );
        tracing::subscriber::with_default(subscriber, || {
            let request = tracing::info_span!("request");
            let trace_id = trace_id_of(&request).expect("a recorded span has a trace");
            assert_eq!(trace_id.len(), 32);
            assert!(trace_id.chars().all(|c| c.is_ascii_hexdigit()));
            // The call's own span (here a child) is in the same trace.
            let call = tracing::info_span!(parent: &request, "gen_ai.chat");
            assert_eq!(trace_id_of(&call).as_deref(), Some(trace_id.as_str()));
        });
        // Without an OpenTelemetry layer there is no trace to tie it to.
        assert_eq!(trace_id_of(&tracing::info_span!("untraced")), None);
    }

    #[test]
    fn each_agent_has_one_stable_session() {
        let agent = Uuid::new_v4();
        assert_eq!(session_id_for(agent), session_id_for(agent));
        assert_ne!(session_id_for(agent), session_id_for(Uuid::new_v4()));
    }
}
