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

use std::sync::{Arc, Mutex};

use agent_client_protocol::component::ConnectTo;
use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest, RequestPermissionRequest,
    SessionId, SessionNotification, TextContent,
};
use agent_client_protocol::{Agent, Client, ConnectionTo};
use tokio::sync::{mpsc, oneshot};

use crate::capability::{NegotiatedCapabilities, RequiredCapabilities, negotiate};
use crate::error::AcpError;
use crate::permission::PermissionPolicy;
use crate::turn::{NewSessionParams, TurnEvent, TurnStream};

/// Holds the readiness signal until exactly one side sends it: the connection
/// task takes it to report `initialize`/negotiation, or — if the transport
/// never spawns — the task's error arm takes it to report the transport error.
type ReadySlot = Arc<Mutex<Option<oneshot::Sender<Result<NegotiatedCapabilities, AcpError>>>>>;

/// Configuration for opening a connection.
#[derive(Debug, Clone, Default)]
pub struct AcpConnectConfig {
    /// What we require of the agent at `initialize`.
    pub required: RequiredCapabilities,
    /// How to answer the agent's permission requests.
    pub permission_policy: PermissionPolicy,
}

/// A handle to a live ACP agent connection.
///
/// Cloneable is deliberately *not* derived: the handle owns the shutdown of the
/// background task on drop.
pub struct AcpConnection {
    commands: mpsc::UnboundedSender<Command>,
    capabilities: NegotiatedCapabilities,
    task: tokio::task::JoinHandle<()>,
}

/// Commands the handle sends to the background task.
enum Command {
    NewSession {
        params: NewSessionParams,
        reply: oneshot::Sender<Result<SessionId, AcpError>>,
    },
    Prompt {
        session_id: SessionId,
        text: String,
        events: mpsc::UnboundedSender<TurnEvent>,
    },
    Cancel {
        session_id: SessionId,
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
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Command>();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<NegotiatedCapabilities, AcpError>>();
        // Shared so the transport-error arm below can still report if the
        // connection task fails before the handshake ever runs.
        let ready_slot: ReadySlot = Arc::new(Mutex::new(Some(ready_tx)));
        let ready_for_run = ready_slot.clone();
        // Every streamed notification from the agent lands on this channel; the
        // command loop routes it to the active turn.
        let (update_tx, update_rx) = mpsc::unbounded_channel::<SessionNotification>();

        let required = config.required;
        let policy = config.permission_policy;

        let task = tokio::spawn(async move {
            let result = Client
                .builder()
                .on_receive_notification(
                    async move |notification: SessionNotification, _cx| {
                        // Receiver gone == command loop ended; drop quietly.
                        let _ = update_tx.send(notification);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder, _cx| {
                        responder.respond(policy.decide(&request))
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |conn: ConnectionTo<Agent>| {
                    run_connection(conn, required, ready_for_run, cmd_rx, update_rx).await;
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
        let (reply, reply_rx) = oneshot::channel();
        self.commands
            .send(Command::NewSession { params, reply })
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
        let (events_tx, events_rx) = mpsc::unbounded_channel::<TurnEvent>();
        self.commands
            .send(Command::Prompt {
                session_id,
                text: text.into(),
                events: events_tx,
            })
            .map_err(|_| AcpError::ConnectionClosed)?;
        Ok(TurnStream { events: events_rx })
    }

    /// Cancel the running turn for a session (`session/cancel`).
    pub fn cancel(&self, session_id: SessionId) -> Result<(), AcpError> {
        self.commands
            .send(Command::Cancel { session_id })
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
        let _ = self.commands.send(Command::Shutdown);
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
async fn run_connection(
    conn: ConnectionTo<Agent>,
    required: RequiredCapabilities,
    ready_slot: ReadySlot,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
    mut update_rx: mpsc::UnboundedReceiver<SessionNotification>,
) {
    // Claim the readiness sender; from here on this task owns signaling, and the
    // transport-error arm in `connect` finds the slot empty.
    let ready = ready_slot.lock().unwrap().take();

    // Handshake.
    let init = match conn
        .send_request(InitializeRequest::new(required.protocol_version))
        .block_task()
        .await
    {
        Ok(init) => init,
        Err(e) => {
            if let Some(tx) = ready {
                let _ = tx.send(Err(AcpError::from(e)));
            }
            return;
        }
    };

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
                let result = conn
                    .send_request(request)
                    .block_task()
                    .await
                    .map(|resp| resp.session_id)
                    .map_err(AcpError::from);
                let _ = reply.send(result);
            }
            Command::Prompt {
                session_id,
                text,
                events,
            } => {
                run_turn(&conn, &mut update_rx, &session_id, &text, &events).await;
            }
            Command::Cancel { session_id } => {
                let _ = conn.send_notification(
                    agent_client_protocol::schema::v1::CancelNotification::new(session_id),
                );
            }
            Command::Shutdown => break,
        }
    }
}

/// Drive one prompt turn: send it, forward matching `session/update`s, and
/// finish on the prompt response (or a dropped connection).
async fn run_turn(
    conn: &ConnectionTo<Agent>,
    updates: &mut mpsc::UnboundedReceiver<SessionNotification>,
    session_id: &SessionId,
    text: &str,
    events: &mpsc::UnboundedSender<TurnEvent>,
) {
    // Clear any stragglers buffered while the connection was idle between turns.
    while updates.try_recv().is_ok() {}

    let blocks = vec![ContentBlock::Text(TextContent::new(text.to_string()))];
    let prompt = conn
        .send_request(PromptRequest::new(session_id.clone(), blocks))
        .block_task();
    tokio::pin!(prompt);

    loop {
        tokio::select! {
            result = &mut prompt => {
                match result {
                    Ok(resp) => {
                        // Flush any updates that landed alongside the response.
                        while let Ok(n) = updates.try_recv() {
                            if &n.session_id == session_id {
                                let _ = events.send(TurnEvent::Update(Box::new(n.update)));
                            }
                        }
                        let _ = events.send(TurnEvent::Finished(resp.stop_reason));
                    }
                    Err(e) => {
                        let _ = events.send(TurnEvent::Failed(AcpError::from(e)));
                    }
                }
                break;
            }
            maybe = updates.recv() => {
                match maybe {
                    Some(n) if &n.session_id == session_id => {
                        let _ = events.send(TurnEvent::Update(Box::new(n.update)));
                    }
                    Some(_) => {} // update for another session
                    None => {
                        let _ = events.send(TurnEvent::Failed(AcpError::ConnectionClosed));
                        break;
                    }
                }
            }
        }
    }
}
