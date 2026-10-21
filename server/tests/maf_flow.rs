/// Integration tests for the MAF (Multi-Agent Flow) API.
///
/// Requires: `docker compose -f docker-compose.infra.yml up -d`
/// Run with:  `cargo test --test maf_flow -- --test-threads=1`
///
/// Each test creates its own isolated Postgres database via TestServer::start().
/// Redis is shared across tests (stream key `nasiko:maf:execute`).
mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

// ─── Auth helpers ──────────────────────────────────────────────────────────

// `require_auth` validates a signed JWT (Authorization: Bearer or access_token
// cookie) — it does not trust inbound identity headers. Reuse the shared JWT
// helper so these requests actually authenticate under the current auth model.
fn auth(rb: reqwest::RequestBuilder, user_id: Uuid) -> reqwest::RequestBuilder {
    let user_id_str = user_id.to_string();
    common::as_member(rb, &user_id_str, &format!("user_{}", &user_id_str[..8]))
}

// ─── DB seed helpers ───────────────────────────────────────────────────────

/// Insert a minimal user row (needed as FK for agents.owner_id).
async fn seed_user(server: &common::TestServer, user_id: Uuid) {
    sqlx::query(
        "INSERT INTO users (id, username, email) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(format!("user_{}", &user_id.to_string()[..8]))
    .bind(format!("user_{}@test.example", &user_id.to_string()[..8]))
    .execute(&server.db)
    .await
    .expect("seed_user");
}

/// Insert a minimal agent row and return its id.
/// The URL is set to a non-routable address — tests don't need the agent to respond.
async fn seed_agent(server: &common::TestServer, owner_id: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, url) VALUES ('MAF Test Agent', $1, 'http://fake-agent.local/a2a') RETURNING id",
    )
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .expect("seed_agent")
}

// ─── Request helpers ───────────────────────────────────────────────────────

fn create_maf_body(name: &str, agent_id: Uuid) -> Value {
    json!({
        "name": name,
        "description": "Integration test MAF",
        "steps": [
            {
                "agent_id": agent_id,
                "task_description": "Summarise the provided text and return the key points"
            }
        ]
    })
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn test_create_and_get_maf() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    // Create
    let res = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("My MAF", agent_id)),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201, "expected 201 Created");

    let body: Value = res.json().await.unwrap();
    let data = &body["data"];
    let maf_id = data["id"].as_str().expect("id in response").to_string();
    assert_eq!(data["name"], "My MAF");
    assert_eq!(data["status"], "active");
    assert!(data["maf_json"]["steps"].is_array());
    assert_eq!(
        data["maf_json"]["steps"][0]["agent_id"],
        agent_id.to_string()
    );

    // GET by id
    let res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/{maf_id}"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body2: Value = res.json().await.unwrap();
    assert_eq!(body2["data"]["id"], maf_id);
    assert_eq!(body2["data"]["name"], "My MAF");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_list_mafs() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    for name in ["First MAF", "Second MAF"] {
        auth(
            server
                .client
                .post(server.url("/api/maf/workflows"))
                .json(&create_maf_body(name, agent_id)),
            user_id,
        )
        .send()
        .await
        .unwrap();
    }

    let res = auth(server.client.get(server.url("/api/maf/workflows")), user_id)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let items = body["data"]["data"].as_array().unwrap();
    assert_eq!(items.len(), 2, "expected 2 MAFs in list");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_update_maf_name() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    let res: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Old Name", agent_id)),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = res["data"]["id"].as_str().unwrap();

    let update_res = auth(
        server
            .client
            .put(server.url(&format!("/api/maf/workflow/{maf_id}")))
            .json(&json!({
                "name": "New Name"
            })),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(update_res.status(), 200);
    let updated: Value = update_res.json().await.unwrap();
    assert_eq!(updated["data"]["name"], "New Name");
    // Steps unchanged
    assert!(updated["data"]["maf_json"]["steps"].is_array());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_update_maf_name_via_put() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    let res: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&json!({
                "name": "Original Name",
                "steps": [{ "agent_id": agent_id, "task_description": "do something" }]
            })),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = res["data"]["id"].as_str().unwrap();
    assert_eq!(res["data"]["name"], "Original Name");

    let updated: Value = auth(
        server
            .client
            .put(server.url(&format!("/api/maf/workflow/{maf_id}")))
            .json(&json!({
                "name": "Updated Name"
            })),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(updated["data"]["name"], "Updated Name");

    server.cleanup().await;
}

