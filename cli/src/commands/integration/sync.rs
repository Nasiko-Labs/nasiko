//! Destination-bound delivery for queued coding-agent telemetry.
//!
//! A failed delivery is retried, never dropped, unless retrying can't help:
//!
//! | Failure | What happens to the batch's turns |
//! | --- | --- |
//! | HTTP 429 or 503 with `Retry-After` (the server is pacing its clients) | Every turn for that server waits exactly as long as it asked (plus a little jitter), no attempt counted |
//! | HTTP 413 (the batch is too large), or the connection dropped while sending a large batch | Sent again at once in halves, down to one turn; a single turn still refused with `413` is set aside |
//! | One turn larger than the server accepts at all ([`SINGLE_EVENT_MAX`]) | Set aside before it is sent |
//! | Network (connect, DNS, timeout), HTTP 5xx, 408, 429, or a reply that isn't one result per event | Retried after 1 min, then 2, 4 … up to 1 h, as often as it takes; set aside only once 30 days old |
//! | HTTP 401, 403, or this computer's sign-in no longer matches the turn's destination | Kept, no attempt counted, until the user signs in again |
//! | The server rejects an event, or any other HTTP 4xx for the batch | Set aside with the server's reason |
//!
//! Set-aside turns go to `rejected/`, from where `nasiko agents sync --retry-rejected` brings them back.
//!
//! One batch is in flight per computer at a time: every delivery (a hook's, an import's courier, a
//! `nasiko agents sync`) holds the queue's sync lock while it sends, one batch after another, at the
//! size [`Pace`] allows.
//!
//! A turn waiting on a retry waits out its delay, unless the user asked to send now
//! ([`Backoff::Ignore`]): then every queued turn gets one attempt at once, and a turn that fails again
//! waits as usual. A server's own `Retry-After` pause holds either way.

use anyhow::Result;
use chrono::{DateTime, Utc};
use nasiko_types::{CodingAgentEventBatchRequest, CodingAgentEventStatus};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

use super::pace;
pub use super::pace::Pace;
pub use super::queue::Backoff;
use super::queue::{self, DeliveryState, QueueDestination, QueueRecord};
use crate::api::Attempt;
use crate::config::{ClusterEntry, Config};

/// How long a delivery waits for another one (a hook's) to finish before leaving the turns to it.
const LOCK_WAIT: Duration = Duration::from_millis(250);
/// The largest request body the server reads (`8 MiB`). Over it, the server answers `413` but closes
/// the connection before reading the body, so the client sees a broken pipe, not the `413`.
const SERVER_BODY_LIMIT: usize = 8 * 1024 * 1024;
/// Bytes of events one batch may carry: under [`SERVER_BODY_LIMIT`] with room for the request
/// envelope.
const BATCH_BYTE_BUDGET: usize = 6 * 1024 * 1024;
/// Room left in a request for what wraps its events (`{"events":[...]}`, commas): far more than it
/// takes.
const ENVELOPE_HEADROOM: usize = 64 * 1024;
/// The largest one turn can be and still be sent. A larger one can never be delivered (the server
/// drops the connection, which looks like a network failure and would be retried forever), so it is
/// set aside instead of being sent.
const SINGLE_EVENT_MAX: usize = SERVER_BODY_LIMIT - ENVELOPE_HEADROOM;
/// A batch at least this large whose connection breaks while it is sent is taken as refused for its
/// size, for a server whose limit is lower than [`SERVER_BODY_LIMIT`]: it is sent again in halves.
const LARGE_BATCH_BYTES: usize = 1024 * 1024;
const MIB: f64 = 1024.0 * 1024.0;
/// Bounds each batch request.
const BATCH_TIMEOUT: Duration = Duration::from_secs(10);
/// The first retry after a transient failure; each further failure doubles it, up to [`MAX_RETRY_SECS`].
const FIRST_RETRY_SECS: i64 = 60;
const MAX_RETRY_SECS: i64 = 60 * 60;
/// A turn still undelivered this long after it was queued is set aside.
const MAX_UNDELIVERED_DAYS: i64 = 30;
const EXPIRED_REASON: &str = "expired after 30 days undelivered";
/// A run waits for this many empty scans in a row before it stops, so turns a hook is writing as it
/// starts are delivered too.
const QUIET_SCANS: u32 = 2;
const QUIET_SCAN_INTERVAL: Duration = Duration::from_millis(100);

/// What one delivery did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pass {
    /// Turns the server took (accepted, or already had).
    pub delivered: usize,
    /// Turns set aside.
    pub set_aside: usize,
    /// Why the turns that are left can't be delivered until the user acts (signed out, or signed in as
    /// someone else); `None` when they are only waiting for a retry.
    pub blocked: Option<String>,
}

impl Pass {
    fn progressed(&self) -> bool {
        self.delivered + self.set_aside > 0
    }

    fn absorb(&mut self, other: Pass) {
        self.delivered += other.delivered;
        self.set_aside += other.set_aside;
        self.blocked = other.blocked.or(self.blocked.take());
    }
}

/// How a failed delivery is treated; see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    Transient(String),
    Unauthorized(String),
    Permanent(String),
}

/// What sending one batch came to.
#[derive(Debug)]
enum Sent {
    /// Answered (or failed in a way [`fail_all`] has dealt with).
    Done(Pass),
    /// Refused as too large: nothing was recorded, the batch can be sent again smaller.
    TooLarge(String),
    /// The server is pacing its clients: nothing was recorded, the destination waits this long.
    Paced(chrono::Duration),
}

/// `nasiko agents sync` as a hook starts it: delivers what is due until the queue holds nothing more
/// to send now, then stops. Turns waiting on a retry are left for a later run.
pub fn run() -> Result<()> {
    let mut pace = Pace::default();
    let mut quiet = 0;
    while quiet < QUIET_SCANS {
        if deliver_paced(&mut pace, Backoff::Honour)?.progressed() {
            quiet = 0;
        } else {
            quiet += 1;
            std::thread::sleep(QUIET_SCAN_INTERVAL);
        }
    }
    Ok(())
}

