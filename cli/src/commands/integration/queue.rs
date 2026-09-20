//! Durable, destination-bound queue for coding-agent events.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use nasiko_types::CodingAgentEventV1;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use uuid::Uuid;

const MAX_SCAN_RECORDS: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueDestination {
    pub cluster_name: String,
    pub cluster_url: String,
    pub principal_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Pending,
    Deferred,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueRecord {
    pub destination: QueueDestination,
    pub delivery_state: DeliveryState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub event: CodingAgentEventV1,
}

impl QueueRecord {
    pub fn new(destination: QueueDestination, event: CodingAgentEventV1) -> Self {
        let now = Utc::now();
        Self {
            destination,
            delivery_state: DeliveryState::Pending,
            last_error: None,
            created_at: now,
            updated_at: now,
            attempts: 0,
            next_attempt_at: None,
            event,
        }
    }
}

pub struct SyncLock {
    _file: File,
}

pub fn enqueue(record: &QueueRecord) -> Result<PathBuf> {
    enqueue_at(&super::state::integrations_dir(), record)
}

pub fn load(path: &Path) -> Result<QueueRecord> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let record: QueueRecord = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    validate_record(&record)?;
    Ok(record)
}

pub fn update(path: &Path, record: &QueueRecord) -> Result<()> {
    validate_record(record)?;
    atomic_owner_write(path, &serde_json::to_vec(record)?)
}

pub fn remove(path: &Path) -> Result<()> {
    std::fs::remove_file(path).with_context(|| format!("failed to remove {}", path.display()))
}

pub fn quarantine(path: &Path, record: &QueueRecord) -> Result<PathBuf> {
    let root = path
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "queue"))
        .and_then(Path::parent)
        .unwrap_or_else(|| path.parent().unwrap_or_else(|| Path::new(".")));
    quarantine_at(root, path, record)
}

pub fn reject_invalid(record: &QueueRecord, error: &str) -> Result<PathBuf> {
    reject_invalid_at(&super::state::integrations_dir(), record, error)
}

pub fn records() -> Result<Vec<(PathBuf, QueueRecord)>> {
    records_at(&super::state::integrations_dir())
}

pub fn has_pending_records() -> Result<bool> {
    has_pending_records_at(&super::state::integrations_dir())
}

pub fn acquire_sync_lock(timeout: Duration) -> Result<SyncLock> {
    acquire_sync_lock_at(&super::state::integrations_dir(), timeout)
}

fn enqueue_at(root: &Path, record: &QueueRecord) -> Result<PathBuf> {
    validate_record(record)?;
    let path = record_path(root, &record.destination, &record.event.event_id);
    create_owner_dirs(&root.join("queue"))?;
    atomic_owner_write(&path, &serde_json::to_vec(record)?)?;
    Ok(path)
}

fn validate_record(record: &QueueRecord) -> Result<()> {
    if record.destination.cluster_name.trim().is_empty()
        || record.destination.cluster_url.trim().is_empty()
    {
        bail!("queue destination cluster name and URL must not be empty");
    }
    record.event.validate().map_err(anyhow::Error::msg)
}

fn cluster_hash(destination: &QueueDestination) -> String {
    let identity = format!(
        "{}\0{}\0{}",
        destination.cluster_name,
        destination.cluster_url.trim_end_matches('/'),
        destination.principal_id,
    );
    hex::encode(Sha256::digest(identity.as_bytes()))[..24].to_string()
}

fn record_path(root: &Path, destination: &QueueDestination, event_id: &str) -> PathBuf {
    root.join("queue")
        .join(cluster_hash(destination))
        .join(format!("{event_id}.json"))
}

fn records_at(root: &Path) -> Result<Vec<(PathBuf, QueueRecord)>> {
    let queue = root.join("queue");
    if !queue.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    let now = Utc::now();
    for cluster in std::fs::read_dir(&queue)? {
        let cluster = cluster?;
        if !cluster.file_type()?.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(cluster.path())? {
            let entry = entry?;
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                match load(&path) {
                    Ok(record)
                        if record
                            .next_attempt_at
                            .is_none_or(|next_attempt| next_attempt <= now) =>
                    {
                        records.push((path, record));
                        if records.len() >= MAX_SCAN_RECORDS {
                            records.sort_by(|left, right| left.0.cmp(&right.0));
                            return Ok(records);
                        }
                    }
                    Ok(_) => {}
                    Err(_) => quarantine_invalid_at(root, &path)?,
                }
            }
        }
    }
    records.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(records)
}

