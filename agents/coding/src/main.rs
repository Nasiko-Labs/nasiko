use std::sync::Arc;

use a2a::*;
use a2a_server::*;
use futures::stream::BoxStream;

mod instructions;
mod project;
mod prompt_comments;
mod sandbox;
mod telemetry;
mod tools;

/// Max ReAct iterations. Coding needs more read→edit→test cycles than a search agent, so this
/// is higher than the paper agent's 4.
const MAX_TURNS: usize = 12;

struct CodingAgent {
    model: String,
    api_key: String,
    base_url: String,
    http: reqwest::Client,
    minimal_code: bool,
    self_review_enabled: bool,
}

impl CodingAgent {
    fn new() -> Self {
        Self {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            http: reqwest::Client::new(),
            // Off by default so the ladder's effect on code volume can be A/B'd per
            // deployment (docs/CODING_AGENT_MINIMALISM.md) — set via the agent's
            // secrets/env panel, then restart the agent to pick it up.
            minimal_code: nasiko_coding_policy::minimal_code_enabled(),
            self_review_enabled: nasiko_coding_policy::self_review_enabled(),
        }
    }

    #[tracing::instrument(name = "ChatCompletion", skip_all, fields(
        gen_ai.operation.name = "chat",
        gen_ai.provider.name = "openai",
        gen_ai.request.model = %self.model,
        gen_ai.usage.input_tokens = tracing::field::Empty,
        gen_ai.usage.output_tokens = tracing::field::Empty,
        gen_ai.input.messages = tracing::field::Empty,
        gen_ai.output.messages = tracing::field::Empty,
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
        let capture = telemetry::capture_content();
        if capture {
            tracing::Span::current().record(
                "gen_ai.input.messages",
                telemetry::genai_input_messages(messages).to_string().as_str(),
            );
        }
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "tools": tools,
            "temperature": 0.1,
        });
        // OpenAI-compatible APIs reject an empty tools array.
        if tools.is_empty() {
            body.as_object_mut().unwrap().remove("tools");
        }

        let mut req = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key);
        // The platform's LLM gateway rejects calls with no W3C trace context — it
        // attributes token/cost usage back to the flow that triggered them.
        if let Some(tp) = parent_cx.and_then(telemetry::traceparent_for_context) {
            req = req.header("traceparent", tp);
        }
        let resp = req
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

        if capture {
            let message = &response["choices"][0]["message"];
            let text = message["content"].as_str().unwrap_or("");
            let tool_calls = message["tool_calls"].as_array().cloned().unwrap_or_default();
            let finish_reason = if tool_calls.is_empty() { "stop" } else { "tool_call" };
            tracing::Span::current().record(
                "gen_ai.output.messages",
                telemetry::genai_output_message(text, &tool_calls, finish_reason)
                    .to_string()
                    .as_str(),
            );
        }

        Ok(response)
    }
}

const SYSTEM_PROMPT: &str = "\
You are a focused coding agent operating inside a sandboxed workspace. You can read, write, edit, \
search code, run shell commands, and run tests — all confined to the workspace directory. Your \
sandbox has the Rust toolchain (cargo/rustc), git, and ripgrep installed — nothing else. There is \
no Python, Node, Go, or any other language runtime unless you verify one exists.

Rules:
- Investigate before changing: use list_directory / read_file / search_code to understand the code first.
- Make minimal, targeted edits with edit_file (search/replace). The search block must be unique — \
include enough surrounding context. Use write_file only for new files or full rewrites.
- After changing code, run_tests (or run_command for a build/lint) to verify your work — but if the \
code isn't Rust, first confirm the toolchain it needs actually exists here (e.g. run_command(\"which \
python3\")). If it doesn't, say so plainly and give the code as reference without attempting to run \
it — don't burn tool calls on an execution that's guaranteed to fail. When the person doesn't specify \
a language, prefer Rust: it's the one toolchain guaranteed to work in this sandbox.
- If tests fail, read the output, fix, and re-run. Iterate until green or until you've clearly \
explained what's blocking.
- Stay within the workspace. Never assume tools or paths that you haven't verified exist.
- Your workspace is your own private sandbox — the person chatting with you has no terminal \
or filesystem access into it. If asked where a file is, how to find it, or how to get it, do \
NOT give local shell steps (cd/ls/dir) as if they can run them against your workspace. Instead \
say plainly that the file lives in your sandbox, not their computer, and use read_file to show \
them its actual contents right in the conversation.
- Be economical with tool calls — don't re-read a file you already have, and don't repeat an \
identical command.
- When done, respond with a concise summary of what you changed and the test/verification result \
(no tool call).
- When you create or change a file, include its actual current content (in a code block) in your \
response, not just a description of the change — read it back and show it. For a large file, the \
relevant changed section is enough; you don't need to repaste an unchanged file in full.";

