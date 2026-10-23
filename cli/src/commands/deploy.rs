use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use nasiko_utils::term;

use crate::api::{Client, ContainerStatus, DeploySpec};
use crate::commands::local_agents::agent_command;
use crate::oci;
use crate::util::parse_image_name_and_tag;
use crate::version_prompt::{
    VersionContext, VersionFlags, resolve_deploy_version, resolve_image_deploy_version,
};

const AGENT_FILE: &str = ".nasiko/agent.json";

/// Deploy an agent from a directory (reads AgentCard.json) or a raw image.
///
/// Flow:
/// 1. Read AgentCard.json for name + version
/// 2. Push image to CP OCI registry
/// 3. Check .nasiko/agent.json for existing agent ID
///    - Exists → update agent + restart container
///    - Not found → create new agent, save ID
/// 4. Deploy/restart container
///
/// With `dry_run`, stops after step 1's lookups (existing agent, version history, whether the
/// version was already pushed) and prints the plan — nothing is built, pushed, registered,
/// deployed, or written to disk. Version selection runs exactly as in a real deploy, prompt
/// included, so the dry run fails the same way the real deploy would.
#[allow(clippy::too_many_arguments)]
pub fn deploy_with_version_flags(
    image: &str,
    name: Option<&str>,
    port: u16,
    env_file: Option<&str>,
    env_args: &[String],
    flags: VersionFlags,
    writable: bool,
    writable_path: Option<&str>,
    coding_agent: bool,
    hitl: bool,
    dry_run: bool,
) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let env = parse_env(env_file, env_args)?;
    // A path implies the mount (mirrors the server-side rule).
    let writable = writable || writable_path.is_some();

    if Path::new(image).join("AgentCard.json").exists() {
        deploy_from_directory(
            image,
            name,
            port,
            &env,
            flags,
            writable,
            writable_path,
            coding_agent,
            hitl,
            dry_run,
            &client,
        )
    } else {
        deploy_from_image(
            image,
            name,
            port,
            &env,
            flags,
            writable,
            writable_path,
            coding_agent,
            hitl,
            dry_run,
            &client,
        )
    }
}

/// What a deploy would do, as shown by `deploy --dry-run`.
struct DeployPlan<'a> {
    agent_name: &'a str,
    /// The agent is already registered, so the deploy updates it rather than creating it.
    agent_exists: bool,
    /// The live version being replaced; `None` when nothing is deployed yet.
    current_version: Option<&'a str>,
    version: &'a str,
    image_ref: &'a str,
    /// The version is already in the registry (`nasiko push`), so it would be reused as-is.
    already_pushed: bool,
    /// Deploying from source, so the image is built first; a raw image is only pushed.
    builds_image: bool,
    port: u16,
    /// `--coding-agent` or the card's `workloadKind`; `None` for an ordinary HTTP agent.
    workload_kind: Option<&'a str>,
    /// Whether the deploy turns HITL on (only ever sent as on).
    hitl: bool,
    writable: bool,
    writable_path: Option<&'a str>,
    env: &'a HashMap<String, String>,
}

