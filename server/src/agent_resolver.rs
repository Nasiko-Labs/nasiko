//! One place that turns an agent id into somewhere to send a request, by the agent's kind.
//!
//! Every inbound A2A call used to resolve its own endpoint and each assumed a live container.
//! Now an agent is one of three things, and the answer differs for each:
//!
//! - a **harness** agent has no endpoint: its turns run in a per-session sandbox
//!   (`sessions/turn.rs`), so the caller branches to that path;
//! - a **resident** agent has a container: the live runtime address, else the stored URL;
//! - a **parkable** agent has no workload while idle: the sandbox provider wakes it
//!   (`acquire` is create-or-resume) and its transport URL is the target. One that still has
//!   a live workload (its first deploy was resident) keeps being served by it.
//!
//! Routability is exactly `status IN ('running', 'parked')`.

use axum::http::StatusCode;
use nasiko_flow::FlowEvent;
use nasiko_runtime::{
    ContainerId, ForwardTarget, RuntimeError, RuntimeState, SandboxKey, SandboxProviderKind,
    SandboxTransport,
};
use uuid::Uuid;

use crate::agent_scaling::ParkingEligibility;
use crate::agents::sandbox_spec::prepare_sandbox_spec;
use crate::state::AppState;

/// An agent's address: where it listens, and the path its A2A handler is mounted at. Kept apart
/// because a caller forwarding a sub-path (`/.well-known/agent-card.json`) needs the base alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub base: ForwardTarget,
    /// The card's `transport_path`; empty for a root mount.
    pub mount: String,
}

impl Endpoint {
    /// The URL a message is sent to: the base with the mount path appended.
    pub fn into_forward_target(self) -> ForwardTarget {
        ForwardTarget {
            url: format!("{}{}", self.base.url.trim_end_matches('/'), self.mount),
            headers: self.base.headers,
        }
    }
}

/// Where a request for an agent goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedTarget {
    /// A resident container (or an external agent's stored URL).
    Container(Endpoint),
    /// A parked-capable agent's sandbox. `woke` is true when this request brought it back, and
    /// `wake_ms` is then how long the wake took (the figure `AgentWaking` announces).
    Sandbox {
        target: Endpoint,
        woke: bool,
        wake_ms: Option<u64>,
        /// The backend that owns the sandbox, for telemetry.
        provider: SandboxProviderKind,
    },
    /// A harness agent: the caller drives a session turn instead of forwarding.
    Harness,
}

impl ResolvedTarget {
    /// The message URL, or `None` for a harness agent, which has none.
    pub fn into_forward_target(self) -> Option<ForwardTarget> {
        match self {
            Self::Container(endpoint)
            | Self::Sandbox {
                target: endpoint, ..
            } => Some(endpoint.into_forward_target()),
            Self::Harness => None,
        }
    }
}

/// Who is asking, as far as the resolver needs to know.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The user the request is for. Required to open a per-conversation sandbox.
    pub user_id: Option<Uuid>,
    /// The flow the request belongs to, so a wake can be announced on its event stream.
    pub flow_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("agent not found")]
    NotFound,
    #[error("agent not running (status: {0})")]
    NotRunning(String),
    /// The row is parked but its policy is now resident, so nothing will wake it.
    #[error("agent is parked but its policy is now resident")]
    ParkedButResident,
    /// The row says running; the runtime says otherwise.
    #[error("agent workload is not running (runtime reports: {0})")]
    WorkloadDown(String),
    #[error("no endpoint found for agent '{0}'")]
    NoEndpoint(String),
    #[error("sandbox: {0}")]
    Sandbox(#[from] RuntimeError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

impl ResolveError {
    /// The one mapping to an HTTP status, shared by the proxy and A2A dispatch.
    pub fn http_status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::NotRunning(_) | Self::ParkedButResident | Self::WorkloadDown(_) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::Sandbox(RuntimeError::Timeout(_)) => StatusCode::SERVICE_UNAVAILABLE,
            Self::NoEndpoint(_) | Self::Sandbox(_) => StatusCode::BAD_GATEWAY,
            Self::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// What a client is told. Backend and database text stays in the logs.
    pub fn client_message(&self) -> String {
        match self {
            Self::NotFound => "agent not found".into(),
            Self::NotRunning(status) => format!("agent not running (status: {status})"),
            Self::ParkedButResident => {
                "agent is parked but its policy is now resident; run `nasiko start`".into()
            }
            Self::WorkloadDown(state) => {
                format!("agent workload is not running (runtime reports: {state})")
            }
            Self::NoEndpoint(_) => "agent has no reachable endpoint".into(),
            Self::Sandbox(RuntimeError::Timeout(_)) => "agent is waking".into(),
            Self::Sandbox(_) => "agent could not be started".into(),
            Self::Db(_) => "internal error".into(),
        }
    }
}

/// The `agents` columns resolution reads.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AgentRow {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub name: String,
    pub status: String,
    pub workload_kind: String,
    pub image: Option<String>,
    pub url: Option<String>,
    pub transport_path: Option<String>,
    pub writable: bool,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_internal: bool,
    /// Whether this internal agent keeps its own session memory (so the proxy does not inject
    /// conversation history into its prompt).
    pub keeps_session_memory: bool,
}

