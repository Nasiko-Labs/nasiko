//! OpenCode XDG plugin lifecycle and concrete session-message parsing.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nasiko_types::{
    CodingAgentTimestampQuality, CodingAgentToolAssociation, CodingAgentToolCallStatus,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::super::catalog::{AgentSpec, Support};
use super::super::launcher;
use super::super::model::{LlmCall, SessionSnapshot, ToolCall, Turn};

pub const INSTALL_VERSION: u32 = 4;
pub const SPEC: AgentSpec = AgentSpec {
    id: "opencode",
    display_name: "OpenCode",
    binary: "opencode",
    agent_name: "opencode",
    support: Support::Instrumented,
};

const PLUGIN_NAME: &str = "nasiko-session-report.js";

#[derive(Debug, Deserialize)]
struct HookPayload {
    session_id: String,
    messages: Vec<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    info: MessageInfo,
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Debug, Deserialize)]
struct MessageInfo {
    id: String,
    role: String,
    #[serde(rename = "parentID")]
    parent_id: Option<String>,
    #[serde(rename = "providerID")]
    provider_id: Option<String>,
    #[serde(rename = "modelID")]
    model_id: Option<String>,
    finish: Option<String>,
    #[serde(default)]
    summary: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
    #[serde(default)]
    time: MessageTime,
    #[serde(default)]
    tokens: Tokens,
}

