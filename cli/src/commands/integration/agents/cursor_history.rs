//! Cursor's past sessions. Two stores, both read-only:
//!
//! - the IDE's `User/globalStorage/state.vscdb` (SQLite), where `composerData:<composer>` holds a chat's
//!   name, model and ordered message ids, and `bubbleId:<composer>:<bubble>` each message;
//! - the agent's `~/.cursor/projects/*/agent-transcripts/<id>/<id>.jsonl`, for a chat the IDE store
//!   doesn't have (one it does have is read from the store, which keeps times and tool calls).
//!
//! A subagent's chat is a chat of its own, linked to the chat that spawned it: the store's
//! `composerData:<subagent>` says by whom (`subagentInfo`), and its transcript sits in the spawning
//! chat's `agent-transcripts/<parent>/subagents/<subagent>.jsonl`.
//!
//! Cursor stores no usage per turn, so imported turns carry what the bubbles say (in practice 0 tokens)
//! and the model the chat used.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use nasiko_types::{
    CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCallStatus,
};
use rusqlite::Connection;
use rusqlite::types::ValueRef;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::super::model::{LlmCall, SessionSnapshot, ToolCall, Turn, subagent_parent};
use super::cursor::{self, provider_for_model};
use super::opencode_history::open_read_only;
use super::{PastSession, SessionSource, codex::nonempty, epoch_millis};
use nasiko_types::coding_agent::CodingAgentSessionParent;

/// The newest `composerData` layout this reads. A newer one may have moved what it relies on, so such
/// a chat counts as failed rather than being read wrong.
const NEWEST_COMPOSER_VERSION: u64 = 18;
const USER_BUBBLE: u64 = 1;
const ASSISTANT_BUBBLE: u64 = 2;
const UNKNOWN_MODEL: &str = "unknown";

/// Every Cursor chat, oldest first: the IDE's, then the agent transcripts the IDE store lacks.
pub fn past_sessions() -> Result<Vec<PastSession>> {
    past_sessions_in(store_path().as_deref(), &transcripts_root())
}

/// The IDE's store, when this computer has one.
fn store_path() -> Option<PathBuf> {
    dirs::config_dir()
        .map(|dir| dir.join("Cursor/User/globalStorage/state.vscdb"))
        .filter(|path| path.is_file())
}

fn transcripts_root() -> PathBuf {
    cursor::config_path().join("projects")
}

/// The chat that spawned chat `conversation_id`, when it is a subagent's: from the IDE store, or for a
/// chat it lacks, from where the agent filed its transcript. Read-only, like an import.
pub(super) fn subagent_parent_of(
    conversation_id: &str,
) -> Result<Option<CodingAgentSessionParent>> {
    subagent_parent_in(
        store_path().as_deref(),
        &transcripts_root(),
        conversation_id,
    )
}

fn subagent_parent_in(
    store: Option<&Path>,
    projects: &Path,
    conversation_id: &str,
) -> Result<Option<CodingAgentSessionParent>> {
    if let Some(store) = store {
        let db = open_read_only(store)?;
        if let Some(composer) = stored_json(&db, &format!("composerData:{conversation_id}"))? {
            return Ok(composer_parent(conversation_id, &composer));
        }
    }
    Ok(transcript_files(projects)
        .into_iter()
        .find(|transcript| transcript.id == conversation_id)
        .and_then(|transcript| transcript.parent)
        .and_then(|parent| subagent_parent(conversation_id, &parent, None, None)))
}

/// A subagent chat's link to the chat that spawned it, from the store's `subagentInfo`.
fn composer_parent(composer_id: &str, composer: &Value) -> Option<CodingAgentSessionParent> {
    let info = composer.get("subagentInfo")?;
    let parent_id = info["parentComposerId"]
        .as_str()
        .or_else(|| info["rootParentConversationId"].as_str())?;
    subagent_parent(
        composer_id,
        parent_id,
        info["toolCallId"].as_str(),
        info["subagentTypeName"].as_str(),
    )
}

