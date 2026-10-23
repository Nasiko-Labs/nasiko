use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::Command;

use nasiko_utils::display::opt_dash;
use nasiko_utils::term;
use serde::Deserialize;
use tabled::settings::{Alignment, Style};
use tabled::{Table, Tabled};

use crate::api::{
    AgentRecord, AgentVersion, Client, DeletedAgent, UpdateQueued, UploadedAgent, unwrap_data,
};
use crate::commands::local_agents::agent_command;

#[derive(Debug, Deserialize)]
struct LogLine {
    timestamp: Option<String>,
    level: Option<String>,
    message: String,
    source: Option<String>,
}

// ─── Lifecycle ────────────────────────────────────────────────────────────────

/// How often `ps --watch` refreshes.
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

pub fn ps(json: bool, watch: bool) -> Result<()> {
    let client = Client::from_active_cluster()?;
    if json {
        let raw: serde_json::Value = client.get_json("/agents?limit=100")?;
        println!("{}", serde_json::to_string_pretty(&raw)?);
        return Ok(());
    }
    if watch {
        return watch_ps(&client);
    }
    print_agents(&client)
}

/// Redraws the `ps` table every [`WATCH_INTERVAL`] until Ctrl-C. Refuses non-terminal output,
/// where clearing the screen would just fill a file or pipe with escape codes.
fn watch_ps(client: &Client) -> Result<()> {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        bail!(
            "--watch needs an interactive terminal; use `{}` or `{}`",
            agent_command("ps"),
            agent_command("ps --json")
        );
    }
    let mut first = true;
    loop {
        print!("\x1b[2J\x1b[H"); // clear the screen, cursor to the top-left
        let now = chrono::Local::now().format("%H:%M:%S");
        println!(
            "{}\n",
            term::dim(&format!(
                "every {}s · {now} · Ctrl-C to stop",
                WATCH_INTERVAL.as_secs()
            ))
        );
        if let Err(error) = print_agents(client) {
            // A first failure is a setup problem, so stop; after that, a watch rides out a
            // blip (a dropped connection, a server restart) and tries again.
            if first {
                return Err(error);
            }
            println!(
                "{}",
                term::red(&format!("refresh failed: {error:#} — retrying"))
            );
        }
        first = false;
        std::thread::sleep(WATCH_INTERVAL);
    }
}

fn print_agents(client: &Client) -> Result<()> {
    let agents: Vec<AgentRecord> = client.get_json("/agents?limit=100")?;
    if agents.is_empty() {
        println!("No agents registered.");
        return Ok(());
    }
    let base = client.base_url().trim_end_matches('/').to_string();

    // Split "created by you" vs "shared with you" (mirrors `nasiko mcp
    // connector list`) — only when we actually know our own id; if the
    // token can't be decoded locally, fall back to one flat list rather
    // than mislabeling everything as "shared".
    match client.current_user_id() {
        Some(my_id) => {
            let (created, shared): (Vec<&AgentRecord>, Vec<&AgentRecord>) = agents
                .iter()
                .partition(|a| a.owner_id.as_deref() == Some(my_id.as_str()));
            if !created.is_empty() {
                println!("Created by you ({}):", created.len());
                print_ps_table(&created, &base);
            }
            if !shared.is_empty() {
                if !created.is_empty() {
                    println!();
                }
                println!("Shared with you ({}):", shared.len());
                print_ps_table(&shared, &base);
            }
        }
        None => print_ps_table(&agents.iter().collect::<Vec<_>>(), &base),
    }
    Ok(())
}

fn print_ps_table(agents: &[&AgentRecord], base: &str) {
    let rows: Vec<PsTableRow> = agents
        .iter()
        .map(|a| {
            let status = a.status.as_deref().unwrap_or("unknown");
            // transport_path comes from the agent's own card (persisted at deploy
            // time); the proxy route requires the agent UUID, not the name.
            let path = a.transport_path.as_deref().unwrap_or("/");
            let url = format!("{base}/api/agents/{}{path}", a.id);
            PsTableRow {
                id: a.id.clone(),
                name: a.name.clone(),
                status: colorize_status(status),
                version: a.version.as_deref().unwrap_or("-").to_string(),
                url,
            }
        })
        .collect();
    println!(
        "{}",
        Table::new(rows)
            .with(Style::blank())
            .with(Alignment::left())
    );
}

/// Green when healthy, yellow while on its way, red when broken; anything else (stopped,
/// registered, unknown) stays uncolored — it isn't an error.
fn colorize_status(status: &str) -> String {
    match status {
        "running" => term::green(status),
        "deploying" | "starting" | "pending" | "building" | "restarting" => term::yellow(status),
        "failed" | "error" | "crashed" => term::red(status),
        _ => status.to_string(),
    }
}

#[derive(Tabled)]
struct PsTableRow {
    #[tabled(rename = "AGENT ID")]
    id: String,
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(rename = "STATUS")]
    status: String,
    #[tabled(rename = "VERSION")]
    version: String,
    #[tabled(rename = "URL")]
    url: String,
}

