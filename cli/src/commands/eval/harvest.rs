//! `nasiko eval local harvest`: finished Claude Code sessions → candidate tasks.
//!
//! For each session log of the repo: read it, find the commits made during it (or up to two
//! hours after), and let the shared harvest steps (`nasiko_evals::harvest`) link and draft the
//! task. This module only reads the logs and asks local git for what those steps need.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
pub use nasiko_evals::harvest::Outcome;
use nasiko_evals::harvest::{self, FileVersions};
use nasiko_evals::link::{self, LinkDecision};
use nasiko_evals::session::claude_code::parse_claude_code_log;
use nasiko_evals::{SessionSource, SplitPatch, TaskInstance, TaskStatus};
use serde::Serialize;

use super::git::{Repo, TimeWindow};
use super::store::Store;

/// Inputs of one harvest.
pub struct HarvestArgs {
    pub repo: PathBuf,
    /// Only sessions that started at or after this time.
    pub since: Option<DateTime<Utc>>,
    /// Where Claude Code keeps its logs; default `~/.claude/projects`.
    pub log_dir: Option<PathBuf>,
    /// Look at sessions again even if an earlier harvest already did.
    pub redo: Redo,
}

/// Whether to revisit sessions an earlier harvest already looked at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Redo {
    Skip,
    All,
}

/// Result of one harvest.
pub struct HarvestSummary {
    pub repo: String,
    pub logs_found: usize,
    pub skipped_seen: usize,
    pub outcomes: BTreeMap<Outcome, usize>,
    pub new_tasks: Vec<String>,
}

/// Runs one harvest and writes its tasks.
pub fn harvest(store: &Store, args: &HarvestArgs) -> Result<HarvestSummary> {
    let repo = Repo::open(&args.repo)?;
    let slug = repo.slug();
    let roots: Vec<String> = repo
        .worktree_roots()?
        .iter()
        .map(|r| r.to_string_lossy().into_owned())
        .collect();
    let log_root = match &args.log_dir {
        Some(dir) => dir.clone(),
        None => default_log_root()?,
    };
    let logs = find_logs(&log_root)?;
    let mut ledger = store.load_ledger(&slug)?;
    let mut summary = HarvestSummary {
        repo: slug.clone(),
        logs_found: logs.len(),
        skipped_seen: 0,
        outcomes: BTreeMap::new(),
        new_tasks: Vec::new(),
    };
    let _status =
        nasiko_utils::term::start_status(format!("harvesting {} session logs", logs.len()));
    for log in &logs {
        let key = log
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if args.redo == Redo::Skip && ledger.contains_key(&key) {
            summary.skipped_seen += 1;
            continue;
        }
        let outcome = match harvest_one(store, &repo, &roots, log, args.since) {
            Ok(Some((outcome, task_ids))) => {
                summary.new_tasks.extend(task_ids);
                outcome
            }
            Ok(None) => continue,
            Err(e) => {
                eprintln!("warning: {}: {e:#}", log.display());
                Outcome::Unreadable
            }
        };
        *summary.outcomes.entry(outcome).or_default() += 1;
        ledger.insert(key, outcome.label().to_string());
    }
    store.save_ledger(&slug, &ledger)?;
    Ok(summary)
}

fn default_log_root() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home folder")?
        .join(".claude")
        .join("projects"))
}

/// Every session log under `log_root` (one folder per project). A session started anywhere may
/// have edited this repo, so all are read; the ones without edits here are dropped later.
fn find_logs(log_root: &Path) -> Result<Vec<PathBuf>> {
    let mut logs = Vec::new();
    let Ok(entries) = fs::read_dir(log_root) else {
        return Ok(logs);
    };
    for entry in entries.flatten().filter(|e| e.path().is_dir()) {
        for file in fs::read_dir(entry.path())?.flatten() {
            let path = file.path();
            if path.extension().is_some_and(|e| e == "jsonl") {
                logs.push(path);
            }
        }
    }
    logs.sort();
    Ok(logs)
}

/// `Ok(None)` when the session is outside the `--since` window (not recorded in the ledger).
fn harvest_one(
    store: &Store,
    repo: &Repo,
    roots: &[String],
    log: &Path,
    since: Option<DateTime<Utc>>,
) -> Result<Option<(Outcome, Vec<String>)>> {
    let text =
        fs::read_to_string(log).with_context(|| format!("failed to read {}", log.display()))?;
    let session = parse_claude_code_log(&text)?;
    if since.is_some_and(|s| session.started_at < s) {
        return Ok(None);
    }
    let edited = match harvest::precheck(&session, roots) {
        Ok(edited) => edited,
        Err(outcome) => return Ok(Some((outcome, Vec::new()))),
    };
    let window = TimeWindow {
        from: session.started_at,
        to: session.ended_at + link::window_after_end(),
    };
    let mut candidates = Vec::new();
    for (sha, files) in repo.commits_in(&window)? {
        if harvest::touches_edits(&files, &edited) {
            candidates.push(repo.commit_facts(&sha)?);
        }
    }
    let tasks = match harvest::link_tasks(&session, roots, candidates) {
        Ok(tasks) => tasks,
        Err(outcome) => return Ok(Some((outcome, Vec::new()))),
    };
    let mut ids = Vec::new();
    for linked in tasks {
        if let Some(id) = save_task(store, repo, &session, linked)? {
            ids.push(id);
        }
    }
    let outcome = if ids.is_empty() {
        Outcome::NoTests
    } else {
        Outcome::Task
    };
    Ok(Some((outcome, ids)))
}

