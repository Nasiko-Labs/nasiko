//! Reads what a finished run left on disk beyond pass or fail, for the report: each attempt's
//! raw patch (did it touch existing tests), its graded patch next to the real fix (scope), and
//! the agent's session export (how it worked: where it looked, what it ran and when, how much
//! it rewrote, whether it tested its change).
//!
//! Everything is computed at report time, so it also works on runs made before these sections
//! existed. A file that is missing or unreadable is never guessed at: the attempt is left out
//! of that figure, or a stated fallback is used, and [`RunSignals::gaps`] says which and why.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use nasiko_evals::method::{self, Evidence, Method, MethodSummary};
use nasiko_evals::scope::{self, ScopeDiff, ScopeSummary};
use nasiko_evals::selfcheck::{self, SelfCheck, SelfCheckSummary};
use nasiko_evals::spec::EnvSpec;
use nasiko_evals::tamper::{self, TamperReport, TamperSummary, TamperVerdict};
use nasiko_evals::trace::{self, Trace};
use nasiko_evals::{FileDiff, SessionSource, TaskInstance, TestPathRules, agent, diff, story};
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
    /// How the agent worked, read from its session. `None` without a readable session.
    pub method: Option<Method>,
    /// One plain sentence per measure of how it worked that could be read.
    pub how_it_worked: Vec<String>,
    /// For an attempt that did not resolve its task and left a readable session: what stands
    /// out in how it worked, as one line. A likely story, not the reason it failed.
    pub story: Option<String>,
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
    /// [`Self::self_check`] split by whether the attempt resolved its task.
    pub self_check_by_outcome: ByOutcome<SelfCheckSummary>,
    pub method: MethodSummary,
}

/// A figure for the attempts that resolved their task and for those that did not.
#[derive(Debug, Clone, Serialize)]
pub struct ByOutcome<T> {
    pub resolved: T,
    pub not_resolved: T,
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
    let checks_of = |resolved: Option<bool>| -> Vec<(&SelfCheck, bool)> {
        mine.iter()
            .filter(|a| resolved.is_none_or(|wanted| a.resolved == wanted))
            .filter_map(|a| Some((a.self_check.as_ref()?, a.handed_in_patch)))
            .collect()
    };
    let methods: Vec<(bool, &Method)> = mine
        .iter()
        .filter_map(|a| Some((a.resolved, a.method.as_ref()?)))
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
        self_check: selfcheck::summarise(&checks_of(None)),
        self_check_by_outcome: ByOutcome {
            resolved: selfcheck::summarise(&checks_of(Some(true))),
            not_resolved: selfcheck::summarise(&checks_of(Some(false))),
        },
        method: method::summarise(&methods),
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
        "That is every attempt with a session, so the session format is probably not the one this report knows: do not rely on \"How it worked\" for this run."
    } else {
        "They are counted under \"Changed files another way\", not as untested, and are left out of the figures that need the order of edits."
    };
    Some(format!(
        "{} attempt(s) changed files although their session shows no call of an edit tool this report knows ({}). {tools_seen}. {verdict}",
        unseen.len(),
        trace::edit_tools().join(", "),
    ))
}

/// What an attempt's session shows of how the agent worked; all empty without a session.
#[derive(Default)]
struct Working {
    self_check: Option<SelfCheck>,
    method: Option<Method>,
    sentences: Vec<String>,
    story: Option<String>,
}

impl Working {
    /// Measures the agent's steps.
    fn read(trace: &Trace, evidence: &Evidence) -> Self {
        let self_check = selfcheck::of_trace(trace, evidence.own_commands);
        let method = method::measure(trace, evidence);
        Self {
            sentences: method.sentences(Some(&self_check)),
            story: None,
            self_check: Some(self_check),
            method: Some(method),
        }
    }

