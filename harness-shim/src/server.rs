//! HTTP surface and relay between the child's stdio and the single attached WebSocket client.
//!
//! Delivery is pull-based: the [`FrameBuffer`] is the only outbound queue. Whichever client is
//! current pops from it, so a takeover or a dead socket never strands frames in a per-client
//! channel. Without client-side acks the one in-flight frame is the floor of what can go wrong:
//! it may be delivered twice, delivered only to the replaced client, or (after a send timeout)
//! re-queued behind frames the new client already received.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::Result;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::buffer::{FrameBuffer, PushOutcome};
use crate::child::{self, Shutdown, Signalled, SpawnedChild, StdoutEnd};
use crate::control::{self, child_state};
use crate::handshake::{Handshake, Launch, Mode, NO_COMMAND_MESSAGE};
use crate::procstats::Sampler;
use crate::reaper::OwnChild;

const CLOSE_SERVICE_RESTART: u16 = 1012;
const CLOSE_UNSUPPORTED_DATA: u16 = 1003;
const CLOSE_INTERNAL_ERROR: u16 = 1011;
const CLOSE_GOING_AWAY: u16 = 1001;
const STDIN_QUEUE_DEPTH: usize = 256;

/// How often the shim pings the attached client. A send failure, or no Pong by the next tick,
/// detaches the client; its undelivered frames stay in the capped buffer for the next one.
pub const PING_INTERVAL: Duration = Duration::from_secs(20);
/// Time allowed for remaining stdout to drain after the child exits. Bounded because a
/// grandchild that inherited the stdout pipe can hold it open, so EOF may never come.
const STDOUT_DRAIN_WAIT: Duration = Duration::from_secs(2);
/// After stdout closes, how long to wait for the child to exit on its own before treating
/// the closed stdout as a dead protocol.
const STDOUT_EOF_GRACE: Duration = Duration::from_secs(2);
/// Time allowed for the attached client's Close frame to be written before the process exits.
const CLOSE_FLUSH_WAIT: Duration = Duration::from_secs(1);

const METHOD_INITIALIZE: &str = "initialize";

/// The environment variable carrying the shim's bearer ([`Config::token`]). Read by the binary,
/// set by the supervisor, and kept out of the harness's environment.
pub const SHIM_TOKEN_ENV: &str = "HARNESS_SHIM_TOKEN";

/// Runtime settings for [`run`].
pub struct Config {
    pub listen: SocketAddr,
    pub buffer_lines: usize,
    /// The harness command. Required in env mode; in handshake mode a fallback for a `configure`
    /// that names none.
    pub argv: Option<Vec<String>>,
    /// When set, `/acp` and the control routes require `Authorization: Bearer <token>`. In
    /// handshake mode the first `configure` sets it when this is `None`.
    pub token: Option<String>,
    /// A repository to clone into the workspace before the harness starts (env mode only; a
    /// handshake shim clones what `configure` names).
    pub workspace: Option<crate::workspace::CloneSpec>,
    /// The session's working directory: where a handshake `configure`'s `workspace` block is
    /// cloned, the shim's own cwd, and the workspace `status` reports. Set by `--workspace` /
    /// `HARNESS_WORKSPACE` (`/workspace` in the image).
    pub workspace_mount: std::path::PathBuf,
    /// Files written before the harness starts (env mode only, from `HARNESS_FILES`; a
    /// handshake shim writes what `configure` names).
    pub files: Vec<crate::files::FileSpec>,
    /// Whether the session is configured by environment or by `POST /control/configure`.
    pub mode: Mode,
}

#[derive(Clone)]
struct Exit {
    code: u16,
    reason: String,
}

/// The child's answer to the first `initialize`, replayed to later clients.
#[derive(Default)]
struct InitializeCache {
    pending_id: Option<Value>,
    result: Option<Value>,
}

struct Inner {
    buffer: FrameBuffer,
    current: Option<u64>,
    exit: Option<Exit>,
    initialize: InitializeCache,
}

impl Inner {
    /// Buffers a frame, warning once when the buffer must grow past its limit.
    fn buffer_frame(&mut self, frame: String) {
        if let PushOutcome::OverLimit { first: true } = self.buffer.push(frame) {
            tracing::warn!("frame buffer over limit with only id-bearing frames; growing");
        }
    }

