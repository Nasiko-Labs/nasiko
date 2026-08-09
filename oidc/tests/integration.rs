//! HTTP-level integration tests for `OidcClient`, run entirely against a
//! mockito mock server standing in for an OIDC IdP — no real Entra tenant or
//! external network access required. This is deliberately the same "runs
//! with zero external credentials" bar as `oss/github/tests/integration.rs`.

mod support;

use nasiko_oidc::{OidcClient, OidcConfig};

const AUD: &str = "test-client-id";
const NONCE: &str = "test-nonce-value";

fn test_config(issuer_url: String) -> OidcConfig {
    OidcConfig {
        issuer_url,
        client_id: AUD.to_string(),
        client_secret: "test-client-secret".to_string(),
        redirect_uri: "https://app.example.com/api/auth/oidc/callback".to_string(),
        central_callback_url: None,
        scopes: "openid profile email".to_string(),
    }
}

/// The fleet-relay override: when `central_callback_url` is set, it — not
/// `redirect_uri` — is the `redirect_uri` advertised to the IdP (and, by the
/// same helper, sent on token exchange). This is what lets many workspace CPs
/// share one OIDC app whose single registered callback points at the relay.
#[tokio::test]
async fn central_callback_url_overrides_redirect_uri_in_authorize() {
    let mut server = mockito::Server::new_async().await;
    let issuer = server.url();
    let _discovery = server
        .mock("GET", "/.well-known/openid-configuration")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            support::discovery_json(
                &issuer,
                &format!("{issuer}/authorize"),
                &format!("{issuer}/token"),
                &format!("{issuer}/jwks"),
            )
            .to_string(),
        )
        .create_async()
        .await;

    let central = "https://nasiko.dev/auth/oidc/callback/11111111-1111-1111-1111-111111111111";
    let mut config = test_config(issuer.clone());
    config.central_callback_url = Some(central.to_string());
    let client = OidcClient::new(config, reqwest::Client::new());

    let url = client
        .authorization_url("state-1", "nonce-1", "challenge-1")
        .await
        .expect("authorization_url should succeed");

    let parsed = reqwest::Url::parse(&url).unwrap();
    let redirect = parsed
        .query_pairs()
        .find(|(k, _)| k == "redirect_uri")
        .map(|(_, v)| v.to_string());
    assert_eq!(
        redirect.as_deref(),
        Some(central),
        "central_callback_url must be advertised as the redirect_uri"
    );
}

#[tokio::test]
async fn discovery_and_jwks_are_cached_across_repeated_calls() {
    let mut server = mockito::Server::new_async().await;
    let issuer = server.url();

    let discovery_mock = server
        .mock("GET", "/.well-known/openid-configuration")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            support::discovery_json(
                &issuer,
                &format!("{issuer}/authorize"),
                &format!("{issuer}/token"),
                &format!("{issuer}/jwks"),
            )
            .to_string(),
        )
        .expect(1)
        .create_async()
        .await;

    let jwks_mock = server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(support::jwks_json_with_test_key().to_string())
        .expect(1)
        .create_async()
        .await;

    let client = OidcClient::new(test_config(issuer.clone()), reqwest::Client::new());

    // First call triggers a discovery fetch.
    client
        .authorization_url("state-1", "nonce-1", "challenge-1")
        .await
        .expect("authorization_url should succeed");

    let claims = support::valid_claims(&issuer, AUD, NONCE);
    let token = support::sign_id_token(&claims, support::TEST_KID);

    // Triggers a JWKS fetch (discovery already cached from the call above).
    client
        .verify_id_token(&token, NONCE)
        .await
        .expect("first verify should succeed");
    // A second verification must hit neither endpoint again.
    client
        .verify_id_token(&token, NONCE)
        .await
        .expect("second verify should succeed");
    client
        .authorization_url("state-2", "nonce-2", "challenge-2")
        .await
        .expect("second authorization_url should succeed");

    discovery_mock.assert_async().await;
    jwks_mock.assert_async().await;
}

