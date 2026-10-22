//! Import local history through the same durable capture seam as live hooks.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, Utc};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::agents::{Agent, codex};
use super::model::SessionSnapshot;
use super::state::IntegrationState;
use super::{catalog, report, sync};

const SESSION_BUDGET: Duration = Duration::from_secs(5);

pub struct ImportOptions<'a> {
    pub agent_id: &'a str,
    pub since: Option<&'a str>,
    pub dry_run: bool,
}

#[derive(Default)]
struct Summary {
    sessions: usize,
    turns: usize,
    unsupported: usize,
    failed_files: usize,
    replayed: usize,
    subagents: usize,
    queued: usize,
    already_reported: usize,
    failed_turns: usize,
}

pub fn run(options: ImportOptions<'_>) -> Result<()> {
    options.since.map(parse_since).transpose()?;
    let sources = selected_agents(&options)?
        .into_iter()
        .map(|agent| Ok((agent, transcripts(agent)?)))
        .collect::<Result<Vec<_>>>()?;
    println!(
        "{:<10} {:<10} {:<10} {:<13} FAILED",
        "AGENT", "SESSIONS", "TURNS", "UNSUPPORTED"
    );
    println!("{}", "-".repeat(54));
    let mut failed = 0;
    for (agent, files) in sources {
        let summary = import_files(agent, files, &options)?;
        println!(
            "{:<10} {:<10} {:<10} {:<13} {}",
            agent.spec().id,
            summary.sessions,
            summary.turns,
            summary.unsupported,
            summary.failed_files
        );
        if summary.replayed > 0 {
            println!("  {} fork replay turn(s) counted once", summary.replayed);
        }
        if summary.subagents > 0 {
            println!("  {} subagent transcript(s) included", summary.subagents);
        }
        if !options.dry_run {
            println!(
                "  Queued {} turn(s); already reported {}; failed {}",
                summary.queued, summary.already_reported, summary.failed_turns
            );
        }
        failed += summary.failed_files;
    }
    println!("\nUNSUPPORTED and FAILED count transcript files; empty sessions are omitted.");
    if options.dry_run {
        println!("Dry run: nothing was queued. Already reported turns are checked on import.");
    } else {
        sync::run().context("failed to sync imported turns — retry: nasiko agents sync")?;
    }
    if failed > 0 {
        bail!("failed to import {failed} transcript file(s)");
    }
    Ok(())
}

fn selected_agents(options: &ImportOptions<'_>) -> Result<Vec<Agent>> {
    let agents = match options.agent_id {
        "all" => vec![Agent::Claude, Agent::Codex],
        "claude" => vec![Agent::Claude],
        "codex" => vec![Agent::Codex],
        other => bail!("history import supports claude, codex or all, not '{other}'"),
    };
    // Preview works before installation and never opens Nasiko's state directory.
    if options.dry_run {
        return Ok(agents);
    }
    let settings = IntegrationState::load()?;
    let agents: Vec<_> = agents
        .into_iter()
        .filter(|agent| options.agent_id != "all" || settings.get(agent.spec().id).is_some())
        .collect();
    if agents.is_empty() {
        bail!("no agent is installed — run: nasiko agents install <claude|codex>");
    }
    for agent in &agents {
        let spec = agent.spec();
        let installed = settings.get(spec.id).with_context(|| {
            format!(
                "{} is not installed — run: nasiko agents install {}",
                spec.display_name, spec.id
            )
        })?;
        if !installed.capture_content {
            println!(
                "{}: content capture is disabled; imported metadata has no TokenOps costs. The server prices Content events only.",
                spec.display_name
            );
        }
    }
    Ok(agents)
}

