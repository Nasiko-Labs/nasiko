//! Conformance suite every `SandboxProvider` must pass. Provider crates call
//! [`run_conformance`] (or the individual `case_*` functions) against their own backend.

use std::collections::HashMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, resume_unwind};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::FutureExt;
use uuid::Uuid;

use crate::{
    Result, RuntimeError, SandboxHandle, SandboxInfo, SandboxKey, SandboxProvider,
    SandboxProviderKind, SandboxSpec, SandboxState, SandboxStats, SandboxTransport, TransportKind,
};

/// Pullable and serving HTTP on `PORT`, so a real backend can be reached.
pub const IMAGE: &str = "busybox:1.36";
pub const PORT: u16 = 8000;

type SpecFactory<'a> = &'a dyn Fn(SandboxKey) -> SandboxSpec;

pub fn spec_for(key: SandboxKey, transport: TransportKind) -> SandboxSpec {
    SandboxSpec {
        key,
        image: IMAGE.to_owned(),
        command: ["httpd", "-f", "-p", "8000"].map(str::to_owned).to_vec(),
        env: HashMap::new(),
        resources: None,
        workspace: None,
        runtime_class: None,
        ports: vec![PORT],
        transport,
        image_pull_secret: None,
    }
}

pub fn default_spec_factory(key: SandboxKey) -> SandboxSpec {
    spec_for(key, TransportKind::Http)
}

pub fn agent_key() -> SandboxKey {
    SandboxKey::Agent {
        agent_id: Uuid::new_v4(),
    }
}

