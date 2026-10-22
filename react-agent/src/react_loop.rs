use std::sync::Arc;

use crate::completion::{CompletionEvent, UsageModel};
use futures::StreamExt;
use rig::completion::message::ToolCall;
use rig::completion::{AssistantContent, CompletionModel as _, Message, ToolDefinition};
use rig::tool::{ToolDyn, ToolError, ToolSet, ToolSetError};
use tokio::sync::mpsc;

use crate::a2a::{A2aClient, PauseInfo};
use crate::context::{ContextConfig, ContextManager};
use crate::error::OrchestratorError;
use crate::events::{OrchestratorEvent, PolicyRejectionKind};
use crate::guard::CallGuard;
use crate::policy::DelegationPolicy;
use crate::registry::{AgentInfo, AgentRegistry, RegistrySource};
use crate::tool::{A2aTool, A2aToolError};
use nasiko_types::agent_call::AgentCallHandle;

/// The outcome of one `toolset.call()`, with a real pause recovered from `rig`'s type-erased
/// `Result<String, ToolSetError>` instead of being indistinguishable from an ordinary failure.
/// Pure — no side effects, no channel sends — so every `toolset.call()` site (`Orchestrator::run()`
/// and both of `run_stream_inner()`'s) can classify identically by calling the same function,
/// rather than each independently deciding what counts as a pause.
///
/// Deliberately not `Success(String)`/`Failure(String)` variants carrying the `Ok`/`Err` payload:
/// every call site already has its own existing, unchanged handling for the non-pause case
/// (`match result { Ok(..) => .., Err(..) => .. }`, untouched by this classification) — a
/// `NotAwaitingHuman` outcome that duplicated that payload would just be dead weight nothing
/// reads, which is exactly what the compiler's `dead_code` lint caught on the first version of
/// this enum.
enum ToolOutcome {
    AwaitingHuman {
        agent: String,
        agent_id: String,
        pause: PauseInfo,
    },
    NotAwaitingHuman,
}

/// Recovers `A2aToolError::AwaitingHuman` through `rig-core`'s type erasure: `ToolSetError`
/// wraps `ToolError::ToolCallError(Box<dyn std::error::Error + Send + Sync>)`, built by `rig`'s
/// own blanket `ToolDyn` impl from the tool's real `A2aToolError`. `downcast_ref` recovers the
/// concrete type from that box — proven against the actual pinned `rig-core` dependency in
/// `tool.rs`'s `awaiting_human_survives_rig_toolset_erasure` test, not assumed here.
fn classify_tool_result(result: &Result<String, ToolSetError>) -> ToolOutcome {
    if let Err(ToolSetError::ToolCallError(ToolError::ToolCallError(boxed))) = result
        && let Some(A2aToolError::AwaitingHuman {
            agent,
            agent_id,
            pause,
        }) = boxed.downcast_ref::<A2aToolError>()
    {
        return ToolOutcome::AwaitingHuman {
            agent: agent.clone(),
            agent_id: agent_id.clone(),
            pause: pause.clone(),
        };
    }
    ToolOutcome::NotAwaitingHuman
}

/// End one agent call's in-flight handle (see `CallGuard::before_call`) once `toolset.call()`
/// has returned. A pause is the agent answering with a question, so only an ordinary error
/// counts as a failed call. The handle is dropped here, which is what ends the call's span.
fn finish_agent_call(
    handle: Option<Box<dyn AgentCallHandle>>,
    result: &Result<String, ToolSetError>,
) {
    let Some(mut handle) = handle else {
        return;
    };
    let paused = matches!(
        classify_tool_result(result),
        ToolOutcome::AwaitingHuman { .. }
    );
    handle.set_succeeded(result.is_ok() || paused);
}

/// What the policy + call-guard gate decided for one tool call.
enum CallAdmission {
    /// Already reported on `tx` and pushed onto `results_for_context`; skip the call.
    Blocked,
    /// Cleared every gate. Carries the guard's in-flight handle (`None` without a guard),
    /// held across the call and passed to `finish_agent_call` once it returns.
    Admitted(Option<Box<dyn AgentCallHandle>>),
}

/// Attribute one completion's total token cost evenly across the tool calls
/// it produced — the API gives one usage figure per completion, not per tool
/// call, so this is the best available granularity for `CallGuard::after_call`.
/// `None` (no usage reported) or zero tool calls both yield 0.
fn tokens_per_tool_call(completion_tokens: Option<u64>, num_tool_calls: usize) -> u64 {
    completion_tokens
        .map(|t| t / num_tool_calls.max(1) as u64)
        .unwrap_or(0)
}

/// Does the operator's policy refuse this call? `Some(reason)` if so.
///
/// The decision only — no reporting — because the two loops report it in
/// different shapes (`run_stream_inner` sends an event, `run` records a trace)
/// and only the *decision* has to stay identical between them.
fn policy_refusal(config: &OrchestratorConfig, tc: &ToolCall) -> Option<String> {
    config
        .policy
        .as_ref()
        .and_then(|p| p.check_tool_call(&tc.function.arguments).err())
}

/// What the model is told when a call is blocked, in the tool-result context.
///
/// One wording for both loops. They had drifted — one said `Blocked:` and the
/// other `BLOCKED: … Do NOT retry`, so the same policy taught the model two
/// different lessons depending on which entry point ran it.
fn blocked_note(tool_name: &str, reason: &str) -> String {
    format!(
        "[{tool_name}] BLOCKED: {reason}. Do NOT retry this agent with different \
         arguments unless you have a concrete reason to."
    )
}

/// The policy + call-guard gate for one tool call, in `run_stream_inner`'s
/// event-channel reporting style (`OrchestratorEvent::PolicyRejected` plus a
/// message telling the model not to retry). Both of its branches call this —
/// they differ in how a turn gets here, not in what a rejection looks like once
/// it has, and this used to be a byte-for-byte copy in each, with no structural
/// signal that a change to one needed the other.
///
/// `CallAdmission::Blocked` means the call was blocked (already reported on `tx`
/// and already pushed onto `results_for_context`), so the caller should
/// `continue` its loop rather than call the tool.
async fn admit_call(
    tc: &ToolCall,
    agent_display: &str,
    config: &OrchestratorConfig,
    guard: Option<&dyn CallGuard>,
    tx: &mpsc::Sender<OrchestratorEvent>,
    turn_idx: usize,
    results_for_context: &mut Vec<String>,
) -> CallAdmission {
    let name = &tc.function.name;

    // Operator policy first, before the flow guard: a call the policy rejects
    // should never consume fan-out or depth budget, and `before_call`
    // increments both.
    if let Some(reason) = policy_refusal(config, tc) {
        let _ = tx
            .send(OrchestratorEvent::PolicyRejected {
                agent: agent_display.to_string(),
                reason: reason.clone(),
                turn: turn_idx + 1,
                kind: PolicyRejectionKind::Delegation,
            })
            .await;
        results_for_context.push(blocked_note(name, &reason));
        return CallAdmission::Blocked;
    }

    let Some(g) = guard else {
        return CallAdmission::Admitted(None);
    };
    match g.before_call(agent_display).await {
        Ok(handle) => CallAdmission::Admitted(Some(handle)),
        Err(reason) => {
            let _ = tx
                .send(OrchestratorEvent::PolicyRejected {
                    agent: agent_display.to_string(),
                    reason: reason.clone(),
                    turn: turn_idx + 1,
                    kind: PolicyRejectionKind::FlowGuard,
                })
                .await;
            results_for_context.push(blocked_note(name, &reason));
            CallAdmission::Blocked
        }
    }
}

