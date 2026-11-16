//! Assistant agent: LLM tool loop over MCP gateway tools, with A2A auth/input pauses.

use std::sync::Arc;

use a2a::*;
use a2a_server::*;
use futures::StreamExt;
use futures::stream::BoxStream;

use nasiko_agent_kit::llm_stream::StreamAccumulator;
use serde_json::{Value, json};

use nasiko_agent_kit::mcp::{McpClient, McpSession, ToolOutcome};
use nasiko_agent_kit::pause::{self, InputRequest};
use nasiko_agent_kit::session::{Conversation, ConversationStore, Pending};

#[allow(dead_code)]
mod telemetry;

const ASK_USER: &str = "ask_user";
const MAX_ROUNDS: usize = 8;

const SYSTEM: &str = "You are a helpful personal assistant. Use the connected-account tools for \
anything about the user's own data. When a request is ambiguous or you need a choice from the \
user (which account, which item, confirm before sending), call `ask_user` instead of guessing. \
Prefer drafts over sending. Be concise. If the tools for an app the user mentions (Gmail, Calendar, \
Slack, GitHub, ...) are missing, call `nasiko_connect_toolkit` with its slug so the user can connect it. \
Never use `ask_user` to ask the user to connect or authorize an app; only `nasiko_connect_toolkit` can show the Connect button. \
If `nasiko_connect_toolkit` is not available, say the tool gateway is unreachable.";

fn ask_user_def() -> Value {
    json!({"type": "function", "function": {
        "name": ASK_USER,
        "description": "Ask the user a question and wait for the answer. Give options when the answer is a choice.",
        "parameters": {"type": "object", "required": ["question"], "properties": {
            "question": {"type": "string"},
            "options": {"type": "array", "items": {"type": "object", "required": ["label"],
                "properties": {"label": {"type": "string"}, "description": {"type": "string"}}}},
            "multi_select": {"type": "boolean"}
        }}
    }})
}

#[derive(Clone)]
struct Llm {
    model: String,
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl Llm {
    fn from_env() -> Self {
        Self {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            http: reqwest::Client::new(),
        }
    }

    /// Starts a streamed completion; read the body with `bytes_stream()`.
    async fn chat_stream(
        &self,
        messages: &[Value],
        tools: &[Value],
        traceparent: &str,
    ) -> Result<reqwest::Response, String> {
        if self.api_key.is_empty() {
            return Err("OPENAI_API_KEY is not set for this agent.".into());
        }
        let mut body: Value =
            json!({"model": self.model, "messages": messages, "temperature": 0.2, "stream": true});
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key);
        if !traceparent.is_empty() {
            req = req.header("traceparent", traceparent);
        }
        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let text: String = resp.text().await.unwrap_or_default();
            return Err(format!("LLM API {status}: {text}"));
        }
        Ok(resp)
    }
}

struct Assistant {
    llm: Llm,
    mcp: Option<Arc<McpClient>>,
    store: Arc<ConversationStore>,
}

