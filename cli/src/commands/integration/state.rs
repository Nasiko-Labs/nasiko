//! On-disk state for installed integrations, under `~/.nasiko/integrations/`.
//!
//! Two kinds of state live here:
//!
//! - `config.json` — one entry per installed agent: where to send spans and
//!   whether prompt text may be captured. Written by `install`, read by the
//!   hook on every report.
//! - `watermarks/<agent>/<session>.json` — stable turn ids already exported and
//!   persisted, so hooks remain idempotent across transcript reordering.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationBinding {
    pub cluster_name: String,
    pub cluster_url: String,
    pub principal_id: Uuid,
}

/// Per-agent settings recorded at install time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    /// Name this agent is registered under in the control plane.
    pub agent_name: String,
    /// Whether prompt text may be attached to spans.
    pub capture_content: bool,
    /// Version of the installed hook script.
    pub hook_version: u32,
    /// Immutable delivery destination selected by the explicit installation.
    /// Legacy state without this field fails closed and must be reinstalled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<InstallationBinding>,
}

/// Every installed integration, keyed by catalog id.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IntegrationState {
    #[serde(default)]
    pub agents: HashMap<String, AgentState>,
}

impl IntegrationState {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        create_parent_dir(&path)?;
        let content = serde_json::to_string_pretty(self)?;
        atomic_write(&path, content.as_bytes())
    }

    pub fn get(&self, agent_id: &str) -> Option<&AgentState> {
        self.agents.get(agent_id)
    }
}

// ─── Watermarks ──────────────────────────────────────────────────────────────

/// A process-wide exclusive lock for one session's reporting state.
///
/// Keep this guard alive across reading watermarks, exporting/uploading, and
/// advancing them so overlapping hook processes cannot perform the same work.
pub struct SessionLock {
    _file: File,
    watermark_path: PathBuf,
}

/// Independent stable-id progress for span export and message persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionProgress {
    pub exported_turn_ids: HashSet<String>,
    pub uploaded_turn_ids: HashSet<String>,
    pub captured_turn_ids: HashSet<String>,
    /// A count-only file cannot be mapped safely after transcript edits. Its
    /// first ID-aware run deliberately replays complete turns once.
    pub migrated_legacy_counts: bool,
}

