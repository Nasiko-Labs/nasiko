use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::api::OciClient;
use crate::config;

const AGENT_MEDIA_TYPE: &str = "application/vnd.nasiko.agent.v1.tar+gzip";
const SKILL_MEDIA_TYPE: &str = "application/vnd.nasiko.skill.v1.tar+gzip";
const CONFIG_MEDIA_TYPE: &str = "application/vnd.nasiko.config.v1+json";
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

pub fn publish(directory: &str, owner: Option<&str>) -> Result<()> {
    let dir = Path::new(directory);
    let registry_url = config::artifact_registry_url()
        .ok_or_else(|| anyhow::anyhow!("NASIKO_REGISTRY_URL not set"))?;

    let oci = OciClient::for_artifact_registry()?
        .ok_or_else(|| anyhow::anyhow!("cannot connect to registry at {registry_url}"))?;

    if dir.join("skill.json").exists() {
        publish_skill(dir, owner.unwrap_or("nasiko"), &oci)
    } else if dir.join("AgentCard.json").exists() {
        publish_agent(dir, owner.unwrap_or("nasiko"), &oci)
    } else {
        bail!("no AgentCard.json or skill.json found in {directory}")
    }
}

fn publish_agent(dir: &Path, owner: &str, oci: &OciClient) -> Result<()> {
    let card_content = fs::read_to_string(dir.join("AgentCard.json"))
        .context("cannot read AgentCard.json")?;
    let card: serde_json::Value = serde_json::from_str(&card_content)
        .context("invalid AgentCard.json")?;

    let raw_name = card.get("name").and_then(|n| n.as_str())
        .context("AgentCard.json missing 'name'")?;
    let version = card.get("version").and_then(|v| v.as_str())
        .context("AgentCard.json missing 'version'")?;

    let name = slugify(raw_name);
    let repo = format!("{owner}/{name}");
    println!("Publishing {repo}:{version}...");

    // 1. Tar source → layer blob
    let tarball = tar_directory(dir)?;
    let layer_digest = sha256_hex(&tarball);
    let layer_size = tarball.len();

    eprint!("  pushing layer ({} bytes)... ", layer_size);
    oci.push_blob(&repo, &layer_digest, &tarball)?;
    eprintln!("done");

    // 2. Config blob = AgentCard.json
    let config_bytes = card_content.as_bytes();
    let config_digest = sha256_hex(config_bytes);
    let config_size = config_bytes.len();

    eprint!("  pushing config... ");
    oci.push_blob(&repo, &config_digest, config_bytes)?;
    eprintln!("done");

    // 3. Build and push manifest
    let annotations = build_annotations(&card, "agent");
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "artifactType": "application/vnd.nasiko.agent.v1",
        "config": {
            "mediaType": CONFIG_MEDIA_TYPE,
            "digest": config_digest,
            "size": config_size,
        },
        "layers": [{
            "mediaType": AGENT_MEDIA_TYPE,
            "digest": layer_digest,
            "size": layer_size,
        }],
        "annotations": annotations,
    });

    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    eprint!("  pushing manifest... ");
    oci.push_manifest(&repo, version, MANIFEST_MEDIA_TYPE, &manifest_bytes)?;
    eprintln!("done");

    println!("✓ Published {repo}:{version}");
    Ok(())
}

