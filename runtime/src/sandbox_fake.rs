//! In-memory `SandboxProvider` for hermetic tests and the simulated runtime.
//!
//! A double, not a simulator: state is a map, `acquire` returns at once, and nothing listens
//! on the URLs it reports.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::{
    Result, RuntimeError, SandboxHandle, SandboxInfo, SandboxKey, SandboxProvider,
    SandboxProviderKind, SandboxSpec, SandboxState, SandboxStats, SandboxTransport, TransportKind,
};

struct Entry {
    /// The spec the sandbox was created with; a resume ignores any changed spec, except for its
    /// env when the fake is set to take the resuming env (`set_resume_takes_new_env`).
    spec: SandboxSpec,
    state: SandboxState,
    /// `None` models a backend that does not report a start time.
    started_at: Option<DateTime<Utc>>,
    stats: Option<SandboxStats>,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<SandboxKey, Entry>,
    acquire_counts: HashMap<SandboxKey, usize>,
    creations: HashMap<SandboxKey, usize>,
    fail_next_acquire: Option<RuntimeError>,
    fail_next_suspend: Option<RuntimeError>,
    transport_urls: HashMap<SandboxKey, String>,
    last_acquire_specs: HashMap<SandboxKey, SandboxSpec>,
    acquire_delay: Option<std::time::Duration>,
    suspend_delay: Option<std::time::Duration>,
    release_delay: Option<std::time::Duration>,
    fail_next_release: Option<RuntimeError>,
    fail_next_list: Option<RuntimeError>,
    unavailable: bool,
    resume_takes_new_env: bool,
}

/// In-memory [`SandboxProvider`]. Test-only: the composition root rejects `fake:` refs.
#[derive(Default)]
pub struct FakeSandboxProvider {
    inner: Mutex<Inner>,
}

impl FakeSandboxProvider {
    /// Test hook: scripts the reading `stats` returns for this sandbox. No-op if it is Absent.
    /// Unscripted, `stats` returns zero CPU and memory sampled at call time.
    pub fn set_stats(&self, handle: &SandboxHandle, stats: SandboxStats) {
        if let Some(entry) = self.lock().entries.get_mut(&handle.key) {
            entry.stats = Some(stats);
        }
    }

    /// Test-only: forces this sandbox into `state` (for instance `Starting`, which no
    /// operation here reaches on its own). No-op if it is Absent.
    pub fn set_state(&self, handle: &SandboxHandle, state: SandboxState) {
        if let Some(entry) = self.lock().entries.get_mut(&handle.key) {
            entry.state = state;
        }
    }

    /// Test-only: makes the next `suspend` fail with `error`, leaving the sandbox as it was.
    pub fn fail_next_suspend(&self, error: RuntimeError) {
        self.lock().fail_next_suspend = Some(error);
    }

    /// Test-only: sets the start time `list` reports for this sandbox; `None` models a backend
    /// that reports none. No-op if it is Absent.
    pub fn set_started_at(&self, handle: &SandboxHandle, started_at: Option<DateTime<Utc>>) {
        if let Some(entry) = self.lock().entries.get_mut(&handle.key) {
            entry.started_at = started_at;
        }
    }

    /// Test hook: how many times `acquire` was called for this key. Calls rejected by spec
    /// validation are not counted; an injected failure is.
    pub fn acquire_count(&self, key: &SandboxKey) -> usize {
        self.lock().acquire_counts.get(key).copied().unwrap_or(0)
    }

    /// Test hook: how many times `acquire` created this key's sandbox afresh rather than resuming
    /// it (an acquire after a `release` counts again), so a test can tell a sandbox that kept its
    /// state from a rebuilt one.
    pub fn creations(&self, key: &SandboxKey) -> usize {
        self.lock().creations.get(key).copied().unwrap_or(0)
    }

    /// Test-only: makes `attach` report `url` for this key's WebSocket or HTTP transport, so a
    /// test can point it at a real in-process server (a harness shim) instead of the default
    /// `fake-...` host nothing listens on. The override outlives `release`; without one the
    /// default URL is unchanged.
    pub fn set_transport_url(&self, key: &SandboxKey, url: String) {
        self.lock().transport_urls.insert(key.clone(), url);
    }

