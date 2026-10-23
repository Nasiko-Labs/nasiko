//! The ACP connection: a background task that owns one JSON-RPC connection to
//! an agent and drives it via a command channel.
//!
//! Why an actor? The SDK's `connect_with` runs a closure that owns the
//! connection for its whole lifetime and tears it down when the closure
//! returns. To hold a session open across many turns (so a session registry
//! can drive it later — Issue 4), that closure instead loops over a command
//! channel: each `new_session` / `prompt` / `cancel` call from the handle
//! becomes a command the loop executes against the live connection.
//!
//! One turn runs at a time per connection (the platform's one-active-turn rule),
//! which keeps update routing simple: streamed `session/update` notifications
//! are funneled through one channel and filtered by session id.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::component::ConnectTo;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    RequestPermissionRequest, RequestPermissionResponse, SessionId, SessionNotification,
    TextContent,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Responder};
use tokio::sync::{mpsc, oneshot};

use crate::capability::{NegotiatedCapabilities, RequiredCapabilities, negotiate};
use crate::error::AcpError;
use crate::permission::PermissionPolicy;
use crate::turn::{NewSessionParams, TurnEvent, TurnStream};

/// Holds the readiness signal until exactly one side sends it: the connection
/// task takes it to report `initialize`/negotiation, or — if the transport
/// never spawns — the task's error arm takes it to report the transport error.
type ReadySlot = Arc<Mutex<Option<oneshot::Sender<Result<NegotiatedCapabilities, AcpError>>>>>;

/// Capacity of the per-turn update and event channels. Big enough that a chatty
/// agent never blocks a consumer that is keeping up, bounded so a stalled one
/// can't grow memory without limit — an overflow is logged and shed, not stored.
const STREAM_CHANNEL_CAPACITY: usize = 256;

/// Capacity of the low-volume control channels (commands, cancels). At most one
/// command is in flight at a time in normal use (a turn holds the slot), so this
/// only guards against a pathological caller.
const CONTROL_CHANNEL_CAPACITY: usize = 64;

/// Configuration for opening a connection.
#[derive(Debug, Clone)]
pub struct AcpConnectConfig {
    /// What we require of the agent at `initialize`.
    pub required: RequiredCapabilities,
    /// How to answer the agent's permission requests.
    pub permission_policy: PermissionPolicy,
    /// Budget for the `initialize` handshake. If the agent doesn't respond within
    /// this window, [`connect`](AcpConnection::connect) fails with
    /// [`AcpError::Timeout`] instead of blocking forever — so a wedged agent
    /// still lets the caller fall back to the hook path.
    pub handshake_timeout: Duration,
    /// Budget for a single post-handshake request (`session/new`, `session/load`).
    /// A turn's own wall-clock is governed separately by the caller.
    pub request_timeout: Duration,
}

impl Default for AcpConnectConfig {
    fn default() -> Self {
        Self {
            required: RequiredCapabilities::default(),
            permission_policy: PermissionPolicy::default(),
            handshake_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(60),
        }
    }
}

/// A handle to a live ACP agent connection.
///
/// Cloneable is deliberately *not* derived: the handle owns the shutdown of the
/// background task on drop.
pub struct AcpConnection {
    commands: mpsc::Sender<Command>,
    /// Side channel for `session/cancel`, kept separate from `commands` so a
    /// cancel can reach the agent *during* a turn — the command loop is busy
    /// awaiting that turn and can't service a queued `Command` until it ends.
    cancels: mpsc::Sender<SessionId>,
    /// Set while a turn is in flight so a second `prompt` is rejected up front
    /// rather than silently queued behind it (one active turn per connection).
    turn_active: Arc<AtomicBool>,
    capabilities: NegotiatedCapabilities,
    task: tokio::task::JoinHandle<()>,
}

/// Commands the handle sends to the background task.
enum Command {
    NewSession {
        params: NewSessionParams,
        reply: oneshot::Sender<Result<SessionId, AcpError>>,
    },
    LoadSession {
        session_id: SessionId,
        params: NewSessionParams,
        reply: oneshot::Sender<Result<(), AcpError>>,
    },
    Prompt {
        session_id: SessionId,
        text: String,
        events: mpsc::Sender<TurnEvent>,
    },
    Shutdown,
}

