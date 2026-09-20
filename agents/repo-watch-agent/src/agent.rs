//! Agent execution — the A2A entry point and the ReAct loop behind it.
//!
//! `RepoWatchAgent` implements `a2a_server::AgentExecutor`: each request runs a bounded
//! tool-calling loop over the read-only GitHub tools in [`crate::tools`], then streams back a
//! markdown digest. All external concerns (GitHub, the LLM, tracing) live behind their own
//! modules; this file owns the orchestration and the prompt that drives it.

use a2a::Message as A2aMessage;
use a2a::{
    A2AError, AgentCapabilities, AgentCard, AgentInterface, AgentProvider, AgentSkill, Artifact,
    Part, PartContent, Role, StreamResponse, TRANSPORT_PROTOCOL_JSONRPC, TaskArtifactUpdateEvent,
    TaskState, TaskStatus, TaskStatusUpdateEvent, new_artifact_id, new_message_id,
};
use a2a_server::{AgentExecutor, ExecutorContext};
use futures::stream::BoxStream;
use rig::OneOrMany;
use rig::completion::message::{ToolResultContent, UserContent};
use rig::completion::{
    AssistantContent, CompletionModel as _, CompletionRequestBuilder, CompletionResponse, Message,
};
use rig::prelude::CompletionClient as _;
use rig::providers::openai;

use crate::github::short_hash;
use crate::slack;
use crate::telemetry;
use crate::tools::build_tools;

/// Upper bound on LLM tool-calling turns per request. The flow (survey the window, triage
/// commits with get_commit, deep-dive risky ones with read_file/find_references) can make
/// many calls; this caps a runaway while leaving ample room for a normal digest.
const MAX_TOOL_TURNS: usize = 20;

/// Sampling temperature for the digest — low, because this is factual reporting, not creative
/// text.
const COMPLETION_TEMPERATURE: f64 = 0.2;

/// A live progress message per tool call, for callers that stream status back to a caller
/// (the A2A path). `None` for callers that only want the final digest (the Slack scheduler).
pub(crate) type ProgressSink = Option<tokio::sync::mpsc::UnboundedSender<String>>;

/// The agent's configuration, read once from the environment at startup.
#[derive(Clone)]
pub struct RepoWatchAgent {
    model: String,
    api_key: String,
    base_url: String,
    /// The watch list: repos to check when the caller doesn't name any explicitly (the
    /// scheduled/notification use case, where nobody types a query naming specific repos).
    /// Configured as `GITHUB_REPO="owner/a owner/b"` — space-separated in one env var.
    default_repos: Vec<String>,
}

