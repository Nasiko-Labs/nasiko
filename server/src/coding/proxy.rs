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

/// The coding-agent kinds Nasiko knows how to drive over ACP.
///
/// Each exposes ACP through a different entry point and selects its model a
/// different way, so the *kind* — not the raw image string — is what the rest of
/// this module branches on. There is deliberately no "default" kind: every kind
/// is treated the same, and an image that matches none is a misconfiguration the
/// caller rejects (see [`classify_image`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodingAgentKind {
    /// Driven with `opencode acp`; model + permissions come from the image's own
    /// baked `opencode.json`.
    OpenCode,
    /// Driven with `claude-code-acp`; routes via env and asks by default.
    ClaudeCode,
    /// Driven with `codex-acp`; routes via its baked config, asks only when forced.
    Codex,
}

/// Classify a coding agent's container image into a [`CodingAgentKind`], or
/// `None` when the image matches no kind Nasiko can drive.
///
/// Keyed off a recognizable token in the image reference. There is **no default
/// kind**: an image that is neither OpenCode, Claude Code nor Codex cannot be run
/// with any of their adapters, so the caller must reject it rather than guess —
/// silently assuming one adapter would just fail obscurely at attach time.
pub fn classify_image(image: &str) -> Option<CodingAgentKind> {
    let lower = image.to_ascii_lowercase();
    if lower.contains("claude") {
        Some(CodingAgentKind::ClaudeCode)
    } else if lower.contains("codex") {
        Some(CodingAgentKind::Codex)
    } else if lower.contains("opencode") {
        Some(CodingAgentKind::OpenCode)
    } else {
        None
    }
}

/// Derive the ACP stdio command for a coding agent's image, or `None` when the
/// image is not a recognized kind (see [`classify_image`]).
///
/// Each kind has its own entry point (`opencode acp`, `claude-code-acp`,
/// `codex-acp`). When `hitl_enabled`, Codex also needs its approval policy forced
/// so it routes tool use through `session/request_permission` (the HITL
/// delegate): codex-acp honors Codex's `-c key=value` overrides, so
/// `approval_policy=untrusted` makes it ask before acting, and
/// `sandbox_mode=workspace-write` lets the gated action actually run once
/// approved. (A read-only sandbox makes Codex conclude it *cannot* write and
/// decline outright — suggesting a shell command to the user — rather than
/// requesting permission, so no approval is ever raised.) OpenCode is nudged a
/// different way — via `OPENCODE_CONFIG_CONTENT`, see [`routed_model_env`] — and
/// Claude Code asks by default (its `claude-code-acp` adapter runs in
/// `permissionMode = "default"`), so neither needs a command argument.
pub fn derive_acp_command(image: &str, hitl_enabled: bool) -> Option<Vec<String>> {
    let command = match classify_image(image)? {
        CodingAgentKind::ClaudeCode => vec!["claude-code-acp".to_owned()],
        CodingAgentKind::Codex => {
            let mut cmd = vec!["codex-acp".to_owned()];
            if hitl_enabled {
                cmd.extend([
                    "-c".to_owned(),
                    "approval_policy=\"untrusted\"".to_owned(),
                    "-c".to_owned(),
                    "sandbox_mode=\"workspace-write\"".to_owned(),
                ]);
            }
            cmd
        }
        CodingAgentKind::OpenCode => vec!["opencode".to_owned(), "acp".to_owned()],
    };
    Some(command)
}

/// Per-turn env a coding agent needs beyond the base routing wiring.
///
/// Routing itself — the router base URL and the session-token key — is injected
/// by the proxy, and every coding-agent image now bakes its own default model
/// (the `openai-chat/gpt-4o` provider for OpenCode, the `nasiko` provider for
/// Codex, pure env for Claude Code), so there is nothing to inject for routing.
///
/// The one thing that cannot live in the image is the HITL permission flip:
/// `hitl_enabled` is a per-agent toggle (the same image is deployed with HITL on
/// or off), and OpenCode's permissions default to `"allow"` — it acts silently
/// and never sends `session/request_permission`. So when HITL is on the server
/// forces them to `"ask"` via `OPENCODE_CONFIG_CONTENT` (an inline config that
/// *merges* over the image's `opencode.json`, leaving its baked model intact).
/// Codex is forced to ask via command arguments (see [`derive_acp_command`]) and
/// Claude Code asks by default, so neither needs anything here — nor does any
/// agent when HITL is off.
pub fn routed_model_env(
    image: &str,
    hitl_enabled: bool,
) -> std::collections::HashMap<String, String> {
    let mut env = std::collections::HashMap::new();
    if hitl_enabled && classify_image(image) == Some(CodingAgentKind::OpenCode) {
        let config = serde_json::json!({
            "permission": { "edit": "ask", "bash": "ask", "webfetch": "ask" }
        });
        env.insert("OPENCODE_CONFIG_CONTENT".to_owned(), config.to_string());
    }
    env
}

/// Why [`load_target`] could not produce a drivable coding-agent target.
#[derive(Debug, thiserror::Error)]
pub enum LoadTargetError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// The agent is marked a coding agent but its image matches no kind Nasiko
    /// can drive, so there is no ACP command to launch it with. A deploy-time
    /// mistake that we surface loudly here rather than guessing an adapter.
    #[error(
        "coding-agent image {image:?} is not a recognized ACP kind \
         (the image name must contain 'opencode', 'claude', or 'codex')"
    )]
    UnrecognizedImage { image: String },
}

