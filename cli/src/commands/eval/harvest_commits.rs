//! `nasiko eval local harvest-commits`: a repository's own commit history → candidate tasks.
//!
//! For a repository with no Claude Code sessions: every single-parent commit reachable from
//! `HEAD` that changes code and its tests, and whose message says enough to state a problem,
//! becomes a candidate task for the same later steps a session's task goes through. The shared
//! harvest steps (`nasiko_evals::harvest`) classify and draft; this module lists the commits,
//! asks local git for what those steps need, and counts what each commit turned out to be.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Utc};
use nasiko_evals::diff::DiffError;
use nasiko_evals::harvest::{self, CommitOutcome, HistoryCommit, Linked};
use nasiko_evals::{TaskInstance, TaskStatus};

use super::git::Repo;
use super::harvest::{Redo, split_inline_tests, write_task};
use super::store::{HarvestLedger, Store};

/// Inputs of one commit-history harvest.
pub struct HarvestCommitsArgs {
    pub repo: PathBuf,
    /// Only commits made at or after this time.
    pub since: Option<DateTime<Utc>>,
    /// Look no further back than the commit that makes this many usable tasks, newest first.
    /// Tasks of an earlier harvest count, so the same command run twice adds nothing; rejected
    /// ones do not, so the same command run after `rewrite` rejected some replaces them.
    pub max_tasks: Option<usize>,
    /// Look at commits again even if an earlier harvest already did.
    pub redo: Redo,
    pub writes: Writes,
}

/// Whether a harvest saves what it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writes {
    Save,
    /// Count and list only: no task and no ledger entry is written.
    DryRun,
}

/// A commit that became a candidate task (in a dry run: would become one).
pub struct CommitTask {
    pub instance_id: String,
    pub committed_at: DateTime<Utc>,
    pub subject: String,
}

/// Result of one commit-history harvest: what every commit turned out to be.
pub struct CommitHarvestSummary {
    pub repo: String,
    /// The `--since` the listing was cut at.
    pub since: Option<DateTime<Utc>>,
    /// The clone is shallow: its history ends where it was cut, not where the project began.
    pub shallow: bool,
    /// Commits reachable from `HEAD`, within `--since`.
    pub commits_listed: usize,
    /// Left alone: an earlier harvest already looked at them.
    pub skipped_seen: usize,
    /// Left alone: a task for the commit is already on disk.
    pub already_tasks: usize,
    /// The commits looked at this time, by what they turned out to be.
    pub outcomes: BTreeMap<CommitOutcome, usize>,
    /// How many of the thin-message commits name an issue or pull request.
    pub thin_naming_an_issue: usize,
    /// How many of the new tasks have a message long enough to likely survive the rewrite.
    pub detailed_messages: usize,
    pub new_tasks: Vec<CommitTask>,
    /// The `--max` that ended the walk before the oldest listed commit, when it did.
    pub stopped_at_max: Option<usize>,
    pub writes: Writes,
}

impl CommitHarvestSummary {
    fn new(repo: &str, writes: Writes) -> Self {
        Self {
            repo: repo.to_string(),
            since: None,
            shallow: false,
            commits_listed: 0,
            skipped_seen: 0,
            already_tasks: 0,
            outcomes: BTreeMap::new(),
            thin_naming_an_issue: 0,
            detailed_messages: 0,
            new_tasks: Vec::new(),
            stopped_at_max: None,
            writes,
        }
    }

    /// Counts one commit looked at this time, and what its message says about its chances.
    fn record(&mut self, commit: &HistoryCommit, outcome: CommitOutcome) {
        *self.outcomes.entry(outcome).or_default() += 1;
        match outcome {
            CommitOutcome::Task => {
                self.detailed_messages +=
                    usize::from(harvest::is_detailed_message(&commit.message));
            }
            CommitOutcome::ThinMessage => {
                self.thin_naming_an_issue +=
                    usize::from(harvest::references_issue(&commit.message));
            }
            _ => {}
        }
    }

    fn count(&self, outcome: CommitOutcome) -> usize {
        self.outcomes.get(&outcome).copied().unwrap_or_default()
    }

    /// Commits looked at this time: the top of the funnel.
    pub fn commits_scanned(&self) -> usize {
        self.outcomes.values().sum()
    }

    /// `demo: 120 commits reachable from HEAD since 2025-01-01`.
    pub fn headline(&self) -> String {
        let since = self
            .since
            .map(|since| format!(" since {}", since.format("%Y-%m-%d")))
            .unwrap_or_default();
        format!(
            "{}: {} reachable from HEAD{since}",
            self.repo,
            counted(self.commits_listed, "commit")
        )
    }