// The minimal-code decision-ladder addendum is no longer built into this system
// prompt — the control plane injects it directly into the incoming task text
// at A2A dispatch time when minimal_code_enabled is set (oss/server/src/
// router/a2a_dispatch.rs), reading nasiko-coding-policy's own
// MINIMAL_CODE_ADDENDUM constant so there's still exactly one copy of that
// text. This agent still reads minimal_code_enabled() itself (below) — not to
// build a prompt with it, only because wants_self_review() requires it as one
// of its three conditions. See docs/CODING_AGENT_MINIMALISM.md.

/// Appended to the system prompt only when the prompt-comments feature is opted in.
/// Kept separate so a default (opted-out) session never sees instructions referring to
/// tools that were not registered.
const INSTRUCTION_MAINTENANCE_PROMPT: &str = "\
Instruction maintenance:
- If you discover a recurring pattern, convention, or corrective rule that should persist across \
future sessions, use update_instructions to record it with its rationale.
- Only add instructions when there is a clear trigger (a failure you fixed, a convention you \
discovered, or an explicit user request to remember something). Do not speculatively add instructions.";

impl AgentExecutor for CodingAgent {
    fn execute(
        &self,
        ctx: ExecutorContext,
    ) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        // Join the caller's W3C trace (the platform forwards `traceparent` through
        // the agent proxy/orchestrator) so the LLM gateway accepts our outbound
        // calls — see telemetry.rs.
        let remote_cx = ctx
            .service_params
            .get("traceparent")
            .and_then(|v| v.first())
            .and_then(|tp| telemetry::remote_context_from_traceparent(tp));

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
        let minimal_code = self.minimal_code;
        let self_review_enabled = self.self_review_enabled;

