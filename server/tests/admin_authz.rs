//! Regression tests for the admin-router (`/api/containers`) cross-team authz
//! gap: `container_routes` in `lib.rs` is gated only by `require_deployer` (a
//! ROLE check), which in OSS is allow-all. Without an ownership check inside
//! each handler, any deployer-role user could destroy, stop, restart, or read
//! the logs of ANY other user's agent just by knowing its name.
//!
//! These tests seed an agent owned by one user, then verify a *different*
//! non-owner user is rejected with 403 while the owner and a superuser are
//! not blocked by the ownership check (FakeRuntime always succeeds, so a
//! 2xx/204 response cleanly proves the request reached the runtime).
//!
//! Requires infra (Postgres :5432, Redis, S3):
//!   cargo test -p nasiko-server --test admin_authz -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

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

/// Seed an `agents` row directly (bypassing the API) owned by `owner_id`,
/// with `image`/`is_public` set so the admin router's `restart` (image lookup)
/// and `deploy` (existing-name resolution) paths both have real data to work
/// with. Returns the agent's UUID.
async fn seed_agent(server: &common::TestServer, owner_id: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, image, status) VALUES ($1, $2, 'nasiko/echo:1.0.0', 'running') RETURNING id",
    )
    .bind(name)
    .bind(owner_id)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

/// Create a plain (non-superuser) user row so we have a genuine second
/// identity distinct from the admin created by `initialize-admin`.
async fn seed_user(server: &common::TestServer, username: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO users (username, email, is_superuser) VALUES ($1, $2, false) RETURNING id",
    )
    .bind(username)
    .bind(format!("{username}@test.local"))
    .fetch_one(&server.db)
    .await
    .unwrap()
}

struct Scenario {
    server: common::TestServer,
    owner_id: Uuid,
    other_id: Uuid,
    super_id: Uuid,
}

impl Scenario {
    async fn setup(agent_name: &str) -> (Self, Uuid) {
        let server = common::TestServer::start().await;
        let admin = init_admin(&server).await;
        let super_id: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();

        let owner_id = seed_user(&server, "owner-user").await;
        let other_id = seed_user(&server, "other-user").await;
        let agent_id = seed_agent(&server, owner_id, agent_name).await;

        (Self { server, owner_id, other_id, super_id }, agent_id)
    }

    fn as_owner(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        common::as_member(rb, &self.owner_id.to_string(), "owner-user")
    }

    fn as_other(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        common::as_member(rb, &self.other_id.to_string(), "other-user")
    }

    fn as_super(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        common::as_superuser(rb, &self.super_id.to_string(), "admin")
    }
}

// ─── read/status-style ops: GET /api/containers/{name} ─────────────────────

#[tokio::test]
#[serial]
async fn status_rejects_non_owner_allows_owner_and_superuser() {
    let (s, _agent_id) = Scenario::setup("status-authz-agent").await;
    let path = s.server.url("/api/containers/status-authz-agent");

    let res = s.as_other(s.server.client.get(&path)).send().await.unwrap();
    assert_eq!(res.status(), 403, "non-owner must be forbidden");

    let res = s.as_owner(s.server.client.get(&path)).send().await.unwrap();
    assert_eq!(res.status(), 200, "owner must be allowed");

    let res = s.as_super(s.server.client.get(&path)).send().await.unwrap();
    assert_eq!(res.status(), 200, "superuser must be allowed");

    s.server.cleanup().await;
}

// ─── destructive ops: DELETE /api/containers/{name} ─────────────────────────