/// The dry-run plan as printable lines. Env values are never included — deploy env commonly
/// carries API keys and passwords, which must not end up in scrollback or CI logs.
fn plan_lines(plan: &DeployPlan) -> Vec<String> {
    let action = match (plan.current_version, plan.agent_exists) {
        (Some(current), _) => format!("update {current} → {}", plan.version),
        (None, true) => format!("deploy {} to the existing agent", plan.version),
        (None, false) => format!("register new agent at {}", plan.version),
    };
    let image_step = if plan.already_pushed {
        "reuse the already-pushed image"
    } else if plan.builds_image {
        "build + push"
    } else {
        "push the local image"
    };
    let writable = match (plan.writable, plan.writable_path) {
        (_, Some(path)) => format!("yes, at {path}"),
        (true, None) => "yes, at /workspace".to_string(),
        (false, None) => "no".to_string(),
    };
    let env = if plan.env.is_empty() {
        "(none)".to_string()
    } else {
        let mut keys: Vec<&str> = plan.env.keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys.iter()
            .map(|k| format!("{k}=****"))
            .collect::<Vec<_>>()
            .join("  ")
    };
    // The same rule the deploy itself applies, so the plan can't disagree with it.
    let port = match deploy_ports(plan.workload_kind, plan.port).as_slice() {
        [] => "none (driven over stdio)".to_string(),
        ports => ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    };
    let mut lines = vec![
        format!("Plan for {}:", plan.agent_name),
        format!("  action    {action}"),
        format!("  image     {}   ({image_step})", plan.image_ref),
    ];
    if let Some(kind) = plan.workload_kind {
        let line = if kind == "acp_coding_agent" {
            // HITL is only ever sent as on; otherwise the server leaves an existing agent's
            // setting alone and creates a new agent with it off.
            let hitl = match (plan.hitl, plan.agent_exists) {
                (true, _) => "on",
                (false, true) => "unchanged",
                (false, false) => "off",
            };
            format!("coding agent, HITL {hitl}")
        } else {
            kind.to_string()
        };
        lines.push(format!("  kind      {line}"));
    }
    lines.extend([
        format!("  port      {port}"),
        format!("  writable  {writable}"),
        format!("  env       {env}"),
    ]);
    lines
}

fn print_deploy_plan(plan: &DeployPlan) {
    for line in plan_lines(plan) {
        println!("{line}");
    }
    println!();
    println!(
        "{}",
        term::dim("# dry run — nothing built, pushed, or deployed")
    );
}

/// The success line plus where to go next; the MCP-gateway hint follows when the agent's
/// source references the gateway.
fn print_deploy_success(
    agent_name: &str,
    version: &str,
    agent_id: Option<&str>,
    references_mcp_gateway: bool,
) {
    println!();
    match agent_id {
        Some(id) => term::print_success(&format!("Deployed {agent_name}:{version} (id: {id})")),
        None => term::print_success(&format!("Deployed {agent_name}:{version}")),
    }
    println!(
        "  {}   ·   {}",
        term::cyan(&format!(
            "→ {}",
            agent_command(&format!("logs {agent_name}"))
        )),
        term::cyan(&format!(
            "→ {}",
            agent_command(&format!("chat {agent_name}"))
        ))
    );
    if references_mcp_gateway {
        print_mcp_gateway_hint(agent_name);
    }
}

/// Resolve the agent's workload kind for a deploy: an explicit `--coding-agent`
/// flag wins; otherwise a `"workloadKind"` string in AgentCard.json is the
/// default; otherwise `None` (an ordinary HTTP agent — the server defaults the
/// column to `agent`). Returned as the DB token the server validates. Shared
/// with `push` (same flag + card-default rule).
pub(crate) fn resolve_workload_kind(
    coding_agent: bool,
    card: &serde_json::Value,
) -> Option<String> {
    if coding_agent {
        return Some("acp_coding_agent".to_owned());
    }
    card.get("workloadKind")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The container ports for a deploy: a coding agent is driven over stdio and
/// exposes none, so it deploys port-less; every other agent uses its service
/// port.
fn deploy_ports(workload_kind: Option<&str>, port: u16) -> Vec<u16> {
    if workload_kind == Some("acp_coding_agent") {
        Vec::new()
    } else {
        vec![port]
    }
}

/// Gets the currently-deployed version and full version history from an
/// already-looked-up agent. Empty for a brand-new agent.
///
/// A history-fetch failure is propagated, not treated as "no history" —
/// otherwise a reused version could slip past `resolve_deploy_version`.
fn used_version_context<'a>(
    client: &Client,
    existing: Option<&'a (String, serde_json::Value)>,
) -> Result<(Option<&'a str>, Vec<String>)> {
    let current_deployed_version = existing
        .and_then(|(_, current)| current.get("version"))
        .and_then(|v| v.as_str());
    let used_versions = match existing {
        Some((id, _)) => client.used_versions(id)?,
        None => Vec::new(),
    };
    Ok((current_deployed_version, used_versions))
}

