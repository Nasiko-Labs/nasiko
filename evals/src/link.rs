//! Matching a session to the commit it produced.
//!
//! Each candidate commit gets a score from 0 to 1: how many of its files the session edited,
//! how many of its added lines the session wrote, whether it carries an AI co-author trailer,
//! and whether it is on the session's branch. The best one wins only if it is good enough and
//! clearly better than the next.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::session::SessionRecord;

/// What git says about one commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitFacts {
    pub sha: String,
    /// First parent; empty for a root commit.
    pub parent: String,
    pub committed_at: DateTime<Utc>,
    pub subject: String,
    /// Paths relative to the repo root.
    pub files: Vec<String>,
    pub added_lines: Vec<String>,
    /// Lines of the message that look like `Key: value` trailers.
    pub trailers: Vec<String>,
    pub branches: Vec<String>,
    pub is_merge: bool,
}

/// One candidate's score and its parts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkScore {
    pub sha: String,
    pub total: f64,
    pub file_overlap: f64,
    pub line_overlap: f64,
    pub has_ai_trailer: bool,
    pub same_branch: bool,
}

/// The outcome of linking one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum LinkDecision {
    /// One commit is the work.
    Single {
        link: LinkScore,
    },
    /// Several back-to-back commits are the work, oldest first.
    Range {
        first: LinkScore,
        last: LinkScore,
    },
    /// Two candidates are too close to call.
    Ambiguous {
        best: LinkScore,
        runner_up: LinkScore,
    },
    NoMatch,
}

pub const WEIGHT_FILES: f64 = 0.45;
pub const WEIGHT_LINES: f64 = 0.35;
pub const WEIGHT_TRAILER: f64 = 0.10;
pub const WEIGHT_BRANCH: f64 = 0.10;
/// Lowest score a commit may have and still be linked.
pub const ACCEPT_AT: f64 = 0.60;
/// How far the best candidate must lead the next one.
pub const MIN_LEAD: f64 = 0.15;
/// Commits touching more files than this are too big to be one task.
pub const MAX_FILES: usize = 40;
/// Most back-to-back commits merged into one task.
pub const MAX_RANGE: usize = 5;
/// How long after the session's last line a commit may still be its work.
pub fn window_after_end() -> Duration {
    Duration::hours(2)
}

/// Trailer prefixes that mark AI-assisted commits (lower-cased).
const AI_TRAILERS: [&str; 3] = [
    "co-authored-by: claude",
    "co-authored-by: codex",
    "co-authored-by: openai codex",
];

/// Whether `commit` falls in the session's time window.
pub fn in_window(session: &SessionRecord, commit: &CommitFacts) -> bool {
    commit.committed_at >= session.started_at
        && commit.committed_at <= session.ended_at + window_after_end()
}

/// Scores one candidate. `repo_roots` are the repo's absolute folders (main + worktrees) that
/// the session's edit paths may start with.
pub fn score(session: &SessionRecord, repo_roots: &[String], commit: &CommitFacts) -> LinkScore {
    let edited = session.edited_paths_in(repo_roots);
    let file_overlap = share(&commit.files, |f| edited.iter().any(|e| e == f));
    let written: Vec<String> = session
        .edits
        .iter()
        .flat_map(|e| e.added_lines.iter().map(|l| normalise(l)))
        .collect();
    let meaningful: Vec<String> = commit
        .added_lines
        .iter()
        .map(|l| normalise(l))
        .filter(|l| l.len() > 3)
        .collect();
    let line_overlap = share(&meaningful, |l| written.iter().any(|w| w == l));
    let has_ai_trailer = commit
        .trailers
        .iter()
        .any(|t| AI_TRAILERS.iter().any(|p| t.to_lowercase().starts_with(p)));
    let same_branch = session
        .git_branch
        .as_ref()
        .is_some_and(|b| commit.branches.iter().any(|c| c == b));
    let total = WEIGHT_FILES * file_overlap
        + WEIGHT_LINES * line_overlap
        + WEIGHT_TRAILER * f64::from(u8::from(has_ai_trailer))
        + WEIGHT_BRANCH * f64::from(u8::from(same_branch));
    LinkScore {
        sha: commit.sha.clone(),
        total,
        file_overlap,
        line_overlap,
        has_ai_trailer,
        same_branch,
    }
}

fn share<T>(items: &[T], hit: impl Fn(&T) -> bool) -> f64 {
    if items.is_empty() {
        return 0.0;
    }
    items.iter().filter(|i| hit(i)).count() as f64 / items.len() as f64
}