/// `nasiko logs` filters. `level`/`search` mirror the server's own semantics (level compared
/// uppercased, search case-insensitive) so a line matches the same way whether the server or
/// the CLI did the filtering.
pub struct LogFilters {
    pub level: Option<String>,
    pub search: Option<String>,
    /// `30m`, `2h`, `1d`, or an ISO-8601 time (one-shot mode only)
    pub since: Option<String>,
}

impl LogFilters {
    fn is_empty(&self) -> bool {
        self.level.is_none() && self.search.is_none() && self.since.is_none()
    }

    fn matches(&self, line: &LogLine, since: Option<DateTime<Utc>>) -> bool {
        let level_ok = self.level.as_deref().is_none_or(|want| {
            line.level
                .as_deref()
                .is_some_and(|l| l.eq_ignore_ascii_case(want))
        });
        let search_ok = self
            .search
            .as_deref()
            .is_none_or(|want| line.message.to_lowercase().contains(&want.to_lowercase()));
        // Re-checked in case a server ignores `since`; a line without a readable timestamp is
        // kept rather than hidden. Raw container output can't be filtered by time at all: the
        // server stamps each of those lines with the moment it read them.
        let since_ok = since.is_none_or(|since| {
            line.timestamp
                .as_deref()
                .and_then(|t| t.parse::<DateTime<Utc>>().ok())
                .is_none_or(|t| t >= since)
        });
        level_ok && search_ok && since_ok
    }
}

/// Parses `--since`: a relative `<n>m`/`<n>h`/`<n>d` back from `now`, or an ISO-8601 time.
fn resolve_since(input: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    if let Ok(time) = input.parse::<DateTime<Utc>>() {
        return Ok(time);
    }
    let invalid = || {
        anyhow::anyhow!(
            "invalid --since '{input}' — use 30m, 2h, 1d, or an ISO-8601 time like 2026-10-22T09:00:00Z"
        )
    };
    let unit_at = input.char_indices().last().ok_or_else(invalid)?.0;
    let (amount, unit) = input.split_at(unit_at);
    let amount = i64::from(amount.parse::<u32>().map_err(|_| invalid())?);
    let span = match unit {
        "m" => chrono::Duration::minutes(amount),
        "h" => chrono::Duration::hours(amount),
        "d" => chrono::Duration::days(amount),
        _ => return Err(invalid()),
    };
    now.checked_sub_signed(span).ok_or_else(invalid)
}

/// Fetch or stream agent logs.
///
/// Without `--follow`: queries the structured logs endpoint for the last `tail` lines, with
/// `filters` passed to the server and re-applied locally.
/// With    `--follow`: opens an SSE stream and prints new lines until Ctrl-C, filtered locally
/// (the stream endpoint takes no filters).
pub fn logs(agent: &str, tail: u32, follow: bool, filters: &LogFilters) -> Result<()> {
    let client = Client::from_active_cluster()?;

    if follow {
        return stream_logs(agent, filters);
    }

    let since = filters
        .since
        .as_deref()
        .map(|s| resolve_since(s, Utc::now()))
        .transpose()?;
    let mut path = format!("/observability/agents/{agent}/logs?limit={tail}");
    if let Some(level) = &filters.level {
        path.push_str(&format!("&level={}", crate::api::urlencode(level)));
    }
    if let Some(search) = &filters.search {
        path.push_str(&format!("&search={}", crate::api::urlencode(search)));
    }
    if let Some(since) = since {
        let since = since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        path.push_str(&format!("&since={}", crate::api::urlencode(&since)));
    }

    // One-shot: fetch structured log lines from the observe endpoint.
    match client.get_json::<Vec<LogLine>>(&path) {
        Ok(lines) => {
            let lines: Vec<LogLine> = lines
                .into_iter()
                .filter(|l| filters.matches(l, since))
                .collect();
            print_log_lines(&lines);
            Ok(())
        }
        Err(_) => {
            // Fallback: raw container logs (older /containers API). The route
            // returns a JSON array of lines, not plain text.
            let lines: Vec<String> =
                client.get_json(&format!("/containers/{agent}/logs?tail={tail}"))?;
            if !filters.is_empty() {
                term::print_warning("this server can't filter logs — showing them unfiltered");
            }
            for line in &lines {
                println!("{line}");
            }
            Ok(())
        }
    }
}

/// Print a slice of `LogLine`s with aligned columns (chronological order).
fn print_log_lines(lines: &[LogLine]) {
    // API returns newest-first; reverse for chronological display.
    for l in lines.iter().rev() {
        println!("{}", format_log_line(l));
    }
}

/// One log line as `<time> <LEVEL> [<source>] <message>`. The time is cut to millisecond
/// precision (23 chars); a shorter timestamp — e.g. a whole-second `…T12:00:00Z`, which omits
/// the fraction — is shown as-is rather than dropped.
fn format_log_line(l: &LogLine) -> String {
    let ts = l.timestamp.as_deref().unwrap_or("");
    let ts = ts.get(..23).unwrap_or(ts);
    let lvl = l.level.as_deref().unwrap_or("INFO");
    let src = l.source.as_deref().unwrap_or("?");
    format!("{ts} {lvl:<5} [{src}] {}", l.message)
}

