//! Handshake mode: the shim starts with no session configuration and receives it over HTTP.
//!
//! On a backend whose sandbox environment is fixed on an immutable template and frozen into
//! memory snapshots, a session's settings cannot arrive as process env. The shim instead
//! serves first, answers the wakeup probe at once, and takes its configuration from
//! `POST /control/configure`. The configuration lives in memory only, so a snapshot keeps it.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use std::path::PathBuf;

use serde::Deserialize;
use tokio::sync::Notify;

use crate::files::{FileSpec, expand_home};
use crate::workspace::{CloneSpec, DEFAULT_TIMEOUT};

/// The key (process env in env mode, configure `env` in handshake mode) naming the harness command.
pub const HARNESS_CMD: &str = "HARNESS_CMD";

/// What to say when no command can be found anywhere.
pub const NO_COMMAND_MESSAGE: &str =
    "no harness command: pass `-- <cmd> [args...]` or set HARNESS_CMD";

/// Whether session settings come from the process environment or from `configure`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Env,
    Handshake,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Env => "env",
            Mode::Handshake => "handshake",
        }
    }
}

/// The repository to clone before the first spawn.
#[derive(Deserialize, Clone)]
pub(crate) struct WorkspaceConfig {
    pub repo: String,
    pub token: Option<String>,
    pub timeout_secs: Option<u64>,
}

impl std::fmt::Debug for WorkspaceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceConfig")
            .field("repo", &self.repo)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl WorkspaceConfig {
    fn clone_spec(&self, mount: &std::path::Path) -> CloneSpec {
        CloneSpec {
            repo: self.repo.clone(),
            token: self.token.clone().filter(|t| !t.is_empty()),
            mount: mount.to_path_buf(),
            timeout: self
                .timeout_secs
                .filter(|secs| *secs > 0)
                .map_or(DEFAULT_TIMEOUT, Duration::from_secs),
        }
    }
}

/// The body of `POST /control/configure`.
#[derive(Deserialize, Clone)]
pub(crate) struct Configure {
    pub token: String,
    pub env: HashMap<String, String>,
    pub workspace: Option<WorkspaceConfig>,
    /// Files written before the harness starts (see [`crate::files`]).
    #[serde(default)]
    pub files: Vec<FileSpec>,
    pub env_hash: String,
}

/// Secrets travel in `env` and `token`, so only the env keys are shown.
impl std::fmt::Debug for Configure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut keys: Vec<&String> = self.env.keys().collect();
        keys.sort();
        f.debug_struct("Configure")
            .field("token", &"***")
            .field("env_keys", &keys)
            .field("workspace", &self.workspace)
            .field("files", &self.files)
            .field("env_hash", &self.env_hash)
            .finish()
    }
}

/// What a `configure` did to the stored configuration.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Applied {
    /// The first configuration: the shim can now start the harness.
    First,
    /// A different `env_hash`: stored for the next restart; the running child is untouched.
    Changed,
    /// The same `env_hash`: nothing to do.
    Unchanged,
}

/// Everything needed to start the harness child.
pub(crate) struct Launch {
    pub argv: Vec<String>,
    /// The stored configuration's `env_hash` (none in env mode).
    pub env_hash: Option<String>,
    pub env: HashMap<String, String>,
    /// The child's `HOME`, settled once so the files and the env agree on it.
    pub home: String,
}

impl std::fmt::Debug for Launch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut keys: Vec<&String> = self.env.keys().collect();
        keys.sort();
        f.debug_struct("Launch")
            .field("argv", &self.argv)
            .field("env_hash", &self.env_hash)
            .field("env_keys", &keys)
            .finish()
    }
}

#[derive(Default)]
struct HandshakeState {
    config: Option<Configure>,
    evicting: bool,
    spawned: bool,
    /// Incremented on every spawn.
    generation: u64,
    /// A configure (the first, or one finding the child gone) asked for a (re)spawn that the
    /// supervisor has not taken yet. A flag, not a bare permit, so one that lands while the
    /// supervisor is busy cannot make a later child exit respawn by itself.
    respawn_wanted: bool,
    /// The `env_hash` the current child was launched with.
    running_env_hash: Option<String>,
}

