//! One turn of a harness session, run on the `SandboxProvider`: take the session's turn lease,
//! acquire (or resume) its sandbox, attach over WebSocket, drive the ACP turn, stream it as A2A
//! SSE.
//!
//! The exec-based coding path (`coding::turn`) does the same job against a container it execs
//! into; this one talks to the shim inside a per-session sandbox. Everything that does not
//! depend on the transport is shared with it: the lease, the permission policy, the ACP session
//! open/restore, the turn driver (heartbeat, deadline, reply persistence, telemetry, token
//! revocation) and the SSE mapping.
//!
//! # Two credentials
//!
//! A sandbox's environment is applied whenever the provider creates or recreates it, and the
//! provider contract is that every acquire passes the full, stable env. So the *sandbox token*
//! is minted once, kept encrypted on the session row (`sessions::token`), and every acquire
//! decrypts it and rebuilds the whole env from it. It is replaced only when missing, unusable
//! (secret rotated) or within 24 h of expiry, and then the sandbox is stopped first so the new
//! env lands: suspended where the provider resumes it on the new env, released only where it
//! cannot (`sessions::token::stop_for_new_env`), so its files stay. The *turn token* is minted
//! per turn and handed to the agent only as the ACP MCP header, and is revoked when the turn
//! ends. A failed acquire revokes nothing: a container or pod that survived the failure still
//! carries the token.
//!
//! # The profile
//!
//! The model the harness asks the router for, and the per-harness settings that point it there,
//! are the session's *profile* (`sessions::profile`), rendered on every turn from the user's
//! default LLM config and the HITL toggle. It joins the env (and, for a harness that reads its
//! settings from disk, the `files` the shim writes before the harness starts). A handshake-mode
//! shim restarts the harness when the configuration changes, so a changed profile reaches a
//! running sandbox on the next turn; an env-mode sandbox carries the profile it was created with
//! until it is released and created again (Codex's HITL argv likewise).
//!
//! # Runtime state on failure
//!
//! The row says `starting` while a turn is being set up. A turn that fails before the sandbox
//! was acquired leaves the row as it found it when it already had a sandbox (`sandbox_ref` set),
//! and `parked` when it did not: parking a row whose sandbox is alive would orphan it. A turn
//! that fails after acquire leaves the row `running`, so the next turn re-attaches and the idle
//! reaper suspends it if nobody does.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::response::Response;
use nasiko_acp_client::{AcpConnectConfig, AcpConnection, AcpError, connect_ws};
use nasiko_runtime::{SandboxHandle, SandboxKey, SandboxTransport, WORKSPACE_MOUNT};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::catalog::{Harness, HarnessCatalog, spec_for_session};
use super::handshake::{
    Configure, ControlClient, HandshakeError, acp_tls, configure_body, control_http, prepare,
    sanitize_routing, upgrade_headers,
};
use super::identity::{mint_session_token, session_env, shim_token};
use super::keys::{session_key, set_sandbox_key};
use super::live::{LiveRegistration, LiveTurn};
use super::model::{SessionRow, SessionRuntimeState, SessionStore};
use super::pending::{clear_pending, set_pending};
use super::profile::{HarnessProfile, ModelChoice, ProfileInputs, model_choice_for_user};
use super::scratch::add_scratch_env;
use super::token::{TokenNeeds, ensure_sandbox_token};
use crate::coding::lifecycle::{self, TurnLease};
use crate::coding::permission::PendingPermissionSink;
use crate::coding::proxy::{CodingAgentKind, HarnessTarget, codex_hitl_args};
use crate::coding::questions::TurnQuestions;
use crate::coding::revocation::{RevokedTokenId, revoke_session_token};
use crate::coding::turn::{
    CodingTurnError, CodingTurnEvent, DriveTurn, HitlPolicyInput, ShimRestart, TelemetryKeys,
    hitl_permission_policy, new_turn_telemetry, open_acp_session, replica_id, session_params,
};
use crate::state::AppState;
use sqlx::PgPool;

/// How many times a refused or unreachable shim is retried right after `acquire`: the sandbox
/// may still be booting its shim when the provider returns.
const MAX_CONNECT_ATTEMPTS: u32 = 3;
const CONNECT_BACKOFF: Duration = Duration::from_millis(500);

/// How long a turn waits for the reaper to finish suspending the session's sandbox before it
/// reports the session busy. 15 s covers the common case; a Docker stop can take up to its 10 s
/// grace plus the operation timeout, after which the turn is `Busy` and the client retries.
/// `acquire` is create-or-resume, so once the row is `parked` the turn simply resumes the
/// sandbox.
const SUSPEND_WAIT: Duration = Duration::from_secs(15);
const SUSPEND_POLL: Duration = Duration::from_millis(250);

/// What one harness turn needs from its caller. The caller has already authorized the user,
/// resolved the session (`SessionStore::find_or_create_for_harness`) and opened the flow the
/// session token binds to.
pub struct HarnessTurnRequest<'a> {
    pub target: &'a HarnessTarget,
    pub session: SessionRow,
    pub user_id: Uuid,
    pub prompt: &'a str,
    pub flow_id: &'a str,
    pub task_id: &'a str,
}