    /// Caches the child's response to the pending `initialize` when `line` is it.
    fn capture_initialize_response(&mut self, line: &str) {
        let Some(pending) = self.initialize.pending_id.as_ref() else {
            return;
        };
        let Ok(Value::Object(mut frame)) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if frame.get("id") != Some(pending) || frame.contains_key("method") {
            return;
        }
        self.initialize.pending_id = None;
        self.initialize.result = frame.remove("result");
    }
}

/// Answered by the supervisor once a restart has spawned the new child (its generation) or failed.
pub(crate) type RestartAck = oneshot::Sender<std::result::Result<u64, String>>;

/// State shared by the stdout reader, the HTTP handlers and the supervisor.
///
/// One lock covers the buffer and the client slot, so popping, takeover and re-queueing can
/// never reorder frames.
pub(crate) struct Relay {
    inner: Mutex<Inner>,
    /// Fired whenever a client task should re-check the buffer, the client slot or the exit.
    wake: Notify,
    /// Fired by a client task once it has written the final Close frame.
    close_flushed: Notify,
    stdin: Mutex<mpsc::Sender<String>>,
    buffer_lines: usize,
    /// `POST /control/restart` queues an ack here; the supervisor replaces the child and answers it.
    restart_tx: mpsc::UnboundedSender<RestartAck>,
    next_client_id: AtomicU64,
    /// The bearer guarding `/acp` and the control routes; in handshake mode the first
    /// `configure` sets it.
    token: Mutex<Option<String>>,
    /// Why the workspace clone failed, shown to the user in each turn.
    workspace_notice: Mutex<Option<String>>,
    handshake: Handshake,
    sampler: Sampler,
}

enum Next {
    Frame(String),
    Idle,
    Replaced,
    Exited(Exit),
}

impl Relay {
    fn new(
        buffer_lines: usize,
        stdin: mpsc::Sender<String>,
        token: Option<String>,
        handshake: Handshake,
        restart_tx: mpsc::UnboundedSender<RestartAck>,
    ) -> Self {
        Self {
            inner: Mutex::new(Inner {
                buffer: FrameBuffer::new(buffer_lines),
                current: None,
                exit: None,
                initialize: InitializeCache::default(),
            }),
            wake: Notify::new(),
            close_flushed: Notify::new(),
            stdin: Mutex::new(stdin),
            buffer_lines,
            restart_tx,
            next_client_id: AtomicU64::new(1),
            token: Mutex::new(token),
            workspace_notice: Mutex::new(None),
            handshake,
            sampler: Sampler::default(),
        }
    }

    pub(crate) fn handshake(&self) -> &Handshake {
        &self.handshake
    }

    pub(crate) fn sampler(&self) -> &Sampler {
        &self.sampler
    }