/// Loads the agent once; a deleted or unknown agent is `NotFound`. Pass the row to
/// [`resolve_loaded`] to avoid a second read.
pub async fn load_agent(state: &AppState, agent_id: Uuid) -> Result<AgentRow, ResolveError> {
    let row = sqlx::query_as::<_, AgentRow>(
        "SELECT id, owner_id, name, status, workload_kind, image, url, transport_path, writable, \
                deleted_at, is_internal, \
                is_internal AND COALESCE(capabilities -> 'extensions' @> \
                    jsonb_build_array(jsonb_build_object('uri', $2::text)), false) \
                    AS keeps_session_memory \
         FROM agents WHERE id = $1",
    )
    .bind(agent_id)
    .bind(nasiko_agent_proxy::SESSION_MEMORY_EXTENSION)
    .fetch_optional(&state.db)
    .await?;
    row.filter(|row| row.deleted_at.is_none())
        .ok_or(ResolveError::NotFound)
}

/// Errors unless the agent's status makes it routable (`running` or `parked`).
pub fn ensure_routable(row: &AgentRow) -> Result<(), ResolveError> {
    route_without_endpoint(row).map(|_| ())
}

/// Resolves `agent_id` for a request that is about to be served: a parked agent is woken and
/// its last-request time stamped, so the idle reaper measures from this request.
pub async fn resolve_target(
    state: &AppState,
    agent_id: Uuid,
    caller: &Caller,
    context_id: Option<&str>,
) -> Result<ResolvedTarget, ResolveError> {
    let row = load_agent(state, agent_id).await?;
    resolve_loaded(state, &row, caller, context_id).await
}

/// [`resolve_target`] for a caller that already holds the agent's row.
pub async fn resolve_loaded(
    state: &AppState,
    row: &AgentRow,
    caller: &Caller,
    context_id: Option<&str>,
) -> Result<ResolvedTarget, ResolveError> {
    resolve(state, row, caller, context_id).await
}

/// The path appended to an agent's base address: its card's `transport_path`, with a root mount
/// as the empty string (a bare `/` would break the EE injection proxy's routes).
pub fn mount_path(transport_path: Option<&str>) -> &str {
    match transport_path {
        None | Some("/") | Some("") => "",
        Some(path) => path,
    }
}

