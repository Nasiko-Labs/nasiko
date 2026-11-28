//! `nasiko eval spec detect` and `nasiko eval build`: Docker images per task, and the sanity
//! check that fills FAIL_TO_PASS / PASS_TO_PASS (`nasiko_evals::gate`).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use nasiko_evals::gate::{self, GateRejection, GateRuns, GateVerdict, RunOutcome};
use nasiko_evals::spec::{self, EXIT_BUILD, EXIT_MODEL_PATCH, EXIT_TEST_PATCH, EnvSpec};
use nasiko_evals::testlog::LogParserKind;
use nasiko_evals::{TaskInstance, TaskStatus, diff, testlog};

use super::containers::{
    Containers, ImageBuild, JobLimits, JobOutcome, JobSpec, Mount, Network, StopReason,
};
use super::git::Repo;
use super::images;
use super::store::Store;

/// `nasiko eval spec detect`: proposes and saves the build/test spec of a repo.
pub fn spec_detect(store: &Store, repo_dir: &Path) -> Result<EnvSpec> {
    let repo = Repo::open(repo_dir)?;
    let slug = repo.slug();
    let root = repo.root_files("HEAD")?;
    let toolchain = ["rust-toolchain.toml", "rust-toolchain"]
        .iter()
        .find_map(|f| repo.file_at("HEAD", f).ok().flatten());
    let proposed = spec::propose(&repo.full_name(), &root, toolchain.as_deref())
        .context("only Cargo (Rust) repositories are supported so far")?;
    // A saved spec that cannot be read is the user's to mend: it is not replaced unasked.
    let saved = store.load_spec(&slug).with_context(|| {
        format!(
            "the spec saved for {slug} could not be read; mend its file under specs/, or \
             delete it to start again from the proposal"
        )
    })?;
    let (mut spec, notes) = settle_spec(saved, proposed);
    for note in notes {
        eprintln!("note: {note}");
    }
    spec.local_path = Some(repo.root().to_string_lossy().into_owned());
    store.save_spec(&slug, &spec)?;
    Ok(spec)
}

/// How to drop a saved spec for the proposal; the end of several notes.
const START_AGAIN: &str = "To start again from the proposal, delete the spec's file under specs/ and run this \
     command again.";

/// The spec `spec detect` saves, and what to tell the user about it. A spec already saved is
/// kept, not replaced by the proposal: its settings may be the user's own (a pinned toolchain,
/// more memory), and tasks already gated were built with them. So a better proposal, or a
/// `rust-toolchain` file changed since, reaches a saved spec only as a note.
fn settle_spec(saved: Option<EnvSpec>, proposed: EnvSpec) -> (EnvSpec, Vec<String>) {
    let mut notes = Vec::new();
    let spec = match saved {
        Some(mut saved) => {
            let differing = spec::differing_settings(&saved, &proposed);
            if !differing.is_empty() {
                notes.push(format!(
                    "kept the saved spec, which differs from what would be proposed today in: \
                     {}. {START_AGAIN}",
                    differing.join(", ")
                ));
            }
            // Specs are filed by repository name alone, so two repositories can share a file.
            if saved.repo != proposed.repo {
                notes.push(format!(
                    "the saved spec was made for {}, and this repository is {}: specs are \
                     filed by repository name alone. Its settings are kept and it now names \
                     this repository; check that they suit it. {START_AGAIN}",
                    saved.repo, proposed.repo
                ));
                saved.repo = proposed.repo;
            }
            saved
        }
        None => proposed,
    };
    if spec::commands_set_rustflags(&spec) {
        notes.push(rustflags_note());
    }
    (spec, notes)
}

/// What to tell the owner of a spec whose commands mention compiler flags.
fn rustflags_note() -> String {
    let (name, value) = spec::LINT_CAP_ENV;
    format!(
        "a command of the spec (build_cmd, test_cmd or setup) mentions RUSTFLAGS. If it sets \
         them, that replaces the lint cap the images set ({name}={value}, the cap_lints \
         setting) for that command, and the agent's own cargo runs compile with other flags, \
         so each side compiles from scratch. If it is there only to cap lints, remove it, \
         and then build again every task that was gated with it (`nasiko eval local build \
         --only <ids>`) and freeze again any dataset made from them before the next `run`. \
         A task keeps the image and the grading script it was gated with, while the agent is \
         shown the spec's commands as they are now: without the rebuild the agent would be \
         told to run a command that fails in the old image."
    )
}

