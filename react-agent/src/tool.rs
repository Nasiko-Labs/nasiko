use std::sync::Arc;

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::a2a::{A2aClient, A2aClientError, AgentStreamEvent, PauseInfo, SendOutcome};
use crate::events::OrchestratorEvent;
use crate::policy::DelegationPolicy;
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
    /// File parts from the user's upload, forwarded to the agent alongside
    /// the LLM-generated text message. Pre-serialized as JSON values.
    file_parts: Vec<serde_json::Value>,
    /// Operator policy, which may require extra arguments on this tool. `None`
    /// leaves the schema as the loop itself defines it.
    policy: Option<Arc<dyn DelegationPolicy>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct A2aToolArgs {
    pub message: String,
    #[serde(default)]
    pub context_id: Option<String>,
}

// No `deny_unknown_fields`, deliberately: a `DelegationPolicy` may add its own
// required arguments to this tool's schema (see `tool_schema_extra`), and it
// reads them off the RAW arguments before the call is dispatched. They are not
// this struct's business, and refusing to deserialize them here would turn a
// policy argument into an opaque tool failure.

#[derive(Debug, thiserror::Error)]
pub enum A2aToolError {
    #[error("agent '{agent}' call failed: {reason}")]
    Failed { agent: String, reason: String },

    /// Not a real failure — the agent needs a human before it can continue. Boxed by `rig-core`'s
    /// blanket `ToolDyn` impl (`Box::new(e)` where `e: A2aToolError`) when this is returned from
    /// `Tool::call`, and recovered by `react_loop.rs` via `downcast_ref::<A2aToolError>()` on that
    /// box — the concrete type survives `rig`'s type-erased `Result<String, ToolSetError>`
    /// because `Box<dyn std::error::Error>` supports downcasting, not because `rig` knows
    /// anything about this variant.
    ///
    /// `agent` is the display-folded name (`agent_display_name`), not the raw one — both
    /// construction sites in this file agree on that (a raw/display mismatch was found in
    /// review). Kept consistent with `OrchestratorEvent::AwaitingHuman`'s own `agent` field, since
    /// `a2a_dispatch.rs`'s `flow_steps` keying needs the display form either way (see
    /// `agent_display_name`'s own doc comment).
    #[error("agent '{agent}' is awaiting a human ({:?}: {})", pause.kind, pause.message)]
    AwaitingHuman {
        agent: String,
        agent_id: String,
        pause: PauseInfo,
    },
}

impl A2aTool {
    pub fn new(agent: AgentInfo, client: Arc<A2aClient>) -> Self {
        Self {
            agent,
            client,
            progress: None,
            file_parts: vec![],
            policy: None,
        }
    }

    /// Apply the operator's delegation policy to this tool's schema.
    ///
    /// Takes the `Option` the config already holds rather than a bare policy, so
    /// the unconfigured case is one call site fewer to get wrong: every caller
    /// passes `config.policy.clone()` unconditionally.
    pub fn with_policy(mut self, policy: Option<Arc<dyn DelegationPolicy>>) -> Self {
        self.policy = policy;
        self
    }

    /// The tool's JSON-Schema parameters, with any arguments the policy demands
    /// merged in.
    ///
    /// `message` and `context_id` are the loop's own and are never overwritten:
    /// a policy that could rewrite the whole schema could break dispatch, so it
    /// only ever adds.
    fn parameters_schema(&self) -> serde_json::Value {
        let mut properties = serde_json::Map::new();
        properties.insert(
            "message".to_string(),
            json!({
                "type": "string",
                "description": "The query or instruction to send to this agent"
            }),
        );
        properties.insert(
            "context_id".to_string(),
            json!({
                "type": "string",
                "description": "Optional conversation context ID for multi-turn interaction"
            }),
        );
        let mut required = vec!["message".to_string()];

        if let Some(extra) = self.policy.as_ref().and_then(|p| p.tool_schema_extra()) {
            for (name, schema) in extra.properties {
                properties.entry(name).or_insert(schema);
            }
            for name in extra.required {
                if !required.contains(&name) {
                    required.push(name);
                }
            }
        }

        json!({
            "type": "object",
            "properties": properties,
            "required": required,
        })
    }

