//! `nasiko eval run`: replays a frozen dataset with opencode in Docker, one model after another,
//! grades every attempt, and records the router's cost for each.
//!
//! Per model: pin the `eval` identity to it, make one tagged preflight call that must come back
//! from `/api/evals/usage` with a real price, start the gateway, run the attempts on N workers,
//! then stop the gateway. After all models, read the router's totals per attempt tag.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use nasiko_evals::diff::FileDiff;
use nasiko_evals::grade::{self, AgentExit, Grade, GradeInput};
use nasiko_evals::opencode::{self, AgentConfig};
use nasiko_evals::prompt::{self, ChatPrompt};
use nasiko_evals::report::AttemptRecord;
use nasiko_evals::spec::EnvSpec;
use nasiko_evals::{TaskInstance, TestPathRules, agent, cost, diff, harvest, testlog};
use serde::{Deserialize, Serialize};

use super::build::{sanitize, test_limits};
use super::containers::{Containers, JobLimits, JobOutcome, JobSpec, Mount, Network, StopReason};
use super::dataset;
use super::gateway::{self, GatewayPlan};
use super::git::Repo;
use super::images::opencode_version;
use super::llm::LlmClient;
use super::router::RouterClient;
use super::store::{EvalConfig, Store};

/// Inputs of `nasiko eval run`.
pub struct RunArgs {
    pub dataset: String,
    /// Short model names from `config.json`; the control (if any) is added first.
    pub models: Vec<String>,
    pub attempts: u32,
    pub workers: Option<u32>,
    pub builders: BuilderPolicy,
}

/// Whether models that rewrote or screened the dataset may be tested on it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BuilderPolicy {
    Refuse,
    /// Test them anyway (e.g. as a strong reference); the report marks their scores as
    /// optimistic.
    AllowAndMark,
}

/// `run.json`: everything needed to reproduce and report the run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    pub run_id: String,
    pub dataset: String,
    pub dataset_version: u32,
    pub models: Vec<String>,
    pub control: Option<String>,
    /// Tested models that also built the dataset (allowed with `--allow-builder`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builders_tested: Vec<String>,
    pub attempts: u32,
    pub prompt_version: String,
    pub opencode_version: String,
    pub started_at: chrono::DateTime<Utc>,
    pub finished_at: Option<chrono::DateTime<Utc>>,
}

/// Router tokens last 10 minutes; refresh well before.
const TOKEN_REFRESH: Duration = Duration::from_secs(8 * 60);

pub fn run(
    store: &Store,
    containers: &dyn Containers,
    router: &RouterClient,
    args: &RunArgs,
) -> Result<RunInfo> {
    let config = store.config()?;
    let (manifest, tasks) = dataset::load(store, &args.dataset)?;
    let mut models: Vec<String> = config.control.iter().cloned().collect();
    for m in &args.models {
        if !models.contains(m) {
            models.push(m.clone());
        }
    }
    if models.is_empty() {
        bail!("name at least one model with --model (short names from config.json)");
    }
    for m in &models {
        config
            .models
            .get(m)
            .with_context(|| format!("model {m:?} is not under \"models\" in config.json"))?;
    }
    let builders_tested = builders_among(store, &tasks, &config, &models)?;
    if !builders_tested.is_empty() && args.builders == BuilderPolicy::Refuse {
        bail!(
            "{builders_tested:?} also rewrote or screened this dataset; build the dataset with a stronger model (\"rewriter\" and \"reviewers\" in config.json, then `rewrite`, `screen`, `freeze` again), or pass --allow-builder to run it as a marked reference"
        );
    }
    let run_id = format!(
        "{}-{}",
        Utc::now().format("%Y%m%dT%H%M%S"),
        &uuid::Uuid::new_v4().simple().to_string()[..6]
    );
    let mut info = RunInfo {
        run_id: run_id.clone(),
        dataset: manifest.name.clone(),
        dataset_version: manifest.version,
        models: models.clone(),
        control: config.control.clone(),
        builders_tested,
        attempts: args.attempts.max(1),
        prompt_version: prompt::PROMPT_VERSION.to_string(),
        opencode_version: opencode_version().to_string(),
        started_at: Utc::now(),
        finished_at: None,
    };
    let run_dir = store.layout().run_dir(&run_id);
    store.save_json(&run_dir.join("run.json"), &info)?;
    let agent_id = router.ensure_eval_agent()?;
    let network = format!("nasiko-eval-{}", sanitize(&run_id));
    containers.ensure_internal_network(&network, &run_id)?;
    for short in &models {
        eprintln!(
            "model {short}: {} tasks × {} attempts",
            tasks.len(),
            info.attempts
        );
        let ctx = ModelRun {
            store,
            containers,
            router,
            config: &config,
            run_id: &run_id,
            run_dir: &run_dir,
            agent_id: &agent_id,
            short,
            network: &network,
        };
        ctx.run_model(
            &tasks,
            info.attempts,
            args.workers.unwrap_or(config.workers).max(1),
        )?;
    }
    let _ = containers.remove_labelled(Some(&run_id));
    attach_costs(store, router, &run_dir, &run_id)?;
    write_predictions(store, &run_dir)?;
    info.finished_at = Some(Utc::now());
    store.save_json(&run_dir.join("run.json"), &info)?;
    Ok(info)
}