/// Delivers every queued turn that is due, batch by batch, until none is left that this attempt could
/// send. Fails if another delivery holds the queue (it sends these too). Batches are as large as
/// `pace` allows, which adapts as the server answers; one pace kept across passes (an import's
/// courier) carries what it learned from one pass to the next. With [`Backoff::Ignore`], turns waiting
/// on a retry are sent too, once: one that fails again waits as usual.
pub fn deliver_paced(pace: &mut Pace, backoff: Backoff) -> Result<Pass> {
    deliver_at(
        &super::state::integrations_dir(),
        crate::config::load,
        pace,
        backoff,
    )
}

/// [`deliver_paced`] for the queue under `root`, with the sign-ins `load_config` reads.
pub(super) fn deliver_at(
    root: &Path,
    load_config: impl Fn() -> Result<Config>,
    pace: &mut Pace,
    backoff: Backoff,
) -> Result<Pass> {
    let _lock = queue::acquire_sync_lock_at(root, LOCK_WAIT)?;
    let mut total = Pass::default();
    let mut backoff = backoff;
    loop {
        let config = load_config()?;
        let records = queue::records_at(root, backoff)?;
        if records.is_empty() {
            break;
        }
        let pass = sync_records(&config, records, pace, backoff)?;
        // Each turn gets one attempt regardless of its retry delay; what failed again now waits it out.
        backoff = Backoff::Honour;
        let progressed = pass.progressed();
        total.absorb(pass);
        if !progressed {
            break;
        }
    }
    Ok(total)
}

fn sync_records(
    config: &Config,
    records: Vec<(PathBuf, QueueRecord)>,
    pace: &mut Pace,
    backoff: Backoff,
) -> Result<Pass> {
    let mut pass = Pass::default();
    let mut groups: BTreeMap<(String, String, Uuid), Vec<(PathBuf, QueueRecord)>> = BTreeMap::new();
    for record in records {
        if record.1.delivery_state == DeliveryState::Rejected {
            queue::quarantine(&record.0, &record.1)?;
            pass.set_aside += 1;
            continue;
        }
        if !backoff.lets_through(&record.1, Utc::now()) {
            continue;
        }
        let destination = &record.1.destination;
        groups
            .entry((
                destination.cluster_name.clone(),
                destination.cluster_url.clone(),
                destination.principal_id,
            ))
            .or_default()
            .push(record);
    }
    for ((cluster_name, cluster_url, principal_id), records) in groups {
        let destination = QueueDestination {
            cluster_name,
            cluster_url,
            principal_id,
        };
        let cluster = match validate_destination(config, &destination) {
            Ok(cluster) => cluster,
            Err(error) => {
                defer_blocked_all(&records, &error)?;
                pass.blocked = Some(error);
                continue;
            }
        };
        let client =
            crate::api::Client::from_cluster_entry_with_timeout(cluster, Some(BATCH_TIMEOUT));
        pass.absorb(deliver_destination(&client, &records, pace)?);
    }
    Ok(pass)
}

/// Sends one destination's due turns, one batch at a time at the size `pace` allows: a batch refused
/// as too large is sent again at once in halves, and a server that asks to be left alone holds back
/// every turn for it (see [`queue::pause`]) until it said to come back.
fn deliver_destination(
    client: &crate::api::Client,
    records: &[(PathBuf, QueueRecord)],
    pace: &mut Pace,
) -> Result<Pass> {
    let mut pass = Pass::default();
    let mut rest = records;
    while !rest.is_empty() {
        // Only the first turn of a batch can be this large: `batch_len` ends a batch before one.
        if let Some(reason) = too_large_to_send(event_bytes(&rest[0].1)) {
            let (path, record) = &rest[0];
            reject(path, record, format!("permanently rejected: {reason}"))?;
            pass.set_aside += 1;
            rest = &rest[1..];
            continue;
        }
        let (batch, after) = rest.split_at(batch_len(rest, pace.batch_size()));
        let started = Instant::now();
        match deliver_batch(client, batch)? {
            Sent::Done(done) => {
                pace.answered_in(started.elapsed());
                pass.absorb(done);
            }
            Sent::TooLarge(_) if batch.len() > 1 => {
                pace.too_large(batch.len());
                continue;
            }
            Sent::TooLarge(reason) => {
                let (path, record) = &batch[0];
                reject(path, record, format!("permanently rejected: {reason}"))?;
                pass.set_aside += 1;
            }
            Sent::Paced(wait) => {
                let destination_dir = batch[0].0.parent().unwrap_or(Path::new("."));
                queue::pause(destination_dir, pace::resume_at(wait, Utc::now()))?;
                return Ok(pass);
            }
        }
        rest = after;
    }
    Ok(pass)
}

/// How many of `records` the next batch takes: at most `max` turns and at most [`BATCH_BYTE_BUDGET`]
/// bytes of events, but always at least one. A server over its body limit closes the connection
/// before reading the body, so the client sees a broken pipe rather than the `413` — the batch has to
/// fit before it is sent.
fn batch_len(records: &[(PathBuf, QueueRecord)], max: usize) -> usize {
    let mut bytes = 0usize;
    let mut len = 0usize;
    for (_, record) in records.iter().take(max.max(1)) {
        bytes += event_bytes(record);
        if len > 0 && bytes > BATCH_BYTE_BUDGET {
            break;
        }
        len += 1;
    }
    len.max(1)
}

/// How many bytes `record`'s event takes in a request.
fn event_bytes(record: &QueueRecord) -> usize {
    serde_json::to_vec(&record.event).map_or(0, |event| event.len())
}

/// Why a turn of `bytes` can't be sent at all, if it can't: it is over [`SINGLE_EVENT_MAX`].
fn too_large_to_send(bytes: usize) -> Option<String> {
    (bytes > SINGLE_EVENT_MAX).then(|| {
        format!(
            "turn is larger than the server accepts ({:.1} MiB > {:.0} MiB)",
            bytes as f64 / MIB,
            SERVER_BODY_LIMIT as f64 / MIB
        )
    })
}