fn past_sessions_in(store: Option<&Path>, projects: &Path) -> Result<Vec<PastSession>> {
    let mut sessions = Vec::new();
    let mut known = HashSet::new();
    if let Some(store) = store {
        let db = Rc::new(open_read_only(store)?);
        for (updated, composer) in composer_ids(&db)? {
            known.insert(composer.clone());
            let db = db.clone();
            sessions.push(PastSession {
                key: composer.clone(),
                last_active: epoch_millis(updated),
                source: SessionSource::Load(Box::new(move || composer_session(&db, &composer))),
            });
        }
    }
    sessions.extend(
        transcript_files(projects)
            .into_iter()
            .filter(|transcript| !known.contains(&transcript.id))
            .map(|transcript| PastSession {
                key: transcript.id.clone(),
                last_active: transcript.modified.into(),
                source: SessionSource::Load(Box::new(move || transcript_session(&transcript))),
            }),
    );
    Ok(sessions)
}

// ─── The IDE store ───────────────────────────────────────────────────────────

/// Composer ids with when each chat was last updated (in ms; its creation when the store kept no
/// update), oldest chat first.
fn composer_ids(db: &Connection) -> Result<Vec<(i64, String)>> {
    let mut statement = db.prepare(
        "SELECT substr(key, 14),
                coalesce(json_extract(value, '$.lastUpdatedAt'), json_extract(value, '$.createdAt'))
         FROM cursorDiskKV WHERE key >= 'composerData:' AND key < 'composerData;'",
    )?;
    let mut composers: Vec<(i64, String)> = statement
        .query_map([], |row| {
            Ok((row.get::<_, Option<i64>>(1)?.unwrap_or(0), row.get(0)?))
        })?
        .collect::<Result<_, _>>()
        .context("failed to list Cursor chats")?;
    composers.sort();
    Ok(composers)
}

fn composer_session(db: &Connection, composer_id: &str) -> Result<SessionSnapshot> {
    let composer = stored_json(db, &format!("composerData:{composer_id}"))?
        .with_context(|| format!("Cursor chat {composer_id} is gone"))?;
    let version = composer["_v"].as_u64().unwrap_or(0);
    if version == 0 || version > NEWEST_COMPOSER_VERSION {
        bail!("Cursor chat {composer_id} has an unknown layout (version {version})");
    }
    let bubbles = composer_bubbles(db, composer_id)?;
    let ordered: Vec<&Value> = composer["fullConversationHeadersOnly"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|header| bubbles.get(header["bubbleId"].as_str()?))
        .collect();
    let model = nonempty(composer["modelConfig"]["modelName"].as_str())
        .unwrap_or_else(|| UNKNOWN_MODEL.to_string());
    let created = composer["createdAt"]
        .as_i64()
        .and_then(DateTime::from_timestamp_millis);
    Ok(SessionSnapshot {
        place: None,
        session_id: composer_id.to_string(),
        title: nonempty(composer["name"].as_str()),
        turns: bubble_turns(composer_id, &ordered, &model, created),
        parent: composer_parent(composer_id, &composer),
    })
}

/// A chat's messages by bubble id. One range scan of the key index, not a lookup per message.
fn composer_bubbles(db: &Connection, composer_id: &str) -> Result<HashMap<String, Value>> {
    let mut statement =
        db.prepare("SELECT key, value FROM cursorDiskKV WHERE key >= ?1 AND key < ?2")?;
    let rows = statement.query_map(
        [
            format!("bubbleId:{composer_id}:"),
            format!("bubbleId:{composer_id};"),
        ],
        |row| Ok((row.get::<_, String>(0)?, json_value(row.get_ref(1)?))),
    )?;
    let mut bubbles = HashMap::new();
    for row in rows {
        let (key, value) = row?;
        if let (Some(id), Some(value)) = (key.rsplit(':').next(), value) {
            bubbles.insert(id.to_string(), value);
        }
    }
    Ok(bubbles)
}

