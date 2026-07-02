use serde_json::json;

pub fn definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "github_repo_info",
                "description": "Get info about a GitHub repository including stars, forks, open issues, language, and description.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": {
                            "type": "string",
                            "description": "Repository owner (user or organization)"
                        },
                        "repo": {
                            "type": "string",
                            "description": "Repository name"
                        }
                    },
                    "required": ["owner", "repo"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "github_actions_runs",
                "description": "List recent CI/CD workflow runs for a GitHub repository. Shows workflow name, status, conclusion, branch, and trigger event.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": {
                            "type": "string",
                            "description": "Repository owner (user or organization)"
                        },
                        "repo": {
                            "type": "string",
                            "description": "Repository name"
                        }
                    },
                    "required": ["owner", "repo"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "docker_hub_search",
                "description": "Search Docker Hub for container images. Returns image name, description, star count, pull count, and whether it's an official image.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query for Docker Hub images"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "check_endpoint",
                "description": "Check if an HTTP endpoint is responding and measure its latency. Reports status code, response time, content-type, and health status.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to check (must include scheme, e.g. https://example.com)"
                        }
                    },
                    "required": ["url"]
                }
            }
        }),
    ]
}

pub async fn execute(name: &str, arguments: &str) -> String {
    let result = match name {
        "github_repo_info" => github_repo_info(arguments).await,
        "github_actions_runs" => github_actions_runs(arguments).await,
        "docker_hub_search" => docker_hub_search(arguments).await,
        "check_endpoint" => check_endpoint(arguments).await,
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

async fn github_repo_info(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let owner = args["owner"].as_str().ok_or("missing 'owner'")?;
    let repo = args["repo"].as_str().ok_or("missing 'repo'")?;

    let url = format!("https://api.github.com/repos/{}/{}", urlencode(owner), urlencode(repo));

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "nasiko-devops-agent/1.0")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    if let Some(message) = resp["message"].as_str() {
        if message == "Not Found" {
            return Ok(format!("Repository '{owner}/{repo}' not found."));
        }
    }

    let name = resp["full_name"].as_str().unwrap_or("unknown");
    let description = resp["description"].as_str().unwrap_or("No description");
    let stars = resp["stargazers_count"].as_u64().unwrap_or(0);
    let forks = resp["forks_count"].as_u64().unwrap_or(0);
    let open_issues = resp["open_issues_count"].as_u64().unwrap_or(0);
    let language = resp["language"].as_str().unwrap_or("Not specified");
    let pushed_at = resp["pushed_at"].as_str().unwrap_or("unknown");
    let license = resp["license"]["spdx_id"].as_str().unwrap_or("None");

    Ok(format!(
        "**{name}**\n\
         Description: {description}\n\
         Language: {language}\n\
         Stars: {stars}\n\
         Forks: {forks}\n\
         Open Issues: {open_issues}\n\
         License: {license}\n\
         Last Push: {pushed_at}"
    ))
}

async fn github_actions_runs(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let owner = args["owner"].as_str().ok_or("missing 'owner'")?;
    let repo = args["repo"].as_str().ok_or("missing 'repo'")?;

    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/runs?per_page=5",
        urlencode(owner),
        urlencode(repo),
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "nasiko-devops-agent/1.0")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    if let Some(message) = resp["message"].as_str() {
        if message == "Not Found" {
            return Ok(format!("Repository '{owner}/{repo}' not found or Actions not enabled."));
        }
    }

    let runs = resp["workflow_runs"]
        .as_array()
        .ok_or("no workflow_runs field in response")?;

    if runs.is_empty() {
        return Ok(format!("No workflow runs found for '{owner}/{repo}'."));
    }

    let mut results = Vec::new();
    for run in runs {
        let workflow_name = run["name"].as_str().unwrap_or("unnamed");
        let status = run["status"].as_str().unwrap_or("unknown");
        let conclusion = run["conclusion"].as_str().unwrap_or("pending");
        let created_at = run["created_at"].as_str().unwrap_or("unknown");
        let head_branch = run["head_branch"].as_str().unwrap_or("unknown");
        let event = run["event"].as_str().unwrap_or("unknown");

        results.push(format!(
            "- **{workflow_name}** [{status}/{conclusion}]\n  Branch: {head_branch} | Event: {event} | Created: {created_at}"
        ));
    }

    Ok(format!(
        "Recent workflow runs for {owner}/{repo}:\n\n{}",
        results.join("\n")
    ))
}

async fn docker_hub_search(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;

    let url = format!(
        "https://hub.docker.com/v2/search/repositories/?query={}&page_size=5",
        urlencode(query),
    );

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let results_arr = resp["results"]
        .as_array()
        .ok_or("no results field in response")?;

    if results_arr.is_empty() {
        return Ok(format!("No Docker Hub images found for '{query}'."));
    }

    let mut results = Vec::new();
    for image in results_arr {
        let repo_name = image["repo_name"].as_str().unwrap_or("unknown");
        let description = image["short_description"].as_str().unwrap_or("No description");
        let star_count = image["star_count"].as_u64().unwrap_or(0);
        let pull_count = image["pull_count"].as_u64().unwrap_or(0);
        let is_official = image["is_official"].as_bool().unwrap_or(false);

        let official_badge = if is_official { " [OFFICIAL]" } else { "" };

        results.push(format!(
            "- **{repo_name}**{official_badge}\n  {description}\n  Stars: {star_count} | Pulls: {pull_count}"
        ));
    }

    Ok(format!(
        "Docker Hub results for '{query}':\n\n{}",
        results.join("\n")
    ))
}

async fn check_endpoint(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let url = args["url"].as_str().ok_or("missing 'url'")?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build client: {e}"))?;

    let start = std::time::Instant::now();
    let resp = client
        .get(url)
        .header("User-Agent", "nasiko-devops-agent/1.0")
        .send()
        .await;
    let elapsed_ms = start.elapsed().as_millis();

    match resp {
        Ok(response) => {
            let status = response.status();
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("not specified")
                .to_string();

            let healthy = status.is_success();
            let health_status = if healthy { "HEALTHY" } else { "UNHEALTHY" };

            Ok(format!(
                "Endpoint: {url}\n\
                 Status: {status} [{health_status}]\n\
                 Response Time: {elapsed_ms}ms\n\
                 Content-Type: {content_type}"
            ))
        }
        Err(e) => {
            if e.is_timeout() {
                Ok(format!(
                    "Endpoint: {url}\n\
                     Status: TIMEOUT [UNHEALTHY]\n\
                     Response Time: >10000ms\n\
                     Error: Request timed out after 10 seconds"
                ))
            } else {
                Ok(format!(
                    "Endpoint: {url}\n\
                     Status: CONNECTION FAILED [UNHEALTHY]\n\
                     Response Time: {elapsed_ms}ms\n\
                     Error: {e}"
                ))
            }
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                String::from(b as char)
            }
            b' ' => "+".into(),
            _ => format!("%{:02X}", b),
        })
        .collect()
}