    /// The funnel as lines: every outcome with its count, in the order a commit is judged.
    pub fn funnel(&self) -> String {
        let became = match self.writes {
            Writes::Save => "became a candidate task",
            Writes::DryRun => "would become a candidate task",
        };
        let thin = self.note(
            CommitOutcome::ThinMessage,
            self.thin_naming_an_issue,
            "name an issue or pull request",
        );
        let detailed = self.note(
            CommitOutcome::Task,
            self.detailed_messages,
            "with a detailed message",
        );
        let mut lines = String::new();
        if self.skipped_seen > 0 {
            lines += &funnel_row("already harvested", self.skipped_seen, "");
        }
        if self.already_tasks > 0 {
            lines += &funnel_row("already a task", self.already_tasks, "");
        }
        lines += &funnel_row("commits scanned", self.commits_scanned(), "");
        for (outcome, label, note) in [
            (CommitOutcome::MergeOrRoot, "merge or root commit", ""),
            (CommitOutcome::TooLarge, "too many files", ""),
            (CommitOutcome::NoCodeChange, "no Rust code change", ""),
            (CommitOutcome::NoTests, "no tests", ""),
            (
                CommitOutcome::ThinMessage,
                "too thin a message",
                thin.as_str(),
            ),
            (CommitOutcome::IdTaken, "task id taken", ""),
            (CommitOutcome::Unreadable, "unreadable", ""),
            (CommitOutcome::Task, became, detailed.as_str()),
        ] {
            // A clash of ids is too rare to show a row of zeroes for.
            if outcome == CommitOutcome::IdTaken && self.count(outcome) == 0 {
                continue;
            }
            lines += &funnel_row(&format!("  {label}"), self.count(outcome), note);
        }
        lines
    }

    /// `  (3 name an issue)` for a row that has commits; nothing for an empty one.
    fn note(&self, row: CommitOutcome, count: usize, what: &str) -> String {
        if self.count(row) == 0 {
            String::new()
        } else {
            format!("  ({count} {what})")
        }
    }
}

fn funnel_row(label: &str, count: usize, note: &str) -> String {
    format!("  {label:<32} {count:>6}{note}\n")
}

/// `1 commit`, `2 commits`.
pub fn counted(count: usize, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{plural}")
}

/// Reads the repo's history and writes a candidate task for every commit that can be one.
pub fn harvest_commits(store: &Store, args: &HarvestCommitsArgs) -> Result<CommitHarvestSummary> {
    let repo = Repo::open(&args.repo)?;
    let slug = repo.slug();
    let history = repo.history(args.since)?;
    let mut walk = Walk {
        redo: args.redo,
        ledger: store.load_ledger(&slug)?,
        summary: CommitHarvestSummary {
            since: args.since,
            shallow: repo.is_shallow()?,
            commits_listed: history.len(),
            ..CommitHarvestSummary::new(&slug, args.writes)
        },
        usable_tasks: 0,
        harvester: CommitHarvester {
            store,
            repo_full_name: repo.full_name(),
            repo,
            writes: args.writes,
            on_disk: TasksOnDisk::load(store)?,
            new_tasks: Vec::new(),
        },
    };
    let _status = nasiko_utils::term::start_status(format!("reading {} commits", history.len()));
    for commit in &history {
        if args.max_tasks.is_some_and(|max| walk.usable_tasks >= max) {
            walk.summary.stopped_at_max = args.max_tasks;
            break;
        }
        walk.visit(commit)?;
    }
    if args.writes == Writes::Save {
        store.save_ledger(&slug, &walk.ledger)?;
    }
    Ok(CommitHarvestSummary {
        new_tasks: walk.harvester.new_tasks,
        ..walk.summary
    })
}

/// The reproducible tasks that `screen` and `review` leave out because their statement is
/// still a raw commit message: a reviewer's verdict must be on what a model will be shown.
pub fn awaiting_rewrite(store: &Store) -> Result<Vec<String>> {
    Ok(store
        .tasks()?
        .into_iter()
        .filter(|t| t.x_nasiko.status == TaskStatus::Reproducible && t.x_nasiko.needs_rewrite())
        .map(|t| t.instance_id)
        .collect())
}

/// A commit's key in the repo's harvest ledger, which it shares with sessions (keyed by their
/// id).
fn ledger_key(sha: &str) -> String {
    format!("commit:{sha}")
}

