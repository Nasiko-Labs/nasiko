//! What the queue still holds: turns waiting to be delivered and turns set aside (`rejected/`), counted
//! per agent for progress reports, and the re-drive that gives set-aside turns another go.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use super::queue::{self, DeliveryState, QueueRecord};
use super::sanitize::sanitized;

/// At most this many distinct reasons are reported per agent; the rest are left out of the list (but
/// not the count).
pub const MAX_REASONS: usize = 5;
/// A reason is cut to this many characters.
pub const REASON_MAX_CHARS: usize = 200;

/// One agent's share of the queue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentBacklog {
    /// Turns waiting to be delivered.
    pub pending: usize,
    /// Turns set aside.
    pub rejected: usize,
    /// Why they were set aside, most common first, at most [`MAX_REASONS`].
    pub reasons: Vec<(String, usize)>,
}

/// The queue as it stands, by agent id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backlog {
    pub agents: BTreeMap<String, AgentBacklog>,
    /// Waiting turns due an attempt now (never tried, or their retry time has come).
    pub due: usize,
    /// The soonest a waiting turn is due another attempt; `None` when none is waiting on a retry.
    pub next_attempt_at: Option<DateTime<Utc>>,
}

impl Backlog {
    pub fn pending(&self) -> usize {
        self.agents.values().map(|agent| agent.pending).sum()
    }

    pub fn agent(&self, agent_id: &str) -> AgentBacklog {
        self.agents.get(agent_id).cloned().unwrap_or_default()
    }
}

/// Just what counting needs from a record, so a turn's content is skipped rather than kept.
#[derive(Deserialize)]
struct RecordHeader {
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    next_attempt_at: Option<DateTime<Utc>>,
    event: EventHeader,
}

#[derive(Deserialize)]
struct EventHeader {
    source: SourceHeader,
}

#[derive(Deserialize)]
struct SourceHeader {
    agent_id: String,
}

/// Counts the queue under `root`. A record that can't be read is not counted (the next delivery
/// quarantines it).
pub(super) fn backlog_at(root: &Path) -> Result<Backlog> {
    let mut backlog = Backlog::default();
    let now = Utc::now();
    let mut pauses: HashMap<PathBuf, Option<DateTime<Utc>>> = HashMap::new();
    for (path, header) in headers(&root.join("queue"))? {
        backlog
            .agents
            .entry(header.event.source.agent_id)
            .or_default()
            .pending += 1;
        let destination_dir = path.parent().unwrap_or(root).to_path_buf();
        let paused = *pauses
            .entry(destination_dir)
            .or_insert_with_key(|dir| queue::paused_until(dir, now));
        let next_attempt_at = header.next_attempt_at.max(paused);
        match next_attempt_at.filter(|at| *at > now) {
            Some(at) => {
                backlog.next_attempt_at = Some(
                    backlog
                        .next_attempt_at
                        .map_or(at, |soonest| soonest.min(at)),
                );
            }
            None => backlog.due += 1,
        }
    }
    let mut reasons: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (_, header) in headers(&root.join("rejected"))? {
        let agent_id = header.event.source.agent_id;
        backlog.agents.entry(agent_id.clone()).or_default().rejected += 1;
        let reason = header.last_error.unwrap_or_else(|| "rejected".into());
        *reasons
            .entry(agent_id)
            .or_default()
            .entry(reason.chars().take(REASON_MAX_CHARS).collect())
            .or_default() += 1;
    }
    for (agent_id, counts) in reasons {
        let mut counts: Vec<(String, usize)> = counts.into_iter().collect();
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        counts.truncate(MAX_REASONS);
        backlog.agents.entry(agent_id).or_default().reasons = counts;
    }
    Ok(backlog)
}

fn headers(dir: &Path) -> Result<Vec<(PathBuf, RecordHeader)>> {
    Ok(queue::record_files(dir)?
        .into_iter()
        .filter_map(|path| {
            let header = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            Some((path, header))
        })
        .collect())
}

/// What a re-drive did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Requeued {
    /// Set-aside turns back in the queue, as if just captured.
    pub requeued: usize,
    /// Turns still invalid after cleaning, left set aside.
    pub invalid: usize,
}

/// Moves every set-aside turn back into the queue with its attempts reset, cleaning its content first
/// (a turn set aside for a NUL character is valid once that is gone). One still invalid stays set
/// aside with the reason.
pub fn requeue_rejected() -> Result<Requeued> {
    requeue_rejected_at(&super::state::integrations_dir())
}

