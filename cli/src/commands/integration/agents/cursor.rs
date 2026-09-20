//! Cursor user-hook lifecycle and split-event turn assembly.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use nasiko_types::{
    CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCallStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::catalog::{self, AgentSpec, Support};
use super::super::launcher::{
    SCRIPT_NAME, install as install_launcher, installed_version as launcher_version, script_path,
    shell_quote, uninstall as uninstall_launcher,
};
use super::super::model::{LlmCall, SessionSnapshot, ToolCall, Turn};
use super::super::state;

pub const INSTALL_VERSION: u32 = 2;
pub const SPEC: AgentSpec = AgentSpec {
    id: "cursor",
    display_name: "Cursor CLI",
    binary: "cursor-agent",
    agent_name: "cursor",
    support: Support::Instrumented,
};

const HOOK_TIMEOUT_SECS: u32 = 10;
const EVENTS: [(&str, &str); 6] = [
    ("beforeSubmitPrompt", "UserPromptSubmit"),
    ("afterAgentResponse", "AgentResponse"),
    ("stop", "Stop"),
    ("preToolUse", "*"),
    ("postToolUse", "*"),
    ("postToolUseFailure", "*"),
];

#[derive(Debug, Deserialize)]
struct HookPayload {
    conversation_id: String,
    generation_id: String,
    hook_event_name: String,
    model: Option<String>,
    model_id: Option<String>,
    prompt: Option<String>,
    text: Option<String>,
    status: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    tool_use_id: Option<String>,
    #[serde(alias = "tool_name")]
    name: Option<String>,
    #[serde(alias = "tool_input")]
    input: Option<Value>,
    #[serde(alias = "tool_output")]
    output: Option<Value>,
    error: Option<Value>,
    error_message: Option<Value>,
    duration: Option<u64>,
    duration_ms: Option<u64>,
    failure_type: Option<String>,
    is_interrupt: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PendingTurn {
    prompt: Option<String>,
    response: Option<String>,
    model: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    status: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallRecord>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ToolCallRecord {
    id: String,
    name: String,
    arguments: Option<Value>,
    output: Option<Value>,
    error: Option<String>,
    started_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
    duration_ms: Option<u64>,
    status: CodingAgentToolCallStatus,
}

pub fn config_path() -> PathBuf {
    config_path_from(
        std::env::var_os("CURSOR_CONFIG_DIR"),
        std::env::var_os("XDG_CONFIG_HOME"),
        catalog::home(),
    )
}

fn config_path_from(
    cursor_config_dir: Option<std::ffi::OsString>,
    xdg_config_home: Option<std::ffi::OsString>,
    home: PathBuf,
) -> PathBuf {
    if let Some(path) = cursor_config_dir.filter(|value| !value.is_empty()) {
        return PathBuf::from(path);
    }
    if let Some(path) = xdg_config_home.filter(|value| !value.is_empty()) {
        return PathBuf::from(path).join("cursor");
    }
    home.join(".cursor")
}

pub fn install() -> Result<(PathBuf, PathBuf)> {
    let config = config_path();
    let launcher = install_launcher(&config, SPEC.id, INSTALL_VERSION)?;
    let hooks = hooks_path(&config);
    if let Err(error) = register_hooks(&hooks, &launcher) {
        let _ = uninstall_launcher(&config);
        return Err(error);
    }
    Ok((launcher, hooks))
}

pub fn uninstall() -> Result<()> {
    let config = config_path();
    deregister_hooks(&hooks_path(&config))?;
    uninstall_launcher(&config)
}

pub fn installed_version() -> Option<u32> {
    let config = config_path();
    let version = launcher_version(&config)?;
    let hooks = read_json(&hooks_path(&config)).ok()?;
    let command = hook_command(&script_path(&config));
    EVENTS
        .iter()
        .all(|(event, matcher)| has_exact_handler(&hooks, event, matcher, &command))
        .then_some(version)
}

pub fn snapshot(raw: &str) -> Result<SessionSnapshot> {
    let payload: HookPayload = serde_json::from_str(raw).with_context(|| {
        format!(
            "Cursor hook payload is not expected JSON; got: {}",
            raw.chars().take(200).collect::<String>()
        )
    })?;
    let session_id = payload.conversation_id.clone();
    let turns = assemble_spooled(payload)?;
    Ok(SessionSnapshot { session_id, turns })
}

fn hooks_path(config: &Path) -> PathBuf {
    config.join("hooks.json")
}

fn hook_command(launcher: &Path) -> String {
    format!("bash {}", shell_quote(&launcher.to_string_lossy()))
}

fn expected_handler(matcher: &str, command: &str) -> Value {
    json!({
        "command": command,
        "timeout": HOOK_TIMEOUT_SECS,
        "matcher": matcher,
    })
}

fn register_hooks(path: &Path, launcher: &Path) -> Result<()> {
    let mut config = read_json(path)?;
    ensure_hooks_object(&mut config);
    config["version"] = json!(1);
    let command = hook_command(launcher);
    for (event, matcher) in EVENTS {
        let mut handlers = handlers_without_nasiko(&config, event);
        handlers.push(expected_handler(matcher, &command));
        config["hooks"][event] = Value::Array(handlers);
    }
    write_json(path, &config)
}

fn deregister_hooks(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut config = read_json(path)?;
    ensure_hooks_object(&mut config);
    for (event, _) in EVENTS {
        let handlers = handlers_without_nasiko(&config, event);
        if handlers.is_empty() {
            config["hooks"].as_object_mut().unwrap().remove(event);
        } else {
            config["hooks"][event] = Value::Array(handlers);
        }
    }
    write_json(path, &config)
}

fn handlers_without_nasiko(config: &Value, event: &str) -> Vec<Value> {
    config["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|handler| !is_nasiko_handler(handler))
        .cloned()
        .collect()
}

fn is_nasiko_handler(handler: &Value) -> bool {
    handler["command"]
        .as_str()
        .is_some_and(|command| command.contains(SCRIPT_NAME))
}

fn has_exact_handler(config: &Value, event: &str, matcher: &str, command: &str) -> bool {
    let expected = expected_handler(matcher, command);
    config["hooks"][event]
        .as_array()
        .is_some_and(|handlers| handlers.iter().any(|handler| handler == &expected))
}

fn ensure_hooks_object(config: &mut Value) {
    if !config.is_object() {
        *config = json!({});
    }
    if !config["hooks"].is_object() {
        config["hooks"] = json!({});
    }
}

fn read_json(path: &Path) -> Result<Value> {
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

fn write_json(path: &Path, value: &Value) -> Result<()> {
    atomic_write(path, serde_json::to_string_pretty(value)?.as_bytes())
}

fn assemble_spooled(payload: HookPayload) -> Result<Vec<Turn>> {
    let directory = state::integrations_dir().join("events").join(SPEC.id);
    let name = spool_name(&payload.conversation_id, &payload.generation_id);
    let path = directory.join(format!("{name}.json"));
    let lock_path = directory.join(format!("{name}.lock"));
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("failed to open {}", lock_path.display()))?;
    lock.lock()
        .with_context(|| format!("failed to lock {}", lock_path.display()))?;
    let mut pending = read_pending(&path)?;
    let turns = apply_event(&mut pending, &payload, Utc::now());
    atomic_write(&path, serde_json::to_vec(&pending)?.as_slice())?;
    Ok(turns)
}

fn spool_name(session_id: &str, generation_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update([0]);
    hasher.update(generation_id.as_bytes());
    hex::encode(hasher.finalize())
}

fn read_pending(path: &Path) -> Result<PendingTurn> {
    if !path.exists() {
        return Ok(PendingTurn::default());
    }
    let content =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&content).with_context(|| format!("failed to parse {}", path.display()))
}

fn apply_event(pending: &mut PendingTurn, payload: &HookPayload, now: DateTime<Utc>) -> Vec<Turn> {
    match payload.hook_event_name.as_str() {
        "UserPromptSubmit" | "beforeSubmitPrompt" => {
            if let Some(prompt) = payload.prompt.as_deref().and_then(nonempty) {
                pending.prompt = Some(prompt);
            }
            pending.started_at.get_or_insert(now);
        }
        "AgentResponse" | "afterAgentResponse" => {
            if let Some(response) = payload.text.as_deref().and_then(nonempty) {
                pending.response = Some(response);
            }
            pending.model = payload
                .model_id
                .as_deref()
                .and_then(nonempty)
                .or_else(|| payload.model.as_deref().and_then(nonempty))
                .or_else(|| pending.model.clone());
            update_if_some(&mut pending.input_tokens, payload.input_tokens);
            update_if_some(&mut pending.output_tokens, payload.output_tokens);
            update_if_some(&mut pending.cache_read_tokens, payload.cache_read_tokens);
            update_if_some(&mut pending.cache_write_tokens, payload.cache_write_tokens);
            pending.ended_at = Some(now);
        }
        "Stop" | "stop" => {
            pending.status = payload.status.as_deref().and_then(nonempty);
            if pending.status.as_deref() == Some("completed") {
                for tool in &mut pending.tool_calls {
                    if matches!(
                        tool.status,
                        CodingAgentToolCallStatus::Pending | CodingAgentToolCallStatus::Running
                    ) {
                        tool.status = CodingAgentToolCallStatus::Unknown;
                    }
                }
            }
        }
        "PreToolUse" | "preToolUse" => upsert_tool_start(pending, payload, now),
        "PostToolUse" | "postToolUse" => upsert_tool_end(pending, payload, now, false),
        "PostToolUseFailure" | "postToolUseFailure" => upsert_tool_end(pending, payload, now, true),
        _ => return Vec::new(),
    }

    complete_turn(pending, payload)
        .into_iter()
        .collect::<Vec<_>>()
}

fn complete_turn(pending: &mut PendingTurn, payload: &HookPayload) -> Option<Turn> {
    if pending.status.as_deref() != Some("completed") {
        return None;
    }
    if pending.tool_calls.iter().any(|tool| {
        matches!(
            tool.status,
            CodingAgentToolCallStatus::Pending | CodingAgentToolCallStatus::Running
        )
    }) {
        return None;
    }
    let prompt = pending.prompt.clone()?;
    let response = pending.response.clone()?;
    let ended_at = pending.ended_at?;
    let started_at = pending.started_at?.min(ended_at);
    let model = pending.model.clone().unwrap_or_else(|| "unknown".into());
    Some(Turn {
        uuid: payload.generation_id.clone(),
        prompt,
        response: Some(response),
        started_at,
        ended_at,
        calls: vec![LlmCall {
            uuid: payload.generation_id.clone(),
            provider: provider_for_model(&model).to_string(),
            model,
            input_tokens: pending.input_tokens.unwrap_or(0),
            output_tokens: pending.output_tokens.unwrap_or(0),
            cache_read_tokens: pending.cache_read_tokens.unwrap_or(0),
            cache_creation_tokens: pending.cache_write_tokens.unwrap_or(0),
            started_at,
            ended_at,
        }],
        tool_calls: pending
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
                duration_ms: tool.duration_ms.or_else(|| {
                    tool.started_at
                        .zip(tool.ended_at)
                        .map(|(start, end)| (end - start).num_milliseconds().max(0) as u64)
                }),
                association: CodingAgentToolAssociation::Turn,
                timestamp_quality: CodingAgentTimestampQuality::Receipt,
            })
            .collect(),
    })
}