impl RepoWatchAgent {
    pub fn new() -> Self {
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

    /// Runs the full tool-calling loop for `prompt_text` and returns the final digest text. No
    /// A2A dependency — called from a real request (via `execute`, which streams `progress` as
    /// live status) and from the Slack scheduler (which passes `progress: None` and just awaits
    /// the result).
    pub(crate) async fn run_digest(
        &self,
        prompt_text: String,
        remote_cx: Option<&opentelemetry::Context>,
        progress: ProgressSink,
    ) -> Result<String, String> {
        // `CompletionsClient` (the classic `/chat/completions` shape), not the default
        // `openai::Client` (OpenAI's newer Responses API) — DeepSeek and other
        // OpenAI-compatible providers implement the former, not the latter.
        let client = openai::CompletionsClient::builder()
            .api_key(&self.api_key)
            .base_url(&self.base_url)
            .build()
            .map_err(|e| format!("LLM client setup failed: {e}"))?;
        let model = client.completion_model(&self.model);
        let (toolset, tool_defs) = build_tools();
        let now = chrono::Utc::now();
        let system = system_prompt(&self.default_repos, now);

        let mut history: Vec<Message> = Vec::new();
        let mut prompt = Message::user(prompt_text);
        let mut final_text = String::new();
        let capture = telemetry::capture_content();

        for _ in 0..MAX_TOOL_TURNS {
            let req = model
                .completion_request(prompt.clone())
                .preamble(system.clone())
                .messages(history.clone())
                .tools(tool_defs.clone())
                .temperature(COMPLETION_TEMPERATURE);

            let input_json = capture.then(|| openai_format_messages(&system, &history, &prompt));
            let response =
                send_completion(req, &self.model, remote_cx, input_json.as_deref()).await?;

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
            history.push(Message::Assistant {
                id: None,
                content: response.choice.clone(),
            });

            let mut next_prompt = None;
            let num_calls = tool_calls.len();
            for (i, tc) in tool_calls.iter().enumerate() {
                if let Some(tx) = &progress {
                    let _ = tx.send(format!(
                        "{}: {}",
                        tc.function.name,
                        extract_preview(&tc.function.arguments)
                    ));
                }

                let result = toolset
                    .call(&tc.function.name, tc.function.arguments.to_string())
                    .await;
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
                 using only the information already gathered above. Respond with plain text only.",
            );
            let input_json = capture.then(|| openai_format_messages(&system, &history, &nudge));
            let req = model
                .completion_request(nudge.clone())
                .preamble(system.clone())
                .messages(history.clone())
                .temperature(COMPLETION_TEMPERATURE);

            let response =
                send_completion(req, &self.model, remote_cx, input_json.as_deref()).await?;
            final_text = strip_tool_markup(&assistant_text(&response));
        }

        if final_text.is_empty() {
            final_text = "Reached the maximum number of steps without a final answer.".into();
        }

        Ok(final_text)
    }
}

impl Default for RepoWatchAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentExecutor for RepoWatchAgent {
    fn execute(
        &self,
        ctx: ExecutorContext,
    ) -> BoxStream<'static, Result<StreamResponse, A2AError>> {
        // Join the caller's W3C trace (the platform forwards `traceparent` through the
        // agent proxy/orchestrator). Without adopting it, the OTel SDK mints a fresh root
        // trace id per request and the control plane's session-trace view can't find this
        // agent's spans.
        let remote_cx = ctx
            .service_params
            .get("traceparent")
            .and_then(|v| v.first())
            .and_then(|tp| telemetry::remote_context_from_traceparent(tp));

        let task_id = ctx.task_id.clone();
        let context_id = ctx.context_id.clone();
        let user_text = extract_user_text(&ctx);

        // GenAI agent-span semconv: the A2A request/response is the agent invocation, so
        // record it as invoke_agent with the exchanged messages (content gated by the
        // platform capture flag). `session.id` lets the control plane find this trace by
        // A2A contextId directly in Tempo.
        let agent_name =
            std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| env!("CARGO_PKG_NAME").into());
        let span = tracing::info_span!(
            "a2a.execute",
            otel.kind = "server",
            gen_ai.operation.name = "invoke_agent",
            gen_ai.agent.name = %agent_name,
            session.id = %context_id,
            gen_ai.input.messages = tracing::field::Empty,
            gen_ai.output.messages = tracing::field::Empty,
        );
        if let Some(ref cx) = remote_cx {
            use tracing_opentelemetry::OpenTelemetrySpanExt as _;
            span.set_parent(cx.clone());
        }
        let capture = telemetry::capture_content();
        if capture && !user_text.is_empty() {
            span.record(
                "gen_ai.input.messages",
                telemetry::genai_text_message("user", &user_text)
                    .to_string()
                    .as_str(),
            );
        }
        // Decide delivery from the request in code, not via an LLM tool call: a model that
        // ends its turn with the digest as plain text (as DeepSeek does) would end the tool
        // loop before ever calling a notify tool, silently dropping the request. Detecting the
        // intent here and posting deterministically after the digest mirrors the scheduler.
        let wants_slack = mentions_slack(&user_text);
        let agent = self.clone();
        let record_span = span.clone();

