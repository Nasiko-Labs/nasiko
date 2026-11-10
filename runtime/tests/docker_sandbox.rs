//! `DockerSandboxProvider` against a live Docker daemon.
//!
//! Ignored by default; run with
//!   DOCKER_SANDBOX_TESTS=1 cargo test -p nasiko-runtime --test docker_sandbox -- --ignored

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;
use nasiko_runtime::{
    ContainerId, DockerRuntimeConfig, DockerSandboxProvider, RuntimeError, SandboxHandle,
    SandboxKey, SandboxProvider, SandboxProviderKind, SandboxSpec, SandboxState, SandboxTransport,
    TransportKind, WorkspaceRef,
};
use uuid::Uuid;

fn enabled() -> bool {
    std::env::var("DOCKER_SANDBOX_TESTS").as_deref() == Ok("1")
}

fn spec(key: SandboxKey) -> SandboxSpec {
    SandboxSpec {
        key,
        image: "busybox:1.36".to_owned(),
        command: ["httpd", "-f", "-p", "8000"].map(str::to_owned).to_vec(),
        env: HashMap::new(),
        resources: None,
        workspace: None,
        runtime_class: None,
        ports: vec![8000],
        transport: TransportKind::Http,
        image_pull_secret: None,
    }
}

/// Plain HTTP/1.0 GET; returns the status line.
fn get_status_line(url: &str) -> String {
    let addr = url.strip_prefix("http://").expect("http url");
    let mut stream = TcpStream::connect(addr).expect("connect");
    write!(stream, "GET / HTTP/1.0\r\nHost: x\r\n\r\n").unwrap();
    let mut body = String::new();
    stream.read_to_string(&mut body).unwrap();
    body.lines().next().unwrap_or_default().to_owned()
}

fn http_url(transport: SandboxTransport) -> String {
    match transport {
        SandboxTransport::Http { url } => url,
        other => panic!("expected http transport, got {other:?}"),
    }
}

/// A handle addressing the container by name, which Docker accepts wherever it takes an id.
fn handle_by_name(key: &SandboxKey) -> SandboxHandle {
    SandboxHandle {
        key: key.clone(),
        provider: SandboxProviderKind::Docker,
        backend_ref: format!("nasiko-sbx-{}", key.name()),
    }
}

/// Runs `body`, always releases the sandbox afterwards, then re-raises any panic so a
/// failed assertion never leaks a container.
async fn with_cleanup<F: std::future::Future<Output = ()>>(
    provider: &DockerSandboxProvider,
    key: &SandboxKey,
    body: F,
) {
    with_cleanup_then(provider, key, body, async {}).await;
}