/// The orchestrator's system prompt. One builder for both loops — `run()` and
/// `run_stream_inner()` previously carried byte-identical copies of this text,
/// so a change to the policy had to be made twice to take effect.
///
/// The roster/protocol/rules block is the stable prefix and the two
/// caller-supplied sections come last, so OpenAI's prefix-based prompt caching
/// hits across turns even when those sections change. `{custom}` (e.g. the L1A
/// supplemental context built in `oss/server/src/router/a2a_dispatch.rs`) is a
/// query-ranked top-k slice, so it varies turn to turn; it used to sit at the
/// very top, where any variation invalidated the whole cached prompt.
///
/// `{policy_footer}` is placed after `{custom}` even though it is the more
/// stable of the two, because it is the one section whose purpose depends on
/// being read last: a rule about HOW to answer was reliably ignored when it sat
/// earlier (see `DelegationPolicy::preamble_footer`). The cost is that the
/// org-rules block — at most fifty short lines — falls outside the cached
/// prefix, while the far larger stable block ahead of it still caches. Ordering
/// them the other way would re-bury the rules behind a `{custom}` that grows
/// with the size of the fleet.
fn build_preamble(config: &OrchestratorConfig, agents: &[AgentInfo]) -> String {
    let agent_list: String = agents
        .iter()
        .map(|a| {
            let skills = a
                .skills
                .iter()
                .map(|s| {
                    // The skill's own documented inputs. Without them the model invents wording
                    // for a skill that may only answer to an exact phrase — and the agent then
                    // answers a question the user never asked.
                    let examples = if s.examples.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "\n      send exactly: {}",
                            s.examples
                                .iter()
                                .map(|e| format!("\"{e}\""))
                                .collect::<Vec<_>>()
                                .join(" | ")
                        )
                    };
                    format!("    - {}: {}{}", s.name, s.description, examples)
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "  • {} (tool: `{}`)\n    {}\n{}",
                a.name,
                A2aTool::tool_name(&a.name),
                a.description,
                skills
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    // Each section carries its OWN separator rather than taking one from the
    // template, so an absent section leaves nothing behind — not even the blank
    // line a `{placeholder}` wrapped in newlines would. With no policy and no
    // caller preamble the prompt below carries no trace that either seam exists,
    // which `no_policy_leaves_the_preamble_untouched` pins against the exact
    // joins rather than against trimmed text. (It no longer matches the prompt
    // from before the seam byte for byte — `{custom}` has since moved from the
    // top to the tail for prompt-cache stability, see this function's docs.)
    // `custom` carries its own separator for the same reason the policy sections
    // below do: an absent section must leave nothing behind, not even the blank
    // line a `{placeholder}` wrapped in newlines would.
    let custom = match config.preamble.as_deref() {
        Some(text) if !text.trim().is_empty() => format!("\n\n{text}"),
        _ => String::new(),
    };

    let policy = config.policy.as_ref();
    let delegation_policy = match policy.map(|p| p.preamble_policy()) {
        Some(text) if !text.trim().is_empty() => format!("{text}\n\n"),
        _ => String::new(),
    };
    let policy_footer = match policy.map(|p| p.preamble_footer()) {
        Some(text) if !text.trim().is_empty() => format!("\n\n{text}"),
        _ => String::new(),
    };

    format!(
        r#"You are a ReAct orchestrator. Fulfill user requests by reasoning and delegating to specialized agents.

## Available Agents

{agent_list}

{delegation_policy}## Protocol

1. Analyze the user's request. Determine which agent(s) can help.
2. Call the appropriate agent tool with a clear, specific message.
3. If the task requires multiple agents, call them sequentially — use earlier results to inform later calls.
4. Once you have enough information, respond with a complete answer as plain text (no tool call).
5. If an agent fails, reason about alternatives or inform the user.

## Rules

- Only relay facts from agent responses. Never fabricate.
- Prefer the most specific agent for each sub-task.
- Pass the user's own wording through when the request is itself the thing to relay — an exact
  phrase, a quoted string, a command, an identifier, a fixed test input. Paraphrasing it loses
  information the agent matches on, and the agent then answers a question the user never asked.
- If no agent fits, tell the user directly.
- When calling an agent tool, call it directly — do not first restate its message as your own chat reply. If that agent pauses to ask the user something, your own words would otherwise repeat the same question twice.{custom}{policy_footer}"#
    )
}

/// The last gate before a final answer reaches the user: the configured policy,
/// if any, gets to replace it.
///
/// Returns the text to actually send, so callers cannot forget to use the
/// result. With no policy the model's own answer is what ships, which is the
/// open-source behaviour.
fn finalize_answer(
    text: &str,
    config: &OrchestratorConfig,
    agents: &[AgentInfo],
    delegated: bool,
    turn_idx: usize,
) -> String {
    match &config.policy {
        Some(policy) => policy.review_final_answer(text, agents, delegated, turn_idx),
        None => text.to_string(),
    }
}

/// Configuration for the orchestrator.
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    pub max_turns: usize,
    pub model: String,
    pub preamble: Option<String>,
    pub context: ContextConfig,
    pub temperature: Option<f64>,
    /// OpenAI-compatible base URL. If None, uses OPENAI_BASE_URL env var.
    pub base_url: Option<String>,
    /// API key. If None, uses OPENAI_API_KEY env var.
    pub api_key: Option<String>,
    /// Operator policy governing what this orchestration is allowed to do —
    /// see [`DelegationPolicy`]. `None` imposes nothing, which is what an
    /// unconfigured deployment runs with.
    ///
    /// Separate from `preamble` on purpose: `preamble` is the caller's own
    /// framing of what this orchestrator is for, while a policy is operator
    /// rules that outlive any one caller.
    pub policy: Option<Arc<dyn DelegationPolicy>>,
    /// Text sent with every agent call this orchestration makes, as its own
    /// message part after the model's own message. `None` sends the model's
    /// own wording unchanged.
    ///
    /// Distinct from `preamble`, which reaches only this orchestrator's own
    /// system prompt: a minted sub-leg is a fresh conversation with a different
    /// model that never sees the preamble, so content that has to hold for the
    /// *agents* has to travel on the wire to each of them. The loop carries it
    /// as a resolved string rather than a callback because it is by definition
    /// the same for every leg — resolving it once per turn is what keeps a
    /// fan-out from paying for it N times.
    pub outbound_prefix: Option<String>,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_turns: 15,
            model: "gpt-5.5".to_string(),
            preamble: None,
            context: ContextConfig::default(),
            temperature: Some(0.2),
            base_url: None,
            api_key: None,
            policy: None,
            outbound_prefix: None,
        }
    }
}

/// Trace of a single ReAct turn for observability.
#[derive(Debug, Clone)]
pub struct TurnTrace {
    pub turn: usize,
    pub tool_calls: Vec<ToolCallTrace>,
    pub text_response: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolCallTrace {
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub result: Result<String, String>,
}

/// Result returned from a completed orchestration.
#[derive(Debug, Clone)]
pub struct OrchestrationResult {
    pub response: String,
    pub turns: Vec<TurnTrace>,
    pub context_compacted: bool,
}

/// The ReAct orchestrator. Holds the registry, context, and LLM config.
pub struct Orchestrator {
    config: OrchestratorConfig,
    registry: AgentRegistry,
    a2a_client: Arc<A2aClient>,
    context: ContextManager,
    guard: Option<Arc<dyn CallGuard>>,
    model: Result<UsageModel, String>,
}

impl Orchestrator {
    pub fn new(config: OrchestratorConfig, registry_source: RegistrySource) -> Self {
        let a2a_client = Arc::new(A2aClient::new());
        let registry = AgentRegistry::new(registry_source);
        let context = ContextManager::new(config.context.clone());
        let model = UsageModel::from_config(&config);
        Self {
            model,
            config,
            registry,
            a2a_client,
            context,
            guard: None,
        }
    }

    /// This turn's IP-3 measurement (tool-result bytes in and out), shared with the clone
    /// `run_stream` hands to its spawned task — so a streaming turn's compression is visible here
    /// even though nothing else about that turn's context is. Take it before the run, read it
    /// after.
    pub fn compression_tally(&self) -> crate::context::CompressionTally {
        self.context.compression_tally()
    }

    pub fn with_a2a_client(mut self, client: A2aClient) -> Self {
        self.a2a_client = Arc::new(client);
        self
    }

