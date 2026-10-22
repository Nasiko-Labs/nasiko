//! Completed turns from Codex rollouts. No hook spool or reporting side effects.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::Path;

use super::{PendingTurn, TokenUsage, completed_turn, nonempty, token_usage};
use crate::commands::integration::model::SessionSnapshot;

#[derive(Default)]
pub struct HistoryIndex {
    sessions: HashMap<String, HashSet<String>>,
    completed: HashSet<String>,
}

pub struct History {
    pub snapshot: SessionSnapshot,
    pub replayed: usize,
}

pub fn snapshot(path: &Path, known_turns: &mut HistoryIndex) -> Result<Option<History>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    parse(&content, known_turns)
}

pub fn origin(path: &Path) -> Result<(Option<DateTime<Utc>>, bool)> {
    let file =
        std::fs::File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    for line in std::io::BufReader::new(file).lines().take(8) {
        let entry: Value = serde_json::from_str(&line?)?;
        if entry["type"] == "session_meta" {
            return Ok(metadata_origin(&entry));
        }
    }
    Ok((None, false))
}

fn metadata_origin(entry: &Value) -> (Option<DateTime<Utc>>, bool) {
    let meta = &entry["payload"];
    let started = meta["timestamp"]
        .as_str()
        .or(entry["timestamp"].as_str())
        .and_then(|at| at.parse().ok());
    (started, meta["forked_from_id"].is_string())
}

#[derive(Default)]
struct Rollout {
    session_id: Option<String>,
    replaying: bool,
    usage: TokenUsage,
    model: Option<String>,
    next_prompt: Option<(Option<String>, String)>,
    current: Option<(String, PendingTurn)>,
    saw_start: bool,
    saw_message: bool,
}

fn parse(content: &str, known_turns: &mut HistoryIndex) -> Result<Option<History>> {
    let mut rollout = Rollout::default();
    let mut turns = Vec::new();
    let mut seen = HashSet::new();
    let mut started = HashSet::new();
    let mut replayed = 0;
    let mut parent_turns = None;
    for (index, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        // Dropping a broken record could charge missing usage to the next turn.
        let entry: Value = serde_json::from_str(line)
            .with_context(|| format!("invalid rollout record at line {}", index + 1))?;
        let item = &entry["payload"];
        if entry["type"] == "session_meta" && rollout.session_id.is_none() {
            rollout.session_id = nonempty(item["id"].as_str());
            if let Some(parent) = item["forked_from_id"].as_str() {
                parent_turns =
                    Some(known_turns.sessions.get(parent).context(
                        "fork parent was not read; restore its rollout before importing",
                    )?);
                rollout.replaying = true;
            }
            continue;
        }
        if entry["type"] == "message" {
            rollout.saw_message = true;
        }
        match (entry["type"].as_str(), item["type"].as_str()) {
            (Some("turn_context"), _)
                if rollout.current.is_none() || rollout.owns(item["turn_id"].as_str()) =>
            {
                rollout.model = nonempty(item["model"].as_str());
                if let Some((_, pending)) = &mut rollout.current {
                    pending.model = rollout.model.clone();
                }
            }
            (Some("response_item"), Some("message")) => rollout.message(item),
            (Some("event_msg"), Some("token_count")) => {
                timestamp(&entry)?;
                if let Some(usage) = token_usage(&entry) {
                    rollout.usage = usage;
                    if let Some((_, pending)) = &mut rollout.current {
                        pending.usage_at_end = Some(usage);
                    }
                } else if !item["info"]["total_token_usage"].is_null()
                    || !item["total_token_usage"].is_null()
                {
                    bail!("invalid token usage at line {}", index + 1);
                }
            }
            (Some("event_msg"), Some("task_started")) => {
                let id = item["turn_id"]
                    .as_str()
                    .context("task_started has no turn_id")?;
                if rollout.replaying && parent_turns.is_some_and(|turns| !turns.contains(id)) {
                    // Copied records can be re-stamped with the fork's time.
                    // Turn IDs separate that history from restarted totals.
                    rollout.usage = TokenUsage::default();
                    rollout.replaying = false;
                }
                started.insert(id.to_string());
                rollout.start(item, timestamp(&entry)?)?;
            }
            (Some("event_msg"), Some("task_complete")) => {
                if let Some(turn) = rollout.complete(item, timestamp(&entry)?)
                    && seen.insert(turn.uuid.clone())
                {
                    if parent_turns.is_some() && known_turns.completed.contains(&turn.uuid) {
                        replayed += 1;
                    } else {
                        turns.push(turn);
                    }
                }
            }
            (Some("event_msg"), Some("turn_aborted")) if rollout.owns(item["turn_id"].as_str()) => {
                rollout.current = None;
                rollout.next_prompt = None;
            }
            _ => {}
        }
    }
    let Some(session_id) = rollout.session_id else {
        return Ok(None);
    };
    if !rollout.saw_start && rollout.saw_message {
        return Ok(None);
    }
    known_turns.sessions.insert(session_id.clone(), started);
    known_turns.completed.extend(seen);
    Ok(Some(History {
        snapshot: SessionSnapshot {
            session_id,
            title: None,
            turns,
        },
        replayed,
    }))
}

