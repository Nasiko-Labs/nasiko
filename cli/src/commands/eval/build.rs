//! `nasiko eval spec detect` and `nasiko eval build`: Docker images per task, and the sanity
//! check that fills FAIL_TO_PASS / PASS_TO_PASS (`nasiko_evals::gate`).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use nasiko_evals::gate::{self, GateRuns, GateVerdict, RunOutcome};
use nasiko_evals::spec::{self, EXIT_BUILD, EXIT_MODEL_PATCH, EXIT_TEST_PATCH, EnvSpec};
use nasiko_evals::{TaskInstance, TaskStatus, diff, testlog};

use super::containers::{Containers, ImageBuild, JobLimits, JobSpec, Mount, Network};
use super::git::Repo;
use super::images;
use super::store::Store;

/// `nasiko eval spec detect`: proposes and saves the build/test spec of a repo.
pub fn spec_detect(store: &Store, repo_dir: &Path) -> Result<EnvSpec> {
    let repo = Repo::open(repo_dir)?;
    let slug = repo.slug();
    let root = repo.root_files("HEAD")?;
    let toolchain = if root.iter().any(|f| f == "Cargo.toml") {
        ["rust-toolchain.toml", "rust-toolchain"]
            .iter()
            .find_map(|f| repo.file_at("HEAD", f).ok().flatten())
    } else if root.iter().any(|f| f == "package.json") {
        repo.file_at("HEAD", "package.json").ok().flatten()
    } else {
        ["pyproject.toml", "requirements.txt", "pytest.ini"]
            .iter()
            .find(|&&name| root.iter().any(|f| f == name))
            .and_then(|name| repo.file_at("HEAD", name).ok().flatten())
    };
    let mut spec = spec::propose(&repo.full_name(), &root, toolchain.as_deref()).context(
        "only Cargo (Rust) and Vitest (TypeScript/JavaScript) repositories are supported so far",
    )?;
    spec.local_path = Some(repo.root().to_string_lossy().into_owned());
    store.save_spec(&slug, &spec)?;
    Ok(spec)
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
        let verdict = build_task(store, containers, &mut task, args.runs.max(1), disk_mib);
        let verdict = match verdict {
            Ok(v) => {
                apply_verdict(&mut task, &v);
                store.save_task(&task)?;
                Ok(v)
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

fn build_task(
    store: &Store,
    containers: &dyn Containers,
    task: &mut TaskInstance,
    runs: u32,
    disk_mib: u32,
) -> Result<GateVerdict> {
    let slug = repo_slug(&task.repo);
    let spec = store.load_spec(slug)?.with_context(|| {
        format!("no spec for {slug}; run `nasiko eval spec detect --repo <path>`")
    })?;
    let repo_path = spec
        .local_path
        .as_deref()
        .context("the spec has no local_path; re-run `nasiko eval spec detect`")?;
    let repo = Repo::open(Path::new(repo_path))?;
    let scope = if spec.language == spec::Language::Rust {
        test_scope(&repo, task)?
    } else {
        Vec::new()
    };
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
    let pristine = ctx.run(Setup::Pristine, 1)?;
    let mut before = Vec::new();
    let mut after = Vec::new();
    for n in 1..=runs {
        before.push(ctx.run(Setup::Before, n)?);
        after.push(ctx.run(Setup::After, n)?);
    }
    Ok(gate::judge(&GateRuns {
        pristine,
        before,
        after,
    }))
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

impl GateCtx<'_> {
    fn run(&self, setup: Setup, n: u32) -> Result<RunOutcome> {
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
        fs::write(dir.join("output.log"), &log)?;
        // New tests that cannot even be applied to the old code cannot exist before the fix:
        // count it like a build failure, and the pristine run decides what passed before.
        if matches!(setup, Setup::Before) && outcome.exit_code == Some(EXIT_TEST_PATCH) {
            return Ok(RunOutcome {
                built: false,
                results: Default::default(),
            });
        }
        if matches!(outcome.exit_code, Some(EXIT_MODEL_PATCH | EXIT_TEST_PATCH)) {
            bail!(
                "the {} patch did not apply in the task image (see {})",
                setup.name(),
                dir.join("output.log").display()
            );
        }
        let built = outcome.stopped.is_none() && outcome.exit_code != Some(EXIT_BUILD);
        Ok(RunOutcome {
            built,
            results: testlog::parse(spec.log_parser, &log),
        })
    }
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
    use crate::commands::eval::containers::JobOutcome;
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
        let run = |s, n| ctx.run(s, n).expect("runs");
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

    #[test]
    fn names_are_safe_for_docker() {
        assert_eq!(sanitize("repo__a1/b c"), "repo__a1-b-c");
    }
}
