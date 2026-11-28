//! Reading a repository's history through the REST API: the commits in a time window, one
//! commit with its files, the diff between two commits, a file at a ref and the repository's
//! root listing. The control plane has no git CLI; replay evals link coding sessions to the
//! commits they produced with these.

use serde::Deserialize;
use tracing::instrument;

use crate::error::{Error, Result};
use crate::service::{GitHubService, validate_branch_name, validate_repo_full_name};

/// Most pages of commits read for one window (100 commits each).
const MAX_COMMIT_PAGES: u32 = 10;

const DIFF_MEDIA_TYPE: &str = "application/vnd.github.diff";
const RAW_MEDIA_TYPE: &str = "application/vnd.github.raw";

/// A repository the token can read.
#[derive(Debug, Clone, Deserialize)]
pub struct RepoInfo {
    pub full_name: String,
    pub default_branch: String,
    #[serde(default)]
    pub private: bool,
}

/// One commit with the files it changed.
#[derive(Debug, Clone, Deserialize)]
pub struct CommitDetail {
    pub sha: String,
    #[serde(default)]
    pub parents: Vec<ShaRef>,
    pub commit: CommitMeta,
    /// GitHub lists at most 300 files per commit.
    #[serde(default)]
    pub files: Vec<CommitFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShaRef {
    pub sha: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitMeta {
    pub message: String,
    pub committer: Option<Signature>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Signature {
    pub date: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommitFile {
    pub filename: String,
    #[serde(default)]
    pub status: String,
    /// The file's unified-diff hunks; absent for binary files and very large diffs.
    pub patch: Option<String>,
}

impl CommitDetail {
    /// When it was committed (the committer date, as `git log` uses).
    pub fn committed_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.commit.committer.as_ref().and_then(|c| c.date)
    }

    /// Lines the commit added, across its files.
    pub fn added_lines(&self) -> Vec<String> {
        self.files
            .iter()
            .filter_map(|f| f.patch.as_deref())
            .flat_map(|p| p.lines())
            .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
            .map(|l| l[1..].to_string())
            .collect()
    }
}

/// One pull request's identifying commits, read for code-review evals: the diff and touched
/// files come from `compare_diff`/`file_at` at `head.sha`, same as any other ref.
#[derive(Debug, Clone, Deserialize)]
pub struct PullRequestDetail {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub base: PullRequestRef,
    pub head: PullRequestRef,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PullRequestRef {
    pub sha: String,
}

#[derive(Deserialize)]
struct ListedCommit {
    sha: String,
}

#[derive(Deserialize)]
struct ContentEntry {
    name: String,
}

impl GitHubService {
    /// The repository, when the token can read it; `Ok(None)` when GitHub answers 404 (it does
    /// for a private repository the token cannot see).
    #[instrument(skip(self, token))]
    pub async fn repo(&self, token: &str, repo: &str) -> Result<Option<RepoInfo>> {
        validate_repo_full_name(repo)?;
        match self
            .api_client
            .get_authed::<RepoInfo>(&format!("/repos/{repo}"), token)
            .await
        {
            Ok(info) => Ok(Some(info)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Shas of the commits on `branch` (default branch when `None`) committed in
    /// `[since, until]`, newest first, at most 1000.
    #[instrument(skip(self, token))]
    pub async fn commits_between(
        &self,
        token: &str,
        repo: &str,
        branch: Option<&str>,
        since: chrono::DateTime<chrono::Utc>,
        until: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<String>> {
        validate_repo_full_name(repo)?;
        if let Some(b) = branch {
            validate_branch_name(b)?;
        }
        let (since, until) = (since.to_rfc3339(), until.to_rfc3339());
        let mut shas = Vec::new();
        for page in 1..=MAX_COMMIT_PAGES {
            let page = page.to_string();
            let mut params = vec![
                ("since", since.as_str()),
                ("until", until.as_str()),
                ("per_page", "100"),
                ("page", page.as_str()),
            ];
            if let Some(b) = branch {
                params.push(("sha", b));
            }
            let listed: Vec<ListedCommit> = match self
                .api_client
                .get_authed_params(&format!("/repos/{repo}/commits"), token, &params)
                .await
            {
                Ok(listed) => listed,
                // A branch that no longer exists has no commits to offer.
                Err(Error::NotFound(_)) if branch.is_some() => return Ok(shas),
                Err(e) => return Err(e),
            };
            let full = listed.len() == 100;
            shas.extend(listed.into_iter().map(|c| c.sha));
            if !full {
                break;
            }
        }
        Ok(shas)
    }

    /// One commit with its parents and files.
    #[instrument(skip(self, token))]
    pub async fn commit(&self, token: &str, repo: &str, sha: &str) -> Result<CommitDetail> {
        validate_repo_full_name(repo)?;
        validate_ref(sha)?;
        self.api_client
            .get_authed(&format!("/repos/{repo}/commits/{sha}"), token)
            .await
    }

    /// The unified diff from `base` to `head`, as `git diff base head` prints it (no binary
    /// patches).
    #[instrument(skip(self, token))]
    pub async fn compare_diff(
        &self,
        token: &str,
        repo: &str,
        base: &str,
        head: &str,
    ) -> Result<String> {
        validate_repo_full_name(repo)?;
        validate_ref(base)?;
        validate_ref(head)?;
        self.api_client
            .get_text_authed(
                &format!("/repos/{repo}/compare/{base}...{head}"),
                token,
                DIFF_MEDIA_TYPE,
            )
            .await
    }

    /// A file's text at `git_ref`; `Ok(None)` when it does not exist there.
    #[instrument(skip(self, token))]
    pub async fn file_at(
        &self,
        token: &str,
        repo: &str,
        git_ref: &str,
        path: &str,
    ) -> Result<Option<String>> {
        validate_repo_full_name(repo)?;
        validate_ref(git_ref)?;
        let path = encode_path(path)?;
        match self
            .api_client
            .get_text_authed(
                &format!("/repos/{repo}/contents/{path}?ref={git_ref}"),
                token,
                RAW_MEDIA_TYPE,
            )
            .await
        {
            Ok(text) => Ok(Some(text)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// One pull request: its number, title and base/head commits.
    #[instrument(skip(self, token))]
    pub async fn pull_request(
        &self,
        token: &str,
        repo: &str,
        number: u64,
    ) -> Result<PullRequestDetail> {
        validate_repo_full_name(repo)?;
        self.api_client
            .get_authed(&format!("/repos/{repo}/pulls/{number}"), token)
            .await
    }

    /// Names of the files and folders at the repository's root at `git_ref`.
    #[instrument(skip(self, token))]
    pub async fn root_files(&self, token: &str, repo: &str, git_ref: &str) -> Result<Vec<String>> {
        validate_repo_full_name(repo)?;
        validate_ref(git_ref)?;
        let entries: Vec<ContentEntry> = self
            .api_client
            .get_authed_params(
                &format!("/repos/{repo}/contents"),
                token,
                &[("ref", git_ref)],
            )
            .await?;
        Ok(entries.into_iter().map(|e| e.name).collect())
    }
}

/// A sha or branch name that is safe to put in a URL path.
fn validate_ref(git_ref: &str) -> Result<()> {
    validate_branch_name(git_ref)?;
    if git_ref.contains("..") || git_ref.contains('#') || git_ref.contains('&') {
        return Err(Error::GitClone(format!("invalid git ref {git_ref:?}")));
    }
    Ok(())
}

/// A repo-relative path, percent-encoded per segment; `..` and empty segments are refused.
fn encode_path(path: &str) -> Result<String> {
    let mut out = Vec::new();
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(Error::GitClone(format!("invalid repository path {path:?}")));
        }
        let encoded: String = segment
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        out.push(encoded);
    }
    Ok(out.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_encoded_per_segment_and_traversal_is_refused() {
        assert_eq!(encode_path("src/my file.rs").unwrap(), "src/my%20file.rs");
        assert_eq!(encode_path("a/b#c?.rs").unwrap(), "a/b%23c%3F.rs");
        assert!(encode_path("../etc/passwd").is_err());
        assert!(encode_path("a//b").is_err());
        assert!(validate_ref("main..evil").is_err());
        assert!(validate_ref("0123abcd").is_ok());
    }

    #[test]
    fn a_pull_request_reply_gives_its_base_and_head_shas() {
        let pr: PullRequestDetail = serde_json::from_value(serde_json::json!({
            "number": 42,
            "title": "Fix the thing",
            "html_url": "https://github.com/o/r/pull/42",
            "base": {"sha": "aaa"},
            "head": {"sha": "bbb"}
        }))
        .unwrap();
        assert_eq!(pr.base.sha, "aaa");
        assert_eq!(pr.head.sha, "bbb");
    }

    #[test]
    fn added_lines_skip_file_headers() {
        let detail: CommitDetail = serde_json::from_value(serde_json::json!({
            "sha": "c1",
            "parents": [{"sha": "p1"}],
            "commit": {"message": "m", "committer": {"date": "2026-11-01T10:00:00Z"}},
            "files": [
                {"filename": "a.rs", "status": "modified", "patch": "@@ -1 +1 @@\n-old\n+new\n+++odd"},
                {"filename": "b.bin", "status": "added"}
            ]
        }))
        .unwrap();
        assert_eq!(detail.added_lines(), vec!["new"]);
        assert!(detail.committed_at().is_some());
    }
}
