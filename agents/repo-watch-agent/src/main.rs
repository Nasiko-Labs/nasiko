use std::sync::Arc;

use a2a::*;
use a2a_server::*;
use futures::stream::BoxStream;

mod telemetry;
mod tools;

struct RepoWatchAgent {
    model: String,
    api_key: String,
    base_url: String,
    http: reqwest::Client,
    default_repo: Option<String>,
}

impl RepoWatchAgent {
    fn new() -> Self {
        Self {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            http: reqwest::Client::new(),
            default_repo: std::env::var("GITHUB_REPO").ok(),
        }
    }

    #[tracing::instrument(name = "ChatCompletion", skip_all, fields(
        gen_ai.operation.name = "chat",
        gen_ai.request.model = %self.model,
        gen_ai.usage.input_tokens = tracing::field::Empty,
        gen_ai.usage.output_tokens = tracing::field::Empty,
    ))]
    async fn chat(
        &self,
        messages: &[serde_json::Value],
        tools: &[serde_json::Value],
        parent_cx: Option<&opentelemetry::Context>,
    ) -> Result<serde_json::Value, String> {
        // The remote parent must be set on THIS span explicitly: contextual
        // inheritance from a2a.execute strands the span in an orphan trace —
        // tracing-opentelemetry children inherit the parent's originally
        // sampled (local) trace id, not the one `set_parent` re-homed it to.
        if let Some(cx) = parent_cx {
            use tracing_opentelemetry::OpenTelemetrySpanExt as _;
            tracing::Span::current().set_parent(cx.clone());
        }
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "temperature": 0.2,
        });
        // OpenAI-compatible APIs reject an empty tools array.
        if tools.is_empty() {
            body.as_object_mut().unwrap().remove("tools");
        }

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

        let response = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("JSON parse: {e}"))?;

        if let Some(usage) = response.get("usage") {
            let span = tracing::Span::current();
            if let Some(v) = usage.get("prompt_tokens").and_then(|v| v.as_u64()) {
                span.record("gen_ai.usage.input_tokens", v);
            }
            if let Some(v) = usage.get("completion_tokens").and_then(|v| v.as_u64()) {
                span.record("gen_ai.usage.output_tokens", v);
            }
        }

        Ok(response)
    }
}

fn system_prompt(default_repo: &Option<String>, now: chrono::DateTime<chrono::Utc>) -> String {
    let default_line = match default_repo {
        Some(r) => format!("If the user doesn't name a repo, use `{r}`.\n"),
        None => String::new(),
    };
    let now_str = now.to_rfc3339();
    let since_12h = (now - chrono::Duration::hours(12)).to_rfc3339();
    format!(
        "You are a repo-watch agent. You report on GitHub activity — commits, file-level \
diffs, and pull requests — since a point in time, and flag anything risky. You MUST use \
your tools; never answer from memory.\n\n\
{default_line}\
The current UTC time is {now_str}. If the user doesn't give an explicit time window, report \
on the last 12 hours — pass since={since_12h} (ISO-8601) to every tool call. Never guess the \
current time; use the values given here.\n\n\
Tools:\n\
- list_commits — always call this first for the requested owner/repo/since.\n\
- compare_diff — call this next if list_commits found any commits, to get file-level changes.\n\
- search_prs — always call this too, to catch PR activity that isn't in the commit list.\n\n\
When you have gathered everything, respond with a markdown digest in exactly these four \
sections, using clear subheadings:\n\
## New Commits\n\
One line per commit: short sha, message, author.\n\
## File-Level Changes\n\
Which files changed and a plain-English summary of what changed in the significant ones. \
Note file counts even for files whose patch wasn't shown due to truncation.\n\
## PR Activity\n\
Classify each PR as opened, merged, or closed (not merged) within the window, from the \
created/merged/closed timestamps returned by search_prs.\n\
## Risk Flags\n\
Call out anything touching auth/authorization code, secrets/credentials handling, database \
migrations (*.sql), dependency manifests (Cargo.toml/Cargo.lock, package.json, \
pyproject.toml), Dockerfiles, or CI workflow files (.github/workflows/*). Also flag \
unusually large diffs. If nothing stands out, say so explicitly — don't invent risk.\n\n\
If there is no activity at all in the window, say so plainly instead of forcing the four \
sections."
    )
}

