//! Hook-time parsing and durable capture. Network delivery belongs to `sync`.

use anyhow::{Result, bail};
use chrono::Utc;
use nasiko_types::{
    CODING_AGENT_CONTENT_MAX_BYTES, CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentEventV1,
    CodingAgentLlmCall, CodingAgentSession, CodingAgentSource, CodingAgentToolCall,
    CodingAgentTurn, coding_agent_event_id, coding_agent_session_id,
};
use std::collections::HashSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::agents::Agent;
use super::model::Turn;
use super::queue::{self, QueueDestination, QueueRecord};
use super::state::{self, IntegrationState, SessionLock};

const REPORT_BUDGET: Duration = Duration::from_secs(9);

pub fn run(agent: Agent) -> Result<()> {
    let deadline = Instant::now() + REPORT_BUDGET;
    let raw = read_payload()?;
    let snapshot = agent.snapshot(&raw, deadline)?;
    let spec = agent.spec();
    let settings = IntegrationState::load()?;
    let Some(agent_state) = settings.get(spec.id) else {
        bail!(
            "{} is not installed — run: nasiko agents install {}",
            spec.display_name,
            spec.id
        );
    };
    let destination = destination_from_state(agent_state)?;
    let lock = state::lock_session(
        spec.id,
        &snapshot.session_id,
        deadline.saturating_duration_since(Instant::now()),
    )?;
    let completed = complete_turns(&snapshot.turns);
    if completed.is_empty() {
        log(&format!(
            "session {} — no completed turns; deferred",
            snapshot.session_id
        ));
        return Ok(());
    }
    let progress = lock.progress()?;
    if progress.migrated_legacy_counts {
        log(&format!(
            "session {} — migrated legacy progress; replaying complete turns once",
            snapshot.session_id
        ));
    }
    let pending = pending_turns(&completed, &progress.captured_turn_ids);
    let mut queued = 0;
    let mut rejected = 0;
    for turn in &pending {
        let record = QueueRecord::new(
            destination.clone(),
            canonical_event(
                spec.id,
                &agent_state.agent_name,
                &snapshot.session_id,
                turn,
                agent_state.capture_content,
            ),
        );
        if let Err(error) = record.event.validate() {
            queue::reject_invalid(&record, &error)?;
            lock.mark_captured(std::slice::from_ref(&record.event.turn.id))?;
            rejected += 1;
            log(&format!(
                "session {} turn {} — quarantined invalid event: {error}",
                snapshot.session_id, record.event.turn.id
            ));
            continue;
        }
        queue_then_mark(&record, &lock)?;
        queued += 1;
    }
    drop(lock);

    if queued > 0 {
        spawn_sync()?;
    }
    if !pending.is_empty() {
        log(&format!(
            "session {} — queued {queued} completed turn(s) for {}; quarantined {rejected}",
            snapshot.session_id, destination.cluster_name
        ));
    }
    Ok(())
}

fn destination_from_state(agent_state: &super::state::AgentState) -> Result<QueueDestination> {
    let binding = agent_state.binding.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "installed integration has no cluster binding; reinstall it with: nasiko agents install <agent>"
        )
    })?;
    Ok(QueueDestination {
        cluster_name: binding.cluster_name.clone(),
        cluster_url: binding.cluster_url.clone(),
        principal_id: binding.principal_id,
    })
}

