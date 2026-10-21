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
            config::refuse_derived_name_clash(&derived, url, existing.as_deref())?;
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
