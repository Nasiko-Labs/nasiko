//! Codex's past sessions, read from its rollout files (`$CODEX_HOME/sessions/**/rollout-*.jsonl`).
//!
//! The live hook builds a turn from `UserPromptSubmit` + `Stop` events and stamps them with the time it
//! receives them, so replaying it would date every imported turn to the import. A rollout records the
//! whole turn with its own times instead: `task_started` / `task_complete` bracket it by `turn_id` (the
//! id the hook uses too, so imported and live turns dedupe), and the cumulative `token_count` events
//! between them give its usage.
//!
//! A subagent's thread is a rollout of its own; its `session_meta` names the thread that spawned it, as
//! does Codex's state database (`state_<n>.sqlite`, table `thread_spawn_edges`), read for a rollout
//! that doesn't say.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nasiko_types::{
    CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCallStatus,
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::super::model::{LlmCall, SessionSnapshot, ToolCall, Turn, subagent_parent};
use super::codex::{
    self, TokenUsage, nonempty, provider_for, token_usage_entry, tool_response_status, usage_delta,
};
use super::opencode_history::open_read_only;
use super::{PastSession, SessionSource};

/// Every rollout Codex kept, oldest first, each read only when the import reaches it.
pub fn past_sessions() -> Vec<PastSession> {
    past_sessions_in(&codex::config_path())
}

fn past_sessions_in(codex_home: &Path) -> Vec<PastSession> {
    let known = std::rc::Rc::new(KnownThreads {
        titles: session_titles(&codex_home.join("session_index.jsonl")),
        spawned_by: spawn_edges(codex_home),
    });
    rollout_files(&codex_home.join("sessions"))
        .into_iter()
        .map(|(modified, path)| {
            let known = known.clone();
            PastSession {
                key: path.to_string_lossy().into_owned(),
                last_active: modified.into(),
                source: SessionSource::Load(Box::new(move || {
                    let content = std::fs::read_to_string(&path)
                        .with_context(|| format!("failed to read {}", path.display()))?;
                    parse_rollout(&content, &fallback_session_id(&path), &known)
                })),
            }
        })
        .collect()
}

/// What Codex keeps about threads outside their rollouts.
#[derive(Default)]
struct KnownThreads {
    /// `thread_name` per thread id, from `session_index.jsonl`.
    titles: HashMap<String, String>,
    /// The spawning thread per subagent thread, from the state database.
    spawned_by: HashMap<String, String>,
}

/// `child → parent` from the newest state database's `thread_spawn_edges`. None when Codex keeps no
/// such database or table; one that can't be read only costs the links a rollout doesn't carry itself.
fn spawn_edges(codex_home: &Path) -> HashMap<String, String> {
    let Some(path) = newest_state_db(codex_home) else {
        return HashMap::new();
    };
    read_spawn_edges(&path).unwrap_or_else(|error| {
        eprintln!("Codex subagent links in {}: {error:#}", path.display());
        HashMap::new()
    })
}

fn newest_state_db(codex_home: &Path) -> Option<PathBuf> {
    std::fs::read_dir(codex_home)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let version: u32 = name
                .strip_prefix("state_")?
                .strip_suffix(".sqlite")?
                .parse()
                .ok()?;
            Some((version, entry.path()))
        })
        .max()
        .map(|(_, path)| path)
}

fn read_spawn_edges(path: &Path) -> Result<HashMap<String, String>> {
    let db = open_read_only(path)?;
    let has_table: bool = db.query_row(
        "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'thread_spawn_edges'",
        [],
        |row| row.get(0),
    )?;
    if !has_table {
        return Ok(HashMap::new());
    }
    let mut statement =
        db.prepare("SELECT child_thread_id, parent_thread_id FROM thread_spawn_edges")?;
    let edges = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()
        .context("failed to read Codex subagent links")?;
    Ok(edges)
}