        let stream = async_stream::stream! {
            yield Ok(status_working(&task_id, &context_id, None));

            // Bridge `run_digest`'s progress channel to live A2A status events: poll the
            // digest future and the channel side by side so a tool-call update can be yielded
            // as soon as it's sent, without the digest logic itself knowing about A2A.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let digest = agent.run_digest(user_text, remote_cx.as_ref(), Some(tx));
            tokio::pin!(digest);

            loop {
                tokio::select! {
                    Some(message) = rx.recv() => {
                        yield Ok(status_working(&task_id, &context_id, Some(&message)));
                    }
                    result = &mut digest => {
                        match result {
                            Ok(digest_text) => {
                                let reply = if wants_slack {
                                    yield Ok(status_working(
                                        &task_id, &context_id, Some("posting digest to Slack"),
                                    ));
                                    deliver_to_slack(&digest_text).await
                                } else {
                                    digest_text
                                };
                                yield Ok(artifact_event(&task_id, &context_id, &reply));
                                if capture && !reply.is_empty() {
                                    record_span.record(
                                        "gen_ai.output.messages",
                                        telemetry::genai_text_message("assistant", &reply)
                                            .to_string()
                                            .as_str(),
                                    );
                                }
                                yield Ok(status_completed(&task_id, &context_id));
                            }
                            Err(e) => yield Ok(status_failed(&task_id, &context_id, &e)),
                        }
                        break;
                    }
                }
            }
        };

        // Poll the stream inside `span` so every span created during execution
        // (ChatCompletion, tool calls) lands under the remote parent — even though the body
        // streams after the HTTP handler has returned. (tracing's Instrumented wraps Futures,
        // not Streams, so instrument each item-poll future rather than the stream itself.)
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

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Whether the request asks for the digest to go to Slack. Deliberately a simple keyword check:
/// the delivery decision must not depend on the LLM (which may end its turn with plain text and
/// never call a notify tool), so it's made here in code from the user's own words.
fn mentions_slack(user_text: &str) -> bool {
    user_text.to_lowercase().contains("slack")
}

/// Posts an already-generated digest to Slack and returns the chat reply to show: a short
/// confirmation on success, or the digest itself plus the failure reason so the work isn't lost.
async fn deliver_to_slack(digest: &str) -> String {
    let comment = format!("Repo digest — {}", chrono::Utc::now().to_rfc3339());
    match slack::post_markdown_file(digest, "repo-digest.md", &comment).await {
        Ok(()) => format!("✅ Posted the digest to Slack.\n\n{digest}"),
        Err(e) => format!("{digest}\n\n⚠️ Could not post to Slack: {e}"),
    }
}

/// The instruction prompt that steers the whole digest: window survey → risky-commit triage →
/// deep-dive, then the four required output sections. `now` and the derived 12h window are
/// injected so the model never has to guess the current time.
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
The window-survey tools (list_commits, compare_diff, search_prs) take `repos` as a list of \
\"owner/name\" strings (e.g. [\"Nasiko-Labs/nasiko-cloud-rs\"]) — pass every repo the user \
asked about in one call. The per-commit tools (get_commit, read_file, find_references) take a \
single `repo` string.\n\n\
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
get_commit(repo, commit_hash) for each commit in the window (it returns that commit's file \
list + parent_hash cheaply). Commits touching none of those areas need NO deep-dive — the \
window summary is enough for them.\n\n\
STEP 3 — Deep-dive ONLY the risky commits. For each risky commit, for its SIGNIFICANT changed \
files (cap ~5 most-changed per commit — say when you rely on stats-only for the rest, never \
skip silently): call read_file(repo, path, git_ref) TWICE — at the commit hash (AFTER) and at \
parent_hash (BEFORE) — and read the real code, not just the patch. Then for each changed \
symbol call find_references(repo, symbol) to ground what else is impacted.\n\n\
Then respond with a markdown digest (note which repo each item belongs to when covering more \
than one). Sections:\n\
## New Commits\n\
One line per commit: short hash, message, author.\n\
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
Just produce the digest as your reply — delivery (showing it in chat or posting it to Slack) \
is handled for you; do not worry about where it goes.\n\n\
If the user explicitly names a single commit, skip the window survey and deep-dive just that \
commit (get_commit → read_file at the commit hash and parent_hash → find_references).\n\n\
If there is no activity in the window at all, say so plainly instead of forcing the sections. \
You MUST use tools for every claim; never answer from memory."
    )
}

