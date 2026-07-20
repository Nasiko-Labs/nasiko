use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::a2a::{A2aClient, A2aClientError, AgentStreamEvent};
use crate::events::OrchestratorEvent;
use crate::registry::AgentInfo;

/// Wraps a remote A2A agent as a Rig `Tool` so the orchestrator LLM can invoke it.
#[derive(Clone)]
pub struct A2aTool {
    agent: AgentInfo,
    client: Arc<A2aClient>,
    /// When set, the agent is called via streaming and its live progress
    /// (internal tool activity + reply chunks) is relayed as
    /// `SubStatus`/`SubContent` orchestrator events.
    progress: Option<tokio::sync::mpsc::Sender<OrchestratorEvent>>,
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
        Self {
            agent,
            client,
            progress: None,
        }
    }

    /// Relay the agent's live progress into the orchestrator's event stream.
    pub fn with_progress(mut self, tx: tokio::sync::mpsc::Sender<OrchestratorEvent>) -> Self {
        self.progress = Some(tx);
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

        // Streaming first when a progress channel is attached; the match below
        // decides per error whether falling back to non-streaming is safe.
        let streamed = match self.progress {
            Some(ref orch_tx) => match self.call_streaming(&args, orch_tx.clone()).await {
                Ok(text) => Some(text),
                // Setup-stage failures (endpoint rejects the method / not an
                // A2A stream): the agent never started work, safe to retry
                // non-streaming.
                Err(A2aClientError::Http(..)) | Err(A2aClientError::InvalidResponse(_)) => None,
                Err(A2aClientError::A2aProtocol { code: -32601, .. }) => None,
                // Task failures and mid-stream errors: the agent may have run —
                // do NOT re-send (side effects would duplicate); report instead.
                Err(e) => {
                    return Err(A2aToolError {
                        agent: self.agent.name.clone(),
                        reason: e.to_string(),
                    });
                }
            },
            None => None,
        };

        let text = match streamed {
            Some(text) => text,
            None => {
                let response = self
                    .client
                    .send_message(
                        &self.agent.endpoint,
                        &args.message,
                        args.context_id.as_deref(),
                    )
                    .await
                    .map_err(|e| A2aToolError {
                        agent: self.agent.name.clone(),
                        reason: e.to_string(),
                    })?;
                A2aClient::extract_text(&response).unwrap_or_default()
            }
        };

        tracing::info!(agent = %self.agent.name, len = text.len(), "agent responded");
        if text.trim().is_empty() {
            // Explicit marker so the LLM knows the call succeeded but yielded
            // nothing — retrying the same message verbatim won't help.
            Ok("[agent returned an empty response]".to_string())
        } else {
            Ok(text)
        }
    }
}

impl A2aTool {
    /// Call the agent via `SendStreamingMessage`, forwarding its live events
    /// into the orchestrator stream as `SubStatus`/`SubContent`.
    async fn call_streaming(
        &self,
        args: &A2aToolArgs,
        orch_tx: tokio::sync::mpsc::Sender<OrchestratorEvent>,
    ) -> Result<String, A2aClientError> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentStreamEvent>(64);
        let agent_name = self.agent.name.clone();

        let forwarder = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                let mapped = match event {
                    AgentStreamEvent::Status(message) => OrchestratorEvent::SubStatus {
                        agent: agent_name.clone(),
                        message,
                    },
                    AgentStreamEvent::Content(content) => OrchestratorEvent::SubContent {
                        agent: agent_name.clone(),
                        content,
                    },
                };
                if orch_tx.send(mapped).await.is_err() {
                    // Orchestrator stream is gone (client disconnected) —
                    // drain silently so the agent call itself still completes.
                    while rx.recv().await.is_some() {}
                    break;
                }
            }
        });

        let result = self
            .client
            .send_message_streaming(
                &self.agent.endpoint,
                &args.message,
                args.context_id.as_deref(),
                Some(tx),
            )
            .await;

        // tx dropped above → forwarder drains and exits; join to avoid
        // interleaving a later call's events with this one's stragglers.
        let _ = forwarder.await;
        result
    }
}
