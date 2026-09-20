use serde_json::json;

pub fn definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "crates_io",
                "description": "Search crates.io for Rust crates. Returns name, version, description, and documentation links. Use when the user asks about Rust libraries.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Crate name or search query"
                        },
                        "sort": {
                            "type": "string",
                            "enum": ["downloads", "relevance", "recent-downloads"],
                            "description": "Sort order for results (default: downloads)"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "crate_info",
                "description": "Get detailed info about a specific Rust crate: version history, features, dependencies, and docs.rs link.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Exact crate name"
                        },
                        "version": {
                            "type": "string",
                            "description": "Specific version (optional, defaults to latest)"
                        }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "npm_search",
                "description": "Search npm for JavaScript/TypeScript packages. Returns name, version, description. Use for Node.js/frontend library questions.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Package name or search query"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "npm_package",
                "description": "Get detailed info about a specific npm package: versions, dependencies, repository, and README excerpt.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Exact package name (e.g. 'zod', '@tanstack/react-query')"
                        },
                        "version": {
                            "type": "string",
                            "description": "Specific version (optional, defaults to latest)"
                        }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "pypi_package",
                "description": "Get info about a Python package from PyPI: version, description, dependencies, and project links.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Package name (e.g. 'fastapi', 'pydantic')"
                        }
                    },
                    "required": ["name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "github_readme",
                "description": "Fetch the README of a GitHub repository. Use for understanding how to use a library when registry docs aren't enough.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "repo": {
                            "type": "string",
                            "description": "Repository in 'owner/repo' format (e.g. 'tokio-rs/tokio')"
                        }
                    },
                    "required": ["repo"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web for information. Use when you need to find documentation, blog posts, comparisons, or any information not available through package registries. Good for 'latest', 'best', 'comparison' queries.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
    ]
}

pub async fn execute(name: &str, arguments: &str) -> String {
    let result = match name {
        "crates_io" => crates_io_search(arguments).await,
        "crate_info" => crate_info(arguments).await,
        "npm_search" => npm_search(arguments).await,
        "npm_package" => npm_package(arguments).await,
        "pypi_package" => pypi_package(arguments).await,
        "github_readme" => github_readme(arguments).await,
        "web_search" => web_search(arguments).await,
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

async fn crates_io_search(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;
    let sort = args["sort"].as_str().unwrap_or("downloads");

    let url = format!(
        "https://crates.io/api/v1/crates?q={}&per_page=10&sort={}",
        urlencode(query),
        urlencode(sort),
    );

    let resp = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "nasiko-docs-agent/1.0")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let crates = resp["crates"].as_array().ok_or("no results")?;

    if crates.is_empty() {
        return Ok(format!("No crates found for '{query}'."));
    }

    let results: Vec<String> = crates
        .iter()
        .map(|c| {
            let name = c["id"].as_str().unwrap_or("?");
            let version = c["max_version"].as_str().unwrap_or("?");
            let desc = c["description"].as_str().unwrap_or("");
            let downloads = c["downloads"].as_u64().unwrap_or(0);
            format!(
                "**{name}** v{version} ({downloads} downloads)\n  {desc}\n  docs: https://docs.rs/{name}/{version}",
            )
        })
        .collect();

    Ok(results.join("\n\n"))
}

async fn crate_info(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let name = args["name"].as_str().ok_or("missing 'name'")?;
    let version = args["version"].as_str();

    let url = format!("https://crates.io/api/v1/crates/{name}");
    let resp = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "nasiko-docs-agent/1.0")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let krate = &resp["crate"];
    let latest = krate["max_version"].as_str().unwrap_or("?");
    let desc = krate["description"].as_str().unwrap_or("");
    let repo = krate["repository"].as_str().unwrap_or("");
    let downloads = krate["downloads"].as_u64().unwrap_or(0);

    let target_version = version.unwrap_or(latest);

    // Get version-specific info
    let versions = resp["versions"].as_array();
    let mut features_str = String::new();
    let mut deps_str = String::new();

    if let Some(versions) = versions {
        if let Some(v) = versions.iter().find(|v| v["num"].as_str() == Some(target_version)) {
            if let Some(features) = v["features"].as_object() {
                let feature_list: Vec<&String> = features.keys().take(15).collect();
                if !feature_list.is_empty() {
                    features_str = format!("\n\nFeatures: {}", feature_list.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", "));
                }
            }
        }
    }

    // Fetch dependencies
    let deps_url = format!("https://crates.io/api/v1/crates/{name}/{target_version}/dependencies");
    if let Ok(deps_resp) = reqwest::Client::new()
        .get(&deps_url)
        .header("User-Agent", "nasiko-docs-agent/1.0")
        .send()
        .await
    {
        if let Ok(deps_json) = deps_resp.json::<serde_json::Value>().await {
            if let Some(deps) = deps_json["dependencies"].as_array() {
                let normal_deps: Vec<String> = deps
                    .iter()
                    .filter(|d| d["kind"].as_str() == Some("normal"))
                    .take(10)
                    .map(|d| {
                        let n = d["crate_id"].as_str().unwrap_or("?");
                        let req = d["req"].as_str().unwrap_or("*");
                        let optional = d["optional"].as_bool().unwrap_or(false);
                        if optional {
                            format!("  {n} {req} (optional)")
                        } else {
                            format!("  {n} {req}")
                        }
                    })
                    .collect();
                if !normal_deps.is_empty() {
                    deps_str = format!("\n\nDependencies:\n{}", normal_deps.join("\n"));
                }
            }
        }
    }

    Ok(format!(
        "**{name}** v{target_version}\n{desc}\n\nLatest: {latest}\nDownloads: {downloads}\nRepository: {repo}\nDocs: https://docs.rs/{name}/{target_version}{features_str}{deps_str}"
    ))
}

