//! `nasiko eval harvest`: finished Claude Code sessions → candidate tasks.
//!
//! For each session log of the repo: read it, find the commits made during it (or up to two
//! hours after), score each, and when one commit (or one unbroken run of commits) clearly is
//! the session's work, write a candidate task with the gold patch and test patch split apart.
//! The problem statement is the person's first message for now; prompt P1 rewrites it later.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nasiko_evals::link::{self, LinkDecision};
use nasiko_evals::session::claude_code::parse_claude_code_log;
use nasiko_evals::{
    NasikoExtras, OriginalRun, SessionRecord, TaskInstance, TaskStatus, TestPathRules, TestSource,
    diff, instance, rust_tests,
};
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

/// What happened to each session, for the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Task,
    NoTypedMessage,
    NoEditsInRepo,
    NoMatch,
    Ambiguous,
    Unreadable,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Outcome::Task => "task",
            Outcome::NoTypedMessage => "no_typed_message",
            Outcome::NoEditsInRepo => "no_edits_in_repo",
            Outcome::NoMatch => "no_match",
            Outcome::Ambiguous => "ambiguous",
            Outcome::Unreadable => "unreadable",
        }
    }
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
            Ok(Some((outcome, task_id))) => {
                if let Some(id) = task_id {
                    summary.new_tasks.push(id);
                }
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
) -> Result<Option<(Outcome, Option<String>)>> {
    let text =
        fs::read_to_string(log).with_context(|| format!("failed to read {}", log.display()))?;
    let session = parse_claude_code_log(&text)?;
    if since.is_some_and(|s| session.started_at < s) {
        return Ok(None);
    }
    if session.user_messages.is_empty() {
        return Ok(Some((Outcome::NoTypedMessage, None)));
    }
    if session.edited_paths_in(roots).is_empty() {
        return Ok(Some((Outcome::NoEditsInRepo, None)));
    }
    let window = TimeWindow {
        from: session.started_at,
        to: session.ended_at + link::window_after_end(),
    };
    let edited = session.edited_paths_in(roots);
    let mut commits = Vec::new();
    for (sha, files) in repo.commits_in(&window)? {
        if !files.iter().any(|f| edited.contains(f)) {
            continue;
        }
        let facts = repo.commit_facts(&sha)?;
        if link::in_window(&session, &facts) && link::is_eligible(&facts) {
            commits.push(facts);
        }
    }
    let scores: Vec<_> = commits
        .iter()
        .map(|c| link::score(&session, roots, c))
        .collect();
    let decision = link::decide(&scores);
    let (first, last) = match &decision {
        LinkDecision::Single { link } => (link.clone(), link.clone()),
        LinkDecision::Range { first, last } => (first.clone(), last.clone()),
        LinkDecision::Ambiguous { .. } => return Ok(Some((Outcome::Ambiguous, None))),
        LinkDecision::NoMatch => return Ok(Some((Outcome::NoMatch, None))),
    };
    let first_commit = commits
        .iter()
        .find(|c| c.sha == first.sha)
        .context("linked commit vanished")?;
    let last_commit = commits
        .iter()
        .find(|c| c.sha == last.sha)
        .context("linked commit vanished")?;
    // A long session often holds several tasks: keep only the part from the message that led
    // to the first edit of the commit's files up to the commit itself.
    let linked_files: Vec<String> = commits
        .iter()
        .filter(|c| {
            c.committed_at >= first_commit.committed_at
                && c.committed_at <= last_commit.committed_at
        })
        .flat_map(|c| c.files.clone())
        .collect();
    let start = session.task_start(roots, &linked_files);
    let slice = session.slice(start, last_commit.committed_at);
    let task = draft_task(
        repo,
        &slice,
        first_commit,
        last_commit,
        first.total.min(last.total),
    )?;
    let id = task.instance_id.clone();
    let layout = store.layout();
    store.save_task(&task)?;
    let linked: Vec<&nasiko_evals::CommitFacts> = commits
        .iter()
        .filter(|c| {
            c.committed_at >= first_commit.committed_at
                && c.committed_at <= last_commit.committed_at
        })
        .collect();
    store.save_json(
        &layout.source_file(&id),
        &nasiko_evals::TaskSource::new(&session, &slice, &linked),
    )?;
    store.save_json(
        &layout.link_file(&id),
        &LinkFile {
            decision,
            candidates: scores,
        },
    )?;
    Ok(Some((Outcome::Task, Some(id))))
}

