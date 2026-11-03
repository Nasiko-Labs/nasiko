//! The import `nasiko agents setup` leaves running when it returns: a copy of this binary started
//! detached (`nasiko agents import-worker`), so closing the terminal doesn't stop it, writing what it
//! prints to [`IMPORT_LOG_FILE`] and how far it has got to [`IMPORT_STATUS_FILE`] for
//! `nasiko agents status`.
//!
//! One import runs at a time on a computer: every import (background or foreground) holds
//! [`ImportLock`] while it runs, and one that can't get it leaves the import to the one that has it.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::backlog;
use super::backlog::Backlog;
use super::import::{ImportRun, WindowStatus};
use super::queue;

/// Held by the running import.
const IMPORT_LOCK_FILE: &str = "import.lock";
/// What the background import prints; started afresh by each one.
pub const IMPORT_LOG_FILE: &str = "import.log";
/// How far the running (or last) import has got.
const IMPORT_STATUS_FILE: &str = "import-status.json";
/// The hidden command a detached import runs as.
const WORKER_ARGS: [&str; 2] = ["agents", "import-worker"];
/// The status file is rewritten at most this often, except when the import ends.
const STATUS_INTERVAL: Duration = Duration::from_secs(1);
/// How many of the log's last lines `nasiko agents status` shows.
const LOG_TAIL_LINES: usize = 8;

/// Held while an import runs; released when dropped (or when the process ends, however it ends).
pub struct ImportLock {
    _file: File,
}

impl ImportLock {
    /// The lock, or `None` when another import holds it.
    pub fn try_acquire(dir: &Path) -> Result<Option<Self>> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let path = dir.join(IMPORT_LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        Ok(file.try_lock().is_ok().then_some(Self { _file: file }))
    }
}

/// Whether an import is running on this computer now.
pub fn import_running(dir: &Path) -> Result<bool> {
    Ok(ImportLock::try_acquire(dir)?.is_none())
}

/// Starts the import as a detached copy of this binary writing to [`IMPORT_LOG_FILE`] in `dir`, and
/// returns its process id and the log's path.
pub fn spawn_import(dir: &Path) -> Result<(u32, PathBuf)> {
    let log_path = dir.join(IMPORT_LOG_FILE);
    let log = owner_only_create(&log_path)?;
    let exe = std::env::current_exe().context("couldn't find this program to run the import")?;
    let mut command = Command::new(exe);
    command
        .args(WORKER_ARGS)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    detach(&mut command);
    let child = command
        .spawn()
        .context("couldn't start the import in the background")?;
    Ok((child.id(), log_path))
}

fn owner_only_create(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))
}

/// Its own process group, so the terminal closing (or Ctrl-C in it) doesn't reach it.
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

/// No console, and its own process group, so closing the console doesn't reach it.
#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}

// ─── Progress on disk ────────────────────────────────────────────────────────

/// How far an import has got, as `nasiko agents status` reads it. Reading and delivery are counted
/// apart, so an import whose deliveries lag its reading is seen to be moving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportStatus {
    pub pid: u32,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Windows read whose turns have all been delivered or set aside.
    pub windows_done: usize,
    /// Windows whose sessions have all been read.
    #[serde(default)]
    pub windows_read: usize,
    pub windows_total: usize,
    /// The window being read, as `Oct 17 · 10/10 sessions`; empty when none is.
    #[serde(default)]
    pub reading: String,
    /// Turns the server took during this import.
    #[serde(default)]
    pub delivered: usize,
    /// What delivery waits for once every window is read; empty before then.
    pub line: String,
    /// Set once the import has returned.
    pub finished: bool,
}

/// Keeps [`IMPORT_STATUS_FILE`] current for one import, at most every [`STATUS_INTERVAL`].
pub struct StatusFile {
    path: PathBuf,
    started_at: DateTime<Utc>,
    last_write: Option<Instant>,
}

impl StatusFile {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join(IMPORT_STATUS_FILE),
            started_at: Utc::now(),
            last_write: None,
        }
    }

    /// Records `run` (`line` says what its delivery waits for), unless it was recorded less than
    /// [`STATUS_INTERVAL`] ago. Best effort: a status that can't be written is no reason to stop.
    pub fn update(&mut self, run: &ImportRun, line: &str) {
        if self
            .last_write
            .is_some_and(|at| at.elapsed() < STATUS_INTERVAL)
        {
            return;
        }
        self.write(run, line, false);
    }

    /// Records the import's end.
    pub fn finish(&mut self, run: &ImportRun, line: &str) {
        self.write(run, line, true);
    }

    fn write(&mut self, run: &ImportRun, line: &str, finished: bool) {
        self.last_write = Some(Instant::now());
        let status = ImportStatus {
            pid: std::process::id(),
            started_at: self.started_at,
            updated_at: Utc::now(),
            windows_done: run.windows_done(),
            windows_read: run.windows_read(),
            windows_total: run.windows.len(),
            reading: reading(run),
            delivered: run.delivered,
            line: line.to_string(),
            finished,
        };
        if let Ok(bytes) = serde_json::to_vec(&status) {
            let _ = queue::atomic_owner_write(&self.path, &bytes);
        }
    }
}

