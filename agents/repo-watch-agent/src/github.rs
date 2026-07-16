//! GitHub REST client — everything about talking to `api.github.com`.
//!
//! Exposes one `pub(crate)` async function per operation the agent needs, each returning a
//! ready-to-show text block (or a `String` error). All requests are HTTP GET: this client
//! only reads, so the agent can never modify a repository regardless of token scope. The LLM
//! tool surface that wraps these lives in [`crate::tools`].

const GITHUB_API: &str = "https://api.github.com";

/// GitHub's max page size for the commits endpoint. Also the "there may be more" signal: a
/// full page means fetch another, a shorter page is the last one.
const COMMITS_PER_PAGE: usize = 100;

/// Upper bound on pages `fetch_commits` will walk, so an enormous window can't turn one tool
/// call into unbounded requests. `MAX_COMMIT_PAGES * COMMITS_PER_PAGE` commits max.
const MAX_COMMIT_PAGES: u32 = 10;

/// Cap on total diff-patch bytes returned per `compare_diff`, so a huge window can't blow the
/// LLM's context. Full file stats are always included regardless.
const PATCH_BUDGET: usize = 12_000;

/// Cap on a single file's content returned by `read_file`, so one huge file can't blow the
/// model's context. Truncated content is flagged in the returned text.
const FILE_READ_BUDGET: usize = 60_000;

/// How many results to request from the search endpoints (PRs and code references).
const SEARCH_RESULT_LIMIT: usize = 50;

/// Length of an abbreviated commit hash, matching git's conventional short form.
const SHORT_HASH_LEN: usize = 7;

// ─── Public operations (one per tool; read these to know what this module offers) ───

/// Abbreviated commit hash. Shared with the agent's status previews.
pub(crate) fn short_hash(commit_hash: &str) -> &str {
    &commit_hash[..SHORT_HASH_LEN.min(commit_hash.len())]
}

/// Commits since `since` on the default branch of each repo, one section per repo. A per-repo
/// failure is rendered inline rather than aborting the others.
pub(crate) async fn list_commits(repos: &[String], since: &str) -> String {
    per_repo(repos, |owner, repo| list_commits_for(owner, repo, since)).await
}

/// The aggregated file-level diff across every commit since `since`, one section per repo.
pub(crate) async fn compare_diff(repos: &[String], since: &str) -> String {
    per_repo(repos, |owner, repo| compare_diff_for(owner, repo, since)).await
}

/// Pull requests with activity since `since`, one section per repo.
pub(crate) async fn search_prs(repos: &[String], since: &str) -> String {
    per_repo(repos, |owner, repo| search_prs_for(owner, repo, since)).await
}

/// One commit's metadata + changed-file list (+ parent hash). Omit `commit_hash` for the
/// latest commit on the default branch.
pub(crate) async fn get_commit(repo: &str, commit_hash: Option<&str>) -> Result<String, String> {
    let (owner, name) = split_repo(repo)?;
    get_commit_for(owner, name, commit_hash).await
}

/// The full contents of one file at a git ref (commit hash, branch, or tag).
pub(crate) async fn read_file(repo: &str, path: &str, git_ref: &str) -> Result<String, String> {
    let (owner, name) = split_repo(repo)?;
    read_file_for(owner, name, path, git_ref).await
}

/// Files that reference a symbol, via GitHub code search (textual, default-branch only).
pub(crate) async fn find_references(repo: &str, symbol: &str) -> Result<String, String> {
    let (owner, name) = split_repo(repo)?;
    find_references_for(owner, name, symbol).await
}

// ─── Single-repo bodies (the algorithm behind each operation above) ──────────

async fn list_commits_for(owner: &str, repo: &str, since: &str) -> Result<String, String> {
    let commits = fetch_commits(owner, repo, since).await?;
    if commits.is_empty() {
        return Ok(format!("No commits to {owner}/{repo} since {since}."));
    }

    let lines: Vec<String> = commits
        .iter()
        .map(|commit| {
            let hash = commit["sha"].as_str().unwrap_or("");
            let subject = commit["commit"]["message"]
                .as_str()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("");
            let author = commit["commit"]["author"]["name"]
                .as_str()
                .unwrap_or("unknown");
            let date = commit["commit"]["author"]["date"].as_str().unwrap_or("");
            let url = commit["html_url"].as_str().unwrap_or("");
            format!("- {} {subject} ({author}, {date}) {url}", short_hash(hash))
        })
        .collect();

    Ok(format!(
        "{} commit(s) to {owner}/{repo} since {since}:\n{}",
        commits.len(),
        lines.join("\n")
    ))
}