async fn resolve(
    state: &AppState,
    row: &AgentRow,
    caller: &Caller,
    context_id: Option<&str>,
) -> Result<ResolvedTarget, ResolveError> {
    if let Some(early) = route_without_endpoint(row)? {
        return Ok(early);
    }
    if !state
        .sandbox
        .is_available_for(&SandboxKey::Agent { agent_id: row.id })
    {
        return resident_or_parked(state, row, ParkedWhy::NoSandboxProvider).await;
    }
    match state.agent_scaling.parking_eligibility(row.id).await? {
        ParkingEligibility::Parkable => {
            // A workload that is still running (the agent's first deploy was resident, then it
            // registered durable state) keeps serving; the deploy path parks it at its next
            // deploy or restart. Starting a second copy in a sandbox would split its traffic.
            if workload_is_running(state, row.id).await {
                return resident_target(state, row)
                    .await
                    .map(ResolvedTarget::Container);
            }
            sandbox_target(state, row, caller, context_id).await
        }
        ParkingEligibility::Resident(_) => {
            resident_or_parked(state, row, ParkedWhy::PolicyResident).await
        }
    }
}

enum ParkedWhy {
    NoSandboxProvider,
    PolicyResident,
}

/// The resident path. A row that is `parked` has no workload to reach: with no provider that is
/// a plain refusal; with a policy that is now resident it is a row nothing will ever wake, so it
/// is marked `stopped` (an owner's `nasiko start` brings it back) and the caller is told why.
async fn resident_or_parked(
    state: &AppState,
    row: &AgentRow,
    why: ParkedWhy,
) -> Result<ResolvedTarget, ResolveError> {
    if row.status != "parked" {
        return resident_target(state, row)
            .await
            .map(ResolvedTarget::Container);
    }
    match why {
        ParkedWhy::NoSandboxProvider => Err(ResolveError::NotRunning(row.status.clone())),
        ParkedWhy::PolicyResident => {
            sqlx::query("UPDATE agents SET status = 'stopped' WHERE id = $1 AND status = 'parked'")
                .bind(row.id)
                .execute(&state.db)
                .await?;
            Err(ResolveError::ParkedButResident)
        }
    }
}

async fn workload_is_running(state: &AppState, agent_id: Uuid) -> bool {
    matches!(
        state.runtime.status(&ContainerId::from_uuid(agent_id)).await,
        Ok(status) if status.state == RuntimeState::Running
    )
}

/// The outcomes that need no endpoint: a harness agent, or an agent that is not routable.
/// `Ok(None)` means the agent has somewhere to send the request.
fn route_without_endpoint(row: &AgentRow) -> Result<Option<ResolvedTarget>, ResolveError> {
    if row.workload_kind == "harness" {
        return Ok(Some(ResolvedTarget::Harness));
    }
    if !matches!(row.status.as_str(), "running" | "parked") {
        return Err(ResolveError::NotRunning(row.status.clone()));
    }
    Ok(None)
}

// ─── Resident agents ─────────────────────────────────────────────────────────

async fn resident_target(state: &AppState, row: &AgentRow) -> Result<Endpoint, ResolveError> {
    let mount = mount_path(row.transport_path.as_deref()).to_owned();
    // Prefer the live runtime endpoint (Docker port mapping can change on restart).
    // `forwarding_endpoint()`, not `endpoint()`: this resolves the address an actual outbound
    // A2A call goes to (a no-op alias for `endpoint()` on OSS/no-memory EE; the EE injection
    // proxy's address when memory is enabled) — never the deploy-time persisted snapshot.
    let container_id = ContainerId::from_uuid(row.id);
    match state.runtime.forwarding_endpoint(&container_id).await {
        Ok(base) => return Ok(Endpoint { base, mount }),
        Err(_) => {
            if let Ok(status) = state.runtime.status(&container_id).await
                && status.state != RuntimeState::Running
            {
                // A dead resident workload is the caller's error and a log line; the status
                // column is the reconciler's to change, not a request's.
                tracing::warn!(
                    agent_id = %row.id, name = %row.name, state = ?status.state,
                    "agent resolver: resident workload is not running"
                );
                return Err(ResolveError::WorkloadDown(format!("{:?}", status.state)));
            }
        }
    }

    // Fall back to the stored URL (e.g. external agents, K8s with stable DNS). Bypasses whatever
    // `forwarding_endpoint()` would otherwise route through (the injection proxy, when memory is
    // enabled), so this forward gets no L1A — worth a line in the logs, since a silent skip here
    // looks identical to "L1A had nothing to say."
    match row.url.as_deref().filter(|url| !url.is_empty()) {
        Some(url) => {
            tracing::warn!(
                agent_id = %row.id, name = %row.name,
                "agent resolver: live runtime lookup failed, falling back to stored agents.url \
                 (bypasses the injection proxy — this forward gets no L1A)"
            );
            Ok(Endpoint {
                base: ForwardTarget::from(url.trim_end_matches('/').to_owned()),
                mount,
            })
        }
        None => Err(ResolveError::NoEndpoint(row.name.clone())),
    }
}

