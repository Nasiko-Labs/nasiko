use std::sync::Arc;

use axum::{ body::Body, http::{ Request, StatusCode } };
use tower::util::ServiceExt;

use crate::config::GitHubConfig;
use crate::service::GitHubService;

use super::github_router;

// ── Shared test helper ─────────────────────────────────────────────────────────

fn make_svc(mock_github_url: &str, mock_api_url: &str) -> Arc<GitHubService> {
    let cfg = GitHubConfig {
        client_id: "test-client".into(),
        client_secret: "test-secret".into(),
        callback_url: "https://example.com/api/github/callback".into(),
        oauth_state_secret: "test-oauth-secret-key-32bytes!!".into(),
        central_callback_url: None,
        clone_timeout_secs: 300,
        clone_max_size_bytes: 500 * 1024 * 1024,
    };
    Arc::new(GitHubService::with_base_urls(cfg, mock_github_url, mock_api_url).unwrap())
}

// ── Login ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn github_login_redirects() {
    let svc = make_svc("https://github.com", "https://api.github.com");
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/github/login")
                .header("x-user-id", "user-1")
                .body(Body::empty())
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(loc.contains("github.com/login/oauth/authorize"));
    assert!(loc.contains("client_id=test-client"));
}

#[tokio::test]
async fn github_login_requires_user_id() {
    let svc = make_svc("https://github.com", "https://api.github.com");
    let app = github_router(svc);

    let resp = app
        .oneshot(Request::builder().uri("/github/login").body(Body::empty()).unwrap()).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── Logout ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn github_logout_returns_200() {
    let svc = make_svc("https://github.com", "https://api.github.com");
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/github/logout")
                .header("x-user-id", "user-1")
                .body(Body::empty())
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

// ── Status ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn github_status_returns_not_connected_without_token_header() {
    let svc = make_svc("https://github.com", "https://api.github.com");
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/github/status")
                .header("x-user-id", "user-1")
                .body(Body::empty())
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["connected"], false);
}

#[tokio::test]
async fn github_status_connected_and_valid_for_live_token() {
    let mut api_server = mockito::Server::new_async().await;
    let mock = api_server
        .mock("GET", "/user")
        .with_status(200)
        .with_body("{}")
        .create_async().await;

    let svc = make_svc("https://github.com", &api_server.url());
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/github/status")
                .header("x-user-id", "user-1")
                .header("x-github-token", "valid-token")
                .body(Body::empty())
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["connected"], true, "must be connected");
    assert_eq!(json["valid"], true, "must be valid");
    mock.assert_async().await;
}

#[tokio::test]
async fn github_status_connected_but_invalid_for_expired_token() {
    let mut api_server = mockito::Server::new_async().await;
    let mock = api_server
        .mock("GET", "/user")
        .with_status(401)
        .with_body("{}")
        .create_async().await;

    let svc = make_svc("https://github.com", &api_server.url());
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/github/status")
                .header("x-user-id", "user-1")
                .header("x-github-token", "expired-token")
                .body(Body::empty())
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["connected"], true, "token was provided, so connected=true");
    assert_eq!(json["valid"], false, "but 401 means token is invalid");
    mock.assert_async().await;
}

// ── Callback ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn github_callback_invalid_state_returns_400() {
    let svc = make_svc("https://github.com", "https://api.github.com");
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/github/callback?code=abc&state=bad-state")
                .body(Body::empty())
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn github_callback_success_returns_200_with_user_info() {
    let mut gh_server = mockito::Server::new_async().await;
    let mut api_server = mockito::Server::new_async().await;

    let token_mock = gh_server
        .mock("POST", "/login/oauth/access_token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"access_token":"gho_xyz","token_type":"bearer","scope":"repo"}"#)
        .create_async().await;
    let user_mock = api_server
        .mock("GET", "/user")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":42,"login":"octocat","name":"Octocat","email":null,"avatar_url":"https://github.com/octocat.png"}"#
        )
        .create_async().await;

    let svc = make_svc(&gh_server.url(), &api_server.url());
    // Build a valid state using the same service so the HMAC key matches.
    let valid_state = svc.build_state("user-1").unwrap();
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .uri(format!("/github/callback?code=real-code&state={valid_state}"))
                .body(Body::empty())
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["access_token"], "gho_xyz");
    assert_eq!(json["user"]["login"], "octocat");
    token_mock.assert_async().await;
    user_mock.assert_async().await;
}

// ── Repos ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn github_repos_requires_github_token_header() {
    let svc = make_svc("https://github.com", "https://api.github.com");
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/github/repos")
                .header("x-user-id", "user-1")
                // no X-GitHub-Token
                .body(Body::empty())
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn github_repos_returns_401_on_upstream_auth_failure() {
    // Verifies that a GitHub 401 (bad/expired token) surfaces as
    // HTTP 401 from the route, not 502.
    let mut api_server = mockito::Server::new_async().await;
    let mock = api_server
        .mock("GET", mockito::Matcher::Regex(r"^/user/repos".to_string()))
        .with_status(401)
        .with_body(r#"{"message":"Bad credentials"}"#)
        .create_async().await;

    let svc = make_svc("https://github.com", &api_server.url());
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/github/repos")
                .header("x-user-id", "user-1")
                .header("x-github-token", "bad-token")
                .body(Body::empty())
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    mock.assert_async().await;
}

#[tokio::test]
async fn github_repos_returns_list_on_success() {
    let repo_json =
        r#"[{"id":1,"name":"my-repo","full_name":"owner/my-repo",
        "description":null,"private":false,"clone_url":"https://github.com/owner/my-repo.git",
        "ssh_url":"git@github.com:owner/my-repo.git","html_url":"https://github.com/owner/my-repo",
        "default_branch":"main","updated_at":"2024-01-01T00:00:00Z"}]"#;

    let mut api_server = mockito::Server::new_async().await;
    let mock = api_server
        .mock("GET", mockito::Matcher::Regex(r"^/user/repos".to_string()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(repo_json)
        .create_async().await;

    let svc = make_svc("https://github.com", &api_server.url());
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/github/repos")
                .header("x-user-id", "user-1")
                .header("x-github-token", "good-token")
                .body(Body::empty())
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 8192).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total"], 1);
    assert_eq!(json["repositories"][0]["name"], "my-repo");
    mock.assert_async().await;
}

// ── Clone ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn github_clone_requires_github_token_header() {
    let svc = make_svc("https://github.com", "https://api.github.com");
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/github/clone")
                .header("x-user-id", "user-1")
                .header("content-type", "application/json")
                // no X-GitHub-Token
                .body(Body::from(r#"{"repo_full_name":"owner/repo"}"#))
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn github_clone_rejects_invalid_repo_name() {
    // Invalid repo_full_name → 422 before any expensive operation.
    let svc = make_svc("https://github.com", "https://api.github.com");
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/github/clone")
                .header("x-user-id", "user-1")
                .header("x-github-token", "tok")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"repo_full_name":"no-slash-here"}"#))
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn github_clone_valid_inputs_returns_501_until_minio_wired() {
    // Valid inputs → 501 (MinIO upload not yet implemented).
    // Ensures the handler never returns a fake s3_key that doesn't
    // point to anything in object storage.
    let svc = make_svc("https://github.com", "https://api.github.com");
    let app = github_router(svc);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/github/clone")
                .header("x-user-id", "user-1")
                .header("x-github-token", "tok")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"repo_full_name":"owner/repo","branch":"main"}"#))
                .unwrap()
        ).await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json["suggested_s3_key"].as_str().unwrap().starts_with("github/owner/repo/"),
        "suggested_s3_key must identify the intended object path"
    );
}