/// Run the turn and return its events as A2A `message/stream` SSE. Its callers have asked the
/// session admission already (`super::admission`), before anything was written for the turn.
///
/// The turn is registered as a live task (its id is the request's task id) before it starts, so
/// `tasks/cancel` and `tasks/resubscribe` can reach it. Its frames are published to a broadcast;
/// this response is the first subscriber. Because a pump task, not the response, drains the
/// turn, the turn keeps running if this client disconnects (and can be re-attached or cancelled);
/// the turn deadline still bounds it.
pub async fn run_harness_turn_sse(
    state: &AppState,
    req: HarnessTurnRequest<'_>,
) -> Result<Response, CodingTurnError> {
    let task_id = req.task_id.to_owned();
    let context_id = req.session.session_id.clone();
    // Registered before anything is awaited, and held by an RAII guard: if this request is
    // dropped while the sandbox is still being provisioned, or the start fails, the entry goes
    // with it. On success the driver keeps it until the turn has finished.
    let (live, first) = LiveTurn::register(&task_id);
    let events = run_harness_turn(state, req, live.clone()).await?;
    let frames = crate::coding::sse::frames(events, task_id.clone(), context_id);
    let publisher = (**live).clone();
    // Other replicas follow this turn through Redis; the local subscribers never do.
    let remote = super::live::FramePublisher::spawn(state.redis.client(), &task_id);
    tokio::spawn(async move {
        futures::pin_mut!(frames);
        while let Some(frame) = futures::StreamExt::next(&mut frames).await {
            publisher.publish(frame.clone());
            remote.publish(frame).await;
        }
    });
    Ok(super::live::sse(first))
}

/// A turn that failed while starting, and the state the session should be left in.
struct StartFailure {
    error: CodingTurnError,
    leave: SessionRuntimeState,
}

impl StartFailure {
    /// Failed before a sandbox was acquired.
    fn before_acquire(session: &SessionRow, error: impl Into<CodingTurnError>) -> Self {
        Self {
            error: error.into(),
            leave: prior_state(session),
        }
    }

    /// Failed with the sandbox up.
    fn running(error: impl Into<CodingTurnError>) -> Self {
        Self {
            error: error.into(),
            leave: SessionRuntimeState::Running,
        }
    }
}

/// The state a session had before this turn began: its own state when it already owns a sandbox
/// (`running`, or `starting` when another attempt was mid-acquire; never `parked` while a sandbox
/// exists), `parked` when it has none.
fn prior_state(session: &SessionRow) -> SessionRuntimeState {
    if session.sandbox_ref.is_none() {
        return SessionRuntimeState::Parked;
    }
    match session.runtime_state.as_str() {
        "running" => SessionRuntimeState::Running,
        "starting" => SessionRuntimeState::Starting,
        _ => SessionRuntimeState::Parked,
    }
}

/// Start the turn. A turn that does not start (busy, superseded, or failed setup) withdraws its
/// flow, so the rejected message is not mistaken for the session's latest turn.
async fn run_harness_turn(
    state: &AppState,
    req: HarnessTurnRequest<'_>,
    live: Arc<LiveRegistration>,
) -> Result<mpsc::UnboundedReceiver<CodingTurnEvent>, CodingTurnError> {
    let (flow_id, agent_id) = (req.flow_id.to_owned(), req.target.agent_id);
    let started = start_harness_turn(state, req, live).await;
    if started.is_err() {
        crate::flows::abandon_rejected(&state.db, &flow_id, agent_id).await;
    }
    started
}

async fn start_harness_turn(
    state: &AppState,
    req: HarnessTurnRequest<'_>,
    live: Arc<LiveRegistration>,
) -> Result<mpsc::UnboundedReceiver<CodingTurnEvent>, CodingTurnError> {
    // The turn id is the A2A task id, so `tasks/*` find the turn by the id the client was given.
    let turn_id = req.task_id.to_owned();
    let session_id = req.session.session_id.clone();
    let lease = acquire_lease(state, &session_id, &turn_id).await?;

    // Past this point a failure must release the lease, or the session is wedged until it
    // lapses. The row the caller resolved may be stale by now (a suspend or a clear can have
    // settled while the lease was awaited), so every decision below reads it again.
    let session = match SessionStore::new(state.db.clone()).get(&session_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            let _ = lifecycle::release_turn(&state.db, &lease).await;
            return Err(CodingTurnError::Setup("session no longer exists".into()));
        }
        Err(e) => {
            let _ = lifecycle::release_turn(&state.db, &lease).await;
            return Err(e.into());
        }
    };
    let req = HarnessTurnRequest { session, ..req };

    let minted = match mint_session_token(
        &state.config,
        req.target.agent_id,
        req.user_id,
        &session_id,
        req.flow_id,
    ) {
        Ok(minted) => minted,
        Err(e) => {
            let _ = lifecycle::release_turn(&state.db, &lease).await;
            return Err(CodingTurnError::from_credential("session credential", e));
        }
    };
    let token_id = RevokedTokenId::from_minted(&minted);

    let (setup_beat, lease_lost) = StartHeartbeat::spawn(state, &lease);
    let ctx = TurnCtx {
        state,
        req: &req,
        lease: &lease,
        turn_id: &turn_id,
        live,
        lease_lost,
        questions: TurnQuestions::default(),
    };
    let started = start_turn(&ctx, minted.token, &token_id).await;
    drop(setup_beat);
    match started {
        Ok(rx) => Ok(rx),
        Err(failure) => {
            // State first, then the lease: releasing it first would let another turn start in
            // between and have this write land on its row.
            let left = SessionStore::new(state.db.clone())
                .set_runtime_state_fenced(&lease, failure.leave)
                .await;
            warn_missed_write("leave state", &session_id, left);
            let _ = lifecycle::release_turn(&state.db, &lease).await;
            let _ = revoke_session_token(&state.db, &token_id.jti, token_id.expires_at).await;
            Err(failure.error)
        }
    }
}

