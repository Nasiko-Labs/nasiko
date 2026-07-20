//! Unit-level tests for the `agent_skills` sync helper (`catalog::skills`).
//!
//! Provisions an isolated database (like `build_status_enum_regression.rs`) and
//! exercises `sync_agent_skills` / `sync_agent_skills_json` directly, asserting
//! against the `agent_skills` table. No HTTP server, no Docker.
//!
//! Run with infra up: `docker compose --profile infra up -d postgres`
//!   `cargo test -p nasiko-server --test agent_skills_sync -- --test-threads=1`
//! Override the admin DSN with TEST_PG_ADMIN_URL if Postgres isn't on :5432.

use nasiko_server::catalog::models::Skill;
use nasiko_server::catalog::skills::{sync_agent_skills, sync_agent_skills_json};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn admin_url() -> String {
    std::env::var("TEST_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".to_string())
}

fn skill(id: &str, tags: &[&str]) -> Skill {
    Skill {
        id: id.to_string(),
        name: format!("{id}-name"),
        description: format!("{id}-desc"),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        examples: vec![json!({"q": id})],
    }
}

/// (skill_key, tags) rows for an agent, ordered by skill_key.
async fn rows(pool: &PgPool, agent_id: Uuid) -> Vec<(String, Vec<String>)> {
    sqlx::query("SELECT skill_key, tags FROM agent_skills WHERE agent_id = $1 ORDER BY skill_key")
        .bind(agent_id)
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>("skill_key"),
                r.get::<Vec<String>, _>("tags"),
            )
        })
        .collect()
}

/// Create an isolated DB + a user + an agent; return (pool, db_name, agent_id).
async fn setup() -> (PgPool, String, Uuid) {
    let admin_dsn = admin_url();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_dsn)
        .await
        .expect("connect to postgres — is `docker compose --profile infra up -d` running?");
    let db_name = format!("nasiko_test_skillsync_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin)
        .await
        .unwrap();
    let base = admin_dsn.rsplit_once('/').map(|(b, _)| b).unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&format!("{base}/{db_name}"))
        .await
        .unwrap();
    sqlx::migrate!("../migrations").run(&pool).await.unwrap();

    let owner: Uuid =
        sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
            .bind(format!("u-{}", Uuid::new_v4().simple()))
            .bind(format!("u-{}@test.local", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .unwrap();
    let agent_id: Uuid =
        sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("agent-{}", Uuid::new_v4().simple()))
            .bind(owner)
            .fetch_one(&pool)
            .await
            .unwrap();

    (pool, db_name, agent_id)
}

async fn teardown(db_name: &str) {
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
        .unwrap();
    let _ = sqlx::query(&format!(
        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
    ))
    .execute(&admin)
    .await;
}

#[tokio::test]
async fn replace_dedup_empty_and_roundtrip() {
    let Ok(_) = std::env::var("TEST_PG_ADMIN_URL").or_else(|_| {
        // Probe connectivity; skip cleanly if Postgres is unreachable.
        Ok::<String, std::env::VarError>(admin_url())
    }) else {
        return;
    };
    let (pool, db_name, agent_id) = setup().await;

    // 1. Initial sync [a(nlp,text), b(vision)].
    sync_agent_skills(
        &mut pool.acquire().await.unwrap(),
        agent_id,
        &[skill("a", &["nlp", "text"]), skill("b", &["vision"])],
    )
    .await
    .unwrap();
    let r = rows(&pool, agent_id).await;
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].0, "a");
    assert_eq!(r[0].1, vec!["nlp".to_string(), "text".to_string()]); // tags round-trip
    assert_eq!(r[1].0, "b");

    // 2. Replace with [b(vision), c(audio)] — 'a' must be dropped, 'b' kept, 'c' added.
    sync_agent_skills(
        &mut pool.acquire().await.unwrap(),
        agent_id,
        &[skill("b", &["vision"]), skill("c", &["audio"])],
    )
    .await
    .unwrap();
    let keys: Vec<String> = rows(&pool, agent_id)
        .await
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(
        keys,
        vec!["b".to_string(), "c".to_string()],
        "replace semantics"
    );

    // 3. Duplicate skill_key in input → single row (dedup, no 'affect row twice').
    sync_agent_skills(
        &mut pool.acquire().await.unwrap(),
        agent_id,
        &[skill("dup", &["x"]), skill("dup", &["y"])],
    )
    .await
    .unwrap();
    let r = rows(&pool, agent_id).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].0, "dup");

    // 4. Empty input clears all rows.
    sync_agent_skills(&mut pool.acquire().await.unwrap(), agent_id, &[])
        .await
        .unwrap();
    assert!(rows(&pool, agent_id).await.is_empty());

    // 5. examples jsonb round-trips.
    sync_agent_skills(
        &mut pool.acquire().await.unwrap(),
        agent_id,
        &[skill("e", &[])],
    )
    .await
    .unwrap();
    let ex: Value =
        sqlx::query_scalar("SELECT examples FROM agent_skills WHERE agent_id=$1 AND skill_key='e'")
            .bind(agent_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(ex, json!([{"q": "e"}]));

    teardown(&db_name).await;
    drop(pool);
}

#[tokio::test]
async fn json_helper_handles_valid_and_malformed() {
    let (pool, db_name, agent_id) = setup().await;

    // Valid skills JSON populates rows.
    let valid = json!([{"id": "j1", "name": "J1", "description": "d", "tags": ["t1"]}]);
    sync_agent_skills_json(&pool, agent_id, &valid).await;
    assert_eq!(rows(&pool, agent_id).await.len(), 1);

    // Malformed JSON (object, not array) is a no-op — must not panic or wipe rows.
    let malformed = json!({"not": "an array"});
    sync_agent_skills_json(&pool, agent_id, &malformed).await;
    assert_eq!(
        rows(&pool, agent_id).await.len(),
        1,
        "malformed JSON must not change rows"
    );

    teardown(&db_name).await;
    drop(pool);
}
