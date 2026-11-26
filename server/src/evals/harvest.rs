//! The harvest job: each picked session → its commit on GitHub → a candidate task.
//!
//! The linking, splitting and drafting are `nasiko_evals::harvest`'s, shared with the CLI;
//! this module only reads the session from the platform's telemetry and the repository from
//! GitHub. What happened to each session is kept on the dataset (`outcomes`), so the person
//! sees why a session gave no task.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use nasiko_evals::harvest::{self, FileVersions, Linked, Outcome};
use nasiko_evals::link;
use nasiko_evals::{TaskSource, TaskStatus};
use serde_json::json;

use super::forge::Forge;
use super::jobs::{Job, JobKind, NewJob, enqueue, notify};
use super::sessions::{self, PlacedSession};
use super::steps::need;
use super::store::{self, Dataset, NewTask};
use crate::state::AppState;

/// Largest fix plus tests a task may have: what a batch job's inputs can carry everywhere.
const MAX_PATCH_BYTES: usize = 900 * 1024;

pub async fn run(state: &AppState, job: &Job) -> Result<()> {
    let dataset = store::dataset(&state.db, need(job.dataset_id)?)
        .await?
        .context("dataset gone")?;
    let mut outcomes = BTreeMap::new();
    let mut forges: BTreeMap<String, Option<Forge>> = BTreeMap::new();
    let mut tasks = 0;
    for session_id in &dataset.session_ids {
        let outcome = match harvest_session(state, &dataset, session_id, &mut forges).await {
            Ok(task_ids) => {
                tasks += task_ids.len();
                for task_id in task_ids {
                    enqueue_first_step(state, &dataset, task_id).await?;
                }
                "task".to_string()
            }
            Err(SessionOutcome::Skipped(code)) => code,
            Err(SessionOutcome::Failed(e)) => {
                tracing::warn!(dataset = %dataset.id, %session_id, error = %e, "evals: a session could not be harvested");
                "unreadable".to_string()
            }
        };
        outcomes.insert(session_id.clone(), outcome);
    }
    store::set_outcomes(&state.db, dataset.id, &outcomes).await?;
    let status = if tasks == 0 { "ready" } else { "preparing" };
    store::set_dataset_status(&state.db, dataset.id, status, None).await?;
    Ok(())
}

