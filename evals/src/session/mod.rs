//! A finished coding-agent session, reduced to what the evals need.

pub mod claude_code;
pub mod coding_events;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cost::TokenUsage;
use crate::instance::SessionSource;

/// One session, as read from the coding agent's own log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub source: SessionSource,
    /// The folder the session ran in.
    pub cwd: String,
    pub git_branch: Option<String>,
    /// Every branch the session reported, first seen first (`git_branch` is the first): a
    /// session that switches branches commits on them all.
    #[serde(default)]
    pub git_branches: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    /// Only text the person typed, in order.
    pub user_messages: Vec<UserMessage>,
    /// Files the session edited, with the lines it added.
    pub edits: Vec<FileEdit>,
    /// One entry per distinct model reply.
    pub usage: Vec<ModelUsage>,
}

/// The distinct non-empty branch names among `seen`, first seen first.
pub fn branches_in_order(seen: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut branches: Vec<String> = Vec::new();
    for branch in seen {
        let branch = branch.trim();
        if !branch.is_empty() && !branches.iter().any(|b| b == branch) {
            branches.push(branch.to_string());
        }
    }
    branches
}

/// Something the person typed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserMessage {
    pub at: DateTime<Utc>,
    pub text: String,
}

/// One edit the session made to a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEdit {
    pub at: DateTime<Utc>,
    /// Absolute path, as the log recorded it.
    pub path: String,
    pub added_lines: Vec<String>,
}

/// Token usage of one model reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelUsage {
    pub at: DateTime<Utc>,
    pub message_id: String,
    pub model: String,
    pub usage: TokenUsage,
}

impl SessionRecord {
    /// Total tokens per model, in first-seen order.
    pub fn usage_by_model(&self) -> Vec<(String, TokenUsage)> {
        let mut totals: Vec<(String, TokenUsage)> = Vec::new();
        for u in &self.usage {
            match totals.iter_mut().find(|(m, _)| *m == u.model) {
                Some((_, t)) => t.add(&u.usage),
                None => totals.push((u.model.clone(), u.usage)),
            }
        }
        totals
    }

    /// When the task behind a commit began: the last typed message at or before the session's
    /// first edit to any of `files` (repo-relative, under one of `repo_roots`). `None` when the
    /// session never edited those files.
    pub fn task_start(&self, repo_roots: &[String], files: &[String]) -> Option<DateTime<Utc>> {
        self.task_start_after(repo_roots, files, None)
    }

    /// [`SessionRecord::task_start`] counting only edits after `after`: where the next of a
    /// long session's tasks began, once the previous one was committed at `after`.
    pub fn task_start_after(
        &self,
        repo_roots: &[String],
        files: &[String],
        after: Option<DateTime<Utc>>,
    ) -> Option<DateTime<Utc>> {
        let first_edit = self
            .edits
            .iter()
            .filter(|e| after.is_none_or(|a| e.at > a))
            .filter(|e| repo_relative(&e.path, repo_roots).is_some_and(|rel| files.contains(&rel)))
            .map(|e| e.at)
            .min()?;
        self.user_messages
            .iter()
            .map(|m| m.at)
            .filter(|t| *t <= first_edit)
            .max()
    }

    /// The part of the session that produced one task: typed messages and model replies from
    /// `from` (inclusive; `None` = the start) up to `until` (inclusive). A long session often
    /// holds several tasks; this keeps only the linked one's share.
    pub fn slice(&self, from: Option<DateTime<Utc>>, until: DateTime<Utc>) -> SessionRecord {
        let inside = |t: DateTime<Utc>| from.is_none_or(|a| t >= a) && t <= until;
        SessionRecord {
            user_messages: self
                .user_messages
                .iter()
                .filter(|m| inside(m.at))
                .cloned()
                .collect(),
            usage: self
                .usage
                .iter()
                .filter(|u| inside(u.at))
                .cloned()
                .collect(),
            ..self.clone()
        }
    }

    /// Sum of all usage, across models.
    pub fn total_usage(&self) -> TokenUsage {
        let mut total = TokenUsage::default();
        for u in &self.usage {
            total.add(&u.usage);
        }
        total
    }

    /// The model that produced the most output tokens.
    pub fn main_model(&self) -> Option<String> {
        self.usage_by_model()
            .into_iter()
            .max_by_key(|(_, t)| t.output)
            .map(|(m, _)| m)
    }

    /// Edited paths relative to whichever of `repo_roots` holds them; edits outside every
    /// root are dropped. A repo can have several roots: its main folder and its `git worktree`s,
    /// which share one history, and a session may edit any of them.
    pub fn edited_paths_in(&self, repo_roots: &[String]) -> Vec<String> {
        let mut paths: Vec<String> = self
            .edits
            .iter()
            .filter_map(|e| repo_relative(&e.path, repo_roots))
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }
}

/// `path` relative to whichever of `roots` holds it, with `/` separators as git writes them;
/// `None` outside every root. A session run on Windows records `C:\Users\me\repo\src\a.ts` under
/// the root `C:\Users\me\repo`: both are read with `/` for `\`, and the drive letter in either
/// case, or no Windows session could ever be matched to its commits.
fn repo_relative(path: &str, roots: &[String]) -> Option<String> {
    let path = comparable(path);
    roots.iter().find_map(|root| {
        let root = format!("{}/", comparable(root).trim_end_matches('/'));
        path.strip_prefix(&root).map(str::to_string)
    })
}