#[derive(Debug, Default, Deserialize)]
struct MessageTime {
    created: Option<i64>,
    completed: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct Tokens {
    #[serde(default)]
    input: f64,
    #[serde(default)]
    output: f64,
    #[serde(default)]
    reasoning: f64,
    #[serde(default)]
    cache: CacheTokens,
}

#[derive(Debug, Default, Deserialize)]
struct CacheTokens {
    #[serde(default)]
    read: f64,
    #[serde(default)]
    write: f64,
}

#[derive(Debug, Deserialize)]
struct Part {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    #[serde(default)]
    ignored: bool,
    #[serde(default)]
    synthetic: bool,
    #[serde(rename = "callID", alias = "callId", alias = "call_id")]
    call_id: Option<String>,
    tool: Option<String>,
    name: Option<String>,
    #[serde(rename = "messageID", alias = "messageId", alias = "message_id")]
    message_id: Option<String>,
    state: Option<Value>,
}

pub fn config_path() -> PathBuf {
    crate::commands::opencode::config_path()
}

pub fn install() -> Result<(PathBuf, PathBuf)> {
    let script = launcher::install(&config_path(), SPEC.id, INSTALL_VERSION)?;
    let plugin = plugin_path();
    if let Err(error) = write_plugin(&plugin, &script) {
        let _ = launcher::uninstall(&config_path());
        return Err(error);
    }
    Ok((script, plugin))
}

pub fn uninstall() -> Result<()> {
    let plugin = plugin_path();
    if plugin.exists() {
        std::fs::remove_file(&plugin)
            .with_context(|| format!("failed to remove {}", plugin.display()))?;
    }
    launcher::uninstall(&config_path())
}

pub fn installed_version() -> Option<u32> {
    let script = launcher::installed_version(&config_path())?;
    let plugin_body = std::fs::read_to_string(plugin_path()).ok()?;
    let plugin = launcher::version_marker(&plugin_body)?;
    if !plugin_targets_script(&plugin_body, &launcher::script_path(&config_path())) {
        return None;
    }
    Some(script.min(plugin))
}

pub fn snapshot(raw: &str) -> Result<SessionSnapshot> {
    let payload: HookPayload = serde_json::from_str(raw).with_context(|| {
        format!(
            "OpenCode hook payload is not expected JSON; got: {}",
            raw.chars().take(200).collect::<String>()
        )
    })?;
    Ok(SessionSnapshot {
        session_id: payload.session_id,
        turns: turns_from_messages(&payload.messages),
    })
}

fn plugin_path() -> PathBuf {
    config_path().join("plugins").join(PLUGIN_NAME)
}

fn write_plugin(path: &Path, script: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(path, plugin_body(script))
        .with_context(|| format!("failed to write {}", path.display()))
}

fn plugin_body(script: &Path) -> String {
    let script = serde_json::to_string(&script.to_string_lossy()).expect("serializable path");
    format!(
        r#"// Managed by nasiko - do not edit. nasiko-hook-version: {INSTALL_VERSION}
export const NasikoSessionReporter = async ({{ client }}) => ({{
  event: async ({{ event }}) => {{
    if (event.type !== "session.idle") return
    try {{
      const response = await client.session.messages({{ path: {{ id: event.properties.sessionID }} }})
      if (!response.data) return
      const payload = JSON.stringify({{ session_id: event.properties.sessionID, messages: response.data }})
      const process = Bun.spawn([{script}], {{
        stdin: new TextEncoder().encode(payload), stdout: "ignore", stderr: "ignore",
      }})
      const completed = await Promise.race([
        process.exited.then(() => true),
        Bun.sleep(8000).then(() => false),
      ])
      if (!completed) process.unref()
    }} catch {{
      // Telemetry must never interrupt the coding session.
    }}
  }},
}})
"#
    )
}

fn plugin_targets_script(body: &str, script: &Path) -> bool {
    let script = serde_json::to_string(&script.to_string_lossy()).expect("serializable path");
    body.contains(&format!("Bun.spawn([{script}]"))
}

fn turns_from_messages(messages: &[Message]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut owners = HashMap::new();
    for message in messages {
        let info = &message.info;
        match info.role.as_str() {
            "user" => {
                let prompt = text_parts(&message.parts, true);
                if prompt.is_empty() {
                    if message
                        .parts
                        .iter()
                        .any(|part| part.kind == "text" && part.synthetic)
                        && let Some(owner) = turns.len().checked_sub(1)
                    {
                        owners.insert(info.id.clone(), owner);
                    }
                    continue;
                }
                let at = millis(info.time.created);
                owners.insert(info.id.clone(), turns.len());
                turns.push(Turn {
                    uuid: info.id.clone(),
                    prompt,
                    response: None,
                    started_at: at,
                    ended_at: at,
                    calls: Vec::new(),
                    tool_calls: Vec::new(),
                });
            }
            "assistant"
                if !info
                    .summary
                    .as_ref()
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false) =>
            {
                let Some(owner) = info
                    .parent_id
                    .as_ref()
                    .and_then(|id| owners.get(id))
                    .copied()
                else {
                    continue;
                };
                let started_at = millis(info.time.created);
                let ended_at = millis(info.time.completed.or(info.time.created));
                let turn = &mut turns[owner];
                turn.ended_at = turn.ended_at.max(ended_at);
                turn.calls.push(LlmCall {
                    uuid: info.id.clone(),
                    provider: info.provider_id.clone().unwrap_or_else(|| "unknown".into()),
                    model: info.model_id.clone().unwrap_or_else(|| "unknown".into()),
                    input_tokens: token(info.tokens.input),
                    output_tokens: token(info.tokens.output)
                        .saturating_add(token(info.tokens.reasoning)),
                    cache_read_tokens: token(info.tokens.cache.read),
                    cache_creation_tokens: token(info.tokens.cache.write),
                    started_at,
                    ended_at,
                });
                turn.tool_calls.extend(
                    message
                        .parts
                        .iter()
                        .enumerate()
                        .filter_map(|(index, part)| part.tool_call(info, index)),
                );
                if info.error.is_none() && info.finish.as_deref() != Some("tool-calls") {
                    let response = text_parts(&message.parts, false);
                    if !response.is_empty() {
                        turn.response = Some(response);
                    }
                }
            }
            _ => {}
        }
    }
    turns.retain(|turn| !turn.is_empty() && turn.response.is_some());
    turns
}

