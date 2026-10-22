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
}

/// Why a turn could not be started.
#[derive(Debug, thiserror::Error)]
pub enum CodingTurnError {
    /// Another turn already holds this session's lock (one turn at a time).
    #[error("another turn is already running for this session")]
    Busy,
    /// The agent container never reached a serving state in time.
    #[error("agent container did not become ready in time")]
    NotReady,
    /// The turn needs the Nasiko MCP gateway (a session token + gateway URL are
    /// configured) but the agent did not advertise HTTP MCP support at
    /// `initialize`. We refuse rather than hand the agent a transport it would
    /// silently ignore, leaving it toolless with no clear cause.
    #[error("agent does not support HTTP MCP, required to attach the Nasiko gateway")]
    McpGatewayUnsupported,
    #[error(transparent)]
    Runtime(#[from] nasiko_runtime::RuntimeError),
    #[error(transparent)]
    Acp(#[from] nasiko_acp_client::AcpError),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
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
    let conn = AcpConnection::connect(
        transport,
        AcpConnectConfig {
            permission_policy: permission_policy(state, req, out_tx.clone()),
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
    let prior = lifecycle::acp_session_id(&state.db, &req.session_id).await?;
    let acp_session = match prior {
        Some(prior_id) if conn.capabilities().load_session => {
            let session = SessionId::new(prior_id);
            // A saved id can become unloadable — e.g. the container was recreated
            // and the agent no longer holds that session. Letting `session/load`
            // fail here would wedge the conversation at a permanent 502, because
            // the (unchanged) id is retried on every turn. Fall back to a fresh
            // session instead: the `/workspace` files survive, so only the agent's
            // in-memory chat history is lost, and we re-record the new id so later
            // turns stop retrying the dead one.
            match conn.load_session(session.clone(), params.clone()).await {
                Ok(()) => session,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session_id = %req.session_id,
                        acp_session_id = %session.0,
                        "coding-agent: session/load failed; starting a fresh agent session"
                    );
                    let session = conn.new_session(params).await?;
                    lifecycle::record_acp_session(&state.db, &req.session_id, &session.0).await?;
                    session
                }
            }
        }
        _ => {
            let session = conn.new_session(params).await?;
            lifecycle::record_acp_session(&state.db, &req.session_id, &session.0).await?;
            session
        }
    };

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
    tokio::spawn(drive_turn(
        conn,
        turn,
        out_tx,
        db,
        lease,
        lease_secs,
        heartbeat_secs,
        telemetry,
        req.session_token_id.clone(),
        cancel_session,
        turn_timeout,
    ));

    Ok(out_rx)
}

/// Per-turn telemetry accumulator plus the keys needed to persist it.
struct TurnTelemetryCtx {
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
    let user_id = req.user_id?;
    let agent_id = uuid::Uuid::parse_str(req.spec.container_id.as_str()).ok()?;
    let telemetry = TurnTelemetry::new(
        agent_id.to_string(),
        agent_name.to_owned(),
        acp_session_id.to_owned(),
        turn_id.to_owned(),
        req.prompt.clone(),
        CapturePolicy::MetadataOnly,
        Utc::now(),
    );
    Some(TurnTelemetryCtx {
        telemetry,
        user_id,
        agent_id,
        session_id: req.session_id.clone(),
    })
}

/// The background driver for one running turn.
#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    conn: AcpConnection,
    mut turn: TurnStream,
    out_tx: mpsc::UnboundedSender<CodingTurnEvent>,
    db: sqlx::PgPool,
    lease: TurnLease,
    lease_secs: i64,
    heartbeat_secs: u64,
    mut telemetry: Option<TurnTelemetryCtx>,
    session_token_id: Option<super::revocation::RevokedTokenId>,
    cancel_session: SessionId,
    turn_timeout: Duration,
) {
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
                    let terminal = ev.is_terminal();
                    // Stop if the caller hung up or the turn reached its end.
                    if out_tx.send(CodingTurnEvent::Turn(ev)).is_err() || terminal {
                        break;
                    }
                }
                None => break,
            },
            _ = beat.tick() => {
                // Renew the lease. If we've been fenced out (another owner took
                // over) or the DB errored, stop driving — we no longer own this
                // session and must not keep writing to its agent.
                if !matches!(lifecycle::heartbeat(&db, &lease, lease_secs).await, Ok(true)) {
                    break;
                }
            }
            _ = &mut deadline => {
                // Over budget: tell the agent to stop, then end the turn. Dropping
                // `conn` below closes the pipe as a backstop if cancel is ignored.
                let _ = conn.cancel(cancel_session.clone());
                let _ = out_tx.send(CodingTurnEvent::Turn(TurnEvent::Failed(AcpError::Cancelled(
                    format!("turn exceeded its {}s budget", turn_timeout.as_secs()),
                ))));
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
    let _ = lifecycle::release_turn(&db, &lease).await;

    // Revoke this turn's session token: the turn is over, so the credential has
    // no more legitimate use, and revoking it caps a leaked copy's lifetime to
    // the turn rather than the token's full TTL.
    if let Some(id) = session_token_id
        && let Err(e) = super::revocation::revoke_session_token(&db, &id.jti, id.expires_at).await
    {
        tracing::warn!(error = %e, jti = %id.jti, "coding-agent: session-token revoke failed");
    }
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
fn session_params(
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

/// Choose how the turn answers the agent's `session/request_permission` requests.
///
/// Auto mode (the default) approves everything so a turn runs unattended. When
/// the agent is `hitl_enabled`, route each request to the Nasiko HITL surface via
/// a [`super::permission::HitlPermissionDelegate`] — but only when the turn's user
/// and agent id are both known; if HITL is asked for and either is missing we
/// fail closed (reject every request) rather than silently auto-approving an
/// agent that was meant to be gated.
fn permission_policy(
    state: &AppState,
    req: &CodingTurnRequest,
    notice_tx: mpsc::UnboundedSender<CodingTurnEvent>,
) -> PermissionPolicy {
    if !req.hitl_enabled {
        return PermissionPolicy::ApproveAll;
    }
    match (
        uuid::Uuid::parse_str(req.spec.container_id.as_str()).ok(),
        req.user_id,
    ) {
        (Some(agent_id), Some(user_id)) => {
            PermissionPolicy::Delegate(Arc::new(super::permission::HitlPermissionDelegate::new(
                state.hitl_store.clone(),
                agent_id,
                user_id,
                req.session_id.clone(),
                Some(notice_tx),
            )))
        }
        _ => {
            tracing::warn!(
                session_id = %req.session_id,
                "coding HITL enabled but the turn's user or agent id is unknown; denying all tool use"
            );
            PermissionPolicy::RejectAll
        }
    }
}

/// This control-plane replica's stable identity, used as the lease owner so a
/// replica recognizes (and can renew) its own lease across turns.
fn replica_id() -> &'static str {
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
    use super::{CodingTurnError, gateway_mcp_servers, safe_session_label};

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
