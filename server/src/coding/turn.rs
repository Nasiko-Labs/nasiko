//! End-to-end coding-agent turn execution.
//!
//! Joins the three pieces built separately:
//!   - the container runtime's per-session stdio attach,
//!   - the ACP client that speaks JSON-RPC to the agent,
//!   - the session lifecycle lease/registry ([`super::lifecycle`]).
//!
//! One [`run_turn`] call: take the one-active-turn lease, make sure the agent
//! container is up, open a stdio pipe into a fresh per-session process, bridge it
//! to an `AcpConnection`, send the prompt, and stream the agent's updates back —
//! renewing the lease while the turn runs and releasing it when the turn ends.

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use agent_client_protocol::ByteStreams;
use agent_client_protocol::schema::v1::{HttpHeader, McpServer, McpServerHttp, SessionId};
use chrono::Utc;
use nasiko_acp_client::{
    AcpConnectConfig, AcpConnection, AcpError, CapturePolicy, NewSessionParams, PermissionPolicy,
    TurnEvent, TurnStream, TurnTelemetry,
};
use nasiko_runtime::{ContainerId, DeploymentSpec, RuntimeState, SessionAttach};
use tokio::sync::mpsc;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::lifecycle::{self, SessionRuntimeState, TurnLease};
use super::questions::TurnQuestions;
use crate::state::AppState;

/// What [`run_turn`] needs to drive one turn.
///
/// The coding-agent *registration* model — how an image, launch command, and
/// owner are chosen for a session — is a separate concern; this struct is the
/// contract that model must satisfy, so the turn path stays agnostic to it.
pub struct CodingTurnRequest {
    /// The chat session (A2A contextId) — the `chat_sessions` primary key.
    pub session_id: String,
    /// Fully-specified container for the agent. Deployed idempotently, so a warm
    /// (already-running) container is reused and a parked one is started.
    pub spec: DeploymentSpec,
    /// The argv the container execs per session, e.g. `["opencode", "acp"]`.
    pub acp_command: Vec<String>,
    /// Per-session environment for the agent process. The session credential
    /// that binds the agent's calls to this user and flow is delivered here
    /// (as the agent's LLM-router key), rather than baked into the image.
    pub env: std::collections::HashMap<String, String>,
    /// The user the turn runs for, when known. Used to attribute the turn's
    /// telemetry event; `None` skips telemetry (the turn still runs).
    pub user_id: Option<uuid::Uuid>,
    /// The user's prompt for this turn.
    pub prompt: String,
    /// The session-bound token (NAS-839) for this turn, if the platform is
    /// configured to mint one. When present it is handed to the agent as its MCP
    /// gateway credential via `session/new`'s `mcpServers` (and to its model
    /// calls via [`env`](Self::env)); the gateway and router verify it instead
    /// of inferring identity from a `traceparent`.
    pub session_token: Option<String>,
    /// The minted token's revocation coordinates, when a token was minted. The
    /// turn revokes this `jti` when it ends, so the per-turn credential dies with
    /// the turn rather than lingering its full TTL inside the container (a copy
    /// that escaped the agent process can't then be replayed).
    pub session_token_id: Option<super::revocation::RevokedTokenId>,
    /// Whether this agent gates its tool use behind human approval (NAS-842).
    /// When set, the turn drives the agent with a [`HitlPermissionDelegate`]
    /// instead of auto-approving every permission request.
    pub hitl_enabled: bool,
}

/// What the coding-turn SSE layer consumes: either a raw ACP [`TurnEvent`], or a
/// control-plane notice that the turn is now blocked on a human HITL decision.
///
/// The HITL permission delegate runs off the ACP event loop and so never
/// produces a `TurnEvent`; without this the SSE stream would simply hang while a
/// human is asked. [`CodingTurnEvent::AwaitingApproval`] surfaces the pending
/// `hitl_requests` id to the client (as a `TASK_STATE_INPUT_REQUIRED` event — see
/// [`super::sse`]) so it can resolve it via `POST /api/hitl/{id}/resolve`.
pub enum CodingTurnEvent {
    /// A streamed ACP turn event (agent text, tool call, completion, failure).
    Turn(TurnEvent),
    /// A tool-permission request is awaiting a human decision. Carries the
    /// `hitl_requests` id to resolve and the question payload (the tool call and
    /// the offered options) so the chat UI can render the approve/reject card
    /// inline from the stream — not just via the Waiting-queue poll.
    AwaitingApproval {
        hitl_id: uuid::Uuid,
        question: serde_json::Value,
    },
    /// The turn ended because the caller cancelled the task (`tasks/cancel`). Terminal.
    Canceled,
}