async fn npm_search(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;

    let url = format!("https://registry.npmjs.org/-/v1/search?text={}&size=5", urlencode(query));

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let objects = resp["objects"].as_array().ok_or("no results")?;

    if objects.is_empty() {
        return Ok(format!("No npm packages found for '{query}'."));
    }

    let results: Vec<String> = objects
        .iter()
        .map(|o| {
            let pkg = &o["package"];
            let name = pkg["name"].as_str().unwrap_or("?");
            let version = pkg["version"].as_str().unwrap_or("?");
            let desc = pkg["description"].as_str().unwrap_or("");
            format!("**{name}** v{version}\n  {desc}\n  https://www.npmjs.com/package/{name}")
        })
        .collect();

    Ok(results.join("\n\n"))
}

async fn npm_package(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let name = args["name"].as_str().ok_or("missing 'name'")?;
    let version = args["version"].as_str();

    let url = if let Some(v) = version {
        format!("https://registry.npmjs.org/{name}/{v}")
    } else {
        format!("https://registry.npmjs.org/{name}/latest")
    };

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    if resp.get("error").is_some() {
        return Err(format!("Package not found: {}", resp["error"]));
    }

    let ver = resp["version"].as_str().unwrap_or("?");
    let desc = resp["description"].as_str().unwrap_or("");
    let repo_url = resp["repository"]["url"].as_str().unwrap_or("");
    let homepage = resp["homepage"].as_str().unwrap_or("");

    let deps: Vec<String> = resp["dependencies"]
        .as_object()
        .map(|d| {
            d.iter()
                .take(15)
                .map(|(k, v)| format!("  {k}: {}", v.as_str().unwrap_or("*")))
                .collect()
        })
        .unwrap_or_default();

    let readme = resp["readme"]
        .as_str()
        .map(|r| {
            if r.len() > 2000 {
                format!("{}...\n[truncated]", &r[..2000])
            } else {
                r.to_string()
            }
        })
        .unwrap_or_default();

    let mut output = format!(
        "**{name}** v{ver}\n{desc}\n\nHomepage: {homepage}\nRepository: {repo_url}"
    );

    if !deps.is_empty() {
        output.push_str(&format!("\n\nDependencies ({}):\n{}", deps.len(), deps.join("\n")));
    }

    if !readme.is_empty() {
        output.push_str(&format!("\n\n---\nREADME:\n{readme}"));
    }

    Ok(output)
}