/// A Windows path (`C:\…`, `c:/…`, `\\server\share\…`) with `/` separators and an upper-case
/// drive letter; any other path as it is, since `\` is an ordinary character in a Unix name.
fn comparable(path: &str) -> String {
    let bytes = path.as_bytes();
    let has_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    if !has_drive && !path.starts_with("\\\\") {
        return path.to_string();
    }
    let mut out = path.replace('\\', "/");
    if has_drive {
        out[..1].make_ascii_uppercase();
    }
    out
}

/// Most typed messages kept as context for rewriting the request (prompt P1).
pub const CONTEXT_MESSAGES: usize = 40;

/// What is kept of a session next to its task (never the full log): enough to rewrite the
/// request (prompt P1) and to price the original work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSource {
    pub session_id: String,
    pub source: SessionSource,
    pub git_branch: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    /// The typed messages from the one that led to the first edit up to the commit.
    pub task_messages: Vec<UserMessage>,
    /// Up to [`CONTEXT_MESSAGES`] typed messages before the commit, oldest first: the request
    /// is often far earlier than the edit that answers it.
    pub context_messages: Vec<UserMessage>,
    /// Subjects of the linked commits, oldest first.
    pub commit_subjects: Vec<String>,
    /// Files the linked commits changed (repo-relative).
    pub changed_files: Vec<String>,
    /// Token usage of the task's slice, per model.
    pub usage_by_model: Vec<(String, TokenUsage)>,
}

impl TaskSource {
    /// Builds the summary from the whole session, the task's slice of it, and the commits.
    pub fn new(
        session: &SessionRecord,
        task: &SessionRecord,
        commits: &[&crate::CommitFacts],
    ) -> Self {
        let until = commits
            .iter()
            .map(|c| c.committed_at)
            .max()
            .unwrap_or(session.ended_at);
        let before: Vec<&UserMessage> = session
            .user_messages
            .iter()
            .filter(|m| m.at <= until)
            .collect();
        let skip = before.len().saturating_sub(CONTEXT_MESSAGES);
        let mut changed_files: Vec<String> = commits.iter().flat_map(|c| c.files.clone()).collect();
        changed_files.sort();
        changed_files.dedup();
        Self {
            session_id: session.session_id.clone(),
            source: session.source,
            git_branch: session.git_branch.clone(),
            started_at: session.started_at,
            ended_at: session.ended_at,
            task_messages: task.user_messages.clone(),
            context_messages: before.into_iter().skip(skip).cloned().collect(),
            commit_subjects: commits.iter().map(|c| c.subject.clone()).collect(),
            changed_files,
            usage_by_model: task.usage_by_model(),
        }
    }
}

/// Why a log could not be read as a session.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SessionError {
    #[error("the log has no session id")]
    NoSessionId,
    #[error("the log has no working folder (cwd)")]
    NoWorkingFolder,
    #[error("the log has no timestamps")]
    NoTimestamps,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().expect("timestamp")
    }

    fn session_editing(paths: &[&str]) -> SessionRecord {
        SessionRecord {
            session_id: "s".into(),
            source: SessionSource::ClaudeCode,
            cwd: r"C:\Users\me\repo".into(),
            git_branch: None,
            git_branches: Vec::new(),
            started_at: at("2026-11-19T10:00:00Z"),
            ended_at: at("2026-11-19T11:00:00Z"),
            user_messages: vec![UserMessage {
                at: at("2026-11-19T10:00:00Z"),
                text: "Fix the fallback".into(),
            }],
            edits: paths
                .iter()
                .map(|p| FileEdit {
                    at: at("2026-11-19T10:05:00Z"),
                    path: p.to_string(),
                    added_lines: Vec::new(),
                })
                .collect(),
            usage: Vec::new(),
        }
    }

    #[test]
    fn a_windows_session_s_edits_are_found_in_its_repo_with_git_separators() {
        let s = session_editing(&[
            r"C:\Users\me\repo\server\src\llm.ts",
            r"c:\Users\me\repo\server\test\llm.test.ts",
            r"C:\Users\me\other\notes.md",
        ]);
        let roots = vec![r"C:\Users\me\repo".to_string()];
        assert_eq!(
            s.edited_paths_in(&roots),
            vec!["server/src/llm.ts", "server/test/llm.test.ts"]
        );
        let files = vec!["server/src/llm.ts".to_string()];
        assert_eq!(
            s.task_start(&roots, &files),
            Some(at("2026-11-19T10:00:00Z"))
        );
    }

    #[test]
    fn a_folder_that_only_starts_like_the_root_is_outside_it() {
        let s = session_editing(&[r"C:\Users\me\repo-old\a.ts", "/srv/repo2/a.rs"]);
        let roots = vec![r"C:\Users\me\repo".to_string(), "/srv/repo".to_string()];
        assert!(s.edited_paths_in(&roots).is_empty());
    }

    #[test]
    fn unix_paths_are_matched_as_before_and_keep_their_backslashes() {
        let s = session_editing(&["/srv/repo/src/a.rs", r"/srv/repo/odd\name.rs"]);
        let roots = vec!["/srv/repo/".to_string()];
        assert_eq!(s.edited_paths_in(&roots), vec!["odd\\name.rs", "src/a.rs"]);
    }
}
