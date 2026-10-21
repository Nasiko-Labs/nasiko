use anyhow::Result;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::Client;
use crate::config;

/// Register an existing control plane by URL.
pub fn connect(url: &str, name: Option<&str>) -> Result<()> {
    let cluster_name = match name {
        Some(name) => name.to_string(),
        None => {
            let derived = config::cluster_name_from_url(url);
            let existing = config::load()?
                .clusters
                .get(&derived)
                .map(|c| c.url.clone());
            refuse_name_clash(&derived, url, existing.as_deref())?;
            derived
        }
    };

    eprint!("Checking {url}... ");
    Client::health_check(url)?;
    eprintln!("ok");

    config::connect(&cluster_name, url)?;
    println!("Connected: {cluster_name} ({url})");
    crate::commands::integration::auto_install_if_authenticated();
    Ok(())
}

/// A name derived from the URL must not silently replace another control plane saved under it
/// (`cp.prod.example.com` and `cp.staging.example.com` both derive `cp`). Reconnecting to the
/// same URL is fine; anything else needs `--name`.
fn refuse_name_clash(derived: &str, url: &str, existing_url: Option<&str>) -> Result<()> {
    match existing_url {
        Some(existing) if existing.trim_end_matches('/') != url.trim_end_matches('/') => {
            anyhow::bail!(
                "a cluster named '{derived}' already points at {existing}; choose a name for {url}: \
                 nasiko connect {url} --name <name>"
            )
        }
        _ => Ok(()),
    }
}

/// List configured clusters.
pub fn list() -> Result<()> {
    let cfg = config::load()?;
    if cfg.clusters.is_empty() {
        println!("No clusters configured. Run `nasiko connect <url>`.");
        return Ok(());
    }
    let rows: Vec<ClusterTableRow> = cfg
        .clusters
        .iter()
        .map(|(name, entry)| {
            let marker = if cfg.active.as_deref() == Some(name.as_str()) {
                "*"
            } else {
                ""
            };
            ClusterTableRow {
                active: marker.to_string(),
                name: name.clone(),
                url: entry.url.clone(),
            }
        })
        .collect();
    println!(
        "{}",
        Table::new(rows)
            .with(Style::blank())
            .with(Alignment::left())
    );
    Ok(())
}

#[derive(Tabled)]
struct ClusterTableRow {
    #[tabled(rename = "ACTIVE")]
    active: String,
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(rename = "URL")]
    url: String,
}

/// Switch active cluster.
pub fn use_cluster(name: &str) -> Result<()> {
    config::use_cluster(name)?;
    println!("Switched to: {name}");
    crate::commands::integration::auto_install_if_authenticated();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_derived_name_never_replaces_another_control_plane() {
        assert!(refuse_name_clash("cp", "https://cp.prod.example.com", None).is_ok());
        assert!(
            refuse_name_clash(
                "cp",
                "https://cp.prod.example.com/",
                Some("https://cp.prod.example.com")
            )
            .is_ok()
        );
        let error = refuse_name_clash(
            "cp",
            "https://cp.staging.example.com",
            Some("https://cp.prod.example.com"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("--name"), "{error}");
    }
}