fn canonical_event(
    agent_id: &str,
    agent_name: &str,
    source_session_id: &str,
    turn: &Turn,
    capture_content: bool,
) -> CodingAgentEventV1 {
    CodingAgentEventV1 {
        version: CODING_AGENT_EVENT_VERSION,
        event_id: coding_agent_event_id(agent_id, source_session_id, &turn.uuid),
        captured_at: turn.ended_at,
        source: CodingAgentSource {
            agent_id: agent_id.to_string(),
            agent_name: agent_name.to_string(),
        },
        session: CodingAgentSession {
            id: coding_agent_session_id(agent_id, source_session_id),
            source_id: source_session_id.to_string(),
        },
        turn: CodingAgentTurn {
            id: turn.uuid.clone(),
            prompt: capture_content.then(|| turn.prompt.clone()),
            response: capture_content.then(|| turn.response.clone()).flatten(),
            started_at: turn.started_at,
            ended_at: turn.ended_at,
            llm_calls: turn
                .calls
                .iter()
                .map(|call| CodingAgentLlmCall {
                    id: call.uuid.clone(),
                    provider: call.provider.clone(),
                    model: call.model.clone(),
                    input_tokens: call.input_tokens,
                    output_tokens: call.output_tokens,
                    cache_read_tokens: call.cache_read_tokens,
                    cache_creation_tokens: call.cache_creation_tokens,
                    started_at: call.started_at,
                    ended_at: call.ended_at,
                })
                .collect(),
            tool_calls: turn
                .tool_calls
                .iter()
                .map(|tool| CodingAgentToolCall {
                    id: tool.id.clone(),
                    name: tool.name.clone(),
                    kind: tool.kind.clone(),
                    model_call_id: tool.model_call_id.clone(),
                    status: tool.status,
                    arguments: capture_content
                        .then(|| tool.arguments.as_ref().map(bounded_value))
                        .flatten(),
                    output: capture_content
                        .then(|| tool.output.as_ref().map(bounded_value))
                        .flatten(),
                    raw: capture_content
                        .then(|| tool.raw.as_deref().map(bounded_text))
                        .flatten(),
                    error: capture_content
                        .then(|| tool.error.as_deref().map(bounded_text))
                        .flatten(),
                    started_at: tool.started_at,
                    ended_at: tool.ended_at,
                    duration_ms: tool.duration_ms,
                    association: tool.association,
                    timestamp_quality: tool.timestamp_quality,
                })
                .collect(),
        },
        capture_policy: if capture_content {
            CapturePolicy::Content
        } else {
            CapturePolicy::MetadataOnly
        },
    }
}

fn bounded_value(value: &serde_json::Value) -> serde_json::Value {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
    if serialized.len() <= CODING_AGENT_CONTENT_MAX_BYTES {
        return value.clone();
    }
    serde_json::Value::String(bounded_text_to(
        &serialized,
        CODING_AGENT_CONTENT_MAX_BYTES / 2,
    ))
}

fn bounded_text(value: &str) -> String {
    bounded_text_to(value, CODING_AGENT_CONTENT_MAX_BYTES)
}

fn bounded_text_to(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let suffix = "...";
    let mut end = max.saturating_sub(suffix.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &value[..end])
}

fn queue_then_mark(record: &QueueRecord, lock: &SessionLock) -> Result<()> {
    ordered_commit(
        || queue::enqueue(record).map(|_| ()),
        || lock.mark_captured(std::slice::from_ref(&record.event.turn.id)),
    )
}

fn ordered_commit(
    enqueue: impl FnOnce() -> Result<()>,
    mark: impl FnOnce() -> Result<()>,
) -> Result<()> {
    enqueue()?;
    mark()
}

fn complete_turns(turns: &[Turn]) -> Vec<Turn> {
    turns
        .iter()
        .filter(|turn| !turn.is_empty() && turn.response.is_some())
        .cloned()
        .collect()
}

fn pending_turns(turns: &[Turn], captured: &HashSet<String>) -> Vec<Turn> {
    turns
        .iter()
        .filter(|turn| !captured.contains(&turn.uuid))
        .cloned()
        .collect()
}

