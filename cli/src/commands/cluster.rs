use std::io::IsTerminal;

use anyhow::{Result, bail};
use nasiko_utils::term;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::Client;
use crate::commands::local_agents;
use crate::config;

/// Resolves `connect`'s TARGET, `--url` and `--name` into the URL to register and the name
/// given for it (`None` derives one). Accepts three forms:
/// `connect <url> [--name <name>]`, `connect <name> --url <url>` and `connect --url <url>`.
pub fn control_plane_to_register(
    target: Option<String>,
    url: Option<String>,
    name: Option<String>,
) -> Result<(String, Option<String>)> {
    // Only an explicit http(s) URL may stand in for `--url`: a bare word is a name.
    let is_url = |value: &str| value.starts_with("https://") || value.starts_with("http://");
    let (url, name) = match (target, url, name) {
        (Some(target), Some(_), None) if is_url(&target) => bail!(
            "pass the URL once: nasiko connect {target} [--name <name>], or nasiko connect <name> --url {target}"
        ),
        (Some(name), Some(url), None) | (None, Some(url), Some(name)) => (url, Some(name)),
        (None, Some(url), None) => (url, None),
        (Some(url), None, name) if is_url(&url) => (url, name),
        (Some(_), Some(_), Some(_)) => {
            bail!("pass the name once: `nasiko connect <name> --url <url>`")
        }
        (Some(name), None, _) => bail!(
            "missing --url: nasiko connect {name} --url <url>, or nasiko connect https://<host> --name {name}"
        ),
        (None, None, _) => {
            bail!("usage: nasiko connect <name> --url <url>, or nasiko connect https://<host>")
        }
    };
    if let Some(name) = name
        .as_deref()
        .filter(|name| local_agents::is_coding_agent(name))
    {
        bail!("'{name}' is reserved for the {name} coding agent; choose another name");
    }
    Ok((url, name))
}

/// Register an existing control plane by URL, then log in to it when `login` is set, a
/// terminal is there to ask on, and no live session is already saved for it.
pub fn connect(url: &str, name: Option<&str>, login: bool) -> Result<()> {
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
    if login {
        log_in_after_connect();
    }
    crate::commands::integration::auto_install_if_authenticated();
    Ok(())
}

/// A failed or impossible login never fails `connect`: the control plane is registered either
/// way, and `nasiko auth login` signs in later.
fn log_in_after_connect() {
    let has_session = config::active_cluster()
        .ok()
        .and_then(|(_, entry)| entry.token)
        .is_some_and(|token| !matches!(config::token_expired(&token), Some(true)));
    if has_session {
        return;
    }
    if !std::io::stdin().is_terminal() {
        term::print_warning("Login skipped: no terminal to ask on. Run `nasiko auth login`.");
        return;
    }
    println!();
    if let Err(e) = crate::commands::auth::login_without_setup() {
        term::print_warning(&format!("Login skipped: {e}. Run `nasiko auth login`."));
    }
}

/// List configured clusters.
pub fn list() -> Result<()> {
    // Resolving the active cluster first saves the hosted default on a fresh install, so the
    // list shows what every other command would talk to.
    let _ = config::active_cluster();
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
    use super::control_plane_to_register;

    fn resolve(
        target: Option<&str>,
        url: Option<&str>,
        name: Option<&str>,
    ) -> anyhow::Result<(String, Option<String>)> {
        control_plane_to_register(
            target.map(String::from),
            url.map(String::from),
            name.map(String::from),
        )
    }

    #[test]
    fn accepts_every_nasiko_ee_form() {
        let url = "https://cp.example.com";
        let named = (url.to_string(), Some("prod".to_string()));
        assert_eq!(resolve(Some(url), None, Some("prod")).unwrap(), named);
        assert_eq!(resolve(Some("prod"), Some(url), None).unwrap(), named);
        assert_eq!(resolve(None, Some(url), Some("prod")).unwrap(), named);
        assert_eq!(
            resolve(Some(url), None, None).unwrap(),
            (url.to_string(), None)
        );
        assert_eq!(
            resolve(None, Some(url), None).unwrap(),
            (url.to_string(), None)
        );
    }

    #[test]
    fn refuses_ambiguous_or_incomplete_forms() {
        let url = "https://cp.example.com";
        assert!(resolve(Some(url), Some(url), None).is_err());
        assert!(resolve(Some("prod"), Some(url), Some("prod")).is_err());
        assert!(resolve(Some("prod"), None, None).is_err());
        assert!(resolve(None, None, Some("prod")).is_err());
        assert!(resolve(Some("claude"), Some(url), None).is_err());
    }
}
