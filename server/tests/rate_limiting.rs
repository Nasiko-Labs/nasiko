//! Regression test for Phase A5: gateway removal left this server with no
//! rate limiting anywhere. This proves the wiring (not just the isolated
//! `RateLimiter` unit) — a middleware ordering bug (e.g. layered before
//! `require_auth` populates `Claims`) wouldn't be caught by the unit tests
//! in `rate_limit.rs` alone.

mod common;

use serial_test::serial;

/// `/api/auth/login` is limited to 30 requests/60s (global bucket — see
/// `rate_limit::limit_globally`'s doc comment for why it's global rather
/// than per-caller). The 31st request in the window must be rejected
/// regardless of whether the credentials are valid.
#[tokio::test]
#[serial]
async fn login_route_is_rate_limited() {
    let server = common::TestServer::start().await;

    let mut saw_429 = false;
    for _ in 0..40 {
        let res = server
            .client
            .post(server.url("/api/auth/login"))
            .json(&serde_json::json!({"username": "nobody", "password": "wrong"}))
            .send()
            .await
            .unwrap();
        if res.status() == 429 {
            saw_429 = true;
            break;
        }
    }

    assert!(
        saw_429,
        "expected a 429 within 40 rapid login attempts (limit is 30/60s)"
    );
    server.cleanup().await;
}
