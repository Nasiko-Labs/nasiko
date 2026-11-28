//! Reads what a finished run left on disk beyond pass or fail, for the report: each attempt's
//! raw patch (did it touch existing tests), its graded patch next to the real fix (scope), and
//! the agent's session export (did it check its work).
//!
//! Everything is computed at report time, so it also works on runs made before these sections
//! existed. A file that is missing or unreadable is never guessed at: the attempt is left out
//! of that figure, or a stated fallback is used, and [`RunSignals::gaps`] says which and why.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use nasiko_evals::scope::{self, ScopeDiff, ScopeSummary};
use nasiko_evals::selfcheck::{self, EDIT_TOOLS, SelfCheck, SelfCheckSummary};
use nasiko_evals::spec::EnvSpec;
use nasiko_evals::tamper::{self, TamperReport, TamperSummary, TamperVerdict};
use nasiko_evals::{FileDiff, SessionSource, TaskInstance, TestPathRules, agent, diff};
use serde::Serialize;

use crate::commands::eval::dataset;
use crate::commands::eval::run::{AttemptFile, RunInfo};
use crate::commands::eval::store::Store;

/// The patch that was graded, next to `attempt.json`.
const GRADED_PATCH_FILE: &str = "model.patch";

/// Most attempts named one by one under "Not read"; the rest are counted.
const MAX_NAMED_GAPS: usize = 8;

/// One attempt's signals. `None` means the files needed were not there.
#[derive(Debug, Clone, Serialize)]
pub struct AttemptSignals {
    pub instance_id: String,
    pub model: String,
    pub attempt: u32,
    pub resolved: bool,
    /// Whether the agent handed in a patch that changes at least one file.
    pub handed_in_patch: bool,
    pub tamper: Option<TamperReport>,
    /// `None` also when the attempt handed in no code change: there is nothing to compare.
    pub scope: Option<ScopeDiff>,
    pub self_check: Option<SelfCheck>,
}

impl AttemptSignals {
    /// Whether this attempt edited, removed or disabled tests that were already there.
    pub fn touched_existing_tests(&self) -> bool {
        self.tamper
            .as_ref()
            .is_some_and(|t| t.verdict != TamperVerdict::LeftTestsAlone)
    }

    /// Whether it did so in a test file the real commit left alone.
    pub fn touched_tests_the_real_commit_did_not(&self) -> bool {
        self.touched_existing_tests()
            && self
                .tamper
                .as_ref()
                .is_some_and(|t| !t.only_where_real_commit_did())
    }

    /// Whether this attempt's change is much larger than the real fix.
    pub fn is_oversized(&self) -> bool {
        self.scope.as_ref().is_some_and(ScopeDiff::is_oversized)
    }

    /// Whether the attempt changed files although its session shows no edit-tool call.
    fn changed_files_without_edit_tools(&self) -> bool {
        self.handed_in_patch && self.self_check.as_ref().is_some_and(|c| c.edits == 0)
    }
}

/// One model's signals over its attempts. Each rate's `n` is the attempts it could be read for.
#[derive(Debug, Clone, Serialize)]
pub struct ModelSignals {
    pub model: String,
    pub attempts: u32,
    pub tamper: TamperSummary,
    pub scope: ScopeSummary,
    /// Attempts whose patch held build files (lock files) that were left out of the comparison.
    pub with_build_files: u32,
    pub self_check: SelfCheckSummary,
}

/// Everything the report shows beyond the grades.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunSignals {
    pub attempts: Vec<AttemptSignals>,
    /// Sorted by model name.
    pub models: Vec<ModelSignals>,
    /// What could not be read and which fallbacks were used, in plain words, so a missing
    /// figure is never silent.
    pub gaps: Vec<String>,
    /// Whether every task of the dataset was built from commit history, so no original session
    /// exists to compare with. False when the dataset could not be read.
    pub without_sessions: bool,
}

impl RunSignals {
    pub fn model(&self, name: &str) -> Option<&ModelSignals> {
        self.models.iter().find(|m| m.model == name)
    }

    pub fn of_task<'a>(
        &'a self,
        instance_id: &'a str,
        model: &'a str,
    ) -> impl Iterator<Item = &'a AttemptSignals> {
        self.attempts
            .iter()
            .filter(move |a| a.instance_id == instance_id && a.model == model)
    }
}