/// Why a turn could not be started.
#[derive(Debug, thiserror::Error)]
pub enum CodingTurnError {
    /// Another turn already holds this session's lock (one turn at a time).
    #[error("another turn is already running for this session")]
    Busy,
    /// A suspend claim took this turn's lease and fence while it was starting; the turn stops
    /// before touching the sandbox and the client may retry once the suspend settles.
    #[error("the session is being suspended")]
    Superseded,
    /// The agent container never reached a serving state in time.
    #[error("agent container did not become ready in time")]
    NotReady,
    /// The turn needs the Nasiko MCP gateway (a session token + gateway URL are
    /// configured) but the agent did not advertise HTTP MCP support at
    /// `initialize`. We refuse rather than hand the agent a transport it would
    /// silently ignore, leaving it toolless with no clear cause.
    #[error("agent does not support HTTP MCP, required to attach the Nasiko gateway")]
    McpGatewayUnsupported,
    /// The turn could not be set up for a reason that is neither the runtime nor ACP: no session
    /// credential, an unusable catalog entry, a sandbox that offers the wrong transport.
    #[error("harness turn setup failed: {0}")]
    Setup(String),
    /// This server is not set up to run the turn: no session-signing secret, or a harness that is
    /// switched off. The message names what to change and is safe to show the caller, so it is
    /// answered as `503` with the message, never as an internal error.
    #[error("{0}")]
    NotConfigured(String),
    #[error(transparent)]
    Runtime(#[from] nasiko_runtime::RuntimeError),
    #[error(transparent)]
    Acp(#[from] nasiko_acp_client::AcpError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// What the caller is told when coding sessions cannot run because no session-signing secret is set.
pub const SESSIONS_NOT_CONFIGURED: &str =
    "Coding sessions are not configured on this server (AGENT_JWT_SECRET is not set)";

impl CodingTurnError {
    /// A session credential could not be minted. A missing secret is a setup fault the caller can
    /// be told about; any other failure stays an internal setup error.
    pub fn from_credential(what: &str, error: nasiko_session_token::SessionTokenError) -> Self {
        match error {
            nasiko_session_token::SessionTokenError::SecretNotConfigured => {
                Self::NotConfigured(SESSIONS_NOT_CONFIGURED.to_owned())
            }
            other => Self::Setup(format!("{what}: {other}")),
        }
    }
}

/// How long to wait for a freshly-deployed container to report `Running` before
/// giving up on the attach (the cold path).
const READINESS_TIMEOUT: Duration = Duration::from_secs(60);
const READINESS_POLL: Duration = Duration::from_millis(500);

/// Run one coding-agent turn end to end.
///
/// Returns a receiver that yields the agent's [`TurnEvent`]s until a terminal
/// one. The turn's lease is held and renewed in the background for as long as the
/// turn runs, and released when it ends (which also stamps `last_turn_at`).
pub async fn run_turn(
    state: &AppState,
    req: CodingTurnRequest,
) -> Result<mpsc::UnboundedReceiver<CodingTurnEvent>, CodingTurnError> {
    let owner = replica_id();
    let turn_id = uuid::Uuid::new_v4().to_string();
    let lease_secs = state.config.coding_agent_turn_lease_secs;

    // One turn at a time per session: refuse rather than run a second concurrent
    // turn against the same agent process.
    let lease = lifecycle::acquire_turn(&state.db, &req.session_id, owner, &turn_id, lease_secs)
        .await?
        .ok_or(CodingTurnError::Busy)?;

    // Past this point a failure must release the lease, or the session would be
    // wedged until the lease lapses.
    match start_turn(state, &req, &lease, &turn_id).await {
        Ok(rx) => Ok(rx),
        Err(e) => {
            let _ = lifecycle::release_turn(&state.db, &lease).await;
            // `session/new` may already have handed the token to the container,
            // so a failed turn must still revoke it — the success path revokes in
            // `drive_turn`, but that never runs here.
            if let Some(id) = &req.session_token_id {
                let _ = super::revocation::revoke_session_token(&state.db, &id.jti, id.expires_at)
                    .await;
            }
            Err(e)
        }
    }
}

/// Everything between acquiring the lease and handing back the event stream.
async fn start_turn(
    state: &AppState,
    req: &CodingTurnRequest,
    lease: &TurnLease,
    turn_id: &str,
) -> Result<mpsc::UnboundedReceiver<CodingTurnEvent>, CodingTurnError> {
    let container_id = req.spec.container_id.clone();
    let handle = container_id.as_str().to_owned();

    // Make sure the container is up: deploy() is idempotent, so the warm path
    // (already running) is a near no-op and the cold path starts it. Then wait
    // for Running before attaching — a pod/container mid-start can't be exec'd.
    lifecycle::set_runtime_state(
        &state.db,
        &req.session_id,
        SessionRuntimeState::Starting,
        Some(&handle),
    )
    .await?;
    state.runtime.deploy(&req.spec).await?;
    wait_for_running(state, &container_id).await?;
    lifecycle::set_runtime_state(
        &state.db,
        &req.session_id,
        SessionRuntimeState::Running,
        None,
    )
    .await?;

    // Open the per-session stdio pipe, then bridge the runtime's tokio
    // AsyncRead/AsyncWrite halves to the futures-io the ACP SDK's ByteStreams
    // transport wants. ByteStreams::new(outgoing, incoming): outgoing is what we
    // write to the agent (its stdin), incoming is what we read (its stdout).
    //
    // The agent works out of the writable mount root (`/workspace`); how on-disk
    // state is partitioned per session is internal to the ACP agent, not ours, so
    // we deliberately do not carve a per-session subdirectory here. `session_label`
    // is an opaque, filesystem/shell-safe identifier carried only for tracing.
    let session_label = safe_session_label(&req.session_id);
    let attach = SessionAttach {
        session_id: session_label,
        command: req.acp_command.clone(),
        env: req.env.clone(),
    };
    let stdio = state.runtime.attach_stdio(&container_id, attach).await?;
    let transport = ByteStreams::new(stdio.stdin.compat_write(), stdio.stdout.compat());

    // Create the SSE channel up front so the HITL permission delegate (built
    // inside the connection below) can push `AwaitingApproval` notices onto the
    // same stream the turn's updates flow through — it runs off the ACP loop and
    // has no `TurnEvent` of its own to carry the pause.
    let (out_tx, out_rx) = mpsc::unbounded_channel::<CodingTurnEvent>();
    let questions = TurnQuestions::default();
    let conn = AcpConnection::connect(
        transport,
        AcpConnectConfig {
            permission_policy: hitl_permission_policy(
                state,
                policy_input(req, questions.clone()),
                out_tx.clone(),
            ),
            ..Default::default()
        },
    )
    .await?;

    // Open the agent session in the writable mount root (the same directory the
    // attach cd'd the process into), then send the prompt. Session-scoped file
    // layout is the ACP agent's own concern — the platform hands it one workspace.
    //
    // Cold-path restore: if this session already has an agent-side id from an
    // earlier turn *and* the agent advertised `session/load`, load it so the
    // agent resumes its conversation state instead of starting fresh. Otherwise
    // open a new session and record its id for next time.
    let cwd = std::path::PathBuf::from(DeploymentSpec::DEFAULT_WRITABLE_PATH);
    // Only attach the gateway over a transport the agent advertised — otherwise
    // fail loud instead of leaving the agent silently toolless.
    let params = session_params(state, &req.session_token, cwd, conn.capabilities().mcp_http)?;
    let acp_session = open_acp_session(&state.db, &conn, lease, params).await?;

    // Accumulate a telemetry event from the turn's updates (NAS-841), attributed
    // to the user the proxy resolved. Metadata-only: the proxy already wrote the
    // chat session + user message, so this records the tool/turn metadata the
    // OTLP outbox exports without duplicating content.
    let telemetry = build_turn_telemetry(req, turn_id, &req.spec.name, &acp_session.0);

    // Keep the session id so the driver can `session/cancel` it if the turn runs
    // past its budget; `prompt` consumes the original.
    let cancel_session = acp_session.clone();
    let turn_timeout = Duration::from_secs(state.config.agent_call_timeout_secs);
    let turn = conn.prompt(acp_session, req.prompt.clone())?;

    // Drive the turn in the background: forward events to the caller, renew the
    // lease while it runs, and release it (and close the pipe) when it ends. The
    // `out_tx`/`out_rx` channel was created above so the delegate could share it.
    let db = state.db.clone();
    let lease = lease.clone();
    let lease_secs = state.config.coding_agent_turn_lease_secs;
    let heartbeat_secs = state.config.coding_agent_lease_heartbeat_secs.max(1);
    tokio::spawn(
        DriveTurn {
            conn,
            turn,
            out_tx,
            db,
            lease,
            lease_secs,
            heartbeat_secs,
            telemetry,
            session_token_id: req.session_token_id.clone(),
            cancel_session,
            turn_timeout,
            turn_id: turn_id.to_owned(),
            cancel: None,
            keep_alive: None,
            shim_restart: None,
            questions,
        }
        .run(),
    );

    Ok(out_rx)
}

/// Open the agent-side session on `conn`: restore the saved one when the agent can, else start
/// a fresh one, and record whichever id is now current.
///
/// Cold-path restore: if this session already has an agent-side id from an earlier turn *and*
/// the agent advertised `session/load`, load it so the agent resumes its conversation state
/// instead of starting fresh. A saved id can become unloadable (the sandbox was recreated and
/// the agent no longer holds it); letting that fail would wedge the conversation at a permanent
/// error, because the unchanged id is retried every turn. So a failed load falls back to a
/// fresh session — the `/workspace` files survive, only the agent's in-memory history is lost —
/// and the new id is re-recorded so later turns stop retrying the dead one.
///
/// Shared by the exec path and the harness path (`sessions::turn`).
pub(crate) async fn open_acp_session(
    db: &sqlx::PgPool,
    conn: &AcpConnection,
    lease: &TurnLease,
    params: NewSessionParams,
) -> Result<SessionId, CodingTurnError> {
    let session_id = lease.session_id.as_str();
    let prior = lifecycle::acp_session_id(db, session_id).await?;
    if let Some(prior_id) = prior
        && conn.capabilities().load_session
    {
        let session = SessionId::new(prior_id);
        match conn.load_session(session.clone(), params.clone()).await {
            Ok(()) => return Ok(session),
            Err(e) => tracing::warn!(
                error = %e,
                %session_id,
                acp_session_id = %session.0,
                "coding-agent: session/load failed; starting a fresh agent session"
            ),
        }
    }
    let session = conn.new_session(params).await?;
    // Recorded under the turn's fence: a suspend claim that took the lease since must not have its
    // session row written, and the turn stops rather than prompt on a session it no longer owns.
    if !lifecycle::record_acp_session_fenced(db, lease, &session.0).await? {
        return Err(CodingTurnError::Superseded);
    }
    Ok(session)
}

/// Per-turn telemetry accumulator plus the keys needed to persist it.
pub(crate) struct TurnTelemetryCtx {
    telemetry: TurnTelemetry,
    user_id: uuid::Uuid,
    agent_id: uuid::Uuid,
    session_id: String,
}

/// Build the telemetry accumulator for a turn, or `None` when the user isn't
/// known or the agent id can't be parsed (telemetry is best-effort, never a
/// reason to fail a turn).
fn build_turn_telemetry(
    req: &CodingTurnRequest,
    turn_id: &str,
    agent_name: &str,
    acp_session_id: &str,
) -> Option<TurnTelemetryCtx> {
    new_turn_telemetry(TelemetryKeys {
        user_id: req.user_id,
        agent_id: uuid::Uuid::parse_str(req.spec.container_id.as_str()).ok(),
        session_id: &req.session_id,
        turn_id,
        agent_name,
        acp_session_id,
        prompt: &req.prompt,
    })
}

/// What identifies one turn for telemetry.
pub(crate) struct TelemetryKeys<'a> {
    pub user_id: Option<uuid::Uuid>,
    pub agent_id: Option<uuid::Uuid>,
    pub session_id: &'a str,
    pub turn_id: &'a str,
    pub agent_name: &'a str,
    pub acp_session_id: &'a str,
    pub prompt: &'a str,
}

/// The accumulator for `keys`, or `None` when the user or agent is unknown.
pub(crate) fn new_turn_telemetry(keys: TelemetryKeys<'_>) -> Option<TurnTelemetryCtx> {
    let user_id = keys.user_id?;
    let agent_id = keys.agent_id?;
    let telemetry = TurnTelemetry::new(
        agent_id.to_string(),
        keys.agent_name.to_owned(),
        keys.acp_session_id.to_owned(),
        keys.turn_id.to_owned(),
        keys.prompt.to_owned(),
        CapturePolicy::MetadataOnly,
        Utc::now(),
    );
    Some(TurnTelemetryCtx {
        telemetry,
        user_id,
        agent_id,
        session_id: keys.session_id.to_owned(),
    })
}

/// Why a turn that can no longer renew its lease was ended, read from the session row: deleted,
/// being (or just) suspended, or reset by a reconciler or takeover. `turn_started` tells a
/// suspension that happened during this turn from an old one.
pub(crate) async fn lease_lost_message(
    db: &sqlx::PgPool,
    session_id: &str,
    turn_started: chrono::DateTime<Utc>,
) -> &'static str {
    type LostRow = (String, bool, Option<chrono::DateTime<Utc>>);
    let row: sqlx::Result<Option<LostRow>> = sqlx::query_as(
        "SELECT runtime_state, deleted_at IS NOT NULL, suspended_at FROM chat_sessions \
         WHERE session_id = $1",
    )
    .bind(session_id)
    .fetch_optional(db)
    .await;
    match row {
        Ok(None) => lost_reason(None, turn_started),
        Ok(Some((state, deleted, suspended_at))) => {
            lost_reason(Some((&state, deleted, suspended_at)), turn_started)
        }
        Err(_) => lost_reason(Some(("", false, None)), turn_started),
    }
}

/// The wording for a lost lease from what the row says (`None`: the row is gone).
fn lost_reason(
    row: Option<(&str, bool, Option<chrono::DateTime<Utc>>)>,
    turn_started: chrono::DateTime<Utc>,
) -> &'static str {
    match row {
        None | Some((_, true, _)) => "session was deleted",
        Some((state, false, _)) if state == SessionRuntimeState::Stopping.as_str() => {
            "session was suspended"
        }
        // A suspend that has already settled while this turn was running.
        Some((state, false, Some(at)))
            if state == SessionRuntimeState::Parked.as_str() && at >= turn_started =>
        {
            "session was suspended"
        }
        Some(_) => "session was reset; the turn lost its lease",
    }
}

