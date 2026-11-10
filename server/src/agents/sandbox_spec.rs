//! The [`SandboxSpec`] a parked agent is woken with.
//!
//! The same image, port, platform env and default resources a resident deploy of the agent
//! gets (`agents::build_agent_spec`), shaped for a [`SandboxProvider`](nasiko_runtime::SandboxProvider)
//! instead of a long-lived deployment.
//!
//! Image pulls: the Kubernetes pod provider uses `SandboxSpec::image_pull_secret`; the Docker
//! sandbox provider authenticates its pulls from the embedded registry itself
//! (`oss/runtime/src/docker/sandbox.rs`), as `DockerRuntime::deploy` does.

use std::collections::HashMap;

use nasiko_runtime::{
    ContainerId, ResourceLimits, RuntimeError, SandboxKey, SandboxSpec, TransportKind, WorkspaceRef,
};
use uuid::Uuid;

use crate::agent_resolver::AgentRow;
use crate::agents::deploy_env;
use crate::agents::sandbox_credentials;
use crate::agents::{DEFAULT_AGENT_PORT, qualify_deploy_image};
use crate::state::AppState;

/// Matches the CPU request `build_agent_spec` gives every resident agent.
const AGENT_CPU_MILLI: u32 = 500;

/// A spec, and whether building it minted the agent's credentials (so this is the first wake
/// since the agent was deployed, updated or released).
pub struct PreparedSpec {
    pub spec: SandboxSpec,
    pub credentials_minted: bool,
}

/// The spec `acquire` is called with for `agent` under `key`.
///
/// The env is rebuilt in full on every call, as `SandboxProvider::acquire` requires, with the
/// platform's credentials taken from the store rather than minted anew
/// ([`sandbox_credentials`]): a sandbox that keeps its first env keeps working tokens.
pub async fn agent_sandbox_spec(
    state: &AppState,
    agent: &AgentRow,
    key: SandboxKey,
) -> Result<SandboxSpec, RuntimeError> {
    Ok(prepare_sandbox_spec(state, agent, key).await?.spec)
}

/// [`agent_sandbox_spec`], also reporting whether the credentials were just minted.
pub async fn prepare_sandbox_spec(
    state: &AppState,
    agent: &AgentRow,
    key: SandboxKey,
) -> Result<PreparedSpec, RuntimeError> {
    let credentials = sandbox_credentials::load_or_mint(state, agent.id, agent.owner_id)
        .await
        .map_err(|e| RuntimeError::Internal(format!("sandbox credentials: {e}")))?;
    let mut env =
        deploy_env::build_with_platform_env(state, agent.id, HashMap::new(), &credentials.env)
            .await;
    let ports = exposed_ports(&state.db, agent.id, &mut env).await;
    let spec = assemble_spec(
        agent,
        key,
        env,
        ports,
        &state.config.agent_default_memory,
        &state.config.agent_image_registry,
        pull_secret_name(&state.config.agent_runtime, agent.id),
    )?;
    Ok(PreparedSpec {
        spec,
        credentials_minted: credentials.minted,
    })
}

/// The Kubernetes Secret a resident deploy creates for pulling the agent's image
/// (`agents::attach_pull_credential`); `None` outside Kubernetes.
fn pull_secret_name(agent_runtime: &str, agent_id: Uuid) -> Option<String> {
    (agent_runtime == "kubernetes").then(|| format!("pull-{agent_id}"))
}