// ─── Progress lines ──────────────────────────────────────────────────────────

/// The window `run` is reading, as `Oct 17 · 10/10 sessions`; empty when it reads none.
pub fn reading(run: &ImportRun) -> String {
    run.windows
        .iter()
        .find(|window| window.status == WindowStatus::Reading)
        .map(|window| {
            format!(
                "{} · {}/{} sessions",
                window.label, window.done, window.sessions
            )
        })
        .unwrap_or_default()
}

/// `Read 9/23 windows · Oct 17 · 10/10 sessions`: how far reading has got (`reading` is the window
/// being read, if one is).
pub fn read_line(read: usize, total: usize, reading: &str) -> String {
    let windows = format!("Read {read}/{total} windows");
    if reading.is_empty() {
        windows
    } else {
        format!("{windows} · {reading}")
    }
}

/// `Delivered 1,730 turns · 499 waiting · 0 set aside`: how far delivery has got.
pub fn delivered_line(delivered: usize, backlog: &Backlog) -> String {
    format!(
        "Delivered {} turns · {} waiting · {} set aside",
        thousands(delivered),
        thousands(backlog.pending()),
        thousands(rejected(backlog))
    )
}

fn rejected(backlog: &Backlog) -> usize {
    backlog.agents.values().map(|agent| agent.rejected).sum()
}

/// `n` with its thousands grouped: `1,730`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn read_status(dir: &Path) -> Option<ImportStatus> {
    serde_json::from_slice(&std::fs::read(dir.join(IMPORT_STATUS_FILE)).ok()?).ok()
}

// ─── `nasiko agents status` ──────────────────────────────────────────────────

/// What `nasiko agents status` prints for the queue under `dir`: the import (running or last) with its
/// reading and its delivery apart, what is waiting to send and set aside, and the end of the import's
/// log. `fit` cuts a progress line to the terminal's width.
pub fn status_report(dir: &Path, fit: impl Fn(&str) -> String) -> Result<Vec<String>> {
    let running = import_running(dir)?;
    let status = read_status(dir);
    let backlog = backlog::backlog_at(dir)?;
    let rejected = rejected(&backlog);
    let mut lines = vec![import_line(running, status.as_ref())];
    match status.filter(|s| running && !s.finished) {
        Some(status) => {
            let progress = [
                read_line(status.windows_read, status.windows_total, &status.reading),
                format!(
                    "{} · {}/{} windows complete",
                    delivered_line(status.delivered, &backlog),
                    status.windows_done,
                    status.windows_total
                ),
                status.line,
            ];
            lines.extend(
                progress
                    .iter()
                    .filter(|line| !line.is_empty())
                    .map(|line| fit(&format!("  {line}"))),
            );
        }
        None => lines.push(format!(
            "Queue: {} turn(s) waiting to send, {rejected} set aside",
            backlog.pending()
        )),
    }
    for (agent_id, agent) in &backlog.agents {
        for (reason, count) in &agent.reasons {
            lines.push(format!("  {agent_id}: {count} × {reason}"));
        }
    }
    if rejected > 0 {
        lines.push("  Try them again with: nasiko agents sync --retry-rejected".into());
    }
    let log = dir.join(IMPORT_LOG_FILE);
    let tail = log_tail(&log, LOG_TAIL_LINES);
    if !tail.is_empty() {
        lines.push(format!("Log: {}", log.display()));
        lines.extend(tail.into_iter().map(|line| format!("  {line}")));
    }
    Ok(lines)
}

fn import_line(running: bool, status: Option<&ImportStatus>) -> String {
    let windows = |s: &ImportStatus| {
        format!(
            "{}/{} windows done, {} turns delivered",
            s.windows_done,
            s.windows_total,
            thousands(s.delivered)
        )
    };
    match (running, status) {
        (true, Some(s)) if !s.finished => format!(
            "Import: running (pid {}), started {}",
            s.pid,
            s.started_at.format("%Y-%m-%d %H:%M UTC"),
        ),
        (true, _) => "Import: running".into(),
        (false, Some(s)) if s.finished => format!(
            "Import: not running; the last one ended {}, {}",
            s.updated_at.format("%Y-%m-%d %H:%M UTC"),
            windows(s)
        ),
        (false, Some(s)) => format!(
            "Import: not running; the last one stopped at {}, {}. Resume it with: nasiko agents setup",
            s.updated_at.format("%Y-%m-%d %H:%M UTC"),
            windows(s)
        ),
        (false, None) => "Import: not running".into(),
    }
}

