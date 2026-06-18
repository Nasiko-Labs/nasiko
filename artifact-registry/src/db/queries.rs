use pgvector::Vector;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    error::{AppError, Result},
    models::{
        artifact::{Artifact, PublishRequest},
        search::{SearchParams, SearchResult},
    },
};

pub async fn insert_artifact(pool: &PgPool, req: &PublishRequest) -> Result<Artifact> {
    sqlx::query_as::<_, Artifact>(
        r#"
        INSERT INTO artifacts (owner, name, version, artifact_type, description, metadata, tags, framework, license)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, owner, name, version, artifact_type, status, description,
                  metadata, oci_digest, size_bytes, tags, framework, license,
                  created_at, updated_at
        "#,
    )
    .bind(&req.owner)
    .bind(&req.name)
    .bind(&req.version)
    .bind(&req.artifact_type)
    .bind(&req.description)
    .bind(&req.metadata)
    .bind(&req.tags)
    .bind(&req.framework)
    .bind(&req.license)
    .fetch_one(pool)
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("artifacts_owner_name_version_key") {
            AppError::Conflict(format!(
                "{}/{} version {} already exists",
                req.owner, req.name, req.version
            ))
        } else {
            AppError::Database(e)
        }
    })
}

pub async fn list_artifacts_by_owner(
    pool: &PgPool,
    owner: &str,
    artifact_type: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(Vec<Artifact>, i64)> {
    let items = sqlx::query_as::<_, Artifact>(
        r#"
        SELECT id, owner, name, version, artifact_type, status, description,
               metadata, oci_digest, size_bytes, tags, framework, license,
               created_at, updated_at
        FROM artifacts
        WHERE owner = $1
          AND status != 'yanked'
          AND ($2::text IS NULL OR artifact_type = $2)
        ORDER BY name ASC, created_at DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(owner)
    .bind(artifact_type)
    .bind(limit.min(100))
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let total: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM artifacts
        WHERE owner = $1
          AND status != 'yanked'
          AND ($2::text IS NULL OR artifact_type = $2)
        "#,
    )
    .bind(owner)
    .bind(artifact_type)
    .fetch_one(pool)
    .await?;

    Ok((items, total))
}

pub async fn get_artifact_latest(pool: &PgPool, owner: &str, name: &str) -> Result<Artifact> {
    sqlx::query_as::<_, Artifact>(
        r#"
        SELECT id, owner, name, version, artifact_type, status, description,
               metadata, oci_digest, size_bytes, tags, framework, license,
               created_at, updated_at
        FROM artifacts
        WHERE owner = $1 AND name = $2 AND status != 'yanked'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(owner)
    .bind(name)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound(format!("{owner}/{name} not found")))
}

pub async fn get_artifact_version(
    pool: &PgPool,
    owner: &str,
    name: &str,
    version: &str,
) -> Result<Artifact> {
    sqlx::query_as::<_, Artifact>(
        r#"
        SELECT id, owner, name, version, artifact_type, status, description,
               metadata, oci_digest, size_bytes, tags, framework, license,
               created_at, updated_at
        FROM artifacts
        WHERE owner = $1 AND name = $2 AND version = $3
        "#,
    )
    .bind(owner)
    .bind(name)
    .bind(version)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| AppError::NotFound(format!("{owner}/{name}:{version} not found")))
}

pub async fn list_artifact_versions(
    pool: &PgPool,
    owner: &str,
    name: &str,
) -> Result<Vec<Artifact>> {
    Ok(sqlx::query_as::<_, Artifact>(
        r#"
        SELECT id, owner, name, version, artifact_type, status, description,
               metadata, oci_digest, size_bytes, tags, framework, license,
               created_at, updated_at
        FROM artifacts
        WHERE owner = $1 AND name = $2
        ORDER BY created_at DESC
        "#,
    )
    .bind(owner)
    .bind(name)
    .fetch_all(pool)
    .await?)
}

pub async fn yank_artifact(
    pool: &PgPool,
    owner: &str,
    name: &str,
    version: &str,
) -> Result<()> {
    let rows = sqlx::query(
        "UPDATE artifacts SET status = 'yanked', updated_at = NOW() WHERE owner = $1 AND name = $2 AND version = $3",
    )
    .bind(owner)
    .bind(name)
    .bind(version)
    .execute(pool)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(AppError::NotFound(format!("{owner}/{name}:{version} not found")));
    }
    Ok(())
}