    pub fn with_guard(mut self, guard: Arc<dyn CallGuard>) -> Self {
        self.guard = Some(guard);
        self
    }

    /// Discover agents from the registry. Call before `run()`.
    pub async fn init(&self) -> Result<Vec<AgentInfo>, OrchestratorError> {
        let agents = self
            .registry
            .discover()
            .await
            .map_err(|e| OrchestratorError::Registry(e.to_string()))?;

        tracing::info!(count = agents.len(), "agents discovered");
        for a in &agents {
            tracing::debug!(name = %a.name, endpoint = %a.endpoint, "registered agent");
        }
        Ok(agents)
    }

    /// Run the ReAct loop for a user query.
    pub async fn run(
        &mut self,
        user_query: &str,
    ) -> Result<OrchestrationResult, OrchestratorError> {
        self.context.push_user(user_query);

        if self.context.needs_compaction() {
            self.context.compact_simple();
            tracing::info!(
                tokens = self.context.estimated_tokens(),
                "context compacted"
            );
        }

        let agents = self.registry.agents().await;
        if agents.is_empty() {
            return Err(OrchestratorError::NoAgents);
        }

        let model = self.build_model()?;
        let (toolset, tool_defs) = self.build_tools(&agents).await;
        let preamble = build_preamble(&self.config, &agents);

        let mut turns = Vec::new();
        let mut context_compacted = false;
        let mut delegated = false;

        // Preamble is STABLE across turns — provider can cache this prefix.
        // All dynamic context goes into user messages instead.
        for turn_idx in 0..self.config.max_turns {
            let window = self.context.window();

            // Build the user prompt with context embedded (changes each turn)
            let user_prompt = if turn_idx == 0 && window.summary.is_none() {
                user_query.to_string()
            } else {
                let ctx = window.format_for_prompt();
                format!(
                    "{ctx}\n\nCurrent request: {user_query}\n\n\
                     Based on the above context and tool results, continue. \
                     If you have enough information, respond with your final answer (no tool call)."
                )
            };

            let mut req = model
                .completion_request(Message::user(&user_prompt))
                .preamble(preamble.clone())
                .tools(tool_defs.clone());

            // Nothing the policy will accept from an undelegated turn is long,
            // and anything long is about to be discarded — so stop paying to
            // generate it. Sized against the query, because this is also the
            // turn that relays it into a tool call.
            if !delegated
                && let Some(cap) = self
                    .config
                    .policy
                    .as_ref()
                    .and_then(|p| p.undelegated_max_tokens(user_query.chars().count()))
            {
                req = req.max_tokens(cap);
            }

            if let Some(temp) = self.config.temperature {
                req = req.temperature(temp);
            }

            let response = req
                .send()
                .await
                .map_err(|e| OrchestratorError::Completion(e.to_string()))?;

            // This completion's total token cost, attributed evenly across
            // however many tool calls it produced (best available granularity
            // — the API gives one usage figure per completion, not per tool
            // call). Without this, `after_call` always received a literal 0
            // and `FlowGuard::record_tokens`/`TokenBudgetExhausted` could
            // never fire.
            let completion_tokens = response.raw_response.usage.as_ref().map(|u| u.total_tokens);

            // Partition the response into text and tool calls
            let mut text_parts = Vec::new();
            let mut tool_calls = Vec::new();

            for content in response.choice.iter() {
                match content {
                    AssistantContent::Text(t) => text_parts.push(t.text.clone()),
                    AssistantContent::ToolCall(tc) => tool_calls.push(tc.clone()),
                }
            }

            // If there are tool calls, execute them all
            if !tool_calls.is_empty() {
                let tokens_per_call = tokens_per_tool_call(completion_tokens, tool_calls.len());
                let mut trace = TurnTrace {
                    turn: turn_idx + 1,
                    tool_calls: Vec::new(),
                    text_response: if text_parts.is_empty() {
                        None
                    } else {
                        Some(text_parts.join("\n"))
                    },
                };

                let mut results_for_context = Vec::new();

                for tc in &tool_calls {
                    let name = &tc.function.name;
                    let args_str = tc.function.arguments.to_string();

                    let agent_display = name
                        .strip_prefix("call_agent_")
                        .unwrap_or(name)
                        .replace('_', "-");

                    // Operator policy first — see the streaming path's own note.
                    // Shares `policy_refusal`/`blocked_note` with that path, so
                    // the two entry points cannot reach different verdicts or
                    // teach the model different lessons about the same block.
                    if let Some(reason) = policy_refusal(&self.config, tc) {
                        tracing::warn!(tool = %name, %reason, "delegation policy blocked");
                        trace.tool_calls.push(ToolCallTrace {
                            tool_name: name.clone(),
                            arguments: tc.function.arguments.clone(),
                            result: Err(reason.clone()),
                        });
                        results_for_context.push(blocked_note(name, &reason));
                        continue;
                    }

                    // Enforce call guard
                    let mut call_handle = None;
                    if let Some(g) = &self.guard {
                        match g.before_call(&agent_display).await {
                            Ok(handle) => call_handle = Some(handle),
                            Err(reason) => {
                                tracing::warn!(tool = %name, %reason, "call guard blocked");
                                trace.tool_calls.push(ToolCallTrace {
                                    tool_name: name.clone(),
                                    arguments: tc.function.arguments.clone(),
                                    result: Err(format!("blocked: {}", reason)),
                                });
                                results_for_context.push(format!("[{}] Blocked: {}", name, reason));
                                continue;
                            }
                        }
                    }

                    tracing::info!(turn = turn_idx + 1, tool = %name, "executing tool");

                    let result = toolset.call(name, args_str).await;
                    finish_agent_call(call_handle, &result);

                    // A pause is not a tool outcome to trace or reason over — stop the whole run
                    // immediately, before it's recorded as just another failed/successful call.
                    if let ToolOutcome::AwaitingHuman {
                        agent,
                        agent_id,
                        pause,
                    } = classify_tool_result(&result)
                    {
                        if let Some(g) = &self.guard {
                            g.after_call(&agent_display, tokens_per_call).await;
                        }
                        // Preserve any earlier calls in this same batch that already completed
                        // before this one paused — without this, they're silently discarded here,
                        // since the push_tool_result call below (which normally records the whole
                        // batch) is never reached once we return. `results_for_context.len()` is
                        // exactly the count of `tool_calls` processed so far: every earlier
                        // iteration either pushed a result/error/block entry or hit this same
                        // pause check itself, so the slice lines up with what's actually recorded.
                        if !results_for_context.is_empty() {
                            let completed_names = tool_calls[..results_for_context.len()]
                                .iter()
                                .map(|tc| tc.function.name.as_str())
                                .collect::<Vec<_>>();
                            self.context.push_tool_result(
                                &completed_names,
                                &results_for_context.join("\n\n"),
                            );
                        }
                        return Err(OrchestratorError::AwaitingHuman {
                            agent,
                            agent_id,
                            pause: Box::new(pause),
                        });
                    }

                    let call_trace = ToolCallTrace {
                        tool_name: name.clone(),
                        arguments: tc.function.arguments.clone(),
                        result: result
                            .as_ref()
                            .map(|s| s.clone())
                            .map_err(|e| e.to_string()),
                    };
                    trace.tool_calls.push(call_trace);

                    match result {
                        Ok(output) => {
                            if let Some(g) = &self.guard {
                                g.after_call(&agent_display, tokens_per_call).await;
                            }
                            results_for_context.push(format!("[{}] Result: {}", name, output));
                            delegated = true;
                        }
                        Err(e) => {
                            // Balance the before_call() depth increment even on
                            // failure — otherwise a failed tool call permanently
                            // leaks flow-depth and later legitimate calls in the
                            // same flow get falsely rejected with MaxDepthExceeded.
                            if let Some(g) = &self.guard {
                                g.after_call(&agent_display, tokens_per_call).await;
                            }
                            tracing::warn!(tool = %name, error = %e, "tool failed");
                            results_for_context.push(format!("[{}] Error: {}", name, e));
                        }
                    }
                }

                // Push combined tool results as a single observation
                let combined = results_for_context.join("\n\n");
                self.context.push_tool_result(
                    &tool_calls
                        .iter()
                        .map(|tc| tc.function.name.as_str())
                        .collect::<Vec<_>>(),
                    &combined,
                );

                turns.push(trace);
            } else {
                // No tool calls — this is the final text response
                let final_text = finalize_answer(
                    &text_parts.join("\n"),
                    &self.config,
                    &agents,
                    delegated,
                    turn_idx,
                );
                self.context.push_assistant(&final_text);

                turns.push(TurnTrace {
                    turn: turn_idx + 1,
                    tool_calls: Vec::new(),
                    text_response: Some(final_text.clone()),
                });

                return Ok(OrchestrationResult {
                    response: final_text,
                    turns,
                    context_compacted,
                });
            }

            // Mid-loop compaction check
            if self.context.needs_compaction() {
                self.context.compact_simple();
                context_compacted = true;
                tracing::info!(
                    tokens = self.context.estimated_tokens(),
                    "mid-loop compaction"
                );
            }
        }

        Err(OrchestratorError::MaxTurnsExceeded(self.config.max_turns))
    }

