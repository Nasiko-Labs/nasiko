pub(crate) mod external_turn;
pub(crate) mod models;
mod routes;

pub use routes::router;

/// Fire-and-forget insert of a `chat_messages` user turn, deduped against the same
/// `(session_id, content)` pair written in the last two seconds. A retry can legitimately reach
/// the caller a second time for the same logical turn — `nasiko chat`'s protocol-negotiation
/// resend in `agent_proxy.rs`, or a client retrying a HITL resolve POST in `router/hitl.rs` — and
/// without this dedup window that showed up as a duplicate user bubble in chat history. Shared by
/// both call sites: same query, same window, previously duplicated in each.
pub(crate) fn spawn_dedup_user_message_insert(
    db: sqlx::PgPool,
    session_id: String,
    content: String,
) {
    tokio::spawn(async move {
        if let Err(e) = sqlx::query(
            "INSERT INTO chat_messages (session_id, role, content) \
             SELECT $1, 'user', $2 \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM chat_messages \
                 WHERE session_id = $1 AND role = 'user' AND content = $2 \
                   AND timestamp > now() - interval '2 seconds' \
             )",
        )
        .bind(&session_id)
        .bind(&content)
        .execute(&db)
        .await
        {
            // Not critical (the answer/message still resolved/sent successfully), but a failed
            // insert here previously vanished silently — this is the difference between "why
            // didn't the answer show up in history" being diagnosable or not.
            tracing::warn!(
                error = %e, %session_id,
                "spawn_dedup_user_message_insert: chat_messages insert failed"
            );
        }
    });
}
