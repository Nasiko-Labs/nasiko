//! `nasiko eval report <run>`: the headline sentence and the tables, as text, JSON or Markdown.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use nasiko_evals::consistency::{self, Consistency};
use nasiko_evals::method::{MethodGroup, REWORK_RATIO};
use nasiko_evals::report::{self, ModelSummary, Rate, TaskRow};
use nasiko_evals::scope::{OVERSIZE_RATIO, ScopeGroup};
use nasiko_evals::selfcheck::SelfCheckSummary;
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

/// "2.5"; a dash when there is nothing to take a middle of.
fn median(value: Option<f64>) -> String {
    value.map_or_else(|| "—".into(), |v| format!("{v:.1}"))
}

/// The report's sections in the order they are read. To reorder the report, move a line here
/// or in [`how_it_worked`] and [`what_it_produced`].
fn markdown(r: &Report) -> String {
    [
        headlines_and_models(r),
        how_it_worked(r),
        what_it_produced(r),
        per_task(r),
        failures(r.summaries),
        by_difficulty(r.summaries),
        test_tampering(r),
        not_read(&r.signals.gaps),
        footer(r),
    ]
    .concat()
}

/// How each agent went about the task, the most telling measure first.
fn how_it_worked(r: &Report) -> String {
    [
        HOW_IT_WORKED.to_string(),
        bigger_picture(r),
        ran_before_changing(r),
        guessed_and_redid(r),
        tested_its_fix(r),
        read_before_changing(r),
    ]
    .concat()
}

/// What each agent handed in: how steadily it solves a task, and its change next to the fix.
fn what_it_produced(r: &Report) -> String {
    [
        "\n## What it produced\n".to_string(),
        works_every_time(r.consistency),
        scope_against_the_fix(r),
    ]
    .concat()
}

const HOW_IT_WORKED: &str = "
## How it worked

How each agent went about the task, read from its own session record (the files it opened, the searches and commands it ran, the edits it made) next to its patch and your fix. These are signals about method, not verdicts: a correct change can come out of a session that reads badly here, and the other way round. A session records only part of what an agent did, so where a figure could not be told it is counted as \"cannot tell\" and left out of the share, never guessed. Each table has a row for every attempt whose session was read, then one for the attempts that resolved their task and one for those that did not, so you can see which habits go with failure; with fewer than a handful of attempts in a row the range is wide and the row says little. The session reader was written from the opencode release this run pins and has not yet been checked against the export of a real run: if a table looks empty or implausible, read \"Not read\" first. Each attempt's own figures, with the files and names behind them, are in the JSON report (`signals.attempts[].method`).
";

/// One table row per model and group of its attempts (all, resolved, not resolved); `cells`
/// gives the columns after the two that name them.
fn method_rows(r: &Report, cells: impl Fn(&MethodGroup) -> Vec<String>) -> String {
    let mut s = String::new();
    for m in models_in_order(r) {
        let groups = [
            ("all", &m.method.all),
            ("resolved", &m.method.resolved),
            ("not resolved", &m.method.not_resolved),
        ];
        for (name, group) in groups {
            s.push_str(&format!(
                "| {} | {name} | {} |\n",
                m.model,
                cells(group).join(" | ")
            ));
        }
    }
    s
}