#[tokio::test]
async fn discovery_issuer_mismatched_with_config_is_rejected() {
    let mut server = mockito::Server::new_async().await;
    let issuer = server.url();

    server
        .mock("GET", "/.well-known/openid-configuration")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            // Issuer in the document deliberately does NOT match `issuer` —
            // simulates a misconfigured OIDC_ISSUER_URL.
            support::discovery_json(
                "https://not-the-configured-issuer.example.com",
                &format!("{issuer}/authorize"),
                &format!("{issuer}/token"),
                &format!("{issuer}/jwks"),
            )
            .to_string(),
        )
        .create_async()
        .await;

    let client = OidcClient::new(test_config(issuer), reqwest::Client::new());
    let err = client.authorization_url("s", "n", "c").await.unwrap_err();
    assert!(
        err.to_string().contains("does not match configured"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn jwks_kid_rotation_triggers_exactly_one_forced_refetch() {
    let mut server = mockito::Server::new_async().await;
    let issuer = server.url();

    server
        .mock("GET", "/.well-known/openid-configuration")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            support::discovery_json(
                &issuer,
                &format!("{issuer}/authorize"),
                &format!("{issuer}/token"),
                &format!("{issuer}/jwks"),
            )
            .to_string(),
        )
        .create_async()
        .await;

    // Registered first: served on the client's first (cache-populating) fetch.
    let stale_jwks = server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(support::jwks_json_decoy_only().to_string())
        .create_async()
        .await;
    // Registered second: served once the first mock's single expected hit is used up —
    // i.e. on the forced refetch after the unknown-kid failure.
    let rotated_jwks = server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(support::jwks_json_with_rotated_key().to_string())
        .create_async()
        .await;

    let client = OidcClient::new(test_config(issuer.clone()), reqwest::Client::new());

    let claims = support::valid_claims(&issuer, AUD, NONCE);
    let token = support::sign_id_token(&claims, support::OTHER_KID);

    let verified = client
        .verify_id_token(&token, NONCE)
        .await
        .expect("should succeed after one forced refetch");
    assert_eq!(verified.stable_subject(), "oid-123");

    stale_jwks.assert_async().await;
    rotated_jwks.assert_async().await;
}

#[tokio::test]
async fn authorization_url_contains_pkce_state_and_nonce() {
    let mut server = mockito::Server::new_async().await;
    let issuer = server.url();

    server
        .mock("GET", "/.well-known/openid-configuration")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            support::discovery_json(
                &issuer,
                &format!("{issuer}/authorize"),
                &format!("{issuer}/token"),
                &format!("{issuer}/jwks"),
            )
            .to_string(),
        )
        .create_async()
        .await;

    let client = OidcClient::new(test_config(issuer.clone()), reqwest::Client::new());
    let url = client
        .authorization_url("my-state", "my-nonce", "my-challenge")
        .await
        .expect("should build url");

    let parsed = reqwest::Url::parse(&url).unwrap();
    assert!(parsed.as_str().starts_with(&format!("{issuer}/authorize")));
    let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(pairs.get("client_id").unwrap(), AUD);
    assert_eq!(pairs.get("response_type").unwrap(), "code");
    assert_eq!(pairs.get("state").unwrap(), "my-state");
    assert_eq!(pairs.get("nonce").unwrap(), "my-nonce");
    assert_eq!(pairs.get("code_challenge").unwrap(), "my-challenge");
    assert_eq!(pairs.get("code_challenge_method").unwrap(), "S256");
    assert_eq!(
        pairs.get("redirect_uri").unwrap(),
        "https://app.example.com/api/auth/oidc/callback"
    );
    assert_eq!(pairs.get("scope").unwrap(), "openid profile email");
}

#[tokio::test]
async fn full_flow_exchange_code_then_verify_id_token_succeeds() {
    let mut server = mockito::Server::new_async().await;
    let issuer = server.url();

    server
        .mock("GET", "/.well-known/openid-configuration")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            support::discovery_json(
                &issuer,
                &format!("{issuer}/authorize"),
                &format!("{issuer}/token"),
                &format!("{issuer}/jwks"),
            )
            .to_string(),
        )
        .create_async()
        .await;

    server
        .mock("GET", "/jwks")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(support::jwks_json_with_test_key().to_string())
        .create_async()
        .await;

    let claims = support::valid_claims(&issuer, AUD, NONCE);
    let fixture_id_token = support::sign_id_token(&claims, support::TEST_KID);

    let token_mock = server
        .mock("POST", "/token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            serde_json::json!({
                "id_token": fixture_id_token,
                "access_token": "fixture-access-token",
                "token_type": "Bearer",
                "expires_in": 3600,
            })
            .to_string(),
        )
        .create_async()
        .await;

    let client = OidcClient::new(test_config(issuer.clone()), reqwest::Client::new());
    let token_response = client
        .exchange_code("fixture-auth-code", "fixture-code-verifier")
        .await
        .expect("code exchange should succeed");
    assert_eq!(
        token_response.access_token.as_deref(),
        Some("fixture-access-token")
    );

    let verified = client
        .verify_id_token(&token_response.id_token, NONCE)
        .await
        .expect("id_token from the exchange should verify");
    assert_eq!(verified.stable_subject(), "oid-123");
    assert_eq!(verified.display_username(), "alice@example.com");

    token_mock.assert_async().await;
}

#[tokio::test]
async fn token_endpoint_error_status_is_surfaced() {
    let mut server = mockito::Server::new_async().await;
    let issuer = server.url();

    server
        .mock("GET", "/.well-known/openid-configuration")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            support::discovery_json(
                &issuer,
                &format!("{issuer}/authorize"),
                &format!("{issuer}/token"),
                &format!("{issuer}/jwks"),
            )
            .to_string(),
        )
        .create_async()
        .await;

    server
        .mock("POST", "/token")
        .with_status(400)
        .with_header("content-type", "application/json")
        .with_body(r#"{"error":"invalid_grant"}"#)
        .create_async()
        .await;

    let client = OidcClient::new(test_config(issuer), reqwest::Client::new());
    let err = client
        .exchange_code("bad-code", "verifier")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("400"), "unexpected error: {err}");
}
