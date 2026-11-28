//! Building the dataset after `build`: `rewrite` (prompt P1), `screen` (prompt P3 + the
//! Verified keep/drop rule) and `review` (a person, only for tasks no reviewer could judge).
//!
//! Every model call goes through the Nasiko router tagged `build:<step>:<task>`, so the router
//! also reports what building the dataset cost (`nasiko eval report build`).

use anyhow::{Context, Result, bail};
use nasiko_evals::prompt::{self, Distilled, ScreenVerdict};
use nasiko_evals::screen::{self, ScreenDecision};
use nasiko_evals::{Difficulty, TaskInstance, TaskSource, TaskStatus};

use super::llm::LlmClient;
use super::router::RouterClient;
use super::store::{EvalConfig, ModelEntry, Store};

/// Attempt-tag prefix of dataset-building calls.
pub const BUILD_RUN: &str = "build";

fn model<'a>(config: &'a EvalConfig, short: &str) -> Result<&'a ModelEntry> {
    config
        .models
        .get(short)
        .with_context(|| format!("model {short:?} is not under \"models\" in config.json"))
}

/// What happened to one task.
pub struct StepResult {
    pub instance_id: String,
    pub outcome: String,
}

/// `nasiko eval rewrite`: rewrites each candidate's request as an issue (prompt P1).
pub fn rewrite(store: &Store, router: &RouterClient, only: &[String]) -> Result<Vec<StepResult>> {
    let config = store.config()?;
    let short = config
        .rewriter
        .clone()
        .context("set \"rewriter\" in config.json to the model that rewrites requests")?;
    let llm = LlmClient::pinned(router, &short, model(&config, &short)?)?;
    let mut results = Vec::new();
    for mut task in store.tasks()? {
        let wanted = if only.is_empty() {
            !task.x_nasiko.statement_rewritten && task.x_nasiko.status != TaskStatus::Rejected
        } else {
            only.contains(&task.instance_id)
        };
        if !wanted {
            continue;
        }
        let outcome = match rewrite_one(store, &llm, &mut task) {
            Ok(o) => {
                store.save_task(&task)?;
                o
            }
            Err(e) => format!("error: {e:#}"),
        };
        results.push(StepResult {
            instance_id: task.instance_id.clone(),
            outcome,
        });
    }
    Ok(results)
}

fn rewrite_one(store: &Store, llm: &LlmClient, task: &mut TaskInstance) -> Result<String> {
    let path = store.layout().source_file(&task.instance_id);
    let source: TaskSource = serde_json::from_str(
        &std::fs::read_to_string(&path).with_context(|| format!("no {}", path.display()))?,
    )?;
    let request = prompt::distill_prompt(&task.repo, &source);
    let tag = format!("{BUILD_RUN}:p1:{}", task.instance_id);
    let reply = llm.complete(&request, &tag)?;
    let parsed = match prompt::parse_distill_reply(&reply) {
        Ok(p) => p,
        // One retry: small local models sometimes wrap or break the JSON.
        Err(_) => prompt::parse_distill_reply(&llm.complete(&request, &tag)?)?,
    };
    apply_distilled(task, &source, parsed)
}

/// Puts prompt P1's answer on the task. A statement that leaks what its commit message says
/// about the fix is an error and leaves the task as it was: still unrewritten, which keeps it
/// out of `screen` and `freeze`.
fn apply_distilled(
    task: &mut TaskInstance,
    source: &TaskSource,
    parsed: Distilled,
) -> Result<String> {
    Ok(match parsed {
        Distilled::Ok {
            title,
            statement,
            hints,
        } => {
            let statement = if title.is_empty() {
                statement
            } else {
                format!("{title}\n\n{statement}")
            };
            if let Some(leak) = prompt::distilled_statement_leak(source, &statement) {
                bail!("not rewritten: {leak}; run rewrite on it again, or reject it");
            }
            task.problem_statement = statement;
            task.hints_text = hints;
            task.x_nasiko.statement_rewritten = true;
            "rewritten".to_string()
        }
        Distilled::Reject(reason) => {
            task.x_nasiko.status = TaskStatus::Rejected;
            task.x_nasiko.reject_reason = Some(format!("p1: {reason}"));
            format!("rejected: {reason}")
        }
    })
}