    /// Run the ReAct loop, streaming events to the caller via a channel.
    /// Returns a receiver; the orchestration runs in the background.
    /// `file_parts` are pre-serialized A2A Part JSON values from the user's
    /// upload — forwarded to whichever agent the orchestrator selects.
    ///
    /// The in-memory context this streams against is write-only from the caller's perspective:
    /// `context` below is a clone handed to the spawned task, and every `push_tool_result`/
    /// `push_assistant` inside `run_stream_inner` mutates that clone, which is simply dropped when
    /// the task ends — `self.context` on this `Orchestrator` is never updated by a streaming turn,
    /// including the record that a sub-agent paused awaiting a human (found in review). This is
    /// currently safe only because the one caller that resumes after a streaming pause
    /// (`trigger_new_orchestrator_turn`, `oss/server/src/hitl/mod.rs`) rebuilds its own context
    /// from `SessionHistory::fetch` rather than trusting this `Orchestrator`'s in-memory state — do
    /// not add a caller that relies on `self.context` reflecting a prior `run_stream` call's
    /// effects without first making this shared (e.g. `Arc<Mutex<ContextManager>>`) rather than
    /// cloned.
    pub fn run_stream(
        &mut self,
        user_query: &str,
        file_parts: Vec<nasiko_types::a2a::Part>,
    ) -> mpsc::Receiver<OrchestratorEvent> {
        let (tx, rx) = mpsc::channel(64);
        let query = user_query.to_string();

        // Pre-serialize file parts to JSON once; the A2aTool will include them
        // in every agent call's message.parts alongside the text part.
        let file_parts_json: Vec<serde_json::Value> = file_parts
            .iter()
            .filter_map(|p| serde_json::to_value(p).ok())
            .collect();

        // Clone what we need for the spawned task
        let config = self.config.clone();
        let registry = self.registry.clone();
        let agents_ctx = AgentCallContext {
            a2a_client: self.a2a_client.clone(),
            model: self.model.clone(),
        };
        let mut context = self.context.clone();
        let guard = self.guard.clone();

        use tracing::Instrument;
        tokio::spawn(
            async move {
                let _ = run_stream_inner(
                    &config,
                    &registry,
                    &agents_ctx,
                    &mut context,
                    &query,
                    &tx,
                    guard.as_deref(),
                    &file_parts_json,
                )
                .await;
            }
            .in_current_span(),
        );

        rx
    }

    /// Reset the context for a new conversation.
    pub fn reset_context(&mut self) {
        self.context = ContextManager::new(self.config.context.clone());
    }

    fn build_model(&self) -> Result<UsageModel, OrchestratorError> {
        self.model.clone().map_err(OrchestratorError::LlmConfig)
    }