/// Everything the background driver of one running turn owns.
pub(crate) struct DriveTurn {
    pub conn: AcpConnection,
    pub turn: TurnStream,
    pub out_tx: mpsc::UnboundedSender<CodingTurnEvent>,
    pub db: sqlx::PgPool,
    pub lease: TurnLease,
    pub lease_secs: i64,
    pub heartbeat_secs: u64,
    pub telemetry: Option<TurnTelemetryCtx>,
    pub session_token_id: Option<super::revocation::RevokedTokenId>,
    pub cancel_session: SessionId,
    pub turn_timeout: Duration,
    /// The turn's id (its A2A task id), recorded with its outcome.
    pub turn_id: String,
    /// Fired by `tasks/cancel` on the replica running the turn. A cancel that lands on another
    /// replica reaches the turn through the row flag instead, on the next heartbeat.
    pub cancel: Option<tokio_util::sync::CancellationToken>,
    /// Keeps the turn registered as a live task until the driver has finished and released the
    /// session's lease; dropped last. See `sessions::live`.
    pub keep_alive: Option<Arc<crate::sessions::live::LiveRegistration>>,
    /// How to ask the sandbox's shim to restart its harness, used when a cancelled turn had to be
    /// ended by force.
    pub shim_restart: Option<ShimRestart>,
    /// The questions the turn asked a person; the ones still open are closed when it ends.
    pub questions: TurnQuestions,
}