impl Rollout {
    fn owns(&self, owner: Option<&str>) -> bool {
        owner.is_none_or(|owner| self.current.as_ref().is_some_and(|(id, _)| id == owner))
    }

    fn start(&mut self, item: &Value, at: DateTime<Utc>) -> Result<()> {
        let Some(id) = nonempty(item["turn_id"].as_str()) else {
            bail!("task_started has no turn_id");
        };
        self.saw_start = true;
        let prompt = self
            .next_prompt
            .take()
            .filter(|(owner, _)| owner.as_deref().is_none_or(|owner| owner == id))
            .map(|(_, text)| text);
        self.current = Some((
            id,
            PendingTurn {
                prompt,
                model: self.model.clone(),
                started_at: Some(at),
                usage_at_start: Some(self.usage),
                ..PendingTurn::default()
            },
        ));
        Ok(())
    }

    fn message(&mut self, item: &Value) {
        let Some(text) = message_text(item) else {
            return;
        };
        self.saw_message = true;
        let owner = item["internal_chat_message_metadata_passthrough"]["turn_id"].as_str();
        if item["role"] == "user" && (self.current.is_none() || !self.owns(owner)) {
            self.next_prompt = Some((owner.map(str::to_string), text));
            return;
        }
        if !self.owns(owner) {
            return;
        }
        match (item["role"].as_str(), &mut self.current) {
            (Some("assistant"), Some((_, pending))) => pending.response = Some(text),
            (Some("user"), Some((_, pending))) if pending.prompt.is_none() => {
                pending.prompt = Some(text)
            }
            _ => {}
        }
    }

    fn complete(&mut self, item: &Value, at: DateTime<Utc>) -> Option<super::Turn> {
        let owner = item["turn_id"].as_str()?;
        if !self.owns(Some(owner)) {
            return None;
        }
        let (id, mut pending) = self.current.take()?;
        pending.response = nonempty(item["last_agent_message"].as_str()).or(pending.response);
        pending.ended_at = Some(at);
        completed_turn(&pending, self.session_id.as_deref()?, &id)
    }
}

fn timestamp(entry: &Value) -> Result<DateTime<Utc>> {
    entry["timestamp"]
        .as_str()
        .context("rollout event has no timestamp")?
        .parse()
        .context("rollout event has an invalid timestamp")
}

fn message_text(message: &Value) -> Option<String> {
    let text = message["content"]
        .as_array()?
        .iter()
        .filter_map(|item| item["text"].as_str())
        .map(str::trim)
        .filter(|text| message["role"] != "user" || !is_context_block(text))
        .collect::<Vec<_>>()
        .join("\n");
    nonempty(Some(&text))
}

