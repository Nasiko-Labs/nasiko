use std::sync::Arc;

use a2a::Message as A2aMessage;
use a2a::{
    A2AError, Artifact, AgentCapabilities, AgentCard, AgentInterface, AgentProvider, AgentSkill,
    Part, PartContent, Role, StreamResponse, TaskArtifactUpdateEvent, TaskState, TaskStatus,
    TaskStatusUpdateEvent, TRANSPORT_PROTOCOL_JSONRPC, new_artifact_id, new_message_id,
};
use a2a_server::{
    AgentExecutor, DefaultRequestHandler, ExecutorContext, InMemoryTaskStore, StaticAgentCard,
};
use futures::stream::BoxStream;
use rig::OneOrMany;
use rig::completion::message::{ToolResultContent, UserContent};
use rig::completion::{
    AssistantContent, CompletionModel as _, CompletionRequestBuilder, CompletionResponse, Message,
    ToolDefinition,
};
use rig::prelude::CompletionClient as _;
use rig::providers::openai;
use rig::tool::{ToolDyn, ToolSet};

mod telemetry;
mod tools;

use tools::{CompareDiff, FindReferences, GetCommit, ListCommits, ReadFile, SearchPrs};

/// Max tool-calling turns per request: list_commits, compare_diff, search_prs, then a
/// final answer comfortably fit in this budget even one call at a time.
const MAX_TOOL_TURNS: usize = 20;

struct RepoWatchAgent {
    model: String,
    api_key: String,
    base_url: String,
    /// The watch list: repos to check when the caller doesn't name any explicitly (the
    /// scheduled/notification use case, where nobody types a query naming specific repos).
    /// Configured as `GITHUB_REPO="owner/a owner/b"` — space-separated in one env var.
    default_repos: Vec<String>,
}

impl RepoWatchAgent {
    fn new() -> Self {
        let default_repos = std::env::var("GITHUB_REPO")
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();
        Self {
            base_url: std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".into()),
            api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            default_repos,
        }
    }
}

/// Builds a `ToolDefinition` from a tool's own (now sync) `name`/`description`/`parameters`
/// — rig no longer bundles these into one async `definition()` call.
fn definition_of(tool: &dyn ToolDyn) -> ToolDefinition {
    ToolDefinition {
        name: tool.name(),
        description: tool.description(),
        parameters: tool.parameters(),
    }
}

/// Builds the read-only GitHub tools and their rig `ToolDefinition`s. Each tool struct and its
/// argument schema is generated entirely by `#[rig_tool]` in tools.rs from the corresponding
/// function's signature and doc comments. Window-digest tools (`ListCommits`/`CompareDiff`/
/// `SearchPrs`) plus per-commit impact-analysis tools (`GetCommit`/`ReadFile`/`FindReferences`).
fn build_tools() -> (ToolSet, Vec<ToolDefinition>) {
    let mut builder = ToolSet::builder();
    let mut defs = Vec::new();

    macro_rules! register {
        ($tool:expr) => {{
            let t = $tool;
            defs.push(definition_of(&t));
            builder = builder.static_tool(t);
        }};
    }

    register!(ListCommits);
    register!(CompareDiff);
    register!(SearchPrs);
    register!(GetCommit);
    register!(ReadFile);
    register!(FindReferences);

    (builder.build(), defs)
}

/// Sends one completion request, joining the caller's trace and recording GenAI span
/// attributes — the rig-based equivalent of the old hand-rolled `chat()` method.
#[tracing::instrument(name = "ChatCompletion", skip_all, fields(
    gen_ai.operation.name = "chat",
    gen_ai.request.model = %model_name,
    gen_ai.usage.input_tokens = tracing::field::Empty,
    gen_ai.usage.output_tokens = tracing::field::Empty,
))]
async fn send_completion(
    req: CompletionRequestBuilder<openai::CompletionModel>,
    model_name: &str,
    parent_cx: Option<&opentelemetry::Context>,
) -> Result<CompletionResponse<openai::CompletionResponse>, String> {
    // The remote parent must be set on THIS span explicitly: contextual
    // inheritance from a2a.execute strands the span in an orphan trace —
    // tracing-opentelemetry children inherit the parent's originally
    // sampled (local) trace id, not the one `set_parent` re-homed it to.
    if let Some(cx) = parent_cx {
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        tracing::Span::current().set_parent(cx.clone());
    }

    let response = req.send().await.map_err(|e| format!("LLM API error: {e}"))?;

    if let Some(usage) = &response.raw_response.usage {
        let span = tracing::Span::current();
        span.record("gen_ai.usage.input_tokens", usage.prompt_tokens as u64);
        span.record(
            "gen_ai.usage.output_tokens",
            usage.total_tokens.saturating_sub(usage.prompt_tokens) as u64,
        );
    }

    Ok(response)
}

