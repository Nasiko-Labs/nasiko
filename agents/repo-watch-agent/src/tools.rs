//! GitHub API tools for the repo-watch agent.
//!
//! Each tool is a plain async function annotated with `#[rig_tool]` — rig generates the
//! `Tool` impl, the JSON Schema (via `schemars`, derived from the function's own parameter
//! types), and the argument struct automatically from the function signature and its doc
//! comments. There is no hand-written schema and no manual `serde_json::from_str` parsing.
//!
//! READ-ONLY BY DESIGN: every tool here issues HTTP GET requests only (list commits,
//! compare diffs, search PRs). There is deliberately no tool that creates, edits, or
//! deletes anything, so the agent cannot modify a repository even when its GITHUB_TOKEN
//! carries write permissions. Do not add write operations here — this agent's contract is
//! observation only.

use rig::tool_macro as rig_tool;

const GITHUB_API: &str = "https://api.github.com";
/// Cap on total diff-patch bytes fed back to the model per `compare_diff` call, so a huge
/// window can't blow the LLM's context. Full file stats are always included regardless.
const PATCH_BUDGET: usize = 12_000;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct GitHubToolError(String);

/// Splits an "owner/name" repo reference into its two halves.
fn split_repo(repo_ref: &str) -> Result<(&str, &str), String> {
    repo_ref
        .split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty())
        .ok_or_else(|| format!("'{repo_ref}' is not a valid \"owner/name\" repo reference"))
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

async fn list_commits_body(owner: &str, repo: &str, since: &str) -> Result<String, String> {
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

async fn compare_diff_body(owner: &str, repo: &str, since: &str) -> Result<String, String> {
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

async fn search_prs_body(owner: &str, repo: &str, since: &str) -> Result<String, String> {
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

/// Runs `body` against every requested repo and stitches the per-repo results (or, for a
/// bad reference or a failed call, an inline error) into one combined report — a repo-level
/// failure never aborts the others.
async fn per_repo<'a, F, Fut>(repos: &'a [String], body: F) -> String
where
    F: Fn(&'a str, &'a str) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let mut sections = Vec::with_capacity(repos.len());
    for repo_ref in repos {
        let section = match split_repo(repo_ref) {
            Ok((owner, name)) => body(owner, name).await.unwrap_or_else(|e| format!("Error: {e}")),
            Err(e) => format!("Error: {e}"),
        };
        sections.push(format!("### {repo_ref}\n{section}"));
    }
    sections.join("\n\n")
}

/// Cap on a single file's content returned by `read_file`, so one huge file can't blow the
/// model's context. Truncated content is flagged in the returned text.
const FILE_READ_BUDGET: usize = 60_000;

async fn get_commit_body(owner: &str, repo: &str, sha: Option<&str>) -> Result<String, String> {
    // Resolve the target sha: an explicit one, else the newest commit on the default branch.
    let sha = match sha {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => {
            let commits = auth(client().get(format!("{GITHUB_API}/repos/{owner}/{repo}/commits")))
                .query(&[("per_page", "1")])
                .send()
                .await
                .map_err(|e| format!("request failed: {e}"))?;
            if !commits.status().is_success() {
                let status = commits.status();
                let body = commits.text().await.unwrap_or_default();
                return Err(format!("GitHub commits API {status}: {body}"));
            }
            let arr: Vec<serde_json::Value> =
                commits.json().await.map_err(|e| format!("parse failed: {e}"))?;
            arr.first()
                .and_then(|c| c["sha"].as_str())
                .ok_or("repo has no commits")?
                .to_string()
        }
    };

    let resp = auth(client().get(format!("{GITHUB_API}/repos/{owner}/{repo}/commits/{sha}")))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub commit API {status}: {body}"));
    }

    let commit: serde_json::Value = resp.json().await.map_err(|e| format!("parse failed: {e}"))?;
    let full_sha = commit["sha"].as_str().unwrap_or(&sha);
    let parent_sha = commit["parents"][0]["sha"].as_str().unwrap_or("");
    let message = commit["commit"]["message"].as_str().unwrap_or("").lines().next().unwrap_or("");
    let author = commit["commit"]["author"]["name"].as_str().unwrap_or("unknown");
    let date = commit["commit"]["author"]["date"].as_str().unwrap_or("");

    let mut files = commit["files"].as_array().cloned().unwrap_or_default();
    files.sort_by_key(|f| {
        let a = f["additions"].as_u64().unwrap_or(0);
        let d = f["deletions"].as_u64().unwrap_or(0);
        std::cmp::Reverse(a + d)
    });

    let file_lines: Vec<String> = files
        .iter()
        .map(|f| {
            let name = f["filename"].as_str().unwrap_or("");
            let status = f["status"].as_str().unwrap_or("");
            let a = f["additions"].as_u64().unwrap_or(0);
            let d = f["deletions"].as_u64().unwrap_or(0);
            format!("- {name} ({status}, +{a}/-{d})")
        })
        .collect();

    let parent_line = if parent_sha.is_empty() {
        "parent_sha: (none — this is the repo's first commit)".to_string()
    } else {
        format!("parent_sha: {parent_sha}  (use as `git_ref` in read_file for the BEFORE version)")
    };

    Ok(format!(
        "Commit {full_sha}\nmessage: {message}\nauthor: {author}\ndate: {date}\n{parent_line}\n\n\
         {} file(s) changed (most-changed first):\n{}\n\n\
         To read a file's exact before/after, call read_file with git_ref={full_sha} (after) and \
         git_ref={parent_sha} (before).",
        files.len(),
        file_lines.join("\n"),
    ))
}

