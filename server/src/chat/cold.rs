//! Chat history for sessions whose older messages were tiered to Lance
//! (`chat_sessions.cold_since` set). Hot sessions keep the SQL keyset path in
//! `routes.rs`; cold sessions read one bounded page from each tier through
//! [`nasiko_event_store::EventStore::chat_page`], mapped to API rows here.

use nasiko_event_store::chat_messages::ChatMessageRow;
use sqlx::PgPool;

use super::models::ChatMessage;

/// Whether any of this session's messages live in Lance.
pub(crate) async fn is_cold(db: &PgPool, session_id: &str) -> Result<bool, sqlx::Error> {
    let cold: Option<bool> = sqlx::query_scalar(
        "SELECT cold_since IS NOT NULL FROM chat_sessions WHERE session_id = $1",
    )
    .bind(session_id)
    .fetch_optional(db)
    .await?;
    Ok(cold.unwrap_or(false))
}

/// Event-store row → API row. Text-encoded JSON and decimals that fail to
/// parse become `None` rather than failing the whole page.
pub(crate) fn to_message(r: ChatMessageRow) -> ChatMessage {
    let json = |s: Option<String>| -> Option<sqlx::types::Json<serde_json::Value>> {
        s.and_then(|t| serde_json::from_str(&t).ok())
            .map(sqlx::types::Json)
    };
    ChatMessage {
        id: r.id,
        session_id: r.session_id,
        external_turn_id: r.external_turn_id,
        role: r.role,
        content: r.content,
        file_parts: json(r.file_parts),
        has_file_parts: r.has_file_parts,
        timestamp: r.timestamp,
        input_tokens: r.input_tokens,
        output_tokens: r.output_tokens,
        cache_read_tokens: r.cache_read_tokens,
        cache_creation_tokens: r.cache_creation_tokens,
        model: r.model,
        duration_ms: r.duration_ms,
        cost_usd: r.cost_usd.and_then(|c| c.parse().ok()),
        usage_estimated: r.usage_estimated,
        trace_id: r.trace_id,
        metadata: json(r.metadata),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;

    fn msg(secs: i64, role: &str) -> ChatMessage {
        to_message(ChatMessageRow {
            id: Uuid::from_u128(secs as u128),
            session_id: "s".into(),
            user_id: Uuid::nil(),
            role: role.into(),
            content: format!("m{secs}"),
            file_parts: None,
            has_file_parts: false,
            timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
            input_tokens: None,
            output_tokens: None,
            model: None,
            duration_ms: None,
            cost_usd: Some("0.25".into()),
            usage_estimated: None,
            trace_id: None,
            metadata: Some(r#"{"k":1}"#.into()),
            cache_read_tokens: None,
            cache_creation_tokens: None,
            external_turn_id: None,
        })
    }

    #[test]
    fn text_columns_decode() {
        let m = msg(1, "user");
        assert_eq!(m.cost_usd.map(|d| d.to_string()), Some("0.25".into()));
        assert_eq!(
            m.metadata.map(|j| j.0["k"].clone()),
            Some(serde_json::json!(1))
        );
    }
}