/// Writes one linked piece of work's task; `None` when its commit changes no test.
fn save_task(
    store: &Store,
    repo: &Repo,
    session: &nasiko_evals::SessionRecord,
    linked: harvest::Linked,
) -> Result<Option<String>> {
    let (base, gold) = (linked.first().parent.clone(), linked.last().sha.clone());
    let mut split = harvest::split(&repo.diff(&base, &gold)?)?;
    split_inline_tests(repo, &base, &gold, &mut split)?;
    if !harvest::has_tests(&split) {
        return Ok(None);
    }
    let task = harvest::draft_task(&repo.full_name(), &linked, split);
    let id = task.instance_id.clone();
    let commits: Vec<String> = linked.commits.iter().map(|c| c.sha.clone()).collect();
    write_task(store, &task, session, linked)?;
    for retired in reject_commit_history_tasks(store, &commits, &id)? {
        eprintln!(
            "note: {retired} (from commit history) is rejected: {id} is the same commit, from its session"
        );
    }
    Ok(Some(id))
}

/// Rejects the tasks `harvest-commits` made of `commits`, which a session's task now covers,
/// and returns their ids. One commit must not be two tasks of a dataset, and the session's
/// task is the one to keep: it has the request as it was typed and what the work cost.
fn reject_commit_history_tasks(
    store: &Store,
    commits: &[String],
    session_task: &str,
) -> Result<Vec<String>> {
    let mut rejected = Vec::new();
    for mut task in store.tasks()? {
        let extras = &mut task.x_nasiko;
        if extras.source != SessionSource::CommitHistory
            || extras.status == TaskStatus::Rejected
            || !commits.contains(&extras.gold_commit)
        {
            continue;
        }
        extras.status = TaskStatus::Rejected;
        extras.reject_reason = Some(format!(
            "harvest: session task {session_task} is the same commit"
        ));
        store.save_task(&task)?;
        rejected.push(task.instance_id);
    }
    Ok(rejected)
}

/// Moves the inline Rust test changes of `base..gold` from the fix to the tests, reading from
/// git the file versions that takes.
pub(super) fn split_inline_tests(
    repo: &Repo,
    base: &str,
    gold: &str,
    split: &mut SplitPatch,
) -> Result<()> {
    let mut versions = BTreeMap::new();
    for path in harvest::inline_test_paths(split) {
        versions.insert(
            path.clone(),
            FileVersions {
                base: repo.file_at(base, &path)?,
                gold: repo.file_at(gold, &path)?.unwrap_or_default(),
            },
        );
    }
    harvest::split_inline_rust_tests(split, &versions)?;
    Ok(())
}

/// Writes a drafted task with what is kept of its source: `session` is the whole session the
/// task's slice was cut from.
pub(super) fn write_task(
    store: &Store,
    task: &TaskInstance,
    session: &nasiko_evals::SessionRecord,
    linked: harvest::Linked,
) -> Result<()> {
    let layout = store.layout();
    store.save_task(task)?;
    let commits: Vec<&nasiko_evals::CommitFacts> = linked.commits.iter().collect();
    store.save_json(
        &layout.source_file(&task.instance_id),
        &nasiko_evals::TaskSource::new(session, &linked.slice, &commits),
    )?;
    store.save_json(
        &layout.link_file(&task.instance_id),
        &LinkFile {
            decision: linked.decision,
            candidates: linked.candidates,
        },
    )?;
    Ok(())
}

#[derive(Serialize)]
struct LinkFile {
    decision: LinkDecision,
    candidates: Vec<nasiko_evals::LinkScore>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_logs_in_every_project_folder() {
        let dir = tempfile::tempdir().expect("temp dir");
        for (folder, file) in [("-a", "s1.jsonl"), ("-b", "s2.jsonl"), ("-b", "notes.txt")] {
            fs::create_dir_all(dir.path().join(folder)).expect("folder");
            fs::write(dir.path().join(folder).join(file), "").expect("file");
        }
        let logs = find_logs(dir.path()).expect("lists");
        let names: Vec<_> = logs.iter().filter_map(|p| p.file_name()).collect();
        assert_eq!(names, vec!["s1.jsonl", "s2.jsonl"]);
    }

    #[test]
    fn a_sessions_task_retires_the_commit_history_task_of_the_same_commit() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::new(nasiko_evals::EvalLayout::new(dir.path()));
        let from_session: TaskInstance =
            serde_json::from_str(include_str!("testdata/task.json")).expect("task");
        let from_commit = |id: &str, gold_commit: &str| {
            let mut task = from_session.clone();
            task.instance_id = id.into();
            task.x_nasiko.source = SessionSource::CommitHistory;
            task.x_nasiko.gold_commit = gold_commit.into();
            task
        };
        let same_commit = from_commit("repo__1a2b3c4d", &from_session.x_nasiko.gold_commit);
        for task in [
            &from_session,
            &same_commit,
            &from_commit("repo__5e6f7a8b", "another commit"),
        ] {
            store.save_task(task).expect("saves");
        }

        let commits = vec![from_session.x_nasiko.gold_commit.clone()];
        let retire = || reject_commit_history_tasks(&store, &commits, &from_session.instance_id);
        assert_eq!(retire().expect("rejects"), ["repo__1a2b3c4d"]);
        assert!(retire().expect("rejects").is_empty(), "only once");

        let status = |id: &str| {
            let tasks = store.tasks().expect("loads");
            let task = tasks.iter().find(|t| t.instance_id == id).expect("saved");
            (task.x_nasiko.status, task.x_nasiko.reject_reason.clone())
        };
        let (rejected, reason) = status("repo__1a2b3c4d");
        assert_eq!(rejected, TaskStatus::Rejected);
        assert!(reason.is_some_and(|r| r.contains(&from_session.instance_id)));
        assert_ne!(status("repo__5e6f7a8b").0, TaskStatus::Rejected);
        assert_ne!(status(&from_session.instance_id).0, TaskStatus::Rejected);
    }
}
