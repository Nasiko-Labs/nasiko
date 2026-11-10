//! The harness child process: spawn, stdin writer, stdout line reader, signal handling.

use std::collections::HashMap;
use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::mpsc;

use crate::reaper::OwnChild;

/// How long a SIGTERMed child gets before SIGKILL.
pub const TERMINATE_GRACE: Duration = Duration::from_secs(10);

/// Largest single stdout line (one JSON-RPC frame) accepted. Matches axum's default WebSocket
/// max message size (64 MiB), so a frame the shim reads is a frame it could also deliver.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// A spawned harness with its stdio handles split out.
pub struct SpawnedChild {
    pub process: OwnChild,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
}

/// Why the stdout reader stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum StdoutEnd {
    /// The child closed stdout.
    Eof,
    /// A line exceeded the frame cap; the stream can no longer be framed.
    Overflow,
    /// Reading failed.
    Error,
}

/// Which signal asked the shim to stop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Signalled {
    Term,
    Int,
}

/// SIGTERM/SIGINT listeners. Installed before the child is spawned so no signal is missed.
pub struct Shutdown {
    term: Signal,
    int: Signal,
}

impl Shutdown {
    /// Registers the handlers.
    pub fn install() -> Result<Self> {
        Ok(Self {
            term: signal(SignalKind::terminate()).context("installing SIGTERM handler")?,
            int: signal(SignalKind::interrupt()).context("installing SIGINT handler")?,
        })
    }

    /// Resolves when SIGTERM or SIGINT arrives, saying which.
    pub async fn recv(&mut self) -> Signalled {
        tokio::select! {
            _ = self.term.recv() => Signalled::Term,
            _ = self.int.recv() => Signalled::Int,
        }
    }
}

/// Starts `argv` with `env` merged over the process environment, piped stdin/stdout and
/// inherited stderr (so it lands in container logs).
pub fn spawn(argv: &[String], env: &HashMap<String, String>) -> Result<SpawnedChild> {
    let Some((program, args)) = argv.split_first() else {
        bail!("empty harness command");
    };
    let mut command = Command::new(program);
    command
        .args(args)
        // The session's settings sit over the process env; the removals below still win.
        .envs(env)
        // The bearer that guards `/acp` is the shim's, not the harness's, so it is kept out of the
        // child's environment. This is hygiene, not isolation: the harness runs as the same uid as
        // the shim, so code it runs can still read the shim's own environment (`/proc/<shim pid>/
        // environ`, including `/proc/1/environ` under tini). The sandbox is the trust boundary.
        .env_remove(crate::server::SHIM_TOKEN_ENV)
        // Same for the repository credential: it is for the clone, not for code the harness runs
        // (and equally readable by it from the shim's environment, as above).
        .env_remove(crate::workspace::REPO_TOKEN_ENV)
        // The files carrier has been written to disk by now; the harness reads the files.
        .env_remove(crate::files::FILES_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut process =
        OwnChild::spawn(&mut command).with_context(|| format!("spawning harness `{program}`"))?;
    let stdin = process.take_stdin().context("child stdin not piped")?;
    let stdout = process.take_stdout().context("child stdout not piped")?;
    Ok(SpawnedChild {
        process,
        stdin,
        stdout,
    })
}

/// Writes each received frame to the child's stdin followed by a newline.
pub async fn write_stdin(mut stdin: ChildStdin, mut frames: mpsc::Receiver<String>) {
    while let Some(frame) = frames.recv().await {
        let line = format!("{frame}\n");
        if let Err(err) = stdin.write_all(line.as_bytes()).await {
            tracing::warn!(%err, "writing to child stdin failed");
            return;
        }
        if let Err(err) = stdin.flush().await {
            tracing::warn!(%err, "flushing child stdin failed");
            return;
        }
    }
}

/// Calls `on_line` for every stdout line (one JSON-RPC frame each) until the stream ends.
pub async fn read_stdout_lines(stdout: ChildStdout, on_line: impl FnMut(String)) -> StdoutEnd {
    read_lines(BufReader::new(stdout), MAX_FRAME_BYTES, on_line).await
}

/// Line reader with a hard cap on a single line, so a runaway child cannot exhaust memory.
async fn read_lines(
    mut reader: impl AsyncBufRead + Unpin,
    max_line_bytes: usize,
    mut on_line: impl FnMut(String),
) -> StdoutEnd {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let chunk = match reader.fill_buf().await {
            Ok(chunk) => chunk,
            Err(err) => {
                tracing::warn!(%err, "reading child stdout failed");
                return StdoutEnd::Error;
            }
        };
        if chunk.is_empty() {
            emit_line(&mut line, &mut on_line);
            return StdoutEnd::Eof;
        }
        let newline = chunk.iter().position(|&b| b == b'\n');
        let take = newline.unwrap_or(chunk.len());
        if line.len() + take > max_line_bytes {
            tracing::error!(max_line_bytes, "child stdout line exceeds the frame cap");
            return StdoutEnd::Overflow;
        }
        line.extend_from_slice(&chunk[..take]);
        reader.consume(take + usize::from(newline.is_some()));
        if newline.is_some() {
            emit_line(&mut line, &mut on_line);
        }
    }
}

/// Hands a completed line to `on_line` (empty lines are skipped) and clears the accumulator.
fn emit_line(line: &mut Vec<u8>, on_line: &mut impl FnMut(String)) {
    if !line.is_empty() {
        on_line(String::from_utf8_lossy(line).into_owned());
    }
    line.clear();
}

/// Sends SIGTERM, waits up to `grace`, then SIGKILLs.
pub async fn terminate(process: &mut OwnChild, grace: Duration) {
    if let Some(pid) = process.id() {
        // SAFETY: plain signal delivery to a pid we own; no memory is touched.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    }
    if tokio::time::timeout(grace, process.wait()).await.is_err() {
        tracing::warn!("child ignored SIGTERM, sending SIGKILL");
        let _ = process.kill().await;
    }
}

/// The shim's exit code for a child exit: the child's code, or `128 + signal` when signalled.
pub fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn collect(input: &[u8], max: usize) -> (Vec<String>, StdoutEnd) {
        let mut lines = Vec::new();
        let end = read_lines(input, max, |l| lines.push(l)).await;
        (lines, end)
    }

    #[tokio::test]
    async fn splits_lines_and_skips_empty() {
        let (lines, end) = collect(b"a\n\nb\nc", 100).await;
        assert_eq!(lines, vec!["a", "b", "c"]);
        assert_eq!(end, StdoutEnd::Eof);
    }

    #[tokio::test]
    async fn overlong_line_overflows() {
        let (lines, end) = collect(b"ok\nthis line is too long\n", 8).await;
        assert_eq!(lines, vec!["ok"]);
        assert_eq!(end, StdoutEnd::Overflow);
    }
}