/// Take the session's turn lease, waiting out an in-progress suspend (up to [`SUSPEND_WAIT`]).
/// `Busy` when another turn holds it, or the suspend does not finish in time.
async fn acquire_lease(
    state: &AppState,
    session_id: &str,
    turn_id: &str,
) -> Result<TurnLease, CodingTurnError> {
    let store = SessionStore::new(state.db.clone());
    let deadline = tokio::time::Instant::now() + SUSPEND_WAIT;
    loop {
        let lease = lifecycle::acquire_turn(
            &state.db,
            session_id,
            replica_id(),
            turn_id,
            state.config.coding_agent_turn_lease_secs,
        )
        .await?;
        if let Some(lease) = lease {
            return Ok(lease);
        }
        let suspending = store
            .get(session_id)
            .await?
            .is_some_and(|row| is_suspending(&row));
        if !suspending || tokio::time::Instant::now() >= deadline {
            return Err(CodingTurnError::Busy);
        }
        tokio::time::sleep(SUSPEND_POLL).await;
    }
}

/// Log a failure-path state write that missed because a suspend claim took the lease: the row
/// now belongs to the claim, and there is nothing left for this turn to write.
fn warn_missed_write(what: &str, session_id: &str, result: sqlx::Result<bool>) {
    match result {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(%session_id, "{what}: turn superseded by a suspend; write skipped")
        }
        Err(e) => tracing::warn!(%session_id, error = %e, "{what}: write failed"),
    }
}

/// A fenced state write the turn depends on. A write that matched no row means a suspend claim
/// took the lease and fence, so the turn must stop before it touches the sandbox again.
fn require_fence(result: sqlx::Result<bool>) -> Result<(), CodingTurnError> {
    if result? {
        Ok(())
    } else {
        Err(CodingTurnError::Superseded)
    }
}

/// Keeps the turn's lease alive while the turn is being set up (`acquire` can outlast a lease),
/// the way `drive_turn` does once it runs, and raises a flag when the lease is lost so the next
/// gate stops the turn even if the database cannot be asked. Stops when dropped.
struct StartHeartbeat(tokio::task::JoinHandle<()>);

impl StartHeartbeat {
    fn spawn(state: &AppState, lease: &TurnLease) -> (Self, Arc<AtomicBool>) {
        let db = state.db.clone();
        let lease = lease.clone();
        let lease_secs = state.config.coding_agent_turn_lease_secs;
        let every = Duration::from_secs(state.config.coding_agent_lease_heartbeat_secs.max(1));
        let lost = Arc::new(AtomicBool::new(false));
        let flag = lost.clone();
        let task = tokio::spawn(async move {
            let mut beat = tokio::time::interval(every);
            beat.tick().await;
            loop {
                beat.tick().await;
                if lifecycle::heartbeat_retrying(&db, &lease, lease_secs, every).await
                    == lifecycle::Beat::Lost
                {
                    flag.store(true, Ordering::SeqCst);
                    return;
                }
            }
        });
        (Self(task), lost)
    }
}

impl Drop for StartHeartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Whether the idle reaper is mid-suspend on this session.
fn is_suspending(row: &SessionRow) -> bool {
    row.runtime_state == SessionRuntimeState::Stopping.as_str()
}

/// What every step of starting a turn needs, so the steps take one argument for it.
struct TurnCtx<'a, 'r> {
    state: &'a AppState,
    req: &'a HarnessTurnRequest<'r>,
    lease: &'a TurnLease,
    turn_id: &'a str,
    /// The turn's place in the live registry: its cancel token, and the registration the driver
    /// keeps until the turn is over.
    live: Arc<LiveRegistration>,
    /// Raised by [`StartHeartbeat`] when the lease is lost.
    lease_lost: Arc<AtomicBool>,
    /// The questions the turn asks a person: the delegate notes each one, and the driver closes
    /// the ones still open when the turn ends.
    questions: TurnQuestions,
}

impl TurnCtx<'_, '_> {
    fn session_id(&self) -> &str {
        &self.req.session.session_id
    }

    /// Stop here if the lease was lost while an earlier step ran.
    fn gate(&self) -> Result<(), CodingTurnError> {
        if self.lease_lost.load(Ordering::SeqCst) {
            Err(CodingTurnError::Superseded)
        } else {
            Ok(())
        }
    }

    /// A last lease check, immediately before the prompt goes out: the lease is renewed, or the
    /// turn is [`Superseded`](CodingTurnError::Superseded). An unreachable database does not
    /// stop it; the driver's own heartbeat takes over from here.
    async fn renew_before_prompt(&self) -> Result<(), CodingTurnError> {
        let budget =
            Duration::from_secs(self.state.config.coding_agent_lease_heartbeat_secs.max(1));
        let secs = self.state.config.coding_agent_turn_lease_secs;
        match lifecycle::heartbeat_retrying(&self.state.db, self.lease, secs, budget).await {
            lifecycle::Beat::Lost => Err(CodingTurnError::Superseded),
            lifecycle::Beat::Renewed | lifecycle::Beat::Unreachable => Ok(()),
        }
    }
}