fn stored_json(db: &Connection, key: &str) -> Result<Option<Value>> {
    let mut statement = db.prepare("SELECT value FROM cursorDiskKV WHERE key = ?1")?;
    let mut rows = statement.query([key])?;
    Ok(match rows.next()? {
        Some(row) => json_value(row.get_ref(0)?),
        None => None,
    })
}

/// The store keeps values as text or as blobs, depending on the Cursor version that wrote them.
fn json_value(value: ValueRef<'_>) -> Option<Value> {
    match value {
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => serde_json::from_slice(bytes).ok(),
        _ => None,
    }
}

/// One turn per user message, with the assistant messages that follow it.
fn bubble_turns(
    session_id: &str,
    bubbles: &[&Value],
    composer_model: &str,
    created: Option<DateTime<Utc>>,
) -> Vec<Turn> {
    let mut turns = Vec::new();
    let mut rest = bubbles;
    while let Some(start) = rest
        .iter()
        .position(|b| b["type"].as_u64() == Some(USER_BUBBLE))
    {
        let user = rest[start];
        let replies_end = rest[start + 1..]
            .iter()
            .position(|b| b["type"].as_u64() == Some(USER_BUBBLE))
            .map_or(rest.len(), |offset| start + 1 + offset);
        let replies: Vec<&Value> = rest[start + 1..replies_end]
            .iter()
            .copied()
            .filter(|b| b["type"].as_u64() == Some(ASSISTANT_BUBBLE))
            .collect();
        turns.extend(bubble_turn(
            session_id,
            user,
            &replies,
            composer_model,
            created,
        ));
        rest = &rest[replies_end..];
    }
    turns
}

fn bubble_turn(
    session_id: &str,
    user: &Value,
    replies: &[&Value],
    composer_model: &str,
    created: Option<DateTime<Utc>>,
) -> Option<Turn> {
    let uuid = nonempty(user["bubbleId"].as_str())?;
    let prompt = nonempty(user["text"].as_str())?;
    let started_at = bubble_time(user).or(created)?;
    let ended_at = replies
        .iter()
        .filter_map(|reply| bubble_time(reply))
        .max()
        .unwrap_or(started_at)
        .max(started_at);
    let response: Vec<&str> = replies
        .iter()
        .filter_map(|reply| reply["text"].as_str())
        .filter(|text| !text.trim().is_empty())
        .collect();
    let model = nonempty(user["modelInfo"]["modelName"].as_str())
        .unwrap_or_else(|| composer_model.to_string());
    let tokens = |field: &str| -> u64 {
        replies
            .iter()
            .filter_map(|reply| reply["tokenCount"][field].as_u64())
            .sum()
    };
    Some(Turn {
        prompt,
        response: nonempty(Some(&response.join("\n\n"))),
        started_at,
        ended_at,
        calls: vec![LlmCall {
            uuid: format!("{session_id}:{uuid}:llm"),
            provider: provider_for_model(&model).to_string(),
            model,
            input_tokens: tokens("inputTokens"),
            output_tokens: tokens("outputTokens"),
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            accounting: None,
            started_at,
            ended_at,
        }],
        tool_calls: replies
            .iter()
            .filter_map(|reply| bubble_tool(reply))
            .collect(),
        uuid,
    })
}

fn bubble_tool(bubble: &Value) -> Option<ToolCall> {
    let tool = &bubble["toolFormerData"];
    let id = nonempty(tool["toolCallId"].as_str())?;
    let at = bubble_time(bubble);
    Some(ToolCall {
        id,
        name: nonempty(tool["name"].as_str()).unwrap_or_else(|| "unknown".into()),
        kind: "tool".into(),
        model_call_id: None,
        status: tool_status(tool["status"].as_str()),
        arguments: tool
            .get("rawArgs")
            .or_else(|| tool.get("params"))
            .map(json_or_text),
        output: tool.get("result").map(json_or_text),
        raw: None,
        error: None,
        started_at: at,
        ended_at: at,
        duration_ms: None,
        association: CodingAgentToolAssociation::Turn,
        timestamp_quality: CodingAgentTimestampQuality::Inferred,
    })
}

