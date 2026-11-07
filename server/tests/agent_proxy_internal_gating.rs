mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{Json, Router, extract::State, routing::post};
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

type Recorded = Arc<Mutex<Vec<Value>>>;

const MEMORY_URI: &str = "urn:nasiko:a2a:session-memory";

async fn init_admin(server: &common::TestServer) -> Uuid {
    let v = server
        .client
        .post(server.url("/api/auth/initialize-admin"))
        .json(&json!({"username": "admin", "email": "admin@test.local"}))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    Uuid::parse_str(v["user_id"].as_str().unwrap()).unwrap()
}

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

async fn seed_agent(
    server: &common::TestServer,
    owner: Uuid,
    name: &str,
    url: &str,
    is_internal: bool,
    capabilities: Option<Value>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, url, is_public, is_internal, capabilities) \
         VALUES ($1, $2, 'x:1.0.0', 'running', $3, true, $4, $5) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(url)
    .bind(is_internal)
    .bind(capabilities.unwrap_or_else(|| json!({})))
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn record(State(rec): State<Recorded>, Json(body): Json<Value>) -> Json<Value> {
    rec.lock().unwrap().push(body);
    Json(json!({
        "jsonrpc": "2.0", "id": "1",
        "result": {"kind": "task", "status": {"state": "completed"}, "artifacts": [{"parts": [{"kind": "text", "text": "stub reply"}]}]}
    }))
}