/// Whether `version` is already recorded with `status = "pushed"` for this
/// agent — an image `nasiko push` made available in the registry but never
/// deployed. When true, the artifact is already sitting in the registry
/// under this exact tag, so deploy must promote it as-is instead of
/// re-uploading: an OCI tag isn't content-addressed, so pushing again would
/// silently repoint it if the local image has changed since the push.
fn already_pushed(
    client: &Client,
    existing: Option<&(String, serde_json::Value)>,
    version: &str,
) -> Result<bool> {
    let Some((id, _)) = existing else {
        return Ok(false);
    };
    Ok(client.version_status(id, version)?.as_deref() == Some("pushed"))
}

/// Finds the existing agent for a directory deploy: first checks the local
/// cache file, then falls back to looking it up by name (in case the agent
/// was registered another way, e.g. `nasiko push`, and the cache is stale
/// or missing).
fn find_existing_agent_binding(
    client: &Client,
    agent_file: &Path,
    agent_name: &str,
) -> Result<Option<(String, serde_json::Value)>> {
    let existing_id = load_agent_id(agent_file);
    let cached = match &existing_id {
        Some(id) => client.get_agent(id)?.map(|current| (id.clone(), current)),
        None => None,
    };
    if cached.is_some() {
        return Ok(cached);
    }
    if existing_id.is_some() {
        println!(
            "  {} Cached agent binding is stale — checking by name",
            term::yellow("!")
        );
    }

    Ok(client.get_agent(agent_name)?.map(|current| {
        let id = current
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        (id, current)
    }))
}

fn parse_env(env_file: Option<&str>, env_args: &[String]) -> Result<HashMap<String, String>> {
    let mut env = HashMap::new();

    if let Some(path) = env_file {
        let content =
            fs::read_to_string(path).with_context(|| format!("cannot read env file: {path}"))?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let value = value.trim_matches('"').trim_matches('\'');
                env.insert(key.trim().to_string(), value.to_string());
            }
        }
    }

    for arg in env_args {
        if let Some((key, value)) = arg.split_once('=') {
            env.insert(key.to_string(), value.to_string());
        } else {
            anyhow::bail!("invalid env format: {arg} (expected KEY=VALUE)");
        }
    }

    Ok(env)
}

