//! The per-agent MCP management API (`/api/mcp/agents/{id}/*`) shows a caller
//! only the connectors and tools they are entitled to, and a restricted
//! caller's edits never touch rules for tools they cannot see.
//!
//!   cargo test -p nasiko-server --test mcp_tool_entitlement_views -- --test-threads=1

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use nasiko_mcp_gateway::error::Result as McpResult;
use nasiko_mcp_gateway::{ToolEntitlements, ToolSet, UserEntitlement};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

const TOOL_A: &str = "SEND_EMAIL";
const TOOL_B: &str = "READ_EMAIL";

/// Entitlement the test sets after it knows the caller's id.
struct FakeEntitlements(Mutex<UserEntitlement>);

#[async_trait]
impl ToolEntitlements for FakeEntitlements {
    async fn for_user(&self, _user_id: Uuid) -> McpResult<UserEntitlement> {
        Ok(self.0.lock().unwrap().clone())
    }
}

struct Fixture {
    server: common::TestServer,
    admin_id: String,
    agent_id: Uuid,
    /// Has tools A and B.
    c1: Uuid,
    /// Has tool A only; the caller is never entitled to it.
    c2: Uuid,
    ent: Arc<FakeEntitlements>,
}

async fn setup() -> Fixture {
    let ent = Arc::new(FakeEntitlements(Mutex::new(UserEntitlement::Unrestricted)));
    let server = common::TestServer::start_with_entitlements(ent.clone()).await;
    let v: Value = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let admin_id = v["user_id"].as_str().unwrap().to_string();
    let admin = Uuid::parse_str(&admin_id).unwrap();
    let agent_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, is_public) VALUES ('ent-agent', $1, 'x:1', 'stopped', false) RETURNING id",
    )
    .bind(admin)
    .fetch_one(&server.db)
    .await
    .unwrap();
    let c1 = seed_connector(&server, admin, "ent-c1", &[TOOL_A, TOOL_B]).await;
    let c2 = seed_connector(&server, admin, "ent-c2", &[TOOL_A]).await;
    Fixture {
        server,
        admin_id,
        agent_id,
        c1,
        c2,
        ent,
    }
}

async fn seed_connector(
    server: &common::TestServer,
    owner: Uuid,
    name: &str,
    tools: &[&str],
) -> Uuid {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, url, auth_type)
         VALUES ('mcp_server', $1, $2, 'https://example.com', 'none') RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(&server.db)
    .await
    .unwrap();
    for tool in tools {
        sqlx::query("INSERT INTO mcp_connector_tools (connector_id, tool_name, description) VALUES ($1, $2, 'd')")
            .bind(id)
            .bind(tool)
            .execute(&server.db)
            .await
            .unwrap();
    }
    id
}

fn only(names: &[&str]) -> ToolSet {
    ToolSet::Only(names.iter().map(|n| n.to_string()).collect::<HashSet<_>>())
}

impl Fixture {
    /// Swap the caller's entitlement. The gateway caches every answer, so the
    /// cached one is dropped like an edition's write path would.
    async fn restrict(&self, map: HashMap<Uuid, ToolSet>) {
        *self.ent.0.lock().unwrap() = UserEntitlement::Restricted(map);
        let admin = Uuid::parse_str(&self.admin_id).unwrap();
        nasiko_mcp_gateway::entitlements::invalidate_user_entitlements(
            &self.server.state.mcp,
            &[admin],
        )
        .await;
    }

    fn req(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        common::as_superuser(rb, &self.admin_id, "admin")
    }

    fn path(&self, suffix: &str) -> String {
        self.server
            .url(&format!("/api/mcp/agents/{}{suffix}", self.agent_id))
    }

    async fn get(&self, suffix: &str) -> (u16, Value) {
        let res = self
            .req(self.server.client.get(self.path(suffix)))
            .send()
            .await
            .unwrap();
        (
            res.status().as_u16(),
            res.json().await.unwrap_or(Value::Null),
        )
    }