/// The dataset must be built by stronger models than the ones tested: a model that rewrote a
/// task's request or screened it would be scored on tasks it shaped itself. Returns the tested
/// models that did.
fn builders_among(
    store: &Store,
    tasks: &[TaskInstance],
    config: &EvalConfig,
    models: &[String],
) -> Result<Vec<String>> {
    let mut builders: Vec<String> = config.reviewers.clone();
    builders.extend(config.rewriter.clone());
    for task in tasks {
        builders.extend(
            store
                .load_reviews(&task.instance_id)?
                .into_iter()
                .map(|r| r.reviewer),
        );
    }
    Ok(models
        .iter()
        .filter(|m| builders.contains(m))
        .cloned()
        .collect())
}

/// Everything one model's batch shares.
struct ModelRun<'a> {
    store: &'a Store,
    containers: &'a dyn Containers,
    router: &'a RouterClient,
    config: &'a EvalConfig,
    run_id: &'a str,
    run_dir: &'a Path,
    agent_id: &'a str,
    short: &'a str,
    network: &'a str,
}

/// One attempt to run.
#[derive(Clone)]
struct AttemptKey {
    task: TaskInstance,
    attempt: u32,
    secret: String,
    tag: String,
}

impl ModelRun<'_> {
    fn run_model(&self, tasks: &[TaskInstance], attempts: u32, workers: u32) -> Result<()> {
        let entry = &self.config.models[self.short];
        self.router.pin_model(self.agent_id, self.short, entry)?;
        self.preflight()?;
        let keys: Vec<AttemptKey> = tasks
            .iter()
            .flat_map(|t| (1..=attempts).map(move |a| (t, a)))
            .filter(|(t, a)| {
                !self
                    .attempt_dir(&t.instance_id, *a)
                    .join("attempt.json")
                    .exists()
            })
            .map(|(t, a)| AttemptKey {
                task: t.clone(),
                attempt: a,
                secret: uuid::Uuid::new_v4().simple().to_string(),
                tag: format!("{}:{}:{}:{a}", self.run_id, self.short, t.instance_id),
            })
            .collect();
        if keys.is_empty() {
            return Ok(());
        }
        let gw = gateway::start(
            self.containers,
            GatewayPlan {
                upstream: gateway::upstream_from_container(
                    self.router.router_base().trim_end_matches("/v1"),
                ),
                attempts: keys
                    .iter()
                    .map(|k| (k.secret.clone(), k.tag.clone()))
                    .collect(),
                container: format!("nasiko-eval-gw-{}", sanitize(self.run_id)),
                networks: vec![self.network.to_string()],
                config_dir: self.run_dir.join("gateway").join(self.short),
                label: self.run_id.to_string(),
            },
            &self.router.mint_token(self.agent_id)?.token,
        )?;
        let queue = Mutex::new(VecDeque::from(keys));
        let stop = Arc::new(AtomicBool::new(false));
        std::thread::scope(|s| -> Result<()> {
            let refresher = s.spawn(|| {
                let mut last = Instant::now();
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(5));
                    if last.elapsed() >= TOKEN_REFRESH {
                        match self.router.mint_token(self.agent_id) {
                            Ok(t) => {
                                if let Err(e) = gw.refresh(&t.token) {
                                    eprintln!("warning: gateway token refresh failed: {e:#}");
                                }
                            }
                            Err(e) => eprintln!("warning: could not mint a router token: {e:#}"),
                        }
                        last = Instant::now();
                    }
                }
            });
            let workers: Vec<_> = (0..workers)
                .map(|_| {
                    s.spawn(|| {
                        while let Some(key) = queue.lock().map(|mut q| q.pop_front()).ok().flatten()
                        {
                            let id = format!("{} #{}", key.task.instance_id, key.attempt);
                            match self.run_attempt(&key, &gw.base_url()) {
                                Ok(r) => eprintln!("  {id}: {}", outcome_word(&r.grade)),
                                Err(e) => eprintln!("  {id}: error: {e:#}"),
                            }
                        }
                    })
                })
                .collect();
            for w in workers {
                let _ = w.join();
            }
            stop.store(true, Ordering::Relaxed);
            let _ = refresher.join();
            Ok(())
        })?;
        drop(gw);
        Ok(())
    }

    /// One tiny tagged call; it must come back from the router's usage with a real price.
    fn preflight(&self) -> Result<()> {
        let entry = &self.config.models[self.short];
        let llm = LlmClient::pinned(self.router, self.short, entry)?;
        let tag = format!("{}:preflight:{}", self.run_id, self.short);
        llm.complete(
            &ChatPrompt {
                system: "Answer with one word.".into(),
                user: "Say ok.".into(),
            },
            &tag,
        )
        .context("preflight call through the router failed")?;
        // The router writes usage asynchronously; give it a moment.
        for _ in 0..10 {
            if let Some(u) = self.router.run_usage(self.run_id)?.get(&tag) {
                return cost::check_priced(u).with_context(|| {
                    format!(
                        "the router has no price for {}; add one before running (see oss/docs/EVALS.md)",
                        entry.model
                    )
                });
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        bail!(
            "the router did not record the preflight call under its eval tag; is this cluster on a version with replay evals?"
        )
    }

    fn attempt_dir(&self, instance_id: &str, attempt: u32) -> PathBuf {
        self.run_dir
            .join("attempts")
            .join(instance_id)
            .join(self.short)
            .join(attempt.to_string())
    }

    fn spec_for(&self, task: &TaskInstance) -> Result<EnvSpec> {
        let slug = task.repo.rsplit('/').next().unwrap_or(&task.repo);
        self.store
            .load_spec(slug)?
            .with_context(|| format!("no spec for {slug}"))
    }

    fn run_attempt(&self, key: &AttemptKey, base_url: &str) -> Result<AttemptRecord> {
        let task = &key.task;
        let spec = self.spec_for(task)?;
        let dir = self.attempt_dir(&task.instance_id, key.attempt);
        let (in_dir, out_dir) = (dir.join("in"), dir.join("out"));
        fs::create_dir_all(&in_dir)?;
        let _ = fs::remove_dir_all(&out_dir);
        fs::create_dir_all(&out_dir)?;
        let agent_cfg = opencode::config_json(&AgentConfig {
            context_tokens: self.config.context_tokens,
            output_tokens: self.config.output_tokens,
            max_steps: self.config.max_steps,
        });
        fs::write(
            in_dir.join("opencode.json"),
            serde_json::to_string_pretty(&agent_cfg)?,
        )?;
        fs::write(
            in_dir.join("prompt.md"),
            prompt::agent_task_prompt(task, &spec),
        )?;
        let wall = u64::from(self.config.attempt_minutes) * 60;
        fs::write(
            in_dir.join("agent.sh"),
            agent::script(wall.saturating_sub(agent::TIMEOUT_MARGIN_SECS)),
        )?;
        let outcome = self.containers.run_to_end(&JobSpec {
            name: format!("nasiko-eval-agent-{}", sanitize(&key.tag)),
            image: task.image.clone(),
            cmd: vec!["bash".into(), "/eval/in/agent.sh".into()],
            network: Network::Named(self.network.to_string()),
            mounts: vec![
                Mount {
                    host: in_dir.canonicalize()?,
                    container: "/eval/in".into(),
                    read_only: true,
                },
                Mount {
                    host: out_dir.canonicalize()?,
                    container: "/eval/out".into(),
                    read_only: false,
                },
            ],
            env: vec![
                ("OPENCODE_CONFIG".into(), "/eval/in/opencode.json".into()),
                ("OPENAI_BASE_URL".into(), base_url.to_string()),
                ("EVAL_GATEWAY_SECRET".into(), key.secret.clone()),
                ("HOME".into(), "/tmp/home".into()),
                // Fewer parallel rustc jobs: lower peak memory, so builds are not OOM-killed.
                ("CARGO_BUILD_JOBS".into(), agent::CARGO_JOBS.into()),
            ],
            limits: JobLimits {
                wall: Duration::from_secs(wall),
                cpus: 4.0,
                memory_mib: 6144,
                pids: 4096,
                disk_mib: self.config.storage_mib,
            },
            label: self.run_id.to_string(),
            // A run stopped for time or disk still hands in what it changed.
            before_stop: Some(agent::SAVE_DIFF.to_string()),
        })?;
        fs::write(
            out_dir.join("container.log"),
            format!(
                "container exit: {:?}, stopped: {:?}\n{}\n--- stderr ---\n{}",
                outcome.exit_code, outcome.stopped, outcome.stdout, outcome.stderr
            ),
        )?;
        let export = fs::read_to_string(out_dir.join("session-export.json")).ok();
        let agent_record = export
            .as_deref()
            .and_then(|e| opencode::parse_export(e).ok());
        let exit = agent_exit(&outcome, &out_dir, agent_record.as_ref());
        let raw = fs::read_to_string(out_dir.join("raw.patch")).unwrap_or_default();
        let rules = TestPathRules::from_globs(&spec.test_paths);
        let files = diff::without_tests(diff::parse_unified(&raw)?, &rules);
        let repo = spec
            .local_path
            .as_deref()
            .map(|p| Repo::open(Path::new(p)))
            .transpose()?;
        let model_patch = diff::join(&without_inline_test_edits(
            repo.as_ref(),
            &task.base_commit,
            files,
        )?);
        fs::write(dir.join("model.patch"), &model_patch)?;
        // Nothing to grade: an empty patch cannot resolve anything, so skip the test build.
        let (script_exit, results) = if model_patch.trim().is_empty() {
            (Some(0), testlog::TestResults::new())
        } else {
            grade_patch(
                self.containers,
                task,
                &spec,
                &dir,
                test_limits(&spec, self.config.grade_storage_mib),
                self.run_id,
            )?
        };
        let grade = grade::grade(&GradeInput {
            fail_to_pass: &task.fail_to_pass,
            pass_to_pass: &task.pass_to_pass,
            agent_exit: exit,
            patch_is_empty: model_patch.trim().is_empty(),
            script_exit,
            results: &results,
        });
        let record = AttemptRecord {
            instance_id: task.instance_id.clone(),
            model: self.short.to_string(),
            attempt: key.attempt,
            difficulty: task.difficulty,
            grade,
            usage: None,
            cost_usd: None,
            worst_price: None,
            wall_secs: outcome.wall.as_secs(),
            original_cost_usd: None,
        };
        self.store.save_json(
            &dir.join("attempt.json"),
            &AttemptFile {
                tag: key.tag.clone(),
                agent_tokens: agent_record.map(|r| r.usage),
                record: record.clone(),
            },
        )?;
        Ok(record)
    }
}

/// The model's changes with every Rust inline test module put back as it was at the base
/// commit (`nasiko_evals::harvest::graded_files`); local git supplies the base texts.
fn without_inline_test_edits(
    repo: Option<&Repo>,
    base_commit: &str,
    files: Vec<FileDiff>,
) -> Result<Vec<FileDiff>> {
    let Some(repo) = repo else {
        return Ok(files);
    };
    let mut base_texts = BTreeMap::new();
    for path in harvest::graded_paths(&files) {
        if let Some(text) = repo.file_at(base_commit, &path)? {
            base_texts.insert(path, text);
        }
    }
    Ok(harvest::graded_files(files, &base_texts)?)
}

/// `attempt.json`: the graded record plus what is needed to attach the router's cost later.
#[derive(Serialize, Deserialize)]
pub struct AttemptFile {
    pub tag: String,
    /// opencode's own token count, kept only to cross-check the router's.
    pub agent_tokens: Option<nasiko_evals::TokenUsage>,
    pub record: AttemptRecord,
}

fn agent_exit(
    outcome: &JobOutcome,
    out: &Path,
    record: Option<&opencode::AgentRecord>,
) -> AgentExit {
    let stopped = outcome.stopped.as_ref().map(|s| match s {
        StopReason::DiskLimit => agent::Stopped::DiskLimit,
        StopReason::TimeLimit => agent::Stopped::TimeLimit,
    });
    let code = fs::read_to_string(out.join(agent::EXIT_CODE_FILE)).ok();
    agent::exit(
        stopped,
        outcome.exit_code,
        code.as_deref(),
        record.is_some_and(|r| r.hit_step_limit),
    )
}

/// Applies `<dir>/model.patch` and the task's test patch in a fresh container with no network,
/// and runs the tests. Returns the script's exit code and the parsed results.
pub fn grade_patch(
    containers: &dyn Containers,
    task: &TaskInstance,
    spec: &EnvSpec,
    dir: &Path,
    limits: JobLimits,
    label: &str,
) -> Result<(Option<i32>, testlog::TestResults)> {
    let grade_dir = dir.join("grade");
    fs::create_dir_all(&grade_dir)?;
    fs::write(
        grade_dir.join("model.patch"),
        fs::read_to_string(dir.join("model.patch")).unwrap_or_default(),
    )?;
    fs::write(grade_dir.join("test.patch"), &task.test_patch)?;
    fs::write(grade_dir.join("run.sh"), &task.eval_script)?;
    let outcome = containers.run_to_end(&JobSpec {
        name: format!(
            "nasiko-eval-grade-{}",
            sanitize(&format!("{label}-{}", dir.display()))
        )
        .chars()
        .take(120)
        .collect(),
        image: task.image.clone(),
        cmd: vec!["bash".into(), "/eval/run.sh".into()],
        network: Network::None,
        mounts: vec![Mount {
            host: grade_dir.canonicalize()?,
            container: "/eval".into(),
            read_only: true,
        }],
        env: vec![("CARGO_BUILD_JOBS".into(), agent::CARGO_JOBS.into())],
        limits,
        label: label.to_string(),
        before_stop: None,
    })?;
    let log = format!("{}\n--- stderr ---\n{}", outcome.stdout, outcome.stderr);
    fs::write(grade_dir.join("output.log"), &log)?;
    let exit = if outcome.stopped.is_some() {
        None
    } else {
        outcome.exit_code
    };
    Ok((exit, testlog::parse(spec.log_parser, &log)))
}

fn outcome_word(g: &Grade) -> String {
    match g.failure {
        None => "resolved".into(),
        Some(f) => format!("not resolved ({f:?})"),
    }
}

/// Every `attempt.json` under a run.
pub fn attempt_files(run_dir: &Path) -> Result<Vec<(PathBuf, AttemptFile)>> {
    let mut out = Vec::new();
    let root = run_dir.join("attempts");
    let Ok(tasks) = fs::read_dir(&root) else {
        return Ok(out);
    };
    for t in tasks.flatten() {
        for m in fs::read_dir(t.path())?.flatten() {
            for a in fs::read_dir(m.path())?.flatten() {
                let path = a.path().join("attempt.json");
                if let Ok(text) = fs::read_to_string(&path) {
                    out.push((path, serde_json::from_str(&text)?));
                }
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Fills each attempt's cost from the router's usage, and prices the original session once per
/// task with the router's price book.
pub fn attach_costs(
    store: &Store,
    router: &RouterClient,
    run_dir: &Path,
    run_id: &str,
) -> Result<()> {
    let usage = router.run_usage(run_id)?;
    let mut original: BTreeMap<String, Option<f64>> = BTreeMap::new();
    for (path, mut file) in attempt_files(run_dir)? {
        if let Some(u) = usage.get(&file.tag) {
            file.record.usage = Some(u.usage);
            file.record.worst_price = Some(u.worst_price);
            file.record.cost_usd = cost::check_priced(u).ok().map(|_| u.cost_usd);
            if let Some(agent) = &file.agent_tokens
                && !cost::tokens_agree(&u.usage, agent)
            {
                eprintln!(
                    "warning: {}: router and agent token counts differ by more than 2%",
                    file.tag
                );
            }
        }
        let id = file.record.instance_id.clone();
        if !original.contains_key(&id) {
            original.insert(id.clone(), original_cost(store, router, &id).ok().flatten());
        }
        file.record.original_cost_usd = original[&id];
        store.save_json(&path, &file)?;
    }
    Ok(())
}

fn original_cost(store: &Store, router: &RouterClient, id: &str) -> Result<Option<f64>> {
    let text = fs::read_to_string(store.layout().source_file(id))?;
    let source: nasiko_evals::TaskSource = serde_json::from_str(&text)?;
    let mut total = 0.0;
    for (model, usage) in &source.usage_by_model {
        let priced = router.price(None, model, usage)?;
        if priced.quality == nasiko_evals::PriceQuality::Default {
            return Ok(None);
        }
        total += priced.cost_usd;
    }
    Ok(Some(total))
}

/// `preds.jsonl` in SWE-bench's predictions format, so the public grader can re-check a run.
fn write_predictions(store: &Store, run_dir: &Path) -> Result<()> {
    let mut lines = String::new();
    for (path, file) in attempt_files(run_dir)? {
        if file.record.attempt != 1 {
            continue;
        }
        let patch = fs::read_to_string(path.with_file_name("model.patch")).unwrap_or_default();
        lines.push_str(&serde_json::to_string(&serde_json::json!({
            "instance_id": file.record.instance_id,
            "model_name_or_path": file.record.model,
            "model_patch": patch,
        }))?);
        lines.push('\n');
    }
    store.save_text(&run_dir.join("preds.jsonl"), &lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_that_built_the_dataset_is_not_tested_on_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::new(nasiko_evals::EvalLayout::new(dir.path()));
        let task: TaskInstance =
            serde_json::from_str(include_str!("testdata/task.json")).expect("task");
        let mut config: EvalConfig = serde_json::from_str("{}").expect("config");
        config.rewriter = Some("strong".into());
        config.reviewers = vec!["strong".into()];
        let tasks = vec![task];
        assert!(
            builders_among(&store, &tasks, &config, &["flint".into()])
                .expect("reads")
                .is_empty()
        );
        assert_eq!(
            builders_among(&store, &tasks, &config, &["strong".into()]).expect("reads"),
            vec!["strong".to_string()]
        );
        config.reviewers = vec![];
        config.rewriter = None;
        let review = nasiko_evals::prompt::ScreenVerdict {
            reviewer: "flint".into(),
            clarity: 0,
            clarity_reason: String::new(),
            test_validity: 0,
            test_validity_reason: String::new(),
            other_problems: vec![],
            effort: nasiko_evals::Difficulty::Under15Min,
            effort_reason: String::new(),
        };
        store
            .save_json(
                &store.layout().review_file(&tasks[0].instance_id, "flint"),
                &review,
            )
            .expect("saves");
        assert_eq!(
            builders_among(&store, &tasks, &config, &["flint".into()]).expect("reads"),
            vec!["flint".to_string()]
        );
    }

    fn outcome(exit_code: Option<i32>, stopped: Option<StopReason>) -> JobOutcome {
        JobOutcome {
            exit_code,
            stopped,
            stdout: String::new(),
            stderr: String::new(),
            wall: Duration::from_secs(1),
        }
    }

    #[test]
    fn exits_are_classified() {
        let dir = tempfile::tempdir().expect("temp dir");
        let d = dir.path();
        assert_eq!(
            agent_exit(&outcome(None, Some(StopReason::DiskLimit)), d, None),
            AgentExit::DiskLimit
        );
        assert_eq!(
            agent_exit(&outcome(None, None), d, None),
            AgentExit::InfraError
        );
        assert_eq!(
            agent_exit(&outcome(Some(137), None), d, None),
            AgentExit::Crashed,
            "a container killed before the script ended is a crash"
        );
        fs::write(d.join("exit_code"), "124\n").expect("write");
        assert_eq!(
            agent_exit(&outcome(Some(0), None), d, None),
            AgentExit::TimeLimit
        );
        fs::write(d.join("exit_code"), "0\n").expect("write");
        let limited = opencode::AgentRecord {
            hit_step_limit: true,
            ..Default::default()
        };
        assert_eq!(
            agent_exit(&outcome(Some(0), None), d, Some(&limited)),
            AgentExit::StepLimit
        );
        assert_eq!(
            agent_exit(&outcome(Some(0), None), d, None),
            AgentExit::Finished
        );
    }

    /// A repository with one commit holding `src/lib.rs`; returns it and the commit.
    fn one_commit_repo(lib: &str) -> (Repo, String) {
        let dir = std::env::temp_dir().join(format!(
            "nasiko-eval-run-test-{}-{}",
            std::process::id(),
            lib.len()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).expect("dir");
        fs::write(dir.join("src/lib.rs"), lib).expect("write");
        for args in [
            &["init", "-q"][..],
            &["add", "-A"],
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "base",
            ],
        ] {
            let ok = std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .status()
                .expect("git runs")
                .success();
            assert!(ok, "git {args:?}");
        }
        let repo = Repo::open(&dir).expect("repo");
        let head = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("rev-parse");
        (
            repo,
            String::from_utf8_lossy(&head.stdout).trim().to_string(),
        )
    }

    #[test]
    fn a_model_test_named_like_a_hidden_one_is_not_graded() {
        let base = "pub fn f() -> u8 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn old() {}\n}\n";
        let model = "pub fn f() -> u8 {\n    2\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn old() {}\n\n    #[test]\n    fn hidden_name() {}\n}\n";
        let (repo, head) = one_commit_repo(base);
        let raw = nasiko_evals::patch::diff_texts("src/lib.rs", Some(base), model);
        let files = diff::parse_unified(&raw).expect("parses");
        let graded = diff::join(&without_inline_test_edits(Some(&repo), &head, files).expect("ok"));
        assert!(graded.contains("+    2"), "{graded}");
        assert!(!graded.contains("hidden_name"), "{graded}");

        // A file with only test edits is not graded at all; without a repo nothing changes.
        let tests_only = base.replace("fn old() {}", "fn old() { assert!(true) }");
        let raw = nasiko_evals::patch::diff_texts("src/lib.rs", Some(base), &tests_only);
        let files = diff::parse_unified(&raw).expect("parses");
        assert!(
            without_inline_test_edits(Some(&repo), &head, files.clone())
                .expect("ok")
                .is_empty()
        );
        assert_eq!(
            without_inline_test_edits(None, &head, files)
                .expect("ok")
                .len(),
            1
        );
        let _ = fs::remove_dir_all(repo.root());
    }

    #[test]
    fn work_the_agent_committed_is_still_saved() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (repo, out) = (dir.path().join("testbed"), dir.path().join("raw.patch"));
        fs::create_dir_all(&repo).expect("dir");
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .current_dir(&repo)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status()
                .expect("git runs")
                .success();
            assert!(ok, "git {args:?}");
        };
        fs::write(repo.join("lib.rs"), "fn a() {}\n").expect("write");
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "base"]);
        // The agent edits, commits, then edits again without committing.
        fs::write(repo.join("lib.rs"), "fn b() {}\n").expect("write");
        git(&["commit", "-qam", "agent fix"]);
        fs::write(repo.join("new.rs"), "fn c() {}\n").expect("write");
        let script = agent::SAVE_DIFF
            .replace("/testbed", &repo.display().to_string())
            .replace("/eval/out/raw.patch", &out.display().to_string());
        let ok = std::process::Command::new("sh")
            .args(["-c", &script])
            .status()
            .expect("sh runs")
            .success();
        assert!(ok);
        let patch = fs::read_to_string(&out).expect("saved");
        assert!(patch.contains("+fn b() {}"), "{patch}");
        assert!(patch.contains("+fn c() {}"), "{patch}");
    }
}