/// `nasiko eval screen`: every reviewer model judges every reproducible task (prompt P3); then
/// the keep/drop rule decides. A task whose statement is still a raw commit message waits for
/// `rewrite`: a verdict must be on the statement a model will be shown.
pub fn screen(store: &Store, router: &RouterClient) -> Result<Vec<StepResult>> {
    let config = store.config()?;
    let tasks: Vec<TaskInstance> = store
        .tasks()?
        .into_iter()
        .filter(|t| t.x_nasiko.status == TaskStatus::Reproducible && !t.x_nasiko.needs_rewrite())
        .collect();
    let mut errors: Vec<StepResult> = Vec::new();
    for short in &config.reviewers {
        let llm = LlmClient::pinned(router, short, model(&config, short)?)?;
        for task in &tasks {
            let file = store.layout().review_file(&task.instance_id, short);
            if file.exists() {
                continue;
            }
            match review_with_model(&llm, short, task) {
                Ok(v) => store.save_json(&file, &v)?,
                Err(e) => errors.push(StepResult {
                    instance_id: task.instance_id.clone(),
                    outcome: format!("{short} could not review: {e:#}"),
                }),
            }
        }
    }
    let mut results = errors;
    for task in tasks {
        let instance_id = task.instance_id.clone();
        results.push(StepResult {
            outcome: apply_decision(store, task)?,
            instance_id,
        });
    }
    Ok(results)
}

fn review_with_model(llm: &LlmClient, short: &str, task: &TaskInstance) -> Result<ScreenVerdict> {
    let request = prompt::screen_prompt(task);
    let tag = format!("{BUILD_RUN}:p3:{}:{short}", task.instance_id);
    let reply = llm.complete(&request, &tag)?;
    match prompt::parse_screen_reply(short, &reply) {
        Ok(v) => Ok(v),
        Err(_) => Ok(prompt::parse_screen_reply(
            short,
            &llm.complete(&request, &tag)?,
        )?),
    }
}

/// Re-decides a task from all its reviews and saves the result. Returns the outcome in words.
pub fn apply_decision(store: &Store, mut task: TaskInstance) -> Result<String> {
    let reviews = store.load_reviews(&task.instance_id)?;
    let decision = screen::decide(&reviews);
    let line = match &decision {
        ScreenDecision::Accept { difficulty } => {
            task.difficulty = Some(*difficulty);
            task.x_nasiko.status = TaskStatus::Verified;
            task.x_nasiko.reject_reason = None;
            format!("verified ({})", screen::level(*difficulty))
        }
        ScreenDecision::Reject { reason } => {
            task.x_nasiko.status = TaskStatus::Rejected;
            task.x_nasiko.reject_reason = Some(format!("screen: {reason}"));
            format!("rejected: {reason}")
        }
        ScreenDecision::NeedsHuman => "needs a person: run `nasiko eval review`".to_string(),
    };
    store.save_task(&task)?;
    Ok(line)
}

/// A person's scores for one task.
pub struct HumanScores {
    pub reviewer: String,
    pub clarity: u8,
    pub test_validity: u8,
    pub effort: Difficulty,
    pub problems: Vec<String>,
}

/// Records a person's review and re-decides the task.
pub fn record_human_review(
    store: &Store,
    task: TaskInstance,
    scores: HumanScores,
) -> Result<String> {
    let reviewer = format!("human:{}", scores.reviewer);
    let verdict = ScreenVerdict {
        reviewer: reviewer.clone(),
        clarity: scores.clarity,
        clarity_reason: String::new(),
        test_validity: scores.test_validity,
        test_validity_reason: String::new(),
        other_problems: scores.problems,
        effort: scores.effort,
        effort_reason: String::new(),
    };
    store.save_json(
        &store.layout().review_file(&task.instance_id, &reviewer),
        &verdict,
    )?;
    apply_decision(store, task)
}

