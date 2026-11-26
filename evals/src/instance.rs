//! The task format: one SWE-bench row plus Nasiko's own fields under `x_nasiko`.
//!
//! Field names and meanings follow the SWE-bench dataset card; `image`, `eval_script`,
//! `log_parser` and `eval_type` are the extra fields the SWE-bench v5 grader needs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cost::TokenUsage;

/// One benchmark task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskInstance {
    pub instance_id: String,
    pub repo: String,
    pub base_commit: String,
    pub environment_setup_commit: String,
    pub version: String,
    pub created_at: DateTime<Utc>,
    pub problem_statement: String,
    pub hints_text: String,
    /// The real fix, without test files.
    pub patch: String,
    /// Only the test-file part of the real fix.
    pub test_patch: String,
    #[serde(rename = "FAIL_TO_PASS")]
    pub fail_to_pass: Vec<String>,
    #[serde(rename = "PASS_TO_PASS")]
    pub pass_to_pass: Vec<String>,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub eval_script: String,
    #[serde(default)]
    pub log_parser: String,
    #[serde(default)]
    pub eval_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
    pub x_nasiko: NasikoExtras,
}

/// Nasiko's own bookkeeping for a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NasikoExtras {
    pub status: TaskStatus,
    pub source: SessionSource,
    pub source_session_id: String,
    pub gold_commit: String,
    pub link_score: f64,
    pub test_source: TestSource,
    pub orig: OriginalRun,
    /// Files whose test code sits inside normal code (e.g. Rust `#[cfg(test)]`), so the
    /// commit could not be split cleanly by file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mixed_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<String>,
    /// Whether prompt P1 rewrote `problem_statement` from the session (else it is the raw
    /// message that led to the first edit).
    #[serde(default)]
    pub statement_rewritten: bool,
    /// What the tests run on, e.g. `["-p nasiko-compress"]` for Cargo; set by the gate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_scope: Vec<String>,
}

/// What the original session used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OriginalRun {
    pub model: String,
    pub usage: TokenUsage,
    /// How many messages the person typed: 1–2 is "one-shot", more is "steered".
    pub user_turns: u32,
}

/// Where a task is in the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Candidate,
    Reproducible,
    Verified,
    Rejected,
}

/// SWE-bench Verified's effort groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Difficulty {
    #[serde(rename = "<15 min fix")]
    Under15Min,
    #[serde(rename = "15 min - 1 hour")]
    Min15To1H,
    #[serde(rename = "1-4 hours")]
    H1To4,
    #[serde(rename = ">4 hours")]
    Over4H,
}

/// Where the real test patch came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestSource {
    Commit,
    Generated,
    /// The commit had no tests yet; filled in later or the task is dropped.
    None,
}

/// Which coding agent produced the original session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionSource {
    ClaudeCode,
    Codex,
}

/// Number of session-id characters used in an instance id.
pub const SESSION_ID_CHARS: usize = 8;

/// Number of commit-sha characters used in the id of one of a session's several tasks.
pub const COMMIT_SHA_CHARS: usize = 7;

/// `<repo-slug>__<first 8 chars of the session id>`, e.g. `nasiko-cloud-rs__a2231e9a`.
pub fn instance_id(repo_slug: &str, session_id: &str) -> String {
    let short: String = session_id.chars().take(SESSION_ID_CHARS).collect();
    format!("{repo_slug}__{short}")
}

/// The id of one of a session's several tasks: [`instance_id`] plus its commit,
/// e.g. `nasiko-cloud-rs__a2231e9a-6a886ae`.
pub fn commit_instance_id(repo_slug: &str, session_id: &str, sha: &str) -> String {
    let short: String = sha.chars().take(COMMIT_SHA_CHARS).collect();
    format!("{}-{short}", instance_id(repo_slug, session_id))
}

impl TaskInstance {
    /// The task as one line of a SWE-bench `.jsonl` file.
    pub fn to_swebench_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_id_uses_eight_session_chars() {
        assert_eq!(
            instance_id("nasiko-cloud-rs", "a2231e9a-fb41-48e9"),
            "nasiko-cloud-rs__a2231e9a"
        );
    }

    #[test]
    fn swebench_keys_are_exact() {
        let task = TaskInstance {
            instance_id: "r__1".into(),
            repo: "o/r".into(),
            base_commit: "a".into(),
            environment_setup_commit: "a".into(),
            version: "1".into(),
            created_at: DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is valid"),
            problem_statement: "p".into(),
            hints_text: String::new(),
            patch: String::new(),
            test_patch: String::new(),
            fail_to_pass: vec!["t1".into()],
            pass_to_pass: vec![],
            image: String::new(),
            eval_script: String::new(),
            log_parser: "parse_log_cargo".into(),
            eval_type: String::new(),
            difficulty: Some(Difficulty::Under15Min),
            x_nasiko: NasikoExtras {
                status: TaskStatus::Candidate,
                source: SessionSource::ClaudeCode,
                source_session_id: "1".into(),
                gold_commit: "b".into(),
                link_score: 0.9,
                test_source: TestSource::Commit,
                orig: OriginalRun {
                    model: "m".into(),
                    usage: TokenUsage::default(),
                    user_turns: 1,
                },
                mixed_files: vec![],
                reject_reason: None,
                statement_rewritten: false,
                test_scope: vec![],
            },
        };
        let line = task.to_swebench_line().expect("serialises");
        assert!(line.contains("\"FAIL_TO_PASS\":[\"t1\"]"));
        assert!(line.contains("\"difficulty\":\"<15 min fix\""));
        let back: TaskInstance = serde_json::from_str(&line).expect("round-trips");
        assert_eq!(back, task);
    }
}
