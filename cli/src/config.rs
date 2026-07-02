use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub clusters: std::collections::HashMap<String, ClusterEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterEntry {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".nasiko")
        .join("config.json")
}

pub fn load() -> Result<Config> {
    let path = config_path();
    if !path.exists() {
        return Ok(Config::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}

pub fn save(config: &Config) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        // Restrict the ~/.nasiko/ directory so other local users cannot list it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let content = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, &content)?;
    // Restrict to owner-only: the file contains a raw JWT token.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Get the active cluster entry.
pub fn active_cluster() -> Result<(String, ClusterEntry)> {
    let config = load()?;
    let name = config
        .active
        .ok_or_else(|| anyhow::anyhow!("no active cluster — run `nasiko connect <url>`"))?;
    let entry = config
        .clusters
        .get(&name)
        .ok_or_else(|| anyhow::anyhow!("cluster '{}' not found in config", name))?
        .clone();
    Ok((name, entry))
}

/// Get the active cluster's base URL.
pub fn active_url() -> Result<String> {
    let (_, entry) = active_cluster()?;
    Ok(entry.url)
}

/// Get the active cluster's token (if logged in).
pub fn active_token() -> Result<Option<String>> {
    let (_, entry) = active_cluster()?;
    Ok(entry.token)
}

/// Add or update a cluster and set it as active.
pub fn connect(name: &str, url: &str) -> Result<()> {
    let mut config = load()?;
    config.clusters.insert(
        name.to_string(),
        ClusterEntry {
            url: url.trim_end_matches('/').to_string(),
            username: None,
            token: None,
        },
    );
    config.active = Some(name.to_string());
    save(&config)
}

/// Save username + token for the active cluster after login.
pub fn save_login(username: &str, token: &str) -> Result<()> {
    let mut config = load()?;
    let name = config
        .active
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no active cluster"))?;
    if let Some(entry) = config.clusters.get_mut(&name) {
        entry.username = Some(username.to_string());
        entry.token = Some(token.to_string());
    }
    save(&config)
}


/// Clear the token for the active cluster.
pub fn clear_token() -> Result<()> {
    let mut config = load()?;
    let name = config
        .active
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no active cluster"))?;
    if let Some(entry) = config.clusters.get_mut(&name) {
        entry.token = None;
    }
    save(&config)
}

/// Switch active cluster.
pub fn use_cluster(name: &str) -> Result<()> {
    let mut config = load()?;
    if !config.clusters.contains_key(name) {
        anyhow::bail!("cluster '{}' not found. Available: {:?}", name, config.clusters.keys().collect::<Vec<_>>());
    }
    config.active = Some(name.to_string());
    save(&config)
}

/// Returns the artifact registry URL.
/// Priority: NASIKO_REGISTRY_URL env var > config.json registry_url field.
pub fn artifact_registry_url() -> Option<String> {
    std::env::var("NASIKO_REGISTRY_URL")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| load().ok().and_then(|c| c.registry_url))
}

/// Set the registry URL in config.
pub fn set_registry_url(url: &str) -> Result<()> {
    let mut config = load()?;
    config.registry_url = Some(url.trim_end_matches('/').to_string());
    save(&config)
}

/// Clear the registry URL from config.
pub fn clear_registry_url() -> Result<()> {
    let mut config = load()?;
    config.registry_url = None;
    save(&config)
}

