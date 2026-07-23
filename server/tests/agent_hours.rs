//! Integration tests for container-hours metering:
//! the `hours_meter` reconciler and GET /api/observability/finops/agent-hours.
//!
//! Verifies:
//!   - Session lifecycle: open on first observation, last_seen bump, close on
//!     disappearance; docker-restart (same key, new started_at) opens a second
//!     session; not-ready instances never bill
//!   - Sessions survive hard agent deletion; the API reports deleted agents
//!     with `deleted: true`
//!   - Windowed math: interval clipping, agent_id filter, epoch default, and
//!     bucket=hour series additivity (Σ buckets == total_hours)
//!   - FinOps dashboard carries container_hours / total_container_hours
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_hours -- --test-threads=1

mod common;

use chrono::{DateTime, Duration, Utc};
use nasiko_runtime::{ContainerId, InstanceInfo};
use nasiko_server::agents::hours_meter;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

// ─── helpers ────────────────────────────────────────────────────────────────

async fn init_admin(server: &common::TestServer) -> Value {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn create_agent(server: &common::TestServer, uid: &str, name: &str) -> Uuid {
    let agent: Value = common::as_superuser(
        server.client.post(server.url("/api/agents")),
        uid,
        "admin",
    )
    .json(&json!({"name": name, "version": "1.0.0"}))
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    agent["id"]
        .as_str()
        .unwrap_or_else(|| panic!("agent create response missing id: {agent}"))
        .parse()
        .unwrap()
}

fn instance(agent_id: Uuid, key: &str, started_at: Option<DateTime<Utc>>, ready: bool) -> InstanceInfo {
    InstanceInfo {
        container_id: ContainerId::from_uuid(agent_id),
        instance_key: key.to_owned(),
        started_at,
        ready,
    }
}

async fn reconcile(server: &common::TestServer) -> hours_meter::ReconcileStats {
    hours_meter::reconcile_once(&server.db, server.runtime.as_ref(), "docker")
        .await
        .expect("reconcile_once failed")
}

#[derive(sqlx::FromRow, Debug)]
struct SessionRow {
    started_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
}

async fn sessions_for(server: &common::TestServer, agent_id: Uuid) -> Vec<SessionRow> {
    sqlx::query_as(
        "SELECT started_at, last_seen_at, ended_at FROM agent_instance_sessions
         WHERE agent_id = $1 ORDER BY started_at",
    )
    .bind(agent_id)
    .fetch_all(&server.db)
    .await
    .unwrap()
}

/// Insert a session row with fully-controlled timestamps for exact math tests.
async fn seed_session(
    server: &common::TestServer,
    agent_id: Uuid,
    key: &str,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
) {
    sqlx::query(
        r#"INSERT INTO agent_instance_sessions
               (agent_id, agent_name, instance_key, runtime, started_at, last_seen_at, ended_at)
           VALUES ($1, $2, $3, 'docker', $4, COALESCE($5, $4), $5)"#,
    )
    .bind(agent_id)
    .bind(format!("agent-{agent_id}"))
    .bind(key)
    .bind(started_at)
    .bind(ended_at)
    .execute(&server.db)
    .await
    .unwrap();
}

async fn get_agent_hours(server: &common::TestServer, uid: &str, query: &str) -> Value {
    let res = common::as_superuser(
        server
            .client
            .get(server.url(&format!("/api/observability/finops/agent-hours{query}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    res.json().await.unwrap()
}

fn ts(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

// ─── reconciler lifecycle ────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn reconcile_opens_bumps_and_closes_sessions() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = create_agent(&server, uid, "hours-lifecycle-agent").await;

    let t0 = Utc::now() - Duration::minutes(10);
    server
        .runtime
        .set_instances(vec![instance(agent_id, "container-1", Some(t0), true)]);

    // First observation opens a session with the runtime-reported start.
    let stats = reconcile(&server).await;
    assert_eq!(stats.observed, 1);
    let rows = sessions_for(&server, agent_id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].started_at, t0);
    assert!(rows[0].ended_at.is_none(), "session must be open");
    let first_seen = rows[0].last_seen_at;

    // Second observation bumps last_seen on the SAME row.
    let _ = reconcile(&server).await;
    let rows = sessions_for(&server, agent_id).await;
    assert_eq!(rows.len(), 1, "same run must not mint a new row");
    assert!(rows[0].last_seen_at >= first_seen);
    assert!(rows[0].ended_at.is_none());

    // Instance disappears -> session closes at last confirmed sighting.
    server.runtime.set_instances(vec![]);
    let stats = reconcile(&server).await;
    assert_eq!(stats.closed, 1);
    let rows = sessions_for(&server, agent_id).await;
    assert_eq!(rows[0].ended_at, Some(rows[0].last_seen_at));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn docker_restart_same_key_new_start_opens_second_session() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = create_agent(&server, uid, "hours-restart-agent").await;

    let t0 = Utc::now() - Duration::minutes(30);
    server
        .runtime
        .set_instances(vec![instance(agent_id, "container-1", Some(t0), true)]);
    reconcile(&server).await;

    // Docker restart: same container id, new StartedAt.
    let t1 = Utc::now() - Duration::minutes(5);
    server
        .runtime
        .set_instances(vec![instance(agent_id, "container-1", Some(t1), true)]);
    reconcile(&server).await;

    let rows = sessions_for(&server, agent_id).await;
    assert_eq!(rows.len(), 2, "restart must open a fresh session");
    assert_eq!(rows[0].started_at, t0);
    assert!(rows[0].ended_at.is_some(), "pre-restart run must be closed");
    assert_eq!(rows[1].started_at, t1);
    assert!(rows[1].ended_at.is_none(), "post-restart run must be open");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn not_ready_instances_do_not_bill() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = create_agent(&server, uid, "hours-notready-agent").await;

    server.runtime.set_instances(vec![instance(
        agent_id,
        "container-crashloop",
        Some(Utc::now()),
        false,
    )]);
    let stats = reconcile(&server).await;
    assert_eq!(stats.observed, 0);
    assert!(sessions_for(&server, agent_id).await.is_empty());

    server.cleanup().await;
}

// ─── deletion survival ───────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn sessions_survive_hard_delete_and_api_flags_deleted() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = create_agent(&server, uid, "hours-deleted-agent").await;

    // Accrue a settled session, then hard-delete the agent via the API.
    seed_session(
        &server,
        agent_id,
        "container-1",
        Utc::now() - Duration::hours(2),
        Some(Utc::now() - Duration::hours(1)),
    )
    .await;
    let res = common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/agents/{agent_id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // The agent row is gone (hard delete) but the metering rows survive.
    let agent_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(agent_rows, 0);
    assert_eq!(sessions_for(&server, agent_id).await.len(), 1);

    let body = get_agent_hours(&server, uid, "").await;
    let agents = body["data"]["agents"].as_array().unwrap();
    let row = agents
        .iter()
        .find(|a| a["agent_id"] == agent_id.to_string())
        .expect("deleted agent must still appear in the hours report");
    assert_eq!(row["deleted"], json!(true));
    assert!(row["hours"].as_f64().unwrap() > 0.9, "one settled hour expected");

    server.cleanup().await;
}

// ─── windowed math, filters, buckets ─────────────────────────────────────────

#[tokio::test]
#[serial]
async fn windowed_math_filter_and_bucket_series() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let a1 = create_agent(&server, uid, "hours-math-agent-1").await;
    let a2 = create_agent(&server, uid, "hours-math-agent-2").await;

    // Window under test: 10:00 → 13:00.
    let win_start = "2026-07-20T10:00:00Z";
    let win_end = "2026-07-20T13:00:00Z";

    // a1: one run spanning all three hour-buckets (09:30 → 13:30, clipped to
    // 3.0h) plus one fully-inside run (10:15 → 10:45 = 0.5h).
    seed_session(&server, a1, "a1-span", ts("2026-07-20T09:30:00Z"), Some(ts("2026-07-20T13:30:00Z"))).await;
    seed_session(&server, a1, "a1-inside", ts("2026-07-20T10:15:00Z"), Some(ts("2026-07-20T10:45:00Z"))).await;
    // a2: 1h inside the window, plus one entirely outside (must not count).
    seed_session(&server, a2, "a2-inside", ts("2026-07-20T11:00:00Z"), Some(ts("2026-07-20T12:00:00Z"))).await;
    seed_session(&server, a2, "a2-outside", ts("2026-07-20T07:00:00Z"), Some(ts("2026-07-20T08:00:00Z"))).await;

    // Plain window: a1 = 3.5h, a2 = 1.0h, total 4.5h.
    let body = get_agent_hours(
        &server,
        uid,
        &format!("?start_time={win_start}&end_time={win_end}"),
    )
    .await;
    assert!((body["data"]["total_hours"].as_f64().unwrap() - 4.5).abs() < 1e-6);
    let hours_of = |body: &Value, id: Uuid| -> f64 {
        body["data"]["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["agent_id"] == id.to_string())
            .unwrap_or_else(|| panic!("no row for {id}"))["hours"]
            .as_f64()
            .unwrap()
    };
    assert!((hours_of(&body, a1) - 3.5).abs() < 1e-6);
    assert!((hours_of(&body, a2) - 1.0).abs() < 1e-6);

    // agent_id filter: total collapses to that agent's hours.
    let body = get_agent_hours(
        &server,
        uid,
        &format!("?start_time={win_start}&end_time={win_end}&agent_id={a1}"),
    )
    .await;
    assert!((body["data"]["total_hours"].as_f64().unwrap() - 3.5).abs() < 1e-6);
    assert_eq!(body["data"]["agents"].as_array().unwrap().len(), 1);

    // Invalid agent_id filter returns nothing — never everything.
    let body = get_agent_hours(
        &server,
        uid,
        &format!("?start_time={win_start}&end_time={win_end}&agent_id=not-a-uuid"),
    )
    .await;
    assert_eq!(body["data"]["total_hours"].as_f64().unwrap(), 0.0);
    assert!(body["data"]["agents"].as_array().unwrap().is_empty());

    // Epoch default (no start_time): the outside session (1.0h) now counts,
    // and the spanning session regains its pre-window 09:30→10:00 half hour
    // (3.5h + 0.5h + 1.0h + 1.0h).
    let body = get_agent_hours(&server, uid, &format!("?end_time={win_end}")).await;
    assert!((body["data"]["total_hours"].as_f64().unwrap() - 6.0).abs() < 1e-6);

    // bucket=hour: the spanning session splits 1.0/1.0/1.0 across buckets,
    // a1-inside adds 0.5 to bucket 10:00, a2-inside adds 1.0 to bucket 11:00;
    // Σ buckets == window total (additivity).
    let body = get_agent_hours(
        &server,
        uid,
        &format!("?start_time={win_start}&end_time={win_end}&bucket=hour"),
    )
    .await;
    let buckets = body["data"]["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 3);
    let bucket_hours: Vec<f64> = buckets
        .iter()
        .map(|b| b["total_hours"].as_f64().unwrap())
        .collect();
    assert!((bucket_hours[0] - 1.5).abs() < 1e-6, "10:00 bucket: {bucket_hours:?}");
    assert!((bucket_hours[1] - 2.0).abs() < 1e-6, "11:00 bucket: {bucket_hours:?}");
    assert!((bucket_hours[2] - 1.0).abs() < 1e-6, "12:00 bucket: {bucket_hours:?}");
    let sum: f64 = bucket_hours.iter().sum();
    assert!(
        (sum - body["data"]["total_hours"].as_f64().unwrap()).abs() < 1e-6,
        "series must sum to the window total"
    );

    // Unknown bucket value is ignored — no series in the response.
    let body = get_agent_hours(
        &server,
        uid,
        &format!("?start_time={win_start}&end_time={win_end}&bucket=fortnight"),
    )
    .await;
    assert!(body["data"].get("buckets").is_none());

    server.cleanup().await;
}

// ─── dashboard integration ───────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn finops_dashboard_carries_container_hours() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = create_agent(&server, uid, "hours-dashboard-agent").await;

    // One settled 2h session inside the dashboard's default 30-day window.
    seed_session(
        &server,
        agent_id,
        "container-1",
        Utc::now() - Duration::hours(3),
        Some(Utc::now() - Duration::hours(1)),
    )
    .await;

    let res = common::as_superuser(
        server
            .client
            .get(server.url("/api/observability/finops/dashboard")),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();

    let total = body["data"]["summary"]["total_container_hours"].as_f64().unwrap();
    assert!((total - 2.0).abs() < 1e-6, "summary hours: {total}");
    let row = body["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["agent_id"] == agent_id.to_string())
        .expect("agent row present");
    assert!((row["container_hours"].as_f64().unwrap() - 2.0).abs() < 1e-6);

    server.cleanup().await;
}
