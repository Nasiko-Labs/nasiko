//! Workflows the server installs at boot, for every user to run.
//!
//! The workflow counterpart of `SEED_AGENTS` (`crate::seed`), and the same
//! flow: `SEED_WORKFLOWS` names templates (space-separated ids from
//! [`TEMPLATES`]), read once at startup in the background task that seeds the
//! agents, right after them. Each template becomes one `mafs` row owned by the
//! first superuser and marked `is_public`, so every user lists and runs it,
//! each run recorded as theirs, while only its owner may edit or delete it.
//!
//! Every step names the seed agent it runs on and the version it was written
//! for. The seeder finds that agent among the superuser's own rows — the ones
//! `SEED_AGENTS` registers — and stores its id, so a template whose agent is
//! not seeded is skipped with a warning rather than failing startup.
//!
//! Installing is idempotent, keyed by `mafs.seed_template` (unique):
//! - no row yet → insert it;
//! - a row at an older version → update it in place, keeping its id and runs;
//! - a row at this version whose steps still point at the live seed agents →
//!   leave it; a seed agent re-registered under a new id is repointed;
//! - a row someone deleted → leave it deleted. Removing a template from
//!   `SEED_WORKFLOWS` is how to stop installing it.

use nasiko_orchestrator::maf::types::{MafDefinition, MafStep};
use tracing::{info, warn};
use uuid::Uuid;

use crate::state::AppState;

/// One workflow the server can install.
pub struct WorkflowTemplate {
    /// What `SEED_WORKFLOWS` names it by; stored as `mafs.seed_template`.
    pub id: &'static str,
    /// Raise it whenever anything below changes, so servers already running
    /// the template update their row on the next start.
    pub version: i32,
    pub name: &'static str,
    pub description: &'static str,
    pub steps: &'static [TemplateStep],
}

/// One step of a template: which seed agent runs it, and what it should do.
pub struct TemplateStep {
    /// The seed agent's name: its image's last path segment without the tag.
    pub agent: &'static str,
    /// The agent version the step was written against. A seeded agent at
    /// another version still gets the step, with a warning: seed agents are
    /// upgraded on their own schedule, and a template must not vanish because
    /// one of its agents moved on.
    pub agent_version: &'static str,
    pub task: &'static str,
}