    pub(crate) fn token(&self) -> Option<String> {
        self.token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Sets the bearer to `candidate` when none is set yet (the caller is then the first and
    /// needs no credentials); otherwise checks `headers` against the existing one.
    pub(crate) fn claim_or_authorize(&self, headers: &HeaderMap, candidate: &str) -> bool {
        let mut token = self
            .token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if token.is_none() {
            *token = Some(candidate.to_owned());
            return true;
        }
        drop(token);
        authorized(self, headers)
    }

    fn set_workspace_notice(&self, notice: Option<String>) {
        *self
            .workspace_notice
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = notice;
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The sender for the current child's stdin.
    fn stdin(&self) -> mpsc::Sender<String> {
        self.stdin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Starts over with a new child: its stdin replaces the old one, and everything the old child
    /// left behind (buffered frames, the cached `initialize` answer, a client attached to it) is
    /// dropped, since none of it means anything to the new one.
    fn reset_for_new_child(&self, stdin: mpsc::Sender<String>) {
        *self
            .stdin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = stdin;
        let mut inner = self.lock();
        inner.buffer = FrameBuffer::new(self.buffer_lines);
        inner.initialize = InitializeCache::default();
        inner.current = None;
        inner.exit = None;
        drop(inner);
        self.wake.notify_waiters();
    }

    /// Queues one child stdout line for whichever client is (or later becomes) current.
    fn on_child_line(&self, line: String) {
        let mut inner = self.lock();
        inner.capture_initialize_response(&line);
        inner.buffer_frame(line);
        drop(inner);
        self.wake.notify_waiters();
    }

    /// Makes a new client current; the previous one notices and closes itself with 1012.
    fn attach(&self) -> u64 {
        let id = self.next_client_id.fetch_add(1, Ordering::Relaxed);
        self.lock().current = Some(id);
        self.wake.notify_waiters();
        id
    }

    /// Releases the client slot if `id` still holds it. Undelivered frames stay buffered.
    fn detach(&self, id: u64) {
        let mut inner = self.lock();
        if inner.current == Some(id) {
            inner.current = None;
        }
    }

    /// What client `id` should do next: send a frame, wait, or finish.
    fn next_for(&self, id: u64) -> Next {
        let mut inner = self.lock();
        if inner.current != Some(id) {
            return Next::Replaced;
        }
        if let Some(frame) = inner.buffer.pop_front() {
            return Next::Frame(frame);
        }
        match &inner.exit {
            Some(exit) => Next::Exited(exit.clone()),
            None => Next::Idle,
        }
    }

    /// Returns a popped-but-undelivered frame to the head of the queue.
    fn return_frame(&self, frame: String) {
        self.lock().buffer.push_front(frame);
        self.wake.notify_waiters();
    }

    /// Decides what to do with a frame from the client: `Some` is forwarded to the child.
    ///
    /// A repeated `initialize` (a fresh client after a sandbox resume) is answered from the
    /// cached first response instead, because the child is already initialised.
    ///
    /// Known race: a second `initialize` that arrives before the first response is cached is
    /// forwarded too, and each client is answered by the child. Error responses are deliberately
    /// not cached, so a failed `initialize` is retried against the child.
    fn route_inbound(&self, text: String) -> Option<String> {
        self.announce_workspace_failure(&text);
        let Some(id) = initialize_request(&text) else {
            return Some(text);
        };
        let mut inner = self.lock();
        let Some(result) = inner.initialize.result.clone() else {
            inner.initialize.pending_id = Some(id);
            return Some(text);
        };
        tracing::debug!("replaying cached initialize response");
        let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
        inner.buffer_frame(reply.to_string());
        drop(inner);
        self.wake.notify_waiters();
        None
    }

    /// When `text` is a `session/prompt` and the workspace clone failed, queues an agent message
    /// for that session saying so, ahead of whatever the harness answers. The session id comes
    /// from the prompt itself, so the client routes the message into the turn that is starting.
    fn announce_workspace_failure(&self, text: &str) {
        let Some(notice) = self
            .workspace_notice
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        else {
            return;
        };
        let Some(session_id) = prompt_session_id(text) else {
            return;
        };
        let update = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": format!("{notice}\n\n")}
                }
            }
        });
        let mut inner = self.lock();
        inner.buffer_frame(update.to_string());
        drop(inner);
        self.wake.notify_waiters();
    }

    /// Records the child's exit. Returns whether a client is attached to be told.
    fn mark_exited(&self, code: u16, reason: &str) -> bool {
        let mut inner = self.lock();
        inner.exit = Some(Exit {
            code,
            reason: reason.to_owned(),
        });
        let attached = inner.current.is_some();
        drop(inner);
        self.wake.notify_waiters();
        attached
    }

    pub(crate) fn has_exited(&self) -> bool {
        self.lock().exit.is_some()
    }

    fn snapshot(&self) -> (bool, usize) {
        let inner = self.lock();
        (inner.current.is_some(), inner.buffer.len())
    }
}

/// The session id of a `session/prompt` request.
fn prompt_session_id(text: &str) -> Option<String> {
    if !text.contains("session/prompt") {
        return None;
    }
    let frame: Value = serde_json::from_str(text).ok()?;
    if frame.get("method")?.as_str()? != "session/prompt" {
        return None;
    }
    frame
        .pointer("/params/sessionId")?
        .as_str()
        .map(str::to_owned)
}