impl AgentExecutor for RepoWatchAgent {
    fn execute(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        // Join the caller's W3C trace (the platform forwards `traceparent` through the
        // agent proxy/orchestrator). Without adopting it, the OTel SDK mints a fresh root
        // trace id per request and the control plane's session-trace view can't find this
        // agent's spans.
        let remote_cx = ctx
            .service_params
            .get("traceparent")
            .and_then(|v| v.first())
            .and_then(|tp| telemetry::remote_context_from_traceparent(tp));
        let span = tracing::info_span!("a2a.execute", otel.kind = "server");
        if let Some(ref cx) = remote_cx {
            use tracing_opentelemetry::OpenTelemetrySpanExt as _;
            span.set_parent(cx.clone());
        }

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
        let default_repo = self.default_repo.clone();

        let stream = async_stream::stream! {
            yield Ok(status_working(&task_id, &context_id, None));

            let agent = RepoWatchAgent { model, api_key, base_url, http, default_repo: default_repo.clone() };
            let tool_defs = tools::definitions();
            let now = chrono::Utc::now();
            let system = system_prompt(&default_repo, now);

            let mut messages = vec![
                serde_json::json!({"role": "system", "content": system}),
                serde_json::json!({"role": "user", "content": user_text}),
            ];

            let mut final_text = String::new();

            for _ in 0..6 {
                let resp = match agent.chat(&messages, &tool_defs, remote_cx.as_ref()).await {
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

                        yield Ok(status_working(
                            &task_id, &context_id,
                            Some(&format!("{name}: {}", extract_preview(args))),
                        ));

                        let result = tools::execute(name, args).await;

                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": result,
                        }));
                    }
                } else {
                    final_text = strip_tool_markup(choice["content"].as_str().unwrap_or(""));
                    break;
                }
            }

            // Tool budget exhausted while the model still wanted tools: force a
            // final answer from the gathered context, else the artifact is empty.
            if final_text.is_empty() {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": "Tool calls are no longer available. Answer the original question now, using only the information already gathered above. Respond with plain text only."
                }));
                match agent.chat(&messages, &[], remote_cx.as_ref()).await {
                    Ok(resp) => {
                        final_text = strip_tool_markup(
                            resp["choices"][0]["message"]["content"].as_str().unwrap_or(""));
                    }
                    Err(e) => {
                        yield Ok(status_failed(&task_id, &context_id, &e));
                        return;
                    }
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

        // Poll the stream inside `span` so every span created during execution
        // (ChatCompletion, tool calls) lands under the remote parent — even
        // though the body streams after the HTTP handler has returned.
        use futures::StreamExt as _;
        use tracing::Instrument as _;
        Box::pin(async_stream::stream! {
            let mut inner = std::pin::pin!(stream);
            while let Some(item) = inner.next().instrument(span.clone()).await {
                yield item;
            }
        })
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
        RepoWatchAgent::new(),
        InMemoryTaskStore::new(),
    ));

    let agent_card = AgentCard {
        name: "Repo Watch Agent".to_string(),
        description: "Reports on GitHub repo activity — commits, file diffs, and PRs — since a given time, with risk flags".to_string(),
        version: "0.1.0".to_string(),
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
        skills: vec![AgentSkill {
            id: "repo-digest".into(),
            name: "Repo Digest".into(),
            description: "Summarize commits, file-level diffs, and PR activity for a GitHub repo since a given time, flagging risky changes".into(),
            tags: vec!["github".into(), "monitoring".into(), "digest".into()],
            examples: None, input_modes: None, output_modes: None, security_requirements: None,
        }],
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
        .merge(a2a_server::agent_card::agent_card_router(card_producer));

    tracing::info!("Repo Watch Agent listening on 0.0.0.0:{port}");

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

/// Build a short human-readable status line for a tool call, e.g. `owner/repo`.
fn extract_preview(args: &str) -> String {
    serde_json::from_str::<serde_json::Value>(args)
        .ok()
        .and_then(|v| {
            let owner = v.get("owner")?.as_str()?;
            let repo = v.get("repo")?.as_str()?;
            Some(format!("{owner}/{repo}"))
        })
        .unwrap_or_else(|| "...".into())
}

/// DeepSeek sometimes emits its internal tool-call markup (`<｜DSML｜…`) as
/// plain content instead of structured tool_calls. Anything from the first
/// marker onward is machinery, not an answer — cut it so an all-markup
/// response reads as empty and triggers the forced-answer fallback.
fn strip_tool_markup(content: &str) -> String {
    match content.find("<｜") {
        Some(idx) => content[..idx].trim().to_string(),
        None => content.trim().to_string(),
    }
}
