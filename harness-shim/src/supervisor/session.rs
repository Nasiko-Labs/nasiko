//! One session's shim: start it on the port the platform chose, wait until it answers, stop it,
//! purge its directories, and sample its process tree. A session's directory is `<root>/<id>`,
//! and its scratch directory, when the pod has a scratch area, `<scratch root>/<id>`.
//!
//! Every lifecycle operation on one session (ensure, stop, purge) runs under that session's own
//! lock, so a `PUT` racing a `DELETE` for the same session waits for it instead of spawning into
//! a directory being removed; different sessions proceed in parallel.

use std::future::Future;
use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::{Entry, SESSION_HOME, Supervisor, TOKEN_ENV};
use crate::child::TERMINATE_GRACE;
use crate::procstats::{self, Sampler};
use crate::reaper::TREE_STOP_GRACE;
use crate::server::SHIM_TOKEN_ENV;

/// How often a fresh shim's `/healthz` is polled, and for how long before the start has failed.
const READY_POLL: Duration = Duration::from_millis(100);
const READY_BUDGET: Duration = Duration::from_secs(10);
/// One probe's own limit, so a shim that accepts but never answers cannot stall the poll.
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const HEALTHZ_REQUEST: &[u8] =
    b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
const HTTP_200: &[u8; 12] = b"HTTP/1.1 200";
/// How long a SIGINTed shim gets before its group is SIGKILLed. A shim shutting down SIGTERMs
/// its harness, waits up to `TERMINATE_GRACE` for it, takes up to 1 s to flush the client's
/// Close, then up to `TREE_STOP_GRACE` stopping what the session left in its tree; the margin
/// lets that orderly shutdown finish before the supervisor's SIGKILL lands.
const STOP_GRACE: Duration = TERMINATE_GRACE
    .saturating_add(TREE_STOP_GRACE)
    .saturating_add(Duration::from_secs(5));
/// How long to wait for the reaper after a SIGKILL.
const KILL_WAIT: Duration = Duration::from_secs(5);
/// The lowest port a session may be told to use: below it binding needs privileges the pod lacks.
const MIN_PORT: u16 = 1024;
/// Longest file name the filesystem takes (`NAME_MAX`).
const MAX_SEGMENT_BYTES: usize = 255;
/// `u+rwx`: what a purge needs on a directory to empty it.
const OWNER_ALL: u32 = 0o700;
/// A session's temporary directory inside its scratch directory, its `TMPDIR`: node-local, and
/// writable where the pod's root filesystem is not.
const SCRATCH_TMP: &str = "tmp";

/// The body of `PUT /sessions/{id}`.
#[derive(Deserialize)]
pub(super) struct EnsureRequest {
    /// The port the platform placed the session on.
    port: u16,
    /// The bearer the shim requires from its first request (`HARNESS_SHIM_TOKEN`). Without one,
    /// the first `configure` sets it, so whoever reaches the shim first owns it. A running
    /// shim keeps the bearer it was started with.
    #[serde(default)]
    shim_token: Option<String>,
}

/// Where a session's shim answers.
#[derive(Serialize, Clone, Copy)]
pub(super) struct Placed {
    port: u16,
    generation: u64,
}

/// Why a session request failed, split by what the caller does about it.
#[derive(Debug)]
pub(super) enum SessionError {
    /// The request is malformed; retrying it cannot help.
    Invalid(String),
    /// This pod cannot place the session as asked: it is full, the port belongs to another
    /// session, or the session already runs on another port.
    Conflict(String),
    /// Starting or cleaning up the session failed on this pod.
    Failed(String),
}

/// Starts the session's shim on the requested port, or returns the one already running there,
/// once it answers `/healthz`. A session whose shim had exited is started again in its
/// directory, under a new generation.
pub(super) async fn ensure(
    sup: &Supervisor,
    id: &str,
    request: &EnsureRequest,
) -> Result<Placed, SessionError> {
    validate_request(id, request, sup.listen_port)?;
    serialized(sup, id, start_or_confirm(sup, id, request)).await
}