/// Everything between holding the lease and handing back the event stream.
async fn start_turn(
    ctx: &TurnCtx<'_, '_>,
    turn_token: String,
    token_id: &RevokedTokenId,
) -> Result<mpsc::UnboundedReceiver<CodingTurnEvent>, StartFailure> {
    // The first fenced write is the gate: a suspend claim that took the lease since it was
    // acquired makes it miss, and the turn stops here, before it touches the sandbox.
    let started = SessionStore::new(ctx.state.db.clone())
        .set_runtime_state_fenced(ctx.lease, SessionRuntimeState::Starting)
        .await;
    require_fence(started).map_err(|e| StartFailure::before_acquire(&ctx.req.session, e))?;
    // Record which sandbox is about to be created, under the lease, before anything can create it:
    // `sandbox_ref` is only written after the provider answers, and the orphan pass must be able to
    // tell a first acquire's sandbox from an orphan.
    let keyed = set_sandbox_key(&ctx.state.db, ctx.lease, &session_key(&ctx.req.session)).await;
    require_fence(keyed).map_err(|e| StartFailure::before_acquire(&ctx.req.session, e))?;

    let (handle, env) = provision_sandbox(ctx).await?;
    ctx.gate().map_err(StartFailure::running)?;
    begin_prompt(ctx, turn_token, token_id, &handle, env).await
}

/// Get the session a sandbox with its full env: harness, token, spec, acquire, record the ref.
/// Nothing here revokes a credential when `acquire` fails.
async fn provision_sandbox(
    ctx: &TurnCtx<'_, '_>,
) -> Result<(SandboxHandle, HashMap<String, String>), StartFailure> {
    let (state, req) = (ctx.state, ctx.req);
    let early = |e: CodingTurnError| StartFailure::before_acquire(&req.session, e);
    let hitl = hitl_kind(&req.target.harness_id, req.target.hitl_enabled).map_err(early)?;
    let harness = turn_harness(state, req.target, hitl).await.map_err(early)?;
    let token = ensure_sandbox_token(&TokenNeeds {
        state,
        lease: ctx.lease,
        session: &req.session,
        agent_id: req.target.agent_id,
        user_id: req.user_id,
        flow_id: req.flow_id,
    })
    .await
    .map_err(early)?;
    ctx.gate().map_err(early)?;
    let workspace = workspace_env(state, req).await;
    let gateway = nasiko_llm_router::GatewayConfig::from_env();
    let profile = turn_profile(state, req, &gateway, hitl.is_some()).await;
    let env = sandbox_env(
        state,
        &session_key(&req.session),
        &token,
        &gateway.llm_gateway_base_url,
        profile,
        workspace,
    )
    .map_err(early)?;
    let spec = spec_for_session(
        &harness,
        &req.session,
        env,
        req.target.owner_id,
        &state.config,
    );
    let handle = state
        .sandbox
        .acquire(&spec)
        .await
        .map_err(|e| early(e.into()))?;
    let recorded = SessionStore::new(state.db.clone())
        .set_sandbox_ref_fenced(ctx.lease, &handle.sandbox_ref())
        .await;
    require_fence(recorded).map_err(StartFailure::running)?;
    // The decorator adds the platform's OTEL env inside `acquire`; a shim configured over HTTP
    // gets the same additions here.
    let mut env = spec.env;
    env.extend(crate::sandbox_wiring::session_env_additions(
        &state.config,
        &spec.key,
    ));
    Ok((handle, env))
}

/// Attach to the sandbox, open the agent session, send the prompt and hand the turn to its driver.
async fn begin_prompt(
    ctx: &TurnCtx<'_, '_>,
    turn_token: String,
    token_id: &RevokedTokenId,
    handle: &SandboxHandle,
    env: HashMap<String, String>,
) -> Result<mpsc::UnboundedReceiver<CodingTurnEvent>, StartFailure> {
    let (state, req, lease) = (ctx.state, ctx.req, ctx.lease);
    // The delegate runs off the ACP loop and announces approvals on the same channel the turn's
    // updates flow through.
    let (out_tx, out_rx) = mpsc::unbounded_channel::<CodingTurnEvent>();
    let policy = turn_policy(ctx, out_tx.clone());
    let (
        Attached {
            conn,
            url,
            routing,
            http,
            workspace,
        },
        bearer,
    ) = connect_shim(ctx, handle, env, policy)
        .await
        .map_err(StartFailure::running)?;
    let shim_restart = ShimRestart {
        client: http,
        ws_url: url,
        bearer,
        headers: routing,
    };
    ctx.gate().map_err(StartFailure::running)?;
    let running = SessionStore::new(state.db.clone())
        .set_runtime_state_fenced(lease, SessionRuntimeState::Running)
        .await;
    require_fence(running).map_err(StartFailure::running)?;

    let cwd = session_cwd(workspace.as_deref());
    let params = session_params(state, &Some(turn_token), cwd, conn.capabilities().mcp_http)
        .map_err(StartFailure::running)?;
    let acp_session = open_acp_session(&state.db, &conn, lease, params)
        .await
        .map_err(StartFailure::running)?;
    // The session may have been loaded, which writes nothing fenced: check the lease directly.
    ctx.gate().map_err(StartFailure::running)?;
    ctx.renew_before_prompt()
        .await
        .map_err(StartFailure::running)?;

    let telemetry = new_turn_telemetry(TelemetryKeys {
        user_id: Some(req.user_id),
        agent_id: Some(req.target.agent_id),
        session_id: ctx.session_id(),
        turn_id: ctx.turn_id,
        agent_name: &req.target.name,
        acp_session_id: &acp_session.0,
        prompt: req.prompt,
    });
    let cancel_session = acp_session.clone();
    let turn = conn
        .prompt(acp_session, req.prompt.to_owned())
        .map_err(StartFailure::running)?;
    let driver = DriveTurn {
        conn,
        turn,
        out_tx,
        db: state.db.clone(),
        lease: lease.clone(),
        lease_secs: state.config.coding_agent_turn_lease_secs,
        heartbeat_secs: state.config.coding_agent_lease_heartbeat_secs.max(1),
        telemetry,
        session_token_id: Some(token_id.clone()),
        cancel_session,
        turn_timeout: Duration::from_secs(state.config.agent_call_timeout_secs),
        turn_id: ctx.turn_id.to_owned(),
        cancel: Some(ctx.live.cancel.clone()),
        keep_alive: Some(ctx.live.clone()),
        shim_restart: Some(shim_restart),
        questions: ctx.questions.clone(),
    };
    tokio::spawn(driver.run());
    Ok(out_rx)
}