/// The shim's restart endpoint (`POST /control/restart`) and what it needs.
#[derive(Clone)]
pub struct ShimRestart {
    pub client: reqwest::Client,
    /// The WebSocket URL the sandbox attached us to; the control URL derives from it.
    pub ws_url: String,
    pub bearer: String,
    /// The sandbox's routing headers, sent on the control call as on the attach.
    pub headers: Vec<(String, String)>,
}

impl ShimRestart {
    /// Ask the shim to replace its harness child. Best-effort: the turn is over either way.
    async fn request(&self) {
        let control = crate::sessions::handshake::ControlClient {
            http: &self.client,
            base_ws_url: &self.ws_url,
            routing: &self.headers,
            bearer: &self.bearer,
        };
        if let Err(e) = control.restart().await {
            tracing::warn!(error = %e, "shim restart failed");
        }
    }
}

/// How long a cancelled turn is given to wind down (the agent answering its `session/cancel`)
/// before it is ended anyway.
const CANCEL_GRACE: Duration = Duration::from_secs(10);

impl DriveTurn {
    /// Forward the turn's events, renew its lease, end it at its deadline, then persist what it
    /// produced and release the lease.
    pub(crate) async fn run(self) {
        let started_at = Utc::now();
        let DriveTurn {
            conn,
            mut turn,
            out_tx,
            db,
            lease,
            lease_secs,
            heartbeat_secs,
            mut telemetry,
            session_token_id,
            cancel_session,
            turn_timeout,
            turn_id,
            cancel,
            keep_alive,
            shim_restart,
            questions,
        } = self;
        let mut outcome = lifecycle::TurnOutcome::Failed;
        let mut cancelling = false;
        // Set when a cancelled turn had to be ended without the agent answering.
        let mut forced_end = false;
        let mut beat = tokio::time::interval(Duration::from_secs(heartbeat_secs));
        beat.tick().await; // the first tick fires immediately; skip it

        // Hard ceiling on the turn's wall-clock. The lease heartbeat keeps the *lock*
        // alive for as long as the turn runs, so nothing else bounds a runaway turn —
        // this does, by dispatching `session/cancel` and ending the turn (TRD §3.4).
        let deadline = tokio::time::sleep(turn_timeout);
        tokio::pin!(deadline);

        // Accumulate the agent's visible reply text across streamed chunks, so it can
        // be persisted to `chat_messages` when the turn ends (NAS-C2). Without this the
        // reply only ever lives in the SSE stream and the UI shows "No reply saved" on
        // reload.
        let mut reply = String::new();

        loop {
            tokio::select! {
                event = turn.recv() => match event {
                    Some(ev) => {
                        // Fold streamed updates into the telemetry event and the reply
                        // accumulator before forwarding them on.
                        if let TurnEvent::Update(update) = &ev {
                            if let Some(ctx) = telemetry.as_mut() {
                                ctx.telemetry.observe(update, Utc::now());
                            }
                            if let Some(text) = super::sse::agent_text(update) {
                                reply.push_str(&text);
                            }
                        }
                        // A transport failure can be the first sign the session was deleted or
                        // suspended (the provider closed the connection before the next heartbeat):
                        // if the lease is in fact gone, say that instead.
                        let ev = match ev {
                            TurnEvent::Failed(err) if !matches!(err, AcpError::Cancelled(_)) => {
                                explain_failure(err, &db, &lease, lease_secs, heartbeat_secs, started_at)
                                    .await
                            }
                            other => other,
                        };
                        let terminal = ev.is_terminal();
                        let out = if terminal && cancelling {
                            outcome = lifecycle::TurnOutcome::Canceled;
                            CodingTurnEvent::Canceled
                        } else {
                            if let TurnEvent::Finished(stop) = &ev {
                                outcome = if matches!(stop, nasiko_acp_client::StopReason::Cancelled) {
                                    lifecycle::TurnOutcome::Canceled
                                } else {
                                    lifecycle::TurnOutcome::Completed
                                };
                            }
                            CodingTurnEvent::Turn(ev)
                        };
                        // Stop if the caller hung up or the turn reached its end.
                        if out_tx.send(out).is_err() || terminal {
                            break;
                        }
                    }
                    None => {
                        // The event stream ended without a terminal event. During a cancel that
                        // is the agent going away in answer to it: still a cancelled turn.
                        if cancelling {
                            outcome = lifecycle::TurnOutcome::Canceled;
                            let _ = out_tx.send(CodingTurnEvent::Canceled);
                        }
                        break;
                    }
                },
                () = async {
                    match &cancel {
                        Some(token) => token.cancelled().await,
                        None => std::future::pending().await,
                    }
                }, if !cancelling => {
                    begin_cancel(&conn, &cancel_session, &mut cancelling, deadline.as_mut(), cancel.as_ref());
                }
                _ = beat.tick() => {
                    // Renew the lease, retrying a database error within the tick. Only a lease the
                    // database says is gone ends the turn — we no longer own the session and must
                    // not keep writing to its agent. An unreachable database is not that: the next
                    // tick tries again, and a takeover meanwhile shows up as a lost lease. The same
                    // statement says whether a `tasks/cancel` (possibly served by another replica)
                    // asked for this turn to stop.
                    let budget = Duration::from_secs(heartbeat_secs);
                    let (beat_result, cancel_requested) =
                        lifecycle::heartbeat_retrying_flag(&db, &lease, lease_secs, budget).await;
                    if beat_result == lifecycle::Beat::Lost {
                        // Stop the agent and say why, instead of the generic "ended without
                        // completing".
                        let _ = conn.cancel(cancel_session.clone());
                        let message = lease_lost_message(&db, &lease.session_id, started_at).await;
                        let _ = out_tx.send(CodingTurnEvent::Turn(TurnEvent::Failed(
                            AcpError::Cancelled(message.to_owned()),
                        )));
                        break;
                    }
                    if cancel_requested && !cancelling {
                        begin_cancel(&conn, &cancel_session, &mut cancelling, deadline.as_mut(), cancel.as_ref());
                    }
                }
                _ = &mut deadline => {
                    // Over budget: tell the agent to stop, then end the turn. Dropping
                    // `conn` below closes the pipe as a backstop if cancel is ignored.
                    let _ = conn.cancel(cancel_session.clone());
                    if cancelling {
                        outcome = lifecycle::TurnOutcome::Canceled;
                        forced_end = true;
                        let _ = out_tx.send(CodingTurnEvent::Canceled);
                    } else {
                        let _ = out_tx.send(CodingTurnEvent::Turn(TurnEvent::Failed(
                            AcpError::Cancelled(format!(
                                "turn exceeded its {}s budget",
                                turn_timeout.as_secs()
                            )),
                        )));
                    }
                    break;
                }
            }
        }

        // Persist the turn's telemetry event (best-effort — a telemetry failure must
        // never surface as a turn failure). A turn cut short still emits a partial.
        if let Some(ctx) = telemetry {
            let TurnTelemetryCtx {
                telemetry,
                user_id,
                agent_id,
                session_id,
            } = ctx;
            match telemetry.finish(Utc::now()) {
                Ok(event) => {
                    if let Err(e) = crate::coding_agent_telemetry::persist_coding_turn_telemetry(
                        &db,
                        user_id,
                        agent_id,
                        &session_id,
                        &event,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, %agent_id, "coding-agent: telemetry persist failed");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, %agent_id, "coding-agent: telemetry event invalid")
                }
            }
        }

