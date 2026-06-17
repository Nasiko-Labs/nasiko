use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::a2a::A2aClient;
use crate::registry::AgentInfo;

/// Wraps a remote A2A agent as a Rig `Tool` so the orchestrator LLM can invoke it.
#[derive(Clone)]
pub struct A2aTool {
    agent: AgentInfo,
    client: Arc<A2aClient>,
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
        Self { agent, client }
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

        let response = self
            .client
            .send_message(&self.agent.endpoint, &args.message, args.context_id.as_deref())
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
