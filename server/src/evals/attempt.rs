//! Running an eval: a preflight per model (its calls must come back priced), one job per
//! attempt (opencode replays the task in the task image, reaching only the router, then the
//! patch is graded in a fresh container with no network), and the report once every attempt
//! ended, with every cost read back from the router's own usage rows.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use nasiko_evals::grade::{self, FailureClass, Grade, GradeInput, Resolution};
use nasiko_evals::opencode::{self, AgentConfig};
use nasiko_evals::report::{self, AttemptRecord};
use nasiko_evals::{PriceQuality, RouterUsage, TestPathRules, TokenUsage, agent, diff, harvest};
use nasiko_runtime::{BatchFile, BatchJob, BatchLimits, BatchNetwork, BatchRunner, BatchStop};
use serde_json::json;
use uuid::Uuid;

use super::build::test_run;
use super::forge::Forge;
use super::jobs::{Job, JobKind, NewJob, enqueue, notify};
use super::steps::need;
use super::store::{self, AttemptRow, RunModel};
use super::usage::{AttemptUsage, run_usage};
use crate::state::AppState;

/// Wall-clock budget of one attempt.
pub const ATTEMPT_MINUTES: u64 = 45;
/// Disk the agent's container may write.
const AGENT_DISK_MIB: u64 = 2048;
/// What the replayed agent is told about its model (opencode compacts its context by it).
const AGENT_CONFIG: AgentConfig = AgentConfig {
    context_tokens: 128_000,
    output_tokens: 16_384,
    max_steps: 150,
};
/// How long the preflight waits for its call's usage row.
const PREFLIGHT_WAIT: Duration = Duration::from_secs(15);

pub async fn preflight(state: &AppState, job: &Job) -> Result<()> {
    let run_id = need(job.run_id)?;
    let model = need(job.model_id)?;
    let run = store::run(&state.db, run_id).await?.context("run gone")?;
    if run.status == "cancelled" {
        return Ok(());
    }
    store::set_run_status(&state.db, run_id, "running", None).await?;
    let tag = format!("{run_id}:preflight:{model}");
    let check = nasiko_evals::prompt::ChatPrompt {
        system: "You are a health check.".into(),
        user: "Reply with the single word OK.".into(),
    };
    super::llm::complete(state, job.user_id, model, &tag, &check)
        .await
        .context("the model could not be called")?;
    let usage = wait_for_usage(state, job.user_id, run_id, &tag).await?;
    if let Err(unpriced) = nasiko_evals::cost::check_priced(&router_usage(&usage)) {
        store::fail_open_attempts(
            &state.db,
            run_id,
            Some(model),
            &format!("model_unpriced: {unpriced}"),
        )
        .await?;
        return Ok(());
    }
    let attempts: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM eval_attempts WHERE run_id = $1 AND model_id = $2 AND status = 'pending'",
    )
    .bind(run_id)
    .bind(model)
    .fetch_all(&state.db)
    .await?;
    for attempt in attempts {
        let mut next = NewJob::new(JobKind::RunAttempt, job.user_id);
        next.run_id = Some(run_id);
        next.attempt_id = Some(attempt);
        enqueue(&state.db, &next).await?;
    }
    notify(state);
    Ok(())
}

