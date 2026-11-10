//! A [`SandboxProvider`] with a latency model, for scale benchmarks of harness sessions.
//!
//! The in-tree `FakeSandboxProvider` answers at once; real backends take hundreds of
//! milliseconds to resume a sandbox and seconds to create one. This provider keeps the same
//! in-memory state machine but draws each `acquire`'s latency from a log-normal distribution
//! (one for a cold create, one for a resume of a suspended sandbox) and records it as
//! *virtual* time: nothing sleeps, so a 10 000-session run finishes in seconds while the
//! reported wake percentiles still reflect the model.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use nasiko_runtime::{
    Result, RuntimeError, SandboxHandle, SandboxInfo, SandboxKey, SandboxProvider,
    SandboxProviderKind, SandboxSpec, SandboxState, SandboxStats, SandboxTransport, TransportKind,
};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand_distr::{Distribution, LogNormal};

/// CPU a sandbox reports while its activity flag is set.
pub const BUSY_CPU_MILLICORES: u32 = 800;

/// A log-normal latency: `median_ms` is the median, `sigma` the log-space spread (0 = constant).
#[derive(Debug, Clone, Copy)]
pub struct LatencyDist {
    pub median_ms: f64,
    pub sigma: f64,
}

impl LatencyDist {
    fn sample(&self, rng: &mut StdRng) -> Duration {
        let ms = match LogNormal::new(self.median_ms.max(f64::MIN_POSITIVE).ln(), self.sigma) {
            Ok(dist) => dist.sample(rng),
            Err(_) => self.median_ms,
        };
        Duration::from_secs_f64(ms.max(0.0) / 1000.0)
    }
}

/// Acquire latency for the two kinds of wake.
#[derive(Debug, Clone, Copy)]
pub struct LatencyModel {
    /// No sandbox exists for the key yet: pull, create, start.
    pub cold: LatencyDist,
    /// A suspended sandbox is brought back.
    pub resume: LatencyDist,
}

impl Default for LatencyModel {
    fn default() -> Self {
        Self {
            cold: LatencyDist {
                median_ms: 4000.0,
                sigma: 0.4,
            },
            resume: LatencyDist {
                median_ms: 800.0,
                sigma: 0.5,
            },
        }
    }
}

/// How an `acquire` found the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeKind {
    Cold,
    Resume,
}

/// One `acquire` that had to start something (an already-Running sandbox is not a wake).
#[derive(Debug, Clone, Copy)]
pub struct Wake {
    pub kind: WakeKind,
    pub latency: Duration,
}

struct Entry {
    state: SandboxState,
    busy: bool,
    ports: u16,
    transport: TransportKind,
}

struct Inner {
    entries: HashMap<SandboxKey, Entry>,
    wakes: Vec<Wake>,
    rng: StdRng,
}

/// In-memory provider whose `acquire` latency follows a [`LatencyModel`].
pub struct SimulatedSandboxProvider {
    model: LatencyModel,
    inner: Mutex<Inner>,
}

impl SimulatedSandboxProvider {
    /// `seed` fixes the latency draws, so two runs of one scenario report the same numbers.
    pub fn new(model: LatencyModel, seed: u64) -> Self {
        Self {
            model,
            inner: Mutex::new(Inner {
                entries: HashMap::new(),
                wakes: Vec::new(),
                rng: StdRng::seed_from_u64(seed),
            }),
        }
    }

    /// Marks the sandbox busy (its `stats` report CPU) or quiet. No-op if it is Absent.
    pub fn set_busy(&self, handle: &SandboxHandle, busy: bool) {
        if let Some(entry) = self.lock().entries.get_mut(&handle.key) {
            entry.busy = busy;
        }
    }

    /// How many sandboxes are Running now.
    pub fn running_count(&self) -> usize {
        self.count(SandboxState::Running)
    }

    /// How many sandboxes are Suspended now.
    pub fn suspended_count(&self) -> usize {
        self.count(SandboxState::Suspended)
    }

    /// The wakes recorded since the last call, in order.
    pub fn take_wakes(&self) -> Vec<Wake> {
        std::mem::take(&mut self.lock().wakes)
    }

