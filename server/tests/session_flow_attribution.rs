//! `nasiko_session_token::attribution::attributed_flow` against real `flows` and
//! `flow_participants` tables: traceparent flow, else the session's newest flow, else the claim.
//!
//! Run with infra up (`just infra`):
//!   `cargo test -p nasiko-server --test session_flow_attribution`

use nasiko_session_token::SessionClaims;
use nasiko_session_token::attribution::{FlowSource, attributed_flow};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn admin_url() -> String {
    std::env::var("TEST_PG_ADMIN_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".to_string())
}

#[tokio::test]
async fn attribution_follows_traceparent_only_for_the_owner() {
    let admin_dsn = admin_url();
    let admin = match PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_dsn)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SKIP: cannot reach Postgres at {admin_dsn}: {e}");
            return;
        }
    };
    let db_name = format!("nasiko_test_flowattr_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin)
        .await
        .expect("create test database");
    let base = admin_dsn
        .rsplit_once('/')
        .map(|(b, _)| b)
        .unwrap_or(&admin_dsn);
    let result = run_checks(&format!("{base}/{db_name}")).await;
    let _ = sqlx::query(&format!(
        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
    ))
    .execute(&admin)
    .await;
    result.expect("flow attribution decision table");
}

async fn insert_user(pool: &sqlx::PgPool, name: &str) -> Result<Uuid, String> {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@example.com"))
        .fetch_one(pool)
        .await
        .map_err(|e| format!("insert user {name}: {e}"))
}

async fn insert_agent(pool: &sqlx::PgPool, owner: Uuid, name: &str) -> Result<Uuid, String> {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status) \
         VALUES ($1, $2, 'img:1', 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(pool)
    .await
    .map_err(|e| format!("insert agent {name}: {e}"))
}

/// A flow in `session`, optionally with `agent` as a participant; `age_secs` orders the rows.
async fn insert_flow(
    pool: &sqlx::PgPool,
    flow_id: &str,
    user: Uuid,
    session: &str,
    participant: Option<Uuid>,
    age_secs: i32,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO flows (flow_id, user_id, session_id, created_at) \
         VALUES ($1, $2, $3, now() - make_interval(secs => $4))",
    )
    .bind(flow_id)
    .bind(user)
    .bind(session)
    .bind(f64::from(age_secs))
    .execute(pool)
    .await
    .map_err(|e| format!("insert flow {flow_id}: {e}"))?;
    if let Some(agent) = participant {
        sqlx::query("INSERT INTO flow_participants (flow_id, agent_id) VALUES ($1, $2)")
            .bind(flow_id)
            .bind(agent)
            .execute(pool)
            .await
            .map_err(|e| format!("insert participant {flow_id}: {e}"))?;
    }
    Ok(())
}

fn claims(agent: Uuid, user: Uuid, session: &str, flow: &str) -> SessionClaims {
    SessionClaims {
        agent_id: agent.to_string(),
        user_id: user.to_string(),
        session_id: session.into(),
        flow_id: flow.into(),
        jti: "jti".into(),
        aud: String::new(),
        iat: 0,
        exp: 0,
    }
}

async fn run_checks(db_url: &str) -> Result<(), String> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(db_url)
        .await
        .map_err(|e| format!("connect test db: {e}"))?;
    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;

    let alice = insert_user(&pool, "alice").await?;
    let bob = insert_user(&pool, "bob").await?;
    let agent = insert_agent(&pool, alice, "harness").await?;
    let first = "11111111111111111111111111111111";
    let older = "22222222222222222222222222222222";
    let newest = "33333333333333333333333333333333";
    let bobs = "44444444444444444444444444444444";
    let no_agent = "55555555555555555555555555555555";
    let other_session = "66666666666666666666666666666666";
    insert_flow(&pool, first, alice, "ses-1", Some(agent), 300).await?;
    insert_flow(&pool, older, alice, "ses-1", Some(agent), 200).await?;
    insert_flow(&pool, newest, alice, "ses-1", Some(agent), 100).await?;
    insert_flow(&pool, bobs, bob, "ses-1", Some(agent), 1).await?;
    insert_flow(&pool, no_agent, alice, "ses-1", None, 2).await?;
    insert_flow(&pool, other_session, alice, "ses-2", Some(agent), 3).await?;

    // A rejected turn's flow (newest of all) must not take attribution.
    let rejected = "77777777777777777777777777777777";
    insert_flow(&pool, rejected, alice, "ses-1", Some(agent), 0).await?;
    sqlx::query("UPDATE flows SET status = 'failed' WHERE flow_id = $1")
        .bind(rejected)
        .execute(&pool)
        .await
        .map_err(|e| format!("fail flow: {e}"))?;

    let unknown = "dddddddddddddddddddddddddddddddd";
    // (case, traceparent flow, session, expected flow, expected source)
    let cases = [
        (
            "traceparent override",
            Some(older),
            "ses-1",
            older,
            FlowSource::Traceparent,
        ),
        (
            "no traceparent: newest session flow",
            None,
            "ses-1",
            newest,
            FlowSource::Session,
        ),
        (
            "foreign flow: session flow",
            Some(bobs),
            "ses-1",
            newest,
            FlowSource::Session,
        ),
        (
            "flow without the agent: session flow",
            Some(no_agent),
            "ses-1",
            newest,
            FlowSource::Session,
        ),
        (
            "unknown trace: session flow",
            Some(unknown),
            "ses-1",
            newest,
            FlowSource::Session,
        ),
        (
            "session with no flows: claim",
            None,
            "ses-9",
            first,
            FlowSource::Claim,
        ),
        (
            "a traceparent flow of another session is not adopted",
            Some(other_session),
            "ses-9",
            first,
            FlowSource::Claim,
        ),
    ];
    for (name, trace_flow, session, expected, source) in cases {
        let got = attributed_flow(&pool, trace_flow, &claims(agent, alice, session, first)).await;
        if got.flow_id != expected || got.source != source {
            return Err(format!(
                "{name}: got {got:?}, expected {expected} / {source:?}"
            ));
        }
    }
    Ok(())
}