/// Every template this server knows. Ids must be unique.
pub const TEMPLATES: &[WorkflowTemplate] = &[
    // Showcase workflows: a run needs no input (the dashboard's Run sends none), so each
    // carries its own sample data in its first step, and its description says so.
    WorkflowTemplate {
        id: "chief-of-staff-follow-up",
        version: 1,
        name: "Chief of Staff: Meeting Follow-up",
        description: "Turns a meeting into decisions, owners and a ready-to-send \
                      follow-up email. Runs on a built-in sample meeting.",
        steps: &[
            TemplateStep {
                agent: "meeting-notes-processor",
                agent_version: "1.1.1",
                task: "Read this sample meeting transcript and pull out the decisions, \
                       the action items with owner and due date, and the open \
                       questions. Transcript: \"Weekly launch sync, 12 Nov. Attendees: \
                       Priya (PM), Rahul (Eng), Anil (Marketing). Priya: We agreed to \
                       ship the new onboarding flow on 20 Nov. Rahul: I'll fix the CSV \
                       export bug by 18 Nov. Anil: I'll have the launch blog post ready \
                       by 17 Nov. Open question: do we launch the mobile app now? \
                       Decision: the mobile app moves to December. Priya: Rahul, please \
                       also add usage alerts before launch.\"",
            },
            TemplateStep {
                agent: "task-breakdown",
                agent_version: "1.1.1",
                task: "Turn the action items into an ordered task list for the launch, \
                       keeping each owner and due date, and flag anything due before \
                       18 Nov as urgent.",
            },
            TemplateStep {
                agent: "reply-drafter",
                agent_version: "1.1.1",
                task: "Draft the follow-up email the meeting organiser sends to Priya, \
                       Rahul and Anil: thank them, confirm the decisions, list each task \
                       with owner and due date, and note that the mobile app moved to \
                       December. It goes out by email.",
            },
            TemplateStep {
                agent: "humanizer",
                agent_version: "1.1.1",
                task: "Rewrite the email so it reads warm and natural, without changing \
                       any decision, owner or date.",
            },
        ],
    },
    WorkflowTemplate {
        id: "voice-of-customer-roadmap",
        version: 1,
        name: "Voice of Customer \u{2192} Roadmap",
        description: "Turns raw customer feedback into ranked themes, roadmap tasks and \
                      a reply to the customer in their own language. Runs on built-in \
                      sample feedback.",
        steps: &[
            TemplateStep {
                agent: "summarizer",
                agent_version: "1.1.1",
                task: "Group this sample customer feedback into themes, ranked by how \
                       many customers raised each, with one short quote per theme. \
                       Feedback: \"1. Acme (500 users): Monthly reports download as \
                       blank PDFs, our finance team is blocked. 2. Brightly: I can't \
                       find where to add teammates. 3. Acme: Blank PDF reports again \
                       this month, please fix. 4. Zento: Love the dashboard, but \
                       exports are slow. 5. Lumen (Spain): Los informes en PDF salen en \
                       blanco, necesitamos una soluci\u{f3}n urgente. 6. Brightly: \
                       Adding teammates is confusing.\"",
            },
            TemplateStep {
                agent: "task-breakdown",
                agent_version: "1.1.1",
                task: "Turn the top themes into prioritised roadmap tasks, highest \
                       customer impact first, each with a one-line reason.",
            },
            TemplateStep {
                agent: "reply-drafter",
                agent_version: "1.1.1",
                task: "Draft a reply to Lumen, the Spanish customer whose PDF reports \
                       come out blank: apologise, say the fix is planned, and promise \
                       an update by 20 Nov. It goes out by email.",
            },
            TemplateStep {
                agent: "humanizer",
                agent_version: "1.1.1",
                task: "Make the reply sound natural and caring, keeping the commitment \
                       and the date.",
            },
            TemplateStep {
                agent: "translator",
                agent_version: "1.1.1",
                task: "Translate the reply into Spanish for Lumen, keeping its tone.",
            },
        ],
    },
];

/// What seeding did with one template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeedOutcome {
    Installed,
    Updated,
    Unchanged,
    /// Not installed or touched, and why.
    Skipped(String),
}

/// Install every template `SEED_WORKFLOWS` names.
/// Designed to run as a background task — does not block server startup.
pub async fn seed_workflows_if_configured(state: &AppState) {
    let ids: Vec<&str> = state.config.seed_workflows.split_whitespace().collect();
    if ids.is_empty() {
        info!("SEED_WORKFLOWS not set, skipping workflow seeding");
        return;
    }
    info!(templates = ?ids, "SEED_WORKFLOWS configured, installing");
    seed_workflows(state, &ids).await;
}

/// Install the templates named by `ids`, reporting what happened to each.
pub async fn seed_workflows(state: &AppState, ids: &[&str]) -> Vec<(String, SeedOutcome)> {
    let Some(owner_id) = first_superuser(&state.db).await else {
        warn!("no admin user found, cannot seed workflows (run bootstrap first)");
        return Vec::new();
    };
    let mut outcomes = Vec::with_capacity(ids.len());
    for &id in ids {
        let outcome = match template(id) {
            Some(template) => seed_one(state, owner_id, template).await,
            None => SeedOutcome::Skipped("no template with this id".into()),
        };
        match &outcome {
            SeedOutcome::Skipped(reason) => warn!(template = id, %reason, "seed workflow skipped"),
            other => info!(template = id, outcome = ?other, "seed workflow checked"),
        }
        outcomes.push((id.to_owned(), outcome));
    }
    outcomes
}

pub fn template(id: &str) -> Option<&'static WorkflowTemplate> {
    TEMPLATES.iter().find(|t| t.id == id)
}

