//! Claude Code hook lifecycle, typed payload, and concrete JSONL correlation.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nasiko_types::{
    CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCallStatus,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::super::catalog::{self, AgentSpec, Support};
use super::super::launcher;
use super::super::model::{LlmCall, SessionSnapshot, ToolCall, Turn, subagent_parent};
use super::super::sanitize::{cache_ttl_within, tool_link};
use super::{PastSession, SessionSource};

pub const INSTALL_VERSION: u32 = 4;
pub const SPEC: AgentSpec = AgentSpec {
    id: "claude",
    display_name: "Claude Code",
    binary: "claude",
    agent_name: "claude-code",
    support: Support::Instrumented,
};

/// `Stop` ends a session's turn; `SubagentStop` ends a subagent's, which Claude Code keeps in a
/// transcript of its own (`<session>/subagents/agent-<id>.jsonl`).
const HOOK_EVENTS: [&str; 2] = ["Stop", SUBAGENT_STOP];
const SUBAGENT_STOP: &str = "SubagentStop";
const HOOK_TIMEOUT_SECS: u32 = 10;
/// Ends the instructions Claude Code puts before a forked subagent's directive.
const FORK_BOILERPLATE_END: &str = "</fork-boilerplate>";
/// How long a live capture rereads a transcript whose last turn isn't written completely yet.
const COMPLETION_WAIT: Duration = Duration::from_secs(3);

/// What a Claude Code hook sends. `session_id` and `transcript_path` are always the main session's;
/// `SubagentStop` adds the finished subagent's own transcript. An import of a subagent's transcript
/// replays that same shape.
#[derive(Debug, Deserialize)]
struct HookPayload {
    session_id: String,
    transcript_path: PathBuf,
    #[serde(default)]
    hook_event_name: Option<String>,
    #[serde(default)]
    agent_transcript_path: Option<PathBuf>,
}

/// What Claude Code records beside a subagent's transcript (`agent-<id>.meta.json`).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentMeta {
    agent_type: Option<String>,
    description: Option<String>,
    tool_use_id: Option<String>,
    /// The subagent that spawned this one, for a subagent of a subagent.
    parent_agent_id: Option<String>,
}

pub fn config_path() -> PathBuf {
    catalog::home().join(".claude")
}

/// Every session Claude Code kept (`projects/<project>/<session>.jsonl`) and every subagent's
/// (`projects/<project>/<session>/subagents/agent-<id>.jsonl`), oldest first, as the payload its Stop or
/// SubagentStop hook would carry, so an import replays them through the hook's own capture.
pub fn past_sessions() -> Vec<PastSession> {
    past_sessions_in(&config_path().join("projects"))
}

fn past_sessions_in(projects: &Path) -> Vec<PastSession> {
    let Ok(dirs) = std::fs::read_dir(projects) else {
        return Vec::new();
    };
    let mut sessions: Vec<(std::time::SystemTime, PathBuf)> = dirs
        .flatten()
        .filter_map(|project| std::fs::read_dir(project.path()).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .flat_map(|path| {
            let subagents = subagent_transcripts(&path.join("subagents"));
            std::iter::once(path).chain(subagents)
        })
        .filter(|path| is_transcript(path))
        .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
        .collect();
    sessions.sort();
    sessions
        .into_iter()
        .filter_map(|(modified, path)| {
            Some(PastSession {
                key: path.to_string_lossy().into_owned(),
                last_active: modified.into(),
                source: SessionSource::Payload(past_payload(&path)?.to_string()),
            })
        })
        .collect()
}

fn is_transcript(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|ext| ext == "jsonl")
}

/// The hook payload that would report the transcript at `path`: its session's Stop, or for a
/// subagent's transcript, the SubagentStop of the session it belongs to.
fn past_payload(path: &Path) -> Option<Value> {
    let in_subagents = path
        .parent()
        .is_some_and(|dir| dir.file_name().is_some_and(|name| name == "subagents"));
    if !in_subagents {
        return Some(json!({
            "session_id": path.file_stem()?.to_str()?,
            "transcript_path": path,
            "hook_event_name": "Stop",
        }));
    }
    let session_dir = path.parent()?.parent()?;
    let session_id = session_dir.file_name()?.to_str()?;
    Some(json!({
        "session_id": session_id,
        "transcript_path": session_dir.with_extension("jsonl"),
        "hook_event_name": SUBAGENT_STOP,
        "agent_transcript_path": path,
    }))
}

/// The subagent transcripts (`agent-*.jsonl`) in a session's `subagents/` folder; none when it has
/// no such folder.
fn subagent_transcripts(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            is_transcript(path)
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("agent-"))
        })
        .collect();
    paths.sort();
    paths
}

/// A subagent's own session id: its transcript's name (`agent-<id>`) within the session that ran it.
/// Stable across reads, and never the parent's.
fn subagent_session_id(parent_session_id: &str, transcript_stem: &str) -> String {
    format!("{parent_session_id}:{transcript_stem}")
}

pub fn install() -> Result<(PathBuf, PathBuf)> {
    let script = launcher::install(&config_path(), SPEC.id, INSTALL_VERSION)?;
    let settings = settings_path();
    if let Err(error) = register_settings(&settings, &script) {
        let _ = launcher::uninstall(&config_path());
        return Err(error);
    }
    Ok((script, settings))
}

pub fn uninstall() -> Result<()> {
    deregister_settings(&settings_path())?;
    launcher::uninstall(&config_path())
}

pub fn installed_version() -> Option<u32> {
    let version = launcher::installed_version(&config_path())?;
    let expected_command = hook_command(&launcher::script_path(&config_path()));
    let settings = read_settings(&settings_path()).ok()?;
    HOOK_EVENTS
        .iter()
        .all(|event| has_current_hook(&settings, event, &expected_command))
        .then_some(version)
}

fn has_current_hook(settings: &Value, event: &str, expected_command: &str) -> bool {
    settings["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
        .any(|hook| is_current_hook(hook, expected_command))
}

fn settings_path() -> PathBuf {
    config_path().join("settings.json")
}

fn register_settings(path: &Path, script: &Path) -> Result<()> {
    let mut settings = read_settings(path)?;
    for event in HOOK_EVENTS {
        let mut groups = hook_groups_without_nasiko(&settings, event);
        groups.push(json!({
            "matcher": "*",
            "hooks": [{
                "type": "command",
                "command": hook_command(script),
                "timeout": HOOK_TIMEOUT_SECS,
            }],
        }));
        set_hook_groups(&mut settings, event, groups);
    }
    write_settings(path, &settings)
}

fn deregister_settings(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut settings = read_settings(path)?;
    for event in HOOK_EVENTS {
        let groups = hook_groups_without_nasiko(&settings, event);
        set_hook_groups(&mut settings, event, groups);
    }
    write_settings(path, &settings)
}

fn hook_groups_without_nasiko(settings: &Value, event: &str) -> Vec<Value> {
    let Some(groups) = settings["hooks"][event].as_array() else {
        return Vec::new();
    };
    groups
        .iter()
        .filter_map(|group| {
            let mut group = group.clone();
            let Some(hooks) = group["hooks"].as_array() else {
                return Some(group);
            };
            let kept: Vec<_> = hooks
                .iter()
                .filter(|hook| !is_nasiko_hook(hook))
                .cloned()
                .collect();
            if kept.is_empty() {
                None
            } else {
                group["hooks"] = Value::Array(kept);
                Some(group)
            }
        })
        .collect()
}

fn is_nasiko_hook(hook: &Value) -> bool {
    hook["command"]
        .as_str()
        .is_some_and(|command| command.contains(launcher::SCRIPT_NAME))
}

fn hook_command(script: &Path) -> String {
    format!("bash {}", launcher::shell_quote(&script.to_string_lossy()))
}

fn is_current_hook(hook: &Value, expected_command: &str) -> bool {
    hook["type"] == "command" && hook["command"].as_str() == Some(expected_command)
}

fn set_hook_groups(settings: &mut Value, event: &str, groups: Vec<Value>) {
    if !settings.is_object() {
        *settings = json!({});
    }
    if !settings["hooks"].is_object() {
        settings["hooks"] = json!({});
    }
    if groups.is_empty() {
        if let Some(hooks) = settings["hooks"].as_object_mut() {
            hooks.remove(event);
        }
    } else {
        settings["hooks"][event] = Value::Array(groups);
    }
}

fn read_settings(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&content).with_context(|| {
        format!(
            "{} is not valid JSON — fix or move it, then retry",
            path.display()
        )
    })
}