/// One pass over the history, newest commit first: what is known of each commit from earlier
/// harvests, and what this one adds.
struct Walk<'a> {
    redo: Redo,
    /// Every commit an earlier harvest looked at, with what it turned out to be.
    ledger: HarvestLedger,
    summary: CommitHarvestSummary,
    /// Commits passed so far that are a task `--max` counts: saved by this harvest or an
    /// earlier one, and not rejected since.
    usable_tasks: usize,
    harvester: CommitHarvester<'a>,
}

impl Walk<'_> {
    /// Gives one commit its outcome: left alone when an earlier harvest saw it or a task for it
    /// exists, harvested otherwise.
    fn visit(&mut self, commit: &HistoryCommit) -> Result<()> {
        let key = ledger_key(&commit.sha);
        let on_disk = &self.harvester.on_disk;
        if self.redo == Redo::Skip && self.ledger.contains_key(&key) {
            self.summary.skipped_seen += 1;
            self.usable_tasks += usize::from(on_disk.has_usable_task(&commit.sha));
            return Ok(());
        }
        let outcome = if on_disk.has_task(&commit.sha) {
            self.summary.already_tasks += 1;
            self.usable_tasks += usize::from(on_disk.has_usable_task(&commit.sha));
            CommitOutcome::Task
        } else {
            let outcome = self.harvester.harvest(commit)?;
            self.summary.record(commit, outcome);
            self.usable_tasks += usize::from(outcome == CommitOutcome::Task);
            outcome
        };
        self.ledger.insert(key, outcome.label().to_string());
        Ok(())
    }
}

/// What is already under `tasks/`, so that no commit becomes a second task.
struct TasksOnDisk {
    ids: BTreeSet<String>,
    /// The commit each task's fix ends in. A session's task has one too: a commit that a
    /// harvested session produced must not be harvested again from the history.
    gold_commits: BTreeSet<String>,
    /// Those of them with a task that was not rejected.
    usable_gold_commits: BTreeSet<String>,
}

impl TasksOnDisk {
    fn load(store: &Store) -> Result<Self> {
        let tasks = store.tasks()?;
        let gold_commits = |usable_only: bool| {
            tasks
                .iter()
                .filter(|t| !usable_only || t.x_nasiko.status != TaskStatus::Rejected)
                .map(|t| t.x_nasiko.gold_commit.clone())
                .collect()
        };
        Ok(Self {
            ids: tasks.iter().map(|t| t.instance_id.clone()).collect(),
            gold_commits: gold_commits(false),
            usable_gold_commits: gold_commits(true),
        })
    }

    fn has_task(&self, sha: &str) -> bool {
        self.gold_commits.contains(sha)
    }

    /// A rejected task still stops its commit from being harvested again, but it is not one
    /// of the tasks `--max` asks for.
    fn has_usable_task(&self, sha: &str) -> bool {
        self.usable_gold_commits.contains(sha)
    }
}

/// Turns commits into saved tasks, one at a time.
struct CommitHarvester<'a> {
    store: &'a Store,
    repo: Repo,
    repo_full_name: String,
    writes: Writes,
    on_disk: TasksOnDisk,
    /// The tasks made so far, newest commit first.
    new_tasks: Vec<CommitTask>,
}

impl CommitHarvester<'_> {
    /// What the commit turned out to be. A task is saved (unless this is a dry run) and added
    /// to `new_tasks`. A commit git cannot read is an outcome with a warning; only a failed
    /// save is an error.
    fn harvest(&mut self, commit: &HistoryCommit) -> Result<CommitOutcome> {
        let drafted = match draft(&self.repo, &self.repo_full_name, commit) {
            Ok(drafted) => drafted,
            Err(NotDrafted::Verdict(outcome)) => return Ok(outcome),
            Err(NotDrafted::Failed(e)) => {
                eprintln!("warning: commit {}: {e:#}", commit.sha);
                return Ok(CommitOutcome::Unreadable);
            }
        };
        let instance_id = drafted.task.instance_id.clone();
        // Ids carry eight sha characters, so two commits can share one; the second is skipped
        // rather than written over the first.
        if !self.on_disk.ids.insert(instance_id.clone()) {
            eprintln!(
                "warning: commit {}: its task id {instance_id} is taken by another commit",
                commit.sha
            );
            return Ok(CommitOutcome::IdTaken);
        }
        if self.writes == Writes::Save {
            let stand_in_session = drafted.linked.slice.clone();
            write_task(self.store, &drafted.task, &stand_in_session, drafted.linked)?;
        }
        self.new_tasks.push(CommitTask {
            instance_id,
            committed_at: commit.committed_at,
            subject: commit.subject().to_string(),
        });
        Ok(CommitOutcome::Task)
    }
}