impl Part {
    fn tool_call(&self, info: &MessageInfo, index: usize) -> Option<ToolCall> {
        if self.kind != "tool" {
            return None;
        }
        let state = self.state.as_ref().unwrap_or(&Value::Null);
        let status = state
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let status = match status {
            "pending" => CodingAgentToolCallStatus::Pending,
            "running" => CodingAgentToolCallStatus::Running,
            "completed" | "success" => CodingAgentToolCallStatus::Succeeded,
            "error" | "failed" => CodingAgentToolCallStatus::Failed,
            _ => CodingAgentToolCallStatus::Unknown,
        };
        let times = state.get("time").unwrap_or(&Value::Null);
        let started_at = json_millis(times.get("start").or_else(|| times.get("created")));
        let ended_at = json_millis(times.get("end").or_else(|| times.get("completed")));
        let model_call_id = self.message_id.clone();
        Some(ToolCall {
            id: self
                .call_id
                .clone()
                .unwrap_or_else(|| format!("{}:tool:{index}", info.id)),
            name: self
                .tool
                .clone()
                .or_else(|| self.name.clone())
                .unwrap_or_else(|| "unknown".into()),
            kind: "tool".into(),
            model_call_id: model_call_id.clone(),
            status,
            arguments: state.get("input").cloned(),
            output: state.get("output").cloned(),
            raw: None,
            error: state
                .get("error")
                .filter(|value| !value.is_null())
                .map(value_text),
            started_at,
            ended_at,
            duration_ms: started_at
                .zip(ended_at)
                .map(|(start, end)| (end - start).num_milliseconds().max(0) as u64),
            association: if model_call_id.is_some() {
                CodingAgentToolAssociation::Exact
            } else {
                CodingAgentToolAssociation::Turn
            },
            timestamp_quality: if started_at.is_some() || ended_at.is_some() {
                CodingAgentTimestampQuality::Exact
            } else {
                CodingAgentTimestampQuality::Unknown
            },
        })
    }
}

fn json_millis(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;
    value
        .as_i64()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .or_else(|| value.as_str()?.parse().ok())
}