fn write_settings(path: &Path, settings: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, serde_json::to_string_pretty(settings)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

/// The transcript the payload names: the session's, or on `SubagentStop` the finished subagent's.
pub fn snapshot(raw: &str, report_deadline: Instant) -> Result<SessionSnapshot> {
    let payload = hook_payload(raw)?;
    match &payload.agent_transcript_path {
        Some(path) => until_complete(report_deadline, || read_subagent(&payload.session_id, path)),
        None => until_complete(report_deadline, || read_transcript(&payload)),
    }
}

/// Every session a live hook has finished turns in. `Stop`: the session, then each of its subagents
/// (a subagent that finished earlier is re-read, and the watermarks keep its turns from being queued
/// twice). `SubagentStop`: the subagent that finished, or every subagent when the payload doesn't say
/// which; the session's own turn is still running then, so it isn't read. A subagent that can't be
/// read is an error of its own and doesn't cost the others.
pub fn hook_snapshots(raw: &str, report_deadline: Instant) -> Result<Vec<Result<SessionSnapshot>>> {
    let payload = hook_payload(raw)?;
    let is_subagent_stop = payload.hook_event_name.as_deref() == Some(SUBAGENT_STOP);
    if is_subagent_stop && payload.agent_transcript_path.is_some() {
        return Ok(vec![snapshot(raw, report_deadline)]);
    }
    let session =
        (!is_subagent_stop).then(|| until_complete(report_deadline, || read_transcript(&payload)));
    let subagents = subagent_transcripts(&subagents_dir(&payload))
        .into_iter()
        .map(|path| read_subagent(&payload.session_id, &path));
    Ok(session.into_iter().chain(subagents).collect())
}

/// [`snapshot`] without the wait: one read, for a session imported long after it was written.
pub fn snapshot_once(raw: &str) -> Result<SessionSnapshot> {
    let payload = hook_payload(raw)?;
    match &payload.agent_transcript_path {
        Some(path) => read_subagent(&payload.session_id, path),
        None => read_transcript(&payload),
    }
}

/// `read` until the transcript's last turn is complete or the wait is over: a hook can fire before
/// Claude Code has written the turn's last record.
fn until_complete(
    report_deadline: Instant,
    read: impl Fn() -> Result<SessionSnapshot>,
) -> Result<SessionSnapshot> {
    let wait_deadline = (Instant::now() + COMPLETION_WAIT).min(report_deadline);
    loop {
        let snapshot = read()?;
        if snapshot
            .turns
            .last()
            .is_some_and(|turn| !turn.is_empty() && turn.response.is_some())
            || Instant::now() >= wait_deadline
        {
            return Ok(snapshot);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `<project>/<session>/subagents`, beside the session's `<project>/<session>.jsonl`.
fn subagents_dir(payload: &HookPayload) -> PathBuf {
    payload
        .transcript_path
        .with_file_name(&payload.session_id)
        .join("subagents")
}

fn hook_payload(raw: &str) -> Result<HookPayload> {
    serde_json::from_str(raw).with_context(|| {
        format!(
            "Claude hook payload is not expected JSON; got: {}",
            preview(raw)
        )
    })
}

fn read_transcript(payload: &HookPayload) -> Result<SessionSnapshot> {
    let content = std::fs::read_to_string(&payload.transcript_path).with_context(|| {
        format!(
            "failed to read transcript {}",
            payload.transcript_path.display()
        )
    })?;
    Ok(SessionSnapshot {
        title: session_title(&content, &payload.session_id),
        session_id: payload.session_id.clone(),
        turns: turns_from_lines(&content),
        parent: None,
    })
}

/// A subagent's transcript as a session of its own, linked to the session (or, for a subagent a
/// subagent spawned, the subagent) that ran it. Its records are all sidechain records, and are its turns.
fn read_subagent(parent_session_id: &str, path: &Path) -> Result<SessionSnapshot> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .with_context(|| format!("subagent transcript {} has no name", path.display()))?;
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read transcript {}", path.display()))?;
    let meta = subagent_meta(&path.with_extension("meta.json"))?;
    let session_id = subagent_session_id(parent_session_id, stem);
    let parent_id = meta.parent_agent_id.as_deref().map_or_else(
        || parent_session_id.to_string(),
        |agent| subagent_session_id(parent_session_id, &format!("agent-{agent}")),
    );
    Ok(SessionSnapshot {
        parent: subagent_parent(
            &session_id,
            &parent_id,
            meta.tool_use_id.as_deref(),
            meta.agent_type.as_deref(),
        ),
        title: meta.description.as_deref().and_then(nonempty),
        turns: subagent_turns_from_lines(&content),
        session_id,
    })
}

/// A subagent's `agent-<id>.meta.json`; none written yet is no metadata, one that isn't JSON an error.
fn subagent_meta(path: &Path) -> Result<SubagentMeta> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not the expected JSON", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SubagentMeta::default()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn preview(raw: &str) -> String {
    raw.chars().take(200).collect()
}

fn session_title(content: &str, session_id: &str) -> Option<String> {
    let mut custom = None;
    let mut generated = None;
    for line in content.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record["sessionId"].as_str() != Some(session_id) {
            continue;
        }
        let (target, field) = match record["type"].as_str() {
            Some("custom-title") => (&mut custom, "customTitle"),
            Some("ai-title") => (&mut generated, "aiTitle"),
            _ => continue,
        };
        if let Some(title) = record[field].as_str() {
            *target = nonempty(title);
        }
    }
    custom.or(generated)
}

/// The turns in a transcript, each tool linked exactly only to an LLM call its turn has: a tool whose
/// call made no LLM call of its own (no usage, say) is linked to the turn instead.
fn turns_from_lines(content: &str) -> Vec<Turn> {
    parse_turns(content, false)
        .into_iter()
        .map(with_known_tool_links)
        .collect()
}

/// [`turns_from_lines`] for a subagent's own transcript, where every record is a sidechain record and
/// the sidechain is the conversation.
fn subagent_turns_from_lines(content: &str) -> Vec<Turn> {
    parse_turns(content, true)
        .into_iter()
        .map(with_known_tool_links)
        .collect()
}

fn with_known_tool_links(turn: Turn) -> Turn {
    let calls: HashSet<String> = turn.calls.iter().map(|call| call.uuid.clone()).collect();
    let tool_calls = turn
        .tool_calls
        .into_iter()
        .map(|tool| {
            let (association, model_call_id) =
                tool_link(tool.association, tool.model_call_id.clone(), &calls);
            ToolCall {
                association,
                model_call_id,
                ..tool
            }
        })
        .collect();
    Turn { tool_calls, ..turn }
}

/// `sidechain_is_own`: the transcript is a subagent's, so its sidechain records answer its prompts. In a
/// session's transcript they are a subagent's, and never answer the session's turn.
fn parse_turns(content: &str, sidechain_is_own: bool) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut owners: HashMap<String, usize> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut calls: HashMap<String, (usize, usize, bool, DateTime<Utc>)> = HashMap::new();
    let mut tools: HashMap<String, (usize, usize)> = HashMap::new();
    let mut fallback_turns: HashSet<usize> = HashSet::new();
    let mut previous_at: Option<DateTime<Utc>> = None;
    let mut adjacent_user: Option<usize> = None;
    let mut parents: HashMap<String, String> = HashMap::new();
    let mut pending_assistants: Vec<PendingAssistant> = Vec::new();

    for (line_index, line) in content.lines().enumerate() {
        let Some(mut entry) = parse_entry(line) else {
            continue;
        };
        if sidechain_is_own {
            entry.is_sidechain = false;
            // A forked subagent's transcript opens with what it inherited from its parent, the
            // parent's own LLM call included: that is the parent's, so nothing before its own prompt
            // counts.
            if turns.is_empty() && entry.kind == "assistant" {
                continue;
            }
        }
        if matches!(entry.kind.as_str(), "ai-title" | "custom-title") {
            continue;
        }
        if let (Some(uuid), Some(parent)) = (&entry.uuid, &entry.parent_uuid) {
            parents.insert(uuid.clone(), parent.clone());
        }
        let duplicate = entry
            .uuid
            .as_ref()
            .is_some_and(|id| !seen.insert(id.clone()));
        let directly_preceding_user = adjacent_user.take();

        // A later copy of an assistant record can be the completed form of an
        // earlier partial record. It may upgrade the call, but cannot add one.
        if duplicate && entry.kind != "assistant" {
            continue;
        }

        let parent_owner = entry
            .parent_uuid
            .as_ref()
            .and_then(|parent| owners.get(parent))
            .copied();

        if entry.kind == "last-prompt" {
            let Some(prompt) = entry.user_prompt() else {
                continue;
            };
            let linked = entry
                .leaf_uuid
                .as_ref()
                .and_then(|leaf| owners.get(leaf))
                .copied()
                .or(directly_preceding_user);
            if let Some(owner) = linked {
                // leafUuid is an identity link, not a text match. Preserve the
                // real user record's UUID and timestamp when both are present.
                if let Some(leaf) = &entry.leaf_uuid {
                    owners.insert(leaf.clone(), owner);
                }
                continue;
            }

            let identity = entry.stable_identity(line_index);
            let at = previous_at.unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
            let owner = turns.len();
            turns.push(Turn {
                uuid: identity.clone(),
                prompt,
                response: None,
                started_at: at,
                ended_at: at,
                calls: Vec::new(),
                tool_calls: Vec::new(),
            });
            if entry.uuid.is_none() && entry.leaf_uuid.is_none() {
                fallback_turns.insert(owner);
            }
            owners.insert(identity, owner);
            if let Some(leaf) = &entry.leaf_uuid {
                owners.insert(leaf.clone(), owner);
                let mut remaining = Vec::new();
                for pending in pending_assistants.drain(..) {
                    if ancestry_related(&pending.entry, leaf, &parents) {
                        attach_assistant(
                            &pending.entry,
                            owner,
                            pending.started_at,
                            pending.ended_at,
                            &mut turns,
                            AssistantIndexes {
                                owners: &mut owners,
                                calls: &mut calls,
                                tools: &mut tools,
                                fallback_turns: &mut fallback_turns,
                            },
                        );
                    } else {
                        remaining.push(pending);
                    }
                }
                pending_assistants = remaining;
            }
            continue;
        }

        // A forked subagent's prompt arrives beside the result of the parent's call that forked it.
        let is_fork_prompt = sidechain_is_own && turns.is_empty() && entry.user_prompt().is_some();
        if entry.kind == "user" && !is_fork_prompt {
            let results = entry.tool_results();
            if !results.is_empty() {
                if let Some(owner) = parent_owner {
                    for result in results {
                        if let Some(&(tool_owner, index)) = tools.get(&result.id) {
                            let tool = &mut turns[tool_owner].tool_calls[index];
                            tool.output = result.output;
                            tool.error = result.error;
                            tool.status = if result.is_error {
                                CodingAgentToolCallStatus::Failed
                            } else {
                                CodingAgentToolCallStatus::Succeeded
                            };
                            tool.ended_at = entry.timestamp;
                            tool.duration_ms = tool
                                .started_at
                                .zip(tool.ended_at)
                                .map(|(start, end)| (end - start).num_milliseconds().max(0) as u64);
                        }
                    }
                    if let Some(uuid) = &entry.uuid {
                        owners.insert(uuid.clone(), owner);
                    }
                }
                if let Some(at) = entry.timestamp {
                    previous_at = Some(at);
                }
                continue;
            }
        }

        if let Some(prompt) = entry.user_prompt() {
            let identity = entry.stable_identity(line_index);
            let at = entry.timestamp.unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
            let owner = turns.len();
            turns.push(Turn {
                uuid: identity.clone(),
                prompt,
                response: None,
                started_at: at,
                ended_at: at,
                calls: Vec::new(),
                tool_calls: Vec::new(),
            });
            owners.insert(identity, owner);
            adjacent_user = Some(owner);
        } else if entry.kind == "assistant" {
            let Some(at) = entry.timestamp else {
                continue;
            };
            let owner = parent_owner.or_else(|| {
                turns
                    .len()
                    .checked_sub(1)
                    .filter(|&owner| turns[owner].response.is_none())
            });
            let Some(owner) = owner else {
                pending_assistants.push(PendingAssistant {
                    started_at: previous_at.unwrap_or(at),
                    ended_at: at,
                    entry,
                });
                previous_at = Some(at);
                continue;
            };
            attach_assistant(
                &entry,
                owner,
                previous_at.unwrap_or(at),
                at,
                &mut turns,
                AssistantIndexes {
                    owners: &mut owners,
                    calls: &mut calls,
                    tools: &mut tools,
                    fallback_turns: &mut fallback_turns,
                },
            );
        } else if let (Some(uuid), Some(owner)) = (&entry.uuid, parent_owner) {
            // Tool results and metadata preserve ancestry to the open turn.
            owners.insert(uuid.clone(), owner);
        }

        if let Some(at) = entry.timestamp {
            previous_at = Some(at);
        }
    }

    turns
}

