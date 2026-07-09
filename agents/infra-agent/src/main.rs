use std::sync::Arc;

use a2a::*;
use a2a_server::*;
use futures::stream::BoxStream;
use futures::StreamExt;
mod telemetry;
mod tools;

struct InfraAgent {
    model: String,
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

/// A tool call as it's incrementally assembled from streamed deltas — the API
/// sends `name` in the first delta for a given `index` and `arguments` in
/// fragments across subsequent deltas, so callers must accumulate by index.
#[derive(Default, Clone)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments: String,
}

/// One event out of a streamed chat completion: incremental text as it's
/// generated, or the terminal state once the stream ends.
enum ChatEvent {
    Content(String),
    Done { tool_calls: Vec<ToolCallBuilder> },
    Error(String),
}

impl InfraAgent {
    fn new() -> Self {
        Self {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            http: reqwest::Client::new(),
        }
    }

    /// Stream a chat completion, yielding text as the model generates it
    /// instead of buffering the whole response — `nasiko chat` renders each
    /// `ChatEvent::Content` chunk as it arrives rather than dumping the full
    /// answer at once at the end.
    ///
    /// Not `#[tracing::instrument]`'d: that macro spans only the synchronous
    /// call that builds this generator, not its later polling, so it would
    /// close before any HTTP work happens. Usage is logged as a plain event
    /// instead (see the `gen_ai.usage.*` fields below) once the API reports it.
    fn chat_stream(
        &self,
        messages: Vec<serde_json::Value>,
        tools: Vec<serde_json::Value>,
    ) -> impl futures::Stream<Item = ChatEvent> {
        let model = self.model.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let http = self.http.clone();

        async_stream::stream! {
            let body = serde_json::json!({
                "model": model,
                "messages": messages,
                "tools": tools,
                "temperature": 0.2,
                "stream": true,
                "stream_options": {"include_usage": true},
            });

            let resp = match http
                .post(format!("{base_url}/chat/completions"))
                .bearer_auth(&api_key)
                .json(&body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    yield ChatEvent::Error(format!("HTTP error: {e}"));
                    return;
                }
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                yield ChatEvent::Error(format!("LLM API {status}: {text}"));
                return;
            }

            let mut byte_stream = resp.bytes_stream();
            let mut buf = String::new();
            let mut tool_calls: Vec<ToolCallBuilder> = Vec::new();

            while let Some(chunk) = byte_stream.next().await {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        yield ChatEvent::Error(format!("stream error: {e}"));
                        return;
                    }
                };
                buf.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim_end_matches('\r').to_string();
                    buf.drain(..=pos);

                    let Some(data) = line.strip_prefix("data:") else { continue };
                    let data = data.trim();
                    if data.is_empty() || data == "[DONE]" {
                        continue;
                    }

                    let event: serde_json::Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    // The final chunk (when `stream_options.include_usage` is set) carries
                    // top-level `usage` and typically an empty `choices` array — check it
                    // independently rather than after the `choices`-dependent early return.
                    if let Some(usage) = event.get("usage").filter(|u| !u.is_null()) {
                        let input = usage.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                        let output = usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                        tracing::info!(
                            gen_ai.operation.name = "chat",
                            gen_ai.request.model = %model,
                            gen_ai.usage.input_tokens = input,
                            gen_ai.usage.output_tokens = output,
                            "chat completion usage",
                        );
                    }

                    let Some(choice) = event["choices"].get(0) else { continue };
                    let delta = &choice["delta"];

                    if let Some(content) = delta["content"].as_str()
                        && !content.is_empty()
                    {
                        yield ChatEvent::Content(content.to_string());
                    }

                    if let Some(calls) = delta["tool_calls"].as_array() {
                        for call in calls {
                            let idx = call["index"].as_u64().unwrap_or(0) as usize;
                            if tool_calls.len() <= idx {
                                tool_calls.resize(idx + 1, ToolCallBuilder::default());
                            }
                            if let Some(id) = call["id"].as_str() {
                                tool_calls[idx].id = id.to_string();
                            }
                            if let Some(name) = call["function"]["name"].as_str() {
                                tool_calls[idx].name.push_str(name);
                            }
                            if let Some(args) = call["function"]["arguments"].as_str() {
                                tool_calls[idx].arguments.push_str(args);
                            }
                        }
                    }
                }
            }

            yield ChatEvent::Done { tool_calls };
        }
    }
}

impl AgentExecutor for InfraAgent {
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
            yield Ok(status_working(&task_id, &context_id, None));

            let agent = InfraAgent { model, api_key, base_url, http };
            let tool_defs = tools::definitions();

            let system = "\
You are an Infrastructure Engineer assistant. You MUST use your tools for every answer — never \
respond from memory alone. Every claim must be backed by tool output.\n\n\
Tools:\n\
- terraform_modules — search Terraform Registry for IaC modules\n\
- terraform_provider — provider details (version, downloads, tier, source repo)\n\
- dns_lookup — resolve DNS records (A, AAAA, CNAME, MX, TXT, etc.)\n\
- ssl_check — verify TLS certificates and expiry dates\n\
- ip_info — IP geolocation and network information\n\n\
Rules:\n\
- ALWAYS call at least one tool before answering\n\
- For 'find a Terraform module for X': use terraform_modules\n\
- For domain diagnostics: dns_lookup first, then ssl_check\n\
- Always include specific versions and pin recommendations\n\
- All data must come from tool output, never from memory";