/// Returns the request id when `text` is an `initialize` request.
fn initialize_request(text: &str) -> Option<Value> {
    if !text.contains(METHOD_INITIALIZE) {
        return None;
    }
    let frame: Value = serde_json::from_str(text).ok()?;
    if frame.get("method")?.as_str()? != METHOD_INITIALIZE {
        return None;
    }
    frame.get("id").cloned()
}

/// Builds the router: `/healthz`, `/acp`, `POST /control/restart` and the control routes.
fn router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/acp", get(acp_upgrade))
        .route("/control/restart", post(restart_child))
        .merge(control::routes())
        .with_state(relay)
}

/// `POST /control/restart`: replace the harness child with a fresh one, same bearer as `/acp`.
/// For a turn that ended while the child was still blocked on something (an unanswered
/// permission request) and may not answer the next prompt: the next turn starts clean instead of
/// attaching to it. Answers only once the new child is spawned and the relay reset, so a client
/// connecting right after never lands on the old one: 200 `{"child_generation": n}`, or 500
/// `{"error": ..}` when the spawn failed (the child then shows as `exited`; a handshake shim keeps
/// serving). A child that is gone is started again the same way. 409 while no child has been
/// started yet, since there is nothing to replace.
async fn restart_child(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> Response {
    if !authorized(&relay, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !relay.handshake().is_spawned() {
        return (
            StatusCode::CONFLICT,
            axum::Json(json!({"error": "no harness child has been started yet"})),
        )
            .into_response();
    }
    let (ack, answer) = oneshot::channel();
    if relay.restart_tx.send(ack).is_err() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    match answer.await {
        Ok(Ok(generation)) => axum::Json(json!({"child_generation": generation})).into_response(),
        Ok(Err(reason)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({"error": reason})),
        )
            .into_response(),
        // The shim is shutting down.
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn healthz(State(relay): State<Arc<Relay>>) -> Response {
    let (attached, buffered) = relay.snapshot();
    let state = child_state(&relay);
    // A gone child fails the probe in env mode, where the shim is about to exit. A handshake
    // shim stays up for the server to reconfigure, so its actor is alive and healthy.
    if state == control::ChildState::Exited && relay.handshake().mode() == Mode::Env {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({"child": "exited"})),
        )
            .into_response();
    }
    if state == control::ChildState::Running {
        return axum::Json(json!({"child": "running", "attached": attached, "buffered": buffered}))
            .into_response();
    }
    axum::Json(json!({"child": state.as_str()})).into_response()
}

/// True when no token is configured or the request carries the matching bearer token.
pub(crate) fn authorized(relay: &Relay, headers: &HeaderMap) -> bool {
    let Some(token) = relay.token() else {
        return true;
    };
    bearer_matches(headers, &token)
}

/// True when `headers` carry `Authorization: Bearer <token>`, compared in constant time.
pub(crate) fn bearer_matches(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|presented| constant_time_eq(presented.as_bytes(), token.as_bytes()))
}

/// Equality that takes the same time wherever the inputs first differ, so the bearer cannot be
/// guessed a byte at a time. Only the length is allowed to leak.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

async fn acp_upgrade(
    State(relay): State<Arc<Relay>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !authorized(&relay, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| serve_client(relay, socket))
}

fn close_message(code: u16, reason: String) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.into(),
    }))
}

/// Sends `message`, treating a stall longer than [`PING_INTERVAL`] as a dead socket.
async fn send(socket: &mut WebSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(PING_INTERVAL, socket.send(message)).await,
        Ok(Ok(()))
    )
}

/// Whether a client task keeps going after a step.
enum Step {
    Continue,
    End,
}

/// Drives one WebSocket client until it disconnects, is replaced, or the child exits.
async fn serve_client(relay: Arc<Relay>, mut socket: WebSocket) {
    let id = relay.attach();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await;
    let mut ping_outstanding = false;
    loop {
        let notified = relay.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if let Step::End = pump(&relay, id, &mut socket).await {
            break;
        }
        let step = tokio::select! {
            () = &mut notified => Step::Continue,
            incoming = socket.recv() => on_incoming(&relay, &mut socket, incoming, &mut ping_outstanding).await,
            _ = ping.tick() => on_ping_tick(&mut socket, &mut ping_outstanding).await,
        };
        if let Step::End = step {
            break;
        }
    }
    relay.detach(id);
}