// ─── Parkable agents ─────────────────────────────────────────────────────────

async fn sandbox_target(
    state: &AppState,
    row: &AgentRow,
    caller: &Caller,
    context_id: Option<&str>,
) -> Result<ResolvedTarget, ResolveError> {
    let was_parked = row.status == "parked";
    let mount = mount_path(row.transport_path.as_deref()).to_owned();
    let key = sandbox_key(state, row, caller, context_id).await?;
    let prepared = prepare_sandbox_spec(state, row, key).await?;
    // Stamped before the wake, so the idle reaper's re-check after it claims the agent sees a
    // request already under way instead of an agent idle for as long as the wake takes.
    mark_serving(state, row.id).await;
    let started = std::time::Instant::now();
    let handle = acquire_with_one_retry(state, &prepared.spec).await?;
    let url = http_url(state.sandbox.attach(&handle).await?)?;

    let wake_ms = was_parked.then(|| millis_saturating(started.elapsed()));
    if let Some(wake_ms) = wake_ms {
        announce_wake(state, row.id, caller, wake_ms);
    }
    // Fresh credentials mean the agent was deployed or updated since its last wake (whatever its
    // status was), and a parked deploy fetches no card.
    if prepared.credentials_minted {
        refresh_agent_card(state, row.id, &url);
    }
    // TODO NAS-1105: a sandbox target bypasses `forwarding_endpoint()`, so an EE injection proxy
    // (L1A) never sees calls to a woken agent. The `InstrumentedSandbox` decorator is the fix.
    Ok(ResolvedTarget::Sandbox {
        target: Endpoint {
            base: ForwardTarget::from(url.trim_end_matches('/').to_owned()),
            mount,
        },
        woke: was_parked,
        wake_ms,
        provider: handle.provider,
    })
}

/// Pause before the one retry of a wake that raced the reaper.
const ACQUIRE_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);

/// `acquire`, and once more after a short pause when it fails the way a container the reaper was
/// suspending at that moment does (seen exiting mid-stop): the second attempt finds it settled.
async fn acquire_with_one_retry(
    state: &AppState,
    spec: &nasiko_runtime::SandboxSpec,
) -> Result<nasiko_runtime::SandboxHandle, RuntimeError> {
    match state.sandbox.acquire(spec).await {
        Err(RuntimeError::SandboxNotRunning(_) | RuntimeError::Internal(_)) => {
            tokio::time::sleep(ACQUIRE_RETRY_PAUSE).await;
            state.sandbox.acquire(spec).await
        }
        other => other,
    }
}

fn http_url(transport: SandboxTransport) -> Result<String, ResolveError> {
    match transport {
        SandboxTransport::Http { url } => Ok(url),
        SandboxTransport::WebSocket { .. } => {
            Err(RuntimeError::Internal("agent sandbox attached over WebSocket".into()).into())
        }
    }
}

/// A per-conversation sandbox only when the edition asks for one for this agent and the caller
/// named a conversation they own; otherwise the one shared agent sandbox.
async fn sandbox_key(
    state: &AppState,
    row: &AgentRow,
    caller: &Caller,
    context_id: Option<&str>,
) -> Result<SandboxKey, ResolveError> {
    let per_context = state.agent_scaling.per_context_sandbox(row.id).await?;
    let Some(context_id) = context_id.filter(|_| per_context) else {
        return Ok(SandboxKey::Agent { agent_id: row.id });
    };
    let user_id = caller.user_id.ok_or(ResolveError::NotFound)?;
    ensure_context_owner(&state.db, context_id, user_id, row.id).await?;
    Ok(SandboxKey::AgentContext {
        agent_id: row.id,
        context_id: context_id.to_owned(),
    })
}