fn is_context_block(text: &str) -> bool {
    text.starts_with("# AGENTS.md instructions")
        || [
            "environment_context",
            "user_instructions",
            "in-app-browser-context",
            "permissions instructions",
            "collaboration_mode",
        ]
        .iter()
        .any(|tag| text.starts_with(&format!("<{tag}>")) && text.ends_with(&format!("</{tag}>")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(content: &str) -> SessionSnapshot {
        parse(content, &mut HistoryIndex::default())
            .unwrap()
            .unwrap()
            .snapshot
    }

    #[test]
    fn tagged_turns_keep_their_text_and_matching_completion() {
        let snapshot = fixture(include_str!("fixtures/current.jsonl"));
        assert_eq!(snapshot.turns.len(), 2);
        let first = &snapshot.turns[0];
        assert_eq!(first.prompt, "<request>Fix the build</request>");
        assert_eq!(
            first.response.as_deref(),
            Some("<environment_context>Tagged answer</environment_context>")
        );
        assert_eq!(first.ended_at.to_rfc3339(), "2026-03-01T10:00:06+00:00");
        assert_eq!(first.calls[0].uuid, "current:t1:llm");
    }

    #[test]
    fn cumulative_usage_is_differenced_once_per_turn_including_cache() {
        let snapshot = fixture(include_str!("fixtures/current.jsonl"));
        let counts: Vec<_> = snapshot
            .turns
            .iter()
            .map(|turn| {
                let call = &turn.calls[0];
                (
                    call.input_tokens,
                    call.cache_read_tokens,
                    call.output_tokens,
                )
            })
            .collect();
        assert_eq!(counts, [(180, 120, 20), (50, 30, 10)]);
    }

    #[test]
    fn older_prompt_order_and_response_fallback_are_supported() {
        let snapshot = fixture(include_str!("fixtures/older.jsonl"));
        let turn = &snapshot.turns[0];
        assert_eq!(turn.prompt, "Explain the test");
        assert_eq!(turn.response.as_deref(), Some("Explanation"));
        assert_eq!(turn.calls[0].input_tokens, 60);
        assert_eq!(turn.calls[0].model, "gpt-test");
    }

    #[test]
    fn fork_totals_restart_even_when_larger_than_the_parent() {
        let mut known = HistoryIndex::default();
        parse(include_str!("fixtures/parent.jsonl"), &mut known).unwrap();
        let history = parse(include_str!("fixtures/fork.jsonl"), &mut known)
            .unwrap()
            .unwrap();
        assert_eq!(history.replayed, 1);
        let snapshot = history.snapshot;
        let counts: Vec<_> = snapshot
            .turns
            .iter()
            .map(|turn| {
                let call = &turn.calls[0];
                (
                    call.input_tokens,
                    call.cache_read_tokens,
                    call.output_tokens,
                )
            })
            .collect();
        assert_eq!(counts, [(200, 160, 20), (50, 30, 5)]);
    }

    #[test]
    fn a_missing_fork_parent_fails_instead_of_guessing_its_usage_boundary() {
        assert!(
            parse(
                include_str!("fixtures/fork.jsonl"),
                &mut HistoryIndex::default()
            )
            .is_err()
        );
    }

    #[test]
    fn aborted_parent_turns_still_identify_the_copied_prefix() {
        let parent = include_str!("fixtures/parent.jsonl").replace("task_complete", "turn_aborted");
        let fork = include_str!("fixtures/fork.jsonl").replacen("task_complete", "turn_aborted", 1);
        let mut known = HistoryIndex::default();
        let parent = parse(&parent, &mut known).unwrap().unwrap();
        assert!(parent.snapshot.turns.is_empty());
        let fork = parse(&fork, &mut known).unwrap().unwrap();
        assert_eq!(fork.snapshot.turns[0].calls[0].input_tokens, 200);
    }

    #[test]
    fn aborted_usage_is_not_charged_to_the_resumed_turn() {
        let snapshot = fixture(include_str!("fixtures/abort-resume.jsonl"));
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(snapshot.turns[0].uuid, "resumed");
        assert_eq!(snapshot.turns[0].calls[0].input_tokens, 50);
    }

    #[test]
    fn legacy_messages_are_unsupported_but_empty_sessions_are_not() {
        assert!(
            parse(
                include_str!("fixtures/legacy.jsonl"),
                &mut HistoryIndex::default()
            )
            .unwrap()
            .is_none()
        );
        assert!(
            fixture(include_str!("fixtures/empty.jsonl"))
                .turns
                .is_empty()
        );
    }

    #[test]
    fn malformed_gaps_fail_instead_of_reassigning_usage() {
        let content = include_str!("fixtures/current.jsonl");
        let broken = content.replacen("\n", "\n{broken\n", 7);
        assert!(parse(&broken, &mut HistoryIndex::default()).is_err());
        assert!(
            parse(
                &content.replace("\"input_tokens\": 100", "\"input_tokens\": \"broken\""),
                &mut HistoryIndex::default()
            )
            .is_err()
        );
    }

    #[test]
    fn an_owned_prompt_can_precede_its_start_event() {
        let content = include_str!("fixtures/current.jsonl");
        let mut lines: Vec<_> = content.lines().collect();
        lines.swap(5, 7);
        let snapshot = fixture(&lines.join("\n"));
        assert_eq!(snapshot.turns.len(), 2);
        assert_eq!(snapshot.turns[0].uuid, "t1");
    }

    #[test]
    fn repeated_turn_records_count_once() {
        let content = include_str!("fixtures/current.jsonl");
        let snapshot = fixture(&format!("{content}{content}"));
        assert_eq!(snapshot.turns.len(), 2);
    }

    #[test]
    fn codex_subagents_and_auto_reviews_are_usage() {
        let content = include_str!("fixtures/older.jsonl")
            .replace(
                "\"id\": \"older\"",
                "\"id\": \"review\", \"source\": {\"subagent\": \"review\"}",
            )
            .replace("gpt-test", "codex-auto-review");
        let snapshot = fixture(&content);
        assert_eq!(snapshot.session_id, "review");
        assert_eq!(snapshot.turns[0].calls[0].model, "codex-auto-review");
    }
}
