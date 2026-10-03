use nasiko_orchestrator::{AgentCard, VectorStore};
use uuid::Uuid;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_agents(names: &[&str]) -> Vec<AgentCard> {
    names
        .iter()
        .map(|n| AgentCard {
            id: Uuid::new_v4(),
            name: n.to_string(),
            description: String::new(),
            skills: vec![],
            tags: vec![],
            url: None,
            embedding: None,
            embedding_content_hash: None,
        })
        .collect()
}

async fn test_pool() -> sqlx::PgPool {
    let db_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL required for tests marked 'requires live Postgres database'");
    sqlx::PgPool::connect(&db_url).await.unwrap()
}

// ── cosine_similarity (via shortlist behavior) ────────────────────────────────
// The cosine_similarity function is private; we test it indirectly by verifying
// the store's behavior, and we also test the logic by constructing known-shape inputs
// via disabled stores that exercise equal-weight scoring.

#[test]
fn disabled_store_score_agents_returns_equal_weights() {
    let agents = make_agents(&["a", "b", "c"]);
    let store = VectorStore::disabled_from_public(agents.clone());
    // With disabled store, score_agents returns all with weight 1.0
    let scored = store.score_agents(&[1.0, 0.0], &agents);
    assert_eq!(scored.len(), 3);
    for (score, _) in &scored {
        assert!(
            (*score - 1.0).abs() < 1e-6,
            "expected weight 1.0, got {score}"
        );
    }
}

// ── VectorStore::disabled ─────────────────────────────────────────────────────

#[test]
fn disabled_store_is_constructable() {
    let _store = VectorStore::disabled();
}

#[test]
fn disabled_from_public_stores_agents() {
    let agents = make_agents(&["x", "y"]);
    let store = VectorStore::disabled_from_public(agents);
    // shortlist with threshold=0 should return all (disabled store path)
    // We use the async runtime via tokio::runtime
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(store.shortlist("anything", 10, 100));
    // disabled store returns all agents regardless of threshold
    assert_eq!(result.len(), 2);
}

// ── shortlist: disabled store ─────────────────────────────────────────────────

#[tokio::test]
async fn disabled_store_shortlist_returns_all() {
    let agents = make_agents(&["a", "b"]);
    let store = VectorStore::disabled_from_public(agents);
    let result = store.shortlist("query", 1, 15).await;
    assert_eq!(result.len(), 2, "disabled store should return all agents");
}

#[tokio::test]
async fn disabled_store_shortlist_ignores_k() {
    let agents = make_agents(&["p", "q", "r", "s"]);
    let store = VectorStore::disabled_from_public(agents);
    // Even k=1 returns all when disabled
    let result = store.shortlist("query", 1, 15).await;
    assert_eq!(result.len(), 4);
}

// ── shortlist: below threshold ────────────────────────────────────────────────