/// Sends every frame that is ready for client `id`, then reports whether to keep going.
async fn pump(relay: &Relay, id: u64, socket: &mut WebSocket) -> Step {
    loop {
        match relay.next_for(id) {
            Next::Idle => return Step::Continue,
            Next::Frame(frame) => {
                if !send(socket, Message::Text(frame.clone().into())).await {
                    relay.return_frame(frame);
                    return Step::End;
                }
            }
            Next::Replaced => {
                send(
                    socket,
                    close_message(CLOSE_SERVICE_RESTART, "service restart".into()),
                )
                .await;
                return Step::End;
            }
            Next::Exited(exit) => {
                send(socket, close_message(exit.code, exit.reason)).await;
                relay.close_flushed.notify_one();
                return Step::End;
            }
        }
    }
}

/// Handles one message (or the end) from the client side of the socket.
async fn on_incoming(
    relay: &Relay,
    socket: &mut WebSocket,
    incoming: Option<Result<Message, axum::Error>>,
    ping_outstanding: &mut bool,
) -> Step {
    match incoming {
        Some(Ok(Message::Text(text))) => {
            forward_to_child(relay, socket, text.as_str().to_owned()).await
        }
        Some(Ok(Message::Binary(_))) => {
            send(
                socket,
                close_message(CLOSE_UNSUPPORTED_DATA, "binary frames unsupported".into()),
            )
            .await;
            Step::End
        }
        Some(Ok(Message::Pong(_))) => {
            *ping_outstanding = false;
            Step::Continue
        }
        Some(Ok(Message::Close(_))) => Step::End,
        Some(Ok(_)) => Step::Continue,
        Some(Err(_)) | None => Step::End,
    }
}

/// Writes a client frame to the child's stdin, unless the shim answers it itself.
async fn forward_to_child(relay: &Relay, socket: &mut WebSocket, text: String) -> Step {
    let Some(forward) = relay.route_inbound(text) else {
        return Step::Continue;
    };
    if relay.stdin().send(forward).await.is_err() {
        send(
            socket,
            close_message(CLOSE_INTERNAL_ERROR, "harness stdin closed".into()),
        )
        .await;
        return Step::End;
    }
    Step::Continue
}

/// Pings the client; a missing Pong since the last tick, or a failed send, means it is gone.
async fn on_ping_tick(socket: &mut WebSocket, ping_outstanding: &mut bool) -> Step {
    if *ping_outstanding || !send(socket, Message::Ping(Bytes::new())).await {
        tracing::info!("client unresponsive, detaching");
        return Step::End;
    }
    *ping_outstanding = true;
    Step::Continue
}

/// Why the supervisor stopped.
#[derive(Clone, Copy)]
enum Stop {
    ChildExited(std::process::ExitStatus),
    StdoutClosed,
    Signalled(Signalled),
}

/// What the supervisor wants next.
enum Outcome {
    /// Asked to replace the child (`POST /control/restart`); the ack is answered once spawned.
    Restart(RestartAck),
    Stop(Stop),
}

/// What woke the supervisor first.
enum Event {
    Child(std::process::ExitStatus),
    Stdout(StdoutEnd),
    Signal(Signalled),
    Restart(RestartAck),
}

/// A running harness child and the task reading its stdout.
struct Live {
    process: OwnChild,
    reader: JoinHandle<StdoutEnd>,
    /// How many children the shim has started, this one included.
    generation: u64,
}