/// Sends one completion request, joining the caller's trace and recording GenAI token usage on
/// the span so the platform's cost dashboards see it. When `input_messages` is `Some` (the
/// caller checked the platform capture flag), the exchanged messages are recorded on the span
/// per the GenAI semconv (`gen_ai.input.messages` / `gen_ai.output.messages`).
#[tracing::instrument(name = "ChatCompletion", skip_all, fields(
    gen_ai.operation.name = "chat",
    gen_ai.provider.name = "openai",
    gen_ai.request.model = %model_name,
    gen_ai.usage.input_tokens = tracing::field::Empty,
    gen_ai.usage.output_tokens = tracing::field::Empty,
    gen_ai.input.messages = tracing::field::Empty,
    gen_ai.output.messages = tracing::field::Empty,
))]
async fn send_completion(
    req: CompletionRequestBuilder<openai::CompletionModel>,
    model_name: &str,
    parent_cx: Option<&opentelemetry::Context>,
    input_messages: Option<&[serde_json::Value]>,
) -> Result<CompletionResponse<openai::CompletionResponse>, String> {
    // The remote parent must be set on THIS span explicitly: contextual inheritance from
    // a2a.execute strands the span in an orphan trace — tracing-opentelemetry children inherit
    // the parent's originally sampled (local) trace id, not the one `set_parent` re-homed it to.
    if let Some(cx) = parent_cx {
        use tracing_opentelemetry::OpenTelemetrySpanExt as _;
        tracing::Span::current().set_parent(cx.clone());
    }
    if let Some(messages) = input_messages {
        tracing::Span::current().record(
            "gen_ai.input.messages",
            telemetry::genai_input_messages(messages)
                .to_string()
                .as_str(),
        );
    }

    let response = req
        .send()
        .await
        .map_err(|e| format!("LLM API error: {e}"))?;

    if let Some(usage) = &response.raw_response.usage {
        let span = tracing::Span::current();
        span.record("gen_ai.usage.input_tokens", usage.prompt_tokens as u64);
        span.record(
            "gen_ai.usage.output_tokens",
            usage.total_tokens.saturating_sub(usage.prompt_tokens) as u64,
        );
    }

    if input_messages.is_some() {
        let mut text_parts = Vec::new();
        let mut calls_json = Vec::new();
        for content in response.choice.iter() {
            match content {
                AssistantContent::Text(t) => text_parts.push(t.text.clone()),
                AssistantContent::ToolCall(tc) => calls_json.push(serde_json::json!({
                    "id": tc.id,
                    "function": {"name": tc.function.name, "arguments": tc.function.arguments.to_string()},
                })),
                AssistantContent::Reasoning(_) | AssistantContent::Image(_) => {}
            }
        }
        let finish_reason = if calls_json.is_empty() {
            "stop"
        } else {
            "tool_call"
        };
        tracing::Span::current().record(
            "gen_ai.output.messages",
            telemetry::genai_output_message(&text_parts.join("\n"), &calls_json, finish_reason)
                .to_string()
                .as_str(),
        );
    }

    Ok(response)
}

/// Convert the rig conversation (preamble + history + current prompt) into OpenAI-format chat
/// messages — the shape `telemetry::genai_input_messages` expects — for span content capture.
/// Non-text content (images, audio, documents) is skipped: this agent is text-only.
fn openai_format_messages(
    preamble: &str,
    history: &[Message],
    prompt: &Message,
) -> Vec<serde_json::Value> {
    let mut out = vec![serde_json::json!({"role": "system", "content": preamble})];
    for msg in history.iter().chain(std::iter::once(prompt)) {
        match msg {
            Message::System { content } => {
                out.push(serde_json::json!({"role": "system", "content": content}));
            }
            Message::User { content } => {
                for item in content.iter() {
                    match item {
                        UserContent::Text(t) => {
                            out.push(serde_json::json!({"role": "user", "content": t.text}));
                        }
                        UserContent::ToolResult(tr) => {
                            let text = tr
                                .content
                                .iter()
                                .filter_map(|c| match c {
                                    ToolResultContent::Text(t) => Some(t.text.as_str()),
                                    ToolResultContent::Image(_) => None,
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            out.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": tr.id,
                                "content": text,
                            }));
                        }
                        _ => {}
                    }
                }
            }
            Message::Assistant { content, .. } => {
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();
                for item in content.iter() {
                    match item {
                        AssistantContent::Text(t) => text_parts.push(t.text.clone()),
                        AssistantContent::ToolCall(tc) => {
                            tool_calls.push(serde_json::json!({
                                "id": tc.id,
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments.to_string(),
                                },
                            }));
                        }
                        AssistantContent::Reasoning(_) | AssistantContent::Image(_) => {}
                    }
                }
                let mut msg = serde_json::json!({
                    "role": "assistant",
                    "content": text_parts.join("\n"),
                });
                if !tool_calls.is_empty() {
                    msg["tool_calls"] = serde_json::Value::Array(tool_calls);
                }
                out.push(msg);
            }
        }
    }
    out
}