impl AcpConnection {
    /// Connect over an already-built transport (a spawned child, or a byte
    /// stream from a container attach). Runs `initialize`, negotiates
    /// capabilities, and returns once the agent is ready.
    ///
    /// Returns [`AcpError::Incompatible`] if the agent can't speak our protocol
    /// — the caller's cue to fall back to the hook transport.
    pub async fn connect<T>(transport: T, config: AcpConnectConfig) -> Result<Self, AcpError>
    where
        T: ConnectTo<Client> + Send + 'static,
    {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(CONTROL_CHANNEL_CAPACITY);
        let (cancel_tx, cancel_rx) = mpsc::channel::<SessionId>(CONTROL_CHANNEL_CAPACITY);
        // Shared between the handle (claims it in `prompt`) and the command loop
        // (releases it when the turn ends).
        let turn_active = Arc::new(AtomicBool::new(false));
        let turn_active_task = turn_active.clone();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<NegotiatedCapabilities, AcpError>>();
        // Shared so the transport-error arm below can still report if the
        // connection task fails before the handshake ever runs.
        let ready_slot: ReadySlot = Arc::new(Mutex::new(Some(ready_tx)));
        let ready_for_run = ready_slot.clone();
        // Every streamed notification from the agent lands on this channel; the
        // command loop routes it to the active turn.
        let (update_tx, update_rx) = mpsc::channel::<SessionNotification>(STREAM_CHANNEL_CAPACITY);

        let required = config.required;
        let policy = config.permission_policy;
        let handshake_timeout = config.handshake_timeout;
        let request_timeout = config.request_timeout;

        let task = tokio::spawn(async move {
            let result = Client
                .builder()
                .on_receive_notification(
                    async move |notification: SessionNotification, _cx| {
                        // Never block the agent's event loop on a full channel:
                        // a closed receiver means the command loop ended (drop
                        // quietly), a full one means the consumer is behind — shed
                        // the update and log it rather than stall or grow memory.
                        match update_tx.try_send(notification) {
                            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
                            Err(mpsc::error::TrySendError::Full(_)) => tracing::warn!(
                                "acp update channel full; dropping a session/update (slow consumer)"
                            ),
                        }
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    async move |request: RequestPermissionRequest,
                                responder: Responder<RequestPermissionResponse>,
                                cx: ConnectionTo<Agent>| {
                        match policy.delegate() {
                            // Delegated (e.g. HITL): answer from a task spawned on
                            // the connection, so a wait on a human never stalls the
                            // event loop or the turn's streamed updates. The handler
                            // itself returns immediately.
                            Some(delegate) => {
                                let delegate = delegate.clone();
                                cx.spawn(async move {
                                    responder.respond(delegate.decide(&request).await)
                                })?;
                            }
                            // Fixed policy (reject/approve all): answer inline.
                            None => {
                                responder.respond(policy.decide_fixed(&request))?;
                            }
                        }
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |conn: ConnectionTo<Agent>| {
                    run_connection(
                        conn,
                        required,
                        handshake_timeout,
                        request_timeout,
                        ready_for_run,
                        cmd_rx,
                        update_rx,
                        cancel_rx,
                        turn_active_task,
                    )
                    .await;
                    Ok(())
                })
                .await;

            if let Err(e) = result {
                // If the handshake never ran (spawn/attach failure), the slot
                // still holds the sender — report the real transport cause so
                // the caller sees it instead of a bare `ConnectionClosed`.
                if let Some(tx) = ready_slot.lock().unwrap().take() {
                    let _ = tx.send(Err(AcpError::Transport(e.to_string())));
                } else {
                    tracing::warn!(error = %e, "acp connection ended after handshake");
                }
            }
        });

        match ready_rx.await {
            Ok(Ok(capabilities)) => Ok(AcpConnection {
                commands: cmd_tx,
                cancels: cancel_tx,
                turn_active,
                capabilities,
                task,
            }),
            Ok(Err(e)) => Err(e),
            // The task ended before signaling readiness (spawn/attach failure).
            Err(_) => Err(AcpError::ConnectionClosed),
        }
    }

    /// The capabilities the agent advertised at `initialize`.
    pub fn capabilities(&self) -> &NegotiatedCapabilities {
        &self.capabilities
    }

    /// Open a new session (`session/new`). Returns the agent's session id.
    pub async fn new_session(&self, params: NewSessionParams) -> Result<SessionId, AcpError> {
        ensure_absolute_cwd(&params.cwd)?;
        let (reply, reply_rx) = oneshot::channel();
        self.commands
            .send(Command::NewSession { params, reply })
            .await
            .map_err(|_| AcpError::ConnectionClosed)?;
        reply_rx.await.map_err(|_| AcpError::ConnectionClosed)?
    }

    /// Restore an existing session (`session/load`) by its agent-side id, so a
    /// cold turn resumes the agent's conversation state instead of starting over.
    ///
    /// Only valid when the agent advertised the `load_session` capability at
    /// `initialize` ([`capabilities`](Self::capabilities)); the caller checks that
    /// and falls back to [`new_session`](Self::new_session) otherwise. Reuses
    /// [`NewSessionParams`] because `session/load` takes the same `cwd` + MCP
    /// servers as `session/new`.
    pub async fn load_session(
        &self,
        session_id: SessionId,
        params: NewSessionParams,
    ) -> Result<(), AcpError> {
        ensure_absolute_cwd(&params.cwd)?;
        let (reply, reply_rx) = oneshot::channel();
        self.commands
            .send(Command::LoadSession {
                session_id,
                params,
                reply,
            })
            .await
            .map_err(|_| AcpError::ConnectionClosed)?;
        reply_rx.await.map_err(|_| AcpError::ConnectionClosed)?
    }

    /// Send a prompt (`session/prompt`) and return the stream of turn events.
    /// Only one turn runs at a time per connection.
    pub fn prompt(
        &self,
        session_id: SessionId,
        text: impl Into<String>,
    ) -> Result<TurnStream, AcpError> {
        // Claim the turn slot before sending, so a prompt issued while another
        // turn is live is rejected here instead of silently queued behind it. The
        // command loop releases the slot when the turn ends.
        if self
            .turn_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(AcpError::TurnInProgress);
        }
        let (events_tx, events_rx) = mpsc::channel::<TurnEvent>(STREAM_CHANNEL_CAPACITY);
        if self
            .commands
            .try_send(Command::Prompt {
                session_id,
                text: text.into(),
                events: events_tx,
            })
            .is_err()
        {
            // The task is gone and no turn will run; release the slot we claimed.
            self.turn_active.store(false, Ordering::Release);
            return Err(AcpError::ConnectionClosed);
        }
        Ok(TurnStream { events: events_rx })
    }

    /// Cancel the running turn for a session (`session/cancel`).
    ///
    /// Delivered on a side channel, not the command queue, so it reaches the agent
    /// *during* the turn it targets — the command loop is busy awaiting that turn
    /// and would not service a queued command until it ended. [`run_turn`] selects
    /// on this channel and sends `session/cancel`; the turn then resolves with a
    /// cancelled stop reason. A cancel with no matching live turn is dropped.
    pub fn cancel(&self, session_id: SessionId) -> Result<(), AcpError> {
        self.cancels
            .try_send(session_id)
            .map_err(|_| AcpError::ConnectionClosed)
    }

    /// Abort the background task without waiting (used on drop as a backstop).
    fn abort(&self) {
        self.task.abort();
    }
}

impl Drop for AcpConnection {
    fn drop(&mut self) {
        // Best-effort graceful shutdown; abort if the loop is already wedged.
        // Aborting the task drops the SDK connection future, which drops the
        // transport's `ChildGuard`; on a local-process transport (`AcpAgent`) that
        // guard sends SIGKILL to the whole child process group on drop, so the
        // agent subprocess and anything it spawned are reaped — not orphaned.
        let _ = self.commands.try_send(Command::Shutdown);
        self.abort();
    }
}

/// Convenience: spawn a local agent binary (e.g. `"opencode acp"`) as a child
/// process and connect to it over its stdio. Container transports call
/// [`AcpConnection::connect`] with a byte-stream transport instead.
pub async fn connect_local(
    command: &str,
    config: AcpConnectConfig,
) -> Result<AcpConnection, AcpError> {
    use std::str::FromStr;
    let agent = agent_client_protocol::AcpAgent::from_str(command)
        .map_err(|e| AcpError::Transport(e.to_string()))?;
    AcpConnection::connect(agent, config).await
}

/// The body of the connection task: initialize, negotiate, then serve commands
/// until the handle is dropped or the connection drops.
// The spawned task owns each of its channels and the handshake config outright;
// bundling them into a struct would only move the same fields behind one name.
#[allow(clippy::too_many_arguments)]
async fn run_connection(
    conn: ConnectionTo<Agent>,
    required: RequiredCapabilities,
    handshake_timeout: Duration,
    request_timeout: Duration,
    ready_slot: ReadySlot,
    mut cmd_rx: mpsc::Receiver<Command>,
    mut update_rx: mpsc::Receiver<SessionNotification>,
    mut cancel_rx: mpsc::Receiver<SessionId>,
    turn_active: Arc<AtomicBool>,
) {
    // Claim the readiness sender; from here on this task owns signaling, and the
    // transport-error arm in `connect` finds the slot empty.
    let ready = ready_slot.lock().unwrap().take();

    // Handshake, bounded so an agent that never answers `initialize` can't wedge
    // `connect()` forever (the caller needs the failure to fall back to the hook
    // path).
    let init_request = conn
        .send_request(InitializeRequest::new(required.protocol_version))
        .block_task();
    let init = match tokio::time::timeout(handshake_timeout, init_request).await {
        Ok(Ok(init)) => init,
        Ok(Err(e)) => {
            if let Some(tx) = ready {
                let _ = tx.send(Err(AcpError::from(e)));
            }
            return;
        }
        Err(_) => {
            if let Some(tx) = ready {
                let _ = tx.send(Err(AcpError::Timeout(format!(
                    "initialize did not complete within {}s",
                    handshake_timeout.as_secs()
                ))));
            }
            return;
        }
    };

    // Note: no ACP `authenticate` step. The agent may advertise `auth_methods` at
    // `initialize`, but Nasiko does not authenticate *to* the agent over ACP — the
    // agent authenticates to Nasiko's gateway/router out of band via the Layer B
    // session JWT (TECH-2026-004 §3.5, PRD ACP-1 "authenticate (no-op)"; OpenCode's
    // `authenticate` is itself a no-op). An agent that genuinely refuses to open a
    // session without it answers `session/new` with JSON-RPC `auth_required`, which
    // surfaces as `AcpError::is_auth_required()` so the caller can fall back.
    match negotiate(&init, &required) {
        Ok(caps) => {
            if let Some(tx) = ready
                && tx.send(Ok(caps)).is_err()
            {
                return; // caller gave up waiting
            }
        }
        Err(reason) => {
            tracing::warn!(%reason, "acp capability negotiation failed; caller should use hook path");
            if let Some(tx) = ready {
                let _ = tx.send(Err(AcpError::Incompatible(reason)));
            }
            return;
        }
    }

    // Command loop.
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Command::NewSession { params, reply } => {
                let request = NewSessionRequest::new(params.cwd).mcp_servers(params.mcp_servers);
                let call = conn.send_request(request).block_task();
                let result = match tokio::time::timeout(request_timeout, call).await {
                    Ok(inner) => inner.map(|resp| resp.session_id).map_err(AcpError::from),
                    Err(_) => Err(AcpError::Timeout(format!(
                        "session/new did not complete within {}s",
                        request_timeout.as_secs()
                    ))),
                };
                let _ = reply.send(result);
            }
            Command::LoadSession {
                session_id,
                params,
                reply,
            } => {
                // `session/load` replays the session's prior `session/update`s
                // before responding; those land on `update_rx` and are cleared by
                // the next `run_turn` (which drains stragglers), so the restore is
                // transparent to the caller.
                let request =
                    LoadSessionRequest::new(session_id, params.cwd).mcp_servers(params.mcp_servers);
                let call = conn.send_request(request).block_task();
                let result = match tokio::time::timeout(request_timeout, call).await {
                    Ok(inner) => inner.map(|_| ()).map_err(AcpError::from),
                    Err(_) => Err(AcpError::Timeout(format!(
                        "session/load did not complete within {}s",
                        request_timeout.as_secs()
                    ))),
                };
                let _ = reply.send(result);
            }
            Command::Prompt {
                session_id,
                text,
                events,
            } => {
                run_turn(
                    &conn,
                    &mut update_rx,
                    &mut cancel_rx,
                    &session_id,
                    &text,
                    &events,
                )
                .await;
                // Release the one-active-turn slot the handle claimed in `prompt`.
                turn_active.store(false, Ordering::Release);
            }
            Command::Shutdown => break,
        }
    }
}