    async fn build_tools(&self, agents: &[AgentInfo]) -> (ToolSet, Vec<ToolDefinition>) {
        let mut builder = ToolSet::builder();
        let mut defs = Vec::new();

        for agent in agents {
            let tool = A2aTool::new(agent.clone(), self.a2a_client.clone())
                .with_policy(self.config.policy.clone())
                .with_outbound_prefix(self.config.outbound_prefix.clone());
            defs.push(ToolDyn::definition(&tool, String::new()).await);
            builder = builder.static_tool(tool);
        }

        (builder.build(), defs)
    }
}

/// What's needed to actually reach an agent — the shared HTTP client. (Agents
/// carry their own MCP gateway credential; no per-call user token exists.)
struct AgentCallContext {
    a2a_client: Arc<A2aClient>,
    model: Result<UsageModel, String>,
}

/// Inner streaming implementation. Sends events to the channel as orchestration progresses.
#[allow(clippy::too_many_arguments)]
async fn run_stream_inner(
    config: &OrchestratorConfig,
    registry: &AgentRegistry,
    agents_ctx: &AgentCallContext,
    context: &mut ContextManager,
    user_query: &str,
    tx: &mpsc::Sender<OrchestratorEvent>,
    guard: Option<&dyn CallGuard>,
    file_parts: &[serde_json::Value],
) -> Result<(), OrchestratorError> {
    context.push_user(user_query);

    if context.needs_compaction() {
        context.compact_simple();
    }

    let agents = registry.agents().await;
    if agents.is_empty() {
        let _ = tx
            .send(OrchestratorEvent::Error {
                message: "no agents available".into(),
            })
            .await;
        return Err(OrchestratorError::NoAgents);
    }

    let model = match agents_ctx.model.clone() {
        Ok(model) => model,
        Err(message) => {
            let _ = tx
                .send(OrchestratorEvent::Error {
                    message: message.clone(),
                })
                .await;
            return Err(OrchestratorError::LlmConfig(message));
        }
    };

    // Build tools from agents. This is the streaming loop, so each agent call
    // relays the sub-agent's live progress into the event stream.
    let mut builder = ToolSet::builder();
    let mut tool_defs = Vec::new();
    for agent in &agents {
        // Streaming loop: each agent call relays live progress into the stream.
        let tool = A2aTool::new(agent.clone(), agents_ctx.a2a_client.clone())
            .with_progress(tx.clone())
            .with_file_parts(file_parts.to_vec())
            .with_policy(config.policy.clone())
            .with_outbound_prefix(config.outbound_prefix.clone());
        tool_defs.push(ToolDyn::definition(&tool, String::new()).await);
        builder = builder.static_tool(tool);
    }
    let toolset = builder.build();

    let preamble = build_preamble(config, &agents);

    let mut context_compacted = false;
    // Set once any agent call in this run returns successfully. Handed to the
    // policy's final-answer review below, which may treat an answer produced
    // without a single successful call differently — see [`DelegationPolicy`].
    let mut delegated = false;

    for turn_idx in 0..config.max_turns {
        let window = context.window();

        // Restated per turn, not only in the system prompt: the turn that writes
        // the answer is the one that has to obey a rule about HOW to answer, and
        // this is the last text the model reads. Empty when no policy is
        // configured, or when the policy has nothing to restate, so an
        // unconfigured deployment's prompt is byte-identical to before.
        let policy_reminder = config
            .policy
            .as_ref()
            .and_then(|p| p.turn_reminder())
            .unwrap_or("");

        let user_prompt = if turn_idx == 0 && window.summary.is_none() {
            // Turn 0 gets the reminder too. It used to be the bare query, on the
            // assumption that turn 0 only ever plans a tool call and the answer
            // comes later — but a turn that answers immediately lands here, and
            // that is exactly what the HITL resume does (its own prompt tells it
            // not to call anyone again). Observed live: a resumed email turn
            // answered without naming the agent, with a "cite the agent" rule
            // configured, because this branch never mentioned the rules.
            format!("{user_query}{policy_reminder}")
        } else {
            let ctx = window.format_for_prompt();
            format!(
                "{ctx}\n\nCurrent request: {user_query}\n\n\
                 Based on the above context and tool results, continue. \
                 If you have enough information, respond with your final answer (no tool call).\
                 {policy_reminder}"
            )
        };

        // Keep turn 0 responsive unless policy can replace its final answer.
        // Later turns remain buffered so final-answer review precedes delivery.
        // Both paths capture provider usage; neither reissues a completed call.
        let use_non_streaming = turn_idx > 0
            || config
                .policy
                .as_ref()
                .is_some_and(|p| p.buffer_every_turn());

        if use_non_streaming {
            let mut req = model
                .completion_request(Message::user(&user_prompt))
                .preamble(preamble.clone())
                .tools(tool_defs.clone());

            // Nothing the policy will accept from an undelegated turn is long,
            // and anything long is about to be discarded — so stop paying to
            // generate it. Sized against the query, because this is also the
            // turn that relays it into a tool call.
            if !delegated
                && let Some(cap) = config
                    .policy
                    .as_ref()
                    .and_then(|p| p.undelegated_max_tokens(user_query.chars().count()))
            {
                req = req.max_tokens(cap);
            }

            if let Some(temp) = config.temperature {
                req = req.temperature(temp);
            }

            let call_identity = model.call_identity(false);
            let response = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx
                        .send(OrchestratorEvent::Error {
                            message: e.to_string(),
                        })
                        .await;
                    return Err(OrchestratorError::Completion(e.to_string()));
                }
            };

            let completion_tokens = response
                .raw_response
                .usage
                .as_ref()
                .map(|usage| usage.total_tokens);
            if let Some(usage) = response.raw_response.usage {
                let _ = tx.send(OrchestratorEvent::Usage { usage }).await;
            }

            if completion_tokens.is_none() {
                let output_chars = response
                    .choice
                    .iter()
                    .map(|content| match content {
                        AssistantContent::Text(text) => text.text.len(),
                        AssistantContent::ToolCall(tool) => {
                            tool.function.arguments.to_string().len()
                        }
                    })
                    .sum();
                let usage = call_identity.estimated(
                    estimate_tokens_from_chars(preamble.len() + user_prompt.len()),
                    estimate_tokens_from_chars(output_chars),
                );
                let _ = tx.send(OrchestratorEvent::Usage { usage }).await;
            }

            // Partition the response
            let mut text_parts = Vec::new();
            let mut tool_calls = Vec::new();

            for content in response.choice.iter() {
                match content {
                    AssistantContent::Text(t) => text_parts.push(t.text.clone()),
                    AssistantContent::ToolCall(tc) => tool_calls.push(tc.clone()),
                }
            }

            if !tool_calls.is_empty() {
                if !text_parts.is_empty() {
                    let _ = tx
                        .send(OrchestratorEvent::Thinking {
                            content: text_parts.join(""),
                        })
                        .await;
                }

                // This completion's total token cost, attributed evenly across
                // however many tool calls it produced — see `run`'s identical
                // comment for why `after_call` previously always got a literal 0.
                let tokens_per_call = tokens_per_tool_call(completion_tokens, tool_calls.len());

                let mut results_for_context = Vec::new();

                for tc in &tool_calls {
                    let name = &tc.function.name;
                    let args_str = tc.function.arguments.to_string();

                    let msg = tc
                        .function
                        .arguments
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let agent_display = name
                        .strip_prefix("call_agent_")
                        .unwrap_or(name)
                        .replace('_', "-");

                    let call_handle = match admit_call(
                        tc,
                        &agent_display,
                        config,
                        guard,
                        tx,
                        turn_idx,
                        &mut results_for_context,
                    )
                    .await
                    {
                        CallAdmission::Blocked => continue,
                        CallAdmission::Admitted(handle) => handle,
                    };

                    let policy_score = config
                        .policy
                        .as_ref()
                        .and_then(|p| p.call_score(&tc.function.arguments));
                    tracing::info!(
                        target: "nasiko::orchestrator",
                        agent = %agent_display,
                        policy_score = ?policy_score,
                        turn = turn_idx + 1,
                        "delegating to agent: the call cleared every configured gate"
                    );
                    let _ = tx
                        .send(OrchestratorEvent::ToolCall {
                            agent: agent_display.clone(),
                            message: msg,
                            turn: turn_idx + 1,
                            policy_score,
                        })
                        .await;

                    let started = std::time::Instant::now();
                    let result = toolset.call(name, args_str).await;
                    let duration_ms = started.elapsed().as_millis() as u64;
                    finish_agent_call(call_handle, &result);

                    // A pause is not a tool outcome to relay as ToolResult or reason over — stop
                    // this run immediately, before the LLM ever sees it as a completed call.
                    //
                    // Do NOT also send OrchestratorEvent::AwaitingHuman here: this A2aTool was
                    // built `.with_progress(tx.clone())` (same `tx` this function itself uses),
                    // so `call_streaming`'s own forwarder task already relayed the identical
                    // event on this same channel, live, before `toolset.call()` even returned —
                    // sending it again here would deliver it twice to a2a_dispatch.rs.
                    if let ToolOutcome::AwaitingHuman { .. } = classify_tool_result(&result) {
                        if let Some(g) = guard {
                            g.after_call(&agent_display, tokens_per_call).await;
                        }
                        // Preserve any earlier calls in this same batch that already completed
                        // before this one paused — without this, they're silently discarded here,
                        // since the push_tool_result call below (which normally records the whole
                        // batch) is never reached once we return. `results_for_context.len()` is
                        // exactly the count of `tool_calls` processed so far: every earlier
                        // iteration either pushed a result/error/block entry or hit this same
                        // pause check itself, so the slice lines up with what's actually recorded.
                        if !results_for_context.is_empty() {
                            let completed_names = tool_calls[..results_for_context.len()]
                                .iter()
                                .map(|tc| tc.function.name.as_str())
                                .collect::<Vec<_>>();
                            context.push_tool_result(
                                &completed_names,
                                &results_for_context.join("\n\n"),
                            );
                        }
                        return Ok(());
                    }

                    match &result {
                        Ok(output) => {
                            if let Some(g) = guard {
                                g.after_call(&agent_display, tokens_per_call).await;
                            }
                            let _ = tx
                                .send(OrchestratorEvent::ToolResult {
                                    agent: agent_display,
                                    result: output.clone(),
                                    success: true,
                                    turn: turn_idx + 1,
                                    duration_ms,
                                })
                                .await;
                            results_for_context.push(format!("[{}] Result: {}", name, output));
                            delegated = true;
                        }
                        Err(e) => {
                            // Balance the before_call() depth increment even on
                            // failure — otherwise a failed tool call permanently
                            // leaks flow-depth and later legitimate calls in the
                            // same flow get falsely rejected with MaxDepthExceeded.
                            if let Some(g) = guard {
                                g.after_call(&agent_display, tokens_per_call).await;
                            }
                            let err_str = e.to_string();
                            let _ = tx
                                .send(OrchestratorEvent::ToolResult {
                                    agent: agent_display,
                                    result: err_str.clone(),
                                    success: false,
                                    turn: turn_idx + 1,
                                    duration_ms,
                                })
                                .await;
                            results_for_context.push(format!("[{}] Error: {}", name, err_str));
                        }
                    }
                }

                let combined = results_for_context.join("\n\n");
                context.push_tool_result(
                    &tool_calls
                        .iter()
                        .map(|tc| tc.function.name.as_str())
                        .collect::<Vec<_>>(),
                    &combined,
                );
            } else {
                // Final answer from non-streaming — emit as Content chunks
                let final_text =
                    finalize_answer(&text_parts.join("\n"), config, &agents, delegated, turn_idx);
                for chunk in final_text.chars().collect::<Vec<_>>().chunks(200) {
                    let s: String = chunk.iter().collect();
                    let _ = tx.send(OrchestratorEvent::Content { content: s }).await;
                }
                context.push_assistant(&final_text);

                let _ = tx
                    .send(OrchestratorEvent::Done {
                        turns: turn_idx + 1,
                        context_compacted,
                    })
                    .await;

                return Ok(());
            }
        } else {
            // Streaming path (turn 0 or when we want real-time output)
            let mut req = model
                .completion_request(Message::user(&user_prompt))
                .preamble(preamble.clone())
                .tools(tool_defs.clone());

            // Nothing the policy will accept from an undelegated turn is long,
            // and anything long is about to be discarded — so stop paying to
            // generate it. Sized against the query, because this is also the
            // turn that relays it into a tool call.
            if !delegated
                && let Some(cap) = config
                    .policy
                    .as_ref()
                    .and_then(|p| p.undelegated_max_tokens(user_query.chars().count()))
            {
                req = req.max_tokens(cap);
            }

            if let Some(temp) = config.temperature {
                req = req.temperature(temp);
            }

            let call_identity = model.call_identity(true);
            let mut stream = match model.stream_request(req.build()).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx
                        .send(OrchestratorEvent::Error {
                            message: e.to_string(),
                        })
                        .await;
                    return Err(OrchestratorError::Completion(e.to_string()));
                }
            };

            let mut text_parts = Vec::new();
            let mut tool_calls = Vec::new();

            let mut completion_tokens = None;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(CompletionEvent::Usage(usage)) => {
                        completion_tokens = Some(usage.total_tokens);
                        let _ = tx.send(OrchestratorEvent::Usage { usage }).await;
                    }
                    Ok(CompletionEvent::Text(text)) => {
                        text_parts.push(text.clone());
                        let _ = tx.send(OrchestratorEvent::Content { content: text }).await;
                    }
                    Ok(CompletionEvent::Tool(tool)) => tool_calls.push(tool),
                    Err(e) => {
                        let _ = tx
                            .send(OrchestratorEvent::Error {
                                message: e.to_string(),
                            })
                            .await;
                        return Err(OrchestratorError::Completion(e.to_string()));
                    }
                }
            }

            // Missing provider usage is not evidence of zero usage or cache misses.
            if completion_tokens.is_none() {
                let output_chars: usize = text_parts.iter().map(|t| t.len()).sum::<usize>()
                    + tool_calls
                        .iter()
                        .map(|tc| tc.function.arguments.to_string().len())
                        .sum::<usize>();
                let input_chars = preamble.len() + user_prompt.len();
                let _ = tx
                    .send(OrchestratorEvent::Usage {
                        usage: call_identity.estimated(
                            estimate_tokens_from_chars(input_chars),
                            estimate_tokens_from_chars(output_chars),
                        ),
                    })
                    .await;
            }

            if !tool_calls.is_empty() {
                // Note: unlike the non-streaming branch below, no `Thinking`
                // event is sent here — any pre-tool-call text was already
                // delivered live via `Content` as it streamed above, so
                // re-sending it as `Thinking` would just print it twice.

                let tokens_per_call = tokens_per_tool_call(completion_tokens, tool_calls.len());
                let mut results_for_context = Vec::new();

                for tc in &tool_calls {
                    let name = &tc.function.name;
                    let args_str = tc.function.arguments.to_string();

                    let msg = tc
                        .function
                        .arguments
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let agent_display = name
                        .strip_prefix("call_agent_")
                        .unwrap_or(name)
                        .replace('_', "-");

                    let call_handle = match admit_call(
                        tc,
                        &agent_display,
                        config,
                        guard,
                        tx,
                        turn_idx,
                        &mut results_for_context,
                    )
                    .await
                    {
                        CallAdmission::Blocked => continue,
                        CallAdmission::Admitted(handle) => handle,
                    };

                    let policy_score = config
                        .policy
                        .as_ref()
                        .and_then(|p| p.call_score(&tc.function.arguments));
                    tracing::info!(
                        target: "nasiko::orchestrator",
                        agent = %agent_display,
                        policy_score = ?policy_score,
                        turn = turn_idx + 1,
                        "delegating to agent: the call cleared every configured gate"
                    );
                    let _ = tx
                        .send(OrchestratorEvent::ToolCall {
                            agent: agent_display.clone(),
                            message: msg,
                            turn: turn_idx + 1,
                            policy_score,
                        })
                        .await;

                    let started = std::time::Instant::now();
                    let result = toolset.call(name, args_str).await;
                    let duration_ms = started.elapsed().as_millis() as u64;
                    finish_agent_call(call_handle, &result);

                    // A pause is not a tool outcome to relay as ToolResult or reason over — stop
                    // this run immediately, before the LLM ever sees it as a completed call.
                    //
                    // Do NOT also send OrchestratorEvent::AwaitingHuman here: this A2aTool was
                    // built `.with_progress(tx.clone())` (same `tx` this function itself uses),
                    // so `call_streaming`'s own forwarder task already relayed the identical
                    // event on this same channel, live, before `toolset.call()` even returned —
                    // sending it again here would deliver it twice to a2a_dispatch.rs.
                    if let ToolOutcome::AwaitingHuman { .. } = classify_tool_result(&result) {
                        if let Some(g) = guard {
                            g.after_call(&agent_display, tokens_per_call).await;
                        }
                        // Preserve any earlier calls in this same batch that already completed
                        // before this one paused — without this, they're silently discarded here,
                        // since the push_tool_result call below (which normally records the whole
                        // batch) is never reached once we return. `results_for_context.len()` is
                        // exactly the count of `tool_calls` processed so far: every earlier
                        // iteration either pushed a result/error/block entry or hit this same
                        // pause check itself, so the slice lines up with what's actually recorded.
                        if !results_for_context.is_empty() {
                            let completed_names = tool_calls[..results_for_context.len()]
                                .iter()
                                .map(|tc| tc.function.name.as_str())
                                .collect::<Vec<_>>();
                            context.push_tool_result(
                                &completed_names,
                                &results_for_context.join("\n\n"),
                            );
                        }
                        return Ok(());
                    }

                    match &result {
                        Ok(output) => {
                            if let Some(g) = guard {
                                g.after_call(&agent_display, tokens_per_call).await;
                            }
                            let _ = tx
                                .send(OrchestratorEvent::ToolResult {
                                    agent: agent_display,
                                    result: output.clone(),
                                    success: true,
                                    turn: turn_idx + 1,
                                    duration_ms,
                                })
                                .await;
                            results_for_context.push(format!("[{}] Result: {}", name, output));
                            delegated = true;
                        }
                        Err(e) => {
                            // Balance the before_call() depth increment even on
                            // failure — otherwise a failed tool call permanently
                            // leaks flow-depth and later legitimate calls in the
                            // same flow get falsely rejected with MaxDepthExceeded.
                            if let Some(g) = guard {
                                g.after_call(&agent_display, tokens_per_call).await;
                            }
                            let err_str = e.to_string();
                            let _ = tx
                                .send(OrchestratorEvent::ToolResult {
                                    agent: agent_display,
                                    result: err_str.clone(),
                                    success: false,
                                    turn: turn_idx + 1,
                                    duration_ms,
                                })
                                .await;
                            results_for_context.push(format!("[{}] Error: {}", name, err_str));
                        }
                    }
                }

                let combined = results_for_context.join("\n\n");
                context.push_tool_result(
                    &tool_calls
                        .iter()
                        .map(|tc| tc.function.name.as_str())
                        .collect::<Vec<_>>(),
                    &combined,
                );
            } else {
                // Final answer — already streamed token-by-token via Content events.
                //
                // No policy review here, and none is possible: the text has
                // already reached the client chunk-by-chunk and cannot be
                // recalled. A policy whose `review_final_answer` can change an
                // answer must therefore return `true` from `buffer_every_turn`,
                // which forces `use_non_streaming` above and makes this branch
                // unreachable under it.
                //
                // Note the condition is "can change an answer", not "enforces
                // something": a policy that rewrites only some turns still has
                // to buffer all of them, because this branch cannot ask. That
                // distinction is not academic — the enterprise policy returned
                // `require_delegation` here, so the HITL resume streamed turn 0
                // and silently skipped its own final-answer review.
                let final_text = text_parts.join("");
                context.push_assistant(&final_text);

                let _ = tx
                    .send(OrchestratorEvent::Done {
                        turns: turn_idx + 1,
                        context_compacted,
                    })
                    .await;

                return Ok(());
            }
        } // end streaming else branch

        if context.needs_compaction() {
            context.compact_simple();
            context_compacted = true;
        }
    }

    let _ = tx
        .send(OrchestratorEvent::Error {
            message: format!("max turns ({}) exceeded", config.max_turns),
        })
        .await;
    Err(OrchestratorError::MaxTurnsExceeded(config.max_turns))
}

