//! One device importing its history the way the real client does
//! (`oss/cli/src/commands/integration/{sync,pace}.rs`):
//!
//! - one batch in flight, at most 100 turns, sent with a 10 s timeout;
//! - `429`/`503` with `Retry-After`: wait exactly that long plus jitter (a tenth
//!   of the wait, at least 1 s, at most 30 s), then send the same batch;
//! - `413`: halve the batch and send again; slow answers (> 5 s) halve it, three
//!   fast ones in a row double it back toward 100;
//! - transport errors, timeouts and other `5xx`: send the same batch again after
//!   a backoff. The CLI defers such turns for a minute, doubling; this client
//!   starts at one second so a run finishes — the resend is what matters, and
//!   idempotency by event id is what makes it safe.
//!
//! On top of that it re-sends a small share of batches that were already
//! answered `200`, as a client whose response was lost would, and checks every
//! turn comes back `duplicate`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use nasiko_types::{CODING_AGENT_BATCH_MAX_EVENTS, CodingAgentEventV1};
use rand::Rng;
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::Value;

use super::history::History;

const BATCH_PATH: &str = "/api/telemetry/coding-agent/events/batch";
/// The CLI's `BATCH_TIMEOUT`.
const BATCH_TIMEOUT: Duration = Duration::from_secs(10);
/// The CLI's `SLOW_RESPONSE` and `FAST_ANSWERS_TO_GROW`.
const SLOW_RESPONSE: Duration = Duration::from_secs(5);
const FAST_ANSWERS_TO_GROW: u32 = 3;
const FIRST_RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(30);
/// The CLI's `BATCH_BYTE_BUDGET`: a batch is cut to fit well under the server's
/// 8 MiB body limit before it is sent. A server over its limit closes the
/// connection without reading the body, so an oversized batch would surface as
/// a broken pipe, not a `413`.
const BATCH_BYTE_BUDGET: usize = 6 * 1024 * 1024;
/// Consecutive failed attempts after which this import is declared failed.
const GIVE_UP_AFTER: u32 = 40;
/// Rejection reasons kept for the report.
const KEPT_ERRORS: usize = 10;

pub struct Device {
    pub index: usize,
    pub base_url: String,
    pub token: String,
    pub resend_share: f64,
}

/// What one import did, as the client saw it.
#[derive(Debug, Default)]
pub struct ImportStats {
    pub generated: usize,
    pub accepted: u64,
    pub duplicate: u64,
    pub rejected: u64,
    pub errors: Vec<String>,
    /// Every event id answered `accepted` or `duplicate` at least once.
    pub acknowledged: HashSet<String>,
    pub requests: u64,
    pub status_429: u64,
    pub status_503: u64,
    pub status_413: u64,
    pub timeouts: u64,
    pub transport_errors: u64,
    pub server_errors: u64,
    pub waited_429: Duration,
    pub waited_503: Duration,
    pub resends: u64,
    /// Turns a deliberate resend got back as anything but `duplicate`.
    pub resend_not_duplicate: u64,
    pub smallest_batch: usize,
    /// Batches cut short by [`BATCH_BYTE_BUDGET`] rather than by the pace.
    pub byte_capped: u64,
    pub batch_ms: Vec<u32>,
    pub elapsed: Duration,
    pub failure: Option<String>,
    /// When each live turn was first acknowledged.
    pub live_acked_at: HashMap<String, DateTime<Utc>>,
}

#[derive(Serialize)]
struct Batch<'a> {
    events: &'a [CodingAgentEventV1],
}

enum Answer {
    Results(Vec<(String, String, Option<String>)>),
    Wait(StatusCode, Duration),
    TooLarge,
    Retry(RetryKind),
    Fatal(String),
}

enum RetryKind {
    Timeout,
    Transport,
    Server,
}

/// The CLI's adaptive batch size.
struct Pace {
    size: usize,
    fast: u32,
}

impl Pace {
    fn answered_in(&mut self, elapsed: Duration) {
        if elapsed > SLOW_RESPONSE {
            self.size = (self.size / 2).max(1);
            self.fast = 0;
            return;
        }
        self.fast += 1;
        if self.fast >= FAST_ANSWERS_TO_GROW {
            self.size = (self.size * 2).min(CODING_AGENT_BATCH_MAX_EVENTS);
            self.fast = 0;
        }
    }
}