#[allow(clippy::too_many_arguments)]
fn deploy_from_directory(
    dir: &str,
    name_override: Option<&str>,
    port: u16,
    env: &HashMap<String, String>,
    flags: VersionFlags,
    writable: bool,
    writable_path: Option<&str>,
    coding_agent: bool,
    hitl: bool,
    dry_run: bool,
    client: &Client,
) -> Result<()> {
    let root = Path::new(dir);
    let card_path = root.join("AgentCard.json");
    let card: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&card_path).context("cannot read AgentCard.json")?,
    )
    .context("invalid AgentCard.json")?;

    // Flag wins, AgentCard `workloadKind` is the default. Used both to mark the
    // catalog row and to deploy a coding agent port-less.
    let workload_kind = resolve_workload_kind(coding_agent, &card);

    let agent_name = name_override
        .map(String::from)
        .or_else(|| card.get("name").and_then(|n| n.as_str()).map(String::from))
        .unwrap_or_else(|| "agent".into());

    // Checked once up front, same signal `upload.rs`'s `source_references_mcp_gateway` uses for
    // `nasiko upload` — this path builds straight from `root` and never zips it, so it walks the
    // directory directly instead of scanning zip entries.
    let references_mcp_gateway = crate::util::dir_references_mcp_gateway(root);

    // Find the agent first, so we can catch a same-version redeploy below.
    let agent_file = root.join(AGENT_FILE);
    let existing = find_existing_agent_binding(client, &agent_file, &agent_name)?;

    let (current_deployed_version, used_versions) =
        used_version_context(client, existing.as_ref())?;
    let card_version = card.get("version").and_then(|v| v.as_str());
    let context = VersionContext {
        card_version,
        current_deployed_version,
        used_versions: &used_versions,
    };
    let decision = resolve_deploy_version(context, flags)?;
    let version = decision.version;
    let repo = format!("nasiko/{agent_name}");
    let image_ref = format!("{repo}:{version}");
    let reuse_pushed = already_pushed(client, existing.as_ref(), &version)?;

    if dry_run {
        print_deploy_plan(&DeployPlan {
            agent_name: &agent_name,
            agent_exists: existing.is_some(),
            current_version: current_deployed_version,
            version: &version,
            image_ref: &image_ref,
            already_pushed: reuse_pushed,
            builds_image: true,
            port,
            workload_kind: workload_kind.as_deref(),
            hitl,
            writable,
            writable_path,
            env,
        });
        return Ok(());
    }

    if reuse_pushed {
        println!(
            "  Version {version} was already pushed — deploying the existing registry image \
             without rebuilding."
        );
    } else {
        let image_tag = format!("{agent_name}:{version}");

        // Build for linux/amd64 (the cluster's arch), not the host arch — an
        // Apple Silicon build here would CrashLoop with "exec format error".
        super::build::build(dir, Some(&image_tag), Some("linux/amd64"))?;

        // Push image to OCI
        println!("Pushing {image_tag} → {repo}:{version}...");
        oci::push_image(&image_tag, &repo, &version)?;
    }

    let agent_id = match existing {
        Some((id, current)) => {
            let current_version = current
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            println!("  Updating: {current_version} → {version}");
            save_agent_id(&agent_file, &id, &agent_name)?;

            // Deploy the container BEFORE activating the new version in the
            // catalog — activating first would archive the version that's
            // genuinely still running and mark it a rollback target even
            // though the deploy below hasn't happened yet, so a failed
            // deploy would leave history claiming a version is live when
            // it never actually ran.
            println!("Deploying container...");
            let spec = DeploySpec {
                image: image_ref.clone(),
                name: agent_name.clone(),
                ports: deploy_ports(workload_kind.as_deref(), port),
                env: env.clone(),
                writable,
                writable_path: writable_path.map(str::to_owned),
                workload_kind: workload_kind.clone(),
            };
            let status: ContainerStatus = client.post_json("/containers", &spec)?;
            println!("  {} → {}", agent_name, status.state);

            let update = serde_json::json!({
                "version": version,
                "image": image_ref,
                "description": card.get("description"),
                "skills": card.get("skills"),
                "capabilities": card.get("capabilities"),
                "workload_kind": workload_kind,
                // Only send when set, so a plain redeploy never flips HITL off.
                "hitl_enabled": hitl.then_some(true),
            });
            let _: serde_json::Value = client.put_json(&format!("/agents/{id}"), &update)?;
            if let Err(e) = crate::util::sync_card_version(&card_path, &card, &version) {
                term::print_warning(&format!(
                    "Deployed, but failed to update AgentCard.json's version: {e}"
                ));
            }
            print_deploy_success(&agent_name, &version, Some(&id), references_mcp_gateway);
            return Ok(());
        }
        None => {
            // Create new agent — nothing was running before, so there's no
            // prior version to falsely archive; register then deploy as before.
            println!("  Registering new agent: {agent_name}");
            let create = serde_json::json!({
                "name": agent_name,
                "display_name": card.get("name").and_then(|n| n.as_str()).unwrap_or(&agent_name),
                "description": card.get("description").and_then(|d| d.as_str()).unwrap_or(""),
                "url": card.get("url").and_then(|u| u.as_str()).unwrap_or(""),
                "version": version,
                "image": image_ref,
                "skills": card.get("skills").unwrap_or(&serde_json::json!([])),
                "capabilities": card.get("capabilities"),
                "workload_kind": workload_kind,
                "hitl_enabled": hitl.then_some(true),
            });
            let resp: serde_json::Value = client.post_json("/agents", &create)?;
            let id = resp
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string();
            save_agent_id(&agent_file, &id, &agent_name)?;
            id
        }
    };

    // Deploy container
    println!("Deploying container...");
    let spec = DeploySpec {
        image: image_ref.clone(),
        name: agent_name.clone(),
        ports: deploy_ports(workload_kind.as_deref(), port),
        env: env.clone(),
        writable,
        writable_path: writable_path.map(str::to_owned),
        workload_kind: workload_kind.clone(),
    };
    let status: ContainerStatus = client.post_json("/containers", &spec)?;
    println!("  {} → {}", agent_name, status.state);
    if let Err(e) = crate::util::sync_card_version(&card_path, &card, &version) {
        term::print_warning(&format!(
            "Deployed, but failed to update AgentCard.json's version: {e}"
        ));
    }
    print_deploy_success(
        &agent_name,
        &version,
        Some(&agent_id),
        references_mcp_gateway,
    );
    Ok(())
}

