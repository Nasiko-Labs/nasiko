//! `nasiko agents setup`: discover, connect and import every coding agent on this computer, reporting
//! progress to the server so the web app's Harnesses page can follow it. Running it is the user's OK;
//! nothing else (a login, say) touches their coding agents. The import runs in the background
//! ([`background`]) unless `--wait` keeps it in the terminal. `nasiko agents import` runs the import
//! alone, `nasiko agents status` says how it is going, and `nasiko agents sync` by hand delivers what
//! is queued.

use anyhow::{Result, bail};
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use crate::api::Client;

use super::agents::Agent;
use super::background::{self, ImportLock, StatusFile};
use super::backlog;
use super::import::{self, Drain, ImportRun, WindowStatus};
use super::onboarding::{self, Phase, Reporter, Tracker};
use super::state::{self, IntegrationState};
use super::sync as delivery;

/// Bounds each progress and retry-request call, so a server that stops answering can't stall the run.
const SERVER_CALL_TIMEOUT: Duration = Duration::from_secs(10);
const ALREADY_IMPORTING: &str = "An import is already running on this computer; follow it with \
     `nasiko agents status` or in Nasiko (Coding harnesses).";
/// The progress line's width when the terminal's can't be read.
const DEFAULT_WIDTH: usize = 80;
/// Without a terminal (a log file), the progress line is printed at most this often.
const PLAIN_PROGRESS_INTERVAL: Duration = Duration::from_secs(5);

/// Options accepted by `nasiko agents setup`.
pub struct SetupOptions {
    /// Accepted for scripts written when setup asked first; it no longer does.
    pub yes: bool,
    /// Connect the agents but don't import their past sessions.
    pub no_import: bool,
    /// Import in this terminal, returning when it is done, instead of in the background.
    pub wait: bool,
}

pub fn setup(options: SetupOptions) -> Result<()> {
    let client = signed_in_client()?;
    let device_id = onboarding::device_id(&state::integrations_dir());
    retry_if_requested(&client, &device_id);
    let mut tracker = Tracker::new(Reporter::new(Some(client), device_id));
    let connected = connect(&mut tracker);
    if connected.is_err() || options.no_import {
        tracker.finish(error_text(&connected));
        return connected;
    }
    if !options.wait {
        // From here on the background import reports its progress.
        let started = start_background_import();
        if started.is_err() {
            tracker.finish(error_text(&started));
        }
        return started;
    }
    tracker.phase(Phase::Importing, super::agent_rows());
    println!("\nImporting past sessions, newest first...");
    let imported = import_here(|run| tracker.import(run));
    // An import already running elsewhere reports for itself.
    if !matches!(imported, Ok(None)) {
        tracker.finish(error_text(&imported));
    }
    imported.map(|_| ())
}