fn spawn_sync() -> Result<()> {
    Command::new(std::env::current_exe()?)
        .args(["integration", "sync"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

fn read_payload() -> Result<String> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    Ok(raw)
}

fn log(message: &str) {
    println!("{} {message}", Utc::now().to_rfc3339());
}

#[cfg(test)]
mod tests {
    use super::super::model::{LlmCall, ToolCall};
    use super::*;
    use chrono::{DateTime, Utc};
    use std::cell::Cell;

    fn turn(id: &str, complete: bool) -> Turn {
        let at = "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        Turn {
            uuid: id.into(),
            prompt: "prompt".into(),
            response: complete.then(|| "response".into()),
            started_at: at,
            ended_at: at,
            calls: complete
                .then(|| LlmCall {
                    uuid: format!("call-{id}"),
                    provider: "provider".into(),
                    model: "model".into(),
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    started_at: at,
                    ended_at: at,
                })
                .into_iter()
                .collect(),
            tool_calls: vec![],
        }
    }

    #[test]
    fn incomplete_history_does_not_block_later_complete_turns() {
        let turns = [turn("a", true), turn("b", false), turn("c", true)];
        assert_eq!(
            complete_turns(&turns)
                .iter()
                .map(|turn| turn.uuid.as_str())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );
    }

    #[test]
    fn captured_progress_filters_completed_turns() {
        let turns = [turn("a", true), turn("b", true)];
        assert_eq!(
            pending_turns(&turns, &HashSet::from(["a".to_string()]))[0].uuid,
            "b"
        );
    }

    #[test]
    fn queue_failure_never_advances_the_watermark() {
        let marked = Cell::new(false);
        let result = ordered_commit(
            || Err(anyhow::anyhow!("disk full")),
            || {
                marked.set(true);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!marked.get());
    }

    #[test]
    fn reporting_uses_the_install_time_destination() {
        let principal_id = uuid::Uuid::new_v4();
        let state = super::super::state::AgentState {
            agent_name: "alice-claude-code".into(),
            capture_content: true,
            hook_version: 1,
            binding: Some(super::super::state::InstallationBinding {
                cluster_name: "cluster-a".into(),
                cluster_url: "https://a.example".into(),
                principal_id,
            }),
        };

        let destination = destination_from_state(&state).unwrap();
        assert_eq!(destination.cluster_name, "cluster-a");
        assert_eq!(destination.cluster_url, "https://a.example");
        assert_eq!(destination.principal_id, principal_id);
    }

    #[test]
    fn legacy_install_state_without_a_destination_fails_closed() {
        let state = super::super::state::AgentState {
            agent_name: "alice-claude-code".into(),
            capture_content: true,
            hook_version: 1,
            binding: None,
        };

        assert!(
            destination_from_state(&state)
                .unwrap_err()
                .to_string()
                .contains("reinstall")
        );
    }

    #[test]
    fn canonical_identity_is_stable_and_content_policy_is_enforced() {
        let mut turn = turn("same-turn", true);
        turn.tool_calls.push(ToolCall {
            id: "tool-1".into(),
            name: "Read".into(),
            kind: "tool".into(),
            model_call_id: Some("call-same-turn".into()),
            status: nasiko_types::CodingAgentToolCallStatus::Succeeded,
            arguments: Some(serde_json::json!({"path": "secret"})),
            output: Some(serde_json::json!("secret output")),
            raw: Some("raw".into()),
            error: Some("hidden".into()),
            started_at: Some(turn.started_at),
            ended_at: Some(turn.ended_at),
            duration_ms: Some(0),
            association: nasiko_types::CodingAgentToolAssociation::Exact,
            timestamp_quality: nasiko_types::CodingAgentTimestampQuality::Exact,
        });
        let first = canonical_event("claude", "claude-code", "same", &turn, false);
        let second = canonical_event("claude", "claude-code", "same", &turn, false);
        assert_eq!(first.event_id, second.event_id);
        assert_eq!(first, second);
        assert_eq!(first.session.id, "claude:same");
        assert!(first.turn.prompt.is_none());
        assert!(first.turn.response.is_none());
        assert_eq!(first.turn.tool_calls[0].name, "Read");
        assert!(first.turn.tool_calls[0].arguments.is_none());
        assert!(first.turn.tool_calls[0].output.is_none());
        assert!(first.turn.tool_calls[0].error.is_none());
        assert!(first.validate().is_ok());
        assert_ne!(
            first.event_id,
            canonical_event("opencode", "opencode", "same", &turn, false).event_id
        );
        let content = canonical_event("claude", "claude-code", "same", &turn, true);
        assert_eq!(
            content.turn.tool_calls[0].arguments,
            Some(serde_json::json!({"path": "secret"}))
        );
        assert!(content.validate().is_ok());
    }

    #[test]
    fn tool_content_projection_bounds_large_json_and_unicode_text() {
        let value = serde_json::json!({"value": "x".repeat(CODING_AGENT_CONTENT_MAX_BYTES)});
        assert!(
            serde_json::to_vec(&bounded_value(&value)).unwrap().len()
                <= CODING_AGENT_CONTENT_MAX_BYTES
        );
        let text = "é".repeat(CODING_AGENT_CONTENT_MAX_BYTES);
        let bounded = bounded_text(&text);
        assert!(bounded.len() <= CODING_AGENT_CONTENT_MAX_BYTES);
        assert!(bounded.ends_with("..."));
    }
}