async fn read_file_body(owner: &str, repo: &str, path: &str, git_ref: &str) -> Result<String, String> {
    let resp = auth(client().get(format!("{GITHUB_API}/repos/{owner}/{repo}/contents/{path}")))
        // Raw media type returns the file bytes directly (no base64 envelope).
        .header("Accept", "application/vnd.github.raw")
        .query(&[("ref", git_ref)])
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        // Normal for an added file at its parent ref, or a deleted file at the commit ref.
        return Ok(format!(
            "'{path}' does not exist at {git_ref} (added or deleted at this ref — no content)."
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub contents API {status}: {body}"));
    }

    let bytes = resp.bytes().await.map_err(|e| format!("read failed: {e}"))?;
    let text = match std::str::from_utf8(&bytes) {
        Ok(t) => t,
        Err(_) => return Ok(format!("'{path}' at {git_ref} is binary/non-UTF-8 ({} bytes) — not shown.", bytes.len())),
    };

    let (shown, note) = if text.len() > FILE_READ_BUDGET {
        (safe_truncate(text, FILE_READ_BUDGET), format!("\n\n(truncated — file is {} bytes)", text.len()))
    } else {
        (text, String::new())
    };

    Ok(format!("{path} @ {git_ref}:\n```\n{shown}\n```{note}"))
}

async fn find_references_body(owner: &str, repo: &str, symbol: &str) -> Result<String, String> {
    let query = format!("{symbol} repo:{owner}/{repo}");
    let resp = auth(client().get(format!("{GITHUB_API}/search/code")))
        .query(&[("q", query.as_str()), ("per_page", "30")])
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN {
        // Code search has a stricter (~30/min) secondary rate limit and needs auth.
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub code search {status} (rate limit or auth): {body}"));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub code search API {status}: {body}"));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| format!("parse failed: {e}"))?;
    let items = body["items"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return Ok(format!("No files in {owner}/{repo} reference `{symbol}` (per code search)."));
    }

    let files: Vec<String> = items
        .iter()
        .filter_map(|it| it["path"].as_str().map(|p| format!("- {p}")))
        .collect();

    Ok(format!(
        "{} file(s) reference `{symbol}` in {owner}/{repo} (textual match on the default branch — \
         verify each is a real dependency, not a name collision):\n{}",
        files.len(),
        files.join("\n"),
    ))
}

