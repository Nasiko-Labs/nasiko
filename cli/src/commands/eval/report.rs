//! `nasiko eval report <run>`: the headline sentence and the tables, as text, JSON or Markdown.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use nasiko_evals::consistency::{self, Consistency};
use nasiko_evals::report::{self, ModelSummary, Rate, TaskRow};
use nasiko_evals::scope::{OVERSIZE_RATIO, ScopeGroup};
use nasiko_evals::tamper::{TamperReport, TamperVerdict};

use super::run::{RunInfo, attempt_files};
use super::signals::{self, AttemptSignals, RunSignals};
use super::store::Store;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    Markdown,
}

pub fn render(store: &Store, run_id: &str, format: Format) -> Result<String> {
    let run_dir = store.layout().run_dir(run_id);
    let info: RunInfo = serde_json::from_str(
        &fs::read_to_string(run_dir.join("run.json"))
            .with_context(|| format!("no run {run_id}"))?,
    )?;
    let files = attempt_files(&run_dir)?;
    let signals = signals::collect(store, &info, &files);
    let records: Vec<_> = files.into_iter().map(|(_, f)| f.record).collect();
    let control = info.control.as_deref();
    let summaries = report::summarise(&records, control.unwrap_or(""));
    let consistency = consistency::per_model(&records);
    let tasks = report::task_table(&records);
    let lost_first_attempts = report::infra_failed_first_attempts(&records);
    let dataset = format!("{}@{}", info.dataset, info.dataset_version);
    let out = match format {
        Format::Json => serde_json::to_string_pretty(&serde_json::json!({
            "run": info,
            "summaries": summaries,
            "attempts": records,
            "consistency": consistency,
            "tasks": tasks,
            "sandbox_failed_first_attempts": lost_first_attempts,
            "signals": signals,
        }))?,
        Format::Text | Format::Markdown => markdown(&Report {
            info: &info,
            summaries: &summaries,
            control,
            dataset: &dataset,
            consistency: &consistency,
            tasks: &tasks,
            lost_first_attempts: &lost_first_attempts,
            signals: &signals,
        }),
    };
    if format == Format::Markdown {
        store.save_text(&run_dir.join("report.md"), &out)?;
    }
    if format == Format::Json {
        store.save_text(&run_dir.join("report.json"), &out)?;
    }
    Ok(out)
}

/// Everything the text report is written from.
struct Report<'a> {
    info: &'a RunInfo,
    summaries: &'a [ModelSummary],
    control: Option<&'a str>,
    dataset: &'a str,
    consistency: &'a [Consistency],
    tasks: &'a [TaskRow],
    /// Model → tasks whose first attempt failed on the sandbox.
    lost_first_attempts: &'a BTreeMap<String, Vec<String>>,
    signals: &'a RunSignals,
}

fn pct(v: f64) -> String {
    format!("{:.0}%", v * 100.0)
}

/// "67% (21%–94%)"; a dash when nothing was measured.
fn with_range(r: &Rate) -> String {
    if r.n == 0 {
        return "—".into();
    }
    format!("{} ({}–{})", pct(r.value), pct(r.low95), pct(r.high95))
}

fn markdown(r: &Report) -> String {
    [
        headlines_and_models(r),
        by_difficulty(r.summaries),
        failures(r.summaries),
        works_every_time(r.consistency),
        test_tampering(r),
        scope_against_the_fix(r),
        checked_its_work(r),
        per_task(r),
        not_read(&r.signals.gaps),
        footer(r),
    ]
    .concat()
}

fn headlines_and_models(r: &Report) -> String {
    let mut s = format!("# Replay eval {}\n\n", r.info.run_id);
    for m in r.summaries.iter().filter(|m| !m.is_control) {
        s.push_str(&format!(
            "**{}**\n\n",
            if r.signals.without_sessions {
                report::headline_without_sessions(m, r.control, r.dataset)
            } else {
                report::headline(m, r.control, r.dataset)
            }
        ));
    }
    s.push_str("| Model | Resolved (95% range) | Match | Cost (US$) | Per resolved | Saving vs control | Saving vs original | Unpriced |\n");
    s.push_str("|---|---|---|---|---|---|---|---|\n");
    for m in r.summaries {
        let opt = |v: Option<f64>, f: fn(f64) -> String| v.map(f).unwrap_or_else(|| "—".into());
        s.push_str(&format!(
            "| {}{} | {} ({}–{}), n={} | {} | {:.4} | {} | {} | {} | {} |\n",
            m.model,
            if r.info.builders_tested.contains(&m.model) {
                " (built the dataset: optimistic)"
            } else if m.is_control {
                " (control)"
            } else {
                ""
            },
            pct(m.resolved.value),
            pct(m.resolved.low95),
            pct(m.resolved.high95),
            m.resolved.n,
            opt(m.match_rate.map(|r| r.value), pct),
            m.cost_usd,
            opt(m.cost_per_resolved, |v| format!("{v:.4}")),
            opt(m.saving_vs_control, pct),
            opt(m.saving_vs_original, pct),
            m.unpriced_attempts,
        ));
    }
    for (model, tasks) in r.lost_first_attempts {
        s.push_str(&format!(
            "\n{model}: {} task(s) count as not resolved because their first attempt failed on the sandbox (infra_error), not on the model's work: {}. Run them again before reading this rate as the model's.\n",
            tasks.len(),
            tasks.join(", ")
        ));
    }
    s
}