async fn compare_diff_for(owner: &str, repo: &str, since: &str) -> Result<String, String> {
    let commits = fetch_commits(owner, repo, since).await?;
    if commits.is_empty() {
        return Ok(format!("No commits since {since}, nothing to diff."));
    }

    // GitHub returns commits newest-first.
    let head_hash = commits
        .first()
        .and_then(|c| c["sha"].as_str())
        .ok_or("missing head hash")?
        .to_string();
    let earliest_hash = commits
        .last()
        .and_then(|c| c["sha"].as_str())
        .ok_or("missing earliest hash")?
        .to_string();

    // Diff from the parent of the earliest commit so that commit's own changes are included
    // (compare is exclusive of `base`). The repo's first commit has no parent, so fall back
    // to the commit itself (which yields an empty diff there).
    let base_hash = fetch_parent_hash(owner, repo, &earliest_hash)
        .await?
        .unwrap_or_else(|| earliest_hash.clone());

    let response = authorized(client().get(format!(
        "{GITHUB_API}/repos/{owner}/{repo}/compare/{base_hash}...{head_hash}"
    )))
    .send()
    .await
    .map_err(|e| format!("request failed: {e}"))?;
    let response = ensure_success(response, "GitHub compare API").await?;

    let comparison: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;
    let mut files = comparison["files"].as_array().cloned().unwrap_or_default();
    if files.is_empty() {
        return Ok(format!(
            "{} commit(s) since {since}, but no file changes reported.",
            commits.len()
        ));
    }

    // Most-changed first, so the patch budget below is spent on the biggest changes.
    files.sort_by_key(|file| std::cmp::Reverse(file_churn(file)));

    let stats = files
        .iter()
        .map(format_file_stat)
        .collect::<Vec<_>>()
        .join("\n");
    let (patches, omitted) = render_patches(&files);

    let mut out = format!(
        "{} file(s) changed since {since} (compare {}...{}, {} commit(s)):\n{stats}",
        files.len(),
        short_hash(&base_hash),
        short_hash(&head_hash),
        commits.len(),
    );
    if !patches.is_empty() {
        out.push_str("\n\nPatches (largest changes first, may be truncated):\n\n");
        out.push_str(&patches);
    }
    if omitted > 0 {
        out.push_str(&format!(
            "\n\n({omitted} more file(s) changed, patch not shown due to size budget)"
        ));
    }
    Ok(out)
}

async fn search_prs_for(owner: &str, repo: &str, since: &str) -> Result<String, String> {
    let query = format!("repo:{owner}/{repo} is:pr updated:>={since}");
    let per_page = SEARCH_RESULT_LIMIT.to_string();
    let response = authorized(client().get(format!("{GITHUB_API}/search/issues")))
        .query(&[
            ("q", query.as_str()),
            ("sort", "updated"),
            ("order", "desc"),
            ("per_page", &per_page),
        ])
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let response = ensure_success(response, "GitHub PR search API").await?;

    let results: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;
    let prs = results["items"].as_array().cloned().unwrap_or_default();
    if prs.is_empty() {
        return Ok(format!("No PR activity on {owner}/{repo} since {since}."));
    }

    let lines = prs
        .iter()
        .map(format_pr_activity)
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "{} PR(s) with activity since {since}:\n{lines}",
        prs.len()
    ))
}