/// `rollout-*.jsonl` anywhere under `sessions/` (Codex nests them by date) with when each was last
/// written, oldest first.
fn rollout_files(root: &Path) -> Vec<(std::time::SystemTime, PathBuf)> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let modified = path.metadata().ok().and_then(|m| m.modified().ok());
            if let (true, Some(modified)) = (is_rollout(&path), modified) {
                found.push((modified, path));
            }
        }
    }
    found.sort();
    found
}

fn is_rollout(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("rollout-"))
}

/// The session id a rollout's name ends with (`rollout-<time>-<uuid>.jsonl`), for a file whose
/// `session_meta` line is missing.
fn fallback_session_id(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let start = stem.len().saturating_sub(UUID_LEN);
    stem.get(start..).unwrap_or(stem).to_string()
}

const UUID_LEN: usize = 36;

/// `thread_name` per session id from `session_index.jsonl`; the last line for an id wins.
fn session_titles(path: &Path) -> HashMap<String, String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|entry| {
            let id = entry["id"].as_str()?.to_string();
            let name = nonempty(entry["thread_name"].as_str())?;
            Some((id, name))
        })
        .collect()
}

/// One turn as the rollout lines about it accumulate.
#[derive(Default)]
struct TurnRecord {
    id: String,
    prompt: Option<String>,
    response: Option<String>,
    model: Option<String>,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    completed: bool,
    usage_at_start: Option<TokenUsage>,
    usage_at_end: Option<TokenUsage>,
    /// Per-response usage, for rollouts that record it instead of cumulative `token_count` events.
    recorded_usage: Option<TokenUsage>,
    tool_calls: Vec<ToolCall>,
}

