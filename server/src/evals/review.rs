//! Prompt P1 (rewrite the session's request as an issue) and prompt P3 (screen the task the way
//! SWE-bench Verified did), run on the caller's own LLM configs through the router, and a
//! person's verdict for tasks no reviewer could judge.

use anyhow::{Context, Result};
use nasiko_evals::prompt::{self, ChatPrompt, Distilled, ScreenVerdict};
use nasiko_evals::screen::{self, ScreenDecision};
use nasiko_evals::{TaskInstance, TaskStatus};
use serde_json::Value;
use uuid::Uuid;

use super::jobs::{Job, JobKind};
use super::steps::{need, next_task_job};
use super::store::{self, TaskRow};
use crate::state::AppState;

/// The reviewer name a person's verdict is recorded under: it overrides the models'.
pub fn human_reviewer(username: &str) -> String {
    format!("human:{username}")
}

/// Whether a person may give the task a verdict: any task the gate proved reproducible, the ones
/// the reviewer models kept or rejected included, since a person's verdict overrides theirs.
pub fn reviewable(row: &TaskRow) -> bool {
    match row.instance.0.x_nasiko.status {
        TaskStatus::Reproducible | TaskStatus::Verified => true,
        TaskStatus::Rejected => passed_gate(row.gate.as_ref().map(|g| &g.0)),
        TaskStatus::Candidate => false,
    }
}

/// Whether the saved gate result (`{"verdict": GateVerdict, "runs": …}`) is reproducible.
fn passed_gate(gate: Option<&Value>) -> bool {
    gate.and_then(|g| g.pointer("/verdict/verdict"))
        .and_then(Value::as_str)
        == Some("reproducible")
}

/// The reviewer name a model's verdict is recorded under: its LLM config.
pub fn config_reviewer(config_id: Uuid) -> String {
    format!("config:{config_id}")
}

pub async fn rewrite(state: &AppState, job: &Job) -> Result<()> {
    let row = load(state, job).await?;
    let dataset = store::dataset(&state.db, row.dataset_id)
        .await?
        .context("dataset gone")?;
    let mut task = row.instance.0.clone();
    if task.x_nasiko.status != TaskStatus::Candidate {
        return Ok(());
    }
    if let Some(config) = dataset.rewriter_config_id {
        let request = prompt::distill_prompt(&row.repo, &row.source.0);
        let tag = format!("build-{}:p1:{}", row.dataset_id, row.id);
        let parsed = ask(
            state,
            job.user_id,
            config,
            &tag,
            &request,
            prompt::parse_distill_reply,
        )
        .await?;
        apply_distilled(&mut task, parsed);
    }
    let next = if task.x_nasiko.status == TaskStatus::Rejected {
        "done"
    } else {
        "build"
    };
    store::save_task(&state.db, row.id, &task, next).await?;
    if next == "build" {
        next_task_job(state, job, JobKind::TaskBuild).await?;
    }
    Ok(())
}

fn apply_distilled(task: &mut TaskInstance, parsed: Distilled) {
    match parsed {
        Distilled::Ok {
            title,
            statement,
            hints,
        } => {
            task.problem_statement = if title.is_empty() {
                statement
            } else {
                format!("{title}\n\n{statement}")
            };
            task.hints_text = hints;
            task.x_nasiko.statement_rewritten = true;
        }
        Distilled::Reject(reason) => {
            task.x_nasiko.status = TaskStatus::Rejected;
            task.x_nasiko.reject_reason = Some(format!("p1: {reason}"));
        }
    }
}

