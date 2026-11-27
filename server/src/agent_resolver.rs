//! One place that turns an agent id into somewhere to send a request, by the agent's kind.
//!
//! Every inbound A2A call used to resolve its own endpoint and each assumed a live container.
//! Now an agent is one of two things, and the answer differs for each:
//!
//! - a **harness** agent has no endpoint: its turns run in a per-session sandbox
//!   (`sessions/turn.rs`), so the caller branches to that path;
//! - any other agent has a workload of its own: the live runtime address, else the stored URL.
//!
//! Every forward asks the wake seam (`agent_wake`) before resolving, so an agent parked by scaling
//! its workload to zero is running again by the time it is resolved here. Every request that is
//! served stamps `agents.last_request_at`, the idle signal the reaper parks by.
//!
//! Routability is exactly `status IN ('running', 'parked')`.

use axum::http::StatusCode;
use nasiko_runtime::{ContainerId, ForwardTarget, RuntimeState};
use uuid::Uuid;

use crate::agent_wake::AgentWake;
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
    /// The agent's workload (or an external agent's stored URL).
    Container(Endpoint),
    /// A harness agent: the caller drives a session turn instead of forwarding.
    Harness,
}

impl ResolvedTarget {
    /// The message URL, or `None` for a harness agent, which has none.
    pub fn into_forward_target(self) -> Option<ForwardTarget> {
        match self {
            Self::Container(endpoint) => Some(endpoint.into_forward_target()),
            Self::Harness => None,
        }
    }
}

/// Who is asking, as far as the resolver needs to know.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// The flow the request belongs to, so a wake can be announced on its event stream.
    pub flow_id: Option<String>,
}