        // Persist the agent's reply so it survives a reload: the chat UI loads the
        // transcript from `chat_messages`, but a coding turn only ever streamed its
        // answer over SSE. Role `assistant` matches the normal agent path; the dedup
        // guard mirrors the other transcript writers in case the client saved it too.
        // Best-effort — a persistence failure never fails the turn.
        if !reply.trim().is_empty() {
            let _ = sqlx::query(
                "INSERT INTO chat_messages (session_id, role, content) \
             SELECT $1, 'assistant', $2 WHERE NOT EXISTS ( \
                 SELECT 1 FROM chat_messages \
                 WHERE session_id = $1 AND role = 'assistant' AND content = $2 \
                   AND timestamp > now() - INTERVAL '10 seconds')",
            )
            .bind(&lease.session_id)
            .bind(&reply)
            .execute(&db)
            .await;
        }

        // Dropping the connection closes the pipe; the per-session agent process
        // exits on stdin EOF while the container stays up for other sessions. Then
        // release the lease (which stamps `last_turn_at` for idle tracking).
        drop(conn);
        // A question lives no longer than its turn: nothing delivers an answer given after this.
        close_open_questions(&db, &lease, &questions).await;
        // Outcome before release: release clears the live turn id, and `tasks/get` must always
        // find the task as one or the other.
        let _ = lifecycle::record_outcome(&db, &lease, &turn_id, outcome).await;
        let _ = lifecycle::release_turn(&db, &lease).await;
        // Revoke this turn's session token: the turn is over, so the credential has
        // no more legitimate use, and revoking it caps a leaked copy's lifetime to
        // the turn rather than the token's full TTL.
        if let Some(id) = session_token_id
            && let Err(e) =
                super::revocation::revoke_session_token(&db, &id.jti, id.expires_at).await
        {
            tracing::warn!(error = %e, jti = %id.jti, "coding-agent: session-token revoke failed");
        }
        // The agent never answered the cancel, so it may still be blocked on something (an
        // unanswered permission request): have the shim start the next turn on a fresh harness
        // rather than attach to this one. After the revoke: a restart can take
        // the shim's terminate grace, and the credential must not outlive the turn by that long.
        if forced_end && let Some(restart) = &shim_restart {
            restart.request().await;
        }

