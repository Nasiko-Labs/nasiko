//! Integration tests for v2 connector sharing (`/api/mcp/connectors/{id}/grants/*`)
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

async fn create_user(
    server: &common::TestServer,
    admin_id: &str,
    username: &str,
) -> (String, Uuid) {
    let v = common::as_superuser(
        server.client.post(server.url("/api/users")),
        admin_id,
        "admin",
    )
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
    body["data"]["services"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == name)
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
    let (grantee_id, grantee_uuid) = create_user(&server, &admin, "shr-grantee").await;
    let cid = seed_connector(&server, owner_uuid, "shared-tool").await;

    // Grantee cannot see it yet.
    let before = common::as_member(
        server.client.get(server.url("/api/mcp/catalog")),
        &grantee_id,
        "shr-grantee",
    )
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap();
    assert!(!catalog_has(&before, "shared-tool"));

    // Owner shares with the grantee.
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/users/{grantee_uuid}"
        ))),
        &owner_id,
        "shr-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    // Now visible.
    let after = common::as_member(
        server.client.get(server.url("/api/mcp/catalog")),
        &grantee_id,
        "shr-grantee",
    )
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap();
    assert!(
        catalog_has(&after, "shared-tool"),
        "grantee must see the shared connector"
    );

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

    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/users/{owner_uuid}"
        ))),
        &other_id,
        "os-other",
    )
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

    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/grants/public"))),
        &owner_id,
        "pub-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    let after = common::as_member(
        server.client.get(server.url("/api/mcp/catalog")),
        &other_id,
        "pub-other",
    )
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap();
    assert!(
        catalog_has(&after, "public-tool"),
        "a public grant must be visible to everyone"
    );

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
    common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/users/{grantee_uuid}"
        ))),
        &owner_id,
        "rev-owner",
    )
    .send()
    .await
    .unwrap();
    seed_connection(&server, grantee_uuid, cid).await;

    // Revoke.
    let res = common::as_member(
        server.client.delete(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/users/{grantee_uuid}"
        ))),
        &owner_id,
        "rev-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // Fix #2: the grantee's connection row must be gone.
    let conns: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mcp_user_connections WHERE user_id = $1 AND connector_id = $2",
    )
    .bind(grantee_uuid)
    .bind(cid)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(
        conns, 0,
        "revoking a grant must delete the grantee's connection"
    );

    // And it is no longer visible.
    let after = common::as_member(
        server.client.get(server.url("/api/mcp/catalog")),
        &grantee_id,
        "rev-grantee",
    )
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
    common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/users/{grantee_uuid}"
        ))),
        &owner_id,
        "t2-owner",
    )
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
    common::as_member(
        server.client.delete(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/users/{grantee_uuid}"
        ))),
        &owner_id,
        "t2-owner",
    )
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
    assert_eq!(
        stale_rows, 1,
        "the stale enabled row is intentionally left in place for this test"
    );

    let after = common::as_member(
        server.client.get(server.url("/api/mcp/catalog")),
        &grantee_id,
        "t2-grantee",
    )
    .send()
    .await
    .unwrap()
    .json::<Value>()
    .await
    .unwrap();
    assert!(
        !catalog_has(&after, "t2-tool"),
        "revoked grant must deny visibility despite the stale access row"
    );

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

    let del = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(owner_uuid)
        .execute(&server.db)
        .await;
    assert!(
        del.is_err(),
        "deleting a connector owner must be blocked by ON DELETE RESTRICT"
    );

    let survived: i64 = sqlx::query_scalar("SELECT count(*) FROM mcp_connectors WHERE id = $1")
        .bind(cid)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(
        survived, 1,
        "the connector must survive the blocked owner deletion"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn list_shares_includes_granted_by() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "gb-owner").await;
    let owner_uuid_str = owner_uuid.to_string();
    let (_grantee_id, grantee_uuid) = create_user(&server, &admin, "gb-grantee").await;
    let cid = seed_connector(&server, owner_uuid, "gb-tool").await;

    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/users/{grantee_uuid}"
        ))),
        &owner_id,
        "gb-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    let body: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/grants"))),
        &owner_id,
        "gb-owner",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let grants = body["data"]["grants"].as_array().unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(
        grants[0]["granted_by"], owner_uuid_str,
        "the grant response must include who created it: {grants:?}"
    );

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

    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/users/{grantee_uuid}"
        ))),
        &owner_id,
        "ar-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    let body: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/grants"))),
        &owner_id,
        "ar-owner",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["data"]["is_public"], false);
    let reasons = body["data"]["access_reasons"].as_array().unwrap();
    assert_eq!(
        reasons.len(),
        2,
        "owner + one direct grantee, no public row: {reasons:?}"
    );
    assert!(
        reasons
            .iter()
            .any(|r| r["user_id"] == owner_uuid.to_string() && r["via"] == "owner")
    );
    assert!(
        reasons
            .iter()
            .any(|r| r["user_id"] == grantee_uuid.to_string() && r["via"] == "direct")
    );

    // Make it public too — access_reasons must stay exactly the same two
    // people (public is a flag, not a third "everyone" reason).
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/grants/public"))),
        &owner_id,
        "ar-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    let body: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/grants"))),
        &owner_id,
        "ar-owner",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(body["data"]["is_public"], true);
    assert_eq!(
        body["data"]["access_reasons"].as_array().unwrap().len(),
        2,
        "public must not add a per-person reason row"
    );

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

    let res = common::as_member(
        server
            .client
            .get(server.url("/api/mcp/share-targets?q=sts-target")),
        &caller_id,
        "sts-caller",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let names: Vec<&str> = body["data"]["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["username"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"sts-target-alice"), "{names:?}");
    assert!(names.contains(&"sts-target-bob"), "{names:?}");
    assert!(
        !names.contains(&"sts-other"),
        "must not match an unrelated username: {names:?}"
    );

    // A soft-deleted user must never surface as a share target.
    sqlx::query("UPDATE users SET deleted_at = now() WHERE username = $1")
        .bind("sts-target-bob")
        .execute(&server.db)
        .await
        .unwrap();
    let res = common::as_member(
        server
            .client
            .get(server.url("/api/mcp/share-targets?q=sts-target")),
        &caller_id,
        "sts-caller",
    )
    .send()
    .await
    .unwrap();
    let body: Value = res.json().await.unwrap();
    let names: Vec<&str> = body["data"]["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["username"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"sts-target-alice"),
        "live user still matches: {names:?}"
    );
    assert!(
        !names.contains(&"sts-target-bob"),
        "soft-deleted user must be excluded: {names:?}"
    );

    // Query too short is rejected (prevents a full-directory dump via q=).
    let res = common::as_member(
        server.client.get(server.url("/api/mcp/share-targets?q=s")),
        &caller_id,
        "sts-caller",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 400);

    server.cleanup().await;
}

/// Configure `agent`'s override row for `cid` (as `member`), which is what makes
/// an agent a "consumer" of the connector.
async fn configure_agent_connector(
    server: &common::TestServer,
    member_id: &str,
    member_name: &str,
    agent: Uuid,
    cid: Uuid,
) {
    let res = common::as_member(
        server
            .client
            .put(server.url(&format!("/api/mcp/agents/{agent}/connectors/{cid}"))),
        member_id,
        member_name,
    )
    .json(&json!({"enabled": false}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        200,
        "configure {member_name}'s agent for the connector"
    );
}

/// Consumers = agents that have actually CONFIGURED this connector (have an
/// override row), regardless of how the owner reaches it. Crucially this stays
/// correct for a PUBLIC connector: a public-only user's configured agent still
/// appears (regression guard for the access_reasons-driven gap).
#[tokio::test]
#[serial]
async fn consumers_lists_only_agents_that_configured_the_connector() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "cons-owner").await;
    let (grantee_id, grantee_uuid) = create_user(&server, &admin, "cons-grantee").await;
    let (_stranger_id, stranger_uuid) = create_user(&server, &admin, "cons-stranger").await;
    let (public_id, public_uuid) = create_user(&server, &admin, "cons-public").await;

    let cid = seed_connector(&server, owner_uuid, "cons-tool").await;
    let owner_agent = seed_agent(&server, owner_uuid, "cons-owner-agent").await;
    let grantee_agent = seed_agent(&server, grantee_uuid, "cons-grantee-agent").await;
    let _stranger_agent = seed_agent(&server, stranger_uuid, "cons-stranger-agent").await;
    let public_agent = seed_agent(&server, public_uuid, "cons-public-agent").await;

    // Owner configures their own agent → a consumer.
    configure_agent_connector(&server, &owner_id, "cons-owner", owner_agent, cid).await;

    // Share directly with the grantee, who then configures their agent.
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/users/{grantee_uuid}"
        ))),
        &owner_id,
        "cons-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    configure_agent_connector(&server, &grantee_id, "cons-grantee", grantee_agent, cid).await;

    // Make the connector public; a public-ONLY user (no direct grant) configures
    // their agent — this is the case the old access_reasons path silently dropped.
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/grants/public"))),
        &owner_id,
        "cons-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);
    configure_agent_connector(&server, &public_id, "cons-public", public_agent, cid).await;

    let body: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/consumers"))),
        &owner_id,
        "cons-owner",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();

    let agent_ids: Vec<&str> = body["data"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["agent_id"].as_str().unwrap())
        .collect();
    assert!(
        agent_ids.contains(&owner_agent.to_string().as_str()),
        "{agent_ids:?}"
    );
    assert!(
        agent_ids.contains(&grantee_agent.to_string().as_str()),
        "{agent_ids:?}"
    );
    assert!(
        agent_ids.contains(&public_agent.to_string().as_str()),
        "public-only user's configured agent must appear: {agent_ids:?}"
    );
    assert_eq!(
        agent_ids.len(),
        3,
        "the unconfigured stranger agent must not appear: {agent_ids:?}"
    );

    // Direct-user grant (the "cons-grantee" share above) shows up as a consumer;
    // the public flag does NOT synthesize a "users" row (that's `is_public` on
    // the share endpoint, not a specific consumer here).
    let usernames: Vec<&str> = body["data"]["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["username"].as_str().unwrap())
        .collect();
    assert_eq!(usernames, vec!["cons-grantee"], "{usernames:?}");

    // OSS has no team/department concept — always empty via the authorizer seam.
    assert_eq!(body["data"]["teams"].as_array().unwrap().len(), 0);
    assert_eq!(body["data"]["departments"].as_array().unwrap().len(), 0);

    // A non-owner, non-admin caller must not be able to view consumers.
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/connectors/{cid}/consumers"))),
        &grantee_id,
        "cons-grantee",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

/// Sharing a connector directly with an AGENT (grant_type="agent") lets
/// whoever manages that agent configure it, even with zero personal
/// reachability to the connector otherwise — the gap this closes vs. the
/// existing user/team/department grant kinds. Covers the full lifecycle:
/// invisible before the grant, visible + configurable after, gone after
/// revoke, and a true stranger (zero reachability to the connector) may not
/// grant it. (Someone who merely has *some* reachability to the connector —
/// e.g. a user-share — but doesn't own it CAN grant-agent; see
/// [`connector_reachable_non_owner_can_grant_agent`] for that case.)
#[tokio::test]
#[serial]
async fn agent_grant_lets_owner_configure_connector_without_personal_reachability() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "ag-owner").await;
    let (agent_owner_id, agent_owner_uuid) = create_user(&server, &admin, "ag-agent-owner").await;
    let (stranger_id, _stranger_uuid) = create_user(&server, &admin, "ag-stranger").await;

    let cid = seed_connector(&server, owner_uuid, "ag-tool").await;
    let agent_id = seed_agent(&server, agent_owner_uuid, "ag-agent").await;

    // Before any grant: agent_owner has no reachability at all, so the
    // connector doesn't show up in their agent's connector list...
    let before: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        &agent_owner_id,
        "ag-agent-owner",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(
        !before["data"]["connectors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["connector_id"] == cid.to_string()),
        "connector must not be visible before any grant: {before:?}"
    );
    // ...and trying to configure it outright fails.
    let res = common::as_member(
        server
            .client
            .put(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}"))),
        &agent_owner_id,
        "ag-agent-owner",
    )
    .json(&json!({"enabled": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        404,
        "no reachability yet, configure must fail"
    );

    // A true stranger (zero reachability to the connector — not owner, not
    // admin, not shared with) cannot grant it to the agent. 404, not 403 —
    // matching every other Layer-1 reachability check in this file, which
    // hide existence rather than confirm it to someone with no access at all.
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &stranger_id,
        "ag-stranger",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        404,
        "a caller with zero reachability to the connector may not grant it"
    );

    // A nonexistent agent id is rejected, not silently accepted.
    let bogus_agent = Uuid::new_v4();
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{bogus_agent}"
        ))),
        &owner_id,
        "ag-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 404, "nonexistent agent id must be rejected");

    // Owner grants the connector directly to the agent.
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &owner_id,
        "ag-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        201,
        "owner sharing with the agent should succeed"
    );

    // Now it's visible in the agent's connector list...
    let after: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        &agent_owner_id,
        "ag-agent-owner",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(
        after["data"]["connectors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["connector_id"] == cid.to_string()),
        "connector must be visible after the agent grant: {after:?}"
    );
    // ...and the agent's own owner can now configure it, despite having no
    // personal grant on the connector themselves.
    let res = common::as_member(
        server
            .client
            .put(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}"))),
        &agent_owner_id,
        "ag-agent-owner",
    )
    .json(&json!({"enabled": true}))
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        200,
        "agent grant should unlock configuring it"
    );

    // Revoke: owner removes the agent grant.
    let res = common::as_member(
        server.client.delete(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &owner_id,
        "ag-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        200,
        "owner revoking the agent grant should succeed"
    );

    // Revoking twice reports "no matching share to revoke" (not idempotent-success).
    let res = common::as_member(
        server.client.delete(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &owner_id,
        "ag-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        404,
        "revoking an already-revoked grant must not silently succeed"
    );

    server.cleanup().await;
}

/// A caller who does NOT own a connector, but has been given reachability to
/// it via a plain user-share, can still grant it directly to an agent (theirs
/// or someone else's) — attaching a connector you can already use to an agent
/// is a narrower act than sharing it with a new person, so it's gated by
/// reachability, not ownership. It's still off by default: the grant alone
/// doesn't enable it, and revoking works the same way.
#[tokio::test]
#[serial]
async fn connector_reachable_non_owner_can_grant_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "rg-owner").await;
    let (sharee_id, sharee_uuid) = create_user(&server, &admin, "rg-sharee").await;

    let cid = seed_connector(&server, owner_uuid, "rg-tool").await;
    let agent_id = seed_agent(&server, sharee_uuid, "rg-agent").await;

    // Owner shares the connector with the sharee (a plain user-grant — not
    // ownership).
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/users/{sharee_uuid}"
        ))),
        &owner_id,
        "rg-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    // The sharee doesn't own the connector, but can still grant it to their
    // own agent.
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &sharee_id,
        "rg-sharee",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        201,
        "a non-owner with reachability to the connector should be able to grant it to an agent"
    );

    // The grant alone doesn't enable it — still needs an explicit enable.
    let after: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        &sharee_id,
        "rg-sharee",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(
        after["data"]["connectors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["connector_id"] == cid.to_string()),
        "connector must be visible after the agent grant: {after:?}"
    );

    // The same non-owner sharee can also revoke the grant they made.
    let res = common::as_member(
        server.client.delete(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &sharee_id,
        "rg-sharee",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        200,
        "a non-owner with reachability should also be able to revoke their own agent grant"
    );

    server.cleanup().await;
}

/// `GET /agents/{id}/connectors` used to be the one sibling endpoint still
/// hard-denying a connector owner who isn't the agent's manager, even though
/// `PUT .../connectors/{id}` (enable/configure), `GET .../connectors/{id}/tools`,
/// and `PUT .../tools` were all already relaxed (`a9012ded`/`56e46b07`) to let
/// exactly this caller act on their own granted connector. Without this, the
/// discovery step a caller would naturally reach for before those other calls
/// was undiscoverable to them — this proves it now matches its siblings:
/// narrowed to the connector(s) they can reach, not blocked outright.
#[tokio::test]
#[serial]
async fn list_connectors_lets_a_granted_connector_owner_see_their_own_entry() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "lc-owner").await;
    let (agent_owner_id, agent_owner_uuid) = create_user(&server, &admin, "lc-agent-owner").await;
    let (stranger_id, _stranger_uuid) = create_user(&server, &admin, "lc-stranger").await;

    let cid = seed_connector(&server, owner_uuid, "lc-tool").await;
    let agent_id = seed_agent(&server, agent_owner_uuid, "lc-agent").await;

    // Before any grant: the connector owner has no relationship to this agent
    // at all — must still be denied outright, not just filtered to empty.
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        &owner_id,
        "lc-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        403,
        "zero relationship to the agent must still be denied outright"
    );

    // Owner grants their connector to the agent.
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &owner_id,
        "lc-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    // Now the connector owner (still not the agent's manager) can see their
    // own granted connector in this list — the fix.
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        &owner_id,
        "lc-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    let connectors = body["data"]["connectors"].as_array().unwrap();
    assert!(
        connectors
            .iter()
            .any(|c| c["connector_id"] == cid.to_string()),
        "granted connector owner must see their own connector: {body:?}"
    );

    // A true stranger — no ownership, no grant, no agent management — still
    // gets 403, never the unfiltered list.
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        &stranger_id,
        "lc-stranger",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        403,
        "a caller with zero relationship to the agent must never see any connector"
    );

    // The agent's own manager (full `can_manage_agent`) still sees the
    // unfiltered list — the relaxation never narrows the manager's own view.
    let res = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        &agent_owner_id,
        "lc-agent-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    server.cleanup().await;
}