/// Waits for the child to exit, its stdout to die, a restart request or a shutdown signal.
async fn supervise(
    live: &mut Live,
    shutdown: &mut Shutdown,
    restarts: &mut mpsc::UnboundedReceiver<RestartAck>,
) -> Result<Outcome> {
    let event = tokio::select! {
        Some(ack) = restarts.recv() => Event::Restart(ack),
        status = live.process.wait() => Event::Child(status?),
        end = &mut live.reader => Event::Stdout(end.unwrap_or(StdoutEnd::Error)),
        signal = shutdown.recv() => Event::Signal(signal),
    };
    Ok(match event {
        Event::Child(status) => {
            let _ = tokio::time::timeout(STDOUT_DRAIN_WAIT, &mut live.reader).await;
            Outcome::Stop(Stop::ChildExited(status))
        }
        Event::Stdout(StdoutEnd::Eof) => {
            // Stdout closing alongside a normal exit is expected; only a live child is a fault.
            match tokio::time::timeout(STDOUT_EOF_GRACE, live.process.wait()).await {
                Ok(status) => Outcome::Stop(Stop::ChildExited(status?)),
                Err(_) => Outcome::Stop(Stop::StdoutClosed),
            }
        }
        Event::Stdout(_) => Outcome::Stop(Stop::StdoutClosed),
        // The signal path is covered only by the binary-level tests (a hermetic in-process test
        // would signal itself).
        Event::Signal(signal) => Outcome::Stop(Stop::Signalled(signal)),
        Event::Restart(ack) => Outcome::Restart(ack),
    })
}

/// Starts the harness for `launch` and attaches it to the relay, replacing any previous child.
fn start_child(relay: &Arc<Relay>, launch: &Launch) -> Result<Live> {
    let SpawnedChild {
        process,
        stdin,
        stdout,
    } = child::spawn(&launch.argv, &launch.env)?;
    let (stdin_tx, stdin_rx) = mpsc::channel(STDIN_QUEUE_DEPTH);
    tokio::spawn(child::write_stdin(stdin, stdin_rx));
    relay.reset_for_new_child(stdin_tx);
    let reader_relay = relay.clone();
    let reader = tokio::spawn(async move {
        child::read_stdout_lines(stdout, |line| reader_relay.on_child_line(line)).await
    });
    let generation = relay.handshake().mark_spawned(launch);
    Ok(Live {
        process,
        reader,
        generation,
    })
}

/// Clones the workspace (the harness must see it when it starts), writes the session's files,
/// then spawns from the configuration as it stands *after* the clone: a reconfigure that lands
/// during a long clone is honoured. A SIGTERM during the clone is only seen afterwards
/// (`evicting` is set late).
async fn start_from_config(relay: &Arc<Relay>) -> Result<Live> {
    let notice = match relay.handshake().workspace() {
        Some(spec) => crate::workspace::ensure_cloned(&spec).await,
        None => None,
    };
    relay.set_workspace_notice(notice);
    start_from_stored(relay)
}

/// Writes the stored configuration's files, then spawns the harness on it.
fn start_from_stored(relay: &Arc<Relay>) -> Result<Live> {
    let launch = relay
        .handshake()
        .launch()
        .ok_or_else(|| anyhow::anyhow!(NO_COMMAND_MESSAGE))?;
    crate::files::write_all(&relay.handshake().files(), &launch.home)?;
    start_child(relay, &launch)
}

/// Handshake mode: a failed (re)spawn must not take the shim down, since the server can
/// reconfigure it. The failure shows as `child: "exited"` and comes back as the reason; env mode
/// keeps its exit semantics (the error propagates).
fn settle(
    relay: &Relay,
    mode: Mode,
    started: Result<Live>,
) -> Result<std::result::Result<Live, String>> {
    match started {
        Ok(live) => Ok(Ok(live)),
        Err(err) if mode == Mode::Handshake => {
            tracing::error!(%err, "starting the harness failed; waiting for a new configure");
            relay.mark_exited(CLOSE_INTERNAL_ERROR, "harness failed to start");
            Ok(Err(err.to_string()))
        }
        Err(err) => Err(err),
    }
}

/// Answers a restart request with the new generation, or why the spawn failed.
fn answer(ack: Option<RestartAck>, started: &std::result::Result<Live, String>) {
    let Some(ack) = ack else {
        return;
    };
    let _ = ack.send(match started {
        Ok(live) => Ok(live.generation),
        Err(reason) => Err(reason.clone()),
    });
}

/// What woke a handshake shim that has no child.
struct Wake {
    /// A restart request waiting for its answer (none for a configure).
    ack: Option<RestartAck>,
}