/// A contextId names a sandbox only for its owner. An unclaimed contextId is claimed for
/// `user_id` here (the same agent), so a second user cannot slip in between this check and the
/// chat-session upsert that would otherwise create the row. Someone else's, or one bound to a
/// different agent, is `NotFound`: indistinguishable from an unknown one, like the agent ACL.
pub async fn ensure_context_owner(
    db: &sqlx::PgPool,
    context_id: &str,
    user_id: Uuid,
    agent_id: Uuid,
) -> Result<(), ResolveError> {
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title) \
         VALUES ($1, $2, $3, $4, 'New chat') ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(context_id)
    .bind(user_id)
    .bind(agent_id)
    .bind(format!("/api/agents/{agent_id}"))
    .execute(db)
    .await?;
    let owner: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT user_id, agent_id FROM chat_sessions WHERE session_id = $1 AND deleted_at IS NULL",
    )
    .bind(context_id)
    .fetch_optional(db)
    .await?;
    match owner {
        Some((owner, Some(bound_agent))) if owner == user_id && bound_agent == agent_id => Ok(()),
        _ => Err(ResolveError::NotFound),
    }
}

/// The agent is serving: running, and its idle clock restarts. Guarded so a status that moved
/// on meanwhile (stopped, failed) is not overwritten, and throttled so a busy agent does not
/// take a write per request. A failure is logged and the request goes on: the stamp is
/// bookkeeping for the idle reaper, not a precondition of serving.
async fn mark_serving(state: &AppState, agent_id: Uuid) {
    let stamped = sqlx::query(
        "UPDATE agents SET status = 'running', last_request_at = now() \
         WHERE id = $1 AND status IN ('running', 'parked') \
           AND (status = 'parked' OR last_request_at IS NULL \
                OR last_request_at < now() - interval '5 seconds')",
    )
    .bind(agent_id)
    .execute(&state.db)
    .await;
    if let Err(e) = stamped {
        tracing::warn!(%agent_id, error = %e, "could not stamp last_request_at; the idle reaper may park this agent early");
    }
}

/// After a parked redeploy no card was fetched, so the stored `transport_path` and skills still
/// describe the old image: read them once from the freshly woken sandbox, off the request path.
fn refresh_agent_card(state: &AppState, agent_id: Uuid, sandbox_url: &str) {
    let (db, http, url) = (
        state.db.clone(),
        state.http_client.clone(),
        sandbox_url.to_owned(),
    );
    tokio::spawn(crate::agents::utils::fetch_agent_card_with_retry(
        db,
        http,
        state.runtime.clone(),
        agent_id,
        url,
    ));
}

fn millis_saturating(took: std::time::Duration) -> u64 {
    u64::try_from(took.as_millis()).unwrap_or(u64::MAX)
}

fn announce_wake(state: &AppState, agent_id: Uuid, caller: &Caller, wake_ms: u64) {
    let Some(flow_id) = caller.flow_id.clone() else {
        return;
    };
    let bus = state.flow_events.clone();
    let event = FlowEvent::AgentWaking {
        agent_id: agent_id.to_string(),
        wake_ms,
    };
    tokio::spawn(async move { bus.publish(&flow_id, event).await });
}

/// The resolver leaf crates (the MAF executor, the HITL resume notifier) call through: the same
/// by-kind resolution the proxy and A2A dispatch use, so a parked agent is woken for them too.
pub struct ServerTargetResolver {
    state: AppState,
}