/// The completed turns of one rollout. A turn without `task_complete` (still running, or cut off) is
/// left out.
fn parse_rollout(
    content: &str,
    fallback_id: &str,
    known: &KnownThreads,
) -> Result<SessionSnapshot> {
    let mut session_id = None;
    let mut session_meta = None;
    let mut turns: Vec<TurnRecord> = Vec::new();
    let mut current: Option<usize> = None;
    let mut usage_so_far = TokenUsage::default();
    for entry in content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        let payload = &entry["payload"];
        let at = line_time(&entry);
        match (entry["type"].as_str(), payload["type"].as_str()) {
            (Some("session_meta"), _) => {
                session_id = session_id.or_else(|| nonempty(payload["id"].as_str()));
                session_meta.get_or_insert_with(|| payload.clone());
            }
            (Some("event_msg"), Some("task_started")) => {
                let index = turn_index(&mut turns, payload["turn_id"].as_str());
                if let Some(turn) = index.map(|i| &mut turns[i]) {
                    turn.started_at = epoch_secs(&payload["started_at"]).or(at);
                    turn.usage_at_start.get_or_insert(usage_so_far);
                }
                current = index.or(current);
            }
            (Some("turn_context"), _) => {
                let index = turn_index(&mut turns, payload["turn_id"].as_str());
                if let Some(turn) = index.map(|i| &mut turns[i]) {
                    turn.model = nonempty(payload["model"].as_str()).or(turn.model.take());
                }
                current = index.or(current);
            }
            (Some("event_msg"), Some("item_completed"))
                if payload["item"]["type"] == "UserMessage" =>
            {
                let index = turn_index(&mut turns, payload["turn_id"].as_str());
                if let (Some(turn), Some(text)) =
                    (index.map(|i| &mut turns[i]), message_text(&payload["item"]))
                {
                    turn.prompt = Some(match turn.prompt.take() {
                        Some(earlier) => format!("{earlier}\n\n{text}"),
                        None => text,
                    });
                }
            }
            (Some("event_msg"), Some("token_count")) => {
                if let Some(usage) = token_usage_entry(&entry) {
                    usage_so_far = usage;
                    if let Some(turn) = current.map(|i| &mut turns[i]) {
                        turn.usage_at_end = Some(usage);
                    }
                }
            }
            (Some("token_usage_record"), _) => {
                let index = turn_index(&mut turns, payload["turn_id"].as_str());
                if let (Some(turn), Some(usage)) = (
                    index.map(|i| &mut turns[i]),
                    usage_record(&payload["usage"]),
                ) {
                    let total = turn.recorded_usage.unwrap_or_default();
                    turn.recorded_usage = Some(TokenUsage {
                        input: total.input + usage.input,
                        output: total.output + usage.output,
                        cache_read: total.cache_read + usage.cache_read,
                        cache_write: total.cache_write + usage.cache_write,
                    });
                }
            }
            (Some("response_item"), Some("function_call" | "custom_tool_call")) => {
                if let Some(turn) = current.map(|i| &mut turns[i]) {
                    turn.tool_calls.extend(tool_started(payload, at));
                }
            }
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
                tool_finished(&mut turns, payload, at);
            }
            (Some("event_msg"), Some("task_complete")) => {
                let index = turn_index(&mut turns, payload["turn_id"].as_str());
                if let Some(turn) = index.map(|i| &mut turns[i]) {
                    turn.response = nonempty(payload["last_agent_message"].as_str());
                    turn.started_at = turn
                        .started_at
                        .or_else(|| epoch_secs(&payload["started_at"]));
                    turn.ended_at = epoch_secs(&payload["completed_at"]).or(at);
                    turn.usage_at_end = Some(usage_so_far);
                    turn.completed = true;
                }
            }
            _ => {}
        }
    }
    let session_id = session_id.unwrap_or_else(|| fallback_id.to_string());
    let model_provider = session_meta
        .as_ref()
        .and_then(|meta| nonempty(meta["model_provider"].as_str()));
    let parent = session_meta
        .and_then(|meta| codex::spawned_by(&session_id, &meta))
        .or_else(|| {
            let parent_id = known.spawned_by.get(&session_id)?;
            subagent_parent(&session_id, parent_id, None, None)
        });
    Ok(SessionSnapshot {
        parent,
        title: known.titles.get(&session_id).cloned(),
        turns: turns
            .into_iter()
            .filter_map(|turn| finished_turn(&session_id, model_provider.as_deref(), turn))
            .collect(),
        session_id,
    })
}

/// The index of the turn `turn_id` names, added on first sight.
fn turn_index(turns: &mut Vec<TurnRecord>, turn_id: Option<&str>) -> Option<usize> {
    let id = nonempty(turn_id)?;
    if let Some(index) = turns.iter().position(|turn| turn.id == id) {
        return Some(index);
    }
    turns.push(TurnRecord {
        id,
        ..TurnRecord::default()
    });
    Some(turns.len() - 1)
}

/// `model_provider` is the provider id the rollout's `session_meta` names, if any.
fn finished_turn(session_id: &str, model_provider: Option<&str>, turn: TurnRecord) -> Option<Turn> {
    if !turn.completed {
        return None;
    }
    let started_at = turn.started_at.or(turn.ended_at)?;
    let ended_at = turn.ended_at.unwrap_or(started_at).max(started_at);
    let usage = match (turn.usage_at_start, turn.usage_at_end, turn.recorded_usage) {
        (Some(start), Some(end), _) if end != start => usage_delta(Some(start), Some(end)),
        (_, _, Some(recorded)) => recorded,
        (start, end, None) => usage_delta(start, end),
    };
    let model = turn.model.unwrap_or_else(|| UNKNOWN_MODEL.to_string());
    Some(Turn {
        prompt: turn.prompt?,
        response: turn.response,
        started_at,
        ended_at,
        calls: vec![LlmCall {
            uuid: format!("{session_id}:{}:llm", turn.id),
            provider: provider_for(&model, model_provider).to_string(),
            model,
            input_tokens: usage.fresh_input(),
            output_tokens: usage.output,
            cache_read_tokens: usage.cache_read,
            cache_creation_tokens: usage.cache_write,
            accounting: None,
            started_at,
            ended_at,
        }],
        tool_calls: turn.tool_calls,
        uuid: turn.id,
    })
}