async fn pypi_package(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let name = args["name"].as_str().ok_or("missing 'name'")?;

    let url = format!("https://pypi.org/pypi/{name}/json");

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let info = &resp["info"];
    let version = info["version"].as_str().unwrap_or("?");
    let summary = info["summary"].as_str().unwrap_or("");
    let homepage = info["home_page"].as_str().unwrap_or("");
    let project_url = info["project_url"].as_str().unwrap_or("");
    let requires_python = info["requires_python"].as_str().unwrap_or("any");
    let license = info["license"].as_str().unwrap_or("?");

    let requires_dist: Vec<&str> = info["requires_dist"]
        .as_array()
        .map(|deps| deps.iter().take(15).filter_map(|d| d.as_str()).collect())
        .unwrap_or_default();

    let description = info["description"]
        .as_str()
        .map(|d| {
            if d.len() > 2000 {
                format!("{}...\n[truncated]", &d[..2000])
            } else {
                d.to_string()
            }
        })
        .unwrap_or_default();

    let mut output = format!(
        "**{name}** v{version}\n{summary}\n\nPython: {requires_python}\nLicense: {license}\nHomepage: {homepage}\nPyPI: {project_url}"
    );

    if !requires_dist.is_empty() {
        output.push_str(&format!("\n\nDependencies:\n  {}", requires_dist.join("\n  ")));
    }

    if !description.is_empty() {
        output.push_str(&format!("\n\n---\nDescription:\n{description}"));
    }

    Ok(output)
}

async fn github_readme(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let repo = args["repo"].as_str().ok_or("missing 'repo'")?;

    let url = format!("https://raw.githubusercontent.com/{repo}/HEAD/README.md");

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("README not found for '{repo}' (HTTP {})", resp.status()));
    }

    let body = resp.text().await.map_err(|e| format!("read failed: {e}"))?;

    if body.len() > 4000 {
        Ok(format!("{}...\n\n[truncated, full README at https://github.com/{repo}]", &body[..4000]))
    } else {
        Ok(body)
    }
}

async fn web_search(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let query = args["query"].as_str().ok_or("missing 'query'")?;

    let url = format!("https://html.duckduckgo.com/html/?q={}", urlencode(query));

    let resp = reqwest::Client::new()
        .get(&url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("search failed (HTTP {})", resp.status()));
    }

    let body = resp.text().await.map_err(|e| format!("read failed: {e}"))?;

    let mut results: Vec<String> = Vec::new();

    // DuckDuckGo HTML results have <a class="result__a" href="URL">TITLE</a>
    // and <a class="result__snippet">SNIPPET</a>
    let link_parts: Vec<&str> = body.split("class=\"result__a\"").collect();
    let snippet_parts: Vec<&str> = body.split("class=\"result__snippet\"").collect();

    for (i, link_chunk) in link_parts.iter().enumerate().skip(1) {
        if results.len() >= 5 {
            break;
        }

        // After split on class="result__a", chunk starts with ` href="URL">TITLE</a>`
        let href = extract_between(link_chunk, "href=\"", "\"").unwrap_or_default();

        // Title is between first > and </a>
        let title = extract_between(link_chunk, ">", "</a>")
            .unwrap_or_default()
            .replace("<b>", "")
            .replace("</b>", "");

        // Get snippet from the corresponding snippet part
        let snippet = if i < snippet_parts.len() {
            extract_between(snippet_parts[i], ">", "</a>")
                .unwrap_or_default()
                .replace("<b>", "")
                .replace("</b>", "")
                .trim()
                .to_string()
        } else {
            String::new()
        };

        if !title.is_empty() && !href.is_empty() {
            let mut entry = format!("**{}**\n  {}", title.trim(), href);
            if !snippet.is_empty() {
                entry.push_str(&format!("\n  {snippet}"));
            }
            results.push(entry);
        }
    }

    if results.is_empty() {
        return Ok(format!("No web results found for '{query}'."));
    }

    Ok(results.join("\n\n"))
}

fn extract_between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start_idx = s.find(start)? + start.len();
    let rest = &s[start_idx..];
    let end_idx = rest.find(end)?;
    Some(&rest[..end_idx])
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
