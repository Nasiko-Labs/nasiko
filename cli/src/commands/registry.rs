use anyhow::{Result, bail};

use crate::api::RegistryClient;
use crate::config;

pub fn connect(url: &str) -> Result<()> {
    let url = url.trim_end_matches('/');

    eprint!("Checking {url}... ");
    let http = ureq::Agent::new_with_defaults();
    let resp = http.get(&format!("{url}/v1/search?limit=1")).call();
    match resp {
        Ok(r) if r.status().as_u16() < 400 => eprintln!("ok"),
        Ok(r) => bail!("registry returned HTTP {}", r.status().as_u16()),
        Err(e) => bail!("cannot reach registry: {e}"),
    }

    config::set_registry_url(url)?;
    println!("Registry connected: {url}");
    Ok(())
}

pub fn disconnect() -> Result<()> {
    config::clear_registry_url()?;
    println!("Registry disconnected.");
    Ok(())
}

pub fn status() -> Result<()> {
    match config::artifact_registry_url() {
        Some(url) => println!("Registry: {url}"),
        None => println!("No registry connected. Run: nasiko registry connect <url>"),
    }
    Ok(())
}

pub fn search(
    query: Option<&str>,
    artifact_type: Option<&str>,
    framework: Option<&str>,
    top: u32,
    min_score: Option<f32>,
    json: bool,
) -> Result<()> {
    let client = RegistryClient::new()
        .ok_or_else(|| anyhow::anyhow!("no registry connected — run `nasiko registry connect <url>`"))?;

    let results = client.search_opts(query, artifact_type, framework, top, min_score)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }

    if results.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    // Semantic results carry a relevance score; browse/keyword results don't.
    let scored = results.iter().any(|a| a.score.is_some());

    // Highlight the top match when ranking semantically.
    if scored && query.is_some()
        && let Some(best) = results.first() {
            let pct = best.score.map(|s| (s * 100.0).round() as i32).unwrap_or(0);
            println!(
                "Top match: {}/{}:{}  ({pct}% relevant)\n",
                best.owner, best.name, best.version
            );
        }

    let name_width = results.iter()
        .map(|a| format!("{}/{}:{}", a.owner, a.name, a.version).len())
        .max()
        .unwrap_or(30)
        .max(10);

    if scored {
        println!("{:>5}  {:<width$} {:<50} TAGS", "SCORE", "ARTIFACT", "DESCRIPTION", width = name_width);
    } else {
        println!("{:<width$} {:<60} TAGS", "ARTIFACT", "DESCRIPTION", width = name_width);
    }

    for artifact in &results {
        let ref_str = format!("{}/{}:{}", artifact.owner, artifact.name, artifact.version);
        let desc = artifact.description.as_deref().unwrap_or("—");
        let tags = if artifact.tags.is_empty() { "—".to_string() } else { artifact.tags.join(", ") };
        if scored {
            let score = artifact.score.map(|s| format!("{:.0}%", s * 100.0)).unwrap_or_else(|| "—".to_string());
            let desc_truncated = if desc.len() > 47 { format!("{}...", &desc[..47]) } else { desc.to_string() };
            println!("{score:>5}  {ref_str:<name_width$} {desc_truncated:<50} {tags}");
        } else {
            let desc_truncated = if desc.len() > 57 { format!("{}...", &desc[..57]) } else { desc.to_string() };
            println!("{ref_str:<name_width$} {desc_truncated:<60} {tags}");
        }
    }
    println!("\n{} result(s)", results.len());
    Ok(())
}

pub fn list(artifact_type: Option<&str>, json: bool) -> Result<()> {
    search(None, artifact_type, None, 100, None, json)
}
