use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};

use super::models::{ChatMessage, ExternalTurn};

pub(crate) struct PersistedExternalTurn {
    pub inserted: bool,
    pub user_message: ChatMessage,
    pub assistant_message: ChatMessage,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PersistExternalTurnError {
    #[error("external turn is incomplete")]
    Incomplete,
    #[error("turn_id already exists with a different payload")]
    Conflict,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub(crate) async fn persist_external_turn(
    tx: &mut Transaction<'_, Postgres>,
    session_id: &str,
    body: &ExternalTurn,
    timestamp: DateTime<Utc>,
    touch_session: bool,
) -> Result<PersistedExternalTurn, PersistExternalTurnError> {
    let turn_id = body.turn_id.trim();
    let assistant_metadata = body
        .assistant_metadata
        .as_ref()
        .map(|object| serde_json::Value::Object(object.clone()));
    let user_inserted = sqlx::query_as::<_, ChatMessage>(
        r#"INSERT INTO chat_messages
               (session_id, external_turn_id, role, content, timestamp)
           VALUES ($1, $2, 'user', $3, $4)
           ON CONFLICT (session_id, external_turn_id, role) DO NOTHING
           RETURNING *"#,
    )
    .bind(session_id)
    .bind(turn_id)
    .bind(&body.user_content)
    .bind(timestamp)
    .fetch_optional(&mut **tx)
    .await?;
    let usage = body.assistant_usage.as_ref();

    if user_inserted.is_none() {
        let mut messages = sqlx::query_as::<_, ChatMessage>(
            r#"SELECT * FROM chat_messages
               WHERE session_id = $1 AND external_turn_id = $2
                 AND role IN ('user', 'assistant')
               ORDER BY CASE role WHEN 'user' THEN 0 ELSE 1 END"#,
        )
        .bind(session_id)
        .bind(turn_id)
        .fetch_all(&mut **tx)
        .await?;
        if messages.len() != 2 {
            return Err(PersistExternalTurnError::Incomplete);
        }
        let assistant_message = messages.pop().expect("message count checked");
        let user_message = messages.pop().expect("message count checked");
        let exact_replay = user_message.content == body.user_content
            && assistant_message.content == body.assistant_content
            && assistant_message.input_tokens == usage.and_then(|u| u.input_tokens)
            && assistant_message.output_tokens == usage.and_then(|u| u.output_tokens)
            && assistant_message.model.as_deref() == usage.and_then(|u| u.model.as_deref())
            && assistant_message.duration_ms == usage.and_then(|u| u.duration_ms)
            && assistant_message.cost_usd == usage.and_then(|u| u.cost_usd)
            && assistant_message.usage_estimated == usage.and_then(|u| u.estimated)
            && assistant_message.trace_id.as_deref() == usage.and_then(|u| u.trace_id.as_deref())
            && assistant_message.metadata.as_deref() == assistant_metadata.as_ref();
        if !exact_replay {
            return Err(PersistExternalTurnError::Conflict);
        }
        return Ok(PersistedExternalTurn {
            inserted: false,
            user_message,
            assistant_message,
        });
    }

    let user_message = user_inserted.expect("user insertion checked");
    let assistant_message = sqlx::query_as::<_, ChatMessage>(
        r#"INSERT INTO chat_messages
               (session_id, external_turn_id, role, content, timestamp,
                input_tokens, output_tokens, model, duration_ms, cost_usd,
                 usage_estimated, trace_id, metadata)
            VALUES ($1, $2, 'assistant', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
           ON CONFLICT (session_id, external_turn_id, role) DO NOTHING
           RETURNING *"#,
    )
    .bind(session_id)
    .bind(turn_id)
    .bind(&body.assistant_content)
    .bind(timestamp + chrono::Duration::microseconds(1))
    .bind(usage.and_then(|u| u.input_tokens))
    .bind(usage.and_then(|u| u.output_tokens))
    .bind(usage.and_then(|u| u.model.as_deref()))
    .bind(usage.and_then(|u| u.duration_ms))
    .bind(usage.and_then(|u| u.cost_usd))
    .bind(usage.and_then(|u| u.estimated))
    .bind(usage.and_then(|u| u.trace_id.as_deref()))
    .bind(assistant_metadata.as_ref())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(PersistExternalTurnError::Incomplete)?;

    if touch_session {
        sqlx::query("UPDATE chat_sessions SET updated_at = now() WHERE session_id = $1")
            .bind(session_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(PersistedExternalTurn {
        inserted: true,
        user_message,
        assistant_message,
    })
}