/// Drive one prompt turn: send it, forward matching `session/update`s, and
/// finish on the prompt response (or a dropped connection).
async fn run_turn(
    conn: &ConnectionTo<Agent>,
    updates: &mut mpsc::Receiver<SessionNotification>,
    cancels: &mut mpsc::Receiver<SessionId>,
    session_id: &SessionId,
    text: &str,
    events: &mpsc::Sender<TurnEvent>,
) {
    // Clear any stragglers buffered while the connection was idle between turns —
    // e.g. the `available_commands_update` / mode updates an agent sends after
    // `session/new`, which belong to no turn's stream. Count and log them so a
    // surprising number is visible instead of silently swallowed.
    let mut discarded = 0usize;
    while updates.try_recv().is_ok() {
        discarded += 1;
    }
    if discarded > 0 {
        tracing::debug!(
            count = discarded,
            "acp: discarded buffered updates before turn"
        );
    }
    // Drop cancels left over from a prior turn so they can't abort this one before
    // it has a chance to run.
    while cancels.try_recv().is_ok() {}

    let blocks = vec![ContentBlock::Text(TextContent::new(text.to_string()))];
    let prompt = conn
        .send_request(PromptRequest::new(session_id.clone(), blocks))
        .block_task();
    tokio::pin!(prompt);

    // Once `session/cancel` is sent we stop watching for cancels and let the
    // prompt resolve — the agent still answers it, with a cancelled stop reason.
    let mut cancel_sent = false;
    // Cleared when the cancel channel closes (handle dropped), so the select arm
    // stops firing on an endlessly-ready closed receiver.
    let mut cancels_open = true;

    loop {
        tokio::select! {
            result = &mut prompt => {
                match result {
                    Ok(resp) => {
                        // Flush any updates that landed alongside the response.
                        while let Ok(n) = updates.try_recv() {
                            if &n.session_id == session_id {
                                let _ = events.send(TurnEvent::Update(Box::new(n.update))).await;
                            }
                        }
                        let _ = events.send(TurnEvent::Finished(resp.stop_reason)).await;
                    }
                    Err(e) => {
                        let _ = events.send(TurnEvent::Failed(AcpError::from(e))).await;
                    }
                }
                break;
            }
            maybe = updates.recv() => {
                match maybe {
                    Some(n) if &n.session_id == session_id => {
                        let _ = events.send(TurnEvent::Update(Box::new(n.update))).await;
                    }
                    Some(_) => {} // update for another session
                    None => {
                        let _ = events.send(TurnEvent::Failed(AcpError::ConnectionClosed)).await;
                        break;
                    }
                }
            }
            maybe_cancel = cancels.recv(), if cancels_open && !cancel_sent => {
                match maybe_cancel {
                    Some(sid) if &sid == session_id => {
                        // Ask the agent to end this turn early. It still resolves
                        // the prompt, so we keep looping until that response lands.
                        let _ = conn.send_notification(
                            agent_client_protocol::schema::v1::CancelNotification::new(
                                session_id.clone(),
                            ),
                        );
                        cancel_sent = true;
                    }
                    Some(_) => {} // cancel aimed at another session
                    None => cancels_open = false, // handle dropped; teardown imminent
                }
            }
        }
    }
}

/// Reject a non-absolute session `cwd` before it reaches the agent: the ACP
/// protocol requires an absolute path, and a relative one otherwise fails later
/// with a vaguer agent-side error.
fn ensure_absolute_cwd(cwd: &Path) -> Result<(), AcpError> {
    if cwd.is_absolute() {
        Ok(())
    } else {
        Err(AcpError::InvalidParams(format!(
            "session cwd must be an absolute path, got {}",
            cwd.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_cwd_is_accepted_relative_is_rejected() {
        assert!(ensure_absolute_cwd(Path::new("/abs/workspace")).is_ok());
        match ensure_absolute_cwd(Path::new("rel/workspace")) {
            Err(AcpError::InvalidParams(_)) => {}
            other => panic!("expected InvalidParams, got {other:?}"),
        }
    }
}