async fn start_agent() -> (String, Recorded) {
    let rec: Recorded = Arc::default();
    let app = Router::new()
        .route("/", post(record))
        .with_state(rec.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{port}"), rec)
}

fn rpc(context_id: Option<&str>, text: &str, params_metadata: Option<Value>) -> Value {
    let mut message = json!({
        "messageId": Uuid::new_v4().to_string(),
        "role": "user",
        "parts": [{"kind": "text", "text": text}]
    });
    if let Some(c) = context_id {
        message["contextId"] = json!(c);
    }
    let mut params = json!({"message": message});
    if let Some(m) = params_metadata {
        params["metadata"] = m;
    }
    json!({"jsonrpc": "2.0", "method": "message/send", "id": "1", "params": params})
}

async fn post_as(
    server: &common::TestServer,
    agent: Uuid,
    user: Uuid,
    superuser: bool,
    body: &Value,
) -> reqwest::Response {
    let rb = server
        .client
        .post(server.url(&format!("/api/agents/{agent}")));
    let rb = if superuser {
        common::as_superuser(rb, &user.to_string(), "admin")
    } else {
        common::as_member(rb, &user.to_string(), "member")
    };
    rb.json(body).send().await.unwrap()
}

async fn count(server: &common::TestServer, sql: &str, session: &str) -> i64 {
    sqlx::query_scalar(sql)
        .bind(session)
        .fetch_one(&server.db)
        .await
        .unwrap()
}

async fn sessions(server: &common::TestServer, id: &str) -> i64 {
    count(
        server,
        "SELECT COUNT(*) FROM chat_sessions WHERE session_id = $1",
        id,
    )
    .await
}

async fn messages(server: &common::TestServer, id: &str) -> i64 {
    count(
        server,
        "SELECT COUNT(*) FROM chat_messages WHERE session_id = $1",
        id,
    )
    .await
}

async fn traces(server: &common::TestServer, id: &str) -> i64 {
    count(
        server,
        "SELECT COUNT(*) FROM flows WHERE session_id = $1",
        id,
    )
    .await
}

async fn settle_until(server: &common::TestServer, id: &str, want_messages: i64) {
    for _ in 0..100 {
        if messages(server, id).await >= want_messages {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("chat_messages for {id} never reached {want_messages}");
}

async fn settle_traces(server: &common::TestServer, id: &str) {
    for _ in 0..100 {
        if traces(server, id).await >= 1 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn quiet() {
    tokio::time::sleep(Duration::from_millis(600)).await;
}

fn forwarded_text(rec: &Recorded, index: usize) -> String {
    rec.lock().unwrap()[index]["params"]["message"]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

fn ephemeral() -> Option<Value> {
    Some(json!({"nasiko_ephemeral": true}))
}

fn memory_capabilities() -> Value {
    json!({"extensions": [{"uri": MEMORY_URI}]})
}

fn session_id() -> String {
    format!("ses_{}", Uuid::new_v4().simple())
}

#[tokio::test]
#[serial]
async fn a_normal_agent_ignores_the_ephemeral_flag_and_persists_everything() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "normal-eph", &url, false, None).await;
    let ctx = session_id();

    let res = post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "hello normal", ephemeral()),
    )
    .await;
    assert!(res.status().is_success(), "{}", res.status());
    settle_until(&server, &ctx, 2).await;
    settle_traces(&server, &ctx).await;

    assert_eq!(sessions(&server, &ctx).await, 1);
    let user_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM chat_messages WHERE session_id = $1 AND role = 'user' AND content = 'hello normal'",
    )
    .bind(&ctx)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(user_rows, 1);
    assert_eq!(traces(&server, &ctx).await, 1);
    assert_eq!(rec.lock().unwrap().len(), 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_normal_agent_without_a_context_gets_one_minted_even_when_flagged_ephemeral() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "normal-eph-noctx", &url, false, None).await;

    let res = post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(None, "no context", ephemeral()),
    )
    .await;
    assert!(res.status().is_success());

    let forwarded = rec.lock().unwrap()[0].clone();
    assert!(
        forwarded["params"]["message"]["contextId"].is_string(),
        "normal agents must still get a generated contextId: {forwarded}"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn a_normal_agent_listing_the_memory_extension_still_gets_history_stitched() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(
        &server,
        admin,
        "normal-ext",
        &url,
        false,
        Some(memory_capabilities()),
    )
    .await;
    let ctx = session_id();

    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "first turn alpha", None),
    )
    .await;
    settle_until(&server, &ctx, 2).await;
    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "second turn beta", None),
    )
    .await;

    let second = forwarded_text(&rec, 1);
    assert!(
        second.contains("first turn alpha"),
        "history missing: {second}"
    );
    assert!(
        second.contains("second turn beta"),
        "current query missing: {second}"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_ephemeral_call_leaves_no_rows_and_forwards_the_body_unchanged() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "internal-eph", &url, true, None).await;
    let ctx = session_id();
    let body = rpc(Some(&ctx), "secret dashboard prompt", ephemeral());

    let res = post_as(&server, agent, admin, true, &body).await;
    assert!(res.status().is_success());
    quiet().await;

    assert_eq!(sessions(&server, &ctx).await, 0);
    assert_eq!(messages(&server, &ctx).await, 0);
    assert_eq!(traces(&server, &ctx).await, 0);
    assert_eq!(rec.lock().unwrap()[0], body);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_ephemeral_call_without_a_context_gets_none_injected() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "internal-eph-noctx", &url, true, None).await;
    let body = rpc(None, "no ctx", ephemeral());

    post_as(&server, agent, admin, true, &body).await;
    quiet().await;

    assert_eq!(rec.lock().unwrap()[0], body);
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_sessions WHERE agent_id = $1")
        .bind(agent)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(total, 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_ephemeral_call_never_stitches_history_of_an_existing_session() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "internal-eph-nostitch", &url, true, None).await;
    let ctx = session_id();

    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "persisted earlier", None),
    )
    .await;
    settle_until(&server, &ctx, 2).await;
    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "ephemeral now", ephemeral()),
    )
    .await;
    quiet().await;

    assert_eq!(forwarded_text(&rec, 1), "ephemeral now");
    assert_eq!(messages(&server, &ctx).await, 2);

    server.cleanup().await;
}

async fn seed_foreign_session(server: &common::TestServer, owner: Uuid, agent: Uuid) -> String {
    let id = session_id();
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title) VALUES ($1, $2, $3, '/x', 't')",
    )
    .bind(&id)
    .bind(owner)
    .bind(agent)
    .execute(&server.db)
    .await
    .unwrap();
    id
}