    /// Test hook: makes the next `suspend` wait this long before doing anything, once, so a test
    /// can act while a suspend is in flight (a slow provider).
    pub fn delay_next_suspend(&self, delay: std::time::Duration) {
        self.lock().suspend_delay = Some(delay);
    }

    /// Test hook: makes the next `release` wait this long before doing anything, once, so a test
    /// can act while a release is in flight.
    pub fn delay_next_release(&self, delay: std::time::Duration) {
        self.lock().release_delay = Some(delay);
    }

    /// Test hook: makes the next `release` fail with `error`, once, leaving the sandbox as it was.
    pub fn fail_next_release(&self, error: RuntimeError) {
        self.lock().fail_next_release = Some(error);
    }

    /// Test hook: makes this provider behave like the placeholder a server wires when its backend
    /// cannot be used: `is_available` is false and `list` fails.
    pub fn set_unavailable(&self, unavailable: bool) {
        self.lock().unavailable = unavailable;
    }

    /// Test hook: makes the next `list` fail with `error`, once.
    pub fn fail_next_list(&self, error: RuntimeError) {
        self.lock().fail_next_list = Some(error);
    }

    /// Test hook: with `true`, a resumed sandbox runs on the resuming spec's env, as the bare pod
    /// (recreated from the resuming spec) and a harness agent's sessions (configured at attach) do, and
    /// `resume_takes_new_env` says so. The default, `false`, keeps the env the sandbox was created
    /// with, as Docker does.
    pub fn set_resume_takes_new_env(&self, takes: bool) {
        self.lock().resume_takes_new_env = takes;
    }

    /// Test hook: makes the next `acquire` wait this long before doing anything, once, so a test
    /// can act while a turn is mid-acquire (a slow provider).
    pub fn delay_next_acquire(&self, delay: std::time::Duration) {
        self.lock().acquire_delay = Some(delay);
    }

    /// Test hook: makes the next `acquire` fail with `error`, once.
    pub fn fail_next_acquire(&self, error: RuntimeError) {
        self.lock().fail_next_acquire = Some(error);
    }

    /// Test hook: the spec the most recent `acquire` for this key was called with, whether or not
    /// it created anything (a resume ignores it). Lets a test check that the caller passes the
    /// same full env every time.
    pub fn last_acquire_spec(&self, key: &SandboxKey) -> Option<SandboxSpec> {
        self.lock().last_acquire_specs.get(key).cloned()
    }

    /// Test hook: the spec the sandbox was created with (a resume keeps it, and its env unless
    /// [`Self::set_resume_takes_new_env`]), so a test can check what the platform put in the
    /// sandbox's env.
    pub fn spec_of(&self, key: &SandboxKey) -> Option<SandboxSpec> {
        self.lock().entries.get(key).map(|e| e.spec.clone())
    }

