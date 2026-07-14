//! GitHub API tools for the repo-watch agent.
//!
//! READ-ONLY BY DESIGN: every tool here issues HTTP GET requests only (list commits,
//! compare diffs, search PRs). There is deliberately no tool that creates, edits, or
//! deletes anything, so the agent cannot modify a repository even when its GITHUB_TOKEN
//! carries write permissions. Do not add write operations here — this agent's contract is
//! observation only.

use serde_json::json;

const GITHUB_API: &str = "https://api.github.com";
/// Cap on total diff-patch bytes fed back to the model per `compare_diff` call, so a huge
/// window can't blow the LLM's context. Full file stats are always included regardless.
const PATCH_BUDGET: usize = 12_000;

pub fn definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "list_commits",
                "description": "List commits to the repo's default branch since a given time. Returns sha, message, author, date, and url for each commit, newest first.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": {"type": "string", "description": "Repo owner/org, e.g. 'Nasiko-Labs'"},
                        "repo": {"type": "string", "description": "Repo name, e.g. 'nasiko-cloud-rs'"},
                        "since": {"type": "string", "description": "ISO-8601 timestamp; only commits after this time are returned"}
                    },
                    "required": ["owner", "repo", "since"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "compare_diff",
                "description": "Get the aggregated file-level diff (files changed, added/removed lines, and patches) covering every commit since a given time. Call this after list_commits, and only when there is at least one commit in the window.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": {"type": "string"},
                        "repo": {"type": "string"},
                        "since": {"type": "string", "description": "Same ISO-8601 timestamp passed to list_commits"}
                    },
                    "required": ["owner", "repo", "since"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "search_prs",
                "description": "Search for pull requests with activity (opened, merged, or closed) since a given time. Returns number, title, state, and the created/merged/closed timestamps for each, so you can classify them yourself relative to the window.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": {"type": "string"},
                        "repo": {"type": "string"},
                        "since": {"type": "string", "description": "ISO-8601 timestamp"}
                    },
                    "required": ["owner", "repo", "since"]
                }
            }
        }),
    ]
}

pub async fn execute(name: &str, arguments: &str) -> String {
    let result = match name {
        "list_commits" => list_commits(arguments).await,
        "compare_diff" => compare_diff(arguments).await,
        "search_prs" => search_prs(arguments).await,
        _ => Err(format!("Unknown tool: {name}")),
    };

    match result {
        Ok(s) => s,
        Err(e) => format!("Error: {e}"),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("nasiko-repo-watch-agent")
        .build()
        .expect("failed to build reqwest client")
}

/// Attaches the GitHub-required headers plus a bearer token when `GITHUB_TOKEN` is set.
/// Public repos work without a token (at a much lower rate limit); private repos need one.
fn auth(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let req = req
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    match std::env::var("GITHUB_TOKEN") {
        Ok(token) if !token.is_empty() => req.bearer_auth(token),
        _ => req,
    }
}

async fn fetch_commits(owner: &str, repo: &str, since: &str) -> Result<Vec<serde_json::Value>, String> {
    // Paginate so a busy window isn't silently truncated at one page (100). This keeps
    // `compare_diff`'s earliest-commit (and thus its parent-based diff base) correct even
    // when the window holds more than 100 commits. Bounded to avoid a runaway on a huge
    // window; if the bound is hit we warn rather than silently drop history.
    const MAX_PAGES: u32 = 10;
    let mut all = Vec::new();
    let mut page = 1u32;
    loop {
        let page_str = page.to_string();
        let resp = auth(client().get(format!("{GITHUB_API}/repos/{owner}/{repo}/commits")))
            .query(&[("since", since), ("per_page", "100"), ("page", page_str.as_str())])
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "GitHub commits API {status}: {body} (private repos need a GITHUB_TOKEN with read access)"
            ));
        }

        let batch: Vec<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| format!("parse failed: {e}"))?;
        let n = batch.len();
        all.extend(batch);
        if n < 100 {
            break;
        }
        page += 1;
        if page > MAX_PAGES {
            tracing::warn!(
                "commit history for {owner}/{repo} since {since} exceeds {} commits; \
                 diff base and commit list truncated to that many",
                MAX_PAGES * 100
            );
            break;
        }
    }
    Ok(all)
}

async fn fetch_parent_sha(owner: &str, repo: &str, sha: &str) -> Result<Option<String>, String> {
    let resp = auth(client().get(format!("{GITHUB_API}/repos/{owner}/{repo}/commits/{sha}")))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub commit API {status}: {body}"));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| format!("parse failed: {e}"))?;
    Ok(body["parents"][0]["sha"].as_str().map(|s| s.to_string()))
}

async fn list_commits(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let owner = args["owner"].as_str().ok_or("missing 'owner'")?;
    let repo = args["repo"].as_str().ok_or("missing 'repo'")?;
    let since = args["since"].as_str().ok_or("missing 'since'")?;

    let commits = fetch_commits(owner, repo, since).await?;
    if commits.is_empty() {
        return Ok(format!("No commits to {owner}/{repo} since {since}."));
    }

    let lines: Vec<String> = commits
        .iter()
        .map(|c| {
            let sha = c["sha"].as_str().unwrap_or("");
            let short_sha = &sha[..7.min(sha.len())];
            let message = c["commit"]["message"]
                .as_str()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("");
            let author = c["commit"]["author"]["name"].as_str().unwrap_or("unknown");
            let date = c["commit"]["author"]["date"].as_str().unwrap_or("");
            let url = c["html_url"].as_str().unwrap_or("");
            format!("- {short_sha} {message} ({author}, {date}) {url}")
        })
        .collect();

    Ok(format!(
        "{} commit(s) to {owner}/{repo} since {since}:\n{}",
        commits.len(),
        lines.join("\n")
    ))
}

