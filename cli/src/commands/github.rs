use anyhow::Result;
use dialoguer::{Select, theme::ColorfulTheme};
use serde::Deserialize;

use crate::api::Client;

#[derive(Debug, Deserialize)]
struct GithubStatus {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRepo {
    name: String,
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    private: bool,
}

pub fn status() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let s: GithubStatus = client.get_json("/auth/github/token")?;
    if s.success {
        println!("GitHub connected (username: {})", s.username.as_deref().unwrap_or("unknown"));
    } else {
        println!("GitHub is not connected.");
        println!("Run `nasiko github connect` to authenticate.");
    }
    Ok(())
}

pub fn repos() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let raw: serde_json::Value = client.get_json("/github/repositories")?;
    let repos: Vec<GithubRepo> = if let Some(arr) = raw.as_array() {
        serde_json::from_value(serde_json::Value::Array(arr.clone()))?
    } else if let Some(data) = raw.get("data") {
        serde_json::from_value(data.clone())?
    } else {
        serde_json::from_value(raw)?
    };

    if repos.is_empty() {
        println!("No repositories found. Run `nasiko github connect` first.");
        return Ok(());
    }

    println!("{:<40} {:<8} DESCRIPTION", "REPOSITORY", "PRIVATE");
    println!("{}", "-".repeat(90));
    for r in &repos {
        println!(
            "{:<40} {:<8} {}",
            r.full_name.as_deref().unwrap_or(&r.name),
            if r.private { "yes" } else { "no" },
            r.description.as_deref().unwrap_or("-"),
        );
    }
    println!("\n{} repo(s).", repos.len());
    Ok(())
}

pub fn connect() -> Result<()> {
    println!("GitHub OAuth connect is not yet implemented in the CLI.");
    println!();
    println!("This will be implemented once the backend device-flow contract is confirmed.");
    Ok(())
}

pub fn disconnect() -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.post_json::<serde_json::Value, _>("/auth/github/logout", &serde_json::json!({}))?;
    println!("GitHub disconnected.");
    Ok(())
}

pub fn clone(repo: Option<&str>, branch: Option<&str>) -> Result<()> {
    let client = Client::from_active_cluster()?;

    let chosen = if let Some(r) = repo {
        r.to_string()
    } else {
        let raw: serde_json::Value = client.get_json("/github/repositories")?;
        let repos: Vec<GithubRepo> = if let Some(arr) = raw.as_array() {
            serde_json::from_value(serde_json::Value::Array(arr.clone()))?
        } else if let Some(data) = raw.get("data") {
            serde_json::from_value(data.clone())?
        } else {
            serde_json::from_value(raw)?
        };

        if repos.is_empty() {
            println!("No GitHub repositories found. Run `nasiko github connect` first.");
            return Ok(());
        }
        let names: Vec<String> = repos
            .iter()
            .map(|r| r.full_name.as_deref().unwrap_or(r.name.as_str()).to_string())
            .collect();
        let idx = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Select a repository to clone and deploy")
            .items(&names)
            .default(0)
            .interact()?;
        names[idx].clone()
    };

    let branch = branch.unwrap_or("main");
    println!("Cloning '{}' (branch: {})...", chosen, branch);

    let result: serde_json::Value = client.post_json(
        "/github/clone",
        &serde_json::json!({ "repo": chosen, "branch": branch }),
    )?;
    let data = result.get("data").cloned().unwrap_or(result.clone());

    let success = data.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
    if success {
        let agent_name = data.get("agent_name").and_then(|v| v.as_str()).unwrap_or("unknown");
        let status = data.get("status").and_then(|v| v.as_str()).unwrap_or("uploaded");
        println!("Agent '{}' cloned and deployed (status: {}).", agent_name, status);
    } else {
        let msg = data.get("status")
            .or_else(|| result.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("Clone failed: {}", msg);
    }
    Ok(())
}