impl SessionLock {
    #[cfg(test)]
    fn acquire(lock_path: &Path, watermark_path: PathBuf) -> Result<Self> {
        create_parent_dir(lock_path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .with_context(|| format!("failed to open {}", lock_path.display()))?;
        file.lock()
            .with_context(|| format!("failed to lock {}", lock_path.display()))?;
        Ok(Self {
            _file: file,
            watermark_path,
        })
    }

    fn acquire_with_timeout(
        lock_path: &Path,
        watermark_path: PathBuf,
        legacy_watermark_path: Option<&Path>,
        timeout: Duration,
    ) -> Result<Self> {
        create_parent_dir(lock_path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .with_context(|| format!("failed to open {}", lock_path.display()))?;
        let deadline = Instant::now() + timeout;
        while file.try_lock().is_err() {
            if Instant::now() >= deadline {
                anyhow::bail!("timed out locking {}", lock_path.display());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if !watermark_path.exists()
            && let Some(legacy_path) = legacy_watermark_path.filter(|path| path.exists())
        {
            let legacy = read_watermark_path(legacy_path);
            // Session-only IDs may belong to another adapter. Replay instead
            // of suppressing turns; Tempo and the server deduplicate retries.
            let conservative = Watermark {
                exported_turns: legacy
                    .exported_turn_ids
                    .as_ref()
                    .map_or(legacy.exported_turns, Vec::len),
                uploaded_messages: legacy
                    .uploaded_turn_ids
                    .as_ref()
                    .map_or(legacy.uploaded_messages, Vec::len),
                exported_turn_ids: None,
                uploaded_turn_ids: None,
                captured_turn_ids: None,
            };
            write_watermark_path(&watermark_path, &conservative)?;
        }
        Ok(Self {
            _file: file,
            watermark_path,
        })
    }

    /// Read progress. Legacy counts are intentionally not assigned to current
    /// IDs: truncation or reordering makes that mapping unknowable. Existing
    /// complete turns replay once, after which stable IDs govern all progress.
    pub fn progress(&self) -> Result<SessionProgress> {
        let mut current = read_watermark_path(&self.watermark_path);
        let mut migrated = false;
        let mut migrated_legacy_counts = false;
        if current.exported_turn_ids.is_none() {
            migrated_legacy_counts |= current.exported_turns > 0;
            current.exported_turn_ids = Some(Vec::new());
            migrated = true;
        }
        if current.uploaded_turn_ids.is_none() {
            migrated_legacy_counts |= current.uploaded_messages > 0;
            current.uploaded_turn_ids = Some(Vec::new());
            migrated = true;
        }
        if current.captured_turn_ids.is_none() {
            // Only turns completed by both old delivery paths can safely be
            // treated as captured by the replacement pipeline.
            let exported: HashSet<_> = current
                .exported_turn_ids
                .as_deref()
                .unwrap_or_default()
                .iter()
                .cloned()
                .collect();
            let captured = current
                .uploaded_turn_ids
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|id| exported.contains(*id))
                .cloned()
                .collect();
            current.captured_turn_ids = Some(captured);
            migrated = true;
        }
        if migrated {
            write_watermark_path(&self.watermark_path, &current)?;
        }
        Ok(SessionProgress {
            exported_turn_ids: current
                .exported_turn_ids
                .unwrap_or_default()
                .into_iter()
                .collect(),
            uploaded_turn_ids: current
                .uploaded_turn_ids
                .unwrap_or_default()
                .into_iter()
                .collect(),
            captured_turn_ids: current
                .captured_turn_ids
                .unwrap_or_default()
                .into_iter()
                .collect(),
            migrated_legacy_counts,
        })
    }

    pub fn mark_captured(&self, turn_ids: &[String]) -> Result<()> {
        self.mark(turn_ids, |watermark| &mut watermark.captured_turn_ids)
    }

    fn mark(
        &self,
        turn_ids: &[String],
        field: impl FnOnce(&mut Watermark) -> &mut Option<Vec<String>>,
    ) -> Result<()> {
        let mut watermark = read_watermark_path(&self.watermark_path);
        let ids = field(&mut watermark).get_or_insert_with(Vec::new);
        let mut known: HashSet<String> = ids.iter().cloned().collect();
        for turn_id in turn_ids {
            if known.insert(turn_id.clone()) {
                ids.push(turn_id.clone());
            }
        }
        watermark.exported_turns = watermark
            .exported_turn_ids
            .as_ref()
            .map_or(watermark.exported_turns, Vec::len);
        watermark.uploaded_messages = watermark
            .uploaded_turn_ids
            .as_ref()
            .map_or(watermark.uploaded_messages, Vec::len);
        write_watermark_path(&self.watermark_path, &watermark)
    }
}

/// Acquire the per-session report lock. `report.rs` should hold this guard for
/// its complete read/export/upload/watermark transaction.
pub fn lock_session(agent_id: &str, session_id: &str, timeout: Duration) -> Result<SessionLock> {
    let watermark = watermark_path(agent_id, session_id);
    SessionLock::acquire_with_timeout(
        &watermark.with_extension("lock"),
        watermark,
        Some(&legacy_watermark_path(session_id)),
        timeout,
    )
}

fn read_watermark_path(path: &Path) -> Watermark {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn write_watermark_path(path: &Path, watermark: &Watermark) -> Result<()> {
    atomic_write(path, serde_json::to_string(watermark)?.as_bytes())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Watermark {
    #[serde(default)]
    exported_turns: usize,
    #[serde(default)]
    uploaded_messages: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exported_turn_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uploaded_turn_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    captured_turn_ids: Option<Vec<String>>,
}

// ─── Paths ───────────────────────────────────────────────────────────────────

/// Root of Nasiko's integration state.
pub fn integrations_dir() -> PathBuf {
    super::catalog::home().join(".nasiko").join("integrations")
}

/// Where the hook writes its own diagnostics. A hook must never print to the
/// coding agent's stdout, so failures go here instead.
pub fn log_path() -> PathBuf {
    integrations_dir().join("report.log")
}

fn config_path() -> PathBuf {
    integrations_dir().join("config.json")
}

/// Session ids come from the coding agent, so they are sanitised before being
/// used as a filename — a `../` in an id must not escape the state directory.
fn safe_component(value: &str) -> String {
    let safe: String = value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if safe == value && !safe.is_empty() {
        safe
    } else {
        let digest = hex::encode(Sha256::digest(value.as_bytes()));
        format!("{safe}-{}", &digest[..12])
    }
}

fn watermark_path(agent_id: &str, session_id: &str) -> PathBuf {
    integrations_dir()
        .join("watermarks")
        .join(safe_component(agent_id))
        .join(format!("{}.json", safe_component(session_id)))
}

fn legacy_watermark_path(session_id: &str) -> PathBuf {
    integrations_dir()
        .join("watermarks")
        .join(format!("{}.json", safe_component(session_id)))
}

fn create_parent_dir(path: &std::path::Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    create_parent_dir(path)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
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
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitises_a_traversing_session_id_into_one_filename() {
        let path = watermark_path("claude", "../../etc/passwd");

        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("------etc-passwd-"));
        assert!(name.ends_with(".json"));
        assert!(path.starts_with(integrations_dir()));
    }

    #[test]
    fn distinct_unsafe_session_ids_do_not_share_state_or_locks() {
        assert_ne!(
            watermark_path("claude", "a/b"),
            watermark_path("claude", "a-b")
        );
        assert_ne!(watermark_path("claude", ""), watermark_path("claude", "-"));
        assert_ne!(
            watermark_path("claude", "s"),
            watermark_path("opencode", "s")
        );
    }

    #[test]
    fn keeps_a_normal_uuid_session_id_readable() {
        let path = watermark_path("claude", "2a0bd16a-0c3b-4493-9c86-51d91ed27fd9");

        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "2a0bd16a-0c3b-4493-9c86-51d91ed27fd9.json"
        );
    }

    #[test]
    fn returns_no_agent_state_before_install() {
        assert!(IntegrationState::default().get("claude").is_none());
    }

    #[test]
    fn legacy_agent_state_loads_without_inventing_a_destination() {
        let state: IntegrationState = serde_json::from_str(
            r#"{"agents":{"claude":{"agent_name":"claude-code","capture_content":true,"hook_version":1}}}"#,
        )
        .unwrap();

        assert!(state.get("claude").unwrap().binding.is_none());
    }

    fn test_lock(dir: &Path) -> SessionLock {
        SessionLock::acquire(&dir.join("session.lock"), dir.join("session.json")).unwrap()
    }

    #[test]
    fn session_lock_wait_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = test_lock(dir.path());

        let result = SessionLock::acquire_with_timeout(
            &dir.path().join("session.lock"),
            dir.path().join("session.json"),
            None,
            Duration::from_millis(20),
        );
        assert!(result.is_err());
    }

    #[test]
    fn legacy_counts_trigger_a_conservative_one_time_replay() {
        let dir = tempfile::tempdir().unwrap();
        let guard = test_lock(dir.path());
        std::fs::write(
            dir.path().join("session.json"),
            r#"{"exported_turns":2,"uploaded_messages":1}"#,
        )
        .unwrap();
        let progress = guard.progress().unwrap();
        assert!(progress.exported_turn_ids.is_empty());
        assert!(progress.uploaded_turn_ids.is_empty());
        assert!(progress.captured_turn_ids.is_empty());
        assert!(progress.migrated_legacy_counts);

        let second = guard.progress().unwrap();
        assert!(!second.migrated_legacy_counts);
    }

    #[test]
    fn session_only_id_progress_is_replayed_into_agent_scoped_state() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.json");
        let scoped = dir.path().join("claude").join("session.json");
        std::fs::write(
            &legacy,
            r#"{"exported_turn_ids":["a"],"uploaded_turn_ids":["a"]}"#,
        )
        .unwrap();
        let guard = SessionLock::acquire_with_timeout(
            &scoped.with_extension("lock"),
            scoped,
            Some(&legacy),
            Duration::from_secs(1),
        )
        .unwrap();

        let progress = guard.progress().unwrap();
        assert!(progress.migrated_legacy_counts);
        assert!(progress.exported_turn_ids.is_empty());
        assert!(progress.uploaded_turn_ids.is_empty());
        assert!(progress.captured_turn_ids.is_empty());
    }

    #[test]
    fn captured_ids_migrate_conservatively_from_completed_old_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let guard = test_lock(dir.path());
        std::fs::write(
            dir.path().join("session.json"),
            r#"{"exported_turn_ids":["a","b"],"uploaded_turn_ids":["b","c"]}"#,
        )
        .unwrap();

        let progress = guard.progress().unwrap();
        assert_eq!(progress.captured_turn_ids, HashSet::from(["b".to_string()]));
        guard.mark_captured(&["d".to_string()]).unwrap();
        assert_eq!(
            guard.progress().unwrap().captured_turn_ids,
            HashSet::from(["b".to_string(), "d".to_string()])
        );
    }

    #[test]
    fn atomic_write_leaves_only_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("watermark.json");
        atomic_write(&path, br#"{"exported_turns":2}"#).unwrap();

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            r#"{"exported_turns":2}"#
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