async fn first_superuser(db: &sqlx::PgPool) -> Option<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM users WHERE is_superuser = true AND deleted_at IS NULL ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
}

/// The template's row as it stands, if it has one.
#[derive(sqlx::FromRow)]
struct InstalledRow {
    seed_version: Option<i32>,
    status: String,
    maf_json: String,
}

async fn seed_one(state: &AppState, owner_id: Uuid, template: &WorkflowTemplate) -> SeedOutcome {
    let installed = match sqlx::query_as::<_, InstalledRow>(
        "SELECT seed_version, status, maf_json::text AS maf_json FROM mafs WHERE seed_template = $1",
    )
    .bind(template.id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(row) => row,
        Err(e) => return SeedOutcome::Skipped(format!("could not read its row: {e}")),
    };
    if installed
        .as_ref()
        .is_some_and(|row| row.status == "deleted")
    {
        return SeedOutcome::Unchanged;
    }

    let steps = match resolve_steps(&state.db, owner_id, template).await {
        Ok(steps) => steps,
        Err(reason) => return SeedOutcome::Skipped(reason),
    };
    if let Some(row) = &installed
        && is_current(row, template.version, &steps)
    {
        return SeedOutcome::Unchanged;
    }

    let definition = MafDefinition {
        // Generated by the runtime planner on each execution, as for any workflow.
        description: None,
        steps,
        output_generation: None,
    };
    let definition = match state
        .maf_policy
        .finalize(&state.db, owner_id, definition, None)
        .await
    {
        Ok(definition) => definition,
        Err(refused) => {
            return SeedOutcome::Skipped(format!(
                "the workflow policy refused it: {}",
                refused.status_and_message().1
            ));
        }
    };
    let maf_json = serde_json::to_value(&definition).unwrap_or_default();

    // `<=`, not `<`: an equal version is rewritten when its agents moved (see
    // `is_current`), while an older server still running during a rolling
    // deploy can never downgrade a row a newer one already updated.
    let inserted: Result<Option<bool>, sqlx::Error> = sqlx::query_scalar(
        r#"INSERT INTO mafs (user_id, name, description, maf_json, is_public, seed_template, seed_version)
           VALUES ($1, $2, $3, $4, true, $5, $6)
           ON CONFLICT (seed_template) WHERE seed_template IS NOT NULL
           DO UPDATE SET name = EXCLUDED.name, description = EXCLUDED.description,
                         maf_json = EXCLUDED.maf_json, seed_version = EXCLUDED.seed_version,
                         updated_at = now()
              WHERE mafs.status <> 'deleted' AND mafs.seed_version <= EXCLUDED.seed_version
           RETURNING (xmax = 0)"#,
    )
    .bind(owner_id)
    .bind(template.name)
    .bind(template.description)
    .bind(&maf_json)
    .bind(template.id)
    .bind(template.version)
    .fetch_optional(&state.db)
    .await;

    match inserted {
        Ok(Some(true)) => SeedOutcome::Installed,
        Ok(Some(false)) => SeedOutcome::Updated,
        // The conflict's `WHERE` declined the update: another server got there
        // first, or the row was deleted in between.
        Ok(None) => SeedOutcome::Unchanged,
        Err(e) => SeedOutcome::Skipped(format!("could not write its row: {e}")),
    }
}

/// Whether the installed row needs no write: it is at this version or newer,
/// and its steps run on the agents the template resolves to today. A seed
/// agent that was deleted and seeded again has a new id, and a row still
/// pointing at the old one would fail every run.
fn is_current(row: &InstalledRow, version: i32, steps: &[MafStep]) -> bool {
    if row.seed_version.unwrap_or(0) < version {
        return false;
    }
    let Ok(stored) = serde_json::from_str::<MafDefinition>(&row.maf_json) else {
        return false;
    };
    stored
        .steps
        .iter()
        .map(|s| s.agent_id)
        .eq(steps.iter().map(|s| s.agent_id))
}

#[derive(sqlx::FromRow)]
struct SeedAgent {
    id: Uuid,
    url: Option<String>,
    image: Option<String>,
}

