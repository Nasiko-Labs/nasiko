//! Custom providers: any signed-in user may register one, but only its creator or a
//! superuser may change it, and only a superuser may point one at a private address
//! (the server calls the endpoint and relays what it answers). A member's provider is
//! private to them, since a call through it spends their key; a superuser's is shared.
//!
//!   cargo test -p nasiko-server --test custom_provider_access -- --test-threads=1
mod common;

use reqwest::StatusCode;
use serde_json::{Value, json};
use serial_test::serial;
use uuid::Uuid;

async fn user(server: &common::TestServer, superuser: bool) -> String {
    let name = format!("cp-{}", Uuid::new_v4().simple());
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (username, email, is_superuser) \
         VALUES ($1, $1 || '@test.local', $2) RETURNING id",
    )
    .bind(&name)
    .bind(superuser)
    .fetch_one(&server.db)
    .await
    .unwrap();
    id.to_string()
}

/// A provider row owned by `owner` with one served model, inserted directly so no test
/// calls out. Returns its id and label.
async fn provider(server: &common::TestServer, owner: &str) -> (Uuid, String) {
    let label = format!("cp-{}", &Uuid::new_v4().simple().to_string()[..8]);
    let id = sqlx::query_scalar(
        "INSERT INTO custom_providers (label, display_name, base_url, encrypted_api_key, created_by) \
         VALUES ($1, $1, 'https://llm.example.com/v1', 'x', $2::uuid) RETURNING id",
    )
    .bind(&label)
    .bind(owner)
    .fetch_one(&server.db)
    .await
    .unwrap();
    sqlx::query("INSERT INTO provider_models (provider, model) VALUES ($1, 'm')")
        .bind(&label)
        .execute(&server.db)
        .await
        .unwrap();
    (id, label)
}

/// The `key` of every row a GET of `path` lists for `user`.
async fn seen(server: &common::TestServer, user: &str, path: &str, key: &str) -> Vec<String> {
    let rb = common::as_member(server.client.get(server.url(path)), user, "u");
    let body: Value = rb.send().await.unwrap().json().await.unwrap();
    body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r[key].as_str().map(str::to_owned))
        .collect()
}

async fn send(rb: reqwest::RequestBuilder, body: Option<&Value>) -> StatusCode {
    let rb = match body {
        Some(b) => rb.json(b),
        None => rb,
    };
    rb.send().await.unwrap().status()
}