/// Same "give it tool access" suggestion `upload.rs` prints for a source that references the MCP
/// gateway — kept here too since `deploy_from_directory` has its own two success points.
fn print_mcp_gateway_hint(agent_name: &str) {
    println!(
        "\nThis agent's source references the MCP gateway — to give it tool access:\n\
         \x20 nasiko mcp catalog                                      # find a connector\n\
         \x20 nasiko mcp connect --connector-id <id>                  # connect your account (if not already)\n\
         \x20 nasiko mcp agent-tools enable {agent_name} <id>                   # grant this agent access"
    );
}

// Same values `deploy_with_version_flags` threads into
// `deploy_from_directory` above, which carries the same allow.
#[allow(clippy::too_many_arguments)]
fn deploy_from_image(
    image: &str,
    name_override: Option<&str>,
    port: u16,
    env: &HashMap<String, String>,
    flags: VersionFlags,
    writable: bool,
    writable_path: Option<&str>,
    coding_agent: bool,
    hitl: bool,
    dry_run: bool,
    client: &Client,
) -> Result<()> {
    // No AgentCard.json on the raw-image path, so only the explicit flag can mark
    // a coding agent here.
    let workload_kind: Option<String> = coding_agent.then(|| "acp_coding_agent".to_owned());

    // A bare image name means Docker's implicit `:latest`, not a real
    // choice — require an explicit tag so the deployed version always
    // matches what `nasiko build` actually produced.
    if !crate::util::image_has_explicit_tag(image) {
        anyhow::bail!(
            "deploy requires an explicit image:tag (e.g. {image}:1.0.1) — run `nasiko build` \
             first, then deploy exactly the tag it printed."
        );
    }
    if !oci::local_image_exists(image)? {
        anyhow::bail!("no local image found for {image} — build it first with `nasiko build`.");
    }

    let (image_name, image_tag_version) = parse_image_name_and_tag(image);
    let agent_name = name_override.map(String::from).unwrap_or(image_name);
    let repo = format!("nasiko/{agent_name}");

    // Upsert agent in registry: update if exists, create otherwise.
    let existing = client.get_agent(&agent_name)?.map(|agent| {
        let id = agent
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        (id, agent)
    });
    let (current_deployed_version, used_versions) =
        used_version_context(client, existing.as_ref())?;
    let decision = resolve_image_deploy_version(
        image,
        &image_tag_version,
        flags,
        current_deployed_version,
        &used_versions,
        "deploy",
    )?;
    let version = decision.version;

    let image_ref = format!("{repo}:{version}");
    let reuse_pushed = already_pushed(client, existing.as_ref(), &version)?;

    if dry_run {
        print_deploy_plan(&DeployPlan {
            agent_name: &agent_name,
            agent_exists: existing.is_some(),
            current_version: current_deployed_version,
            version: &version,
            image_ref: &image_ref,
            already_pushed: reuse_pushed,
            builds_image: false,
            port,
            workload_kind: workload_kind.as_deref(),
            hitl,
            writable,
            writable_path,
            env,
        });
        return Ok(());
    }

    if reuse_pushed {
        println!(
            "  Version {version} was already pushed — deploying the existing registry image \
             without re-pushing."
        );
    } else {
        // Tag locally so Docker can find it by the canonical ref without a registry pull.
        let tag_status = std::process::Command::new(crate::util::container_bin())
            .args(["tag", image, &image_ref])
            .status()
            .context("failed to run container tag command — is the container runtime running?")?;
        if !tag_status.success() {
            anyhow::bail!("failed to tag {image} as {image_ref}");
        }

        println!("Pushing {image} → {image_ref}...");
        oci::push_image(image, &repo, &version)?;
    }

    if let Some((id, _)) = existing {
        println!("  Updating agent: {agent_name}");

        // Deploy the container BEFORE activating the new version in the
        // catalog — see the matching comment in `deploy_from_directory`.
        println!("Deploying container...");
        let spec = DeploySpec {
            image: image_ref.clone(),
            name: agent_name.clone(),
            ports: deploy_ports(workload_kind.as_deref(), port),
            env: env.clone(),
            writable,
            writable_path: writable_path.map(str::to_owned),
            workload_kind: workload_kind.clone(),
        };
        let status: ContainerStatus = client.post_json("/containers", &spec)?;
        println!("  {} → {}", agent_name, status.state);

        let update = serde_json::json!({
            "version": version,
            "image": image_ref,
            "workload_kind": workload_kind,
            // Only send when set, so a plain redeploy never flips HITL off.
            "hitl_enabled": hitl.then_some(true),
        });
        let _: serde_json::Value = client.put_json(&format!("/agents/{id}"), &update)?;
        print_deploy_success(&agent_name, &version, Some(&id), false);
        return Ok(());
    }

    println!("  Registering agent: {agent_name}");
    let create = serde_json::json!({
        "name": agent_name,
        "display_name": agent_name,
        "version": version,
        "image": image_ref,
        "workload_kind": workload_kind,
        "hitl_enabled": hitl.then_some(true),
    });
    let _: serde_json::Value = client.post_json("/agents", &create)?;

    // Deploy container — nothing was running before, so there's no prior
    // version to falsely archive; register then deploy as before.
    println!("Deploying container...");
    let spec = DeploySpec {
        image: image_ref,
        name: agent_name.clone(),
        ports: deploy_ports(workload_kind.as_deref(), port),
        env: env.clone(),
        writable,
        writable_path: writable_path.map(str::to_owned),
        workload_kind: workload_kind.clone(),
    };
    let status: ContainerStatus = client.post_json("/containers", &spec)?;
    println!("  {} → {}", agent_name, status.state);
    print_deploy_success(&agent_name, &version, None, false);
    Ok(())
}

fn load_agent_id(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("agent_id")
        .and_then(|i| i.as_str())
        .map(String::from)
}

fn save_agent_id(path: &Path, agent_id: &str, name: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::json!({
        "agent_id": agent_id,
        "name": name,
    });
    fs::write(path, serde_json::to_string_pretty(&data)?)?;
    Ok(())
}

// Kept in a separate file (`tests/deploy_tests.rs`) instead of an inline
// `mod tests { ... }` block — see the matching comment in `push.rs`.
#[cfg(test)]
#[path = "tests/deploy_tests.rs"]
mod tests;