#[tokio::test]
#[serial]
async fn an_internal_ephemeral_call_on_another_users_session_is_forbidden_and_not_forwarded() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let alice = seed_user(&server, "alice").await;
    let bob = seed_user(&server, "bob").await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "internal-403", &url, true, None).await;
    let ctx = seed_foreign_session(&server, alice, agent).await;

    let res = post_as(
        &server,
        agent,
        bob,
        false,
        &rpc(Some(&ctx), "peek", ephemeral()),
    )
    .await;

    assert_eq!(res.status(), 403);
    assert!(rec.lock().unwrap().is_empty());

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_ephemeral_call_on_the_callers_own_session_is_allowed() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let alice = seed_user(&server, "alice2").await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "internal-own", &url, true, None).await;
    let ctx = seed_foreign_session(&server, alice, agent).await;

    let res = post_as(
        &server,
        agent,
        alice,
        false,
        &rpc(Some(&ctx), "mine", ephemeral()),
    )
    .await;

    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(rec.lock().unwrap().len(), 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_ephemeral_call_by_a_superuser_on_any_session_is_allowed() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let alice = seed_user(&server, "alice3").await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "internal-su", &url, true, None).await;
    let ctx = seed_foreign_session(&server, alice, agent).await;

    let res = post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "su", ephemeral()),
    )
    .await;

    assert!(res.status().is_success(), "{}", res.status());
    assert_eq!(rec.lock().unwrap().len(), 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_ephemeral_call_on_a_session_bound_to_another_agent_still_checks_ownership() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let alice = seed_user(&server, "alice4").await;
    let bob = seed_user(&server, "bob4").await;
    let (url, _rec) = start_agent().await;
    let other = seed_agent(&server, admin, "other-agent", &url, false, None).await;
    let agent = seed_agent(&server, admin, "internal-cross", &url, true, None).await;
    let ctx = seed_foreign_session(&server, alice, other).await;

    let res = post_as(
        &server,
        agent,
        bob,
        false,
        &rpc(Some(&ctx), "peek", ephemeral()),
    )
    .await;

    assert_eq!(res.status(), 403);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_ephemeral_call_with_an_unknown_context_is_allowed_and_creates_nothing() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let bob = seed_user(&server, "bob5").await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "internal-unknown", &url, true, None).await;
    let ctx = session_id();

    let res = post_as(
        &server,
        agent,
        bob,
        false,
        &rpc(Some(&ctx), "fresh", ephemeral()),
    )
    .await;
    quiet().await;

    assert!(res.status().is_success());
    assert_eq!(rec.lock().unwrap().len(), 1);
    assert_eq!(sessions(&server, &ctx).await, 0);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_ephemeral_call_with_an_empty_context_is_treated_as_having_none() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let bob = seed_user(&server, "bob6").await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "internal-emptyctx", &url, true, None).await;
    let body = rpc(Some(""), "empty ctx", ephemeral());

    let res = post_as(&server, agent, bob, false, &body).await;

    assert!(res.status().is_success());
    assert_eq!(rec.lock().unwrap()[0], body);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_agent_with_the_memory_extension_is_not_stitched_on_the_second_turn() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(
        &server,
        admin,
        "internal-mem",
        &url,
        true,
        Some(memory_capabilities()),
    )
    .await;
    let ctx = session_id();

    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "turn one", None),
    )
    .await;
    settle_until(&server, &ctx, 2).await;
    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "turn two", None),
    )
    .await;

    assert_eq!(forwarded_text(&rec, 1), "turn two");
    assert_eq!(sessions(&server, &ctx).await, 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_agent_without_the_memory_extension_is_stitched_on_the_second_turn() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, "internal-nomem", &url, true, None).await;
    let ctx = session_id();

    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "turn one", None),
    )
    .await;
    settle_until(&server, &ctx, 2).await;
    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "turn two", None),
    )
    .await;

    assert!(forwarded_text(&rec, 1).contains("turn one"));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_agent_whose_extensions_list_other_uris_is_stitched() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let caps =
        json!({"extensions": [{"uri": "urn:other"}, {"uri": "urn:nasiko:a2a:session-memory-v2"}]});
    let agent = seed_agent(&server, admin, "internal-otherext", &url, true, Some(caps)).await;
    let ctx = session_id();

    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "turn one", None),
    )
    .await;
    settle_until(&server, &ctx, 2).await;
    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "turn two", None),
    )
    .await;

    assert!(forwarded_text(&rec, 1).contains("turn one"));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_agent_with_malformed_extensions_is_stitched_instead_of_failing() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let caps = json!({"extensions": "urn:nasiko:a2a:session-memory"});
    let agent = seed_agent(&server, admin, "internal-badext", &url, true, Some(caps)).await;
    let ctx = session_id();

    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "turn one", None),
    )
    .await;
    settle_until(&server, &ctx, 2).await;
    let res = post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "turn two", None),
    )
    .await;

    assert!(res.status().is_success(), "{}", res.status());
    assert!(forwarded_text(&rec, 1).contains("turn one"));

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn an_internal_agent_with_json_null_capabilities_is_stitched_instead_of_failing() {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(
        &server,
        admin,
        "internal-nullcaps",
        &url,
        true,
        Some(Value::Null),
    )
    .await;
    let ctx = session_id();

    post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "turn one", None),
    )
    .await;
    settle_until(&server, &ctx, 2).await;
    let res = post_as(
        &server,
        agent,
        admin,
        true,
        &rpc(Some(&ctx), "turn two", None),
    )
    .await;

    assert!(res.status().is_success(), "{}", res.status());
    assert!(forwarded_text(&rec, 1).contains("turn one"));

    server.cleanup().await;
}