/// The permission policy of this turn's agent session.
fn turn_policy(
    ctx: &TurnCtx<'_, '_>,
    out_tx: mpsc::UnboundedSender<CodingTurnEvent>,
) -> nasiko_acp_client::PermissionPolicy {
    let (state, req) = (ctx.state, ctx.req);
    hitl_permission_policy(
        state,
        HitlPolicyInput {
            hitl_enabled: req.target.hitl_enabled,
            agent_id: Some(req.target.agent_id),
            user_id: Some(req.user_id),
            session_id: ctx.session_id(),
            pending_sink: Some(Arc::new(SessionPendingSink {
                db: state.db.clone(),
                lease: ctx.lease.clone(),
            })),
            cancel: Some(ctx.live.cancel.clone()),
            questions: ctx.questions.clone(),
        },
        out_tx,
    )
}

/// Mint the shim bearer, build the configuration once, and connect to the shim.
async fn connect_shim(
    ctx: &TurnCtx<'_, '_>,
    handle: &SandboxHandle,
    env: HashMap<String, String>,
    policy: nasiko_acp_client::PermissionPolicy,
) -> Result<(Attached, String), CodingTurnError> {
    let (state, req) = (ctx.state, ctx.req);
    let bearer = shim_token(&state.config.agent_jwt_secret, &session_key(&req.session))
        .map_err(|e| CodingTurnError::Setup(format!("shim bearer: {e}")))?;
    let configure = configure_body(&bearer, env);
    let target = ShimTarget {
        state,
        handle,
        session_id: ctx.session_id(),
        bearer: &bearer,
        configure: &configure,
        lease_lost: &ctx.lease_lost,
    };
    let attached = connect_with_retry(&target, policy).await?;
    Ok((attached, bearer))
}

/// The approval mode a HITL-gated harness needs, from its catalog id. `None` when HITL is off. A
/// harness whose kind is unknown cannot be gated, so HITL on it is an error rather than a turn
/// that runs unguarded.
fn hitl_kind(
    harness_id: &str,
    hitl_enabled: bool,
) -> Result<Option<CodingAgentKind>, CodingTurnError> {
    if !hitl_enabled {
        return Ok(None);
    }
    CodingAgentKind::from_harness_id(harness_id)
        .map(Some)
        .map_err(|_| {
            CodingTurnError::Setup(format!(
                "HITL requested but harness {harness_id} has no approval mode"
            ))
        })
}

/// The catalog entry a turn runs, with the HITL-forcing argv applied where the harness needs it.
async fn turn_harness(
    state: &AppState,
    target: &HarnessTarget,
    hitl: Option<CodingAgentKind>,
) -> Result<Harness, CodingTurnError> {
    let harness = HarnessCatalog::new(state.db.clone())
        .get(&target.harness_id)
        .await?
        .filter(|h| h.enabled)
        .ok_or_else(|| {
            CodingTurnError::NotConfigured(format!(
                "Harness '{}' is not enabled on this server",
                target.harness_id
            ))
        })?;
    Ok(with_hitl_argv(harness, hitl))
}

/// Codex only asks before acting when told to on its command line (OpenCode is nudged through
/// env, see [`sandbox_env`]), so a HITL-gated Codex harness gets the approval flags appended to
/// the catalog's own argv, after any of its own (the last `-c` wins). The kind comes from the
/// catalog id, not the image name.
fn with_hitl_argv(mut harness: Harness, hitl: Option<CodingAgentKind>) -> Harness {
    if hitl == Some(CodingAgentKind::Codex) {
        harness.acp_command.extend(codex_hitl_args());
    }
    harness
}