fn publish_skill(dir: &Path, owner: &str, oci: &OciClient) -> Result<()> {
    let manifest_content = fs::read_to_string(dir.join("skill.json"))
        .context("cannot read skill.json")?;
    let manifest_json: serde_json::Value = serde_json::from_str(&manifest_content)
        .context("invalid skill.json")?;

    let raw_name = manifest_json.pointer("/metadata/name").and_then(|n| n.as_str())
        .context("skill.json missing metadata.name")?;
    let version = manifest_json.pointer("/metadata/version").and_then(|v| v.as_str())
        .context("skill.json missing metadata.version")?;

    let name = slugify(raw_name);
    let repo = format!("{owner}/{name}");
    println!("Publishing skill {repo}:{version}...");

    // 1. Tar source → layer blob
    let tarball = tar_directory(dir)?;
    let layer_digest = sha256_hex(&tarball);
    let layer_size = tarball.len();

    eprint!("  pushing layer ({} bytes)... ", layer_size);
    oci.push_blob(&repo, &layer_digest, &tarball)?;
    eprintln!("done");

    // 2. Config blob = skill.json
    let config_bytes = manifest_content.as_bytes();
    let config_digest = sha256_hex(config_bytes);
    let config_size = config_bytes.len();

    eprint!("  pushing config... ");
    oci.push_blob(&repo, &config_digest, config_bytes)?;
    eprintln!("done");

    // 3. Build and push manifest
    let annotations = build_skill_annotations(&manifest_json);
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "artifactType": "application/vnd.nasiko.skill.v1",
        "config": {
            "mediaType": CONFIG_MEDIA_TYPE,
            "digest": config_digest,
            "size": config_size,
        },
        "layers": [{
            "mediaType": SKILL_MEDIA_TYPE,
            "digest": layer_digest,
            "size": layer_size,
        }],
        "annotations": annotations,
    });

    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    eprint!("  pushing manifest... ");
    oci.push_manifest(&repo, version, MANIFEST_MEDIA_TYPE, &manifest_bytes)?;
    eprintln!("done");

    println!("✓ Published skill {repo}:{version}");
    Ok(())
}

fn slugify(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .replace(' ', "-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    s.trim_matches('-').to_string()
}

fn tar_directory(dir: &Path) -> Result<Vec<u8>> {
    let output = Command::new("tar")
        .args([
            "-czf", "-",
            "--exclude=.env",
            "--exclude=__pycache__",
            "--exclude=*.pyc",
            "--exclude=target",
            "--exclude=node_modules",
            "--exclude=.git",
            ".",
        ])
        .current_dir(dir)
        .output()
        .context("failed to run tar")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("tar failed: {stderr}");
    }
    Ok(output.stdout)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

fn build_annotations(card: &serde_json::Value, artifact_type: &str) -> serde_json::Value {
    let mut annotations = serde_json::Map::new();
    annotations.insert("org.nasiko.type".into(), artifact_type.into());

    if let Some(desc) = card.get("description").and_then(|d| d.as_str()) {
        annotations.insert("org.opencontainers.image.description".into(), desc.into());
    }
    if let Some(version) = card.get("version").and_then(|v| v.as_str()) {
        annotations.insert("org.opencontainers.image.version".into(), version.into());
    }
    if let Some(fw) = card.get("agentFramework").and_then(|f| f.as_str()) {
        annotations.insert("org.nasiko.framework".into(), fw.into());
    }

    // Collect tags from skills
    if let Some(skills) = card.get("skills").and_then(|s| s.as_array()) {
        let tags: Vec<&str> = skills
            .iter()
            .filter_map(|s| s.get("tags").and_then(|t| t.as_array()))
            .flatten()
            .filter_map(|t| t.as_str())
            .collect();
        if !tags.is_empty() {
            annotations.insert("org.nasiko.tags".into(), tags.join(",").into());
        }
    }

    serde_json::Value::Object(annotations)
}

fn build_skill_annotations(manifest: &serde_json::Value) -> serde_json::Value {
    let mut annotations = serde_json::Map::new();
    annotations.insert("org.nasiko.type".into(), "skill".into());

    if let Some(desc) = manifest.pointer("/metadata/description").and_then(|d| d.as_str()) {
        annotations.insert("org.opencontainers.image.description".into(), desc.into());
    }
    if let Some(version) = manifest.pointer("/metadata/version").and_then(|v| v.as_str()) {
        annotations.insert("org.opencontainers.image.version".into(), version.into());
    }
    if let Some(lang) = manifest.pointer("/runtime/language").and_then(|l| l.as_str()) {
        annotations.insert("org.nasiko.framework".into(), lang.into());
    }
    if let Some(tags) = manifest.pointer("/metadata/tags").and_then(|t| t.as_array()) {
        let tags_str: Vec<&str> = tags.iter().filter_map(|t| t.as_str()).collect();
        if !tags_str.is_empty() {
            annotations.insert("org.nasiko.tags".into(), tags_str.join(",").into());
        }
    }

    serde_json::Value::Object(annotations)
}
