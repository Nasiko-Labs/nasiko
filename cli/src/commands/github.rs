use anyhow::Result;
use dialoguer::{Select, theme::ColorfulTheme};
use nasiko_utils::display::{opt_dash, yes_no};
use serde::Deserialize;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::Client;

/// Response body of `POST /github/clone` — mirrors the server's `CloneResult`
/// (`oss/server/src/github.rs`). `success: true` only means the build was
/// queued, not that it finished — real completion comes from polling
/// `upload_id` (which doubles as the build id) via SSE.
#[derive(Debug, Deserialize)]
struct CloneQueued {
    success: bool,
    message: String,
    agent_name: Option<String>,
    upload_id: Option<String>,
}

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

fn repo_name(name: &str, full_name: &Option<String>) -> String {
    full_name.as_deref().unwrap_or(name).to_string()
}

/// Pull the repo array out of a `/github/repositories` body. The server answers
/// `{"repositories": [...], "total": N}` with no `ApiResponse` envelope
/// (`server/src/github.rs`). `data` and a bare array are legacy tolerance
/// carried over unchanged from the two ladders this replaces; no server in this
/// repo emits either, so neither is a contract.
fn parse_repo_list(raw: serde_json::Value) -> Result<Vec<GithubRepo>> {
    let repos = match raw {
        serde_json::Value::Object(mut body) => {
            let list = body.remove("repositories").or_else(|| body.remove("data"));
            list.unwrap_or(serde_json::Value::Object(body))
        }
        bare_array => bare_array,
    };
    Ok(serde_json::from_value(repos)?)
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
    let repos = parse_repo_list(raw)?;

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
        let repos = parse_repo_list(raw)?;

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

    let result: CloneQueued = client.post_json(
        "/github/clone",
        &serde_json::json!({ "repository_full_name": chosen, "branch": branch }),
    )?;

    if !result.success {
        anyhow::bail!("Clone failed: {}", result.message);
    }

    let agent_name = result.agent_name.as_deref().unwrap_or("unknown");
    println!("{}", result.message);
    if let Some(build_id) = &result.upload_id {
        println!("build_id: {}", build_id);
        println!("Waiting for server to build and deploy... (this may take a few minutes)");
        client.poll_build_status(build_id)?;
    }
    println!("\nDeployed: {}", agent_name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_repo_list;
    use serde_json::json;

    #[test]
    fn parses_the_shape_the_server_sends() {
        let raw = json!({
            "repositories": [
                {"name": "nasiko", "full_name": "Nasiko-Labs/nasiko", "private": false},
                {"name": "scratch", "full_name": "acme/scratch", "private": true}
            ],
            "total": 2
        });

        let repos = parse_repo_list(raw).unwrap();

        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].full_name.as_deref(), Some("Nasiko-Labs/nasiko"));
        assert!(repos[1].private);
    }
}