/// The `Retry-After` a request that met a park mid-way is told to wait.
const PARKED_MID_REQUEST_RETRY_AFTER_SECS: u64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("agent not found")]
    NotFound,
    #[error("agent not running (status: {0})")]
    NotRunning(String),
    /// The row says running; the runtime says otherwise.
    #[error("agent workload is not running (runtime reports: {0})")]
    WorkloadDown(String),
    /// The agent is parked, and waking it in this request did not bring it back: the idle reaper
    /// parked it as the request reached it, or the wake that ran first did not finish.
    #[error("agent was parked as this request reached it")]
    ParkedMidRequest,
    #[error("no endpoint found for agent '{0}'")]
    NoEndpoint(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

impl ResolveError {
    /// The one mapping to an HTTP status, shared by the proxy and A2A dispatch.
    pub fn http_status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::NotRunning(_) | Self::WorkloadDown(_) | Self::ParkedMidRequest => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::NoEndpoint(_) => StatusCode::BAD_GATEWAY,
            Self::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// How many seconds a client should wait before it tries again, for a refusal that a retry
    /// can get past: a park that raced the request is settled, or the agent woken, by then.
    pub fn retry_after_secs(&self) -> Option<u64> {
        matches!(self, Self::ParkedMidRequest).then_some(PARKED_MID_REQUEST_RETRY_AFTER_SECS)
    }

    /// What a client is told. Backend and database text stays in the logs.
    pub fn client_message(&self) -> String {
        match self {
            Self::NotFound => "agent not found".into(),
            Self::NotRunning(status) => format!("agent not running (status: {status})"),
            Self::WorkloadDown(state) => {
                format!("agent workload is not running (runtime reports: {state})")
            }
            Self::ParkedMidRequest => {
                "the agent was parked as this request reached it; try again shortly".into()
            }
            Self::NoEndpoint(_) => "agent has no reachable endpoint".into(),
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

/// Resolves `agent_id` for a request that is about to be served, stamping its last-request time so
/// the idle reaper measures from this request. Call the wake seam first: a parked agent has no
/// workload to resolve to until it is woken.
pub async fn resolve_target(
    state: &AppState,
    agent_id: Uuid,
    caller: &Caller,
) -> Result<ResolvedTarget, ResolveError> {
    let row = load_agent(state, agent_id).await?;
    resolve_loaded(state, &row, caller).await
}

/// [`resolve_target`] for a caller that already holds the agent's row. A row read before the wake
/// seam ran says `parked` for an agent the wake has since started, so a parked row is read again;
/// any other row is resolved as it is, without a second read.
pub async fn resolve_loaded(
    state: &AppState,
    row: &AgentRow,
    caller: &Caller,
) -> Result<ResolvedTarget, ResolveError> {
    if row.status == "parked" {
        let current = load_agent(state, row.id).await?;
        return resolve(state, &current, caller).await;
    }
    resolve(state, row, caller).await
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
) -> Result<ResolvedTarget, ResolveError> {
    if let Some(early) = route_without_endpoint(row)? {
        return Ok(early);
    }
    let endpoint = if row.status == "parked" {
        // The wake this request asked for first did not leave the agent running.
        resolve_after_park(state, row, caller).await?
    } else {
        workload_target(state, row, caller).await?
    };
    Ok(ResolvedTarget::Container(endpoint))
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

// ─── Workloads ───────────────────────────────────────────────────────────────

/// A running workload's address. Serving from it restarts the agent's idle clock, so the reaper
/// measures every agent from its last request. A stamp that finds the row parked means the reaper
/// parked the workload after this request resolved it.
async fn workload_target(
    state: &AppState,
    row: &AgentRow,
    caller: &Caller,
) -> Result<Endpoint, ResolveError> {
    let endpoint = workload_endpoint(state, row).await?;
    match stamp_last_request(state, row.id).await {
        Stamp::FoundParked => resolve_after_park(state, row, caller).await,
        Stamp::Stamped | Stamp::Skipped => Ok(endpoint),
    }
}

/// The agent was parked as this request reached it (or the wake asked for first did not finish).
/// Wake it again, once, and resolve the woken workload; if that does not work out, the caller gets
/// a 503 to retry on, never a forward to a workload going down.
async fn resolve_after_park(
    state: &AppState,
    row: &AgentRow,
    caller: &Caller,
) -> Result<Endpoint, ResolveError> {
    tracing::info!(agent_id = %row.id, "agent resolver: the agent was parked as this request reached it; waking it again");
    if let Err(msg) = state
        .agent_wake
        .before_forward(row.id, caller.flow_id.as_deref())
        .await
    {
        tracing::warn!(agent_id = %row.id, %msg, "agent resolver: waking a just-parked agent failed");
        return Err(ResolveError::ParkedMidRequest);
    }
    let current = load_agent(state, row.id).await?;
    if current.status != "running" {
        return Err(ResolveError::ParkedMidRequest);
    }
    workload_endpoint(state, &current).await
}

async fn workload_endpoint(state: &AppState, row: &AgentRow) -> Result<Endpoint, ResolveError> {
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
                // A dead workload is the caller's error and a log line; the status column is
                // the reconciler's to change, not a request's.
                tracing::warn!(
                    agent_id = %row.id, name = %row.name, state = ?status.state,
                    "agent resolver: the agent's workload is not running"
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

/// How often at most a served request writes `last_request_at`; the reaper's park window allows
/// for it (`nasiko_config::LAST_REQUEST_STAMP_THROTTLE_SECS`).
const STAMP_THROTTLE: std::time::Duration =
    std::time::Duration::from_secs(nasiko_config::LAST_REQUEST_STAMP_THROTTLE_SECS);

/// A `running` row's idle clock restarts. A `parked` row is matched but keeps its clock and status,
/// and the status is returned: the reaper parked the workload after this request resolved it (the
/// park holds the row while it scales, so this write waits for its commit), and reviving the row
/// would leave it `running` with no workload behind it. So the row is written only when it is due a
/// stamp, or in that race.
const STAMP_LAST_REQUEST: &str = "UPDATE agents \
     SET last_request_at = CASE WHEN status = 'running' THEN now() ELSE last_request_at END \
     WHERE id = $1 AND status IN ('running', 'parked') \
       AND (status = 'parked' OR last_request_at IS NULL \
            OR last_request_at < now() - make_interval(secs => $2)) \
     RETURNING status";

/// What the last-request stamp found.
#[derive(Debug, PartialEq, Eq)]
enum Stamp {
    /// A running row, its idle clock restarted.
    Stamped,
    /// Nothing written: stamped within the throttle, the status moved on, or the write failed.
    Skipped,
    /// The row is parked: the reaper parked the workload this request resolved.
    FoundParked,
}

/// Guarded so a status that moved on meanwhile (stopped, failed) is not overwritten, and throttled
/// to one write per [`STAMP_THROTTLE`] per agent: the idle window is an hour by default, so a finer
/// stamp buys the reaper nothing and costs a write per request. The reaper's park window allows
/// for the throttle, and a park is fenced on this stamp, so a request inside the throttle window
/// leaves it unchanged. A failure is logged and the request goes on: the stamp is bookkeeping for
/// the idle reaper, not a precondition of serving.
async fn stamp_last_request(state: &AppState, agent_id: Uuid) -> Stamp {
    let found: sqlx::Result<Option<String>> = sqlx::query_scalar(STAMP_LAST_REQUEST)
        .bind(agent_id)
        .bind(STAMP_THROTTLE.as_secs_f64())
        .fetch_optional(&state.db)
        .await;
    match found {
        Ok(Some(status)) if status == "parked" => Stamp::FoundParked,
        Ok(Some(_)) => Stamp::Stamped,
        Ok(None) => Stamp::Skipped,
        Err(e) => {
            warn_unstamped(agent_id, &e);
            Stamp::Skipped
        }
    }
}

fn warn_unstamped(agent_id: Uuid, e: &sqlx::Error) {
    tracing::warn!(%agent_id, error = %e, "could not stamp last_request_at; the idle reaper may park this agent early");
}

/// The resolver leaf crates (the MAF executor, the HITL resume notifier) call through: the same
/// wake and by-kind resolution the proxy and A2A dispatch use, so a parked agent is woken for
/// them too.
pub struct ServerTargetResolver {
    state: AppState,
    /// Whether `resolve` asks the wake seam first. Off only for a caller that asks it itself.
    wakes_first: bool,
}

impl ServerTargetResolver {
    /// Asks the wake seam before resolving, as every forward must: MAF steps and HITL nudges
    /// reach agents only through here.
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            wakes_first: true,
        }
    }

    /// For a caller that asks the wake seam itself before it resolves (`CpCallGuard::before_call`
    /// does, ahead of spending flow budget), so the seam is asked once per call.
    pub fn after_caller_wake(state: AppState) -> Self {
        Self {
            state,
            wakes_first: false,
        }
    }
}

#[async_trait::async_trait]
impl nasiko_hitl::AgentTargetResolver for ServerTargetResolver {
    async fn resolve(
        &self,
        agent_id: Uuid,
        caller: &nasiko_hitl::ResolveCaller,
        _context_id: Option<&str>,
    ) -> Result<ForwardTarget, nasiko_hitl::ResolveFailure> {
        if self.wakes_first
            && let Err(msg) = self
                .state
                .agent_wake
                .before_forward(agent_id, caller.flow_id.as_deref())
                .await
        {
            tracing::warn!(%agent_id, %msg, "agent target resolution: before_forward refused");
            return Err(nasiko_hitl::ResolveFailure::new(
                StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                msg,
            ));
        }
        let caller = Caller {
            flow_id: caller.flow_id.clone(),
        };
        match resolve_target(&self.state, agent_id, &caller).await {
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
            (ResolveError::WorkloadDown("Stopped".into()), 503),
            (ResolveError::ParkedMidRequest, 503),
            (ResolveError::NoEndpoint("a".into()), 502),
            (ResolveError::Db(sqlx::Error::PoolClosed), 500),
        ];
        for (error, expected) in cases {
            assert_eq!(error.http_status().as_u16(), expected, "{error}");
        }
    }

    #[test]
    fn a_request_that_lost_the_park_race_is_told_to_retry_and_when() {
        let refused = ResolveError::ParkedMidRequest;
        assert!(refused.client_message().contains("try again"));
        assert_eq!(
            refused.retry_after_secs(),
            Some(PARKED_MID_REQUEST_RETRY_AFTER_SECS)
        );
        for other in [
            ResolveError::NotFound,
            ResolveError::NotRunning("stopped".into()),
            ResolveError::WorkloadDown("Crashed".into()),
        ] {
            assert_eq!(other.retry_after_secs(), None, "{other}");
        }
    }

    #[test]
    fn a_dead_workload_names_what_the_runtime_reports() {
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