/// Rough chars→tokens estimate (~4 chars/token for English-ish text) for
/// streamed turns where the provider reports no usage. Never returns 0 for
/// non-empty text so estimated usage is distinguishable from "no data".
fn estimate_tokens_from_chars(chars: usize) -> u64 {
    (chars as u64).div_ceil(4)
}

#[cfg(test)]
mod tokens_per_tool_call_tests {
    use super::tokens_per_tool_call;

    #[test]
    fn splits_total_evenly_across_tool_calls() {
        assert_eq!(tokens_per_tool_call(Some(300), 3), 100);
    }

    #[test]
    fn no_usage_reported_yields_zero() {
        assert_eq!(tokens_per_tool_call(None, 3), 0);
    }

    #[test]
    fn zero_tool_calls_does_not_divide_by_zero() {
        assert_eq!(tokens_per_tool_call(Some(300), 0), 300);
    }

    #[test]
    fn single_tool_call_gets_the_full_amount() {
        assert_eq!(tokens_per_tool_call(Some(150), 1), 150);
    }
}

#[cfg(test)]
mod awaiting_human_tests {
    use super::*;
    use crate::a2a::A2aClient;
    use crate::registry::{AgentInfo, RegistrySource};
    use crate::tool::A2aToolError;

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

    /// Pure classification, exercised twice with identical input — the property that makes the
    /// "both loops call the same function" claim actually checkable rather than asserted by
    /// inspection: if this function's output ever depended on anything but its argument, the two
    /// calls below could disagree.
    #[test]
    fn classify_tool_result_is_deterministic_and_recovers_awaiting_human_through_rig_erasure() {
        // Built the same way `ToolSet::call()` really builds it (see
        // tool::tests::awaiting_human_survives_rig_toolset_erasure for the full round trip
        // through a real ToolSet; this test targets classify_tool_result directly).
        let inner: Box<dyn std::error::Error + Send + Sync> =
            Box::new(A2aToolError::AwaitingHuman {
                agent: "test-agent".into(),
                agent_id: "agent-under-test".into(),
                pause: PauseInfo {
                    kind: nasiko_types::a2a::AwaitingHumanKind::InputRequired,
                    message: "Which repository?".into(),
                    task_id: "task-321".into(),
                    context_id: "ctx-654".into(),
                    metadata: serde_json::Value::Null,
                },
            });
        let result: Result<String, ToolSetError> =
            Err(ToolSetError::ToolCallError(ToolError::ToolCallError(inner)));

        for _ in 0..2 {
            match classify_tool_result(&result) {
                ToolOutcome::AwaitingHuman {
                    agent, agent_id, ..
                } => {
                    assert_eq!(agent, "test-agent");
                    assert_eq!(agent_id, "agent-under-test");
                }
                ToolOutcome::NotAwaitingHuman => panic!("expected AwaitingHuman"),
            }
        }
    }

