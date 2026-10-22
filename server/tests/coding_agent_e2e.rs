//! Live end-to-end (Docker-gated, `#[ignore]`): deploy the real OpenCode image as
//! a coding-agent container, open a per-session stdio attach, bridge it to the
//! ACP client, and run the ACP handshake against the real agent.
//!
//! This wires together the three pieces that only a live agent can exercise:
//!   - `DockerRuntime::deploy` + `attach_stdio` (NAS-837),
//!   - the tokio -> futures-io bridge (`tokio_util::compat` + `ByteStreams`),
//!   - `AcpConnection` (NAS-836) speaking real ACP to real OpenCode.
//!
//! A full prompt is intentionally not attempted: OpenCode needs
//! `opencode auth login` for LLM calls, which isn't available in CI. The
//! handshake (`initialize` + `session/new`) needs no auth and is the strongest
//! signal the whole transport works against the real agent.
//!
//! Run with Docker + the `opencode:0.1.0` image present:
//!   cargo test -p nasiko-server --test coding_agent_e2e -- --ignored --nocapture

use std::collections::HashMap;
use std::time::Duration;

use agent_client_protocol::ByteStreams;
use nasiko_acp_client::{AcpConnectConfig, AcpConnection, NewSessionParams};
use nasiko_runtime::{
    ContainerId, ContainerRuntime, DeploymentSpec, DockerRuntime, DockerRuntimeConfig,
    RuntimeState, SessionAttach, WorkloadKind,
};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

fn coding_spec(id: &ContainerId) -> DeploymentSpec {
    DeploymentSpec {
        container_id: id.clone(),
        name: "test-opencode-acp".to_owned(),
        image: "opencode:0.1.0".to_owned(),
        min_replicas: 1,
        max_replicas: 1,
        replica_target: None,
        env_vars: HashMap::new(),
        ports: Vec::new(),
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: WorkloadKind::AcpCodingAgent,
        // The container-layer /workspace is writable enough for the handshake;
        // the shared agent-memory volume isn't needed just to initialize.
        writable: false,
        writable_path: None,
        owner_id: uuid::Uuid::nil(),
        force_pull: false,
    }
}

#[tokio::test]
#[ignore = "requires Docker + the opencode:0.1.0 image"]
async fn opencode_acp_handshake_through_runtime_attach() {
    let rt = DockerRuntime::new(DockerRuntimeConfig::default())
        .await
        .expect("Docker must be running");
    let id = ContainerId::new("test-opencode-acp");
    let _ = rt.destroy(&id).await;

    // STEP 1 (NAS-837): bring the coding-agent container up.
    rt.deploy(&coding_spec(&id)).await.expect("deploy opencode");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let status = rt.status(&id).await.expect("status");
        if status.state == RuntimeState::Running {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "container never reached Running: {:?}",
            status.state
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // STEP 2 (NAS-837): open a per-session stdio attach that execs `opencode acp`.
    let stdio = rt
        .attach_stdio(
            &id,
            SessionAttach {
                session_id: "s1".to_owned(),
                command: vec!["opencode".to_owned(), "acp".to_owned()],
                env: HashMap::new(),
            },
        )
        .await
        .expect("attach_stdio");

    // STEP 3 (bridge + NAS-836): tokio halves -> futures-io -> ACP transport,
    // then run `initialize` + capability negotiation against the real agent.
    let transport = ByteStreams::new(stdio.stdin.compat_write(), stdio.stdout.compat());
    let conn = AcpConnection::connect(transport, AcpConnectConfig::default())
        .await
        .expect("ACP initialize/negotiate against real opencode");

    // STEP 4: open a session — the next step of a real turn (no prompt; that
    // needs LLM auth). A non-empty agent session id proves session/new worked.
    let session = conn
        .new_session(NewSessionParams::new("/workspace"))
        .await
        .expect("session/new against real opencode");
    assert!(
        !session.0.is_empty(),
        "expected a non-empty agent session id"
    );

    // Dropping `conn` closes the pipe; clean up the container.
    drop(conn);
    rt.destroy(&id).await.ok();
}