#[rig_tool(description = "Read one specific commit in a GitHub repo: its message, author, date, \
    parent_sha, and the list of changed files (status + line counts + per-file patch), \
    most-changed first. Omit `sha` to get the latest commit on the default branch. Use the \
    returned parent_sha with read_file to fetch the BEFORE version of a changed file.")]
pub async fn get_commit(
    /// A single repo in "owner/name" format, e.g. "Nasiko-Labs/nasiko-cloud-rs".
    repo: String,
    /// Commit SHA to inspect. Omit or leave empty for the latest commit on the default branch.
    sha: Option<String>,
) -> Result<String, GitHubToolError> {
    let (owner, name) = split_repo(&repo).map_err(GitHubToolError)?;
    get_commit_body(owner, name, sha.as_deref()).await.map_err(GitHubToolError)
}

#[rig_tool(description = "Read the FULL contents of one file in a GitHub repo at a specific git \
    ref (commit SHA, branch, or tag). Use this to read a changed file's exact before/after: pass \
    the commit SHA for the new version and the commit's parent_sha for the old version. Prefer \
    this over the truncated patch when you need real line-by-line context.")]
pub async fn read_file(
    /// A single repo in "owner/name" format.
    repo: String,
    /// File path within the repo, e.g. "oss/auth/src/lib.rs".
    path: String,
    /// Git ref to read at: a commit SHA (use parent_sha for the BEFORE version), branch, or tag.
    git_ref: String,
) -> Result<String, GitHubToolError> {
    let (owner, name) = split_repo(&repo).map_err(GitHubToolError)?;
    read_file_body(owner, name, &path, &git_ref).await.map_err(GitHubToolError)
}

#[rig_tool(description = "Find files in a GitHub repo that reference a symbol (function, type, \
    constant name). Use this to ground an impacted-files list: for a changed symbol, this returns \
    the files that mention it. Matches are TEXTUAL (default branch only) so verify each is a real \
    dependency rather than a name collision.")]
pub async fn find_references(
    /// A single repo in "owner/name" format.
    repo: String,
    /// The symbol to search for (e.g. a function or type name).
    symbol: String,
) -> Result<String, GitHubToolError> {
    let (owner, name) = split_repo(&repo).map_err(GitHubToolError)?;
    find_references_body(owner, name, &symbol).await.map_err(GitHubToolError)
}

#[rig_tool(description = "List commits since a given time to the default branch of one or \
    more GitHub repos. Returns sha, message, author, date, and url for each commit, newest \
    first, in one section per repo.")]
pub async fn list_commits(
    /// One or more repos in "owner/name" format, e.g. ["Nasiko-Labs/nasiko-cloud-rs"]. Pass
    /// several entries to cover multiple repos in a single call.
    repos: Vec<String>,
    /// ISO-8601 timestamp; only commits after this time are returned
    since: String,
) -> Result<String, GitHubToolError> {
    Ok(per_repo(&repos, |owner, repo| list_commits_body(owner, repo, &since)).await)
}

#[rig_tool(description = "Get the aggregated file-level diff (files changed, added/removed \
    lines, and patches) covering every commit since a given time, for one or more repos. Call \
    this after list_commits, and only for repos where it found at least one commit.")]
pub async fn compare_diff(
    /// One or more repos in "owner/name" format, matching what was passed to list_commits.
    repos: Vec<String>,
    /// Same ISO-8601 timestamp passed to list_commits
    since: String,
) -> Result<String, GitHubToolError> {
    Ok(per_repo(&repos, |owner, repo| compare_diff_body(owner, repo, &since)).await)
}

#[rig_tool(description = "Search for pull requests with activity (opened, merged, or closed) \
    since a given time, across one or more repos. Returns number, title, state, and the \
    created/merged/closed timestamps for each, so you can classify them yourself relative to \
    the window.")]
pub async fn search_prs(
    /// One or more repos in "owner/name" format.
    repos: Vec<String>,
    /// ISO-8601 timestamp
    since: String,
) -> Result<String, GitHubToolError> {
    Ok(per_repo(&repos, |owner, repo| search_prs_body(owner, repo, &since)).await)
}