/// Inputs of `nasiko eval build`.
pub struct BuildArgs {
    /// Only these task ids; empty = every candidate task.
    pub only: Vec<String>,
    /// How many times `before` and `after` are run, to catch flaky tests.
    pub runs: u32,
}

/// What `build` did per task.
pub struct BuildReport {
    pub instance_id: String,
    pub verdict: std::result::Result<GateVerdict, String>,
}

/// Limits for one gate test run.
pub fn test_limits(spec: &EnvSpec, disk_mib: u32) -> JobLimits {
    JobLimits {
        wall: Duration::from_secs(spec.timeout_secs),
        cpus: 4.0,
        memory_mib: spec.memory_mib,
        pids: 4096,
        disk_mib,
    }
}

pub fn build(
    store: &Store,
    containers: &dyn Containers,
    args: &BuildArgs,
) -> Result<Vec<BuildReport>> {
    let disk_mib = match store.load_config()? {
        Some(c) => c.grade_storage_mib,
        None => serde_json::from_str::<super::store::EvalConfig>("{}")?.grade_storage_mib,
    };
    let tasks: Vec<TaskInstance> = store
        .tasks()?
        .into_iter()
        .filter(|t| {
            if args.only.is_empty() {
                t.x_nasiko.status == TaskStatus::Candidate
            } else {
                args.only.contains(&t.instance_id)
            }
        })
        .collect();
    let mut reports = Vec::new();
    for mut task in tasks {
        let id = task.instance_id.clone();
        eprintln!("building {id}");
        let gated = build_task(store, containers, &mut task, args.runs.max(1), disk_mib);
        let verdict = match gated {
            Ok(Gated {
                verdict,
                failed_builds,
            }) => {
                if let GateVerdict::Rejected { reason } = &verdict {
                    eprintln!("{}", rejection_note(&id, *reason, &failed_builds));
                }
                apply_verdict(&mut task, &verdict);
                store.save_task(&task)?;
                Ok(verdict)
            }
            Err(e) => Err(format!("{e:#}")),
        };
        reports.push(BuildReport {
            instance_id: id,
            verdict,
        });
    }
    Ok(reports)
}

fn repo_slug(full_name: &str) -> &str {
    full_name.rsplit('/').next().unwrap_or(full_name)
}

/// The gate's verdict on a task and, when builds that failed decided it, those builds.
struct Gated {
    verdict: GateVerdict,
    failed_builds: Vec<FailedBuild>,
}

/// A gate run that did not build, and the one line that says why.
#[derive(Debug, Clone, PartialEq)]
struct FailedBuild {
    /// The run's folder name under the task's `gate/`, e.g. `pristine-1`.
    run: String,
    /// The first compiler error of the run's log, or that the run was stopped for a limit.
    cause: String,
    /// The limit that stopped the run, when one did: then nothing is known about the build.
    stopped: Option<StopReason>,
    log: PathBuf,
}

/// What `build` prints for a rejected task: the reason in words, and for each failed build
/// behind it the first error and where the whole log is. When the first of them was stopped
/// at a limit, the words say that instead of stating a compile failure nobody saw.
fn rejection_note(id: &str, reason: GateRejection, failed: &[FailedBuild]) -> String {
    let stopped = failed
        .first()
        .and_then(|f| Some((f.stopped?, f.run.as_str())));
    let words = match stopped {
        Some((limit, run)) => unfinished_words(limit, run),
        None => reason.explain().to_string(),
    };
    let mut note = format!("  {id}: rejected ({reason:?}): {words}");
    for failed in failed {
        note.push_str(&format!(
            "\n    {}: {}\n    log: {}",
            failed.run,
            failed.cause,
            failed.log.display()
        ));
    }
    note
}

/// The rejection in words when the run behind it never finished.
fn unfinished_words(limit: StopReason, run: &str) -> String {
    let (limit, remedy) = match limit {
        StopReason::TimeLimit => ("time", "raise timeout_secs in the spec"),
        StopReason::DiskLimit => ("disk", "raise grade_storage_mib in config.json"),
    };
    format!(
        "the {run} run was stopped at the {limit} limit before it finished, so the gate does \
         not know whether that code builds, and the reason's name says more than was seen. \
         To find out, {remedy} and build the task again"
    )
}