/// Open an SSE stream from `/api/observability/agents/{agent}/logs/stream` and print
/// each arriving log line to stdout. Blocks until interrupted or server closes.
fn stream_logs(agent: &str, filters: &LogFilters) -> Result<()> {
    let (_, entry) = crate::config::active_cluster()?;
    let url = format!(
        "{}/api/observability/agents/{}/logs/stream",
        entry.url, agent
    );

    let mut resp = ureq::Agent::new_with_defaults()
        .get(&url)
        .header(
            "Authorization",
            &format!("Bearer {}", entry.token.as_deref().unwrap_or("")),
        )
        .call()?;

    if resp.status().as_u16() >= 400 {
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        anyhow::bail!("HTTP {}: {}", resp.status(), body);
    }

    eprintln!("Following logs for '{}' (Ctrl-C to stop)…", agent);

    let reader = BufReader::new(resp.body_mut().as_reader());
    for raw in reader.lines() {
        let raw = raw?;
        if let Some(json) = raw.strip_prefix("data: ")
            && let Ok(l) = serde_json::from_str::<LogLine>(json)
            && filters.matches(&l, None)
        {
            println!("{}", format_log_line(&l));
        }
    }
    Ok(())
}

pub fn stop(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.post_void(&format!("/containers/{agent}/stop"))?;
    println!("Stopped: {agent}");
    Ok(())
}

pub fn start(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.post_void(&format!("/containers/{agent}/start"))?;
    println!("Started: {agent}");
    Ok(())
}

pub fn restart(agent: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.post_void(&format!("/containers/{agent}/restart"))?;
    println!("Restarted: {agent}");
    Ok(())
}

pub fn scale(agent: &str, replicas: u32) -> Result<()> {
    let client = Client::from_active_cluster()?;
    client.post_json_void(
        &format!("/containers/{agent}/scale"),
        &serde_json::json!({"replicas": replicas}),
    )?;
    println!("Scaled {agent} to {replicas} replica(s)");
    Ok(())
}

/// Terminate and deregister `agent` — a UUID, or a name resolved to one by exact match.
pub fn rm(agent: &str, force: bool) -> Result<()> {
    let agent_id = resolve_agent_id(agent)?;
    let label = agent;

    if !force {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!("Terminate '{label}' and deregister?"))
            .default(false)
            .interact()?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }
    let client = Client::from_active_cluster()?;
    // `DELETE /agents/{id}` (not `/containers/{agent}`) — it tears down every
    // container for this agent *and* deletes the catalog row. The container-only
    // route left the catalog entry behind, so the agent kept showing up in
    // `ps`/`agents ls` forever after a "successful" removal.
    let result: DeletedAgent = client.delete_json(&format!("/agents/{agent_id}"))?;
    for err in &result.runtime_errors {
        term::print_warning(err);
    }
    term::print_success(&format!(
        "Removed: {label} ({} container(s) stopped)",
        result.containers_stopped
    ));
    Ok(())
}

const PUBLIC_REGISTRY_URL: &str = "https://registry.nasiko.dev";

fn registry_url() -> String {
    crate::config::artifact_registry_url().unwrap_or_else(|| PUBLIC_REGISTRY_URL.to_string())
}

fn unwrap_agents(raw: serde_json::Value) -> Result<Vec<AgentRecord>> {
    if let Some(arr) = raw.as_array() {
        Ok(serde_json::from_value(serde_json::Value::Array(
            arr.clone(),
        ))?)
    } else if let Some(data) = raw.get("data") {
        Ok(serde_json::from_value(data.clone())?)
    } else {
        Ok(serde_json::from_value(raw)?)
    }
}

/// Resolve a chat target (URL, "orchestrator", or agent name/id) into a full
/// A2A endpoint URL.
///
/// Used by `nasiko chat -a <target>` so callers can pass a name instead of a
/// full proxy URL.
pub fn resolve_chat_target(target: &str) -> Result<String> {
    resolve_chat_target_with_kind(target).map(|(url, _)| url)
}