/// The port the agent listens on, as its last deployment recorded it (what restart uses), else
/// its `PORT` env, else the platform default. A recorded port also becomes `PORT`, so the env
/// and the exposed port cannot disagree.
pub(crate) async fn exposed_ports(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    env: &mut HashMap<String, String>,
) -> Vec<u16> {
    let recorded: Option<Vec<i32>> = sqlx::query_scalar(
        "SELECT spec_ports FROM agent_deployments \
         WHERE agent_id = $1 AND spec_ports IS NOT NULL AND cardinality(spec_ports) > 0 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    let ports = choose_ports(recorded, env.get("PORT").map(String::as_str));
    env.insert("PORT".to_owned(), ports[0].to_string());
    ports
}

fn choose_ports(recorded: Option<Vec<i32>>, env_port: Option<&str>) -> Vec<u16> {
    let recorded: Vec<u16> = recorded
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .collect();
    if !recorded.is_empty() {
        return recorded;
    }
    let from_env = env_port
        .and_then(|p| p.parse::<u16>().ok())
        .filter(|p| *p != 0);
    vec![from_env.unwrap_or(DEFAULT_AGENT_PORT)]
}

/// A writable agent's own workspace directory: the `{owner}/{agent}` a resident deploy mounts,
/// so a woken agent finds its files again.
fn agent_workspace(agent: &AgentRow) -> WorkspaceRef {
    WorkspaceRef {
        owner_id: agent.owner_id,
        container_id: ContainerId::from_uuid(agent.id),
    }
}

fn assemble_spec(
    agent: &AgentRow,
    key: SandboxKey,
    env: HashMap<String, String>,
    ports: Vec<u16>,
    default_memory: &str,
    image_registry: &str,
    image_pull_secret: Option<String>,
) -> Result<SandboxSpec, RuntimeError> {
    let image = agent
        .image
        .as_deref()
        .filter(|image| !image.is_empty())
        .ok_or_else(|| RuntimeError::InvalidSpec("agent has no image to run".into()))?;
    Ok(SandboxSpec {
        key,
        image: qualify_deploy_image(image_registry, image),
        command: vec![],
        env,
        resources: Some(ResourceLimits {
            memory: default_memory.to_owned(),
            cpu_milli: AGENT_CPU_MILLI,
        }),
        workspace: agent.writable.then(|| agent_workspace(agent)),
        runtime_class: None,
        ports,
        transport: TransportKind::Http,
        image_pull_secret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(image: Option<&str>, writable: bool) -> AgentRow {
        AgentRow {
            id: Uuid::nil(),
            owner_id: Uuid::nil(),
            name: "echo".into(),
            status: "parked".into(),
            workload_kind: "agent".into(),
            image: image.map(str::to_owned),
            url: None,
            transport_path: None,
            writable,
            deleted_at: None,
            is_internal: false,
            keeps_session_memory: false,
        }
    }

    fn key() -> SandboxKey {
        SandboxKey::Agent {
            agent_id: Uuid::nil(),
        }
    }

    fn assemble(agent: &AgentRow, ports: Vec<u16>) -> Result<SandboxSpec, RuntimeError> {
        assemble_spec(
            agent,
            key(),
            HashMap::new(),
            ports,
            "1Gi",
            "registry.example.com",
            Some("pull-x".into()),
        )
    }

    #[test]
    fn spec_carries_image_port_resources_pull_secret_and_no_workspace() {
        let spec = assemble(&agent(Some("nasiko/echo:1.0.0"), false), vec![3000]).unwrap();
        assert_eq!(spec.image, "registry.example.com/nasiko/echo:1.0.0");
        assert_eq!(spec.ports, vec![3000]);
        assert_eq!(spec.transport, TransportKind::Http);
        assert_eq!(spec.image_pull_secret.as_deref(), Some("pull-x"));
        assert!(spec.workspace.is_none() && spec.runtime_class.is_none());
        let resources = spec.resources.as_ref().unwrap();
        assert_eq!(
            (resources.memory.as_str(), resources.cpu_milli),
            ("1Gi", 500)
        );
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn a_recorded_port_wins_then_the_env_port_then_the_default() {
        assert_eq!(
            choose_ports(Some(vec![3000, 4000]), Some("9000")),
            [3000, 4000]
        );
        assert_eq!(choose_ports(None, Some("9000")), [9000]);
        assert_eq!(choose_ports(Some(vec![]), None), [DEFAULT_AGENT_PORT]);
        assert_eq!(choose_ports(None, Some("not-a-port")), [DEFAULT_AGENT_PORT]);
        assert_eq!(
            choose_ports(Some(vec![0, -1]), Some("0")),
            [DEFAULT_AGENT_PORT]
        );
    }

    #[test]
    fn the_pull_secret_exists_only_on_kubernetes() {
        let id = Uuid::nil();
        assert_eq!(
            pull_secret_name("kubernetes", id),
            Some(format!("pull-{id}"))
        );
        assert_eq!(pull_secret_name("docker", id), None);
    }

    #[test]
    fn a_writable_agent_carries_its_own_workspace_and_a_plain_one_does_not() {
        let mut row = agent(Some("x:1"), true);
        row.id = Uuid::from_u128(7);
        row.owner_id = Uuid::from_u128(9);
        let spec = assemble(&row, vec![8000]).unwrap();
        assert!(spec.validate().is_ok());
        let ws = spec.workspace.expect("writable agent has a workspace");
        assert_eq!(ws.subpath(), format!("{}/{}", row.owner_id, row.id));

        let plain = assemble(&agent(Some("x:1"), false), vec![8000]).unwrap();
        assert!(plain.workspace.is_none());
    }

    #[test]
    fn an_agent_without_an_image_is_refused() {
        for image in [None, Some("")] {
            let err = assemble(&agent(image, false), vec![8000]).unwrap_err();
            assert!(matches!(err, RuntimeError::InvalidSpec(_)));
        }
    }
}