pub(super) fn requeue_rejected_at(root: &Path) -> Result<Requeued> {
    let mut outcome = Requeued::default();
    for path in queue::record_files(&root.join("rejected"))? {
        let Ok(stored) = std::fs::read(&path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(serde_json::from_slice::<QueueRecord>(&bytes)?))
        else {
            continue;
        };
        let record = fresh(stored);
        match record.event.validate() {
            Ok(()) => {
                queue::enqueue_at(root, &record)?;
                std::fs::remove_file(&path)
                    .with_context(|| format!("failed to remove {}", path.display()))?;
                outcome.requeued += 1;
            }
            Err(error) => {
                let mut still = record;
                still.delivery_state = DeliveryState::Rejected;
                still.last_error = Some(format!("invalid event: {error}"));
                queue::atomic_owner_write(&path, &serde_json::to_vec(&still)?)?;
                outcome.invalid += 1;
            }
        }
    }
    Ok(outcome)
}

/// `record` as a turn captured just now: cleaned, pending, never attempted. Its age restarts too, so
/// the 30 days a turn may wait undelivered count from the re-drive.
fn fresh(record: QueueRecord) -> QueueRecord {
    let now = Utc::now();
    QueueRecord {
        delivery_state: DeliveryState::Pending,
        last_error: None,
        created_at: now,
        updated_at: now,
        attempts: 0,
        next_attempt_at: None,
        event: sanitized(record.event),
        ..record
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::integration::queue::QueueDestination;
    use chrono::TimeZone;
    use nasiko_types::{
        CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentEventV1, CodingAgentSession,
        CodingAgentSource, CodingAgentTurn, coding_agent_event_id, coding_agent_session_id,
    };
    use uuid::Uuid;

    fn record(agent_id: &str, turn: &str) -> QueueRecord {
        let at = Utc.timestamp_opt(1, 0).unwrap();
        QueueRecord::new(
            QueueDestination {
                cluster_name: "one".into(),
                cluster_url: "https://one.example".into(),
                principal_id: Uuid::nil(),
            },
            CodingAgentEventV1 {
                version: CODING_AGENT_EVENT_VERSION,
                event_id: coding_agent_event_id(agent_id, "s", turn),
                captured_at: at,
                source: CodingAgentSource {
                    agent_id: agent_id.into(),
                    agent_name: agent_id.into(),
                },
                session: CodingAgentSession {
                    id: coding_agent_session_id(agent_id, "s"),
                    source_id: "s".into(),
                    title: None,
                    parent: None,
                },
                turn: CodingAgentTurn {
                    id: turn.into(),
                    prompt: Some("ask".into()),
                    response: Some("answer".into()),
                    started_at: at,
                    ended_at: at,
                    llm_calls: vec![],
                    tool_calls: vec![],
                },
                capture_policy: CapturePolicy::Content,
            },
        )
    }

    fn set_aside(root: &Path, record: &QueueRecord, reason: &str) {
        let path = queue::enqueue_at(root, record).unwrap();
        let mut rejected = record.clone();
        rejected.delivery_state = DeliveryState::Rejected;
        rejected.last_error = Some(reason.into());
        rejected.attempts = 7;
        queue::update(&path, &rejected).unwrap();
        queue::quarantine(&path, &rejected).unwrap();
    }

    #[test]
    fn the_backlog_counts_waiting_and_set_aside_turns_per_agent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        queue::enqueue_at(root, &record("claude", "a")).unwrap();
        let mut later = record("claude", "b");
        later.next_attempt_at = Some(Utc::now() + chrono::Duration::minutes(2));
        queue::enqueue_at(root, &later).unwrap();
        set_aside(root, &record("codex", "c"), "permanently rejected: too big");
        set_aside(root, &record("codex", "d"), "permanently rejected: too big");
        set_aside(
            root,
            &record("codex", "e"),
            "permanently rejected: bad time",
        );

        let backlog = backlog_at(root).unwrap();

        assert_eq!((backlog.pending(), backlog.due), (2, 1));
        assert_eq!(backlog.agent("claude").pending, 2);
        assert_eq!(backlog.next_attempt_at, later.next_attempt_at);
        let codex = backlog.agent("codex");
        assert_eq!((codex.pending, codex.rejected), (0, 3));
        assert_eq!(
            codex.reasons,
            [
                ("permanently rejected: too big".to_string(), 2),
                ("permanently rejected: bad time".to_string(), 1)
            ]
        );
    }

    #[test]
    fn reasons_are_capped_in_number_and_length() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..MAX_REASONS + 2 {
            set_aside(
                dir.path(),
                &record("claude", &format!("t{i}")),
                &format!("{i}{}", "x".repeat(REASON_MAX_CHARS)),
            );
        }

        let claude = backlog_at(dir.path()).unwrap().agent("claude");

        assert_eq!(claude.rejected, MAX_REASONS + 2);
        assert_eq!(claude.reasons.len(), MAX_REASONS);
        assert!(
            claude
                .reasons
                .iter()
                .all(|(reason, _)| reason.chars().count() == REASON_MAX_CHARS)
        );
    }

    #[test]
    fn retry_rejected_requeues_cleaned_turns_with_attempts_reset() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Set aside when it carried a NUL; cleaned, it is valid.
        let mut dirty = record("claude", "a");
        dirty.event.turn.prompt = Some("a\0sk".into());
        let rejected_dir = root.join("rejected").join("x");
        std::fs::create_dir_all(&rejected_dir).unwrap();
        let mut stored = dirty.clone();
        stored.delivery_state = DeliveryState::Rejected;
        stored.attempts = 9;
        stored.last_error = Some("invalid event: NUL".into());
        std::fs::write(
            rejected_dir.join(format!("{}.json", dirty.event.event_id)),
            serde_json::to_vec(&stored).unwrap(),
        )
        .unwrap();
        // Still invalid however it is cleaned.
        let mut hopeless = record("claude", "b");
        hopeless.event.turn.response = Some("\0".into());
        std::fs::write(
            rejected_dir.join(format!("{}.json", hopeless.event.event_id)),
            serde_json::to_vec(&hopeless).unwrap(),
        )
        .unwrap();

        let outcome = requeue_rejected_at(root).unwrap();

        assert_eq!(
            outcome,
            Requeued {
                requeued: 1,
                invalid: 1
            }
        );
        let queued = queue::queued_ids_at(root).unwrap();
        assert!(queued.contains(&dirty.event.event_id));
        let path = queue::record_files(&root.join("queue")).unwrap().remove(0);
        let requeued = queue::load(&path).unwrap();
        assert_eq!(requeued.attempts, 0);
        assert_eq!(requeued.delivery_state, DeliveryState::Pending);
        assert_eq!(requeued.event.turn.prompt.as_deref(), Some("ask"));
        let still = backlog_at(root).unwrap().agent("claude");
        assert_eq!((still.pending, still.rejected), (1, 1));
        assert!(still.reasons[0].0.contains("nonempty"));
    }

    #[test]
    fn retry_rejected_repairs_turns_set_aside_for_accounting_slips() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut slipped = record("claude", "a");
        let at = slipped.event.turn.started_at;
        slipped.event.turn.llm_calls = vec![nasiko_types::CodingAgentLlmCall {
            id: "call".into(),
            provider: "anthropic".into(),
            model: "claude-test".into(),
            input_tokens: 1,
            output_tokens: 1,
            cache_read_tokens: 0,
            cache_creation_tokens: 10,
            accounting: Some(nasiko_types::CodingAgentCallAccounting {
                version: 2,
                cache_creation_5m_tokens: Some(8),
                cache_creation_1h_tokens: Some(8),
                ..Default::default()
            }),
            started_at: at,
            ended_at: at,
        }];
        let reason = "invalid event: cache creation TTL counts exceed aggregate creation";
        let rejected_dir = root.join("rejected").join("x");
        std::fs::create_dir_all(&rejected_dir).unwrap();
        let mut stored = slipped.clone();
        stored.delivery_state = DeliveryState::Rejected;
        stored.last_error = Some(reason.into());
        std::fs::write(
            rejected_dir.join(format!("{}.json", slipped.event.event_id)),
            serde_json::to_vec(&stored).unwrap(),
        )
        .unwrap();

        let outcome = requeue_rejected_at(root).unwrap();

        assert_eq!(outcome.requeued, 1);
        let path = queue::record_files(&root.join("queue")).unwrap().remove(0);
        assert_eq!(queue::load(&path).unwrap().event.validate(), Ok(()));
    }
}
