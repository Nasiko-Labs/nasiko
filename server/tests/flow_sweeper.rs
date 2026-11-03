//! The flow sweeper closes flows whose request died, and only those.

mod common;

use std::time::Duration;

use nasiko_server::flow_sweeper::sweep;
use serde_json::Value;
use serial_test::serial;
use uuid::Uuid;

const STALE_AFTER: Duration = Duration::from_secs(1200);

async fn seed_flow(
    server: &common::TestServer,
    user_id: Uuid,
    flow_id: &str,
    status: &str,
    age_minutes: i64,
) {
    let session = format!("session-{flow_id}");
    sqlx::query("INSERT INTO chat_sessions (session_id, user_id, title) VALUES ($1, $2, 'sweep')")
        .bind(&session)
        .bind(user_id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO flows (flow_id, user_id, status, session_id, created_at) \
         VALUES ($1, $2, $3, $4, now() - make_interval(mins => $5::int))",
    )
    .bind(flow_id)
    .bind(user_id)
    .bind(status)
    .bind(session)
    .bind(age_minutes as i32)
    .execute(&server.db)
    .await
    .unwrap();
}

async fn flow_row(
    server: &common::TestServer,
    flow_id: &str,
) -> (String, Option<i64>, Option<String>) {
    sqlx::query_as("SELECT status, duration_ms, error_message FROM flows WHERE flow_id = $1")
        .bind(flow_id)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
#[serial]
async fn sweep_closes_only_stale_running_flows() {
    let server = common::TestServer::start().await;
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email) VALUES ('sweep', 'sweep@t.test') RETURNING id",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    seed_flow(&server, user_id, "stale", "running", 600).await;
    seed_flow(&server, user_id, "fresh", "running", 5).await;
    seed_flow(&server, user_id, "waiting-on-human", "paused", 600).await;
    seed_flow(&server, user_id, "done", "completed", 600).await;
    // Old, but a step ran 3 minutes ago: still alive, not stale.
    seed_flow(&server, user_id, "busy", "running", 600).await;
    sqlx::query(
        "INSERT INTO flow_steps (flow_id, step_order, agent_name, status, created_at) \
         VALUES ('busy', 1, 'a', 'running', now() - interval '3 minutes')",
    )
    .execute(&server.db)
    .await
    .unwrap();
    // Old, its last step ended 580 minutes ago: the duration freezes there.
    sqlx::query(
        "INSERT INTO flow_steps (flow_id, step_order, agent_name, status, created_at, completed_at) \
         VALUES ('stale', 1, 'a', 'completed', now() - interval '590 minutes', \
                 now() - interval '580 minutes')",
    )
    .execute(&server.db)
    .await
    .unwrap();

    assert_eq!(sweep(&server.db, STALE_AFTER).await.unwrap(), 1);

    let (status, duration_ms, error) = flow_row(&server, "stale").await;
    assert_eq!(status, "timed_out");
    // Created 600 minutes ago, last step ended 580 minutes ago; the two `now()`s differ by ms.
    let frozen = duration_ms.unwrap();
    assert!(
        (20 * 60 * 1000..20 * 60 * 1000 + 1000).contains(&frozen),
        "{frozen}"
    );
    assert!(error.unwrap().contains("no activity"));
    for kept in [
        ("fresh", "running"),
        ("waiting-on-human", "paused"),
        ("done", "completed"),
        ("busy", "running"),
    ] {
        assert_eq!(flow_row(&server, kept.0).await.0, kept.1, "{}", kept.0);
    }
    assert_eq!(
        sweep(&server.db, STALE_AFTER).await.unwrap(),
        0,
        "a second pass finds nothing"
    );

    let list: Value = server
        .client
        .get(server.url("/api/observability/session/list?status=timed_out"))
        .bearer_auth(common::sign_token(
            &user_id.to_string(),
            "sweep",
            true,
            "admin",
        ))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sessions = list["data"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1, "{list}");
    assert_eq!(sessions[0]["status"], "timed_out");
    server.cleanup().await;
}