fn tool_status(status: Option<&str>) -> CodingAgentToolCallStatus {
    match status.unwrap_or_default() {
        "completed" | "success" => CodingAgentToolCallStatus::Succeeded,
        "error" | "failed" => CodingAgentToolCallStatus::Failed,
        "cancelled" | "aborted" => CodingAgentToolCallStatus::Cancelled,
        "rejected" | "denied" => CodingAgentToolCallStatus::Denied,
        _ => CodingAgentToolCallStatus::Unknown,
    }
}

fn json_or_text(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_else(|| value.clone())
}

/// A bubble's `createdAt`: an RFC 3339 string in current stores, epoch milliseconds in older ones.
fn bubble_time(bubble: &Value) -> Option<DateTime<Utc>> {
    let created = &bubble["createdAt"];
    created
        .as_str()
        .and_then(|text| text.parse().ok())
        .or_else(|| created.as_i64().and_then(DateTime::from_timestamp_millis))
}

// ─── Agent transcripts ───────────────────────────────────────────────────────

/// One agent transcript: `projects/*/agent-transcripts/<id>/<id>.jsonl`, or a subagent's
/// `projects/*/agent-transcripts/<parent>/subagents/<id>.jsonl`.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct TranscriptFile {
    modified: std::time::SystemTime,
    id: String,
    path: PathBuf,
    /// The chat that spawned this one, for a subagent's.
    parent: Option<String>,
}

/// Every chat's transcript and every subagent's, oldest first.
fn transcript_files(projects: &Path) -> Vec<TranscriptFile> {
    let Ok(projects) = std::fs::read_dir(projects) else {
        return Vec::new();
    };
    let mut found: Vec<TranscriptFile> = projects
        .flatten()
        .filter_map(|project| std::fs::read_dir(project.path().join("agent-transcripts")).ok())
        .flatten()
        .flatten()
        .flat_map(|session| {
            let id = session.file_name().to_str().map(str::to_string);
            let own = id
                .as_ref()
                .and_then(|id| transcript_file(session.path().join(format!("{id}.jsonl")), None));
            let subagents = id
                .map(|id| subagent_transcripts(&session.path().join("subagents"), &id))
                .unwrap_or_default();
            own.into_iter().chain(subagents)
        })
        .collect();
    found.sort();
    found
}

fn subagent_transcripts(dir: &Path, parent: &str) -> Vec<TranscriptFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|path| transcript_file(path, Some(parent.to_string())))
        .collect()
}

fn transcript_file(path: PathBuf, parent: Option<String>) -> Option<TranscriptFile> {
    let id = path.file_stem()?.to_str()?.to_string();
    let modified = path.metadata().ok()?.modified().ok()?;
    Some(TranscriptFile {
        modified,
        id,
        path,
        parent,
    })
}

/// A transcript's turns, split where each ends. It keeps no times, so every turn is dated to when the
/// file was last written, and no model.
fn transcript_session(transcript: &TranscriptFile) -> Result<SessionSnapshot> {
    let path = &transcript.path;
    let session_id = transcript.id.as_str();
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let at: DateTime<Utc> = path
        .metadata()
        .and_then(|metadata| metadata.modified())
        .with_context(|| format!("failed to read the time of {}", path.display()))?
        .into();
    Ok(SessionSnapshot {
        place: None,
        session_id: session_id.to_string(),
        title: None,
        turns: transcript_turns(session_id, &content, at),
        parent: transcript
            .parent
            .as_deref()
            .and_then(|parent| subagent_parent(session_id, parent, None, None)),
    })
}

fn transcript_turns(session_id: &str, content: &str, at: DateTime<Utc>) -> Vec<Turn> {
    let entries: Vec<Value> = content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    entries
        .split(|entry| entry["type"] == "turn_ended")
        .enumerate()
        .filter_map(|(index, lines)| transcript_turn(session_id, index, lines, at))
        .collect()
}

