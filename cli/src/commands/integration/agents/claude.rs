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
use super::super::model::{LlmCall, SessionSnapshot, ToolCall, Turn};

pub const INSTALL_VERSION: u32 = 3;
pub const SPEC: AgentSpec = AgentSpec {
    id: "claude",
    display_name: "Claude Code",
    binary: "claude",
    agent_name: "claude-code",
    support: Support::Instrumented,
};

const HOOK_EVENT: &str = "Stop";
const HOOK_TIMEOUT_SECS: u32 = 10;

#[derive(Debug, Deserialize)]
struct HookPayload {
    session_id: String,
    transcript_path: PathBuf,
}

pub fn config_path() -> PathBuf {
    catalog::home().join(".claude")
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
    settings["hooks"][HOOK_EVENT]
        .as_array()?
        .iter()
        .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
        .any(|hook| is_current_hook(hook, &expected_command))
        .then_some(version)
}

fn settings_path() -> PathBuf {
    config_path().join("settings.json")
}

fn register_settings(path: &Path, script: &Path) -> Result<()> {
    let mut settings = read_settings(path)?;
    let mut groups = hook_groups_without_nasiko(&settings);
    groups.push(json!({
        "matcher": "*",
        "hooks": [{
            "type": "command",
            "command": hook_command(script),
            "timeout": HOOK_TIMEOUT_SECS,
        }],
    }));
    set_hook_groups(&mut settings, groups);
    write_settings(path, &settings)
}

fn deregister_settings(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut settings = read_settings(path)?;
    let groups = hook_groups_without_nasiko(&settings);
    set_hook_groups(&mut settings, groups);
    write_settings(path, &settings)
}

fn hook_groups_without_nasiko(settings: &Value) -> Vec<Value> {
    let Some(groups) = settings["hooks"][HOOK_EVENT].as_array() else {
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

fn set_hook_groups(settings: &mut Value, groups: Vec<Value>) {
    if !settings.is_object() {
        *settings = json!({});
    }
    if !settings["hooks"].is_object() {
        settings["hooks"] = json!({});
    }
    if groups.is_empty() {
        settings["hooks"]
            .as_object_mut()
            .unwrap()
            .remove(HOOK_EVENT);
    } else {
        settings["hooks"][HOOK_EVENT] = Value::Array(groups);
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

pub fn snapshot(raw: &str, report_deadline: Instant) -> Result<SessionSnapshot> {
    let payload: HookPayload = serde_json::from_str(raw).with_context(|| {
        format!(
            "Claude hook payload is not expected JSON; got: {}",
            preview(raw)
        )
    })?;
    let wait_deadline = (Instant::now() + Duration::from_secs(3)).min(report_deadline);
    loop {
        let turns = parse_turns(&payload.transcript_path)?;
        if turns
            .last()
            .is_some_and(|turn| !turn.is_empty() && turn.response.is_some())
            || Instant::now() >= wait_deadline
        {
            return Ok(SessionSnapshot {
                session_id: payload.session_id,
                turns,
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn preview(raw: &str) -> String {
    raw.chars().take(200).collect()
}

pub fn parse_turns(path: &Path) -> Result<Vec<Turn>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read transcript {}", path.display()))?;
    Ok(turns_from_lines(&content))
}

fn turns_from_lines(content: &str) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut owners: HashMap<String, usize> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut calls: HashMap<String, (usize, usize)> = HashMap::new();
    let mut tools: HashMap<String, (usize, usize)> = HashMap::new();
    let mut fallback_turns: HashSet<usize> = HashSet::new();
    let mut previous_at: Option<DateTime<Utc>> = None;
    let mut adjacent_user: Option<usize> = None;
    let mut parents: HashMap<String, String> = HashMap::new();
    let mut pending_assistants: Vec<PendingAssistant> = Vec::new();

    for (line_index, line) in content.lines().enumerate() {
        let Some(entry) = parse_entry(line) else {
            continue;
        };
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

        if entry.kind == "user" {
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
    calls: &'a mut HashMap<String, (usize, usize)>,
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
        let call_owner = if let Some(&(old_owner, call_index)) = calls.get(&call_id) {
            turns[old_owner].calls[call_index] = call;
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
            calls.insert(call_id.clone(), (owner, turn.calls.len()));
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
    #[serde(rename = "lastPrompt")]
    last_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
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
}

impl Entry {
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
        let text = text.trim();
        let internal = text.starts_with("<command-") || text.starts_with("<local-command-");
        (!text.is_empty() && !internal).then(|| text.to_string())
    }

    fn llm_call(&self, started_at: DateTime<Utc>, ended_at: DateTime<Utc>) -> Option<LlmCall> {
        let message = self.message.as_ref()?;
        let usage = message.usage.as_ref()?;
        Some(LlmCall {
            uuid: self.uuid.clone()?,
            provider: "anthropic".to_string(),
            model: message.model.clone().unwrap_or_else(|| "unknown".into()),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
            cache_creation_tokens: usage.cache_creation_input_tokens,
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
        let model_call_id = self.uuid.clone();
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

    const USAGE: &str = r#""usage":{"input_tokens":1,"output_tokens":2}"#;

    #[test]
    fn v3_hook_registration_is_idempotent_and_preserves_foreign_hooks() {
        assert_eq!(INSTALL_VERSION, 3);
        let foreign = json!({
            "matcher": "*",
            "hooks": [{"type": "command", "command": "echo other"}]
        });
        let nasiko = json!({
            "matcher": "*",
            "hooks": [{"type": "command", "command": "bash '/tmp/nasiko-session-report.sh'"}]
        });
        let settings = json!({"hooks": {HOOK_EVENT: [foreign, nasiko]}});
        let groups = hook_groups_without_nasiko(&settings);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], "echo other");
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
}