        // The turn is over and its lease released: only now stop being a live task.
        drop(keep_alive);
    }
}

/// Ask the agent to stop and give it [`CANCEL_GRACE`] to wind down: the turn then ends as
/// canceled whether the agent answered or not.
fn begin_cancel(
    conn: &AcpConnection,
    session: &SessionId,
    cancelling: &mut bool,
    deadline: std::pin::Pin<&mut tokio::time::Sleep>,
    token: Option<&tokio_util::sync::CancellationToken>,
) {
    *cancelling = true;
    // Fired here as well as by the request, so a cancel that arrived through the row flag also
    // reaches a pending permission wait.
    if let Some(token) = token {
        token.cancel();
    }
    let _ = conn.cancel(session.clone());
    deadline.reset(tokio::time::Instant::now() + CANCEL_GRACE);
}

/// Close the questions the turn asked that nobody answered (`coding::questions`). Best effort: a
/// failure leaves them open, and a later suspend of the session closes them.
async fn close_open_questions(db: &sqlx::PgPool, lease: &TurnLease, questions: &TurnQuestions) {
    match questions.close_open(db).await {
        Ok(0) => {}
        Ok(closed) => {
            tracing::info!(session_id = %lease.session_id, closed, "coding HITL: closed the questions the turn left unanswered");
        }
        Err(e) => {
            tracing::warn!(session_id = %lease.session_id, error = %e, "coding HITL: could not close the turn's unanswered questions");
        }
    }
}

/// A failed turn's terminal event, with the real reason when the turn's lease is gone.
async fn explain_failure(
    err: AcpError,
    db: &sqlx::PgPool,
    lease: &TurnLease,
    lease_secs: i64,
    heartbeat_secs: u64,
    started_at: chrono::DateTime<Utc>,
) -> TurnEvent {
    let budget = Duration::from_secs(heartbeat_secs);
    if lifecycle::heartbeat_retrying(db, lease, lease_secs, budget).await == lifecycle::Beat::Lost {
        let message = lease_lost_message(db, &lease.session_id, started_at).await;
        return TurnEvent::Failed(AcpError::Cancelled(message.to_owned()));
    }
    TurnEvent::Failed(err)
}

/// Poll the runtime until the container is `Running`, or fail if it crashes or
/// the readiness window elapses.
async fn wait_for_running(state: &AppState, id: &ContainerId) -> Result<(), CodingTurnError> {
    let deadline = tokio::time::Instant::now() + READINESS_TIMEOUT;
    loop {
        let status = state.runtime.status(id).await?;
        match status.state {
            RuntimeState::Running => return Ok(()),
            RuntimeState::Crashed | RuntimeState::Failed => return Err(CodingTurnError::NotReady),
            _ if tokio::time::Instant::now() >= deadline => return Err(CodingTurnError::NotReady),
            _ => tokio::time::sleep(READINESS_POLL).await,
        }
    }
}