fn value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn text_parts(parts: &[Part], exclude_synthetic: bool) -> String {
    parts
        .iter()
        .filter(|part| part.kind == "text" && !part.ignored)
        .filter(|part| !exclude_synthetic || !part.synthetic)
        .filter_map(|part| {
            part.text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn millis(value: Option<i64>) -> DateTime<Utc> {
    value
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

fn token(value: f64) -> u64 {
    value.max(0.0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_loop_and_completion() {
        let payload = r#"{"session_id":"s","messages":[
          {"info":{"id":"u","role":"user","time":{"created":1000}},"parts":[{"type":"text","text":"Do it"}]},
          {"info":{"id":"a1","role":"assistant","parentID":"u","providerID":"openai","modelID":"gpt-5","finish":"tool-calls","time":{"created":1100,"completed":1200},"tokens":{"input":10,"output":2,"cache":{"read":3,"write":4}}},"parts":[{"type":"text","text":"Checking"}]},
          {"info":{"id":"a2","role":"assistant","parentID":"u","providerID":"openai","modelID":"gpt-5","finish":"stop","time":{"created":1300,"completed":1500},"tokens":{"input":20,"output":5,"reasoning":1,"cache":{"read":6,"write":7}}},"parts":[{"type":"text","text":"Finished"}]}
        ]}"#;
        let snapshot = snapshot(payload).unwrap();
        assert_eq!(snapshot.turns[0].calls.len(), 2);
        assert_eq!(snapshot.turns[0].calls[1].output_tokens, 6);
        assert_eq!(snapshot.turns[0].response.as_deref(), Some("Finished"));
    }

    #[test]
    fn accepts_user_summary_metadata_object() {
        let payload = r#"{"session_id":"s","messages":[
          {"info":{"id":"u","role":"user","summary":{"title":"Test","diffs":[]},"time":{"created":1000}},"parts":[{"type":"text","text":"Hello"}]},
          {"info":{"id":"a","role":"assistant","parentID":"u","summary":false,"finish":"stop","time":{"created":1100,"completed":1200},"tokens":{"input":1,"output":1}},"parts":[{"type":"text","text":"Hi"}]}
        ]}"#;

        let snapshot = snapshot(payload).unwrap();

        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(snapshot.turns[0].prompt, "Hello");
        assert_eq!(snapshot.turns[0].response.as_deref(), Some("Hi"));
    }

    #[test]
    fn plugin_bounds_its_wait_for_the_reporter() {
        let body = plugin_body(Path::new("/tmp/report script"));
        assert!(body.contains("session.idle"));
        assert!(body.contains("process.exited.then"));
        assert_eq!(launcher::version_marker(&body), Some(INSTALL_VERSION));
        assert!(body.contains("Bun.sleep(8000)"));
        assert!(body.contains("process.unref()"));
        assert!(plugin_targets_script(
            &body,
            Path::new("/tmp/report script")
        ));
        assert!(!plugin_targets_script(&body, Path::new("/tmp/old script")));
    }

    #[test]
    fn synthetic_compaction_continues_the_original_turn() {
        let payload = r#"{"session_id":"s","messages":[
          {"info":{"id":"u1","role":"user","time":{"created":1000}},"parts":[{"type":"text","text":"Task"}]},
          {"info":{"id":"u2","role":"user","time":{"created":2000}},"parts":[{"type":"text","text":"Continue","synthetic":true}]},
          {"info":{"id":"a","role":"assistant","parentID":"u2","finish":"stop","time":{"created":2100,"completed":2200},"tokens":{"output":1}},"parts":[{"type":"text","text":"Done"}]}
        ]}"#;
        let snapshot = snapshot(payload).unwrap();
        assert_eq!(snapshot.turns[0].uuid, "u1");
        assert_eq!(snapshot.turns[0].response.as_deref(), Some("Done"));
    }

    #[test]
    fn errored_turn_does_not_block_a_later_completed_turn() {
        let payload = r#"{"session_id":"s","messages":[
          {"info":{"id":"u1","role":"user"},"parts":[{"type":"text","text":"Fail"}]},
          {"info":{"id":"a1","role":"assistant","parentID":"u1","error":{"name":"APIError"},"tokens":{"input":1}},"parts":[]},
          {"info":{"id":"u2","role":"user"},"parts":[{"type":"text","text":"Work"}]},
          {"info":{"id":"a2","role":"assistant","parentID":"u2","finish":"stop","tokens":{"output":1}},"parts":[{"type":"text","text":"Done"}]}
        ]}"#;
        let snapshot = snapshot(payload).unwrap();
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(snapshot.turns[0].uuid, "u2");
    }

    #[test]
    fn parses_structured_tool_state_and_exact_message_correlation() {
        let payload = r#"{"session_id":"s","messages":[
          {"info":{"id":"u","role":"user","time":{"created":1000}},"parts":[{"type":"text","text":"Do it"}]},
          {"info":{"id":"a","role":"assistant","parentID":"u","finish":"stop","time":{"created":1100,"completed":1500},"tokens":{"output":1}},"parts":[
            {"type":"tool","callID":"call-1","tool":"bash","messageID":"a","state":{"status":"completed","input":{"command":"redacted"},"output":"ok","time":{"start":1200,"end":1400}}},
            {"type":"text","text":"Done"}
          ]}
        ]}"#;
        let snapshot = snapshot(payload).unwrap();
        let tool = &snapshot.turns[0].tool_calls[0];
        assert_eq!(tool.id, "call-1");
        assert_eq!(tool.model_call_id.as_deref(), Some("a"));
        assert_eq!(tool.status, CodingAgentToolCallStatus::Succeeded);
        assert_eq!(tool.duration_ms, Some(200));
    }
}