/// Whether a batch of `turns` turns and `body_bytes` bytes whose sending failed with `error` was
/// most likely refused for its size: the connection broke (a server over its body limit closes it
/// without reading the body) while a large batch of more than one turn was sent. Such a batch is sent
/// again in halves; a single turn is only retried, as the break may have been the network's.
fn dropped_for_size(error: &anyhow::Error, turns: usize, body_bytes: usize) -> bool {
    turns > 1 && body_bytes > LARGE_BATCH_BYTES && connection_broke(error)
}

/// Whether `error` is the connection being broken or reset by the other end. macOS reports some uses
/// of a socket the other end has reset as `EINVAL` ("Invalid argument") rather than as a reset.
fn connection_broke(error: &anyhow::Error) -> bool {
    use std::io::ErrorKind;
    let broke = |io: &std::io::Error| {
        matches!(
            io.kind(),
            ErrorKind::BrokenPipe
                | ErrorKind::ConnectionReset
                | ErrorKind::ConnectionAborted
                | ErrorKind::InvalidInput
        )
    };
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(broke)
            || matches!(cause.downcast_ref::<ureq::Error>(), Some(ureq::Error::Io(io)) if broke(io))
    })
}

fn deliver_batch(client: &crate::api::Client, records: &[(PathBuf, QueueRecord)]) -> Result<Sent> {
    let request = CodingAgentEventBatchRequest {
        events: records
            .iter()
            .map(|(_, record)| record.event.clone())
            .collect(),
    };
    let response = match client.post_coding_agent_batch(&request) {
        Ok(Attempt::Ok(response)) => response,
        Ok(Attempt::Rejected(failure)) => {
            let server_wait = failure
                .retry_after()
                .and_then(|value| pace::server_wait(value, Utc::now()));
            if let Some(sent) = server_directed(failure.status, failure.message(), server_wait) {
                return Ok(sent);
            }
            return fail_all(records, classify_status(failure.status, failure.message()))
                .map(Sent::Done);
        }
        Err(error) => {
            let body_bytes = serde_json::to_vec(&request).map_or(0, |body| body.len());
            if dropped_for_size(&error, records.len(), body_bytes) {
                return Ok(Sent::TooLarge(format!(
                    "connection dropped sending {:.1} MiB: {error:#}",
                    body_bytes as f64 / MIB
                )));
            }
            return fail_all(
                records,
                Failure::Transient(format!("delivery failed: {error:#}")),
            )
            .map(Sent::Done);
        }
    };
    let response_matches = response.results.len() == records.len()
        && response
            .results
            .iter()
            .zip(records)
            .all(|(result, (_, record))| result.event_id == record.event.event_id);
    if !response_matches {
        return fail_all(
            records,
            Failure::Transient(
                "server response did not contain one ordered result per event".into(),
            ),
        )
        .map(Sent::Done);
    }
    let mut pass = Pass::default();
    for (result, (path, record)) in response.results.into_iter().zip(records) {
        match result.status {
            CodingAgentEventStatus::Accepted | CodingAgentEventStatus::Duplicate => {
                queue::remove(path)?;
                pass.delivered += 1;
            }
            CodingAgentEventStatus::Rejected => {
                let reason = result
                    .error
                    .unwrap_or_else(|| "server rejected event".into());
                reject(path, record, format!("permanently rejected: {reason}"))?;
                pass.set_aside += 1;
            }
        }
    }
    Ok(Sent::Done(pass))
}

/// An HTTP error that says how to send again rather than what became of the turns: a `429` or `503`
/// with a `Retry-After` that could be read (`server_wait`), or a `413`. Nothing is recorded for these.
fn server_directed(
    status: u16,
    message: &str,
    server_wait: Option<chrono::Duration>,
) -> Option<Sent> {
    match (status, server_wait) {
        (429 | 503, Some(wait)) => Some(Sent::Paced(wait)),
        (413, _) => Some(Sent::TooLarge(format!("HTTP 413: {message}"))),
        _ => None,
    }
}

/// What an HTTP error for a whole batch means for its turns.
fn classify_status(status: u16, message: &str) -> Failure {
    let reason = format!("HTTP {status}: {message}");
    match status {
        401 | 403 => Failure::Unauthorized(reason),
        408 | 429 | 500..=599 => Failure::Transient(reason),
        _ => Failure::Permanent(reason),
    }
}

fn fail_all(records: &[(PathBuf, QueueRecord)], failure: Failure) -> Result<Pass> {
    let mut pass = Pass::default();
    for (path, record) in records {
        match &failure {
            Failure::Transient(reason) => {
                if retry_later(path, record, reason.clone(), Utc::now())? {
                    pass.set_aside += 1;
                }
            }
            Failure::Permanent(reason) => {
                reject(path, record, format!("permanently rejected: {reason}"))?;
                pass.set_aside += 1;
            }
            Failure::Unauthorized(reason) => {
                defer_unauthorized(path, record, reason)?;
                pass.blocked = Some(reason.clone());
            }
        }
    }
    Ok(pass)
}

/// Turns whose destination isn't usable from here wait, uncounted, for the next delivery after the
/// user fixes it.
fn defer_blocked_all(records: &[(PathBuf, QueueRecord)], error: &str) -> Result<()> {
    for (path, record) in records {
        let mut record = record.clone();
        record.delivery_state = DeliveryState::Deferred;
        record.last_error = Some(error.to_string());
        record.updated_at = Utc::now();
        record.next_attempt_at = None;
        queue::update(path, &record)?;
    }
    Ok(())
}

/// The server refused the sign-in: not the turn's fault, so no attempt is counted, but it waits the
/// first retry interval rather than asking again at once.
fn defer_unauthorized(path: &Path, record: &QueueRecord, error: &str) -> Result<()> {
    let mut record = record.clone();
    record.delivery_state = DeliveryState::Deferred;
    record.last_error = Some(error.to_string());
    record.updated_at = Utc::now();
    record.next_attempt_at = Some(record.updated_at + chrono::Duration::seconds(FIRST_RETRY_SECS));
    queue::update(path, &record)
}