/// The session's profile for this turn: the user's model choice rendered for the harness's
/// kind. A harness whose kind is unknown gets no profile (its image's own defaults apply).
async fn turn_profile(
    state: &AppState,
    req: &HarnessTurnRequest<'_>,
    gateway: &nasiko_llm_router::GatewayConfig,
    hitl: bool,
) -> HarnessProfile {
    let Ok(kind) = CodingAgentKind::from_harness_id(&req.target.harness_id) else {
        tracing::info!(harness = %req.target.harness_id, "harness has no known kind; no profile rendered");
        return HarnessProfile::default();
    };
    let choice = model_choice_for_user(&state.db, req.user_id, gateway).await;
    let platform = ModelChoice {
        provider: gateway.default_provider.clone(),
        model: gateway.default_model.clone(),
        source: super::profile::ModelSource::PlatformDefault,
    };
    tracing::debug!(
        harness = %req.target.harness_id,
        provider = %choice.provider,
        model = %choice.model,
        source = ?choice.source,
        "harness profile: model choice"
    );
    super::profile::render(ProfileInputs {
        kind,
        choice: &choice,
        platform: &platform,
        llm_base_url: &gateway.llm_gateway_base_url,
        hitl,
    })
}

/// The sandbox's full env, rebuilt on every acquire from the stored token: its credentials and
/// gateway wiring, the shim's bearer, the session's profile (the harness's model and
/// permission settings, including any files the shim writes for it), and the scratch folders
/// its package caches and build output go to (`sessions::scratch`), each unless the profile
/// already sets it.
///
/// Deliberately not the agent's deploy-time env (`agents::deploy_env::build`): a harness agent
/// is one shared row owned by the admin who seeded it, so its secrets and rotating agent-scoped
/// tokens must never reach another user's sandbox. The sandbox token is the sandbox's identity
/// for the router and the MCP gateway.
fn sandbox_env(
    state: &AppState,
    key: &SandboxKey,
    sandbox_token: &str,
    llm_gateway_base_url: &str,
    profile: HarnessProfile,
    workspace: std::collections::HashMap<String, String>,
) -> Result<std::collections::HashMap<String, String>, CodingTurnError> {
    let mcp_url = state.config.mcp_gateway_public_url.as_deref().unwrap_or("");
    let mut env = session_env(sandbox_token, llm_gateway_base_url, mcp_url);
    env.insert(
        "HARNESS_SHIM_TOKEN".to_owned(),
        shim_token(&state.config.agent_jwt_secret, key)
            .map_err(|e| CodingTurnError::Setup(format!("shim bearer: {e}")))?,
    );
    profile.apply_to(&mut env);
    env.extend(workspace);
    add_scratch_env(&mut env, key);
    Ok(env)
}

/// The env that has the shim clone the session's repository into the workspace on the
/// sandbox's first start: `WORKSPACE_REPO` from the row, and `WORKSPACE_REPO_TOKEN` when the
/// repository is on `github.com` and the session's user has connected GitHub.
///
/// That token is the user's own credential going into the user's own sandbox. It is stored
/// encrypted at rest like the user's other GitHub token, but in the sandbox it is readable by any
/// process in it, as the sandbox token is, and the shim keeps it from the harness's environment.
/// Every other repository is cloned anonymously.
async fn workspace_env(
    state: &AppState,
    req: &HarnessTurnRequest<'_>,
) -> std::collections::HashMap<String, String> {
    let mut env = std::collections::HashMap::new();
    let Some(repo) = req
        .session
        .workspace_repo
        .as_deref()
        .filter(|r| !r.trim().is_empty())
    else {
        return env;
    };
    env.insert("WORKSPACE_REPO".to_owned(), repo.to_owned());
    if is_github_https(repo)
        && let Some(token) = crate::github::load_github_token(&state.db, req.user_id).await
    {
        env.insert("WORKSPACE_REPO_TOKEN".to_owned(), token);
    }
    env
}

/// Whether `repo` is an `https://github.com/` URL, the only host the user's GitHub token is for.
fn is_github_https(repo: &str) -> bool {
    repo.trim()
        .to_ascii_lowercase()
        .starts_with("https://github.com/")
}

/// The directory the ACP session runs in: what the shim reported, else the fixed mount.
fn session_cwd(reported: Option<&str>) -> std::path::PathBuf {
    reported
        .filter(|w| !w.trim().is_empty())
        .map(Into::into)
        .unwrap_or_else(|| std::path::PathBuf::from(WORKSPACE_MOUNT))
}

/// An open ACP connection to a sandbox's shim, with what the control calls on the same shim need.
struct Attached {
    conn: AcpConnection,
    /// The WebSocket URL the sandbox attached us to.
    url: String,
    /// The provider's routing headers (without any `Authorization`), repeated on every control call.
    routing: Vec<(String, String)>,
    /// The client that reached the shim's control endpoints, trusting the sandbox's CA if it has
    /// one; the restart of a cancelled turn reuses it.
    http: reqwest::Client,
    /// The directory the shim reported serving, if any: the ACP session's cwd (see [`session_cwd`]).
    workspace: Option<String>,
}

/// What reaching one session's shim needs.
struct ShimTarget<'a> {
    state: &'a AppState,
    handle: &'a SandboxHandle,
    session_id: &'a str,
    bearer: &'a str,
    /// The session's configuration, built once for every attempt.
    configure: &'a Configure,
    lease_lost: &'a AtomicBool,
}

/// Why one attempt to reach the shim failed.
#[derive(Debug, thiserror::Error)]
enum AttachFailure {
    #[error("{0}")]
    Handshake(HandshakeError),
    #[error("{0}")]
    Acp(AcpError),
    /// The sandbox could not be attached or offers no WebSocket: never retried.
    #[error("{0}")]
    Other(CodingTurnError),
}