/// Did the agent look where the real fix is, search for the names of the shared items it
/// changed, and look widely before editing.
fn bigger_picture(r: &Report) -> String {
    let mut s = String::from("\n### Looked at the bigger picture\n\n");
    s.push_str("Where it looked. For every file your fix changed (files your fix added, test files and lock files left out): did the attempt's final patch change it too, did the agent look at it and leave it alone, or does its session show no look at it. \"Looked at\" means it opened the file, a search it ran listed it, or a shell command named it without only changing it. A broad search lists many files, so the files it looked at without ever opening them are counted apart. \"Cannot tell\" is a file a command may have shown without naming it (`cat src/*.rs`, `$(…)`, `xargs`, a script); an attempt with such a file and no file without a look is left out of the share.\n\n");
    s.push_str("| Model | Attempts | Compared with your fix | Of those, could be told | Looked at every file your fix changed (95% range) | Files of your fix it changed | Looked at, not changed | Of those, never opened | Cannot tell | No look shown |\n");
    s.push_str("|---|---|---|---|---|---|---|---|---|---|\n");
    s.push_str(&method_rows(r, |g| {
        vec![
            g.compared_with_fix.to_string(),
            g.looked_at_every_fix_file.n.to_string(),
            with_range(&g.looked_at_every_fix_file),
            g.fix_files_changed.to_string(),
            g.fix_files_looked_not_changed.to_string(),
            g.fix_files_never_opened.to_string(),
            g.fix_files_cannot_tell.to_string(),
            g.fix_files_never_looked.to_string(),
        ]
    }));
    s.push_str("\nSearched for what it changed. A shared item is a Rust `pub` function, type, constant or field whose declaration the attempt's patch removes or changes (the line up to its body; a line that only moved or was reformatted is not a change, and a signature spread over several lines is seen only when its first line changes). \"Searched for\" means the agent ran a contents search (its search tool, `grep`, `rg`) for the item's name as a whole word, beyond the file that declares it, at any point of its session. That is the nearest a session comes to \"checked who uses it\": it does not show that the search found the users or that the agent read them, and a short name (`new`) is matched by searches for other things. Rust only: a patch without Rust code is not read, and is not counted as changing nothing.\n\n");
    s.push_str("| Model | Attempts | Patches with Rust code (read) | Changed a shared item | Searched for the name of every one (95% range) | Shared items changed | Of which searched for |\n");
    s.push_str("|---|---|---|---|---|---|---|\n");
    s.push_str(&method_rows(r, |g| {
        vec![
            g.shared_items_read.to_string(),
            g.searched_for_every_shared_item.n.to_string(),
            with_range(&g.searched_for_every_shared_item),
            g.shared_items_changed.to_string(),
            g.shared_items_searched.to_string(),
        ]
    }));
    s.push_str("\nHow widely, and when. Files opened are those it read or pointed a shell viewer or search at by name (files a glob stands for are not counted); looking is opening, listing or searching.\n\n");
    s.push_str("| Model | Attempts | Sessions read | Median files opened | Median files edited | Median share of its looking done before its first edit |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    s.push_str(&method_rows(r, |g| {
        vec![
            g.sessions.to_string(),
            median(g.median_files_looked_at),
            median(g.median_files_edited),
            g.median_share_of_looking_before_first_edit
                .map_or_else(|| "—".into(), pct),
        ]
    }));
    s
}

/// Whether the agent ran the build or tests before it first changed existing code.
fn ran_before_changing(r: &Report) -> String {
    let mut s = String::from("\n### Ran the build or tests before changing code\n\n");
    s.push_str("The nearest a session comes to \"saw the problem before changing anything\": whether the agent ran a build or test command before it first changed code that was already there. Writing or adjusting a file of its own (a reproduction, say), writing outside the repository, editing a test file and adding a Rust test to a source file do not count as that change. A command that did not start (not found) is not a run. \"Showed a failure\" means such a run ended with a failing exit status of its own, or, where the status is another command's (`cargo test | tail`), with failure text in the output the session kept. Neither figure shows that the agent saw this task's problem: the failure can be about something else, and an agent that understood the problem by reading reads as \"did not run first\".\n\n");
    s.push_str("| Model | Attempts | Changed existing code with edit tools | Ran build or tests before that (95% range) | Of those, a run showed a failure |\n");
    s.push_str("|---|---|---|---|---|\n");
    s.push_str(&method_rows(r, |g| {
        vec![
            g.ran_before_first_change.n.to_string(),
            with_range(&g.ran_before_first_change),
            g.first_run_showed_failure.to_string(),
        ]
    }));
    s
}

/// How much the agent wrote against how much it kept.
fn guessed_and_redid(r: &Report) -> String {
    let mut s = String::from("\n### Guessed and redid\n\n");
    s.push_str("Lines the agent wrote over all its edits to files that are not test files, per line its patch as handed in adds to such files. 1.0 means it wrote its change once; 5.0 means five lines written for each one kept. A test it wrote inside a source file is in both counts. A whole-file rewrite counts every line of the file, so a model that rewrites files in full reads high, and a scratch file written inside the repository counts as written; under 1.0 means part of the change was made another way (a shell command). An attempt that wrote lines and kept none has no ratio: it is counted in its own column and with those that wrote more than they kept. Rewriting is not wrong: the figure shows how directly the agent got to its change.\n\n");
    s.push_str(&format!(
        "| Model | Attempts | Edited with edit tools | Median lines written per line kept | Wrote more than {REWORK_RATIO:.0}x what it kept, or kept none (95% range) | Kept none of what it wrote | Edited a file not in its final patch |\n"
    ));
    s.push_str("|---|---|---|---|---|---|---|\n");
    s.push_str(&method_rows(r, |g| {
        vec![
            g.reworked_heavily.n.to_string(),
            median(g.median_rework_ratio),
            with_range(&g.reworked_heavily),
            g.kept_nothing.to_string(),
            g.with_abandoned_files.to_string(),
        ]
    }));
    s
}

