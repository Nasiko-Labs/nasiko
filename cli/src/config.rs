use anyhow::{Context, Result, bail};
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
    let name = config.active.ok_or_else(|| {
        // One plain line: callers wrap this error into their own messages and some print it to
        // stdout, so it must carry no line breaks or color codes. No `up` hint either: the error
        // is shared with `nasiko-ee`, which has no `up`.
        anyhow::anyhow!(
            "no active cluster; run: {} connect <url>",
            crate::commands::local_agents::cli_name()
        )
    })?;
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

/// Decode the `exp` claim (unix seconds) from a JWT without verifying its
/// signature. Returns None for tokens that aren't parseable JWTs — callers
/// must treat that as "unknown", not "valid".
pub fn token_expiry(token: &str) -> Option<i64> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("exp")?.as_i64()
}

/// The JWT's `sub` claim — the caller's own user id. Decoded locally (same
/// approach as `token_expiry`), so callers needing "is this mine?" (e.g.
/// `agents ps` splitting owned vs shared) don't need an extra round-trip.
pub fn token_subject(token: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("sub")?.as_str().map(str::to_string)
}

/// Whether a stored JWT is already expired. None when the token's expiry
/// can't be determined locally (not a JWT, or no `exp` claim).
pub fn token_expired(token: &str) -> Option<bool> {
    let exp = token_expiry(token)?;
    Some(exp <= chrono::Utc::now().timestamp())
}

/// The name `connect <url>` gives a control plane when none is passed: the URL's host up to
/// its first dot (`cp` for `https://cp.example.com`). Public so a CLI built on this one names
/// a control plane the same way.
pub fn cluster_name_from_url(url: &str) -> String {
    url.replace("https://", "")
        .replace("http://", "")
        .split('.')
        .next()
        .unwrap_or("cluster")
        .to_string()
}

/// Refuses a name derived from a URL when it is already saved for another control plane
/// (`cp.prod.example.com` and `cp.staging.example.com` both derive `cp`), so `connect <url>`
/// never silently replaces one. `existing` is what is saved under the name, as a URL or a
/// bare host; reconnecting to the same control plane is fine.
pub fn refuse_derived_name_clash(derived: &str, url: &str, existing: Option<&str>) -> Result<()> {
    match existing {
        Some(existing) if control_plane_key(existing) != control_plane_key(url) => bail!(
            "'{derived}' is already saved for {existing}; choose a name for {url}: {} connect {url} --name <name>",
            crate::commands::local_agents::cli_name()
        ),
        _ => Ok(()),
    }
}

/// A control-plane address compared the way a saved login is (exactly, but for a trailing
/// slash), except that `https://`, the default scheme, may be left out — as some CLIs built on
/// this one store it. `http://` is kept: it is a different address.
fn control_plane_key(address: &str) -> &str {
    let address = address.trim();
    address
        .strip_prefix("https://")
        .unwrap_or(address)
        .trim_end_matches('/')
}

/// Whether two user ids name the same user. Ids are UUIDs, written as the token has them or in
/// canonical form, so they are compared as UUIDs; anything else is compared as written.
pub fn same_principal(left: &str, right: &str) -> bool {
    match (uuid::Uuid::parse_str(left), uuid::Uuid::parse_str(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Add or update a cluster and set it as active.
///
/// Preserves the existing username/token when reconnecting to the same name
/// *and* URL — otherwise a re-run of `nasiko connect` against an
/// already-authenticated cluster would silently log the user out. A new URL
/// under an existing name clears credentials, since a token minted for one
/// cluster is meaningless against another.
pub fn connect(name: &str, url: &str) -> Result<()> {
    let mut config = load()?;
    let url = url.trim_end_matches('/').to_string();
    let (username, token) = match config.clusters.get(name) {
        Some(existing) if existing.url == url => {
            (existing.username.clone(), existing.token.clone())
        }
        _ => (None, None),
    };
    config.clusters.insert(
        name.to_string(),
        ClusterEntry {
            url,
            username,
            token,
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
        anyhow::bail!(
            "cluster '{}' not found. Available: {:?}",
            name,
            config.clusters.keys().collect::<Vec<_>>()
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_derived_name_never_replaces_another_control_plane() {
        let url = "https://cp.prod.example.com";
        assert!(refuse_derived_name_clash("cp", url, None).is_ok());
        // The same control plane, with or without the default scheme, is a reconnect.
        for same in ["https://cp.prod.example.com/", "cp.prod.example.com"] {
            assert!(
                refuse_derived_name_clash("cp", url, Some(same)).is_ok(),
                "{same}"
            );
        }
        // Anything a saved login would not match is another control plane.
        for other in [
            "https://cp.staging.example.com",
            "http://cp.prod.example.com",
            "HTTPS://CP.PROD.EXAMPLE.COM",
        ] {
            let error = refuse_derived_name_clash("cp", url, Some(other))
                .unwrap_err()
                .to_string();
            assert!(error.contains("--name"), "{other}: {error}");
        }
    }

    #[test]
    fn a_derived_name_is_the_hosts_first_label() {
        assert_eq!(cluster_name_from_url("https://cp.example.com"), "cp");
        assert_eq!(
            cluster_name_from_url("http://localhost:8080"),
            "localhost:8080"
        );
    }
}
