//! `nasiko agents import`: report sessions that finished before the hook was
//! installed. Each transcript is parsed by the adapter and handed to the
//! hook's own [`report::report_snapshot`], so the per-session watermark makes
//! re-imports, and imports of sessions the hook already saw, send nothing twice.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, Utc};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use super::agents::{Agent, codex};
use super::model::SessionSnapshot;
use super::state::IntegrationState;
use super::{catalog, report, sync};

pub struct ImportOptions<'a> {
    /// `claude`, `codex`, or `all` installed ones.
    pub agent_id: &'a str,
    /// `YYYY-MM-DD` or RFC 3339: only turns that started at or after it.
    pub since: Option<&'a str>,
    pub dry_run: bool,
    pub no_content: bool,
}

pub fn run(options: ImportOptions<'_>) -> Result<()> {
    let since = options.since.map(parse_since).transpose()?;
    let settings = IntegrationState::load()?;
    let agents: Vec<Agent> = match options.agent_id {
        "all" => [Agent::Claude, Agent::Codex]
            .into_iter()
            .filter(|agent| settings.get(agent.spec().id).is_some())
            .collect(),
        "claude" => vec![Agent::Claude],
        "codex" => vec![Agent::Codex],
        other => bail!("history import supports claude, codex or all, not '{other}'"),
    };
    if agents.is_empty() {
        bail!("no agent is installed — run: nasiko agents install <claude|codex>");
    }
    for agent in agents {
        let spec = agent.spec();
        if settings.get(spec.id).is_none() {
            bail!(
                "{} is not installed — run: nasiko agents install {}",
                spec.display_name,
                spec.id
            );
        }
        let (mut sessions, mut turns, mut tokens, mut queued) = (0, 0, 0, 0);
        for path in transcripts(agent, since)? {
            let mut snapshot = match parse(agent, &path) {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => continue,
                Err(error) => {
                    eprintln!("warning: skipped {}: {error:#}", path.display());
                    continue;
                }
            };
            snapshot.turns.retain(|turn| {
                turn.response.is_some()
                    && !turn.is_empty()
                    && since.is_none_or(|at| turn.started_at >= at)
            });
            if snapshot.turns.is_empty() {
                continue;
            }
            sessions += 1;
            turns += snapshot.turns.len();
            tokens += snapshot
                .turns
                .iter()
                .flat_map(|turn| &turn.calls)
                .map(|call| {
                    call.input_tokens
                        + call.output_tokens
                        + call.cache_read_tokens
                        + call.cache_creation_tokens
                })
                .sum::<u64>();
            if !options.dry_run {
                let deadline = Instant::now() + Duration::from_secs(5);
                match report::report_snapshot(agent, snapshot, deadline, !options.no_content) {
                    Ok(count) => queued += count,
                    Err(error) => eprintln!("warning: {}: {error:#}", path.display()),
                }
            }
        }
        println!(
            "{}: {turns} completed turn(s), {tokens} tokens, in {sessions} session(s)",
            spec.display_name
        );
        if !options.dry_run {
            println!("  queued {queued} new turn(s); the rest were already reported");
        }
    }
    if options.dry_run {
        println!("Dry run: nothing was queued. Turns already reported are skipped on a real run.");
        return Ok(());
    }
    // Hooks spawn a background sync per session; this flushes what is left.
    if let Err(error) = sync::run() {
        eprintln!(
            "Delivery is running in the background ({error:#}); to flush now: nasiko agents sync"
        );
    }
    Ok(())
}

/// The session the hook would have seen. Claude's hook payload is just the
/// session id (the file stem) and transcript path.
fn parse(agent: Agent, path: &Path) -> Result<Option<SessionSnapshot>> {
    match agent {
        Agent::Codex => codex::history_snapshot(path),
        _ => {
            let Some(session_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                return Ok(None);
            };
            let payload = serde_json::json!({"session_id": session_id, "transcript_path": path});
            agent
                .snapshot(&payload.to_string(), Instant::now())
                .map(Some)
        }
    }
}

/// Main-session transcripts (Claude sub-agent transcripts are not reported by
/// the hook either), skipping files last written before `since`.
fn transcripts(agent: Agent, since: Option<DateTime<Utc>>) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    match agent {
        Agent::Codex => {
            let home = codex::config_path();
            jsonl_files(&home.join("sessions"), 3, &mut files)?;
            jsonl_files(&home.join("archived_sessions"), 0, &mut files)?;
        }
        _ => jsonl_files(
            &catalog::home().join(".claude").join("projects"),
            1,
            &mut files,
        )?,
    }
    files.retain(|path| {
        since.is_none_or(|since| {
            let modified = std::fs::metadata(path).and_then(|meta| meta.modified());
            DateTime::<Utc>::from(modified.unwrap_or(SystemTime::now())) >= since
        })
    });
    files.sort();
    Ok(files)
}

/// `*.jsonl` files in `dir` and up to `depth` levels of subdirectories.
fn jsonl_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() && depth > 0 {
            jsonl_files(&path, depth - 1, out)?;
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

fn parse_since(value: &str) -> Result<DateTime<Utc>> {
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Ok(date
            .and_hms_opt(0, 0, 0)
            .expect("midnight exists")
            .and_utc());
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
    fn walks_jsonl_files_to_the_given_depth_only() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("project").join("session");
        std::fs::create_dir_all(session.join("subagents")).unwrap();
        for file in [
            "project/a.jsonl",
            "project/notes.txt",
            "project/session/subagents/b.jsonl",
        ] {
            std::fs::write(dir.path().join(file), "").unwrap();
        }
        let mut files = Vec::new();
        jsonl_files(dir.path(), 1, &mut files).unwrap();
        assert_eq!(files, [dir.path().join("project/a.jsonl")]);
        jsonl_files(&dir.path().join("missing"), 1, &mut files).unwrap();
        assert_eq!(files.len(), 1);
    }
}