/// Session configuration and the eviction flag, shared with the relay.
pub struct Handshake {
    mode: Mode,
    /// The command used in env mode, or in handshake mode when `configure` names none.
    default_argv: Option<Vec<String>>,
    /// The clone in env mode only: a handshake shim starts after `configure`, whose `workspace`
    /// block (or its absence) decides.
    default_workspace: Option<CloneSpec>,
    /// The files in env mode only (`HARNESS_FILES`); a handshake shim writes what `configure` names.
    default_files: Vec<FileSpec>,
    /// Where a configured `workspace` block is cloned; also the workspace `status` reports.
    mount: PathBuf,
    state: Mutex<HandshakeState>,
    /// Wakes the supervisor to look at `respawn_wanted`; a stale wake finds the flag clear.
    wake: Notify,
}

/// Splits a command string on whitespace, the same rule env mode applies to `HARNESS_CMD`.
pub fn split_command(command: &str) -> Vec<String> {
    command.split_whitespace().map(str::to_owned).collect()
}

impl Handshake {
    pub(crate) fn new(
        mode: Mode,
        default_argv: Option<Vec<String>>,
        default_workspace: Option<CloneSpec>,
        default_files: Vec<FileSpec>,
        mount: PathBuf,
    ) -> Self {
        Self {
            mode,
            default_argv,
            default_workspace,
            default_files,
            mount,
            state: Mutex::new(HandshakeState::default()),
            wake: Notify::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HandshakeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    /// Whether `configure` could start a harness: it names a command, or one is the default.
    pub(crate) fn command_available(&self, configure: &Configure) -> bool {
        let named = configure
            .env
            .get(HARNESS_CMD)
            .is_some_and(|cmd| !split_command(cmd).is_empty());
        named || self.default_argv.is_some()
    }

    /// Stores `configure`. A repeated `env_hash` changes nothing.
    pub(crate) fn apply(&self, configure: Configure) -> Applied {
        let mut state = self.lock();
        let applied = match &state.config {
            None => Applied::First,
            Some(current) if current.env_hash == configure.env_hash => return Applied::Unchanged,
            Some(_) => Applied::Changed,
        };
        state.config = Some(configure);
        applied
    }

    pub(crate) fn is_configured(&self) -> bool {
        self.lock().config.is_some()
    }

    pub(crate) fn env_hash(&self) -> Option<String> {
        self.lock().config.as_ref().map(|c| c.env_hash.clone())
    }

    /// What to run now: the stored configuration over the defaults. `None` while a handshake
    /// shim is unconfigured, or when no command is known.
    ///
    /// Env values starting with `~` are expanded against the child's HOME
    /// (`files::expand_home`); argv is built from the unexpanded `HARNESS_CMD`, so a `~` there
    /// reaches spawn literally.
    pub(crate) fn launch(&self) -> Option<Launch> {
        let state = self.lock();
        let config = state.config.as_ref();
        let argv = config
            .and_then(|c| c.env.get(HARNESS_CMD))
            .map(|cmd| split_command(cmd))
            .filter(|argv| !argv.is_empty())
            .or_else(|| self.default_argv.clone())?;
        if self.mode == Mode::Handshake && config.is_none() {
            return None;
        }
        let mut env = config.map(|c| c.env.clone()).unwrap_or_default();
        let home = self.child_home(&env);
        for value in env.values_mut() {
            *value = expand_home(value, &home);
        }
        Some(Launch {
            argv,
            env_hash: config.map(|c| c.env_hash.clone()),
            env,
            home,
        })
    }

    /// The directory the session works in; reported so the server can use it as the ACP cwd.
    pub(crate) fn mount(&self) -> &std::path::Path {
        &self.mount
    }

    /// The child's `HOME`: the configuration's, else the shim's own, else `home` under the
    /// session's workspace, so sessions sharing a pod never share a fallback home.
    fn child_home(&self, env: &HashMap<String, String>) -> String {
        env.get("HOME")
            .cloned()
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| self.mount.join("home").display().to_string())
    }

    /// The repository to clone before the first spawn.
    pub(crate) fn workspace(&self) -> Option<CloneSpec> {
        let state = self.lock();
        match state.config.as_ref() {
            Some(c) => c.workspace.as_ref().map(|w| w.clone_spec(&self.mount)),
            None => self.default_workspace.clone(),
        }
    }

    /// The files to write before every spawn: the stored configuration's, else env mode's.
    pub(crate) fn files(&self) -> Vec<FileSpec> {
        let state = self.lock();
        match state.config.as_ref() {
            Some(c) => c.files.clone(),
            None => self.default_files.clone(),
        }
    }

    /// Records a spawn from `launch`: the generation advances and its `env_hash` is the one the
    /// child now runs with.
    pub(crate) fn mark_spawned(&self, launch: &Launch) -> u64 {
        let mut state = self.lock();
        state.spawned = true;
        state.generation += 1;
        // The spawn this request asked for has happened.
        state.respawn_wanted = false;
        state.running_env_hash = launch.env_hash.clone();
        state.generation
    }

    /// Asks the supervisor to (re)spawn the child.
    pub(crate) fn want_respawn(&self) {
        self.lock().respawn_wanted = true;
        self.wake.notify_one();
    }

    /// Takes the pending respawn request, if any.
    pub(crate) fn take_respawn_wanted(&self) -> bool {
        std::mem::take(&mut self.lock().respawn_wanted)
    }

    /// Resolves when `want_respawn` may have been called; re-check with `take_respawn_wanted`.
    pub(crate) async fn woken(&self) {
        self.wake.notified().await;
    }

    pub(crate) fn generation(&self) -> u64 {
        self.lock().generation
    }

    pub(crate) fn running_env_hash(&self) -> Option<String> {
        self.lock().running_env_hash.clone()
    }

    pub(crate) fn is_spawned(&self) -> bool {
        self.lock().spawned
    }

    /// SIGTERM arrived in handshake mode: keep running, but say so.
    pub(crate) fn set_evicting(&self) {
        self.lock().evicting = true;
    }

    pub(crate) fn is_evicting(&self) -> bool {
        self.lock().evicting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configure(hash: &str) -> Configure {
        Configure {
            token: "t".into(),
            env: HashMap::from([(HARNESS_CMD.to_owned(), "cat".to_owned())]),
            workspace: None,
            files: Vec::new(),
            env_hash: hash.into(),
        }
    }

    fn handshake() -> Handshake {
        Handshake::new(Mode::Handshake, None, None, Vec::new(), "/workspace".into())
    }

    #[test]
    fn same_hash_is_a_no_op() {
        let h = handshake();
        assert_eq!(h.apply(configure("h1")), Applied::First);
        let mut again = configure("h1");
        again.env.insert("X".into(), "changed".into());
        assert_eq!(h.apply(again), Applied::Unchanged);
        assert!(!h.launch().unwrap().env.contains_key("X"));
    }

    #[test]
    fn a_new_hash_replaces_the_stored_env() {
        let h = handshake();
        h.apply(configure("h1"));
        let mut next = configure("h2");
        next.env.insert("X".into(), "2".into());
        assert_eq!(h.apply(next), Applied::Changed);
        assert_eq!(h.env_hash().as_deref(), Some("h2"));
        assert_eq!(h.launch().unwrap().env["X"], "2");
    }

    #[test]
    fn launch_splits_the_command_on_whitespace() {
        let h = handshake();
        let mut c = configure("h1");
        c.env.insert(HARNESS_CMD.into(), "sh  -c 'x'".into());
        h.apply(c);
        assert_eq!(h.launch().unwrap().argv, vec!["sh", "-c", "'x'"]);
    }

    #[test]
    fn an_unconfigured_handshake_shim_has_nothing_to_launch() {
        assert!(handshake().launch().is_none());
    }

    #[test]
    fn a_configure_without_a_command_needs_a_default() {
        let mut c = configure("h1");
        c.env.clear();
        assert!(!handshake().command_available(&c));
        let with_default = Handshake::new(
            Mode::Handshake,
            Some(vec!["cat".into()]),
            None,
            Vec::new(),
            "/workspace".into(),
        );
        assert!(with_default.command_available(&c));
    }
}