/// Stops the session's shim and keeps the session, `exited`, with its directory: the next
/// `ensure` starts it there again. An unknown session is a no-op.
pub(super) async fn stop(sup: &Arc<Supervisor>, id: &str) -> Result<(), SessionError> {
    validate_id(id)?;
    let (sup, owned) = (Arc::clone(sup), id.to_owned());
    detached("stopping", id, async move {
        serialized(&sup, &owned, halt_session(&sup, &owned)).await;
    })
    .await
}

/// Stops the session, removes its directory and its scratch directory, then forgets it. The
/// directories go first, so a failed removal leaves the session listed and the purge can be
/// retried. An unknown session's directories are removed too: the supervisor forgets its
/// sessions when it restarts, and the platform's release still has to clean up after them.
pub(super) async fn purge(sup: &Arc<Supervisor>, id: &str) -> Result<(), SessionError> {
    validate_id(id)?;
    let (sup, owned) = (Arc::clone(sup), id.to_owned());
    detached("purging", id, async move {
        serialized(&sup, &owned, purge_serialized(&sup, &owned)).await
    })
    .await?
}

/// Stops every session at once, each with the full grace.
pub(super) async fn stop_all(sup: &Arc<Supervisor>) {
    let ids: Vec<String> = sup.sessions().keys().cloned().collect();
    let mut stops = JoinSet::new();
    for id in ids {
        let sup = Arc::clone(sup);
        stops.spawn(async move {
            // Ids in the map were validated on the way in.
            let _ = stop(&sup, &id).await;
        });
    }
    stops.join_all().await;
}

/// The session as the routes report it, `None` when there is no such session.
pub(super) async fn view(sup: &Supervisor, id: &str) -> Option<SessionView> {
    let found = sup.sessions().get(id).map(|entry| Unsampled::of(id, entry));
    Some(found?.sample().await)
}

/// Every session the supervisor knows, by id.
pub(super) async fn view_all(sup: &Supervisor) -> Vec<SessionView> {
    let mut found: Vec<Unsampled> = sup
        .sessions()
        .iter()
        .map(|(id, entry)| Unsampled::of(id, entry))
        .collect();
    found.sort_by(|a, b| a.id.cmp(&b.id));
    let mut views = Vec::with_capacity(found.len());
    for session in found {
        views.push(session.sample().await);
    }
    views
}

/// One session in `GET /sessions` and `GET /sessions/{id}`.
#[derive(Serialize)]
pub(super) struct SessionView {
    id: String,
    port: u16,
    generation: u64,
    started_at: DateTime<Utc>,
    state: State,
    /// `None` once the shim has exited, or when sampling its process tree failed.
    stats: Option<Stats>,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum State {
    Running,
    Exited,
}

/// The shim's process tree at one instant, the same figures the shim's own `/control/stats` gives.
#[derive(Serialize)]
struct Stats {
    cpu_millicores: u32,
    memory_bytes: u64,
    /// `None` where `/proc` cannot be read: unknown, never 0.
    processes: Option<u32>,
    sampled_at: DateTime<Utc>,
}

/// A session copied out from under the lock, so sampling can await.
struct Unsampled {
    id: String,
    port: u16,
    generation: u64,
    started_at: DateTime<Utc>,
    pid: u32,
    state: State,
    sampler: Arc<Sampler>,
}

impl Unsampled {
    fn of(id: &str, entry: &Entry) -> Self {
        Self {
            id: id.to_owned(),
            port: entry.port,
            generation: entry.generation,
            started_at: entry.started_at,
            pid: entry.pid,
            state: if entry.is_running() {
                State::Running
            } else {
                State::Exited
            },
            sampler: Arc::clone(&entry.sampler),
        }
    }

