use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::api::Client;
use crate::oci;

/// Push an agent image to the cluster's OCI registry and register in catalog.
/// Does NOT deploy a container.
pub fn push(image: &str, name_override: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;

    if Path::new(image).join("AgentCard.json").exists() {
        push_from_directory(image, name_override, &client)
    } else {
        push_from_image(image, name_override, &client)
    }
}

fn push_from_directory(dir: &str, name_override: Option<&str>, client: &Client) -> Result<()> {
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
    let version = card
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("latest");
    let image_tag = format!("{agent_name}:{version}");

    // Build image
    // Cluster nodes are amd64 on every supported provider — build for the
    // deployment target, not the host arch, or Apple Silicon builds
    // CrashLoop on the cluster with "exec format error".
    super::build::build(dir, Some(&image_tag), Some("linux/amd64"))?;

    // Push to OCI
    let repo = format!("nasiko/{agent_name}");
    println!("Pushing {image_tag} → {repo}:{version}...");
    oci::push_image(&image_tag, &repo, version)?;

    let image_ref = format!("{repo}:{version}");

    // Register in catalog
    register_agent(client, &agent_name, version, &image_ref, &card)?;

    println!("\n✓ Pushed {agent_name}:{version} (image: {image_ref})");
    println!("  Deploy with: nasiko deploy {dir}");
    Ok(())
}

fn push_from_image(image: &str, name_override: Option<&str>, client: &Client) -> Result<()> {
    let agent_name = name_override.map(String::from).unwrap_or_else(|| {
        image
            .rsplit('/')
            .next()
            .unwrap_or(image)
            .split(':')
            .next()
            .unwrap_or("agent")
            .to_string()
    });
    let version = image.split(':').nth(1).unwrap_or("latest");
    let repo = format!("nasiko/{agent_name}");

    println!("Pushing {image} → {repo}:{version}...");
    oci::push_image(image, &repo, version)?;

    let image_ref = format!("{repo}:{version}");

    // Register in catalog
    let card = serde_json::json!({});
    register_agent(client, &agent_name, version, &image_ref, &card)?;

    println!("\n✓ Pushed {agent_name}:{version} (image: {image_ref})");
    println!("  Deploy with: nasiko deploy {image}");
    Ok(())
}

fn register_agent(
    client: &Client,
    name: &str,
    version: &str,
    image_ref: &str,
    card: &serde_json::Value,
) -> Result<()> {
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
