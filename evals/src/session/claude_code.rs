//! Reads a Claude Code session log (`~/.claude/projects/<folder>/<session-id>.jsonl`).
//!
//! The format is Claude Code's own and is not documented, so every field is optional here and
//! unknown lines are skipped. Fields used: `sessionId`, `cwd`, `gitBranch`, `timestamp`,
//! `origin.kind`, `isMeta`, `isSidechain`, `message.{id, model, usage, content}` and
//! `toolUseResult.{filePath, structuredPatch}`. One API reply is often written as several lines
//! with the same `message.id`, so usage is counted once per id.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use super::{FileEdit, ModelUsage, SessionError, SessionRecord, UserMessage};
use crate::cost::TokenUsage;
use crate::instance::SessionSource;

/// Reads one session log.
pub fn parse_claude_code_log(jsonl: &str) -> Result<SessionRecord, SessionError> {
    let lines: Vec<LogLine> = jsonl.lines().filter_map(parse_line).collect();
    let session_id = first(&lines, |l| l.session_id.clone()).ok_or(SessionError::NoSessionId)?;
    let cwd = first(&lines, |l| l.cwd.clone()).ok_or(SessionError::NoWorkingFolder)?;
    let times: Vec<DateTime<Utc>> = lines.iter().filter_map(|l| l.timestamp).collect();
    let (started_at, ended_at) = match (times.iter().min(), times.iter().max()) {
        (Some(a), Some(b)) => (*a, *b),
        _ => return Err(SessionError::NoTimestamps),
    };
    Ok(SessionRecord {
        session_id,
        source: SessionSource::ClaudeCode,
        cwd,
        git_branch: first(&lines, |l| l.git_branch.clone().filter(|b| !b.is_empty())),
        started_at,
        ended_at,
        user_messages: lines.iter().filter_map(typed_message).collect(),
        edits: lines.iter().filter_map(file_edit).collect(),
        usage: dedupe_usage(&lines),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    git_branch: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    #[serde(default)]
    is_meta: bool,
    #[serde(default)]
    is_sidechain: bool,
    origin: Option<Origin>,
    message: Option<Message>,
    tool_use_result: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct Origin {
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ApiUsage>,
    content: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

/// Skips lines that are not JSON or not shaped like a log entry.
fn parse_line(line: &str) -> Option<LogLine> {
    serde_json::from_str(line).ok()
}

fn first<T>(lines: &[LogLine], pick: impl Fn(&LogLine) -> Option<T>) -> Option<T> {
    lines.iter().find_map(pick)
}

/// Text the person typed. Tool results, task notifications, agent hand-backs, slash-command
/// echoes and sub-agent turns are not typed text.
fn typed_message(line: &LogLine) -> Option<UserMessage> {
    if line.kind.as_deref() != Some("user") || line.is_meta || line.is_sidechain {
        return None;
    }
    if let Some(kind) = line.origin.as_ref().and_then(|o| o.kind.as_deref())
        && kind != "human"
    {
        return None;
    }
    let text = typed_text(line.message.as_ref()?.content.as_ref()?)?;
    Some(UserMessage {
        at: line.timestamp?,
        text,
    })
}

/// The plain text parts of a message, without tag-wrapped context the editor or harness added
/// (`<ide_selection>`, `<system-reminder>`, `<command-name>` …).
fn typed_text(content: &Value) -> Option<String> {
    let parts: Vec<&str> = match content {
        Value::String(s) => vec![s.as_str()],
        Value::Array(items) => items
            .iter()
            .filter(|i| i.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect(),
        _ => return None,
    };
    let text = parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.starts_with('<'))
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

/// An Edit/Write/MultiEdit result: the file path and the lines its patch added.
fn file_edit(line: &LogLine) -> Option<FileEdit> {
    let result = line.tool_use_result.as_ref()?;
    let path = result.get("filePath")?.as_str()?.to_string();
    let hunks = result.get("structuredPatch")?.as_array()?;
    let added_lines = hunks
        .iter()
        .filter_map(|h| h.get("lines").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|l| l.strip_prefix('+'))
        .map(str::to_string)
        .collect();
    Some(FileEdit {
        at: line.timestamp?,
        path,
        added_lines,
    })
}

/// One usage entry per distinct API message id; later lines for an id are repeats.
fn dedupe_usage(lines: &[LogLine]) -> Vec<ModelUsage> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in lines
        .iter()
        .filter(|l| l.kind.as_deref() == Some("assistant"))
    {
        let Some(msg) = &line.message else { continue };
        let (Some(id), Some(model), Some(u), Some(at)) =
            (&msg.id, &msg.model, &msg.usage, line.timestamp)
        else {
            continue;
        };
        if model.starts_with('<') || !seen.insert(id.clone()) {
            continue;
        }
        out.push(ModelUsage {
            at,
            message_id: id.clone(),
            model: model.clone(),
            usage: TokenUsage {
                input: u.input_tokens,
                output: u.output_tokens,
                cache_write: u.cache_creation_input_tokens,
                cache_read: u.cache_read_input_tokens,
                reasoning: 0,
            },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = r#"{"type":"permission-mode","sessionId":"s1"}
{"type":"user","sessionId":"s1","cwd":"/repo","gitBranch":"main","timestamp":"2026-11-20T10:00:00Z","origin":{"kind":"human"},"message":{"role":"user","content":[{"type":"text","text":"<ide_selection>x</ide_selection>"},{"type":"text","text":"Fix the retry bug"}]}}
{"type":"assistant","sessionId":"s1","timestamp":"2026-11-20T10:00:05Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_read_input_tokens":0}}}
{"type":"assistant","sessionId":"s1","timestamp":"2026-11-20T10:00:06Z","message":{"id":"m1","model":"claude-opus-5-5","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":100,"cache_read_input_tokens":0}}}
{"type":"user","sessionId":"s1","timestamp":"2026-11-20T10:00:07Z","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]},"toolUseResult":{"filePath":"/repo/src/a.rs","structuredPatch":[{"lines":[" ctx","+new line","-old"]}]}}
{"type":"user","sessionId":"s1","timestamp":"2026-11-20T10:00:08Z","isMeta":true,"message":{"role":"user","content":"Another session sent a message"}}
{"type":"user","sessionId":"s1","timestamp":"2026-11-20T10:00:09Z","origin":{"kind":"task-notification"},"message":{"role":"user","content":"<task-notification>done</task-notification>"}}
not json at all
{"type":"user","sessionId":"s1","timestamp":"2026-11-20T10:05:00Z","origin":{"kind":"human"},"message":{"role":"user","content":"also add a test"}}
"#;

    #[test]
    fn reads_typed_messages_edits_and_deduped_usage() {
        let s = parse_claude_code_log(LOG).expect("valid log");
        assert_eq!(s.session_id, "s1");
        assert_eq!(s.cwd, "/repo");
        assert_eq!(s.git_branch.as_deref(), Some("main"));
        let texts: Vec<&str> = s.user_messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, vec!["Fix the retry bug", "also add a test"]);
        assert_eq!(s.edits.len(), 1);
        assert_eq!(s.edits[0].added_lines, vec!["new line"]);
        assert_eq!(s.usage.len(), 1, "same message id counted once");
        assert_eq!(s.usage[0].usage.cache_write, 100);
        assert_eq!(s.main_model().as_deref(), Some("claude-opus-5-5"));
        assert_eq!(
            s.edited_paths_in(&["/other".into(), "/repo".into()]),
            vec!["src/a.rs"]
        );
        assert_eq!((s.ended_at - s.started_at).num_seconds(), 300);
        let first_part = s.slice(None, "2026-11-20T10:01:00Z".parse().expect("time"));
        assert_eq!(first_part.user_messages.len(), 1);
        assert_eq!(first_part.total_usage().cache_write, 100);
        let later = s.slice(
            Some("2026-11-20T10:01:00Z".parse().expect("time")),
            s.ended_at,
        );
        assert_eq!(later.user_messages[0].text, "also add a test");
        assert_eq!(later.usage.len(), 0);
        let start = s.task_start(&["/repo".into()], &["src/a.rs".into()]);
        assert_eq!(start, Some("2026-11-20T10:00:00Z".parse().expect("time")));
        assert_eq!(s.task_start(&["/repo".into()], &["src/z.rs".into()]), None);
    }

    #[test]
    fn a_log_without_session_id_is_an_error() {
        assert_eq!(
            parse_claude_code_log("{\"type\":\"x\"}\n"),
            Err(SessionError::NoSessionId)
        );
    }
}