pub async fn update_artifact_embedding(pool: &PgPool, id: Uuid, embedding: Vec<f32>) -> Result<()> {
    let vec = Vector::from(embedding);
    sqlx::query("UPDATE artifacts SET embedding = $1 WHERE id = $2")
        .bind(vec)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Row wrapper that includes a window-function total count alongside the artifact.
#[derive(sqlx::FromRow)]
struct ArtifactRow {
    #[sqlx(flatten)]
    artifact: Artifact,
    total_count: i64,
}

pub async fn search_artifacts(
    pool: &PgPool,
    params: &SearchParams,
    query_embedding: Option<Vec<f32>>,
) -> Result<SearchResult> {
    let limit = params.limit.min(100);
    let offset = params.offset;

    let tags: Option<Vec<String>> = params
        .tags
        .as_deref()
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect());

    let vec = query_embedding.map(Vector::from);
    let has_q = params.q.as_deref().map(|s| !s.is_empty()).unwrap_or(false);
    let has_vec = vec.is_some();

    // Split into two paths so the planner can use the compound index
    // (artifact_type, framework, status, created_at DESC) for the common case.
    // Both paths use COUNT(*) OVER() to fold count into the same roundtrip.
    let rows: Vec<ArtifactRow> = if !has_q && !has_vec {
        // Fast path: no semantic scoring → simple ORDER BY created_at DESC
        sqlx::query_as::<_, ArtifactRow>(
            r#"
            SELECT id, owner, name, version, artifact_type, status, description,
                   metadata, oci_digest, size_bytes, tags, framework, license,
                   created_at, updated_at,
                   COUNT(*) OVER() AS total_count
            FROM artifacts
            WHERE
                ($1::text IS NULL OR artifact_type = $1)
                AND ($2::text[] IS NULL OR tags @> $2)
                AND ($3::text IS NULL OR framework = $3)
                AND status != 'yanked'
                AND ($4::text IS NULL OR status = $4)
            ORDER BY created_at DESC
            LIMIT $5 OFFSET $6
            "#,
        )
        .bind(&params.artifact_type)
        .bind(&tags)
        .bind(&params.framework)
        .bind(&params.status)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else {
        // Semantic path: ts_rank / vector cosine scoring
        sqlx::query_as::<_, ArtifactRow>(
            r#"
            SELECT id, owner, name, version, artifact_type, status, description,
                   metadata, oci_digest, size_bytes, tags, framework, license,
                   created_at, updated_at,
                   COUNT(*) OVER() AS total_count
            FROM artifacts
            WHERE
                ($1::text IS NULL OR artifact_type = $1)
                AND ($2::text[] IS NULL OR tags @> $2)
                AND ($3::text IS NULL OR framework = $3)
                AND status != 'yanked'
                AND ($4::text IS NULL OR status = $4)
                AND ($5::text IS NULL OR search_vector @@ plainto_tsquery('english', $5))
            ORDER BY
                CASE
                    WHEN $6::vector IS NOT NULL AND embedding IS NOT NULL AND $5::text IS NOT NULL
                        THEN 0.6 * ts_rank(search_vector, plainto_tsquery('english', $5))
                             + 0.4 * (1.0 - (embedding <=> $6::vector))
                    WHEN $6::vector IS NOT NULL AND embedding IS NOT NULL
                        THEN 1.0 - (embedding <=> $6::vector)
                    ELSE ts_rank(search_vector, plainto_tsquery('english', $5))
                END DESC,
                created_at DESC
            LIMIT $7 OFFSET $8
            "#,
        )
        .bind(&params.artifact_type)
        .bind(&tags)
        .bind(&params.framework)
        .bind(&params.status)
        .bind(&params.q)
        .bind(&vec)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    };

    let total = rows.first().map(|r| r.total_count).unwrap_or(0);
    let items = rows.into_iter().map(|r| r.artifact).collect();

    Ok(SearchResult { items, total, limit, offset })
}

pub async fn distinct_frameworks(pool: &PgPool) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT framework FROM artifacts WHERE framework IS NOT NULL AND status != 'yanked' ORDER BY framework"
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn distinct_owners(pool: &PgPool) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT owner FROM artifacts WHERE status != 'yanked' ORDER BY owner"
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}
