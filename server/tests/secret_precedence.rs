//! An agent's secrets layer over its owner's (NAS-1458): every agent gets the secrets its owner
//! keeps in Settings → Secrets (`user_secrets`), and an agent secret of the same name overrides
//! one of them, on every deploy path. Before, only the admin restart and ad-hoc deploy paths
//! read the owner's secrets, so a redeploy by any other path silently dropped them.
//!
//! Requires infra (Postgres, Redis, S3 - see TEST_PG_URL / TEST_REDIS_URL / TEST_S3_ENDPOINT):
//!   cargo test -p nasiko-server --test secret_precedence -- --test-threads=1

mod common;

use std::time::Duration;

use nasiko_runtime::{ContainerId, DeploymentSpec};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

struct Fixture {
    server: common::TestServer,
    admin_id: Uuid,
}

impl Fixture {
    async fn start() -> Self {
        let server = common::TestServer::start().await;
        let admin: Value = server
            .client
            .post(server.url("/api/auth/initialize-admin"))
            .json(&json!({"username": "admin", "email": "admin@test.local"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let admin_id = admin["user_id"].as_str().unwrap().parse().unwrap();
        Self { server, admin_id }
    }

    fn as_admin(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        common::as_superuser(rb, &self.admin_id.to_string(), "admin")
    }

    /// A resident agent owned by the admin, so a deploy always gives it a workload.
    async fn agent(&self, name: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO agents (name, owner_id, image, status, version, parking_policy) \
             VALUES ($1, $2, 'nasiko/echo:1.0.0', 'running', '1.0.0', 'resident') RETURNING id",
        )
        .bind(name)
        .bind(self.admin_id)
        .fetch_one(&self.server.db)
        .await
        .expect("seed agent")
    }

    async fn set_owner_secret(&self, name: &str, value: &str) {
        let res = self
            .as_admin(self.server.client.post(self.server.url("/api/secrets")))
            .json(&json!({"name": name, "value": value}))
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "owner secret: {}", res.status());
    }

    async fn set_agent_secret(&self, agent_id: Uuid, name: &str, value: &str) {
        let path = format!("/api/agents/{agent_id}/secrets");
        let res = self
            .as_admin(self.server.client.post(self.server.url(&path)))
            .json(&json!({"name": name, "value": value}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 201, "agent secret");
    }

    /// The owner holds `SHARED_KEY` and `OWNER_ONLY`; the agent overrides `SHARED_KEY`.
    async fn layered_secrets(&self, agent_id: Uuid) {
        self.set_owner_secret("SHARED_KEY", "from-owner").await;
        self.set_owner_secret("OWNER_ONLY", "owner-value").await;
        self.set_agent_secret(agent_id, "SHARED_KEY", "from-agent")
            .await;
    }

    async fn first_spec(&self, agent_id: Uuid) -> DeploymentSpec {
        let container = ContainerId::from_uuid(agent_id);
        for _ in 0..200 {
            if let Some(spec) = self.server.runtime.last_spec_for(&container) {
                return spec;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("agent {agent_id} was never deployed");
    }
}

fn assert_agent_overrides_owner(spec: &DeploymentSpec) {
    let env = &spec.env_vars;
    assert_eq!(
        env.get("SHARED_KEY").map(String::as_str),
        Some("from-agent"),
        "the agent's secret overrides the owner's of the same name"
    );
    assert_eq!(
        env.get("OWNER_ONLY").map(String::as_str),
        Some("owner-value"),
        "an owner secret the agent does not set is inherited"
    );
}

#[tokio::test]
#[serial]
async fn a_startup_reconcile_deploy_layers_the_agent_secrets_over_the_owners() {
    let f = Fixture::start().await;
    let agent_id = f.agent("reconciled").await;
    f.layered_secrets(agent_id).await;
    f.server.runtime.set_status_follows_deploys(true);

    f.server.state.init().await;

    assert_agent_overrides_owner(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_admin_restart_layers_the_agent_secrets_over_the_owners() {
    let f = Fixture::start().await;
    let agent_id = f.agent("restarted").await;
    f.layered_secrets(agent_id).await;

    let res = f
        .as_admin(
            f.server
                .client
                .post(f.server.url("/api/containers/restarted/restart")),
        )
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "restart: {}", res.status());

    assert_agent_overrides_owner(&f.first_spec(agent_id).await);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_inherited_list_names_the_owners_secrets_and_marks_the_overridden_ones() {
    let f = Fixture::start().await;
    let agent_id = f.agent("listed").await;
    f.layered_secrets(agent_id).await;

    let path = format!("/api/agents/{agent_id}/secrets/inherited");
    let res = f
        .as_admin(f.server.client.get(f.server.url(&path)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body = res.text().await.unwrap();
    assert!(
        !body.contains("from-owner") && !body.contains("owner-value"),
        "values never leave the server: {body}"
    );
    let listed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        listed,
        json!([
            {"name": "OWNER_ONLY", "overridden": false},
            {"name": "SHARED_KEY", "overridden": true},
        ])
    );
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn someone_who_cannot_manage_the_agent_cannot_list_what_it_inherits() {
    let f = Fixture::start().await;
    let agent_id = f.agent("private").await;
    f.layered_secrets(agent_id).await;

    // A real member: an unknown user id is refused as unauthenticated (401) before access is checked.
    let stranger_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) VALUES ('stranger', 'stranger@test.local', false) RETURNING id",
    )
    .fetch_one(&f.server.db)
    .await
    .unwrap();

    let path = format!("/api/agents/{agent_id}/secrets/inherited");
    let res = common::as_member(
        f.server.client.get(f.server.url(&path)),
        &stranger_id.to_string(),
        "stranger",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);
    f.server.cleanup().await;
}