fn upsert_tool_start(pending: &mut PendingTurn, payload: &HookPayload, now: DateTime<Utc>) {
    let Some(id) = payload.tool_use_id.as_deref().and_then(nonempty) else {
        return;
    };
    if let Some(tool) = pending.tool_calls.iter_mut().find(|tool| tool.id == id) {
        tool.name = payload.name.clone().unwrap_or_else(|| tool.name.clone());
        tool.arguments = payload.input.clone().or_else(|| tool.arguments.clone());
        tool.started_at.get_or_insert(now);
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
            id,
            name: payload.name.clone().unwrap_or_else(|| "unknown".into()),
            arguments: payload.input.clone(),
            output: None,
            error: None,
            started_at: Some(now),
            ended_at: None,
            duration_ms: None,
            status: CodingAgentToolCallStatus::Running,
        });
    }
}

fn upsert_tool_end(
    pending: &mut PendingTurn,
    payload: &HookPayload,
    now: DateTime<Utc>,
    failed: bool,
) {
    let Some(id) = payload.tool_use_id.as_deref().and_then(nonempty) else {
        return;
    };
    if !pending.tool_calls.iter().any(|tool| tool.id == id) {
        pending.tool_calls.push(ToolCallRecord {
            id: id.clone(),
            name: payload.name.clone().unwrap_or_else(|| "unknown".into()),
            arguments: payload.input.clone(),
            output: None,
            error: None,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            status: CodingAgentToolCallStatus::Unknown,
        });
    }
    let tool = pending
        .tool_calls
        .iter_mut()
        .find(|tool| tool.id == id)
        .expect("tool exists");
    tool.output = payload.output.clone();
    tool.error = payload
        .error_message
        .as_ref()
        .map(value_text)
        .or_else(|| payload.error.as_ref().map(value_text))
        .or_else(|| payload.failure_type.clone());
    tool.ended_at = Some(now);
    tool.duration_ms = payload.duration_ms.or(payload.duration);
    let failure_type = payload
        .failure_type
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    tool.status = if payload.is_interrupt == Some(true) {
        CodingAgentToolCallStatus::Cancelled
    } else if failure_type.contains("denied") || failure_type.contains("permission") {
        CodingAgentToolCallStatus::Denied
    } else if failure_type.contains("timeout") {
        CodingAgentToolCallStatus::TimedOut
    } else if failed || tool.error.is_some() {
        CodingAgentToolCallStatus::Failed
    } else {
        CodingAgentToolCallStatus::Succeeded
    };
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn provider_for_model(model: &str) -> &'static str {
    let model = model.to_ascii_lowercase();
    let o_series = model
        .strip_prefix('o')
        .and_then(|suffix| suffix.chars().next())
        .is_some_and(|character| character.is_ascii_digit());
    if model.contains("claude") {
        "anthropic"
    } else if model.contains("gpt")
        || model == "o"
        || model.starts_with("o-")
        || o_series
        || model.contains("openai")
    {
        "openai"
    } else if model.contains("gemini") {
        "google"
    } else {
        "unknown"
    }
}