    async fn sample(self) -> SessionView {
        let stats = match self.state {
            State::Running => sample_tree(self.pid, &self.sampler).await,
            State::Exited => None,
        };
        SessionView {
            id: self.id,
            port: self.port,
            generation: self.generation,
            started_at: self.started_at,
            state: self.state,
            stats,
        }
    }
}

/// Samples the descendants of the shim `pid` off the async workers; `None` if sampling failed.
async fn sample_tree(pid: u32, sampler: &Sampler) -> Option<Stats> {
    let sample = tokio::task::spawn_blocking(move || procstats::sample(pid))
        .await
        .ok()?;
    Some(Stats {
        cpu_millicores: sampler.record(sample),
        memory_bytes: sample.memory_bytes,
        processes: sample.processes,
        sampled_at: Utc::now(),
    })
}

/// Runs `operation` as a task of its own and waits for it. The operation takes the session's
/// lifecycle lock inside that task, so a caller that goes away (a dropped `DELETE`) cannot
/// release the lock while the stop or purge is still under way.
async fn detached<T: Send + 'static>(
    action: &str,
    id: &str,
    operation: impl Future<Output = T> + Send + 'static,
) -> Result<T, SessionError> {
    tokio::spawn(operation)
        .await
        .map_err(|err| SessionError::Failed(format!("{action} session {id}: {err}")))
}

/// The body of [`purge`], under the session's lifecycle lock.
async fn purge_serialized(sup: &Supervisor, id: &str) -> Result<(), SessionError> {
    halt_session(sup, id).await;
    remove_dir(&sup.session_dir(id)).await?;
    if let Some(scratch) = sup.scratch_dir(id) {
        remove_dir(&scratch).await?;
    }
    sup.sessions().remove(id);
    Ok(())
}

/// Runs `operation` holding `id`'s lifecycle lock, then lets the lock go from the map if it
/// guards nothing any more.
async fn serialized<T>(sup: &Supervisor, id: &str, operation: impl Future<Output = T>) -> T {
    let lock = sup.lifecycle_lock(id);
    let held = lock.lock().await;
    let outcome = operation.await;
    sup.prune_lifecycle_lock(id, &lock);
    drop(held);
    outcome
}

/// Starts the shim or confirms the running one, then waits until it answers. A shim that does
/// not answer is torn down, so the next `PUT` starts it afresh instead of failing on the same
/// broken shim again.
async fn start_or_confirm(
    sup: &Supervisor,
    id: &str,
    request: &EnsureRequest,
) -> Result<Placed, SessionError> {
    let Claim {
        placed,
        pid,
        mut exited,
    } = claim(sup, id, request)?;
    if let Err(reason) = wait_ready(placed.port, &mut exited).await {
        kill(id, pid, exited).await;
        return Err(SessionError::Failed(reason));
    }
    Ok(placed)
}

/// A shim `ensure` waits on: one it just started, or the one already running for the session.
struct Claim {
    placed: Placed,
    pid: u32,
    exited: watch::Receiver<bool>,
}

impl Claim {
    fn of(entry: &Entry) -> Self {
        Self {
            placed: Placed {
                port: entry.port,
                generation: entry.generation,
            },
            pid: entry.pid,
            exited: entry.exited.clone(),
        }
    }
}

/// Under the sessions lock, so two sessions can never take the same port or the last slot:
/// returns the session's running shim, or starts one after checking the port and the capacity.
fn claim(sup: &Supervisor, id: &str, request: &EnsureRequest) -> Result<Claim, SessionError> {
    let mut sessions = sup.sessions();
    if let Some(entry) = sessions.get(id).filter(|entry| entry.is_running()) {
        if entry.port != request.port {
            return Err(SessionError::Conflict(format!(
                "session {id} already runs on port {}, not {}",
                entry.port, request.port
            )));
        }
        return Ok(Claim::of(entry));
    }
    let running: Vec<(&String, &Entry)> = sessions
        .iter()
        .filter(|(_, entry)| entry.is_running())
        .collect();
    if let Some((holder, _)) = running.iter().find(|(_, e)| e.port == request.port) {
        return Err(SessionError::Conflict(format!(
            "port {} is held by session {holder}",
            request.port
        )));
    }
    if running.len() >= sup.config.max_sessions {
        return Err(SessionError::Conflict(format!(
            "this pod is full: {} sessions running",
            running.len()
        )));
    }
    let entry = start_shim(sup, id, request)?;
    let claim = Claim::of(&entry);
    sessions.insert(id.to_owned(), entry);
    Ok(claim)
}

