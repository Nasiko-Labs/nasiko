//! Conformance suite every `SandboxProvider` must pass. Select the backend with
//! `SANDBOX_CONFORMANCE_PROVIDER` (`fake` by default, or `docker` with the `docker` feature);
//! an unwired value skips vacuously unless `SANDBOX_CONFORMANCE_REQUIRE=1`, which panics.

use std::sync::Arc;

use nasiko_runtime::conformance::{
    IMAGE, agent_key, case_acquire_attach, case_acquire_idempotent, case_changed_spec_keeps_handle,
    case_distinct_keys, case_invalid_specs, case_list, case_release, case_suspend_resume,
    default_spec_factory, spec_for,
};
use nasiko_runtime::sandbox_fake::FakeSandboxProvider;
#[cfg(feature = "docker")]
use nasiko_runtime::{DockerRuntimeConfig, DockerSandboxProvider};
use nasiko_runtime::{SandboxProvider, TransportKind};

/// The provider under test, or `None` when the selected backend is not wired here.
fn provider_under_test() -> Option<Arc<dyn SandboxProvider>> {
    let name = std::env::var("SANDBOX_CONFORMANCE_PROVIDER").unwrap_or_else(|_| "fake".to_owned());
    match name.as_str() {
        "fake" => Some(Arc::new(FakeSandboxProvider::default())),
        #[cfg(feature = "docker")]
        "docker" => Some(Arc::new(DockerSandboxProvider::new(
            bollard::Docker::connect_with_local_defaults().expect("connect to the Docker daemon"),
            DockerRuntimeConfig::default(),
            None,
        ))),
        other => {
            assert!(
                std::env::var("SANDBOX_CONFORMANCE_REQUIRE").as_deref() != Ok("1"),
                "provider {other} not wired in this suite but SANDBOX_CONFORMANCE_REQUIRE=1"
            );
            eprintln!("SKIP: provider {other} not wired in this suite yet");
            None
        }
    }
}

#[tokio::test]
async fn acquire_then_attach_returns_requested_transport() {
    let Some(p) = provider_under_test() else {
        return;
    };
    case_acquire_attach(p.clone(), &default_spec_factory).await;
    case_acquire_attach(p, &|k| spec_for(k, TransportKind::WebSocket)).await;
}

#[tokio::test]
async fn acquire_twice_returns_same_handle() {
    let Some(p) = provider_under_test() else {
        return;
    };
    case_acquire_idempotent(p, &default_spec_factory).await;
}

#[tokio::test]
async fn suspend_blocks_attach_until_reacquired() {
    let Some(p) = provider_under_test() else {
        return;
    };
    case_suspend_resume(p, &default_spec_factory).await;
}

#[tokio::test]
async fn changed_image_keeps_handle() {
    let Some(p) = provider_under_test() else {
        return;
    };
    case_changed_spec_keeps_handle(p, &default_spec_factory).await;
}

#[tokio::test]
async fn release_makes_sandbox_absent_and_is_idempotent() {
    let Some(p) = provider_under_test() else {
        return;
    };
    case_release(p, &default_spec_factory).await;
}

#[tokio::test]
async fn list_reflects_mixed_states() {
    let Some(p) = provider_under_test() else {
        return;
    };
    case_list(p, &default_spec_factory).await;
}

#[tokio::test]
async fn invalid_specs_are_rejected() {
    let Some(p) = provider_under_test() else {
        return;
    };
    case_invalid_specs(p, &default_spec_factory).await;
}

#[tokio::test]
async fn distinct_key_variants_get_distinct_handles() {
    let Some(p) = provider_under_test() else {
        return;
    };
    case_distinct_keys(p, &default_spec_factory).await;
}

// The two fake-only checks build the concrete fake: the hooks are not on the trait and an
// `Arc<dyn SandboxProvider>` cannot be downcast.
#[tokio::test]
async fn fake_counts_acquires() {
    let fake = FakeSandboxProvider::default();
    let key = agent_key();
    fake.acquire(&default_spec_factory(key.clone()))
        .await
        .unwrap();
    fake.acquire(&default_spec_factory(key.clone()))
        .await
        .unwrap();
    assert_eq!(fake.acquire_count(&key), 2);
}

#[tokio::test]
async fn fake_keeps_original_image_on_resume() {
    let fake = FakeSandboxProvider::default();
    let key = agent_key();
    fake.acquire(&default_spec_factory(key.clone()))
        .await
        .unwrap();
    let mut changed = default_spec_factory(key.clone());
    changed.image = "registry.example/other:2".to_owned();
    fake.acquire(&changed).await.unwrap();
    assert_eq!(fake.image_of(&key).as_deref(), Some(IMAGE));
}

/// Creates the sandbox, then reports failure: the shape of an `acquire` that times out
/// waiting for readiness after the backend object already exists.
struct LeakyProvider(Arc<FakeSandboxProvider>);

#[async_trait::async_trait]
impl SandboxProvider for LeakyProvider {
    fn kind(&self) -> nasiko_runtime::SandboxProviderKind {
        self.0.kind()
    }
    async fn acquire(
        &self,
        spec: &nasiko_runtime::SandboxSpec,
    ) -> nasiko_runtime::Result<nasiko_runtime::SandboxHandle> {
        self.0.acquire(spec).await?;
        Err(nasiko_runtime::RuntimeError::Timeout(
            "not ready".to_owned(),
        ))
    }
    async fn attach(
        &self,
        h: &nasiko_runtime::SandboxHandle,
    ) -> nasiko_runtime::Result<nasiko_runtime::SandboxTransport> {
        self.0.attach(h).await
    }
    async fn status(
        &self,
        h: &nasiko_runtime::SandboxHandle,
    ) -> nasiko_runtime::Result<nasiko_runtime::SandboxState> {
        self.0.status(h).await
    }
    async fn suspend(&self, h: &nasiko_runtime::SandboxHandle) -> nasiko_runtime::Result<()> {
        self.0.suspend(h).await
    }
    async fn release(&self, h: &nasiko_runtime::SandboxHandle) -> nasiko_runtime::Result<()> {
        self.0.release(h).await
    }
    async fn resume_takes_new_env(
        &self,
        h: &nasiko_runtime::SandboxHandle,
    ) -> nasiko_runtime::Result<bool> {
        self.0.resume_takes_new_env(h).await
    }
    async fn stats(
        &self,
        h: &nasiko_runtime::SandboxHandle,
    ) -> nasiko_runtime::Result<nasiko_runtime::SandboxStats> {
        self.0.stats(h).await
    }
    async fn list(&self) -> nasiko_runtime::Result<Vec<nasiko_runtime::SandboxInfo>> {
        self.0.list().await
    }
}

#[tokio::test]
async fn cleanup_releases_a_sandbox_whose_acquire_failed_after_creating() {
    let fake = Arc::new(FakeSandboxProvider::default());
    let leaky: Arc<dyn SandboxProvider> = Arc::new(LeakyProvider(fake.clone()));
    let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
        case_acquire_idempotent(leaky, &default_spec_factory),
    ))
    .await;
    assert!(outcome.is_err(), "the case must still fail");
    assert!(
        fake.list().await.unwrap().is_empty(),
        "the leaked sandbox must be released"
    );
}