async fn get_commit_for(
    owner: &str,
    repo: &str,
    commit_hash: Option<&str>,
) -> Result<String, String> {
    let commit_hash = resolve_commit_hash(owner, repo, commit_hash).await?;

    let response = authorized(client().get(format!(
        "{GITHUB_API}/repos/{owner}/{repo}/commits/{commit_hash}"
    )))
    .send()
    .await
    .map_err(|e| format!("request failed: {e}"))?;
    let response = ensure_success(response, "GitHub commit API").await?;
    let commit: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let full_hash = commit["sha"].as_str().unwrap_or(&commit_hash);
    let parent_hash = commit["parents"][0]["sha"].as_str().unwrap_or("");
    let subject = commit["commit"]["message"]
        .as_str()
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("");
    let author = commit["commit"]["author"]["name"]
        .as_str()
        .unwrap_or("unknown");
    let date = commit["commit"]["author"]["date"].as_str().unwrap_or("");

    let mut files = commit["files"].as_array().cloned().unwrap_or_default();
    files.sort_by_key(|file| std::cmp::Reverse(file_churn(file)));
    let file_list = files
        .iter()
        .map(format_file_stat)
        .collect::<Vec<_>>()
        .join("\n");

    let parent_line = if parent_hash.is_empty() {
        "parent_hash: (none — this is the repo's first commit)".to_string()
    } else {
        format!(
            "parent_hash: {parent_hash}  (use as `git_ref` in read_file for the BEFORE version)"
        )
    };

    Ok(format!(
        "Commit {full_hash}\nmessage: {subject}\nauthor: {author}\ndate: {date}\n{parent_line}\n\n\
         {} file(s) changed (most-changed first):\n{file_list}\n\n\
         To read a file's exact before/after, call read_file with git_ref={full_hash} (after) and \
         git_ref={parent_hash} (before).",
        files.len(),
    ))
}

async fn read_file_for(
    owner: &str,
    repo: &str,
    path: &str,
    git_ref: &str,
) -> Result<String, String> {
    let response =
        authorized(client().get(format!("{GITHUB_API}/repos/{owner}/{repo}/contents/{path}")))
            // Raw media type returns the file bytes directly (no base64 envelope).
            .header("Accept", "application/vnd.github.raw")
            .query(&[("ref", git_ref)])
            .send()
            .await
            .map_err(|e| format!("request failed: {e}"))?;

    // A 404 is expected, not an error: an added file has no content at its parent ref, and a
    // deleted file has none at the commit ref.
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(format!(
            "'{path}' does not exist at {git_ref} (added or deleted at this ref — no content)."
        ));
    }
    let response = ensure_success(response, "GitHub contents API").await?;

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("read failed: {e}"))?;
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Ok(format!(
            "'{path}' at {git_ref} is binary/non-UTF-8 ({} bytes) — not shown.",
            bytes.len()
        ));
    };

    if text.len() <= FILE_READ_BUDGET {
        return Ok(format!("{path} @ {git_ref}:\n```\n{text}\n```"));
    }
    let shown = safe_truncate(text, FILE_READ_BUDGET);
    Ok(format!(
        "{path} @ {git_ref}:\n```\n{shown}\n```\n\n(truncated — file is {} bytes)",
        text.len()
    ))
}

async fn find_references_for(owner: &str, repo: &str, symbol: &str) -> Result<String, String> {
    let query = format!("{symbol} repo:{owner}/{repo}");
    let per_page = SEARCH_RESULT_LIMIT.to_string();
    let response = authorized(client().get(format!("{GITHUB_API}/search/code")))
        .query(&[("q", query.as_str()), ("per_page", &per_page)])
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    // Code search has a stricter (~30/min) secondary rate limit surfaced as 403 — call it out
    // distinctly rather than as the generic token hint from `ensure_success`.
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "GitHub code search 403 (rate limit or auth): {body}"
        ));
    }
    let response = ensure_success(response, "GitHub code search API").await?;

    let results: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;
    let matches = results["items"].as_array().cloned().unwrap_or_default();
    if matches.is_empty() {
        return Ok(format!(
            "No files in {owner}/{repo} reference `{symbol}` (per code search)."
        ));
    }

    let files: Vec<String> = matches
        .iter()
        .filter_map(|item| item["path"].as_str().map(|path| format!("- {path}")))
        .collect();
    Ok(format!(
        "{} file(s) reference `{symbol}` in {owner}/{repo} (textual match on the default branch — \
         verify each is a real dependency, not a name collision):\n{}",
        files.len(),
        files.join("\n"),
    ))
}

