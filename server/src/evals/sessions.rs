//! The caller's coding harnesses and their sessions, as replay evals see them: whether a
//! session can become a task, and the session itself rebuilt from its synced turns.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nasiko_evals::SessionRecord;
use nasiko_evals::session::coding_events::{self, SessionPlace};
use serde::Serialize;
use uuid::Uuid;

/// The integrations replay evals can read sessions of today.
pub const SUPPORTED_HARNESSES: [&str; 1] = ["claude"];
/// The eval runner's own identity is never a harness to evaluate.
const EVAL_INTEGRATION: &str = "eval";

/// One connected coding harness.
#[derive(Debug, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Harness {
    /// The integration id (`claude`, `codex`, `opencode`, `cursor`).
    pub id: String,
    pub sessions: i64,
    pub last_active_at: Option<DateTime<Utc>>,
    /// Whether evals can be built from its sessions yet (Claude Code only, for now).
    #[sqlx(skip)]
    pub supported: bool,
}

pub async fn harnesses(db: &sqlx::PgPool, user_id: Uuid) -> Result<Vec<Harness>> {
    let mut rows: Vec<Harness> = sqlx::query_as(
        r#"SELECT a.coding_agent_integration_id AS id,
                  count(cs.session_id) AS sessions,
                  max(cs.updated_at) AS last_active_at
           FROM agents a
           LEFT JOIN chat_sessions cs
                  ON cs.agent_id = a.id AND cs.user_id = $1
                 AND cs.deleted_at IS NULL AND cs.parent_session_id IS NULL
           WHERE a.owner_id = $1 AND a.deleted_at IS NULL
             AND a.coding_agent_integration_id IS NOT NULL
             AND a.coding_agent_integration_id <> $2
           GROUP BY a.coding_agent_integration_id
           ORDER BY max(cs.updated_at) DESC NULLS LAST"#,
    )
    .bind(user_id)
    .bind(EVAL_INTEGRATION)
    .persistent(false)
    .fetch_all(db)
    .await?;
    for h in &mut rows {
        h.supported = SUPPORTED_HARNESSES.contains(&h.id.as_str());
    }
    Ok(rows)
}

/// Whether the caller has this harness connected.
pub async fn has_harness(db: &sqlx::PgPool, user_id: Uuid, harness: &str) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM agents WHERE owner_id = $1 AND deleted_at IS NULL \
         AND coding_agent_integration_id = $2)",
    )
    .bind(user_id)
    .bind(harness)
    .fetch_one(db)
    .await?)
}

/// Why a session cannot become a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Ineligible {
    /// Synced without chat history: no prompts or edits were kept.
    NoChatHistory,
    /// It edited no files.
    NoEdits,
    /// Synced before the platform recorded sessions' repositories: sync it again.
    NoRepo,
    /// Its repository is not on GitHub.
    NotGithub,
    /// GitHub is not connected.
    GithubNotConnected,
    /// The connected GitHub account cannot read the repository.
    RepoNotAccessible,
    /// Its harness is not supported yet.
    UnsupportedHarness,
}

impl Ineligible {
    pub fn code(self) -> &'static str {
        match self {
            Ineligible::NoChatHistory => "no_chat_history",
            Ineligible::NoEdits => "no_edits",
            Ineligible::NoRepo => "no_repo",
            Ineligible::NotGithub => "not_github",
            Ineligible::GithubNotConnected => "github_not_connected",
            Ineligible::RepoNotAccessible => "repo_not_accessible",
            Ineligible::UnsupportedHarness => "unsupported_harness",
        }
    }
}

/// One session in the harness's list.
#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub started_at: DateTime<Utc>,
    #[serde(skip)]
    pub updated_at: DateTime<Utc>,
    pub turns: i64,
    pub cost_usd: f64,
    pub models: Vec<String>,
    /// `owner/name`, when the session's repository is known.
    pub repo: Option<String>,
    pub git_branch: Option<String>,
    #[serde(skip)]
    pub repo_host: Option<String>,
    #[serde(skip)]
    pub has_content: bool,
    #[serde(skip)]
    pub has_edits: bool,
    #[sqlx(skip)]
    pub eligible: bool,
    #[sqlx(skip)]
    pub ineligible_reason: Option<Ineligible>,
}