async fn compare_diff(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let owner = args["owner"].as_str().ok_or("missing 'owner'")?;
    let repo = args["repo"].as_str().ok_or("missing 'repo'")?;
    let since = args["since"].as_str().ok_or("missing 'since'")?;

    let commits = fetch_commits(owner, repo, since).await?;
    if commits.is_empty() {
        return Ok(format!("No commits since {since}, nothing to diff."));
    }

    // GitHub returns commits newest-first.
    let head_sha = commits
        .first()
        .and_then(|c| c["sha"].as_str())
        .ok_or("missing head sha")?
        .to_string();
    let earliest_sha = commits
        .last()
        .and_then(|c| c["sha"].as_str())
        .ok_or("missing earliest sha")?
        .to_string();

    // Diff from the parent of the earliest commit so that commit's own changes are
    // included (compare is exclusive of `base`). Falls back to the commit itself for
    // a repo's very first commit, which has no parent (yields an empty diff there).
    let base_sha = fetch_parent_sha(owner, repo, &earliest_sha)
        .await?
        .unwrap_or_else(|| earliest_sha.clone());

    let resp = auth(client().get(format!(
        "{GITHUB_API}/repos/{owner}/{repo}/compare/{base_sha}...{head_sha}"
    )))
    .send()
    .await
    .map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub compare API {status}: {body}"));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| format!("parse failed: {e}"))?;
    let mut files = body["files"].as_array().cloned().unwrap_or_default();

    if files.is_empty() {
        return Ok(format!(
            "{} commit(s) since {since}, but no file changes reported.",
            commits.len()
        ));
    }

    // Most-changed files first, so truncation below drops the least significant ones.
    files.sort_by_key(|f| {
        let additions = f["additions"].as_u64().unwrap_or(0);
        let deletions = f["deletions"].as_u64().unwrap_or(0);
        std::cmp::Reverse(additions + deletions)
    });

    let stats: Vec<String> = files
        .iter()
        .map(|f| {
            let name = f["filename"].as_str().unwrap_or("");
            let status = f["status"].as_str().unwrap_or("");
            let additions = f["additions"].as_u64().unwrap_or(0);
            let deletions = f["deletions"].as_u64().unwrap_or(0);
            format!("- {name} ({status}, +{additions}/-{deletions})")
        })
        .collect();

    let mut patch_budget = PATCH_BUDGET;
    let mut patches = Vec::new();
    let mut omitted = 0usize;
    for f in &files {
        let Some(patch) = f["patch"].as_str() else {
            continue;
        };
        let name = f["filename"].as_str().unwrap_or("");
        if patch_budget == 0 {
            omitted += 1;
            continue;
        }
        let slice = safe_truncate(patch, patch_budget);
        patch_budget -= slice.len();
        patches.push(format!("### {name}\n```diff\n{slice}\n```"));
    }

    let mut out = format!(
        "{} file(s) changed since {since} (compare {}...{}, {} commit(s)):\n{}",
        files.len(),
        short_sha(&base_sha),
        short_sha(&head_sha),
        commits.len(),
        stats.join("\n")
    );
    if !patches.is_empty() {
        out.push_str("\n\nPatches (largest changes first, may be truncated):\n\n");
        out.push_str(&patches.join("\n\n"));
    }
    if omitted > 0 {
        out.push_str(&format!(
            "\n\n({omitted} more file(s) changed, patch not shown due to size budget)"
        ));
    }

    Ok(out)
}

async fn search_prs(arguments: &str) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).map_err(|e| e.to_string())?;
    let owner = args["owner"].as_str().ok_or("missing 'owner'")?;
    let repo = args["repo"].as_str().ok_or("missing 'repo'")?;
    let since = args["since"].as_str().ok_or("missing 'since'")?;

    let query = format!("repo:{owner}/{repo} is:pr updated:>={since}");
    let resp = auth(client().get(format!("{GITHUB_API}/search/issues")))
        .query(&[
            ("q", query.as_str()),
            ("sort", "updated"),
            ("order", "desc"),
            ("per_page", "50"),
        ])
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub search API {status}: {body}"));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| format!("parse failed: {e}"))?;
    let items = body["items"].as_array().cloned().unwrap_or_default();

    if items.is_empty() {
        return Ok(format!("No PR activity on {owner}/{repo} since {since}."));
    }

    let lines: Vec<String> = items
        .iter()
        .map(|pr| {
            let number = pr["number"].as_u64().unwrap_or(0);
            let title = pr["title"].as_str().unwrap_or("");
            let state = pr["state"].as_str().unwrap_or("");
            let created = pr["created_at"].as_str().unwrap_or("");
            let closed = pr["closed_at"].as_str();
            let merged = pr["pull_request"]["merged_at"].as_str();
            let url = pr["html_url"].as_str().unwrap_or("");

            let mut bits = vec![format!("state={state}"), format!("created={created}")];
            if let Some(m) = merged {
                bits.push(format!("merged={m}"));
            } else if let Some(c) = closed {
                bits.push(format!("closed={c} (not merged)"));
            }
            format!("- #{number} {title} [{}] {url}", bits.join(", "))
        })
        .collect();

    Ok(format!(
        "{} PR(s) with activity since {since}:\n{}",
        items.len(),
        lines.join("\n")
    ))
}

fn short_sha(s: &str) -> &str {
    &s[..7.min(s.len())]
}

/// Truncate to at most `max_bytes`, backing off to the nearest UTF-8 char boundary so a
/// multi-byte character straddling the cut point can't cause a slice panic.
fn safe_truncate(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