fn by_difficulty(summaries: &[ModelSummary]) -> String {
    let mut s =
        String::from("\n## By difficulty\n\n| Model | Level | Resolved | n |\n|---|---|---|---|\n");
    for m in summaries {
        for (level, r) in &m.by_level {
            s.push_str(&format!(
                "| {} | {level} | {} | {} |\n",
                m.model,
                pct(r.value),
                r.n
            ));
        }
    }
    s
}

fn failures(summaries: &[ModelSummary]) -> String {
    let mut s =
        String::from("\n## Why attempts failed\n\n| Model | Reason | Count |\n|---|---|---|\n");
    for m in summaries {
        for (reason, n) in &m.failures {
            s.push_str(&format!("| {} | {reason} | {n} |\n", m.model));
        }
    }
    s
}

/// Whether a model solves a task in every attempt, or only in some.
fn works_every_time(consistency: &[Consistency]) -> String {
    let mut s = String::from("\n## Works every time\n\n");
    if consistency.is_empty() {
        s.push_str(
            "No task has more than one attempt, so this run cannot show whether a model solves a task every time or only sometimes. Run with `--attempts 3` to find out.\n",
        );
        return s;
    }
    s.push_str("Tasks with repeated attempts, by how many of the attempts solved them.\n\n");
    s.push_str("| Model | Attempts per task | Tasks | Solved every time (95% range) | Solved at least once | Solved only sometimes |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    for c in consistency {
        let attempts = if c.min_attempts == c.max_attempts {
            c.min_attempts.to_string()
        } else {
            format!("{}–{}", c.min_attempts, c.max_attempts)
        };
        s.push_str(&format!(
            "| {} | {attempts} | {} | {} | {} | {} |\n",
            c.model,
            c.every_time.n,
            with_range(&c.every_time),
            with_range(&c.at_least_once),
            with_range(&c.sometimes),
        ));
    }
    for c in consistency.iter().filter(|c| !c.sometimes_tasks.is_empty()) {
        s.push_str(&format!(
            "\nSolved only sometimes by {}: {}\n",
            c.model,
            c.sometimes_tasks.join(", ")
        ));
    }
    for c in consistency.iter().filter(|c| c.single_attempt_tasks > 0) {
        s.push_str(&format!(
            "\n{}: {} task(s) with a single attempt are left out.\n",
            c.model, c.single_attempt_tasks
        ));
    }
    for c in consistency
        .iter()
        .filter(|c| !c.sandbox_failed_tasks.is_empty())
    {
        s.push_str(&format!(
            "\n{}: an attempt on these tasks failed on the sandbox (infra_error), which counts as not solved here, so they may look less steady than the model is: {}\n",
            c.model,
            c.sandbox_failed_tasks.join(", ")
        ));
    }
    s
}

/// What each model did to tests that were already there.
fn test_tampering(r: &Report) -> String {
    let mut s = String::from("\n## Test tampering\n\n");
    s.push_str("A behaviour signal, not part of the score: grading drops a model's changes to test files before the hidden tests run. (Its edits to tests that sit inside source files are undone too when the repository's spec points at a local checkout; otherwise those are graded as handed in.) Read from each attempt's patch as the agent handed it in, by Rust's test conventions and the test paths of the repository's spec as it is on disk now.\n\n");
    s.push_str("| Model | Attempts read | No change to existing tests seen | Edited existing tests | Removed or disabled tests | Of those two, only in test files your commit changed too | Assert lines deleted | Wrote its own tests |\n");
    s.push_str("|---|---|---|---|---|---|---|---|\n");
    for m in models_in_order(r) {
        let t = &m.tamper;
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            m.model,
            t.left_tests_alone.n,
            with_range(&t.left_tests_alone),
            with_range(&t.edited_existing_tests),
            with_range(&t.removed_or_disabled_tests),
            t.only_where_real_commit_did,
            t.deleted_asserts,
            with_range(&t.wrote_own_tests),
        ));
    }
    let flagged: Vec<&AttemptSignals> = r
        .signals
        .attempts
        .iter()
        .filter(|a| a.touched_existing_tests())
        .collect();
    if !flagged.is_empty() {
        s.push('\n');
    }
    for a in flagged {
        if let Some(t) = &a.tamper {
            s.push_str(&format!(
                "- {} · {} · attempt {}: {}\n",
                a.instance_id,
                a.model,
                a.attempt,
                tamper_evidence(t)
            ));
        }
    }
    s.push_str(TAMPER_NOTES);
    s
}

/// How to read the tampering table, and what it cannot see.
const TAMPER_NOTES: &str = "
How to read this table:

- Removed or disabled: a test file or test function deleted or renamed, a test that lost its test attribute, or `#[ignore]`, `#[should_panic]` or a `#[cfg]` gate put on a test that was there.
- Edited: any other change to existing test code or to an existing file under a test path (fixtures, snapshots), including lines added inside an existing test. The files are named above for a person to read: this report does not judge whether an edit weakened a test.
- An edit can be needed. When your own commit changed the same test file, the line says so; an attempt that touched no other test file is counted in the \"only in test files your commit changed too\" column.
- \"No change to existing tests seen\" means the patch shows none of the above. It is not proof: a change deep inside a long test module of a source file can go unseen, and an assert swapped for a weaker one reads as an ordinary edit.
- Writing new test files or new test functions is counted under \"Wrote its own tests\", never as tampering.
";

fn tamper_evidence(t: &TamperReport) -> String {
    let verdict = match t.verdict {
        TamperVerdict::LeftTestsAlone => "left existing tests alone",
        TamperVerdict::EditedExistingTests => "edited existing tests",
        TamperVerdict::RemovedOrDisabledTests => "removed or disabled tests",
    };
    let mut evidence = Vec::new();
    let mut list = |what: &str, items: &[String]| {
        if !items.is_empty() {
            evidence.push(format!("{what} {}", items.join(", ")));
        }
    };
    list("deleted", &t.deleted_test_files);
    list("removed, renamed or unmarked", &t.removed_tests);
    list("changed test code in", &t.edited_test_files);
    list("changed test data", &t.changed_test_data);
    list("renamed or moved", &t.renamed_test_files);
    let mut count = |n: u32, what: &str| {
        if n > 0 {
            evidence.push(format!("{n} {what}"));
        }
    };
    count(t.ignored_tests, "`#[ignore]` added");
    count(t.should_panic_tests, "`#[should_panic]` added");
    count(t.cfg_gated_tests, "`#[cfg]` gate(s) added");
    count(t.deleted_asserts, "assert line(s) deleted");
    let real_commit = if t.also_changed_by_real_commit.is_empty() {
        String::new()
    } else {
        format!(
            "; your commit changed {} too",
            t.also_changed_by_real_commit.join(", ")
        )
    };
    format!("{verdict} ({}{real_commit})", evidence.join("; "))
}

/// How each model's changes differ from the real fix in files and size.
fn scope_against_the_fix(r: &Report) -> String {
    let mut s = String::from("\n## Scope against your fix\n\n");
    s.push_str("How each change differs from the fix your team made, test files and lock files (`Cargo.lock` and the like, which a build can rewrite by itself) left out of both. Different is not wrong: another correct fix can touch other files or be another size. Size is added plus removed lines.\n\n");
    s.push_str(&format!(
        "| Model | Attempts | Changes compared | Touched a file your fix touched | Median size vs your fix | Touched extra files | Left out a file your fix touched | More than {OVERSIZE_RATIO:.0}x your fix |\n"
    ));
    s.push_str("|---|---|---|---|---|---|---|---|\n");
    for m in models_in_order(r) {
        let groups = [
            ("all", &m.scope.all),
            ("resolved", &m.scope.resolved),
            ("not resolved", &m.scope.not_resolved),
        ];
        for (name, group) in groups {
            s.push_str(&scope_row(&m.model, name, group));
        }
    }
    for m in models_in_order(r).filter(|m| m.with_build_files > 0) {
        s.push_str(&format!(
            "\n{}: {} attempt(s) also changed lock files; those are not counted as the model's change.\n",
            m.model, m.with_build_files
        ));
    }
    s
}

fn scope_row(model: &str, name: &str, g: &ScopeGroup) -> String {
    format!(
        "| {model} | {name} | {} | {} | {} | {} | {} | {} |\n",
        g.found_place.n,
        with_range(&g.found_place),
        g.median_size_ratio
            .map(|ratio| format!("{ratio:.1}x"))
            .unwrap_or_else(|| "—".into()),
        with_range(&g.extra_files),
        with_range(&g.missed_files),
        with_range(&g.oversized),
    )
}

/// Whether each model ran the build or the tests after its last edit.
fn checked_its_work(r: &Report) -> String {
    let mut s = String::from("\n## Checked its own work\n\n");
    s.push_str("Whether the agent ran a build or test command (`cargo test`, `nextest`, `build`, `check`, `clippy`, or the spec's own build or test command) after its last file edit, read from its session. Only edits made with the agent's edit tools are seen. An attempt that changed files another way (a shell command such as `sed -i`) is counted apart: the order of its edits and checks cannot be told. A command counts when the agent ran it, whether or not it passed; a test script the spec does not name is not recognised.\n\n");
    s.push_str("| Model | Attempts with edit-tool calls | Ran build or tests after the last edit (95% range) | Never ran them | Changed files another way (cannot tell) | No edit-tool calls, no change |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    for m in models_in_order(r) {
        let c = &m.self_check;
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            m.model,
            c.checked_after_last_edit.n,
            with_range(&c.checked_after_last_edit),
            c.never_checked,
            c.edited_outside_edit_tools,
            c.without_edits,
        ));
    }
    s
}

/// Marks in a per-task cell.
const TAMPER_MARK: &str = "T";
const SHARED_TEST_EDIT_MARK: &str = "t";
const OVERSIZE_MARK: &str = "L";