    /// Attach file parts from the user's upload to forward to the agent.
    pub fn with_file_parts(mut self, parts: Vec<serde_json::Value>) -> Self {
        self.file_parts = parts;
        self
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

    /// The same display form `react_loop.rs` derives for `ToolCall`/`ToolResult` events
    /// (`name.strip_prefix("call_agent_").unwrap_or(name).replace('_', "-")`, applied to the LLM
    /// tool-call name) — computed here directly from the raw agent name instead, for the
    /// `AwaitingHuman` event this module sends. Composing `tool_name`'s forward map
    /// (`['-',' ','.','/']` → `_`) with react_loop's reverse map (`_` → `-`) collapses to mapping
    /// all five of those characters straight to `-`: `tool_name` never touches an original `_`,
    /// so it passes through unchanged into react_loop's replace same as a `-`/`.`/` `/`/`/` would.
    /// Must produce the exact same string react_loop.rs's `agent_display` does for the display
    /// name to match: `a2a_dispatch.rs`'s `flow_steps` INSERT (keyed on `ToolCall.agent`, i.e.
    /// `agent_display`) and its pause-closing UPDATE (keyed on `AwaitingHuman.agent`, this
    /// function) must agree, or an agent whose name contains anything `tool_name` folds into `_`
    /// (a `.` or space or `/`) never matches and its step stays `running` forever after a pause.
    ///
    /// `pub` for a third consumer in another crate:
    /// `oss/server/src/hitl/mod.rs::close_resumed_flow_step`, which only holds the raw name.
    pub fn agent_display_name(agent_name: &str) -> String {
        agent_name.replace(['-', ' ', '.', '/', '_'], "-")
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
            parameters: self.parameters_schema(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        tracing::info!(agent = %self.agent.name, message = %args.message, "invoking A2A agent");

        // No per-call user credential: the invoked agent authenticates to
        // /api/mcp with its own deploy-time MCP_GATEWAY_TOKEN; the user binding
        // rides the client-wide `traceparent` + the flow_participants record
        // the platform's CallGuard writes before this call.

        // Streaming first when a progress channel is attached; the match below
        // decides per error whether falling back to non-streaming is safe.
        let streamed = match self.progress {
            Some(ref orch_tx) => {
                match self.call_streaming(&args, orch_tx.clone(), &[]).await {
                    Ok(SendOutcome::Text(text)) => Some(text),
                    Ok(SendOutcome::AwaitingHuman(pause)) => {
                        return Err(A2aToolError::AwaitingHuman {
                            agent: Self::agent_display_name(&self.agent.name),
                            agent_id: self.agent.id.clone(),
                            pause,
                        });
                    }
                    // Setup-stage failures (endpoint rejects the method / not an
                    // A2A stream): the agent never started work, safe to retry
                    // non-streaming.
                    Err(A2aClientError::Http(..)) | Err(A2aClientError::InvalidResponse(_)) => None,
                    Err(A2aClientError::A2aProtocol { code: -32601, .. })
                    | Err(A2aClientError::A2aProtocol { code: -32004, .. }) => None,
                    // Task failures and mid-stream errors: the agent may have run —
                    // do NOT re-send (side effects would duplicate); report instead.
                    Err(e) => {
                        return Err(A2aToolError::Failed {
                            agent: self.agent.name.clone(),
                            reason: e.to_string(),
                        });
                    }
                }
            }
            None => None,
        };

        let text = match streamed {
            Some(text) => text,
            None => {
                let (sent_context_id, response) = self
                    .client
                    .send_message_with_headers(
                        &self.agent.endpoint,
                        &args.message,
                        args.context_id.as_deref(),
                        &[],
                        &self.file_parts,
                    )
                    .await
                    .map_err(|e| A2aToolError::Failed {
                        agent: self.agent.name.clone(),
                        reason: e.to_string(),
                    })?;
                // Not exempt from pausing just because it never opened an event stream — same
                // classification the streaming path uses, on the response's raw result value.
                let result_value = response.result.clone().unwrap_or(serde_json::Value::Null);
                for sse in nasiko_types::a2a::classify_sse_event(&result_value) {
                    if let nasiko_types::a2a::SseEvent::AwaitingHuman {
                        kind,
                        message,
                        metadata,
                    } = sse
                    {
                        // Fall back to `sent_context_id` (what was actually put on the wire —
                        // real either way, whether the LLM supplied it or the client minted one),
                        // never `args.context_id`, which is `None` in exactly the case this
                        // fallback exists for and would otherwise default to an empty string that
                        // doesn't match the conversation the agent was actually talked under.
                        let (task_id, context_id) =
                            A2aClient::extract_task_and_context_id(&result_value, &sent_context_id);
                        let pause = PauseInfo {
                            kind,
                            message,
                            task_id,
                            context_id,
                            metadata,
                        };
                        // Unlike a pause detected on the streaming path, nothing has relayed this
                        // one on `orch_tx` yet — `call_streaming`'s own forwarder task is what
                        // normally does that, and it never runs for this non-streaming fallback
                        // (react_loop.rs's callers rely on exactly that forwarder having already
                        // fired, so without this the orchestrator never learns the run paused at
                        // all: no `hitl_requests` row, no SSE frame, the turn silently completes).
                        if let Some(ref orch_tx) = self.progress {
                            let _ = orch_tx
                                .send(OrchestratorEvent::AwaitingHuman {
                                    agent: Self::agent_display_name(&self.agent.name),
                                    agent_id: self.agent.id.clone(),
                                    pause: pause.clone(),
                                })
                                .await;
                        }
                        return Err(A2aToolError::AwaitingHuman {
                            agent: Self::agent_display_name(&self.agent.name),
                            agent_id: self.agent.id.clone(),
                            pause,
                        });
                    }
                }
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
    /// Call the agent via streaming (`message/stream`, or the proto
    /// `SendStreamingMessage` fallback), forwarding its live events into the
    /// orchestrator stream as `SubStatus`/`SubContent`/`SubData`.
    async fn call_streaming(
        &self,
        args: &A2aToolArgs,
        orch_tx: tokio::sync::mpsc::Sender<OrchestratorEvent>,
        per_call_headers: &[(String, String)],
    ) -> Result<SendOutcome, A2aClientError> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentStreamEvent>(64);
        let agent_name = self.agent.name.clone();
        let agent_id = self.agent.id.clone();

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
                    AgentStreamEvent::AwaitingHuman(pause) => OrchestratorEvent::AwaitingHuman {
                        agent: A2aTool::agent_display_name(&agent_name),
                        agent_id: agent_id.clone(),
                        pause,
                    },
                    AgentStreamEvent::Data(data) => OrchestratorEvent::SubData {
                        via_agent: agent_name.clone(),
                        data,
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
                per_call_headers,
                &self.file_parts,
            )
            .await;

        // tx dropped above → forwarder drains and exits; join to avoid
        // interleaving a later call's events with this one's stragglers.
        let _ = forwarder.await;
        result
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

    /// The invoked agent must receive NO per-call credential header — the
    /// delegation-token scheme is gone (agents authenticate to /api/mcp with
    /// their own deploy-time MCP_GATEWAY_TOKEN); a stray header here would
    /// leak a caller-scoped secret into agent containers again.
    #[tokio::test]
    async fn call_sends_no_agent_token_header() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .match_header("x-nasiko-agent-token", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(a2a_response_body())
            .create_async()
            .await;

        let tool = A2aTool::new(test_agent(&server.url()), Arc::new(A2aClient::new()));
        let result = tool
            .call(A2aToolArgs {
                message: "hi".into(),
                context_id: None,
            })
            .await;

        mock.assert_async().await;
        assert!(result.is_ok());
    }
    fn input_required_response_body() -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "1",
            "result": {"task": {
                "id": "task-321",
                "contextId": "ctx-654",
                "status": {
                    "state": "TASK_STATE_INPUT_REQUIRED",
                    "message": {"parts": [{"text": "Which repository?"}]}
                }
            }}
        })
        .to_string()
    }

    /// A pause must never resolve to `Ok(String)` — that's the exact misclassification this
    /// whole mechanism exists to prevent. Exercises the non-streaming path (no `.with_progress`
    /// attached), which does its own pause classification separately from the streaming path.
    #[tokio::test]
    async fn call_without_progress_reports_awaiting_human_not_ok() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(input_required_response_body())
            .create_async()
            .await;

        let tool = A2aTool::new(test_agent(&server.url()), Arc::new(A2aClient::new()));
        let result = tool
            .call(A2aToolArgs {
                message: "hi".into(),
                context_id: Some("sent-ctx".into()),
            })
            .await;

        mock.assert_async().await;
        match result {
            Err(A2aToolError::AwaitingHuman {
                agent,
                agent_id,
                pause,
            }) => {
                assert_eq!(agent, "test-agent");
                assert_eq!(agent_id, "agent-under-test");
                assert_eq!(pause.message, "Which repository?");
                assert_eq!(pause.task_id, "task-321");
                assert_eq!(pause.context_id, "ctx-654");
            }
            other => panic!("expected Err(AwaitingHuman), got {other:?}"),
        }
    }

    /// `test_agent`'s name ("test-agent") happens to be a no-op under `agent_display_name`'s fold
    /// (its only special character, `-`, maps to itself), so the test above can't tell a raw name
    /// from a display-folded one apart. This one uses a name with a space specifically to catch
    /// that regression: `A2aToolError::AwaitingHuman.agent` must carry the SAME display-folded
    /// value `OrchestratorEvent::AwaitingHuman.agent` does (see `agent_display_name`'s own doc
    /// comment on why the two must agree), not the raw registry name.
    #[tokio::test]
    async fn awaiting_human_agent_field_is_display_folded_not_raw() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(input_required_response_body())
            .create_async()
            .await;

        let mut agent = test_agent(&server.url());
        agent.name = "Weather Agent".to_string();
        let tool = A2aTool::new(agent, Arc::new(A2aClient::new()));
        let result = tool
            .call(A2aToolArgs {
                message: "hi".into(),
                context_id: Some("sent-ctx".into()),
            })
            .await;

        mock.assert_async().await;
        match result {
            Err(A2aToolError::AwaitingHuman { agent, .. }) => {
                assert_eq!(agent, "Weather-Agent", "must be display-folded, not raw");
            }
            other => panic!("expected Err(AwaitingHuman), got {other:?}"),
        }
    }