/// Like [`resolve_chat_target`], but also returns the resolved agent's
/// `workload_kind` (e.g. `acp_coding_agent`) so `chat` can label it and the
/// `--coding-agent` guardrail can assert on it. The kind is `None` for a raw URL
/// or the orchestrator endpoint (neither resolves to a single catalog agent).
pub fn resolve_chat_target_with_kind(target: &str) -> Result<(String, Option<String>)> {
    if target.starts_with("http://") || target.starts_with("https://") {
        return Ok((target.to_string(), None));
    }

    // "orchestrator" is not a catalog agent — it's the CP's own routing
    // endpoint (same one `nasiko chat` with no target uses).
    if target.eq_ignore_ascii_case("orchestrator") {
        let base = crate::config::active_url()?;
        return Ok((
            format!("{}/api/orchestrator/a2a", base.trim_end_matches('/')),
            None,
        ));
    }

    let client = Client::from_active_cluster()?;
    let agents: Vec<AgentRecord> = client.get_json("/agents?limit=100")?;
    let agent = agents
        .iter()
        .find(|a| a.id == target || a.name == target)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no agent found with id or name '{target}' (see `nasiko ps`; \
                 to message the orchestrator instead: nasiko chat \"{target}\" — \
                 any argument with spaces goes to the orchestrator)"
            )
        })?;

    let base = client.base_url().trim_end_matches('/');
    // Append only a real transport path (e.g. `/a2a/jsonrpc` for an HTTP agent).
    // A coding agent has no HTTP mount, so its card carries no transport path and
    // this is NULL — the canonical proxy URL is then the bare `/api/agents/{id}`.
    // A trailing-slash `/api/agents/{id}/` does NOT route (404); one-shot `chat`
    // hides this by trimming the slash, but the TUI posts the URL verbatim, so an
    // empty/`"/"` path must contribute no suffix here.
    let path = match agent.transport_path.as_deref() {
        Some(p) if !p.is_empty() && p != "/" => p,
        _ => "",
    };
    Ok((
        format!("{}/api/agents/{}{}", base, agent.id, path),
        agent.workload_kind.clone(),
    ))
}

/// Resolve a name or UUID into a deployed agent's UUID via the CP registry.
///
/// Used by `nasiko chat --agent <name>` so callers never have to look up and
/// paste an agent's proxy URL by hand.
pub fn resolve_agent_id(name_or_id: &str) -> Result<String> {
    if uuid::Uuid::parse_str(name_or_id).is_ok() {
        return Ok(name_or_id.to_string());
    }

    let client = Client::from_active_cluster()?;

    // Page through results — the server clamps `limit` to 100 per request and
    // defaults to the 50 most-recently-created agents if omitted entirely, so a
    // single unpaginated call silently misses any agent past the first page.
    // `q=` pre-filters server-side (name/description ILIKE) to cut down how many
    // pages a typical name needs to page through; the exact match below is what
    // actually decides membership.
    let mut matches: Vec<AgentRecord> = Vec::new();
    let mut offset = 0i64;
    loop {
        let path = format!(
            "/registry/user/agents?q={}&limit=100&offset={offset}",
            crate::api::urlencode(name_or_id)
        );
        let raw: serde_json::Value = client.get_json(&path)?;
        let page = unwrap_agents(raw)?;
        let page_len = page.len();
        matches.extend(page.into_iter().filter(|a| a.name == name_or_id));
        if page_len < 100 {
            break;
        }
        offset += 100;
    }

    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => {
            let hint = suggest_agent_name(&client, name_or_id)
                .map(|name| format!(" — did you mean '{name}'?"))
                .unwrap_or_default();
            bail!(
                "no agent named '{name_or_id}' found on the active cluster{hint} (run `{}`)",
                agent_command("ps")
            )
        }
        many => bail!(
            "multiple agents named '{name_or_id}': {} — use an ID instead",
            many.iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Pages of agent names [`suggest_agent_name`] reads at most — it only runs after a lookup
/// has already failed, so it stays cheap even on a large cluster.
const SUGGESTION_MAX_PAGES: i64 = 5;

/// The one agent name close enough to `typed` to be what was meant, for a "did you mean"
/// hint. Only ever shown, never acted on. Fetches names without the `q=` filter, since a
/// misspelling won't match it; any fetch error just means no suggestion.
fn suggest_agent_name(client: &Client, typed: &str) -> Option<String> {
    let mut names = Vec::new();
    for page in 0..SUGGESTION_MAX_PAGES {
        let path = format!("/registry/user/agents?limit=100&offset={}", page * 100);
        let raw: serde_json::Value = client.get_json(&path).ok()?;
        let agents = unwrap_agents(raw).ok()?;
        let full_page = agents.len() == 100;
        names.extend(agents.into_iter().map(|a| a.name));
        if !full_page {
            break;
        }
    }
    closest_match(typed, &names).map(str::to_string)
}

/// The candidate nearest to `typed` by edit distance (case-insensitive), if it is within a
/// third of `typed`'s length (at least 1) and strictly closer than every other candidate —
/// a tie means no suggestion, rather than a guess.
fn closest_match<'a>(typed: &str, candidates: &'a [String]) -> Option<&'a str> {
    let typed = typed.to_lowercase();
    let max_distance = (typed.chars().count() / 3).max(1);
    let mut best: Option<(&str, usize)> = None;
    let mut tied = false;
    for candidate in candidates {
        let distance = strsim::levenshtein(&typed, &candidate.to_lowercase());
        if distance > max_distance {
            continue;
        }
        match best {
            Some((name, d)) if distance == d && name != candidate => tied = true,
            Some((_, d)) if distance >= d => {}
            _ => {
                best = Some((candidate, distance));
                tied = false;
            }
        }
    }
    best.filter(|_| !tied).map(|(name, _)| name)
}

