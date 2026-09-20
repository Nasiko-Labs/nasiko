//! HTTP-level tests for the public Composio webhook route: fail-closed without a
//! secret, signature enforcement, and the valid-signature path.
//!
//!   cargo test -p nasiko-server --test mcp_webhooks -- --test-threads=1

mod common;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, Mac};
use serde_json::json;
use serial_test::serial;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const SECRET: &str = "whsec_test_secret";

fn set_secret() {
    // SAFETY: serialized by #[serial].
    unsafe { std::env::set_var("COMPOSIO_WEBHOOK_SECRET", SECRET) };
}
fn clear_secret() {
    // SAFETY: serialized by #[serial].
    unsafe { std::env::remove_var("COMPOSIO_WEBHOOK_SECRET") };
}

fn sign(id: &str, ts: &str, body: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(format!("{id}.{ts}.{body}").as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

#[tokio::test]
#[serial]
async fn webhook_without_secret_configured_is_service_unavailable() {
    clear_secret();
    let server = common::TestServer::start().await;

    let res = server
        .client
        .post(server.url("/api/mcp/webhooks/composio"))
        .json(&json!({"type": "composio.connected_account.expired"}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        503,
        "must fail closed when no secret is configured"
    );

    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn webhook_missing_headers_is_unauthorized() {
    set_secret();
    let server = common::TestServer::start().await;

    let res = server
        .client
        .post(server.url("/api/mcp/webhooks/composio"))
        .json(&json!({"type": "composio.connected_account.expired"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    clear_secret();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn webhook_bad_signature_is_unauthorized() {
    set_secret();
    let server = common::TestServer::start().await;
    let body = json!({"type": "composio.connected_account.expired"}).to_string();

    let res = server
        .client
        .post(server.url("/api/mcp/webhooks/composio"))
        .header("webhook-id", "wh_1")
        .header("webhook-timestamp", "1700000000")
        .header("webhook-signature", "not-a-valid-sig")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    clear_secret();
    server.cleanup().await;
}

#[tokio::test]
#[serial]
async fn webhook_valid_signature_unknown_account_is_ok() {
    set_secret();
    let server = common::TestServer::start().await;
    let (id, ts) = ("wh_2", "1700000001");
    let body = json!({"type": "composio.connected_account.expired", "data": {"id": "ca_unknown"}})
        .to_string();
    let sig = sign(id, ts, &body);

    let res = server
        .client
        .post(server.url("/api/mcp/webhooks/composio"))
        .header("webhook-id", id)
        .header("webhook-timestamp", ts)
        .header("webhook-signature", sig)
        .body(body)
        .send()
        .await
        .unwrap();
    // Valid signature accepted; unknown account is a no-op → 200 ok.
    assert_eq!(res.status(), 200);

    clear_secret();
    server.cleanup().await;
}