/// Reads the signals of every attempt of a run.
pub fn collect(store: &Store, info: &RunInfo, attempts: &[(PathBuf, AttemptFile)]) -> RunSignals {
    let mut reader = Reader::new(store, info);
    let attempts: Vec<AttemptSignals> = attempts
        .iter()
        .map(|(path, file)| reader.read_attempt(path, file))
        .collect();
    let mut models: Vec<&str> = attempts.iter().map(|a| a.model.as_str()).collect();
    models.sort();
    models.dedup();
    let without_sessions = reader.without_sessions();
    let mut gaps = reader.gaps();
    gaps.extend(edit_tool_gap(&attempts));
    RunSignals {
        models: models.iter().map(|m| summarise(m, &attempts)).collect(),
        gaps,
        attempts,
        without_sessions,
    }
}

fn summarise(model: &str, attempts: &[AttemptSignals]) -> ModelSignals {
    let mine: Vec<&AttemptSignals> = attempts.iter().filter(|a| a.model == model).collect();
    let tampers: Vec<&TamperReport> = mine.iter().filter_map(|a| a.tamper.as_ref()).collect();
    let scopes: Vec<(bool, &ScopeDiff)> = mine
        .iter()
        .filter_map(|a| Some((a.resolved, a.scope.as_ref()?)))
        .collect();
    let checks: Vec<(&SelfCheck, bool)> = mine
        .iter()
        .filter_map(|a| Some((a.self_check.as_ref()?, a.handed_in_patch)))
        .collect();
    let with_build_files = scopes
        .iter()
        .filter(|(_, scope)| !scope.build_files.is_empty())
        .count();
    ModelSignals {
        model: model.to_string(),
        attempts: mine.len() as u32,
        tamper: tamper::summarise(&tampers),
        scope: scope::summarise(&scopes),
        with_build_files: with_build_files as u32,
        self_check: selfcheck::summarise(&checks),
    }
}

/// Says so when attempts changed files without any edit-tool call. If that is every attempt
/// with a session, the likeliest reason is a session format this reader does not know.
fn edit_tool_gap(attempts: &[AttemptSignals]) -> Option<String> {
    let with_session = attempts.iter().filter(|a| a.self_check.is_some()).count();
    let unseen: Vec<&AttemptSignals> = attempts
        .iter()
        .filter(|a| a.changed_files_without_edit_tools())
        .collect();
    if unseen.is_empty() {
        return None;
    }
    let other_tools: BTreeSet<&str> = unseen
        .iter()
        .filter_map(|a| a.self_check.as_ref())
        .flat_map(|check| check.other_tools.iter().map(String::as_str))
        .collect();
    let tools_seen = if other_tools.is_empty() {
        "Their sessions called no other tool than the shell".to_string()
    } else {
        let names: Vec<&str> = other_tools.into_iter().collect();
        format!("Other tools their sessions called: {}", names.join(", "))
    };
    let verdict = if unseen.len() == with_session {
        "That is every attempt with a session, so the session format is probably not the one this report knows: do not rely on \"Checked its own work\" for this run."
    } else {
        "They are counted under \"Changed files another way\", not as unchecked."
    };
    Some(format!(
        "{} attempt(s) changed files although their session shows no call of an edit tool this report knows ({}). {tools_seen}. {verdict}",
        unseen.len(),
        EDIT_TOOLS.join(", "),
    ))
}

/// Reads attempts one by one, remembering the dataset, each repository's spec, and what was
/// missing.
struct Reader<'a> {
    store: &'a Store,
    dataset: String,
    tasks: BTreeMap<String, TaskInstance>,
    specs: BTreeMap<String, Option<EnvSpec>>,
    dataset_problem: Option<String>,
    /// Problems with a task or a spec, each said once however many attempts met it.
    shared_problems: BTreeSet<String>,
    without_raw_patch: u32,
    without_graded_patch: u32,
    without_code_change: u32,
    without_session: u32,
    /// Problems with one attempt's files.
    unreadable: Vec<String>,
}