/// Build the `session/new` parameters, attaching the Nasiko MCP gateway as an
/// HTTP MCP server when a session token and a gateway URL are both configured.
///
/// Per the ACP spec the agent uses each entry's `headers` to authenticate to the
/// MCP server, so the session token rides `Authorization: Bearer …` there — the
/// gateway verifies it exactly as it does for the agent's model calls (NAS-839).
/// With no token or no gateway URL the session simply has no MCP servers.
///
/// We only inject over a transport the agent advertised at `initialize`: if the
/// gateway is required but the agent didn't negotiate HTTP MCP
/// (`agent_supports_http_mcp`), we return [`CodingTurnError::McpGatewayUnsupported`]
/// rather than hand it a server it will ignore (the agent would then run with no
/// tools and no clear reason why). SSE fallback is future work (NAS-838).
pub(crate) fn session_params(
    state: &AppState,
    session_token: &Option<String>,
    cwd: std::path::PathBuf,
    agent_supports_http_mcp: bool,
) -> Result<NewSessionParams, CodingTurnError> {
    let servers = gateway_mcp_servers(
        session_token.as_deref(),
        state.config.mcp_gateway_public_url.as_deref(),
        agent_supports_http_mcp,
    )?;
    Ok(NewSessionParams::new(cwd).with_mcp_servers(servers))
}

/// Decide which MCP servers to attach to `session/new`: the Nasiko gateway over
/// HTTP when a session token and gateway URL are both present, otherwise none.
///
/// Returns [`CodingTurnError::McpGatewayUnsupported`] when the gateway is required
/// but the agent didn't advertise HTTP MCP — failing loud beats handing it a
/// transport it will silently ignore. Pure (no `AppState`) so it is unit-testable.
fn gateway_mcp_servers(
    session_token: Option<&str>,
    gateway_url: Option<&str>,
    agent_supports_http_mcp: bool,
) -> Result<Vec<McpServer>, CodingTurnError> {
    match (session_token, gateway_url) {
        (Some(token), Some(gateway_url)) => {
            if !agent_supports_http_mcp {
                tracing::warn!(
                    "coding-agent: MCP gateway configured but agent did not advertise \
                     HTTP MCP support; refusing to start a toolless turn"
                );
                return Err(CodingTurnError::McpGatewayUnsupported);
            }
            Ok(vec![McpServer::Http(
                McpServerHttp::new("nasiko-gateway", gateway_url).headers(vec![HttpHeader::new(
                    "Authorization",
                    format!("Bearer {token}"),
                )]),
            )])
        }
        _ => Ok(Vec::new()),
    }
}

/// A stable, filesystem- and shell-safe per-session label.
///
/// The ACP agent owns how it partitions on-disk state, so this value is not a
/// directory — it is the opaque per-session identifier the attach carries for
/// tracing. Most A2A context ids (`ses_<alnum>`, a UUID) are already valid
/// [`SessionAttach`] segments and pass through unchanged for readability;
/// anything else is mapped to a collision-resistant hash so the label stays a
/// safe, stable segment.
fn safe_session_label(session_id: &str) -> String {
    let already_safe = !session_id.is_empty()
        && session_id.len() <= 63
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if already_safe {
        session_id.to_owned()
    } else {
        use sha2::{Digest, Sha256};
        let digest = hex::encode(Sha256::digest(session_id.as_bytes()));
        format!("s-{}", &digest[..32])
    }
}

/// What decides how this turn answers the agent's `session/request_permission` requests
/// ([`hitl_permission_policy`]).
///
/// Auto mode (the default) approves everything so a turn runs unattended. When
/// the agent is `hitl_enabled`, route each request to the Nasiko HITL surface via
/// a [`super::permission::HitlPermissionDelegate`] — but only when the turn's user
/// and agent id are both known; if HITL is asked for and either is missing we
/// fail closed (reject every request) rather than silently auto-approving an
/// agent that was meant to be gated. Each question asked is noted in `questions`.
fn policy_input(req: &CodingTurnRequest, questions: TurnQuestions) -> HitlPolicyInput<'_> {
    HitlPolicyInput {
        hitl_enabled: req.hitl_enabled,
        agent_id: uuid::Uuid::parse_str(req.spec.container_id.as_str()).ok(),
        user_id: req.user_id,
        session_id: &req.session_id,
        pending_sink: None,
        cancel: None,
        questions,
    }
}

/// What decides how a turn answers permission requests.
pub(crate) struct HitlPolicyInput<'a> {
    pub hitl_enabled: bool,
    pub agent_id: Option<uuid::Uuid>,
    pub user_id: Option<uuid::Uuid>,
    pub session_id: &'a str,
    /// Persists a pending approval across a suspend (harness sessions only).
    pub pending_sink: Option<Arc<dyn super::permission::PendingPermissionSink>>,
    /// Fired when the task is cancelled, so a pending approval is closed instead of left waiting.
    pub cancel: Option<tokio_util::sync::CancellationToken>,
    /// The turn's record of the questions it asks, which its driver closes when the turn ends.
    pub questions: super::questions::TurnQuestions,
}