/// Whether the agent looked at the files it edited before editing them.
fn read_before_changing(r: &Report) -> String {
    let mut s = String::from("\n### Read before changing\n\n");
    s.push_str("Whether the agent looked at a file that was already there before its first edit to it: it opened the file, or a search it ran listed it. Files it made itself do not count. \"Cannot tell\" is a file a command named or may have shown first (a script given the path, a glob). Expect this to read high for every model: opencode's own edit tool tells the model to read a file before editing it, and an edit the tool turned down is not in the session's steps, so this figure says more about the tool than about the model.\n\n");
    s.push_str("| Model | Attempts | Edited existing files with edit tools | Of those, could be told | Looked at each one first (95% range) | Files edited with no look shown first | Cannot tell |\n");
    s.push_str("|---|---|---|---|---|---|---|\n");
    s.push_str(&method_rows(r, |g| {
        vec![
            g.edited_existing_files.to_string(),
            g.looked_before_every_edit.n.to_string(),
            with_range(&g.looked_before_every_edit),
            g.files_edited_without_looking.to_string(),
            g.files_edited_cannot_tell.to_string(),
        ]
    }));
    s
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
    let mut s = String::from("\n### Works every time\n\n");
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
    s.push_str("A closing note, not part of the score: grading drops a model's changes to test files before the hidden tests run. Read from each attempt's patch as the agent handed it in. An edit to a test can be needed (a line below says when your own commit changed the same test file), and \"no change seen\" is not proof: this report does not judge whether an edit weakened a test. `docs/EVALS.md` says how each column is counted.\n\n");
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
    s
}

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
    let mut s = String::from("\n### Scope against your fix\n\n");
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
fn tested_its_fix(r: &Report) -> String {
    let mut s = String::from("\n### Tested its own fix before saying done\n\n");
    s.push_str("Whether the agent ran a build or test command (`cargo test`, `nextest`, `build`, `check`, `clippy`, the usual build and test runners of Python, Go, Node and the JVM, `make test`, or the spec's own build or test command) after its last file edit. Only edits made with the agent's edit tools to files inside the repository are seen; every such edit counts, also a note written there after the last test run. An attempt that changed files another way (a shell command such as `sed -i`) is counted apart: the order of its edits and checks cannot be told. A command counts when the agent ran it, whether or not it passed, but not when it did not start (not found) or only listed tests. A test script the spec does not name is not recognised, so such an attempt reads as \"never ran them\".\n\n");
    s.push_str("| Model | Attempts | With edit-tool calls | Ran build or tests after the last edit (95% range) | Never ran them | Changed files another way (cannot tell) | No edit-tool calls, no change |\n");
    s.push_str("|---|---|---|---|---|---|---|\n");
    for m in models_in_order(r) {
        let groups = [
            ("all", &m.self_check),
            ("resolved", &m.self_check_by_outcome.resolved),
            ("not resolved", &m.self_check_by_outcome.not_resolved),
        ];
        for (name, group) in groups {
            s.push_str(&self_check_row(&m.model, name, group));
        }
    }
    s
}

fn self_check_row(model: &str, name: &str, c: &SelfCheckSummary) -> String {
    format!(
        "| {model} | {name} | {} | {} | {} | {} | {} |\n",
        c.checked_after_last_edit.n,
        with_range(&c.checked_after_last_edit),
        c.never_checked,
        c.edited_outside_edit_tools,
        c.without_edits,
    )
}

/// Most failed attempts whose story is spelled out under the per-task table; the rest are
/// counted.
const MAX_STORIES: usize = 40;

/// Most attempts spelled out sentence by sentence under the per-task table; the rest are
/// counted.
const MAX_ATTEMPT_LINES: usize = 20;

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
    s.push_str(&failed_attempts(r.signals));
    s.push_str(&attempt_by_attempt(r.signals));
    s
}