async fn assert_not_ephemeral(
    name: &str,
    params_metadata: Option<Value>,
    message_metadata: Option<Value>,
) {
    let server = common::TestServer::start().await;
    let admin = init_admin(&server).await;
    let (url, rec) = start_agent().await;
    let agent = seed_agent(&server, admin, name, &url, true, None).await;
    let ctx = session_id();
    let mut body = rpc(Some(&ctx), "should persist", params_metadata);
    if let Some(m) = message_metadata {
        body["params"]["message"]["metadata"] = m;
    }

    let res = post_as(&server, agent, admin, true, &body).await;
    assert!(res.status().is_success());
    settle_until(&server, &ctx, 1).await;

    assert_eq!(
        sessions(&server, &ctx).await,
        1,
        "{name}: session must be created"
    );
    assert_eq!(rec.lock().unwrap().len(), 1);

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn the_string_true_is_not_ephemeral() {
    assert_not_ephemeral(
        "eph-string",
        Some(json!({"nasiko_ephemeral": "true"})),
        None,
    )
    .await;
}

#[tokio::test]
#[serial]
async fn false_is_not_ephemeral() {
    assert_not_ephemeral("eph-false", Some(json!({"nasiko_ephemeral": false})), None).await;
}

#[tokio::test]
#[serial]
async fn the_number_one_is_not_ephemeral() {
    assert_not_ephemeral("eph-one", Some(json!({"nasiko_ephemeral": 1})), None).await;
}

#[tokio::test]
#[serial]
async fn null_is_not_ephemeral() {
    assert_not_ephemeral("eph-null", Some(json!({"nasiko_ephemeral": null})), None).await;
}

#[tokio::test]
#[serial]
async fn missing_metadata_is_not_ephemeral() {
    assert_not_ephemeral("eph-missing", None, None).await;
}

#[tokio::test]
#[serial]
async fn metadata_without_the_key_is_not_ephemeral() {
    assert_not_ephemeral("eph-otherkey", Some(json!({"other": true})), None).await;
}

#[tokio::test]
#[serial]
async fn the_flag_inside_message_metadata_is_not_ephemeral() {
    assert_not_ephemeral("eph-msgmeta", None, Some(json!({"nasiko_ephemeral": true}))).await;
}

#[tokio::test]
#[serial]
async fn metadata_that_is_not_an_object_is_not_ephemeral() {
    assert_not_ephemeral("eph-arraymeta", Some(json!([true])), None).await;
}

#[tokio::test]
#[serial]
async fn a_differently_cased_key_is_not_ephemeral() {
    assert_not_ephemeral("eph-case", Some(json!({"Nasiko_Ephemeral": true})), None).await;
}
