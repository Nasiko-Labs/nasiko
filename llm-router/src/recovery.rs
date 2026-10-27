//! Persistence for the compression recovery store (PRD §9 IP-5).
//!
//! [`crate::compress`] mints the handles and hands back the originals; this module is the I/O
//! half. Kept out of `nasiko-compress` deliberately — that crate forbids itself RNG and I/O so
//! it stays a pure, testable function of its input.
//!
//! # Why this write is awaited rather than spawned
//!
//! The elision marker naming the handle goes out with the request. If the insert were spawned,
//! a fast model could call `recover_compressed` before the row landed and get a "not found" for
//! content that does exist — the one failure mode that would teach an agent to stop trusting
//! recovery, which is the whole point of the store.

use sqlx::PgPool;
use uuid::Uuid;

use crate::compress::Original;

/// Store the originals held back for one request. Best-effort per row: a failure to persist
/// costs recoverability, not the request, and the marker degrades to an uncounted elision.
pub(crate) async fn persist(
    db: &PgPool,
    originals: &[Original],
    flow_id: &str,
    owner_id: Uuid,
    agent_id: Option<Uuid>,
) {
    for original in originals {
        if let Err(e) = sqlx::query(
            "INSERT INTO compression_originals \
                 (handle, flow_id, owner_id, agent_id, content, content_type) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (handle) DO NOTHING",
        )
        .bind(original.handle)
        .bind(flow_id)
        .bind(owner_id)
        .bind(agent_id)
        .bind(&original.content)
        .bind(original.content_type)
        .execute(db)
        .await
        {
            tracing::warn!(
                target: "nasiko::llm_router::recovery",
                handle = %original.handle,
                error = %e,
                "recovery: failed to persist an original; its marker is not recoverable"
            );
        }
    }
}

/// Delete originals past their TTL. Called by the host's sweep loop.
///
/// An original is only useful while its flow is alive; past that it is a full-size copy of a
/// payload we deliberately stopped sending.
pub async fn sweep_expired(db: &PgPool, ttl_secs: u64) -> Result<u64, sqlx::Error> {
    let deleted = sqlx::query(
        "DELETE FROM compression_originals \
         WHERE created_at < now() - make_interval(secs => $1)",
    )
    .bind(ttl_secs as f64)
    .execute(db)
    .await?
    .rows_affected();
    Ok(deleted)
}