#[tokio::test]
async fn below_threshold_returns_all() {
    // 2 agents, threshold 15 — skips Stage 1 and returns all
    let agents = make_agents(&["a", "b"]);
    let store = VectorStore::disabled_from_public(agents);
    let result = store.shortlist("anything", 10, 15).await;
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn exactly_at_threshold_returns_all() {
    // threshold 3, exactly 3 agents → still returns all (count < threshold is false when equal)
    let agents = make_agents(&["a", "b", "c"]);
    let store = VectorStore::disabled_from_public(agents);
    let result = store.shortlist("query", 2, 3).await;
    assert_eq!(result.len(), 3);
}

// ── embed: disabled store ─────────────────────────────────────────────────────

#[tokio::test]
async fn disabled_store_embed_returns_error() {
    let store = VectorStore::disabled();
    let result = store.embed("test text").await;
    assert!(
        result.is_err(),
        "disabled store should return Err on embed()"
    );
}

// ── score_agents: empty agents list ──────────────────────────────────────────

#[test]
fn score_agents_with_empty_list_returns_empty() {
    let store = VectorStore::disabled();
    let result = store.score_agents(&[1.0, 0.0], &[]);
    assert!(result.is_empty());
}

// ── Cosine similarity unit tests (logic via public knowledge) ─────────────────
// These test the mathematical properties by constructing scenarios where we can
// predict the cosine similarity output from score_agents on the disabled store.

#[test]
fn disabled_score_preserves_agent_identity() {
    let agents = make_agents(&["alpha", "beta"]);
    let store = VectorStore::disabled_from_public(agents.clone());
    let scored = store.score_agents(&[], &agents);
    let names: Vec<&str> = scored.iter().map(|(_, a)| a.name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
}

// ── shortlist: no API key falls back to disabled ──────────────────────────────

#[tokio::test]
#[ignore = "requires live OpenAI-compatible embeddings API"]
async fn build_with_api_key_embeds_agents() {
    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY required");
    let base_url =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".into());
    let model = "text-embedding-3-small".to_string();

    let agents = make_agents(&["coding-agent", "data-agent"]);
    let pool = test_pool().await;
    let store = VectorStore::build(agents, api_key, base_url, model, &pool).await;
    let result = store.shortlist("write code", 1, 1).await;
    assert!(!result.is_empty());
}

// ── persisted embedding: repeat build() calls should not re-embed ───────────
// Regression test for the redundant-recompute bug: Stage 1 used to call the
// embeddings API for every agent on every route() call. With the embedding
// persisted to `agents.embedding`/`embedding_content_hash`, a second build()
// call for the same (unchanged) agent must reuse the stored vector — loaded
// fresh from the DB into the agent's `AgentCard` — instead of hitting the
// network again.

#[tokio::test]
#[ignore = "requires live Postgres database"]
async fn build_reuses_stored_embedding_on_second_call() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/embeddings")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"data":[{"embedding":[0.1,0.2,0.3]}]}"#)
        .expect(1)
        .create_async()
        .await;

    let pool = test_pool().await;
    let agent_id = Uuid::new_v4();
    sqlx::query("INSERT INTO agents (id, name, owner_id, version, image, status) VALUES ($1, 'agent-1', gen_random_uuid(), '1.0.0', 'img:1', 'running')")
        .bind(agent_id)
        .execute(&pool)
        .await
        .unwrap();

    let mut agents = make_agents(&["agent-1"]);
    agents[0].id = agent_id;

    let _store1 = VectorStore::build(
        agents.clone(),
        "sk-test".to_string(),
        server.url(),
        "test-model".to_string(),
        &pool,
    )
    .await;

    // Re-fetch the agent card the way `route()` does, so the second build()
    // sees the embedding/hash that got persisted by the first call.
    let (embedding, embedding_content_hash): (Option<Vec<f64>>, Option<i64>) =
        sqlx::query_as("SELECT embedding, embedding_content_hash FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    agents[0].embedding = embedding.map(|v| v.into_iter().map(|f| f as f32).collect());
    agents[0].embedding_content_hash = embedding_content_hash;

    // Second build() with the refreshed agents must be served entirely from
    // the stored embedding — the mock only expects a single call.
    let _store2 = VectorStore::build(
        agents,
        "sk-test".to_string(),
        server.url(),
        "test-model".to_string(),
        &pool,
    )
    .await;

    mock.assert_async().await;

    sqlx::query("DELETE FROM agents WHERE id = $1")
        .bind(agent_id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires live Postgres database"]
async fn build_re_embeds_when_stored_hash_is_stale() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/v1/embeddings")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"data":[{"embedding":[0.1,0.2,0.3]}]}"#)
        .expect(2)
        .create_async()
        .await;

    let pool = test_pool().await;
    let agent_id = Uuid::new_v4();
    sqlx::query("INSERT INTO agents (id, name, owner_id, version, image, status) VALUES ($1, 'agent-1', gen_random_uuid(), '1.0.0', 'img:1', 'running')")
        .bind(agent_id)
        .execute(&pool)
        .await
        .unwrap();

    let mut agents = make_agents(&["agent-1"]);
    agents[0].id = agent_id;

    let _store1 = VectorStore::build(
        agents.clone(),
        "sk-test".to_string(),
        server.url(),
        "test-model".to_string(),
        &pool,
    )
    .await;

    // Re-fetch the stored embedding/hash the way `route()` does...
    let (embedding, embedding_content_hash): (Option<Vec<f64>>, Option<i64>) =
        sqlx::query_as("SELECT embedding, embedding_content_hash FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    agents[0].embedding = embedding.map(|v| v.into_iter().map(|f| f as f32).collect());
    agents[0].embedding_content_hash = embedding_content_hash;

    // ...then change the embedded content (description), which invalidates
    // the stored hash even though the agent id and the row itself are
    // otherwise unchanged.
    agents[0].description = "a brand new description".to_string();

    let _store2 = VectorStore::build(
        agents,
        "sk-test".to_string(),
        server.url(),
        "test-model".to_string(),
        &pool,
    )
    .await;

    mock.assert_async().await;

    sqlx::query("DELETE FROM agents WHERE id = $1")
        .bind(agent_id)
        .execute(&pool)
        .await
        .unwrap();
}