/// What each attempt's session shows, one plain sentence per measure of how it worked.
fn attempt_by_attempt(signals: &RunSignals) -> String {
    let read: Vec<&AttemptSignals> = signals
        .attempts
        .iter()
        .filter(|a| !a.how_it_worked.is_empty())
        .collect();
    if read.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n### Attempt by attempt\n\n");
    s.push_str("What each attempt's session shows, one sentence per measure under \"How it worked\", in that order. A measure that could not be read for an attempt has no sentence.\n\n");
    for a in read.iter().take(MAX_ATTEMPT_LINES) {
        let outcome = if a.resolved {
            "resolved"
        } else {
            "not resolved"
        };
        s.push_str(&format!(
            "- {} ({outcome}): {}\n",
            attempt_label(a),
            a.how_it_worked.join(" ")
        ));
    }
    let more = read.len().saturating_sub(MAX_ATTEMPT_LINES);
    if more > 0 {
        s.push_str(&format!(
            "\n{more} more attempt(s) are in the JSON report (`signals.attempts[].how_it_worked`).\n"
        ));
    }
    s
}

/// "task · model · attempt 2".
fn attempt_label(a: &AttemptSignals) -> String {
    format!("{} · {} · attempt {}", a.instance_id, a.model, a.attempt)
}

