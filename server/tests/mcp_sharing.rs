//! Integration tests for v2 connector sharing (`/api/mcp/connectors/{id}/share`)
//! and the audited invariants: revoke cleans up the grantee's connection (fix #2),
//! deleting an owner is blocked (fix #5), and Layer-1 gates visibility even with a
//! stale per-agent access row (audited rule #7).
//!
//!   cargo test -p nasiko-server --test mcp_sharing -- --test-threads=1

mod common;

use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn init_admin(server: &common::TestServer) -> String {
    server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["user_id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn create_user(server: &common::TestServer, admin_id: &str, username: &str) -> (String, Uuid) {
    let v = common::as_superuser(server.client.post(server.url("/api/users")), admin_id, "admin")
        .json(&json!({"username": username, "email": format!("{username}@test.local")}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let id = v["id"].as_str().unwrap().to_string();
    let uuid = Uuid::parse_str(&id).unwrap();
    (id, uuid)
}

async fn seed_connector(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, url, auth_type)
         VALUES ('mcp_server', $1, $2, 'https://example.com', 'none') RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn seed_connection(server: &common::TestServer, user: Uuid, connector: Uuid) {
    sqlx::query(
        "INSERT INTO mcp_user_connections (user_id, connector_id, status, encrypted_credential)
         VALUES ($1, $2, 'ACTIVE', 'enc')",
    )
    .bind(user)
    .bind(connector)
    .execute(&server.db)
    .await
    .unwrap();
}

fn catalog_has(body: &Value, name: &str) -> bool {
    body["services"].as_array().unwrap().iter().any(|s| s["name"] == name)
}

async fn seed_agent(server: &common::TestServer, owner: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, image, status, is_public) VALUES ($1, $2, 'x:1', 'stopped', false) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

#[tokio::test]
#[serial]
async fn share_by_username_grants_visibility() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "shr-owner").await;
    let (grantee_id, _) = create_user(&server, &admin, "shr-grantee").await;
    let cid = seed_connector(&server, owner_uuid, "shared-tool").await;

    // Grantee cannot see it yet.
    let before = common::as_member(server.client.get(server.url("/api/mcp/catalog")), &grantee_id, "shr-grantee")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(!catalog_has(&before, "shared-tool"));

    // Owner shares with the grantee.
    let res = common::as_member(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "shr-owner")
        .json(&json!({"username": "shr-grantee"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);

    // Now visible.
    let after = common::as_member(server.client.get(server.url("/api/mcp/catalog")), &grantee_id, "shr-grantee")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(catalog_has(&after, "shared-tool"), "grantee must see the shared connector");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn only_owner_can_share() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (_owner_id, owner_uuid) = create_user(&server, &admin, "os-owner").await;
    let (other_id, _) = create_user(&server, &admin, "os-other").await;
    let cid = seed_connector(&server, owner_uuid, "owned-tool").await;

    let res = common::as_member(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &other_id, "os-other")
        .json(&json!({"username": "os-owner"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "non-owner must not be able to share");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn public_share_visible_to_everyone() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "pub-owner").await;
    let (other_id, _) = create_user(&server, &admin, "pub-other").await;
    let cid = seed_connector(&server, owner_uuid, "public-tool").await;

    let res = common::as_member(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "pub-owner")
        .json(&json!({"public": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);

    let after = common::as_member(server.client.get(server.url("/api/mcp/catalog")), &other_id, "pub-other")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(catalog_has(&after, "public-tool"), "a public grant must be visible to everyone");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn revoke_deletes_grantee_connection() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "rev-owner").await;
    let (grantee_id, grantee_uuid) = create_user(&server, &admin, "rev-grantee").await;
    let cid = seed_connector(&server, owner_uuid, "rev-tool").await;

    // Share, then the grantee connects (seed a connection row).
    common::as_member(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "rev-owner")
        .json(&json!({"username": "rev-grantee"}))
        .send()
        .await
        .unwrap();
    seed_connection(&server, grantee_uuid, cid).await;

    // Revoke.
    let res = common::as_member(server.client.delete(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "rev-owner")
        .json(&json!({"username": "rev-grantee"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);

    // Fix #2: the grantee's connection row must be gone.
    let conns: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_user_connections WHERE user_id = $1 AND connector_id = $2")
        .bind(grantee_uuid)
        .bind(cid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(conns, 0, "revoking a grant must delete the grantee's connection");

    // And it is no longer visible.
    let after = common::as_member(server.client.get(server.url("/api/mcp/catalog")), &grantee_id, "rev-grantee")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(!catalog_has(&after, "rev-tool"));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn revoked_grant_denies_despite_stale_access_row() {
    // Audited rule #7: Layer 1 gates whether Layer 2 is even consulted. A leftover
    // enabled per-agent access row must not re-admit a revoked grantee.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "t2-owner").await;
    let (grantee_id, grantee_uuid) = create_user(&server, &admin, "t2-grantee").await;
    let cid = seed_connector(&server, owner_uuid, "t2-tool").await;

    // Share, grantee has an agent with an explicit enabled=true access row.
    common::as_member(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "t2-owner")
        .json(&json!({"username": "t2-grantee"}))
        .send()
        .await
        .unwrap();
    let agent_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO agents (name, owner_id, image, status, is_public) VALUES ('t2-agent', $1, 'x:1', 'stopped', false) RETURNING id",
    )
    .bind(grantee_uuid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled) VALUES ($1, $2, true)")
        .bind(agent_id)
        .bind(cid)
        .execute(&server.db)
        .await
        .unwrap();

    // Revoke the grant.
    common::as_member(server.client.delete(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "t2-owner")
        .json(&json!({"username": "t2-grantee"}))
        .send()
        .await
        .unwrap();

    // Even though the enabled access row still exists, Layer 1 now denies: the
    // connector must not appear in the grantee's catalog.
    let stale_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_agent_connector_access WHERE connector_id = $1 AND enabled = true")
        .bind(cid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(stale_rows, 1, "the stale enabled row is intentionally left in place for this test");

    let after = common::as_member(server.client.get(server.url("/api/mcp/catalog")), &grantee_id, "t2-grantee")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(!catalog_has(&after, "t2-tool"), "revoked grant must deny visibility despite the stale access row");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn deleting_owner_is_restricted_not_cascaded() {
    // Fix #5: owner_id is ON DELETE RESTRICT — deleting a user who owns a connector
    // must fail rather than silently destroying the (possibly shared) connector.
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (_owner_id, owner_uuid) = create_user(&server, &admin, "rst-owner").await;
    let cid = seed_connector(&server, owner_uuid, "rst-tool").await;

    let del = sqlx::query("DELETE FROM users WHERE id = $1").bind(owner_uuid).execute(&server.db).await;
    assert!(del.is_err(), "deleting a connector owner must be blocked by ON DELETE RESTRICT");

    let survived: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_connectors WHERE id = $1")
        .bind(cid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(survived, 1, "the connector must survive the blocked owner deletion");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_shares_includes_granted_by() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "gb-owner").await;
    let owner_uuid_str = owner_uuid.to_string();
    create_user(&server, &admin, "gb-grantee").await;
    let cid = seed_connector(&server, owner_uuid, "gb-tool").await;

    let res = common::as_member(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "gb-owner")
        .json(&json!({"username": "gb-grantee"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);

    let body: Value = common::as_member(server.client.get(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "gb-owner")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let grants = body["data"].as_array().unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0]["granted_by"], owner_uuid_str, "the grant response must include who created it: {grants:?}");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_shares_access_reasons_cover_owner_and_direct_grant_not_public() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "ar-owner").await;
    let (_grantee_id, grantee_uuid) = create_user(&server, &admin, "ar-grantee").await;
    let (_stranger_id, _) = create_user(&server, &admin, "ar-stranger").await;
    let cid = seed_connector(&server, owner_uuid, "ar-tool").await;

    let res = common::as_member(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "ar-owner")
        .json(&json!({"username": "ar-grantee"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);

    let body: Value = common::as_member(server.client.get(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "ar-owner")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["is_public"], false);
    let reasons = body["access_reasons"].as_array().unwrap();
    assert_eq!(reasons.len(), 2, "owner + one direct grantee, no public row: {reasons:?}");
    assert!(reasons.iter().any(|r| r["user_id"] == owner_uuid.to_string() && r["via"] == "owner"));
    assert!(reasons.iter().any(|r| r["user_id"] == grantee_uuid.to_string() && r["via"] == "direct"));

    // Make it public too — access_reasons must stay exactly the same two
    // people (public is a flag, not a third "everyone" reason).
    let res = common::as_member(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "ar-owner")
        .json(&json!({"public": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);

    let body: Value = common::as_member(server.client.get(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "ar-owner")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["is_public"], true);
    assert_eq!(body["access_reasons"].as_array().unwrap().len(), 2, "public must not add a per-person reason row");

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn share_target_search_works_for_any_authenticated_user_and_validates_query() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    // Not an admin/superuser — proves the endpoint isn't admin-gated like the
    // platform's general `/users?q=` directory.
    let (caller_id, _) = create_user(&server, &admin, "sts-caller").await;
    create_user(&server, &admin, "sts-target-alice").await;
    create_user(&server, &admin, "sts-target-bob").await;
    create_user(&server, &admin, "sts-other").await;

    let res = common::as_member(server.client.get(server.url("/api/mcp/share-targets?q=sts-target")), &caller_id, "sts-caller")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let names: Vec<&str> = body["data"].as_array().unwrap().iter().map(|u| u["username"].as_str().unwrap()).collect();
    assert!(names.contains(&"sts-target-alice"), "{names:?}");
    assert!(names.contains(&"sts-target-bob"), "{names:?}");
    assert!(!names.contains(&"sts-other"), "must not match an unrelated username: {names:?}");

    // Query too short is rejected (prevents a full-directory dump via q=).
    let res = common::as_member(server.client.get(server.url("/api/mcp/share-targets?q=s")), &caller_id, "sts-caller")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn consumers_lists_owner_and_grantee_agents_not_a_strangers() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "cons-owner").await;
    let (grantee_id, grantee_uuid) = create_user(&server, &admin, "cons-grantee").await;
    let (_stranger_id, stranger_uuid) = create_user(&server, &admin, "cons-stranger").await;

    let cid = seed_connector(&server, owner_uuid, "cons-tool").await;
    let owner_agent = seed_agent(&server, owner_uuid, "cons-owner-agent").await;
    let grantee_agent = seed_agent(&server, grantee_uuid, "cons-grantee-agent").await;
    let _stranger_agent = seed_agent(&server, stranger_uuid, "cons-stranger-agent").await;

    // Share directly with the grantee, and record a couple of live connections
    // (one per user) so the counts have something real to report.
    let res = common::as_member(server.client.post(server.url(&format!("/api/mcp/connectors/{cid}/share"))), &owner_id, "cons-owner")
        .json(&json!({"username": "cons-grantee"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    seed_connection(&server, owner_uuid, cid).await;
    seed_connection(&server, grantee_uuid, cid).await;

    let body: Value = common::as_member(server.client.get(server.url(&format!("/api/mcp/connectors/{cid}/consumers"))), &owner_id, "cons-owner")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let agent_ids: Vec<&str> = body["agents"].as_array().unwrap().iter().map(|a| a["agent_id"].as_str().unwrap()).collect();
    assert!(agent_ids.contains(&owner_agent.to_string().as_str()), "{agent_ids:?}");
    assert!(agent_ids.contains(&grantee_agent.to_string().as_str()), "{agent_ids:?}");
    assert_eq!(agent_ids.len(), 2, "the stranger's agent must not appear: {agent_ids:?}");

    assert_eq!(body["connections"], 2);
    assert_eq!(body["distinct_users"], 2);

    // A non-owner, non-admin caller must not be able to view consumers.
    let res = common::as_member(server.client.get(server.url(&format!("/api/mcp/connectors/{cid}/consumers"))), &grantee_id, "cons-grantee")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}
