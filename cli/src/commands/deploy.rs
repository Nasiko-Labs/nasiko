use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::api::{Client, ContainerStatus, DeploySpec};
use crate::oci;
use crate::util::parse_image_name_and_tag;
use crate::version_prompt::{VersionContext, VersionFlags, resolve_deploy_version};

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
pub fn deploy(
    image: &str,
    name: Option<&str>,
    port: u16,
    env_file: Option<&str>,
    env_args: &[String],
) -> Result<()> {
    deploy_with_version_flags(
        image,
        name,
        port,
        env_file,
        env_args,
        VersionFlags::default(),
    )
}

pub fn deploy_with_version_flags(
    image: &str,
    name: Option<&str>,
    port: u16,
    env_file: Option<&str>,
    env_args: &[String],
    flags: VersionFlags,
) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let env = parse_env(env_file, env_args)?;

    if Path::new(image).join("AgentCard.json").exists() {
        deploy_from_directory(image, name, port, &env, flags, &client)
    } else {
        deploy_from_image(image, name, port, &env, flags, &client)
    }
}

/// Gets the currently-deployed version and full version history from an
/// already-looked-up agent. Both come back empty for a brand-new agent.
/// Shared by `deploy_from_directory` and `deploy_from_image`.
///
/// Propagates a history-fetch failure instead of treating it as "no
/// history" — failing open there would let a duplicate/already-used version
/// through the check in `resolve_deploy_version` and push/deploy over that
/// version's content before the server gets a chance to reject the update.
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
        println!("  ! Cached agent binding is stale — checking by name");
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

fn deploy_from_directory(
    dir: &str,
    name_override: Option<&str>,
    port: u16,
    env: &HashMap<String, String>,
    flags: VersionFlags,
    client: &Client,
) -> Result<()> {
    let root = Path::new(dir);
    let card_path = root.join("AgentCard.json");
    let card: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&card_path).context("cannot read AgentCard.json")?,
    )
    .context("invalid AgentCard.json")?;

    let agent_name = name_override
        .map(String::from)
        .or_else(|| card.get("name").and_then(|n| n.as_str()).map(String::from))
        .unwrap_or_else(|| "agent".into());

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
    let image_tag = format!("{agent_name}:{version}");

    // Build for linux/amd64 (the cluster's arch), not the host arch — an
    // Apple Silicon build here would CrashLoop with "exec format error".
    super::build::build(dir, Some(&image_tag), Some("linux/amd64"))?;

    // Push image to OCI
    let repo = format!("nasiko/{agent_name}");
    println!("Pushing {image_tag} → {repo}:{version}...");
    oci::push_image(&image_tag, &repo, &version)?;

    let image_ref = format!("{repo}:{version}");

    let agent_id = match existing {
        Some((id, current)) => {
            let current_version = current
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if decision.overwrite {
                println!("  Overwriting: {version} (was: {current_version})");
            } else {
                println!("  Updating: {current_version} → {version}");
            }
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
                ports: vec![port],
                env: env.clone(),
            };
            let status: ContainerStatus = client.post_json("/containers", &spec)?;
            println!("  {} → {}", agent_name, status.state);

            let update = serde_json::json!({
                "version": version,
                "image": image_ref,
                "description": card.get("description"),
                "skills": card.get("skills"),
                "capabilities": card.get("capabilities"),
                "allow_overwrite": decision.overwrite,
            });
            let _: serde_json::Value = client.put_json(&format!("/agents/{id}"), &update)?;
            println!("\n✓ Deployed {agent_name}:{version} (id: {id})");
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
        ports: vec![port],
        env: env.clone(),
    };
    let status: ContainerStatus = client.post_json("/containers", &spec)?;
    println!("  {} → {}", agent_name, status.state);
    println!("\n✓ Deployed {agent_name}:{version} (id: {agent_id})");
    Ok(())
}

fn deploy_from_image(
    image: &str,
    name_override: Option<&str>,
    port: u16,
    env: &HashMap<String, String>,
    flags: VersionFlags,
    client: &Client,
) -> Result<()> {
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
    // Only trust the tag if the user actually wrote one (`image:tag`) — a
    // bare `image` implicitly means Docker's "latest", not a real choice.
    let card_version =
        crate::util::image_has_explicit_tag(image).then_some(image_tag_version.as_str());
    let context = VersionContext {
        card_version,
        current_deployed_version,
        used_versions: &used_versions,
    };
    let decision = resolve_deploy_version(context, flags)?;
    let version = decision.version;

    let image_ref = format!("{repo}:{version}");

    // Tag locally so Docker can find it by the canonical ref without a registry pull.
    let _ = std::process::Command::new("docker")
        .args(["tag", image, &image_ref])
        .status();

    println!("Pushing {image} → {image_ref}...");
    oci::push_image(image, &repo, &version)?;

    if let Some((id, _)) = existing {
        if decision.overwrite {
            println!("  Overwriting agent: {agent_name} @ {version}");
        } else {
            println!("  Updating agent: {agent_name}");
        }

        // Deploy the container BEFORE activating the new version in the
        // catalog — see the matching comment in `deploy_from_directory`.
        println!("Deploying container...");
        let spec = DeploySpec {
            image: image_ref.clone(),
            name: agent_name.clone(),
            ports: vec![port],
            env: env.clone(),
        };
        let status: ContainerStatus = client.post_json("/containers", &spec)?;
        println!("  {} → {}", agent_name, status.state);

        let update = serde_json::json!({
            "version": version,
            "image": image_ref,
            "allow_overwrite": decision.overwrite,
        });
        let _: serde_json::Value = client.put_json(&format!("/agents/{id}"), &update)?;
        println!("\n✓ Deployed {agent_name}:{version}");
        return Ok(());
    }

    println!("  Registering agent: {agent_name}");
    let create = serde_json::json!({
        "name": agent_name,
        "display_name": agent_name,
        "version": version,
        "image": image_ref,
    });
    let _: serde_json::Value = client.post_json("/agents", &create)?;

    // Deploy container — nothing was running before, so there's no prior
    // version to falsely archive; register then deploy as before.
    println!("Deploying container...");
    let spec = DeploySpec {
        image: image_ref,
        name: agent_name.clone(),
        ports: vec![port],
        env: env.clone(),
    };
    let status: ContainerStatus = client.post_json("/containers", &spec)?;
    println!("  {} → {}", agent_name, status.state);
    println!("\n✓ Deployed {agent_name}:{version}");
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