/// One line per attempt that did not resolve its task: what stands out in how it worked.
fn failed_attempts(signals: &RunSignals) -> String {
    let failed: Vec<&AttemptSignals> = signals.attempts.iter().filter(|a| !a.resolved).collect();
    if failed.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n### Failed attempts: what the session shows\n\n");
    s.push_str("One line per attempt that did not resolve its task: what its session shows and does not show, in the order of \"How it worked\", the first three that apply. A session records only part of what an agent did, and what could not be told is left out. A likely story to check against the session, not the reason the attempt failed.\n\n");
    for a in failed.iter().take(MAX_STORIES) {
        s.push_str(&format!(
            "- {}: {}\n",
            attempt_label(a),
            a.story
                .as_deref()
                .unwrap_or("Failed; it left no session this report could read.")
        ));
    }
    let more = failed.len().saturating_sub(MAX_STORIES);
    if more > 0 {
        s.push_str(&format!(
            "\n{more} more failed attempt(s) are in the JSON report (`signals.attempts[].story`).\n"
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
        "\nDataset {} · prompt {} · opencode {} · {} attempt(s) per task · costs from the Nasiko router.\nResolved, match and by-difficulty rates count each task once, by its first attempt; further attempts are used only under \"Works every time\" and \"Per task\". Cost and cost per resolved cover every attempt: the cost of all attempts divided by the attempts that resolved, not by tasks. Failure reasons, everything under \"How it worked\", scope and test tampering count attempts. Ranges are 95% Wilson score intervals.\n",
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
        /// The tool calls of the session, in order; `None` leaves no session export.
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
            let export = json!({"info": {"id": "ses_1", "directory": "/testbed"},
                "messages": [{"info": {"role": "assistant"}, "parts": parts}]});
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

    /// A tool call as opencode's export writes it.
    fn call(tool: &str, input: serde_json::Value, output: &str, exit: i64) -> serde_json::Value {
        json!({"type": "tool", "callID": "call_1", "tool": tool,
               "state": {"status": "completed", "input": input, "output": output, "title": "",
                         "metadata": {"exit": exit}, "time": {"start": 1, "end": 2}}})
    }

    fn shell(command: &str) -> serde_json::Value {
        call("bash", json!({"command": command}), "", 0)
    }

    /// A build or test run that failed.
    fn failing(command: &str) -> serde_json::Value {
        call(
            "bash",
            json!({"command": command}),
            "test result: FAILED",
            101,
        )
    }

    fn read(path: &str) -> serde_json::Value {
        call(
            "read",
            json!({"filePath": format!("/testbed/{path}")}),
            "<type>file</type>",
            0,
        )
    }

    /// An edit of one line of `path`.
    fn edit_of(path: &str, old: &str, new: &str) -> serde_json::Value {
        let input = json!({"filePath": format!("/testbed/{path}"),
                           "oldString": old, "newString": new});
        call("edit", input, "Edit applied successfully.", 0)
    }

    /// The edit that makes the real fix.
    fn edit() -> serde_json::Value {
        edit_of("src/lib.rs", "fn b() -> u8 { 1 }", "fn b() -> u8 { 2 }")
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
            !section.contains("flint · attempt 2:"),
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
        let section = md.split("### Tested its own fix").nth(1).expect("section");
        assert_eq!(
            row(section, "| claude | all |"),
            format!(
                "| claude | all | 2 | {} | 0 | 0 | 0 |",
                with_range(&report::rate_of(2, 2))
            )
        );
        assert_eq!(
            row(section, "| claude | not resolved |"),
            "| claude | not resolved | 0 | — | 0 | 0 | 0 |"
        );
        // flint built before its only edit, never after.
        assert_eq!(
            row(section, "| flint | resolved |"),
            format!(
                "| flint | resolved | 1 | {} | 0 | 0 | 0 |",
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
        let section = md.split("### Tested its own fix").nth(1).expect("section");
        assert_eq!(
            row(section, "| flint | all |"),
            "| flint | all | 0 | — | 0 | 1 | 0 |"
        );
        assert!(
            md.contains("- 1 attempt(s) changed files although their session shows no call of an edit tool this report knows"),
            "{md}"
        );
        assert!(md.contains("Other tools their sessions called: str_replace"));
        assert!(md.contains("do not rely on \"How it worked\" for this run"));
        // What needs the order of edits is left out, not guessed, for an attempt that only
        // edited through the shell.
        let run_first = md
            .split("### Ran the build or tests before changing code")
            .nth(1)
            .expect("section");
        assert_eq!(
            row(run_first, "| flint | all |"),
            "| flint | all | 0 | — | 0 |"
        );
        let rework = md.split("### Guessed and redid").nth(1).expect("section");
        assert_eq!(
            row(rework, "| flint | all |"),
            "| flint | all | 0 | — | — | 0 | 0 |"
        );
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
            section.contains("- 10 attempt(s) had no graded patch on disk; their raw patch without its test files was used in its place."),
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
        let scope = md
            .split("### Scope against your fix")
            .nth(1)
            .expect("scope");
        assert!(row(scope, "| flint | all |").starts_with("| flint | all | 10 |"));
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
        let scope = md
            .split("### Scope against your fix")
            .nth(1)
            .expect("scope");
        assert!(row(scope, "| flint | all |").contains("| 0 | — | — |"));
        // Nothing was read of how any attempt worked, and no figure pretends otherwise.
        let places = md.split("Where it looked.").nth(1).expect("section");
        assert_eq!(
            row(places, "| flint | all |"),
            "| flint | all | 0 | 0 | — | 0 | 0 | 0 | 0 | 0 |"
        );
        assert!(md.contains("- 4 attempt(s) left no session export and are left out of everything under \"How it worked\"."));
        assert!(!md.contains("### Attempt by attempt"), "{md}");
    }

    /// One attempt per model: the control reads the file of the real fix, sees the tests fail,
    /// makes the fix and tests it; flint edits another file it has read and stops.
    fn a_run_with_two_ways_of_working(dir: &Path) -> Store {
        let store = store_with_run(dir, 1);
        let fix = the_real_fix();
        save_attempt(
            &store,
            &Attempt {
                model: "claude",
                attempt: 1,
                resolved: true,
                raw_patch: &fix,
                graded_patch: &fix,
                session: Some(json!([
                    read("src/lib.rs"),
                    failing("cargo test"),
                    edit(),
                    shell("cargo test"),
                ])),
            },
        );
        let (old, new) = ("pub fn c() {}\n", "pub fn c(deep: bool) {\n    b();\n}\n");
        let elsewhere = patch::diff_texts("src/other.rs", Some(old), new);
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 1,
                resolved: false,
                raw_patch: &elsewhere,
                graded_patch: &elsewhere,
                session: Some(json!([
                    read("src/other.rs"),
                    edit_of(
                        "src/other.rs",
                        "pub fn c() {}",
                        "pub fn c(deep: bool) {\n    1;\n    2;\n    3;\n    4;\n    5;\n}"
                    ),
                    edit_of(
                        "src/other.rs",
                        "    1;\n    2;\n    3;\n    4;\n    5;",
                        "    b();"
                    ),
                    edit_of("src/main.rs", "fn main() {}", "fn main() { c(true) }"),
                ])),
            },
        );
        store
    }

    #[test]
    fn the_sections_come_in_the_order_the_team_reads_them() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = a_two_attempt_run(dir.path());
        let md = render(&store, RUN, Format::Text).expect("renders");
        let headings = [
            "# Replay eval",
            "| Model | Resolved (95% range) |",
            "## How it worked",
            "### Looked at the bigger picture",
            "### Ran the build or tests before changing code",
            "### Guessed and redid",
            "### Tested its own fix before saying done",
            "### Read before changing",
            "## What it produced",
            "### Works every time",
            "### Scope against your fix",
            "## Per task",
            "### Failed attempts: what the session shows",
            "### Attempt by attempt",
            "## Why attempts failed",
            "## By difficulty",
            "## Test tampering",
            "## Not read",
            "\nDataset ds@1",
        ];
        let places: Vec<usize> = headings
            .iter()
            .map(|heading| {
                md.find(heading)
                    .unwrap_or_else(|| panic!("no {heading:?} in:\n{md}"))
            })
            .collect();
        assert!(places.is_sorted(), "{places:?}\n{md}");
    }

    #[test]
    fn how_it_worked_is_split_by_outcome_with_the_evidence_counted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let md = render(
            &a_run_with_two_ways_of_working(dir.path()),
            RUN,
            Format::Text,
        )
        .expect("renders");
        let (all, none) = (
            with_range(&report::rate_of(1, 1)),
            with_range(&report::rate_of(0, 1)),
        );
        let section = |title: &str| md.split(title).nth(1).expect("section");
        // The real fix changed src/lib.rs: the control changed it, flint never looked at it.
        let places = section("Where it looked.");
        assert_eq!(
            row(places, "| claude | resolved |"),
            format!("| claude | resolved | 1 | 1 | {all} | 1 | 0 | 0 | 0 | 0 |")
        );
        assert_eq!(
            row(places, "| flint | not resolved |"),
            format!("| flint | not resolved | 1 | 1 | {none} | 0 | 0 | 0 | 0 | 1 |")
        );
        assert_eq!(
            row(places, "| flint | resolved |"),
            "| flint | resolved | 0 | 0 | — | 0 | 0 | 0 | 0 | 0 |"
        );
        // flint changed the signature of `pub fn c` and never searched for `c`. Both patches
        // are Rust, so both were read for shared items.
        let users = section("Searched for what it changed.");
        assert_eq!(
            row(users, "| flint | all |"),
            format!("| flint | all | 1 | 1 | {none} | 1 | 0 |")
        );
        assert_eq!(
            row(users, "| claude | all |"),
            "| claude | all | 1 | 0 | — | 0 | 0 |"
        );
        let breadth = section("How widely, and when.");
        assert_eq!(
            row(breadth, "| claude | all |"),
            "| claude | all | 1 | 1.0 | 1.0 | 100% |"
        );
        assert_eq!(
            row(breadth, "| flint | all |"),
            "| flint | all | 1 | 1.0 | 2.0 | 100% |"
        );
        let run_first = section("### Ran the build or tests before changing code");
        assert_eq!(
            row(run_first, "| claude | all |"),
            format!("| claude | all | 1 | {all} | 1 |")
        );
        assert_eq!(
            row(run_first, "| flint | all |"),
            format!("| flint | all | 1 | {none} | 0 |")
        );
        // flint wrote 7 + 1 + 1 lines to keep 3; one edited file is not in its patch.
        let rework = section("### Guessed and redid");
        assert_eq!(
            row(rework, "| claude | all |"),
            format!("| claude | all | 1 | 1.0 | {none} | 0 | 0 |")
        );
        assert_eq!(
            row(rework, "| flint | all |"),
            format!("| flint | all | 1 | 3.0 | {all} | 0 | 1 |")
        );
        let read_first = section("### Read before changing");
        assert_eq!(
            row(read_first, "| claude | all |"),
            format!("| claude | all | 1 | 1 | {all} | 0 | 0 |")
        );
        assert_eq!(
            row(read_first, "| flint | all |"),
            format!("| flint | all | 1 | 1 | {none} | 1 | 0 |")
        );
        assert!(md.contains("signals about method, not verdicts"));
        assert!(md.contains("has not yet been checked against the export of a real run"));
        assert!(!md.contains("## Not read"), "{md}");
    }

    #[test]
    fn a_failed_attempt_gets_a_story_worded_as_a_signal() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = a_run_with_two_ways_of_working(dir.path());
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 2,
                resolved: false,
                raw_patch: "",
                graded_patch: "",
                session: None,
            },
        );
        let md = render(&store, RUN, Format::Text).expect("renders");
        let after = md
            .split("### Failed attempts: what the session shows")
            .nth(1)
            .expect("section");
        let (stories, sentences) = after
            .split_once("\n### Attempt by attempt")
            .expect("the sentences follow the stories");
        let sentences = sentences.split("\n## ").next().expect("the section's text");
        assert!(stories.contains(
            "A likely story to check against the session, not the reason the attempt failed."
        ));
        assert!(
            stories.contains(&format!(
                "- {TASK} · flint · attempt 1: Failed. Its session shows no look at src/lib.rs, which the real fix changed; no search for the name of 1 shared item(s) it changed (c); no build or test run before its first change to existing code.\n"
            )),
            "{md}"
        );
        assert!(
            stories.contains(&format!(
                "- {TASK} · flint · attempt 2: Failed; it left no session this report could read.\n"
            )),
            "{md}"
        );
        assert!(
            !stories.contains("claude"),
            "a resolved attempt has no story"
        );
        // Every attempt with a session read has its sentences, resolved or not.
        assert!(
            sentences.contains(&format!(
                "- {TASK} · claude · attempt 1 (resolved): The real fix changed 1 file(s); it changed 1 of them. Changed no shared item (no `pub` declaration changed in its Rust files). Opened 1 file(s) and edited 1; 100% of its looking came before its first edit. Ran the build or tests before changing existing code, and that run showed a failure. Wrote 1 line(s) to end with a 1-line change. Ran the build or tests after its last edit. Looked at each of the 1 existing file(s) it edited first.\n"
            )),
            "{md}"
        );
        assert!(
            sentences.contains(&format!(
                "- {TASK} · flint · attempt 1 (not resolved): The real fix changed 1 file(s); it changed 0 of them; its session shows no look at src/lib.rs. Changed 1 shared item(s) (c); its session shows no search for the name of any."
            )),
            "{md}"
        );
        assert!(!sentences.contains("attempt 2"), "no session, no sentence");
        let json = render(&store, RUN, Format::Json).expect("renders");
        let v: serde_json::Value = serde_json::from_str(&json).expect("json");
        let flint = &v["signals"]["attempts"][1];
        assert_eq!(
            flint["method"]["fix_files"]["never_looked"][0],
            "src/lib.rs"
        );
        assert_eq!(flint["method"]["shared_items"]["items"][0]["name"], "c");
        assert_eq!(flint["method"]["rework"]["lines_written"], 9);
        assert_eq!(
            flint["method"]["rework"]["abandoned_files"][0],
            "src/main.rs"
        );
        assert_eq!(
            flint["method"]["blind_edits"]["without_looking"][0],
            "src/main.rs"
        );
        assert_eq!(
            flint["method"]["first_run"]["ran_before_first_change"],
            false
        );
        assert!(
            flint["story"]
                .as_str()
                .expect("story")
                .starts_with("Failed. Its session shows no look at")
        );
        assert_eq!(
            flint["how_it_worked"][0],
            "The real fix changed 1 file(s); it changed 0 of them; its session shows no look at src/lib.rs."
        );
        assert_eq!(flint["how_it_worked"].as_array().map(Vec::len), Some(7));
        let claude = &v["signals"]["attempts"][0];
        assert_eq!(claude["story"], serde_json::Value::Null);
        assert_eq!(claude["method"]["first_run"]["outcome"], "showed_failure");
        assert_eq!(
            v["signals"]["attempts"][2]["method"],
            serde_json::Value::Null
        );
        let models = &v["signals"]["models"];
        assert_eq!(
            models[0]["method"]["resolved"]["looked_at_every_fix_file"]["value"],
            1.0
        );
        assert_eq!(
            models[1]["method"]["not_resolved"]["fix_files_never_looked"],
            1
        );
        assert_eq!(
            models[1]["self_check_by_outcome"]["not_resolved"]["never_checked"],
            1
        );
    }

    #[test]
    fn a_session_in_a_shape_the_reader_does_not_know_is_named_not_guessed_at() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_with_run(dir.path(), 1);
        let fix = the_real_fix();
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 1,
                resolved: false,
                raw_patch: &fix,
                graded_patch: &fix,
                session: Some(json!([
                    {"type": "tool", "tool": "bash", "state": {"status": "completed"}},
                    {"type": "tool", "tool": "edit", "state": {"status": "completed"}},
                ])),
            },
        );
        let md = render(&store, RUN, Format::Text).expect("renders");
        assert!(
            md.contains(&format!(
                "- {TASK} · flint · attempt 1: session export: none of the session's 1 `bash` call(s) carries the input this report reads"
            )),
            "{md}"
        );
        let places = md.split("Where it looked.").nth(1).expect("section");
        assert_eq!(
            row(places, "| flint | all |"),
            "| flint | all | 0 | 0 | — | 0 | 0 | 0 | 0 | 0 |"
        );
        let tested = md.split("### Tested its own fix").nth(1).expect("section");
        assert_eq!(
            row(tested, "| flint | all |"),
            "| flint | all | 0 | — | 0 | 0 | 0 |"
        );
        assert!(md.contains("attempt 1: Failed; it left no session this report could read."));
    }

    #[test]
    fn a_session_with_no_readable_step_for_a_changed_tree_is_not_read_as_doing_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_with_run(dir.path(), 1);
        let fix = the_real_fix();
        // Tool calls under a part type this reader does not know.
        let other_format = json!([{"type": "tool_use", "name": "bash", "input": {"cmd": "ls"}}]);
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 1,
                resolved: false,
                raw_patch: &fix,
                graded_patch: &fix,
                session: Some(other_format),
            },
        );
        let md = render(&store, RUN, Format::Text).expect("renders");
        assert!(
            md.contains(&format!(
                "- {TASK} · flint · attempt 1: session export: it shows no tool call this report reads, yet the attempt changed files"
            )),
            "{md}"
        );
        let breadth = md.split("How widely, and when.").nth(1).expect("section");
        assert_eq!(
            row(breadth, "| flint | all |"),
            "| flint | all | 0 | — | — | — |"
        );
        assert!(!md.contains("Its session shows"), "{md}");
        assert!(md.contains("attempt 1: Failed; it left no session this report could read."));
    }

    #[test]
    fn a_call_cut_off_before_its_input_was_written_is_said_and_the_rest_is_read() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_with_run(dir.path(), 1);
        let fix = the_real_fix();
        let cut_off = json!({"type": "tool", "tool": "bash",
            "state": {"status": "running", "input": {}, "time": {"start": 1}}});
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 1,
                resolved: true,
                raw_patch: &fix,
                graded_patch: &fix,
                session: Some(json!([
                    read("src/lib.rs"),
                    edit(),
                    shell("cargo test"),
                    cut_off
                ])),
            },
        );
        let md = render(&store, RUN, Format::Text).expect("renders");
        assert!(
            md.contains("- 1 attempt(s) have tool calls in their session that lack the input this report reads (a call cut off while it ran, or a search in a form this report does not know); those calls are missing from the figures under \"How it worked\"."),
            "{md}"
        );
        let tested = md.split("### Tested its own fix").nth(1).expect("section");
        assert_eq!(
            row(tested, "| flint | all |"),
            format!(
                "| flint | all | 1 | {} | 0 | 0 | 0 |",
                with_range(&report::rate_of(1, 1))
            )
        );
    }

    #[test]
    fn what_cannot_be_told_and_a_change_thrown_away_have_their_own_columns() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = store_with_run(dir.path(), 1);
        // flint may have seen the file of the real fix through a glob, then wrote a change to
        // another file and took it back: it handed in nothing.
        let (before, after) = ("pub fn c() {}", "pub fn c() {\n    1;\n}");
        save_attempt(
            &store,
            &Attempt {
                model: "flint",
                attempt: 1,
                resolved: false,
                raw_patch: "",
                graded_patch: "",
                session: Some(json!([
                    shell("cat src/*.rs"),
                    edit_of("src/other.rs", before, after),
                    edit_of("src/other.rs", after, before),
                ])),
            },
        );
        let md = render(&store, RUN, Format::Text).expect("renders");
        let section = |title: &str| md.split(title).nth(1).expect("section");
        // Compared with the fix, but not counted as looking or as not looking.
        assert_eq!(
            row(section("Where it looked."), "| flint | all |"),
            "| flint | all | 1 | 0 | — | 0 | 0 | 0 | 1 | 0 |"
        );
        // An empty patch has no Rust code to read declarations from.
        assert_eq!(
            row(section("Searched for what it changed."), "| flint | all |"),
            "| flint | all | 0 | 0 | — | 0 | 0 |"
        );
        assert_eq!(
            row(section("### Guessed and redid"), "| flint | all |"),
            format!(
                "| flint | all | 1 | — | {} | 1 | 1 |",
                with_range(&report::rate_of(1, 1))
            )
        );
        assert_eq!(
            row(section("### Read before changing"), "| flint | all |"),
            "| flint | all | 1 | 0 | — | 0 | 1 |"
        );
        assert!(
            md.contains(&format!(
                "- {TASK} · flint · attempt 1: Failed. Its session shows no build or test run before its first change to existing code; 4 line(s) written and none kept; no build or test run after its last edit.\n"
            )),
            "{md}"
        );
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