/// Creates the session's directory with `home/` and its scratch directory, and spawns its shim.
/// A session with a scratch directory takes its `TMPDIR` there (`<scratch>/tmp`, created here):
/// node-local, and writable on a pod whose root filesystem is read-only.
fn start_shim(sup: &Supervisor, id: &str, request: &EnsureRequest) -> Result<Entry, SessionError> {
    let dir = sup.session_dir(id);
    create_dir(&dir.join(SESSION_HOME))?;
    let scratch = sup.scratch_dir(id);
    if let Some(scratch) = &scratch {
        create_dir(&scratch.join(SCRATCH_TMP))?;
    }
    let binary = &sup.config.shim_binary;
    let mut command = shim_command(binary, &dir, request);
    if let Some(scratch) = &scratch {
        command.env("TMPDIR", scratch.join(SCRATCH_TMP));
    }
    let child = command
        .spawn()
        .map_err(|err| SessionError::Failed(format!("spawning {}: {err}", binary.display())))?;
    let pid = child
        .id()
        .ok_or_else(|| SessionError::Failed("the shim exited before it was watched".into()))?;
    tracing::info!(session = %id, pid, port = request.port, dir = %dir.display(), "session shim started");
    Ok(Entry {
        port: request.port,
        generation: sup.next_generation(),
        started_at: Utc::now(),
        pid,
        exited: watch_exit(id, child, pid),
        sampler: Arc::default(),
    })
}

/// The shim for one session: handshake mode on the requested port, working in `dir` with
/// `dir/home` as `HOME`.
fn shim_command(binary: &Path, dir: &Path, request: &EnsureRequest) -> Command {
    let mut command = Command::new(binary);
    command
        .arg("--workspace")
        .arg(dir)
        .current_dir(dir)
        // Its own group, so a stop reaches the harness and whatever it started, and a Ctrl-C at
        // the supervisor's terminal does not reach the sessions behind its back.
        .process_group(0)
        .env("HARNESS_SHIM_MODE", "handshake")
        .env("HARNESS_SHIM_LISTEN", format!("0.0.0.0:{}", request.port))
        .env("HOME", dir.join(SESSION_HOME))
        // The supervisor's bearer is not the session's. Hygiene, not isolation: a session runs as
        // the supervisor's uid and could still read `/proc/<supervisor pid>/environ`.
        .env_remove(TOKEN_ENV)
        .stdin(Stdio::null())
        // Dropping the reaper (a runtime shutting down) kills the shim rather than orphaning it.
        .kill_on_drop(true);
    match &request.shim_token {
        // Preset, so there is no window in which the first caller to configure the shim sets it.
        Some(token) => command.env(SHIM_TOKEN_ENV, token),
        // Never inherited: a bearer in the supervisor's own environment is not this session's.
        None => command.env_remove(SHIM_TOKEN_ENV),
    };
    command
}

/// Reaps the shim in the background; the returned flag turns `true` once it is gone. Whatever
/// it left in its group (a harness it could not stop) is killed with it, so an exited session
/// holds no process and no port.
fn watch_exit(id: &str, mut child: Child, pid: u32) -> watch::Receiver<bool> {
    let (exited_tx, exited) = watch::channel(false);
    let id = id.to_owned();
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => tracing::info!(session = %id, pid, %status, "session shim exited"),
            Err(err) => tracing::warn!(session = %id, pid, %err, "reaping the session shim failed"),
        }
        signal_group(pid, libc::SIGKILL);
        let _ = exited_tx.send(true);
    });
    exited
}

/// Polls the shim's `/healthz` until it answers 200, the shim exits, or the budget runs out.
async fn wait_ready(port: u16, exited: &mut watch::Receiver<bool>) -> Result<(), String> {
    let deadline = Instant::now() + READY_BUDGET;
    loop {
        if *exited.borrow() {
            return Err(format!(
                "the shim exited before answering /healthz on port {port}"
            ));
        }
        if healthz_answers(port).await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the shim did not answer /healthz on port {port} within {}s",
                READY_BUDGET.as_secs()
            ));
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

/// One `GET /healthz` on the loopback over a bare socket (the crate has no HTTP client).
async fn healthz_answers(port: u16) -> bool {
    let probe = async {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await?;
        stream.write_all(HEALTHZ_REQUEST).await?;
        let mut status = [0u8; HTTP_200.len()];
        stream.read_exact(&mut status).await?;
        Ok::<_, std::io::Error>(status == *HTTP_200)
    };
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, probe).await,
        Ok(Ok(true))
    )
}

