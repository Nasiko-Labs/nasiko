//! The harness catalog read model and the sandbox spec a session runs under.
//!
//! `harness_catalog` is data, not code: which image to run, which argv the in-image ACP shim
//! executes, and the minimum resources it needs. An operator withdraws a harness with
//! `enabled = false`.

use std::collections::HashMap;

use nasiko_config::Config;
use nasiko_runtime::{
    ContainerId, ResourceLimits, SandboxKey, SandboxSpec, TransportKind, WorkspaceRef,
};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use super::model::SessionRow;

/// The port the in-image ACP shim listens on (WebSocket).
pub const SHIM_PORT: u16 = 7000;

/// The `agents` rows that are the platform's own harness agents (unqualified columns): the one
/// boot seeding names after its harness. Other rows of kind `harness` may exist (a provider can
/// give each user an agent of their own for a harness), so a lookup of "the" agent of a harness
/// must match this, never the kind and `harness_id` alone.
pub const PLATFORM_HARNESS_AGENT: &str = "workload_kind = 'harness' AND name = harness_id";

/// One `harness_catalog` row.
#[derive(Debug, Clone, FromRow)]
pub struct Harness {
    pub id: String,
    pub name: String,
    pub image: String,
    pub acp_command: Vec<String>,
    pub protocol_version: i32,
    pub min_cpu_milli: i32,
    pub min_memory: String,
    pub enabled: bool,
}

const COLUMNS: &str =
    "id, name, image, acp_command, protocol_version, min_cpu_milli, min_memory, enabled";

/// Reads the harness catalog.
#[derive(Clone)]
pub struct HarnessCatalog {
    db: PgPool,
}

impl HarnessCatalog {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    pub async fn list_enabled(&self) -> sqlx::Result<Vec<Harness>> {
        sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM harness_catalog WHERE enabled ORDER BY id"
        ))
        .fetch_all(&self.db)
        .await
    }

    pub async fn get(&self, id: &str) -> sqlx::Result<Option<Harness>> {
        sqlx::query_as(&format!(
            "SELECT {COLUMNS} FROM harness_catalog WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.db)
        .await
    }
}

/// The sandbox spec for one session of `harness`.
///
/// The image's ENTRYPOINT is the shim, so `command` is empty and the harness's own argv
/// travels as `HARNESS_CMD`. A bare image name (no `/`) is a catalog shorthand and gets
/// `config.harness_image_prefix`; a qualified one is used as is. The workspace is the
/// harness agent's per-owner directory, so a session's files persist across suspensions.
pub fn spec_for_session(
    harness: &Harness,
    session: &SessionRow,
    env: HashMap<String, String>,
    owner_id: Uuid,
    config: &Config,
) -> SandboxSpec {
    build_spec(
        harness,
        session,
        env,
        owner_id,
        &config.harness_image_prefix,
    )
}

/// [`spec_for_session`] over just the config value it reads, so it is testable without a
/// whole `Config`.
fn build_spec(
    harness: &Harness,
    session: &SessionRow,
    mut env: HashMap<String, String>,
    owner_id: Uuid,
    image_prefix: &str,
) -> SandboxSpec {
    env.insert("HARNESS_CMD".to_owned(), harness.acp_command.join(" "));
    SandboxSpec {
        key: SandboxKey::Session {
            session_id: session.sandbox_uuid(),
        },
        image: qualified_image(&harness.image, image_prefix),
        command: vec![],
        env,
        resources: Some(ResourceLimits {
            memory: harness.min_memory.clone(),
            cpu_milli: u32::try_from(harness.min_cpu_milli).unwrap_or(0),
            cpu_request_milli: None,
        }),
        workspace: session.agent_id.map(|agent_id| WorkspaceRef {
            owner_id,
            container_id: ContainerId::from_uuid(agent_id),
        }),
        runtime_class: None,
        ports: vec![SHIM_PORT],
        transport: TransportKind::WebSocket,
        image_pull_secret: None,
    }
}

fn qualified_image(image: &str, prefix: &str) -> String {
    if image.contains('/') {
        image.to_owned()
    } else {
        format!("{prefix}{image}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harness(image: &str) -> Harness {
        Harness {
            id: "opencode".into(),
            name: "OpenCode".into(),
            image: image.into(),
            acp_command: vec!["opencode".into(), "acp".into()],
            protocol_version: 1,
            min_cpu_milli: 500,
            min_memory: "1Gi".into(),
            enabled: true,
        }
    }

    fn session(agent_id: Option<Uuid>) -> SessionRow {
        SessionRow {
            session_id: Uuid::nil().to_string(),
            user_id: Uuid::nil(),
            agent_id,
            acp_session_id: None,
            runtime_handle: None,
            runtime_state: "parked".into(),
            volume_ref: None,
            turn_owner: None,
            lease_expires_at: None,
            turn_fence: 0,
            last_turn_at: None,
            active_turn_id: None,
            sandbox_ref: None,
            suspended_at: None,
            workspace_repo: None,
            pending_permission: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn qualified_image_is_kept_and_bare_image_is_prefixed() {
        let spec = |image| {
            build_spec(
                &harness(image),
                &session(None),
                HashMap::new(),
                Uuid::nil(),
                "reg.example/h-",
            )
            .image
        };
        assert_eq!(
            spec("nasiko/harness-opencode:1"),
            "nasiko/harness-opencode:1"
        );
        assert_eq!(spec("opencode:1"), "reg.example/h-opencode:1");
    }

    #[test]
    fn spec_carries_shim_command_ports_transport_and_resources() {
        let agent = Uuid::new_v4();
        let owner = Uuid::new_v4();
        let env = HashMap::from([("A".to_owned(), "b".to_owned())]);
        let spec = build_spec(&harness("x/y:1"), &session(Some(agent)), env, owner, "p-");

        assert!(spec.validate().is_ok());
        assert_eq!(spec.env["HARNESS_CMD"], "opencode acp");
        assert_eq!(spec.env["A"], "b");
        assert!(spec.command.is_empty());
        assert_eq!(spec.ports, vec![7000]);
        assert_eq!(spec.transport, TransportKind::WebSocket);
        assert_eq!(spec.runtime_class, None);
        let res = spec.resources.expect("resources");
        assert_eq!((res.memory.as_str(), res.cpu_milli), ("1Gi", 500));
        let ws = spec.workspace.expect("workspace");
        assert_eq!(ws.owner_id, owner);
        assert_eq!(ws.container_id, ContainerId::from_uuid(agent));
    }
}
