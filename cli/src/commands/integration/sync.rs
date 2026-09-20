//! Destination-bound delivery for queued coding-agent telemetry.

use anyhow::Result;
use nasiko_types::{
    CODING_AGENT_BATCH_MAX_EVENTS, CodingAgentEventBatchRequest, CodingAgentEventStatus,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

use super::queue::{self, DeliveryState, QueueDestination, QueueRecord};
use crate::config::{ClusterEntry, Config};

const MAX_DELIVERY_ATTEMPTS: u32 = 5;

pub fn run() -> Result<()> {
    let _lock = queue::acquire_sync_lock(Duration::from_millis(250))?;
    let started = std::time::Instant::now();
    let mut quiescent_scans = 0;
    loop {
        let config = crate::config::load()?;
        let records = queue::records()?;
        if records.is_empty() {
            if queue::has_pending_records()? && started.elapsed() < Duration::from_secs(300) {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            quiescent_scans += 1;
            if quiescent_scans >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        quiescent_scans = 0;
        let retry = sync_records(&config, records)?;
        if !retry || started.elapsed() >= Duration::from_secs(300) {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Ok(())
}

fn sync_records(config: &Config, records: Vec<(PathBuf, QueueRecord)>) -> Result<bool> {
    let mut retry = false;
    let mut attempted_delivery = false;
    let mut groups: BTreeMap<(String, String, Uuid), Vec<(PathBuf, QueueRecord)>> = BTreeMap::new();
    for record in records {
        if record.1.delivery_state == DeliveryState::Rejected {
            queue::quarantine(&record.0, &record.1)?;
            continue;
        }
        if record
            .1
            .next_attempt_at
            .is_some_and(|at| at > chrono::Utc::now())
        {
            retry = true;
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
                continue;
            }
        };
        let client = crate::api::Client::from_cluster_entry_with_timeout(
            cluster,
            Some(Duration::from_secs(10)),
        );
        attempted_delivery = true;
        for batch in records.chunks(CODING_AGENT_BATCH_MAX_EVENTS) {
            retry |= deliver_batch(&client, batch)?;
        }
    }
    Ok(retry || attempted_delivery)
}

fn deliver_batch(client: &crate::api::Client, records: &[(PathBuf, QueueRecord)]) -> Result<bool> {
    let request = CodingAgentEventBatchRequest {
        events: records
            .iter()
            .map(|(_, record)| record.event.clone())
            .collect(),
    };
    let response = match client.post_coding_agent_batch(&request) {
        Ok(response) => response,
        Err(error) => {
            return defer_delivery_all(records, &format!("delivery failed: {error:#}"));
        }
    };
    let response_matches = response.results.len() == records.len()
        && response
            .results
            .iter()
            .zip(records)
            .all(|(result, (_, record))| result.event_id == record.event.event_id);
    if !response_matches {
        return defer_delivery_all(
            records,
            "server response did not contain one ordered result per event",
        );
    }
    for (result, (path, record)) in response.results.into_iter().zip(records) {
        match result.status {
            CodingAgentEventStatus::Accepted | CodingAgentEventStatus::Duplicate => {
                queue::remove(path)?;
            }
            CodingAgentEventStatus::Rejected => {
                let reason = result
                    .error
                    .unwrap_or_else(|| "server rejected event".into());
                reject(path, record, format!("permanently rejected: {reason}"))?;
            }
        }
    }
    Ok(false)
}

fn defer_blocked_all(records: &[(PathBuf, QueueRecord)], error: &str) -> Result<()> {
    for (path, record) in records {
        let mut record = record.clone();
        record.delivery_state = DeliveryState::Deferred;
        record.last_error = Some(error.to_string());
        record.updated_at = chrono::Utc::now();
        record.next_attempt_at = None;
        queue::update(path, &record)?;
    }
    Ok(())
}

fn defer_delivery_all(records: &[(PathBuf, QueueRecord)], error: &str) -> Result<bool> {
    let mut retry = false;
    for (path, record) in records {
        retry |= defer_delivery(path, record, error.to_string())?;
    }
    Ok(retry)
}

fn defer_delivery(path: &std::path::Path, record: &QueueRecord, error: String) -> Result<bool> {
    let mut record = record.clone();
    record.delivery_state = DeliveryState::Deferred;
    record.last_error = Some(error);
    record.updated_at = chrono::Utc::now();
    record.attempts = record.attempts.saturating_add(1);
    if record.attempts >= MAX_DELIVERY_ATTEMPTS {
        reject(
            path,
            &record,
            format!(
                "permanently rejected after {} delivery attempts: {}",
                record.attempts,
                record.last_error.as_deref().unwrap_or("delivery failed")
            ),
        )?;
        return Ok(false);
    }
    let delay = 1_i64 << record.attempts.clamp(1, 6).saturating_sub(1);
    record.next_attempt_at = Some(record.updated_at + chrono::Duration::seconds(delay));
    queue::update(path, &record)?;
    Ok(true)
}

fn reject(path: &std::path::Path, record: &QueueRecord, error: String) -> Result<()> {
    let mut record = record.clone();
    record.delivery_state = DeliveryState::Rejected;
    record.last_error = Some(error);
    record.updated_at = chrono::Utc::now();
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
        CodingAgentSource, CodingAgentTurn, coding_agent_event_id, coding_agent_session_id,
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
        let retry = sync_records(
            &config("original", "https://new.example", Some(&token(Uuid::nil()))),
            vec![queued.clone()],
        )
        .unwrap();
        assert!(
            !retry,
            "configuration mismatches wait for a later sync trigger"
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

        assert!(sync_records(&config, vec![first.clone(), second.clone()]).unwrap());
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
        )
        .unwrap();
        let retained = queue::load(&queued.0).unwrap();
        assert_eq!(retained.delivery_state, DeliveryState::Deferred);
        assert!(retained.last_error.unwrap().contains("one ordered result"));
    }

    #[test]
    fn fifth_delivery_failure_is_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = record("bound", "https://bound.example", "s", "retry-cap");
        record.attempts = MAX_DELIVERY_ATTEMPTS - 1;
        let queued = queued(&dir, record);

        assert!(!defer_delivery(&queued.0, &queued.1, "offline".into()).unwrap());
        assert!(!queued.0.exists());
        let rejected = dir.path().join("rejected");
        assert!(rejected.exists());
        let rejected_group = std::fs::read_dir(rejected)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let rejected_path = std::fs::read_dir(rejected_group)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let rejected_record = queue::load(&rejected_path).unwrap();
        assert_eq!(rejected_record.delivery_state, DeliveryState::Rejected);
        assert_eq!(rejected_record.attempts, MAX_DELIVERY_ATTEMPTS);
    }
}