    /// Adds the story of an attempt that did not resolve its task.
    fn with_story(mut self) -> Self {
        self.story = self
            .method
            .as_ref()
            .map(|method| story::story(method, self.self_check.as_ref()));
        self
    }
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
    /// Attempts whose session has tool calls that could not be read.
    with_unread_calls: u32,
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
            with_unread_calls: 0,
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
        let fix = task
            .as_ref()
            .map(|t| self.task_diff(t, "real fix", &t.patch));
        let graded = self.graded_patch(dir, &label, raw.as_deref(), &rules);
        let scope = match (&task, &fix, &graded) {
            (Some(task), Some(fix), Some(model)) => self.scope(task, model, fix),
            _ => None,
        };
        let own_commands = selfcheck::spec_commands(spec.as_ref());
        let evidence = Evidence {
            fix: fix.as_deref(),
            graded: graded.as_deref(),
            handed_in: raw.as_deref(),
            rules: &rules,
            own_commands: &own_commands,
        };
        let resolved = record.grade.resolved;
        let working = self
            .trace(dir, &label, raw.as_deref())
            .map(|trace| Working::read(&trace, &evidence))
            .unwrap_or_default();
        let working = if resolved {
            working
        } else {
            working.with_story()
        };
        AttemptSignals {
            instance_id: record.instance_id.clone(),
            model: record.model.clone(),
            attempt: record.attempt,
            resolved,
            handed_in_patch,
            tamper,
            scope,
            self_check: working.self_check,
            method: working.method,
            how_it_worked: working.sentences,
            story: working.story,
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
        raw: Option<&[FileDiff]>,
        rules: &TestPathRules,
    ) -> Option<Vec<FileDiff>> {
        let graded = match fs::read_to_string(dir.join(GRADED_PATCH_FILE)) {
            Ok(text) => match diff::parse_unified(&text) {
                Ok(files) => Some(files),
                Err(e) => {
                    self.unreadable.push(format!(
                        "{label}: {GRADED_PATCH_FILE}: {e}; the raw patch without its test files was used instead"
                    ));
                    None
                }
            },
            Err(_) => {
                self.without_graded_patch += u32::from(raw.is_some());
                None
            }
        };
        graded.or_else(|| raw.map(|files| diff::without_tests(files.to_vec(), rules)))
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

    /// The agent's steps, from its session export. An export that is missing, not JSON or not
    /// laid out the way the reader knows is said and read as nothing. So is one that shows no
    /// step at all for an attempt whose patch as handed in (`raw`) changes files: the steps
    /// are there, in a form this reader does not see.
    fn trace(&mut self, dir: &Path, label: &str, raw: Option<&[FileDiff]>) -> Option<Trace> {
        let Ok(export) = fs::read_to_string(dir.join("out").join(agent::EXPORT_FILE)) else {
            self.without_session += 1;
            return None;
        };
        let changed_files = raw.is_some_and(|files| !files.is_empty());
        let problem = match trace::read_opencode_export(&export) {
            Ok(trace) if trace.steps.is_empty() && changed_files => {
                "it shows no tool call this report reads, yet the attempt changed files".into()
            }
            Ok(trace) => {
                self.with_unread_calls += u32::from(trace.unread_calls > 0);
                return Some(trace);
            }
            Err(e) => e.to_string(),
        };
        self.unreadable
            .push(format!("{label}: session export: {problem}"));
        None
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
                "left no raw patch (the agent did not get as far as saving one) and are left out of the test-tampering figures; where they have a graded patch, \"Guessed and redid\" counts the lines they kept from it, so a test they wrote inside a source file reads as written and not kept",
            ),
            (
                self.without_graded_patch,
                "had no graded patch on disk; their raw patch without its test files was used in its place",
            ),
            (
                self.without_code_change,
                "handed in no code change (nothing, or only test and lock files) and are left out of the scope figures",
            ),
            (
                self.without_session,
                "left no session export and are left out of everything under \"How it worked\"",
            ),
            (
                self.with_unread_calls,
                "have tool calls in their session that lack the input this report reads (a call cut off while it ran, or a search in a form this report does not know); those calls are missing from the figures under \"How it worked\"",
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
