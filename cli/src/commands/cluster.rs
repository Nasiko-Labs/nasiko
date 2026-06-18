use anyhow::Result;

use crate::api::Client;
use crate::config;

/// Register an existing control plane by URL.
pub fn connect(url: &str, name: Option<&str>) -> Result<()> {
    eprint!("Checking {url}... ");
    Client::health_check(url)?;
    eprintln!("ok");

    let cluster_name = name.map(|n| n.to_string()).unwrap_or_else(|| {
        url.replace("https://", "")
            .replace("http://", "")
            .split('.')
            .next()
            .unwrap_or("cluster")
            .to_string()
    });

    config::connect(&cluster_name, url)?;
    println!("Connected: {cluster_name} ({url})");
    Ok(())
}

/// List configured clusters.
pub fn list() -> Result<()> {
    let cfg = config::load()?;
    if cfg.clusters.is_empty() {
        println!("No clusters configured. Run `nasiko connect <url>`.");
        return Ok(());
    }
    println!("{:<8} {:<16} {}", "ACTIVE", "NAME", "URL");
    for (name, entry) in &cfg.clusters {
        let marker = if cfg.active.as_deref() == Some(name.as_str()) {
            "*"
        } else {
            ""
        };
        println!("{:<8} {:<16} {}", marker, name, entry.url);
    }
    Ok(())
}

/// Switch active cluster.
pub fn use_cluster(name: &str) -> Result<()> {
    config::use_cluster(name)?;
    println!("Switched to: {name}");
    Ok(())
}