fn user_text(ctx: &ExecutorContext) -> String {
    ctx.message
        .as_ref()
        .map(|m| {
            m.parts
                .iter()
                .filter_map(|p| match &p.content {
                    PartContent::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect::<Vec<&str>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn artifact(task_id: &str, context_id: &str, text: &str) -> StreamResponse {
    artifact_chunk(task_id, context_id, &new_artifact_id(), text, false, true)
}

/// One piece of a streamed answer; every chunk of one answer shares `artifact_id`.
fn artifact_chunk(
    task_id: &str,
    context_id: &str,
    artifact_id: &str,
    text: &str,
    append: bool,
    last: bool,
) -> StreamResponse {
    StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        artifact: Artifact {
            artifact_id: artifact_id.into(),
            name: None,
            description: None,
            parts: vec![Part::text(text)],
            metadata: None,
            extensions: None,
        },
        append: Some(append),
        last_chunk: Some(last),
        metadata: None,
    })
}

/// Answers every tool call after index `i` so the transcript stays valid across a pause.
fn skip_rest(conv: &mut Conversation, calls: &[Value], i: usize) {
    for later in &calls[i + 1..] {
        conv.messages.push(json!({"role": "tool",
            "tool_call_id": later["id"].as_str().unwrap_or(""),
            "content": "Skipped: waiting on the user."}));
    }
}

impl AgentExecutor for Assistant {
    fn execute(
        &self,
        ctx: ExecutorContext,
    ) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let traceparent: String = ctx
            .service_params
            .get("traceparent")
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default();
        let task_id: String = ctx.task_id.clone();
        let context_id: String = ctx.context_id.clone();
        let text: String = user_text(&ctx);
        let llm = self.llm.clone();
        let mcp = self.mcp.clone();
        let store = self.store.clone();

        Box::pin(async_stream::stream! {
            yield Ok(pause::status_event(&task_id, &context_id, TaskState::Working, "", None));

            let mut conv: Conversation = store.take(&context_id);
            conv.resume_with(&text);

            let mut session: Option<McpSession> = match &mcp {
                Some(c) => Some(McpSession::open(c.clone(), &traceparent).await),
                None => None,
            };
            match &session {
                Some(s) => tracing::info!(context_id = %context_id, tools = s.tool_defs().len(), "mcp session opened"),
                None => tracing::warn!(context_id = %context_id, "no mcp gateway configured; only ask_user available"),
            }
            let system: String = match &session {
                Some(s) => format!("{SYSTEM}{}", s.system_prompt_hint()),
                None => SYSTEM.to_owned(),
            };

            for _ in 0..MAX_ROUNDS {
                let mut tools: Vec<Value> = vec![ask_user_def()];
                if let Some(s) = &session {
                    tools.extend(s.tool_defs().iter().cloned());
                }
                let mut messages: Vec<Value> = vec![json!({"role": "system", "content": system})];
                messages.extend(conv.messages.iter().cloned());

                let resp: reqwest::Response = match llm.chat_stream(&messages, &tools, &traceparent).await {
                    Ok(r) => r,
                    Err(e) => {
                        store.put(&context_id, conv);
                        yield Ok(pause::status_event(&task_id, &context_id, TaskState::Failed, &e, None));
                        return;
                    }
                };
                // Text deltas go out as they arrive; tool calls accumulate until the end.
                let artifact_id: String = new_artifact_id();
                let mut streamed: bool = false;
                let mut acc = StreamAccumulator::new();
                let mut body = resp.bytes_stream();
                while let Some(chunk) = body.next().await {
                    let bytes = match chunk {
                        Ok(b) => b,
                        Err(e) => {
                            store.put(&context_id, conv);
                            yield Ok(pause::status_event(&task_id, &context_id, TaskState::Failed, &format!("LLM stream error: {e}"), None));
                            return;
                        }
                    };
                    for delta in acc.push_bytes(&bytes) {
                        yield Ok(artifact_chunk(&task_id, &context_id, &artifact_id, &delta, streamed, false));
                        streamed = true;
                    }
                    if acc.is_done() {
                        break;
                    }
                }
                if let Some(err_msg) = acc.error() {
                    let msg = err_msg.to_owned();
                    store.put(&context_id, conv);
                    yield Ok(pause::status_event(&task_id, &context_id, TaskState::Failed, &format!("LLM error: {msg}"), None));
                    return;
                }
                if acc.finish_reason().is_none() {
                    store.put(&context_id, conv);
                    yield Ok(pause::status_event(&task_id, &context_id, TaskState::Failed, "LLM stream ended without finish_reason", None));
                    return;
                }
                let choice: Value = acc.finish();
                conv.messages.push(choice.clone());

                let calls: Vec<Value> = choice["tool_calls"].as_array().cloned().unwrap_or_default();
                if streamed {
                    // Close the answer artifact (pre-tool narration or the final answer).
                    yield Ok(artifact_chunk(&task_id, &context_id, &artifact_id, "", true, true));
                }
                if calls.is_empty() {
                    if !streamed {
                        yield Ok(artifact(&task_id, &context_id, ""));
                    }
                    store.put(&context_id, conv);
                    yield Ok(pause::status_event(&task_id, &context_id, TaskState::Completed, "", None));
                    return;
                }

                for (i, tc) in calls.iter().enumerate() {
                    let name: String = tc["function"]["name"].as_str().unwrap_or("").to_owned();
                    let args: String = tc["function"]["arguments"].as_str().unwrap_or("{}").to_owned();
                    let call_id: String = tc["id"].as_str().unwrap_or("").to_owned();

                    tracing::info!(context_id = %context_id, tool = %name, args = %args, "tool call");
                    if name == ASK_USER {
                        match serde_json::from_str::<InputRequest>(&args) {
                            Ok(req) => {
                                conv.pending = Some(Pending::Input { tool_call_id: call_id });
                                skip_rest(&mut conv, &calls, i);
                                store.put(&context_id, conv);
                                yield Ok(pause::input_required_event(&task_id, &context_id, &req));
                                return;
                            }
                            Err(e) => {
                                conv.messages.push(json!({"role": "tool", "tool_call_id": call_id,
                                    "content": format!("bad ask_user arguments: {e}")}));
                                continue;
                            }
                        }
                    }

                    yield Ok(pause::status_event(&task_id, &context_id, TaskState::Working, &tool_status_text(&name, &args), None));
                    let outcome: ToolOutcome = match &mut session {
                        Some(s) if s.handles(&name) => s.call_tool(&name, &args).await,
                        _ => ToolOutcome::Text(format!("unknown tool {name}")),
                    };
                    match outcome {
                        ToolOutcome::Text(t) => {
                            tracing::info!(context_id = %context_id, tool = %name, bytes = t.len(), preview = %t.chars().take(200).collect::<String>(), "tool result");
                            conv.messages.push(json!({"role": "tool", "tool_call_id": call_id, "content": t}));
                        }
                        ToolOutcome::AuthRequired(auth) => {
                            tracing::info!(context_id = %context_id, tool = %name, connector = %auth.connector, "auth required -> TASK_STATE_AUTH_REQUIRED");
                            conv.pending = Some(Pending::Auth { tool_call_id: call_id, connector: auth.connector.clone() });
                            skip_rest(&mut conv, &calls, i);
                            store.put(&context_id, conv);
                            yield Ok(pause::auth_required_event(&task_id, &context_id, &auth));
                            return;
                        }
                        ToolOutcome::ToolAsk(ask) => {
                            tracing::info!(context_id = %context_id, tool = %name, hitl_request_id = ?ask.hitl_request_id, "tool ask -> TASK_STATE_INPUT_REQUIRED");
                            conv.pending = Some(Pending::Input { tool_call_id: call_id });
                            skip_rest(&mut conv, &calls, i);
                            store.put(&context_id, conv);
                            yield Ok(pause::tool_ask_event(&task_id, &context_id, &ask));
                            return;
                        }
                    }
                }
            }

            store.put(&context_id, conv);
            yield Ok(artifact(&task_id, &context_id, "I ran out of steps before finishing. Ask again to continue."));
            yield Ok(pause::status_event(&task_id, &context_id, TaskState::Completed, "", None));
        })
    }

    fn cancel(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let ev: StreamResponse =
            pause::status_event(&ctx.task_id, &ctx.context_id, TaskState::Canceled, "", None);
        Box::pin(futures::stream::once(async move { Ok(ev) }))
    }
}

/// Working-status text for a tool call: `"{tool}: {preview}"`. The platform
/// (`a2a_dispatch.rs::tool_from_status_text`) turns this shape into a tool-call row in chat.
fn tool_status_text(name: &str, args: &str) -> String {
    let flat: String = args.split_whitespace().collect::<Vec<&str>>().join(" ");
    let preview: String = if flat.chars().count() > 120 {
        format!("{}…", flat.chars().take(120).collect::<String>())
    } else {
        flat
    };
    let preview: &str = if preview.is_empty() { "{}" } else { &preview };
    format!("{name}: {preview}")
}

#[tokio::main]
async fn main() {
    telemetry::init();
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8000);

    let mcp: Option<Arc<McpClient>> = McpClient::from_env().map(Arc::new);
    if let Some(c) = &mcp {
        c.probe().await;
    }

    let agent = Assistant {
        llm: Llm::from_env(),
        mcp,
        store: Arc::new(ConversationStore::new(1000)),
    };
    let handler = Arc::new(DefaultRequestHandler::new(agent, InMemoryTaskStore::new()));

    let card_json: String =
        include_str!("../AgentCard.json").replace("__PORT__", &port.to_string());
    let card: AgentCard = serde_json::from_str(&card_json).expect("invalid AgentCard.json");

    let app = axum::Router::new()
        .merge(a2a_server::jsonrpc::jsonrpc_router(handler))
        .merge(a2a_server::agent_card::agent_card_router(Arc::new(
            StaticAgentCard::new(card),
        )));

    tracing::info!("assistant-rs listening on 0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("bind");
    axum::serve(listener, app).await.expect("server failed");
}

#[cfg(test)]
mod tool_status_tests {
    use super::tool_status_text;

    #[test]
    fn tool_status_text_has_name_colon_preview_shape() {
        assert_eq!(
            tool_status_text("GMAIL_FETCH_EMAILS", "{\"max_results\": 5}"),
            "GMAIL_FETCH_EMAILS: {\"max_results\": 5}"
        );
        assert_eq!(tool_status_text("x", ""), "x: {}");
        assert!(tool_status_text("x", &"a".repeat(500)).ends_with('…'));
    }
}