fn import_files(agent: Agent, files: Vec<PathBuf>, options: &ImportOptions<'_>) -> Result<Summary> {
    let since = options.since.map(parse_since).transpose()?;
    let mut summary = Summary::default();
    let mut history_index = codex::history::HistoryIndex::default();
    for path in files {
        let (mut snapshot, replayed) = match parse(agent, &path, &mut history_index) {
            Ok(Some(history)) => history,
            Ok(None) => {
                summary.unsupported += 1;
                continue;
            }
            Err(error) => {
                summary.failed_files += 1;
                eprintln!("warning: {}: {error:#}", path.display());
                continue;
            }
        };
        summary.replayed += replayed;
        select_turns(&mut snapshot, since);
        if snapshot.turns.is_empty() {
            continue;
        }
        summary.sessions += 1;
        summary.turns += snapshot.turns.len();
        if agent == Agent::Claude {
            summary.subagents += snapshot
                .turns
                .iter()
                .filter_map(|turn| turn.uuid.strip_prefix("subagent:")?.split_once(':'))
                .map(|(agent_id, _)| agent_id)
                .collect::<std::collections::HashSet<_>>()
                .len();
        }
        if !options.dry_run {
            let counts = report::queue_snapshot_chunks(agent, &snapshot, SESSION_BUDGET);
            summary.queued += counts.queued;
            summary.already_reported += counts.already_reported;
            summary.failed_turns += counts.failed;
            if let Some(error) = counts.error {
                summary.failed_files += 1;
                eprintln!("warning: {}: {error:#}", path.display());
            }
        }
    }
    Ok(summary)
}

fn select_turns(snapshot: &mut SessionSnapshot, since: Option<DateTime<Utc>>) {
    snapshot.turns.retain(|turn| {
        turn.response.is_some() && !turn.is_empty() && since.is_none_or(|at| turn.started_at >= at)
    });
}

fn parse(
    agent: Agent,
    path: &Path,
    known_turns: &mut codex::history::HistoryIndex,
) -> Result<Option<(SessionSnapshot, usize)>> {
    match agent {
        Agent::Codex => codex::history::snapshot(path, known_turns)
            .map(|history| history.map(|history| (history.snapshot, history.replayed))),
        _ => {
            let session_id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .context("transcript filename is not a session id")?;
            let payload = serde_json::json!({"session_id": session_id, "transcript_path": path});
            agent
                .snapshot(&payload.to_string(), Instant::now())
                .map(|snapshot| Some((snapshot, 0)))
        }
    }
}

fn transcripts(agent: Agent) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    match agent {
        Agent::Codex => {
            let home = codex::config_path();
            jsonl_files(&home.join("sessions"), 3, &mut files)?;
            jsonl_files(&home.join("archived_sessions"), 0, &mut files)?;
        }
        _ => files = claude_transcripts(&catalog::home().join(".claude").join("projects"))?,
    }
    let mut files: Vec<_> = files
        .into_iter()
        .map(|path| {
            // Read failures are reported by parse; this pass only orders ownership.
            let (started, forked) = if agent == Agent::Codex {
                codex::history::origin(&path).unwrap_or_default()
            } else {
                (None, false)
            };
            ((started, forked), path)
        })
        .collect();
    // Use rollout names to break timestamp ties across archive directories.
    files.sort_by_key(|(origin, path)| (*origin, path.file_name().map(std::ffi::OsStr::to_owned)));
    Ok(files.into_iter().map(|(_, path)| path).collect())
}

fn claude_transcripts(projects: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    jsonl_files(projects, 1, &mut files)?;
    files.retain(|path| {
        !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("agent-"))
    });
    Ok(files)
}

fn jsonl_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", dir.display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() && depth > 0 {
            jsonl_files(&path, depth - 1, out)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