/// The harness's newest top-level sessions updated before `before` (all when `None`).
pub async fn sessions(
    db: &sqlx::PgPool,
    user_id: Uuid,
    harness: &str,
    before: Option<DateTime<Utc>>,
    limit: i64,
) -> Result<Vec<SessionSummary>> {
    Ok(sqlx::query_as(
        r#"SELECT cs.session_id, cs.title, cs.created_at AS started_at, cs.updated_at,
                  cs.git_branch, cs.repo_host,
                  CASE WHEN cs.repo_owner IS NOT NULL THEN cs.repo_owner || '/' || cs.repo_name END
                      AS repo,
                  (SELECT count(*) FROM coding_agent_telemetry_events e
                    WHERE e.user_id = cs.user_id AND e.session_id = cs.session_id) AS turns,
                  COALESCE((SELECT bool_or(e.payload->>'capture_policy' = 'content')
                    FROM coding_agent_telemetry_events e
                    WHERE e.user_id = cs.user_id AND e.session_id = cs.session_id), false)
                      AS has_content,
                  EXISTS (SELECT 1 FROM coding_agent_telemetry_events e,
                                 jsonb_array_elements(e.payload->'turn'->'tool_calls') t
                          WHERE e.user_id = cs.user_id AND e.session_id = cs.session_id
                            AND t->>'name' IN ('Edit', 'Write', 'MultiEdit')) AS has_edits,
                  COALESCE((SELECT sum(u.cost_usd) FROM trace_usage u
                    WHERE u.session_id = cs.session_id), 0)::float8 AS cost_usd,
                  COALESCE((SELECT array_agg(DISTINCT u.model) FROM trace_usage u
                    WHERE u.session_id = cs.session_id AND u.model IS NOT NULL), '{}') AS models
           FROM chat_sessions cs
           JOIN agents a ON a.id = cs.agent_id
           WHERE cs.user_id = $1 AND a.coding_agent_integration_id = $2
             AND cs.deleted_at IS NULL AND cs.parent_session_id IS NULL
             AND ($3::timestamptz IS NULL OR cs.updated_at < $3)
           ORDER BY cs.updated_at DESC
           LIMIT $4"#,
    )
    .bind(user_id)
    .bind(harness)
    .bind(before)
    .bind(limit)
    .persistent(false)
    .fetch_all(db)
    .await?)
}

/// Marks each session eligible or not. `readable(repo)` answers whether the caller's GitHub
/// account can read `owner/name` (`None`: GitHub is not connected).
pub fn judge(harness: &str, session: &mut SessionSummary, readable: impl Fn(&str) -> Option<bool>) {
    let reason = if !SUPPORTED_HARNESSES.contains(&harness) {
        Some(Ineligible::UnsupportedHarness)
    } else if !session.has_content {
        Some(Ineligible::NoChatHistory)
    } else if !session.has_edits {
        Some(Ineligible::NoEdits)
    } else if session.repo.is_none() {
        Some(Ineligible::NoRepo)
    } else if session.repo_host.as_deref() != Some("github.com") {
        Some(Ineligible::NotGithub)
    } else {
        match readable(session.repo.as_deref().unwrap_or_default()) {
            None => Some(Ineligible::GithubNotConnected),
            Some(false) => Some(Ineligible::RepoNotAccessible),
            Some(true) => None,
        }
    };
    session.eligible = reason.is_none();
    session.ineligible_reason = reason;
}

/// Where a session ran, from its `chat_sessions` row (migration 0116).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlacedSession {
    pub session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    /// Every branch the session reported, first seen first.
    pub git_branches: Vec<String>,
    pub repo_host: Option<String>,
    pub repo_owner: Option<String>,
    pub repo_name: Option<String>,
    pub repo_root: Option<String>,
}

impl PlacedSession {
    pub fn repo(&self) -> Option<String> {
        Some(format!(
            "{}/{}",
            self.repo_owner.as_ref()?,
            self.repo_name.as_ref()?
        ))
    }

    /// The folders the session's edit paths start with: the working tree's top, else the cwd.
    pub fn roots(&self) -> Vec<String> {
        self.repo_root
            .iter()
            .chain(self.cwd.iter())
            .cloned()
            .collect()
    }