async fn wait_for_usage(
    state: &AppState,
    user_id: Uuid,
    run_id: Uuid,
    tag: &str,
) -> Result<AttemptUsage> {
    let deadline = tokio::time::Instant::now() + PREFLIGHT_WAIT;
    loop {
        let usage = run_usage(&state.db, user_id, &run_id.to_string()).await?;
        if let Some(found) = usage.into_iter().find(|u| u.attempt == tag) {
            return Ok(found);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("the router recorded no usage for the preflight call");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

pub async fn run(state: &AppState, job: &Job) -> Result<()> {
    let attempt = store::attempt(&state.db, need(job.attempt_id)?)
        .await?
        .context("attempt gone")?;
    let run = store::run(&state.db, attempt.run_id)
        .await?
        .context("run gone")?;
    if attempt.status != "pending" && attempt.status != "running" || run.status == "cancelled" {
        return Ok(());
    }
    store::set_attempt_running(&state.db, attempt.id).await?;
    let task = &attempt.instance.0;
    let spec = store::spec(&state.db, job.user_id, &task.repo)
        .await?
        .context("the repository has no build settings")?;
    let agent_id = super::identity::ensure(&state.db, job.user_id).await?;
    let wall = ATTEMPT_MINUTES * 60;
    let token = super::identity::attempt_token(
        agent_id,
        job.user_id,
        &attempt.tag,
        attempt.model_id,
        wall + 600,
    )?;
    let text = |path: &str, text: String| BatchFile {
        path: path.to_string(),
        contents: text.into_bytes(),
    };
    let label = format!("eval-run-{}", run.id);
    let outcome = state
        .batch_runner
        .run(BatchJob {
            name: format!("eval-agent-{}", attempt.id),
            image: task.image.clone(),
            command: vec!["bash".into(), format!("/eval/in/{}", agent::SCRIPT_FILE)],
            env: vec![
                (
                    "OPENCODE_CONFIG".into(),
                    format!("/eval/in/{}", agent::CONFIG_FILE),
                ),
                (opencode::ROUTER_TOKEN_ENV.into(), token),
                ("HOME".into(), "/tmp/home".into()),
                ("CARGO_BUILD_JOBS".into(), agent::CARGO_JOBS.into()),
            ],
            inputs: vec![
                text(
                    agent::CONFIG_FILE,
                    serde_json::to_string_pretty(&opencode::token_config_json(&AGENT_CONFIG))?,
                ),
                text(
                    agent::PROMPT_FILE,
                    nasiko_evals::prompt::agent_task_prompt(task, &spec),
                ),
                text(
                    agent::SCRIPT_FILE,
                    agent::script(wall.saturating_sub(agent::TIMEOUT_MARGIN_SECS)),
                ),
            ],
            collect_outputs: true,
            network: BatchNetwork::RouterOnly,
            limits: BatchLimits {
                wall: Duration::from_secs(wall),
                cpus: 4.0,
                memory_mib: spec.memory_mib.into(),
                pids: 4096,
                disk_mib: AGENT_DISK_MIB,
            },
            label: label.clone(),
            before_stop: Some(agent::SAVE_DIFF.to_string()),
        })
        .await?;
    let export = outcome.output_text(agent::EXPORT_FILE);
    let record = export
        .as_deref()
        .and_then(|e| opencode::parse_export(e).ok());
    let exit = agent::exit(
        outcome.stopped.map(|s| match s {
            BatchStop::DiskLimit => agent::Stopped::DiskLimit,
            BatchStop::TimeLimit => agent::Stopped::TimeLimit,
        }),
        outcome.exit_code,
        outcome.output_text(agent::EXIT_CODE_FILE).as_deref(),
        record.as_ref().is_some_and(|r| r.hit_step_limit),
    );
    let raw = outcome.output_text(agent::PATCH_FILE).unwrap_or_default();
    let model_patch = graded_patch(state, job.user_id, task, &spec, &raw).await?;

    let mut artifacts = BTreeMap::new();
    let agent_log = format!(
        "container exit: {:?}, stopped: {:?}\n{}\n--- agent.log ---\n{}",
        outcome.exit_code,
        outcome.stopped,
        outcome.logs,
        outcome.output_text("agent.log").unwrap_or_default()
    );
    save_artifact(state, &attempt, "agent.log", &agent_log, &mut artifacts).await;
    if let Some(export) = &export {
        save_artifact(state, &attempt, agent::EXPORT_FILE, export, &mut artifacts).await;
    }

    // Nothing to grade: an empty patch cannot resolve anything, so skip the test build.
    let (script_exit, results) = if model_patch.trim().is_empty() {
        (Some(0), Default::default())
    } else {
        let graded = test_run(
            state.batch_runner.as_ref(),
            task,
            &spec,
            &format!("eval-grade-{}", attempt.id),
            &label,
            &model_patch,
            &task.test_patch,
        )
        .await?;
        save_artifact(state, &attempt, "grade.log", &graded.logs, &mut artifacts).await;
        let results = nasiko_evals::testlog::parse(spec.log_parser, &graded.logs);
        (graded.exit_code, results)
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
        model: model_name(&run.models.0, attempt.model_id),
        attempt: attempt.attempt_no as u32,
        difficulty: task.difficulty,
        grade,
        usage: None,
        cost_usd: None,
        worst_price: None,
        wall_secs: outcome.wall.as_secs(),
        original_cost_usd: None,
    };
    store::finish_attempt(&state.db, attempt.id, &record, &model_patch, &artifacts).await
}

/// The model's raw diff as graded: test files dropped, and its edits to inline Rust test
/// modules undone (`nasiko_evals::harvest::graded_files`).
async fn graded_patch(
    state: &AppState,
    user_id: Uuid,
    task: &nasiko_evals::TaskInstance,
    spec: &nasiko_evals::spec::EnvSpec,
    raw: &str,
) -> Result<String> {
    let files = diff::without_tests(
        diff::parse_unified(raw).context("the agent's diff is unreadable")?,
        &TestPathRules::from_globs(&spec.test_paths),
    );
    let paths = harvest::graded_paths(&files);
    let base_texts = if paths.is_empty() {
        BTreeMap::new()
    } else {
        match Forge::open(state, user_id, &task.repo).await? {
            Ok(forge) => forge.files_at(&task.base_commit, &paths).await?,
            Err(refusal) => bail!("the repository cannot be read: {}", refusal.code()),
        }
    };
    Ok(diff::join(&harvest::graded_files(files, &base_texts)?))
}

async fn save_artifact(
    state: &AppState,
    attempt: &AttemptRow,
    name: &str,
    text: &str,
    artifacts: &mut BTreeMap<String, String>,
) {
    let key = format!("evals/{}/{}/{name}", attempt.run_id, attempt.id);
    match state
        .oci_storage
        .put_blob(&key, bytes::Bytes::copy_from_slice(text.as_bytes()))
        .await
    {
        Ok(_) => {
            artifacts.insert(name.to_string(), key);
        }
        Err(e) => {
            tracing::warn!(attempt = %attempt.id, %name, error = %e, "evals: could not store an artifact")
        }
    }
}

fn model_name(models: &[RunModel], id: Uuid) -> String {
    models
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| id.to_string())
}

pub async fn finalize(state: &AppState, job: &Job) -> Result<()> {
    let run_id = need(job.run_id)?;
    let run = store::run(&state.db, run_id).await?.context("run gone")?;
    if matches!(run.status.as_str(), "done" | "cancelled") {
        return Ok(());
    }
    store::set_run_status(&state.db, run_id, "finalizing", None).await?;
    let usage: BTreeMap<String, AttemptUsage> =
        run_usage(&state.db, job.user_id, &run_id.to_string())
            .await?
            .into_iter()
            .map(|u| (u.attempt.clone(), u))
            .collect();
    let dataset_tasks = store::tasks_of(&state.db, run.dataset_id).await?;
    let mut original_costs: BTreeMap<Uuid, Option<f64>> = BTreeMap::new();
    for task in &dataset_tasks {
        original_costs.insert(task.id, original_cost_of(state, &task.source.0).await);
    }
    let mut records = Vec::new();
    for attempt in store::attempts_of(&state.db, run_id).await? {
        let mut record = match attempt.record.as_ref().map(|r| r.0.clone()) {
            Some(r) => r,
            None => failed_record(&run.models.0, &attempt),
        };
        if let Some(u) = usage.get(&attempt.tag) {
            let routed = router_usage(u);
            record.usage = Some(routed.usage);
            record.worst_price = Some(routed.worst_price);
            record.cost_usd = nasiko_evals::cost::check_priced(&routed)
                .ok()
                .map(|_| routed.cost_usd);
        }
        record.original_cost_usd = original_costs.get(&attempt.task_id).copied().flatten();
        if attempt.record.is_some() {
            store::save_attempt_record(&state.db, attempt.id, &record).await?;
        }
        records.push(record);
    }
    let control = control_model(state, &run).await?;
    let summary = report::summarise(&records, control.as_deref().unwrap_or(""));
    let label = format!("{} tasks", dataset_tasks.len());
    let headlines: BTreeMap<String, String> = summary
        .iter()
        .map(|m| {
            (
                m.model.clone(),
                report::headline(m, control.as_deref(), &label),
            )
        })
        .collect();
    let report = json!({
        "summary": summary,
        "headlines": headlines,
        "control": control,
        "prompt_version": nasiko_evals::prompt::PROMPT_VERSION,
        "opencode_version": nasiko_evals::images::opencode_version(),
    });
    store::save_report(&state.db, run_id, &report).await?;
    let _ = state
        .batch_runner
        .cleanup(Some(&format!("eval-run-{run_id}")))
        .await;
    Ok(())
}

/// The run model that is the dataset's original model, re-run in the same harness: the fair
/// comparison. `None` when no run model serves the model the sessions mostly used.
async fn control_model(state: &AppState, run: &store::RunRow) -> Result<Option<String>> {
    let originals: Vec<String> = store::tasks_of(&state.db, run.dataset_id)
        .await?
        .iter()
        .map(|t| t.instance.0.x_nasiko.orig.model.clone())
        .collect();
    Ok(run
        .models
        .0
        .iter()
        .find(|m| {
            m.model
                .as_deref()
                .is_some_and(|model| originals.iter().any(|o| o == model))
        })
        .map(|m| m.name.clone()))
}

/// The record of an attempt that never produced one (its job was given up).
fn failed_record(models: &[RunModel], attempt: &AttemptRow) -> AttemptRecord {
    let task = &attempt.instance.0;
    AttemptRecord {
        instance_id: task.instance_id.clone(),
        model: model_name(models, attempt.model_id),
        attempt: attempt.attempt_no as u32,
        difficulty: task.difficulty,
        grade: Grade {
            resolution: Resolution::No,
            resolved: false,
            f2p_passed: 0,
            f2p_total: task.fail_to_pass.len() as u32,
            p2p_passed: 0,
            p2p_total: task.pass_to_pass.len() as u32,
            failure: Some(FailureClass::InfraError),
        },
        usage: None,
        cost_usd: None,
        worst_price: None,
        wall_secs: 0,
        original_cost_usd: None,
    }
}

/// The original session's cost for the task, priced by the router's engine; `None` when any of
/// its models has no real price.
pub async fn original_cost_of(state: &AppState, source: &nasiko_evals::TaskSource) -> Option<f64> {
    let mut total = 0.0;
    for (model, usage) in &source.usage_by_model {
        let priced = super::price::price(
            &state.pricing,
            super::price::PriceRequest {
                provider: None,
                model: model.clone(),
                input_tokens: usage.input,
                output_tokens: usage.output,
                cache_read_tokens: usage.cache_read,
                cache_creation_tokens: usage.cache_write,
            },
        )
        .await
        .ok()?;
        if priced.source == "Default" {
            return None;
        }
        total += priced.cost_usd;
    }
    Some(total)
}

/// The router's totals for one tag, as the report reads them.
pub fn router_usage(u: &AttemptUsage) -> RouterUsage {
    let as_u64 = |n: i64| u64::try_from(n).unwrap_or(0);
    RouterUsage {
        calls: u.calls as u32,
        usage: TokenUsage {
            input: as_u64(u.input_tokens),
            output: as_u64(u.output_tokens),
            cache_write: as_u64(u.cache_creation_tokens),
            cache_read: as_u64(u.cache_read_tokens),
            reasoning: 0,
        },
        cost_usd: u.cost_usd,
        models: u.models.clone(),
        worst_price: serde_json::from_value(json!(u.worst_price_source))
            .unwrap_or(PriceQuality::Default),
        estimated_calls: u.estimated_calls as u32,
    }
}

/// The score a comparison cell shows, 0-100: the share of `FAIL_TO_PASS` tests the attempt
/// fixed, and 0 when it broke any `PASS_TO_PASS` test (a fix that breaks other code is no fix).
pub fn score(grade: &Grade) -> u32 {
    if grade.p2p_passed < grade.p2p_total || grade.f2p_total == 0 {
        return if grade.resolved { 100 } else { 0 };
    }
    (100 * grade.f2p_passed / grade.f2p_total).min(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grade(f2p: (u32, u32), p2p: (u32, u32), resolved: bool) -> Grade {
        Grade {
            resolution: if resolved {
                Resolution::Full
            } else {
                Resolution::No
            },
            resolved,
            f2p_passed: f2p.0,
            f2p_total: f2p.1,
            p2p_passed: p2p.0,
            p2p_total: p2p.1,
            failure: None,
        }
    }

    #[test]
    fn scores_are_the_fixed_share_and_zero_after_a_regression() {
        assert_eq!(score(&grade((2, 2), (958, 958), true)), 100);
        assert_eq!(score(&grade((1, 4), (10, 10), false)), 25);
        assert_eq!(score(&grade((4, 4), (9, 10), false)), 0);
        assert_eq!(score(&grade((0, 0), (0, 0), false)), 0);
    }

    #[test]
    fn router_totals_become_report_usage() {
        let u = AttemptUsage {
            attempt: "r:a".into(),
            calls: 3,
            failed_calls: 0,
            models: vec!["m".into()],
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 5,
            cache_creation_tokens: 1,
            cost_usd: 0.5,
            worst_price_source: "Family".into(),
            estimated_calls: 0,
        };
        let r = router_usage(&u);
        assert_eq!(r.usage.cache_write, 1);
        assert_eq!(r.worst_price, PriceQuality::Family);
        let unknown = AttemptUsage {
            worst_price_source: "Nonsense".into(),
            ..u
        };
        assert_eq!(router_usage(&unknown).worst_price, PriceQuality::Default);
    }
}