    /// The single most regression-critical test in this file: proves `A2aToolError::AwaitingHuman`
    /// survives being boxed and type-erased by `rig-core`'s own `ToolSet::call()` — the exact path
    /// `react_loop.rs` uses — and not just when called directly via `A2aTool::call()` in isolation.
    /// If a future `rig-core` upgrade changes how it boxes a tool's error (or stops preserving the
    /// concrete type at all), this test fails here, at the boundary that broke, instead of
    /// downstream where a paused sub-agent's question would silently be reasoned over as if it
    /// were the answer.
    #[tokio::test]
    async fn awaiting_human_survives_rig_toolset_erasure() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(input_required_response_body())
            .create_async()
            .await;

        let agent = test_agent(&server.url());
        let tool_name = A2aTool::tool_name(&agent.name);
        let tool = A2aTool::new(agent, Arc::new(A2aClient::new()));
        let toolset = rig::tool::ToolSet::from_tools(vec![tool]);

        let args = serde_json::to_string(&A2aToolArgs {
            message: "hi".into(),
            context_id: Some("sent-ctx".into()),
        })
        .unwrap();
        let result = toolset.call(&tool_name, args).await;

        mock.assert_async().await;
        let Err(rig::tool::ToolSetError::ToolCallError(rig::tool::ToolError::ToolCallError(boxed))) =
            result
        else {
            panic!(
                "expected ToolSetError::ToolCallError(ToolError::ToolCallError(..)), got {result:?}"
            );
        };
        match boxed.downcast_ref::<A2aToolError>() {
            Some(A2aToolError::AwaitingHuman { pause, .. }) => {
                assert_eq!(pause.message, "Which repository?");
                assert_eq!(pause.task_id, "task-321");
            }
            other => panic!(
                "expected downcast_ref::<A2aToolError>() to recover AwaitingHuman, got {other:?}"
            ),
        }
    }

    /// The streaming path's own relay: `call_streaming`'s forwarder task must translate a pause
    /// into `OrchestratorEvent::AwaitingHuman` on the progress channel — never `SubStatus`, which
    /// would let it slip past a2a_dispatch.rs's terminal-event handling unnoticed.
    #[tokio::test]
    async fn streaming_awaiting_human_relays_orchestrator_event_not_sub_status() {
        let mut server = mockito::Server::new_async().await;
        let sse_body = concat!(
            "data: {\"result\":{\"statusUpdate\":{\"taskId\":\"task-123\",\"contextId\":\"ctx-456\",",
            "\"status\":{\"state\":\"TASK_STATE_INPUT_REQUIRED\",\"message\":{\"parts\":",
            "[{\"text\":\"Which repository?\"}]}}}}}\n\n",
        );
        let mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_body)
            .create_async()
            .await;

        let (orch_tx, mut orch_rx) = tokio::sync::mpsc::channel::<OrchestratorEvent>(8);
        let tool = A2aTool::new(test_agent(&server.url()), Arc::new(A2aClient::new()))
            .with_progress(orch_tx);
        let result = tool
            .call(A2aToolArgs {
                message: "hi".into(),
                context_id: None,
            })
            .await;

        mock.assert_async().await;
        assert!(matches!(result, Err(A2aToolError::AwaitingHuman { .. })));

        let event = orch_rx
            .recv()
            .await
            .expect("forwarder must relay the pause before the channel closes");
        match event {
            OrchestratorEvent::AwaitingHuman {
                agent,
                agent_id,
                pause,
            } => {
                assert_eq!(agent, "test-agent");
                assert_eq!(agent_id, "agent-under-test");
                assert_eq!(pause.message, "Which repository?");
            }
            other => panic!("expected OrchestratorEvent::AwaitingHuman, got {other:?}"),
        }
    }

    /// The non-streaming FALLBACK path's own relay: when `call_streaming` fails at setup (a
    /// 500 here — the same "safe to retry non-streaming" class as a rejected method) and the
    /// non-streaming retry itself pauses, nothing but this fallback branch can ever notice —
    /// `call_streaming`'s forwarder task, which the streaming-path test above relies on, never
    /// even starts. Without relaying `OrchestratorEvent::AwaitingHuman` here too,
    /// `react_loop.rs`'s callers (which assume the forwarder already did this) would never learn
    /// the run paused: no `hitl_requests` row, no SSE frame, the turn would silently complete.
    #[tokio::test]
    async fn non_streaming_fallback_awaiting_human_also_relays_orchestrator_event() {
        let mut server = mockito::Server::new_async().await;
        let stream_mock = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("message/stream".to_string()))
            .with_status(500)
            .with_body("agent unavailable")
            .create_async()
            .await;
        let send_mock = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex("message/send".to_string()))
            .with_status(200)
            .with_body(input_required_response_body())
            .create_async()
            .await;

        let (orch_tx, mut orch_rx) = tokio::sync::mpsc::channel::<OrchestratorEvent>(8);
        let tool = A2aTool::new(test_agent(&server.url()), Arc::new(A2aClient::new()))
            .with_progress(orch_tx);
        let result = tool
            .call(A2aToolArgs {
                message: "hi".into(),
                context_id: Some("sent-ctx".into()),
            })
            .await;

        stream_mock.assert_async().await;
        send_mock.assert_async().await;
        assert!(matches!(result, Err(A2aToolError::AwaitingHuman { .. })));

        let event = orch_rx
            .recv()
            .await
            .expect("the non-streaming fallback must relay the pause itself");
        match event {
            OrchestratorEvent::AwaitingHuman {
                agent,
                agent_id,
                pause,
            } => {
                assert_eq!(agent, "test-agent");
                assert_eq!(agent_id, "agent-under-test");
                assert_eq!(pause.message, "Which repository?");
                assert_eq!(pause.task_id, "task-321");
                assert_eq!(pause.context_id, "ctx-654");
            }
            other => panic!("expected OrchestratorEvent::AwaitingHuman, got {other:?}"),
        }
    }
}