fn parse_since(value: &str) -> Result<DateTime<Utc>> {
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(date.and_hms_opt(0, 0, 0).context("invalid date")?.and_utc());
    }
    DateTime::parse_from_rfc3339(value)
        .map(|at| at.with_timezone(&Utc))
        .with_context(|| format!("--since '{value}' is not YYYY-MM-DD or RFC 3339"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_accepts_a_date_or_an_instant() {
        assert_eq!(
            parse_since("2026-03-01").unwrap().to_rfc3339(),
            "2026-03-01T00:00:00+00:00"
        );
        assert_eq!(
            parse_since("2026-03-01T12:00:00+02:00")
                .unwrap()
                .to_rfc3339(),
            "2026-03-01T10:00:00+00:00"
        );
        assert!(parse_since("yesterday").is_err());
    }

    #[test]
    fn claude_discovery_leaves_subagents_to_the_parent_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("project/session/subagents")).unwrap();
        for file in [
            "project/a.jsonl",
            "project/notes.txt",
            "project/agent-legacy.jsonl",
            "project/session/subagents/b.jsonl",
        ] {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        let mut files = claude_transcripts(dir.path()).unwrap();
        assert_eq!(files, [dir.path().join("project/a.jsonl")]);
        jsonl_files(&dir.path().join("missing"), 1, &mut files).unwrap();
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn claude_dry_run_counts_subagents_under_the_parent() {
        let projects =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/integration/agents/claude");
        let summary = import_files(
            Agent::Claude,
            claude_transcripts(&projects).unwrap(),
            &ImportOptions {
                agent_id: "claude",
                since: None,
                dry_run: true,
            },
        )
        .unwrap();
        assert_eq!(
            (summary.sessions, summary.turns, summary.subagents),
            (1, 3, 2)
        );
        assert_eq!((summary.queued, summary.failed_files), (0, 0));
    }

    #[test]
    fn dry_run_counts_supported_unsupported_empty_and_failed_files() {
        let dir = tempfile::tempdir().unwrap();
        let fixtures = [
            (
                "current",
                include_str!("agents/codex/fixtures/current.jsonl"),
            ),
            ("legacy", include_str!("agents/codex/fixtures/legacy.jsonl")),
            ("empty", include_str!("agents/codex/fixtures/empty.jsonl")),
            ("broken", "{broken"),
        ];
        let files = fixtures
            .into_iter()
            .map(|(name, content)| {
                let path = dir.path().join(format!("{name}.jsonl"));
                std::fs::write(&path, content).unwrap();
                path
            })
            .collect();
        let summary = import_files(
            Agent::Codex,
            files,
            &ImportOptions {
                agent_id: "codex",
                since: None,
                dry_run: true,
            },
        )
        .unwrap();
        assert_eq!(
            (
                summary.sessions,
                summary.turns,
                summary.unsupported,
                summary.failed_files
            ),
            (1, 2, 1, 1)
        );
        assert_eq!(
            (
                summary.queued,
                summary.already_reported,
                summary.failed_turns
            ),
            (0, 0, 0)
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 4);
    }

    fn fixture(name: &str, known: &mut codex::history::HistoryIndex) -> (SessionSnapshot, usize) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/commands/integration/agents/codex/fixtures")
            .join(name);
        parse(Agent::Codex, &path, known).unwrap().unwrap()
    }

    #[test]
    fn forks_replay_once_while_unrelated_sessions_can_reuse_ids() {
        let mut known = codex::history::HistoryIndex::default();
        let (mut parent, replayed) = fixture("parent.jsonl", &mut known);
        assert_eq!(replayed, 0);
        assert_eq!(parent.turns.len(), 1);
        select_turns(
            &mut parent,
            Some(parse_since("2026-03-01T10:00:30Z").unwrap()),
        );
        assert!(parent.turns.is_empty());
        let (fork, replayed) = fixture("fork.jsonl", &mut known);
        assert_eq!(replayed, 1);
        assert_eq!(fork.turns.len(), 2);
        assert_eq!(fork.turns[0].calls[0].input_tokens, 200);
        let (unrelated, replayed) = fixture("parent.jsonl", &mut known);
        assert_eq!(replayed, 0);
        assert_eq!(unrelated.turns.len(), 1);
    }

    #[test]
    fn since_uses_turn_time_even_when_the_file_is_older() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, include_str!("agents/codex/fixtures/current.jsonl")).unwrap();
        std::fs::File::open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH))
            .unwrap();
        let mut files = Vec::new();
        jsonl_files(dir.path(), 0, &mut files).unwrap();
        let (mut snapshot, _) = parse(
            Agent::Codex,
            &files[0],
            &mut codex::history::HistoryIndex::default(),
        )
        .unwrap()
        .unwrap();
        select_turns(
            &mut snapshot,
            Some(parse_since("2026-03-01T10:00:10Z").unwrap()),
        );
        assert_eq!(snapshot.turns.len(), 1);
        assert_eq!(snapshot.turns[0].uuid, "t2");
    }
}
