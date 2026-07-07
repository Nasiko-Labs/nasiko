//! HTTP-client tests for the MCP gateway providers, using `mockito` to stand up
//! fake MCP backends and a fake Composio v3/v3.1 API. These exercise the wire
//! parsing (JSON vs SSE, nested vs flat Composio envelopes, error handling) that
//! can't be covered by pure unit tests.

use std::collections::HashMap;

use nasiko_mcp_gateway::provider::{
    ComposioProvider, ConnectedAccounts, GenericMcpProvider, ToolProvider,
};
use nasiko_mcp_gateway::types::MCPServerConfig;

fn cfg(url: String) -> MCPServerConfig {
    MCPServerConfig { name: "serpapi".into(), url, headers: HashMap::new(), transport: "streamable_http".into() }
}

// ─── GenericMcpProvider (streamable-HTTP transport) ─────────────────────────

#[tokio::test]
async fn generic_parses_application_json() {
    let mut srv = mockito::Server::new_async().await;
    let m = srv
        .mock("POST", "/mcp")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"search"}]}}"#)
        .create_async()
        .await;

    let provider = GenericMcpProvider::new(reqwest::Client::new());
    let tools = provider
        .list_tools(&cfg(format!("{}/mcp", srv.url())), std::time::Duration::from_secs(5), None)
        .await
        .unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "search");
    m.assert_async().await;
}

#[tokio::test]
async fn generic_parses_event_stream() {
    let mut srv = mockito::Server::new_async().await;
    srv.mock("POST", "/mcp")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body("event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n")
        .create_async()
        .await;

    let provider = GenericMcpProvider::new(reqwest::Client::new());
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}});
    let resp = provider
        .request(&cfg(format!("{}/mcp", srv.url())), &body, std::time::Duration::from_secs(5), None)
        .await
        .unwrap();
    assert_eq!(resp["result"]["ok"], true);
}

#[tokio::test]
async fn generic_non_2xx_is_error() {
    let mut srv = mockito::Server::new_async().await;
    srv.mock("POST", "/mcp").with_status(500).with_body("boom").create_async().await;

    let provider = GenericMcpProvider::new(reqwest::Client::new());
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}});
    assert!(
        provider
            .request(&cfg(format!("{}/mcp", srv.url())), &body, std::time::Duration::from_secs(5), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn generic_injects_auth_headers() {
    let mut srv = mockito::Server::new_async().await;
    let m = srv
        .mock("POST", "/mcp")
        .match_header("authorization", "Bearer sk-xyz")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#)
        .create_async()
        .await;

    let provider = GenericMcpProvider::new(reqwest::Client::new());
    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), "Bearer sk-xyz".to_string());
    let server = MCPServerConfig {
        name: "serpapi".into(),
        url: format!("{}/mcp", srv.url()),
        headers,
        transport: "streamable_http".into(),
    };
    provider.list_tools(&server, std::time::Duration::from_secs(5), None).await.unwrap();
    m.assert_async().await; // fails if the auth header wasn't sent
}

#[tokio::test]
async fn generic_propagates_traceparent_when_provided() {
    let mut srv = mockito::Server::new_async().await;
    let m = srv
        .mock("POST", "/mcp")
        .match_header("traceparent", "00-abc123-def456-01")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#)
        .create_async()
        .await;

    let provider = GenericMcpProvider::new(reqwest::Client::new());
    provider
        .list_tools(
            &cfg(format!("{}/mcp", srv.url())),
            std::time::Duration::from_secs(5),
            Some("00-abc123-def456-01"),
        )
        .await
        .unwrap();
    m.assert_async().await; // fails if traceparent wasn't forwarded to the backend
}

#[tokio::test]
async fn generic_omits_traceparent_when_absent() {
    let mut srv = mockito::Server::new_async().await;
    let m = srv
        .mock("POST", "/mcp")
        .match_header("traceparent", mockito::Matcher::Missing)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#)
        .create_async()
        .await;

    let provider = GenericMcpProvider::new(reqwest::Client::new());
    provider
        .list_tools(&cfg(format!("{}/mcp", srv.url())), std::time::Duration::from_secs(5), None)
        .await
        .unwrap();
    m.assert_async().await;
}

// ─── ComposioProvider (v3 / v3.1) ───────────────────────────────────────────

fn composio(url: String) -> ComposioProvider {
    ComposioProvider::new(reqwest::Client::new(), "ak_test".into(), url)
}

