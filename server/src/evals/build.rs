//! The build job: a task's image (the repository at its base commit, from GitHub) and the gate
//! that fills `FAIL_TO_PASS` / `PASS_TO_PASS`: the tests run on the old code, on the old code
//! with the new tests, and on the real fix with the new tests, several times to drop flaky
//! tests. Every run is a batch job with no network.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use nasiko_evals::gate::{self, GateRuns, GateVerdict, RunOutcome};
use nasiko_evals::spec::{self, EXIT_BUILD, EXIT_MODEL_PATCH, EXIT_TEST_PATCH, EnvSpec};
use nasiko_evals::{TaskInstance, TaskStatus, diff, images, testlog};
use nasiko_runtime::{BatchFile, BatchJob, BatchLimits, BatchNetwork, BatchOutcome, BatchRunner};
use serde_json::json;

use super::forge::Forge;
use super::jobs::{Job, JobKind};
use super::steps::{need, next_task_job};
use super::store;
use crate::state::AppState;

/// Disk a gate or grading container may write: building a crate's test binaries takes room,
/// and a correct fix must never fail because the grader ran out of it.
pub const GRADE_DISK_MIB: u64 = 8192;
/// Log tail kept per gate run.
const GATE_LOG_BYTES: usize = 16 * 1024;

pub async fn run(state: &AppState, job: &Job) -> Result<()> {
    let row = store::task(&state.db, need(job.task_id)?)
        .await?
        .context("task gone")?;
    let dataset = store::dataset(&state.db, row.dataset_id)
        .await?
        .context("dataset gone")?;
    let mut task = row.instance.0;
    if task.x_nasiko.status != TaskStatus::Candidate {
        return Ok(());
    }
    let spec = store::spec(&state.db, job.user_id, &row.repo)
        .await?
        .context("the repository has no build settings")?;
    let forge = match Forge::open(state, job.user_id, &row.repo).await? {
        Ok(forge) => forge,
        Err(refusal) => bail!("the repository cannot be read: {}", refusal.code()),
    };
    let Some(scope) = test_scope(&forge, &task).await? else {
        task.x_nasiko.status = TaskStatus::Rejected;
        task.x_nasiko.reject_reason =
            Some("build: no Cargo package owns the task's files, so it has no tests to run".into());
        store::save_task(&state.db, row.id, &task, "done").await?;
        return Ok(());
    };
    task.image = build_image(state, &forge, &spec, &task.base_commit, &scope).await?;
    task.eval_script = spec::eval_script(&spec, &scope);
    task.log_parser = spec.log_parser.swebench_name().to_string();
    task.eval_type = "pass_and_fail".to_string();
    task.x_nasiko.test_scope = scope;

    let runs = dataset.gate_runs.max(1) as u32;
    let label = format!("eval-gate-{}", row.id);
    let mut logs = Vec::new();
    let gate_run = |setup: Setup, n: u32| {
        let (state, task, spec, label) = (state, &task, &spec, label.clone());
        async move {
            let (model, tests) = match setup {
                Setup::Pristine => ("", ""),
                Setup::Before => ("", task.test_patch.as_str()),
                Setup::After => (task.patch.as_str(), task.test_patch.as_str()),
            };
            let name = format!("{label}-{}-{n}", setup.name());
            let outcome = test_run(
                state.batch_runner.as_ref(),
                task,
                spec,
                &name,
                &label,
                model,
                tests,
            )
            .await?;
            Ok::<_, anyhow::Error>((setup, n, outcome))
        }
    };
    let mut judged = |(setup, n, outcome): (Setup, u32, BatchOutcome)| -> Result<RunOutcome> {
        logs.push(json!({
            "run": format!("{}-{n}", setup.name()),
            "exit_code": outcome.exit_code,
            "log": nasiko_runtime::batch::tail(&outcome.logs, GATE_LOG_BYTES),
        }));
        read_gate_run(setup, &outcome, &spec)
    };
    let pristine = judged(gate_run(Setup::Pristine, 1).await?)?;
    let mut before = Vec::new();
    let mut after = Vec::new();
    for n in 1..=runs {
        before.push(judged(gate_run(Setup::Before, n).await?)?);
        after.push(judged(gate_run(Setup::After, n).await?)?);
    }
    let verdict = gate::judge(&GateRuns {
        pristine,
        before,
        after,
    });
    apply_verdict(&mut task, &verdict);
    store::save_gate(
        &state.db,
        row.id,
        &json!({"verdict": verdict, "runs": logs}),
    )
    .await?;
    let next = match task.x_nasiko.status {
        TaskStatus::Reproducible if !dataset.reviewer_config_ids.is_empty() => "screen",
        TaskStatus::Reproducible => "review",
        _ => "done",
    };
    store::save_task(&state.db, row.id, &task, next).await?;
    if next == "screen" {
        next_task_job(state, job, JobKind::TaskScreen).await?;
    }
    Ok(())
}