impl<'a> Reader<'a> {
    fn new(store: &'a Store, info: &RunInfo) -> Self {
        let reference = format!("{}@{}", info.dataset, info.dataset_version);
        let (tasks, dataset_problem) = match dataset::load(store, &reference) {
            Ok((_, tasks)) => (tasks, None),
            Err(e) => (Vec::new(), Some(format!("{reference}: {e:#}"))),
        };
        Self {
            store,
            dataset: reference,
            tasks: tasks
                .into_iter()
                .map(|t| (t.instance_id.clone(), t))
                .collect(),
            specs: BTreeMap::new(),
            dataset_problem,
            shared_problems: BTreeSet::new(),
            without_raw_patch: 0,
            without_graded_patch: 0,
            without_code_change: 0,
            without_session: 0,
            unreadable: Vec::new(),
        }
    }

    /// Whether the dataset holds tasks and all of them come from commit history.
    fn without_sessions(&self) -> bool {
        !self.tasks.is_empty()
            && self
                .tasks
                .values()
                .all(|t| t.x_nasiko.source == SessionSource::CommitHistory)
    }

    fn read_attempt(&mut self, attempt_json: &Path, file: &AttemptFile) -> AttemptSignals {
        let record = &file.record;
        let dir = attempt_json.parent().unwrap_or(Path::new("."));
        let label = format!(
            "{} · {} · attempt {}",
            record.instance_id, record.model, record.attempt
        );
        let task = self.task(&record.instance_id);
        let spec = task.as_ref().and_then(|t| self.spec_of(&t.repo));
        let rules = spec
            .as_ref()
            .map(|s| TestPathRules::from_globs(&s.test_paths))
            .unwrap_or_else(TestPathRules::rust_default);
        let raw = self.raw_patch(dir, &label);
        let real_test_changes = task
            .as_ref()
            .map(|t| self.task_diff(t, "test patch", &t.test_patch))
            .unwrap_or_default();
        let tamper = raw
            .as_ref()
            .map(|files| tamper::inspect(files, &rules).with_real_commit(&real_test_changes));
        let handed_in_patch = raw.as_ref().is_some_and(|files| !files.is_empty());
        let scope = task.as_ref().and_then(|t| {
            let fix = self.task_diff(t, "real fix", &t.patch);
            let model = self.graded_patch(dir, &label, raw, &rules)?;
            self.scope(t, &model, &fix)
        });
        let self_check = self.self_check(dir, &label, spec.as_ref());
        AttemptSignals {
            instance_id: record.instance_id.clone(),
            model: record.model.clone(),
            attempt: record.attempt,
            resolved: record.grade.resolved,
            handed_in_patch,
            tamper,
            scope,
            self_check,
        }
    }

    /// The task as frozen in the run's dataset. A task the dataset does not hold is said once.
    fn task(&mut self, instance_id: &str) -> Option<TaskInstance> {
        let task = self.tasks.get(instance_id).cloned();
        if task.is_none() && self.dataset_problem.is_none() {
            self.shared_problems.insert(format!(
                "Task {instance_id} is not in dataset {}: its attempts were not compared with the real fix, and Rust's default test paths were used for them.",
                self.dataset
            ));
        }
        task
    }

    /// One of the task's own patches, parsed. One that is not a readable diff is said once and
    /// read as empty.
    fn task_diff(&mut self, task: &TaskInstance, name: &str, text: &str) -> Vec<FileDiff> {
        diff::parse_unified(text).unwrap_or_else(|e| {
            self.shared_problems.insert(format!(
                "The {name} of task {} in the dataset is not a readable diff ({e}): nothing was compared with it.",
                task.instance_id
            ));
            Vec::new()
        })
    }

    /// The spec of the task's repository, as `run` looks it up; read once per repository.
    fn spec_of(&mut self, repo: &str) -> Option<EnvSpec> {
        let slug = repo.rsplit('/').next().unwrap_or(repo);
        if let Some(spec) = self.specs.get(slug) {
            return spec.clone();
        }
        let spec = match self.store.load_spec(slug) {
            Ok(Some(spec)) => Some(spec),
            Ok(None) => {
                self.shared_problems.insert(format!(
                    "No spec on disk for {slug}: Rust's default test paths and cargo's build and test commands were assumed."
                ));
                None
            }
            Err(e) => {
                self.shared_problems.insert(format!(
                    "The spec of {slug} could not be read ({e:#}): Rust's default test paths and cargo's build and test commands were assumed."
                ));
                None
            }
        };
        self.specs.insert(slug.to_string(), spec.clone());
        spec
    }

