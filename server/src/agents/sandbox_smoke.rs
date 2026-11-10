//! The optional gVisor smoke run after an agent build (`BUILD_SANDBOX_SMOKE=runsc`).
//!
//! Runs the freshly built image once under `--runtime=runsc` through the Docker sandbox provider,
//! waits for its port to accept a TCP connection, stops it, and records the verdict in
//! `agents.sandbox_compatible`. It informs; it never fails the build.

use std::collections::HashMap;
use std::sync::Once;
use std::time::Duration;

use nasiko_config::BuildSandboxSmoke;
use nasiko_runtime::{
    RuntimeError, SandboxKey, SandboxProvider, SandboxProviderKind, SandboxSpec, TransportKind,
};
use uuid::Uuid;

use crate::agents::{qualify_deploy_image, recorded_ports};
use crate::state::AppState;

/// How long the agent has to open its port once its sandbox is running.
const PORT_BUDGET: Duration = Duration::from_secs(60);
const PORT_POLL: Duration = Duration::from_millis(500);
const RUNSC_RUNTIME: &str = "runsc";

/// Why a smoke run did not succeed.
#[derive(Debug)]
enum SmokeFailure {
    Runtime(RuntimeError),
    PortNeverOpened(Duration),
}

/// What the smoke run says about the agent, as stored in `agents.sandbox_compatible`.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// Started and served its port: `true`.
    Compatible,
    /// gVisor is not available on this host, so nothing is known: left NULL.
    GvisorMissing,
    /// Ran and failed, with the reason for the build log: `false`.
    Incompatible(String),
}

fn decide(outcome: Result<(), SmokeFailure>) -> Verdict {
    match outcome {
        Ok(()) => Verdict::Compatible,
        Err(SmokeFailure::Runtime(e)) if names_missing_runtime(&e) => Verdict::GvisorMissing,
        Err(SmokeFailure::Runtime(e)) => Verdict::Incompatible(e.to_string()),
        Err(SmokeFailure::PortNeverOpened(waited)) => Verdict::Incompatible(format!(
            "the agent did not open its port within {}s under runsc",
            waited.as_secs()
        )),
    }
}

/// Docker reports a runtime it does not know ("Unknown runtime specified runsc").
fn names_missing_runtime(e: &RuntimeError) -> bool {
    let message = e.to_string().to_ascii_lowercase();
    message.contains(RUNSC_RUNTIME) || message.contains("unknown runtime")
}

/// Smoke-runs the image `build_id` produced, when the deployment asks for it. Never fails.
pub async fn smoke_after_build(state: &AppState, agent_id: Uuid, build_id: Uuid) {
    if state.config.build_sandbox_smoke != BuildSandboxSmoke::Runsc {
        return;
    }
    if state.sandbox.kind() != SandboxProviderKind::Docker || !state.sandbox.is_available() {
        tracing::debug!(%agent_id, "sandbox smoke run skipped: not the Docker runtime");
        return;
    }
    let Some(spec) = smoke_spec(state, agent_id, build_id).await else {
        return;
    };
    let verdict = decide(run_once(state.sandbox.as_ref(), &spec).await);
    record_verdict(state, agent_id, build_id, verdict).await;
}

