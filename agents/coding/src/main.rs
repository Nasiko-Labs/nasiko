use std::sync::Arc;

use a2a::*;
use a2a_server::*;
use futures::stream::BoxStream;
use tracing_subscriber::EnvFilter;

mod project;
mod sandbox;
mod tools;

/// Max ReAct iterations. Coding needs more read→edit→test cycles than a search agent, so this
/// is higher than the paper agent's 4.
const MAX_TURNS: usize = 12;

struct CodingAgent {
    model: String,
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl CodingAgent {
    fn new() -> Self {
        Self {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            http: reqwest::Client::new(),
        }
    }

    async fn chat(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
    ) -> Result<serde_json::Value, String> {
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "temperature": 0.1,
        });

        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("LLM API {status}: {body}"));
        }

        resp.json::<serde_json::Value>()
            .await
            .map_err(|e| format!("JSON parse: {e}"))
    }
}

const SYSTEM_PROMPT: &str = "\
You are a focused coding agent operating inside a sandboxed workspace. You can read, write, edit, \
search code, run shell commands, and run tests — all confined to the workspace directory.

Rules:
- Investigate before changing: use list_directory / read_file / search_code to understand the code first.
- Make minimal, targeted edits with edit_file (search/replace). The search block must be unique — \
include enough surrounding context. Use write_file only for new files or full rewrites.
- After changing code, run_tests (or run_command for a build/lint) to verify your work.
- If tests fail, read the output, fix, and re-run. Iterate until green or until you've clearly \
explained what's blocking.
- Stay within the workspace. Never assume tools or paths that you haven't verified exist.
- Be economical with tool calls — don't re-read a file you already have, and don't repeat an \
identical command.
- When done, respond with a concise summary of what you changed and the test/verification result \
(no tool call).";

impl AgentExecutor for CodingAgent {
    fn execute(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let task_id = ctx.task_id.clone();
        let context_id = ctx.context_id.clone();

        let user_text = ctx
            .message
            .as_ref()
            .map(|m| {
                m.parts
                    .iter()
                    .filter_map(|p| match &p.content {
                        PartContent::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        let model = self.model.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let http = self.http.clone();

        let stream = async_stream::stream! {
            yield Ok(status_working(&task_id, &context_id, Some("starting up sandbox")));

            // Build the sandbox for this request (CLI: local workspace; CP: remote, Phase 2).
            let sandbox = match sandbox::from_env() {
                Ok(s) => s,
                Err(e) => {
                    yield Ok(status_failed(&task_id, &context_id, &format!("sandbox init failed: {e}")));
                    return;
                }
            };

            let agent = CodingAgent { model, api_key, base_url, http };
            let tool_defs = tools::definitions();

            let mut messages = vec![
                serde_json::json!({"role": "system", "content": SYSTEM_PROMPT}),
                serde_json::json!({"role": "user", "content": user_text}),
            ];

            let mut final_text = String::new();

            for _ in 0..MAX_TURNS {
                let resp = match agent.chat(&messages, &tool_defs).await {
                    Ok(r) => r,
                    Err(e) => {
                        yield Ok(status_failed(&task_id, &context_id, &e));
                        return;
                    }
                };

                let choice = &resp["choices"][0]["message"];
                messages.push(choice.clone());

                if let Some(calls) = choice["tool_calls"].as_array() {
                    for tc in calls {
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let args = tc["function"]["arguments"].as_str().unwrap_or("{}");
                        let call_id = tc["id"].as_str().unwrap_or("");

                        let preview = extract_preview(name, args);
                        yield Ok(status_working(&task_id, &context_id, Some(&preview)));

                        let result = tools::execute(sandbox.as_ref(), name, args).await;

                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": result,
                        }));
                    }
                } else {
                    final_text = choice["content"].as_str().unwrap_or("").to_string();
                    break;
                }
            }

            if final_text.is_empty() {
                final_text = "Reached the maximum number of steps without a final answer.".into();
            }

            yield Ok(StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                task_id: task_id.clone(),
                context_id: context_id.clone(),
                artifact: Artifact {
                    artifact_id: new_artifact_id(),
                    name: None,
                    description: None,
                    parts: vec![Part::text(&final_text)],
                    metadata: None,
                    extensions: None,
                },
                append: Some(false),
                last_chunk: Some(true),
                metadata: None,
            }));

            yield Ok(status_completed(&task_id, &context_id));
        };