#[tokio::test]
async fn composio_create_auth_config_nested_and_flat_id() {
    let mut srv = mockito::Server::new_async().await;
    // Preferred: id nested under auth_config.
    let m = srv
        .mock("POST", "/api/v3/auth_configs")
        .match_header("x-api-key", "ak_test")
        .with_status(201)
        .with_body(r#"{"toolkit":{"slug":"gmail"},"auth_config":{"id":"ac_nested"}}"#)
        .create_async()
        .await;
    let p = composio(srv.url());
    let created = p.create_auth_config("gmail", true, None, None, None).await.unwrap();
    assert_eq!(created.auth_config_id, "ac_nested");
    m.assert_async().await;

    // Fallback: flat top-level id.
    let mut srv2 = mockito::Server::new_async().await;
    srv2.mock("POST", "/api/v3/auth_configs")
        .with_status(201)
        .with_body(r#"{"id":"ac_flat"}"#)
        .create_async()
        .await;
    let created = composio(srv2.url())
        .create_auth_config("gmail", true, None, None, None)
        .await
        .unwrap();
    assert_eq!(created.auth_config_id, "ac_flat");
}

#[tokio::test]
async fn composio_initiate_connection_reads_redirect() {
    let mut srv = mockito::Server::new_async().await;
    srv.mock("POST", "/api/v3/connected_accounts/link")
        .with_status(200)
        .with_body(r#"{"redirect_url":"https://auth.example/authorize","status":"INITIATED"}"#)
        .create_async()
        .await;
    let out = composio(srv.url())
        .initiate_connection("u1", "ac_1", Some("https://cb"))
        .await
        .unwrap();
    assert_eq!(out.redirect_url.as_deref(), Some("https://auth.example/authorize"));
    assert_eq!(out.status, "INITIATED");
}

#[tokio::test]
async fn composio_check_status_matches_by_auth_config() {
    let mut srv = mockito::Server::new_async().await;
    srv.mock("GET", mockito::Matcher::Regex("/api/v3/connected_accounts.*".into()))
        .with_status(200)
        .with_body(
            r#"{"items":[
                {"id":"ca_other","status":"ACTIVE","auth_config":{"id":"ac_other"}},
                {"id":"ca_1","status":"ACTIVE","auth_config":{"id":"ac_1"}}
            ]}"#,
        )
        .create_async()
        .await;
    let out = composio(srv.url()).check_connection_status("u1", "ac_1").await.unwrap();
    assert_eq!(out.account_id.as_deref(), Some("ca_1"));
    assert_eq!(out.status, "ACTIVE");
}

#[tokio::test]
async fn composio_check_status_not_found() {
    let mut srv = mockito::Server::new_async().await;
    srv.mock("GET", mockito::Matcher::Regex("/api/v3/connected_accounts.*".into()))
        .with_status(200)
        .with_body(r#"{"items":[]}"#)
        .create_async()
        .await;
    let out = composio(srv.url()).check_connection_status("u1", "ac_missing").await.unwrap();
    assert_eq!(out.status, "NOT_FOUND");
    assert!(out.account_id.is_none());
}

#[tokio::test]
async fn composio_create_session_nested_and_flat() {
    // Nested `mcp` envelope.
    let mut srv = mockito::Server::new_async().await;
    srv.mock("POST", "/api/v3.1/tool_router/session")
        .with_status(200)
        .with_body(r#"{"session_id":"s1","mcp":{"url":"https://mcp.composio/x","headers":{"x-api-key":"k"}}}"#)
        .create_async()
        .await;
    let accounts: ConnectedAccounts = HashMap::new();
    let s = composio(srv.url()).create_session("u1", &accounts).await.unwrap();
    assert_eq!(s.session_id, "s1");
    assert_eq!(s.mcp_url, "https://mcp.composio/x");
    // x-api-key is always injected (the real API omits headers; the MCP url 401s
    // without it), overriding whatever the response carried.
    assert_eq!(s.mcp_headers.get("x-api-key").map(String::as_str), Some("ak_test"));

    // Flat envelope (id / mcp_url), no headers in response → x-api-key still injected.
    let mut srv2 = mockito::Server::new_async().await;
    srv2.mock("POST", "/api/v3.1/tool_router/session")
        .with_status(200)
        .with_body(r#"{"id":"s2","mcp_url":"https://mcp/y"}"#)
        .create_async()
        .await;
    let s = composio(srv2.url()).create_session("u1", &accounts).await.unwrap();
    assert_eq!(s.session_id, "s2");
    assert_eq!(s.mcp_url, "https://mcp/y");
    assert_eq!(s.mcp_headers.get("x-api-key").map(String::as_str), Some("ak_test"));
}

#[tokio::test]
async fn composio_reuse_session_404_is_none() {
    let mut srv = mockito::Server::new_async().await;
    srv.mock("GET", "/api/v3.1/tool_router/session/dead")
        .with_status(404)
        .create_async()
        .await;
    assert!(composio(srv.url()).reuse_session("dead").await.unwrap().is_none());
}

#[tokio::test]
async fn composio_patch_and_revoke_status_semantics() {
    let mut srv = mockito::Server::new_async().await;
    srv.mock("PATCH", "/api/v3.1/tool_router/session/s1").with_status(200).create_async().await;
    srv.mock("POST", "/api/v3.1/connected_accounts/ca_1/revoke").with_status(200).create_async().await;
    let accounts: ConnectedAccounts = HashMap::new();
    let p = composio(srv.url());
    assert!(p.patch_session("s1", &accounts).await.unwrap());
    assert!(p.revoke_connection("ca_1").await.unwrap());

    // Failures degrade to false, not error.
    let mut srv2 = mockito::Server::new_async().await;
    srv2.mock("PATCH", "/api/v3.1/tool_router/session/s2").with_status(500).create_async().await;
    srv2.mock("POST", "/api/v3.1/connected_accounts/ca_2/revoke").with_status(404).create_async().await;
    let p2 = composio(srv2.url());
    assert!(!p2.patch_session("s2", &accounts).await.unwrap());
    assert!(!p2.revoke_connection("ca_2").await.unwrap());
}