/// Why a session gave no task: a known reason (its code), or an error.
enum SessionOutcome {
    Skipped(String),
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for SessionOutcome {
    fn from(e: anyhow::Error) -> Self {
        SessionOutcome::Failed(e)
    }
}

fn skipped(code: &str) -> SessionOutcome {
    SessionOutcome::Skipped(code.to_string())
}

/// The session's tasks (several for a long session), inserted; or why it gave none.
async fn harvest_session(
    state: &AppState,
    dataset: &Dataset,
    session_id: &str,
    forges: &mut BTreeMap<String, Option<Forge>>,
) -> std::result::Result<Vec<uuid::Uuid>, SessionOutcome> {
    let db = &state.db;
    let user = dataset.user_id;
    let placed = sessions::placed(db, user, &dataset.harness, session_id)
        .await?
        .ok_or_else(|| skipped("not_found"))?;
    let repo = placed.repo().ok_or_else(|| skipped("no_repo"))?;
    if placed.repo_host.as_deref() != Some("github.com") {
        return Err(skipped("not_github"));
    }
    let session = sessions::record(db, user, &placed)
        .await?
        .ok_or_else(|| skipped("no_chat_history"))?;
    let roots = placed.roots();
    let edited = harvest::precheck(&session, &roots).map_err(|o| skipped(o.label()))?;
    if !forges.contains_key(&repo) {
        let opened = match Forge::open(state, user, &repo).await? {
            Ok(forge) => Some(forge),
            Err(refusal) => return Err(skipped(refusal.code())),
        };
        forges.insert(repo.clone(), opened);
    }
    let forge = forges
        .get(&repo)
        .and_then(Option::as_ref)
        .ok_or_else(|| skipped("repo_not_accessible"))?;
    ensure_spec(state, user, forge).await?;

    let mut ids = Vec::new();
    let mut skips = Vec::new();
    for linked in link_session(forge, &session, &placed, &roots, &edited).await? {
        match draft_and_insert(state, dataset, forge, &session, session_id, &linked).await {
            Ok(id) => ids.push(id),
            Err(SessionOutcome::Skipped(code)) => skips.push(code),
            Err(SessionOutcome::Failed(e)) => {
                tracing::warn!(dataset = %dataset.id, %session_id, commit = %linked.last().sha, error = %e, "evals: a task could not be drafted");
                skips.push("unreadable".to_string());
            }
        }
    }
    match skips.into_iter().next() {
        Some(code) if ids.is_empty() => Err(SessionOutcome::Skipped(code)),
        _ => Ok(ids),
    }
}

/// One linked piece of work's task, inserted; or why it is none.
async fn draft_and_insert(
    state: &AppState,
    dataset: &Dataset,
    forge: &Forge,
    session: &nasiko_evals::SessionRecord,
    session_id: &str,
    linked: &Linked,
) -> std::result::Result<uuid::Uuid, SessionOutcome> {
    let (db, user) = (&state.db, dataset.user_id);
    let base = linked.first().parent.clone();
    if base.is_empty() {
        return Err(skipped(Outcome::NoMatch.label()));
    }
    let gold = linked.last().sha.clone();
    let mut split = harvest::split(&forge.diff(&base, &gold).await?).context("unreadable diff")?;
    let mut versions = BTreeMap::new();
    for path in harvest::inline_test_paths(&split) {
        versions.insert(
            path.clone(),
            FileVersions {
                base: forge.file_at(&base, &path).await?,
                gold: forge.file_at(&gold, &path).await?.unwrap_or_default(),
            },
        );
    }
    harvest::split_inline_rust_tests(&mut split, &versions).context("unreadable diff")?;
    if !harvest::has_tests(&split) {
        return Err(skipped(Outcome::NoTests.label()));
    }
    let mut task = harvest::draft_task(&forge.repo, linked, split);
    if task.patch.len() + task.test_patch.len() > MAX_PATCH_BYTES {
        return Err(skipped("too_large"));
    }
    let commits: Vec<&nasiko_evals::CommitFacts> = linked.commits.iter().collect();
    let source = TaskSource::new(session, &linked.slice, &commits);
    let spec = store::spec(db, user, &forge.repo).await?;
    let stage = if spec.is_none() {
        task.x_nasiko.status = TaskStatus::Rejected;
        task.x_nasiko.reject_reason =
            Some("unsupported: only Cargo (Rust) repositories can be built so far".into());
        "done"
    } else if dataset.rewriter_config_id.is_some() {
        "rewrite"
    } else {
        "build"
    };
    let id = store::insert_task(
        db,
        &NewTask {
            dataset_id: dataset.id,
            repo: &forge.repo,
            source_session_id: session_id,
            stage,
            instance: &task,
            source: &source,
            link: &json!({"decision": linked.decision, "candidates": linked.candidates}),
        },
    )
    .await?;
    Ok(id)
}

/// The commits in the session's window, on every branch it reported and the default one, linked
/// to it: one task, or one per commit for a long session ([`harvest::link_tasks`]).
async fn link_session(
    forge: &Forge,
    session: &nasiko_evals::SessionRecord,
    placed: &PlacedSession,
    roots: &[String],
    edited: &[String],
) -> std::result::Result<Vec<Linked>, SessionOutcome> {
    let branches = placed.branches(&forge.default_branch);
    let until = session.ended_at + link::window_after_end();
    let mut candidates = Vec::new();
    for (sha, on) in forge
        .commits_between(&branches, session.started_at, until)
        .await?
    {
        let facts = forge.commit_facts(&sha, on).await?;
        if harvest::touches_edits(&facts.files, edited) {
            candidates.push(facts);
        }
    }
    harvest::link_tasks(session, roots, candidates).map_err(|o| skipped(o.label()))
}

/// Proposes the repository's build and test settings the first time one of its sessions is
/// harvested; a person may edit them (`PUT /api/evals/specs/...`).
async fn ensure_spec(state: &AppState, user: uuid::Uuid, forge: &Forge) -> Result<()> {
    if store::spec(&state.db, user, &forge.repo).await?.is_some() {
        return Ok(());
    }
    let head = forge.default_branch.as_str();
    let root = forge.root_files(head).await?;
    let mut toolchain = None;
    for name in ["rust-toolchain.toml", "rust-toolchain"] {
        if root.iter().any(|f| f == name) {
            toolchain = forge.file_at(head, name).await?;
            break;
        }
    }
    if let Some(spec) = nasiko_evals::spec::propose(&forge.repo, &root, toolchain.as_deref()) {
        store::save_spec(&state.db, user, &forge.repo, &spec).await?;
    }
    Ok(())
}

async fn enqueue_first_step(
    state: &AppState,
    dataset: &Dataset,
    task_id: uuid::Uuid,
) -> Result<()> {
    let task = store::task(&state.db, task_id)
        .await?
        .context("task gone")?;
    let kind = match task.stage.as_str() {
        "rewrite" => JobKind::TaskRewrite,
        "build" => JobKind::TaskBuild,
        _ => return Ok(()),
    };
    let mut job = NewJob::new(kind, dataset.user_id);
    job.dataset_id = Some(dataset.id);
    job.task_id = Some(task_id);
    enqueue(&state.db, &job).await?;
    notify(state);
    Ok(())
}