fn has_pending_records_at(root: &Path) -> Result<bool> {
    let queue = root.join("queue");
    if !queue.exists() {
        return Ok(false);
    }
    for cluster in std::fs::read_dir(queue)? {
        let cluster = cluster?;
        if !cluster.file_type()?.is_dir() {
            continue;
        }
        if std::fs::read_dir(cluster.path())?.any(|entry| {
            entry.ok().is_some_and(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn quarantine_at(root: &Path, path: &Path, record: &QueueRecord) -> Result<PathBuf> {
    let destination = root
        .join("rejected")
        .join(cluster_hash(&record.destination))
        .join(path.file_name().context("queue record has no filename")?);
    create_owner_dirs(destination.parent().expect("quarantine has parent"))?;
    std::fs::rename(path, &destination)
        .with_context(|| format!("failed to quarantine {}", path.display()))?;
    Ok(destination)
}

fn reject_invalid_at(root: &Path, record: &QueueRecord, error: &str) -> Result<PathBuf> {
    let mut rejected = record.clone();
    rejected.delivery_state = DeliveryState::Rejected;
    rejected.last_error = Some(format!("invalid event: {error}"));
    rejected.updated_at = Utc::now();
    let destination = root
        .join("rejected")
        .join(cluster_hash(&record.destination))
        .join(format!("{}.json", record.event.event_id));
    create_owner_dirs(destination.parent().expect("rejected event has parent"))?;
    atomic_owner_write(&destination, &serde_json::to_vec(&rejected)?)?;
    Ok(destination)
}

fn quarantine_invalid_at(root: &Path, path: &Path) -> Result<()> {
    let cluster = path
        .parent()
        .and_then(Path::file_name)
        .context("invalid queue record has no cluster directory")?;
    let destination = root.join("rejected").join("malformed").join(cluster).join(
        path.file_name()
            .context("invalid queue record has no filename")?,
    );
    create_owner_dirs(destination.parent().expect("invalid quarantine has parent"))?;
    std::fs::rename(path, &destination)
        .with_context(|| format!("failed to quarantine malformed record {}", path.display()))?;
    Ok(())
}

fn acquire_sync_lock_at(root: &Path, timeout: Duration) -> Result<SyncLock> {
    let path = root.join("queue").join(".sync.lock");
    create_owner_dirs(path.parent().expect("lock has parent"))?;
    let file = owner_open(&path, false)?;
    let deadline = Instant::now() + timeout;
    while file.try_lock().is_err() {
        if Instant::now() >= deadline {
            bail!("timed out locking {}", path.display());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    Ok(SyncLock { _file: file })
}

fn create_owner_dirs(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn owner_open(path: &Path, truncate: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .create(true)
        .write(true)
        .read(true)
        .truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

fn atomic_owner_write(path: &Path, bytes: &[u8]) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().context("queue record has no parent")?;
    create_owner_dirs(parent)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("record");
    let temp = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.with_context(|| format!("failed to atomically write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use nasiko_types::{
        CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentSession, CodingAgentSource,
        CodingAgentTurn, coding_agent_event_id,
    };

    fn record(cluster_name: &str, cluster_url: &str) -> QueueRecord {
        let at = Utc.timestamp_opt(1, 0).unwrap();
        let event_id = coding_agent_event_id("claude", "session", "turn");
        QueueRecord::new(
            QueueDestination {
                cluster_name: cluster_name.into(),
                cluster_url: cluster_url.into(),
                principal_id: Uuid::nil(),
            },
            CodingAgentEventV1 {
                version: CODING_AGENT_EVENT_VERSION,
                event_id,
                captured_at: at,
                source: CodingAgentSource {
                    agent_id: "claude".into(),
                    agent_name: "claude-code".into(),
                },
                session: CodingAgentSession {
                    id: "claude:session".into(),
                    source_id: "session".into(),
                },
                turn: CodingAgentTurn {
                    id: "turn".into(),
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

    #[test]
    fn destination_binding_and_path_survive_an_active_switch() {
        let dir = tempfile::tempdir().unwrap();
        let original = record("one", "https://one.example");
        let path = enqueue_at(dir.path(), &original).unwrap();
        let _new_active = record("two", "https://two.example");

        assert_eq!(load(&path).unwrap().destination, original.destination);
        let encoded = std::fs::read_to_string(&path).unwrap();
        assert!(!encoded.contains("token"));
        assert!(
            path.starts_with(
                dir.path()
                    .join("queue")
                    .join(cluster_hash(&original.destination))
            )
        );
    }

    #[test]
    fn duplicate_events_share_one_atomic_queue_file() {
        let dir = tempfile::tempdir().unwrap();
        let record = record("one", "https://one.example");
        let first = enqueue_at(dir.path(), &record).unwrap();
        let second = enqueue_at(dir.path(), &record).unwrap();

        assert_eq!(first, second);
        assert_eq!(records_at(dir.path()).unwrap().len(), 1);
        assert_eq!(
            std::fs::read_dir(first.parent().unwrap()).unwrap().count(),
            1
        );
        assert_eq!(load(&first).unwrap().event.event_id, record.event.event_id);
    }

    #[test]
    fn atomic_update_survives_and_remove_completes_the_record_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut record = record("one", "https://one.example");
        let path = enqueue_at(dir.path(), &record).unwrap();
        record.delivery_state = DeliveryState::Deferred;
        record.last_error = Some("offline".into());

        update(&path, &record).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.delivery_state, DeliveryState::Deferred);
        assert_eq!(loaded.last_error.as_deref(), Some("offline"));
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );

        remove(&path).unwrap();
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn queue_files_and_directories_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = enqueue_at(dir.path(), &record("one", "https://one.example")).unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(dir.path().join("queue"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn global_sync_lock_wait_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = acquire_sync_lock_at(dir.path(), Duration::from_secs(1)).unwrap();
        assert!(acquire_sync_lock_at(dir.path(), Duration::from_millis(20)).is_err());
    }

    #[test]
    fn malformed_records_are_quarantined_without_blocking_ready_records() {
        let dir = tempfile::tempdir().unwrap();
        let malformed = enqueue_at(dir.path(), &record("one", "https://one.example")).unwrap();
        std::fs::write(&malformed, b"not-json").unwrap();

        let mut ready = record("one", "https://one.example");
        ready.event.turn.id = "ready".into();
        ready.event.event_id = coding_agent_event_id("claude", "session", "ready");
        let ready_path = enqueue_at(dir.path(), &ready).unwrap();

        let records = records_at(dir.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].0, ready_path);
        assert!(!malformed.exists());
        assert!(dir.path().join("rejected").join("malformed").exists());
    }

    #[test]
    fn invalid_event_can_be_quarantined_before_it_enters_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let mut invalid = record("one", "https://one.example");
        invalid.event.source.agent_name = "bad\0agent".into();
        let error = invalid.event.validate().unwrap_err();

        let rejected = reject_invalid_at(dir.path(), &invalid, &error).unwrap();
        assert!(rejected.starts_with(dir.path().join("rejected")));
        assert!(!dir.path().join("queue").exists());
        let stored: QueueRecord =
            serde_json::from_slice(&std::fs::read(rejected).unwrap()).unwrap();
        assert_eq!(stored.delivery_state, DeliveryState::Rejected);
        assert!(stored.last_error.unwrap().contains("NUL"));
    }

    #[test]
    fn future_retry_does_not_hide_a_ready_record() {
        let dir = tempfile::tempdir().unwrap();
        let mut deferred = record("one", "https://one.example");
        deferred.next_attempt_at = Some(Utc::now() + chrono::Duration::hours(1));
        enqueue_at(dir.path(), &deferred).unwrap();

        let mut ready = record("one", "https://one.example");
        ready.event.turn.id = "ready".into();
        ready.event.event_id = coding_agent_event_id("claude", "session", "ready");
        enqueue_at(dir.path(), &ready).unwrap();

        let records = records_at(dir.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].1.event.turn.id, "ready");
        assert!(has_pending_records_at(dir.path()).unwrap());
    }
}