pub async fn screen(state: &AppState, job: &Job) -> Result<()> {
    let row = load(state, job).await?;
    let dataset = store::dataset(&state.db, row.dataset_id)
        .await?
        .context("dataset gone")?;
    if row.instance.0.x_nasiko.status != TaskStatus::Reproducible {
        return Ok(());
    }
    let mut reviews = row.reviews.0.clone();
    for config in &dataset.reviewer_config_ids {
        let reviewer = config_reviewer(*config);
        if reviews.iter().any(|r| r.reviewer == reviewer) {
            continue;
        }
        let request = prompt::screen_prompt(&row.instance.0);
        let tag = format!("build-{}:p3:{}:{config}", row.dataset_id, row.id);
        let parse = |reply: &str| prompt::parse_screen_reply(&reviewer, reply);
        match ask(state, job.user_id, *config, &tag, &request, parse).await {
            Ok(verdict) => {
                reviews.push(verdict);
                store::save_reviews(&state.db, row.id, &reviews).await?;
            }
            // A reviewer that cannot answer leaves the task to the others, or to a person.
            Err(e) => {
                tracing::warn!(task = %row.id, %reviewer, error = %e, "evals: a reviewer could not screen a task")
            }
        }
    }
    decide(state, &row, &reviews).await
}

/// Records a person's verdict on a task and decides it again.
pub async fn record_human(
    state: &AppState,
    row: &TaskRow,
    mut verdict: ScreenVerdict,
    username: &str,
) -> Result<()> {
    verdict.reviewer = human_reviewer(username);
    let mut reviews: Vec<ScreenVerdict> = row
        .reviews
        .0
        .iter()
        .filter(|r| r.reviewer != verdict.reviewer)
        .cloned()
        .collect();
    reviews.push(verdict);
    store::save_reviews(&state.db, row.id, &reviews).await?;
    decide(state, row, &reviews).await?;
    store::settle_dataset(&state.db, row.dataset_id).await
}

async fn decide(state: &AppState, row: &TaskRow, reviews: &[ScreenVerdict]) -> Result<()> {
    let mut task = row.instance.0.clone();
    let stage = match screen::decide(reviews) {
        ScreenDecision::Accept { difficulty } => {
            task.difficulty = Some(difficulty);
            task.x_nasiko.status = TaskStatus::Verified;
            task.x_nasiko.reject_reason = None;
            "done"
        }
        ScreenDecision::Reject { reason } => {
            task.x_nasiko.status = TaskStatus::Rejected;
            task.x_nasiko.reject_reason = Some(format!("screen: {reason}"));
            "done"
        }
        ScreenDecision::NeedsHuman => "review",
    };
    store::save_task(&state.db, row.id, &task, stage).await
}

async fn load(state: &AppState, job: &Job) -> Result<TaskRow> {
    store::task(&state.db, need(job.task_id)?)
        .await?
        .context("task gone")
}

/// One completion parsed by `parse`, asked once more when the reply does not parse: small
/// models sometimes wrap or break their JSON.
async fn ask<T, E: std::error::Error + Send + Sync + 'static>(
    state: &AppState,
    user_id: Uuid,
    config: Uuid,
    tag: &str,
    request: &ChatPrompt,
    parse: impl Fn(&str) -> std::result::Result<T, E>,
) -> Result<T> {
    let reply = super::llm::complete(state, user_id, config, tag, request).await?;
    match parse(&reply) {
        Ok(parsed) => Ok(parsed),
        Err(_) => {
            let again = super::llm::complete(state, user_id, config, tag, request).await?;
            Ok(parse(&again)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_evals::gate::{GateRejection, GateVerdict, TestLists};
    use serde_json::json;

    fn saved(verdict: GateVerdict) -> Value {
        json!({"verdict": verdict, "runs": []})
    }

    #[test]
    fn only_a_gate_that_found_the_task_reproducible_counts_as_passed() {
        let lists = TestLists {
            fail_to_pass: vec!["t".into()],
            pass_to_pass: vec![],
            flaky: vec![],
        };
        assert!(passed_gate(Some(&saved(GateVerdict::Reproducible(lists)))));
        let rejected = GateVerdict::Rejected {
            reason: GateRejection::NoFailToPass,
        };
        assert!(!passed_gate(Some(&saved(rejected))));
        assert!(!passed_gate(None));
    }
}
