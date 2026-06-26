use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Result, Context};

use crate::api::{Client, DeploySpec, ContainerStatus};
use crate::oci;

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
pub fn deploy(image: &str, name: Option<&str>, port: u16, env_file: Option<&str>, env_args: &[String]) -> Result<()> {
    let client = Client::from_active_cluster()?;
    let env = parse_env(env_file, env_args)?;

    if Path::new(image).join("AgentCard.json").exists() {
        deploy_from_directory(image, name, port, &env, &client)
    } else {
        deploy_from_image(image, name, port, &env, &client)
    }
}

fn parse_env(env_file: Option<&str>, env_args: &[String]) -> Result<HashMap<String, String>> {
    let mut env = HashMap::new();

    if let Some(path) = env_file {
        let content = fs::read_to_string(path)
            .with_context(|| format!("cannot read env file: {path}"))?;
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

fn deploy_from_directory(dir: &str, name_override: Option<&str>, port: u16, env: &HashMap<String, String>, client: &Client) -> Result<()> {
    let root = Path::new(dir);
    let card_path = root.join("AgentCard.json");
    let card: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&card_path).context("cannot read AgentCard.json")?
    ).context("invalid AgentCard.json")?;

    let agent_name = name_override
        .map(String::from)
        .or_else(|| card.get("name").and_then(|n| n.as_str()).map(String::from))
        .unwrap_or_else(|| "agent".into());
    let version = card.get("version").and_then(|v| v.as_str()).unwrap_or("latest");
    let image_tag = format!("{agent_name}:{version}");

    // Auto-build before pushing
    super::build::build(dir, Some(&image_tag), None)?;

    // Push image to OCI
    let repo = format!("nasiko/{agent_name}");
    println!("Pushing {image_tag} → {repo}:{version}...");
    oci::push_image(&image_tag, &repo, version)?;

    let image_ref = format!("{repo}:{version}");

    // Check for existing agent binding
    let agent_file = root.join(AGENT_FILE);
    let existing_id = load_agent_id(&agent_file);

    let agent_id = match existing_id {
        Some(id) => {
            // Update existing agent
            let current: serde_json::Value = client.get_json(&format!("/catalog/agents/{id}"))?;
            let current_version = current.get("version").and_then(|v| v.as_str()).unwrap_or("");
            if current_version == version {
                eprintln!("  ! Redeploying same version ({version}) — consider bumping version in AgentCard.json");
            } else {
                println!("  Updating: {current_version} → {version}");
            }

            let update = serde_json::json!({
                "version": version,
                "image": image_ref,
                "description": card.get("description"),
                "skills": card.get("skills"),
                "capabilities": card.get("capabilities"),
            });
            let _: serde_json::Value = client.put_json(&format!("/catalog/agents/{id}"), &update)?;
            id
        }
        None => {
            // Create new agent
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
            let resp: serde_json::Value = client.post_json("/catalog/agents", &create)?;
            let id = resp.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
            save_agent_id(&agent_file, &id, &agent_name)?;
            id
        }
    };

    // Deploy container
    println!("Deploying container...");
    let spec = DeploySpec {
        image: image_ref.clone(),
        name: agent_name.clone(),
        port: Some(port),
        env: env.clone(),
    };
    let status: ContainerStatus = client.post_json("/containers", &spec)?;
    println!("  {} → {}", agent_name, status.state);
    println!("\n✓ Deployed {agent_name}:{version} (id: {agent_id})");
    Ok(())
}

fn deploy_from_image(image: &str, name_override: Option<&str>, port: u16, env: &HashMap<String, String>, client: &Client) -> Result<()> {
    let agent_name = name_override
        .map(String::from)
        .unwrap_or_else(|| image.split(':').next().unwrap_or("agent").replace('/', "-"));
    let version = image.split(':').nth(1).unwrap_or("latest");
    let repo = format!("nasiko/{agent_name}");

    println!("Pushing {image} → {repo}:{version}...");
    oci::push_image(image, &repo, version)?;

    let image_ref = format!("{repo}:{version}");

    // Create agent in registry (no local dir to save ID to)
    println!("  Registering agent: {agent_name}");
    let create = serde_json::json!({
        "name": agent_name,
        "display_name": agent_name,
        "version": version,
        "image": image_ref,
    });
    let _: serde_json::Value = client.post_json("/catalog/agents", &create)?;

    // Deploy container
    println!("Deploying container...");
    let spec = DeploySpec {
        image: image_ref,
        name: agent_name.clone(),
        port: Some(port),
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
    val.get("agent_id").and_then(|i| i.as_str()).map(String::from)
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