            let mut messages = vec![
                serde_json::json!({"role": "system", "content": system}),
                serde_json::json!({"role": "user", "content": user_text}),
            ];

            let artifact_id = new_artifact_id();
            let mut streamed_any = false;

            'rounds: for _ in 0..6 {
                let mut round_content = String::new();
                let mut round_tool_calls: Vec<ToolCallBuilder> = Vec::new();
                let mut round_stream = std::pin::pin!(agent.chat_stream(messages.clone(), tool_defs.clone()));

                while let Some(event) = round_stream.next().await {
                    match event {
                        ChatEvent::Content(delta) => {
                            round_content.push_str(&delta);
                            yield Ok(StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                                task_id: task_id.clone(),
                                context_id: context_id.clone(),
                                artifact: Artifact {
                                    artifact_id: artifact_id.clone(),
                                    name: None,
                                    description: None,
                                    parts: vec![Part::text(&delta)],
                                    metadata: None,
                                    extensions: None,
                                },
                                append: Some(streamed_any),
                                last_chunk: Some(false),
                                metadata: None,
                            }));
                            streamed_any = true;
                        }
                        ChatEvent::Done { tool_calls } => {
                            round_tool_calls = tool_calls;
                        }
                        ChatEvent::Error(e) => {
                            yield Ok(status_failed(&task_id, &context_id, &e));
                            return;
                        }
                    }
                }

                let real_calls: Vec<&ToolCallBuilder> =
                    round_tool_calls.iter().filter(|tc| !tc.name.is_empty()).collect();

                if real_calls.is_empty() {
                    // Final round: plain content, already streamed above.
                    break 'rounds;
                }

                let tool_calls_json: Vec<serde_json::Value> = real_calls
                    .iter()
                    .enumerate()
                    .map(|(i, tc)| serde_json::json!({
                        "id": if tc.id.is_empty() { format!("call_{i}") } else { tc.id.clone() },
                        "type": "function",
                        "function": {"name": tc.name, "arguments": tc.arguments},
                    }))
                    .collect();

                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": if round_content.is_empty() { serde_json::Value::Null } else { round_content.clone().into() },
                    "tool_calls": tool_calls_json,
                }));

                for (tc, tc_json) in real_calls.iter().zip(tool_calls_json.iter()) {
                    let preview = extract_preview(&tc.arguments);
                    yield Ok(status_working(
                        &task_id, &context_id,
                        Some(&format!("{}: {preview}", tc.name)),
                    ));

                    let result = tools::execute(&tc.name, &tc.arguments).await;

                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tc_json["id"].clone(),
                        "content": result,
                    }));
                }
            }

            if streamed_any {
                yield Ok(StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                    task_id: task_id.clone(),
                    context_id: context_id.clone(),
                    artifact: Artifact {
                        artifact_id: artifact_id.clone(),
                        name: None,
                        description: None,
                        parts: vec![Part::text("")],
                        metadata: None,
                        extensions: None,
                    },
                    append: Some(true),
                    last_chunk: Some(true),
                    metadata: None,
                }));
            }

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
    telemetry::init();

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8000);

    let handler = Arc::new(DefaultRequestHandler::new(
        InfraAgent::new(),
        InMemoryTaskStore::new(),
    ));

    let agent_card = AgentCard {
        name: "Infrastructure Manager".to_string(),
        description: "Terraform module search, DNS resolution, IP geolocation, and provider info".to_string(),
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
                id: "terraform-search".into(),
                name: "Terraform Registry".into(),
                description: "Search for Terraform modules and provider documentation".into(),
                tags: vec!["infrastructure".into(), "terraform".into(), "iac".into()],
                examples: Some(vec!["Find a Terraform module for AWS VPC".into()]),
                input_modes: None, output_modes: None, security_requirements: None,
            },
            AgentSkill {
                id: "dns-lookup".into(),
                name: "DNS Lookup".into(),
                description: "Resolve DNS records (A, AAAA, CNAME, MX, TXT, NS) for any domain".into(),
                tags: vec!["infrastructure".into(), "dns".into(), "networking".into()],
                examples: Some(vec!["What DNS records does github.com have?".into()]),
                input_modes: None, output_modes: None, security_requirements: None,
            },
            AgentSkill {
                id: "ip-geolocation".into(),
                name: "IP Geolocation".into(),
                description: "Get geolocation and network info for an IP address".into(),
                tags: vec!["infrastructure".into(), "networking".into(), "ip".into()],
                examples: Some(vec!["Where is IP 8.8.8.8 located?".into()]),
                input_modes: None, output_modes: None, security_requirements: None,
            },
        ],
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        supported_interfaces: vec![
            AgentInterface::new(
                &format!("http://0.0.0.0:{port}/"),
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
        .merge(a2a_server::jsonrpc::jsonrpc_router(handler.clone()))
        .merge(a2a_server::agent_card::agent_card_router(card_producer))
        .layer(tower_http::trace::TraceLayer::new_for_http());

    tracing::info!("Infrastructure Manager listening on 0.0.0.0:{port}");

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("failed to bind");

    axum::serve(listener, app).await.expect("server failed");
}

// ─── Event helpers ──────────────────────────────────────────────────────────

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

fn extract_preview(args: &str) -> String {
    serde_json::from_str::<serde_json::Value>(args)
        .ok()
        .and_then(|v| {
            v.as_object()?.values().find_map(|val| {
                val.as_str().map(|s| {
                    if s.len() > 60 { format!("{}...", &s[..60]) } else { s.to_string() }
                })
            })
        })
        .unwrap_or_else(|| "...".into())
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