/// The HITL permission rule over plain values, so the exec path ([`policy_input`]) and the harness
/// path share it.
pub(crate) fn hitl_permission_policy(
    state: &AppState,
    input: HitlPolicyInput<'_>,
    notice_tx: mpsc::UnboundedSender<CodingTurnEvent>,
) -> PermissionPolicy {
    if !input.hitl_enabled {
        return PermissionPolicy::ApproveAll;
    }
    match (input.agent_id, input.user_id) {
        (Some(agent_id), Some(user_id)) => {
            let mut delegate = super::permission::HitlPermissionDelegate::new(
                state.hitl_store.clone(),
                agent_id,
                user_id,
                input.session_id.to_owned(),
                Some(notice_tx),
            );
            if let Some(sink) = input.pending_sink {
                delegate = delegate.with_pending_sink(sink);
            }
            if let Some(cancel) = input.cancel {
                delegate = delegate.with_cancel(cancel);
            }
            delegate = delegate.with_questions(input.questions);
            PermissionPolicy::Delegate(Arc::new(delegate))
        }
        _ => {
            tracing::warn!(
                session_id = %input.session_id,
                "coding HITL enabled but the turn's user or agent id is unknown; denying all tool use"
            );
            PermissionPolicy::RejectAll
        }
    }
}

/// This control-plane replica's stable identity, used as the lease owner so a
/// replica recognizes (and can renew) its own lease across turns.
pub(crate) fn replica_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        std::env::var("HOSTNAME")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("cp-{}", uuid::Uuid::new_v4()))
    })
}

#[cfg(test)]
mod tests {
    use super::{CodingTurnError, Utc, gateway_mcp_servers, lost_reason, safe_session_label};

    #[test]
    fn a_missing_signing_secret_is_named_to_the_caller() {
        let e = CodingTurnError::from_credential(
            "session credential",
            nasiko_session_token::SessionTokenError::SecretNotConfigured,
        );
        assert!(
            matches!(&e, CodingTurnError::NotConfigured(m) if m.contains("AGENT_JWT_SECRET")),
            "{e:?}"
        );
    }

    #[test]
    fn any_other_credential_failure_stays_an_internal_setup_error() {
        let e = CodingTurnError::from_credential(
            "session credential",
            nasiko_session_token::SessionTokenError::MissingToken,
        );
        assert!(matches!(e, CodingTurnError::Setup(_)), "{e:?}");
    }

    #[test]
    fn a_lost_lease_is_worded_by_what_the_row_says() {
        use chrono::Duration as D;
        let started = Utc::now();
        // Gone, or deleted: deleted.
        assert_eq!(lost_reason(None, started), "session was deleted");
        assert_eq!(
            lost_reason(Some(("running", true, None)), started),
            "session was deleted"
        );
        // Mid-suspend, or a suspend that settled during this turn: suspended.
        assert_eq!(
            lost_reason(Some(("stopping", false, None)), started),
            "session was suspended"
        );
        let during = Some(started + D::seconds(1));
        assert_eq!(
            lost_reason(Some(("parked", false, during)), started),
            "session was suspended"
        );
        // A suspension from before this turn started is not why it ended.
        let before = Some(started - D::seconds(60));
        assert_eq!(
            lost_reason(Some(("parked", false, before)), started),
            "session was reset; the turn lost its lease"
        );
        assert_eq!(
            lost_reason(Some(("running", false, None)), started),
            "session was reset; the turn lost its lease"
        );
    }

    #[test]
    fn gateway_attached_only_when_token_and_url_present() {
        // Both present and the agent speaks HTTP MCP: attach exactly the gateway.
        let servers = gateway_mcp_servers(Some("tok"), Some("https://gw"), true).unwrap();
        assert_eq!(servers.len(), 1);

        // Missing either half: no MCP servers, no error.
        assert!(
            gateway_mcp_servers(None, Some("https://gw"), true)
                .unwrap()
                .is_empty()
        );
        assert!(
            gateway_mcp_servers(Some("tok"), None, true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn gateway_required_but_no_http_mcp_is_an_error() {
        // Token + URL mean the gateway is required; an agent without HTTP MCP must
        // fail loud rather than be handed a transport it will ignore.
        assert!(matches!(
            gateway_mcp_servers(Some("tok"), Some("https://gw"), false),
            Err(CodingTurnError::McpGatewayUnsupported)
        ));
    }

    #[test]
    fn no_gateway_configured_tolerates_a_non_http_mcp_agent() {
        // With nothing to inject, the agent's MCP support is irrelevant — no error.
        assert!(gateway_mcp_servers(None, None, false).unwrap().is_empty());
    }

    #[test]
    fn safe_session_ids_pass_through_unchanged() {
        // The common shapes — a "ses_" id and a UUID — are already valid path
        // segments, so they stay readable.
        for id in [
            "ses_01HX8ZP7Q9",
            "6b1f2c9a-0e4d-4a1b-9c3e-2f5a7d8e1b0c",
            "plain_name-1",
        ] {
            assert_eq!(safe_session_label(id), id);
        }
    }

    #[test]
    fn unsafe_session_ids_hash_to_a_safe_stable_name() {
        // Anything with a separator, colon, emptiness, or over-length becomes a
        // safe hashed label — deterministic and collision-resistant.
        for id in ["a/b", "has:colon", "", &"x".repeat(100)] {
            let name = safe_session_label(id);
            assert!(name.starts_with("s-"), "{id:?} -> {name:?}");
            assert_eq!(name.len(), 34);
            assert!(
                name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "{name:?} must be a safe segment"
            );
            // Stable: same input always yields the same label.
            assert_eq!(safe_session_label(id), name);
        }
    }

    #[test]
    fn distinct_unsafe_ids_get_distinct_labels() {
        assert_ne!(safe_session_label("a/b"), safe_session_label("a/c"));
    }
}