/// One row per task, one column per model: resolved attempts out of attempts made.
fn per_task(r: &Report) -> String {
    let models: Vec<&str> = r.summaries.iter().map(|m| m.model.as_str()).collect();
    let mut s = String::from("\n## Per task\n\n");
    s.push_str(&format!(
        "Attempts that resolved the task, out of attempts made. `{TAMPER_MARK}`: an attempt edited, removed or disabled existing tests in a test file your commit left alone. `{SHARED_TEST_EDIT_MARK}`: it did so only in test files your commit changed too. `{OVERSIZE_MARK}`: an attempt's change was more than {OVERSIZE_RATIO:.0}x the size of your fix.\n\n"
    ));
    s.push_str(&format!("| Task | {} |\n", models.join(" | ")));
    s.push_str(&format!("|---|{}\n", "---|".repeat(models.len())));
    for row in r.tasks {
        let cells: Vec<String> = models
            .iter()
            .map(|model| task_cell(r.signals, row, model))
            .collect();
        s.push_str(&format!(
            "| {} | {} |\n",
            row.instance_id,
            cells.join(" | ")
        ));
    }
    s
}

fn task_cell(signals: &RunSignals, row: &TaskRow, model: &str) -> String {
    let Some(cell) = row.cells.get(model) else {
        return "—".into();
    };
    let mut text = format!("{}/{}", cell.resolved, cell.attempts);
    let attempts: Vec<&AttemptSignals> = signals.of_task(&row.instance_id, model).collect();
    if attempts
        .iter()
        .any(|a| a.touched_tests_the_real_commit_did_not())
    {
        text.push_str(&format!(" {TAMPER_MARK}"));
    } else if attempts.iter().any(|a| a.touched_existing_tests()) {
        text.push_str(&format!(" {SHARED_TEST_EDIT_MARK}"));
    }
    if attempts.iter().any(|a| a.is_oversized()) {
        text.push_str(&format!(" {OVERSIZE_MARK}"));
    }
    text
}

fn not_read(gaps: &[String]) -> String {
    if gaps.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n## Not read\n\n");
    for gap in gaps {
        s.push_str(&format!("- {gap}\n"));
    }
    s
}

fn footer(r: &Report) -> String {
    format!(
        "\nDataset {} · prompt {} · opencode {} · {} attempt(s) per task · costs from the Nasiko router.\nResolved, match and by-difficulty rates count each task once, by its first attempt; further attempts are used only under \"Works every time\" and \"Per task\". Cost and cost per resolved cover every attempt: the cost of all attempts divided by the attempts that resolved, not by tasks. Failure reasons, test tampering, scope and checked-its-work count attempts. Ranges are 95% Wilson score intervals.\n",
        r.dataset, r.info.prompt_version, r.info.opencode_version, r.info.attempts
    )
}

/// The models' signals in the order the model table lists them (control first).
fn models_in_order<'a>(r: &'a Report) -> impl Iterator<Item = &'a signals::ModelSignals> {
    r.summaries.iter().filter_map(|m| r.signals.model(&m.model))
}

