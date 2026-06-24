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

    // Three paths, all folding the total via COUNT(*) OVER():
    //   1. Semantic   — embedding available → pure cosine ranking + relevance score.
    //   2. Keyword    — query but no embedder → full-text fallback (ts_rank).
    //   3. Browse     — no query → newest-first; uses the compound index.
    let rows: Vec<ArtifactRow> = if has_vec {
        // Semantic discovery: rank purely by embedding cosine distance. No full-text
        // prefilter, so semantically-related-but-keyword-disjoint matches still surface.
        // `embedding <=> vec` is cosine distance (0 = identical); score = 1 - distance.
        sqlx::query_as::<_, ArtifactRow>(
            r#"
            SELECT id, owner, name, version, artifact_type, status, description,
                   metadata, oci_digest, size_bytes, tags, framework, license,
                   created_at, updated_at,
                   (1.0 - (embedding <=> $5::vector))::real AS score,
                   COUNT(*) OVER() AS total_count
            FROM artifacts
            WHERE
                ($1::text IS NULL OR artifact_type = $1)
                AND ($2::text[] IS NULL OR tags @> $2)
                AND ($3::text IS NULL OR framework = $3)
                AND status != 'yanked'
                AND ($4::text IS NULL OR status = $4)
                AND embedding IS NOT NULL
                AND ($6::real IS NULL OR (1.0 - (embedding <=> $5::vector)) >= $6)
            ORDER BY embedding <=> $5::vector
            LIMIT $7 OFFSET $8
            "#,
        )
        .bind(&params.artifact_type)
        .bind(&tags)
        .bind(&params.framework)
        .bind(&params.status)
        .bind(&vec)
        .bind(params.min_score)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else if has_q {
        // Keyword fallback: no embedder configured/reachable → rank by full-text relevance.
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
                AND search_vector @@ plainto_tsquery('english', $5)
            ORDER BY ts_rank(search_vector, plainto_tsquery('english', $5)) DESC,
                     created_at DESC
            LIMIT $6 OFFSET $7
            "#,
        )
        .bind(&params.artifact_type)
        .bind(&tags)
        .bind(&params.framework)
        .bind(&params.status)
        .bind(&params.q)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?
    } else {
        // Browse path: no query → simple ORDER BY created_at DESC.
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

#[cfg(test)]
mod tests {
    //! Live semantic-search tests against Postgres + pgvector.
    //!
    //! Run with a reachable Postgres (the docker-compose pgvector image):
    //!   DATABASE_URL=postgres://nasiko:nasiko@localhost:5432/nasiko_dev \
    //!     cargo test -p nasiko-artifact-registry -- --nocapture
    //!
    //! `#[sqlx::test]` creates a fresh database per test and applies the crate
    //! migrations (including `CREATE EXTENSION vector`), so these are hermetic.
    //! Embeddings are hand-crafted concept vectors — no OpenAI call involved.

    use super::*;
    use serde_json::json;

    const DIM: usize = 1536;

    // Concept axes packed into the first few dimensions of a 1536-dim vector.
    // food=0, planning=1, finance=2, health=3.
    fn concept(food: f32, planning: f32, finance: f32, health: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; DIM];
        v[0] = food;
        v[1] = planning;
        v[2] = finance;
        v[3] = health;
        v
    }

    fn req(owner: &str, name: &str, atype: &str, desc: &str, tags: &[&str]) -> PublishRequest {
        PublishRequest {
            owner: owner.into(),
            name: name.into(),
            version: "1.0.0".into(),
            artifact_type: atype.into(),
            description: Some(desc.into()),
            metadata: json!({}),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            framework: None,
            license: None,
        }
    }

    fn base_params() -> SearchParams {
        SearchParams {
            q: None,
            artifact_type: None,
            tags: None,
            framework: None,
            status: None,
            min_score: None,
            limit: 20,
            offset: 0,
        }
    }

    fn params(q: Option<&str>, min_score: Option<f32>) -> SearchParams {
        SearchParams { q: q.map(|s| s.into()), min_score, ..base_params() }
    }

    /// Seed four artifacts with concept embeddings and return the search query vector
    /// (a "healthy eating" concept: food + health).
    async fn seed(pool: &PgPool) -> Vec<f32> {
        // nutrition: clearly food + health
        let a = insert_artifact(pool, &req("acme", "nutrition", "skill", "macro and calorie tracking", &["diet", "health"])).await.unwrap();
        update_artifact_embedding(pool, a.id, concept(1.0, 0.0, 0.0, 0.8)).await.unwrap();

        // dietary-meal-planner: food + planning + health, but NO keyword overlap with "nutrition".
        // The old full-text prefilter would have excluded this; pure semantic must surface it.
        let b = insert_artifact(pool, &req("acme", "dietary-meal-planner", "skill", "weekly grocery and recipe scheduling", &["food"])).await.unwrap();
        update_artifact_embedding(pool, b.id, concept(0.9, 0.5, 0.0, 0.6)).await.unwrap();

        // tax-calculator: finance only — should rank last / be filtered by min_score.
        let c = insert_artifact(pool, &req("acme", "tax-calculator", "agent", "income tax estimation", &["finance"])).await.unwrap();
        update_artifact_embedding(pool, c.id, concept(0.0, 0.0, 1.0, 0.0)).await.unwrap();

        // note-taker: planning only — weakly related.
        let d = insert_artifact(pool, &req("acme", "note-taker", "tool", "capture and organize notes", &["productivity"])).await.unwrap();
        update_artifact_embedding(pool, d.id, concept(0.0, 0.4, 0.0, 0.0)).await.unwrap();

        concept(1.0, 0.0, 0.0, 0.9) // "healthy eating advice"
    }

    #[sqlx::test]
    async fn semantic_ranks_by_meaning_not_keywords(pool: PgPool) {
        let query_vec = seed(&pool).await;

        let res = search_artifacts(&pool, &params(Some("healthy eating advice"), None), Some(query_vec))
            .await
            .unwrap();

        let names: Vec<&str> = res.items.iter().map(|a| a.name.as_str()).collect();

        // The two food/health skills must rank above finance/planning artifacts.
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(pos("nutrition") < pos("tax-calculator"), "nutrition should outrank tax-calculator: {names:?}");
        assert!(pos("dietary-meal-planner") < pos("tax-calculator"), "meal planner should outrank tax-calculator: {names:?}");

        // Keyword-disjoint match surfaces (proves no full-text prefilter on the semantic path).
        assert!(names.contains(&"dietary-meal-planner"), "semantic search dropped a keyword-disjoint match: {names:?}");

        // Scores are populated, in [0,1], and monotonically non-increasing.
        let scores: Vec<f32> = res.items.iter().map(|a| a.score.expect("semantic results carry a score")).collect();
        for s in &scores { assert!(*s >= -0.01 && *s <= 1.01, "score out of range: {s}"); }
        for w in scores.windows(2) { assert!(w[0] >= w[1] - 1e-4, "scores not descending: {scores:?}"); }

        // Top match is one of the food/health skills.
        assert!(matches!(names[0], "nutrition" | "dietary-meal-planner"), "unexpected top match: {}", names[0]);
    }

    #[sqlx::test]
    async fn min_score_trims_weak_matches(pool: PgPool) {
        let query_vec = seed(&pool).await;

        // High threshold should keep the food/health skills and drop finance/planning noise.
        let res = search_artifacts(&pool, &params(Some("healthy eating advice"), Some(0.5)), Some(query_vec))
            .await
            .unwrap();
        let names: Vec<&str> = res.items.iter().map(|a| a.name.as_str()).collect();

        assert!(names.contains(&"nutrition"), "high-relevance match dropped by min_score: {names:?}");
        assert!(!names.contains(&"tax-calculator"), "min_score failed to trim finance noise: {names:?}");
    }

    #[sqlx::test]
    async fn keyword_fallback_when_no_embedder(pool: PgPool) {
        seed(&pool).await;

        // No query embedding → full-text fallback. "nutrition" matches by name/description.
        let res = search_artifacts(&pool, &params(Some("nutrition"), None), None).await.unwrap();
        let names: Vec<&str> = res.items.iter().map(|a| a.name.as_str()).collect();

        assert!(names.contains(&"nutrition"), "keyword fallback missed an exact match: {names:?}");
        assert!(!names.contains(&"tax-calculator"), "keyword fallback returned an unrelated artifact: {names:?}");
        // Fallback path doesn't populate a semantic score.
        assert!(res.items.iter().all(|a| a.score.is_none()), "keyword fallback should not set scores");
    }

    #[sqlx::test]
    async fn browse_path_returns_all_newest_first(pool: PgPool) {
        seed(&pool).await;

        // No query, no embedding → browse path returns everything.
        let res = search_artifacts(&pool, &params(None, None), None).await.unwrap();
        assert_eq!(res.total, 4, "browse should return all seeded artifacts");
    }

    #[sqlx::test]
    async fn type_filter_restricts_semantic_results(pool: PgPool) {
        let query_vec = seed(&pool).await;

        let p = SearchParams { artifact_type: Some("skill".into()), ..params(Some("food"), None) };
        let res = search_artifacts(&pool, &p, Some(query_vec)).await.unwrap();

        assert!(!res.items.is_empty(), "expected some skills");
        assert!(res.items.iter().all(|a| a.artifact_type == "skill"), "type filter leaked non-skills: {:?}",
            res.items.iter().map(|a| (&a.name, &a.artifact_type)).collect::<Vec<_>>());
        // Only the two skills are seeded.
        assert_eq!(res.total, 2, "expected exactly the 2 seeded skills");
    }

    #[sqlx::test]
    async fn tags_filter_matches_array_containment(pool: PgPool) {
        let query_vec = seed(&pool).await;

        let p = SearchParams { tags: Some("health".into()), ..params(Some("food"), None) };
        let res = search_artifacts(&pool, &p, Some(query_vec)).await.unwrap();
        let names: Vec<&str> = res.items.iter().map(|a| a.name.as_str()).collect();

        // Only "nutrition" carries the "health" tag.
        assert_eq!(names, vec!["nutrition"], "tag filter should match only the health-tagged artifact: {names:?}");
    }

    #[sqlx::test]
    async fn framework_filter_restricts_results(pool: PgPool) {
        // Insert two artifacts with distinct frameworks + embeddings.
        let mut r = req("acme", "lang-agent", "agent", "an agent built on a framework", &[]);
        r.framework = Some("langchain".into());
        let a = insert_artifact(&pool, &r).await.unwrap();
        update_artifact_embedding(&pool, a.id, concept(0.5, 0.0, 0.0, 0.0)).await.unwrap();

        let mut r2 = req("acme", "other-agent", "agent", "an agent built on another framework", &[]);
        r2.framework = Some("autogen".into());
        let b = insert_artifact(&pool, &r2).await.unwrap();
        update_artifact_embedding(&pool, b.id, concept(0.5, 0.0, 0.0, 0.0)).await.unwrap();

        let p = SearchParams { framework: Some("langchain".into()), ..params(Some("framework"), None) };
        let res = search_artifacts(&pool, &p, Some(concept(0.5, 0.0, 0.0, 0.0))).await.unwrap();
        let names: Vec<&str> = res.items.iter().map(|a| a.name.as_str()).collect();

        assert_eq!(names, vec!["lang-agent"], "framework filter should isolate langchain: {names:?}");
    }

    #[sqlx::test]
    async fn yanked_artifacts_excluded(pool: PgPool) {
        let query_vec = seed(&pool).await;

        yank_artifact(&pool, "acme", "nutrition", "1.0.0").await.unwrap();

        // Semantic path
        let res = search_artifacts(&pool, &params(Some("healthy eating"), None), Some(query_vec.clone())).await.unwrap();
        assert!(!res.items.iter().any(|a| a.name == "nutrition"), "yanked artifact appeared in semantic results");

        // Browse path
        let res2 = search_artifacts(&pool, &params(None, None), None).await.unwrap();
        assert_eq!(res2.total, 3, "yanked artifact should be excluded from browse count");
    }

    #[sqlx::test]
    async fn pagination_limits_results_but_total_reflects_all(pool: PgPool) {
        seed(&pool).await;

        let p = SearchParams { limit: 2, offset: 0, ..base_params() };
        let page1 = search_artifacts(&pool, &p, None).await.unwrap();
        assert_eq!(page1.items.len(), 2, "limit should cap returned rows");
        assert_eq!(page1.total, 4, "total should reflect the full result set, not the page");

        let p2 = SearchParams { limit: 2, offset: 2, ..base_params() };
        let page2 = search_artifacts(&pool, &p2, None).await.unwrap();
        assert_eq!(page2.items.len(), 2, "second page should have the remaining rows");

        // No overlap between pages.
        let ids1: Vec<_> = page1.items.iter().map(|a| a.id).collect();
        assert!(page2.items.iter().all(|a| !ids1.contains(&a.id)), "pages overlap");
    }

    #[sqlx::test]
    async fn empty_db_returns_no_results(pool: PgPool) {
        // Semantic query against an empty registry.
        let res = search_artifacts(&pool, &params(Some("anything"), None), Some(concept(1.0, 0.0, 0.0, 0.0))).await.unwrap();
        assert_eq!(res.total, 0);
        assert!(res.items.is_empty());
    }

    #[sqlx::test]
    async fn search_spans_all_artifact_types_not_just_agents(pool: PgPool) {
        // seed() contains 2 skills, 1 agent, 1 tool. An unfiltered query must consider all.
        let query_vec = seed(&pool).await;
        let res = search_artifacts(&pool, &params(Some("anything"), None), Some(query_vec)).await.unwrap();

        let has = |t: &str| res.items.iter().any(|a| a.artifact_type == t);
        assert!(has("skill"), "search excluded skills");
        assert!(has("agent"), "search excluded agents");
        assert!(has("tool"), "search excluded tools");
    }

    // ─── Live end-to-end semantic test (real OpenAI embeddings) ─────────────────
    //
    // Inserts artifacts of every type with REAL embeddings (generated from
    // name+description+tags, exactly like publish does), then runs natural-language
    // queries through the real ranking. Proves: skills & tools — not just agents —
    // are matched, and matching is driven by description/meaning, not the name.
    //
    // Skips unless OPENAI_API_KEY is set:
    //   set -a && . ./.env && set +a && cargo test -p nasiko-artifact-registry -- --nocapture live_
    #[sqlx::test]
    async fn live_semantic_matches_across_types_and_fields(pool: PgPool) {
        let Ok(key) = std::env::var("OPENAI_API_KEY") else {
            eprintln!("SKIP: OPENAI_API_KEY not set");
            return;
        };
        if key.starts_with("sk-REPLACE") {
            eprintln!("SKIP: placeholder OPENAI_API_KEY");
            return;
        }
        let base = std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into());
        let model = std::env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "text-embedding-3-small".into());

        // Embed text the same way publish does.
        let embed = |text: String| {
            let (k, b, m) = (key.clone(), base.clone(), model.clone());
            async move { crate::embeddings::generate(&k, &b, &m, &text).await.unwrap() }
        };

        // Note: code-reviewer's NAME contains none of "security/vulnerability/program" —
        // a match can only come from its DESCRIPTION.
        let catalog = [
            req("acme", "nutrition-coach", "skill", "Personalized meal plans and calorie tracking for healthy eating", &["diet", "health"]),
            req("acme", "pdf-extractor",   "tool",  "Extract text and tables from PDF documents into structured data", &["documents"]),
            req("acme", "tax-assistant",   "agent", "File income taxes, find deductions, and estimate your refund", &["finance"]),
            req("acme", "code-reviewer",   "skill", "Finds security vulnerabilities and bugs in your source code", &["dev"]),
        ];
        for r in &catalog {
            let a = insert_artifact(&pool, r).await.unwrap();
            let text = format!("{} {} {}", r.name, r.description.clone().unwrap_or_default(), r.tags.join(" "));
            let emb = embed(text).await;
            update_artifact_embedding(&pool, a.id, emb).await.unwrap();
        }

        // Helper: run a natural-language query, return ranked (name, type).
        let discover = |q: &'static str| {
            let pool = pool.clone();
            let embed = &embed;
            async move {
                let qv = embed(q.to_string()).await;
                let res = search_artifacts(&pool, &params(Some(q), None), Some(qv)).await.unwrap();
                res.items.into_iter().map(|a| (a.name, a.artifact_type, a.score.unwrap_or(0.0))).collect::<Vec<_>>()
            }
        };

        // 1. A SKILL should win a health query (not the agent).
        let r = discover("how can I eat healthier and lose weight").await;
        assert_eq!((r[0].0.as_str(), r[0].1.as_str()), ("nutrition-coach", "skill"),
            "expected the nutrition skill on top, got {r:?}");

        // 2. A TOOL should win a document query.
        let r = discover("pull the tables out of a PDF file").await;
        assert_eq!((r[0].0.as_str(), r[0].1.as_str()), ("pdf-extractor", "tool"),
            "expected the pdf tool on top, got {r:?}");

        // 3. Matched purely on DESCRIPTION — the query shares no words with the name.
        let r = discover("scan my program for security holes").await;
        assert_eq!(r[0].0.as_str(), "code-reviewer",
            "expected code-reviewer (matched via description), got {r:?}");
    }
}
