//! `GET /api/observability/finops/spend-calendar?source=` filters spend to one
//! `trace_usage.source` and rejects unknown values with 400.
//!
//! Requires infra (Postgres :5432):
//!   cargo test -p nasiko-server --test spend_calendar_source -- --test-threads=1

#[allow(dead_code)]
mod common;

use serde_json::Value;
use serial_test::serial;
use uuid::Uuid;

async fn seed_user(db: &sqlx::PgPool) -> Uuid {
    let name: String = format!("src-{}", Uuid::new_v4());
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $1 || '@t') RETURNING id")
        .bind(name)
        .fetch_one(db)
        .await
        .unwrap()
}

async fn spend(db: &sqlx::PgPool, user: Uuid, source: &str, cost: f64) {
    sqlx::query(
        "INSERT INTO trace_usage (trace_id, agent_name, user_id, model, provider, input_tokens,
             output_tokens, cost_usd, started_at, source)
         VALUES ($1, 'agent-src', $2, 'gpt-4o', 'openai', 10, 5, $3, now(), $4)",
    )
    .bind(format!("trace-{}", Uuid::new_v4()))
    .bind(user)
    .bind(cost)
    .bind(source)
    .execute(db)
    .await
    .unwrap();
}

async fn calendar(server: &common::TestServer, admin: Uuid, query: &str) -> (u16, Value) {
    let month: String = sqlx::query_scalar("SELECT to_char(now() AT TIME ZONE 'UTC', 'YYYY-MM')")
        .fetch_one(&server.db)
        .await
        .unwrap();
    let url: String = server.url(&format!(
        "/api/observability/finops/spend-calendar?month={month}{query}"
    ));
    let resp = common::as_superuser(server.client.get(&url), &admin.to_string(), "admin")
        .send()
        .await
        .unwrap();
    let status: u16 = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    (status, body)
}

fn total(body: &Value) -> f64 {
    body["data"]["days"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["spend_usd"].as_f64())
        .sum()
}

#[tokio::test]
#[serial]
async fn source_filter_restricts_spend() {
    let server = common::TestServer::start().await;
    let admin: Uuid = seed_user(&server.db).await;
    let user: Uuid = seed_user(&server.db).await;
    spend(&server.db, user, "agent", 1.0).await;
    spend(&server.db, user, "agent", 2.0).await;
    spend(&server.db, user, "coding", 4.0).await;
    spend(&server.db, user, "acp", 8.0).await;

    let (s, all) = calendar(&server, admin, "").await;
    assert_eq!(s, 200, "{all}");
    assert!((total(&all) - 15.0).abs() < 1e-6, "{all}");
    for (src, want) in [("agent", 3.0), ("coding", 4.0), ("acp", 8.0)] {
        let (s, b) = calendar(&server, admin, &format!("&source={src}")).await;
        assert_eq!(s, 200, "{src}: {b}");
        assert!((total(&b) - want).abs() < 1e-6, "{src}: {b}");
    }
    let (s, _) = calendar(&server, admin, "&source=bogus").await;
    assert_eq!(s, 400);

    server.cleanup().await;
}
