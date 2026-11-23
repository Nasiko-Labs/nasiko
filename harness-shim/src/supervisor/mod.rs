//! `harness-supervisor`: many sessions of one harness image inside one pod (one user's harness
//! agent, so the pod is the boundary between users).
//!
//! Per session it creates the session's directory (and, when the pod has a scratch area, the
//! session's scratch directory in it), starts one `harness-shim` in handshake mode with that
//! directory as its workspace and `HOME`, reports the session's resource use, and stops the shim
//! when the platform suspends or releases the session. The platform tells it which port
//! each session listens on: its placement record owns a session's port for the life of the
//! session, so a port picked here could disagree with the one the platform dials.

mod routes;
mod session;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use tokio::net::TcpListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Mutex as AsyncMutex, oneshot, watch};

use crate::procstats::Sampler;

const LISTEN_ENV: &str = "HARNESS_SUPERVISOR_LISTEN";
/// The bearer guarding every route but `/healthz`. Kept out of the session shims' environment.
const TOKEN_ENV: &str = "HARNESS_SUPERVISOR_TOKEN";
const SHIM_BINARY_ENV: &str = "HARNESS_SHIM_BINARY";
const WORKSPACE_ROOT_ENV: &str = "HARNESS_WORKSPACE_ROOT";
const SCRATCH_ROOT_ENV: &str = "HARNESS_SCRATCH_ROOT";
const MAX_SESSIONS_ENV: &str = "HARNESS_SUPERVISOR_MAX_SESSIONS";

const DEFAULT_LISTEN: &str = "0.0.0.0:7001";
const DEFAULT_SHIM_BINARY: &str = "/usr/local/bin/harness-shim";
const DEFAULT_WORKSPACE_ROOT: &str = "/workspace";
const DEFAULT_MAX_SESSIONS: usize = 8;

/// A session's `HOME`, inside its directory. Everything the supervisor creates for a session lives
/// under it: the shim's clone check (`workspace::needs_clone`) treats any other entry at the top of
/// the directory as the user's content and would skip the session's repository.
const SESSION_HOME: &str = "home";

/// Runtime settings for [`run`].
pub struct SupervisorConfig {
    /// Where the supervisor's own API is served.
    pub listen: SocketAddr,
    /// The bearer every route but `/healthz` requires.
    pub token: String,
    /// The `harness-shim` started once per session.
    pub shim_binary: PathBuf,
    /// The directory holding one subdirectory per session (the pod's volume).
    pub workspace_root: PathBuf,
    /// The directory holding one scratch directory per session, `<root>/<id>`: the pod's
    /// node-local scratch volume, where the server's session env points package caches and build
    /// output (`HARNESS_SCRATCH_ROOT`). `None` (unset) makes none.
    pub scratch_root: Option<PathBuf>,
    /// Running sessions allowed at once; a `PUT` past it is a `409`, which the platform reads as
    /// "place this session elsewhere".
    pub max_sessions: usize,
}

impl SupervisorConfig {
    /// Reads the settings from the environment; a missing token is a startup error.
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Builds the settings from `lookup`, treating an empty value as unset.
    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |key: &str| lookup(key).filter(|value| !value.is_empty());
        let listen = get(LISTEN_ENV).unwrap_or_else(|| DEFAULT_LISTEN.to_owned());
        let listen = listen
            .parse()
            .with_context(|| format!("invalid {LISTEN_ENV} `{listen}`"))?;
        let Some(token) = get(TOKEN_ENV) else {
            bail!("{TOKEN_ENV} must be set: it guards the session API");
        };
        let workspace_root =
            PathBuf::from(get(WORKSPACE_ROOT_ENV).unwrap_or_else(|| DEFAULT_WORKSPACE_ROOT.into()));
        // The shim refuses a relative workspace, so every session would fail to start.
        if !workspace_root.is_absolute() {
            bail!(
                "{WORKSPACE_ROOT_ENV} must be an absolute path, got `{}`",
                workspace_root.display()
            );
        }
        // The server's env names absolute paths in it, so a relative root could never match.
        let scratch_root = get(SCRATCH_ROOT_ENV).map(PathBuf::from);
        if let Some(root) = scratch_root.as_ref().filter(|root| !root.is_absolute()) {
            bail!(
                "{SCRATCH_ROOT_ENV} must be an absolute path, got `{}`",
                root.display()
            );
        }
        Ok(Self {
            listen,
            token,
            shim_binary: get(SHIM_BINARY_ENV)
                .unwrap_or_else(|| DEFAULT_SHIM_BINARY.into())
                .into(),
            workspace_root,
            scratch_root,
            max_sessions: max_sessions(get(MAX_SESSIONS_ENV))?,
        })
    }
}