    async fn put(&self, suffix: &str, body: Value) -> (u16, Value) {
        let res = self
            .req(self.server.client.put(self.path(suffix)))
            .json(&body)
            .send()
            .await
            .unwrap();
        (
            res.status().as_u16(),
            res.json().await.unwrap_or(Value::Null),
        )
    }

    async fn put_rules(&self, rules: &[(Uuid, &str, &str)]) -> u16 {
        let rules: Vec<Value> = rules
            .iter()
            .map(|(c, p, s)| json!({"connector_id": c, "tool_pattern": p, "stance": s}))
            .collect();
        self.put("/tools", json!({"rules": rules})).await.0
    }

    /// Stored `{pattern: stance}` for a connector, read straight from the DB.
    async fn stored_rules(&self, connector: Uuid) -> HashMap<String, String> {
        let raw: Value = sqlx::query_scalar(
            "SELECT tool_rules FROM mcp_agent_connector_access WHERE agent_id = $1 AND connector_id = $2",
        )
        .bind(self.agent_id)
        .bind(connector)
        .fetch_one(&self.server.db)
        .await
        .unwrap();
        raw.as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["pattern"].as_str().unwrap().to_string(),
                    r["stance"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }
}

fn names(v: &Value, key: &str, field: &str) -> HashSet<String> {
    v["data"][key]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t[field].as_str().unwrap().to_string())
        .collect()
}