fn update_if_some(target: &mut Option<u64>, value: Option<u64>) {
    if value.is_some() {
        *target = value;
    }
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    let Some(parent) = path.parent() else {
        bail!("cannot write path without parent: {}", path.display());
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("cursor");
    let temporary = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = File::create(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        file.write_all(content)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("failed to replace {}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn payload(event: &str) -> HookPayload {
        HookPayload {
            conversation_id: "conversation".into(),
            generation_id: "generation".into(),
            hook_event_name: event.into(),
            model: None,
            model_id: None,
            prompt: None,
            text: None,
            status: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            tool_use_id: None,
            name: None,
            input: None,
            output: None,
            error: None,
            error_message: None,
            duration: None,
            duration_ms: None,
            failure_type: None,
            is_interrupt: None,
        }
    }

    fn before() -> HookPayload {
        HookPayload {
            prompt: Some("Build it".into()),
            ..payload("UserPromptSubmit")
        }
    }

    fn response() -> HookPayload {
        HookPayload {
            text: Some("Built".into()),
            model_id: Some("claude-sonnet-4".into()),
            input_tokens: Some(11),
            output_tokens: Some(7),
            cache_read_tokens: Some(3),
            cache_write_tokens: Some(2),
            ..payload("AgentResponse")
        }
    }

    fn stop(status: &str) -> HookPayload {
        HookPayload {
            status: Some(status.into()),
            ..payload("Stop")
        }
    }

    #[test]
    fn config_path_honors_cursor_then_xdg_then_home_without_changing_environment() {
        let home = PathBuf::from("/home/test");
        assert_eq!(
            config_path_from(
                Some(OsString::from("/custom")),
                Some(OsString::from("/xdg")),
                home.clone()
            ),
            PathBuf::from("/custom")
        );
        assert_eq!(
            config_path_from(
                Some(OsString::new()),
                Some(OsString::from("/xdg")),
                home.clone()
            ),
            PathBuf::from("/xdg/cursor")
        );
        assert_eq!(
            config_path_from(None, Some(OsString::new()), home.clone()),
            home.join(".cursor")
        );
    }

    #[test]
    fn registration_merges_foreign_config_and_replaces_old_nasiko_handlers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        std::fs::write(
            &path,
            r#"{"foreign":{"kept":true},"hooks":{"beforeSubmitPrompt":[{"command":"echo foreign","timeout":4},{"command":"bash '/old/nasiko-session-report.sh'"}],"customEvent":[{"command":"custom"}]}}"#,
        )
        .unwrap();
        let launcher = Path::new("/new path/nasiko-session-report.sh");

        register_hooks(&path, launcher).unwrap();
        let config = read_json(&path).unwrap();

        assert_eq!(config["version"], 1);
        assert_eq!(config["foreign"]["kept"], true);
        assert_eq!(config["hooks"]["customEvent"][0]["command"], "custom");
        assert_eq!(
            config["hooks"]["beforeSubmitPrompt"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let command = hook_command(launcher);
        for (event, matcher) in EVENTS {
            assert!(has_exact_handler(&config, event, matcher, &command));
        }
        for event in ["preToolUse", "postToolUse", "postToolUseFailure"] {
            assert_eq!(
                config["hooks"][event].as_array().unwrap().last().unwrap()["matcher"],
                "*"
            );
        }
    }

    #[test]
    fn tool_hook_wildcard_matches_shell_and_read_native_names() {
        let command = "bash '/tmp/nasiko-session-report.sh'";
        for event in ["preToolUse", "postToolUse", "postToolUseFailure"] {
            let handler = expected_handler("*", command);
            for native_name in ["Shell", "Read"] {
                assert!(
                    handler["matcher"] == "*" || handler["matcher"] == native_name,
                    "{event} must match native tool {native_name}"
                );
            }
        }
    }

    #[test]
    fn removal_deletes_only_nasiko_handlers_and_preserves_mixed_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.json");
        let launcher = Path::new("/tmp/nasiko-session-report.sh");
        let command = hook_command(launcher);
        let mut config = json!({"version": 1, "other": 9, "hooks": {
            "beforeSubmitPrompt": [{"command": "foreign"}, expected_handler("UserPromptSubmit", &command)],
            "afterAgentResponse": [expected_handler("AgentResponse", &command)],
            "stop": [expected_handler("Stop", &command)],
            "customEvent": [{"command": "custom"}]
        }});
        write_json(&path, &config).unwrap();

        deregister_hooks(&path).unwrap();
        config = read_json(&path).unwrap();

        assert_eq!(config["other"], 9);
        assert_eq!(
            config["hooks"]["beforeSubmitPrompt"][0]["command"],
            "foreign"
        );
        assert!(config["hooks"].get("afterAgentResponse").is_none());
        assert!(config["hooks"].get("stop").is_none());
        assert_eq!(config["hooks"]["customEvent"][0]["command"], "custom");
    }

    #[test]
    fn version_matching_requires_every_exact_handler() {
        let command = "bash '/tmp/nasiko-session-report.sh'";
        let mut config = json!({"hooks": {}});
        ensure_hooks_object(&mut config);
        for (event, matcher) in EVENTS {
            config["hooks"][event] = json!([expected_handler(matcher, command)]);
        }
        assert!(
            EVENTS
                .iter()
                .all(|(event, matcher)| has_exact_handler(&config, event, matcher, command))
        );

        config["hooks"]["stop"][0]["timeout"] = json!(9);
        assert!(!has_exact_handler(&config, "stop", "Stop", command));
        config["hooks"]["stop"][0]["timeout"] = json!(10);
        config["hooks"]["stop"][0]["matcher"] = json!("Other");
        assert!(!has_exact_handler(&config, "stop", "Stop", command));
    }

    #[test]
    fn prompt_response_then_completed_stop_emits_one_complete_turn() {
        let mut pending = PendingTurn::default();
        let start = DateTime::from_timestamp(10, 0).unwrap();
        let end = DateTime::from_timestamp(20, 0).unwrap();
        assert!(apply_event(&mut pending, &before(), start).is_empty());
        assert!(apply_event(&mut pending, &response(), end).is_empty());
        let turns = apply_event(
            &mut pending,
            &stop("completed"),
            DateTime::from_timestamp(30, 0).unwrap(),
        );

        assert_eq!(turns.len(), 1);
        let turn = &turns[0];
        assert_eq!(turn.uuid, "generation");
        assert_eq!(turn.prompt, "Build it");
        assert_eq!(turn.response.as_deref(), Some("Built"));
        assert_eq!((turn.started_at, turn.ended_at), (start, end));
        assert_eq!(turn.calls[0].provider, "anthropic");
        assert_eq!(
            (turn.calls[0].input_tokens, turn.calls[0].output_tokens),
            (11, 7)
        );
        assert_eq!(apply_event(&mut pending, &stop("completed"), end).len(), 1);
    }

    #[test]
    fn completed_stop_before_response_defers_then_emits() {
        let mut pending = PendingTurn::default();
        let at = DateTime::from_timestamp(10, 0).unwrap();
        assert!(apply_event(&mut pending, &before(), at).is_empty());
        assert!(apply_event(&mut pending, &stop("completed"), at).is_empty());
        let turns = apply_event(&mut pending, &response(), at);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].response.as_deref(), Some("Built"));
    }

    #[test]
    fn response_and_stop_before_prompt_emit_when_prompt_arrives() {
        let mut pending = PendingTurn::default();
        let at = DateTime::from_timestamp(10, 0).unwrap();
        assert!(apply_event(&mut pending, &response(), at).is_empty());
        assert!(apply_event(&mut pending, &stop("completed"), at).is_empty());
        assert_eq!(apply_event(&mut pending, &before(), at).len(), 1);
    }

    #[test]
    fn aborted_and_error_stops_never_emit_turns() {
        for status in ["aborted", "error"] {
            let mut pending = PendingTurn::default();
            let at = DateTime::from_timestamp(10, 0).unwrap();
            assert!(apply_event(&mut pending, &before(), at).is_empty());
            assert!(apply_event(&mut pending, &response(), at).is_empty());
            assert!(apply_event(&mut pending, &stop(status), at).is_empty());
        }
    }

    #[test]
    fn tool_failure_is_spooled_and_included_after_lifecycle_completion() {
        let mut pending = PendingTurn::default();
        let at = DateTime::from_timestamp(10, 0).unwrap();
        let pre = HookPayload {
            tool_use_id: Some("native-1".into()),
            name: Some("read_file".into()),
            input: Some(json!({"path": "redacted"})),
            ..payload("preToolUse")
        };
        let failure = HookPayload {
            tool_use_id: Some("native-1".into()),
            name: Some("read_file".into()),
            error: Some(json!("denied")),
            duration_ms: Some(12),
            ..payload("postToolUseFailure")
        };
        assert!(apply_event(&mut pending, &pre, at).is_empty());
        assert!(apply_event(&mut pending, &failure, at).is_empty());
        assert!(apply_event(&mut pending, &before(), at).is_empty());
        assert!(apply_event(&mut pending, &response(), at).is_empty());
        let turns = apply_event(&mut pending, &stop("completed"), at);
        assert_eq!(
            turns[0].tool_calls[0].status,
            CodingAgentToolCallStatus::Failed
        );
        assert_eq!(turns[0].tool_calls[0].duration_ms, Some(12));
    }

    #[test]
    fn native_error_message_takes_precedence_over_documented_failure_type() {
        // Cursor's failure payload documents error_message alongside failure_type.
        let mut pending = PendingTurn::default();
        let at = DateTime::from_timestamp(10, 0).unwrap();
        let failure = HookPayload {
            tool_use_id: Some("native-1".into()),
            name: Some("Shell".into()),
            error_message: Some(json!("command exited with status 2")),
            failure_type: Some("tool_error".into()),
            ..payload("postToolUseFailure")
        };
        apply_event(&mut pending, &failure, at);
        assert_eq!(
            pending.tool_calls[0].error.as_deref(),
            Some("command exited with status 2")
        );
    }

    #[test]
    fn completed_stop_finalizes_a_running_tool_as_unknown() {
        let mut pending = PendingTurn::default();
        let at = DateTime::from_timestamp(10, 0).unwrap();
        let pre = HookPayload {
            tool_use_id: Some("native-1".into()),
            name: Some("read_file".into()),
            ..payload("preToolUse")
        };
        assert!(apply_event(&mut pending, &before(), at).is_empty());
        assert!(apply_event(&mut pending, &pre, at).is_empty());
        assert!(apply_event(&mut pending, &response(), at).is_empty());
        let turns = apply_event(&mut pending, &stop("completed"), at);
        assert_eq!(turns.len(), 1);
        assert_eq!(
            turns[0].tool_calls[0].status,
            CodingAgentToolCallStatus::Unknown
        );
    }

    #[test]
    fn infers_supported_model_providers() {
        assert_eq!(provider_for_model("claude-4"), "anthropic");
        assert_eq!(provider_for_model("gpt-5"), "openai");
        assert_eq!(provider_for_model("o3"), "openai");
        assert_eq!(provider_for_model("openai/custom"), "openai");
        assert_eq!(provider_for_model("gemini-2.5"), "google");
        assert_eq!(provider_for_model("other"), "unknown");
    }
}