/// `HARNESS_SUPERVISOR_MAX_SESSIONS`: a positive count, default 8.
fn max_sessions(raw: Option<String>) -> Result<usize> {
    let Some(raw) = raw else {
        return Ok(DEFAULT_MAX_SESSIONS);
    };
    match raw.parse() {
        Ok(0) | Err(_) => bail!("{MAX_SESSIONS_ENV} must be a positive integer, got `{raw}`"),
        Ok(max) => Ok(max),
    }
}

/// Serves the session API until SIGTERM or SIGINT, then stops every session before returning:
/// the pod lives exactly as long as the supervisor, and the session files are already on disk.
///
/// `bound` receives the actual listen address once the socket is bound (for ephemeral ports).
pub async fn run(
    config: SupervisorConfig,
    bound: Option<oneshot::Sender<SocketAddr>>,
) -> Result<()> {
    // A misbuilt image fails here, loudly, rather than as a 500 on every session.
    if !config.shim_binary.is_file() {
        bail!(
            "the shim binary `{}` does not exist",
            config.shim_binary.display()
        );
    }
    // Likewise a pod whose scratch volume is not mounted where it was said to be.
    if let Some(root) = config.scratch_root.as_ref().filter(|root| !root.is_dir()) {
        bail!("{SCRATCH_ROOT_ENV} `{}` is not a directory", root.display());
    }
    let mut terminate = signal(SignalKind::terminate()).context("installing SIGTERM handler")?;
    let mut interrupt = signal(SignalKind::interrupt()).context("installing SIGINT handler")?;
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding {}", config.listen))?;
    let addr = listener
        .local_addr()
        .context("reading the bound listen address")?;
    tracing::info!(
        %addr,
        max_sessions = config.max_sessions,
        "harness supervisor listening"
    );
    if let Some(bound) = bound {
        let _ = bound.send(addr);
    }
    let supervisor = Arc::new(Supervisor::new(config, addr.port()));
    let shutdown = async move {
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
        tracing::info!("shutdown signal received; stopping every session");
    };
    let served = axum::serve(listener, routes::router(supervisor.clone()))
        .with_graceful_shutdown(shutdown)
        .await
        .context("serving the session API");
    // Sessions are stopped however serving ended, so a failed server never orphans them.
    session::stop_all(&supervisor).await;
    served
}

/// One session the supervisor started. Its directory is `<root>/<id>` ([`Supervisor::session_dir`]).
struct Entry {
    port: u16,
    /// Which spawn this is, unique across the supervisor's sessions.
    generation: u64,
    started_at: DateTime<Utc>,
    /// The shim's pid, which is also its process group's id.
    pid: u32,
    /// Becomes `true` once the shim has exited and been reaped.
    exited: watch::Receiver<bool>,
    /// One per session, so each session's CPU rate is measured against its own last sample.
    sampler: Arc<Sampler>,
}

impl Entry {
    fn is_running(&self) -> bool {
        !*self.exited.borrow()
    }
}

/// State shared by the routes.
struct Supervisor {
    config: SupervisorConfig,
    /// The port the supervisor's own API is bound to, which no session may be told to use.
    listen_port: u16,
    /// Keyed by session id (the platform's sandbox key name, e.g. `s-<uuid>`). A stopped
    /// session stays here, `exited`, until it is purged.
    sessions: Mutex<HashMap<String, Entry>>,
    /// One lock per session id, held across a whole ensure, stop or purge of that session.
    lifecycle_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    generation: AtomicU64,
}