impl ServerTargetResolver {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[async_trait::async_trait]
impl nasiko_hitl::AgentTargetResolver for ServerTargetResolver {
    async fn resolve(
        &self,
        agent_id: Uuid,
        caller: &nasiko_hitl::ResolveCaller,
        context_id: Option<&str>,
    ) -> Result<ForwardTarget, nasiko_hitl::ResolveFailure> {
        let caller = Caller {
            user_id: caller.user_id,
            flow_id: caller.flow_id.clone(),
        };
        match resolve_target(&self.state, agent_id, &caller, context_id).await {
            Ok(resolved) => resolved.into_forward_target().ok_or_else(|| {
                nasiko_hitl::ResolveFailure::new(
                    nasiko_hitl::ResolveFailure::HARNESS,
                    "MAF, HITL and routed turns cannot drive a harness session yet",
                )
            }),
            // A routable agent with nothing to dial: callers may fall back to an address they hold.
            Err(e @ ResolveError::NoEndpoint(_)) => Err(nasiko_hitl::ResolveFailure::new(
                nasiko_hitl::ResolveFailure::NO_ENDPOINT,
                e.client_message(),
            )),
            Err(e) => {
                tracing::warn!(error = %e, %agent_id, "agent target resolution failed");
                Err(nasiko_hitl::ResolveFailure::new(
                    e.http_status().as_u16(),
                    e.client_message(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, kind: &str) -> AgentRow {
        AgentRow {
            id: Uuid::nil(),
            owner_id: Uuid::nil(),
            name: "echo".into(),
            status: status.into(),
            workload_kind: kind.into(),
            image: Some("x:1".into()),
            url: None,
            transport_path: None,
            writable: false,
            deleted_at: None,
            is_internal: false,
            keeps_session_memory: false,
        }
    }

    #[test]
    fn a_harness_agent_is_a_harness_whatever_its_status() {
        for status in ["running", "parked", "stopped"] {
            assert_eq!(
                route_without_endpoint(&row(status, "harness")).unwrap(),
                Some(ResolvedTarget::Harness)
            );
        }
    }

    #[test]
    fn only_running_and_parked_are_routable() {
        for status in ["running", "parked"] {
            assert!(
                route_without_endpoint(&row(status, "agent"))
                    .unwrap()
                    .is_none()
            );
        }
        for status in ["registered", "deploying", "stopped", "failed", "crashed"] {
            match route_without_endpoint(&row(status, "agent")) {
                Err(ResolveError::NotRunning(got)) => assert_eq!(got, status),
                other => panic!("{status}: {other:?}"),
            }
        }
    }

    #[test]
    fn http_status_mapping() {
        let cases = [
            (ResolveError::NotFound, 404),
            (ResolveError::NotRunning("stopped".into()), 503),
            (ResolveError::ParkedButResident, 503),
            (ResolveError::WorkloadDown("Stopped".into()), 503),
            (ResolveError::NoEndpoint("a".into()), 502),
            (RuntimeError::Timeout("slow".into()).into(), 503),
            (RuntimeError::BackendUnreachable("down".into()).into(), 502),
            (ResolveError::Db(sqlx::Error::PoolClosed), 500),
        ];
        for (error, expected) in cases {
            assert_eq!(error.http_status().as_u16(), expected, "{error}");
        }
        assert_eq!(
            ResolveError::from(RuntimeError::Timeout("t".into())).client_message(),
            "agent is waking"
        );
    }

    #[test]
    fn a_parked_resident_agent_is_told_how_to_restart() {
        assert!(
            ResolveError::ParkedButResident
                .client_message()
                .contains("nasiko start")
        );
        assert!(
            ResolveError::WorkloadDown("Crashed".into())
                .client_message()
                .contains("runtime reports: Crashed")
        );
    }

    #[test]
    fn an_endpoint_appends_its_mount_to_the_base() {
        let endpoint = |mount: &str| Endpoint {
            base: ForwardTarget::from("http://a:1/".to_owned()),
            mount: mount.to_owned(),
        };
        assert_eq!(endpoint("/a2a").into_forward_target().url, "http://a:1/a2a");
        assert_eq!(endpoint("").into_forward_target().url, "http://a:1");
    }

    #[test]
    fn mount_path_treats_root_as_empty() {
        assert_eq!(mount_path(None), "");
        assert_eq!(mount_path(Some("/")), "");
        assert_eq!(mount_path(Some("/a2a")), "/a2a");
    }
}