/// `(scheme, host, port, path)` of `scheme://host[:port][/path]`. A URL without an explicit
/// port (a provider that routes through a gateway on the scheme's default port) gets that
/// default: 443 for `https`/`wss`, 80 otherwise.
fn split_url(url: &str) -> (&str, &str, u16, &str) {
    let (scheme, rest) = url.split_once("://").expect("url has a scheme");
    let (authority, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
    let default_port = if matches!(scheme, "https" | "wss") {
        443
    } else {
        80
    };
    match authority.rsplit_once(':') {
        Some((host, port)) => (scheme, host, port.parse().expect("numeric port"), path),
        None => (scheme, authority, default_port, path),
    }
}

async fn acquire(
    provider: &Arc<dyn SandboxProvider>,
    spec_factory: SpecFactory<'_>,
    key: SandboxKey,
) -> SandboxHandle {
    let handle = provider.acquire(&spec_factory(key.clone())).await.unwrap();
    assert_eq!(handle.key, key);
    assert_eq!(handle.provider, provider.kind());
    assert_eq!(
        SandboxHandle::parse(&handle.sandbox_ref(), key).unwrap(),
        handle
    );
    handle
}

async fn cleanup(provider: &Arc<dyn SandboxProvider>, handles: &[&SandboxHandle]) {
    for handle in handles {
        provider.release(handle).await.unwrap();
    }
}

/// Decorator that remembers every key `acquire` is attempted for and every handle it returns,
/// so a case that panics half way (or an `acquire` that fails after creating something) can
/// still release what it created.
struct RecordingProvider {
    inner: Arc<dyn SandboxProvider>,
    keys: Mutex<Vec<SandboxKey>>,
    handles: Mutex<Vec<SandboxHandle>>,
}

#[async_trait]
impl SandboxProvider for RecordingProvider {
    fn kind(&self) -> SandboxProviderKind {
        self.inner.kind()
    }

    async fn acquire(&self, spec: &SandboxSpec) -> Result<SandboxHandle> {
        self.keys.lock().unwrap().push(spec.key.clone());
        let handle = self.inner.acquire(spec).await?;
        self.handles.lock().unwrap().push(handle.clone());
        Ok(handle)
    }

    async fn attach(&self, handle: &SandboxHandle) -> Result<SandboxTransport> {
        self.inner.attach(handle).await
    }

    async fn status(&self, handle: &SandboxHandle) -> Result<SandboxState> {
        self.inner.status(handle).await
    }

    async fn suspend(&self, handle: &SandboxHandle) -> Result<()> {
        self.inner.suspend(handle).await
    }

    async fn release(&self, handle: &SandboxHandle) -> Result<()> {
        self.inner.release(handle).await
    }

    async fn resume_takes_new_env(&self, handle: &SandboxHandle) -> Result<bool> {
        self.inner.resume_takes_new_env(handle).await
    }

    async fn stats(&self, handle: &SandboxHandle) -> Result<SandboxStats> {
        self.inner.stats(handle).await
    }

    async fn list(&self) -> Result<Vec<SandboxInfo>> {
        self.inner.list().await
    }
}

/// Releases everything the body may have created: every listed sandbox whose key was
/// attempted (catches an `acquire` that failed after creating), then every returned handle.
async fn release_recorded(recorder: &RecordingProvider) {
    let keys = recorder.keys.lock().unwrap().clone();
    let handles = recorder.handles.lock().unwrap().clone();
    let listed = match recorder.inner.list().await {
        Ok(infos) => infos
            .into_iter()
            .filter(|info| keys.contains(&info.handle.key))
            .map(|info| info.handle)
            .collect(),
        Err(err) => {
            eprintln!("conformance cleanup: list failed: {err}");
            Vec::new()
        }
    };
    for handle in listed.iter().chain(&handles) {
        if let Err(err) = recorder.inner.release(handle).await {
            eprintln!(
                "conformance cleanup: release {} failed: {err}",
                handle.backend_ref
            );
        }
    }
}

/// Runs `body` against a recording view of `provider`, always releases everything the body
/// created (even when an assertion panicked), then re-raises the panic.
async fn guarded<F, Fut>(provider: Arc<dyn SandboxProvider>, body: F)
where
    F: FnOnce(Arc<dyn SandboxProvider>) -> Fut,
    Fut: Future<Output = ()>,
{
    let recorder = Arc::new(RecordingProvider {
        inner: provider,
        keys: Mutex::new(Vec::new()),
        handles: Mutex::new(Vec::new()),
    });
    let outcome = AssertUnwindSafe(body(recorder.clone()))
        .catch_unwind()
        .await;
    release_recorded(&recorder).await;
    if let Err(panic) = outcome {
        resume_unwind(panic);
    }
}

/// Runs every case in sequence, so a provider can be exercised with one call.
pub async fn run_conformance(
    provider: Arc<dyn SandboxProvider>,
    spec_factory: impl Fn(SandboxKey) -> SandboxSpec,
) {
    let f: SpecFactory<'_> = &spec_factory;
    case_acquire_attach(provider.clone(), f).await;
    let websocket = |key| SandboxSpec {
        transport: TransportKind::WebSocket,
        image_pull_secret: None,
        ..f(key)
    };
    case_acquire_attach(provider.clone(), &websocket).await;
    case_acquire_idempotent(provider.clone(), f).await;
    case_suspend_resume(provider.clone(), f).await;
    case_changed_spec_keeps_handle(provider.clone(), f).await;
    case_release(provider.clone(), f).await;
    case_list(provider.clone(), f).await;
    case_invalid_specs(provider.clone(), f).await;
    case_distinct_keys(provider, f).await;
}

pub async fn case_acquire_attach(
    provider: Arc<dyn SandboxProvider>,
    spec_factory: SpecFactory<'_>,
) {
    guarded(provider, |p| acquire_attach_body(p, spec_factory)).await;
}

async fn acquire_attach_body(provider: Arc<dyn SandboxProvider>, spec_factory: SpecFactory<'_>) {
    let key = agent_key();
    let spec = spec_factory(key.clone());
    let handle = provider.acquire(&spec).await.unwrap();
    assert_eq!(handle.key, key);
    assert_eq!(handle.provider, provider.kind());
    assert_eq!(
        provider.status(&handle).await.unwrap(),
        SandboxState::Running
    );
    let stats = stats_after_scrape_latency(&provider, &handle).await;
    assert!((Utc::now() - stats.sampled_at).num_seconds().abs() < 60);

    let transport = provider.attach(&handle).await.unwrap();
    // A provider that reaches its sandboxes directly hands out `http`/`ws`; one that routes
    // through a TLS gateway hands out `https`/`wss`. Both are that transport kind.
    let (url, schemes, path) = match (spec.transport, &transport) {
        (TransportKind::Http, SandboxTransport::Http { url }) => (url, ["http", "https"], None),
        (TransportKind::WebSocket, SandboxTransport::WebSocket { url, .. }) => {
            (url, ["ws", "wss"], Some("/acp"))
        }
        _ => panic!("wrong transport kind: {transport:?}"),
    };
    let (got_scheme, host, _port, got_path) = split_url(url);
    assert!(
        schemes.contains(&got_scheme),
        "scheme {got_scheme} not in {schemes:?}"
    );
    assert!(!host.is_empty());
    if let Some(path) = path {
        assert_eq!(got_path, path);
    }
    cleanup(&provider, &[&handle]).await;
}

/// `stats` right after `acquire` may precede the backend's first scrape; retry that one
/// error for up to 30 s, then assert. A reading is never accepted as zero.
async fn stats_after_scrape_latency(
    provider: &Arc<dyn SandboxProvider>,
    handle: &SandboxHandle,
) -> SandboxStats {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match provider.stats(handle).await {
            Err(RuntimeError::Internal(msg))
                if msg.contains("no cpu sample") && std::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            other => return other.unwrap(),
        }
    }
}

