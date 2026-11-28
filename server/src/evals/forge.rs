//! One repository on GitHub, read with the caller's connected account: what the harvest and the
//! build need from git, through the REST API (the server has no git CLI).

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use nasiko_evals::CommitFacts;
use nasiko_evals::remote::{package_name, trailers};
use nasiko_github::GitHubService;
use uuid::Uuid;

use crate::state::AppState;

/// Most entries a task's repository may have. A task image holds a whole project, not one agent's
/// source, so this is far above the agent upload's limit; the size cap still applies.
const TASK_REPO_MAX_FILES: usize = 100_000;

/// Why a repository cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeRefusal {
    /// GitHub OAuth is not configured on this server, or the caller never connected.
    GithubNotConnected,
    /// The caller's account cannot see the repository.
    RepoNotAccessible,
}

impl ForgeRefusal {
    pub fn code(self) -> &'static str {
        match self {
            ForgeRefusal::GithubNotConnected => "github_not_connected",
            ForgeRefusal::RepoNotAccessible => "repo_not_accessible",
        }
    }
}

/// The caller's GitHub service and token, when both exist and GitHub still accepts the token.
/// A revoked token counts as not connected, so the person is asked to connect again instead of
/// every repository read failing; a GitHub outage does not (the token may be fine).
pub async fn connection(
    state: &AppState,
    user_id: Uuid,
) -> Result<(Arc<GitHubService>, String), ForgeRefusal> {
    let svc = state
        .github_svc
        .clone()
        .ok_or(ForgeRefusal::GithubNotConnected)?;
    let token = crate::github::load_github_token(&state.db, user_id)
        .await
        .ok_or(ForgeRefusal::GithubNotConnected)?;
    if let Ok(false) = svc.verify_token(&token).await {
        return Err(ForgeRefusal::GithubNotConnected);
    }
    Ok((svc, token))
}

/// One repository the caller can read.
pub struct Forge {
    svc: Arc<GitHubService>,
    token: String,
    pub repo: String,
    pub default_branch: String,
}

impl Forge {
    /// Opens `repo` (`owner/name`) for `user_id`.
    pub async fn open(
        state: &AppState,
        user_id: Uuid,
        repo: &str,
    ) -> Result<std::result::Result<Self, ForgeRefusal>> {
        let (svc, token) = match connection(state, user_id).await {
            Ok(c) => c,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let info = match svc.repo(&token, repo).await {
            Ok(Some(info)) => info,
            Ok(None) => return Ok(Err(ForgeRefusal::RepoNotAccessible)),
            Err(nasiko_github::Error::Auth(_)) => {
                return Ok(Err(ForgeRefusal::GithubNotConnected));
            }
            Err(e) => return Err(e.into()),
        };
        Ok(Ok(Self {
            svc,
            token,
            repo: info.full_name,
            default_branch: info.default_branch,
        }))
    }

    /// Commits made in `[since, until]` on any of `branches` (missing ones are skipped), each
    /// with the branches it was found on, newest first.
    pub async fn commits_between(
        &self,
        branches: &[String],
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<(String, Vec<String>)>> {
        let mut found: Vec<(String, Vec<String>)> = Vec::new();
        for branch in branches {
            for sha in self
                .svc
                .commits_between(&self.token, &self.repo, Some(branch), since, until)
                .await?
            {
                match found.iter_mut().find(|(s, _)| *s == sha) {
                    Some((_, on)) => on.push(branch.clone()),
                    None => found.push((sha, vec![branch.clone()])),
                }
            }
        }
        Ok(found)
    }

    /// What linking needs to know about one commit; `branches` are the ones it was listed on.
    pub async fn commit_facts(&self, sha: &str, branches: Vec<String>) -> Result<CommitFacts> {
        let detail = self.svc.commit(&self.token, &self.repo, sha).await?;
        let committed_at = detail
            .committed_at()
            .with_context(|| format!("commit {sha} has no committer date"))?;
        Ok(CommitFacts {
            parent: detail
                .parents
                .first()
                .map(|p| p.sha.clone())
                .unwrap_or_default(),
            is_merge: detail.parents.len() > 1,
            committed_at,
            subject: detail
                .commit
                .message
                .lines()
                .next()
                .unwrap_or_default()
                .to_string(),
            added_lines: detail.added_lines(),
            files: detail.files.iter().map(|f| f.filename.clone()).collect(),
            trailers: trailers(&detail.commit.message),
            branches,
            sha: detail.sha,
        })
    }

    /// One pull request's number, title and base/head commits.
    pub async fn pull_request(&self, number: u64) -> Result<nasiko_github::PullRequestDetail> {
        Ok(self
            .svc
            .pull_request(&self.token, &self.repo, number)
            .await?)
    }

    /// The unified diff from `base` to `head`.
    pub async fn diff(&self, base: &str, head: &str) -> Result<String> {
        Ok(self
            .svc
            .compare_diff(&self.token, &self.repo, base, head)
            .await?)
    }

    /// A file's text at `git_ref`; `None` when it does not exist there.
    pub async fn file_at(&self, git_ref: &str, path: &str) -> Result<Option<String>> {
        Ok(self
            .svc
            .file_at(&self.token, &self.repo, git_ref, path)
            .await?)
    }

    /// The files' text at `git_ref`, those that exist.
    pub async fn files_at(
        &self,
        git_ref: &str,
        paths: &[String],
    ) -> Result<BTreeMap<String, String>> {
        let mut texts = BTreeMap::new();
        for path in paths {
            if let Some(text) = self.file_at(git_ref, path).await? {
                texts.insert(path.clone(), text);
            }
        }
        Ok(texts)
    }

    pub async fn root_files(&self, git_ref: &str) -> Result<Vec<String>> {
        Ok(self
            .svc
            .root_files(&self.token, &self.repo, git_ref)
            .await?)
    }

    /// The repository at `git_ref` as a flat `tar.gz` (no history).
    pub async fn archive(&self, git_ref: &str) -> Result<Vec<u8>> {
        Ok(self
            .svc
            .clone_to_archive_within(&self.token, &self.repo, git_ref, TASK_REPO_MAX_FILES)
            .await?
            .archive_bytes)
    }

    /// The Cargo package owning `path` at `git_ref`, and its `Cargo.toml`: the nearest one above it.
    pub async fn cargo_package_of(
        &self,
        git_ref: &str,
        path: &str,
    ) -> Result<Option<(String, String)>> {
        for manifest in nasiko_evals::spec::manifests_above(path) {
            if let Some(text) = self.file_at(git_ref, &manifest).await? {
                return Ok(package_name(&text).map(|name| (name, manifest)));
            }
        }
        Ok(None)
    }
}
