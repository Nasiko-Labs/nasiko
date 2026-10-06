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

use std::sync::OnceLock;
use std::time::Duration;

use agent_client_protocol::ByteStreams;
use agent_client_protocol::schema::v1::{HttpHeader, McpServer, McpServerHttp};
use chrono::Utc;
use nasiko_acp_client::{
    AcpConnectConfig, AcpConnection, CapturePolicy, NewSessionParams, PermissionPolicy, TurnEvent,
    TurnStream, TurnTelemetry,
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
) -> Result<mpsc::UnboundedReceiver<TurnEvent>, CodingTurnError> {
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
) -> Result<mpsc::UnboundedReceiver<TurnEvent>, CodingTurnError> {
    let container_id = req.spec.container_id.clone();
    let handle = container_id.as_str().to_owned();

    // Make sure the container is up: deploy() is idempotent, so the warm path
    // (already running) is a near no-op and the cold path starts it. Then wait
    // for Running before attaching — a pod/container mid-start can't be exec'd.
    lifecycle::set_runtime_state(
        &state.db,
        &req.session_id,
        SessionRuntimeState::Starting,
        Some(&state.config.agent_runtime),
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
        None,
    )
    .await?;

    // Open the per-session stdio pipe, then bridge the runtime's tokio
    // AsyncRead/AsyncWrite halves to the futures-io the ACP SDK's ByteStreams
    // transport wants. ByteStreams::new(outgoing, incoming): outgoing is what we
    // write to the agent (its stdin), incoming is what we read (its stdout).
    let dir = workspace_session_name(&req.session_id);
    let attach = SessionAttach {
        session_id: dir.clone(),
        command: req.acp_command.clone(),
        env: req.env.clone(),
    };
    let stdio = state.runtime.attach_stdio(&container_id, attach).await?;
    let transport = ByteStreams::new(stdio.stdin.compat_write(), stdio.stdout.compat());
    // Auto mode for now: approve every permission request the agent makes mid-turn
    // so a turn runs unattended end to end. Real human-in-the-loop routing will
    // replace this with a `Delegate` policy to the Nasiko HITL surface (NAS-842).
    let conn = AcpConnection::connect(
        transport,
        AcpConnectConfig {
            permission_policy: PermissionPolicy::ApproveAll,
            ..Default::default()
        },
    )
    .await?;

    // Open the agent session in its per-session workspace dir (the same dir the
    // attach cd'd the process into), remember the agent's session id for a later
    // cold `session/load`, then send the prompt.
    let cwd =
        std::path::PathBuf::from(format!("{}/{}", DeploymentSpec::DEFAULT_WRITABLE_PATH, dir));
    let acp_session = conn
        .new_session(session_params(state, &req.session_token, cwd))
        .await?;
    lifecycle::record_acp_session(&state.db, &req.session_id, &acp_session.0).await?;

    // Accumulate a telemetry event from the turn's updates (NAS-841), attributed
    // to the user the proxy resolved. Metadata-only: the proxy already wrote the
    // chat session + user message, so this records the tool/turn metadata the
    // OTLP outbox exports without duplicating content.
    let telemetry = build_turn_telemetry(req, turn_id, &req.spec.name, &acp_session.0);

    let turn = conn.prompt(acp_session, req.prompt.clone())?;

    // Drive the turn in the background: forward events to the caller, renew the
    // lease while it runs, and release it (and close the pipe) when it ends.
    let (out_tx, out_rx) = mpsc::unbounded_channel::<TurnEvent>();
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
    out_tx: mpsc::UnboundedSender<TurnEvent>,
    db: sqlx::PgPool,
    lease: TurnLease,
    lease_secs: i64,
    heartbeat_secs: u64,
    mut telemetry: Option<TurnTelemetryCtx>,
) {
    let mut beat = tokio::time::interval(Duration::from_secs(heartbeat_secs));
    beat.tick().await; // the first tick fires immediately; skip it

    loop {
        tokio::select! {
            event = turn.recv() => match event {
                Some(ev) => {
                    // Fold streamed updates into the telemetry event before
                    // forwarding them on.
                    if let (Some(ctx), TurnEvent::Update(update)) = (telemetry.as_mut(), &ev) {
                        ctx.telemetry.observe(update, Utc::now());
                    }
                    let terminal = ev.is_terminal();
                    // Stop if the caller hung up or the turn reached its end.
                    if out_tx.send(ev).is_err() || terminal {
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

    // Dropping the connection closes the pipe; the per-session agent process
    // exits on stdin EOF while the container stays up for other sessions. Then
    // release the lease (which stamps `last_turn_at` for idle tracking).
    drop(conn);
    let _ = lifecycle::release_turn(&db, &lease).await;
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
fn session_params(
    state: &AppState,
    session_token: &Option<String>,
    cwd: std::path::PathBuf,
) -> NewSessionParams {
    let params = NewSessionParams::new(cwd);
    match (
        session_token,
        state.config.mcp_gateway_public_url.as_deref(),
    ) {
        (Some(token), Some(gateway_url)) => params.with_mcp_servers(vec![McpServer::Http(
            McpServerHttp::new("nasiko-gateway", gateway_url).headers(vec![HttpHeader::new(
                "Authorization",
                format!("Bearer {token}"),
            )]),
        )]),
        _ => params,
    }
}

/// A stable, filesystem- and shell-safe per-session directory name.
///
/// Most A2A context ids (`ses_<alnum>`, a UUID) are already valid
/// [`SessionAttach`] segments and pass through unchanged for readability;
/// anything else is mapped to a collision-resistant hash so two sessions can
/// never share one workspace directory.
fn workspace_session_name(session_id: &str) -> String {
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
    use super::workspace_session_name;

    #[test]
    fn safe_session_ids_pass_through_unchanged() {
        // The common shapes — a "ses_" id and a UUID — are already valid path
        // segments, so they stay readable.
        for id in [
            "ses_01HX8ZP7Q9",
            "6b1f2c9a-0e4d-4a1b-9c3e-2f5a7d8e1b0c",
            "plain_name-1",
        ] {
            assert_eq!(workspace_session_name(id), id);
        }
    }

    #[test]
    fn unsafe_session_ids_hash_to_a_safe_stable_name() {
        // Anything with a separator, colon, emptiness, or over-length becomes a
        // safe hashed name — deterministic and collision-resistant.
        for id in ["a/b", "has:colon", "", &"x".repeat(100)] {
            let name = workspace_session_name(id);
            assert!(name.starts_with("s-"), "{id:?} -> {name:?}");
            assert_eq!(name.len(), 34);
            assert!(
                name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "{name:?} must be a safe segment"
            );
            // Stable: same input always yields the same directory.
            assert_eq!(workspace_session_name(id), name);
        }
    }

    #[test]
    fn distinct_unsafe_ids_get_distinct_dirs() {
        assert_ne!(workspace_session_name("a/b"), workspace_session_name("a/c"));
    }
}