struct PendingAssistant {
    entry: Entry,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
}

struct AssistantIndexes<'a> {
    owners: &'a mut HashMap<String, usize>,
    calls: &'a mut HashMap<String, (usize, usize, bool, DateTime<Utc>)>,
    tools: &'a mut HashMap<String, (usize, usize)>,
    fallback_turns: &'a mut HashSet<usize>,
}

fn attach_assistant(
    entry: &Entry,
    owner: usize,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    turns: &mut [Turn],
    indexes: AssistantIndexes<'_>,
) {
    let AssistantIndexes {
        owners,
        calls,
        tools,
        fallback_turns,
    } = indexes;
    // Async hook records can be appended before an older assistant record.
    let started_at = started_at.min(ended_at);
    for tool in entry.tool_uses(ended_at) {
        if let Some(&(old_owner, index)) = tools.get(&tool.id) {
            let existing = &mut turns[old_owner].tool_calls[index];
            existing.name = tool.name;
            existing.kind = tool.kind;
            existing.model_call_id = tool.model_call_id;
            existing.arguments = tool.arguments;
            existing.started_at = existing.started_at.or(tool.started_at);
            if matches!(
                existing.status,
                CodingAgentToolCallStatus::Pending | CodingAgentToolCallStatus::Running
            ) {
                existing.status = tool.status;
            }
        } else {
            tools.insert(tool.id.clone(), (owner, turns[owner].tool_calls.len()));
            turns[owner].tool_calls.push(tool);
        }
    }
    if let Some(call) = entry.llm_call(started_at, ended_at) {
        let call_id = call.uuid.clone();
        let final_observation = entry
            .message
            .as_ref()
            .and_then(|m| m.stop_reason.as_ref())
            .is_some();
        let call_owner =
            if let Some(&(old_owner, call_index, old_final, observed_at)) = calls.get(&call_id) {
                let existing = &mut turns[old_owner].calls[call_index];
                let first_at = existing.started_at.min(call.started_at);
                let last_at = existing.ended_at.max(call.ended_at);
                let conflicting = existing
                    .accounting
                    .as_ref()
                    .is_some_and(|a| a.conflicting_observations)
                    || (old_final
                        && final_observation
                        && usage_signature(existing) != usage_signature(&call));
                if (final_observation, ended_at) >= (old_final, observed_at) {
                    *existing = call;
                    calls.insert(
                        call_id.clone(),
                        (old_owner, call_index, final_observation, ended_at),
                    );
                }
                if let Some(accounting) = &mut existing.accounting {
                    accounting.conflicting_observations = conflicting;
                }
                existing.started_at = first_at;
                existing.ended_at = last_at;
                turns[old_owner].ended_at = turns[old_owner].ended_at.max(ended_at);
                if entry.is_completed_response() {
                    turns[old_owner].response = entry.assistant_text();
                }
                old_owner
            } else {
                let turn = &mut turns[owner];
                if turn.started_at == DateTime::<Utc>::UNIX_EPOCH {
                    turn.started_at = started_at;
                }
                turn.ended_at = turn.ended_at.max(ended_at);
                if entry.is_completed_response() {
                    turn.response = entry.assistant_text();
                }
                calls.insert(
                    call_id.clone(),
                    (owner, turn.calls.len(), final_observation, ended_at),
                );
                turn.calls.push(call);
                owner
            };
        if entry.is_completed_response() && fallback_turns.remove(&call_owner) {
            let identity = format!("assistant:{call_id}");
            turns[call_owner].uuid = identity.clone();
            owners.insert(identity, call_owner);
        }
    }
    if let Some(uuid) = &entry.uuid {
        owners.insert(uuid.clone(), owner);
    }
}