const UNKNOWN_MODEL: &str = "unknown";

fn tool_started(payload: &Value, at: Option<DateTime<Utc>>) -> Option<ToolCall> {
    let id = nonempty(payload["call_id"].as_str())?;
    let arguments = payload
        .get("arguments")
        .or_else(|| payload.get("input"))
        .map(json_or_text);
    Some(ToolCall {
        id,
        name: nonempty(payload["name"].as_str()).unwrap_or_else(|| "unknown".into()),
        kind: "tool".into(),
        model_call_id: None,
        status: CodingAgentToolCallStatus::Unknown,
        arguments,
        output: None,
        raw: None,
        error: None,
        started_at: at,
        ended_at: None,
        duration_ms: None,
        association: CodingAgentToolAssociation::Turn,
        timestamp_quality: CodingAgentTimestampQuality::Receipt,
    })
}

/// Pairs an output with the call it answers, by `call_id`, in whichever turn that call was made.
fn tool_finished(turns: &mut [TurnRecord], payload: &Value, at: Option<DateTime<Utc>>) {
    let Some(call_id) = payload["call_id"].as_str() else {
        return;
    };
    let Some(tool) = turns
        .iter_mut()
        .flat_map(|turn| turn.tool_calls.iter_mut())
        .find(|tool| tool.id == call_id)
    else {
        return;
    };
    let output = payload.get("output").map(json_or_text);
    let (status, error) = tool_response_status(output.as_ref(), None);
    tool.status = status;
    tool.error = error;
    tool.output = output;
    tool.ended_at = at;
    tool.duration_ms = tool
        .started_at
        .zip(at)
        .map(|(start, end)| (end - start).num_milliseconds().max(0) as u64);
}

/// A JSON string field that holds JSON (Codex's tool arguments) parsed; anything else as it is.
fn json_or_text(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| value.clone())
}

fn message_text(item: &Value) -> Option<String> {
    let text: Vec<&str> = item["content"]
        .as_array()?
        .iter()
        .filter(|part| {
            part["type"]
                .as_str()
                .is_some_and(|t| t.eq_ignore_ascii_case("text"))
        })
        .filter_map(|part| part["text"].as_str())
        .collect();
    nonempty(Some(&text.join("\n")))
}

fn usage_record(usage: &Value) -> Option<TokenUsage> {
    Some(TokenUsage {
        input: usage["input_tokens"].as_u64()?,
        output: usage["output_tokens"].as_u64()?,
        cache_read: usage["cached_input_tokens"].as_u64().unwrap_or(0),
        cache_write: usage["cache_write_input_tokens"].as_u64().unwrap_or(0),
    })
}

fn epoch_secs(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(value.as_i64()?, 0)
}

