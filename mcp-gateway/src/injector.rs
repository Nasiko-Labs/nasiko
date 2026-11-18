//! Deploy-time env injection: tells every agent container where to forward tool
//! calls. Two forms of the same endpoint, because MCP has no equivalent of the
//! `OPENAI_API_KEY` env-var convention that lets the LLM router authenticate
//! SDKs with zero agent code:
//!
//! - `MCP_GATEWAY_URL` — plain endpoint. Preferred. The client sends
//!   `Authorization: Bearer $MCP_GATEWAY_TOKEN` (the per-agent credential
//!   minted by the server's `mcp::wiring` at deploy time).
//! - `MCP_GATEWAY_CONNECT_URL` — the same endpoint with that credential in the
//!   path, for framework MCP clients that expose only a `url` and no header
//!   hook. Secret-bearing by construction.
//!
//! Either way the user identity still comes from the propagated `traceparent`,
//! never from the credential — see docs/MCP_GATEWAY_AGENT_AUTH.md. Composes
//! alongside `OtelInjector` — nest a second `InstrumentedRuntime` around it in
//! `oss/server/src/runtime.rs`.

use std::collections::HashMap;

use nasiko_observability::{AgentContext, InstrumentationInjector};

/// Injects `MCP_GATEWAY_URL` (and, when the agent already has a gateway token
/// in its env, `MCP_GATEWAY_CONNECT_URL`) from the platform's configured public
/// gateway URL. A no-op when unset — existing agents are unaffected until an
/// operator configures `MCP_GATEWAY_PUBLIC_URL`.
pub struct McpInjector {
    pub gateway_public_url: Option<String>,
}

/// Compose the credential-bearing connect URL for a gateway token: the same
/// form whether the token was injected into a deployed container's env or
/// minted on demand for a local process (`POST /api/agents/{id}/mcp-token`).
/// The single place this URL shape is written, so the two callers can't drift.
pub fn connect_url(public_url: &str, token: &str) -> String {
    format!("{}/s/{}", public_url.trim_end_matches('/'), token)
}

impl InstrumentationInjector for McpInjector {
    fn inject(&self, env_vars: &mut HashMap<String, String>, _ctx: &AgentContext) {
        let Some(url) = &self.gateway_public_url else {
            return;
        };
        env_vars.insert("MCP_GATEWAY_URL".into(), url.clone());

        // Second, credential-bearing form for MCP clients that can only be
        // handed a URL. Many agent frameworks register MCP servers
        // declaratively (a server list or config block exposing `url` and
        // nothing else), so there is no seam to attach an `Authorization`
        // header to — unlike the LLM router, where every OpenAI SDK reads
        // `OPENAI_API_KEY` by convention, MCP has no such env-var contract.
        //
        // The token is the same credential the header form uses, so it stays
        // one row in `agent_gateway_tokens` and rotates on redeploy with
        // everything else. This URL IS a secret; see the deliberate trade-off
        // recorded in docs/MCP_GATEWAY_AGENT_AUTH.md.
        //
        // Runs after `mcp::wiring` has already minted `MCP_GATEWAY_TOKEN` into
        // the spec env (`McpInjector` is the outermost `InstrumentedRuntime`
        // layer, applied during `deploy()`), so the token is present here.
        let connect = env_vars
            .get("MCP_GATEWAY_TOKEN")
            .map(|token| connect_url(url, token));
        if let Some(connect) = connect {
            env_vars.insert("MCP_GATEWAY_CONNECT_URL".into(), connect);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_when_configured() {
        let injector = McpInjector {
            gateway_public_url: Some("http://gateway:8080/api/mcp".into()),
        };
        let mut env = HashMap::new();
        injector.inject(&mut env, &test_ctx());
        assert_eq!(
            env.get("MCP_GATEWAY_URL").map(String::as_str),
            Some("http://gateway:8080/api/mcp")
        );
    }

    #[test]
    fn composes_connect_url_from_the_minted_token() {
        let injector = McpInjector {
            gateway_public_url: Some("http://gateway:8080/api/mcp".into()),
        };
        let mut env = HashMap::from([("MCP_GATEWAY_TOKEN".to_string(), "ngt_abc".to_string())]);
        injector.inject(&mut env, &test_ctx());
        assert_eq!(
            env.get("MCP_GATEWAY_CONNECT_URL").map(String::as_str),
            Some("http://gateway:8080/api/mcp/s/ngt_abc"),
        );
        // The header form stays available — the URL form is additive.
        assert_eq!(
            env.get("MCP_GATEWAY_URL").map(String::as_str),
            Some("http://gateway:8080/api/mcp")
        );
    }

    #[test]
    fn connect_url_does_not_double_the_separator() {
        let injector = McpInjector {
            gateway_public_url: Some("http://gateway:8080/api/mcp/".into()),
        };
        let mut env = HashMap::from([("MCP_GATEWAY_TOKEN".to_string(), "ngt_abc".to_string())]);
        injector.inject(&mut env, &test_ctx());
        assert_eq!(
            env.get("MCP_GATEWAY_CONNECT_URL").map(String::as_str),
            Some("http://gateway:8080/api/mcp/s/ngt_abc"),
        );
    }

    #[test]
    fn no_connect_url_without_a_token() {
        // Minting is best-effort; a mint failure must not fabricate a URL that
        // would authenticate as the empty credential.
        let injector = McpInjector {
            gateway_public_url: Some("http://gateway:8080/api/mcp".into()),
        };
        let mut env = HashMap::new();
        injector.inject(&mut env, &test_ctx());
        assert!(!env.contains_key("MCP_GATEWAY_CONNECT_URL"));
    }

    #[test]
    fn no_op_when_unconfigured() {
        let injector = McpInjector {
            gateway_public_url: None,
        };
        let mut env = HashMap::new();
        injector.inject(&mut env, &test_ctx());
        assert!(env.is_empty());
    }

    fn test_ctx() -> AgentContext {
        AgentContext {
            agent_id: "agent-1".into(),
            agent_uuid: None,
            tenant_id: None,
            version: None,
            capture_content: false,
            otel_collector_endpoint: "http://collector:4318".into(),
            otel_protocol: "grpc".into(),
        }
    }
}