/// The failed builds behind a rejection. For a base that does not build: the untouched old
/// code's, and the fix's too when it failed for another cause (the fix may have its own
/// trouble, which the base's error would hide). For a fix that does not build: its first.
fn failed_builds_behind(
    verdict: &GateVerdict,
    pristine: &GateRun,
    after: &[GateRun],
) -> Vec<FailedBuild> {
    let GateVerdict::Rejected { reason } = verdict else {
        return Vec::new();
    };
    let of_fix = after.iter().find_map(|r| r.failed_build.clone());
    match reason {
        GateRejection::BaseDoesNotBuild => {
            let of_base = pristine.failed_build.clone();
            let same_cause =
                of_base.as_ref().map(|f| &f.cause) == of_fix.as_ref().map(|f| &f.cause);
            let of_fix = of_fix.filter(|_| !same_cause);
            of_base.into_iter().chain(of_fix).collect()
        }
        GateRejection::GoldDoesNotBuild => of_fix.into_iter().collect(),
        GateRejection::NoFailToPass | GateRejection::AllFlaky => Vec::new(),
    }
}

fn build_task(
    store: &Store,
    containers: &dyn Containers,
    task: &mut TaskInstance,
    runs: u32,
    disk_mib: u32,
) -> Result<Gated> {
    let slug = repo_slug(&task.repo);
    let spec = store.load_spec(slug)?.with_context(|| {
        format!("no spec for {slug}; run `nasiko eval spec detect --repo <path>`")
    })?;
    let repo_path = spec
        .local_path
        .as_deref()
        .context("the spec has no local_path; re-run `nasiko eval spec detect`")?;
    let repo = Repo::open(Path::new(repo_path))?;
    let scope = test_scope(&repo, task)?;
    let env_tag = images::env_image_tag(&spec);
    if !containers.image_exists(&env_tag)? {
        let dir = scratch_dir(store, &task.instance_id, "env-image")?;
        fs::write(dir.join("Dockerfile"), images::env_dockerfile(&spec))?;
        containers.build(&ImageBuild {
            tag: env_tag.clone(),
            context: dir,
        })?;
    }
    let task_tag = images::task_image_tag(&spec, &task.base_commit, &scope);
    if !containers.image_exists(&task_tag)? {
        let dir = scratch_dir(store, &task.instance_id, "task-image")?;
        repo.archive(&task.base_commit, &dir.join("source.tar"))?;
        fs::write(
            dir.join("Dockerfile"),
            images::task_dockerfile(&env_tag, &spec, &scope),
        )?;
        containers.build(&ImageBuild {
            tag: task_tag.clone(),
            context: dir,
        })?;
    }
    task.image = task_tag.clone();
    task.eval_script = spec::eval_script(&spec, &scope);
    task.log_parser = spec.log_parser.swebench_name().to_string();
    task.eval_type = "pass_and_fail".to_string();
    task.x_nasiko.test_scope = scope;
    let ctx = GateCtx {
        store,
        containers,
        task,
        spec: &spec,
        limits: test_limits(&spec, disk_mib),
    };
    ctx.gate(runs)
}

/// The task's test scope, from the Cargo packages owning the files the fix and its tests change
/// ([`spec::task_test_scope`]).
fn test_scope(repo: &Repo, task: &TaskInstance) -> Result<Vec<String>> {
    let mut files = Vec::new();
    for (text, in_tests) in [(&task.patch, false), (&task.test_patch, true)] {
        for file in diff::parse_unified(text)? {
            if let Some((package, manifest)) =
                repo.cargo_package_of(&task.x_nasiko.gold_commit, file.path())?
            {
                files.push(spec::OwnedFile {
                    path: file.path().to_string(),
                    package,
                    manifest,
                    in_tests,
                });
            }
        }
    }
    spec::task_test_scope(&files)
        .context("no Cargo package owns the task's files, so it has no tests to run")
}

/// The three gate set-ups.
#[derive(Clone, Copy)]
enum Setup {
    /// Old code, old tests.
    Pristine,
    /// Old code + the new tests.
    Before,
    /// The real fix + the new tests.
    After,
}

impl Setup {
    fn name(self) -> &'static str {
        match self {
            Setup::Pristine => "pristine",
            Setup::Before => "before",
            Setup::After => "after",
        }
    }
}

/// What every gate run of one task shares.
struct GateCtx<'a> {
    store: &'a Store,
    containers: &'a dyn Containers,
    task: &'a TaskInstance,
    spec: &'a EnvSpec,
    limits: JobLimits,
}

/// One gate run: what the judge reads, and its failed build when it had one.
struct GateRun {
    outcome: RunOutcome,
    failed_build: Option<FailedBuild>,
}

