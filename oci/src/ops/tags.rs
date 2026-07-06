use sqlx::Row;

use crate::OciState;
use crate::error::Result;

pub async fn list_tags(
    state: &OciState,
    repository: &str,
    last: Option<&str>,
    limit: i64,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT reference
        FROM oci_manifests
        WHERE repository = $1
          AND reference IS NOT NULL
          AND reference NOT LIKE 'sha256:%'
          AND ($2::text IS NULL OR reference > $2)
        ORDER BY reference
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
        .filter_map(|r| r.try_get::<Option<String>, _>("reference").ok().flatten())
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