pub async fn case_acquire_idempotent(
    provider: Arc<dyn SandboxProvider>,
    spec_factory: SpecFactory<'_>,
) {
    guarded(provider, |p| acquire_idempotent_body(p, spec_factory)).await;
}

async fn acquire_idempotent_body(
    provider: Arc<dyn SandboxProvider>,
    spec_factory: SpecFactory<'_>,
) {
    let key = agent_key();
    let first = acquire(&provider, spec_factory, key.clone()).await;
    let second = acquire(&provider, spec_factory, key).await;
    assert_eq!(first.sandbox_ref(), second.sandbox_ref());
    assert_eq!(first, second);
    cleanup(&provider, &[&first]).await;
}

pub async fn case_suspend_resume(
    provider: Arc<dyn SandboxProvider>,
    spec_factory: SpecFactory<'_>,
) {
    guarded(provider, |p| suspend_resume_body(p, spec_factory)).await;
}

async fn suspend_resume_body(provider: Arc<dyn SandboxProvider>, spec_factory: SpecFactory<'_>) {
    let key = agent_key();
    let handle = acquire(&provider, spec_factory, key.clone()).await;
    provider.suspend(&handle).await.unwrap();
    assert_eq!(
        provider.status(&handle).await.unwrap(),
        SandboxState::Suspended
    );
    assert!(matches!(
        provider.attach(&handle).await,
        Err(RuntimeError::SandboxNotRunning(_))
    ));
    assert!(matches!(
        provider.stats(&handle).await,
        Err(RuntimeError::SandboxNotRunning(_))
    ));
    let again = acquire(&provider, spec_factory, key).await;
    assert_eq!(again, handle);
    assert_eq!(
        provider.status(&again).await.unwrap(),
        SandboxState::Running
    );
    provider.attach(&again).await.unwrap();
    cleanup(&provider, &[&again]).await;
}

pub async fn case_changed_spec_keeps_handle(
    provider: Arc<dyn SandboxProvider>,
    spec_factory: SpecFactory<'_>,
) {
    guarded(provider, |p| {
        changed_spec_keeps_handle_body(p, spec_factory)
    })
    .await;
}

async fn changed_spec_keeps_handle_body(
    provider: Arc<dyn SandboxProvider>,
    spec_factory: SpecFactory<'_>,
) {
    let key = agent_key();
    let first = acquire(&provider, spec_factory, key.clone()).await;
    let mut changed = spec_factory(key);
    changed.image = "registry.example/other:2".to_owned();
    assert_eq!(provider.acquire(&changed).await.unwrap(), first);
    cleanup(&provider, &[&first]).await;
}