/// Why a run did not build, in one line: the limit that stopped it, or the log's first error
/// (with the signal, for a compiler that was killed).
fn build_failure_cause(stopped: Option<StopReason>, log: &str) -> String {
    match stopped {
        Some(StopReason::TimeLimit) => "stopped at the time limit before it finished".into(),
        Some(StopReason::DiskLimit) => "stopped at the disk limit before it finished".into(),
        None => gate::build_error_summary(log)
            .map(|line| line.chars().take(MAX_CAUSE_CHARS).collect())
            .unwrap_or_else(|| "the build failed without an error line in its log".into()),
    }
}

/// Longest compiler error line kept as a cause; a longer one is cut.
const MAX_CAUSE_CHARS: usize = 240;

/// A finished gate job, as [`read_run`] needs it.
struct RanJob<'a> {
    setup: Setup,
    /// The run's folder name, e.g. `after-2`.
    name: String,
    outcome: &'a JobOutcome,
    /// Its stdout and stderr as saved in `log_path`.
    log: &'a str,
    log_path: PathBuf,
}

/// Reads one finished gate job: whether it built, and its test results.
fn read_run(job: RanJob<'_>, parser: LogParserKind) -> Result<GateRun> {
    let RanJob {
        setup,
        name,
        outcome,
        log,
        log_path,
    } = job;
    let not_built = |failed_build| GateRun {
        outcome: RunOutcome {
            built: false,
            results: Default::default(),
        },
        failed_build,
    };
    // New tests that cannot even be applied to the old code cannot exist before the fix:
    // count it like a build failure, and the pristine run decides what passed before.
    if matches!(setup, Setup::Before) && outcome.exit_code == Some(EXIT_TEST_PATCH) {
        return Ok(not_built(None));
    }
    if matches!(outcome.exit_code, Some(EXIT_MODEL_PATCH | EXIT_TEST_PATCH)) {
        bail!(
            "the {} patch did not apply in the task image (see {})",
            setup.name(),
            log_path.display()
        );
    }
    let build_failed = outcome.exit_code == Some(EXIT_BUILD);
    // The scope comes from the commit of the fix. When the old code has no such test target
    // or package, cargo stops before compiling: the task's tests did not exist yet, which is
    // "no test passed before", not "the old code does not build".
    let tests_not_there_yet = matches!(setup, Setup::Pristine)
        && outcome.stopped.is_none()
        && build_failed
        && gate::scope_absent_on_base(log);
    if tests_not_there_yet {
        return Ok(GateRun {
            outcome: RunOutcome {
                built: true,
                results: Default::default(),
            },
            failed_build: None,
        });
    }
    if outcome.stopped.is_some() || build_failed {
        return Ok(not_built(Some(FailedBuild {
            run: name,
            cause: build_failure_cause(outcome.stopped, log),
            stopped: outcome.stopped,
            log: log_path,
        })));
    }
    Ok(GateRun {
        outcome: RunOutcome {
            built: true,
            results: testlog::parse(parser, log),
        },
        failed_build: None,
    })
}

impl GateCtx<'_> {
    /// Runs the three set-ups (`before` and `after` `runs` times each) and judges them.
    fn gate(&self, runs: u32) -> Result<Gated> {
        let pristine = self.run(Setup::Pristine, 1)?;
        let mut before = Vec::new();
        let mut after = Vec::new();
        for n in 1..=runs {
            before.push(self.run(Setup::Before, n)?);
            after.push(self.run(Setup::After, n)?);
        }
        let outcomes = |runs: &[GateRun]| runs.iter().map(|r| r.outcome.clone()).collect();
        let verdict = gate::judge(&GateRuns {
            pristine: pristine.outcome.clone(),
            before: outcomes(&before),
            after: outcomes(&after),
        });
        let failed_builds = failed_builds_behind(&verdict, &pristine, &after);
        Ok(Gated {
            verdict,
            failed_builds,
        })
    }

    fn run(&self, setup: Setup, n: u32) -> Result<GateRun> {
        let GateCtx {
            store,
            containers,
            task,
            spec,
            limits,
        } = *self;
        let dir = store
            .layout()
            .task_dir(&task.instance_id)
            .join("gate")
            .join(format!("{}-{n}", setup.name()));
        fs::create_dir_all(&dir)?;
        let (model, tests) = match setup {
            Setup::Pristine => ("", ""),
            Setup::Before => ("", task.test_patch.as_str()),
            Setup::After => (task.patch.as_str(), task.test_patch.as_str()),
        };
        fs::write(dir.join("model.patch"), model)?;
        fs::write(dir.join("test.patch"), tests)?;
        fs::write(dir.join("run.sh"), &task.eval_script)?;
        let outcome = containers.run_to_end(&JobSpec {
            name: format!(
                "nasiko-eval-gate-{}-{}-{n}",
                sanitize(&task.instance_id),
                setup.name()
            ),
            image: task.image.clone(),
            cmd: vec!["bash".into(), "/eval/run.sh".into()],
            network: Network::None,
            mounts: vec![Mount {
                host: dir.canonicalize()?,
                container: "/eval".into(),
                read_only: true,
            }],
            env: Vec::new(),
            limits,
            label: format!("gate-{}", task.instance_id),
            before_stop: None,
        })?;
        let log = format!("{}\n--- stderr ---\n{}", outcome.stdout, outcome.stderr);
        let log_path = dir.join("output.log");
        fs::write(&log_path, &log)?;
        let ran = RanJob {
            setup,
            name: format!("{}-{n}", setup.name()),
            outcome: &outcome,
            log: &log,
            log_path,
        };
        read_run(ran, spec.log_parser)
    }
}