fn usage_signature(call: &LlmCall) -> (u64, u64, u64, u64) {
    (
        call.input_tokens,
        call.output_tokens,
        call.cache_read_tokens,
        call.cache_creation_tokens,
    )
}

fn ancestry_related(assistant: &Entry, leaf_uuid: &str, parents: &HashMap<String, String>) -> bool {
    assistant.uuid.as_deref().is_some_and(|assistant_uuid| {
        ancestry_reaches(leaf_uuid, assistant_uuid, parents)
            || ancestry_reaches(assistant_uuid, leaf_uuid, parents)
    }) || assistant
        .parent_uuid
        .as_deref()
        .is_some_and(|parent| ancestry_reaches(parent, leaf_uuid, parents))
}

fn ancestry_reaches(start: &str, target: &str, parents: &HashMap<String, String>) -> bool {
    let mut current = start;
    let mut seen = HashSet::new();
    loop {
        if current == target {
            return true;
        }
        if !seen.insert(current.to_string()) {
            return false;
        }
        let Some(parent) = parents.get(current) else {
            return false;
        };
        current = parent;
    }
}

fn parse_entry(line: &str) -> Option<Entry> {
    let line = line.trim();
    (!line.is_empty())
        .then(|| serde_json::from_str::<Entry>(line).ok())
        .flatten()
}

#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: String,
    uuid: Option<String>,
    #[serde(rename = "parentUuid")]
    parent_uuid: Option<String>,
    #[serde(rename = "leafUuid")]
    leaf_uuid: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    timestamp: Option<DateTime<Utc>>,
    message: Option<Message>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(rename = "lastPrompt")]
    last_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    id: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    content: Option<serde_json::Value>,
    usage: Option<Usage>,
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    cache_creation: Option<CacheCreation>,
    speed: Option<String>,
    service_tier: Option<String>,
    inference_geo: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CacheCreation {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

impl Entry {
    fn call_identity(&self) -> Option<String> {
        let message = self.message.as_ref()?;
        match (self.request_id.as_deref(), message.id.as_deref()) {
            (Some(request), Some(message)) => Some(format!(
                "provider:{}",
                uuid::Uuid::new_v5(
                    &uuid::Uuid::NAMESPACE_OID,
                    format!("{request}\0{message}").as_bytes()
                )
            )),
            (_, Some(message)) => Some(format!("message:{message}")),
            _ => self.uuid.clone(),
        }
    }

    fn stable_identity(&self, line_index: usize) -> String {
        self.uuid
            .as_ref()
            .or(self.leaf_uuid.as_ref())
            .cloned()
            .unwrap_or_else(|| {
                format!(
                    "{}:record:{}",
                    self.session_id.as_deref().unwrap_or("unknown-session"),
                    line_index
                )
            })
    }

    fn user_prompt(&self) -> Option<String> {
        if self.kind == "last-prompt" {
            return nonempty(self.last_prompt.as_deref()?);
        }
        if self.kind != "user" {
            return None;
        }
        let content = self.message.as_ref()?.content.as_ref()?;
        let text = if let Some(text) = content.as_str() {
            text.to_string()
        } else {
            let parts: Vec<&str> = content
                .as_array()?
                .iter()
                .filter_map(|block| {
                    (block.get("type")?.as_str()? == "text")
                        .then(|| block.get("text")?.as_str())
                        .flatten()
                })
                .filter(|text| !text.trim().is_empty())
                .collect();
            if parts.is_empty() {
                return None;
            }
            parts.join("\n")
        };
        // A fork's prompt is its directive; the boilerplate before it is the same for every fork.
        let text = text
            .split_once(FORK_BOILERPLATE_END)
            .map_or(text.as_str(), |(_, directive)| directive)
            .trim();
        let internal = text.starts_with("<command-") || text.starts_with("<local-command-");
        (!text.is_empty() && !internal).then(|| text.to_string())
    }

    fn llm_call(&self, started_at: DateTime<Utc>, ended_at: DateTime<Utc>) -> Option<LlmCall> {
        let message = self.message.as_ref()?;
        let usage = message.usage.as_ref()?;
        if message
            .model
            .as_deref()
            .is_some_and(|model| model.starts_with('<'))
        {
            return None;
        }
        let (cache_creation_5m_tokens, cache_creation_1h_tokens) = cache_ttl_within(
            usage.cache_creation_input_tokens,
            usage
                .cache_creation
                .as_ref()
                .and_then(|c| c.ephemeral_5m_input_tokens),
            usage
                .cache_creation
                .as_ref()
                .and_then(|c| c.ephemeral_1h_input_tokens),
        );
        Some(LlmCall {
            uuid: self.call_identity()?,
            provider: message
                .provider
                .clone()
                .filter(|p| !p.trim().is_empty())
                .unwrap_or_else(|| "unknown".into()),
            model: message.model.clone().unwrap_or_else(|| "unknown".into()),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
            cache_creation_tokens: usage.cache_creation_input_tokens,
            accounting: Some(nasiko_types::CodingAgentCallAccounting {
                version: 2,
                request_id: self.request_id.clone(),
                message_id: message.id.clone(),
                cache_creation_5m_tokens,
                cache_creation_1h_tokens,
                speed: usage.speed.clone(),
                service_tier: usage.service_tier.clone(),
                inference_geo: usage.inference_geo.clone(),
                conflicting_observations: false,
            }),
            started_at,
            ended_at,
        })
    }

    fn is_completed_response(&self) -> bool {
        if self.is_sidechain {
            return false;
        }
        let Some(message) = &self.message else {
            return false;
        };
        match message.stop_reason.as_deref() {
            Some("tool_use") | Some("pause_turn") => false,
            Some(_) => true,
            None => !message.content.as_ref().is_some_and(|content| {
                content
                    .as_array()
                    .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_use"))
            }),
        }
    }

    fn assistant_text(&self) -> Option<String> {
        let content = self.message.as_ref()?.content.as_ref()?;
        if let Some(text) = content.as_str() {
            return nonempty(text);
        }
        let parts: Vec<&str> = content
            .as_array()?
            .iter()
            .filter_map(|block| {
                (block.get("type")?.as_str()? == "text")
                    .then(|| block.get("text")?.as_str())
                    .flatten()
            })
            .filter(|text| !text.trim().is_empty())
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    fn tool_uses(&self, at: DateTime<Utc>) -> Vec<ToolCall> {
        if self.kind != "assistant" {
            return Vec::new();
        }
        let model_call_id = self.call_identity();
        self.message
            .as_ref()
            .and_then(|message| message.content.as_ref())
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "tool_use")
            .filter_map(|block| {
                Some(ToolCall {
                    id: block.get("id")?.as_str()?.to_string(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string(),
                    kind: "tool".into(),
                    model_call_id: model_call_id.clone(),
                    status: CodingAgentToolCallStatus::Running,
                    arguments: block.get("input").cloned(),
                    output: None,
                    raw: None,
                    error: None,
                    started_at: Some(at),
                    ended_at: None,
                    duration_ms: None,
                    association: CodingAgentToolAssociation::Exact,
                    timestamp_quality: CodingAgentTimestampQuality::Inferred,
                })
            })
            .collect()
    }

    fn tool_results(&self) -> Vec<ToolResult> {
        if self.kind != "user" {
            return Vec::new();
        }
        self.message
            .as_ref()
            .and_then(|message| message.content.as_ref())
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "tool_result")
            .filter_map(|block| {
                let is_error = block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let output = block.get("content").cloned();
                Some(ToolResult {
                    id: block.get("tool_use_id")?.as_str()?.to_string(),
                    error: is_error.then(|| value_text(output.as_ref())),
                    output,
                    is_error,
                })
            })
            .collect()
    }
}

struct ToolResult {
    id: String,
    output: Option<Value>,
    error: Option<String>,
    is_error: bool,
}

fn value_text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| value.and_then(|value| serde_json::to_string(value).ok()))
        .unwrap_or_else(|| "tool failed".into())
}