    fn count(&self, state: SandboxState) -> usize {
        self.lock()
            .entries
            .values()
            .filter(|e| e.state == state)
            .count()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn handle_for(key: &SandboxKey) -> SandboxHandle {
    SandboxHandle {
        key: key.clone(),
        provider: SandboxProviderKind::Fake,
        backend_ref: format!("sim-{}", key.name()),
    }
}

#[async_trait]
impl SandboxProvider for SimulatedSandboxProvider {
    /// `Fake`, because the server rejects any stored ref whose kind differs from the wired
    /// provider's and the kind enum has no simulator variant.
    fn kind(&self) -> SandboxProviderKind {
        SandboxProviderKind::Fake
    }

    async fn acquire(&self, spec: &SandboxSpec) -> Result<SandboxHandle> {
        spec.validate()?;
        let mut inner = self.lock();
        let wake = match inner.entries.get(&spec.key).map(|e| e.state) {
            Some(SandboxState::Running) => None,
            Some(_) => Some(WakeKind::Resume),
            None => Some(WakeKind::Cold),
        };
        if let Some(kind) = wake {
            let dist = match kind {
                WakeKind::Cold => self.model.cold,
                WakeKind::Resume => self.model.resume,
            };
            let latency = dist.sample(&mut inner.rng);
            inner.wakes.push(Wake { kind, latency });
        }
        inner
            .entries
            .entry(spec.key.clone())
            .and_modify(|e| e.state = SandboxState::Running)
            .or_insert_with(|| Entry {
                state: SandboxState::Running,
                busy: false,
                ports: spec.ports[0],
                transport: spec.transport,
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
        let host = format!("sim-{}:{}", handle.key.name(), entry.ports);
        Ok(match entry.transport {
            TransportKind::Http => SandboxTransport::Http {
                url: format!("http://{host}"),
            },
            TransportKind::WebSocket => SandboxTransport::WebSocket {
                url: format!("ws://{host}/acp"),
                headers: vec![],
                tls: None,
            },
        })
    }

    async fn status(&self, handle: &SandboxHandle) -> Result<SandboxState> {
        Ok(self
            .lock()
            .entries
            .get(&handle.key)
            .map_or(SandboxState::Absent, |e| e.state))
    }

    async fn suspend(&self, handle: &SandboxHandle) -> Result<()> {
        if let Some(entry) = self.lock().entries.get_mut(&handle.key) {
            entry.state = SandboxState::Suspended;
            entry.busy = false;
        }
        Ok(())
    }

    async fn release(&self, handle: &SandboxHandle) -> Result<()> {
        self.lock().entries.remove(&handle.key);
        Ok(())
    }

    /// It keeps no env, so it answers as the in-tree fake does by default: a resume keeps the
    /// sandbox as created.
    async fn resume_takes_new_env(&self, _handle: &SandboxHandle) -> Result<bool> {
        Ok(false)
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
        Ok(SandboxStats {
            cpu_millicores: if entry.busy { BUSY_CPU_MILLICORES } else { 0 },
            memory_bytes: 0,
            sampled_at: Utc::now(),
            ..Default::default()
        })
    }

    async fn list(&self) -> Result<Vec<SandboxInfo>> {
        Ok(self
            .lock()
            .entries
            .iter()
            .map(|(key, e)| SandboxInfo {
                handle: handle_for(key),
                state: e.state,
                started_at: None,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn spec() -> SandboxSpec {
        SandboxSpec {
            key: SandboxKey::Session {
                session_id: Uuid::new_v4(),
            },
            image: "img:1".into(),
            command: vec![],
            env: HashMap::new(),
            resources: None,
            workspace: None,
            runtime_class: None,
            ports: vec![7000],
            transport: TransportKind::WebSocket,
            image_pull_secret: None,
        }
    }

    fn constant(cold_ms: f64, resume_ms: f64) -> LatencyModel {
        LatencyModel {
            cold: LatencyDist {
                median_ms: cold_ms,
                sigma: 0.0,
            },
            resume: LatencyDist {
                median_ms: resume_ms,
                sigma: 0.0,
            },
        }
    }

    #[tokio::test]
    async fn cold_then_resume_then_running_draw_their_own_latency() {
        let p = SimulatedSandboxProvider::new(constant(4000.0, 800.0), 1);
        let spec = spec();
        let handle = p.acquire(&spec).await.unwrap();
        p.acquire(&spec).await.unwrap(); // already Running: no wake
        p.suspend(&handle).await.unwrap();
        p.acquire(&spec).await.unwrap();

        let wakes = p.take_wakes();
        assert_eq!(wakes.len(), 2);
        assert_eq!(wakes[0].kind, WakeKind::Cold);
        assert_eq!(wakes[0].latency.as_millis(), 4000);
        assert_eq!(wakes[1].kind, WakeKind::Resume);
        assert_eq!(wakes[1].latency.as_millis(), 800);
        assert!(p.take_wakes().is_empty());
    }

    #[tokio::test]
    async fn same_seed_gives_the_same_latencies() {
        let draw = |seed| async move {
            let p = SimulatedSandboxProvider::new(LatencyModel::default(), seed);
            for _ in 0..5 {
                p.acquire(&spec()).await.unwrap();
            }
            p.take_wakes().iter().map(|w| w.latency).collect::<Vec<_>>()
        };
        assert_eq!(draw(7).await, draw(7).await);
        assert_ne!(draw(7).await, draw(8).await);
    }

    #[tokio::test]
    async fn stats_follow_the_busy_flag_and_counts_follow_state() {
        let p = SimulatedSandboxProvider::new(constant(1.0, 1.0), 1);
        let handle = p.acquire(&spec()).await.unwrap();
        assert_eq!(p.stats(&handle).await.unwrap().cpu_millicores, 0);
        p.set_busy(&handle, true);
        assert_eq!(
            p.stats(&handle).await.unwrap().cpu_millicores,
            BUSY_CPU_MILLICORES
        );
        assert_eq!((p.running_count(), p.suspended_count()), (1, 0));

        p.suspend(&handle).await.unwrap();
        assert_eq!((p.running_count(), p.suspended_count()), (0, 1));
        assert!(matches!(
            p.stats(&handle).await,
            Err(RuntimeError::SandboxNotRunning(_))
        ));
        assert_eq!(p.list().await.unwrap().len(), 1);
    }
}
