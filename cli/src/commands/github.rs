use anyhow::Result;
use dialoguer::{Select, theme::ColorfulTheme};
use nasiko_utils::display::{opt_dash, yes_no};
use serde::Deserialize;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::Client;

#[derive(Debug, Deserialize)]
struct GithubStatus {
    #[serde(default)]
    status: String,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Debug, Deserialize, Tabled)]
struct GithubRepo {
    #[tabled(rename = "REPOSITORY", display("repo_name", &self.full_name))]
    name: String,
    #[tabled(skip)]
    #[serde(default)]
    full_name: Option<String>,
    #[tabled(rename = "PRIVATE", display = "yes_no")]
    #[serde(default)]
    private: bool,
    #[tabled(rename = "DESCRIPTION", display = "opt_dash")]
    #[serde(default)]
    description: Option<String>,
}

fn repo_name(name: &String, full_name: &Option<String>) -> String {
    full_name.as_deref().unwrap_or(name).to_string()
}

pub fn status() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let s: GithubStatus = client.get_json("/auth/github/token")?;
    match s.status.as_str() {
        "connected" => println!(
            "GitHub connected (username: {})",
            s.username.as_deref().unwrap_or("unknown")
        ),
        "invalid" => {
            println!("GitHub token stored but invalid — reconnect with `nasiko github connect`.")
        }
        _ => {
            println!("GitHub is not connected.");
            println!("Run `nasiko github connect` to authenticate.");
        }
    }
    Ok(())
}

pub fn repos() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let Some(raw): Option<serde_json::Value> =
        client.get_json_optional_on_forbidden("/github/repositories")?
    else {
        println!("No repositories found. Run `nasiko github connect` first.");
        return Ok(());
    };
    let repos: Vec<GithubRepo> = if let Some(arr) = raw.as_array() {
        serde_json::from_value(serde_json::Value::Array(arr.clone()))?
    } else if let Some(repos) = raw.get("repositories") {
        serde_json::from_value(repos.clone())?
    } else if let Some(data) = raw.get("data") {
        serde_json::from_value(data.clone())?
    } else {
        serde_json::from_value(raw)?
    };

    if repos.is_empty() {
        println!("No repositories found. Run `nasiko github connect` first.");
        return Ok(());
    }

    println!(
        "{}",
        Table::new(&repos)
            .with(Style::blank())
            .with(Alignment::left())
    );
    println!("\n{} repo(s).", repos.len());
    Ok(())
}

pub fn connect() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let resp: serde_json::Value = client.get_json("/github/login")?;
    let url = resp
        .get("auth_url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("no auth_url in response"))?;
    println!("Open this URL in your browser to connect GitHub:\n\n  {url}\n");
    Ok(())
}

pub fn disconnect() -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.delete("/github/logout")?;
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
            .map(|r| {
                r.full_name
                    .as_deref()
                    .unwrap_or(r.name.as_str())
                    .to_string()
            })
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
        &serde_json::json!({ "repository_full_name": chosen, "branch": branch }),
    )?;
    let data = result.get("data").cloned().unwrap_or(result.clone());

    let success = data
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if success {
        let agent_name = data
            .get("agent_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let status = data
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("uploaded");
        println!(
            "Agent '{}' cloned and deployed (status: {}).",
            agent_name, status
        );
    } else {
        let msg = data
            .get("status")
            .or_else(|| result.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("Clone failed: {}", msg);
    }
    Ok(())
}
