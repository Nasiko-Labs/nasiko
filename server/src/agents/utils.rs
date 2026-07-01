use uuid::Uuid;

use crate::build::BuildStatus;

pub(crate) async fn set_build_status(db: &sqlx::PgPool, build_id: Uuid, status: BuildStatus) {
    if let Err(e) =
        sqlx::query("UPDATE agent_builds SET status = $2, updated_at = now() WHERE id = $1")
            .bind(build_id)
            .bind(status)
            .execute(db)
            .await
    {
        tracing::error!(build_id = %build_id, ?status, %e, "failed to update build status");
    }
}

pub(super) async fn set_upload_status(
    db: &sqlx::PgPool,
    upload_id: &str,
    agent_name: &str,
    owner_id: Uuid,
    status: &str,
    agent_id: Option<Uuid>,
    error: Option<&str>,
) {
    if let Err(e) = sqlx::query(
        "INSERT INTO upload_status (upload_id, agent_name, owner_id, status, agent_id, error_message)
         VALUES ($1, $2, $3, $4::upload_pipeline_status, $5, $6)
         ON CONFLICT (upload_id) DO UPDATE
           SET status = EXCLUDED.status,
               agent_id = COALESCE(EXCLUDED.agent_id, upload_status.agent_id),
               error_message = EXCLUDED.error_message",
    )
    .bind(upload_id)
    .bind(agent_name)
    .bind(owner_id)
    .bind(status)
    .bind(agent_id)
    .bind(error)
    .execute(db)
    .await
    {
        tracing::warn!(%e, upload_id, "failed to update upload_status");
    }
}
