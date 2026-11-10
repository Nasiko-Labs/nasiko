//! Integration tests for DELETE /api/agents/{id}.
//!
//! The route **soft**-deletes: it stamps `agents.deleted_at` and leaves the row
//! in place, so history (metering, proxy logs, chat sessions) survives while the
//! agent disappears from every read path. Foreign keys therefore never fire —
//! the assertions below deliberately check that related rows are *retained*,
//! which is the opposite of what a hard delete would give.
//!
//! Verifies:
//!   - Auth / ownership guards (401, 403, 404)
//!   - Clean 200 + JSON deletion report on success; 404 on repeat delete
//!   - `agents.deleted_at` is stamped and the row is retained
//!   - Related rows survive: agent_builds, agent_versions, agent_deployments,
//!     proxy_logs, chat_sessions.agent_id
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test agent_delete -- --test-threads=1

mod common;

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

async fn create_agent(server: &common::TestServer, uid: &str, name: &str) -> Value {
    common::as_superuser(server.client.post(server.url("/api/agents")), uid, "admin")
        .json(&json!({"name": name, "version": "1.0.0"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()
}

async fn delete_as_superuser(
    server: &common::TestServer,
    uid: &str,
    agent_id: &str,
) -> reqwest::Response {
    common::as_superuser(
        server
            .client
            .delete(server.url(&format!("/api/agents/{agent_id}"))),
        uid,
        "admin",
    )
    .send()
    .await
    .unwrap()
}

// ─── auth / guard tests ──────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn delete_agent_requires_auth() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;
    let random_id = Uuid::new_v4();

    let res = server
        .client
        .delete(server.url(&format!("/api/agents/{random_id}")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_unknown_agent_returns_404() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let random_id = Uuid::new_v4();
    let res = delete_as_superuser(&server, uid, &random_id.to_string()).await;
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_agent_by_non_owner_returns_403() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    // Agent owned by admin.
    let agent = create_agent(&server, uid, "owned-agent").await;
    let agent_id = agent["id"].as_str().unwrap();

    // Create a second user.
    let other: Value =
        common::as_superuser(server.client.post(server.url("/api/users")), uid, "admin")
            .json(&json!({"username": "other", "email": "other@test.local"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let other_id = other["id"].as_str().unwrap();

    let res = common::as_member(
        server
            .client
            .delete(server.url(&format!("/api/agents/{agent_id}"))),
        other_id,
        "other",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

// ─── basic delete ────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn delete_agent_returns_200_with_report() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();

    let agent = create_agent(&server, uid, "delete-me").await;
    let agent_id = agent["id"].as_str().unwrap();

    let res = delete_as_superuser(&server, uid, agent_id).await;
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert!(body["deleted"].as_bool().unwrap());
    assert_eq!(body["agent_id"].as_str().unwrap(), agent_id);
    assert!(body["runtime_errors"].is_array());

    // Soft delete: the row stays, stamped with `deleted_at`.
    let deleted_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT deleted_at FROM agents WHERE id = $1")
            .bind(Uuid::parse_str(agent_id).unwrap())
            .fetch_one(&server.db)
            .await
            .expect("soft delete must retain the agents row");
    assert!(deleted_at.is_some(), "deleted_at should be stamped");

    // Second delete → 404, because both the existence check and the UPDATE
    // filter on `deleted_at IS NULL`. Without that filter this returned 200
    // forever and re-ran container teardown on every call.
    let res = delete_as_superuser(&server, uid, agent_id).await;
    assert_eq!(res.status(), 404);

    server.cleanup().await;
}

// ─── history-retention tests ─────────────────────────────────────────────────
//
// A soft delete leaves `agents` intact, so no FK action (CASCADE or SET NULL)
// ever fires. These assert that dependent rows are kept — deleting an agent
// must not destroy its build history, metering or conversations.

#[tokio::test]
#[serial]
async fn delete_retains_builds_and_versions() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let agent = create_agent(&server, uid, "retain-build-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', 'retain-build-agent:1.0.0') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // `POST /api/agents` already registers a version row for the agent's own
    // version (see `catalog::create`), so this second one must use a different
    // version or it collides with `agent_versions_agent_id_version_key`.
    sqlx::query(
        "INSERT INTO agent_versions (agent_id, version, image_tag, is_active, can_rollback) \
         VALUES ($1, '2.0.0', 'retain-build-agent:2.0.0', true, false)",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id) \
         VALUES ($1, $2, 'running', $3)",
    )
    .bind(agent_id)
    .bind(build_id)
    .bind(uid_uuid)
    .execute(&server.db)
    .await
    .unwrap();

    let res = delete_as_superuser(&server, uid, &agent_id.to_string()).await;
    assert_eq!(res.status(), 200);

    let build_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_builds WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(build_count, 1, "agent_builds must survive a soft delete");

    let version_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_versions WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        version_count, 2,
        "agent_versions must survive: the auto-registered 1.0.0 plus 2.0.0"
    );

    let deploy_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_deployments WHERE agent_id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        deploy_count, 1,
        "agent_deployments must survive a soft delete"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_retains_proxy_logs() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let agent = create_agent(&server, uid, "proxy-log-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    sqlx::query(
        "INSERT INTO proxy_logs (caller_id, target_agent_id, method, latency_ms, status) \
         VALUES ($1, $2, 'POST', 42, 200)",
    )
    .bind(uid_uuid)
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let log_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM proxy_logs WHERE target_agent_id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(log_count, 1, "pre-condition: log row inserted");

    let res = delete_as_superuser(&server, uid, &agent_id.to_string()).await;
    assert_eq!(res.status(), 200);

    let log_count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM proxy_logs WHERE target_agent_id = $1")
            .bind(agent_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        log_count_after, 1,
        "proxy_logs must survive a soft delete — call history is audit data"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn delete_retains_chat_session_agent_link() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let agent = create_agent(&server, uid, "chat-session-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    let session_id = "test-session-for-delete";
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title) \
         VALUES ($1, $2, $3, 'Test Session')",
    )
    .bind(session_id)
    .bind(uid_uuid)
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();

    let res = delete_as_superuser(&server, uid, &agent_id.to_string()).await;
    assert_eq!(res.status(), 200);

    let agent_id_in_session: Option<Uuid> =
        sqlx::query_scalar("SELECT agent_id FROM chat_sessions WHERE session_id = $1")
            .bind(session_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        agent_id_in_session,
        Some(agent_id),
        "chat_sessions.agent_id must keep pointing at the soft-deleted agent so \
         the conversation still shows which agent answered"
    );

    server.cleanup().await;
}

// ─── deletion report ─────────────────────────────────────────────────────────

// Regression guard: agent_deployments.namespace is the K8s namespace ('nasiko-agents'),
// NOT the container name. A deployed agent with no k8s_deployment_name (Docker OSS path)
// must NOT produce spurious runtime_errors entries for the namespace value.
#[tokio::test]
#[serial]
async fn delete_report_has_no_spurious_runtime_errors_for_docker_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();

    let agent = create_agent(&server, uid, "docker-teardown-agent").await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();

    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', 'docker-teardown-agent:1.0.0') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();

    // Docker row: namespace='nasiko-agents' (default), k8s_deployment_name=NULL.
    // The teardown query selects k8s_deployment_name; since it is NULL here, no extra
    // container IDs are added and 'nasiko-agents' is never passed to runtime.destroy.
    sqlx::query(
        "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id) \
         VALUES ($1, $2, 'running', $3)",
    )
    .bind(agent_id)
    .bind(build_id)
    .bind(uid_uuid)
    .execute(&server.db)
    .await
    .unwrap();

    let res = delete_as_superuser(&server, uid, &agent_id.to_string()).await;
    assert_eq!(res.status(), 200);

    let body: Value = res.json().await.unwrap();
    assert!(body["deleted"].as_bool().unwrap());
    assert_eq!(body["agent_id"].as_str().unwrap(), agent_id.to_string());

    // runtime.destroy on a never-started container returns an error — that is expected.
    // There must be at most ONE entry (for the agent name itself), never a second one
    // for the K8s namespace string 'nasiko-agents'.
    let errors = body["runtime_errors"].as_array().unwrap();
    assert!(
        errors.len() <= 1,
        "expected at most 1 runtime_error (agent name, may not exist), got {}: {errors:?}",
        errors.len()
    );
    if let Some(err) = errors.first() {
        let msg = err.as_str().unwrap();
        assert!(
            !msg.contains("nasiko-agents"),
            "runtime_errors must not reference the K8s namespace: {msg}"
        );
    }

    server.cleanup().await;
}

// ─── nothing live is left behind ─────────────────────────────────────────────
//
// A soft delete keeps the row, so every read that selects agents must still treat it as gone.
// These cover the reads that did not: the orchestrator's candidate list (by status), the
// search and by-skill listings, the restart routes (a superuser passes the ownership check
// for a deleted agent too) and the lookup by a name a new agent has since taken.

/// A live agent named `name`, `running` with an image, as a deploy leaves it.
async fn running_agent(server: &common::TestServer, uid: &str, name: &str) -> Uuid {
    let agent = create_agent(server, uid, name).await;
    let agent_id: Uuid = agent["id"].as_str().unwrap().parse().unwrap();
    sqlx::query(
        "UPDATE agents SET status = 'running', image = 'nasiko/echo:1.0.0', \
         parking_policy = 'resident' WHERE id = $1",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();
    agent_id
}

async fn get_as_superuser(server: &common::TestServer, uid: &str, path: &str) -> Value {
    common::as_superuser(server.client.get(server.url(path)), uid, "admin")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn post_as_superuser(server: &common::TestServer, uid: &str, path: &str) -> u16 {
    common::as_superuser(server.client.post(server.url(path)), uid, "admin")
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
#[serial]
async fn delete_stops_the_row_and_the_orchestrator_stops_offering_it() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = running_agent(&server, uid, "routed-then-deleted").await;

    let res = delete_as_superuser(&server, uid, &agent_id.to_string()).await;
    assert_eq!(res.status(), 200);

    let status: String = sqlx::query_scalar("SELECT status FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        status, "stopped",
        "a deleted agent must not read as running"
    );

    let candidates = nasiko_orchestrator::selector::AgentSelector::fetch_active_agents(&server.db)
        .await
        .unwrap();
    assert!(
        !candidates.iter().any(|c| c.id == agent_id),
        "a deleted agent must not be a routing candidate"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_row_deleted_while_running_is_never_a_routing_candidate() {
    // Rows deleted before the delete route stopped them still read `running`.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = running_agent(&server, uid, "legacy-deleted").await;
    sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
        .bind(agent_id)
        .execute(&server.db)
        .await
        .unwrap();

    let candidates = nasiko_orchestrator::selector::AgentSelector::fetch_active_agents(&server.db)
        .await
        .unwrap();
    assert!(!candidates.iter().any(|c| c.id == agent_id));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn search_and_by_skill_never_list_a_deleted_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = running_agent(&server, uid, "findable-agent").await;
    sqlx::query(
        "INSERT INTO agent_skills (agent_id, skill_key, name, description, tags) \
         VALUES ($1, 'find', 'Find', 'finds things', ARRAY['findable'])",
    )
    .bind(agent_id)
    .execute(&server.db)
    .await
    .unwrap();
    let ids = |body: &Value| -> Vec<String> {
        body.as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let search = |body: Value| -> Vec<String> { ids(&body["agents"]) };

    // Sanity: both listings find the live agent.
    let found = search(get_as_superuser(&server, uid, "/api/search/agents?q=findable").await);
    assert!(found.contains(&agent_id.to_string()), "search: {found:?}");
    let by_skill = get_as_superuser(&server, uid, "/api/agents/by-skill?tag=findable").await;
    assert!(ids(&by_skill).contains(&agent_id.to_string()));

    assert_eq!(
        delete_as_superuser(&server, uid, &agent_id.to_string())
            .await
            .status(),
        200
    );

    let found = search(get_as_superuser(&server, uid, "/api/search/agents?q=findable").await);
    assert!(!found.contains(&agent_id.to_string()), "search: {found:?}");
    let by_skill = get_as_superuser(&server, uid, "/api/agents/by-skill?tag=findable").await;
    assert!(!ids(&by_skill).contains(&agent_id.to_string()));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_deleted_agent_cannot_be_restarted_back_to_life() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let agent_id = running_agent(&server, uid, "stays-deleted").await;
    assert_eq!(
        delete_as_superuser(&server, uid, &agent_id.to_string())
            .await
            .status(),
        200
    );
    let deploys_before = server.runtime.deployed_specs().len();

    for target in [agent_id.to_string(), "stays-deleted".to_owned()] {
        let status =
            post_as_superuser(&server, uid, &format!("/api/containers/{target}/restart")).await;
        assert_eq!(status, 404, "restart of deleted agent by {target}");
    }
    assert_eq!(
        server.runtime.deployed_specs().len(),
        deploys_before,
        "nothing may be deployed for a deleted agent"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_deleted_agents_deployment_cannot_be_restarted() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let uid_uuid: Uuid = uid.parse().unwrap();
    let agent_id = running_agent(&server, uid, "deployment-of-deleted").await;
    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_builds (agent_id, version_tag, image_reference) \
         VALUES ($1, '1.0.0', 'nasiko/echo:1.0.0') RETURNING id",
    )
    .bind(agent_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    let deployment_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id) \
         VALUES ($1, $2, 'running', $3) RETURNING id",
    )
    .bind(agent_id)
    .bind(build_id)
    .bind(uid_uuid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        delete_as_superuser(&server, uid, &agent_id.to_string())
            .await
            .status(),
        200
    );

    let status = post_as_superuser(
        &server,
        uid,
        &format!("/api/agents/deployment/{deployment_id}/restart"),
    )
    .await;
    assert_eq!(status, 404);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_reused_name_restarts_the_new_agent_not_the_deleted_one() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let uid = admin["user_id"].as_str().unwrap();
    let old_id = running_agent(&server, uid, "reused-name").await;
    assert_eq!(
        delete_as_superuser(&server, uid, &old_id.to_string())
            .await
            .status(),
        200
    );
    let new_id = running_agent(&server, uid, "reused-name").await;
    assert_ne!(old_id, new_id);

    let status = post_as_superuser(&server, uid, "/api/containers/reused-name/restart").await;
    assert_eq!(status, 200);
    let redeployed: Vec<_> = server
        .runtime
        .deployed_specs()
        .into_iter()
        .map(|spec| spec.container_id)
        .collect();
    assert!(redeployed.contains(&nasiko_runtime::ContainerId::from_uuid(new_id)));
    assert!(!redeployed.contains(&nasiko_runtime::ContainerId::from_uuid(old_id)));

    server.cleanup().await;
}