/// Delivers every turn of `history`; hands it back with what it expects the server to hold.
pub async fn import(
    http: &reqwest::Client,
    device: &Device,
    mut history: History,
) -> (ImportStats, History) {
    let started = Instant::now();
    let mut stats = ImportStats {
        generated: history.total(),
        smallest_batch: CODING_AGENT_BATCH_MAX_EVENTS,
        ..ImportStats::default()
    };
    let mut pace = Pace {
        size: CODING_AGENT_BATCH_MAX_EVENTS,
        fast: 0,
    };
    let mut pending: Vec<CodingAgentEventV1> = Vec::new();
    let mut failures = 0_u32;
    loop {
        if pending.len() < pace.size {
            pending.extend(history.next_turns(pace.size - pending.len()));
        }
        if pending.is_empty() {
            break;
        }
        let n = batch_len(&pending, pace.size);
        if n < pace.size.min(pending.len()) {
            stats.byte_capped += 1;
        }
        stats.smallest_batch = stats.smallest_batch.min(n);
        let sent_at = Instant::now();
        let answer = send(http, device, &pending[..n]).await;
        stats.requests += 1;
        match answer {
            Answer::Results(results) => {
                let elapsed = sent_at.elapsed();
                stats.batch_ms.push(elapsed.as_millis() as u32);
                pace.answered_in(elapsed);
                record(&mut stats, &pending[..n], &results, &history);
                if rand::rng().random_bool(device.resend_share) {
                    resend(http, device, &pending[..n], &mut stats).await;
                }
                pending.drain(..n);
                failures = 0;
            }
            Answer::Wait(status, wait) => {
                let pause = wait + jitter(wait);
                if status == StatusCode::TOO_MANY_REQUESTS {
                    stats.status_429 += 1;
                    stats.waited_429 += pause;
                } else {
                    stats.status_503 += 1;
                    stats.waited_503 += pause;
                }
                tokio::time::sleep(pause).await;
            }
            Answer::TooLarge => {
                stats.status_413 += 1;
                if n == 1 {
                    stats.rejected += 1;
                    keep_error(&mut stats, "single turn refused as too large".into());
                    pending.drain(..1);
                } else {
                    pace.size = (n / 2).max(1);
                    pace.fast = 0;
                }
            }
            Answer::Retry(kind) => {
                match kind {
                    RetryKind::Timeout => stats.timeouts += 1,
                    RetryKind::Transport => stats.transport_errors += 1,
                    RetryKind::Server => stats.server_errors += 1,
                }
                failures += 1;
                if failures >= GIVE_UP_AFTER {
                    stats.failure = Some(format!("gave up after {failures} failed attempts"));
                    break;
                }
                tokio::time::sleep(backoff(failures)).await;
            }
            Answer::Fatal(reason) => {
                stats.failure = Some(reason);
                break;
            }
        }
    }
    stats.elapsed = started.elapsed();
    (stats, history)
}

/// How many of `pending` the next batch takes: at most `max` turns and at most
/// [`BATCH_BYTE_BUDGET`] bytes of events, but always at least one (the CLI's `batch_len`).
fn batch_len(pending: &[CodingAgentEventV1], max: usize) -> usize {
    let mut bytes = 0_usize;
    let mut len = 0_usize;
    for event in pending.iter().take(max.max(1)) {
        bytes += serde_json::to_vec(event).map_or(0, |e| e.len());
        if len > 0 && bytes > BATCH_BYTE_BUDGET {
            break;
        }
        len += 1;
    }
    len.max(1)
}

fn record(
    stats: &mut ImportStats,
    batch: &[CodingAgentEventV1],
    results: &[(String, String, Option<String>)],
    history: &History,
) {
    let live = &history.expected.live_event_ids;
    let in_order =
        results.len() == batch.len() && results.iter().zip(batch).all(|(r, e)| r.0 == e.event_id);
    if !in_order {
        keep_error(
            stats,
            "response did not hold one ordered result per turn".into(),
        );
    }
    for (event_id, status, error) in results {
        match status.as_str() {
            "accepted" => stats.accepted += 1,
            "duplicate" => stats.duplicate += 1,
            _ => {
                stats.rejected += 1;
                keep_error(stats, error.clone().unwrap_or_else(|| status.clone()));
                continue;
            }
        }
        if stats.acknowledged.insert(event_id.clone()) && live.contains(event_id) {
            stats.live_acked_at.insert(event_id.clone(), Utc::now());
        }
    }
}