pub async fn case_release(provider: Arc<dyn SandboxProvider>, spec_factory: SpecFactory<'_>) {
    guarded(provider, |p| release_body(p, spec_factory)).await;
}

async fn release_body(provider: Arc<dyn SandboxProvider>, spec_factory: SpecFactory<'_>) {
    let handle = acquire(&provider, spec_factory, agent_key()).await;
    provider.release(&handle).await.unwrap();
    assert_eq!(
        provider.status(&handle).await.unwrap(),
        SandboxState::Absent
    );
    provider.suspend(&handle).await.unwrap();
    provider.release(&handle).await.unwrap();
    assert!(matches!(
        provider.attach(&handle).await,
        Err(RuntimeError::SandboxNotFound(_))
    ));
    assert!(matches!(
        provider.stats(&handle).await,
        Err(RuntimeError::SandboxNotFound(_))
    ));
}

pub async fn case_list(provider: Arc<dyn SandboxProvider>, spec_factory: SpecFactory<'_>) {
    guarded(provider, |p| list_body(p, spec_factory)).await;
}

async fn list_body(provider: Arc<dyn SandboxProvider>, spec_factory: SpecFactory<'_>) {
    let running = acquire(&provider, spec_factory, agent_key()).await;
    let suspended = acquire(&provider, spec_factory, agent_key()).await;
    let released = acquire(&provider, spec_factory, agent_key()).await;
    provider.suspend(&suspended).await.unwrap();
    provider.release(&released).await.unwrap();

    let infos = provider.list().await.unwrap();
    let state_of = |h: &SandboxHandle| infos.iter().find(|i| i.handle == *h).map(|i| i.state);
    assert_eq!(state_of(&running), Some(SandboxState::Running));
    assert_eq!(state_of(&suspended), Some(SandboxState::Suspended));
    assert!(matches!(
        state_of(&released),
        None | Some(SandboxState::Absent)
    ));
    cleanup(&provider, &[&running, &suspended]).await;
}

pub async fn case_invalid_specs(provider: Arc<dyn SandboxProvider>, spec_factory: SpecFactory<'_>) {
    guarded(provider, |p| invalid_specs_body(p, spec_factory)).await;
}

async fn invalid_specs_body(provider: Arc<dyn SandboxProvider>, spec_factory: SpecFactory<'_>) {
    let mut empty_image = spec_factory(agent_key());
    empty_image.image.clear();
    let mut port_zero = spec_factory(agent_key());
    port_zero.ports = vec![0];
    let mut no_ports = spec_factory(agent_key());
    no_ports.ports = vec![];
    for spec in [empty_image, port_zero, no_ports] {
        assert!(matches!(
            provider.acquire(&spec).await,
            Err(RuntimeError::InvalidSpec(_))
        ));
        let probe = SandboxHandle {
            backend_ref: format!("conformance-{}", spec.key.name()),
            key: spec.key,
            provider: provider.kind(),
        };
        assert_eq!(provider.status(&probe).await.unwrap(), SandboxState::Absent);
    }
}

pub async fn case_distinct_keys(provider: Arc<dyn SandboxProvider>, spec_factory: SpecFactory<'_>) {
    guarded(provider, |p| distinct_keys_body(p, spec_factory)).await;
}

async fn distinct_keys_body(provider: Arc<dyn SandboxProvider>, spec_factory: SpecFactory<'_>) {
    let id = Uuid::new_v4();
    let keys = [
        SandboxKey::Session { session_id: id },
        SandboxKey::Agent { agent_id: id },
        SandboxKey::AgentContext {
            agent_id: id,
            context_id: "ctx".to_owned(),
        },
    ];
    let mut handles = Vec::new();
    for key in keys {
        handles.push(acquire(&provider, spec_factory, key).await);
    }
    let mut refs: Vec<_> = handles.iter().map(SandboxHandle::sandbox_ref).collect();
    refs.sort();
    refs.dedup();
    assert_eq!(refs.len(), 3);
    cleanup(&provider, &handles.iter().collect::<Vec<_>>()).await;
}