/// Concatenates the text parts of the incoming A2A message (ignoring non-text parts).
fn extract_user_text(ctx: &ExecutorContext) -> String {
    ctx.message
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
        .unwrap_or_default()
}

/// Joins the text parts of an assistant completion response.
fn assistant_text(response: &CompletionResponse<openai::CompletionResponse>) -> String {
    response
        .choice
        .iter()
        .filter_map(|c| match c {
            AssistantContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A short human-readable status line for a tool call. The window tools carry a `repos` array;
/// the per-commit tools carry a single `repo` plus a path/symbol/commit_hash detail.
fn extract_preview(args: &serde_json::Value) -> String {
    let field = |k: &str| args.get(k).and_then(|v| v.as_str());

    if let Some(repos) = args.get("repos").and_then(|v| v.as_array())
        && !repos.is_empty()
    {
        return repos
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(", ");
    }

    if let Some(repo) = field("repo") {
        if let Some(path) = field("path") {
            return format!("{repo} {path}");
        }
        if let Some(symbol) = field("symbol") {
            return format!("{repo} `{symbol}`");
        }
        if let Some(commit_hash) = field("commit_hash") {
            return format!("{repo} @{}", short_hash(commit_hash));
        }
        return repo.to_string();
    }

    "...".to_string()
}

/// DeepSeek sometimes emits its internal tool-call markup (`<｜DSML｜…`) as plain content
/// instead of structured tool_calls. Anything from the first marker onward is machinery, not
/// an answer — cut it so an all-markup response reads as empty and triggers the forced-answer
/// fallback.
fn strip_tool_markup(content: &str) -> String {
    match content.find("<｜") {
        Some(idx) => content[..idx].trim().to_string(),
        None => content.trim().to_string(),
    }
}

/// The A2A card this agent advertises (served at `/.well-known/agent-card.json`). Static
/// metadata only — read the `AgentExecutor` impl above to understand how the agent behaves.
pub fn agent_card(port: u16) -> AgentCard {
    AgentCard {
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
        supported_interfaces: vec![AgentInterface::new(
            format!("http://0.0.0.0:{port}/"),
            TRANSPORT_PROTOCOL_JSONRPC,
        )],
        security_schemes: None,
        security_requirements: None,
        documentation_url: None,
        icon_url: None,
        signatures: None,
    }
}

// ─── A2A stream-event constructors ────────────────────────────────────────────

fn artifact_event(task_id: &str, context_id: &str, text: &str) -> StreamResponse {
    StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        artifact: Artifact {
            artifact_id: new_artifact_id(),
            name: None,
            description: None,
            parts: vec![Part::text(text)],
            metadata: None,
            extensions: None,
        },
        append: Some(false),
        last_chunk: Some(true),
        metadata: None,
    })
}

fn status_working(task_id: &str, context_id: &str, message: Option<&str>) -> StreamResponse {
    status_event(task_id, context_id, TaskState::Working, message)
}

fn status_completed(task_id: &str, context_id: &str) -> StreamResponse {
    status_event(task_id, context_id, TaskState::Completed, None)
}

fn status_failed(task_id: &str, context_id: &str, error: &str) -> StreamResponse {
    status_event(task_id, context_id, TaskState::Failed, Some(error))
}

fn status_event(
    task_id: &str,
    context_id: &str,
    state: TaskState,
    message: Option<&str>,
) -> StreamResponse {
    StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
        task_id: task_id.into(),
        context_id: context_id.into(),
        status: TaskStatus {
            state,
            message: message.map(|text| A2aMessage {
                message_id: new_message_id(),
                context_id: Some(context_id.into()),
                task_id: Some(task_id.into()),
                role: Role::Agent,
                parts: vec![Part::text(text)],
                metadata: None,
                extensions: None,
                reference_task_ids: None,
            }),
            timestamp: Some(chrono::Utc::now()),
        },
        metadata: None,
    })
}