    /// The patch as the agent handed it in, before grading dropped its test changes.
    fn raw_patch(&mut self, dir: &Path, label: &str) -> Option<Vec<FileDiff>> {
        let Ok(text) = fs::read_to_string(dir.join("out").join(agent::PATCH_FILE)) else {
            self.without_raw_patch += 1;
            return None;
        };
        match diff::parse_unified(&text) {
            Ok(files) => Some(files),
            Err(e) => {
                self.unreadable.push(format!("{label}: raw patch: {e}"));
                None
            }
        }
    }

    /// The patch that was graded (no test files, inline test edits undone where the run could).
    /// A run folder without a readable one falls back to the raw patch without its test files,
    /// and says so.
    fn graded_patch(
        &mut self,
        dir: &Path,
        label: &str,
        raw: Option<Vec<FileDiff>>,
        rules: &TestPathRules,
    ) -> Option<Vec<FileDiff>> {
        let graded = match fs::read_to_string(dir.join(GRADED_PATCH_FILE)) {
            Ok(text) => match diff::parse_unified(&text) {
                Ok(files) => Some(files),
                Err(e) => {
                    self.unreadable.push(format!(
                        "{label}: {GRADED_PATCH_FILE}: {e}; the raw patch without its test files was compared instead"
                    ));
                    None
                }
            },
            Err(_) => {
                self.without_graded_patch += u32::from(raw.is_some());
                None
            }
        };
        graded.or_else(|| raw.map(|files| diff::without_tests(files, rules)))
    }

    /// The model's change next to the real fix; `None` when either has nothing to compare.
    fn scope(
        &mut self,
        task: &TaskInstance,
        model: &[FileDiff],
        fix: &[FileDiff],
    ) -> Option<ScopeDiff> {
        let scope = scope::compare(model, fix);
        if scope.shared.is_empty() && scope.missed.is_empty() {
            self.shared_problems.insert(format!(
                "The real fix of task {} names no file that can be compared: its attempts are left out of the scope figures.",
                task.instance_id
            ));
            return None;
        }
        if !scope.has_model_change() {
            self.without_code_change += 1;
            return None;
        }
        Some(scope)
    }

    fn self_check(&mut self, dir: &Path, label: &str, spec: Option<&EnvSpec>) -> Option<SelfCheck> {
        let Ok(export) = fs::read_to_string(dir.join("out").join(agent::EXPORT_FILE)) else {
            self.without_session += 1;
            return None;
        };
        match selfcheck::inspect(&export, &selfcheck::spec_commands(spec)) {
            Ok(Some(check)) => Some(check),
            Ok(None) => {
                self.unreadable.push(format!(
                    "{label}: the session export lists shell calls without their commands"
                ));
                None
            }
            Err(e) => {
                self.unreadable
                    .push(format!("{label}: session export: {e}"));
                None
            }
        }
    }

    fn gaps(self) -> Vec<String> {
        let mut gaps = Vec::new();
        if let Some(problem) = self.dataset_problem {
            gaps.push(format!(
                "The frozen dataset could not be read ({problem}), so no change was compared with the real fix or with the real commit's test changes, and Rust's default test paths were used."
            ));
        }
        gaps.extend(self.shared_problems);
        let counted = [
            (
                self.without_raw_patch,
                "left no raw patch (the agent did not get as far as saving one) and are left out of the test-tampering figures",
            ),
            (
                self.without_graded_patch,
                "had no graded patch on disk; their raw patch without its test files was compared with the real fix instead",
            ),
            (
                self.without_code_change,
                "handed in no code change (nothing, or only test and lock files) and are left out of the scope figures",
            ),
            (
                self.without_session,
                "left no session export and are left out of the checked-its-work figures",
            ),
        ];
        for (count, what) in counted {
            if count > 0 {
                gaps.push(format!("{count} attempt(s) {what}."));
            }
        }
        let unnamed = self.unreadable.len().saturating_sub(MAX_NAMED_GAPS);
        gaps.extend(self.unreadable.into_iter().take(MAX_NAMED_GAPS));
        if unnamed > 0 {
            gaps.push(format!(
                "{unnamed} more attempt file(s) could not be read; the same kinds of problem as above."
            ));
        }
        gaps
    }
}