async fn smoke_spec(state: &AppState, agent_id: Uuid, build_id: Uuid) -> Option<SandboxSpec> {
    let image: Option<String> =
        sqlx::query_scalar("SELECT image_reference FROM agent_builds WHERE id = $1")
            .bind(build_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let image = image.filter(|i| !i.is_empty())?;
    let mut env = HashMap::new();
    let ports = recorded_ports(&state.db, agent_id, &mut env).await;
    Some(SandboxSpec {
        // A throwaway key: the real agent sandbox, if any, is never touched.
        key: SandboxKey::Session {
            session_id: Uuid::new_v4(),
        },
        image: qualify_deploy_image(&state.config.agent_image_registry, &image),
        command: vec![],
        env,
        resources: None,
        workspace: None,
        runtime_class: Some(RUNSC_RUNTIME.to_owned()),
        ports,
        transport: TransportKind::Http,
        image_pull_secret: None,
    })
}

async fn run_once(provider: &dyn SandboxProvider, spec: &SandboxSpec) -> Result<(), SmokeFailure> {
    let handle = provider
        .acquire(spec)
        .await
        .map_err(SmokeFailure::Runtime)?;
    let reached = match provider.attach(&handle).await {
        Ok(transport) => wait_for_port(&transport_address(&transport)).await,
        Err(e) => Err(SmokeFailure::Runtime(e)),
    };
    if let Err(e) = provider.release(&handle).await {
        tracing::warn!(error = %e, "sandbox smoke run: releasing the container failed");
    }
    reached
}

fn transport_address(transport: &nasiko_runtime::SandboxTransport) -> String {
    let (nasiko_runtime::SandboxTransport::Http { url }
    | nasiko_runtime::SandboxTransport::WebSocket { url, .. }) = transport;
    url.split("://")
        .last()
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

async fn wait_for_port(address: &str) -> Result<(), SmokeFailure> {
    let deadline = tokio::time::Instant::now() + PORT_BUDGET;
    loop {
        if tokio::net::TcpStream::connect(address).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(SmokeFailure::PortNeverOpened(PORT_BUDGET));
        }
        tokio::time::sleep(PORT_POLL).await;
    }
}

async fn record_verdict(state: &AppState, agent_id: Uuid, build_id: Uuid, verdict: Verdict) {
    static WARN_ONCE: Once = Once::new();
    let compatible = match verdict {
        Verdict::Compatible => Some(true),
        Verdict::GvisorMissing => {
            WARN_ONCE.call_once(|| {
                tracing::warn!(
                    "BUILD_SANDBOX_SMOKE=runsc but gVisor (runsc) is not installed on the Docker \
                     host; agents.sandbox_compatible stays unset"
                );
            });
            return;
        }
        Verdict::Incompatible(reason) => {
            append_build_log(
                state,
                build_id,
                &format!("gVisor smoke run failed: {reason}"),
            )
            .await;
            Some(false)
        }
    };
    if let Err(e) = sqlx::query("UPDATE agents SET sandbox_compatible = $2 WHERE id = $1")
        .bind(agent_id)
        .bind(compatible)
        .execute(&state.db)
        .await
    {
        tracing::warn!(%agent_id, error = %e, "sandbox smoke run: recording the verdict failed");
    }
}

async fn append_build_log(state: &AppState, build_id: Uuid, line: &str) {
    let _ = sqlx::query(
        "UPDATE agent_builds SET logs = COALESCE(logs, '') || E'\\n' || $2 WHERE id = $1",
    )
    .bind(build_id)
    .bind(line)
    .execute(&state.db)
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_that_opened_is_compatible() {
        assert_eq!(decide(Ok(())), Verdict::Compatible);
    }

    #[test]
    fn a_host_without_runsc_leaves_the_column_unset() {
        for message in [
            "Unknown runtime specified runsc",
            "unknown runtime: runsc not installed",
        ] {
            let failure = SmokeFailure::Runtime(RuntimeError::Internal(message.into()));
            assert_eq!(decide(Err(failure)), Verdict::GvisorMissing, "{message}");
        }
    }

    #[test]
    fn a_crash_or_silent_port_is_incompatible_with_a_reason() {
        let crashed = SmokeFailure::Runtime(RuntimeError::Internal("container exited (1)".into()));
        assert!(matches!(decide(Err(crashed)), Verdict::Incompatible(r) if r.contains("exited")));
        let silent = SmokeFailure::PortNeverOpened(Duration::from_secs(60));
        assert!(matches!(decide(Err(silent)), Verdict::Incompatible(r) if r.contains("60s")));
    }

    #[test]
    fn the_address_is_the_host_and_port_of_the_attach_url() {
        let http = nasiko_runtime::SandboxTransport::Http {
            url: "http://127.0.0.1:49153/".into(),
        };
        assert_eq!(transport_address(&http), "127.0.0.1:49153");
    }
}