    /// Test hook: the image the sandbox was created with, if it exists.
    pub fn image_of(&self, key: &SandboxKey) -> Option<String> {
        self.lock().entries.get(key).map(|e| e.spec.image.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn handle_for(key: &SandboxKey) -> SandboxHandle {
    SandboxHandle {
        key: key.clone(),
        provider: SandboxProviderKind::Fake,
        backend_ref: format!("fake-{}", key.name()),
    }
}

#[async_trait]
impl SandboxProvider for FakeSandboxProvider {
    fn is_available(&self) -> bool {
        !self.lock().unavailable
    }

    fn kind(&self) -> SandboxProviderKind {
        SandboxProviderKind::Fake
    }

    async fn acquire(&self, spec: &SandboxSpec) -> Result<SandboxHandle> {
        spec.validate()?;
        let delay = self.lock().acquire_delay.take();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        let mut inner = self.lock();
        *inner.acquire_counts.entry(spec.key.clone()).or_default() += 1;
        inner
            .last_acquire_specs
            .insert(spec.key.clone(), spec.clone());
        if let Some(error) = inner.fail_next_acquire.take() {
            return Err(error);
        }
        let takes_new_env = inner.resume_takes_new_env;
        if !inner.entries.contains_key(&spec.key) {
            *inner.creations.entry(spec.key.clone()).or_default() += 1;
        }
        inner
            .entries
            .entry(spec.key.clone())
            .and_modify(|e| {
                if e.state != SandboxState::Running {
                    e.state = SandboxState::Running;
                    e.started_at = Some(Utc::now());
                    if takes_new_env {
                        e.spec.env = spec.env.clone();
                    }
                }
            })
            .or_insert_with(|| Entry {
                spec: spec.clone(),
                state: SandboxState::Running,
                started_at: Some(Utc::now()),
                stats: None,
            });
        Ok(handle_for(&spec.key))
    }

    async fn attach(&self, handle: &SandboxHandle) -> Result<SandboxTransport> {
        let inner = self.lock();
        let entry = inner
            .entries
            .get(&handle.key)
            .ok_or_else(|| RuntimeError::SandboxNotFound(handle.sandbox_ref()))?;
        if entry.state != SandboxState::Running {
            return Err(RuntimeError::SandboxNotRunning(handle.sandbox_ref()));
        }
        let host = format!("fake-{}:{}", handle.key.name(), entry.spec.ports[0]);
        let overridden = inner.transport_urls.get(&handle.key).cloned();
        Ok(match entry.spec.transport {
            TransportKind::Http => SandboxTransport::Http {
                url: overridden.unwrap_or_else(|| format!("http://{host}")),
            },
            TransportKind::WebSocket => SandboxTransport::WebSocket {
                url: overridden.unwrap_or_else(|| format!("ws://{host}/acp")),
                headers: vec![],
                tls: None,
            },
        })
    }

    async fn status(&self, handle: &SandboxHandle) -> Result<SandboxState> {
        let inner = self.lock();
        Ok(inner
            .entries
            .get(&handle.key)
            .map_or(SandboxState::Absent, |e| e.state))
    }

    async fn suspend(&self, handle: &SandboxHandle) -> Result<()> {
        let delay = self.lock().suspend_delay.take();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        if let Some(error) = self.lock().fail_next_suspend.take() {
            return Err(error);
        }
        if let Some(entry) = self.lock().entries.get_mut(&handle.key) {
            entry.state = SandboxState::Suspended;
        }
        Ok(())
    }

    async fn release(&self, handle: &SandboxHandle) -> Result<()> {
        let delay = self.lock().release_delay.take();
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        if let Some(error) = self.lock().fail_next_release.take() {
            return Err(error);
        }
        self.lock().entries.remove(&handle.key);
        Ok(())
    }

    async fn resume_takes_new_env(&self, _handle: &SandboxHandle) -> Result<bool> {
        Ok(self.lock().resume_takes_new_env)
    }

    async fn stats(&self, handle: &SandboxHandle) -> Result<SandboxStats> {
        let inner = self.lock();
        let entry = inner
            .entries
            .get(&handle.key)
            .ok_or_else(|| RuntimeError::SandboxNotFound(handle.sandbox_ref()))?;
        if entry.state != SandboxState::Running {
            return Err(RuntimeError::SandboxNotRunning(handle.sandbox_ref()));
        }
        Ok(entry.stats.clone().unwrap_or(SandboxStats {
            sampled_at: Utc::now(),
            ..Default::default()
        }))
    }

    async fn list(&self) -> Result<Vec<SandboxInfo>> {
        let mut inner = self.lock();
        if inner.unavailable {
            return Err(RuntimeError::Unsupported(
                "sandbox provider unavailable".into(),
            ));
        }
        if let Some(error) = inner.fail_next_list.take() {
            return Err(error);
        }
        Ok(inner
            .entries
            .iter()
            .map(|(key, e)| SandboxInfo {
                handle: handle_for(key),
                state: e.state,
                started_at: e.started_at,
            })
            .collect())
    }
}
