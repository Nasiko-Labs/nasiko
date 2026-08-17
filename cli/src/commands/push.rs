use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::api::Client;
use crate::oci;
use crate::util::parse_image_name_and_tag;
use crate::version_prompt::{
    VersionContext, VersionFlags, resolve_deploy_version, resolve_image_deploy_version,
};

/// Push an agent image to the cluster's OCI registry and register in catalog.
/// Does NOT deploy a container.
pub fn push(image: &str, name_override: Option<&str>) -> Result<()> {
    push_with_version_flags(image, name_override, VersionFlags::default())
}

pub fn push_with_version_flags(
    image: &str,
    name_override: Option<&str>,
    flags: VersionFlags,
) -> Result<()> {
    let client = Client::from_active_cluster()?;

    if Path::new(image).join("AgentCard.json").exists() {
        push_from_directory(image, name_override, flags, &client)
    } else {
        push_from_image(image, name_override, flags, &client)
    }
}

fn push_from_directory(
    dir: &str,
    name_override: Option<&str>,
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
    let existing = lookup_existing(client, &agent_name)?;
    let (current_deployed_version, used_versions) = used_version_context(client, &existing)?;
    let card_version = card.get("version").and_then(|v| v.as_str());
    let context = VersionContext {
        card_version,
        current_deployed_version: current_deployed_version.as_deref(),
        used_versions: &used_versions,
    };
    let decision = resolve_deploy_version(context, flags)?;
    let version = decision.version;
    let image_tag = format!("{agent_name}:{version}");

    // Build for linux/amd64 (the cluster's arch), not the host arch — an
    // Apple Silicon build here would CrashLoop with "exec format error".
    super::build::build(dir, Some(&image_tag), Some("linux/amd64"))?;

    // Push to OCI
    let repo = format!("nasiko/{agent_name}");
    println!("Pushing {image_tag} → {repo}:{version}...");
    oci::push_image(&image_tag, &repo, &version)?;

    let image_ref = format!("{repo}:{version}");

    // Register in catalog
    register_agent(client, &agent_name, &version, &image_ref, &card)?;

    println!("\n✓ Pushed {agent_name}:{version} (image: {image_ref})");
    println!("  Deploy with: nasiko deploy {dir}");
    Ok(())
}

fn push_from_image(
    image: &str,
    name_override: Option<&str>,
    flags: VersionFlags,
    client: &Client,
) -> Result<()> {
    // Same guard as `deploy_from_image`: require an explicit tag, not
    // Docker's implicit `:latest`.
    if !crate::util::image_has_explicit_tag(image) {
        anyhow::bail!(
            "push requires an explicit image:tag (e.g. {image}:1.0.1) — run `nasiko build` \
             first, then push exactly the tag it printed."
        );
    }
    if !oci::local_image_exists(image)? {
        anyhow::bail!("no local image found for {image} — build it first with `nasiko build`.");
    }

    let (image_name, image_tag_version) = parse_image_name_and_tag(image);
    let agent_name = name_override.map(String::from).unwrap_or(image_name);
    let repo = format!("nasiko/{agent_name}");

    let existing = lookup_existing(client, &agent_name)?;
    let (current_deployed_version, used_versions) = used_version_context(client, &existing)?;
    let decision = resolve_image_deploy_version(
        image,
        &image_tag_version,
        flags,
        current_deployed_version.as_deref(),
        &used_versions,
        "push",
    )?;
    let version = decision.version;

    println!("Pushing {image} → {repo}:{version}...");
    oci::push_image(image, &repo, &version)?;

    let image_ref = format!("{repo}:{version}");

    // Register in catalog
    let card = serde_json::json!({});
    register_agent(client, &agent_name, &version, &image_ref, &card)?;

    println!("\n✓ Pushed {agent_name}:{version} (image: {image_ref})");
    println!("  Deploy with: nasiko deploy {image}");
    Ok(())
}

/// Gets the currently-deployed version and full version history from
/// [`lookup_existing`]'s result. Empty for a brand-new agent.
///
/// A history-fetch failure is propagated, not treated as "no history" —
/// otherwise a reused version could slip past `resolve_deploy_version`.
fn used_version_context(
    client: &Client,
    existing: &Option<(String, Option<String>)>,
) -> Result<(Option<String>, Vec<String>)> {
    let current_deployed_version = existing.as_ref().and_then(|(_, v)| v.clone());
    let used_versions = match existing {
        Some((id, _)) => client.used_versions(id)?,
        None => Vec::new(),
    };
    Ok((current_deployed_version, used_versions))
}

/// Looks up an existing agent's id and version by name. `Ok(None)` means it
/// doesn't exist (a real 404) — a network/auth failure is a real `Err`, not
/// treated as "new agent", so a transient failure can't register a
/// duplicate agent.
fn lookup_existing(client: &Client, name: &str) -> Result<Option<(String, Option<String>)>> {
    let Some(agent) = client.get_agent(name)? else {
        return Ok(None);
    };
    let Some(id) = agent.get("id").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let version = agent
        .get("version")
        .and_then(|v| v.as_str())
        .map(String::from);
    Ok(Some((id.to_string(), version)))
}

fn register_agent(
    client: &Client,
    name: &str,
    version: &str,
    image_ref: &str,
    card: &serde_json::Value,
) -> Result<()> {
    // Update if the agent already exists (from a prior push/deploy), else create it.
    let existing = client.get_agent(name)?;
    if let Some(existing) = existing {
        let id = existing.get("id").and_then(|v| v.as_str()).unwrap_or("");
        println!("  Registering pushed version in catalog: {name} @ {version} (not deployed)");
        // `push` never deploys — `activate_version: false` records this version
        // in history without marking it active or archiving whatever agent
        // version is actually still running (see
        // `versions::record_pushed_version_in_tx` on the server).
        let update = serde_json::json!({
            "version": version,
            "image": image_ref,
            "description": card.get("description"),
            "skills": card.get("skills"),
            "capabilities": card.get("capabilities"),
            "activate_version": false,
        });
        let _: serde_json::Value = client.put_json(&format!("/agents/{id}"), &update)?;
        return Ok(());
    }

    println!("  Registering in catalog: {name}");
    let create = serde_json::json!({
        "name": name,
        "display_name": card.get("name").and_then(|n| n.as_str()).unwrap_or(name),
        "description": card.get("description").and_then(|d| d.as_str()).unwrap_or(""),
        "version": version,
        "image": image_ref,
        "skills": card.get("skills").unwrap_or(&serde_json::json!([])),
        "capabilities": card.get("capabilities"),
    });
    let _: serde_json::Value = client.post_json("/agents", &create)?;
    Ok(())
}

// Kept in a separate file (`tests/push_tests.rs`) instead of an inline
// `mod tests { ... }` block so this already-sizable command module doesn't
// keep growing every time coverage is added — `#[path]` still makes it a
// child of this module, so it can call the private helpers above directly.
#[cfg(test)]
#[path = "tests/push_tests.rs"]
mod tests;
