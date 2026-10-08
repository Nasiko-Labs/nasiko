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
    /// Whether this agent gates its tool use behind human approval (NAS-842).
    pub hitl_enabled: bool,
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

/// The provider/model a managed OpenCode turn defaults to.
///
/// `openai-chat` is the custom `@ai-sdk/openai-compatible` provider our OpenCode
/// image declares (it targets `/v1/chat/completions` and reads `OPENAI_BASE_URL`
/// / `OPENAI_API_KEY` from the per-turn env the proxy injects), so this is the
/// model that carries the session token through the LLM router. It intentionally
/// matches the provider name baked into the image's `opencode.json`.
pub const OPENCODE_ROUTED_MODEL: &str = "openai-chat/gpt-4o";

/// Per-turn env that makes a coding agent default to the Nasiko-routed model
/// (and, when `hitl_enabled`, prompt for permission before acting).
///
/// Driven over ACP, OpenCode bypasses the a2a HTTP wrapper that would otherwise
/// force the routed model per prompt, so with no default it falls back to its
/// built-in free model (`opencode/big-pickle`) — which needs no key and routes to
/// OpenCode's own backend, bypassing the router and billing entirely. Setting
/// `OPENCODE_CONFIG_CONTENT` (an inline config that *merges* over the image's
/// `opencode.json`, keeping its `openai-chat` provider) forces the routed model
/// as the session default instead.
///
/// When `hitl_enabled`, the same config forces OpenCode's `permission` rules to
/// `"ask"`: the default is `"allow"` (silent), so without this OpenCode never
/// sends `session/request_permission` and the HITL delegate has nothing to gate.
///
/// Claude Code and Codex select their model — and would force their own
/// permission prompts — via their own env / config, so they need nothing here and
/// get an empty map. Keyed off the image like [`derive_acp_command`].
pub fn routed_model_env(
    image: &str,
    hitl_enabled: bool,
) -> std::collections::HashMap<String, String> {
    let lower = image.to_ascii_lowercase();
    let mut env = std::collections::HashMap::new();
    // Only OpenCode (the default kind) reads this var and needs the nudge.
    if lower.contains("claude") || lower.contains("codex") {
        return env;
    }
    let mut config = serde_json::json!({ "model": OPENCODE_ROUTED_MODEL });
    if hitl_enabled {
        config["permission"] = serde_json::json!({
            "edit": "ask",
            "bash": "ask",
            "webfetch": "ask",
        });
    }
    env.insert("OPENCODE_CONFIG_CONTENT".to_owned(), config.to_string());
    env
}

/// The `agents` columns a coding-agent turn needs: `(name, image, owner_id,
/// writable_path, hitl_enabled)`.
type TargetRow = (String, Option<String>, Uuid, Option<String>, bool);

/// Return the coding-agent target for `agent_id`, or `None` when the agent is not
/// an ACP coding agent (the proxy then takes its ordinary HTTP path).
///
/// A coding agent with no `image` is a misconfiguration we can't deploy, so it
/// is treated as "not a coding agent" here rather than failing every request —
/// the row simply never matches.
pub async fn load_target(db: &PgPool, agent_id: Uuid) -> sqlx::Result<Option<CodingAgentTarget>> {
    let row: Option<TargetRow> = sqlx::query_as(
        "SELECT name, image, owner_id, writable_path, hitl_enabled \
         FROM agents \
         WHERE id = $1 AND workload_kind = 'acp_coding_agent' \
           AND image IS NOT NULL AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await?;

    Ok(
        row.map(|(name, image, owner_id, writable_path, hitl_enabled)| {
            // `image IS NOT NULL` in the query guarantees this unwrap.
            let image = image.unwrap_or_default();
            let acp_command = derive_acp_command(&image);
            CodingAgentTarget {
                name,
                image,
                owner_id,
                writable_path,
                acp_command,
                hitl_enabled,
            }
        }),
    )
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
    fn routed_model_env_forces_the_model_for_opencode_only() {
        // OpenCode (and the unknown→OpenCode default) gets the routed-model nudge.
        for image in ["registry/opencode:0.1.0", "some/mystery-agent:1"] {
            let content = routed_model_env(image, false);
            let config: serde_json::Value =
                serde_json::from_str(&content["OPENCODE_CONFIG_CONTENT"]).unwrap();
            assert_eq!(config["model"], "openai-chat/gpt-4o", "{image}");
            assert!(
                config.get("permission").is_none(),
                "no HITL => no permission block"
            );
        }
        // Claude Code and Codex route via their own env/config — no nudge.
        for image in ["claude-code:acp", "myorg/codex:v1"] {
            assert!(
                routed_model_env(image, true).is_empty(),
                "{image} must not get OpenCode's config var",
            );
        }
    }

    #[test]
    fn routed_model_env_forces_permission_ask_when_hitl_enabled() {
        let content = routed_model_env("registry/opencode:0.1.0", true);
        let config: serde_json::Value =
            serde_json::from_str(&content["OPENCODE_CONFIG_CONTENT"]).unwrap();
        assert_eq!(config["model"], "openai-chat/gpt-4o");
        // Defaults are "allow" (silent); HITL needs them forced to "ask".
        assert_eq!(config["permission"]["edit"], "ask");
        assert_eq!(config["permission"]["bash"], "ask");
    }

    #[test]
    fn spec_is_portless_writable_and_valid() {
        let target = CodingAgentTarget {
            name: "my-coder".to_owned(),
            image: "registry/opencode:v1".to_owned(),
            owner_id: Uuid::nil(),
            writable_path: None,
            acp_command: derive_acp_command("registry/opencode:v1"),
            hitl_enabled: false,
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
