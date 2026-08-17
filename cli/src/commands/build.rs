use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

use crate::api::Client;
use crate::util::container_bin;
use crate::version_prompt::{VersionContext, VersionFlags, resolve_deploy_version};

pub fn build(directory: &str, tag: Option<&str>, platform: Option<&str>) -> Result<()> {
    build_with_version_flags(directory, tag, platform, VersionFlags::default())
}

pub fn build_with_version_flags(
    directory: &str,
    tag: Option<&str>,
    platform: Option<&str>,
    flags: VersionFlags,
) -> Result<()> {
    let root = Path::new(directory)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(directory).to_path_buf());

    if !root.join("Dockerfile").exists() {
        bail!(
            "No Dockerfile found at {}. Run `nasiko new` first.",
            root.display()
        );
    }
    check_prebuilt_binaries_exist(&root)?;

    let resolved_tag = match tag {
        Some(t) => t.to_string(),
        // `build` doesn't talk to the server, so it only checks AgentCard's
        // version — no "already deployed" version to compare against.
        None => default_tag(&root, flags)?,
    };

    println!("Building {resolved_tag}");

    let bin = container_bin();
    let mut cmd = Command::new(&bin);
    cmd.args(["build", "-t", &resolved_tag]);

    if let Some(p) = platform {
        cmd.args(["--platform", p]);
    }

    cmd.arg(root.to_str().unwrap_or("."));

    let cmd_str = format!(
        "{bin} build -t {resolved_tag}{} {}",
        platform
            .map(|p| format!(" --platform {p}"))
            .unwrap_or_default(),
        root.display(),
    );
    println!("$ {cmd_str}\n");

    let status = cmd.status()?;
    if !status.success() {
        bail!("Build failed (exit code: {})", status.code().unwrap_or(1));
    }

    println!("\n✓ Built {resolved_tag}");
    println!("  Run locally: nasiko run {directory}");
    println!("  Deploy:      nasiko deploy {resolved_tag}");

    Ok(())
}

/// Fail fast — with the actual fix — when the Dockerfile COPYs a compiled
/// binary out of `target/` that hasn't been built yet. The Rust templates
/// (`FROM scratch`) expect a `just build` (cargo zigbuild) first; without this
/// check docker fails mid-build with an opaque "not found" error.
fn check_prebuilt_binaries_exist(root: &Path) -> Result<()> {
    let Ok(dockerfile) = fs::read_to_string(root.join("Dockerfile")) else {
        return Ok(());
    };
    if let Some(missing) = missing_copy_sources(&dockerfile, root).first() {
        let fix = if root.join("justfile").exists() {
            "run `just build` in the project directory first (it compiles the binary, \
             then you can `nasiko build`)"
        } else {
            "build it first (e.g. cargo zigbuild --release for the Dockerfile's target)"
        };
        bail!("Dockerfile copies '{missing}' which does not exist — {fix}");
    }
    Ok(())
}

/// COPY sources under `target/` that are absent on disk. Only build-output
/// paths are checked — sources like `src/` always exist in a scaffold, and
/// multi-stage `COPY --from=` sources live in the image, not on disk.
fn missing_copy_sources(dockerfile: &str, root: &Path) -> Vec<String> {
    dockerfile
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("COPY ") && !l.contains("--from"))
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter(|src| src.starts_with("target/") && !root.join(src).exists())
        .map(String::from)
        .collect()
}

fn default_tag(root: &Path, flags: VersionFlags) -> Result<String> {
    let card_path = root.join("AgentCard.json");
    let (name, card_version): (String, Option<String>) = if card_path.exists()
        && let Ok(content) = fs::read_to_string(&card_path)
        && let Ok(card) = serde_json::from_str::<serde_json::Value>(&content)
    {
        let name = card
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("agent")
            .to_string();
        let card_version = card
            .get("version")
            .and_then(|v| v.as_str())
            .map(String::from);
        (name, card_version)
    } else {
        (
            root.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            None,
        )
    };

    // Best-effort lookup of this agent's current version + full history from
    // the active cluster, so `build` resolves against the same information
    // `deploy` would — without this, `build` and a later `deploy` of the same
    // source could each resolve a different version (see CLAUDE.md/the fix
    // for this file). Falls back to "no history" (this function's old,
    // offline-only behavior) if there's no active cluster, the agent hasn't
    // been pushed/deployed yet, or the server is unreachable — `build` must
    // still work standalone; the server remains the actual authority on
    // version uniqueness at push/deploy time regardless.
    let (current_deployed_version, used_versions) = existing_version_context(&name);

    resolve_build_tag(
        &name,
        card_version.as_deref(),
        current_deployed_version.as_deref(),
        &used_versions,
        flags,
    )
}

/// Resolves the version and formats the `name:version` tag `build` uses for
/// the Docker image — split out from [`default_tag`] so this part (the same
/// resolution logic `deploy` uses) is testable without a live cluster.
fn resolve_build_tag(
    name: &str,
    card_version: Option<&str>,
    current_deployed_version: Option<&str>,
    used_versions: &[String],
    flags: VersionFlags,
) -> Result<String> {
    let context = VersionContext {
        card_version,
        current_deployed_version,
        used_versions,
    };
    let decision = resolve_deploy_version(context, flags)?;
    Ok(format!("{name}:{}", decision.version))
}