fn normalise(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a commit can be a task at all.
pub fn is_eligible(commit: &CommitFacts) -> bool {
    !commit.is_merge && !commit.files.is_empty() && commit.files.len() <= MAX_FILES
}

/// Picks the link from scores given oldest commit first.
pub fn decide(scored: &[LinkScore]) -> LinkDecision {
    let accepted: Vec<usize> = (0..scored.len())
        .filter(|&i| scored[i].total >= ACCEPT_AT)
        .collect();
    if let Some(run) = consecutive_run(&accepted)
        && run.len() >= 2
        && run.len() <= MAX_RANGE
        && run.len() == accepted.len()
    {
        return LinkDecision::Range {
            first: scored[run[0]].clone(),
            last: scored[run[run.len() - 1]].clone(),
        };
    }
    let mut ranked: Vec<&LinkScore> = scored.iter().collect();
    ranked.sort_by(|a, b| b.total.total_cmp(&a.total));
    match ranked.as_slice() {
        [] => LinkDecision::NoMatch,
        [best, ..] if best.total < ACCEPT_AT => LinkDecision::NoMatch,
        [best, next, ..] if best.total - next.total < MIN_LEAD => LinkDecision::Ambiguous {
            best: (*best).clone(),
            runner_up: (*next).clone(),
        },
        [best, ..] => LinkDecision::Single {
            link: (*best).clone(),
        },
    }
}

/// `indices` if they are one unbroken run (e.g. 3,4,5), else `None`.
fn consecutive_run(indices: &[usize]) -> Option<&[usize]> {
    let unbroken = indices.windows(2).all(|w| w[1] == w[0] + 1);
    (unbroken && !indices.is_empty()).then_some(indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::SessionSource;
    use crate::session::FileEdit;

    fn at(min: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(1_700_000_000 + min * 60, 0).expect("valid time")
    }

    fn session() -> SessionRecord {
        SessionRecord {
            session_id: "s".into(),
            source: SessionSource::ClaudeCode,
            cwd: "/r".into(),
            git_branch: Some("fix/x".into()),
            started_at: at(0),
            ended_at: at(30),
            user_messages: vec![],
            edits: vec![FileEdit {
                at: at(5),
                path: "/r/src/a.rs".into(),
                added_lines: vec!["let retry = 3;".into(), "fn go() {}".into()],
            }],
            usage: vec![],
        }
    }

    fn commit(sha: &str, files: &[&str], lines: &[&str]) -> CommitFacts {
        CommitFacts {
            sha: sha.into(),
            parent: "p".into(),
            committed_at: at(20),
            subject: "s".into(),
            files: files.iter().map(|s| s.to_string()).collect(),
            added_lines: lines.iter().map(|s| s.to_string()).collect(),
            trailers: vec!["Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>".into()],
            branches: vec!["fix/x".into()],
            is_merge: false,
        }
    }

    #[test]
    fn the_commit_with_the_session_s_code_scores_high() {
        let good = score(
            &session(),
            &["/r".into()],
            &commit("a", &["src/a.rs"], &["let  retry = 3;", "fn go() {}"]),
        );
        assert!((good.total - 1.0).abs() < 1e-9, "{good:?}");
        let other = score(
            &session(),
            &["/r".into()],
            &commit("b", &["src/z.rs"], &["unrelated();"]),
        );
        assert!(other.total < ACCEPT_AT);
        assert!(matches!(decide(&[other, good]), LinkDecision::Single { link } if link.sha == "a"));
    }

    #[test]
    fn two_close_candidates_are_ambiguous() {
        let a = LinkScore {
            sha: "a".into(),
            total: 0.8,
            file_overlap: 1.0,
            line_overlap: 0.5,
            has_ai_trailer: true,
            same_branch: true,
        };
        let mut b = a.clone();
        b.sha = "b".into();
        b.total = 0.7;
        let mut low = a.clone();
        low.sha = "c".into();
        low.total = 0.1;
        assert!(matches!(
            decide(&[a.clone(), low, b.clone()]),
            LinkDecision::Ambiguous { .. }
        ));
    }

    #[test]
    fn back_to_back_matches_become_a_range() {
        let mk = |sha: &str, total| LinkScore {
            sha: sha.into(),
            total,
            file_overlap: 1.0,
            line_overlap: 1.0,
            has_ai_trailer: true,
            same_branch: true,
        };
        let d = decide(&[mk("a", 0.2), mk("b", 0.9), mk("c", 0.8)]);
        assert!(
            matches!(d, LinkDecision::Range { first, last } if first.sha == "b" && last.sha == "c")
        );
    }

    #[test]
    fn nothing_good_enough_is_no_match() {
        assert_eq!(decide(&[]), LinkDecision::NoMatch);
    }

    #[test]
    fn window_includes_two_hours_after_end() {
        let mut c = commit("a", &["x"], &[]);
        c.committed_at = at(30 + 119);
        assert!(in_window(&session(), &c));
        c.committed_at = at(30 + 121);
        assert!(!in_window(&session(), &c));
    }
}