fn system_prompt(default_repos: &[String], now: chrono::DateTime<chrono::Utc>) -> String {
    let default_line = if default_repos.is_empty() {
        String::new()
    } else {
        let list = default_repos
            .iter()
            .map(|r| format!("\"{r}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!("If the user doesn't name any repos, use the configured watch list: [{list}].\n")
    };
    let now_str = now.to_rfc3339();
    let since_12h = (now - chrono::Duration::hours(12)).to_rfc3339();
    format!(
        "You are a repo-watch agent. You report on GitHub activity — commits, file-level \
diffs, and pull requests — since a point in time, and flag anything risky, across one or \
more repos in a single request. You MUST use your tools; never answer from memory.\n\n\
{default_line}\
Every tool takes `repos` as a list of \"owner/name\" strings (e.g. \
[\"Nasiko-Labs/nasiko-cloud-rs\"]) — pass every repo the user asked about in one call rather \
than calling a tool once per repo.\n\n\
The current UTC time is {now_str}. If the user doesn't give an explicit time window, report \
on the last 12 hours — pass since={since_12h} (ISO-8601) to time-windowed tool calls. Never \
guess the current time; use the values given here.\n\n\
Your job is a window digest with an automatic deep-dive on RISKY commits only. Follow these \
steps in order:\n\n\
STEP 1 — Survey the window. Call list_commits(repos, since) and search_prs(repos, since). \
Also call compare_diff(repos, since) for repos with commits, to see the changed files.\n\n\
STEP 2 — Identify RISKY commits. A commit is risky if it touches: auth/authorization code, \
secrets/credentials handling, database migrations (*.sql), dependency manifests \
(Cargo.toml/Cargo.lock, package.json, pyproject.toml), Dockerfiles, or CI workflows \
(.github/workflows/*). To attribute changed files to individual commits, call \
get_commit(repo, sha) for each commit in the window (it returns that commit's file list + \
parent_sha cheaply). Commits touching none of those areas need NO deep-dive — the window \
summary is enough for them.\n\n\
STEP 3 — Deep-dive ONLY the risky commits. For each risky commit, for its SIGNIFICANT changed \
files (cap ~5 most-changed per commit — say when you rely on stats-only for the rest, never \
skip silently): call read_file(repo, path, git_ref) TWICE — at the commit sha (AFTER) and at \
parent_sha (BEFORE) — and read the real code, not just the patch. Then for each changed \
symbol call find_references(repo, symbol) to ground what else is impacted.\n\n\
Then respond with a markdown digest (note which repo each item belongs to when covering more \
than one). Sections:\n\
## New Commits\n\
One line per commit: short sha, message, author.\n\
## File-Level Changes\n\
Plain-English summary of the significant changes across the window.\n\
## PR Activity\n\
Classify each PR as opened, merged, or closed (not merged) within the window, from the \
created/merged/closed timestamps returned by search_prs.\n\
## Risk Flags & Deep-Dive\n\
For each risky commit, from the before/after you actually read: the exact symbol(s) changed, \
the change kind (signature / body / added / removed / type), the specific lines, and a precise \
summary — do not describe changes you didn't see in the code. Then an Impacted Files list: \
include a file ONLY if backed by evidence — a find_references hit for a changed symbol, OR a \
structural rule of this repo (an OSS `pub trait` under oss/*/src changing implies its EE \
implementor under ee/* must change; a new *.sql migration implies the models/queries reading \
those tables; an oss/server route change implies its oss/ui and oss/cli callers; a Cargo.toml \
dep bump implies dependent crates). Mark each \"must change\" (reference to a changed \
signature/removed symbol, or a structural rule) vs \"worth checking\". Never assert impact \
without a signal. If no commit is risky, say so plainly — don't invent risk.\n\n\
If the user explicitly names a single commit, skip the window survey and deep-dive just that \
commit (get_commit → read_file before/after → find_references).\n\n\
If there is no activity in the window at all, say so plainly instead of forcing the sections. \
You MUST use tools for every claim; never answer from memory."
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

        let model_name = self.model.clone();
        let api_key = self.api_key.clone();
        let base_url = self.base_url.clone();
        let default_repos = self.default_repos.clone();

        let stream = async_stream::stream! {
            yield Ok(status_working(&task_id, &context_id, None));

            // `CompletionsClient` (the classic `/chat/completions` shape), not the default
            // `openai::Client` (OpenAI's newer Responses API) — DeepSeek and other
            // OpenAI-compatible providers implement the former, not the latter.
            let client = match openai::CompletionsClient::builder().api_key(&api_key).base_url(&base_url).build() {
                Ok(c) => c,
                Err(e) => {
                    yield Ok(status_failed(&task_id, &context_id, &format!("LLM client setup failed: {e}")));
                    return;
                }
            };
            let model = client.completion_model(&model_name);
            let (toolset, tool_defs) = build_tools();
            let now = chrono::Utc::now();
            let system = system_prompt(&default_repos, now);

            let mut history: Vec<Message> = Vec::new();
            let mut prompt = Message::user(user_text);
            let mut final_text = String::new();

            for _ in 0..MAX_TOOL_TURNS {
                let req = model
                    .completion_request(prompt.clone())
                    .preamble(system.clone())
                    .messages(history.clone())
                    .tools(tool_defs.clone())
                    .temperature(0.2);

                let response = match send_completion(req, &model_name, remote_cx.as_ref()).await {
                    Ok(r) => r,
                    Err(e) => {
                        yield Ok(status_failed(&task_id, &context_id, &e));
                        return;
                    }
                };

                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();
                for content in response.choice.iter() {
                    match content {
                        AssistantContent::Text(t) => text_parts.push(t.text.clone()),
                        AssistantContent::ToolCall(tc) => tool_calls.push(tc.clone()),
                        // This agent is text-only in and out — it neither requests nor
                        // expects reasoning traces or image content back from the model.
                        AssistantContent::Reasoning(_) | AssistantContent::Image(_) => {}
                    }
                }

                if tool_calls.is_empty() {
                    final_text = strip_tool_markup(&text_parts.join("\n"));
                    break;
                }

                // Move this turn's prompt + assistant response into history so the next
                // request carries the full conversation.
                history.push(prompt.clone());
                // `id` is the provider-assigned assistant-message id (used by providers
                // that need it to correlate a later tool result); OpenAI-compatible chat
                // completions don't require one — only the per-tool-call `id` matters,
                // and that's already carried on each `ToolCall`.
                history.push(Message::Assistant { id: None, content: response.choice.clone() });

                let mut next_prompt = None;
                let num_calls = tool_calls.len();
                for (i, tc) in tool_calls.iter().enumerate() {
                    yield Ok(status_working(
                        &task_id, &context_id,
                        Some(&format!("{}: {}", tc.function.name, extract_preview(&tc.function.arguments))),
                    ));

                    let result = toolset.call(&tc.function.name, tc.function.arguments.to_string()).await;
                    let result_text = result.unwrap_or_else(|e| format!("Error: {e}"));

                    let tool_msg = Message::User {
                        content: OneOrMany::one(UserContent::tool_result(
                            tc.id.clone(),
                            OneOrMany::one(ToolResultContent::text(result_text)),
                        )),
                    };

                    // rig's request always needs exactly one "prompt" plus everything else
                    // as history — the last tool result of this turn becomes the next
                    // prompt; any earlier ones (a turn with several calls) go into history.
                    if i == num_calls - 1 {
                        next_prompt = Some(tool_msg);
                    } else {
                        history.push(tool_msg);
                    }
                }
                prompt = next_prompt.expect("at least one tool call this turn");
            }

            // Tool budget exhausted while the model still wanted tools: force a
            // final answer from the gathered context, else the artifact is empty.
            if final_text.is_empty() {
                history.push(prompt.clone());
                let nudge = Message::user(
                    "Tool calls are no longer available. Answer the original question now, \
                     using only the information already gathered above. Respond with plain text only."
                );
                let req = model
                    .completion_request(nudge)
                    .preamble(system.clone())
                    .messages(history.clone())
                    .temperature(0.2);

                match send_completion(req, &model_name, remote_cx.as_ref()).await {
                    Ok(response) => {
                        let text: String = response.choice.iter()
                            .filter_map(|c| match c {
                                AssistantContent::Text(t) => Some(t.text.clone()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        final_text = strip_tool_markup(&text);
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
        // (tracing's Instrumented wraps Futures, not Streams, so instrument
        // each item-poll future rather than the stream itself.)
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
                format!("http://0.0.0.0:{port}/"),
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
            message: msg.map(|t| A2aMessage {
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
            message: Some(A2aMessage {
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

/// Build a short human-readable status line for a tool call. Window tools carry a `repos`
/// array; the per-commit tools carry a single `repo` plus a path/symbol/sha detail.
fn extract_preview(args: &serde_json::Value) -> String {
    let s = |k: &str| args.get(k).and_then(|v| v.as_str());

    // Window-digest tools: repos: [...]
    if let Some(repos) = args.get("repos").and_then(|v| v.as_array())
        && !repos.is_empty()
    {
        return repos.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ");
    }

    // Per-commit tools: repo + one distinguishing detail.
    if let Some(repo) = s("repo") {
        if let Some(path) = s("path") {
            return format!("{repo} {path}");
        }
        if let Some(symbol) = s("symbol") {
            return format!("{repo} `{symbol}`");
        }
        if let Some(sha) = s("sha") {
            let short = &sha[..7.min(sha.len())];
            return format!("{repo} @{short}");
        }
        return repo.to_string();
    }

    "...".to_string()
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