/// Stops the session's shim if it is running.
async fn halt_session(sup: &Supervisor, id: &str) {
    let running = sup
        .sessions()
        .get(id)
        .filter(|entry| entry.is_running())
        .map(|entry| (entry.pid, entry.exited.clone()));
    let Some((pid, exited)) = running else {
        return;
    };
    halt(id, pid, exited).await;
}

/// SIGINT to the shim's group (a handshake shim exits on SIGINT; SIGTERM would only mark it
/// evicting), then SIGKILL when the grace runs out.
async fn halt(id: &str, pid: u32, mut exited: watch::Receiver<bool>) {
    signal_group(pid, libc::SIGINT);
    if gone_within(&mut exited, STOP_GRACE).await {
        return;
    }
    tracing::warn!(session = %id, pid, "session shim still running after SIGINT; killing its group");
    kill(id, pid, exited).await;
}

/// SIGKILLs the shim's whole tree, then its group: what the session left (re-parented to the
/// shim, its subreaper, and possibly in another group or session) dies with it rather than
/// outliving the shim under the pod's init.
async fn kill(id: &str, pid: u32, mut exited: watch::Receiver<bool>) {
    let left = procstats::descendant_processes(pid).unwrap_or_default();
    crate::reaper::signal_all(&left, libc::SIGKILL);
    signal_group(pid, libc::SIGKILL);
    if !gone_within(&mut exited, KILL_WAIT).await {
        tracing::error!(session = %id, pid, "session shim survived SIGKILL");
    }
}

/// Whether the shim is gone within `limit`. A closed channel counts: the reaper was dropped,
/// and dropping it killed the shim.
async fn gone_within(exited: &mut watch::Receiver<bool>, limit: Duration) -> bool {
    tokio::time::timeout(limit, exited.wait_for(|gone| *gone))
        .await
        .is_ok()
}

/// Sends `signal` to the process group `pgid` (the shim's pid: it leads its own group).
fn signal_group(pgid: u32, signal: libc::c_int) {
    // 0 and 1 would address the supervisor's own group or every process; a child is neither.
    let Some(pgid) = libc::pid_t::try_from(pgid).ok().filter(|id| *id > 1) else {
        return;
    };
    // SAFETY: signal delivery only, to the group the supervisor created for this session.
    unsafe { libc::kill(-pgid, signal) };
}

fn create_dir(dir: &Path) -> Result<(), SessionError> {
    std::fs::create_dir_all(dir)
        .map_err(|err| SessionError::Failed(format!("creating {}: {err}", dir.display())))
}

/// Removes `dir` and everything in it; a missing `dir` is fine.
async fn remove_dir(dir: &Path) -> Result<(), SessionError> {
    let owned = dir.to_owned();
    let removed = tokio::task::spawn_blocking(move || remove_tree(&owned))
        .await
        .unwrap_or_else(|err| Err(std::io::Error::other(err)));
    match removed {
        Err(err) if err.kind() != ErrorKind::NotFound => Err(SessionError::Failed(format!(
            "removing {}: {err}",
            dir.display()
        ))),
        _ => Ok(()),
    }
}

/// `remove_dir_all`, retried once the directories under `dir` are writable when a read-only one
/// refused it. Go makes every directory of its module cache read-only, and the supervisor runs as
/// the sessions' own uid, which cannot unlink entries from a directory it may not write.
fn remove_tree(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
            make_writable(dir)?;
            std::fs::remove_dir_all(dir)
        }
        outcome => outcome,
    }
}

/// Gives the owner full access to `dir` and every directory below it. A symbolic link is never
/// followed: only entries that are directories themselves are changed and descended into.
///
/// Known limitation: the walk goes by path, so a directory swapped for a link between the type
/// check and the `chmod` would have the link's target changed. The session is stopped before a
/// purge and runs as the same uid, so this gains it nothing it could not already do, and every
/// session in the pod belongs to the same user.
fn make_writable(dir: &Path) -> std::io::Result<()> {
    let mut permissions = std::fs::symlink_metadata(dir)?.permissions();
    permissions.set_mode(permissions.mode() | OWNER_ALL);
    std::fs::set_permissions(dir, permissions)?;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            make_writable(&entry.path())?;
        }
    }
    Ok(())
}