pub fn cmd_ls() -> Result<()> {
    let client = Client::from_active_cluster()?;
    // GET /agents returns Vec<Agent> directly — field names match AgentRecord.
    // /registry/user/agents returns {data:[{agent_id,...}]} which uses agent_id
    // instead of id, causing deserialization failures.
    let raw: serde_json::Value = client.get_json("/agents?limit=100")?;
    let agents = unwrap_agents(raw)?;

    if agents.is_empty() {
        println!("No agents found.");
        return Ok(());
    }

    // Same label the web UI shows (e.g. "Claude Code (ankit@nasiko.com)") —
    // `AgentRecord.name` stays the real identifier used for chat targeting
    // and resolution elsewhere, so it isn't touched, just not what's printed.
    let rows: Vec<LsTableRow> = agents
        .iter()
        .map(|a| LsTableRow {
            id: a.id.clone(),
            name: a.display_name.clone().unwrap_or_else(|| a.name.clone()),
            status: a.status.clone(),
            version: a.version.clone(),
            url: a.url.clone(),
        })
        .collect();

    println!(
        "{}",
        Table::new(&rows)
            .with(Style::blank())
            .with(Alignment::left())
    );
    println!("\n{} agent(s) total.", agents.len());
    Ok(())
}

#[derive(Tabled)]
struct LsTableRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(rename = "STATUS", display = "opt_dash")]
    status: Option<String>,
    #[tabled(rename = "VERSION", display = "opt_dash")]
    version: Option<String>,
    #[tabled(rename = "URL", display = "opt_dash")]
    url: Option<String>,
}

pub fn cmd_get(agent_id: Option<&str>, name: Option<&str>, format: &str) -> Result<()> {
    let client = Client::from_active_cluster()?;
    // GET /agents/{id} accepts either a UUID or an agent name.
    let raw: serde_json::Value = match (agent_id, name) {
        (Some(id), _) => client.get_json(&format!("/agents/{}", id))?,
        (None, Some(n)) => client.get_json(&format!("/agents/{}", n))?,
        (None, None) => bail!("Provide at least one of --agent-id or --name"),
    };
    let agent: AgentRecord = if let Some(data) = raw.get("data") {
        serde_json::from_value(data.clone())?
    } else {
        serde_json::from_value(raw)?
    };

    if format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": agent.id, "name": agent.name, "status": agent.status,
                "version": agent.version, "framework": agent.framework,
                "url": agent.url, "description": agent.description, "created_at": agent.created_at,
            }))?
        );
    } else {
        println!("ID          : {}", agent.id);
        println!("Name        : {}", agent.name);
        println!("Status      : {}", agent.status.as_deref().unwrap_or("-"));
        println!("Version     : {}", agent.version.as_deref().unwrap_or("-"));
        println!(
            "Framework   : {}",
            agent.framework.as_deref().unwrap_or("-")
        );
        println!("URL         : {}", agent.url.as_deref().unwrap_or("-"));
        println!(
            "Description : {}",
            agent.description.as_deref().unwrap_or("-")
        );
        println!(
            "Created     : {}",
            agent.created_at.as_deref().unwrap_or("-")
        );
    }
    Ok(())
}

pub fn cmd_deploy(source: &str, agent_name: Option<&str>) -> Result<()> {
    let source_path = Path::new(source);

    if !source_path.exists() {
        if source.contains('/') && !source.starts_with('/') {
            println!(
                "Detected GitHub repo '{}', cloning and deploying...",
                source
            );
            return crate::commands::github::clone(Some(source), None);
        }
        bail!(
            "'{}' does not exist and is not a GitHub owner/repo path.",
            source
        );
    }

    let (zip_path, is_temp) = if source_path.is_dir() {
        let tmp = std::env::temp_dir().join(format!(
            "nasiko-agent-{}.zip",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ));
        println!("Zipping directory '{}'...", source);
        let status = Command::new("zip")
            .args(["-r", &tmp.to_string_lossy(), "."])
            .current_dir(source_path)
            .status()
            .map_err(|e| anyhow::anyhow!("'zip' not found: {e}. Install with: brew install zip"))?;
        if !status.success() {
            bail!("zip failed for directory '{}'", source);
        }
        (tmp, true)
    } else if source_path.extension().and_then(|e| e.to_str()) == Some("zip") {
        (source_path.to_path_buf(), false)
    } else {
        bail!(
            "source must be a .zip file or a directory, got: '{}'",
            source
        );
    };

    let name = agent_name.unwrap_or_else(|| {
        source_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("agent")
    });

    let client = Client::from_active_cluster()?;
    println!("Uploading '{}'...", zip_path.display());
    let result = client.upload_agent(
        &zip_path,
        name,
        "latest",
        &[8000],
        &std::collections::HashMap::new(),
        // `nasiko agents deploy` has no --writable/--writable-path/--coding-agent
        // flags of its own yet.
        false,
        None,
        None,
    );

    if is_temp {
        let _ = std::fs::remove_file(&zip_path);
    }

    let queued = result?;
    println!("Status: {}", queued.data.status);
    if let (Some(build_id), Some(agent_id)) = (&queued.data.build_id, &queued.data.agent_id) {
        println!("build_id: {} | agent_id: {}", build_id, agent_id);
        println!("Waiting for build to complete...");
        client.poll_build_status(build_id)?;
    }
    println!("\nDeployed: {}", queued.data.agent_name);
    Ok(())
}