        let stream = async_stream::stream! {
            tracing::info!(minimal_code, self_review_enabled, task_id = %task_id, "handling task");
            yield Ok(status_working(&task_id, &context_id, Some("starting up sandbox")));

            // Build the sandbox for this request (CLI: local workspace; CP: remote, Phase 2).
            let sandbox = match sandbox::from_env() {
                Ok(s) => s,
                Err(e) => {
                    yield Ok(status_failed(&task_id, &context_id, &format!("sandbox init failed: {e}")));
                    return;
                }
            };

            // Discover workspace instructions and check if prompt-comments feature is opted in.
            let mut workspace_instructions = instructions::discover(sandbox.as_ref()).await;
            let feature_enabled = instructions::feature_state(workspace_instructions.as_ref())
                == instructions::FeatureState::Enabled;

            let agent = CodingAgent { model, api_key, base_url, http, minimal_code, self_review_enabled };

            // Prune stale instructions if opted in and the list has grown past threshold.
            if feature_enabled
                && let Some(ref wi) = workspace_instructions
                && instructions::needs_pruning(wi)
            {
                yield Ok(status_working(&task_id, &context_id, Some("pruning stale instructions")));
                let (prune_prompt, annotated) = instructions::build_prune_prompt(wi);
                let prune_messages = vec![
                    serde_json::json!({"role": "system", "content": "You review instruction files for staleness. Respond with only a JSON array."}),
                    serde_json::json!({"role": "user", "content": prune_prompt}),
                ];
                if let Ok(resp) = agent.chat(&prune_messages, &[]).await {
                    let answer = resp["choices"][0]["message"]["content"].as_str().unwrap_or("[]");
                    let indices = instructions::parse_prune_response(answer);
                    if !indices.is_empty() {
                        let _ = instructions::apply_pruning(
                            sandbox.as_ref(), wi, &indices, &annotated,
                        ).await;
                        workspace_instructions = instructions::discover(sandbox.as_ref()).await;
                    }
                }
            }

            let base_prompt = if feature_enabled {
                format!("{SYSTEM_PROMPT}\n\n{INSTRUCTION_MAINTENANCE_PROMPT}")
            } else {
                SYSTEM_PROMPT.to_string()
            };
            let system_prompt = instructions::build_system_prompt(
                &base_prompt,
                workspace_instructions.as_ref(),
                feature_enabled,
            );
            let mut tool_defs = tools::definitions();
            // Only expose instruction management tools when the feature is opted in.
            if feature_enabled {
                tool_defs.push(tools::update_instructions_definition());
                tool_defs.push(tools::prune_instructions_definition());
            }

            let mut messages = vec![
                serde_json::json!({"role": "system", "content": system_prompt}),
                serde_json::json!({"role": "user", "content": user_text}),
            ];

            let mut final_text = String::new();
            let mut wrote_code = false;

            for _ in 0..MAX_TURNS {
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
                        if matches!(name, "write_file" | "edit_file") {
                            wrote_code = true;
                        }

                        let preview = extract_preview(name, args);
                        yield Ok(status_working(&task_id, &context_id, Some(&preview)));

                        let result = if name == "update_instructions" {
                            tools::execute_update_instructions(
                                sandbox.as_ref(),
                                args,
                                workspace_instructions.as_ref(),
                            ).await
                        } else if name == "prune_instructions" {
                            // Manual prune: build prompt, call LLM, apply.
                            match workspace_instructions.as_ref() {
                                Some(wi) => {
                                    let (prune_prompt, annotated) = instructions::build_prune_prompt(wi);
                                    let prune_messages = vec![
                                        serde_json::json!({"role": "system", "content": "You review instruction files for staleness. Respond with only a JSON array."}),
                                        serde_json::json!({"role": "user", "content": prune_prompt}),
                                    ];
                                    match agent.chat(&prune_messages, &[]).await {
                                        Ok(resp) => {
                                            let answer = resp["choices"][0]["message"]["content"].as_str().unwrap_or("[]");
                                            let indices = instructions::parse_prune_response(answer);
                                            if indices.is_empty() {
                                                "No instructions were identified as stale. All current instructions remain active.".to_string()
                                            } else {
                                                match instructions::apply_pruning(sandbox.as_ref(), wi, &indices, &annotated).await {
                                                    Ok(count) => format!("Pruned {count} stale instruction(s). They are now marked as revoked and will be excluded from future sessions."),
                                                    Err(e) => format!("Error applying pruning: {e}"),
                                                }
                                            }
                                        }
                                        Err(e) => format!("Error during pruning review: {e}"),
                                    }
                                }
                                None => "No instruction file found in workspace. Nothing to prune.".to_string(),
                            }
                        } else {
                            tools::execute(sandbox.as_ref(), name, args).await
                        };

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

            // Phase 2 self-audit (docs/CODING_AGENT_MINIMALISM.md) — fail-open: if
            // this extra turn errors or comes back empty, keep the answer already
            // produced above rather than losing a response that otherwise succeeded.
            if self_review_enabled && nasiko_coding_policy::wants_self_review(minimal_code, wrote_code, &final_text) {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": nasiko_coding_policy::SELF_REVIEW_PROMPT,
                }));
                if let Ok(resp) = agent.chat(&messages, &[], remote_cx.as_ref()).await {
                    let reviewed = strip_tool_markup(
                        resp["choices"][0]["message"]["content"].as_str().unwrap_or(""));
                    if !reviewed.is_empty() {
                        final_text = reviewed;
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

    let agent = CodingAgent::new();
    // The only way to confirm which mode a running container is actually in: the
    // toggle (docs/CODING_AGENT_MINIMALISM.md) writes a secret, not a live value,
    // and takes effect only on the next restart — so state both flags plainly
    // here, once, at the moment that restart happens. Visible in the agent's
    // Logs tab immediately, without needing to send it a chat first — the
    // Settings-tab switches themselves can't be trusted for this: they have no
    // read-back route (secrets are write-only) and default to a guess whenever
    // a value was set another way (CLI deploy, a different browser, ...).
    tracing::info!(
        minimal_code = agent.minimal_code,
        self_review_enabled = agent.self_review_enabled,
        "coding agent starting"
    );

    let handler = Arc::new(DefaultRequestHandler::new(agent, InMemoryTaskStore::new()));

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
                description: "Read, write, and make targeted search/replace edits to source files"
                    .into(),
                tags: vec!["coding".into(), "edit".into(), "refactor".into()],
                examples: None,
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            },
            AgentSkill {
                id: "code-test".into(),
                name: "Build & Test".into(),
                description:
                    "Run builds, linters, and the project's test suite, then iterate on failures"
                        .into(),
                tags: vec!["coding".into(), "test".into(), "build".into()],
                examples: None,
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            },
            AgentSkill {
                id: "code-refactor".into(),
                name: "Refactoring".into(),
                description: "Search the codebase and apply structured multi-file refactors".into(),
                tags: vec!["coding".into(), "refactor".into(), "search".into()],
                examples: None,
                input_modes: None,
                output_modes: None,
                security_requirements: None,
            },
        ],
        default_input_modes: vec!["text/plain".to_string()],
        default_output_modes: vec!["text/plain".to_string()],
        supported_interfaces: vec![AgentInterface::new(
            format!("http://0.0.0.0:{port}/"),
            TRANSPORT_PROTOCOL_JSONRPC,
        )],
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

#[cfg(test)]
mod tests {
    use super::*;

    // The ladder itself is injected by the control plane now (a2a_dispatch.rs),
    // not built into this agent's own system prompt — see the comment above
    // SYSTEM_PROMPT. This just confirms the show-code instruction (still
    // hardcoded here, since it's unconditional) actually made it in.
    #[test]
    fn system_prompt_includes_show_code_instruction() {
        assert!(SYSTEM_PROMPT.contains("include its actual current content"));
    }

    // wants_self_review still needs minimal_code as an input even though this
    // agent no longer builds a prompt with it — confirms the crate call
    // compiles and behaves as expected with a value sourced from this agent's
    // own env read, not a hardcoded literal.
    #[test]
    fn self_review_still_gated_on_minimal_code_flag() {
        assert!(!nasiko_coding_policy::wants_self_review(
            false, true, "did something"
        ));
        assert!(nasiko_coding_policy::wants_self_review(
            true, true, "did something"
        ));
    }
}