/// Each step bound to its seed agent, or why the template cannot be installed.
async fn resolve_steps(
    db: &sqlx::PgPool,
    owner_id: Uuid,
    template: &WorkflowTemplate,
) -> Result<Vec<MafStep>, String> {
    let mut steps = Vec::with_capacity(template.steps.len());
    for (index, step) in template.steps.iter().enumerate() {
        // The seed's own row only, as in `crate::seed`: another user's agent
        // may share the name.
        let agent = sqlx::query_as::<_, SeedAgent>(
            "SELECT id, url, image FROM agents WHERE name = $1 AND owner_id = $2 AND deleted_at IS NULL",
        )
        .bind(step.agent)
        .bind(owner_id)
        .fetch_optional(db)
        .await
        .map_err(|e| format!("could not look up agent '{}': {e}", step.agent))?
        .ok_or_else(|| format!("agent '{}' is not seeded (add it to SEED_AGENTS)", step.agent))?;

        let tag = agent.image.as_deref().and_then(image_tag);
        if !tag.is_some_and(|tag| is_version(tag, step.agent_version)) {
            warn!(
                template = template.id,
                agent = step.agent,
                expected = step.agent_version,
                found = tag.unwrap_or("none"),
                "seed workflow step was written for another version of its agent"
            );
        }

        steps.push(MafStep {
            step_id: Uuid::new_v4(),
            step_index: index as i32,
            agent_id: agent.id,
            agent_name: step.agent.to_owned(),
            // A snapshot only: the executor resolves the live endpoint from
            // `agent_id` on every run, so a seed agent still deploying is fine.
            agent_endpoint: agent.url.unwrap_or_default(),
            task_description: step.task.to_owned(),
            ext: Default::default(),
        });
    }
    Ok(steps)
}

/// The tag of an image reference, ignoring a `host:port` registry and any
/// digest: `registry.nasiko.com/nasiko/summarizer:1.0.1-image` → `1.0.1-image`.
fn image_tag(image: &str) -> Option<&str> {
    let image = image.split('@').next().unwrap_or(image);
    let last_segment = image.rsplit('/').next().unwrap_or(image);
    last_segment.split_once(':').map(|(_, tag)| tag)
}

/// Whether `tag` is `version`, allowing a suffix after a dash: the registry
/// publishes `1.0.1` as `1.0.1-image` too.
fn is_version(tag: &str, version: &str) -> bool {
    tag == version
        || tag
            .strip_prefix(version)
            .is_some_and(|rest| rest.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_ids_are_unique() {
        let mut ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TEMPLATES.len());
    }

    #[test]
    fn every_template_has_steps_and_a_version() {
        for template in TEMPLATES {
            assert!(!template.steps.is_empty(), "{} has no steps", template.id);
            assert!(template.version > 0, "{} needs a version", template.id);
        }
    }

    #[test]
    fn templates_are_found_by_id() {
        assert_eq!(
            template("chief-of-staff-follow-up").map(|t| t.id),
            Some("chief-of-staff-follow-up")
        );
        assert!(template("no-such-template").is_none());
    }

    #[test]
    fn the_tag_ignores_the_registry_port_and_digest() {
        assert_eq!(
            image_tag("registry.nasiko.com/nasiko/summarizer:1.0.1-image"),
            Some("1.0.1-image")
        );
        assert_eq!(
            image_tag("localhost:5000/nasiko/summarizer:1.0.0"),
            Some("1.0.0")
        );
        assert_eq!(
            image_tag("nasiko/summarizer:1.0.0@sha256:abc"),
            Some("1.0.0")
        );
        assert_eq!(image_tag("localhost:5000/nasiko/summarizer"), None);
    }

    #[test]
    fn a_dash_suffix_is_still_the_same_version() {
        assert!(is_version("1.0.0", "1.0.0"));
        assert!(is_version("1.0.0-image", "1.0.0"));
        assert!(!is_version("1.0.1-image", "1.0.0"));
        assert!(!is_version("1.0.01", "1.0.0"));
    }
}
