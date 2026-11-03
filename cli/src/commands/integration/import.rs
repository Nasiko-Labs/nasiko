//! Importing the sessions coding agents kept from before they were connected, newest first.
//!
//! Every connected agent's sessions are listed first (cheap: no transcript is parsed) and grouped into
//! [`windows`] by when each was last active. Each window is then read in turn, across all agents,
//! through the same capture their hooks use (turns already sent are skipped, so an import can run again
//! at no cost), and its turns are handed to a [`Courier`] that delivers them while the next window is
//! read. A window is done once it is read and none of the turns it queued is left in the queue: each was
//! accepted by the server, or set aside. Done windows are recorded in the [`Ledger`], so an import that
//! was stopped skips them next time.
//!
//! Once every window is read, the run keeps delivering what is left ([`Drain`]); turns the server can't
//! take yet wait in the queue for a retry, and are never given up on for that.

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::agents::{Agent, PastSession, SessionSource};
use super::backlog::{self, Backlog};
use super::courier::{Courier, Outcome};
use super::ledger::Ledger;
use super::queue;
use super::report::{self, CaptureMode, Captured, REPORT_BUDGET};
use super::state::{self, IntegrationState};
use super::sync::{self, Backoff};
use super::windows;

/// How often a run waiting on deliveries checks on them and says how long it still waits.
const TICK: Duration = Duration::from_secs(1);

/// How far one agent's import has got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportProgress {
    /// Sessions looked at so far, of `total`.
    pub done: usize,
    pub total: usize,
    /// Sessions that had turns not sent yet.
    pub sessions: usize,
    /// Turns queued for delivery.
    pub turns: usize,
    /// Sessions that couldn't be read; the rest are imported regardless.
    pub failed: usize,
    /// The agent's store couldn't be read at all.
    pub error: Option<String>,
    pub finished: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowStatus {
    Waiting,
    Reading,
    /// Read; some of its turns are still waiting to be delivered.
    Sending,
    /// Read, and every turn it queued was accepted or set aside.
    Done,
}

/// One window of the import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowProgress {
    pub label: String,
    pub status: WindowStatus,
    /// Sessions in the window.
    pub sessions: usize,
    /// Of those, the ones looked at so far.
    pub done: usize,
    /// Turns its sessions queued.
    pub turns: usize,
    /// Event ids of those turns, to tell when the last has left the queue.
    queued: HashSet<String>,
    /// The sessions read without error, for the ledger once the window is done.
    read: Vec<(Agent, String, DateTime<Utc>)>,
}

/// Why turns are still waiting once every window is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sending {
    /// Until the next retry is due; `None` when a delivery is due or under way now.
    pub next_attempt_in: Option<Duration>,
    /// Set when they can't be sent until the user acts (signed out, say).
    pub blocked: Option<String>,
}

/// An import as it stands.
#[derive(Debug, Clone, Default)]
pub struct ImportRun {
    /// The connected agents, in catalog order.
    pub agents: Vec<(Agent, ImportProgress)>,
    /// Newest first.
    pub windows: Vec<WindowProgress>,
    /// What the queue holds, per agent.
    pub backlog: Backlog,
    /// Set once every window is read while turns are still waiting to be delivered.
    pub sending: Option<Sending>,
    /// Turns the server took (accepted, or already had) during this run.
    pub delivered: usize,
}

#[cfg(test)]
impl WindowProgress {
    pub(super) fn for_test(
        label: &str,
        status: WindowStatus,
        sessions: usize,
        turns: usize,
    ) -> Self {
        Self {
            label: label.into(),
            status,
            sessions,
            done: 0,
            turns,
            queued: HashSet::new(),
            read: Vec::new(),
        }
    }
}

impl ImportRun {
    /// Every window done and nothing left to deliver.
    pub fn delivered(&self) -> bool {
        self.backlog.pending() == 0
            && self
                .windows
                .iter()
                .all(|window| window.status == WindowStatus::Done)
    }

    /// Windows whose sessions have all been read (their turns may still be waiting to be delivered).
    pub fn windows_read(&self) -> usize {
        self.windows
            .iter()
            .filter(|window| matches!(window.status, WindowStatus::Sending | WindowStatus::Done))
            .count()
    }

