//! Agent-proxy support for coding agents: tell a coding agent apart from an
//! ordinary HTTP agent, and build the container spec a turn deploys.
//!
//! The agent proxy serves one inbound API for every agent. An ordinary agent is
//! an HTTP server it reverse-proxies to; a coding agent has no HTTP port and is
//! driven over a stdio turn instead. These helpers give the proxy the two things
//! it needs to take the coding-agent branch: a way to recognize one, and the
//! [`DeploymentSpec`] to bring its container up.

use nasiko_runtime::{ContainerId, DeploymentSpec, WorkloadKind};
use sqlx::PgPool;
use uuid::Uuid;

/// The catalog facts a coding-agent turn needs that the normal proxy resolve
/// doesn't carry: the container image, its owner (for the per-agent workspace),
/// the human-readable name, and the ACP command to drive it.
#[derive(Debug, Clone)]
pub struct CodingAgentTarget {
    pub name: String,
    pub image: String,
    pub owner_id: Uuid,
    pub writable_path: Option<String>,
    /// The stdio command Nasiko execs in the container to start the ACP session,
    /// derived from the image (see [`derive_acp_command`]). Per-agent, so one
    /// server can drive OpenCode, Claude Code and Codex agents side by side.
    pub acp_command: Vec<String>,
}

/// Derive the ACP stdio command from a coding agent's container image.
///
/// Each coding agent exposes ACP through a different entry point
/// (`opencode acp`, `claude-code-acp`, `codex-acp`), so we key off a recognizable
/// token in the image reference rather than a single global setting. Unknown
/// images default to OpenCode's `opencode acp` — the historical default — so
/// existing deployments are unaffected.
pub fn derive_acp_command(image: &str) -> Vec<String> {
    let lower = image.to_ascii_lowercase();
    if lower.contains("claude") {
        vec!["claude-code-acp".to_owned()]
    } else if lower.contains("codex") {
        vec!["codex-acp".to_owned()]
    } else {
        vec!["opencode".to_owned(), "acp".to_owned()]
    }
}

/// Return the coding-agent target for `agent_id`, or `None` when the agent is not
/// an ACP coding agent (the proxy then takes its ordinary HTTP path).
///
/// A coding agent with no `image` is a misconfiguration we can't deploy, so it
/// is treated as "not a coding agent" here rather than failing every request —
/// the row simply never matches.
pub async fn load_target(db: &PgPool, agent_id: Uuid) -> sqlx::Result<Option<CodingAgentTarget>> {
    let row: Option<(String, Option<String>, Uuid, Option<String>)> = sqlx::query_as(
        "SELECT name, image, owner_id, writable_path \
         FROM agents \
         WHERE id = $1 AND workload_kind = 'acp_coding_agent' \
           AND image IS NOT NULL AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await?;

    Ok(row.map(|(name, image, owner_id, writable_path)| {
        // `image IS NOT NULL` in the query guarantees this unwrap.
        let image = image.unwrap_or_default();
        let acp_command = derive_acp_command(&image);
        CodingAgentTarget {
            name,
            image,
            owner_id,
            writable_path,
            acp_command,
        }
    }))
}

/// Build the container spec for a coding agent.
///
/// A coding agent always gets a writable workspace volume (each session runs in
/// its own subdirectory there) and exposes no inbound port — it's reached over
/// the per-session stdio attach, not HTTP.
pub fn build_spec(agent_id: Uuid, target: &CodingAgentTarget) -> DeploymentSpec {
    DeploymentSpec {
        container_id: ContainerId::from_uuid(agent_id),
        name: target.name.clone(),
        image: target.image.clone(),
        min_replicas: 1,
        max_replicas: 1,
        env_vars: std::collections::HashMap::new(),
        // No inbound HTTP port — the runtime permits an empty list for this kind.
        ports: Vec::new(),
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: WorkloadKind::AcpCodingAgent,
        // Always writable: the per-session workspace lives on the shared volume.
        writable: true,
        writable_path: target.writable_path.clone(),
        owner_id: target.owner_id,
        force_pull: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_command_is_derived_from_the_image() {
        assert_eq!(
            derive_acp_command("registry/opencode:0.1.0-acp"),
            ["opencode", "acp"]
        );
        assert_eq!(derive_acp_command("claude-code:acp"), ["claude-code-acp"]);
        assert_eq!(derive_acp_command("myorg/codex:v1"), ["codex-acp"]);
        // Unknown images fall back to the OpenCode default.
        assert_eq!(
            derive_acp_command("some/mystery-agent:1"),
            ["opencode", "acp"]
        );
    }

    #[test]
    fn spec_is_portless_writable_and_valid() {
        let target = CodingAgentTarget {
            name: "my-coder".to_owned(),
            image: "registry/opencode:v1".to_owned(),
            owner_id: Uuid::nil(),
            writable_path: None,
            acp_command: derive_acp_command("registry/opencode:v1"),
        };
        let spec = build_spec(Uuid::nil(), &target);

        assert_eq!(spec.workload_kind, WorkloadKind::AcpCodingAgent);
        assert!(
            spec.ports.is_empty(),
            "a coding agent exposes no inbound port"
        );
        assert!(
            spec.writable,
            "a coding agent always gets its workspace volume"
        );
        assert_eq!(spec.image, "registry/opencode:v1");
        // The portless spec must pass the runtime's own validation.
        assert!(
            spec.validate().is_ok(),
            "portless coding-agent spec must validate"
        );
    }
}
