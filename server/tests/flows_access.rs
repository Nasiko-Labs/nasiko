//! A flow is its owner's, except that a superuser may open anyone's.

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn seed_user(server: &common::TestServer, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{name}@flows.test"))
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn status_of(
    server: &common::TestServer,
    user: (Uuid, &str, bool),
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> u16 {
    let mut request =
        server
            .client
            .request(method, server.url(path))
            .bearer_auth(common::sign_token(
                &user.0.to_string(),
                user.1,
                user.2,
                "admin",
            ));
    if let Some(body) = body {
        request = request.json(&body);
    }
    request.send().await.unwrap().status().as_u16()
}

#[tokio::test]
#[serial]
async fn a_superuser_may_open_and_extend_a_flow_that_is_not_theirs() {
    let server = common::TestServer::start().await;
    let owner = seed_user(&server, "flow-owner").await;
    let other = seed_user(&server, "flow-other").await;
    let root = seed_user(&server, "flow-root").await;
    sqlx::query("INSERT INTO flows (flow_id, user_id, status, session_id) VALUES ('f1', $1, 'completed', 'f1')")
        .bind(owner)
        .execute(&server.db)
        .await
        .unwrap();

    // (method, path, body, status when allowed)
    let calls = [
        (reqwest::Method::GET, "/api/flows/f1", None, 200),
        (reqwest::Method::GET, "/api/flows/f1/steps", None, 200),
        (
            reqwest::Method::POST,
            "/api/flows/f1/steps",
            Some(json!({ "agent_name": "a" })),
            201,
        ),
    ];
    for (method, path, body, allowed) in calls {
        for (label, user, expected) in [
            ("owner", (owner, "flow-owner", false), allowed),
            ("superuser", (root, "flow-root", true), allowed),
            ("another member", (other, "flow-other", false), 404),
        ] {
            let status = status_of(&server, user, method.clone(), path, body.clone()).await;
            assert_eq!(status, expected, "{label}: {method} {path}");
        }
    }
    server.cleanup().await;
}