/// Schedules another attempt after a transient failure, or sets the turn aside once it has waited
/// [`MAX_UNDELIVERED_DAYS`]. Returns whether it was set aside.
fn retry_later(
    path: &Path,
    record: &QueueRecord,
    error: String,
    now: DateTime<Utc>,
) -> Result<bool> {
    if now - record.created_at > chrono::Duration::days(MAX_UNDELIVERED_DAYS) {
        reject(path, record, format!("{EXPIRED_REASON}: {error}"))?;
        return Ok(true);
    }
    let mut record = record.clone();
    record.delivery_state = DeliveryState::Deferred;
    record.last_error = Some(error);
    record.updated_at = now;
    record.attempts = record.attempts.saturating_add(1);
    record.next_attempt_at = Some(now + retry_delay(record.attempts));
    queue::update(path, &record)?;
    Ok(false)
}

/// How long to wait after the `attempts`-th failure: 1 min, doubling, at most 1 h.
fn retry_delay(attempts: u32) -> chrono::Duration {
    let doublings = attempts.saturating_sub(1).min(i64::BITS - 2);
    let seconds = FIRST_RETRY_SECS.saturating_mul(1_i64 << doublings);
    chrono::Duration::seconds(seconds.min(MAX_RETRY_SECS))
}

fn reject(path: &Path, record: &QueueRecord, error: String) -> Result<()> {
    let mut record = record.clone();
    record.delivery_state = DeliveryState::Rejected;
    record.last_error = Some(error);
    record.updated_at = Utc::now();
    queue::update(path, &record)?;
    queue::quarantine(path, &record).map(|_| ())
}

fn validate_destination<'a>(
    config: &'a Config,
    destination: &QueueDestination,
) -> Result<&'a ClusterEntry, String> {
    let Some(cluster) = config.clusters.get(&destination.cluster_name) else {
        return Err(format!(
            "bound cluster '{}' is no longer configured",
            destination.cluster_name
        ));
    };
    if normalize_url(&cluster.url) != normalize_url(&destination.cluster_url) {
        return Err(format!(
            "bound cluster '{}' URL changed from '{}' to '{}'; refusing to reroute",
            destination.cluster_name, destination.cluster_url, cluster.url
        ));
    }
    let Some(token) = cluster.token.as_deref().filter(|token| !token.is_empty()) else {
        return Err(format!(
            "bound cluster '{}' is not authenticated",
            destination.cluster_name
        ));
    };
    if crate::config::token_expired(token) == Some(true) {
        return Err(format!(
            "authentication for bound cluster '{}' has expired",
            destination.cluster_name
        ));
    }
    let subject = crate::config::token_subject(token)
        .and_then(|subject| Uuid::parse_str(&subject).ok())
        .ok_or_else(|| {
            format!(
                "authentication for bound cluster '{}' has no valid user UUID subject",
                destination.cluster_name
            )
        })?;
    if subject != destination.principal_id {
        return Err(format!(
            "authenticated account for bound cluster '{}' changed; refusing to upload another user's events",
            destination.cluster_name
        ));
    }
    Ok(cluster)
}