fn existing_version_context(name: &str) -> (Option<String>, Vec<String>) {
    let Ok(client) = Client::from_active_cluster() else {
        return (None, Vec::new());
    };
    existing_version_context_with(&client, name)
}

/// Testable core of [`existing_version_context`] — takes an injected client
/// instead of reading `~/.nasiko/config.json`. `(None, vec![])` on any
/// failure (agent not found, network error): this is advisory only, so
/// `build` degrades gracefully rather than erroring.
fn existing_version_context_with(client: &Client, name: &str) -> (Option<String>, Vec<String>) {
    let Ok(Some(agent)) = client.get_agent(name) else {
        return (None, Vec::new());
    };
    let current_version = agent
        .get("version")
        .and_then(|v| v.as_str())
        .map(String::from);
    let id = agent.get("id").and_then(|v| v.as_str()).unwrap_or(name);
    let used_versions = client.used_versions(id).unwrap_or_default();
    (current_version, used_versions)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── existing_version_context_with ───────────────────────────────────────

    #[test]
    fn existing_version_context_with_reports_current_version_and_history() {
        let mut srv = mockito::Server::new();
        srv.mock("GET", "/api/agents/legal-agent")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"agent-1","version":"1.0.0"}"#)
            .create();
        srv.mock("GET", "/api/agents/agent-1/versions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data": [
                    {"version": "1.0.0", "status": "active", "is_active": true,
                     "can_rollback": false, "created_at": "2024-01-01T00:00:00Z"}
                ]}"#,
            )
            .create();
        let client = Client::for_test(&srv.url(), None);

        let (current, used) = existing_version_context_with(&client, "legal-agent");
        assert_eq!(current, Some("1.0.0".to_string()));
        assert_eq!(used, vec!["1.0.0".to_string()]);
    }

    #[test]
    fn existing_version_context_with_is_empty_for_an_unknown_agent() {
        let mut srv = mockito::Server::new();
        srv.mock("GET", "/api/agents/ghost")
            .with_status(404)
            .with_body("not found")
            .create();
        let client = Client::for_test(&srv.url(), None);

        let (current, used) = existing_version_context_with(&client, "ghost");
        assert_eq!(current, None);
        assert!(used.is_empty());
    }

    #[test]
    fn existing_version_context_with_falls_back_silently_on_a_server_error() {
        // Advisory only — build must degrade to "no history" rather than
        // erroring, unlike `deploy`'s `used_version_context` which propagates
        // this same failure (see deploy_tests.rs).
        let mut srv = mockito::Server::new();
        srv.mock("GET", "/api/agents/legal-agent")
            .with_status(500)
            .with_body("boom")
            .create();
        let client = Client::for_test(&srv.url(), None);

        let (current, used) = existing_version_context_with(&client, "legal-agent");
        assert_eq!(current, None);
        assert!(used.is_empty());
    }

    // ─── resolve_build_tag ────────────────────────────────────────────────────

    #[test]
    fn build_resolves_next_patch_version_when_card_has_no_usable_version() {
        // The reported bug: legal-agent:1.0.0 already exists on the platform,
        // AgentCard.json has no usable version — build must resolve 1.0.1
        // (the same next-version `deploy` would suggest), not the hardcoded
        // "0.1.0" first-version fallback it used before this fix.
        let tag = resolve_build_tag(
            "legal-agent",
            None,
            Some("1.0.0"),
            &["1.0.0".to_string()],
            VersionFlags {
                yes: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(tag, "legal-agent:1.0.1");
    }

    #[test]
    fn build_preserves_an_explicit_valid_version_over_existing_history() {
        let tag = resolve_build_tag(
            "legal-agent",
            None,
            Some("1.0.0"),
            &["1.0.0".to_string()],
            VersionFlags {
                version: Some("2.5.0"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(tag, "legal-agent:2.5.0");
    }

    #[test]
    fn build_preserves_a_fresh_unused_card_version_without_prompting() {
        let tag = resolve_build_tag(
            "legal-agent",
            Some("1.2.0"),
            Some("1.0.0"),
            &["1.0.0".to_string()],
            VersionFlags::default(),
        )
        .unwrap();
        assert_eq!(tag, "legal-agent:1.2.0");
    }

    #[test]
    fn missing_target_copy_source_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let dockerfile =
            "FROM scratch\nCOPY target/x86_64-unknown-linux-musl/release/agent /agent\n";
        assert_eq!(
            missing_copy_sources(dockerfile, dir.path()),
            vec!["target/x86_64-unknown-linux-musl/release/agent".to_string()]
        );
    }

    #[test]
    fn present_target_copy_source_passes() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("target/release");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("agent"), b"").unwrap();
        let dockerfile = "COPY target/release/agent /agent\n";
        assert!(missing_copy_sources(dockerfile, dir.path()).is_empty());
    }

    #[test]
    fn non_target_and_multistage_copies_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let dockerfile = "COPY src/ ./src/\nCOPY --from=builder target/release/agent /agent\n";
        assert!(missing_copy_sources(dockerfile, dir.path()).is_empty());
    }
}