    #[test]
    fn classify_tool_result_leaves_ordinary_outcomes_alone() {
        assert!(matches!(
            classify_tool_result(&Ok("fine".to_string())),
            ToolOutcome::NotAwaitingHuman
        ));
        assert!(matches!(
            classify_tool_result(&Err(ToolSetError::ToolNotFoundError("x".into()))),
            ToolOutcome::NotAwaitingHuman
        ));
    }

    /// Records the result a call guard's handle is told, and whether it was dropped.
    struct RecordingHandle(std::sync::Arc<std::sync::Mutex<(Option<bool>, bool)>>);

    impl AgentCallHandle for RecordingHandle {
        fn set_succeeded(&mut self, succeeded: bool) {
            self.0.lock().unwrap().0 = Some(succeeded);
        }
    }

    impl Drop for RecordingHandle {
        fn drop(&mut self) {
            self.0.lock().unwrap().1 = true;
        }
    }

    fn finished_with(result: Result<String, ToolSetError>) -> (Option<bool>, bool) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new((None, false)));
        finish_agent_call(Some(Box::new(RecordingHandle(seen.clone()))), &result);
        *seen.lock().unwrap()
    }

    /// A pause is the agent answering with a question, so it is a successful call; only an
    /// ordinary error fails it. Either way the handle is dropped, which ends the in-flight span.
    #[test]
    fn finishing_a_call_reports_its_result_and_releases_the_handle() {
        assert_eq!(finished_with(Ok("fine".into())), (Some(true), true));
        assert_eq!(
            finished_with(Err(ToolSetError::ToolNotFoundError("x".into()))),
            (Some(false), true)
        );
        let pause: Box<dyn std::error::Error + Send + Sync> =
            Box::new(A2aToolError::AwaitingHuman {
                agent: "test-agent".into(),
                agent_id: "agent-under-test".into(),
                pause: PauseInfo {
                    kind: nasiko_types::a2a::AwaitingHumanKind::InputRequired,
                    message: "Which repository?".into(),
                    task_id: "task-321".into(),
                    context_id: "ctx-654".into(),
                    metadata: serde_json::Value::Null,
                },
            });
        assert_eq!(
            finished_with(Err(ToolSetError::ToolCallError(ToolError::ToolCallError(
                pause
            )))),
            (Some(true), true)
        );
    }

    fn test_agent(endpoint: &str) -> AgentInfo {
        AgentInfo {
            id: "agent-under-test".into(),
            name: "test-agent".into(),
            description: "for tests".into(),
            endpoint: endpoint.into(),
            skills: vec![],
            auth_header: None,
        }
    }

    fn mock_tool_call_completion() -> String {
        serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 1,
            "model": "gpt-4o-mini",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "call_agent_test_agent",
                            "arguments": "{\"message\":\"hi\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
        .to_string()
    }

    /// The real, end-to-end proof this whole step exists for: a full `Orchestrator::run()`
    /// against a mocked LLM (one tool-call completion) and a mocked sub-agent (pauses) —
    /// verifies the loop stops after exactly one LLM call, never asks the LLM what to do next
    /// with the pause, and returns the pause faithfully.
    #[tokio::test]
    async fn run_stops_after_one_llm_call_when_the_tool_call_pauses() {
        let mut llm_server = mockito::Server::new_async().await;
        let llm_mock = llm_server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_tool_call_completion())
            .expect(1)
            .create_async()
            .await;

        let mut agent_server = mockito::Server::new_async().await;
        let agent_mock = agent_server
            .mock("POST", "/")
            .with_status(200)
            .with_body(input_required_response_body())
            .expect(1)
            .create_async()
            .await;

        let config = OrchestratorConfig {
            base_url: Some(llm_server.url()),
            api_key: Some("test-key".into()),
            ..Default::default()
        };
        let mut orchestrator = Orchestrator::new(
            config,
            RegistrySource::Static(vec![test_agent(&agent_server.url())]),
        )
        .with_a2a_client(A2aClient::new());
        // AgentRegistry::agents() only reads its cache — init() is what populates it, even for a
        // Static source. Every real caller (e.g. a2a_dispatch.rs) calls this before run()/
        // run_stream(); omitting it here isn't "a smaller test," it's testing a call sequence
        // that doesn't happen in production and getting Err(NoAgents) for the wrong reason.
        orchestrator
            .init()
            .await
            .expect("registry init must succeed for a Static source");

        let result = orchestrator.run("please help").await;

        llm_mock.assert_async().await;
        agent_mock.assert_async().await;
        match result {
            Err(OrchestratorError::AwaitingHuman {
                agent,
                agent_id,
                pause,
            }) => {
                assert_eq!(agent, "test-agent");
                assert_eq!(agent_id, "agent-under-test");
                assert_eq!(pause.message, "Which repository?");
            }
            other => panic!("expected Err(AwaitingHuman), got {other:?}"),
        }
    }

    /// The stable roster/protocol/rules block must stay the prefix and `{custom}` (e.g. L1A
    /// supplemental context, which can be query-variable for large domains) must come last, so
    /// OpenAI's prefix-based prompt caching hits on the stable part across turns even when
    /// `{custom}` itself changes. See `build_preamble`'s doc comment.
    #[test]
    fn build_preamble_places_custom_content_after_the_stable_roster_and_rules() {
        let config = OrchestratorConfig {
            preamble: Some("## Known facts about specific agents\n\ntest-agent:\n- a fact".into()),
            ..Default::default()
        };
        let agents = vec![test_agent("http://unused")];

        // The free function, not a method: `run()` and `run_stream_inner()` used to carry
        // byte-identical copies of this prompt, and this is the single builder both now call.
        let preamble = build_preamble(&config, &agents);

        let agents_pos = preamble
            .find("## Available Agents")
            .expect("roster header present");
        let rules_pos = preamble.find("## Rules").expect("rules header present");
        let custom_pos = preamble
            .find("Known facts about specific agents")
            .expect("custom preamble content present");

        assert!(
            agents_pos < rules_pos,
            "roster must precede rules in the stable prefix"
        );
        assert!(
            rules_pos < custom_pos,
            "custom content must come after the stable roster/protocol/rules block"
        );
    }
}