/// Runs `body` against every requested repo and stitches the per-repo results (or, for a bad
/// reference or a failed call, an inline error) into one report — a repo-level failure never
/// aborts the others.
async fn per_repo<'a, F, Fut>(repos: &'a [String], body: F) -> String
where
    F: Fn(&'a str, &'a str) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let mut sections = Vec::with_capacity(repos.len());
    for repo_ref in repos {
        let section = match split_repo(repo_ref) {
            Ok((owner, name)) => body(owner, name)
                .await
                .unwrap_or_else(|e| format!("Error: {e}")),
            Err(e) => format!("Error: {e}"),
        };
        sections.push(format!("### {repo_ref}\n{section}"));
    }
    sections.join("\n\n")
}

// ─── Fetch + render (mid-level GitHub calls the bodies above are built from) ─

/// Fetches every commit on the default branch since `since`, paginating so a busy window
/// isn't silently truncated at one page. Bounded by `MAX_COMMIT_PAGES`; if the bound is hit
/// it warns rather than dropping history silently.
async fn fetch_commits(
    owner: &str,
    repo: &str,
    since: &str,
) -> Result<Vec<serde_json::Value>, String> {
    let per_page = COMMITS_PER_PAGE.to_string();
    let mut all = Vec::new();
    let mut page = 1u32;
    loop {
        let page_number = page.to_string();
        let response =
            authorized(client().get(format!("{GITHUB_API}/repos/{owner}/{repo}/commits")))
                .query(&[
                    ("since", since),
                    ("per_page", &per_page),
                    ("page", &page_number),
                ])
                .send()
                .await
                .map_err(|e| format!("request failed: {e}"))?;
        let response = ensure_success(response, "GitHub commits API").await?;

        let page_commits: Vec<serde_json::Value> = response
            .json()
            .await
            .map_err(|e| format!("parse failed: {e}"))?;
        let is_last_page = page_commits.len() < COMMITS_PER_PAGE;
        all.extend(page_commits);

        if is_last_page {
            break;
        }
        page += 1;
        if page > MAX_COMMIT_PAGES {
            tracing::warn!(
                "commit history for {owner}/{repo} since {since} exceeds {} commits; \
                 diff base and commit list truncated to that many",
                MAX_COMMIT_PAGES as usize * COMMITS_PER_PAGE
            );
            break;
        }
    }
    Ok(all)
}

/// The parent hash of a commit, or `None` for a repo's first commit (which has no parent).
async fn fetch_parent_hash(
    owner: &str,
    repo: &str,
    commit_hash: &str,
) -> Result<Option<String>, String> {
    let response = authorized(client().get(format!(
        "{GITHUB_API}/repos/{owner}/{repo}/commits/{commit_hash}"
    )))
    .send()
    .await
    .map_err(|e| format!("request failed: {e}"))?;
    let response = ensure_success(response, "GitHub commit API").await?;
    let commit: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;
    Ok(commit["parents"][0]["sha"].as_str().map(str::to_string))
}

/// Resolves an optional commit hash to a concrete one: the given hash when present and
/// non-blank, else the newest commit on the repo's default branch.
async fn resolve_commit_hash(
    owner: &str,
    repo: &str,
    commit_hash: Option<&str>,
) -> Result<String, String> {
    if let Some(hash) = commit_hash
        && !hash.trim().is_empty()
    {
        return Ok(hash.to_string());
    }
    let response = authorized(client().get(format!("{GITHUB_API}/repos/{owner}/{repo}/commits")))
        .query(&[("per_page", "1")])
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let response = ensure_success(response, "GitHub commits API").await?;
    let commits: Vec<serde_json::Value> = response
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;
    commits
        .first()
        .and_then(|commit| commit["sha"].as_str())
        .map(str::to_string)
        .ok_or_else(|| "repo has no commits".to_string())
}