fn set(items: &[&str]) -> HashSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
#[serial]
async fn unrestricted_sees_everything_and_replaces() {
    let f = setup().await;
    let (st, conns) = f.get("/connectors").await;
    assert_eq!(st, 200);
    assert_eq!(conns["data"]["connectors"].as_array().unwrap().len(), 2);
    let (_, tools) = f.get(&format!("/connectors/{}/tools", f.c1)).await;
    assert_eq!(names(&tools, "tools", "name"), set(&[TOOL_A, TOOL_B]));

    assert_eq!(
        f.put_rules(&[(f.c1, TOOL_A, "block"), (f.c1, TOOL_B, "ask")])
            .await,
        200
    );
    assert_eq!(f.put_rules(&[(f.c1, TOOL_A, "allow")]).await, 200);
    assert_eq!(
        f.stored_rules(f.c1).await,
        HashMap::from([(TOOL_A.into(), "allow".into())])
    );
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restricted_views_are_filtered() {
    let f = setup().await;
    assert_eq!(
        f.put_rules(&[(f.c1, TOOL_A, "block"), (f.c1, TOOL_B, "ask")])
            .await,
        200
    );
    f.restrict(HashMap::from([(f.c1, only(&[TOOL_A]))])).await;

    let (_, conns) = f.get("/connectors").await;
    assert_eq!(
        names(&conns, "connectors", "connector_id"),
        set(&[&f.c1.to_string()])
    );
    let (_, tools) = f.get(&format!("/connectors/{}/tools", f.c1)).await;
    assert_eq!(names(&tools, "tools", "name"), set(&[TOOL_A]));
    let (st, _) = f.get(&format!("/connectors/{}/tools", f.c2)).await;
    assert_eq!(st, 404);

    let (_, rules) = f.get("/tools").await;
    assert_eq!(names(&rules, "rules", "tool_pattern"), set(&[TOOL_A]));

    let (st, _) = f
        .put(&format!("/connectors/{}", f.c2), json!({"enabled": true}))
        .await;
    assert_eq!(st, 404);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restricted_connector_toggle_needs_all() {
    let f = setup().await;
    f.restrict(HashMap::from([(f.c1, only(&[TOOL_A]))])).await;
    let (st, _) = f
        .put(&format!("/connectors/{}", f.c1), json!({"enabled": false}))
        .await;
    assert_eq!(
        st, 403,
        "a partial view must not toggle the whole connector"
    );

    f.restrict(HashMap::from([(f.c1, ToolSet::All)])).await;
    let (st, _) = f
        .put(&format!("/connectors/{}", f.c1), json!({"enabled": false}))
        .await;
    assert_eq!(st, 200);
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restricted_wildcard_rules_are_rejected_and_hidden() {
    let f = setup().await;
    assert_eq!(f.put_rules(&[(f.c1, "*", "ask")]).await, 200);
    f.restrict(HashMap::from([(f.c1, only(&[TOOL_A]))])).await;

    // A wildcard is refused even when every tool it matches today is entitled.
    let wildcard = format!("{}*", &TOOL_A[..TOOL_A.len() - 1]);
    assert_eq!(f.put_rules(&[(f.c1, &wildcard, "allow")]).await, 403);
    assert_eq!(f.put_rules(&[(f.c1, "?", "allow")]).await, 403);
    let (_, rules) = f.get("/tools").await;
    assert!(names(&rules, "rules", "tool_pattern").is_empty());
    assert_eq!(
        f.stored_rules(f.c1).await,
        HashMap::from([("*".into(), "ask".into())])
    );
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restricted_bulk_put_merges_and_rejects() {
    let f = setup().await;
    assert_eq!(
        f.put_rules(&[(f.c1, TOOL_A, "block"), (f.c1, TOOL_B, "ask")])
            .await,
        200
    );
    f.restrict(HashMap::from([(f.c1, only(&[TOOL_A]))])).await;

    assert_eq!(f.put_rules(&[(f.c1, TOOL_A, "allow")]).await, 200);
    assert_eq!(
        f.stored_rules(f.c1).await,
        HashMap::from([
            (TOOL_A.into(), "allow".into()),
            (TOOL_B.into(), "ask".into())
        ])
    );

    assert_eq!(f.put_rules(&[(f.c1, TOOL_B, "allow")]).await, 403);
    assert_eq!(f.put_rules(&[(f.c1, "*_EMAIL", "allow")]).await, 403);
    assert_eq!(f.put_rules(&[(f.c2, TOOL_A, "allow")]).await, 403);
    assert_eq!(
        f.stored_rules(f.c1).await,
        HashMap::from([
            (TOOL_A.into(), "allow".into()),
            (TOOL_B.into(), "ask".into())
        ]),
        "rejected writes must change nothing"
    );

    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restricted_bulk_put_refused_on_one_connector_writes_none() {
    let f = setup().await;
    assert_eq!(f.put_rules(&[(f.c2, TOOL_A, "ask")]).await, 200);
    f.restrict(HashMap::from([
        (f.c1, only(&[TOOL_A])),
        (f.c2, ToolSet::All),
    ]))
    .await;
    // Connectors are written in no fixed order, so try a few times: whichever comes
    // first, the refused rule on c1 must stop the allowed one on c2 too.
    for _ in 0..8 {
        assert_eq!(
            f.put_rules(&[(f.c2, TOOL_A, "block"), (f.c1, TOOL_B, "allow")])
                .await,
            403
        );
        assert_eq!(
            f.stored_rules(f.c2).await,
            HashMap::from([(TOOL_A.into(), "ask".into())]),
            "c2 must not change"
        );
    }
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restricted_with_all_keeps_replace_behaviour() {
    let f = setup().await;
    assert_eq!(
        f.put_rules(&[(f.c1, TOOL_A, "block"), (f.c1, TOOL_B, "ask")])
            .await,
        200
    );
    f.restrict(HashMap::from([(f.c1, ToolSet::All)])).await;
    assert_eq!(f.put_rules(&[(f.c1, "*", "allow")]).await, 200);
    assert_eq!(
        f.stored_rules(f.c1).await,
        HashMap::from([("*".into(), "allow".into())])
    );
    f.server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn restricted_reset_is_forbidden() {
    let f = setup().await;
    assert_eq!(f.put_rules(&[(f.c1, TOOL_B, "ask")]).await, 200);
    f.restrict(HashMap::from([(f.c1, only(&[TOOL_A]))])).await;
    let res = f
        .req(f.server.client.delete(f.path("/permissions")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
    assert_eq!(f.stored_rules(f.c1).await.len(), 1, "rules must survive");
    f.server.cleanup().await;
}