        Box::pin(stream)
    }

    fn cancel(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        let task_id = ctx.task_id.clone();
        let context_id = ctx.context_id.clone();
        Box::pin(futures::stream::once(async move {
            Ok(status_completed(&task_id, &context_id))
        }))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let handler = Arc::new(DefaultRequestHandler::new(
        CodingAgent::new(),
        InMemoryTaskStore::new(),
    ));

    let agent_card = AgentCard {
        name: "Coding Agent".to_string(),
        description: "Develops, tests, and refactors code in a sandboxed workspace".to_string(),
        version: "1.0.0".to_string(),
        provider: Some(AgentProvider {
            organization: "Nasiko".to_string(),
            url: "https://nasiko.io".to_string(),
        }),
        capabilities: AgentCapabilities {
            streaming: Some(true),
            push_notifications: Some(false),
            extensions: None,
            extended_agent_card: None,
        },
        skills: vec![
            AgentSkill {
                id: "code-edit".into(),
                name: "Code Editing".into(),
                description: "Read, write, and make targeted search/replace edits to source files".into(),
                tags: vec!["coding".into(), "edit".into(), "refactor".into()],
                examples: None, input_modes: None, output_modes: None, security_requirements: None,
            },
            AgentSkill {
                id: "code-test".into(),
                name: "Build & Test".into(),
                description: "Run builds, linters, and the project's test suite, then iterate on failures".into(),
                tags: vec!["coding".into(), "test".into(), "build".into()],
                examples: None, input_modes: None, output_modes: None, security_requirements: None,
            },
            AgentSkill {
                id: "code-refactor".into(),
                name: "Refactoring".into(),
                description: "Search the codebase and apply structured multi-file refactors".into(),
                tags: vec!["coding".into(), "refactor".into(), "search".into()],
                examples: None, input_modes: None, output_modes: None, security_requirements: None,
            },
        ],
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        supported_interfaces: vec![
            AgentInterface::new(
                format!("http://0.0.0.0:{port}/jsonrpc"),
                TRANSPORT_PROTOCOL_JSONRPC,
            ),
        ],
        security_schemes: None,
        security_requirements: None,
        documentation_url: None,
        icon_url: None,
        signatures: None,
    };

    let card_producer = Arc::new(StaticAgentCard::new(agent_card));

    let app = axum::Router::new()
        .nest("/jsonrpc", a2a_server::jsonrpc::jsonrpc_router(handler.clone()))
        .merge(a2a_server::agent_card::agent_card_router(card_producer));

    tracing::info!("Coding Agent listening on 0.0.0.0:{port}");

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("failed to bind");

    axum::serve(listener, app).await.expect("server failed");
}

// ─── Event helpers (mirror agents/paper/src/main.rs) ──────────────────────────

fn status_working(task_id: &str, context_id: &str, msg: Option<&str>) -> StreamResponse {
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::Working,
            message: msg.map(|t| Message {
                message_id: new_message_id(),
                context_id: Some(context_id.into()),
                task_id: Some(task_id.into()),
                role: Role::Agent,
                parts: vec![Part::text(t)],
                metadata: None,
                extensions: None,
                reference_task_ids: None,
            }),
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    })
}

fn status_completed(task_id: &str, context_id: &str) -> StreamResponse {
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::Completed,
            message: None,
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    })
}

fn status_failed(task_id: &str, context_id: &str, error: &str) -> StreamResponse {
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state: TaskState::Failed,
            message: Some(Message {
                message_id: new_message_id(),
                context_id: Some(context_id.into()),
                task_id: Some(task_id.into()),
                role: Role::Agent,
                parts: vec![Part::text(error)],
                metadata: None,
                extensions: None,
                reference_task_ids: None,
            }),
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    })
}

/// Build a short human-readable status line for a tool call, e.g. `edit_file: src/lib.rs`.
fn extract_preview(name: &str, args: &str) -> String {
    let parsed: Option<serde_json::Value> = serde_json::from_str(args).ok();
    let detail = parsed.as_ref().and_then(|v| {
        v.get("path")
            .or_else(|| v.get("command"))
            .or_else(|| v.get("pattern"))
            .and_then(|x| x.as_str())
            .map(|s| {
                if s.len() > 60 {
                    format!("{}…", &s[..60])
                } else {
                    s.to_string()
                }
            })
    });
    match detail {
        Some(d) => format!("{name}: {d}"),
        None => name.to_string(),
    }
}