impl Supervisor {
    fn new(config: SupervisorConfig, listen_port: u16) -> Self {
        Self {
            config,
            listen_port,
            sessions: Mutex::new(HashMap::new()),
            lifecycle_locks: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(0),
        }
    }

    fn sessions(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The session's directory: its shim's workspace and cwd, with `home/` inside. The id must
    /// already have been checked to be one path segment.
    fn session_dir(&self, id: &str) -> PathBuf {
        self.config.workspace_root.join(id)
    }

    /// The session's scratch directory, `<scratch root>/<id>`, when the pod has a scratch area:
    /// the same name the server's session env uses. The id must already have been checked.
    fn scratch_dir(&self, id: &str) -> Option<PathBuf> {
        self.config.scratch_root.as_ref().map(|root| root.join(id))
    }

    /// `id`'s lifecycle lock, created on first use.
    fn lifecycle_lock(&self, id: &str) -> Arc<AsyncMutex<()>> {
        Arc::clone(self.lifecycle_locks().entry(id.to_owned()).or_default())
    }

    /// Drops `id`'s lock from the map once it guards nothing: no session entry and no holder
    /// but the map and the caller. Clones are made only under the map's mutex, so the count
    /// cannot rise between the check and the removal. The map so holds one lock per known
    /// session plus those in use, never one per id ever seen.
    fn prune_lifecycle_lock(&self, id: &str, lock: &Arc<AsyncMutex<()>>) {
        let mut locks = self.lifecycle_locks();
        if Arc::strong_count(lock) == 2 && !self.sessions().contains_key(id) {
            locks.remove(id);
        }
    }

    fn lifecycle_locks(&self) -> MutexGuard<'_, HashMap<String, Arc<AsyncMutex<()>>>> {
        self.lifecycle_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn running_count(&self) -> usize {
        self.sessions().values().filter(|e| e.is_running()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> Result<SupervisorConfig> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        SupervisorConfig::from_lookup(|key| vars.get(key).cloned())
    }

    #[test]
    fn defaults_apply_when_only_the_token_is_set() {
        let config = config(&[(TOKEN_ENV, "t")]).unwrap();
        assert_eq!(config.listen, DEFAULT_LISTEN.parse().unwrap());
        assert_eq!(config.token, "t");
        assert_eq!(config.shim_binary, PathBuf::from(DEFAULT_SHIM_BINARY));
        assert_eq!(config.workspace_root, PathBuf::from(DEFAULT_WORKSPACE_ROOT));
        assert_eq!(config.scratch_root, None);
        assert_eq!(config.max_sessions, DEFAULT_MAX_SESSIONS);
    }

    #[test]
    fn every_setting_is_read_from_its_variable() {
        let config = config(&[
            (TOKEN_ENV, "t"),
            (LISTEN_ENV, "127.0.0.1:9"),
            (SHIM_BINARY_ENV, "/bin/shim"),
            (WORKSPACE_ROOT_ENV, "/data"),
            (SCRATCH_ROOT_ENV, "/scratch"),
            (MAX_SESSIONS_ENV, "3"),
        ])
        .unwrap();
        assert_eq!(config.listen.port(), 9);
        assert_eq!(config.shim_binary, PathBuf::from("/bin/shim"));
        assert_eq!(config.workspace_root, PathBuf::from("/data"));
        assert_eq!(config.scratch_root, Some(PathBuf::from("/scratch")));
        assert_eq!(config.max_sessions, 3);
    }

    #[test]
    fn a_missing_or_empty_token_is_refused() {
        assert!(config(&[]).is_err());
        assert!(config(&[(TOKEN_ENV, "")]).is_err());
    }

    #[test]
    fn invalid_values_are_refused() {
        for (key, value) in [
            (LISTEN_ENV, "nope"),
            (WORKSPACE_ROOT_ENV, "relative"),
            (SCRATCH_ROOT_ENV, "scratch"),
            (MAX_SESSIONS_ENV, "0"),
            (MAX_SESSIONS_ENV, "-1"),
            (MAX_SESSIONS_ENV, "many"),
        ] {
            assert!(
                config(&[(TOKEN_ENV, "t"), (key, value)]).is_err(),
                "{key}={value}"
            );
        }
    }
}