/// A repeat `grant-agent` call on an already-granted (agent, connector) pair
/// must NOT silently reset that agent's configured state for the connector —
/// `create_grant`'s own upsert never errors on a repeat grant, so without
/// this, every re-grant would force `enabled=true` and wipe any block/ask
/// rules, even ones the agent's manager deliberately set.
#[tokio::test]
#[serial]
async fn repeat_agent_grant_preserves_existing_enabled_and_rules() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "rg2-owner").await;

    let cid = seed_connector(&server, owner_uuid, "rg2-tool").await;
    let agent_id = seed_agent(&server, owner_uuid, "rg2-agent").await;

    // First grant — establishes the default enabled=true, no-rules row.
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &owner_id,
        "rg2-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    // Configure a restrictive state: disabled, with a block rule.
    let res = common::as_member(
        server
            .client
            .put(server.url(&format!("/api/mcp/agents/{agent_id}/connectors/{cid}"))),
        &owner_id,
        "rg2-owner",
    )
    .json(&json!({"enabled": false}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    let res = common::as_member(
        server
            .client
            .put(server.url(&format!("/api/mcp/agents/{agent_id}/tools"))),
        &owner_id,
        "rg2-owner",
    )
    .json(&json!({"rules": [{"connector_id": cid, "tool_pattern": "SEND_*", "stance": "block"}]}))
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 200);

    // Repeat the exact same grant-agent call.
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &owner_id,
        "rg2-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201, "a repeat grant must still succeed");

    // The block rule must have survived — raw stored rules, no live backend
    // sync needed (unlike the synced-tool-catalog `.../tools` view).
    let rules: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/agents/{agent_id}/tools"))),
        &owner_id,
        "rg2-owner",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let has_block_rule = rules["data"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["connector_id"] == cid.to_string() && r["stance"] == "block");
    assert!(
        has_block_rule,
        "a repeat grant must not silently wipe an existing block rule: {rules:?}"
    );

    // The disabled state must have survived too.
    let connectors: Value = common::as_member(
        server
            .client
            .get(server.url(&format!("/api/mcp/agents/{agent_id}/connectors"))),
        &owner_id,
        "rg2-owner",
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let enabled = connectors["data"]["connectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["connector_id"] == cid.to_string())
        .map(|c| c["enabled"].clone());
    assert_eq!(
        enabled,
        Some(json!(false)),
        "a repeat grant must not silently re-enable a connector that was disabled: {connectors:?}"
    );

    server.cleanup().await;
}

/// A non-owner reachable to a connector only via a PUBLIC grant — reachable
/// by literally every user — must not be able to attach it to an agent they
/// have no relationship to at all. Being able to reach the connector is not
/// enough; the caller must also manage the target agent (own it, or admin).
/// The connector's own owner is unaffected — still unrestricted about the
/// target agent, proven by the second half of this test.
#[tokio::test]
#[serial]
async fn public_connector_cannot_be_attached_to_an_unrelated_agent() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (owner_id, owner_uuid) = create_user(&server, &admin, "pc-owner").await;
    let (_agent_owner_id, agent_owner_uuid) = create_user(&server, &admin, "pc-agent-owner").await;
    let (stranger_id, _stranger_uuid) = create_user(&server, &admin, "pc-stranger").await;

    let cid = seed_connector(&server, owner_uuid, "pc-tool").await;
    let agent_id = seed_agent(&server, agent_owner_uuid, "pc-agent").await;

    // Make the connector public — reachable by literally everyone.
    let res = common::as_member(
        server
            .client
            .post(server.url(&format!("/api/mcp/connectors/{cid}/grants/public"))),
        &owner_id,
        "pc-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 201);

    // A stranger — who only reaches the connector because it's public, has
    // no relationship to `agent_id` at all — must be forbidden from
    // attaching it there.
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &stranger_id,
        "pc-stranger",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        403,
        "reachability via a public grant must not be enough to attach a connector to an unrelated agent"
    );

    // The connector's own owner is unaffected — still unrestricted, even
    // though they don't manage this agent either.
    let res = common::as_member(
        server.client.post(server.url(&format!(
            "/api/mcp/connectors/{cid}/grants/agents/{agent_id}"
        ))),
        &owner_id,
        "pc-owner",
    )
    .send()
    .await
    .unwrap();
    assert_eq!(
        res.status(),
        201,
        "the connector's own owner must still be able to attach it to any agent"
    );

    server.cleanup().await;
}