fn transcript_turn(
    session_id: &str,
    index: usize,
    lines: &[Value],
    at: DateTime<Utc>,
) -> Option<Turn> {
    let text_of = |role: &str| -> Vec<String> {
        lines
            .iter()
            .filter(|entry| entry["role"] == role)
            .flat_map(|entry| {
                entry["message"]["content"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .filter(|part| part["type"] == "text")
            .filter_map(|part| nonempty(part["text"].as_str()))
            .collect()
    };
    let prompt = nonempty(Some(&text_of("user").join("\n\n")))?;
    let uuid = format!("{session_id}:{index}");
    let tool_calls = lines
        .iter()
        .filter(|entry| entry["role"] == "assistant")
        .flat_map(|entry| {
            entry["message"]["content"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|part| part["type"] == "tool_use")
        .enumerate()
        .map(|(number, part)| ToolCall {
            id: format!("{uuid}:tool:{number}"),
            name: nonempty(part["name"].as_str()).unwrap_or_else(|| "unknown".into()),
            kind: "tool".into(),
            model_call_id: None,
            status: CodingAgentToolCallStatus::Unknown,
            arguments: part.get("input").map(json_or_text),
            output: None,
            raw: None,
            error: None,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            association: CodingAgentToolAssociation::Turn,
            timestamp_quality: CodingAgentTimestampQuality::Unknown,
        })
        .collect();
    Some(Turn {
        prompt,
        response: nonempty(Some(&text_of("assistant").join("\n\n"))),
        started_at: at,
        ended_at: at,
        calls: vec![LlmCall {
            uuid: format!("{uuid}:llm"),
            provider: provider_for_model(UNKNOWN_MODEL).to_string(),
            model: UNKNOWN_MODEL.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            accounting: None,
            started_at: at,
            ended_at: at,
        }],
        tool_calls,
        uuid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn put(db: &Connection, key: &str, value: Value) {
        db.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
            [key, &value.to_string()],
        )
        .unwrap();
    }

    fn bubble(id: &str, kind: u64, at: &str, text: &str) -> Value {
        json!({"_v": 3, "bubbleId": id, "type": kind, "createdAt": at, "text": text,
               "tokenCount": {"inputTokens": 0, "outputTokens": 0}})
    }

    fn header(id: &str, kind: u64) -> Value {
        json!({"bubbleId": id, "type": kind})
    }

    /// A store with one readable chat (two turns, one tool call) and one in a layout from the future.
    fn store(path: &Path) {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE cursorDiskKV (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)",
        )
        .unwrap();
        put(
            &db,
            "composerData:c1",
            json!({"_v": 18, "composerId": "c1", "name": "Refactor", "createdAt": 1_790_000_000_000_i64,
                   "lastUpdatedAt": 1_790_000_050_000_i64,
                   "modelConfig": {"modelName": "claude-4.5-sonnet"},
                   "fullConversationHeadersOnly": [header("u1", 1), header("a1", 2), header("a2", 2),
                                                   header("u2", 1), header("a3", 2)]}),
        );
        put(
            &db,
            "bubbleId:c1:u1",
            bubble("u1", 1, "2026-10-01T10:00:00.000Z", "Rename it"),
        );
        put(&db, "bubbleId:c1:a1", {
            let mut tool = bubble("a1", 2, "2026-10-01T10:00:02.000Z", "");
            tool["toolFormerData"] = json!({"toolCallId": "tool-1", "name": "edit_file",
                                             "status": "completed", "rawArgs": "{\"path\":\"a.rs\"}",
                                             "result": "{\"ok\":true}"});
            tool
        });
        put(
            &db,
            "bubbleId:c1:a2",
            bubble("a2", 2, "2026-10-01T10:00:05.000Z", "Renamed"),
        );
        put(
            &db,
            "bubbleId:c1:u2",
            bubble("u2", 1, "2026-10-01T10:01:00.000Z", "Thanks"),
        );
        put(
            &db,
            "bubbleId:c1:a3",
            bubble("a3", 2, "2026-10-01T10:01:01.000Z", "Any time"),
        );
        put(
            &db,
            "composerData:c2",
            json!({"_v": 99, "createdAt": 1_790_000_100_000_i64, "fullConversationHeadersOnly": []}),
        );
    }

    fn load(session: PastSession) -> Result<SessionSnapshot> {
        match session.source {
            SessionSource::Load(load) => load(),
            SessionSource::Payload(_) => panic!("Cursor sessions load from their store"),
        }
    }

    #[test]
    fn ide_chats_become_turns_with_their_model_times_and_no_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.vscdb");
        store(&path);

        let mut sessions = past_sessions_in(Some(&path), &dir.path().join("none")).unwrap();

        assert_eq!(sessions.len(), 2);
        // Dated by the chat's last update, or its creation when it has none.
        let dates: Vec<(&str, i64)> = sessions
            .iter()
            .map(|s| (s.key.as_str(), s.last_active.timestamp_millis()))
            .collect();
        assert_eq!(
            dates,
            [("c1", 1_790_000_050_000), ("c2", 1_790_000_100_000)]
        );
        // A layout this doesn't know fails that chat alone, without a panic.
        assert!(load(sessions.pop().unwrap()).is_err());
        let snapshot = load(sessions.pop().unwrap()).unwrap();
        assert_eq!(snapshot.session_id, "c1");
        assert_eq!(snapshot.title.as_deref(), Some("Refactor"));
        let ids: Vec<&str> = snapshot.turns.iter().map(|t| t.uuid.as_str()).collect();
        assert_eq!(ids, ["u1", "u2"]);
        let first = &snapshot.turns[0];
        assert_eq!(first.prompt, "Rename it");
        assert_eq!(first.response.as_deref(), Some("Renamed"));
        assert_eq!((first.ended_at - first.started_at).num_seconds(), 5);
        assert_eq!(first.calls[0].uuid, "c1:u1:llm");
        assert_eq!(first.calls[0].model, "claude-4.5-sonnet");
        assert_eq!(first.calls[0].provider, "anthropic");
        assert_eq!(
            (first.calls[0].input_tokens, first.calls[0].output_tokens),
            (0, 0)
        );
        assert_eq!(first.tool_calls[0].id, "tool-1");
        assert_eq!(
            first.tool_calls[0].status,
            CodingAgentToolCallStatus::Succeeded
        );
        assert_eq!(first.tool_calls[0].arguments, Some(json!({"path": "a.rs"})));
    }

    #[test]
    fn a_subagent_chat_in_the_store_is_linked_to_the_chat_and_call_that_spawned_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.vscdb");
        store(&path);
        let db = Connection::open(&path).unwrap();
        put(
            &db,
            "composerData:sub",
            json!({"_v": 18, "createdAt": 1_790_000_000_000_i64,
                   "fullConversationHeadersOnly": [header("su", 1), header("sa", 2)],
                   "subagentInfo": {"subagentType": 3, "parentComposerId": "c1",
                                    "rootParentConversationId": "c1",
                                    "toolCallId": "call_1\nfc_1", "subagentTypeName": "explore"}}),
        );
        put(
            &db,
            "bubbleId:sub:su",
            bubble("su", 1, "2026-10-01T10:00:01.000Z", "Look"),
        );
        put(
            &db,
            "bubbleId:sub:sa",
            bubble("sa", 2, "2026-10-01T10:00:03.000Z", "Found"),
        );
        let db = open_read_only(&path).unwrap();

        let snapshot = composer_session(&db, "sub").unwrap();

        let parent = snapshot.parent.as_ref().unwrap();
        assert_eq!(parent.source_id, "c1");
        assert_eq!(parent.tool_call_id.as_deref(), Some("call_1\nfc_1"));
        assert_eq!(parent.agent_type.as_deref(), Some("explore"));
        assert!(composer_session(&db, "c1").unwrap().parent.is_none());
        let event =
            super::super::super::report::canonical_event_for_test(&snapshot, &snapshot.turns[0]);
        assert_eq!(event.validate(), Ok(()));
        assert_eq!(event.session.parent.as_ref(), Some(parent));

        // What a live hook for the subagent's chat looks up.
        let none = dir.path().join("none");
        let live = subagent_parent_in(Some(&path), &none, "sub")
            .unwrap()
            .unwrap();
        assert_eq!(&live, parent);
        assert!(
            subagent_parent_in(Some(&path), &none, "c1")
                .unwrap()
                .is_none()
        );
        assert!(
            subagent_parent_in(Some(&path), &none, "unknown")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_subagent_chat_only_a_transcript_has_is_linked_by_where_it_is_filed() {
        let dir = tempfile::tempdir().unwrap();
        let subagents = dir.path().join("p/agent-transcripts/parent/subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        std::fs::write(subagents.join("child.jsonl"), "").unwrap();

        let parent = subagent_parent_in(None, dir.path(), "child")
            .unwrap()
            .unwrap();

        assert_eq!(parent.source_id, "parent");
        assert_eq!(parent.tool_call_id, None);
        assert!(
            subagent_parent_in(None, dir.path(), "parent")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_zero_token_imported_turn_is_a_valid_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.vscdb");
        store(&path);
        let db = open_read_only(&path).unwrap();
        let snapshot = composer_session(&db, "c1").unwrap();

        let event =
            super::super::super::report::canonical_event_for_test(&snapshot, &snapshot.turns[0]);

        assert_eq!(event.validate(), Ok(()));
    }

    #[test]
    fn transcripts_split_on_turn_end_and_skip_chats_the_store_has() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.vscdb");
        store(&path);
        let projects = dir.path().join("projects");
        let content = [
            json!({"role": "user", "message": {"content": [{"type": "text", "text": "Hi"}]}}),
            json!({"role": "assistant", "message": {"content": [{"type": "tool_use", "name": "Read", "input": {"path": "x"}}]}}),
            json!({"role": "assistant", "message": {"content": [{"type": "text", "text": "Hello"}]}}),
            json!({"type": "turn_ended", "status": "completed"}),
            json!({"role": "user", "message": {"content": [{"type": "text", "text": "Still there?"}]}}),
        ]
        .map(|line| line.to_string())
        .join("\n");
        for id in ["t1", "c1"] {
            let session = projects.join("p/agent-transcripts").join(id);
            std::fs::create_dir_all(session.join("subagents")).unwrap();
            std::fs::write(session.join(format!("{id}.jsonl")), &content).unwrap();
        }
        // Subagents of t1: one the store has (read from there), one only its transcript has.
        let subagents = projects.join("p/agent-transcripts/t1/subagents");
        std::fs::write(subagents.join("sub-c1.jsonl"), &content).unwrap();
        std::fs::write(subagents.join("sub-t.jsonl"), &content).unwrap();
        put(
            &Connection::open(&path).unwrap(),
            "composerData:sub-c1",
            json!({"_v": 18, "createdAt": 1}),
        );

        let sessions = past_sessions_in(Some(&path), &projects).unwrap();

        // c1 and sub-c1 are read from the store; t1 and sub-t only have a transcript.
        let mut keys: Vec<&str> = sessions.iter().map(|s| s.key.as_str()).collect();
        keys.sort();
        assert_eq!(keys, ["c1", "c2", "sub-c1", "sub-t", "t1"]);
        let mut by_key: HashMap<String, PastSession> =
            sessions.into_iter().map(|s| (s.key.clone(), s)).collect();
        let subagent = load(by_key.remove("sub-t").unwrap()).unwrap();
        assert_eq!(subagent.parent.unwrap().source_id, "t1");
        assert_eq!(subagent.turns.len(), 2);
        let snapshot = load(by_key.remove("t1").unwrap()).unwrap();
        assert!(snapshot.parent.is_none());
        assert_eq!(snapshot.session_id, "t1");
        assert_eq!(snapshot.turns.len(), 2);
        let first = &snapshot.turns[0];
        assert_eq!(first.uuid, "t1:0");
        assert_eq!(first.response.as_deref(), Some("Hello"));
        assert_eq!(first.calls[0].model, "unknown");
        assert_eq!(first.tool_calls[0].name, "Read");
        // The unfinished last turn has no answer yet, so it isn't complete.
        assert!(snapshot.turns[1].response.is_none());
    }
}
