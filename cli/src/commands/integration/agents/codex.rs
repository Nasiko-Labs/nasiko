//! Codex user hooks and correlation of the two events that make up a turn.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use nasiko_types::{
    CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCallStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::catalog::{self, AgentSpec, Support};
use super::super::launcher;
use super::super::model::{LlmCall, SessionSnapshot, ToolCall, Turn};
use super::super::state;

pub const INSTALL_VERSION: u32 = 2;
pub const SPEC: AgentSpec = AgentSpec {
    id: "codex",
    display_name: "Codex",
    binary: "codex",
    agent_name: "codex",
    support: Support::Instrumented,
};

const HOOK_EVENTS: [&str; 4] = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"];
const HOOK_TIMEOUT_SECS: u32 = 10;

#[derive(Debug, Deserialize)]
struct HookPayload {
    session_id: String,
    turn_id: String,
    hook_event_name: String,
    model: Option<String>,
    prompt: Option<String>,
    last_assistant_message: Option<String>,
    transcript_path: Option<PathBuf>,
    tool_use_id: Option<String>,
    #[serde(alias = "name")]
    tool_name: Option<String>,
    #[serde(alias = "input")]
    tool_input: Option<Value>,
    #[serde(alias = "response")]
    tool_response: Option<Value>,
    error: Option<Value>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PendingTurn {
    prompt: Option<String>,
    response: Option<String>,
    model: Option<String>,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    usage_at_start: Option<TokenUsage>,
    usage_at_end: Option<TokenUsage>,
    #[serde(default)]
    tool_calls: Vec<ToolCallRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ToolCallRecord {
    id: String,
    name: String,
    arguments: Option<Value>,
    output: Option<Value>,
    error: Option<String>,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    status: CodingAgentToolCallStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct TokenUsage {
    input: u64,
    output: u64,
    cache_read: u64,
}

pub fn config_path() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| catalog::home().join(".codex"))
}

pub fn install() -> Result<(PathBuf, PathBuf)> {
    let config = config_path();
    let script = launcher::install(&config, SPEC.id, INSTALL_VERSION)?;
    let hooks = hooks_path(&config);
    if let Err(error) = register_hooks(&hooks, &script) {
        let _ = launcher::uninstall(&config);
        return Err(error);
    }
    Ok((script, hooks))
}

pub fn uninstall() -> Result<()> {
    let config = config_path();
    deregister_hooks(&hooks_path(&config))?;
    launcher::uninstall(&config)
}

pub fn installed_version() -> Option<u32> {
    let config = config_path();
    let version = launcher::installed_version(&config)?;
    let hooks = read_hooks(&hooks_path(&config)).ok()?;
    hooks_are_current(&hooks, &launcher::script_path(&config)).then_some(version)
}

pub fn snapshot(raw: &str) -> Result<SessionSnapshot> {
    snapshot_in(raw, &state::integrations_dir().join("events").join(SPEC.id))
}

fn hooks_path(config: &Path) -> PathBuf {
    config.join("hooks.json")
}

fn register_hooks(path: &Path, script: &Path) -> Result<()> {
    let mut document = read_hooks(path)?;
    for event in HOOK_EVENTS {
        let mut groups = groups_without_nasiko(&document, event);
        groups.push(json!({"hooks": [expected_hook(script)]}));
        set_groups(&mut document, event, groups);
    }
    write_json(path, &document)
}

fn deregister_hooks(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut document = read_hooks(path)?;
    for event in HOOK_EVENTS {
        let groups = groups_without_nasiko(&document, event);
        set_groups(&mut document, event, groups);
    }
    write_json(path, &document)
}

fn read_hooks(path: &Path) -> Result<Value> {
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
            "{} is not valid JSON; fix or move it, then retry",
            path.display()
        )
    })
}

fn groups_without_nasiko(document: &Value, event: &str) -> Vec<Value> {
    let Some(groups) = document["hooks"][event].as_array() else {
        return Vec::new();
    };
    groups
        .iter()
        .filter_map(|group| {
            let mut group = group.clone();
            let Some(handlers) = group["hooks"].as_array() else {
                return Some(group);
            };
            let handlers: Vec<_> = handlers
                .iter()
                .filter(|handler| !is_nasiko_hook(handler))
                .cloned()
                .collect();
            if handlers.is_empty() {
                None
            } else {
                group["hooks"] = Value::Array(handlers);
                Some(group)
            }
        })
        .collect()
}