fn normalize_url(url: &str) -> &str {
    url.trim_end_matches('/')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ClusterEntry;
    use chrono::{TimeZone, Utc};
    use nasiko_types::{
        CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentEventV1, CodingAgentSession,
        CodingAgentSource, CodingAgentTimestampQuality, CodingAgentToolAssociation,
        CodingAgentToolCall, CodingAgentToolCallStatus, CodingAgentTurn, coding_agent_event_id,
        coding_agent_session_id,
    };
    use std::collections::HashMap;

    fn token(subject: Uuid) -> String {
        use base64::Engine as _;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::json!({"sub": subject.to_string(), "exp": 4_102_444_800_i64}).to_string(),
        );
        format!("header.{payload}.signature")
    }

    fn config(name: &str, url: &str, token: Option<&str>) -> Config {
        Config {
            active: Some("different-active-cluster".into()),
            clusters: HashMap::from([(
                name.into(),
                ClusterEntry {
                    url: url.into(),
                    username: None,
                    token: token.map(str::to_string),
                },
            )]),
            registry_url: None,
        }
    }

    #[test]
    fn batches_stay_under_the_byte_budget_however_many_turns_are_allowed() {
        let big = |turn: &str| {
            let mut record = record("bound", "https://bound.example", "s", turn);
            record.event.turn.prompt = Some("x".repeat(1_400_000));
            (PathBuf::from(turn), record)
        };
        let records: Vec<_> = (0..10).map(|i| big(&format!("t{i}"))).collect();
        // 1.4 MB turns: four fit in 6 MiB, the fifth would not.
        assert_eq!(batch_len(&records, 100), 4);
        assert_eq!(batch_len(&records, 2), 2);
        // A single turn always goes, even alone over the budget.
        let mut huge = big("solo");
        huge.1.event.turn.prompt = Some("x".repeat(7_000_000));
        assert_eq!(batch_len(std::slice::from_ref(&huge), 100), 1);
    }

    #[test]
    fn a_turn_over_the_server_limit_is_too_large_to_send() {
        assert_eq!(too_large_to_send(SINGLE_EVENT_MAX), None);
        assert_eq!(too_large_to_send(7 * 1024 * 1024), None);
        assert_eq!(
            too_large_to_send(9 * 1024 * 1024).as_deref(),
            Some("turn is larger than the server accepts (9.0 MiB > 8 MiB)")
        );
        assert!(too_large_to_send(SINGLE_EVENT_MAX + 1).is_some());
    }

    #[test]
    fn a_broken_connection_on_a_large_batch_is_taken_as_too_large() {
        use std::io::{Error, ErrorKind};
        let large = LARGE_BATCH_BYTES + 1;
        let broken_pipe = anyhow::Error::new(Error::from(ErrorKind::BrokenPipe))
            .context("cannot reach control plane");
        let reset = anyhow::Error::new(ureq::Error::Io(Error::from(ErrorKind::ConnectionReset)))
            .context("cannot reach control plane");
        let timed_out = anyhow::Error::new(Error::from(ErrorKind::TimedOut));
        let reset_on_macos = anyhow::Error::new(Error::from_raw_os_error(22));

        assert!(dropped_for_size(&broken_pipe, 2, large));
        assert!(dropped_for_size(&reset, 10, large));
        assert!(dropped_for_size(&reset_on_macos, 2, large));
        assert!(
            !dropped_for_size(&broken_pipe, 1, large),
            "a single turn is only retried: the break may have been the network's"
        );
        assert!(!dropped_for_size(&broken_pipe, 2, LARGE_BATCH_BYTES));
        assert!(!dropped_for_size(&timed_out, 2, large));
    }

    #[test]
    fn a_turn_over_the_server_limit_is_set_aside_without_being_sent() {
        let mut server = mockito::Server::new();
        let dir = tempfile::tempdir().unwrap();
        let huge = padded(record("bound", &server.url(), "s", "huge"), 9 * 1024 * 1024);
        let huge_path = queue::enqueue_at(dir.path(), &huge).unwrap();
        let small_path =
            queue::enqueue_at(dir.path(), &record("bound", &server.url(), "s", "small")).unwrap();
        let small_id = queue::load(&small_path).unwrap().event.event_id;
        let delivery = server
            .mock("POST", "/api/telemetry/coding-agent/events/batch")
            .match_request(move |request| requested_ids(request) == [small_id.clone()])
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(accepted)
            .expect(1)
            .create();
        let config = config("bound", &server.url(), Some(&token(Uuid::nil())));

        let pass = deliver_at(
            dir.path(),
            || Ok(config.clone()),
            &mut Pace::default(),
            Backoff::Honour,
        )
        .unwrap();

        delivery.assert();
        assert_eq!((pass.delivered, pass.set_aside), (1, 1));
        assert!(!huge_path.exists());
        let set_aside = only(&dir.path().join("rejected"));
        assert_eq!(set_aside.event.event_id, huge.event.event_id);
        assert_eq!(
            set_aside.last_error.as_deref(),
            Some("permanently rejected: turn is larger than the server accepts (9.0 MiB > 8 MiB)")
        );
    }

    /// `record` with its content captured and tool calls added until its event takes about `bytes`
    /// (each tool call's raw transcript is at most 512 KiB, as an event's fields have limits of their
    /// own).
    fn padded(mut record: QueueRecord, bytes: usize) -> QueueRecord {
        const PIECE: usize = 512 * 1024;
        record.event.capture_policy = CapturePolicy::Content;
        record.event.turn.prompt = Some("p".into());
        record.event.turn.response = Some("r".into());
        let mut left = bytes;
        while left > 0 {
            let size = left.min(PIECE);
            let id = format!("tool-{}", record.event.turn.tool_calls.len());
            record.event.turn.tool_calls.push(CodingAgentToolCall {
                id,
                name: "read".into(),
                kind: "read".into(),
                model_call_id: None,
                status: CodingAgentToolCallStatus::Succeeded,
                arguments: None,
                output: None,
                raw: Some("x".repeat(size)),
                error: None,
                started_at: None,
                ended_at: None,
                duration_ms: None,
                association: CodingAgentToolAssociation::Unknown,
                timestamp_quality: CodingAgentTimestampQuality::Unknown,
            });
            left -= size;
        }
        record
    }

    /// A server on localhost that, like one over its body limit, closes the connection without reading
    /// a request body over [`LARGE_BATCH_BYTES`], and accepts every event of a smaller one.
    fn server_dropping_large_bodies() -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut reader = BufReader::new(stream);
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    let header = line.split_once(':');
                    if let Some((_, value)) =
                        header.filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                if length > LARGE_BATCH_BYTES {
                    continue;
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    continue;
                }
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let results: Vec<serde_json::Value> = request["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|event| serde_json::json!({"event_id": event["event_id"], "status": "accepted"}))
                    .collect();
                let reply = serde_json::json!({"data": {"results": results}, "status_code": 200})
                    .to_string();
                let _ = write!(
                    reader.get_mut(),
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        url
    }

    #[test]
    fn a_large_batch_whose_connection_drops_is_sent_again_in_halves() {
        let url = server_dropping_large_bodies();
        let dir = tempfile::tempdir().unwrap();
        // Three 600 kB turns: together over `LARGE_BATCH_BYTES`, each alone under it.
        let records: Vec<_> = (0..3)
            .map(|turn| {
                let record = padded(record("bound", &url, "s", &format!("t{turn}")), 600_000);
                let path = queue::enqueue_at(dir.path(), &record).unwrap();
                (path, record)
            })
            .collect();
        let mut pace = Pace::default();

        let pass = sync_records(
            &config("bound", &url, Some(&token(Uuid::nil()))),
            records,
            &mut pace,
            Backoff::Honour,
        )
        .unwrap();

        assert_eq!((pass.delivered, pass.set_aside), (3, 0));
        assert!(queue::queued_ids_at(dir.path()).unwrap().is_empty());
        assert!(pace.batch_size() < 3, "the pace learned the smaller size");
    }

    #[test]
    fn sending_now_ignores_a_retry_delay_but_honouring_it_waits() {
        let mut server = mockito::Server::new();
        let dir = tempfile::tempdir().unwrap();
        let mut deferred = record("bound", &server.url(), "s", "deferred");
        deferred.attempts = 3;
        deferred.next_attempt_at = Some(Utc::now() + chrono::Duration::minutes(30));
        let path = queue::enqueue_at(dir.path(), &deferred).unwrap();
        let delivery = server
            .mock("POST", "/api/telemetry/coding-agent/events/batch")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(accepted)
            .expect(1)
            .create();
        let config = config("bound", &server.url(), Some(&token(Uuid::nil())));
        let deliver = |backoff| {
            deliver_at(
                dir.path(),
                || Ok(config.clone()),
                &mut Pace::default(),
                backoff,
            )
            .unwrap()
        };

        assert_eq!(deliver(Backoff::Honour), Pass::default());
        assert!(path.exists(), "a hook's delivery leaves it to its retry");
        assert_eq!(deliver(Backoff::Ignore).delivered, 1);

        delivery.assert();
        assert!(!path.exists());
    }

    fn record(name: &str, url: &str, session: &str, turn: &str) -> QueueRecord {
        let at = Utc.timestamp_opt(1, 0).unwrap();
        QueueRecord::new(
            QueueDestination {
                cluster_name: name.into(),
                cluster_url: url.into(),
                principal_id: Uuid::nil(),
            },
            CodingAgentEventV1 {
                version: CODING_AGENT_EVENT_VERSION,
                event_id: coding_agent_event_id("claude", session, turn),
                captured_at: at,
                source: CodingAgentSource {
                    agent_id: "claude".into(),
                    agent_name: "coding-agent".into(),
                },
                session: CodingAgentSession {
                    id: coding_agent_session_id("claude", session),
                    source_id: session.into(),
                    title: None,
                    parent: None,
                },
                turn: CodingAgentTurn {
                    id: turn.into(),
                    prompt: None,
                    response: None,
                    started_at: at,
                    ended_at: at,
                    llm_calls: vec![],
                    tool_calls: vec![],
                },
                capture_policy: CapturePolicy::MetadataOnly,
            },
        )
    }

    fn queued(dir: &tempfile::TempDir, record: QueueRecord) -> (PathBuf, QueueRecord) {
        let path = dir.path().join(format!("{}.json", record.event.event_id));
        queue::update(&path, &record).unwrap();
        (path, record)
    }

    #[test]
    fn validates_the_bound_cluster_not_the_current_active_cluster() {
        let destination = QueueDestination {
            cluster_name: "original".into(),
            cluster_url: "https://original.example".into(),
            principal_id: Uuid::nil(),
        };
        assert!(
            validate_destination(
                &config(
                    "original",
                    "https://original.example/",
                    Some(&token(Uuid::nil()))
                ),
                &destination
            )
            .is_ok()
        );
    }

    #[test]
    fn missing_bound_cluster_is_deferred() {
        let destination = QueueDestination {
            cluster_name: "missing".into(),
            cluster_url: "https://missing.example".into(),
            principal_id: Uuid::nil(),
        };
        let error = validate_destination(&Config::default(), &destination).unwrap_err();
        assert!(error.contains("no longer configured"));
    }

    #[test]
    fn changed_bound_url_is_deferred_instead_of_rerouted() {
        let destination = QueueDestination {
            cluster_name: "original".into(),
            cluster_url: "https://old.example".into(),
            principal_id: Uuid::nil(),
        };
        let error = validate_destination(
            &config("original", "https://new.example", Some(&token(Uuid::nil()))),
            &destination,
        )
        .unwrap_err();
        assert!(error.contains("refusing to reroute"));

        let dir = tempfile::tempdir().unwrap();
        let queued = queued(
            &dir,
            record("original", "https://old.example", "s", "url-change"),
        );
        let pass = sync_records(
            &config("original", "https://new.example", Some(&token(Uuid::nil()))),
            vec![queued.clone()],
            &mut Pace::default(),
            Backoff::Honour,
        )
        .unwrap();
        assert!(
            pass.blocked.unwrap().contains("refusing to reroute"),
            "configuration mismatches wait for the user"
        );
        let retained = queue::load(&queued.0).unwrap();
        assert_eq!(retained.delivery_state, DeliveryState::Deferred);
        assert_eq!(retained.attempts, 0);
        assert!(retained.last_error.unwrap().contains("refusing to reroute"));
    }

    #[test]
    fn missing_auth_is_deferred() {
        let destination = QueueDestination {
            cluster_name: "original".into(),
            cluster_url: "https://original.example".into(),
            principal_id: Uuid::nil(),
        };
        let error = validate_destination(
            &config("original", "https://original.example", None),
            &destination,
        )
        .unwrap_err();
        assert!(error.contains("not authenticated"));
    }

    #[test]
    fn switched_account_is_never_allowed_to_upload_captured_events() {
        let destination = QueueDestination {
            cluster_name: "original".into(),
            cluster_url: "https://original.example".into(),
            principal_id: Uuid::nil(),
        };
        let error = validate_destination(
            &config(
                "original",
                "https://original.example",
                Some(&token(Uuid::new_v4())),
            ),
            &destination,
        )
        .unwrap_err();
        assert!(error.contains("changed"));
    }

    #[test]
    fn accepted_and_duplicate_results_delete_records_from_bound_target() {
        let mut bound = mockito::Server::new();
        let active = mockito::Server::new();
        let dir = tempfile::tempdir().unwrap();
        let first = queued(&dir, record("bound", &bound.url(), "s", "one"));
        let second = queued(&dir, record("bound", &bound.url(), "s", "two"));
        let first_id = first.1.event.event_id.clone();
        let second_id = second.1.event.event_id.clone();
        let delivery = bound
            .mock("POST", "/api/telemetry/coding-agent/events/batch")
            .match_header(
                "authorization",
                format!("Bearer {}", token(Uuid::nil())).as_str(),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"data":{{"results":[{{"event_id":"{first_id}","status":"accepted"}},{{"event_id":"{second_id}","status":"duplicate"}}]}},"status_code":200,"message":"ok"}}"#
            ))
            .create();
        let config = Config {
            active: Some("active".into()),
            clusters: HashMap::from([
                (
                    "bound".into(),
                    ClusterEntry {
                        url: bound.url(),
                        username: None,
                        token: Some(token(Uuid::nil())),
                    },
                ),
                (
                    "active".into(),
                    ClusterEntry {
                        url: active.url(),
                        username: None,
                        token: Some(token(Uuid::new_v4())),
                    },
                ),
            ]),
            registry_url: None,
        };

        assert_eq!(
            sync_records(
                &config,
                vec![first.clone(), second.clone()],
                &mut Pace::default(),
                Backoff::Honour,
            )
            .unwrap()
            .delivered,
            2
        );
        delivery.assert();
        assert!(!first.0.exists());
        assert!(!second.0.exists());
    }

    #[test]
    fn rejected_result_is_retained_with_permanent_error() {
        let mut server = mockito::Server::new();
        let dir = tempfile::tempdir().unwrap();
        let queued = queued(&dir, record("bound", &server.url(), "s", "rejected"));
        let event_id = queued.1.event.event_id.clone();
        let _delivery = server
            .mock("POST", "/api/telemetry/coding-agent/events/batch")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"data":{{"results":[{{"event_id":"{event_id}","status":"rejected","error":"conflict"}}]}},"status_code":200,"message":"ok"}}"#
            ))
            .create();

        sync_records(
            &config("bound", &server.url(), Some(&token(Uuid::nil()))),
            vec![queued.clone()],
            &mut Pace::default(),
            Backoff::Honour,
        )
        .unwrap();
        assert!(!queued.0.exists());
        let rejected = queued.0.parent().unwrap().join("rejected");
        assert!(rejected.exists());
    }

    #[test]
    fn mismatched_result_is_retained() {
        let mut server = mockito::Server::new();
        let dir = tempfile::tempdir().unwrap();
        let queued = queued(&dir, record("bound", &server.url(), "s", "mismatch"));
        let _delivery = server
            .mock("POST", "/api/telemetry/coding-agent/events/batch")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"results":[{"event_id":"other","status":"accepted"}]},"status_code":200,"message":"ok"}"#)
            .create();

        sync_records(
            &config("bound", &server.url(), Some(&token(Uuid::nil()))),
            vec![queued.clone()],
            &mut Pace::default(),
            Backoff::Honour,
        )
        .unwrap();
        let retained = queue::load(&queued.0).unwrap();
        assert_eq!(retained.delivery_state, DeliveryState::Deferred);
        assert!(retained.last_error.unwrap().contains("one ordered result"));
    }

    fn only(dir: &Path) -> QueueRecord {
        let files = queue::record_files(dir).unwrap();
        assert_eq!(files.len(), 1, "one record in {}", dir.display());
        queue::load(&files[0]).unwrap()
    }

    fn batch_failing_with(status: usize) -> (mockito::ServerGuard, tempfile::TempDir, Pass) {
        let mut server = mockito::Server::new();
        let dir = tempfile::tempdir().unwrap();
        let path = queue::enqueue_at(
            dir.path(),
            &record("bound", &server.url(), "s", &format!("http-{status}")),
        )
        .unwrap();
        let record = queue::load(&path).unwrap();
        server
            .mock("POST", "/api/telemetry/coding-agent/events/batch")
            .with_status(status)
            .with_body(r#"{"message":"nope"}"#)
            .create();
        let pass = sync_records(
            &config("bound", &server.url(), Some(&token(Uuid::nil()))),
            vec![(path, record)],
            &mut Pace::default(),
            Backoff::Honour,
        )
        .unwrap();
        (server, dir, pass)
    }

    #[test]
    fn http_failures_are_classified_as_transient_unauthorized_or_permanent() {
        let table = [
            (408, "transient"),
            (429, "transient"),
            (500, "transient"),
            (502, "transient"),
            (503, "transient"),
            (599, "transient"),
            (401, "unauthorized"),
            (403, "unauthorized"),
            (400, "permanent"),
            (404, "permanent"),
            (422, "permanent"),
        ];
        for (status, expected) in table {
            let kind = match classify_status(status, "m") {
                Failure::Transient(_) => "transient",
                Failure::Unauthorized(_) => "unauthorized",
                Failure::Permanent(reason) => {
                    assert_eq!(reason, format!("HTTP {status}: m"));
                    "permanent"
                }
            };
            assert_eq!(kind, expected, "HTTP {status}");
        }
    }

    #[test]
    fn a_server_error_keeps_the_batch_queued_for_a_retry_in_a_minute() {
        let (_server, dir, pass) = batch_failing_with(503);

        assert_eq!(pass, Pass::default());
        let kept = only(&dir.path().join("queue"));
        assert_eq!(kept.attempts, 1);
        assert!(kept.last_error.unwrap().contains("HTTP 503"));
        let wait = kept.next_attempt_at.unwrap() - kept.updated_at;
        assert_eq!(wait, chrono::Duration::seconds(FIRST_RETRY_SECS));
    }

    #[test]
    fn an_unreachable_server_is_transient() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing listens on port 9 of localhost.
        let url = "http://127.0.0.1:9";
        let path = queue::enqueue_at(dir.path(), &record("bound", url, "s", "down")).unwrap();
        let record = queue::load(&path).unwrap();

        let pass = sync_records(
            &config("bound", url, Some(&token(Uuid::nil()))),
            vec![(path, record)],
            &mut Pace::default(),
            Backoff::Honour,
        )
        .unwrap();

        assert_eq!(pass, Pass::default());
        assert_eq!(only(&dir.path().join("queue")).attempts, 1);
    }

    #[test]
    fn a_client_error_sets_the_batch_aside_with_the_reason() {
        let (_server, dir, pass) = batch_failing_with(422);

        assert_eq!(pass.set_aside, 1);
        assert!(queue::queued_ids_at(dir.path()).unwrap().is_empty());
        let set_aside = only(&dir.path().join("rejected"));
        assert_eq!(set_aside.delivery_state, DeliveryState::Rejected);
        assert_eq!(
            set_aside.last_error.as_deref(),
            Some("permanently rejected: HTTP 422: nope")
        );
    }

    #[test]
    fn a_refused_sign_in_blocks_without_counting_an_attempt() {
        let (_server, dir, pass) = batch_failing_with(401);

        assert!(pass.blocked.unwrap().contains("HTTP 401"));
        let kept = only(&dir.path().join("queue"));
        assert_eq!(kept.attempts, 0);
        assert!(kept.next_attempt_at.is_some());
    }

    #[test]
    fn retries_back_off_from_a_minute_doubling_to_an_hour() {
        let minutes: Vec<i64> = (1..=9)
            .map(|attempts| retry_delay(attempts).num_minutes())
            .collect();
        assert_eq!(minutes, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(retry_delay(u32::MAX).num_minutes(), 60);
    }

    #[test]
    fn transient_failures_never_set_a_young_turn_aside() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = record("bound", "https://bound.example", "s", "patient");
        record.created_at = Utc::now() - chrono::Duration::days(MAX_UNDELIVERED_DAYS - 1);
        let path = queue::enqueue_at(dir.path(), &record).unwrap();

        for _ in 0..100 {
            let current = queue::load(&path).unwrap();
            assert!(!retry_later(&path, &current, "offline".into(), Utc::now()).unwrap());
        }

        let kept = queue::load(&path).unwrap();
        assert_eq!(kept.attempts, 100);
        assert_eq!(kept.delivery_state, DeliveryState::Deferred);
        assert!(!dir.path().join("rejected").exists());
    }

    #[test]
    fn a_turn_undelivered_for_30_days_is_set_aside_as_expired() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = record("bound", "https://bound.example", "s", "stale");
        record.created_at = Utc::now() - chrono::Duration::days(MAX_UNDELIVERED_DAYS + 1);
        let path = queue::enqueue_at(dir.path(), &record).unwrap();

        assert!(retry_later(&path, &record, "offline".into(), Utc::now()).unwrap());

        assert!(!path.exists());
        let expired = only(&dir.path().join("rejected"));
        assert_eq!(expired.delivery_state, DeliveryState::Rejected);
        assert!(expired.last_error.unwrap().starts_with(EXPIRED_REASON));
    }

    /// The event ids in a batch request's body.
    fn requested_ids(request: &mockito::Request) -> Vec<String> {
        let body: serde_json::Value =
            serde_json::from_slice(request.body().expect("request body")).expect("JSON body");
        body["events"]
            .as_array()
            .expect("events")
            .iter()
            .map(|event| event["event_id"].as_str().expect("event id").to_string())
            .collect()
    }

    /// The batch response a server that accepts every event in `request` sends.
    fn accepted(request: &mockito::Request) -> Vec<u8> {
        let results: Vec<serde_json::Value> = requested_ids(request)
            .into_iter()
            .map(|id| serde_json::json!({"event_id": id, "status": "accepted"}))
            .collect();
        serde_json::json!({"data": {"results": results}, "status_code": 200, "message": "ok"})
            .to_string()
            .into_bytes()
    }

    /// `turns` turns queued under `root` for the server at `url`.
    fn enqueue_turns(root: &Path, url: &str, turns: usize) -> Vec<(PathBuf, QueueRecord)> {
        (0..turns)
            .map(|turn| {
                let path = queue::enqueue_at(root, &record("bound", url, "s", &format!("t{turn}")))
                    .unwrap();
                let record = queue::load(&path).unwrap();
                (path, record)
            })
            .collect()
    }

    #[test]
    fn a_pacing_server_is_left_alone_exactly_as_long_as_it_asked() {
        for (status, retry_after) in [
            (429, "120".to_string()),
            (
                503,
                (Utc::now() + chrono::Duration::seconds(121))
                    .format("%a, %d %b %Y %H:%M:%S GMT")
                    .to_string(),
            ),
        ] {
            let mut server = mockito::Server::new();
            let dir = tempfile::tempdir().unwrap();
            let records = enqueue_turns(dir.path(), &server.url(), 3);
            let paced = server
                .mock("POST", "/api/telemetry/coding-agent/events/batch")
                .with_status(status)
                .with_header("retry-after", &retry_after)
                .with_body("slow down")
                .expect(1)
                .create();
            let config = config("bound", &server.url(), Some(&token(Uuid::nil())));
            let before = Utc::now();

            let pass = sync_records(
                &config,
                records.clone(),
                &mut Pace::default(),
                Backoff::Honour,
            )
            .unwrap();
            // Even sending now, as the user asked, leaves alone a server that asked to be.
            let again = deliver_at(
                dir.path(),
                || Ok(config.clone()),
                &mut Pace::default(),
                Backoff::Ignore,
            )
            .unwrap();

            paced.assert();
            assert_eq!(pass, Pass::default(), "HTTP {status}");
            assert_eq!(again, Pass::default(), "nothing is sent while paused");
            let destination_dir = records[0].0.parent().unwrap();
            let until = queue::paused_until(destination_dir, before).expect("paused");
            assert!(
                until >= before + chrono::Duration::seconds(119),
                "HTTP {status}"
            );
            assert!(
                until <= Utc::now() + chrono::Duration::seconds(121 + 13),
                "HTTP {status}"
            );
            for (path, _) in &records {
                let kept = queue::load(path).unwrap();
                assert_eq!(kept.attempts, 0, "pacing is no failure of the turn's");
                assert_eq!(kept.next_attempt_at, None);
            }
            let backlog = super::super::backlog::backlog_at(dir.path()).unwrap();
            assert_eq!((backlog.pending(), backlog.due), (3, 0));
            assert_eq!(backlog.next_attempt_at, Some(until));
        }
    }

    #[test]
    fn a_pacing_answer_without_retry_after_is_an_ordinary_transient_failure() {
        let (_server, dir, pass) = batch_failing_with(429);

        assert_eq!(pass, Pass::default());
        assert_eq!(only(&dir.path().join("queue")).attempts, 1);
    }

    #[test]
    fn a_batch_too_large_is_sent_again_at_once_in_halves() {
        let mut server = mockito::Server::new();
        let dir = tempfile::tempdir().unwrap();
        let records = enqueue_turns(dir.path(), &server.url(), 3);
        let too_large = server
            .mock("POST", "/api/telemetry/coding-agent/events/batch")
            .match_request(|request| requested_ids(request).len() > 1)
            .with_status(413)
            .with_body("too many events")
            .expect(1)
            .create();
        let single = server
            .mock("POST", "/api/telemetry/coding-agent/events/batch")
            .match_request(|request| requested_ids(request).len() == 1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(accepted)
            .expect(3)
            .create();
        let mut pace = Pace::default();

        let pass = sync_records(
            &config("bound", &server.url(), Some(&token(Uuid::nil()))),
            records,
            &mut pace,
            Backoff::Honour,
        )
        .unwrap();

        too_large.assert();
        single.assert();
        assert_eq!(pass.delivered, 3);
        assert_eq!(
            pace.batch_size(),
            2,
            "halved to one, then doubled by three fast answers in a row"
        );
        assert!(queue::queued_ids_at(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn a_single_turn_too_large_is_set_aside_with_the_reason() {
        let (_server, dir, pass) = batch_failing_with(413);

        assert_eq!(pass.set_aside, 1);
        let set_aside = only(&dir.path().join("rejected"));
        assert_eq!(
            set_aside.last_error.as_deref(),
            Some("permanently rejected: HTTP 413: nope")
        );
    }
}