/// Handshake mode with no child: waits for a configure, or a restart request once the child is
/// gone, then starts it. `None` when SIGINT asks the shim to quit; SIGTERM only flags eviction.
async fn start_when_configured(
    relay: &Arc<Relay>,
    shutdown: &mut Shutdown,
    restarts: &mut mpsc::UnboundedReceiver<RestartAck>,
) -> Option<(Result<Live>, Wake)> {
    loop {
        // Checked before waiting, so a request that came in while the supervisor was busy counts.
        if relay.handshake().take_respawn_wanted() {
            return Some((start_from_config(relay).await, Wake { ack: None }));
        }
        tokio::select! {
            () = relay.handshake().woken() => {}
            Some(ack) = restarts.recv() => {
                return Some((start_from_config(relay).await, Wake { ack: Some(ack) }));
            }
            signal = shutdown.recv() => match signal {
                Signalled::Term => relay.handshake().set_evicting(),
                Signalled::Int => return None,
            },
        }
    }
}

/// Replaces the running child with one started from the currently stored configuration.
async fn restart_child_process(
    relay: &Arc<Relay>,
    mut live: Live,
    mode: Mode,
) -> Result<std::result::Result<Live, String>> {
    child::terminate(&mut live.process, child::TERMINATE_GRACE).await;
    live.reader.abort();
    settle(relay, mode, start_from_stored(relay))
}

/// Runs the shim to completion and returns the process exit code.
///
/// `bound` receives the actual listen address once the socket is bound (for ephemeral ports).
/// In handshake mode the socket is served first and the harness starts only after `configure`;
/// SIGTERM there means eviction (the shim stays up until snapshotted) and a child that exits or
/// fails to start leaves the shim serving, `child: "exited"`, until the next configure.
pub async fn run(config: Config, bound: Option<oneshot::Sender<SocketAddr>>) -> Result<i32> {
    let mut shutdown = Shutdown::install()?;
    let mode = config.mode;
    let (restart_tx, mut restarts) = mpsc::unbounded_channel();
    // The closed channel is replaced by the real child's stdin when one starts.
    let relay = Arc::new(Relay::new(
        config.buffer_lines,
        mpsc::channel(1).0,
        config.token,
        Handshake::new(
            mode,
            config.argv,
            config.workspace,
            config.files,
            config.workspace_mount,
        ),
        restart_tx,
    ));
    // Env mode starts before binding, so the socket never answers for a missing child.
    let mut live = match mode {
        Mode::Env => Some(start_from_config(&relay).await?),
        Mode::Handshake => None,
    };

    let listener = TcpListener::bind(config.listen).await?;
    let addr = listener.local_addr()?;
    tracing::info!(%addr, mode = mode.as_str(), "harness shim listening");
    if let Some(bound) = bound {
        let _ = bound.send(addr);
    }
    // Detached on purpose: in env mode `/healthz` answers 503 until the process exits.
    tokio::spawn(axum::serve(listener, router(relay.clone())).into_future());

    loop {
        let Some(mut running) = live.take() else {
            let Some((started, wake)) =
                start_when_configured(&relay, &mut shutdown, &mut restarts).await
            else {
                let code = finish(&relay, None, Stop::Signalled(Signalled::Int)).await;
                return Ok(leave(code).await);
            };
            let settled = settle(&relay, mode, started)?;
            answer(wake.ack, &settled);
            live = settled.ok();
            continue;
        };
        match supervise(&mut running, &mut shutdown, &mut restarts).await? {
            Outcome::Restart(ack) => {
                tracing::info!("restart requested; replacing the harness");
                let settled = restart_child_process(&relay, running, mode).await?;
                answer(Some(ack), &settled);
                live = settled.ok();
            }
            Outcome::Stop(Stop::Signalled(Signalled::Term)) if mode == Mode::Handshake => {
                tracing::info!("SIGTERM received; staying up until snapshotted");
                relay.handshake().set_evicting();
                live = Some(running);
            }
            Outcome::Stop(stop) => {
                let code = finish(&relay, Some(&mut running.process), stop).await;
                // A grandchild may hold the old stdout open; its frames must not reach a later child.
                running.reader.abort();
                let child_gone = matches!(stop, Stop::ChildExited(_) | Stop::StdoutClosed);
                if mode == Mode::Env || !child_gone {
                    return Ok(leave(code).await);
                }
            }
        }
    }
}