fn nonempty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn past_sessions_are_each_projects_transcripts_as_hook_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path();
        for path in [
            "p1/a.jsonl",
            "p1/notes.txt",
            "p2/b.jsonl",
            "p1/a/subagents/agent-c.jsonl",
            "p1/a/subagents/agent-c.meta.json",
            "p1/a/subagents/notes.jsonl",
        ] {
            let path = projects.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "{}\n").unwrap();
        }
        let sessions = past_sessions_in(projects);
        let payloads: Vec<Value> = sessions
            .iter()
            .map(|session| match &session.source {
                SessionSource::Payload(raw) => serde_json::from_str(raw).unwrap(),
                SessionSource::Load(_) => panic!("Claude sessions replay the hook payload"),
            })
            .collect();
        let mut keys: Vec<(String, &str)> = sessions
            .iter()
            .zip(&payloads)
            .map(|(session, p)| {
                let relative = Path::new(&session.key).strip_prefix(projects).unwrap();
                (
                    relative.to_string_lossy().into_owned(),
                    p["session_id"].as_str().unwrap(),
                )
            })
            .collect();
        keys.sort();
        // Each session's transcript, and each subagent's transcript as a session of its own.
        assert_eq!(
            keys,
            [
                ("p1/a.jsonl".to_string(), "a"),
                ("p1/a/subagents/agent-c.jsonl".to_string(), "a"),
                ("p2/b.jsonl".to_string(), "b"),
            ]
        );
        for (session, p) in sessions.iter().zip(&payloads) {
            let parsed: HookPayload = serde_json::from_value(p.clone()).unwrap();
            let read = parsed
                .agent_transcript_path
                .as_ref()
                .unwrap_or(&parsed.transcript_path);
            assert!(read.is_file());
            assert_eq!(session.key, read.to_string_lossy());
            assert!(session.last_active > DateTime::UNIX_EPOCH);
            if parsed.agent_transcript_path.is_some() {
                assert_eq!(parsed.hook_event_name.as_deref(), Some(SUBAGENT_STOP));
                assert_eq!(parsed.transcript_path, projects.join("p1/a.jsonl"));
            }
        }
        assert!(past_sessions_in(&projects.join("missing")).is_empty());
    }

    /// A session with one subagent that spawned another, as Claude Code writes them: the subagents'
    /// records are all sidechain records.
    fn session_with_subagents(projects: &Path) -> PathBuf {
        let main = projects.join("p/s.jsonl");
        let subagents = projects.join("p/s/subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        std::fs::write(
            &main,
            concat!(
                "{\"type\":\"user\",\"uuid\":\"u\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"explore\"}}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a\",\"parentUuid\":\"u\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:09Z\",\"message\":{\"model\":\"m\",\"stop_reason\":\"end_turn\",\"content\":\"done\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n",
            ),
        )
        .unwrap();
        for (agent, prompt) in [("a1", "look around"), ("a2", "look deeper")] {
            std::fs::write(
                subagents.join(format!("agent-{agent}.jsonl")),
                format!(
                    concat!(
                        "{{\"type\":\"user\",\"uuid\":\"{a}-u\",\"isSidechain\":true,\"agentId\":\"{a}\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{{\"role\":\"user\",\"content\":\"{p}\"}}}}\n",
                        "{{\"type\":\"assistant\",\"uuid\":\"{a}-t\",\"parentUuid\":\"{a}-u\",\"isSidechain\":true,\"agentId\":\"{a}\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:02Z\",\"message\":{{\"model\":\"m\",\"stop_reason\":\"tool_use\",\"content\":[{{\"type\":\"tool_use\",\"id\":\"{a}-tool\",\"name\":\"Read\",\"input\":{{}}}}],\"usage\":{{\"input_tokens\":5,\"output_tokens\":1}}}}}}\n",
                        "{{\"type\":\"user\",\"uuid\":\"{a}-r\",\"parentUuid\":\"{a}-t\",\"isSidechain\":true,\"agentId\":\"{a}\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:03Z\",\"message\":{{\"content\":[{{\"type\":\"tool_result\",\"tool_use_id\":\"{a}-tool\",\"content\":\"ok\"}}]}}}}\n",
                        "{{\"type\":\"assistant\",\"uuid\":\"{a}-f\",\"parentUuid\":\"{a}-r\",\"isSidechain\":true,\"agentId\":\"{a}\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:04Z\",\"message\":{{\"model\":\"m\",\"stop_reason\":\"end_turn\",\"content\":\"found it\",\"usage\":{{\"input_tokens\":7,\"output_tokens\":3}}}}}}\n",
                    ),
                    a = agent,
                    p = prompt,
                ),
            )
            .unwrap();
        }
        std::fs::write(
            subagents.join("agent-a1.meta.json"),
            r#"{"agentType":"Explore","description":"Look around","toolUseId":"toolu_1","spawnDepth":1}"#,
        )
        .unwrap();
        std::fs::write(
            subagents.join("agent-a2.meta.json"),
            r#"{"agentType":"Explore","description":"Look deeper","toolUseId":"toolu_2","parentAgentId":"a1","spawnDepth":2}"#,
        )
        .unwrap();
        main
    }

    #[test]
    fn a_subagent_transcript_is_its_own_session_linked_to_the_one_that_spawned_it() {
        let dir = tempfile::tempdir().unwrap();
        session_with_subagents(dir.path());
        let mut snapshots: Vec<SessionSnapshot> = past_sessions_in(dir.path())
            .into_iter()
            .map(|session| match session.source {
                SessionSource::Payload(raw) => snapshot_once(&raw).unwrap(),
                SessionSource::Load(_) => panic!("Claude sessions replay the hook payload"),
            })
            .collect();
        snapshots.sort_by(|a, b| a.session_id.cmp(&b.session_id));

        let ids: Vec<&str> = snapshots.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, ["s", "s:agent-a1", "s:agent-a2"]);
        let (session, first, nested) = (&snapshots[0], &snapshots[1], &snapshots[2]);
        assert!(session.parent.is_none());
        // The session's own turn holds only its own call: the subagents' usage is theirs.
        assert_eq!(session.turns[0].calls.len(), 1);

        let parent = first.parent.as_ref().unwrap();
        assert_eq!(parent.source_id, "s");
        assert_eq!(parent.tool_call_id.as_deref(), Some("toolu_1"));
        assert_eq!(parent.agent_type.as_deref(), Some("Explore"));
        assert_eq!(first.title.as_deref(), Some("Look around"));
        // Sidechain records are the subagent's conversation: its turn is complete and answered.
        let turn = &first.turns[0];
        assert_eq!(turn.uuid, "a1-u");
        assert_eq!(turn.prompt, "look around");
        assert_eq!(turn.response.as_deref(), Some("found it"));
        assert_eq!(turn.calls.len(), 2);
        assert_eq!(
            turn.tool_calls[0].status,
            CodingAgentToolCallStatus::Succeeded
        );

        let parent = nested.parent.as_ref().unwrap();
        assert_eq!(parent.source_id, "s:agent-a1");
        assert_eq!(parent.tool_call_id.as_deref(), Some("toolu_2"));

        for snapshot in &snapshots {
            let event =
                super::super::super::report::canonical_event_for_test(snapshot, &snapshot.turns[0]);
            assert_eq!(event.validate(), Ok(()));
            assert_eq!(event.session.parent, snapshot.parent);
        }
    }

    #[test]
    fn a_forked_subagent_counts_from_its_own_directive_not_the_parents_inherited_call() {
        let input = r#"
{"type":"fork-context-ref","parentSessionId":"s","parentLastUuid":"p9","contextLength":3}
{"type":"assistant","uuid":"inherited","isSidechain":true,"requestId":"parent-request","timestamp":"2026-01-01T00:00:00Z","message":{"id":"parent-message","model":"m","stop_reason":"tool_use","content":[{"type":"tool_use","id":"toolu_fork","name":"Agent","input":{}}],"usage":{"input_tokens":900,"output_tokens":90}}}
{"type":"user","uuid":"u","parentUuid":"inherited","isSidechain":true,"timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_fork","content":[{"type":"text","text":"Fork started"}]},{"type":"text","text":"<fork-boilerplate>\nYou are a worker fork.\n</fork-boilerplate>\n\nYour directive: check the limits"}]}}
{"type":"assistant","uuid":"a","parentUuid":"u","isSidechain":true,"requestId":"own","timestamp":"2026-01-01T00:00:02Z","message":{"id":"own-message","model":"m","stop_reason":"end_turn","content":"limits checked","usage":{"input_tokens":5,"output_tokens":2}}}
"#;
        let turns = subagent_turns_from_lines(input);

        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].prompt, "Your directive: check the limits");
        assert_eq!(turns[0].response.as_deref(), Some("limits checked"));
        assert_eq!(turns[0].calls.len(), 1);
        assert_eq!(turns[0].calls[0].input_tokens, 5);
    }

    #[test]
    fn sidechain_records_in_a_sessions_own_transcript_never_answer_its_turn() {
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"hi"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":true,"timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","stop_reason":"end_turn","content":"sidechain","usage":{"input_tokens":1,"output_tokens":2}}}
"#;
        assert_eq!(turns_from_lines(input)[0].response, None);
        assert_eq!(
            subagent_turns_from_lines(input)[0].response.as_deref(),
            Some("sidechain")
        );
    }

    #[test]
    fn stop_reports_the_session_and_every_subagent_and_subagent_stop_only_subagents() {
        let dir = tempfile::tempdir().unwrap();
        let main = session_with_subagents(dir.path());
        let ids = |raw: Value| -> Vec<String> {
            let mut ids: Vec<String> = hook_snapshots(&raw.to_string(), Instant::now())
                .unwrap()
                .into_iter()
                .map(|snapshot| snapshot.unwrap().session_id)
                .collect();
            ids.sort();
            ids
        };

        let stop = json!({"session_id": "s", "transcript_path": main, "hook_event_name": "Stop"});
        assert_eq!(ids(stop), ["s", "s:agent-a1", "s:agent-a2"]);

        // The session's turn is still running at SubagentStop: only subagents are read.
        let any_subagent =
            json!({"session_id": "s", "transcript_path": main, "hook_event_name": SUBAGENT_STOP});
        assert_eq!(ids(any_subagent), ["s:agent-a1", "s:agent-a2"]);

        let one_subagent = json!({
            "session_id": "s",
            "transcript_path": main,
            "hook_event_name": SUBAGENT_STOP,
            "agent_id": "a2",
            "agent_transcript_path": dir.path().join("p/s/subagents/agent-a2.jsonl"),
        });
        let snapshots = hook_snapshots(&one_subagent.to_string(), Instant::now()).unwrap();
        assert_eq!(snapshots.len(), 1);
        let snapshot = snapshots.into_iter().next().unwrap().unwrap();
        assert_eq!(snapshot.session_id, "s:agent-a2");
        assert_eq!(snapshot.parent.unwrap().source_id, "s:agent-a1");
    }

    #[test]
    fn a_session_without_subagents_reports_only_itself() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "").unwrap();
        let raw = json!({"session_id": "s", "transcript_path": path, "hook_event_name": "Stop"});
        let snapshots = hook_snapshots(&raw.to_string(), Instant::now()).unwrap();
        assert_eq!(snapshots.len(), 1);
    }

    #[test]
    fn an_unreadable_subagent_metadata_file_is_an_error_for_that_subagent_only() {
        let dir = tempfile::tempdir().unwrap();
        let main = session_with_subagents(dir.path());
        std::fs::write(dir.path().join("p/s/subagents/agent-a2.meta.json"), "{oops").unwrap();
        let raw = json!({"session_id": "s", "transcript_path": main, "hook_event_name": "Stop"});
        let snapshots = hook_snapshots(&raw.to_string(), Instant::now()).unwrap();
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots.iter().filter(|s| s.is_err()).count(), 1);
    }

    #[test]
    fn a_past_session_is_read_once_without_waiting_for_an_unfinished_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        // A prompt with no answer: a live hook would wait for one.
        std::fs::write(
            &path,
            r#"{"type":"user","uuid":"u1","sessionId":"s","timestamp":"2026-10-01T10:00:00Z","message":{"role":"user","content":"Hi"}}"#,
        )
        .unwrap();
        let raw = serde_json::json!({"session_id": "s", "transcript_path": path}).to_string();

        let started = Instant::now();
        let snapshot = snapshot_once(&raw).unwrap();

        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(snapshot.session_id, "s");
        assert!(snapshot.turns.iter().all(|turn| turn.response.is_none()));
    }

    const USAGE: &str = r#""usage":{"input_tokens":1,"output_tokens":2}"#;

    #[test]
    fn capture_uses_one_final_observation_not_independent_maxima() {
        let user = json!({"type":"user","uuid":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"hello"}});
        let partial = json!({"type":"assistant","uuid":"partial","parentUuid":"user","requestId":"request", "timestamp":"2026-01-01T00:00:03Z", "message":{"id":"message","model":"claude-test","usage":{"input_tokens":90,"output_tokens":1}}});
        let final_record = json!({"type":"assistant","uuid":"final","parentUuid":"user","requestId":"request", "timestamp":"2026-01-01T00:00:02Z", "message":{"id":"message","model":"claude-test","content":"done","stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":7,"cache_creation":{"ephemeral_5m_input_tokens":2,"ephemeral_1h_input_tokens":5}}}});
        for content in [
            format!("{user}\n{partial}\n{final_record}"),
            format!("{user}\n{final_record}\n{partial}"),
        ] {
            let turns = turns_from_lines(&content);
            let calls: Vec<_> = turns.iter().flat_map(|t| &t.calls).collect();
            assert_eq!(calls.len(), 1);
            assert_eq!((calls[0].input_tokens, calls[0].output_tokens), (10, 5));
            assert_eq!(calls[0].provider, "unknown");
            assert_eq!(
                calls[0]
                    .accounting
                    .as_ref()
                    .unwrap()
                    .cache_creation_1h_tokens,
                Some(5)
            );
        }
        let mut retry = final_record.clone();
        retry["uuid"] = "retry".into();
        retry["requestId"] = "different-request".into();
        let turns = turns_from_lines(&format!("{user}\n{final_record}\n{retry}"));
        assert_eq!(turns.iter().map(|t| t.calls.len()).sum::<usize>(), 2);
    }

    #[test]
    fn title_uses_latest_generated_record_for_matching_session() {
        let records = concat!(
            "{\"type\":\"ai-title\",\"aiTitle\":\"Old\",\"sessionId\":\"s\"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Debug integration\",\"sessionId\":\"s\"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Other\",\"sessionId\":\"other\"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":false,\"sessionId\":\"s\"}\n",
            "{incomplete",
        );
        assert_eq!(
            session_title(records, "s").as_deref(),
            Some("Debug integration")
        );
        assert!(session_title(records, "missing").is_none());
        assert!(session_title("", "s").is_none());
    }

    #[test]
    fn custom_title_wins_until_cleared_without_reviving_old_titles() {
        let records = concat!(
            "{\"type\":\"custom-title\",\"customTitle\":\"  Custom name  \",\"sessionId\":\"s\"}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Generated\",\"sessionId\":\"s\"}\n",
        );
        assert_eq!(session_title(records, "s").as_deref(), Some("Custom name"));
        let cleared = format!(
            "{records}{}\n",
            r#"{"type":"custom-title","customTitle":"  ","sessionId":"s"}"#
        );
        assert_eq!(session_title(&cleared, "s").as_deref(), Some("Generated"));
        let all_cleared = format!(
            "{cleared}{}\n",
            r#"{"type":"ai-title","aiTitle":"","sessionId":"s"}"#
        );
        assert!(session_title(&all_cleared, "s").is_none());
    }

    #[test]
    fn title_records_do_not_break_last_prompt_adjacency() {
        let records = concat!(
            "{\"type\":\"user\",\"uuid\":\"u\",\"message\":{\"content\":\"hello\"}}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Generated\",\"sessionId\":\"s\"}\n",
            "{\"type\":\"last-prompt\",\"lastPrompt\":\"hello\",\"sessionId\":\"s\"}\n",
        );
        let turns = turns_from_lines(records);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].uuid, "u");
    }

    #[test]
    fn snapshot_reads_title_and_turns_from_same_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let records = concat!(
            "{\"type\":\"user\",\"uuid\":\"u\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"hello\"}}\n",
            "{\"type\":\"assistant\",\"uuid\":\"a\",\"parentUuid\":\"u\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"message\":{\"model\":\"test\",\"content\":\"done\",\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Debug integration\",\"sessionId\":\"s\"}\n",
        );
        std::fs::write(&path, records).unwrap();
        let payload = json!({"session_id": "s", "transcript_path": path}).to_string();
        let snapshot = snapshot(&payload, Instant::now()).unwrap();
        assert_eq!(snapshot.title.as_deref(), Some("Debug integration"));
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(snapshot.turns[0].uuid, "u");
        assert_eq!(snapshot.turns[0].response.as_deref(), Some("done"));
    }

    #[test]
    fn v4_hook_registration_is_idempotent_and_preserves_foreign_hooks() {
        assert_eq!(INSTALL_VERSION, 4);
        let foreign = json!({
            "matcher": "*",
            "hooks": [{"type": "command", "command": "echo other"}]
        });
        let nasiko = json!({
            "matcher": "*",
            "hooks": [{"type": "command", "command": "bash '/tmp/nasiko-session-report.sh'"}]
        });
        let settings = json!({"hooks": {"Stop": [foreign, nasiko]}});
        let groups = hook_groups_without_nasiko(&settings, "Stop");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], "echo other");
    }

    #[test]
    fn registration_hooks_stop_and_subagent_stop_and_removal_clears_both() {
        let dir = tempfile::tempdir().unwrap();
        let settings = dir.path().join("settings.json");
        let script = dir.path().join("hooks").join(launcher::SCRIPT_NAME);
        std::fs::write(
            &settings,
            r#"{"hooks":{"SubagentStop":[{"matcher":"*","hooks":[{"type":"command","command":"echo other"}]}]}}"#,
        )
        .unwrap();

        register_settings(&settings, &script).unwrap();
        register_settings(&settings, &script).unwrap();

        let registered = read_settings(&settings).unwrap();
        let command = hook_command(&script);
        for event in HOOK_EVENTS {
            assert!(has_current_hook(&registered, event, &command), "{event}");
            let ours = registered["hooks"][event]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|group| group["hooks"].as_array().unwrap())
                .filter(|hook| is_nasiko_hook(hook))
                .count();
            assert_eq!(ours, 1, "{event} is registered once");
        }

        deregister_settings(&settings).unwrap();
        let removed = read_settings(&settings).unwrap();
        assert!(removed["hooks"].get("Stop").is_none());
        assert_eq!(
            removed["hooks"]["SubagentStop"][0]["hooks"][0]["command"],
            "echo other"
        );
    }

    #[test]
    fn status_registration_requires_the_exact_current_script() {
        let current = hook_command(Path::new(
            "/home/current/.claude/hooks/nasiko-session-report.sh",
        ));
        let right = json!({"type": "command", "command": current});
        let wrong = json!({
            "type": "command",
            "command": "bash '/home/old/.claude/hooks/nasiko-session-report.sh'"
        });
        assert!(is_current_hook(&right, right["command"].as_str().unwrap()));
        assert!(!is_current_hook(&wrong, right["command"].as_str().unwrap()));
    }

    #[test]
    fn preserves_consecutive_identical_prompts_with_distinct_uuids() {
        let input = format!(
            r#"{{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{{"content":"same"}}}}
{{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{{"model":"m",{USAGE}}}}}
{{"type":"user","uuid":"u2","timestamp":"2026-01-01T00:00:02Z","message":{{"content":"same"}}}}
{{"type":"assistant","uuid":"a2","parentUuid":"u2","timestamp":"2026-01-01T00:00:03Z","message":{{"model":"m",{USAGE}}}}}"#
        );
        let turns = turns_from_lines(&input);
        assert_eq!(turns.len(), 2);
        assert_eq!(
            (turns[0].uuid.as_str(), turns[1].uuid.as_str()),
            ("u1", "u2")
        );
    }

    #[test]
    fn deduplicates_assistant_records_by_uuid() {
        let input = format!(
            r#"{{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{{"content":"hi"}}}}
{{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{{"model":"m",{USAGE}}}}}
{{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{{"model":"m",{USAGE}}}}}"#
        );
        assert_eq!(turns_from_lines(&input)[0].calls.len(), 1);
    }

    #[test]
    fn last_prompt_has_a_stable_nonempty_identity() {
        let input = r#"{"type":"last-prompt","lastPrompt":"hi","sessionId":"s1"}"#;
        let first = turns_from_lines(input);
        let second = turns_from_lines(input);
        assert!(!first[0].uuid.is_empty());
        assert_eq!(first[0].uuid, second[0].uuid);
    }

    #[test]
    fn completed_uuidless_last_prompts_use_distinct_assistant_identities() {
        let input = r#"
{"type":"last-prompt","lastPrompt":"same","sessionId":"s1"}
{"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","stop_reason":"end_turn","content":"one","usage":{"input_tokens":1,"output_tokens":2}}}
{"type":"last-prompt","lastPrompt":"same","sessionId":"s1"}
{"type":"assistant","uuid":"a2","timestamp":"2026-01-01T00:00:02Z","message":{"model":"m","stop_reason":"end_turn","content":"two","usage":{"input_tokens":1,"output_tokens":2}}}
"#;
        let turns = turns_from_lines(input);

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].uuid, "assistant:a1");
        assert_eq!(turns[1].uuid, "assistant:a2");
        assert_eq!(turns[0].prompt, turns[1].prompt);
    }

    #[test]
    fn leaf_uuid_correlates_last_prompt_without_text_deduplication() {
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"same"}}
{"type":"attachment","uuid":"leaf","parentUuid":"u1","timestamp":"2026-01-01T00:00:00.5Z"}
{"type":"last-prompt","lastPrompt":"same","leafUuid":"leaf","sessionId":"s1"}
{"type":"assistant","uuid":"a1","parentUuid":"leaf","timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","content":"done","usage":{"input_tokens":1,"output_tokens":2}}}
"#;
        let turns = turns_from_lines(input);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].calls.len(), 1);
    }

    #[test]
    fn direct_adjacency_correlates_metadata_poor_last_prompt() {
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"hi"}}
{"type":"last-prompt","lastPrompt":"hi","sessionId":"s1"}
{"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","content":"done","usage":{"input_tokens":1,"output_tokens":2}}}
"#;
        let turns = turns_from_lines(input);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].uuid, "u1");
        assert_eq!(turns[0].calls.len(), 1);
    }

    #[test]
    fn assistant_before_last_prompt_is_correlated_without_stealing_prior_turn() {
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"first"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","stop_reason":"end_turn","content":"first done","usage":{"input_tokens":1,"output_tokens":2}}}
{"type":"assistant","uuid":"a2","parentUuid":"orphan-leaf","timestamp":"2026-01-01T00:00:02Z","message":{"model":"m","stop_reason":"end_turn","content":"second done","usage":{"input_tokens":3,"output_tokens":4}}}
{"type":"last-prompt","lastPrompt":"second","leafUuid":"a2","sessionId":"s1"}
"#;
        let turns = turns_from_lines(input);

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].uuid, "u1");
        assert_eq!(turns[0].calls.len(), 1);
        assert_eq!(turns[0].calls[0].uuid, "a1");
        assert_eq!(turns[1].uuid, "a2");
        assert_eq!(turns[1].calls.len(), 1);
        assert_eq!(turns[1].calls[0].uuid, "a2");
        assert_eq!(turns[1].response.as_deref(), Some("second done"));
    }

    #[test]
    fn captures_only_the_final_tool_loop_response() {
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"do it"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","stop_reason":"tool_use","content":[{"type":"text","text":"I will check"},{"type":"tool_use"}],"usage":{"input_tokens":1,"output_tokens":2}}}
{"type":"user","uuid":"t1","parentUuid":"a1","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","content":"ok"}]}}
{"type":"assistant","uuid":"a2","parentUuid":"t1","timestamp":"2026-01-01T00:00:03Z","message":{"model":"m","stop_reason":"end_turn","content":[{"type":"text","text":"Final answer"}],"usage":{"input_tokens":3,"output_tokens":4}}}
"#;
        let turn = &turns_from_lines(input)[0];
        assert_eq!(turn.calls.len(), 2);
        assert_eq!(turn.response.as_deref(), Some("Final answer"));
    }

    #[test]
    fn reads_structured_user_text_and_ignores_tool_only_arrays() {
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"text","text":"part one"},{"type":"text","text":"part two"}]}}
{"type":"user","uuid":"t1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","content":"ok"}]}}
"#;
        let turns = turns_from_lines(input);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].prompt, "part one\npart two");
    }

    #[test]
    fn upgrades_an_incomplete_assistant_copy_to_complete() {
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"hi"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","content":"partial"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:02Z","message":{"model":"m","stop_reason":"end_turn","content":"complete","usage":{"input_tokens":1,"output_tokens":2}}}
"#;
        let turn = &turns_from_lines(input)[0];
        assert_eq!(turn.calls.len(), 1);
        assert_eq!(turn.response.as_deref(), Some("complete"));
    }

    #[test]
    fn ignores_local_command_control_records() {
        let input = r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"<local-command-stdout>ok</local-command-stdout>"}}"#;
        assert!(turns_from_lines(input).is_empty());
    }

    #[test]
    fn joins_parallel_tool_uses_and_polymorphic_results_by_native_id() {
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"inspect"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","stop_reason":"tool_use","content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"redacted.txt"}},{"type":"tool_use","id":"tool-2","name":"Shell","input":{"command":"redacted"}}],"usage":{"input_tokens":1,"output_tokens":2}}}
{"type":"user","uuid":"r1","parentUuid":"a1","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"tool-2","content":[{"type":"text","text":"denied"}],"is_error":true},{"type":"tool_result","tool_use_id":"tool-1","content":{"lines":3}}]}}
{"type":"assistant","uuid":"a2","parentUuid":"r1","timestamp":"2026-01-01T00:00:03Z","message":{"model":"m","stop_reason":"end_turn","content":"done","usage":{"input_tokens":1,"output_tokens":1}}}
"#;
        let turn = &turns_from_lines(input)[0];
        assert_eq!(turn.tool_calls.len(), 2);
        assert_eq!(turn.tool_calls[0].model_call_id.as_deref(), Some("a1"));
        assert_eq!(
            turn.tool_calls[0].status,
            CodingAgentToolCallStatus::Succeeded
        );
        assert_eq!(turn.tool_calls[0].output, Some(json!({"lines": 3})));
        assert_eq!(turn.tool_calls[1].status, CodingAgentToolCallStatus::Failed);
        assert_eq!(
            turn.tool_calls[1].timestamp_quality,
            CodingAgentTimestampQuality::Inferred
        );
    }

    #[test]
    fn duplicate_assistant_tool_use_preserves_the_terminal_result() {
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"inspect"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","stop_reason":"tool_use","content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"a"}}],"usage":{"input_tokens":1,"output_tokens":2}}}
{"type":"user","uuid":"r1","parentUuid":"a1","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"ok"}]}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:03Z","message":{"model":"m","stop_reason":"tool_use","content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"path":"a"}}],"usage":{"input_tokens":1,"output_tokens":2}}}
"#;
        let tool = &turns_from_lines(input)[0].tool_calls[0];
        assert_eq!(tool.status, CodingAgentToolCallStatus::Succeeded);
        assert_eq!(tool.output, Some(json!("ok")));
        assert!(tool.ended_at.is_some());
        assert_eq!(tool.duration_ms, Some(1000));
    }

    #[test]
    fn clamps_llm_start_when_async_records_are_appended_out_of_order() {
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"hi"}}
{"type":"attachment","uuid":"hook","parentUuid":"u1","timestamp":"2026-01-01T00:00:02Z"}
{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","stop_reason":"end_turn","content":"done","usage":{"input_tokens":1,"output_tokens":2}}}
"#;
        let turn = &turns_from_lines(input)[0];

        assert_eq!(turn.calls[0].started_at, turn.calls[0].ended_at);
    }

    #[test]
    fn cache_ttl_counts_above_the_aggregate_are_clamped_into_it() {
        // Seen in a real transcript: a TTL split that adds up to more than the cache write billed.
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"hi"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","requestId":"r","timestamp":"2026-01-01T00:00:01Z","message":{"id":"m","model":"claude-test","stop_reason":"end_turn","content":"done","usage":{"input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":80,"ephemeral_1h_input_tokens":70}}}}
"#;
        let call = &turns_from_lines(input)[0].calls[0];
        let accounting = call.accounting.as_ref().unwrap();

        assert_eq!(call.cache_creation_tokens, 100);
        assert_eq!(
            (
                accounting.cache_creation_5m_tokens,
                accounting.cache_creation_1h_tokens
            ),
            (Some(30), Some(70))
        );
    }

    #[test]
    fn a_tool_whose_llm_call_has_no_usage_is_linked_to_the_turn() {
        // The tool_use record carries no usage, so it makes no LLM call for the tool to point at.
        let input = r#"
{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"content":"inspect"}}
{"type":"assistant","uuid":"a1","parentUuid":"u1","timestamp":"2026-01-01T00:00:01Z","message":{"model":"m","stop_reason":"tool_use","content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{}}]}}
{"type":"user","uuid":"r1","parentUuid":"a1","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"ok"}]}}
{"type":"assistant","uuid":"a2","parentUuid":"r1","timestamp":"2026-01-01T00:00:03Z","message":{"model":"m","stop_reason":"end_turn","content":"done","usage":{"input_tokens":1,"output_tokens":1}}}
"#;
        let turn = &turns_from_lines(input)[0];
        let tool = &turn.tool_calls[0];

        assert_eq!(turn.calls.len(), 1);
        assert_eq!(tool.association, CodingAgentToolAssociation::Turn);
        assert_eq!(tool.model_call_id, None);
        let snapshot = SessionSnapshot {
            session_id: "s".into(),
            title: None,
            turns: vec![turn.clone()],
            parent: None,
        };
        let event = super::super::super::report::canonical_event_for_test(&snapshot, turn);
        assert_eq!(event.validate(), Ok(()));
    }
}