fn line_time(entry: &Value) -> Option<DateTime<Utc>> {
    entry["timestamp"].as_str()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(at: &str, kind: &str, payload: Value) -> String {
        json!({"timestamp": at, "type": kind, "payload": payload}).to_string()
    }

    fn event(at: &str, payload: Value) -> String {
        line(at, "event_msg", payload)
    }

    fn token_count(at: &str, input: u64, output: u64, cached: u64) -> String {
        event(
            at,
            json!({"type": "token_count", "info": {"total_token_usage": {
                "input_tokens": input, "output_tokens": output, "cached_input_tokens": cached
            }}}),
        )
    }

    fn user_message(at: &str, turn: &str, text: &str) -> String {
        event(
            at,
            json!({"type": "item_completed", "turn_id": turn, "item": {
                "type": "UserMessage", "content": [{"type": "text", "text": text}]
            }}),
        )
    }

    /// Two complete turns (the second with a paired tool call) and a third that never completed.
    fn fixture() -> String {
        [
            line(
                "2026-10-01T10:00:00Z",
                "session_meta",
                json!({"id": "sess-1"}),
            ),
            event(
                "2026-10-01T10:00:00Z",
                json!({"type": "task_started", "turn_id": "t1", "started_at": 1_790_000_000}),
            ),
            line(
                "2026-10-01T10:00:00Z",
                "turn_context",
                json!({"turn_id": "t1", "model": "gpt-5-codex"}),
            ),
            user_message("2026-10-01T10:00:01Z", "t1", "Build it"),
            token_count("2026-10-01T10:00:05Z", 100, 10, 40),
            event(
                "2026-10-01T10:00:06Z",
                json!({"type": "task_complete", "turn_id": "t1", "last_agent_message": "Built",
                       "started_at": 1_790_000_000, "completed_at": 1_790_000_006}),
            ),
            event(
                "2026-10-01T10:01:00Z",
                json!({"type": "task_started", "turn_id": "t2", "started_at": 1_790_000_060}),
            ),
            line(
                "2026-10-01T10:01:00Z",
                "turn_context",
                json!({"turn_id": "t2", "model": "gpt-5-codex"}),
            ),
            user_message("2026-10-01T10:01:01Z", "t2", "Run the tests"),
            line(
                "2026-10-01T10:01:02Z",
                "response_item",
                json!({"type": "function_call", "name": "shell", "call_id": "call-1",
                       "arguments": "{\"command\":[\"cargo\",\"test\"]}"}),
            ),
            line(
                "2026-10-01T10:01:04Z",
                "response_item",
                json!({"type": "function_call_output", "call_id": "call-1",
                       "output": "{\"exit_code\":0,\"output\":\"ok\"}"}),
            ),
            token_count("2026-10-01T10:01:05Z", 250, 30, 100),
            event(
                "2026-10-01T10:01:06Z",
                json!({"type": "task_complete", "turn_id": "t2", "last_agent_message": "All green",
                       "started_at": 1_790_000_060, "completed_at": 1_790_000_066}),
            ),
            event(
                "2026-10-01T10:02:00Z",
                json!({"type": "task_started", "turn_id": "t3", "started_at": 1_790_000_120}),
            ),
            user_message("2026-10-01T10:02:01Z", "t3", "And deploy"),
            "not json".into(),
        ]
        .join("\n")
    }

    #[test]
    fn a_rollout_becomes_its_completed_turns_with_their_own_times_and_usage() {
        let known = KnownThreads {
            titles: HashMap::from([("sess-1".to_string(), "Fix the build".to_string())]),
            ..KnownThreads::default()
        };

        let snapshot = parse_rollout(&fixture(), "fallback", &known).unwrap();

        assert_eq!(snapshot.session_id, "sess-1");
        assert!(snapshot.parent.is_none());
        assert_eq!(snapshot.title.as_deref(), Some("Fix the build"));
        // The third turn has no task_complete, so it is left out.
        let ids: Vec<&str> = snapshot.turns.iter().map(|t| t.uuid.as_str()).collect();
        assert_eq!(ids, ["t1", "t2"]);

        let first = &snapshot.turns[0];
        assert_eq!(first.prompt, "Build it");
        assert_eq!(first.response.as_deref(), Some("Built"));
        assert_eq!(first.started_at.timestamp(), 1_790_000_000);
        assert_eq!(first.ended_at.timestamp(), 1_790_000_006);
        assert_eq!(first.calls[0].uuid, "sess-1:t1:llm");
        assert_eq!(first.calls[0].model, "gpt-5-codex");
        assert_eq!(first.calls[0].provider, "openai");
        // The first turn's usage is the whole cumulative count so far, its input
        // without the cached tokens Codex counts inside it.
        assert_eq!(
            (
                first.calls[0].input_tokens,
                first.calls[0].output_tokens,
                first.calls[0].cache_read_tokens
            ),
            (60, 10, 40)
        );

        // The second turn's usage is the delta of the cumulative counts.
        let second = &snapshot.turns[1];
        assert_eq!(
            (
                second.calls[0].input_tokens,
                second.calls[0].output_tokens,
                second.calls[0].cache_read_tokens
            ),
            (90, 20, 60)
        );
        assert_eq!(second.tool_calls.len(), 1);
        let tool = &second.tool_calls[0];
        assert_eq!(tool.id, "call-1");
        assert_eq!(tool.name, "shell");
        assert_eq!(tool.arguments, Some(json!({"command": ["cargo", "test"]})));
        assert_eq!(tool.status, CodingAgentToolCallStatus::Succeeded);
        assert_eq!(tool.duration_ms, Some(2000));
        assert_eq!(tool.association, CodingAgentToolAssociation::Turn);
    }

    #[test]
    fn turns_are_labelled_with_the_provider_the_rollout_names() {
        let bedrock = fixture().replacen(
            r#"{"id":"sess-1"}"#,
            r#"{"id":"sess-1","model_provider":"amazon-bedrock"}"#,
            1,
        );
        assert_ne!(bedrock, fixture(), "the fixture's meta line was rewritten");

        let snapshot = parse_rollout(&bedrock, "fallback", &KnownThreads::default()).unwrap();

        assert!(
            snapshot
                .turns
                .iter()
                .all(|turn| turn.calls[0].provider == "aws-bedrock")
        );
    }

    #[test]
    fn per_response_usage_records_stand_in_for_missing_token_counts() {
        let content = [
            event(
                "2026-10-01T10:00:00Z",
                json!({"type": "task_started", "turn_id": "t1", "started_at": 1_790_000_000}),
            ),
            user_message("2026-10-01T10:00:01Z", "t1", "Hi"),
            line(
                "2026-10-01T10:00:02Z",
                "token_usage_record",
                json!({"turn_id": "t1", "usage": {"input_tokens": 7, "output_tokens": 3, "cached_input_tokens": 2}}),
            ),
            event(
                "2026-10-01T10:00:03Z",
                json!({"type": "task_complete", "turn_id": "t1", "last_agent_message": "Hello",
                       "completed_at": 1_790_000_003}),
            ),
        ]
        .join("\n");

        let snapshot = parse_rollout(&content, "from-name", &KnownThreads::default()).unwrap();

        assert_eq!(snapshot.session_id, "from-name");
        let call = &snapshot.turns[0].calls[0];
        assert_eq!(
            (
                call.input_tokens,
                call.output_tokens,
                call.cache_read_tokens
            ),
            (5, 3, 2)
        );
        assert_eq!(call.model, "unknown");
    }

    /// A subagent thread's rollout: its own meta line, then one completed turn.
    fn subagent_rollout(meta: String) -> String {
        [
            meta,
            event(
                "2026-10-01T10:00:00Z",
                json!({"type": "task_started", "turn_id": "c1", "started_at": 1_790_000_000}),
            ),
            user_message("2026-10-01T10:00:01Z", "c1", "Look around"),
            event(
                "2026-10-01T10:00:03Z",
                json!({"type": "task_complete", "turn_id": "c1", "last_agent_message": "Found it",
                       "completed_at": 1_790_000_003}),
            ),
        ]
        .join("\n")
    }

    #[test]
    fn a_subagent_threads_rollout_is_its_own_session_linked_to_the_thread_that_spawned_it() {
        let content = subagent_rollout(codex::tests::subagent_rollout_meta("child", "parent"));

        let snapshot = parse_rollout(&content, "fallback", &KnownThreads::default()).unwrap();

        assert_eq!(snapshot.session_id, "child");
        let parent = snapshot.parent.as_ref().unwrap();
        assert_eq!(parent.source_id, "parent");
        assert_eq!(parent.agent_type.as_deref(), Some("explorer"));
        assert_eq!(snapshot.turns.len(), 1);
        let event =
            super::super::super::report::canonical_event_for_test(&snapshot, &snapshot.turns[0]);
        assert_eq!(event.validate(), Ok(()));
    }

    #[test]
    fn the_state_database_links_a_subagent_rollout_that_does_not_say_who_spawned_it() {
        let home = tempfile::tempdir().unwrap();
        let day = home.path().join("sessions/2026/10/01");
        std::fs::create_dir_all(&day).unwrap();
        let meta = line(
            "2026-10-01T10:00:00Z",
            "session_meta",
            json!({"id": "child", "source": "exec"}),
        );
        std::fs::write(day.join("rollout-x-child.jsonl"), subagent_rollout(meta)).unwrap();
        for (version, parent) in [(4, "stale"), (5, "parent")] {
            let db =
                rusqlite::Connection::open(home.path().join(format!("state_{version}.sqlite")))
                    .unwrap();
            db.execute_batch(&format!(
                "CREATE TABLE thread_spawn_edges (parent_thread_id TEXT NOT NULL,
                     child_thread_id TEXT NOT NULL PRIMARY KEY, status TEXT NOT NULL);
                 INSERT INTO thread_spawn_edges VALUES ('{parent}', 'child', 'closed');"
            ))
            .unwrap();
        }

        let sessions = past_sessions_in(home.path());

        let SessionSource::Load(load) = sessions.into_iter().next().unwrap().source else {
            panic!("Codex sessions load from their rollout");
        };
        let parent = load().unwrap().parent.unwrap();
        assert_eq!(parent.source_id, "parent");
        assert_eq!(parent.agent_type, None);
    }

    #[test]
    fn a_state_database_without_spawn_edges_links_nothing() {
        let home = tempfile::tempdir().unwrap();
        rusqlite::Connection::open(home.path().join("state_5.sqlite"))
            .unwrap()
            .execute_batch("CREATE TABLE threads (id TEXT)")
            .unwrap();
        assert!(spawn_edges(home.path()).is_empty());
        assert!(spawn_edges(&home.path().join("missing")).is_empty());
    }

    #[test]
    fn rollouts_are_found_in_dated_folders_and_titled_from_the_index() {
        let home = tempfile::tempdir().unwrap();
        let day = home.path().join("sessions/2026/10/01");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(
            day.join("rollout-2026-10-01T10-00-00-0199aaaa-0000-7000-8000-000000000001.jsonl"),
            fixture(),
        )
        .unwrap();
        std::fs::write(day.join("notes.jsonl"), "{}").unwrap();
        std::fs::write(
            home.path().join("session_index.jsonl"),
            [
                json!({"id": "sess-1", "thread_name": "Old name"}).to_string(),
                json!({"id": "sess-1", "thread_name": "Fix the build"}).to_string(),
            ]
            .join("\n"),
        )
        .unwrap();

        let sessions = past_sessions_in(home.path());

        assert_eq!(sessions.len(), 1);
        let SessionSource::Load(load) = sessions.into_iter().next().unwrap().source else {
            panic!("Codex sessions load from their rollout");
        };
        let snapshot = load().unwrap();
        assert_eq!(snapshot.title.as_deref(), Some("Fix the build"));
        assert_eq!(snapshot.turns.len(), 2);
        assert!(past_sessions_in(&home.path().join("missing")).is_empty());
    }

    #[test]
    fn a_rollout_without_session_meta_takes_its_id_from_the_file_name() {
        let path =
            Path::new("rollout-2026-10-01T10-00-00-0199aaaa-0000-7000-8000-000000000001.jsonl");
        assert_eq!(
            fallback_session_id(path),
            "0199aaaa-0000-7000-8000-000000000001"
        );
    }
}