    /// Windows read whose turns have all been delivered or set aside.
    pub fn windows_done(&self) -> usize {
        self.windows
            .iter()
            .filter(|window| window.status == WindowStatus::Done)
            .count()
    }

    fn progress(&mut self, agent: Agent) -> &mut ImportProgress {
        let index = self
            .agents
            .iter()
            .position(|(a, _)| *a == agent)
            .expect("only connected agents' sessions are imported");
        &mut self.agents[index].1
    }
}

/// How long a run keeps delivering once every window is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drain {
    /// Until the queue is empty, waiting out retries (the CLI; Ctrl-C leaves the rest queued).
    UntilEmpty,
    /// While something is due now; turns waiting on a retry are left for a later delivery (the
    /// desktop app, which delivers again every few minutes).
    WhileDue,
}

/// Imports the past sessions of every connected agent, window by window, newest first, then delivers
/// what is left as `drain` says. `observe` sees the run after each session and each change in delivery.
/// Agents that aren't connected are not imported. Blocking.
pub fn import_past_sessions(drain: Drain, observe: impl FnMut(&ImportRun)) -> Result<ImportRun> {
    let settings = IntegrationState::load()?;
    let agents: Vec<Agent> = Agent::ALL
        .iter()
        .copied()
        .filter(|agent| settings.get(agent.spec().id).is_some())
        .collect();
    let listed: Vec<(Agent, Result<Vec<PastSession>>)> = agents
        .iter()
        .map(|agent| (*agent, agent.past_sessions()))
        .collect();
    importer(observe, Backoff::Honour).run(listed, drain)
}

/// Delivers what the queue holds, as `drain` says, with no import: `nasiko agents sync` run by hand.
/// With [`Backoff::Ignore`] (the user asked to send now) every queued turn gets an attempt at once,
/// even one waiting on a retry; a server's own pause still holds, and a turn that fails again waits as
/// usual.
pub fn deliver_queued(
    drain: Drain,
    backoff: Backoff,
    observe: impl FnMut(&ImportRun),
) -> Result<ImportRun> {
    let mut importer = importer(observe, backoff);
    if backoff == Backoff::Ignore {
        // Nothing may be due by its retry delay, so the drain alone wouldn't send.
        importer.courier.nudge();
    }
    importer.run(Vec::new(), drain)
}

type Capture = fn(Agent, SessionSource) -> Result<Captured>;

/// An importer on this computer's queue, delivering to the server.
fn importer<O: FnMut(&ImportRun)>(observe: O, backoff: Backoff) -> Importer<Capture, O> {
    Importer {
        root: state::integrations_dir(),
        ledger: Ledger::load(),
        courier: paced_courier(backoff, sync::deliver_paced),
        capture: capture_past,
        observe,
        run: ImportRun::default(),
        last_pass: None,
        tick: TICK,
    }
}

/// A courier delivering with `deliver` at one pace kept across its passes. The first pass that runs
/// treats retry delays as `backoff` says; every later one honours them, so a turn that fails again
/// waits out its delay.
fn paced_courier(
    backoff: Backoff,
    mut deliver: impl FnMut(&mut sync::Pace, Backoff) -> Result<sync::Pass> + Send + 'static,
) -> Courier {
    let mut pace = sync::Pace::default();
    let mut backoff = backoff;
    Courier::start(move || {
        let pass = deliver(&mut pace, backoff);
        if pass.is_ok() {
            backoff = Backoff::Honour;
        }
        pass
    })
}

/// Captures one past session the way its hook would, but without waiting on it or logging per session.
fn capture_past(agent: Agent, source: SessionSource) -> Result<Captured> {
    let deadline = Instant::now() + REPORT_BUDGET;
    let snapshot = agent.past_snapshot(source)?;
    report::capture_snapshot(agent, snapshot, deadline, CaptureMode::Import)
}

/// One import: where its queue is, how it captures, delivers and reports.
struct Importer<C, O> {
    root: PathBuf,
    ledger: Ledger,
    courier: Courier,
    capture: C,
    observe: O,
    run: ImportRun,
    /// The last delivery pass's outcome, once one has finished.
    last_pass: Option<Outcome>,
    tick: Duration,
}