/// `nasiko agents import-worker`: the import `nasiko agents setup` started in the background,
/// reporting its progress to the server as setup would have.
pub fn import_worker() -> Result<()> {
    println!(
        "Import started at {}.",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    let client = signed_in_client()?;
    let device_id = onboarding::device_id(&state::integrations_dir());
    let mut tracker = Tracker::new(Reporter::new(Some(client), device_id));
    tracker.phase(Phase::Importing, super::agent_rows());
    let imported = import_here(|run| tracker.import(run));
    if !matches!(imported, Ok(None)) {
        tracker.finish(error_text(&imported));
    }
    if let Ok(Some(_)) = imported {
        println!(
            "Import finished at {}.",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
    }
    imported.map(|_| ())
}

/// `nasiko agents import`: the past sessions of the agents already connected.
pub fn import() -> Result<()> {
    if let Ok(client) = signed_in_client() {
        retry_if_requested(&client, &onboarding::device_id(&state::integrations_dir()));
    }
    if let Some(run) = import_here(|_| {})?
        && run.agents.is_empty()
    {
        println!("No coding agents are connected yet. Run: nasiko agents setup");
    }
    Ok(())
}

/// `nasiko agents status`: the import (running, or the last one), what is still waiting to send, and
/// the end of the background import's log.
pub fn status() -> Result<()> {
    let in_terminal = std::io::stdout().is_terminal();
    let width = terminal_width();
    let lines = background::status_report(&state::integrations_dir(), |line| {
        if in_terminal {
            fit(line, width)
        } else {
            line.to_string()
        }
    })?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

/// `nasiko agents sync`. Run by a hook (no terminal) it delivers what is due and stops; by hand it
/// first acts on a retry asked for in the web app (or `--retry-rejected`), then sends every queued turn
/// at once, even one waiting on a retry (the user asked; a server's own pause still holds), and keeps
/// delivering until the queue is empty, saying what it waits for.
pub fn sync(retry_rejected: bool) -> Result<()> {
    let by_hand = std::io::stdout().is_terminal();
    if retry_rejected {
        requeue_rejected()?;
    } else if by_hand && let Ok(client) = signed_in_client() {
        retry_if_requested(&client, &onboarding::device_id(&state::integrations_dir()));
    }
    if !by_hand {
        return delivery::run();
    }
    let mut printer = Printer::new();
    let run = import::deliver_queued(Drain::UntilEmpty, delivery::Backoff::Ignore, |run| {
        printer.show(run)
    })?;
    printer.finish(&run);
    Ok(())
}

fn signed_in_client() -> Result<Client> {
    let (cluster, entry) = crate::config::active_cluster()?;
    let Some(token) = entry.token.as_deref() else {
        bail!("not signed in to {cluster}. Run `nasiko auth login` first.");
    };
    if crate::config::token_expired(token) == Some(true) {
        bail!("your sign-in to {cluster} has expired. Run `nasiko auth login` first.");
    }
    Ok(Client::from_cluster_entry_with_timeout(
        &entry,
        Some(SERVER_CALL_TIMEOUT),
    ))
}

fn error_text<T>(result: &Result<T>) -> Option<String> {
    result.as_ref().err().map(|error| format!("{error:#}"))
}

/// Re-queues the set-aside turns when the web app asked for it since this computer last did. Best
/// effort: a server that can't say is no request.
fn retry_if_requested(client: &Client, device_id: &str) {
    match onboarding::take_retry_request(client, &state::integrations_dir(), device_id) {
        Ok(true) => {
            if let Err(error) = requeue_rejected() {
                eprintln!("warning: couldn't retry the turns that were set aside: {error:#}");
            }
        }
        Ok(false) => {}
        Err(error) => eprintln!("warning: couldn't check for a retry request: {error:#}"),
    }
}

fn requeue_rejected() -> Result<()> {
    let outcome = backlog::requeue_rejected()?;
    println!(
        "{} set-aside turn(s) queued again{}.",
        outcome.requeued,
        if outcome.invalid > 0 {
            format!("; {} still can't be sent", outcome.invalid)
        } else {
            String::new()
        }
    );
    Ok(())
}

/// Discovers the coding agents on this computer and connects every one not connected yet.
fn connect(tracker: &mut Tracker) -> Result<()> {
    tracker.phase(Phase::Discovering, super::agent_rows());
    tracker.phase(Phase::Connecting, super::agent_rows());
    super::auto_install_detected()?;
    tracker.phase(Phase::Connecting, super::agent_rows());
    Ok(())
}

/// Starts the import as a background process, unless one is running already, and says where to
/// follow it.
fn start_background_import() -> Result<()> {
    let dir = state::integrations_dir();
    if background::import_running(&dir)? {
        println!("\n{ALREADY_IMPORTING}");
        return Ok(());
    }
    let connected = connected_agents();
    if connected.is_empty() {
        return Ok(());
    }
    let (pid, log) = background::spawn_import(&dir)?;
    println!(
        "\nImporting the past sessions of {} in the background (pid {pid}), newest first.\n\
         Follow progress in Nasiko (Coding harnesses), with `nasiko agents status`, or: tail -f {}\n\
         It keeps going if you close this terminal; run `nasiko agents setup --wait` to import here \
         instead.",
        connected.join(", "),
        super::tildify(&log)
    );
    Ok(())
}

/// The display names of the connected coding agents.
fn connected_agents() -> Vec<&'static str> {
    let settings = IntegrationState::load().unwrap_or_default();
    Agent::ALL
        .iter()
        .filter(|agent| settings.get(agent.spec().id).is_some())
        .map(|agent| agent.spec().display_name)
        .collect()
}

/// Imports here, holding the import lock, printing as it goes and keeping the status file current;
/// `follow` sees the run too. `None` when another import holds the lock (it was said so).
fn import_here(mut follow: impl FnMut(&ImportRun)) -> Result<Option<ImportRun>> {
    let dir = state::integrations_dir();
    let Some(_lock) = ImportLock::try_acquire(&dir)? else {
        println!("{ALREADY_IMPORTING}");
        return Ok(None);
    };
    let mut printer = Printer::new();
    let mut status = StatusFile::new(&dir);
    let run = import::import_past_sessions(Drain::UntilEmpty, |run| {
        follow(run);
        printer.show(run);
        status.update(run, &waiting_line(run));
    })?;
    printer.finish(&run);
    status.finish(&run, &outcome_line(&run));
    Ok(Some(run))
}

/// Prints an import as it goes: a line for each window once it is done, in order, under a progress
/// line. In a terminal the progress line is rewritten in place and cut to the terminal's width;
/// elsewhere (a log file) it is a plain line, printed every few seconds.
struct Printer {
    /// Windows printed so far, from the newest.
    printed: usize,
    in_terminal: bool,
    /// When the progress line was last printed, without a terminal.
    last_plain: Option<Instant>,
}

impl Printer {
    fn new() -> Self {
        Self {
            printed: 0,
            in_terminal: std::io::stdout().is_terminal(),
            last_plain: None,
        }
    }

    fn show(&mut self, run: &ImportRun) {
        while let Some(window) = run
            .windows
            .get(self.printed)
            .filter(|window| window.status == WindowStatus::Done)
        {
            self.permanent(&format!(
                "{}: {} session(s), {} turn(s) ✓",
                window.label, window.sessions, window.turns
            ));
            self.printed += 1;
        }
        self.progress(&status_line(run));
    }

    /// The windows still waiting on deliveries, what is left in the queue, and what was set aside.
    fn finish(&mut self, run: &ImportRun) {
        self.show(run);
        for window in &run.windows[self.printed..] {
            self.permanent(&format!(
                "{}: {} session(s), {} turn(s), waiting to send",
                window.label, window.sessions, window.turns
            ));
        }
        self.printed = run.windows.len();
        if self.in_terminal {
            rewrite("");
        }
        if run.delivered() {
            if !run.windows.is_empty() {
                println!("Every imported turn was delivered.");
            }
        } else if run.backlog.pending() > 0 {
            println!("{}", sending_line(run));
        }
        let rejected: usize = run.backlog.agents.values().map(|a| a.rejected).sum();
        if rejected > 0 {
            println!("{rejected} turn(s) were set aside:");
            for (agent_id, agent) in &run.backlog.agents {
                for (reason, count) in &agent.reasons {
                    println!("  {agent_id}: {count} × {reason}");
                }
            }
            println!("Try them again with: nasiko agents sync --retry-rejected");
        }
    }

    /// A line that stays.
    fn permanent(&self, line: &str) {
        if self.in_terminal {
            rewrite(&format!("{line}\n"));
        } else {
            println!("{line}");
        }
    }

    fn progress(&mut self, line: &str) {
        if self.in_terminal {
            rewrite(&fit(line, terminal_width()));
            return;
        }
        let due = self
            .last_plain
            .is_none_or(|at| at.elapsed() >= PLAIN_PROGRESS_INTERVAL);
        if due && !line.is_empty() {
            println!("{line}");
            self.last_plain = Some(Instant::now());
        }
    }
}

/// `Read 9/23 windows · Oct 17 · 10/10 sessions · Delivered 1,730 turns · 499 waiting · 0 set aside`
/// while windows are read, so delivery lagging reading is seen to be moving; what delivery waits for
/// once all are.
fn status_line(run: &ImportRun) -> String {
    if run.sending.is_some() {
        return sending_line(run);
    }
    if run.windows.is_empty() {
        return String::new();
    }
    format!(
        "{} · {}",
        background::read_line(
            run.windows_read(),
            run.windows.len(),
            &background::reading(run)
        ),
        background::delivered_line(run.delivered, &run.backlog)
    )
}

/// What delivery waits for once every window is read, for the status file; empty before then.
fn waiting_line(run: &ImportRun) -> String {
    if run.sending.is_some() {
        sending_line(run)
    } else {
        String::new()
    }
}

/// How an import ended, for the status file.
fn outcome_line(run: &ImportRun) -> String {
    if run.delivered() {
        "Every imported turn was delivered.".into()
    } else {
        sending_line(run)
    }
}

/// What delivery waits for, once every window is read.
fn sending_line(run: &ImportRun) -> String {
    let pending = run.backlog.pending();
    let by_agent: Vec<String> = Agent::ALL
        .iter()
        .filter_map(|agent| {
            let n = run.backlog.agent(agent.spec().id).pending;
            (n > 0).then(|| format!("{} {n}", agent.spec().display_name))
        })
        .collect();
    let blocked = run.sending.as_ref().and_then(|s| s.blocked.as_deref());
    if let Some(reason) = blocked {
        return format!(
            "{pending} turn(s) can't be sent yet: {reason}. Sign in again with `nasiko auth login`, \
             then run `nasiko agents sync`."
        );
    }
    let next = run
        .sending
        .as_ref()
        .and_then(|s| s.next_attempt_in)
        .map(|wait| format!("next attempt in {}s", wait.as_secs()))
        .unwrap_or_else(|| "sending now".into());
    format!(
        "{pending} turn(s) waiting to send ({}), retrying automatically ({next}). Safe to stop: run \
         `nasiko agents sync` later.",
        by_agent.join(", ")
    )
}

/// The terminal's width in columns: as the terminal says, else `$COLUMNS`, else [`DEFAULT_WIDTH`].
fn terminal_width() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .filter(|columns| *columns > 0)
        .or_else(|| std::env::var("COLUMNS").ok()?.trim().parse().ok())
        .unwrap_or(DEFAULT_WIDTH)
}

/// `line` cut to fit in `width` columns with one to spare (a full last column wraps on some
/// terminals), ending in `…` when cut.
fn fit(line: &str, width: usize) -> String {
    let room = width.saturating_sub(1).max(1);
    if line.chars().count() <= room {
        return line.to_string();
    }
    let mut cut: String = line.chars().take(room - 1).collect();
    cut.push('…');
    cut
}

/// Replaces the current terminal line; clears it first, as a new line may be shorter.
fn rewrite(line: &str) {
    print!("\r\x1b[2K{line}");
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::integration::backlog::AgentBacklog;
    use crate::commands::integration::import::{Sending, WindowProgress};

    fn waiting(blocked: Option<&str>) -> ImportRun {
        let mut run = ImportRun {
            windows: vec![WindowProgress::for_test(
                "Oct 19",
                WindowStatus::Sending,
                3,
                41,
            )],
            sending: Some(Sending {
                next_attempt_in: Some(Duration::from_secs(42)),
                blocked: blocked.map(str::to_string),
            }),
            ..ImportRun::default()
        };
        run.backlog.agents.insert(
            "claude".into(),
            AgentBacklog {
                pending: 5,
                ..AgentBacklog::default()
            },
        );
        run
    }

    #[test]
    fn waiting_turns_say_when_the_next_attempt_is_and_that_stopping_is_safe() {
        let line = sending_line(&waiting(None));

        assert!(line.starts_with("5 turn(s) waiting to send (Claude Code 5)"));
        assert!(line.contains("next attempt in 42s"));
        assert!(line.contains("Safe to stop: run `nasiko agents sync` later."));
    }

    #[test]
    fn blocked_turns_say_what_the_user_must_do() {
        let line = sending_line(&waiting(Some("HTTP 401: expired")));

        assert!(line.contains("can't be sent yet: HTTP 401: expired"));
        assert!(line.contains("nasiko auth login"));
    }

    #[test]
    fn the_progress_line_shows_reading_and_delivery_apart_cut_to_the_terminal_width() {
        let mut window = WindowProgress::for_test("Oct 17", WindowStatus::Reading, 10, 40);
        window.done = 10;
        let mut run = ImportRun {
            windows: vec![
                WindowProgress::for_test("Today", WindowStatus::Done, 2, 3),
                WindowProgress::for_test("Yesterday", WindowStatus::Sending, 5, 9),
                window,
                WindowProgress::for_test("Oct 16", WindowStatus::Waiting, 7, 0),
            ],
            delivered: 1_730,
            ..ImportRun::default()
        };
        run.backlog.agents.insert(
            "claude".into(),
            AgentBacklog {
                pending: 499,
                ..AgentBacklog::default()
            },
        );

        let line = status_line(&run);

        assert_eq!(
            line,
            "Read 2/4 windows · Oct 17 · 10/10 sessions · Delivered 1,730 turns · 499 waiting · 0 set aside"
        );
        assert_eq!(fit(&line, 120), line);
        let cut = fit(&line, 20);
        assert_eq!(cut.chars().count(), 19);
        assert!(cut.starts_with("Read 2/4 windows") && cut.ends_with('…'));
        assert_eq!(fit(&line, 0), "…");
    }

    #[test]
    fn the_status_file_says_what_delivery_waits_for_only_once_every_window_is_read() {
        assert_eq!(waiting_line(&ImportRun::default()), "");
        assert!(waiting_line(&waiting(None)).starts_with("5 turn(s) waiting to send"));
    }
}
