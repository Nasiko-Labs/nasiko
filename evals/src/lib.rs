//! Replay evals: turn finished coding-agent sessions into SWE-bench-style tasks, and score
//! cheaper models on them.
//!
//! This crate is pure logic. It never touches the disk, the network, git or Docker: callers
//! pass text in and get values back. The server's eval worker and the `nasiko eval local` CLI
//! do the I/O.

pub mod agent;
pub mod cost;
pub mod diff;
pub mod gate;
pub mod grade;
pub mod harvest;
pub mod images;
pub mod instance;
pub mod layout;
pub mod link;
pub mod opencode;
pub mod patch;
pub mod prompt;
pub mod remote;
pub mod report;
pub mod rust_tests;
pub mod screen;
pub mod session;
pub mod spec;
pub mod testlog;
pub mod verdict;

pub use cost::{PriceQuality, RouterUsage, TokenUsage};
pub use diff::{FileDiff, SplitPatch, TestPathRules};
pub use instance::{
    Difficulty, NasikoExtras, OriginalRun, SessionSource, TaskInstance, TaskStatus, TestSource,
};
pub use layout::EvalLayout;
pub use link::{CommitFacts, LinkDecision, LinkScore};
pub use session::{FileEdit, ModelUsage, SessionRecord, TaskSource, UserMessage};

/// Every error this crate returns.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("session log: {0}")]
    Session(#[from] session::SessionError),
    #[error("diff: {0}")]
    Diff(#[from] diff::DiffError),
    #[error("price: {0}")]
    Price(#[from] cost::PriceError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}
