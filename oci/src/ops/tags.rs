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

pub async fn list_repositories(
    state: &OciState,
    last: Option<&str>,
    limit: i64,
) -> Result<Vec<String>> {
    let rows = sqlx::query(
        r#"
        SELECT DISTINCT repository
        FROM oci_manifests
        WHERE ($1::text IS NULL OR repository > $1)
        ORDER BY repository
        LIMIT $2
        "#,
    )
    .bind(last)
    .bind(limit.min(1000))
    .fetch_all(&state.pool)
    .await?;

    let repos: Vec<String> = rows
        .into_iter()
        .filter_map(|r| r.try_get::<String, _>("repository").ok())
        .collect();

    Ok(repos)
}
