use sqlx::Row;

use crate::OciState;
use crate::error::Result;

/// Reads `oci_tags`, which holds only real tags — a digest-addressed push creates
/// no row there, so nothing needs filtering out.
pub async fn list_tags(
    state: &OciState,
    repository: &str,
    last: Option<&str>,
    limit: i64,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        SELECT tag
        FROM oci_tags
        WHERE repository = $1
          AND ($2::text IS NULL OR tag > $2)
        ORDER BY tag
        LIMIT $3
        "#,
    )
    .bind(repository)
    .bind(last)
    .bind(limit.min(1000))
    .fetch_all(&state.pool)
    .await?;

    let tags: Vec<String> = rows
        .into_iter()
        .filter_map(|r| r.try_get::<String, _>("tag").ok())
        .collect();

    Ok(tags)
}

/// List repository names. `owner_filter` restricts results to repositories
/// whose agent-name segment (`{owner}/{name}` — the constant "owner" prefix
/// carries no real per-tenant meaning today, see `authz::check_repo_access`)
/// is owned by that user; pass `None` (superuser) to see everything.
pub async fn list_repositories(
    state: &OciState,
    owner_filter: Option<uuid::Uuid>,
    last: Option<&str>,
    limit: i64,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT m.repository
        FROM oci_manifests m
        WHERE ($1::text IS NULL OR m.repository > $1)
          AND (
            $2::uuid IS NULL
            OR NOT EXISTS (
                SELECT 1 FROM agents a
                WHERE a.name = split_part(m.repository, '/', 2) AND a.deleted_at IS NULL
            )
            OR EXISTS (
                SELECT 1 FROM agents a
                WHERE a.name = split_part(m.repository, '/', 2)
                  AND a.owner_id = $2
                  AND a.deleted_at IS NULL
            )
          )
        ORDER BY m.repository
        LIMIT $3
        "#,
    )
    .bind(last)
    .bind(owner_filter)
    .bind(limit.min(1000))
    .fetch_all(&state.pool)
    .await?;

    let repos: Vec<String> = rows
        .into_iter()
        .filter_map(|r| r.try_get::<String, _>("repository").ok())
        .collect();

    Ok(repos)
}