/// The `agents` columns a coding-agent turn needs: `(name, image, owner_id,
/// writable_path, hitl_enabled)`.
type TargetRow = (String, Option<String>, Uuid, Option<String>, bool);

/// Return the coding-agent target for `agent_id`, or `None` when the agent is not
/// an ACP coding agent (the proxy then takes its ordinary HTTP path).
///
/// A coding agent with no `image` is a misconfiguration we can't deploy, so it
/// is treated as "not a coding agent" here rather than failing every request —
/// the row simply never matches. An agent that *is* a coding agent but whose
/// image matches no known kind is a different kind of misconfiguration: there is
/// no adapter to drive it, so it fails loudly with [`LoadTargetError::UnrecognizedImage`]
/// rather than being driven with the wrong command.
pub async fn load_target(
    db: &PgPool,
    agent_id: Uuid,
) -> Result<Option<CodingAgentTarget>, LoadTargetError> {
    let row: Option<TargetRow> = sqlx::query_as(
        "SELECT name, image, owner_id, writable_path, hitl_enabled \
         FROM agents \
         WHERE id = $1 AND workload_kind = 'acp_coding_agent' \
           AND image IS NOT NULL AND deleted_at IS NULL",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await?;

    let Some((name, image, owner_id, writable_path, hitl_enabled)) = row else {
        return Ok(None);
    };
    // `image IS NOT NULL` in the query guarantees this unwrap.
    let image = image.unwrap_or_default();
    let acp_command = derive_acp_command(&image, hitl_enabled).ok_or_else(|| {
        LoadTargetError::UnrecognizedImage {
            image: image.clone(),
        }
    })?;
    Ok(Some(CodingAgentTarget {
        name,
        image,
        owner_id,
        writable_path,
        acp_command,
        hitl_enabled,
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
        replica_target: None,
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
    fn classify_image_recognizes_known_kinds_and_rejects_others() {
        assert_eq!(
            classify_image("registry/opencode:0.1.0-acp"),
            Some(CodingAgentKind::OpenCode)
        );
        assert_eq!(
            classify_image("claude-code:acp"),
            Some(CodingAgentKind::ClaudeCode)
        );
        assert_eq!(
            classify_image("myorg/codex:v1"),
            Some(CodingAgentKind::Codex)
        );
        // No default kind: an unrecognized image is not silently an OpenCode.
        assert_eq!(classify_image("some/mystery-agent:1"), None);
    }

    #[test]
    fn acp_command_is_derived_from_the_image() {
        assert_eq!(
            derive_acp_command("registry/opencode:0.1.0-acp", false).unwrap(),
            ["opencode", "acp"]
        );
        assert_eq!(
            derive_acp_command("claude-code:acp", false).unwrap(),
            ["claude-code-acp"]
        );
        assert_eq!(
            derive_acp_command("myorg/codex:v1", false).unwrap(),
            ["codex-acp"]
        );
        // Unrecognized images have no command — the caller must reject them,
        // rather than being driven with a guessed adapter.
        assert_eq!(derive_acp_command("some/mystery-agent:1", false), None);
    }

    #[test]
    fn hitl_forces_codex_approval_but_leaves_others_unchanged() {
        // Codex gains config overrides that make it ask before acting.
        assert_eq!(
            derive_acp_command("myorg/codex:v1", true).unwrap(),
            [
                "codex-acp",
                "-c",
                "approval_policy=\"untrusted\"",
                "-c",
                "sandbox_mode=\"workspace-write\""
            ]
        );
        // OpenCode (nudged via env) and Claude Code (asks by default) are the
        // same with or without HITL.
        assert_eq!(
            derive_acp_command("opencode:0.1.0", true),
            derive_acp_command("opencode:0.1.0", false)
        );
        assert_eq!(
            derive_acp_command("claude-code:acp", true).unwrap(),
            ["claude-code-acp"]
        );
    }

    #[test]
    fn routed_model_env_is_empty_unless_opencode_hitl() {
        // The model is baked in each image now, so nothing is injected for
        // routing — with HITL off, every kind gets an empty map.
        for image in [
            "registry/opencode:0.1.0",
            "claude-code:acp",
            "myorg/codex:v1",
        ] {
            assert!(
                routed_model_env(image, false).is_empty(),
                "{image} without HITL must inject nothing"
            );
        }
        // Claude Code and Codex force "ask" by other means, so still nothing here
        // even with HITL on.
        for image in ["claude-code:acp", "myorg/codex:v1"] {
            assert!(
                routed_model_env(image, true).is_empty(),
                "{image} must not get OpenCode's config var",
            );
        }
    }

    #[test]
    fn routed_model_env_forces_opencode_permission_ask_when_hitl_enabled() {
        let content = routed_model_env("registry/opencode:0.1.0", true);
        let config: serde_json::Value =
            serde_json::from_str(&content["OPENCODE_CONFIG_CONTENT"]).unwrap();
        // Only the permission flip — the model comes from the image's own config.
        assert!(
            config.get("model").is_none(),
            "model must be baked in the image, not injected"
        );
        // Defaults are "allow" (silent); HITL needs them forced to "ask".
        assert_eq!(config["permission"]["edit"], "ask");
        assert_eq!(config["permission"]["bash"], "ask");
        assert_eq!(config["permission"]["webfetch"], "ask");
    }

    #[test]
    fn spec_is_portless_writable_and_valid() {
        let target = CodingAgentTarget {
            name: "my-coder".to_owned(),
            image: "registry/opencode:v1".to_owned(),
            owner_id: Uuid::nil(),
            writable_path: None,
            acp_command: derive_acp_command("registry/opencode:v1", false).unwrap(),
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