#[tokio::test]
#[serial]
async fn only_its_creator_or_a_superuser_changes_a_provider() {
    let server = common::TestServer::start().await;
    let (owner, other, admin) = (
        user(&server, false).await,
        user(&server, false).await,
        user(&server, true).await,
    );
    let url = server.url(&format!(
        "/api/custom-providers/{}",
        provider(&server, &owner).await.0
    ));
    let rename = json!({ "display_name": "Renamed" });
    let c = &server.client;

    let as_other = |rb| common::as_member(rb, &other, "other");
    assert_eq!(
        send(as_other(c.patch(&url)), Some(&rename)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(as_other(c.post(format!("{url}/sync"))), None).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(as_other(c.delete(&url)), None).await,
        StatusCode::FORBIDDEN
    );

    let as_owner = common::as_member(c.patch(&url), &owner, "owner");
    assert_eq!(send(as_owner, Some(&rename)).await, StatusCode::OK);
    let as_admin = common::as_superuser(c.patch(&url), &admin, "admin");
    assert_eq!(send(as_admin, Some(&rename)).await, StatusCode::OK);
}

#[tokio::test]
#[serial]
async fn a_member_cannot_point_a_provider_at_a_private_address() {
    let server = common::TestServer::start().await;
    let member = user(&server, false).await;
    let c = &server.client;
    let private = json!({
        "display_name": "Internal",
        "base_url": "http://169.254.169.254/v1",
        "api_key": "k",
    });

    for path in ["/api/custom-providers", "/api/custom-providers/test"] {
        let rb = common::as_member(c.post(server.url(path)), &member, "member");
        assert_eq!(
            send(rb, Some(&private)).await,
            StatusCode::BAD_REQUEST,
            "{path}"
        );
    }
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM custom_providers WHERE created_by = $1::uuid")
            .bind(&member)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(stored, 0);

    let (own, _) = provider(&server, &member).await;
    let url = server.url(&format!("/api/custom-providers/{own}"));
    let repoint = json!({ "base_url": "http://127.0.0.1:5432" });
    let rb = common::as_member(c.patch(&url), &member, "member");
    assert_eq!(send(rb, Some(&repoint)).await, StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial]
async fn a_provider_is_its_creators_alone() {
    let server = common::TestServer::start().await;
    let (owner, other, admin, admin2) = (
        user(&server, false).await,
        user(&server, false).await,
        user(&server, true).await,
        user(&server, true).await,
    );
    let (mine, mine_label) = provider(&server, &owner).await;
    let (admins, admins_label) = provider(&server, &admin).await;

    for (path, key) in [
        ("/api/custom-providers", "label"),
        ("/api/llm-router/providers", "provider"),
    ] {
        let owners = seen(&server, &owner, path, key).await;
        assert!(
            owners.contains(&mine_label) && !owners.contains(&admins_label),
            "{path}"
        );
        let others = seen(&server, &other, path, key).await;
        assert!(
            !others.contains(&mine_label) && !others.contains(&admins_label),
            "{path}"
        );
        // Superusers share theirs, so the platform's agents run on any of them.
        let admin2s = seen(&server, &admin2, path, key).await;
        assert!(
            !admin2s.contains(&mine_label) && admin2s.contains(&admins_label),
            "{path}"
        );
    }

    let config = |who: &str, provider: String| {
        common::as_member(server.client.post(server.url("/api/llm-configs")), who, "u").json(
            &json!({ "name": format!("c-{}", Uuid::new_v4().simple()), "provider": provider, "model": "m" }),
        )
    };
    let status = |rb: reqwest::RequestBuilder| async move { rb.send().await.unwrap().status() };
    for named in [
        mine.to_string(),
        mine_label.clone(),
        admins.to_string(),
        admins_label.clone(),
        // A name no one has registered yet: once someone does, the router would find
        // their provider by it.
        "not-registered-yet".to_string(),
    ] {
        assert_eq!(
            status(config(&other, named.clone())).await,
            StatusCode::BAD_REQUEST,
            "{named}"
        );
    }
    // Its own provider, by id in either case or by label.
    for named in [
        mine.to_string(),
        mine.to_string().to_uppercase(),
        mine_label,
    ] {
        assert!(
            status(config(&owner, named.clone())).await.is_success(),
            "{named}"
        );
    }
    let as_admin2 = common::as_superuser(
        server.client.post(server.url("/api/llm-configs")),
        &admin2,
        "a2",
    )
    .json(&json!({ "name": "c-shared", "provider": admins.to_string(), "model": "m" }));
    assert!(status(as_admin2).await.is_success());
}

#[tokio::test]
#[serial]
async fn a_provider_never_takes_a_built_in_name() {
    // Its catalog sync would otherwise replace the built-in's model list for everyone.
    let server = common::TestServer::start().await;
    let admin = user(&server, true).await;
    for name in ["OpenAI", "OpenRouter", "Vertex"] {
        let rb = common::as_superuser(
            server.client.post(server.url("/api/custom-providers")),
            &admin,
            "admin",
        );
        let body =
            json!({ "display_name": name, "base_url": "http://127.0.0.1:9/v1", "api_key": "k" });
        assert!(send(rb, Some(&body)).await.is_success(), "{name}");
        let label: String = sqlx::query_scalar(
            "SELECT label FROM custom_providers WHERE display_name = $1 AND created_by = $2::uuid",
        )
        .bind(name)
        .bind(&admin)
        .fetch_one(&server.db)
        .await
        .unwrap();
        assert_eq!(label, format!("{}-custom", name.to_lowercase()));
    }
}

const JWT_SECRET: &str = "custom-provider-access-secret";

/// A stub chat endpoint the router can reach; it answers every call it gets.
async fn upstream() -> mockito::ServerGuard {
    let mut upstream = mockito::Server::new_async().await;
    upstream
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":"c1","object":"chat.completion","created":1,"model":"m",
                "choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#,
        )
        .expect_at_least(0)
        .create_async()
        .await;
    upstream
}

/// A provider row owned by `owner` pointing at `base_url`, with a key the router can decrypt.
async fn provider_at(server: &common::TestServer, owner: &str, base_url: &str) -> String {
    let label = format!("cp-{}", &Uuid::new_v4().simple().to_string()[..8]);
    sqlx::query(
        "INSERT INTO custom_providers (label, display_name, base_url, encrypted_api_key, created_by) \
         VALUES ($1, $1, $2, $3, $4::uuid)",
    )
    .bind(&label)
    .bind(base_url)
    .bind(nasiko_secrets::SecretsCrypto::for_platform_settings().encrypt("sk-test"))
    .bind(owner)
    .execute(&server.db)
    .await
    .unwrap();
    label
}

/// An agent owned by `owner`, attached to an `owner`'s config that names `provider`,
/// written straight to the database as a config saved before these rules would be.
/// Returns the status of one chat call the agent makes as its owner.
async fn chat_through(server: &common::TestServer, owner: &str, provider: &str) -> u16 {
    let owner: Uuid = owner.parse().unwrap();
    let config: Uuid = sqlx::query_scalar(
        "INSERT INTO llm_configs (created_by, name, provider, model) \
         VALUES ($1, $2, $3, 'm') RETURNING id",
    )
    .bind(owner)
    .bind(format!("c-{}", Uuid::new_v4().simple()))
    .bind(provider)
    .fetch_one(&server.db)
    .await
    .unwrap();
    let agent: Uuid = sqlx::query_scalar(
        "INSERT INTO agents (name, owner_id, image, status, llm_config_id) \
         VALUES ($1, $2, 'nasiko/echo:1.0.0', 'running', $3) RETURNING id",
    )
    .bind(format!("dummy-cp-{}", Uuid::new_v4().simple()))
    .bind(owner)
    .bind(config)
    .fetch_one(&server.db)
    .await
    .unwrap();
    let (_, traceparent) = common::open_flow(&server.db, owner, agent).await;
    let token = nasiko_llm_router::auth::mint_agent_token(
        &agent.to_string(),
        &owner.to_string(),
        JWT_SECRET,
        3600,
        jsonwebtoken::Algorithm::HS256,
    )
    .unwrap();
    let resp = server
        .client
        .post(server.url("/v1/chat/completions"))
        .bearer_auth(token)
        .header("traceparent", traceparent)
        .json(&json!({ "model": "m", "messages": [{ "role": "user", "content": "hi" }] }))
        .send()
        .await
        .unwrap();
    resp.status().as_u16()
}

#[tokio::test]
#[serial]
async fn the_router_runs_a_config_only_on_its_owners_providers() {
    // SAFETY: serialized by #[serial] within this test binary.
    unsafe { std::env::set_var("AGENT_JWT_SECRET", JWT_SECRET) };
    let server = common::TestServer::start().await;
    let stub = upstream().await;
    let base = format!("{}/v1", stub.url());
    let (member, admin, admin2) = (
        user(&server, false).await,
        user(&server, true).await,
        user(&server, true).await,
    );
    let admins = provider_at(&server, &admin, &base).await;

    // A seed agent: a superuser's agent on a superuser's config reaches a provider any
    // superuser registered, at a private address too.
    assert_eq!(chat_through(&server, &admin, &admins).await, 200);
    assert_eq!(chat_through(&server, &admin2, &admins).await, 200);
    // A member's config naming someone else's provider is refused, not run on their key.
    assert_eq!(chat_through(&server, &member, &admins).await, 400);
    let members = provider_at(&server, &member, &base).await;
    assert_eq!(chat_through(&server, &admin, &members).await, 400);

    // A member's own provider is reached only through the public-only client, which
    // refuses a host that resolves to a private address.
    let local = base.replace("127.0.0.1", "localhost");
    let members_local = provider_at(&server, &member, &local).await;
    assert_ne!(chat_through(&server, &member, &members_local).await, 200);
    // The stub answers on that name to an unguarded client, so the refusal is the guard's.
    let direct = reqwest::Client::new()
        .post(format!("{local}/chat/completions"))
        .send()
        .await
        .unwrap();
    assert!(direct.status().is_success());

    server.cleanup().await;
}
