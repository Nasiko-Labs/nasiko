//! Deploy-time env injection: gives every agent container `MCP_GATEWAY_URL` so
//! it knows where to forward tool calls. The agent-side contract: read the
//! inbound `X-Nasiko-Agent-Token` header and forward it to that URL on every
//! MCP call. Composes alongside `OtelInjector` — nest a second
//! `InstrumentedRuntime` around it in `oss/server/src/runtime.rs`.

use std::collections::HashMap;

use nasiko_observability::{AgentContext, InstrumentationInjector};

/// Injects `MCP_GATEWAY_URL` from the platform's configured public gateway
/// URL. A no-op when unset — existing agents are unaffected until an operator
/// configures `MCP_GATEWAY_PUBLIC_URL`.
pub struct McpInjector {
    pub gateway_public_url: Option<String>,
}

impl InstrumentationInjector for McpInjector {
    fn inject(&self, env_vars: &mut HashMap<String, String>, _ctx: &AgentContext) {
        if let Some(url) = &self.gateway_public_url {
            env_vars.insert("MCP_GATEWAY_URL".into(), url.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_when_configured() {
        let injector = McpInjector { gateway_public_url: Some("http://gateway:8080/api/mcp".into()) };
        let mut env = HashMap::new();
        injector.inject(&mut env, &test_ctx());
        assert_eq!(env.get("MCP_GATEWAY_URL").map(String::as_str), Some("http://gateway:8080/api/mcp"));
    }

    #[test]
    fn no_op_when_unconfigured() {
        let injector = McpInjector { gateway_public_url: None };
        let mut env = HashMap::new();
        injector.inject(&mut env, &test_ctx());
        assert!(env.is_empty());
    }

    fn test_ctx() -> AgentContext {
        AgentContext {
            agent_id: "agent-1".into(),
            tenant_id: None,
            version: None,
            capture_content: false,
            otel_collector_endpoint: "http://collector:4318".into(),
            otel_protocol: "grpc".into(),
        }
    }
}