    /// The branches to search for the session's commits: every one it reported (the first
    /// alone for a session synced before branches were listed), then `default_branch`.
    pub fn branches(&self, default_branch: &str) -> Vec<String> {
        let reported = self.git_branch.iter().chain(&self.git_branches).cloned();
        nasiko_evals::session::branches_in_order(
            reported.chain(std::iter::once(default_branch.to_string())),
        )
    }
}

/// The caller's session of `harness`, or `None` when it is not theirs.
pub async fn placed(
    db: &sqlx::PgPool,
    user_id: Uuid,
    harness: &str,
    session_id: &str,
) -> Result<Option<PlacedSession>> {
    Ok(sqlx::query_as(
        r#"SELECT cs.session_id, cs.cwd, cs.git_branch, cs.git_branches, cs.repo_host,
                  cs.repo_owner, cs.repo_name, cs.repo_root
           FROM chat_sessions cs JOIN agents a ON a.id = cs.agent_id
           WHERE cs.session_id = $1 AND cs.user_id = $2
             AND a.coding_agent_integration_id = $3 AND cs.deleted_at IS NULL"#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(harness)
    .fetch_optional(db)
    .await?)
}

/// The session rebuilt from its synced turns; `None` when no turn kept its content.
pub async fn record(
    db: &sqlx::PgPool,
    user_id: Uuid,
    session: &PlacedSession,
) -> Result<Option<SessionRecord>> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT source_session_id, payload FROM coding_agent_telemetry_events \
         WHERE user_id = $1 AND session_id = $2 ORDER BY turn_started_at",
    )
    .bind(user_id)
    .bind(&session.session_id)
    .fetch_all(db)
    .await?;
    let payloads: Vec<serde_json::Value> = rows
        .iter()
        .map(|(_, p)| p.clone())
        .filter(coding_events::has_content)
        .collect();
    let Some((source_session_id, _)) = rows.first() else {
        return Ok(None);
    };
    if payloads.is_empty() {
        return Ok(None);
    }
    let place = SessionPlace {
        source_session_id: source_session_id.clone(),
        cwd: session.cwd.clone(),
        git_branch: session.git_branch.clone(),
        git_branches: session.git_branches.clone(),
    };
    coding_events::from_payloads(&place, &payloads)
        .map(Some)
        .context("the session's turns could not be read")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> SessionSummary {
        SessionSummary {
            session_id: "s".into(),
            title: "t".into(),
            started_at: Utc::now(),
            updated_at: Utc::now(),
            turns: 3,
            cost_usd: 0.1,
            models: vec![],
            repo: Some("o/r".into()),
            git_branch: None,
            repo_host: Some("github.com".into()),
            has_content: true,
            has_edits: true,
            eligible: false,
            ineligible_reason: None,
        }
    }

    #[test]
    fn a_session_is_eligible_only_when_everything_is_there() {
        let mut s = summary();
        judge("claude", &mut s, |_| Some(true));
        assert!(s.eligible);

        type Case = (
            &'static str,
            fn(&mut SessionSummary),
            Option<bool>,
            Ineligible,
        );
        let cases: [Case; 6] = [
            ("codex", |_| {}, Some(true), Ineligible::UnsupportedHarness),
            (
                "claude",
                |s| s.has_content = false,
                Some(true),
                Ineligible::NoChatHistory,
            ),
            (
                "claude",
                |s| s.has_edits = false,
                Some(true),
                Ineligible::NoEdits,
            ),
            ("claude", |s| s.repo = None, Some(true), Ineligible::NoRepo),
            (
                "claude",
                |s| s.repo_host = Some("gitlab.com".into()),
                Some(true),
                Ineligible::NotGithub,
            ),
            ("claude", |_| {}, Some(false), Ineligible::RepoNotAccessible),
        ];
        for (harness, change, readable, reason) in cases {
            let mut s = summary();
            change(&mut s);
            judge(harness, &mut s, |_| readable);
            assert_eq!(s.ineligible_reason, Some(reason));
            assert!(!s.eligible);
        }
        let mut s = summary();
        judge("claude", &mut s, |_| None);
        assert_eq!(s.ineligible_reason, Some(Ineligible::GithubNotConnected));
    }
}