/// [`with_cleanup`] plus `after`, which runs once the sandbox is released and before any
/// panic is re-raised (e.g. removing a throwaway volume the body may have created).
async fn with_cleanup_then<F, G>(
    provider: &DockerSandboxProvider,
    key: &SandboxKey,
    body: F,
    after: G,
) where
    F: std::future::Future<Output = ()>,
    G: std::future::Future<Output = ()>,
{
    let outcome = AssertUnwindSafe(body).catch_unwind().await;
    let _ = provider.release(&handle_by_name(key)).await;
    after.await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
#[ignore = "needs a Docker daemon and DOCKER_SANDBOX_TESTS=1"]
async fn sandbox_lifecycle_acquire_suspend_resume_release() {
    if !enabled() {
        return;
    }
    let client = bollard::Docker::connect_with_local_defaults().unwrap();
    let provider = DockerSandboxProvider::new(client, DockerRuntimeConfig::default(), None);
    let spec = spec(SandboxKey::Session {
        session_id: Uuid::new_v4(),
    });

    with_cleanup(&provider, &spec.key, async {
        let handle = provider.acquire(&spec).await.unwrap();
        let again = provider.acquire(&spec).await.unwrap();
        assert_eq!(handle, again, "acquire is idempotent");
        let url = http_url(provider.attach(&handle).await.unwrap());
        assert!(get_status_line(&url).contains("HTTP/1."), "{url}");
        assert!(provider.stats(&handle).await.unwrap().memory_bytes > 0);
        let listed = provider.list().await.unwrap();
        assert!(
            listed
                .iter()
                .any(|i| i.handle == handle && i.state == SandboxState::Running)
        );
        assert!(
            listed
                .iter()
                .any(|i| i.handle == handle && i.started_at.is_some()),
            "a running sandbox reports when it started"
        );

        provider.suspend(&handle).await.unwrap();
        provider.suspend(&handle).await.unwrap();
        assert_eq!(
            provider.status(&handle).await.unwrap(),
            SandboxState::Suspended
        );
        assert!(matches!(
            provider.attach(&handle).await,
            Err(RuntimeError::SandboxNotRunning(_))
        ));

        let resumed = provider.acquire(&spec).await.unwrap();
        assert_eq!(resumed.backend_ref, handle.backend_ref, "same container");
        let url = http_url(provider.attach(&resumed).await.unwrap());
        assert!(get_status_line(&url).contains("HTTP/1."), "{url}");

        provider.release(&handle).await.unwrap();
        assert_eq!(
            provider.status(&handle).await.unwrap(),
            SandboxState::Absent
        );
        provider.release(&handle).await.unwrap();
        assert!(matches!(
            provider.attach(&handle).await,
            Err(RuntimeError::SandboxNotFound(_))
        ));
        assert!(matches!(
            provider.stats(&handle).await,
            Err(RuntimeError::SandboxNotFound(_))
        ));
    })
    .await;
}

#[tokio::test]
#[ignore = "needs a Docker daemon and DOCKER_SANDBOX_TESTS=1"]
async fn workspace_sandbox_runs_as_uid_65534_and_can_write_to_workspace() {
    if !enabled() {
        return;
    }
    let client = bollard::Docker::connect_with_local_defaults().unwrap();
    // A test-specific volume, removed at the end, so no state leaks between runs.
    let volume = format!("nasiko-sbx-test-{}", Uuid::new_v4());
    let config = DockerRuntimeConfig {
        agent_memory_volume: volume.clone(),
        ..Default::default()
    };
    let provider = DockerSandboxProvider::new(client.clone(), config, None);
    let owner = Uuid::new_v4();
    let mut spec = spec(SandboxKey::Session {
        session_id: Uuid::new_v4(),
    });
    spec.workspace = Some(WorkspaceRef {
        owner_id: owner,
        container_id: ContainerId::from_uuid(owner),
    });
    // `touch` failing exits the shell, so a reachable server proves the write succeeded.
    spec.command = ["sh", "-c", "touch /workspace/ok && httpd -f -p 8000"]
        .map(str::to_owned)
        .to_vec();

    let remove_volume = async {
        let _ = client.remove_volume(&volume, None).await;
    };
    with_cleanup_then(
        &provider,
        &spec.key,
        async {
            let handle = provider.acquire(&spec).await.unwrap();
            let url = http_url(provider.attach(&handle).await.unwrap());
            assert!(get_status_line(&url).contains("HTTP/1."), "{url}");
            let info = client
                .inspect_container(&handle.backend_ref, None)
                .await
                .unwrap();
            assert_eq!(
                info.config.and_then(|c| c.user).as_deref(),
                Some("65534:65534")
            );
        },
        remove_volume,
    )
    .await;
}

/// A session sandbox running as uid 65534 can write its scratch area, again after a suspend,
/// and `release` removes the area's volume.
#[tokio::test]
#[ignore = "needs a Docker daemon and DOCKER_SANDBOX_TESTS=1"]
async fn a_session_sandbox_writes_its_scratch_and_release_removes_it() {
    if !enabled() {
        return;
    }
    let client = bollard::Docker::connect_with_local_defaults().unwrap();
    let volume = format!("nasiko-sbx-test-{}", Uuid::new_v4());
    let config = DockerRuntimeConfig {
        agent_memory_volume: volume.clone(),
        ..Default::default()
    };
    let provider = DockerSandboxProvider::new(client.clone(), config, None);
    let owner = Uuid::new_v4();
    let key = SandboxKey::Session {
        session_id: Uuid::new_v4(),
    };
    let mut spec = spec(key.clone());
    // The workspace makes the sandbox run as uid 65534, the harness images' user.
    spec.workspace = Some(WorkspaceRef {
        owner_id: owner,
        container_id: ContainerId::from_uuid(owner),
    });
    // A failed write exits the shell, so a reachable server proves it succeeded. Both starts
    // (the create and the resume) write.
    spec.command = [
        "sh",
        "-c",
        "mkdir -p /scratch/s/npm && echo x >> /scratch/s/npm/log && httpd -f -p 8000",
    ]
    .map(str::to_owned)
    .to_vec();
    let scratch_volume = format!("nasiko-sbx-{}-scratch", key.name());

    let remove_volume = async {
        let _ = client.remove_volume(&volume, None).await;
    };
    with_cleanup_then(
        &provider,
        &key,
        async {
            let handle = provider.acquire(&spec).await.unwrap();
            provider.suspend(&handle).await.unwrap();
            let resumed = provider.acquire(&spec).await.unwrap();
            let url = http_url(provider.attach(&resumed).await.unwrap());
            assert!(get_status_line(&url).contains("HTTP/1."), "{url}");
            let labels = client.inspect_volume(&scratch_volume).await.unwrap().labels;
            assert_eq!(labels["nasiko.com/sandbox-key"], key.name());
            assert_eq!(labels["nasiko.com/sandbox-kind"], "session");

            provider.release(&handle).await.unwrap();
            assert!(
                client.inspect_volume(&scratch_volume).await.is_err(),
                "release leaves the scratch volume behind"
            );
        },
        remove_volume,
    )
    .await;
}

#[tokio::test]
#[ignore = "needs a Docker daemon and DOCKER_SANDBOX_TESTS=1"]
async fn sandbox_ref_survives_the_container_being_recreated() {
    if !enabled() {
        return;
    }
    let client = bollard::Docker::connect_with_local_defaults().unwrap();
    let provider = DockerSandboxProvider::new(client.clone(), DockerRuntimeConfig::default(), None);
    let spec = spec(SandboxKey::Session {
        session_id: Uuid::new_v4(),
    });

    with_cleanup(&provider, &spec.key, async {
        let first = provider.acquire(&spec).await.unwrap();
        let old_id = client
            .inspect_container(&first.backend_ref, None)
            .await
            .unwrap()
            .id;
        // Remove the container behind the provider's back, as a dead-container recovery would.
        let remove = bollard::container::RemoveContainerOptions {
            force: true,
            ..Default::default()
        };
        client
            .remove_container(&first.backend_ref, Some(remove))
            .await
            .unwrap();

        let second = provider.acquire(&spec).await.unwrap();
        let new_id = client
            .inspect_container(&second.backend_ref, None)
            .await
            .unwrap()
            .id;
        assert_ne!(old_id, new_id, "a genuinely new container");
        assert_eq!(first.sandbox_ref(), second.sandbox_ref());
        assert_eq!(
            provider.status(&first).await.unwrap(),
            SandboxState::Running
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "needs a Docker daemon and DOCKER_SANDBOX_TESTS=1"]
async fn a_first_acquire_that_fails_to_start_leaves_no_container() {
    if !enabled() {
        return;
    }
    let client = bollard::Docker::connect_with_local_defaults().unwrap();
    let config = DockerRuntimeConfig {
        operation_timeout: std::time::Duration::from_secs(3),
        ..DockerRuntimeConfig::default()
    };
    let provider = DockerSandboxProvider::new(client.clone(), config, None);
    // Exits at once, so the readiness wait fails (a host port proxy would accept TCP even for a
    // container that listens on nothing, so a never-listening process is not a reliable failure).
    let mut spec = spec(SandboxKey::Session {
        session_id: Uuid::new_v4(),
    });
    spec.command = vec!["false".to_owned()];

    with_cleanup(&provider, &spec.key, async {
        assert!(provider.acquire(&spec).await.is_err());
        assert_eq!(
            provider.status(&handle_by_name(&spec.key)).await.unwrap(),
            SandboxState::Absent,
            "the container this failed acquire created is removed"
        );
        let scratch_volume = format!("nasiko-sbx-{}-scratch", spec.key.name());
        assert!(
            client.inspect_volume(&scratch_volume).await.is_err(),
            "and so is its scratch volume"
        );
    })
    .await;
}