/// Renders per-file diff patches up to `PATCH_BUDGET` total bytes (largest-first, assuming
/// `files` is pre-sorted by churn), returning the rendered patches plus the count of files
/// whose patch was dropped for budget.
fn render_patches(files: &[serde_json::Value]) -> (String, usize) {
    let mut remaining_budget = PATCH_BUDGET;
    let mut patches = Vec::new();
    let mut omitted = 0usize;
    for file in files {
        let Some(patch) = file["patch"].as_str() else {
            continue;
        };
        if remaining_budget == 0 {
            omitted += 1;
            continue;
        }
        let name = file["filename"].as_str().unwrap_or("");
        let shown = safe_truncate(patch, remaining_budget);
        remaining_budget -= shown.len();
        patches.push(format!("### {name}\n```diff\n{shown}\n```"));
    }
    (patches.join("\n\n"), omitted)
}

/// One line describing a PR's state and lifecycle timestamps, leaving the opened/merged/
/// closed classification (relative to the query window) to the model.
fn format_pr_activity(pr: &serde_json::Value) -> String {
    let number = pr["number"].as_u64().unwrap_or(0);
    let title = pr["title"].as_str().unwrap_or("");
    let state = pr["state"].as_str().unwrap_or("");
    let created = pr["created_at"].as_str().unwrap_or("");
    let url = pr["html_url"].as_str().unwrap_or("");

    let mut lifecycle = vec![format!("state={state}"), format!("created={created}")];
    if let Some(merged) = pr["pull_request"]["merged_at"].as_str() {
        lifecycle.push(format!("merged={merged}"));
    } else if let Some(closed) = pr["closed_at"].as_str() {
        lifecycle.push(format!("closed={closed} (not merged)"));
    }
    format!("- #{number} {title} [{}] {url}", lifecycle.join(", "))
}

// ─── Small formatting helpers ─────────────────────────────────────────────────

/// Total lines touched by a changed file — the sort key for "most significant first".
fn file_churn(file: &serde_json::Value) -> u64 {
    file["additions"].as_u64().unwrap_or(0) + file["deletions"].as_u64().unwrap_or(0)
}

/// One `- path (status, +adds/-dels)` line for a changed file.
fn format_file_stat(file: &serde_json::Value) -> String {
    let name = file["filename"].as_str().unwrap_or("");
    let status = file["status"].as_str().unwrap_or("");
    let additions = file["additions"].as_u64().unwrap_or(0);
    let deletions = file["deletions"].as_u64().unwrap_or(0);
    format!("- {name} ({status}, +{additions}/-{deletions})")
}

/// Truncate to at most `max_bytes`, backing off to the nearest UTF-8 char boundary so a
/// multi-byte character straddling the cut point can't cause a slice panic.
fn safe_truncate(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

// ─── HTTP plumbing (lowest level; every call above is built on these) ────────

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("nasiko-repo-watch-agent")
        .build()
        .expect("failed to build reqwest client")
}

/// Attaches the GitHub-required headers plus a bearer token when `GITHUB_TOKEN` is set.
/// Public repos work without a token (at a much lower rate limit); private repos need one.
fn authorized(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    let request = request
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    match std::env::var("GITHUB_TOKEN") {
        Ok(token) if !token.is_empty() => request.bearer_auth(token),
        _ => request,
    }
}

/// Returns the response when the status is 2xx, otherwise an error carrying the endpoint
/// name, status, and body. Auth-ish failures (401/403/404) append a token hint, since a
/// private repo reached without a readable `GITHUB_TOKEN` is the most common cause.
async fn ensure_success(
    response: reqwest::Response,
    endpoint: &str,
) -> Result<reqwest::Response, String> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let hint = match status.as_u16() {
        401 | 403 | 404 => " (a private repo needs a GITHUB_TOKEN with read access)",
        _ => "",
    };
    let body = response.text().await.unwrap_or_default();
    Err(format!("{endpoint} {status}: {body}{hint}"))
}

/// Splits an "owner/name" repo reference into its two halves.
fn split_repo(repo_ref: &str) -> Result<(&str, &str), String> {
    repo_ref
        .split_once('/')
        .filter(|(owner, name)| !owner.is_empty() && !name.is_empty())
        .ok_or_else(|| format!("'{repo_ref}' is not a valid \"owner/name\" repo reference"))
}