fn draft_task(
    repo: &Repo,
    session: &SessionRecord,
    first: &nasiko_evals::CommitFacts,
    last: &nasiko_evals::CommitFacts,
    link_score: f64,
) -> Result<TaskInstance> {
    let base = &first.parent;
    let full = repo.diff(base, &last.sha)?;
    let mut split = diff::split_tests(diff::parse_unified(&full)?, &TestPathRules::rust_default());
    split_inline_rust_tests(repo, base, &last.sha, &mut split)?;
    let usage = session.total_usage();
    let (statement, hints) = statement_and_hints(session);
    Ok(TaskInstance {
        instance_id: instance::instance_id(&repo.slug(), &session.session_id),
        repo: repo.full_name(),
        base_commit: base.clone(),
        environment_setup_commit: base.clone(),
        version: last.committed_at.format("%Y.%m").to_string(),
        created_at: last.committed_at,
        problem_statement: statement,
        hints_text: hints,
        patch: diff::join(&split.gold),
        test_patch: diff::join(&split.test),
        fail_to_pass: Vec::new(),
        pass_to_pass: Vec::new(),
        image: String::new(),
        eval_script: String::new(),
        log_parser: String::new(),
        eval_type: String::new(),
        difficulty: None,
        x_nasiko: NasikoExtras {
            status: TaskStatus::Candidate,
            source: session.source,
            source_session_id: session.session_id.clone(),
            gold_commit: last.sha.clone(),
            link_score,
            test_source: if split.test.is_empty() {
                TestSource::None
            } else {
                TestSource::Commit
            },
            orig: OriginalRun {
                model: session.main_model().unwrap_or_default(),
                usage,
                user_turns: session.user_messages.len() as u32,
            },
            mixed_files: split.mixed,
            reject_reason: None,
            statement_rewritten: false,
            test_scope: Vec::new(),
        },
    })
}

/// Moves each changed Rust file's inline `#[cfg(test)]` module change into the test patch.
/// Files that cannot be split (several test modules) stay in `mixed`.
fn split_inline_rust_tests(
    repo: &Repo,
    base: &str,
    gold: &str,
    split: &mut diff::SplitPatch,
) -> Result<()> {
    let mut gold_files = Vec::new();
    let mut unsplit = Vec::new();
    for file in std::mem::take(&mut split.gold) {
        let path = file.path().to_string();
        if !path.ends_with(".rs") || file.new_path.is_none() {
            gold_files.push(file);
            continue;
        }
        // A file the fix creates keeps its tests: hidden tests in it could only ever apply to a
        // fix that creates the very same file, which would punish a correct fix laid out
        // differently. Hidden tests come from files that already existed, as in SWE-bench.
        let Some(old) = repo.file_at(base, &path)? else {
            gold_files.push(file);
            continue;
        };
        let old = Some(old);
        let new = repo.file_at(gold, &path)?.unwrap_or_default();
        let Some(middle) = rust_tests::code_with_old_tests(old.as_deref().unwrap_or(""), &new)
        else {
            if split.mixed.contains(&path) {
                unsplit.push(path);
            }
            gold_files.push(file);
            continue;
        };
        if middle == new {
            gold_files.push(file);
            continue;
        }
        let code_part = repo.diff_texts(&path, old.as_deref(), &middle)?;
        let test_part = repo.diff_texts(&path, Some(&middle), &new)?;
        gold_files.extend(diff::parse_unified(&code_part)?);
        split.test.extend(diff::parse_unified(&test_part)?);
    }
    split.gold = gold_files;
    split.mixed = unsplit;
    Ok(())
}

/// Until prompt P1 rewrites it: the first typed message is the statement, later ones are hints.
fn statement_and_hints(session: &SessionRecord) -> (String, String) {
    let mut messages = session.user_messages.iter().map(|m| m.text.as_str());
    let statement = messages.next().unwrap_or_default().to_string();
    let hints = messages.collect::<Vec<_>>().join("\n\n---\n\n");
    (statement, hints)
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
}