impl AttachFailure {
    /// Whether a fresh attempt could succeed: only a shim that is not listening yet.
    fn is_retryable(&self) -> bool {
        match self {
            AttachFailure::Handshake(e) => e.is_retryable(),
            AttachFailure::Acp(e) => e.is_retryable(),
            AttachFailure::Other(_) => false,
        }
    }
}

impl From<AttachFailure> for CodingTurnError {
    fn from(f: AttachFailure) -> Self {
        match f {
            AttachFailure::Handshake(e) => e.into(),
            AttachFailure::Acp(e) => e.into(),
            AttachFailure::Other(e) => e,
        }
    }
}

/// Attach and open the ACP connection, retrying while the shim may still be coming up.
async fn connect_with_retry(
    target: &ShimTarget<'_>,
    policy: nasiko_acp_client::PermissionPolicy,
) -> Result<Attached, CodingTurnError> {
    let mut attempt = 1;
    loop {
        match attach_once(target, policy.clone()).await {
            Ok(attached) => return Ok(attached),
            Err(failure) => match retry_delay(&failure, attempt) {
                Some(delay) => {
                    tracing::info!(session_id = %target.session_id, attempt, error = %failure, "harness shim not reachable yet; retrying");
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                None => {
                    if let AttachFailure::Handshake(e) = &failure {
                        tracing::warn!(session_id = %target.session_id, error = %e, "harness shim control call failed");
                    }
                    return Err(failure.into());
                }
            },
        }
    }
}

/// One attempt: attach, bring the shim to the session's configuration, open the ACP connection.
async fn attach_once(
    target: &ShimTarget<'_>,
    policy: nasiko_acp_client::PermissionPolicy,
) -> Result<Attached, AttachFailure> {
    let transport = target
        .state
        .sandbox
        .attach(target.handle)
        .await
        .map_err(|e| AttachFailure::Other(e.into()))?;
    let (url, routing, tls) = match transport {
        SandboxTransport::WebSocket { url, headers, tls } => {
            (url, sanitize_routing(headers, target.session_id), tls)
        }
        other => {
            return Err(AttachFailure::Other(CodingTurnError::Setup(format!(
                "sandbox offered {other:?}, a harness needs a WebSocket transport"
            ))));
        }
    };
    let http =
        control_http(&target.state.http_client, tls.as_ref()).map_err(AttachFailure::Handshake)?;
    let control = ControlClient {
        http: &http,
        base_ws_url: &url,
        routing: &routing,
        bearer: target.bearer,
    };
    let prepared = prepare(&control, target.configure, target.lease_lost)
        .await
        .map_err(AttachFailure::Handshake)?;
    if prepared.evicting {
        tracing::warn!(session_id = %target.session_id, "turn started on a sandbox its backend is evicting");
    }
    let config = AcpConnectConfig {
        permission_policy: policy,
        tls: tls.as_ref().map(acp_tls),
        ..Default::default()
    };
    let headers = upgrade_headers(&routing, target.bearer);
    let conn = connect_ws(&url, &headers, config)
        .await
        .map_err(AttachFailure::Acp)?;
    Ok(Attached {
        conn,
        url,
        routing,
        http,
        workspace: prepared.workspace,
    })
}

/// How long to wait before the next attempt, or `None` to give up. Only an unreachable shim is
/// worth retrying, and only [`MAX_CONNECT_ATTEMPTS`] times in all.
fn retry_delay(failure: &AttachFailure, attempt: u32) -> Option<Duration> {
    (failure.is_retryable() && attempt < MAX_CONNECT_ATTEMPTS).then_some(CONNECT_BACKOFF)
}

/// Persists a pending HITL approval pointer on the session row, so a suspend mid-wait does not
/// lose it.
struct SessionPendingSink {
    db: PgPool,
    /// The turn's lease: a pointer written under a superseded turn must not land.
    lease: TurnLease,
}

#[async_trait]
impl PendingPermissionSink for SessionPendingSink {
    async fn set_pending(&self, hitl_id: Uuid, tool_call_id: &str) {
        if let Err(e) = set_pending(&self.db, &self.lease, hitl_id, tool_call_id).await {
            tracing::warn!(error = %e, session_id = %self.lease.session_id, "could not persist pending permission");
        }
    }

    async fn clear_pending(&self, hitl_id: Uuid) {
        if let Err(e) = clear_pending(&self.db, &self.lease, hitl_id).await {
            tracing::warn!(error = %e, session_id = %self.lease.session_id, "could not clear pending permission");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::token::{RENEW_WITHIN_SECS, has_life};
    use nasiko_acp_client::ConnectFailure;
    use std::path::PathBuf;

    #[test]
    fn the_acp_cwd_is_the_reported_workspace_else_the_mount() {
        assert_eq!(session_cwd(None), PathBuf::from("/workspace"));
        assert_eq!(session_cwd(Some("  ")), PathBuf::from("/workspace"));
        assert_eq!(
            session_cwd(Some("/workspace/s-x")),
            PathBuf::from("/workspace/s-x")
        );
    }

    fn connect_failed(kind: ConnectFailure) -> AcpError {
        AcpError::ConnectFailed {
            kind,
            message: String::new(),
        }
    }

    fn unreachable() -> AttachFailure {
        AttachFailure::Acp(connect_failed(ConnectFailure::Unreachable))
    }

    #[test]
    fn unreachable_is_retried_until_the_attempts_run_out() {
        assert_eq!(retry_delay(&unreachable(), 1), Some(CONNECT_BACKOFF));
        assert_eq!(
            retry_delay(&unreachable(), MAX_CONNECT_ATTEMPTS - 1),
            Some(CONNECT_BACKOFF)
        );
        assert_eq!(retry_delay(&unreachable(), MAX_CONNECT_ATTEMPTS), None);
    }

    #[test]
    fn other_failures_are_not_retried() {
        for kind in [
            ConnectFailure::Tls,
            ConnectFailure::Rejected { status: 401 },
            ConnectFailure::InvalidRequest,
        ] {
            let failure = AttachFailure::Acp(connect_failed(kind));
            assert_eq!(retry_delay(&failure, 1), None, "{kind:?}");
        }
        let closed = AttachFailure::Acp(AcpError::ConnectionClosed);
        assert_eq!(retry_delay(&closed, 1), None);
        let superseded = AttachFailure::Handshake(HandshakeError::Superseded);
        assert_eq!(retry_delay(&superseded, 1), None);
    }

    fn stopping_row() -> SessionRow {
        SessionRow {
            session_id: "s".into(),
            user_id: Uuid::nil(),
            agent_id: None,
            acp_session_id: None,
            runtime_handle: None,
            runtime_state: "stopping".into(),
            volume_ref: None,
            turn_owner: None,
            lease_expires_at: None,
            turn_fence: 0,
            last_turn_at: None,
            active_turn_id: None,
            sandbox_ref: None,
            suspended_at: None,
            workspace_repo: None,
            pending_permission: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn only_a_stopping_row_is_suspending() {
        let mut row = stopping_row();
        assert!(is_suspending(&row));
        row.runtime_state = "running".into();
        assert!(!is_suspending(&row));
    }

    fn harness(image: &str) -> Harness {
        Harness {
            id: "h".into(),
            name: "H".into(),
            image: image.into(),
            acp_command: vec!["catalog-argv".into()],
            protocol_version: 1,
            min_cpu_milli: 500,
            min_memory: "1Gi".into(),
            enabled: true,
        }
    }

    #[test]
    fn hitl_codex_appends_the_approval_flags_to_the_catalog_argv() {
        let codex = harness("mirror.example/anything:1");
        let forced = with_hitl_argv(codex.clone(), Some(CodingAgentKind::Codex));
        assert_eq!(forced.acp_command[0], "catalog-argv");
        assert!(
            forced
                .acp_command
                .iter()
                .any(|a| a.contains("approval_policy"))
        );
        assert_eq!(with_hitl_argv(codex, None).acp_command, ["catalog-argv"]);
    }

    #[test]
    fn only_github_https_urls_get_the_users_github_token() {
        assert!(is_github_https("https://github.com/acme/app.git"));
        assert!(is_github_https("  HTTPS://GitHub.com/acme/app"));
        assert!(!is_github_https("https://github.com.evil.example/acme/app"));
        assert!(!is_github_https("git@github.com:acme/app.git"));
        assert!(!is_github_https("https://gitlab.com/acme/app"));
    }

    #[test]
    fn only_codex_takes_approval_flags() {
        let other = with_hitl_argv(harness("x:1"), Some(CodingAgentKind::OpenCode));
        assert_eq!(other.acp_command, ["catalog-argv"]);
    }

    #[test]
    fn hitl_on_an_unknown_harness_fails_closed() {
        assert_eq!(hitl_kind("mystery", false).unwrap(), None);
        assert_eq!(
            hitl_kind("codex", true).unwrap(),
            Some(CodingAgentKind::Codex)
        );
        let err = hitl_kind("mystery", true).unwrap_err().to_string();
        assert!(err.contains("has no approval mode"), "{err}");
    }

    #[test]
    fn a_token_near_expiry_is_not_reused() {
        let now = 1_000_000;
        assert!(has_life(now + RENEW_WITHIN_SECS + 1, now));
        assert!(!has_life(now + RENEW_WITHIN_SECS, now));
        assert!(!has_life(now, now));
    }

    fn session(sandbox_ref: Option<&str>, state: &str) -> SessionRow {
        SessionRow {
            sandbox_ref: sandbox_ref.map(str::to_owned),
            runtime_state: state.into(),
            ..stopping_row()
        }
    }

    #[test]
    fn a_starting_row_with_a_sandbox_keeps_its_state_and_never_parks() {
        assert_eq!(
            prior_state(&session(Some("fake:x"), "starting")),
            SessionRuntimeState::Starting
        );
        assert_eq!(
            prior_state(&session(None, "starting")),
            SessionRuntimeState::Parked
        );
    }

    #[test]
    fn a_failed_start_leaves_a_live_sandbox_running_and_parks_a_fresh_session() {
        assert_eq!(
            prior_state(&session(None, "parked")),
            SessionRuntimeState::Parked
        );
        assert_eq!(
            prior_state(&session(Some("fake:x"), "running")),
            SessionRuntimeState::Running
        );
        assert_eq!(
            prior_state(&session(Some("fake:x"), "parked")),
            SessionRuntimeState::Parked
        );
        // A row with no sandbox is never reported running, whatever it claimed.
        assert_eq!(
            prior_state(&session(None, "running")),
            SessionRuntimeState::Parked
        );
    }
}
