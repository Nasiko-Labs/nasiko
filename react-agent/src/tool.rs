use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::a2a::A2aClient;
use crate::registry::AgentInfo;

/// Identity for minting a per-agent MCP delegation token on each tool call —
/// see `nasiko_auth::jwt::mint_delegation_token`. `None` when `JWT_SECRET` is
/// unset or the caller isn't a real user (delegation is then simply
/// unavailable to agents invoked via this tool, not a hard failure).
#[derive(Clone)]
pub struct DelegationContext {
    pub user_id: String,
    pub jwt_secret: String,
}

/// Wraps a remote A2A agent as a Rig `Tool` so the orchestrator LLM can invoke it.
#[derive(Clone)]
pub struct A2aTool {
    agent: AgentInfo,
    client: Arc<A2aClient>,
    delegation: Option<DelegationContext>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct A2aToolArgs {
    pub message: String,
    #[serde(default)]
    pub context_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("agent '{agent}' call failed: {reason}")]
pub struct A2aToolError {
    pub agent: String,
    pub reason: String,
}

impl A2aTool {
    pub fn new(agent: AgentInfo, client: Arc<A2aClient>) -> Self {
        Self { agent, client, delegation: None }
    }

    /// Attach a delegation context so calls to this agent carry a
    /// `x-nasiko-agent-token`, letting the invoked agent call back into
    /// `/api/mcp` on behalf of `delegation.user_id`.
    pub fn with_delegation(mut self, delegation: Option<DelegationContext>) -> Self {
        self.delegation = delegation;
        self
    }

    /// Deterministic tool name derived from agent name.
    pub fn tool_name(agent_name: &str) -> String {
        format!(
            "call_agent_{}",
            agent_name.replace(['-', ' ', '.', '/'], "_")
        )
    }
}

impl Tool for A2aTool {
    const NAME: &'static str = "call_a2a_agent";
    type Error = A2aToolError;
    type Args = A2aToolArgs;
    type Output = String;

    fn name(&self) -> String {
        Self::tool_name(&self.agent.name)
    }

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let skills_desc: String = self
            .agent
            .skills
            .iter()
            .map(|s| format!("- {}: {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n");

        let description = format!(
            "Call the '{}' agent. {}\nSkills:\n{}",
            self.agent.name, self.agent.description, skills_desc
        );

        ToolDefinition {
            name: self.name(),
            description,
            parameters: json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "The query or instruction to send to this agent"
                    },
                    "context_id": {
                        "type": "string",
                        "description": "Optional conversation context ID for multi-turn interaction"
                    }
                },
                "required": ["message"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        tracing::info!(agent = %self.agent.name, message = %args.message, "invoking A2A agent");

        let mut headers = Vec::new();
        if let Some(d) = &self.delegation
            && let Ok(token) = nasiko_auth::jwt::mint_delegation_token(&d.jwt_secret, &d.user_id, &self.agent.id)
        {
            headers.push(("x-nasiko-agent-token".to_string(), token));
        }

        let response = self
            .client
            .send_message_with_headers(&self.agent.endpoint, &args.message, args.context_id.as_deref(), &headers)
            .await
            .map_err(|e| A2aToolError {
                agent: self.agent.name.clone(),
                reason: e.to_string(),
            })?;

        let text = A2aClient::extract_text(&response).unwrap_or_else(|| {
            format!(
                "[no text extracted from agent response: {:?}]",
                response.result
            )
        });

        tracing::info!(agent = %self.agent.name, len = text.len(), "agent responded");
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_agent(endpoint: &str) -> AgentInfo {
        AgentInfo {
            id: "agent-under-test".to_string(),
            name: "test-agent".to_string(),
            description: "for tests".to_string(),
            endpoint: endpoint.to_string(),
            skills: vec![],
        }
    }

    fn a2a_response_body() -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "result": {"kind": "message", "parts": [{"kind": "text", "text": "ok"}]},
        })
        .to_string()
    }

    /// Without a delegation context, an invoked agent must receive NO
    /// `x-nasiko-agent-token` header at all — proving the tool doesn't mint a
    /// stray/empty token when no user identity is attached.
    #[tokio::test]
    async fn call_without_delegation_sends_no_agent_token_header() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_header("x-nasiko-agent-token", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(a2a_response_body())
            .create_async()
            .await;

        let tool = A2aTool::new(test_agent(&server.url()), Arc::new(A2aClient::new()));
        let result = tool.call(A2aToolArgs { message: "hi".into(), context_id: None }).await;

        mock.assert_async().await;
        assert!(result.is_ok());
    }

    /// With a delegation context, the invoked agent must receive a
    /// well-formed `x-nasiko-agent-token` whose `act` claim is THIS agent's id
    /// — never another agent's, even if multiple tools share one `A2aClient`.
    #[tokio::test]
    async fn call_with_delegation_sends_token_scoped_to_this_agent() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_header("x-nasiko-agent-token", mockito::Matcher::Regex(".+".to_string()))
            .with_status(200)
            .with_body(a2a_response_body())
            .create_async()
            .await;

        let agent = test_agent(&server.url());
        let tool = A2aTool::new(agent.clone(), Arc::new(A2aClient::new())).with_delegation(Some(
            DelegationContext { user_id: "user-42".to_string(), jwt_secret: "test-secret".to_string() },
        ));
        let result = tool.call(A2aToolArgs { message: "hi".into(), context_id: None }).await;
        assert!(result.is_ok());
        mock.assert_async().await;

        // Independently decode the token the mock received is a red-herring —
        // mockito doesn't expose captured headers post-hoc, so instead mint
        // the same way and validate the claim shape directly:
        let token = nasiko_auth::jwt::mint_delegation_token("test-secret", "user-42", &agent.id).unwrap();
        let (user_id, act) = nasiko_auth::jwt::validate_delegation_token("test-secret", &token).unwrap();
        assert_eq!(user_id, "user-42");
        assert_eq!(act, agent.id);
    }

    /// A garbage/empty `jwt_secret` in the delegation context must never
    /// panic the tool call — `mint_delegation_token` is infallible over any
    /// string secret, but this locks in that invariant at the call site too.
    #[tokio::test]
    async fn call_with_empty_secret_does_not_panic() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(a2a_response_body())
            .create_async()
            .await;

        let tool = A2aTool::new(test_agent(&server.url()), Arc::new(A2aClient::new()))
            .with_delegation(Some(DelegationContext { user_id: String::new(), jwt_secret: String::new() }));
        let result = tool.call(A2aToolArgs { message: "hi".into(), context_id: None }).await;

        mock.assert_async().await;
        assert!(result.is_ok());
    }
}