#[tokio::test]
#[serial]
async fn destroy_rejects_non_owner() {
    let (s, _agent_id) = Scenario::setup("destroy-authz-agent").await;
    let path = s.server.url("/api/containers/destroy-authz-agent");

    let res = s.as_other(s.server.client.delete(&path)).send().await.unwrap();
    assert_eq!(res.status(), 403, "non-owner must be forbidden");

    let res = s.as_owner(s.server.client.delete(&path)).send().await.unwrap();
    assert_eq!(res.status(), 204, "owner must be allowed");

    s.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn destroy_allows_superuser() {
    let (s, _agent_id) = Scenario::setup("destroy-authz-super-agent").await;
    let path = s.server.url("/api/containers/destroy-authz-super-agent");

    let res = s.as_super(s.server.client.delete(&path)).send().await.unwrap();
    assert_eq!(res.status(), 204, "superuser must be allowed");

    s.server.cleanup().await;
}

// ─── stop / start: POST /api/containers/{name}/{stop,start} ────────────────

#[tokio::test]
#[serial]
async fn stop_rejects_non_owner_allows_owner() {
    let (s, _agent_id) = Scenario::setup("stop-authz-agent").await;
    let path = s.server.url("/api/containers/stop-authz-agent/stop");

    let res = s.as_other(s.server.client.post(&path)).send().await.unwrap();
    assert_eq!(res.status(), 403);

    let res = s.as_owner(s.server.client.post(&path)).send().await.unwrap();
    assert_eq!(res.status(), 200);

    s.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn start_rejects_non_owner_allows_owner() {
    let (s, _agent_id) = Scenario::setup("start-authz-agent").await;
    let path = s.server.url("/api/containers/start-authz-agent/start");

    let res = s.as_other(s.server.client.post(&path)).send().await.unwrap();
    assert_eq!(res.status(), 403);

    let res = s.as_owner(s.server.client.post(&path)).send().await.unwrap();
    assert_eq!(res.status(), 200);

    s.server.cleanup().await;
}

// ─── restart: POST /api/containers/{name}/restart ───────────────────────────
// The specific path that had a live bug: the handler's own agent lookup used
// a nonexistent `agents.port` column, so the query always errored, `.ok()
// .flatten()` swallowed it to `None`, and every request — regardless of an
// existing agent row — fell through to the unauthenticated ad-hoc fallback.
// That made the ownership check dead code. This proves it's now reachable.

#[tokio::test]
#[serial]
async fn restart_rejects_non_owner_allows_owner_and_superuser() {
    let (s, _agent_id) = Scenario::setup("restart-authz-agent").await;
    let path = s.server.url("/api/containers/restart-authz-agent/restart");

    let res = s.as_other(s.server.client.post(&path)).send().await.unwrap();
    assert_eq!(res.status(), 403, "non-owner must be forbidden");

    let res = s.as_owner(s.server.client.post(&path)).send().await.unwrap();
    assert_eq!(res.status(), 200, "owner must be allowed");

    s.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restart_allows_superuser() {
    let (s, _agent_id) = Scenario::setup("restart-authz-super-agent").await;
    let path = s.server.url("/api/containers/restart-authz-super-agent/restart");

    let res = s.as_super(s.server.client.post(&path)).send().await.unwrap();
    assert_eq!(res.status(), 200, "superuser must be allowed");

    s.server.cleanup().await;
}

/// A name with no catalog entry has no owner to check against — the ad-hoc
/// fallback path must remain open to any deployer (first-deploy-wins, same
/// reasoning as `deploy`'s ad-hoc-image branch).
#[tokio::test]
#[serial]
async fn restart_allows_any_deployer_when_no_agent_record_exists() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let super_id: Uuid = admin["user_id"].as_str().unwrap().parse().unwrap();
    let other_id = seed_user(&server, "no-record-user").await;
    let _ = super_id;

    let path = server.url("/api/containers/never-registered-container/restart");
    let res = common::as_member(server.client.post(&path), &other_id.to_string(), "no-record-user")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "ad-hoc restart with no catalog record must stay open");

    server.cleanup().await;
}

// ─── scale: POST /api/containers/{name}/scale ───────────────────────────────

#[tokio::test]
#[serial]
async fn scale_rejects_non_owner_allows_owner() {
    let (s, _agent_id) = Scenario::setup("scale-authz-agent").await;
    let path = s.server.url("/api/containers/scale-authz-agent/scale");

    let res = s
        .as_other(s.server.client.post(&path))
        .json(&json!({"replicas": 2}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    let res = s
        .as_owner(s.server.client.post(&path))
        .json(&json!({"replicas": 2}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    s.server.cleanup().await;
}

// ─── logs: GET /api/containers/{name}/logs ──────────────────────────────────

#[tokio::test]
#[serial]
async fn logs_rejects_non_owner_allows_owner() {
    let (s, _agent_id) = Scenario::setup("logs-authz-agent").await;
    let path = s.server.url("/api/containers/logs-authz-agent/logs");

    let res = s.as_other(s.server.client.get(&path)).send().await.unwrap();
    assert_eq!(res.status(), 403, "non-owner must not read another team's logs");

    let res = s.as_owner(s.server.client.get(&path)).send().await.unwrap();
    assert_eq!(res.status(), 200);

    s.server.cleanup().await;
}

// ─── deploy: POST /api/containers (existing agent name) ────────────────────
// The most severe variant: without this check, any deployer could redeploy
// an arbitrary image under another owner's agent name and have that agent's
// `agent_secrets` resolved and injected into their own controlled container.

#[tokio::test]
#[serial]
async fn deploy_onto_existing_agent_name_rejects_non_owner() {
    let (s, _agent_id) = Scenario::setup("deploy-authz-agent").await;
    let path = s.server.url("/api/containers");

    let res = s
        .as_other(s.server.client.post(&path))
        .json(&json!({"image": "attacker/image:evil", "name": "deploy-authz-agent"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "non-owner must not redeploy another owner's agent name");

    let res = s
        .as_owner(s.server.client.post(&path))
        .json(&json!({"image": "owner/image:1.0.0", "name": "deploy-authz-agent"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "owner must be allowed to redeploy their own agent");

    s.server.cleanup().await;
}

// ─── list: GET /api/containers must be scoped to the caller's own agents ───

#[tokio::test]
#[serial]
async fn list_scoped_to_owner_excludes_other_teams_containers() {
    let (s, agent_id) = Scenario::setup("list-authz-agent").await;

    // Deploy it for real so FakeRuntime actually tracks a container keyed by
    // the agent's UUID.
    let deploy_res = s
        .as_owner(s.server.client.post(s.server.url("/api/containers")))
        .json(&json!({"image": "nasiko/echo:1.0.0", "name": "list-authz-agent"}))
        .send()
        .await
        .unwrap();
    assert_eq!(deploy_res.status(), 201);

    // The owner sees it.
    let body: Value = s
        .as_owner(s.server.client.get(s.server.url("/api/containers")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = body.as_array().unwrap().iter().map(|c| c["container_id"].as_str().unwrap()).collect();
    assert!(ids.contains(&agent_id.to_string().as_str()), "owner must see their own container: {ids:?}");

    // A non-owner must NOT see it — previously `list` returned every
    // container in the runtime regardless of caller.
    let body: Value = s
        .as_other(s.server.client.get(s.server.url("/api/containers")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = body.as_array().unwrap().iter().map(|c| c["container_id"].as_str().unwrap()).collect();
    assert!(!ids.contains(&agent_id.to_string().as_str()), "non-owner must not see another team's container: {ids:?}");

    // A superuser bypasses the scoping and sees everything.
    let body: Value = s
        .as_super(s.server.client.get(s.server.url("/api/containers")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = body.as_array().unwrap().iter().map(|c| c["container_id"].as_str().unwrap()).collect();
    assert!(ids.contains(&agent_id.to_string().as_str()), "superuser must see every container: {ids:?}");

    s.server.cleanup().await;
}

/// A name with no catalog entry has no owner to check — first-deploy-wins.
#[tokio::test]
#[serial]
async fn deploy_with_unclaimed_name_allows_any_deployer() {
    let server = common::TestServer::start().await;
    let _ = init_admin(&server).await;
    let other_id = seed_user(&server, "unclaimed-deploy-user").await;

    let path = server.url("/api/containers");
    let res = common::as_member(server.client.post(&path), &other_id.to_string(), "unclaimed-deploy-user")
        .json(&json!({"image": "some/image:1.0.0", "name": "brand-new-adhoc-container"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201, "deploying under an unclaimed name must stay open");

    server.cleanup().await;
}
