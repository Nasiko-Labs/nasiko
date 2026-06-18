//! Sandbox abstraction — the seam between the agent's tools and where code actually runs.
//!
//! `LocalSandbox` runs commands in the current process's working directory (the CLI path).
//! A future `RemoteSandbox` (CP deployment) will provision a per-request container via the
//! control-plane Sandbox API and exec into it. The ReAct loop and tools only ever see this
//! trait, so swapping backends needs no agent/tool changes.

use std::future::Future;
use std::pin::Pin;

pub mod local;

pub use local::LocalSandbox;

/// Boxed future alias keeping the trait `dyn`-compatible (mirrors the repo's
/// `react-orchestrator/src/guard.rs` convention).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Result of running a command inside a sandbox.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// A place the agent can read/write files and run commands, scoped to a workspace root.
///
/// All `path` arguments are relative to the sandbox root and must stay within it;
/// implementations reject traversal outside the root.
pub trait Sandbox: Send + Sync {
    /// Read a file. `range` is an optional inclusive 1-based `(start_line, end_line)` slice.
    /// Returned content is annotated with line numbers (for display to the model).
    fn read_file<'a>(
        &'a self,
        path: &'a str,
        range: Option<(usize, usize)>,
    ) -> BoxFuture<'a, Result<String, String>>;

    /// Read the verbatim file content (no line numbers). Used by search/replace edits.
    fn read_file_raw<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<String, String>>;

    /// Create or overwrite a file.
    fn write_file<'a>(&'a self, path: &'a str, content: &'a str)
        -> BoxFuture<'a, Result<(), String>>;

    /// List a directory tree (relative paths), up to `depth` levels (None = unbounded).
    fn list_dir<'a>(
        &'a self,
        path: &'a str,
        depth: Option<usize>,
    ) -> BoxFuture<'a, Result<String, String>>;

    /// Run `command` via `sh -c` in the sandbox root, with an optional wall-clock timeout.
    fn exec<'a>(
        &'a self,
        command: &'a str,
        timeout_s: Option<u64>,
    ) -> BoxFuture<'a, Result<ExecResult, String>>;
}

/// Build the sandbox backend selected by environment.
///
/// `SANDBOX_MODE=local` (default) → [`LocalSandbox`] rooted at `WORKSPACE_DIR` (default `.`).
/// `SANDBOX_MODE=remote` is reserved for the control-plane backend (Phase 2) and currently
/// returns an error so misconfiguration fails loudly rather than silently running locally.
pub fn from_env() -> Result<Box<dyn Sandbox>, String> {
    let mode = std::env::var("SANDBOX_MODE").unwrap_or_else(|_| "local".into());
    match mode.as_str() {
        "local" => {
            let root = std::env::var("WORKSPACE_DIR").unwrap_or_else(|_| ".".into());
            Ok(Box::new(LocalSandbox::new(&root)?))
        }
        "remote" => Err("SANDBOX_MODE=remote is not implemented yet (Phase 2)".into()),
        other => Err(format!("unknown SANDBOX_MODE '{other}' (expected 'local' or 'remote')")),
    }
}