#[cfg(test)]
mod policy_seam_tests {
    use super::*;
    use crate::policy::ToolSchemaExtra;

    /// Records which hooks the loop actually reaches, and answers every one of
    /// them with something recognisable. Asserting on the *effects* here is what
    /// makes this a test of the seam rather than of a policy: the behaviour it
    /// checks belongs to whatever implementation is plugged in, but the wiring
    /// belongs to this crate, and a hook that quietly stops being called is
    /// invisible any other way.
    #[derive(Debug)]
    struct StubPolicy;

    impl DelegationPolicy for StubPolicy {
        fn preamble_policy(&self) -> String {
            "## Stub Policy".to_string()
        }
        fn preamble_footer(&self) -> String {
            "## Stub Footer".to_string()
        }
        fn turn_reminder(&self) -> Option<&str> {
            Some("\n\n(stub reminder)")
        }
        fn tool_schema_extra(&self) -> Option<ToolSchemaExtra> {
            None
        }
        fn check_tool_call(&self, arguments: &serde_json::Value) -> Result<(), String> {
            match arguments.get("ok") {
                Some(serde_json::Value::Bool(true)) => Ok(()),
                _ => Err("stub rejected this call".to_string()),
            }
        }
        fn call_score(&self, _arguments: &serde_json::Value) -> Option<f64> {
            Some(42.0)
        }
        fn review_final_answer(
            &self,
            _text: &str,
            _agents: &[AgentInfo],
            _delegated: bool,
            _turn_idx: usize,
        ) -> String {
            "stub replaced the answer".to_string()
        }
        fn undelegated_max_tokens(&self, user_query_chars: usize) -> Option<u64> {
            Some(123 + user_query_chars as u64)
        }
        fn buffer_every_turn(&self) -> bool {
            true
        }
        fn is_refusal(&self, text: &str) -> bool {
            text == "stub replaced the answer"
        }
    }

    fn with_policy() -> OrchestratorConfig {
        OrchestratorConfig {
            policy: Some(Arc::new(StubPolicy)),
            ..Default::default()
        }
    }

    /// The open-source default: no policy, and the prompt carries no trace of
    /// one. This is the guarantee that makes the seam free to exist here — an
    /// unconfigured deployment is byte-identical to one built before it.
    #[test]
    fn no_policy_leaves_the_preamble_untouched() {
        let preamble = build_preamble(&OrchestratorConfig::default(), &[]);

        // Byte-exact, NOT trimmed. An earlier version of this test trimmed, and
        // so could not see that the two empty placeholders were each leaving a
        // stray blank line behind — "the same apart from whitespace" is not the
        // same prompt. Both joins are pinned literally: an empty roster renders
        // as `Agents\n\n` + `` + `\n\n` + `Protocol`, exactly as it did before
        // this seam existed, and the built-in rules are the last thing in the
        // string with nothing after them.
        assert!(
            preamble.contains("## Available Agents\n\n\n\n## Protocol"),
            "an absent policy section must leave nothing behind, not a blank line:\n{preamble:?}"
        );
        assert!(
            preamble.ends_with("your own words would otherwise repeat the same question twice."),
            "an absent footer must leave nothing behind, not a trailing blank line:\n{preamble:?}"
        );
    }

    /// Both variable sections render after the stable block, and the footer is
    /// last of all.
    ///
    /// The order between them is a real trade-off, resolved here rather than
    /// left to whoever edits the format string next. `{custom}` is the more
    /// variable of the two, so prompt-cache reasoning alone would put the
    /// footer first — but the footer is the operator's org rules, whose whole
    /// value depends on being the last thing the model reads, and `{custom}`
    /// grows with the size of the fleet. Putting the rules first would re-bury
    /// them behind it, which is the failure they were moved to the footer to
    /// escape in the first place.
    #[test]
    fn the_footer_stays_last_even_behind_caller_supplied_context() {
        let config = OrchestratorConfig {
            preamble: Some("## Known facts about specific agents".into()),
            policy: Some(Arc::new(StubPolicy)),
            ..Default::default()
        };
        let preamble = build_preamble(&config, &[]);

        let rules_at = preamble.find("## Rules").expect("built-in rules");
        let custom_at = preamble
            .find("## Known facts about specific agents")
            .expect("caller preamble");
        let footer_at = preamble.find("## Stub Footer").expect("policy footer");

        assert!(
            rules_at < custom_at,
            "caller context belongs after the stable block:\n{preamble}"
        );
        assert!(
            custom_at < footer_at,
            "the operator's footer must outrank caller context for last place:\n{preamble}"
        );
        assert!(
            preamble.ends_with("## Stub Footer"),
            "nothing may render after the footer:\n{preamble:?}"
        );
    }

    #[test]
    fn no_policy_returns_the_model_answer_verbatim() {
        let out = finalize_answer(
            "Paris is the capital of France.",
            &OrchestratorConfig::default(),
            &[],
            false,
            0,
        );
        assert_eq!(out, "Paris is the capital of France.");
    }

    /// Position is the whole reason these are two hooks rather than one: the
    /// policy section explains the roster it follows, while the footer has to be
    /// the last thing the model reads to survive at all.
    #[test]
    fn a_policy_lands_in_both_prompt_positions_in_order() {
        let preamble = build_preamble(&with_policy(), &[]);

        let policy_at = preamble.find("## Stub Policy").expect("policy section");
        let protocol_at = preamble.find("## Protocol").expect("protocol section");
        let footer_at = preamble.find("## Stub Footer").expect("footer section");

        assert!(policy_at < protocol_at, "policy precedes the protocol");
        assert!(footer_at > protocol_at, "the footer trails every built-in");
        assert!(preamble.trim_end().ends_with("## Stub Footer"));
    }

    #[test]
    fn a_policy_gets_the_last_word_on_the_final_answer() {
        let out = finalize_answer(
            "Paris is the capital of France.",
            &with_policy(),
            &[],
            false,
            0,
        );
        assert_eq!(out, "stub replaced the answer");
    }

    /// The gate reads raw arguments, before deserialization, so a rejection is
    /// an explicit reason the model can act on rather than an opaque JSON error.
    #[test]
    fn the_gate_sees_raw_tool_arguments() {
        let policy = StubPolicy;
        assert!(
            policy
                .check_tool_call(&serde_json::json!({"ok": true}))
                .is_ok()
        );
        let err = policy
            .check_tool_call(&serde_json::json!({"message": "hi"}))
            .expect_err("the stub rejects anything without ok=true");
        assert_eq!(err, "stub rejected this call");
    }
}