/// The last step before the shim exits with `code`: stops whatever the session left running in
/// its tree (a backgrounded job, a daemonized server), so nothing outlives the session.
async fn leave(code: i32) -> i32 {
    crate::reaper::stop_tree(crate::reaper::TREE_STOP_GRACE).await;
    code
}

/// Winds down for `stop`: tells the client why, and returns the exit code.
async fn finish(relay: &Relay, process: Option<&mut OwnChild>, stop: Stop) -> i32 {
    let (code, close, reason) = match stop {
        Stop::ChildExited(status) => {
            tracing::info!(%status, "harness exited");
            (
                child::exit_code(status),
                CLOSE_INTERNAL_ERROR,
                status.to_string(),
            )
        }
        Stop::StdoutClosed => {
            tracing::error!("harness stdout closed while it is still running; terminating it");
            if let Some(process) = process {
                child::terminate(process, child::TERMINATE_GRACE).await;
            }
            (1, CLOSE_INTERNAL_ERROR, "harness stdout closed".to_owned())
        }
        Stop::Signalled(_) => {
            tracing::info!("shutdown signal received, terminating harness");
            // Exit 0 even when the grace period ended in SIGKILL: shutdown was requested.
            if let Some(process) = process {
                child::terminate(process, child::TERMINATE_GRACE).await;
            }
            (0, CLOSE_GOING_AWAY, "shutting down".to_owned())
        }
    };
    if relay.mark_exited(close, &reason) {
        let _ = tokio::time::timeout(CLOSE_FLUSH_WAIT, relay.close_flushed.notified()).await;
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay() -> Relay {
        Relay::new(
            100,
            mpsc::channel(1).0,
            None,
            Handshake::new(Mode::Env, None, None, Vec::new(), "/workspace".into()),
            mpsc::unbounded_channel().0,
        )
    }

    #[test]
    fn bearer_comparison_requires_the_same_bytes() {
        assert!(constant_time_eq(b"s3cret", b"s3cret"));
        assert!(!constant_time_eq(b"s3cret", b"s3creT"));
        assert!(!constant_time_eq(b"s3cret", b"s3cre"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn takeover_delivers_every_undelivered_frame_in_order() {
        let relay = relay();
        let first = relay.attach();
        // Client `first` stalls: it never pops while the child emits.
        for n in 0..20 {
            relay.on_child_line(format!(r#"{{"jsonrpc":"2.0","method":"n{n}"}}"#));
        }
        let second = relay.attach();
        assert!(matches!(relay.next_for(first), Next::Replaced));
        for n in 0..20 {
            let Next::Frame(frame) = relay.next_for(second) else {
                panic!("frame {n} missing");
            };
            assert!(frame.contains(&format!("n{n}\"")));
        }
        assert!(matches!(relay.next_for(second), Next::Idle));
    }

    #[test]
    fn returned_frame_goes_back_to_the_head() {
        let relay = relay();
        let id = relay.attach();
        relay.on_child_line(r#"{"id":1,"result":{}}"#.into());
        relay.on_child_line(r#"{"id":2,"result":{}}"#.into());
        let Next::Frame(head) = relay.next_for(id) else {
            panic!()
        };
        relay.return_frame(head.clone());
        let Next::Frame(again) = relay.next_for(id) else {
            panic!()
        };
        assert_eq!(head, again);
    }

    #[test]
    fn second_initialize_is_answered_from_cache() {
        let relay = relay();
        let id = relay.attach();
        let first = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        assert!(relay.route_inbound(first.into()).is_some());
        relay.on_child_line(r#"{"jsonrpc":"2.0","id":1,"result":{"v":1}}"#.into());
        let second = r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{}}"#;
        assert!(relay.route_inbound(second.into()).is_none());
        let Next::Frame(_original) = relay.next_for(id) else {
            panic!()
        };
        let Next::Frame(replay) = relay.next_for(id) else {
            panic!()
        };
        let replay: Value = serde_json::from_str(&replay).unwrap();
        assert_eq!(replay["id"], 7);
        assert_eq!(replay["result"], json!({"v": 1}));
    }
}