/// Sends an answered batch again, as a client that never saw the answer would.
async fn resend(
    http: &reqwest::Client,
    device: &Device,
    batch: &[CodingAgentEventV1],
    stats: &mut ImportStats,
) {
    stats.requests += 1;
    // A resend turned away by the server's valves is simply not checked.
    if let Answer::Results(results) = send(http, device, batch).await {
        stats.resends += 1;
        stats.resend_not_duplicate += results.iter().filter(|r| r.1 != "duplicate").count() as u64;
    }
}

async fn send(http: &reqwest::Client, device: &Device, batch: &[CodingAgentEventV1]) -> Answer {
    let response = http
        .post(format!("{}{BATCH_PATH}", device.base_url))
        .bearer_auth(&device.token)
        .timeout(BATCH_TIMEOUT)
        .json(&Batch { events: batch })
        .send()
        .await;
    let response = match response {
        Ok(response) => response,
        Err(error) if error.is_timeout() => return Answer::Retry(RetryKind::Timeout),
        Err(_) => return Answer::Retry(RetryKind::Transport),
    };
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(error) if error.is_timeout() => return Answer::Retry(RetryKind::Timeout),
        Err(_) => return Answer::Retry(RetryKind::Transport),
    };
    match (status.as_u16(), retry_after) {
        (200, _) => parse_results(&body),
        (429 | 503, Some(secs)) => Answer::Wait(status, Duration::from_secs(secs)),
        (413, _) => Answer::TooLarge,
        (408 | 429 | 500..=599, _) => Answer::Retry(RetryKind::Server),
        (code, _) => Answer::Fatal(format!(
            "HTTP {code}: {}",
            String::from_utf8_lossy(&body)
                .chars()
                .take(300)
                .collect::<String>()
        )),
    }
}

fn parse_results(body: &[u8]) -> Answer {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return Answer::Retry(RetryKind::Server);
    };
    let Some(results) = value.pointer("/data/results").and_then(Value::as_array) else {
        return Answer::Retry(RetryKind::Server);
    };
    Answer::Results(
        results
            .iter()
            .map(|r| {
                let field = |name: &str| r.get(name).and_then(Value::as_str).map(str::to_owned);
                (
                    field("event_id").unwrap_or_default(),
                    field("status").unwrap_or_default(),
                    field("error"),
                )
            })
            .collect(),
    )
}

/// Up to a tenth of the wait, at least 1 s and at most 30 s (the CLI's `jitter`).
fn jitter(wait: Duration) -> Duration {
    let bound = (wait.as_millis() as u64 / 10).clamp(1_000, 30_000);
    Duration::from_millis(rand::rng().random_range(0..=bound))
}

fn backoff(failures: u32) -> Duration {
    FIRST_RETRY
        .saturating_mul(1 << failures.saturating_sub(1).min(10))
        .min(MAX_RETRY)
}

fn keep_error(stats: &mut ImportStats, error: String) {
    if stats.errors.len() < KEPT_ERRORS {
        stats.errors.push(error);
    }
}

/// A single turn bigger than the server's 8 MiB body limit (nine 1 MiB tool
/// outputs), sent alone — no client can split it. Passes when the server turns
/// it away (`413` or a closed connection) and is still healthy afterwards; the
/// detail records which the client saw, since a closed connection is what a
/// client cannot tell apart from a network fault.
pub async fn oversized_turn(
    http: &reqwest::Client,
    base: &str,
    user: &super::setup::User,
) -> super::verify::Check {
    let event = oversized_event(&user.agent_name);
    let size = serde_json::to_vec(&event).map_or(0, |e| e.len());
    let device = Device {
        index: user.index,
        base_url: base.to_owned(),
        token: user.token.clone(),
        resend_share: 0.0,
    };
    let seen = match send(http, &device, std::slice::from_ref(&event)).await {
        Answer::TooLarge => "HTTP 413".to_owned(),
        Answer::Retry(RetryKind::Transport) => "connection closed (broken pipe / reset)".into(),
        Answer::Retry(RetryKind::Timeout) => "timeout".into(),
        Answer::Retry(RetryKind::Server) => "HTTP 5xx".into(),
        Answer::Wait(status, _) => format!("HTTP {status}"),
        Answer::Results(results) => format!("HTTP 200 {:?}", results.first().map(|r| &r.1)),
        Answer::Fatal(reason) => reason,
    };
    let healthy = http
        .get(format!("{base}/health"))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success());
    let refused = seen == "HTTP 413" || seen.starts_with("connection closed");
    super::verify::Check {
        name: "single turn over 8 MiB is refused and the server stays healthy",
        pass: refused && healthy,
        detail: format!(
            "{:.1} MiB turn → {seen}; server healthy afterwards: {healthy}",
            size as f64 / 1_048_576.0
        ),
    }
}

