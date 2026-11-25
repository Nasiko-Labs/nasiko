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
async fn a_members_provider_is_private_to_them() {
    let server = common::TestServer::start().await;
    let (owner, other, admin) = (
        user(&server, false).await,
        user(&server, false).await,
        user(&server, true).await,
    );
    let (mine, mine_label) = provider(&server, &owner).await;
    let (shared, shared_label) = provider(&server, &admin).await;

    for (path, key) in [
        ("/api/custom-providers", "label"),
        ("/api/llm-router/providers", "provider"),
    ] {
        let owners = seen(&server, &owner, path, key).await;
        assert!(
            owners.contains(&mine_label) && owners.contains(&shared_label),
            "{path}"
        );
        let others = seen(&server, &other, path, key).await;
        assert!(
            !others.contains(&mine_label) && others.contains(&shared_label),
            "{path}"
        );
    }

    let config = |who: &str, provider: String| {
        common::as_member(server.client.post(server.url("/api/llm-configs")), who, "u").json(
            &json!({ "name": format!("c-{}", Uuid::new_v4().simple()), "provider": provider, "model": "m" }),
        )
    };
    for named in [mine.to_string(), mine_label.clone()] {
        let status = config(&other, named.clone()).send().await.unwrap().status();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{named}");
    }
    assert!(
        config(&owner, mine.to_string())
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(
        config(&other, shared.to_string())
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
}