pub fn cmd_search(
    query: Option<&str>,
    artifact_type: Option<&str>,
    framework: Option<&str>,
    tags: Option<&str>,
    owner: Option<&str>,
    limit: usize,
) -> Result<()> {
    let base = registry_url();
    use crate::api::urlencode;
    let mut url = format!("{}/v1/search?limit={}", base, limit);
    if let Some(q) = query {
        url.push_str(&format!("&q={}", urlencode(q)));
    }
    if let Some(t) = artifact_type {
        url.push_str(&format!("&type={}", urlencode(t)));
    }
    if let Some(f) = framework {
        url.push_str(&format!("&framework={}", urlencode(f)));
    }
    if let Some(t) = tags {
        url.push_str(&format!("&tags={}", urlencode(t)));
    }
    if let Some(o) = owner {
        url.push_str(&format!("&owner={}", urlencode(o)));
    }

    let mut resp = ureq::get(&url)
        .call()
        .map_err(|e| anyhow::anyhow!("registry unreachable: {}", e))?;
    let raw: serde_json::Value = resp.body_mut().read_json()?;
    let items = raw
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();

    if items.is_empty() {
        println!("No artifacts found.");
        return Ok(());
    }

    let rows: Vec<SearchTableRow> = items
        .iter()
        .map(|item| {
            let tags_str = item
                .get("tags")
                .and_then(|t| t.as_array())
                .map(|t| {
                    t.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            SearchTableRow {
                name: item
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                owner: item
                    .get("owner")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                artifact_type: item
                    .get("artifact_type")
                    .or_else(|| item.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                framework: item
                    .get("framework")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                version: item
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-")
                    .to_string(),
                tags: tags_str,
            }
        })
        .collect();
    println!(
        "{}",
        Table::new(rows)
            .with(Style::blank())
            .with(Alignment::left())
    );
    println!("\n{} artifact(s) found.", items.len());
    Ok(())
}

#[derive(Tabled)]
struct SearchTableRow {
    #[tabled(rename = "NAME")]
    name: String,
    #[tabled(rename = "OWNER")]
    owner: String,
    #[tabled(rename = "TYPE")]
    artifact_type: String,
    #[tabled(rename = "FRAMEWORK")]
    framework: String,
    #[tabled(rename = "VERSION")]
    version: String,
    #[tabled(rename = "TAGS")]
    tags: String,
}

pub fn cmd_info(name: &str, owner: &str, version: Option<&str>) -> Result<()> {
    let base = registry_url();
    let url = match version {
        Some(v) => format!("{}/v1/artifacts/{}/{}/{}", base, owner, name, v),
        None => format!("{}/v1/artifacts/{}/{}", base, owner, name),
    };

    let mut resp = ureq::get(&url)
        .call()
        .map_err(|e| anyhow::anyhow!("registry unreachable: {}", e))?;
    let raw: serde_json::Value = resp.body_mut().read_json()?;
    let a = raw.get("data").cloned().unwrap_or(raw);

    println!(
        "Name        : {}",
        a.get("name").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "Owner       : {}",
        a.get("owner").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "Version     : {}",
        a.get("version").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "Type        : {}",
        a.get("artifact_type")
            .or_else(|| a.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("-")
    );
    println!(
        "Framework   : {}",
        a.get("framework").and_then(|v| v.as_str()).unwrap_or("-")
    );
    println!(
        "Description : {}",
        a.get("description").and_then(|v| v.as_str()).unwrap_or("-")
    );
    let tags_str = a
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|t| {
            t.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if !tags_str.is_empty() {
        println!("Tags        : {}", tags_str);
    }
    Ok(())
}

pub fn cmd_frameworks() -> Result<()> {
    let base = registry_url();
    let url = format!("{}/v1/meta/frameworks", base);

    let mut resp = ureq::get(&url)
        .call()
        .map_err(|e| anyhow::anyhow!("registry unreachable: {}", e))?;
    let raw: serde_json::Value = resp.body_mut().read_json()?;
    let frameworks = raw
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();

    if frameworks.is_empty() {
        println!("No frameworks found.");
        return Ok(());
    }
    println!("Available frameworks:");
    for fw in &frameworks {
        if let Some(name) = fw.as_str() {
            println!("  - {}", name);
        }
    }
    Ok(())
}

pub fn cmd_list_uploaded() -> Result<()> {
    let client = Client::from_active_cluster()?;
    let raw: serde_json::Value = client.get_json("/agents/my-uploads")?;
    let agents: Vec<UploadedAgent> = if let Some(data) = raw.get("data") {
        serde_json::from_value(data.clone())?
    } else if let Some(arr) = raw.as_array() {
        serde_json::from_value(serde_json::Value::Array(arr.clone()))?
    } else {
        serde_json::from_value(raw)?
    };

    if agents.is_empty() {
        println!("No uploaded agents found.");
        return Ok(());
    }

    println!(
        "{}",
        Table::new(&agents)
            .with(Style::blank())
            .with(Alignment::left())
    );
    println!("\n{} agent(s).", agents.len());
    Ok(())
}

// ─── Reupload / Versions / Rollback ──────────────────────────────────────────

/// Resolve the agent ID from positional UUID or --name, shared by reupload/versions/rollback.
fn resolve_id_or_name(id: Option<&str>, name: Option<&str>) -> Result<(String, String)> {
    match (id, name) {
        (Some(id), None) => {
            if uuid::Uuid::parse_str(id).is_err() {
                bail!(
                    "'{}' is not a valid UUID — use --name to look up by name",
                    id
                );
            }
            Ok((id.to_string(), id.to_string()))
        }
        (None, Some(n)) => {
            let resolved = resolve_agent_id(n)?;
            Ok((resolved, n.to_string()))
        }
        (None, None) => bail!("provide an agent UUID or use --name <name>"),
        (Some(_), Some(_)) => bail!("provide either an id or --name, not both"),
    }
}

pub fn reupload(
    id: Option<&str>,
    name: Option<&str>,
    source: &str,
    version: Option<&str>,
    changelog: Option<&str>,
) -> Result<()> {
    let (agent_id, label) = resolve_id_or_name(id, name)?;

    let source_path = Path::new(source);
    if !source_path.exists() {
        bail!("'{}' does not exist", source);
    }

    // Resolve version: flag → project files → None (server auto-patches)
    let resolved_version: Option<String> = version
        .map(String::from)
        .or_else(|| crate::util::detect_version_from_source(source_path));

    if let Some(ref v) = resolved_version {
        println!("Version: {v}");
    } else {
        println!("Version: (server will auto-bump patch)");
    }

    // Zip directory if needed
    let (zip_path, is_temp) = if source_path.is_dir() {
        let tmp = std::env::temp_dir().join(format!(
            "nasiko-reupload-{}.zip",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ));
        println!("Zipping '{}'...", source_path.display());
        let status = Command::new("zip")
            .args(["-r", &tmp.to_string_lossy(), "."])
            .current_dir(source_path)
            .status()
            .map_err(|e| anyhow::anyhow!("'zip' not found: {e}. Install with: brew install zip"))?;
        if !status.success() {
            bail!("zip failed for '{}'", source);
        }
        (tmp, true)
    } else if source_path.extension().and_then(|e| e.to_str()) == Some("zip") {
        (source_path.to_path_buf(), false)
    } else {
        bail!("source must be a directory or a .zip file");
    };

    let client = Client::from_active_cluster()?;
    let result = client.update_agent(&agent_id, &zip_path, resolved_version.as_deref(), changelog);

    if is_temp {
        let _ = std::fs::remove_file(&zip_path);
    }

    let queued: UpdateQueued = result?;
    println!(
        "Queued: {} → {} (build: {})",
        queued.previous_version, queued.new_version, queued.build_id
    );
    println!("Waiting for server to build and deploy...");
    client.poll_build_status(&queued.build_id)?;
    println!("\nRedeployed: {label} @ {}", queued.new_version);
    Ok(())
}

pub fn versions(id: Option<&str>, name: Option<&str>) -> Result<()> {
    let (agent_id, label) = resolve_id_or_name(id, name)?;
    let client = Client::from_active_cluster()?;
    let raw: serde_json::Value = client.get_json(&format!("/agents/{agent_id}/versions"))?;
    let mut versions: Vec<AgentVersion> = unwrap_data(raw)?;
    if versions.is_empty() {
        println!("No versions found for '{label}'.");
        return Ok(());
    }
    // The active version is never a valid rollback target — `nasiko rollback`
    // only ever considers archived rows — so showing a stale `can_rollback`
    // flag (left over from before it was last activated) on the active row
    // reads as "you can roll back" when you can't. Normalize it to false here
    // rather than teach the server or the Tabled layout about this.
    for v in &mut versions {
        if v.is_active {
            v.can_rollback = false;
        }
    }
    println!(
        "{}",
        Table::new(&versions)
            .with(Style::blank())
            .with(Alignment::left())
    );
    println!("\n{} version(s) for '{label}'.", versions.len());
    Ok(())
}

pub fn rollback(
    id: Option<&str>,
    name: Option<&str>,
    version: Option<&str>,
    reason: Option<&str>,
) -> Result<()> {
    let (agent_id, label) = resolve_id_or_name(id, name)?;

    #[derive(serde::Serialize)]
    struct RollbackRequest<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        target_version: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<&'a str>,
    }

    #[derive(serde::Deserialize)]
    struct RollbackQueued {
        build_id: String,
        rolled_back_to: String,
        rolled_back_from: String,
    }

    let client = Client::from_active_cluster()?;
    let queued: RollbackQueued = client.post_json(
        &format!("/agents/{agent_id}/rollback"),
        &RollbackRequest {
            target_version: version,
            reason,
        },
    )?;

    println!(
        "Rolling back '{}': {} → {} (build: {})",
        label, queued.rolled_back_from, queued.rolled_back_to, queued.build_id
    );
    println!("Waiting for rollback to complete...");
    client.poll_build_status(&queued.build_id)?;
    println!("\nRolled back: {label} @ {}", queued.rolled_back_to);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        "2026-10-22T12:00:00Z".parse().unwrap()
    }

    fn line(level: &str, message: &str, timestamp: Option<&str>) -> LogLine {
        LogLine {
            timestamp: timestamp.map(str::to_string),
            level: Some(level.to_string()),
            message: message.to_string(),
            source: None,
        }
    }

    fn filters(level: Option<&str>, search: Option<&str>) -> LogFilters {
        LogFilters {
            level: level.map(str::to_string),
            search: search.map(str::to_string),
            since: None,
        }
    }

    #[test]
    fn resolve_since_converts_relative_shorthand() {
        let at = |s: &str| resolve_since(s, now()).unwrap().to_rfc3339();
        assert_eq!(at("30m"), "2026-10-22T11:30:00+00:00");
        assert_eq!(at("2h"), "2026-10-22T10:00:00+00:00");
        assert_eq!(at("1d"), "2026-10-21T12:00:00+00:00");
    }

    #[test]
    fn resolve_since_accepts_an_iso_timestamp() {
        let at = resolve_since("2026-10-20T08:15:00Z", now()).unwrap();
        assert_eq!(at.to_rfc3339(), "2026-10-20T08:15:00+00:00");
    }

    #[test]
    fn resolve_since_rejects_junk_instead_of_panicking() {
        for bad in ["", "m", "5", "5w", "-5m", "5µ", "abc", "4294967295d"] {
            assert!(resolve_since(bad, now()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn log_filters_match_level_case_insensitively() {
        let f = filters(Some("error"), None);
        assert!(f.matches(&line("ERROR", "boom", None), None));
        assert!(!f.matches(&line("INFO", "boom", None), None));
    }

    #[test]
    fn log_filters_match_search_case_insensitively() {
        let f = filters(None, Some("Timeout"));
        assert!(f.matches(&line("WARN", "upstream TIMEOUT after 5s", None), None));
        assert!(!f.matches(&line("WARN", "ok", None), None));
    }

    #[test]
    fn log_filters_drop_lines_older_than_since_but_keep_undated_ones() {
        let f = filters(None, None);
        let since = Some(resolve_since("1h", now()).unwrap());
        assert!(f.matches(&line("INFO", "new", Some("2026-10-22T11:30:00Z")), since));
        assert!(!f.matches(&line("INFO", "old", Some("2026-10-22T09:00:00Z")), since));
        assert!(f.matches(&line("INFO", "undated", None), since));
    }

    #[test]
    fn log_line_keeps_whole_second_timestamps_and_trims_long_ones() {
        let whole = line("INFO", "ok", Some("2026-10-22T12:00:00Z"));
        assert_eq!(format_log_line(&whole), "2026-10-22T12:00:00Z INFO  [?] ok");
        let precise = line("WARN", "slow", Some("2026-10-22T12:00:00.123456789Z"));
        assert_eq!(
            format_log_line(&precise),
            "2026-10-22T12:00:00.123 WARN  [?] slow"
        );
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn closest_match_suggests_a_near_typo() {
        let agents = names(&["weather", "billing", "notes"]);
        assert_eq!(closest_match("wether", &agents), Some("weather"));
        assert_eq!(closest_match("WEATHER", &agents), Some("weather"));
        assert_eq!(closest_match("biling", &agents), Some("billing"));
    }

    #[test]
    fn closest_match_stays_quiet_when_nothing_is_close() {
        let agents = names(&["weather", "billing"]);
        assert_eq!(closest_match("payments", &agents), None);
        assert_eq!(closest_match("w", &agents), None);
        assert_eq!(closest_match("anything", &[]), None);
    }

    #[test]
    fn closest_match_refuses_to_guess_between_equally_close_names() {
        let agents = names(&["agent-a", "agent-b"]);
        assert_eq!(closest_match("agent-c", &agents), None);
        // ...but a strictly closer later candidate still wins over an earlier tie.
        let agents = names(&["agent-a", "agent-b", "agent-cc"]);
        assert_eq!(closest_match("agent-ccc", &agents), Some("agent-cc"));
    }

    #[test]
    fn status_leaves_non_error_states_uncolored() {
        // Never wrapped in color codes, whether or not stdout is a terminal.
        assert_eq!(colorize_status("stopped"), "stopped");
        assert_eq!(colorize_status("registered"), "registered");
    }
}