fn set_groups(document: &mut Value, event: &str, groups: Vec<Value>) {
    if !document.is_object() {
        *document = json!({});
    }
    if !document["hooks"].is_object() {
        document["hooks"] = json!({});
    }
    if groups.is_empty() {
        document["hooks"].as_object_mut().unwrap().remove(event);
    } else {
        document["hooks"][event] = Value::Array(groups);
    }
}

fn is_nasiko_hook(handler: &Value) -> bool {
    handler["command"]
        .as_str()
        .is_some_and(|command| command.contains(launcher::SCRIPT_NAME))
}

fn hook_command(script: &Path) -> String {
    format!("bash {}", launcher::shell_quote(&script.to_string_lossy()))
}

fn expected_hook(script: &Path) -> Value {
    json!({
        "type": "command",
        "command": hook_command(script),
        "timeout": HOOK_TIMEOUT_SECS,
    })
}

fn hooks_are_current(document: &Value, script: &Path) -> bool {
    let expected = expected_hook(script);
    HOOK_EVENTS.iter().all(|event| {
        let Some(groups) = document["hooks"][event].as_array() else {
            return false;
        };
        let nasiko: Vec<_> = groups
            .iter()
            .filter_map(|group| group["hooks"].as_array())
            .flatten()
            .filter(|handler| is_nasiko_hook(handler))
            .collect();
        nasiko.len() == 1 && nasiko[0] == &expected
    })
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn snapshot_in(raw: &str, spool_dir: &Path) -> Result<SessionSnapshot> {
    let payload: HookPayload = serde_json::from_str(raw).with_context(|| {
        format!(
            "Codex hook payload is not expected JSON; got: {}",
            raw.chars().take(200).collect::<String>()
        )
    })?;
    validate_payload(&payload)?;
    let received_at = Utc::now();

    std::fs::create_dir_all(spool_dir)
        .with_context(|| format!("failed to create {}", spool_dir.display()))?;
    let path = spool_path(spool_dir, &payload.session_id, &payload.turn_id);
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;
    lock.lock()
        .with_context(|| format!("failed to lock {}", lock_path.display()))?;

    let mut pending = read_pending(&path);
    let usage = payload
        .transcript_path
        .as_deref()
        .and_then(parse_token_usage);
    let snapshot = apply_event(&mut pending, &payload, received_at, usage);
    // Keep complete turns available for retries. The shared delivery
    // watermarks, not this event spool, decide whether export is pending.
    atomic_write(&path, serde_json::to_vec(&pending)?.as_slice())?;

    Ok(snapshot.unwrap_or_else(|| SessionSnapshot {
        session_id: payload.session_id,
        turns: Vec::new(),
    }))
}

fn validate_payload(payload: &HookPayload) -> Result<()> {
    if payload.session_id.trim().is_empty() || payload.turn_id.trim().is_empty() {
        bail!("Codex hook payload has an empty session_id or turn_id");
    }
    if !HOOK_EVENTS.contains(&payload.hook_event_name.as_str()) {
        bail!("unsupported Codex hook event: {}", payload.hook_event_name);
    }
    Ok(())
}

fn apply_event(
    pending: &mut PendingTurn,
    payload: &HookPayload,
    received_at: DateTime<Utc>,
    usage: Option<TokenUsage>,
) -> Option<SessionSnapshot> {
    match payload.hook_event_name.as_str() {
        "UserPromptSubmit" => {
            pending.prompt = nonempty(payload.prompt.as_deref());
            pending.started_at = Some(received_at);
            pending.usage_at_start = usage;
        }
        "Stop" => {
            pending.response = nonempty(payload.last_assistant_message.as_deref());
            pending.ended_at = Some(received_at);
            pending.model = payload
                .model
                .as_deref()
                .and_then(|model| nonempty(Some(model)));
            pending.usage_at_end = usage;
            for tool in &mut pending.tool_calls {
                if matches!(
                    tool.status,
                    CodingAgentToolCallStatus::Pending | CodingAgentToolCallStatus::Running
                ) {
                    tool.status = CodingAgentToolCallStatus::Unknown;
                }
            }
        }
        "PreToolUse" => {
            let id = payload.tool_use_id.as_deref()?.trim();
            if id.is_empty() {
                return None;
            }
            if let Some(tool) = pending.tool_calls.iter_mut().find(|tool| tool.id == id) {
                tool.name = payload
                    .tool_name
                    .clone()
                    .unwrap_or_else(|| tool.name.clone());
                tool.arguments = payload
                    .tool_input
                    .clone()
                    .or_else(|| tool.arguments.clone());
                tool.started_at.get_or_insert(received_at);
                if matches!(
                    tool.status,
                    CodingAgentToolCallStatus::Pending
                        | CodingAgentToolCallStatus::Running
                        | CodingAgentToolCallStatus::Unknown
                ) {
                    tool.status = CodingAgentToolCallStatus::Running;
                }
            } else {
                pending.tool_calls.push(ToolCallRecord {
                    id: id.to_string(),
                    name: payload
                        .tool_name
                        .clone()
                        .unwrap_or_else(|| "unknown".into()),
                    arguments: payload.tool_input.clone(),
                    output: None,
                    error: None,
                    started_at: Some(received_at),
                    ended_at: None,
                    status: CodingAgentToolCallStatus::Running,
                });
            }
        }
        "PostToolUse" => {
            let id = payload.tool_use_id.as_deref()?.trim();
            if id.is_empty() {
                return None;
            }
            let (status, error) =
                tool_response_status(payload.tool_response.as_ref(), payload.error.as_ref());
            let tool = if let Some(tool) = pending.tool_calls.iter_mut().find(|tool| tool.id == id)
            {
                tool
            } else {
                pending.tool_calls.push(ToolCallRecord {
                    id: id.to_string(),
                    name: payload
                        .tool_name
                        .clone()
                        .unwrap_or_else(|| "unknown".into()),
                    arguments: payload.tool_input.clone(),
                    output: None,
                    error: None,
                    started_at: None,
                    ended_at: None,
                    status: CodingAgentToolCallStatus::Unknown,
                });
                pending.tool_calls.last_mut().expect("tool inserted")
            };
            tool.output = payload.tool_response.clone();
            tool.error = error;
            tool.ended_at = Some(received_at);
            tool.status = status;
        }
        _ => return None,
    }

    let (prompt, response, model, started_at, ended_at) = (
        pending.prompt.clone()?,
        pending.response.clone()?,
        pending.model.clone()?,
        pending.started_at?,
        pending.ended_at?,
    );
    if pending.tool_calls.iter().any(|tool| {
        matches!(
            tool.status,
            CodingAgentToolCallStatus::Pending | CodingAgentToolCallStatus::Running
        )
    }) {
        return None;
    }
    let usage = usage_delta(pending.usage_at_start, pending.usage_at_end);
    let call_id = format!("{}:{}:llm", payload.session_id, payload.turn_id);
    let tool_calls = pending
        .tool_calls
        .iter()
        .map(|tool| ToolCall {
            id: tool.id.clone(),
            name: tool.name.clone(),
            kind: "tool".into(),
            model_call_id: None,
            status: tool.status,
            arguments: tool.arguments.clone(),
            output: tool.output.clone(),
            raw: None,
            error: tool.error.clone(),
            started_at: tool.started_at,
            ended_at: tool.ended_at,
            duration_ms: tool
                .started_at
                .zip(tool.ended_at)
                .map(|(start, end)| (end - start).num_milliseconds().max(0) as u64),
            association: CodingAgentToolAssociation::Turn,
            timestamp_quality: CodingAgentTimestampQuality::Receipt,
        })
        .collect();
    Some(SessionSnapshot {
        session_id: payload.session_id.clone(),
        turns: vec![Turn {
            uuid: payload.turn_id.clone(),
            prompt,
            response: Some(response),
            started_at,
            ended_at,
            calls: vec![LlmCall {
                uuid: call_id,
                provider: provider_for(&model).to_string(),
                model,
                input_tokens: usage.input,
                output_tokens: usage.output,
                cache_read_tokens: usage.cache_read,
                cache_creation_tokens: 0,
                started_at,
                ended_at,
            }],
            tool_calls,
        }],
    })
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn tool_response_status(
    response: Option<&Value>,
    hook_error: Option<&Value>,
) -> (CodingAgentToolCallStatus, Option<String>) {
    if let Some(error) = hook_error {
        return (CodingAgentToolCallStatus::Failed, Some(value_text(error)));
    }
    let Some(object) = response.and_then(Value::as_object) else {
        return (CodingAgentToolCallStatus::Unknown, None);
    };
    let status = object
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let error = object
        .get("error_message")
        .or_else(|| object.get("error"))
        .filter(|value| !value.is_null())
        .map(value_text);
    if object.get("denied").and_then(Value::as_bool) == Some(true)
        || status.contains("denied")
        || status.contains("permission")
    {
        return (CodingAgentToolCallStatus::Denied, error);
    }
    if object.get("timed_out").and_then(Value::as_bool) == Some(true) || status.contains("timeout")
    {
        return (CodingAgentToolCallStatus::TimedOut, error);
    }
    if object.get("cancelled").and_then(Value::as_bool) == Some(true) || status.contains("cancel") {
        return (CodingAgentToolCallStatus::Cancelled, error);
    }
    if let Some(code) = object
        .get("exit_code")
        .or_else(|| object.get("exitCode"))
        .and_then(Value::as_i64)
    {
        return if code == 0 {
            (CodingAgentToolCallStatus::Succeeded, error)
        } else {
            (
                CodingAgentToolCallStatus::Failed,
                error.or_else(|| Some(format!("tool exited with code {code}"))),
            )
        };
    }
    if error.is_some() || matches!(status.as_str(), "failed" | "error") {
        return (CodingAgentToolCallStatus::Failed, error);
    }
    if object.get("success").and_then(Value::as_bool) == Some(true)
        || object.get("ok").and_then(Value::as_bool) == Some(true)
        || matches!(status.as_str(), "success" | "succeeded" | "completed")
    {
        return (CodingAgentToolCallStatus::Succeeded, None);
    }
    (CodingAgentToolCallStatus::Unknown, None)
}

fn nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn provider_for(model: &str) -> &'static str {
    let model = model.to_ascii_lowercase();
    if model.starts_with("claude") {
        "anthropic"
    } else if model.starts_with("gemini") {
        "google"
    } else {
        "openai"
    }
}

fn usage_delta(start: Option<TokenUsage>, end: Option<TokenUsage>) -> TokenUsage {
    let (Some(start), Some(end)) = (start, end) else {
        return TokenUsage::default();
    };
    TokenUsage {
        input: end.input.saturating_sub(start.input),
        output: end.output.saturating_sub(start.output),
        cache_read: end.cache_read.saturating_sub(start.cache_read),
    }
}

fn parse_token_usage(path: &Path) -> Option<TokenUsage> {
    parse_token_usage_lines(&std::fs::read_to_string(path).ok()?)
}

fn parse_token_usage_lines(content: &str) -> Option<TokenUsage> {
    content.lines().filter_map(token_usage_line).next_back()
}

fn token_usage_line(line: &str) -> Option<TokenUsage> {
    let entry: Value = serde_json::from_str(line).ok()?;
    if entry["type"] != "event_msg" || entry["payload"]["type"] != "token_count" {
        return None;
    }
    let usage = entry["payload"]["info"]["total_token_usage"]
        .as_object()
        .or_else(|| entry["payload"]["total_token_usage"].as_object())?;
    Some(TokenUsage {
        input: usage.get("input_tokens")?.as_u64()?,
        output: usage.get("output_tokens")?.as_u64()?,
        cache_read: usage
            .get("cached_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

fn spool_path(root: &Path, session_id: &str, turn_id: &str) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(session_id.as_bytes());
    digest.update([0]);
    digest.update(turn_id.as_bytes());
    root.join(format!("{}.json", hex::encode(digest.finalize())))
}

fn read_pending(path: &Path) -> PendingTurn {
    std::fs::read(path)
        .ok()
        .and_then(|content| serde_json::from_slice(&content).ok())
        .unwrap_or_default()
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("turn");
    let temp = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .with_context(|| format!("failed to create {}", temp.display()))?;
        file.write_all(content)
            .with_context(|| format!("failed to write {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", temp.display()))?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("failed to replace {}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(event: &str) -> HookPayload {
        HookPayload {
            session_id: "session".into(),
            turn_id: "turn".into(),
            hook_event_name: event.into(),
            model: Some("gpt-5-codex".into()),
            prompt: (event == "UserPromptSubmit").then(|| "Build it".into()),
            last_assistant_message: (event == "Stop").then(|| "Built".into()),
            transcript_path: None,
            tool_use_id: None,
            tool_name: None,
            tool_input: None,
            tool_response: None,
            error: None,
        }
    }

    #[test]
    fn hook_merge_preserves_foreign_json_and_replaces_old_nasiko_handlers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        let foreign = json!({"type": "command", "command": "echo foreign", "custom": true});
        let old = json!({"type": "command", "command": "bash '/old/nasiko-session-report.sh'"});
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "theme": "dark",
                "hooks": {
                    "Other": [{"custom_group": true}],
                    "UserPromptSubmit": [{"matcher": "*", "hooks": [foreign, old]}]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let script = Path::new("/new path/nasiko-session-report.sh");
        register_hooks(&path, script).unwrap();
        let document = read_hooks(&path).unwrap();

        assert_eq!(document["theme"], "dark");
        assert_eq!(document["hooks"]["Other"][0]["custom_group"], true);
        assert_eq!(document["hooks"]["UserPromptSubmit"][0]["matcher"], "*");
        assert_eq!(
            document["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            "echo foreign"
        );
        assert!(hooks_are_current(&document, script));
        assert_eq!(document["hooks"]["Stop"][0]["hooks"][0]["timeout"], 10);
    }

    #[test]
    fn uninstall_removes_only_nasiko_and_deletes_empty_event_arrays() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        let script = Path::new("/tmp/nasiko-session-report.sh");
        let mut document = json!({"keep": 1, "hooks": {"Other": [1]}});
        for event in HOOK_EVENTS {
            document["hooks"][event] = json!([{"hooks": [expected_hook(script)]}]);
        }
        document["hooks"]["Stop"][0]["hooks"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"type": "command", "command": "other"}));
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();

        deregister_hooks(&path).unwrap();
        let document = read_hooks(&path).unwrap();
        assert_eq!(document["keep"], 1);
        assert!(document["hooks"]["UserPromptSubmit"].is_null());
        assert_eq!(document["hooks"]["Stop"][0]["hooks"][0]["command"], "other");
        assert_eq!(document["hooks"]["Other"], json!([1]));
    }

    #[test]
    fn version_matching_requires_exact_current_handlers_for_both_events() {
        let script = Path::new("/tmp/nasiko-session-report.sh");
        let mut document = json!({"hooks": {}});
        for event in HOOK_EVENTS {
            document["hooks"][event] = json!([{"hooks": [expected_hook(script)]}]);
        }
        assert!(hooks_are_current(&document, script));
        document["hooks"]["Stop"][0]["hooks"][0]["timeout"] = json!(9);
        assert!(!hooks_are_current(&document, script));
        document["hooks"]["Stop"][0]["hooks"][0] = expected_hook(script);
        document["hooks"]["UserPromptSubmit"] = json!([]);
        assert!(!hooks_are_current(&document, script));
    }

    #[test]
    fn split_events_assemble_one_turn_with_a_call() {
        let mut pending = PendingTurn::default();
        let start = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let end = DateTime::parse_from_rfc3339("2026-01-01T00:00:02Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(
            apply_event(
                &mut pending,
                &payload("UserPromptSubmit"),
                start,
                Some(TokenUsage {
                    input: 10,
                    output: 4,
                    cache_read: 2
                })
            )
            .is_none()
        );
        let snapshot = apply_event(
            &mut pending,
            &payload("Stop"),
            end,
            Some(TokenUsage {
                input: 17,
                output: 9,
                cache_read: 5,
            }),
        )
        .unwrap();
        let turn = &snapshot.turns[0];
        assert_eq!(turn.prompt, "Build it");
        assert_eq!(turn.response.as_deref(), Some("Built"));
        assert_eq!(turn.calls.len(), 1);
        assert_eq!(turn.calls[0].input_tokens, 7);
        assert_eq!(turn.calls[0].output_tokens, 5);
        assert_eq!(turn.calls[0].cache_read_tokens, 3);
    }

    #[test]
    fn incomplete_event_is_spooled_without_touching_home() {
        let dir = tempfile::tempdir().unwrap();
        let raw = r#"{"session_id":"s","turn_id":"t","hook_event_name":"UserPromptSubmit","model":"gpt-5","prompt":"hi"}"#;
        let snapshot = snapshot_in(raw, dir.path()).unwrap();
        assert!(snapshot.turns.is_empty());
        assert!(spool_path(dir.path(), "s", "t").exists());
    }

    #[test]
    fn complete_events_remain_replayable_for_delivery_retries() {
        let mut pending = PendingTurn::default();
        let at = DateTime::from_timestamp(10, 0).unwrap();
        assert!(apply_event(&mut pending, &payload("UserPromptSubmit"), at, None).is_none());
        assert!(apply_event(&mut pending, &payload("Stop"), at, None).is_some());
        assert!(apply_event(&mut pending, &payload("Stop"), at, None).is_some());
    }

    #[test]
    fn tool_hooks_pair_out_of_order_without_regressing_on_replay() {
        let mut pending = PendingTurn::default();
        let at = DateTime::from_timestamp(10, 0).unwrap();
        let post = HookPayload {
            tool_use_id: Some("native-1".into()),
            tool_name: Some("shell".into()),
            tool_response: Some(json!({"success": true})),
            ..payload("PostToolUse")
        };
        let pre = HookPayload {
            tool_use_id: Some("native-1".into()),
            tool_name: Some("shell".into()),
            tool_input: Some(json!({"command": "redacted"})),
            ..payload("PreToolUse")
        };
        assert!(apply_event(&mut pending, &post, at, None).is_none());
        assert!(apply_event(&mut pending, &pre, at, None).is_none());
        assert_eq!(
            pending.tool_calls[0].status,
            CodingAgentToolCallStatus::Succeeded
        );
        assert!(apply_event(&mut pending, &payload("UserPromptSubmit"), at, None).is_none());
        let snapshot = apply_event(&mut pending, &payload("Stop"), at, None).unwrap();
        assert_eq!(snapshot.turns[0].tool_calls[0].id, "native-1");
        assert_eq!(
            snapshot.turns[0].tool_calls[0].association,
            CodingAgentToolAssociation::Turn
        );
    }

    #[test]
    fn stop_finalizes_a_running_tool_and_emits_the_complete_turn() {
        let mut pending = PendingTurn::default();
        let at = DateTime::from_timestamp(10, 0).unwrap();
        let pre = HookPayload {
            tool_use_id: Some("native-1".into()),
            tool_name: Some("shell".into()),
            ..payload("PreToolUse")
        };
        assert!(apply_event(&mut pending, &payload("UserPromptSubmit"), at, None).is_none());
        assert!(apply_event(&mut pending, &pre, at, None).is_none());
        let snapshot = apply_event(&mut pending, &payload("Stop"), at, None).unwrap();
        assert_eq!(
            snapshot.turns[0].tool_calls[0].status,
            CodingAgentToolCallStatus::Unknown
        );
    }

    #[test]
    fn post_tool_use_maps_nonzero_bash_exit_and_explicit_success() {
        let mut pending = PendingTurn::default();
        let failed = HookPayload {
            tool_use_id: Some("bash-1".into()),
            tool_name: Some("Bash".into()),
            tool_response: Some(json!({"exit_code": 2, "stdout": "", "stderr": "bad"})),
            ..payload("PostToolUse")
        };
        apply_event(
            &mut pending,
            &failed,
            DateTime::from_timestamp(10, 0).unwrap(),
            None,
        );
        assert_eq!(
            pending.tool_calls[0].status,
            CodingAgentToolCallStatus::Failed
        );
        assert_eq!(
            pending.tool_calls[0].error.as_deref(),
            Some("tool exited with code 2")
        );
        assert_eq!(
            tool_response_status(Some(&json!({"exit_code": 0, "stdout": "ok"})), None).0,
            CodingAgentToolCallStatus::Succeeded
        );
        assert_eq!(
            tool_response_status(Some(&json!({"stdout": "ambiguous"})), None).0,
            CodingAgentToolCallStatus::Unknown
        );
    }

    #[test]
    fn malformed_and_unknown_payloads_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(snapshot_in("not json", dir.path()).is_err());
        let unknown =
            r#"{"session_id":"s","turn_id":"t","hook_event_name":"ToolUse","model":"gpt-5"}"#;
        assert!(snapshot_in(unknown, dir.path()).is_err());
    }

    #[test]
    fn parses_latest_cumulative_token_count_and_ignores_reasoning() {
        let lines = r#"
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":3,"output_tokens":4,"reasoning_output_tokens":99}}}}
not-json
{"type":"response_item","payload":{"type":"reasoning","text":"secret"}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":21,"cached_input_tokens":8,"output_tokens":7,"reasoning_output_tokens":101}}}}
"#;
        assert_eq!(
            parse_token_usage_lines(lines),
            Some(TokenUsage {
                input: 21,
                output: 7,
                cache_read: 8
            })
        );
    }

    #[test]
    fn provider_defaults_to_openai_with_clear_vendor_exceptions() {
        assert_eq!(provider_for("gpt-5-codex"), "openai");
        assert_eq!(provider_for("custom"), "openai");
        assert_eq!(provider_for("Claude-Sonnet"), "anthropic");
        assert_eq!(provider_for("gemini-3"), "google");
    }
}