/// The last `count` lines of the file at `path`; none when it can't be read.
fn log_tail(path: &Path, count: usize) -> Vec<String> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let lines: Vec<String> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(count)..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_import_holds_the_lock_at_a_time() {
        let dir = tempfile::tempdir().unwrap();

        let held = ImportLock::try_acquire(dir.path()).unwrap();
        assert!(held.is_some());
        assert!(import_running(dir.path()).unwrap());
        assert!(ImportLock::try_acquire(dir.path()).unwrap().is_none());

        drop(held);
        assert!(!import_running(dir.path()).unwrap());
    }

    use super::super::import::WindowProgress;

    /// An import that has read two of three windows, confirmed one, is reading the third, and has
    /// had 1,730 turns delivered.
    fn reading_run() -> ImportRun {
        let mut reading = WindowProgress::for_test("Oct 17", WindowStatus::Reading, 10, 0);
        reading.done = 4;
        ImportRun {
            windows: vec![
                WindowProgress::for_test("Today", WindowStatus::Done, 2, 3),
                WindowProgress::for_test("Yesterday", WindowStatus::Sending, 5, 9),
                reading,
            ],
            delivered: 1_730,
            ..ImportRun::default()
        }
    }

    #[test]
    fn the_status_file_records_reading_and_delivery_apart() {
        let dir = tempfile::tempdir().unwrap();
        let run = reading_run();
        let mut file = StatusFile::new(dir.path());

        file.update(&run, "");
        file.update(&ImportRun::default(), "throttled away");
        let status = read_status(dir.path()).unwrap();

        assert_eq!(
            (
                status.windows_read,
                status.windows_done,
                status.windows_total
            ),
            (2, 1, 3)
        );
        assert_eq!(status.reading, "Oct 17 · 4/10 sessions");
        assert_eq!(status.delivered, 1_730);
        assert_eq!(status.line, "");
        assert!(!status.finished);
        file.finish(&run, "done");
        assert!(read_status(dir.path()).unwrap().finished);
    }

    #[test]
    fn a_status_file_from_before_reading_and_delivery_were_counted_apart_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let old = r#"{"pid":7,"started_at":"2026-10-21T08:00:00Z","updated_at":"2026-10-21T08:05:00Z",
            "windows_done":3,"windows_total":9,"line":"","finished":false}"#;
        std::fs::write(dir.path().join(IMPORT_STATUS_FILE), old).unwrap();

        let status = read_status(dir.path()).unwrap();

        assert_eq!((status.windows_read, status.delivered), (0, 0));
        assert_eq!(status.reading, "");
    }

    #[test]
    fn status_shows_reading_and_delivery_apart_the_queue_and_the_end_of_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        std::fs::write(dir.path().join(IMPORT_LOG_FILE), log).unwrap();
        let _lock = ImportLock::try_acquire(dir.path()).unwrap().unwrap();
        let mut file = StatusFile::new(dir.path());
        file.update(&reading_run(), "");

        let report = status_report(dir.path(), str::to_string).unwrap();

        assert!(report[0].starts_with("Import: running (pid "));
        assert!(!report[0].contains("windows"), "{}", report[0]);
        assert_eq!(report[1], "  Read 2/3 windows · Oct 17 · 4/10 sessions");
        assert_eq!(
            report[2],
            "  Delivered 1,730 turns · 0 waiting · 0 set aside · 1/3 windows complete"
        );
        assert_eq!(
            report[3],
            format!("Log: {}", dir.path().join(IMPORT_LOG_FILE).display())
        );
        assert_eq!(report.last().unwrap(), "  line 20");
        assert_eq!(report.len(), 4 + LOG_TAIL_LINES);
    }

    #[test]
    fn status_progress_lines_are_cut_to_fit() {
        let dir = tempfile::tempdir().unwrap();
        let _lock = ImportLock::try_acquire(dir.path()).unwrap().unwrap();
        let mut file = StatusFile::new(dir.path());
        file.update(&reading_run(), "");

        let report =
            status_report(dir.path(), |line| line.chars().take(20).collect::<String>()).unwrap();

        assert!(report[1].starts_with("  Read 2/3 windows"));
        assert!(report[1..=2].iter().all(|line| line.chars().count() <= 20));
    }

    #[test]
    fn counts_are_grouped_by_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_730), "1,730");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn the_read_line_names_the_window_being_read_only_while_one_is() {
        assert_eq!(read_line(23, 23, ""), "Read 23/23 windows");
        assert_eq!(
            read_line(9, 23, "Oct 17 · 10/10 sessions"),
            "Read 9/23 windows · Oct 17 · 10/10 sessions"
        );
    }

    #[test]
    fn a_stopped_import_says_how_to_resume() {
        let now = Utc::now();
        let stopped = ImportStatus {
            pid: 1,
            started_at: now,
            updated_at: now,
            windows_done: 3,
            windows_read: 5,
            windows_total: 9,
            reading: String::new(),
            delivered: 1_730,
            line: String::new(),
            finished: false,
        };

        let line = import_line(false, Some(&stopped));

        assert!(line.contains("3/9 windows done, 1,730 turns delivered"));
        assert!(line.contains("nasiko agents setup"));
    }
}