/// Saves the verdict on the task. The saved reason is the reason's name alone, the same
/// string the server saves, so tasks can be counted by it; the failed build's first error is
/// printed by `build` and stays in the run's log.
fn apply_verdict(task: &mut TaskInstance, verdict: &GateVerdict) {
    match verdict {
        GateVerdict::Reproducible(lists) => {
            task.fail_to_pass = lists.fail_to_pass.clone();
            task.pass_to_pass = lists.pass_to_pass.clone();
            task.x_nasiko.status = TaskStatus::Reproducible;
            task.x_nasiko.reject_reason = None;
        }
        GateVerdict::Rejected { reason } => {
            task.x_nasiko.status = TaskStatus::Rejected;
            task.x_nasiko.reject_reason = Some(format!("gate: {reason:?}"));
        }
    }
}

/// A fresh build-context folder under the task's folder.
fn scratch_dir(store: &Store, id: &str, name: &str) -> Result<PathBuf> {
    let dir = store.layout().task_dir(id).join("build").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Container names allow `[a-zA-Z0-9_.-]`.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_.-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::eval::containers::tests::FakeContainers;
    use nasiko_evals::EvalLayout;

    fn outcome(code: i32, out: &str) -> JobOutcome {
        JobOutcome {
            exit_code: Some(code),
            stopped: None,
            stdout: out.into(),
            stderr: String::new(),
            wall: Duration::from_secs(1),
        }
    }

    #[test]
    fn gate_runs_pristine_before_and_after_and_reads_results() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::new(EvalLayout::new(dir.path()));
        let fake = FakeContainers::default();
        let running = "     Running unittests src/lib.rs (target/debug/deps/c-0a1b)\n";
        *fake.outcomes.lock().expect("lock") = vec![
            outcome(0, &format!("{running}test old ... ok\n")),
            outcome(EXIT_BUILD, "error: could not compile"),
            outcome(0, &format!("{running}test old ... ok\ntest new ... ok\n")),
        ];
        let spec = spec::propose("o/r", &["Cargo.toml".into()], None).expect("rust");
        let task: TaskInstance =
            serde_json::from_str(include_str!("testdata/task.json")).expect("task");
        let limits = test_limits(&spec, 2048);
        let ctx = GateCtx {
            store: &store,
            containers: &fake,
            task: &task,
            spec: &spec,
            limits,
        };
        let run = |s, n| ctx.run(s, n).expect("runs").outcome;
        let runs = GateRuns {
            pristine: run(Setup::Pristine, 1),
            before: vec![run(Setup::Before, 1)],
            after: vec![run(Setup::After, 1)],
        };
        let GateVerdict::Reproducible(lists) = gate::judge(&runs) else {
            panic!("reproducible");
        };
        assert_eq!(lists.fail_to_pass, vec!["c::new"]);
        assert_eq!(lists.pass_to_pass, vec!["c::old"]);
        let jobs = fake.jobs.lock().expect("lock");
        assert!(
            jobs.iter()
                .all(|j| j.network == Network::None && j.limits.disk_mib == 2048)
        );
    }

    /// The hyperium/http case: under `deny(warnings)` a newer compiler's warning stops every
    /// build, the untouched one too. The base is named, with the compiler's own first line.
    #[test]
    fn a_base_that_does_not_build_is_reported_as_the_base_with_its_first_error() {
        let lint = "error: use of deprecated constant `std::u16::MAX`: replaced by the `MAX` associated constant on this type";
        let log = format!(
            "   Compiling http v0.2.0 (/testbed)\n{lint}\n  --> src/header/map.rs:9:5\n\
             error: could not compile `http` (lib) due to 8 previous errors\n"
        );
        let (task, gated) = gate_with(vec![outcome(EXIT_BUILD, &log); 5], 2);
        let reason = GateRejection::BaseDoesNotBuild;
        assert_eq!(gated.verdict, GateVerdict::Rejected { reason });

        // Every run failed on the same line: it is shown once, as the base's.
        let [failed] = gated.failed_builds.as_slice() else {
            panic!("one failed build, got {:?}", gated.failed_builds);
        };
        assert_eq!(failed.run, "pristine-1");
        assert_eq!(failed.cause, lint);
        assert!(failed.log.ends_with("gate/pristine-1/output.log"));
        let note = rejection_note(&task.instance_id, reason, &gated.failed_builds);
        assert!(note.contains("rejected (BaseDoesNotBuild)"), "{note}");
        assert!(note.contains("the fix was not judged"), "{note}");
        assert!(note.contains(&format!("pristine-1: {lint}")), "{note}");

        // The saved reason is the name alone, as the server saves it.
        let mut task = task;
        apply_verdict(&mut task, &gated.verdict);
        assert_eq!(task.x_nasiko.status, TaskStatus::Rejected);
        assert_eq!(
            task.x_nasiko.reject_reason.as_deref(),
            Some("gate: BaseDoesNotBuild")
        );
    }

    /// Gates the test task against canned container outcomes, in the order the gate runs
    /// them: pristine, then before and after for each of `runs`.
    fn gate_with(outcomes: Vec<JobOutcome>, runs: u32) -> (TaskInstance, Gated) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::new(EvalLayout::new(dir.path()));
        let fake = FakeContainers::default();
        *fake.outcomes.lock().expect("lock") = outcomes;
        let spec = spec::propose("o/r", &["Cargo.toml".into()], None).expect("rust");
        let task: TaskInstance =
            serde_json::from_str(include_str!("testdata/task.json")).expect("task");
        let ctx = GateCtx {
            store: &store,
            containers: &fake,
            task: &task,
            spec: &spec,
            limits: test_limits(&spec, 2048),
        };
        let gated = ctx.gate(runs).expect("the gate runs");
        (task, gated)
    }

    const RUNNING: &str = "     Running tests/header_map.rs (target/debug/deps/header_map-0a1b)\n";
    const NO_TARGET: &str = "error: no test target named `header_map` in `c` package\n";

    /// The scope names a test target that the test patch adds, so cargo refuses the pristine
    /// run before compiling. That is not a base that does not build: a fix that fails to
    /// build is still the fix's failure, shown with the fix's own error.
    #[test]
    fn a_test_target_the_old_code_lacks_does_not_take_the_blame_off_the_fix() {
        let missing_dep = "error: failed to get `newdep` as a dependency of package `c v0.1.0`";
        let (task, gated) = gate_with(
            vec![
                outcome(EXIT_BUILD, NO_TARGET),
                outcome(EXIT_BUILD, "error[E0425]: cannot find function `b2`\n"),
                outcome(EXIT_BUILD, &format!("{missing_dep}\n")),
            ],
            1,
        );
        let reason = GateRejection::GoldDoesNotBuild;
        assert_eq!(gated.verdict, GateVerdict::Rejected { reason });
        let note = rejection_note(&task.instance_id, reason, &gated.failed_builds);
        assert!(note.contains(&format!("after-1: {missing_dep}")), "{note}");
        assert!(!note.contains("pristine-1"), "{note}");
    }

    /// The same pristine failure with a fix that builds: the task is valid, and its tests,
    /// which did not exist before, are the ones the fix makes pass.
    #[test]
    fn a_test_target_the_old_code_lacks_counts_as_tests_that_did_not_pass_before() {
        let (_, gated) = gate_with(
            vec![
                outcome(EXIT_BUILD, NO_TARGET),
                outcome(EXIT_BUILD, "error[E0425]: cannot find function `b2`\n"),
                outcome(0, &format!("{RUNNING}test new ... ok\n")),
            ],
            1,
        );
        let GateVerdict::Reproducible(lists) = gated.verdict else {
            panic!("reproducible, got {:?}", gated.verdict);
        };
        assert_eq!(lists.fail_to_pass.len(), 1, "{lists:?}");
        assert!(lists.fail_to_pass[0].ends_with("new"), "{lists:?}");
        assert!(lists.pass_to_pass.is_empty(), "{lists:?}");
        assert!(gated.failed_builds.is_empty());
    }

    /// Only cargo's refusal of the scope is read that way, and only on the untouched code.
    #[test]
    fn a_missing_target_is_excused_only_for_the_pristine_run() {
        let dir = PathBuf::from("gate/x/output.log");
        let job = outcome(EXIT_BUILD, NO_TARGET);
        let read = |setup| {
            let ran = RanJob {
                setup,
                name: "x".into(),
                outcome: &job,
                log: NO_TARGET,
                log_path: dir.clone(),
            };
            read_run(ran, LogParserKind::Cargo).expect("reads")
        };
        assert!(read(Setup::Pristine).outcome.built);
        assert!(read(Setup::Pristine).failed_build.is_none());
        let after = read(Setup::After);
        assert!(!after.outcome.built);
        assert_eq!(
            after.failed_build.expect("a failed build").cause,
            NO_TARGET.trim()
        );
    }

    /// When the base and the fix fail for different causes, both are shown: the base's error
    /// must not hide trouble of the fix's own.
    #[test]
    fn a_fix_failing_for_another_cause_than_the_base_is_shown_too() {
        let (task, gated) = gate_with(
            vec![
                outcome(EXIT_BUILD, "error: use of deprecated constant `X`\n"),
                outcome(EXIT_BUILD, "error: use of deprecated constant `X`\n"),
                outcome(EXIT_BUILD, "error[E0308]: mismatched types\n"),
            ],
            1,
        );
        let reason = GateRejection::BaseDoesNotBuild;
        assert_eq!(gated.verdict, GateVerdict::Rejected { reason });
        let note = rejection_note(&task.instance_id, reason, &gated.failed_builds);
        assert!(
            note.contains("pristine-1: error: use of deprecated"),
            "{note}"
        );
        assert!(note.contains("after-1: error[E0308]"), "{note}");
    }

    fn stopped_outcome(limit: StopReason, out: &str) -> JobOutcome {
        JobOutcome {
            exit_code: None,
            stopped: Some(limit),
            ..outcome(0, out)
        }
    }

    /// A run stopped at a limit is not known to fail to build, so the note must not say it
    /// does; it says what was stopped and which setting to raise.
    #[test]
    fn a_run_stopped_at_a_limit_is_not_called_a_build_failure() {
        let slow = stopped_outcome(StopReason::TimeLimit, "test old ... ok\n");
        let (task, gated) = gate_with(
            vec![
                slow.clone(),
                outcome(EXIT_BUILD, "error[E0425]: cannot find function `b2`\n"),
                outcome(0, &format!("{RUNNING}test new ... ok\n")),
            ],
            1,
        );
        let reason = GateRejection::BaseDoesNotBuild;
        assert_eq!(gated.verdict, GateVerdict::Rejected { reason });
        let note = rejection_note(&task.instance_id, reason, &gated.failed_builds);
        assert!(
            note.contains("pristine-1 run was stopped at the time limit"),
            "{note}"
        );
        assert!(note.contains("timeout_secs"), "{note}");
        assert!(!note.contains("fails on the old code"), "{note}");

        let (task, gated) = gate_with(
            vec![
                outcome(0, &format!("{RUNNING}test old ... ok\n")),
                outcome(EXIT_BUILD, "error[E0425]: cannot find function `b2`\n"),
                stopped_outcome(StopReason::DiskLimit, ""),
            ],
            1,
        );
        let reason = GateRejection::GoldDoesNotBuild;
        assert_eq!(gated.verdict, GateVerdict::Rejected { reason });
        let note = rejection_note(&task.instance_id, reason, &gated.failed_builds);
        assert!(
            note.contains("after-1 run was stopped at the disk limit"),
            "{note}"
        );
        assert!(note.contains("grade_storage_mib"), "{note}");
        assert!(!note.contains("fails on the real fix"), "{note}");
    }

    #[test]
    fn a_fix_that_does_not_build_on_a_sound_base_shows_the_fix_error() {
        let (task, gated) = gate_with(
            vec![
                outcome(0, &format!("{RUNNING}test old ... ok\n")),
                outcome(EXIT_BUILD, "error[E0425]: cannot find function `b2`\n"),
                outcome(EXIT_BUILD, "error[E0308]: mismatched types\n"),
            ],
            1,
        );
        let reason = GateRejection::GoldDoesNotBuild;
        assert_eq!(gated.verdict, GateVerdict::Rejected { reason });
        let [failed] = gated.failed_builds.as_slice() else {
            panic!("one failed build, got {:?}", gated.failed_builds);
        };
        assert_eq!(failed.run, "after-1");
        assert_eq!(failed.cause, "error[E0308]: mismatched types");
        let note = rejection_note(&task.instance_id, reason, &gated.failed_builds);
        assert!(note.contains("works on the old code"), "{note}");
    }

    /// A reason that is not a failed build shows none, whatever the runs were.
    #[test]
    fn a_rejection_that_is_not_a_failed_build_shows_no_log() {
        let passing = outcome(0, &format!("{RUNNING}test old ... ok\n"));
        let (task, gated) = gate_with(vec![passing; 3], 1);
        let reason = GateRejection::NoFailToPass;
        assert_eq!(gated.verdict, GateVerdict::Rejected { reason });
        assert!(gated.failed_builds.is_empty());
        let note = rejection_note(&task.instance_id, reason, &gated.failed_builds);
        assert!(!note.contains("log:"), "{note}");
    }

    #[test]
    fn a_stopped_or_silent_build_says_so_instead_of_guessing() {
        assert_eq!(
            build_failure_cause(Some(StopReason::TimeLimit), "error: something"),
            "stopped at the time limit before it finished"
        );
        assert_eq!(
            build_failure_cause(None, "Killed\n"),
            "the build failed without an error line in its log"
        );
        let long = format!("error: {}", "x".repeat(1000));
        assert_eq!(
            build_failure_cause(None, &long).chars().count(),
            MAX_CAUSE_CHARS
        );
    }

    #[test]
    fn spec_detect_keeps_a_saved_spec_and_says_how_it_differs() {
        let proposed = spec::propose("o/r", &["Cargo.toml".into()], None).expect("rust");
        let (fresh, notes) = settle_spec(None, proposed.clone());
        assert_eq!(fresh, proposed);
        assert!(notes.is_empty(), "{notes:?}");

        let mut saved = proposed.clone();
        saved.memory_mib = 3072;
        saved.toolchain_image = "rust:1.75-bookworm".into();
        let (kept, notes) = settle_spec(Some(saved.clone()), proposed.clone());
        assert_eq!(kept, saved);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("toolchain_image, memory_mib"),
            "{notes:?}"
        );

        // A spec edited by hand before the cap existed: kept, and the leftover is pointed out.
        saved.build_cmd = "env RUSTFLAGS=--cap-lints=warn cargo test {scope} --no-run".into();
        let (kept, notes) = settle_spec(Some(saved.clone()), proposed);
        assert_eq!(kept.build_cmd, saved.build_cmd);
        // Removing it is not enough: the tasks gated with it keep their image and script.
        let note = notes.iter().find(|n| n.contains("mentions RUSTFLAGS"));
        let note = note.unwrap_or_else(|| panic!("a RUSTFLAGS note in {notes:?}"));
        assert!(
            note.contains("build --only") && note.contains("freeze again"),
            "{note}"
        );
    }

    /// Specs are filed by repository name alone: `a/tool` and `b/tool` share one file.
    #[test]
    fn a_spec_saved_for_another_repository_of_the_same_name_is_pointed_out() {
        let saved = spec::propose("a/tool", &["Cargo.toml".into()], None).expect("rust");
        let proposed = spec::propose("b/tool", &["Cargo.toml".into()], None).expect("rust");
        let (kept, notes) = settle_spec(Some(saved), proposed);
        assert_eq!(kept.repo, "b/tool");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("a/tool") && notes[0].contains("b/tool"),
            "{notes:?}"
        );
    }

    #[test]
    fn spec_detect_stops_on_a_saved_spec_it_cannot_read_and_leaves_the_file() {
        let home = tempfile::tempdir().expect("temp dir");
        let store = Store::new(EvalLayout::new(home.path()));
        let file = store.layout().spec_file("tool");
        fs::create_dir_all(file.parent().expect("specs folder")).expect("mkdir");
        fs::write(&file, "{ not json").expect("write");
        let checkout = tempfile::tempdir().expect("temp dir");
        let repo = checkout.path().join("tool");
        fs::create_dir_all(&repo).expect("mkdir");
        fs::write(repo.join("Cargo.toml"), "[package]\nname = \"tool\"\n").expect("write");
        let commit = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "one",
        ];
        for args in [&["init", "-q"][..], &["add", "-A"], &commit] {
            let ok = std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .expect("git runs")
                .success();
            assert!(ok, "git {args:?}");
        }
        let error = spec_detect(&store, &repo).expect_err("an unreadable spec stops it");
        assert!(
            format!("{error:#}").contains("could not be read"),
            "{error:#}"
        );
        assert_eq!(
            fs::read_to_string(&file).expect("still there"),
            "{ not json"
        );
    }

    #[test]
    fn names_are_safe_for_docker() {
        assert_eq!(sanitize("repo__a1/b c"), "repo__a1-b-c");
    }
}