fn oversized_event(agent_name: &str) -> CodingAgentEventV1 {
    use nasiko_types::{
        CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentLlmCall, CodingAgentSession,
        CodingAgentSource, CodingAgentTimestampQuality, CodingAgentToolAssociation,
        CodingAgentToolCall, CodingAgentToolCallStatus, CodingAgentTurn, coding_agent_event_id,
        coding_agent_session_id,
    };
    const TOOLS: usize = 9;
    const OUTPUT_BYTES: usize = 1_000_000;
    let source_id = "lt-oversized-session";
    let at = Utc::now() - chrono::Duration::days(2);
    let call = CodingAgentLlmCall {
        id: "msg_oversized".into(),
        provider: super::history::PROVIDER.into(),
        model: super::history::MAIN_MODEL.into(),
        input_tokens: 10,
        output_tokens: 10,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        accounting: None,
        started_at: at,
        ended_at: at,
    };
    let tools = (0..TOOLS)
        .map(|i| CodingAgentToolCall {
            id: format!("toolu_oversized_{i}"),
            name: "Bash".into(),
            kind: "builtin".into(),
            model_call_id: Some(call.id.clone()),
            status: CodingAgentToolCallStatus::Succeeded,
            arguments: None,
            output: Some(Value::String("x".repeat(OUTPUT_BYTES))),
            raw: None,
            error: None,
            started_at: Some(at),
            ended_at: Some(at),
            duration_ms: Some(0),
            association: CodingAgentToolAssociation::Exact,
            timestamp_quality: CodingAgentTimestampQuality::Exact,
        })
        .collect();
    let integration = super::history::INTEGRATION_ID;
    CodingAgentEventV1 {
        version: CODING_AGENT_EVENT_VERSION,
        event_id: coding_agent_event_id(integration, source_id, "turn-oversized"),
        captured_at: at,
        source: CodingAgentSource {
            agent_id: integration.into(),
            agent_name: agent_name.into(),
        },
        session: CodingAgentSession {
            id: coding_agent_session_id(integration, source_id),
            source_id: source_id.into(),
            title: None,
            parent: None,
            place: None,
        },
        turn: CodingAgentTurn {
            id: "turn-oversized".into(),
            prompt: Some("dump the build log".into()),
            response: Some("done".into()),
            started_at: at,
            ended_at: at,
            llm_calls: vec![call],
            tool_calls: tools,
        },
        capture_policy: CapturePolicy::Content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_are_cut_to_the_byte_budget_but_never_empty() {
        let shape = super::super::history::Shape {
            turns: 300,
            live_turns: 0,
            days: 90,
            seed: 3,
            bulky_turns: 8,
            now: Utc::now(),
        };
        let mut history = History::plan("t", 0, "n", shape, Default::default());
        let turns = history.next_turns(300);
        let bulky = turns
            .iter()
            .position(|e| e.turn.prompt.as_ref().is_some_and(|p| p.len() > 500_000))
            .expect("one session starts with bulky turns");
        let n = batch_len(&turns[bulky..], 100);
        assert!(n < 8, "eight bulky turns do not fit one batch: {n}");
        let bytes: usize = turns[bulky..bulky + n]
            .iter()
            .map(|e| serde_json::to_vec(e).unwrap().len())
            .sum();
        assert!(bytes <= BATCH_BYTE_BUDGET);
        assert_eq!(batch_len(&[oversized_event("n")], 100), 1);
        assert!(oversized_event("n").validate().is_ok());
    }

    #[test]
    fn pace_halves_on_slow_and_grows_after_three_fast() {
        let mut pace = Pace { size: 100, fast: 0 };
        pace.answered_in(Duration::from_secs(6));
        assert_eq!(pace.size, 50);
        for _ in 0..3 {
            pace.answered_in(Duration::from_millis(100));
        }
        assert_eq!(pace.size, 100);
    }

    #[test]
    fn jitter_and_backoff_stay_in_bounds() {
        assert!(jitter(Duration::from_secs(2)) <= Duration::from_secs(1));
        assert!(jitter(Duration::from_secs(600)) <= Duration::from_secs(30));
        assert_eq!(backoff(1), Duration::from_secs(1));
        assert_eq!(backoff(3), Duration::from_secs(4));
        assert_eq!(backoff(30), MAX_RETRY);
    }
}