/// Refuses an id that is not one plain path segment, a port a session cannot or must not use,
/// and an empty shim bearer.
fn validate_request(id: &str, request: &EnsureRequest, own_port: u16) -> Result<(), SessionError> {
    validate_id(id)?;
    if request.port < MIN_PORT {
        return Err(SessionError::Invalid(format!(
            "port {} is below {MIN_PORT}",
            request.port
        )));
    }
    // The supervisor's own `/healthz` would answer the readiness probe for a shim that never bound.
    if request.port == own_port {
        return Err(SessionError::Invalid(format!(
            "port {own_port} is the supervisor's own"
        )));
    }
    if request.shim_token.as_deref() == Some("") {
        return Err(SessionError::Invalid("shim_token must not be empty".into()));
    }
    Ok(())
}

/// The id names the session's directory, so it is checked before any path is built from it.
fn validate_id(id: &str) -> Result<(), SessionError> {
    if is_segment(id) {
        return Ok(());
    }
    Err(SessionError::Invalid(format!(
        "session id `{id}` must be one path segment of [A-Za-z0-9._-]"
    )))
}

/// One directory entry under the workspace root: no separator and no `.`/`..`, so joining it
/// can never leave the root.
fn is_segment(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SEGMENT_BYTES
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN_PORT: u16 = 7001;

    fn request(port: u16, shim_token: Option<&str>) -> EnsureRequest {
        EnsureRequest {
            port,
            shim_token: shim_token.map(str::to_owned),
        }
    }

    fn refused(id: &str, request: &EnsureRequest) -> bool {
        matches!(
            validate_request(id, request, OWN_PORT),
            Err(SessionError::Invalid(_))
        )
    }

    #[test]
    fn a_segment_is_one_plain_directory_name() {
        for good in ["s-a", "s-0f3c_1.2", "A", "...", ".hidden"] {
            assert!(is_segment(good), "{good}");
        }
        for bad in [
            "", ".", "..", "a/b", "../x", "/abs", "a b", "a\\b", "ü", "a\0",
        ] {
            assert!(!is_segment(bad), "{bad:?}");
        }
        assert!(is_segment(&"x".repeat(MAX_SEGMENT_BYTES)));
        assert!(!is_segment(&"x".repeat(MAX_SEGMENT_BYTES + 1)));
    }

    fn read_only(dir: &Path) {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    }

    /// The shape Go leaves its module cache in: read-only directories holding files.
    #[tokio::test]
    async fn a_purge_removes_read_only_directories() {
        let root = tempfile::tempdir().unwrap();
        let scratch = root.path().join("s-a");
        let module = scratch.join("go-mod/example.com/m@v1.0.0");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(module.join("go.mod"), "module m\n").unwrap();
        read_only(&module);
        read_only(module.parent().unwrap());

        remove_dir(&scratch).await.unwrap();
        assert!(!scratch.exists());
        remove_dir(&scratch).await.unwrap();
    }

    #[tokio::test]
    async fn making_a_tree_writable_never_follows_a_link_out_of_it() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        read_only(&outside);
        let scratch = root.path().join("s-a");
        let locked = scratch.join("locked");
        std::fs::create_dir_all(locked.join("inner")).unwrap();
        std::os::unix::fs::symlink(&outside, scratch.join("link")).unwrap();
        read_only(&locked);

        remove_dir(&scratch).await.unwrap();
        assert!(!scratch.exists());
        let mode = std::fs::metadata(&outside).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o555, "the link's target was changed");
    }

    #[test]
    fn validate_checks_the_id_the_port_and_the_bearer() {
        assert!(validate_request("s-a", &request(MIN_PORT, None), OWN_PORT).is_ok());
        assert!(validate_request("s-a", &request(u16::MAX, Some("t")), OWN_PORT).is_ok());
        assert!(refused("../x", &request(MIN_PORT, None)));
        assert!(refused("s-a", &request(MIN_PORT - 1, None)));
        assert!(refused("s-a", &request(OWN_PORT, None)));
        assert!(refused("s-a", &request(MIN_PORT, Some(""))));
    }
}