/// Tasks no reviewer could judge: reproducible, with no review at all (and, like `screen`, not
/// still waiting for `rewrite`).
pub fn needing_people(store: &Store) -> Result<Vec<TaskInstance>> {
    let mut out = Vec::new();
    for task in store.tasks()? {
        if task.x_nasiko.status == TaskStatus::Reproducible
            && !task.x_nasiko.needs_rewrite()
            && store.load_reviews(&task.instance_id)?.is_empty()
        {
            out.push(task);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nasiko_evals::EvalLayout;

    fn task(status: TaskStatus) -> TaskInstance {
        let mut t: TaskInstance =
            serde_json::from_str(include_str!("testdata/task.json")).expect("task");
        t.x_nasiko.status = status;
        t
    }

    #[test]
    fn only_unreviewed_tasks_need_people_and_a_person_decides() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = Store::new(EvalLayout::new(dir.path()));
        store
            .save_task(&task(TaskStatus::Reproducible))
            .expect("saves");
        assert_eq!(needing_people(&store).expect("lists").len(), 1);
        let line = record_human_review(
            &store,
            task(TaskStatus::Reproducible),
            HumanScores {
                reviewer: "manu".into(),
                clarity: 0,
                test_validity: 1,
                effort: Difficulty::Min15To1H,
                problems: vec![],
            },
        )
        .expect("records");
        assert!(line.contains("verified (medium)"), "{line}");
        assert!(needing_people(&store).expect("lists").is_empty());
        let saved = &store.tasks().expect("loads")[0];
        assert_eq!(saved.x_nasiko.status, TaskStatus::Verified);
    }

    #[test]
    fn a_rewrite_that_repeats_the_commit_message_leaves_the_task_unrewritten() {
        const MESSAGE: &str = "Fix panic in split_off\n\nClamp the index in split_off so that at == len returns an empty buffer.";
        let at = "2026-11-01T10:30:00Z";
        let source: TaskSource = serde_json::from_value(serde_json::json!({
            "session_id": "1a2b3c4d",
            "source": "commit-history",
            "git_branch": null,
            "started_at": at,
            "ended_at": at,
            "task_messages": [{"at": at, "text": MESSAGE}],
            "context_messages": [{"at": at, "text": MESSAGE}],
            "commit_subjects": ["Fix panic in split_off"],
            "changed_files": ["src/bytes_mut.rs"],
            "usage_by_model": [],
        }))
        .expect("source");
        let mut from_commit = task(TaskStatus::Candidate);
        from_commit.x_nasiko.source = nasiko_evals::SessionSource::CommitHistory;
        from_commit.problem_statement = MESSAGE.into();
        let answer = |title: &str, statement: &str| Distilled::Ok {
            title: title.into(),
            statement: statement.into(),
            hints: String::new(),
        };

        let (subject, body) = MESSAGE.split_once("\n\n").expect("subject and body");
        let echoed = apply_distilled(&mut from_commit, &source, answer(subject, body));
        let error = echoed.expect_err("an echo is not a rewrite").to_string();
        assert!(error.contains("repeats the commit message"), "{error}");
        assert!(from_commit.x_nasiko.needs_rewrite());
        assert_eq!(from_commit.problem_statement, MESSAGE);

        let rewritten = answer(
            "split_off panics at the end of the buffer",
            "Calling split_off(len) panics. It should return an empty buffer.",
        );
        let line = apply_distilled(&mut from_commit, &source, rewritten).expect("applies");
        assert_eq!(line, "rewritten");
        assert!(!from_commit.x_nasiko.needs_rewrite());
        assert!(
            from_commit
                .problem_statement
                .starts_with("split_off panics")
        );
    }
}