/// Whether a run folder exists (for friendlier errors).
pub fn run_exists(store: &Store, run_id: &str) -> bool {
    Path::new(&store.layout().run_dir(run_id))
        .join("run.json")
        .exists()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use nasiko_evals::grade::{FailureClass, Grade, Resolution};
    use nasiko_evals::report::AttemptRecord;
    use nasiko_evals::{EvalLayout, TaskInstance, TaskStatus, patch, spec};
    use serde_json::json;

    use super::super::dataset;
    use super::super::run::AttemptFile;
    use super::*;

    const RUN: &str = "run-1";
    /// The id of the task in `testdata/task.json`; its real fix changes one line of
    /// `src/lib.rs`.
    const TASK: &str = "repo__abcd1234";

    /// A store with the frozen one-task dataset `ds@1`, its spec, and `run.json` for `RUN`.
    fn store_with_run(dir: &Path, attempts: u32) -> Store {
        let store = Store::new(EvalLayout::new(dir));
        let mut task: TaskInstance =
            serde_json::from_str(include_str!("testdata/task.json")).expect("task");
        task.x_nasiko.status = TaskStatus::Verified;
        store.save_task(&task).expect("saves");
        dataset::freeze(&store, "ds").expect("freezes");
        let spec = spec::propose("o/repo", &["Cargo.toml".to_string()], None).expect("cargo");
        store.save_spec("repo", &spec).expect("saves");
        let info = RunInfo {
            run_id: RUN.into(),
            dataset: "ds".into(),
            dataset_version: 1,
            models: vec!["claude".into(), "flint".into()],
            control: Some("claude".into()),
            builders_tested: vec![],
            attempts,
            prompt_version: "p".into(),
            opencode_version: "1".into(),
            started_at: Utc::now(),
            finished_at: None,
        };
        let run_json = store.layout().run_dir(RUN).join("run.json");
        store.save_json(&run_json, &info).expect("saves");
        store
    }

    /// What one attempt left on disk.
    struct Attempt<'a> {
        model: &'a str,
        attempt: u32,
        resolved: bool,
        /// The patch as the agent handed it in.
        raw_patch: &'a str,
        /// The patch as graded: `raw_patch` without its test changes.
        graded_patch: &'a str,
        /// Shell commands and edits of the session, in order; `None` leaves no session export.
        session: Option<serde_json::Value>,
    }

    fn attempt_dir(store: &Store, model: &str, attempt: u32) -> std::path::PathBuf {
        store
            .layout()
            .run_dir(RUN)
            .join("attempts")
            .join(TASK)
            .join(model)
            .join(attempt.to_string())
    }

    /// Rewrites a saved attempt as one that failed on the sandbox.
    fn mark_lost_to_sandbox(store: &Store, model: &str, attempt: u32) {
        let path = attempt_dir(store, model, attempt).join("attempt.json");
        let text = fs::read_to_string(&path).expect("saved attempt");
        let mut file: AttemptFile = serde_json::from_str(&text).expect("attempt file");
        file.record.grade.failure = Some(FailureClass::InfraError);
        store.save_json(&path, &file).expect("saves");
    }

    /// One attempt by `model` that resolved the task by handing in `patch` as it is.
    fn save_resolved(store: &Store, model: &str, attempt: u32, patch: &str) {
        save_attempt(
            store,
            &Attempt {
                model,
                attempt,
                resolved: true,
                raw_patch: patch,
                graded_patch: patch,
                session: Some(json!([edit(), shell("cargo test")])),
            },
        );
    }

    fn save_attempt(store: &Store, a: &Attempt) {
        let dir = attempt_dir(store, a.model, a.attempt);
        fs::create_dir_all(dir.join("out")).expect("dir");
        fs::write(dir.join("out/raw.patch"), a.raw_patch).expect("write");
        fs::write(dir.join("model.patch"), a.graded_patch).expect("write");
        if let Some(parts) = &a.session {
            let export = json!({"messages": [{"info": {}, "parts": parts}]});
            fs::write(dir.join("out/session-export.json"), export.to_string()).expect("write");
        }
        let record = AttemptRecord {
            instance_id: TASK.into(),
            model: a.model.into(),
            attempt: a.attempt,
            difficulty: None,
            grade: Grade {
                resolution: if a.resolved {
                    Resolution::Full
                } else {
                    Resolution::No
                },
                resolved: a.resolved,
                f2p_passed: u32::from(a.resolved),
                f2p_total: 1,
                p2p_passed: 0,
                p2p_total: 0,
                failure: (!a.resolved).then_some(FailureClass::TestsFailed),
            },
            usage: None,
            cost_usd: Some(1.0),
            worst_price: None,
            wall_secs: 1,
            original_cost_usd: None,
        };
        let file = AttemptFile {
            tag: format!("{RUN}:{}:{TASK}:{}", a.model, a.attempt),
            agent_tokens: None,
            record,
        };
        store
            .save_json(&dir.join("attempt.json"), &file)
            .expect("saves");
    }

    fn shell(command: &str) -> serde_json::Value {
        json!({"type": "tool", "tool": "bash", "state": {"input": {"command": command}}})
    }

    fn edit() -> serde_json::Value {
        json!({"type": "tool", "tool": "edit", "state": {"status": "completed"}})
    }

    /// The real fix of `testdata/task.json`, as a model would hand it in.
    fn the_real_fix() -> String {
        patch::diff_texts(
            "src/lib.rs",
            Some("fn b() -> u8 { 1 }\n"),
            "fn b() -> u8 { 2 }\n",
        )
    }

    /// A fix in the right file that is four times the size of the real one.
    fn a_large_fix() -> String {
        patch::diff_texts(
            "src/lib.rs",
            Some("fn b() -> u8 { 1 }\n"),
            "fn b() -> u8 {\n    let one = 1;\n    let two = one + one;\n    let answer = two;\n    let checked = answer;\n    checked\n}\n",
        )
    }

    const DELETES_A_TEST_FILE: &str = "diff --git a/tests/old.rs b/tests/old.rs\ndeleted file mode 100644\n--- a/tests/old.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-#[test]\n-fn old() {}\n";

    /// Two attempts per task: the control solves it twice with the real fix; flint solves it
    /// once with a large change that also deletes a test file, and fails once with a change
    /// elsewhere plus a test of its own.
    fn a_two_attempt_run(dir: &Path) -> Store {
        let store = store_with_run(dir, 2);
        let fix = the_real_fix();
        for attempt in [1, 2] {
            save_attempt(
                &store,
                &Attempt {
                    model: "claude",
                    attempt,
                    resolved: true,
                    raw_patch: &fix,
                    graded_patch: &fix,
                    session: Some(json!([edit(), shell("cargo test -p repo")])),
                },
            );
        }
        let large = a_large_fix();
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 1,
                resolved: true,
                raw_patch: &format!("{large}{DELETES_A_TEST_FILE}"),
                graded_patch: &large,
                session: Some(json!([shell("cargo build"), edit()])),
            },
        );
        let elsewhere = patch::diff_texts("src/other.rs", Some("fn c() {}\n"), "fn c() { b(); }\n");
        let own_test = patch::diff_texts("tests/mine.rs", None, "#[test]\nfn mine() {}\n");
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 2,
                resolved: false,
                raw_patch: &format!("{elsewhere}{own_test}"),
                graded_patch: &elsewhere,
                session: None,
            },
        );
        store
    }

    /// The table row that starts with these cells.
    fn row<'a>(markdown: &'a str, start: &str) -> &'a str {
        markdown
            .lines()
            .find(|l| l.starts_with(start))
            .unwrap_or_else(|| panic!("no row starting {start:?} in:\n{markdown}"))
    }

    #[test]
    fn the_headline_rate_counts_tasks_and_keeps_a_range_at_100_percent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let md = render(&a_two_attempt_run(dir.path()), RUN, Format::Markdown).expect("renders");
        let one_of_one = with_range(&report::rate_of(1, 1));
        assert_eq!(one_of_one, "100% (21%–100%)");
        // Two attempts on one task are one task, not two.
        assert!(
            row(&md, "| claude (control) |").contains("100% (21%–100%), n=1"),
            "{md}"
        );
        assert!(
            row(&md, "| flint |").contains("100% (21%–100%), n=1"),
            "{md}"
        );
        assert!(!md.contains("(100%–100%)"), "{md}");
        assert!(md.contains("count each task once, by its first attempt"));
        assert!(md.contains("Cost and cost per resolved cover every attempt"));
        assert!(md.contains("2 attempt(s) per task"));
        let saved = fs::read_to_string(dir.path().join("runs").join(RUN).join("report.md"));
        assert_eq!(saved.expect("saved"), md);
    }

    #[test]
    fn repeated_attempts_show_which_tasks_are_solved_only_sometimes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let md = render(&a_two_attempt_run(dir.path()), RUN, Format::Text).expect("renders");
        let always = with_range(&report::rate_of(1, 1));
        let never = with_range(&report::rate_of(0, 1));
        assert_eq!(
            row(&md, "| claude | 2 |"),
            format!("| claude | 2 | 1 | {always} | {always} | {never} |")
        );
        assert_eq!(
            row(&md, "| flint | 2 |"),
            format!("| flint | 2 | 1 | {never} | {always} | {always} |")
        );
        assert!(md.contains(&format!("Solved only sometimes by flint: {TASK}")));
        assert_eq!(
            row(&md, &format!("| {TASK} |")),
            format!("| {TASK} | 2/2 | 1/2 T L |")
        );
    }

    #[test]
    fn one_attempt_per_task_says_consistency_cannot_be_judged() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_with_run(dir.path(), 1);
        let fix = the_real_fix();
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 1,
                resolved: true,
                raw_patch: &fix,
                graded_patch: &fix,
                session: Some(json!([edit(), shell("cargo test")])),
            },
        );
        let md = render(&store, RUN, Format::Text).expect("renders");
        assert!(md.contains("cannot show whether a model solves a task every time"));
        assert!(md.contains("`--attempts 3`"));
        assert_eq!(
            row(&md, &format!("| {TASK} |")),
            format!("| {TASK} | 1/1 |")
        );
        assert!(!md.contains("## Not read"), "{md}");
    }

    #[test]
    fn tampering_is_listed_with_its_evidence_and_own_tests_apart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let md = render(&a_two_attempt_run(dir.path()), RUN, Format::Text).expect("renders");
        let (all, none, half) = (
            with_range(&report::rate_of(2, 2)),
            with_range(&report::rate_of(0, 2)),
            with_range(&report::rate_of(1, 2)),
        );
        let section = md.split("## Test tampering").nth(1).expect("section");
        assert_eq!(
            row(section, "| claude |"),
            format!("| claude | 2 | {all} | {none} | {none} | 0 | 0 | {none} |")
        );
        assert_eq!(
            row(section, "| flint |"),
            format!("| flint | 2 | {half} | {none} | {half} | 0 | 0 | {half} |")
        );
        assert!(
            md.contains(&format!(
                "- {TASK} · flint · attempt 1: removed or disabled tests (deleted tests/old.rs)"
            )),
            "{md}"
        );
        assert!(md.contains("not part of the score"));
        assert!(
            !md.contains("flint · attempt 2:"),
            "own tests are not evidence"
        );
    }

    #[test]
    fn scope_is_split_by_resolved_and_never_calls_a_change_wrong() {
        let dir = tempfile::tempdir().expect("temp dir");
        let md = render(&a_two_attempt_run(dir.path()), RUN, Format::Text).expect("renders");
        let (one, zero) = (
            with_range(&report::rate_of(1, 1)),
            with_range(&report::rate_of(0, 1)),
        );
        let section = md
            .split("## Scope against your fix")
            .nth(1)
            .expect("section");
        assert_eq!(
            row(section, "| flint | resolved |"),
            format!("| flint | resolved | 1 | {one} | 4.0x | {zero} | {zero} | {one} |")
        );
        assert_eq!(
            row(section, "| flint | not resolved |"),
            format!("| flint | not resolved | 1 | {zero} | 1.0x | {one} | {one} | {zero} |")
        );
        assert!(row(section, "| claude | not resolved |").contains("| 0 | — | — |"));
        assert!(section.contains("Different is not wrong"));
        assert!(!md.to_lowercase().contains("wrong fix"));
    }

    #[test]
    fn checking_its_work_is_read_from_the_session_and_gaps_are_named() {
        let dir = tempfile::tempdir().expect("temp dir");
        let md = render(&a_two_attempt_run(dir.path()), RUN, Format::Text).expect("renders");
        let section = md.split("## Checked its own work").nth(1).expect("section");
        assert_eq!(
            row(section, "| claude |"),
            format!(
                "| claude | 2 | {} | 0 | 0 | 0 |",
                with_range(&report::rate_of(2, 2))
            )
        );
        // flint built before its only edit, never after.
        assert_eq!(
            row(section, "| flint |"),
            format!(
                "| flint | 1 | {} | 0 | 0 | 0 |",
                with_range(&report::rate_of(0, 1))
            )
        );
        assert!(md.contains("- 1 attempt(s) left no session export"), "{md}");
        assert!(!md.contains("no call of an edit tool"), "{md}");
    }

    #[test]
    fn files_changed_through_the_shell_are_counted_apart_and_said() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_with_run(dir.path(), 1);
        let fix = the_real_fix();
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 1,
                resolved: true,
                raw_patch: &fix,
                graded_patch: &fix,
                session: Some(json!([
                    shell("sed -i 's/1/2/' src/lib.rs"),
                    json!({"type": "tool", "tool": "str_replace"}),
                    shell("echo \"now run cargo test\""),
                    shell("cd /testbed; cargo test | tail -5"),
                ])),
            },
        );
        let md = render(&store, RUN, Format::Text).expect("renders");
        let section = md.split("## Checked its own work").nth(1).expect("section");
        assert_eq!(row(section, "| flint |"), "| flint | 0 | — | 0 | 1 | 0 |");
        assert!(
            md.contains("- 1 attempt(s) changed files although their session shows no call of an edit tool this report knows"),
            "{md}"
        );
        assert!(md.contains("Other tools their sessions called: str_replace"));
        assert!(md.contains("do not rely on \"Checked its own work\" for this run"));
    }

    #[test]
    fn a_test_edit_your_commit_also_made_is_marked_apart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_with_run(dir.path(), 1);
        let fix = the_real_fix();
        let old_test = "#[test]\nfn old() {\n    assert_eq!(b(), 1);\n}\n";
        let new_test = old_test.replace("b(), 1", "b(), 2");
        // The task's own test patch changes `tests/it.rs`; nothing of it touches `tests/other.rs`.
        let same_file = patch::diff_texts("tests/it.rs", Some(old_test), &new_test);
        let other_file = patch::diff_texts("tests/other.rs", Some(old_test), &new_test);
        for (model, test_edit) in [("claude", &other_file), ("flint", &same_file)] {
            save_attempt(
                &store,
                &Attempt {
                    model,
                    attempt: 1,
                    resolved: true,
                    raw_patch: &format!("{fix}{test_edit}"),
                    graded_patch: &fix,
                    session: None,
                },
            );
        }
        let md = render(&store, RUN, Format::Text).expect("renders");
        assert_eq!(
            row(&md, &format!("| {TASK} |")),
            format!("| {TASK} | 1/1 T | 1/1 t |")
        );
        assert!(
            md.contains(&format!(
                "- {TASK} · flint · attempt 1: edited existing tests (changed test code in tests/it.rs; your commit changed tests/it.rs too)"
            )),
            "{md}"
        );
        assert!(md.contains(&format!(
            "- {TASK} · claude · attempt 1: edited existing tests (changed test code in tests/other.rs)"
        )));
        let section = md.split("## Test tampering").nth(1).expect("section");
        let (none, all) = (
            with_range(&report::rate_of(0, 1)),
            with_range(&report::rate_of(1, 1)),
        );
        assert_eq!(
            row(section, "| flint |"),
            format!("| flint | 1 | {none} | {all} | {none} | 1 | 0 | {none} |")
        );
        assert_eq!(
            row(section, "| claude |"),
            format!("| claude | 1 | {none} | {all} | {none} | 0 | 0 | {none} |")
        );
    }

    #[test]
    fn lock_file_churn_is_not_counted_as_the_models_change() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_with_run(dir.path(), 1);
        let lock = |word: &str| -> String { (0..40).map(|i| format!("{word} {i}\n")).collect() };
        let churn = patch::diff_texts("Cargo.lock", Some(&lock("old")), &lock("new"));
        save_resolved(&store, "flint", 1, &format!("{}{churn}", the_real_fix()));
        let md = render(&store, RUN, Format::Text).expect("renders");
        let (one, zero) = (
            with_range(&report::rate_of(1, 1)),
            with_range(&report::rate_of(0, 1)),
        );
        let section = md
            .split("## Scope against your fix")
            .nth(1)
            .expect("section");
        assert_eq!(
            row(section, "| flint | resolved |"),
            format!("| flint | resolved | 1 | {one} | 1.0x | {zero} | {zero} | {zero} |")
        );
        assert!(section.contains("flint: 1 attempt(s) also changed lock files"));
        assert_eq!(
            row(&md, &format!("| {TASK} |")),
            format!("| {TASK} | 1/1 |")
        );
    }

    #[test]
    fn a_first_attempt_lost_to_the_sandbox_is_named_not_hidden() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_with_run(dir.path(), 2);
        let fix = the_real_fix();
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 1,
                resolved: false,
                raw_patch: "",
                graded_patch: "",
                session: None,
            },
        );
        mark_lost_to_sandbox(&store, "flint", 1);
        save_resolved(&store, "flint", 2, &fix);
        let md = render(&store, RUN, Format::Text).expect("renders");
        assert!(row(&md, "| flint |").contains("0% (0%–79%), n=1"), "{md}");
        assert!(
            md.contains(&format!(
                "flint: 1 task(s) count as not resolved because their first attempt failed on the sandbox (infra_error), not on the model's work: {TASK}."
            )),
            "{md}"
        );
        assert!(md.contains("flint: an attempt on these tasks failed on the sandbox"));
        let json = render(&store, RUN, Format::Json).expect("renders");
        let v: serde_json::Value = serde_json::from_str(&json).expect("json");
        assert_eq!(v["sandbox_failed_first_attempts"]["flint"][0], TASK);
        assert_eq!(v["consistency"][0]["sandbox_failed_tasks"][0], TASK);
    }

    #[test]
    fn unreadable_files_are_named_up_to_a_limit_and_fallbacks_are_said() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_with_run(dir.path(), 10);
        let fix = the_real_fix();
        for attempt in 1..=10 {
            save_resolved(&store, "flint", attempt, &fix);
            let attempt_dir = attempt_dir(&store, "flint", attempt);
            fs::write(attempt_dir.join("out/session-export.json"), "not json").expect("write");
            fs::remove_file(attempt_dir.join("model.patch")).expect("removes");
        }
        fs::write(store.layout().spec_file("repo"), "{ not a spec").expect("write");
        let md = render(&store, RUN, Format::Text).expect("renders");
        let section = md.split("## Not read").nth(1).expect("section");
        assert!(
            section.contains("- The spec of repo could not be read"),
            "{md}"
        );
        assert!(
            section.contains("- 10 attempt(s) had no graded patch on disk; their raw patch without its test files was compared with the real fix instead."),
            "{md}"
        );
        let named = section
            .lines()
            .filter(|line| line.contains("session export:"))
            .count();
        assert_eq!(named, 8, "{md}");
        assert!(
            section.contains("- 2 more attempt file(s) could not be read"),
            "{md}"
        );
        // The fallback still compared every attempt with the real fix.
        assert!(row(&md, "| flint | all |").starts_with("| flint | all | 10 |"));
    }

    #[test]
    fn a_run_with_only_attempt_records_still_renders() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = a_two_attempt_run(dir.path());
        let run_dir = store.layout().run_dir(RUN);
        for (attempt_json, _) in attempt_files(&run_dir).expect("lists") {
            let attempt_dir = attempt_json.parent().expect("attempt dir");
            fs::remove_dir_all(attempt_dir.join("out")).expect("removes");
            fs::remove_file(attempt_dir.join("model.patch")).expect("removes");
        }
        fs::remove_dir_all(store.layout().dataset_dir("ds", 1)).expect("removes");
        let md = render(&store, RUN, Format::Text).expect("renders");
        assert!(
            md.contains("- The frozen dataset could not be read"),
            "{md}"
        );
        assert!(md.contains("- 4 attempt(s) left no raw patch"), "{md}");
        assert_eq!(
            row(&md, &format!("| {TASK} |")),
            format!("| {TASK} | 2/2 | 1/2 |")
        );
        assert!(row(&md, "| flint | all |").contains("| 0 | — | — |"));
    }

    #[test]
    fn json_keeps_the_old_keys_and_adds_the_new_ones() {
        let dir = tempfile::tempdir().expect("temp dir");
        let out = render(&a_two_attempt_run(dir.path()), RUN, Format::Json).expect("renders");
        let v: serde_json::Value = serde_json::from_str(&out).expect("json");
        assert_eq!(v["run"]["run_id"], RUN);
        assert_eq!(v["summaries"][0]["model"], "claude");
        assert_eq!(v["summaries"][1]["resolved"]["n"], 1);
        assert_eq!(v["attempts"].as_array().map(Vec::len), Some(4));
        assert_eq!(v["consistency"][1]["sometimes_tasks"][0], TASK);
        assert_eq!(v["tasks"][0]["cells"]["flint"]["resolved"], 1);
        let flint_first = &v["signals"]["attempts"][2];
        assert_eq!(flint_first["model"], "flint");
        assert_eq!(
            flint_first["tamper"]["verdict"],
            "removed_or_disabled_tests"
        );
        assert_eq!(flint_first["scope"]["size_ratio"], 4.0);
        assert_eq!(flint_first["handed_in_patch"], true);
        assert!(v["sandbox_failed_first_attempts"].is_object());
        assert_eq!(flint_first["self_check"]["checked_after_last_edit"], false);
        assert_eq!(
            v["signals"]["attempts"][3]["tamper"]["wrote_own_tests"],
            true
        );
        assert_eq!(
            v["signals"]["models"][1]["scope"]["resolved"]["oversized"]["value"],
            1.0
        );
    }
}
