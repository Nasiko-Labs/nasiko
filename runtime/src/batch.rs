//! Running one container to completion: the seam the control plane's batch work (replay evals)
//! runs on. A sibling of `ContainerRuntime` (long-lived agents) and `SandboxProvider` (sessions
//! that live while serving): a batch job starts, runs one command, and is gone, and the caller
//! wants its exit code, its log and a few files it wrote.
//!
//! Every job runs hardened whatever the backend: no Linux capabilities, no privilege gain, CPU,
//! memory, process and disk limits, and a wall-clock limit. Its network is either none at all or
//! the platform's model router only ([`BatchNetwork`]).

use std::time::Duration;

use async_trait::async_trait;

use crate::error::{Result, RuntimeError};

/// Where a job's input files appear (read-only).
pub const BATCH_INPUT_DIR: &str = "/eval/in";
/// Where a job writes the files the caller collects.
pub const BATCH_OUTPUT_DIR: &str = "/eval/out";

/// One file handed to, or collected from, a job. `path` is relative to [`BATCH_INPUT_DIR`] or
/// [`BATCH_OUTPUT_DIR`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchFile {
    pub path: String,
    pub contents: Vec<u8>,
}

/// What a job may reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchNetwork {
    /// No network at all.
    None,
    /// Only the platform's model router (`/v1/*`), at the base URL the job reads from
    /// [`BATCH_ROUTER_URL_ENV`]; nothing else, not even the public internet.
    RouterOnly,
}

/// The variable a [`BatchNetwork::RouterOnly`] job reads the router's base URL from (ending in
/// `/v1`). The runner sets it; a job never chooses where it can connect.
pub const BATCH_ROUTER_URL_ENV: &str = "NASIKO_ROUTER_URL";

/// Resource limits of one job.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchLimits {
    pub wall: Duration,
    pub cpus: f64,
    pub memory_mib: u64,
    pub pids: i64,
    /// Most bytes the job may write to its own filesystem.
    pub disk_mib: u64,
}

/// One container run to completion.
#[derive(Debug, Clone)]
pub struct BatchJob {
    /// Unique among running jobs; the backend derives its object names from it.
    pub name: String,
    /// An image reference [`BatchRunner::build_image`] returned (or any pullable image).
    pub image: String,
    pub command: Vec<String>,
    pub env: Vec<(String, String)>,
    pub inputs: Vec<BatchFile>,
    /// Whether to return what the job wrote under [`BATCH_OUTPUT_DIR`].
    pub collect_outputs: bool,
    pub network: BatchNetwork,
    pub limits: BatchLimits,
    /// Groups the jobs of one piece of work, for [`BatchRunner::cleanup`].
    pub label: String,
    /// Run in the job (`sh -c`) before it is stopped for a limit, to keep its work; `None` for
    /// nothing. Best effort: a backend that cannot run it stops the job directly.
    pub before_stop: Option<String>,
}

/// Why the runner stopped a job before it ended by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchStop {
    TimeLimit,
    DiskLimit,
}

/// How a job ended.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchOutcome {
    /// `None` when the runner stopped the job, or it died without an exit code.
    pub exit_code: Option<i32>,
    pub stopped: Option<BatchStop>,
    /// stdout and stderr, interleaved; the last [`BATCH_LOG_MAX_BYTES`] when longer.
    pub logs: String,
    pub outputs: Vec<BatchFile>,
    pub wall: Duration,
}

/// Most log bytes a job returns (its tail).
pub const BATCH_LOG_MAX_BYTES: usize = 4 * 1024 * 1024;
/// Most bytes of output files a job returns.
pub const BATCH_OUTPUT_MAX_BYTES: usize = 32 * 1024 * 1024;

impl BatchOutcome {
    /// One output file's text, if the job wrote it.
    pub fn output_text(&self, path: &str) -> Option<String> {
        self.outputs
            .iter()
            .find(|f| f.path == path)
            .map(|f| String::from_utf8_lossy(&f.contents).into_owned())
    }
}

/// Builds images for, and runs, batch jobs.
#[async_trait]
pub trait BatchRunner: Send + Sync {
    /// Whether this runner can run jobs at all (false for [`UnavailableBatchRunner`]).
    fn is_available(&self) -> bool;

    /// Builds `context_tar` (a tar with a `Dockerfile` at its root) as `tag`, unless an image
    /// with that tag already exists, and returns the reference jobs run it by. Tags name their
    /// contents, so an existing tag is the same image.
    async fn build_image(&self, context_tar: &[u8], tag: &str) -> Result<String>;

    /// Runs `job` to completion, whatever its exit code; `Err` only when it could not be run.
    async fn run(&self, job: BatchJob) -> Result<BatchOutcome>;

    /// Removes what jobs labelled `label` left behind (all jobs when `None`); returns how many
    /// objects it removed.
    async fn cleanup(&self, label: Option<&str>) -> Result<usize>;
}

/// The runner for a deployment that cannot run batch jobs: it says so on every call.
pub struct UnavailableBatchRunner {
    reason: String,
}

impl UnavailableBatchRunner {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    fn refuse<T>(&self) -> Result<T> {
        Err(RuntimeError::Unsupported(format!(
            "batch jobs are not available: {}",
            self.reason
        )))
    }
}

#[async_trait]
impl BatchRunner for UnavailableBatchRunner {
    fn is_available(&self) -> bool {
        false
    }

    async fn build_image(&self, _: &[u8], _: &str) -> Result<String> {
        self.refuse()
    }

    async fn run(&self, _: BatchJob) -> Result<BatchOutcome> {
        self.refuse()
    }

    async fn cleanup(&self, _: Option<&str>) -> Result<usize> {
        Ok(0)
    }
}

/// The last `max` bytes of `text`, cut at a character boundary.
pub fn tail(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_unavailable_runner_refuses_and_says_why() {
        let r = UnavailableBatchRunner::new("no container backend");
        assert!(!r.is_available());
        let err = r.build_image(b"", "t").await.unwrap_err();
        assert!(err.to_string().contains("no container backend"));
        assert_eq!(r.cleanup(None).await.unwrap(), 0);
    }

    #[test]
    fn tails_cut_at_a_character_boundary() {
        assert_eq!(tail("abc", 10), "abc");
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("aé", 1), "");
    }
}