impl<C, O> Importer<C, O>
where
    C: FnMut(Agent, SessionSource) -> Result<Captured>,
    O: FnMut(&ImportRun),
{
    fn run(
        mut self,
        listed: Vec<(Agent, Result<Vec<PastSession>>)>,
        drain: Drain,
    ) -> Result<ImportRun> {
        let mut dated = Vec::new();
        for (agent, sessions) in listed {
            let mut progress = ImportProgress::default();
            match sessions {
                Ok(sessions) => {
                    progress.total = sessions.len();
                    progress.finished = sessions.is_empty();
                    dated.extend(
                        sessions
                            .into_iter()
                            .map(|session| (session.last_active, (agent, session))),
                    );
                }
                Err(error) => {
                    progress.error = Some(format!("{error:#}"));
                    progress.finished = true;
                }
            }
            self.run.agents.push((agent, progress));
        }
        let plan = windows::bucket(dated, Utc::now(), windows::local_zone());
        self.run.windows = plan
            .iter()
            .map(|window| WindowProgress {
                label: window.label.clone(),
                status: WindowStatus::Waiting,
                sessions: window.items.len(),
                done: 0,
                turns: 0,
                queued: HashSet::new(),
                read: Vec::new(),
            })
            .collect();
        self.refresh()?;
        (self.observe)(&self.run);

        for (index, window) in plan.into_iter().enumerate() {
            self.read_window(index, window.items)?;
        }
        self.drain(drain)?;
        Ok(self.run.clone())
    }

    /// Reads every session of window `index`, then hands its turns to the courier.
    fn read_window(&mut self, index: usize, sessions: Vec<(Agent, PastSession)>) -> Result<()> {
        self.run.windows[index].status = WindowStatus::Reading;
        (self.observe)(&self.run);
        for (agent, session) in sessions {
            self.read_session(index, agent, session);
            self.collect_deliveries()?;
            (self.observe)(&self.run);
        }
        self.run.windows[index].status = WindowStatus::Sending;
        self.courier.nudge();
        self.refresh()?;
        (self.observe)(&self.run);
        Ok(())
    }

    fn read_session(&mut self, index: usize, agent: Agent, session: PastSession) {
        let id = agent.spec().id;
        let PastSession {
            key,
            last_active,
            source,
        } = session;
        if self.ledger.is_done(id, &key, last_active) {
            self.run.windows[index].read.push((agent, key, last_active));
        } else {
            match (self.capture)(agent, source) {
                Ok(captured) => {
                    let queued = captured.queued.len();
                    let progress = self.run.progress(agent);
                    progress.turns += queued;
                    progress.sessions += usize::from(queued > 0);
                    let window = &mut self.run.windows[index];
                    window.turns += queued;
                    window.queued.extend(captured.queued);
                    window.read.push((agent, key, last_active));
                }
                Err(_) => self.run.progress(agent).failed += 1,
            }
        }
        self.run.windows[index].done += 1;
        let progress = self.run.progress(agent);
        progress.done += 1;
        progress.finished = progress.done >= progress.total;
    }

    /// Takes in the delivery passes that finished, if any, and what they changed.
    fn collect_deliveries(&mut self) -> Result<()> {
        let finished = self.courier.finished();
        if finished.is_empty() {
            return Ok(());
        }
        for outcome in finished {
            self.take(outcome);
        }
        self.refresh()
    }

    /// Takes in a finished delivery pass.
    fn take(&mut self, outcome: Outcome) {
        if let Ok(pass) = &outcome {
            self.run.delivered += pass.delivered;
        }
        self.last_pass = Some(outcome);
    }

    /// Rereads the queue: windows whose turns have all left it are done (and recorded as such), and the
    /// backlog is recounted.
    fn refresh(&mut self) -> Result<()> {
        let queued = queue::queued_ids_at(&self.root)?;
        let mut finished = Vec::new();
        for window in &mut self.run.windows {
            if window.status == WindowStatus::Sending && window.queued.is_disjoint(&queued) {
                window.status = WindowStatus::Done;
                finished.append(&mut window.read);
            }
        }
        if !finished.is_empty() {
            self.ledger.record(
                finished
                    .iter()
                    .map(|(agent, key, at)| (agent.spec().id, key.as_str(), *at)),
            )?;
        }
        self.run.backlog = backlog::backlog_at(&self.root)?;
        Ok(())
    }

    /// Delivers what is left once every window is read, as `policy` says.
    fn drain(&mut self, policy: Drain) -> Result<()> {
        loop {
            if !self.courier.idle() {
                if let Some(outcome) = self.courier.wait(self.tick) {
                    self.take(outcome);
                    self.refresh()?;
                }
                self.report_sending();
                continue;
            }
            self.refresh()?;
            if self.run.backlog.pending() == 0 {
                self.run.sending = None;
                (self.observe)(&self.run);
                return Ok(());
            }
            let blocked = match &self.last_pass {
                Some(Ok(pass)) => pass.blocked.clone(),
                _ => None,
            };
            self.report_sending();
            if blocked.is_some() {
                return Ok(());
            }
            // Ask again at once only after a pass that got somewhere: one that failed (another
            // delivery held the queue, say) is tried again a tick later.
            let progressed =
                matches!(&self.last_pass, Some(Ok(pass)) if pass.delivered + pass.set_aside > 0);
            if self.run.backlog.due > 0 && (progressed || self.last_pass.is_none()) {
                self.courier.nudge();
                continue;
            }
            // Nothing more to send now: what is left waits on a retry, or the last pass got nowhere.
            if policy == Drain::WhileDue {
                return Ok(());
            }
            std::thread::sleep(self.tick);
            if self.run.backlog.due > 0 || self.retry_due() {
                self.courier.nudge();
            }
        }
    }

    fn retry_due(&self) -> bool {
        self.run
            .backlog
            .next_attempt_at
            .is_some_and(|at| at <= Utc::now())
    }

    fn report_sending(&mut self) {
        let blocked = match &self.last_pass {
            Some(Ok(pass)) => pass.blocked.clone(),
            _ => None,
        };
        let next_attempt_in = self
            .run
            .backlog
            .next_attempt_at
            .filter(|_| self.courier.idle() && self.run.backlog.due == 0)
            .map(|at| (at - Utc::now()).to_std().unwrap_or_default());
        self.run.sending = Some(Sending {
            next_attempt_in,
            blocked,
        });
        (self.observe)(&self.run);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::integration::queue::{QueueDestination, QueueRecord};
    use crate::commands::integration::sync::Pass;
    use nasiko_types::{
        CODING_AGENT_EVENT_VERSION, CapturePolicy, CodingAgentEventV1, CodingAgentSession,
        CodingAgentSource, CodingAgentTurn, coding_agent_event_id, coding_agent_session_id,
    };
    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    const FAST_TICK: Duration = Duration::from_millis(5);

    fn event(session: &str, turn: &str) -> CodingAgentEventV1 {
        let at = Utc::now();
        CodingAgentEventV1 {
            version: CODING_AGENT_EVENT_VERSION,
            event_id: coding_agent_event_id("claude", session, turn),
            captured_at: at,
            source: CodingAgentSource {
                agent_id: "claude".into(),
                agent_name: "claude-code".into(),
            },
            session: CodingAgentSession {
                id: coding_agent_session_id("claude", session),
                source_id: session.into(),
                title: None,
                parent: None,
            },
            turn: CodingAgentTurn {
                id: turn.into(),
                prompt: None,
                response: None,
                started_at: at,
                ended_at: at,
                llm_calls: vec![],
                tool_calls: vec![],
            },
            capture_policy: CapturePolicy::MetadataOnly,
        }
    }

    /// A session last active `hours` before `now` whose "transcript" is its key.
    fn session(key: &str, hours: i64, now: DateTime<Utc>) -> PastSession {
        PastSession {
            key: key.into(),
            last_active: now - chrono::Duration::hours(hours),
            source: SessionSource::Payload(key.into()),
        }
    }

    /// Queues one turn per session into `root`, as a capture would.
    fn capture_into(
        root: &Path,
        reads: Rc<RefCell<Vec<String>>>,
    ) -> impl FnMut(Agent, SessionSource) -> Result<Captured> {
        let root = root.to_path_buf();
        move |_, source| {
            let SessionSource::Payload(key) = source else {
                unreachable!()
            };
            reads.borrow_mut().push(key.clone());
            let record = QueueRecord::new(
                QueueDestination {
                    cluster_name: "c".into(),
                    cluster_url: "https://c.example".into(),
                    principal_id: uuid::Uuid::nil(),
                },
                event(&key, "t"),
            );
            queue::enqueue_at(&root, &record)?;
            Ok(Captured {
                pending: 1,
                queued: vec![record.event.event_id],
                rejected: 0,
            })
        }
    }

    /// A courier whose server accepts everything when `up`, and is unreachable otherwise.
    fn courier(root: &Path, up: bool) -> Courier {
        let root = root.to_path_buf();
        Courier::start(move || {
            if !up {
                return Ok(Pass::default());
            }
            let files = queue::record_files(&root.join("queue"))?;
            for file in &files {
                std::fs::remove_file(file)?;
            }
            Ok(Pass {
                delivered: files.len(),
                ..Pass::default()
            })
        })
    }

    fn importer<'a>(
        root: &Path,
        up: bool,
        reads: Rc<RefCell<Vec<String>>>,
        seen: &'a RefCell<Vec<ImportRun>>,
    ) -> Importer<impl FnMut(Agent, SessionSource) -> Result<Captured>, impl FnMut(&ImportRun) + 'a>
    {
        Importer {
            root: root.to_path_buf(),
            ledger: Ledger::load_at(root),
            courier: courier(root, up),
            capture: capture_into(root, reads),
            observe: move |run: &ImportRun| seen.borrow_mut().push(run.clone()),
            run: ImportRun::default(),
            last_pass: None,
            tick: FAST_TICK,
        }
    }

    fn sessions(now: DateTime<Utc>) -> Vec<(Agent, Result<Vec<PastSession>>)> {
        vec![(
            Agent::Claude,
            Ok(vec![
                session("old", 24 * 40, now),
                session("new", 1, now),
                session("yesterday", 30, now),
            ]),
        )]
    }

    #[test]
    fn windows_are_read_newest_first_and_done_once_the_server_has_their_turns() {
        let dir = tempfile::tempdir().unwrap();
        let reads = Rc::new(RefCell::new(Vec::new()));
        let seen = RefCell::new(Vec::new());

        let run = importer(dir.path(), true, reads.clone(), &seen)
            .run(sessions(Utc::now()), Drain::UntilEmpty)
            .unwrap();

        assert_eq!(*reads.borrow(), ["new", "yesterday", "old"]);
        assert!(run.delivered());
        assert!(run.sending.is_none());
        assert_eq!(run.delivered, 3, "every turn the server took is counted");
        assert_eq!((run.windows_read(), run.windows_done()), (3, 3));
        let progress = &run.agents[0].1;
        assert_eq!((progress.done, progress.total, progress.turns), (3, 3, 3));
        assert!(progress.finished);
        // No window was done before it was read and its turn delivered.
        for snapshot in seen.borrow().iter() {
            for window in &snapshot.windows {
                if window.status == WindowStatus::Done {
                    assert!(window.queued.iter().all(|id| {
                        !snapshot.backlog.agents.contains_key("claude")
                            || !queue::queued_ids_at(dir.path()).unwrap().contains(id)
                    }));
                }
            }
        }
    }

    #[test]
    fn a_window_is_not_done_while_its_turns_wait_in_the_queue() {
        let dir = tempfile::tempdir().unwrap();
        let reads = Rc::new(RefCell::new(Vec::new()));
        let seen = RefCell::new(Vec::new());

        let run = importer(dir.path(), false, reads, &seen)
            .run(sessions(Utc::now()), Drain::WhileDue)
            .unwrap();

        assert!(
            run.windows
                .iter()
                .all(|window| window.status == WindowStatus::Sending)
        );
        assert_eq!(run.backlog.pending(), 3);
        assert!(!run.delivered());
        assert_eq!(run.delivered, 0);
        assert_eq!((run.windows_read(), run.windows_done()), (3, 0));
        assert!(run.sending.is_some());
        assert!(
            seen.borrow()
                .iter()
                .flat_map(|run| &run.windows)
                .all(|window| window.status != WindowStatus::Done)
        );
    }

    #[test]
    fn a_resumed_import_skips_windows_it_finished() {
        let dir = tempfile::tempdir().unwrap();
        let reads = Rc::new(RefCell::new(Vec::new()));
        let seen = RefCell::new(Vec::new());
        let now = Utc::now();
        importer(dir.path(), true, reads.clone(), &seen)
            .run(sessions(now), Drain::UntilEmpty)
            .unwrap();
        reads.borrow_mut().clear();

        // The same sessions, plus one new since.
        let mut again = sessions(now);
        if let Ok(list) = &mut again[0].1 {
            list.push(session("fresh", 0, now));
        }
        let run = importer(dir.path(), true, reads.clone(), &seen)
            .run(again, Drain::UntilEmpty)
            .unwrap();

        assert_eq!(*reads.borrow(), ["fresh"]);
        assert_eq!(run.agents[0].1.done, 4);
        assert!(run.delivered());
    }

    #[test]
    fn a_store_that_cannot_be_read_fails_that_agent_alone() {
        let dir = tempfile::tempdir().unwrap();
        let reads = Rc::new(RefCell::new(Vec::new()));
        let seen = RefCell::new(Vec::new());
        let mut listed = sessions(Utc::now());
        listed.push((Agent::Cursor, Err(anyhow::anyhow!("locked"))));

        let run = importer(dir.path(), true, reads, &seen)
            .run(listed, Drain::UntilEmpty)
            .unwrap();

        assert_eq!(run.agents[1].1.error.as_deref(), Some("locked"));
        assert!(run.agents[1].1.finished);
        assert_eq!(run.agents[0].1.turns, 3);
    }

    /// A sign-in whose subject is the nil user, for the cluster at `url`.
    fn signed_in(url: &str) -> crate::config::Config {
        use base64::Engine as _;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::json!({"sub": uuid::Uuid::nil().to_string(), "exp": 4_102_444_800_i64})
                .to_string(),
        );
        crate::config::Config {
            active: Some("c".into()),
            clusters: std::collections::HashMap::from([(
                "c".into(),
                crate::config::ClusterEntry {
                    url: url.into(),
                    username: None,
                    token: Some(format!("header.{payload}.signature")),
                },
            )]),
            registry_url: None,
        }
    }

    /// A server that accepts every event, noting each event id it is sent.
    fn accepting_server(sent: Arc<Mutex<Vec<String>>>) -> (mockito::ServerGuard, mockito::Mock) {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/api/telemetry/coding-agent/events/batch")
            .with_status(200)
            .with_header("content-type", "application/json")
            .expect_at_least(1)
            .with_body_from_request(move |request| {
                let body: serde_json::Value =
                    serde_json::from_slice(request.body().unwrap()).unwrap();
                let ids: Vec<String> = body["events"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|event| event["event_id"].as_str().unwrap().to_string())
                    .collect();
                sent.lock().unwrap().extend(ids.iter().cloned());
                let results: Vec<serde_json::Value> = ids
                    .iter()
                    .map(|id| serde_json::json!({"event_id": id, "status": "accepted"}))
                    .collect();
                serde_json::json!({"data": {"results": results}, "status_code": 200})
                    .to_string()
                    .into_bytes()
            })
            .create();
        (server, mock)
    }

    /// Captures a session's one turn for the server at `url` unless `captured` (the watermarks a real
    /// capture keeps) says it already was.
    fn capture_once(
        root: &Path,
        url: &str,
        captured: Rc<RefCell<HashSet<String>>>,
    ) -> impl FnMut(Agent, SessionSource) -> Result<Captured> {
        let root = root.to_path_buf();
        let url = url.to_string();
        move |_, source| {
            let SessionSource::Payload(key) = source else {
                unreachable!()
            };
            if !captured.borrow_mut().insert(key.clone()) {
                return Ok(Captured::default());
            }
            let record = QueueRecord::new(
                QueueDestination {
                    cluster_name: "c".into(),
                    cluster_url: url.clone(),
                    principal_id: uuid::Uuid::nil(),
                },
                event(&key, "t"),
            );
            queue::enqueue_at(&root, &record)?;
            Ok(Captured {
                pending: 1,
                queued: vec![record.event.event_id],
                rejected: 0,
            })
        }
    }

    /// An importer on the queue under `root` delivering to the server at `url` the way a real one does
    /// ([`sync::deliver_at`]), whose `observe` is `observe`.
    fn delivering_importer<O: FnMut(&ImportRun)>(
        root: &Path,
        url: &str,
        captured: Rc<RefCell<HashSet<String>>>,
        observe: O,
    ) -> Importer<impl FnMut(Agent, SessionSource) -> Result<Captured>, O> {
        delivering_importer_with(root, url, captured, observe, Backoff::Honour)
    }

    fn delivering_importer_with<O: FnMut(&ImportRun)>(
        root: &Path,
        url: &str,
        captured: Rc<RefCell<HashSet<String>>>,
        observe: O,
        backoff: Backoff,
    ) -> Importer<impl FnMut(Agent, SessionSource) -> Result<Captured>, O> {
        let capture = capture_once(root, url, captured);
        delivering_importer_capturing(root, url, capture, observe, backoff)
    }

    fn delivering_importer_capturing<C, O>(
        root: &Path,
        url: &str,
        capture: C,
        observe: O,
        backoff: Backoff,
    ) -> Importer<C, O>
    where
        C: FnMut(Agent, SessionSource) -> Result<Captured>,
        O: FnMut(&ImportRun),
    {
        let config = signed_in(url);
        let queue_root = root.to_path_buf();
        Importer {
            root: root.to_path_buf(),
            ledger: Ledger::load_at(root),
            courier: paced_courier(backoff, move |pace, backoff| {
                sync::deliver_at(&queue_root, || Ok(config.clone()), pace, backoff)
            }),
            capture,
            observe,
            run: ImportRun::default(),
            last_pass: None,
            tick: FAST_TICK,
        }
    }

    #[test]
    fn an_import_killed_part_way_resumes_and_sends_no_turn_twice() {
        let dir = tempfile::tempdir().unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let (server, mock) = accepting_server(sent.clone());
        let captured = Rc::new(RefCell::new(HashSet::new()));
        let now = Utc::now();

        // Killed as soon as the newest window is done (the next is being read or sent). Delivery runs
        // on the courier's thread, so on a slow machine every window could be read before the newest
        // is done: each capture first waits for the newest turn to leave the queue, which makes the
        // window's end mark it done before the oldest window is read.
        let root = dir.path().to_path_buf();
        let url = server.url();
        let newest = coding_agent_event_id("claude", "new", "t");
        let mut capture = capture_once(dir.path(), &url, captured.clone());
        let after_newest_delivered = move |agent, source| {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            while queue::queued_ids_at(&root).unwrap().contains(&newest) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the newest turn was never delivered"
                );
                std::thread::sleep(FAST_TICK);
            }
            capture(agent, source)
        };
        let killed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            delivering_importer_capturing(
                dir.path(),
                &url,
                after_newest_delivered,
                |run| {
                    if run.windows[0].status == WindowStatus::Done {
                        panic!("killed after the first window");
                    }
                },
                Backoff::Honour,
            )
            .run(sessions(now), Drain::UntilEmpty)
        }));
        assert!(killed.is_err());
        assert!(
            !captured.borrow().contains("old"),
            "the oldest window was never reached"
        );
        assert!(Ledger::load_at(dir.path()).is_done(
            "claude",
            "new",
            now - chrono::Duration::hours(1)
        ));

        let run = delivering_importer(dir.path(), &server.url(), captured, |_| {})
            .run(sessions(now), Drain::UntilEmpty)
            .unwrap();

        assert!(run.delivered());
        let mut sent = sent.lock().unwrap().clone();
        let total = sent.len();
        sent.sort();
        sent.dedup();
        assert_eq!(total, sent.len(), "no event id was sent twice");
        let expected: HashSet<String> = ["new", "yesterday", "old"]
            .into_iter()
            .map(|key| coding_agent_event_id("claude", key, "t"))
            .collect();
        assert_eq!(sent.into_iter().collect::<HashSet<_>>(), expected);
        mock.assert();
    }

    /// A turn queued for `url` that failed before and waits on its retry for another half hour.
    fn waiting_on_a_retry(root: &Path, url: &str) -> PathBuf {
        let mut record = QueueRecord::new(
            QueueDestination {
                cluster_name: "c".into(),
                cluster_url: url.into(),
                principal_id: uuid::Uuid::nil(),
            },
            event("waiting", "t"),
        );
        record.attempts = 4;
        record.next_attempt_at = Some(Utc::now() + chrono::Duration::minutes(30));
        queue::enqueue_at(root, &record).unwrap()
    }

    #[test]
    fn delivering_now_sends_a_turn_waiting_on_a_retry_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let (server, mock) = accepting_server(sent.clone());
        let path = waiting_on_a_retry(dir.path(), &server.url());
        let captured = Rc::new(RefCell::new(HashSet::new()));
        let url = server.url();

        let mut now = delivering_importer_with(dir.path(), &url, captured, |_| {}, Backoff::Ignore);
        now.courier.nudge();
        let run = now.run(Vec::new(), Drain::WhileDue).unwrap();

        mock.assert();
        assert!(!path.exists());
        assert_eq!(run.delivered, 1);
        assert_eq!(run.backlog.pending(), 0);
    }

    #[test]
    fn a_delivery_not_asked_for_by_the_user_waits_out_the_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/api/telemetry/coding-agent/events/batch")
            .expect(0)
            .create();
        let path = waiting_on_a_retry(dir.path(), &server.url());
        let captured = Rc::new(RefCell::new(HashSet::new()));

        let run = delivering_importer(dir.path(), &server.url(), captured, |_| {})
            .run(Vec::new(), Drain::WhileDue)
            .unwrap();

        mock.assert();
        assert!(path.exists());
        assert_eq!((run.delivered, run.backlog.pending()), (0, 1));
    }

    /// Reads this computer's real agent stores (read-only: nothing is captured, queued or written) and
    /// prints what an import would find, and how long listing and reading took. Run by hand: `cargo
    /// test -p nasiko real_stores -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the real coding-agent stores on this computer"]
    fn real_stores_parse_without_errors() {
        let started = Instant::now();
        let mut dated = Vec::new();
        for agent in Agent::ALL.iter().copied() {
            let listing = Instant::now();
            match agent.past_sessions() {
                Ok(sessions) => {
                    println!(
                        "{}: {} session(s) listed in {:?}",
                        agent.spec().id,
                        sessions.len(),
                        listing.elapsed()
                    );
                    dated.extend(sessions.into_iter().map(|s| (s.last_active, (agent, s))));
                }
                Err(error) => println!("{}: store unreadable: {error:#}", agent.spec().id),
            }
        }
        println!("listed everything in {:?}", started.elapsed());
        let mut totals: std::collections::BTreeMap<&str, (usize, usize, usize, usize)> =
            Default::default();
        // Subagent sessions per agent: (sessions, complete turns, linked to a spawning tool call).
        let mut subagents: std::collections::BTreeMap<&str, (usize, usize, usize)> =
            Default::default();
        for window in windows::bucket(dated, Utc::now(), windows::local_zone()) {
            let reading = Instant::now();
            let sessions = window.items.len();
            let mut turns_in_window = 0;
            for (agent, session) in window.items {
                let entry = totals.entry(agent.spec().id).or_default();
                match agent.past_snapshot(session.source) {
                    Ok(snapshot) => {
                        entry.0 += 1;
                        entry.2 += snapshot.turns.len();
                        let complete = snapshot
                            .turns
                            .iter()
                            .filter(|turn| !turn.is_empty() && turn.response.is_some())
                            .count();
                        entry.3 += complete;
                        turns_in_window += complete;
                        if let Some(parent) = &snapshot.parent {
                            let child = subagents.entry(agent.spec().id).or_default();
                            child.0 += 1;
                            child.1 += complete;
                            child.2 += usize::from(parent.tool_call_id.is_some());
                        }
                    }
                    Err(_) => entry.1 += 1,
                }
            }
            println!(
                "{}: {sessions} session(s), {turns_in_window} complete turn(s), read in {:?}",
                window.label,
                reading.elapsed()
            );
        }
        for (agent, (read, failed, turns, complete)) in totals {
            println!(
                "{agent}: {read} session(s) read, {failed} failed, {turns} turn(s), {complete} complete"
            );
        }
        for (agent, (sessions, complete, with_call)) in subagents {
            println!(
                "{agent}: {sessions} subagent session(s) ({with_call} linked to their spawning call), {complete} complete turn(s)"
            );
        }
        println!("listed and read everything in {:?}", started.elapsed());
    }
}