/// A commit's task and the stand-in link it was drafted from.
struct Drafted {
    task: TaskInstance,
    linked: Linked,
}

/// Why a commit has no drafted task.
enum NotDrafted {
    /// The commit cannot be a task: this is its outcome.
    Verdict(CommitOutcome),
    /// git or the diff failed, which says nothing about the commit.
    Failed(anyhow::Error),
}

impl From<CommitOutcome> for NotDrafted {
    fn from(outcome: CommitOutcome) -> Self {
        Self::Verdict(outcome)
    }
}

impl From<anyhow::Error> for NotDrafted {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(error)
    }
}

impl From<DiffError> for NotDrafted {
    fn from(error: DiffError) -> Self {
        Self::Failed(error.into())
    }
}

/// The commit's task, or why there is none.
fn draft(
    repo: &Repo,
    repo_full_name: &str,
    commit: &HistoryCommit,
) -> std::result::Result<Drafted, NotDrafted> {
    let parent = harvest::change_base(commit)?;
    let mut split = harvest::split(&repo.diff(parent, &commit.sha)?)?;
    harvest::commit_precheck(&split)?;
    split_inline_tests(repo, parent, &commit.sha, &mut split)?;
    let outcome = harvest::classify_commit(commit, &split);
    if outcome != CommitOutcome::Task {
        return Err(outcome.into());
    }
    let linked = harvest::linked_from_commit(commit, &split)?;
    let task = harvest::draft_task(repo_full_name, &linked, split);
    Ok(Drafted { task, linked })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use nasiko_evals::{EvalLayout, SessionSource, TaskSource};

    use super::super::screen;
    use super::*;

    const LIB: &str = "pub fn b() -> u8 {\n    1\n}\n";
    const CLEAR: &str =
        "Fix b returning the wrong value\n\nCallers of b() got 1 where the documented result is 2.";

    fn git(dir: &Path, args: &[&str]) -> String {
        git_at(dir, args, None)
    }

    /// Runs git, as if at `date` (author and committer) when one is given.
    fn git_at(dir: &Path, args: &[&str], date: Option<&str>) -> String {
        let mut command = Command::new("git");
        command
            .current_dir(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args);
        if let Some(date) = date {
            command
                .env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date);
        }
        let out = command.output().expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Writes `files`, commits them all with `message`, and returns the new sha.
    fn commit(dir: &Path, files: &[(&str, &str)], message: &str) -> String {
        commit_at(dir, files, message, None)
    }

    fn commit_at(dir: &Path, files: &[(&str, &str)], message: &str, date: Option<&str>) -> String {
        for (path, text) in files {
            let path = dir.join(path);
            fs::create_dir_all(path.parent().expect("a file has a folder")).expect("folder");
            fs::write(path, text).expect("writes");
        }
        git(dir, &["add", "-A"]);
        git_at(dir, &["commit", "-q", "-m", message], date);
        git(dir, &["rev-parse", "HEAD"])
    }

    /// A new repository on `main` with the library as its root commit.
    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        commit(dir, &[("src/lib.rs", LIB)], "Add the library with b");
    }

    /// Commits a fix of `b` to return `value` together with a test of it.
    fn commit_fix_with_tests(dir: &Path, value: u8, date: Option<&str>) -> String {
        let lib = format!("pub fn b() -> u8 {{\n    {value}\n}}\n");
        let test = format!("#[test]\nfn b_is_{value}() {{}}\n");
        commit_at(
            dir,
            &[("src/lib.rs", &lib), (&format!("tests/b{value}.rs"), &test)],
            &format!("Fix b returning the wrong value\n\nCallers of b() expect {value}."),
            date,
        )
    }

    fn args_for(repo: &Path, writes: Writes) -> HarvestCommitsArgs {
        HarvestCommitsArgs {
            repo: repo.to_path_buf(),
            since: None,
            max_tasks: None,
            redo: Redo::Skip,
            writes,
        }
    }

    /// The commits a test asserts on.
    struct History {
        fix_with_tests: String,
        tests_only: String,
        code_only: String,
        thin: String,
        merge: String,
    }

    /// A repository whose history has a commit of every kind, oldest first: the root, a fix
    /// with its tests, tests only, code only, a fix with tests under a thin message, and the
    /// merge of a side branch whose one commit is also a fix with tests.
    fn repo_with_history(dir: &Path) -> History {
        init_repo(dir);
        let fix_with_tests = commit(
            dir,
            &[
                ("src/lib.rs", "pub fn b() -> u8 {\n    2\n}\n"),
                ("tests/b.rs", "#[test]\nfn b_is_two() {}\n"),
            ],
            CLEAR,
        );
        let tests_only = commit(
            dir,
            &[("tests/more.rs", "#[test]\nfn still_two() {}\n")],
            "Cover b with one more test of its value",
        );
        let code_only = commit(
            dir,
            &[("src/c.rs", "pub fn c() -> u8 {\n    3\n}\n")],
            "Add c, which callers asked for last week",
        );
        let thin = commit(
            dir,
            &[
                ("src/c.rs", "pub fn c() -> u8 {\n    4\n}\n"),
                ("tests/c.rs", "#[test]\nfn c_is_four() {}\n"),
            ],
            "Fix #7",
        );
        git(dir, &["checkout", "-q", "-b", "side"]);
        commit(
            dir,
            &[
                ("src/d.rs", "pub fn d() -> u8 {\n    5\n}\n"),
                ("tests/d.rs", "#[test]\nfn d_is_five() {}\n"),
            ],
            "Add d so that callers can read the fifth value",
        );
        git(dir, &["checkout", "-q", "main"]);
        git(dir, &["merge", "-q", "--no-ff", "-m", "Merge side", "side"]);
        History {
            fix_with_tests,
            tests_only,
            code_only,
            thin,
            merge: git(dir, &["rev-parse", "HEAD"]),
        }
    }

    /// A repository with that history, an empty eval home, and the temp folders holding them.
    struct Fixture {
        repo_dir: tempfile::TempDir,
        home: tempfile::TempDir,
        history: History,
        store: Store,
    }

    fn fixture() -> Fixture {
        let repo_dir = tempfile::tempdir().expect("repo folder");
        let home = tempfile::tempdir().expect("eval home");
        Fixture {
            history: repo_with_history(repo_dir.path()),
            store: Store::new(EvalLayout::new(home.path())),
            repo_dir,
            home,
        }
    }

    impl Fixture {
        fn args(&self, writes: Writes) -> HarvestCommitsArgs {
            args_for(self.repo_dir.path(), writes)
        }
    }

    fn task_id(slug: &str, sha: &str) -> String {
        format!("{slug}__{}", &sha[..8])
    }

    #[test]
    fn every_commit_gets_one_outcome_and_fixes_with_tests_become_tasks() {
        let f = fixture();
        let summary = harvest_commits(&f.store, &f.args(Writes::Save)).expect("harvests");

        assert_eq!(summary.commits_listed, 7);
        assert_eq!(summary.commits_scanned(), 7, "each commit has one outcome");
        let counts: Vec<(CommitOutcome, usize)> =
            summary.outcomes.iter().map(|(o, n)| (*o, *n)).collect();
        assert_eq!(
            counts,
            [
                (CommitOutcome::Task, 2),
                (CommitOutcome::MergeOrRoot, 2),
                (CommitOutcome::NoCodeChange, 1),
                (CommitOutcome::NoTests, 1),
                (CommitOutcome::ThinMessage, 1),
            ]
        );
        assert_eq!(summary.thin_naming_an_issue, 1);

        let slug = &summary.repo;
        let tasks = f.store.tasks().expect("loads");
        assert_eq!(tasks.len(), 2, "the fix on main and the side branch's");
        let task = tasks
            .iter()
            .find(|t| t.instance_id == task_id(slug, &f.history.fix_with_tests))
            .expect("the fix with tests is a task named by its sha");
        assert_eq!(task.x_nasiko.source, SessionSource::CommitHistory);
        assert_eq!(task.x_nasiko.status, TaskStatus::Candidate);
        assert_eq!(task.x_nasiko.gold_commit, f.history.fix_with_tests);
        assert_eq!(task.problem_statement, CLEAR);
        assert!(task.x_nasiko.needs_rewrite());
        assert!(task.patch.contains("src/lib.rs") && !task.patch.contains("tests/b.rs"));
        assert!(task.test_patch.contains("b_is_two"));

        let layout = f.store.layout();
        let source: TaskSource = serde_json::from_str(
            &fs::read_to_string(layout.source_file(&task.instance_id)).expect("source.json"),
        )
        .expect("parses");
        assert_eq!(source.source, SessionSource::CommitHistory);
        assert_eq!(source.context_messages[0].text, CLEAR);
        assert!(layout.link_file(&task.instance_id).exists());

        let ledger = f.store.load_ledger(slug).expect("ledger");
        let recorded = |sha: &str| ledger.get(&ledger_key(sha)).map(String::as_str);
        assert_eq!(recorded(&f.history.fix_with_tests), Some("task"));
        assert_eq!(recorded(&f.history.tests_only), Some("no_code_change"));
        assert_eq!(recorded(&f.history.code_only), Some("no_tests"));
        assert_eq!(recorded(&f.history.thin), Some("thin_message"));
        assert_eq!(recorded(&f.history.merge), Some("merge_or_root"));
    }

    #[test]
    fn a_dry_run_lists_the_tasks_and_writes_nothing() {
        let f = fixture();
        let summary = harvest_commits(&f.store, &f.args(Writes::DryRun)).expect("harvests");

        // Newest first: the side branch's commit, then the fix on main.
        let [_, older] = summary.new_tasks.as_slice() else {
            panic!("two tasks, got {}", summary.new_tasks.len());
        };
        assert_eq!(
            older.instance_id,
            task_id(&summary.repo, &f.history.fix_with_tests)
        );
        assert_eq!(older.subject, "Fix b returning the wrong value");
        let funnel = summary.funnel();
        assert!(funnel.contains("would become a candidate task"), "{funnel}");
        let written: Vec<_> = fs::read_dir(f.home.path()).expect("lists").collect();
        assert!(written.is_empty(), "a dry run wrote {written:?}");
    }

    #[test]
    fn a_second_run_adds_nothing_and_redo_keeps_a_task_that_has_moved_on() {
        let f = fixture();
        let mut run = f.args(Writes::Save);
        let first = harvest_commits(&f.store, &run).expect("first");
        assert_eq!(first.new_tasks.len(), 2);

        let second = harvest_commits(&f.store, &run).expect("second");
        assert_eq!((second.skipped_seen, second.commits_scanned()), (7, 0));
        assert!(second.new_tasks.is_empty());
        assert_eq!(f.store.tasks().expect("loads").len(), 2);

        let mut built = f.store.tasks().expect("loads").remove(0);
        built.x_nasiko.status = TaskStatus::Reproducible;
        f.store.save_task(&built).expect("saves");
        run.redo = Redo::All;
        let redone = harvest_commits(&f.store, &run).expect("redo");
        assert_eq!((redone.already_tasks, redone.new_tasks.len()), (2, 0));
        assert_eq!(redone.commits_scanned(), 5);
        let kept = f.store.tasks().expect("loads").remove(0);
        assert_eq!(kept.x_nasiko.status, TaskStatus::Reproducible);
    }

    #[test]
    fn max_stops_at_the_newest_tasks_and_counts_those_already_harvested() {
        let f = fixture();
        let mut run = f.args(Writes::Save);
        run.max_tasks = Some(1);

        let first = harvest_commits(&f.store, &run).expect("first");
        assert_eq!(first.new_tasks.len(), 1);
        assert_eq!(first.stopped_at_max, Some(1));
        assert!(
            first.commits_scanned() < first.commits_listed,
            "older commits were not looked at"
        );
        let older = task_id(&first.repo, &f.history.fix_with_tests);
        assert_ne!(first.new_tasks[0].instance_id, older, "newest first");

        let again = harvest_commits(&f.store, &run).expect("again");
        assert!(again.new_tasks.is_empty(), "the same command adds nothing");
        assert_eq!(f.store.tasks().expect("loads").len(), 1);
    }

    #[test]
    fn a_rejected_task_does_not_count_toward_max_so_the_same_command_replaces_it() {
        let f = fixture();
        let mut run = f.args(Writes::Save);
        run.max_tasks = Some(1);
        let first = harvest_commits(&f.store, &run).expect("first");
        let mut newest = f.store.tasks().expect("loads").remove(0);
        assert_eq!(newest.instance_id, first.new_tasks[0].instance_id);
        newest.x_nasiko.status = TaskStatus::Rejected;
        f.store.save_task(&newest).expect("saves");

        let again = harvest_commits(&f.store, &run).expect("again");
        let [replacement] = again.new_tasks.as_slice() else {
            panic!("one replacement, got {}", again.new_tasks.len());
        };
        assert_eq!(
            replacement.instance_id,
            task_id(&again.repo, &f.history.fix_with_tests),
            "the next older commit that can be a task"
        );
        assert_eq!(f.store.tasks().expect("loads").len(), 2);

        run.redo = Redo::All;
        let redone = harvest_commits(&f.store, &run).expect("redo");
        assert_eq!((redone.already_tasks, redone.new_tasks.len()), (2, 0));
        let rejected = f.store.tasks().expect("loads");
        let rejected = rejected
            .iter()
            .find(|t| t.instance_id == newest.instance_id)
            .expect("kept");
        assert_eq!(
            rejected.x_nasiko.status,
            TaskStatus::Rejected,
            "a rejected task is not made again"
        );
    }

    #[test]
    fn since_keeps_every_commit_from_that_time_on_even_behind_an_older_one() {
        let repo_dir = tempfile::tempdir().expect("repo folder");
        let home = tempfile::tempdir().expect("eval home");
        let dir = repo_dir.path();
        git(dir, &["init", "-q"]);
        let long_ago = Some("2024-01-01T00:00:00Z");
        commit_at(dir, &[("src/lib.rs", LIB)], "Add the library", long_ago);
        // Oldest first. The fourth is dated before the third: `git log --since` would stop
        // there and never reach the third.
        let dates = [
            "2024-06-01T12:00:00Z",
            "2024-12-31T23:59:59Z",
            "2025-01-01T00:00:00Z",
            "2024-12-31T20:00:00Z",
            "2025-06-01T12:00:00Z",
        ];
        let shas: Vec<String> = (2u8..)
            .zip(dates)
            .map(|(value, date)| commit_fix_with_tests(dir, value, Some(date)))
            .collect();

        let mut run = args_for(dir, Writes::DryRun);
        run.since = Some("2025-01-01T00:00:00Z".parse().expect("timestamp"));
        let summary =
            harvest_commits(&Store::new(EvalLayout::new(home.path())), &run).expect("harvests");
        assert_eq!(summary.commits_listed, 2, "the boundary is inclusive");
        let ids: Vec<&str> = summary
            .new_tasks
            .iter()
            .map(|t| t.instance_id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                task_id(&summary.repo, &shas[4]),
                task_id(&summary.repo, &shas[2])
            ]
        );
        assert!(
            summary
                .headline()
                .ends_with("2 commits reachable from HEAD since 2025-01-01"),
            "{}",
            summary.headline()
        );
    }

    #[test]
    fn a_fix_that_is_not_code_and_a_revert_that_only_removes_tests_are_not_tasks() {
        let repo_dir = tempfile::tempdir().expect("repo folder");
        let home = tempfile::tempdir().expect("eval home");
        let dir = repo_dir.path();
        init_repo(dir);
        let test = "#[test]\nfn documented() {}\n";
        commit(
            dir,
            &[("README.md", "b returns 2\n"), ("tests/doc.rs", test)],
            "Document what b returns and check the documented value in a test",
        );
        fs::write(dir.join("logo.png"), [0u8, 159, 146, 150, 0, 1, 2]).expect("writes");
        commit(
            dir,
            &[("tests/logo.rs", "#[test]\nfn has_a_logo() {}\n")],
            "Add the logo that the packaging step reported as missing",
        );
        let fix = commit_fix_with_tests(dir, 2, None);
        git(dir, &["revert", "--no-edit", &fix]);

        let store = Store::new(EvalLayout::new(home.path()));
        let summary = harvest_commits(&store, &args_for(dir, Writes::DryRun)).expect("harvests");
        let counts: Vec<(CommitOutcome, usize)> =
            summary.outcomes.iter().map(|(o, n)| (*o, *n)).collect();
        assert_eq!(
            counts,
            [
                (CommitOutcome::Task, 1),
                (CommitOutcome::MergeOrRoot, 1),
                (CommitOutcome::NoCodeChange, 2),
                (CommitOutcome::NoTests, 1),
            ]
        );
        assert_eq!(
            summary.new_tasks[0].instance_id,
            task_id(&summary.repo, &fix)
        );
    }

    #[test]
    fn a_commit_whose_task_id_is_taken_is_counted_and_overwrites_nothing() {
        let f = fixture();
        let slug = harvest_commits(&f.store, &f.args(Writes::DryRun))
            .expect("harvests")
            .repo;
        let mut other: TaskInstance =
            serde_json::from_str(include_str!("testdata/task.json")).expect("task");
        other.instance_id = task_id(&slug, &f.history.fix_with_tests);
        f.store.save_task(&other).expect("saves");

        let summary = harvest_commits(&f.store, &f.args(Writes::Save)).expect("harvests");
        assert_eq!(summary.count(CommitOutcome::IdTaken), 1);
        assert_eq!(summary.new_tasks.len(), 1, "the side branch's fix");
        assert!(summary.funnel().contains("task id taken"));
        let kept = f.store.tasks().expect("loads");
        let kept = kept
            .iter()
            .find(|t| t.instance_id == other.instance_id)
            .expect("still there");
        assert_eq!(kept.x_nasiko.gold_commit, other.x_nasiko.gold_commit);
    }

    #[test]
    fn a_commit_a_sessions_task_already_ends_in_is_not_harvested_again() {
        let f = fixture();
        let mut from_session: TaskInstance =
            serde_json::from_str(include_str!("testdata/task.json")).expect("task");
        from_session.x_nasiko.gold_commit = f.history.fix_with_tests.clone();
        f.store.save_task(&from_session).expect("saves");

        let summary = harvest_commits(&f.store, &f.args(Writes::Save)).expect("harvests");
        assert_eq!((summary.already_tasks, summary.new_tasks.len()), (1, 1));
        assert_eq!(f.store.tasks().expect("loads").len(), 2);
    }

    #[test]
    fn a_built_task_is_not_offered_for_review_until_its_statement_is_rewritten() {
        let f = fixture();
        harvest_commits(&f.store, &f.args(Writes::Save)).expect("harvests");
        for mut task in f.store.tasks().expect("loads") {
            task.x_nasiko.status = TaskStatus::Reproducible;
            f.store.save_task(&task).expect("saves");
        }
        assert!(screen::needing_people(&f.store).expect("lists").is_empty());
        assert_eq!(awaiting_rewrite(&f.store).expect("lists").len(), 2);

        let mut rewritten = f.store.tasks().expect("loads").remove(0);
        rewritten.x_nasiko.statement_rewritten = true;
        f.store.save_task(&rewritten).expect("saves");
        let reviewable = screen::needing_people(&f.store).expect("lists");
        assert_eq!(reviewable.len(), 1);
        assert_eq!(reviewable[0].instance_id, rewritten.instance_id);
        assert_eq!(awaiting_rewrite(&f.store).expect("lists").len(), 1);
    }

    #[test]
    fn a_repository_without_commits_is_an_error_not_a_panic() {
        let repo_dir = tempfile::tempdir().expect("repo folder");
        let home = tempfile::tempdir().expect("eval home");
        git(repo_dir.path(), &["init", "-q"]);
        let args = args_for(repo_dir.path(), Writes::DryRun);
        let Err(error) = harvest_commits(&Store::new(EvalLayout::new(home.path())), &args) else {
            panic!("there are no commits to list");
        };
        assert!(
            format!("{error:#}").contains("does the repository have any?"),
            "{error:#}"
        );
    }

    #[test]
    fn the_funnel_names_every_outcome_in_the_order_commits_are_judged() {
        let mut summary = CommitHarvestSummary::new("demo", Writes::Save);
        summary.commits_listed = 20;
        summary.skipped_seen = 3;
        summary.outcomes = BTreeMap::from([
            (CommitOutcome::MergeOrRoot, 2),
            (CommitOutcome::NoTests, 9),
            (CommitOutcome::ThinMessage, 2),
            (CommitOutcome::Task, 4),
        ]);
        summary.thin_naming_an_issue = 1;
        summary.detailed_messages = 3;
        let rows: Vec<String> = summary
            .funnel()
            .lines()
            .map(|row| row.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        assert_eq!(
            rows,
            [
                "already harvested 3",
                "commits scanned 17",
                "merge or root commit 2",
                "too many files 0",
                "no Rust code change 0",
                "no tests 9",
                "too thin a message 2 (1 name an issue or pull request)",
                "unreadable 0",
                "became a candidate task 4 (3 with a detailed message)",
            ]
        );
        assert_eq!(summary.headline(), "demo: 20 commits reachable from HEAD");
    }

    #[test]
    fn counts_are_singular_for_one() {
        assert_eq!(counted(1, "new candidate task"), "1 new candidate task");
        assert_eq!(counted(0, "commit"), "0 commits");
        assert_eq!(counted(2, "commit"), "2 commits");
    }

    #[test]
    fn the_command_line_takes_since_max_redo_and_dry_run() {
        use clap::Parser;

        #[derive(Parser)]
        struct Cli {
            #[command(subcommand)]
            command: crate::EvalLocalCommands,
        }
        let cli = Cli::try_parse_from([
            "local",
            "harvest-commits",
            "--repo",
            "/r",
            "--since",
            "2025-01-01",
            "--max",
            "25",
            "--dry-run",
        ])
        .map_err(|e| e.to_string())
        .expect("parses");
        let crate::EvalLocalCommands::HarvestCommits {
            repo,
            since,
            max,
            redo,
            dry_run,
        } = cli.command
        else {
            panic!("expected harvest-commits");
        };
        assert_eq!(repo, PathBuf::from("/r"));
        assert_eq!(since.map(|d| d.to_string()).as_deref(), Some("2025-01-01"));
        assert_eq!((max, redo, dry_run), (Some(25), false, true));
    }
}