/// The task's test scope, from the Cargo packages owning the files the fix and its tests change
/// ([`spec::task_test_scope`]); `None` when no package owns them.
async fn test_scope(forge: &Forge, task: &TaskInstance) -> Result<Option<Vec<String>>> {
    let mut files = Vec::new();
    for (text, in_tests) in [(&task.patch, false), (&task.test_patch, true)] {
        for file in diff::parse_unified(text)? {
            if let Some((package, manifest)) = forge
                .cargo_package_of(&task.x_nasiko.gold_commit, file.path())
                .await?
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
    Ok(spec::task_test_scope(&files))
}

/// The task image: the env and task recipes in one Dockerfile, with the repository at
/// `base_commit` next to it. Skipped when an image with the tag exists (tags name contents).
async fn build_image(
    state: &AppState,
    forge: &Forge,
    spec: &EnvSpec,
    base_commit: &str,
    scope: &[String],
) -> Result<String> {
    let tag = images::task_image_tag(spec, base_commit, scope);
    let source = forge.archive(base_commit).await?;
    let context = build_context(&images::combined_dockerfile(spec, scope), &source)?;
    Ok(state.batch_runner.build_image(&context, &tag).await?)
}

/// A build context: a tar holding the `Dockerfile` and `source.tar`.
fn build_context(dockerfile: &str, source: &[u8]) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, bytes) in [
        ("Dockerfile", dockerfile.as_bytes()),
        ("source.tar", source),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        builder.append_data(&mut header, name, bytes)?;
    }
    Ok(builder.into_inner()?)
}

/// One test run of `task`'s image: `model_patch` and `test_patch` applied, built, tested, with
/// no network. Used by the gate and by grading.
pub async fn test_run(
    runner: &dyn BatchRunner,
    task: &TaskInstance,
    spec: &EnvSpec,
    name: &str,
    label: &str,
    model_patch: &str,
    test_patch: &str,
) -> Result<BatchOutcome> {
    let file = |path: &str, text: &str| BatchFile {
        path: path.to_string(),
        contents: text.as_bytes().to_vec(),
    };
    Ok(runner
        .run(BatchJob {
            name: name.to_string(),
            image: task.image.clone(),
            // The grading script reads its patches from `/eval`.
            command: vec![
                "bash".into(),
                "-c".into(),
                "cp /eval/in/model.patch /eval/in/test.patch /eval/ && exec bash /eval/in/run.sh"
                    .into(),
            ],
            env: vec![(
                "CARGO_BUILD_JOBS".into(),
                nasiko_evals::agent::CARGO_JOBS.into(),
            )],
            inputs: vec![
                file("model.patch", model_patch),
                file("test.patch", test_patch),
                file("run.sh", &task.eval_script),
            ],
            collect_outputs: false,
            network: BatchNetwork::None,
            limits: test_limits(spec),
            label: label.to_string(),
            before_stop: None,
        })
        .await?)
}

pub fn test_limits(spec: &EnvSpec) -> BatchLimits {
    BatchLimits {
        wall: Duration::from_secs(spec.timeout_secs),
        cpus: 4.0,
        memory_mib: spec.memory_mib.into(),
        pids: 4096,
        disk_mib: GRADE_DISK_MIB,
    }
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

fn read_gate_run(setup: Setup, outcome: &BatchOutcome, spec: &EnvSpec) -> Result<RunOutcome> {
    // New tests that cannot even be applied to the old code cannot exist before the fix:
    // count it like a build failure, and the pristine run decides what passed before.
    if matches!(setup, Setup::Before) && outcome.exit_code == Some(EXIT_TEST_PATCH) {
        return Ok(RunOutcome {
            built: false,
            results: Default::default(),
        });
    }
    if matches!(outcome.exit_code, Some(EXIT_MODEL_PATCH | EXIT_TEST_PATCH)) {
        bail!("the {} patch did not apply in the task image", setup.name());
    }
    Ok(RunOutcome {
        built: outcome.stopped.is_none() && outcome.exit_code != Some(EXIT_BUILD),
        results: testlog::parse(spec.log_parser, &outcome.logs),
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_context_holds_the_recipe_and_the_source() {
        let ctx = build_context("FROM x\n", b"src").unwrap();
        let mut archive = tar::Archive::new(ctx.as_slice());
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().display().to_string())
            .collect();
        assert_eq!(names, vec!["Dockerfile", "source.tar"]);
    }
}