/// Regression: `update_maf` must persist each
/// step's array POSITION as `step_index`, never the caller-supplied value verbatim — otherwise a
/// client that sends reordered/non-sequential `step_index`s desyncs the stored steps from their
/// `Vec` position, and the MAF executor's HITL resume path (`run_maf_from`) indexes that `Vec` by
/// `step_index` unchecked, which can panic and take down the whole MAF worker.
#[tokio::test]
#[serial]
async fn test_update_maf_normalizes_step_index_to_array_position() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    let res: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Reindex Test", agent_id)),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = res["data"]["id"].as_str().unwrap();

    // Deliberately bogus, non-sequential `step_index` values on the wire — a client sending
    // reordered or duplicated indices must never be trusted verbatim.
    let update_res = auth(
        server
            .client
            .put(server.url(&format!("/api/maf/workflow/{maf_id}")))
            .json(&json!({
                "steps": [
                    { "step_index": 7, "agent_id": agent_id, "task_description": "first step" },
                    { "step_index": 7, "agent_id": agent_id, "task_description": "second step" },
                    { "step_index": 2, "agent_id": agent_id, "task_description": "third step" },
                ]
            })),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(update_res.status(), 200);
    let updated: Value = update_res.json().await.unwrap();

    let steps = updated["data"]["maf_json"]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 3);
    for (i, step) in steps.iter().enumerate() {
        assert_eq!(
            step["step_index"].as_i64().unwrap(),
            i as i64,
            "stored step_index must be the array position, never the caller-supplied value"
        );
    }
    // Order (and therefore content) must follow submission order, not the bogus indices.
    assert_eq!(steps[0]["task_description"], "first step");
    assert_eq!(steps[1]["task_description"], "second step");
    assert_eq!(steps[2]["task_description"], "third step");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_delete_maf_soft_delete() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    let res: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Delete Me", agent_id)),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = res["data"]["id"].as_str().unwrap();

    // DELETE → 200 (204 can't carry the response envelope body)
    let del = auth(
        server
            .client
            .delete(server.url(&format!("/api/maf/workflow/{maf_id}"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(del.status(), 200);

    // GET after delete → 404 (soft-deleted, not in WHERE status='active')
    let get_after = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/{maf_id}"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(get_after.status(), 404);

    // Verify it's still in the DB with status='deleted'
    let status: String = sqlx::query_scalar("SELECT status FROM mafs WHERE id = $1")
        .bind(Uuid::parse_str(maf_id).unwrap())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(status, "deleted");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_run_workflow_creates_execution() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    let maf: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Run Me", agent_id)),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = maf["data"]["id"].as_str().unwrap();

    // POST /run → 202 with execution_id
    let run_res = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{maf_id}/run"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(run_res.status(), 202, "expected 202 Accepted");
    let run_body: Value = run_res.json().await.unwrap();
    let exec_id = run_body["data"]["execution_id"]
        .as_str()
        .expect("execution_id in response");

    // GET result/{exec_id} → execution record exists (status may be pending/running/failed since
    // the worker is live but the agent URL is unreachable)
    let result_res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/result/{exec_id}"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(result_res.status(), 200);
    let result: Value = result_res.json().await.unwrap();
    assert_eq!(result["data"]["id"], exec_id);
    assert!(["pending", "running", "failed"].contains(&result["data"]["status"].as_str().unwrap()));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_list_executions() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    let maf: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("List Execs MAF", agent_id)),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = maf["data"]["id"].as_str().unwrap();

    // Trigger two runs
    for _ in 0..2 {
        auth(
            server
                .client
                .post(server.url(&format!("/api/maf/workflow/{maf_id}/run"))),
            user_id,
        )
        .send()
        .await
        .unwrap();
    }

    let list_res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/{maf_id}/executions"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(list_res.status(), 200);
    let list_body: Value = list_res.json().await.unwrap();
    let items = list_body["data"]["data"].as_array().unwrap();
    assert_eq!(items.len(), 2, "expected 2 executions");
    // All belong to this MAF
    for item in items {
        assert_eq!(item["maf_id"], maf_id);
        assert_eq!(item["user_id"], user_id.to_string());
    }

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_get_execution_by_id() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    let maf: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Exec Detail MAF", agent_id)),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = maf["data"]["id"].as_str().unwrap();

    let run: Value = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{maf_id}/run"))),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let exec_id = run["data"]["execution_id"].as_str().unwrap();

    // GET /maf/execution/{id}
    let get_res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/execution/{exec_id}"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(get_res.status(), 200);
    let body: Value = get_res.json().await.unwrap();
    assert_eq!(body["data"]["id"], exec_id);
    assert_eq!(body["data"]["maf_id"], maf_id);
    assert_eq!(body["data"]["user_id"], user_id.to_string());
    assert_eq!(body["data"]["max_attempts"], 3); // default

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_ownership_enforced_on_all_endpoints() {
    let server = common::TestServer::start().await;

    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    seed_user(&server, owner).await;
    seed_user(&server, other).await;
    let agent_id = seed_agent(&server, owner).await;

    // Owner creates a MAF
    let maf: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Owner's MAF", agent_id)),
        owner,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = maf["data"]["id"].as_str().unwrap();

    // GET by other user → 403 (ownership-mismatch reads on MAF endpoints are
    // real errors here, not the graceful-degradation 200 pattern).
    let res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/{maf_id}"))),
        other,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403, "GET by non-owner should be 403");

    // PUT by other user → 403
    let status = auth(
        server
            .client
            .put(server.url(&format!("/api/maf/workflow/{maf_id}")))
            .json(&json!({ "name": "Stolen" })),
        other,
    )
    .send()
    .await
    .unwrap()
    .status();
    assert_eq!(status, 403, "PUT by non-owner should be 403");

    // DELETE by other user → 403
    let status = auth(
        server
            .client
            .delete(server.url(&format!("/api/maf/workflow/{maf_id}"))),
        other,
    )
    .send()
    .await
    .unwrap()
    .status();
    assert_eq!(status, 403, "DELETE by non-owner should be 403");

    // POST /run by other user → 403
    let status = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{maf_id}/run"))),
        other,
    )
    .send()
    .await
    .unwrap()
    .status();
    assert_eq!(status, 403, "run by non-owner should be 403");

    // GET /executions by other user → 403.
    let res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/{maf_id}/executions"))),
        other,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        403,
        "list_executions by non-owner should be 403"
    );

    // Verify owner's MAF is untouched
    let owner_check = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/{maf_id}"))),
        owner,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(owner_check.status(), 200);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_create_maf_inaccessible_agent_is_403() {
    // Regression test: a caller-supplied agent_id that *exists* but is private
    // and owned by someone else must be rejected, same as a nonexistent one —
    // otherwise any authenticated user could route work through any agent on
    // the platform by referencing its id directly in a workflow step.
    let server = common::TestServer::start().await;
    let owner_id = Uuid::new_v4();
    let other_user_id = Uuid::new_v4();
    seed_user(&server, owner_id).await;
    seed_user(&server, other_user_id).await;
    let private_agent = seed_agent(&server, owner_id).await;

    let res = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Bypass MAF", private_agent)),
        other_user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        403,
        "inaccessible agent_id should yield 403, not be silently accepted"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_create_maf_unknown_agent_is_403() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let nonexistent_agent = Uuid::new_v4();

    let res = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Bad Agent MAF", nonexistent_agent)),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403, "unknown agent_id should yield 403");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_create_maf_empty_name_is_400() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;

    // Empty steps → 400 (name is now optional, but steps are still required)
    let res = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&json!({
                "steps": []
            })),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_create_maf_empty_steps_is_400() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;

    let res = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&json!({
                "name": "Empty Steps MAF",
                "steps": []
            })),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_get_nonexistent_maf_is_404() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;

    let res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/{}", Uuid::new_v4()))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_result_isolation_between_users() {
    let server = common::TestServer::start().await;

    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    seed_user(&server, owner).await;
    seed_user(&server, other).await;
    let agent_id = seed_agent(&server, owner).await;

    let maf: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Isolation MAF", agent_id)),
        owner,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = maf["data"]["id"].as_str().unwrap();

    let run: Value = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{maf_id}/run"))),
        owner,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let exec_id = run["data"]["execution_id"].as_str().unwrap();

    // Other user tries to GET result/{exec_id} → 403 (real error, not the
    // graceful-degradation pattern).
    let res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/result/{exec_id}"))),
        other,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    // Other user tries to GET /execution/{id} → same.
    let res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/execution/{exec_id}"))),
        other,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_deleted_maf_cannot_be_run() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    let maf: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Delete Then Run", agent_id)),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = maf["data"]["id"].as_str().unwrap();

    // Soft-delete
    auth(
        server
            .client
            .delete(server.url(&format!("/api/maf/workflow/{maf_id}"))),
        user_id,
    )
    .send()
    .await
    .unwrap();

    // Try to run deleted MAF → 404 (fetch_maf filters WHERE status='active')
    let run_res = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{maf_id}/run"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(run_res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_execution_endpoints_survive_workflow_deletion() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;

    let maf: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("Delete Then Poll", agent_id)),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let maf_id = maf["data"]["id"].as_str().unwrap();

    // Run it while the workflow still exists — execution history must
    // survive the soft-delete below (maf_executions.maf_id is ON DELETE SET NULL,
    // and the workflow row itself is never hard-deleted). Only starting a NEW
    // execution should require an active workflow — reading a past one's
    // result must not, since a deleted workflow can still have real execution
    // history a caller needs to see (e.g. an audit/history UI).
    let run_body: Value = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{maf_id}/run"))),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let exec_id = run_body["data"]["execution_id"].as_str().unwrap();

    // Soft-delete the parent workflow.
    auth(
        server
            .client
            .delete(server.url(&format!("/api/maf/workflow/{maf_id}"))),
        user_id,
    )
    .send()
    .await
    .unwrap();

    // GET /maf/workflow/result/{exec_id} → still 200, execution result unaffected
    let result_res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/workflow/result/{exec_id}"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(result_res.status(), 200);
    let result_body: Value = result_res.json().await.unwrap();
    assert_eq!(result_body["data"]["id"], exec_id);

    // GET /maf/execution/{id} → same, still 200
    let exec_res = auth(
        server
            .client
            .get(server.url(&format!("/api/maf/execution/{exec_id}"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(exec_res.status(), 200);
    let exec_body: Value = exec_res.json().await.unwrap();
    assert_eq!(exec_body["data"]["id"], exec_id);

    // But starting a NEW run against the now-deleted workflow must still 404.
    let run_after_delete = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{maf_id}/run"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(run_after_delete.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_maf_not_visible_to_other_user_in_list() {
    let server = common::TestServer::start().await;

    let user_a = Uuid::new_v4();
    let user_b = Uuid::new_v4();
    seed_user(&server, user_a).await;
    seed_user(&server, user_b).await;
    let agent_id = seed_agent(&server, user_a).await;

    // User A creates a MAF
    auth(
        server
            .client
            .post(server.url("/api/maf/workflows"))
            .json(&create_maf_body("User A's MAF", agent_id)),
        user_a,
    )
    .send()
    .await
    .unwrap();

    // User B lists MAFs — should see none
    let res: Value = auth(server.client.get(server.url("/api/maf/workflows")), user_b)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let items = res["data"]["data"].as_array().unwrap();
    assert_eq!(items.len(), 0, "user B should not see user A's MAFs");

    server.cleanup().await;
}

// ─── Draft promotion ───────────────────────────────────────────────────────
//
// `from-instruction` is what normally writes a draft, but it calls the external
// decomposer service — so these seed the row it would have written and test the
// part that matters here: promotion is a status change and nothing else.

/// Insert a draft carrying fully resolved steps, as `from-instruction` leaves it.
async fn seed_draft_with_steps(
    server: &common::TestServer,
    user_id: Uuid,
    agent_id: Uuid,
) -> (Uuid, Value) {
    let maf_json = json!({
        "description": null,
        "output_generation": null,
        "steps": [{
            "step_id": Uuid::new_v4(),
            "step_index": 0,
            "agent_id": agent_id,
            "agent_name": "MAF Test Agent",
            "agent_endpoint": "http://fake-agent.local/a2a",
            "task_description": "Summarise the provided text"
        }]
    });
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO mafs (user_id, name, description, maf_json, status, drafted_at)
         VALUES ($1, $2, $3, $4::jsonb, 'draft', now()) RETURNING id",
    )
    .bind(user_id)
    .bind("Seeded draft")
    .bind("summarise the provided text")
    .bind(maf_json.to_string())
    .fetch_one(&server.db)
    .await
    .expect("seed_draft_with_steps");
    (id, maf_json)
}

#[tokio::test]
#[serial]
async fn test_promote_flips_status_without_touching_steps() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;
    let (draft_id, seeded_json) = seed_draft_with_steps(&server, user_id, agent_id).await;

    let res = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{draft_id}/promote"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    assert_eq!(body["data"]["status"], "active");

    // The whole point: the plan that was reviewed is the plan that survives.
    // A re-decompose would renumber `step_id` even if it produced the same text.
    assert_eq!(
        body["data"]["maf_json"], seeded_json,
        "promotion must leave maf_json byte-for-byte identical"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_promote_empty_draft_is_400() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;

    // What `POST /maf/workflow/draft` writes: the sentence, no steps.
    let res: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflow/draft"))
            .json(&json!({"instruction": "something I typed but never decomposed"})),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let draft_id = res["data"]["id"].as_str().unwrap();

    // Promoting it would otherwise yield an active workflow that runs zero
    // steps and reports success.
    let promote = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{draft_id}/promote"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(promote.status(), 400);

    let status: String = sqlx::query_scalar("SELECT status FROM mafs WHERE id = $1")
        .bind(Uuid::parse_str(draft_id).unwrap())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        status, "draft",
        "a refused promotion must not change status"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_promote_is_not_repeatable_and_is_owner_only() {
    let server = common::TestServer::start().await;
    let owner = Uuid::new_v4();
    let stranger = Uuid::new_v4();
    seed_user(&server, owner).await;
    seed_user(&server, stranger).await;
    let agent_id = seed_agent(&server, owner).await;
    let (draft_id, _) = seed_draft_with_steps(&server, owner, agent_id).await;

    // Someone else's draft is not promotable.
    let res = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{draft_id}/promote"))),
        stranger,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    // The owner promotes once...
    let first = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{draft_id}/promote"))),
        owner,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(first.status(), 200);

    // ...and a second attempt is refused rather than re-running anything.
    let second = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{draft_id}/promote"))),
        owner,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(second.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_draft_runs_before_and_after_promotion() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;
    let (draft_id, _) = seed_draft_with_steps(&server, user_id, agent_id).await;

    // A draft with steps runs without being deployed first — trying a workflow
    // out must not require committing to it.
    let early = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{draft_id}/run"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(early.status(), 202, "a draft with steps is runnable");

    // Before promotion it is in the drafts list only.
    let in_list = |body: &Value| {
        body["data"]["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["id"] == draft_id.to_string())
    };
    let workflows: Value = auth(server.client.get(server.url("/api/maf/workflows")), user_id)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let drafts: Value = auth(
        server.client.get(server.url("/api/maf/workflow/drafts")),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(!in_list(&workflows), "a draft is not a live workflow");
    assert!(in_list(&drafts));

    auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{draft_id}/promote"))),
        user_id,
    )
    .send()
    .await
    .unwrap();

    // After promotion it is runnable, and `drafted_at` keeps it in the drafts
    // list so an idea can still be followed from sentence to runs.
    let run = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{draft_id}/run"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(run.status(), 202);

    let workflows: Value = auth(server.client.get(server.url("/api/maf/workflows")), user_id)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let drafts: Value = auth(
        server.client.get(server.url("/api/maf/workflow/drafts")),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(in_list(&workflows));
    assert!(in_list(&drafts));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_draft_is_editable() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;
    let agent_id = seed_agent(&server, user_id).await;
    let (draft_id, _) = seed_draft_with_steps(&server, user_id, agent_id).await;

    // Fixing a step the decomposer got wrong must not require deploying the
    // workflow first. This used to 404 on a row GET returns 200 for.
    let res = auth(
        server
            .client
            .put(server.url(&format!("/api/maf/workflow/{draft_id}")))
            .json(&json!({
                "name": "Edited while still a draft",
                "steps": [{
                    "step_index": 0,
                    "agent_id": agent_id,
                    "task_description": "A corrected task description"
                }]
            })),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    assert_eq!(body["data"]["name"], "Edited while still a draft");
    assert_eq!(
        body["data"]["maf_json"]["steps"][0]["task_description"],
        "A corrected task description"
    );
    // Editing is not deploying: the row is still a draft afterwards.
    assert_eq!(body["data"]["status"], "draft");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn test_run_stepless_workflow_is_400() {
    let server = common::TestServer::start().await;
    let user_id = Uuid::new_v4();
    seed_user(&server, user_id).await;

    // The row `POST /maf/workflow/draft` writes: a sentence, no steps. Running
    // it would otherwise enqueue an execution that iterates nothing and
    // reports success.
    let saved: Value = auth(
        server
            .client
            .post(server.url("/api/maf/workflow/draft"))
            .json(&json!({"instruction": "typed but never decomposed"})),
        user_id,
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let draft_id = saved["data"]["id"].as_str().unwrap();

    let run = auth(
        server
            .client
            .post(server.url(&format!("/api/maf/workflow/{draft_id}/run"))),
        user_id,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(run.status(), 400);

    // Nothing was enqueued.
    let execs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM maf_executions WHERE maf_id = $1")
        .bind(Uuid::parse_str(draft_id).unwrap())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(execs, 0);

    server.cleanup().await;
}
