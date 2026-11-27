use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::agent_scaling::{AgentDeploy, DeployPlan, ReplicaBounds};
use crate::agent_wake::AgentWake;
use crate::auth::Claims;
use crate::catalog::agent_secrets;
use crate::state::AppState;
use nasiko_runtime::{ContainerId, DeploymentSpec, RuntimeState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(deploy))
        .route("/{name}", delete(destroy))
        .route("/{name}/stop", post(stop))
        .route("/{name}/start", post(start))
        .route("/{name}/restart", post(restart))
        .route("/{name}/scale", post(scale))
}

/// Mounted separately from `router()`, under `require_auth` only — each
/// handler checks `can_deploy` (and, for single-container lookups, agent
/// ownership via `resolve_authorized_container`) itself and returns
/// `crate::unavailable()` (200) instead of a blanket 403. NOT_FOUND
/// (agent doesn't exist by that name) stays as-is either way.
pub fn degradable_router() -> Router<AppState> {
    Router::new()
        .route("/", get(list))
        .route("/{name}", get(status))
        .route("/{name}/logs", get(logs))
}

#[derive(Deserialize)]
struct DeployRequest {
    image: String,
    name: String,
    #[serde(default)]
    ports: Vec<u16>,
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
    #[serde(default)]
    replicas: Option<u32>,
    #[serde(default)]
    writable: bool,
    /// Container-side mount target for the writable volume (`--writable-path`).
    /// `None` = `/workspace`. Implies `writable` when set.
    #[serde(default)]
    writable_path: Option<String>,
    /// Workload kind for this deploy (`agent`, `mcp_connector`,
    /// `acp_coding_agent`). Self-describing so the deploy is correct regardless
    /// of whether the catalog row was written before this call; `None` falls
    /// back to the catalog's stored kind (then `agent`).
    #[serde(default)]
    workload_kind: Option<String>,
}

async fn deploy(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<DeployRequest>,
) -> impl IntoResponse {
    // Start with env from request (inline -e flags)
    let mut env = req.env;

    // Used both for secret resolution below (when this name maps to an existing
    // catalog agent) and to namespace the `--writable` memory subpath (see
    // DeploymentSpec::owner_id) — the caller is the closest thing to an "owner"
    // an ad-hoc, possibly-unclaimed image deploy has.
    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Resolve the catalog agent (if any) once — used for both secret resolution and
    // UUID-keying so this ad-hoc deploy converges with the upload/update/import paths.
    let resolved_agent_id = resolve_agent_id_by_name(&state, &req.name).await;

    // If this name maps to an existing catalog agent, the caller must own it (or
    // be superuser) before we resolve and inject ITS secrets (`agent_secrets`,
    // resolved by the real agent_id regardless of caller identity below) into a
    // container running an arbitrary caller-supplied image — otherwise any
    // deployer could exfiltrate another agent's secrets by deploying their own
    // image under that agent's name and reading them back out. A name with no
    // catalog entry has no owner to check (first-deploy-wins, same reasoning as
    // the ad-hoc `restart` fallback below).
    if let Some(agent_id) = resolved_agent_id
        && !crate::acl::can_manage_agent(&state, &claims, agent_id).await
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(agent_id) = resolved_agent_id
        && let Some(refusal) = refuse_harness_agent(&state, agent_id).await
    {
        return refusal;
    }

    // Read before the deploy env is built, which may reset `state_mode` (see `AgentScaling::begin_deploy`).
    let scaling = match resolved_agent_id {
        Some(agent_id) => Some(state.agent_scaling.begin_deploy(agent_id).await),
        None => None,
    };

    // Resolve vault + agent secrets (vault = base, agent = override, request = highest)
    if let Some(agent_id) = resolved_agent_id {
        let resolved = resolve_full_env(&state, owner_id, agent_id).await;
        // resolved secrets are base; request env overrides
        for (k, v) in resolved {
            env.entry(k).or_insert(v);
        }
        crate::agents::deploy_env::apply_platform_env(
            &state,
            &mut env,
            agent_id,
            owner_id,
            crate::agents::deploy_env::StateModeOnDeploy::Reset,
        )
        .await;
    }

    // UUID-key when the name maps to a catalog agent; fall back to name-keying only
    // for ad-hoc images that have no catalog identity.
    let container_id = match resolved_agent_id {
        Some(agent_id) => ContainerId::from_uuid(agent_id),
        None => ContainerId::new(&req.name),
    };

    // `--writable` is a durable property of a registered agent (persisted in the
    // `agents` row by whichever on-ramp first set it), not a per-deploy flag.
    // Source it from the catalog so an ad-hoc redeploy through this path — e.g. a
    // UI "redeploy" that doesn't re-send the flag — can never silently detach a
    // live volume and drop the agent's files. An explicit request flag still wins
    // (so `nasiko deploy --writable` of an as-yet-unregistered image works too).
    //
    // The same catalog read also carries `workload_kind`: an ACP coding agent is
    // driven over a stdio turn, not reverse-proxied, so it must deploy *port-less*
    // and always writable (each session needs its workspace), exactly like the
    // spec the coding-turn path builds (`coding::proxy::build_spec`). Resolving it
    // here means `nasiko deploy` of a registered coding agent brings its container
    // up correctly without the ad-hoc deploy request having to know the kind.
    let (db_writable, db_writable_path, db_workload_kind) = match resolved_agent_id {
        Some(agent_id) => match sqlx::query_as::<_, (bool, Option<String>, String)>(
            "SELECT writable, writable_path, workload_kind FROM agents WHERE id = $1",
        )
        .bind(agent_id)
        .fetch_optional(&state.db)
        .await
        {
            // A missing row is a genuinely ad-hoc image with no catalog record —
            // `false`/`agent` is correct there. A DB *error*, though, must NOT
            // collapse to `false`: that would deploy a writable agent with no
            // volume and then persist `writable=false`, the exact silent detach
            // this block exists to prevent. Fail the deploy instead of guessing.
            Ok(row) => row.unwrap_or((false, None, "agent".to_owned())),
            Err(e) => {
                tracing::error!(%e, %agent_id, "deploy: could not read writable from catalog");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
                    .into_response();
            }
        },
        None => (false, None, "agent".to_owned()),
    };
    // Prefer the self-describing request field (the CLI always sends it for a
    // coding agent, so a promote-then-deploy in one call is correct even before
    // the catalog row is updated); fall back to the stored kind, then `agent`. An
    // explicit but invalid request value is a clean 400.
    let workload_kind = match req.workload_kind.as_deref() {
        Some(raw) => match nasiko_runtime::WorkloadKind::from_db_str(raw) {
            Some(kind) => kind,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    "workload_kind must be one of: agent, mcp_connector, acp_coding_agent",
                )
                    .into_response();
            }
        },
        // An unrecognized stored value defaults to `Agent` (the
        // backward-compatible behavior) rather than failing the deploy.
        None => nasiko_runtime::WorkloadKind::from_db_str(&db_workload_kind).unwrap_or_default(),
    };
    let is_coding = workload_kind == nasiko_runtime::WorkloadKind::AcpCodingAgent;

    let writable_path = req.writable_path.clone().or(db_writable_path);
    // A coding agent is always writable — every session runs in its own workspace
    // subdirectory on the mount.
    let writable = req.writable || db_writable || writable_path.is_some() || is_coding;

    let ports = if is_coding {
        // Driven over stdio; a coding-agent container exposes no inbound port.
        Vec::new()
    } else if req.ports.is_empty() {
        vec![crate::agents::DEFAULT_AGENT_PORT]
    } else {
        req.ports
    };
    // The Service/port-mapping targets ports[0], so an HTTP agent must listen
    // there — every in-tree agent honors PORT. Explicit env still wins. A coding
    // agent has no port (empty `ports`), so it gets no PORT injection.
    if !is_coding {
        crate::agents::set_port_env(&mut env, ports[0]);
    }

    // The plan reads the stored kind; a request naming another one decides this deploy's.
    let plan = match deploy_plan(scaling.as_deref(), req.replicas, writable).await {
        Ok(plan) => plan.for_kind(workload_kind),
        Err(e) => {
            tracing::error!(%e, name = %req.name, "deploy: could not read the deploy plan");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response();
        }
    };
    let bounds = plan.bounds;

    let mut spec = DeploymentSpec {
        container_id,
        name: req.name.clone(),
        image: {
            // Canonicalize (qualify) then validate - fixes R-123 unqualified-reference bypass
            match crate::agents::canonicalize_and_validate_image_ref(
                &state.db,
                &state.config.agent_image_registry,
                &state.config.agent_image_registry_aliases,
                &req.image,
                owner_id,
                claims.is_superuser,
            )
            .await
            {
                Ok(qualified) => qualified,
                Err((code, msg)) => return (code, msg).into_response(),
            }
        },
        ports,
        env_vars: env,
        min_replicas: bounds.min,
        max_replicas: bounds.max,
        replica_target: bounds.target,
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind,
        // Sourced from the catalog (see above) so redeploys keep the mount.
        writable,
        writable_path,
        owner_id,
        force_pull: false,
        parkable: plan.parkable,
        overrides: Default::default(),
    };
    // Only a name that already maps to a registered catalog agent has an
    // `agents` row to scope a pull credential to (see pull_credentials'
    // agent_id FK) — an ad-hoc, never-registered image deploy has nothing to
    // bind one to.
    if let Some(agent_id) = resolved_agent_id {
        crate::agents::attach_pull_credential(
            &state.db,
            &state.config.agent_runtime,
            &state.config.agent_image_registry,
            &mut spec,
            agent_id,
        )
        .await;
    }

    let outcome = match &scaling {
        Some(scaling) => scaling.deploy(&spec).await,
        None => state.runtime.deploy(&spec).await,
    };
    match outcome {
        Ok(status) => {
            // Update the catalog URL + record deployment (fire-and-forget).
            if let Some(agent_id) = resolve_agent_id_by_name(&state, &req.name).await {
                let db = state.db.clone();
                // Readiness-independent (a fresh k8s Deployment is never
                // Ready by the time deploy() returns) — same fix as the
                // upload/update/restart paths.
                let endpoint =
                    crate::agents::resolve_agent_url(&state.runtime, &status, &spec.container_id)
                        .await;
                let image = spec.image.clone();
                let owner_id = claims.user_uuid().ok();
                // Persist the effective writable config so it survives on the
                // agents row. Without this, a deploy that turned an agent
                // writable here would leave `writable=false` in the catalog, and
                // the next restart/update/rollback (which read the flag from the
                // row, not the request) would silently redeploy with no volume.
                let spec_writable = spec.writable;
                let spec_writable_path = spec.writable_path.clone();

                // Probe the agent's card and persist `transport_path` (plus
                // description/skills/tags/capabilities) — the same probe the
                // seed / upload / update / restart deploy paths already run.
                // Without it, an agent that mounts A2A at a non-root path (Go
                // `a2a-go` agents serve `/a2a`) keeps an empty `transport_path`,
                // so the orchestrator/proxy POSTs to `/` and every routed call
                // 404s; and its description/skills stay empty, starving the
                // routing engine of any signal but the bare name. This ad-hoc
                // `nasiko deploy` (`POST /containers`) path was the only deploy
                // path that skipped the probe.
                tokio::spawn(crate::agents::utils::fetch_agent_card_with_retry(
                    state.db.clone(),
                    state.http_client.clone(),
                    state.runtime.clone(),
                    state.llm_backend.clone(),
                    agent_id,
                    endpoint.clone(),
                ));

                tokio::spawn(async move {
                    // Write the live endpoint URL + running status + image back to
                    // the catalog, so restart (which needs `image` to redeploy) works
                    // for agents deployed through this ad-hoc path too.
                    let _ = sqlx::query(
                        "UPDATE agents SET url = COALESCE(NULLIF($1, ''), url), image = $2, status = 'running', writable = $4, writable_path = $5, updated_at = now() WHERE id = $3",
                    )
                    .bind(&endpoint)
                    .bind(&image)
                    .bind(agent_id)
                    .bind(spec_writable)
                    .bind(&spec_writable_path)
                    .execute(&db)
                    .await;

                    // A first-time deploy-by-image has no agent_builds row (no
                    // server-side build job ran) — ensure_deployment_tracked
                    // synthesizes one so the crash-loop guardian (EE) still sees
                    // this deployment (docs/CRASH_GUARDIAN_REPORT.md §5.1/§5.3).
                    crate::agents::utils::ensure_deployment_tracked(
                        &db, agent_id, owner_id, &image,
                    )
                    .await;
                });
            }
            (StatusCode::CREATED, Json(status)).into_response()
        }
        Err(e) => {
            tracing::error!(%e, name = %spec.name, "deploy: runtime error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// The plan of an ad-hoc deploy. A registered agent gets what its scaling policy gives it,
/// as on every other deploy path, and a writable one stays on its single volume; the
/// caller's `replicas` applies only to an image with no catalog entry, which never parks.
async fn deploy_plan(
    scaling: Option<&dyn AgentDeploy>,
    replicas: Option<u32>,
    writable: bool,
) -> sqlx::Result<DeployPlan> {
    let Some(scaling) = scaling else {
        let fixed = replicas.unwrap_or(1);
        return Ok(DeployPlan {
            bounds: ReplicaBounds {
                min: fixed,
                max: fixed,
                target: None,
            },
            parkable: false,
            resources: Default::default(),
        });
    };
    let plan = scaling.plan().await?;
    if writable {
        return Ok(DeployPlan {
            bounds: ReplicaBounds::SINGLE,
            ..plan
        });
    }
    Ok(plan)
}

async fn list(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    let containers = match state.runtime.list().await {
        Ok(containers) => containers,
        Err(e) => {
            tracing::error!(%e, "list: runtime error");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };

    if claims.is_superuser {
        return Json(containers).into_response();
    }

    let owner_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    let owned_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM agents WHERE owner_id = $1 AND deleted_at IS NULL")
            .bind(owner_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    // Containers are UUID-keyed post-RUN-2b (see `agents::build_agent_spec`);
    // scope the list to the caller's own agents, matching the single-resource
    // ownership check `resolve_authorized_container` already enforces.
    // Previously this returned every container in the runtime regardless of
    // caller — a read-side reconnaissance leak across teams.
    let filtered: Vec<_> = containers
        .into_iter()
        .filter(|c| {
            c.container_id
                .as_str()
                .parse::<Uuid>()
                .is_ok_and(|id| owned_ids.contains(&id))
        })
        .collect();

    Json(filtered).into_response()
}

async fn status(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    let id = match resolve_authorized_container(&state, &claims, &name).await {
        Ok(id) => id,
        // NOT_FOUND stays as-is (don't confirm/deny existence either way);
        // FORBIDDEN (not the owner) degrades like every other converted
        // read — this is a GET, not a mutation.
        Err(resp) if resp.status() == StatusCode::FORBIDDEN => return crate::unavailable(),
        Err(resp) => return resp,
    };
    match state.runtime.status(&id).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => {
            // A parked agent may have no workload to report: one parked before parking scaled
            // workloads to zero gets one back when it is next called.
            if let Ok(agent_id) = Uuid::parse_str(id.as_str())
                && is_parked(&state, agent_id).await
            {
                return Json(parked_status(
                    agent_id,
                    "agent is parked; the first request starts it",
                ))
                .into_response();
            }
            tracing::error!(%e, %name, "status: runtime error");
            (StatusCode::NOT_FOUND, "container not found").into_response()
        }
    }
}

async fn destroy(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let id = match resolve_authorized_container(&state, &claims, &name).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.runtime.destroy(&id).await {
        Ok(()) => {
            if let Some(agent_id) = resolve_agent_id_by_name(&state, &name).await {
                let db = state.db.clone();
                tokio::spawn(async move {
                    let _ = sqlx::query(
                        "UPDATE agent_deployments SET status = 'stopped', updated_at = now()
                         WHERE agent_id = $1 AND status != 'stopped'",
                    )
                    .bind(agent_id)
                    .execute(&db)
                    .await;
                });
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            tracing::error!(%e, %name, "destroy: runtime error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// The `ContainerRuntime::deploy`-shaped body a parked agent answers with, so a client that
/// reads `state` sees `parked` where a running agent shows `running`.
fn parked_status(agent_id: Uuid, message: &str) -> serde_json::Value {
    serde_json::json!({
        "container_id": agent_id,
        "state": "parked",
        "replicas_live": 0,
        "endpoint": null,
        "message": message,
        "restart_count": 0,
    })
}

/// Whether the agent's row says `parked`. A row that cannot be read is not.
async fn is_parked(state: &AppState, agent_id: Uuid) -> bool {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    status.as_deref() == Some("parked")
}

/// Holds the agent's lock ([`crate::agent_wake::lock_agent`]) for one lifecycle change until the
/// returned transaction is dropped, so the change cannot interleave with a wake or a park: a wake
/// that scaled the workload up after a `stop` would leave it running under a `stopped` row that
/// nothing serves or parks. Waits at most one wake budget, which bounds how long a wake holds it.
async fn lock_for_lifecycle(
    state: &AppState,
    agent_id: Uuid,
    name: &str,
) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, axum::response::Response> {
    let budget = std::time::Duration::from_secs(state.config.agent_wake_budget_secs());
    let locked = async {
        let mut tx = state.db.begin().await?;
        crate::agent_wake::lock_agent(&mut tx, agent_id, std::time::Instant::now() + budget)
            .await?;
        Ok::<_, sqlx::Error>(tx)
    };
    locked.await.map_err(|e| {
        tracing::warn!(%e, %name, "lifecycle: could not take the agent's lock");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "the agent is being started or parked, try again shortly",
        )
            .into_response()
    })
}

async fn stop(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let agent_id = match resolve_authorized_agent(&state, &claims, &name).await {
        Ok(agent_id) => agent_id,
        Err(resp) => return resp,
    };
    let _lock = match lock_for_lifecycle(&state, agent_id, &name).await {
        Ok(lock) => lock,
        Err(resp) => return resp,
    };
    let id = ContainerId::from_uuid(agent_id);
    let stopped = match state.runtime.scale(&id, 0).await {
        // An agent parked before parking scaled workloads to zero has none to stop.
        Err(nasiko_runtime::RuntimeError::ContainerNotFound(_))
            if is_parked(&state, agent_id).await =>
        {
            Ok(())
        }
        outcome => outcome,
    };
    match stopped {
        Ok(()) => {
            record_lifecycle_status(&state, &name, "stopped").await;
            StatusCode::OK.into_response()
        }
        Err(e) => {
            tracing::error!(%e, %name, "stop: runtime error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn start(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let agent_id = match resolve_authorized_agent(&state, &claims, &name).await {
        Ok(agent_id) => agent_id,
        Err(resp) => return resp,
    };
    // A parked agent is woken as its next call would wake it: its address and idle clock come back
    // and the answer waits until it serves, or it stays parked. Before the lifecycle lock, which
    // the wake takes itself; inline, so a wake seam that refuses the wake can still answer.
    if is_parked(&state, agent_id).await {
        if let Err(message) = state.agent_wake.before_forward(agent_id, None).await {
            return (StatusCode::SERVICE_UNAVAILABLE, message).into_response();
        }
        // Still parked: no wake applies to it (a harness agent's); it is started below as before.
        if !is_parked(&state, agent_id).await {
            return StatusCode::OK.into_response();
        }
    }
    let _lock = match lock_for_lifecycle(&state, agent_id, &name).await {
        Ok(lock) => lock,
        Err(resp) => return resp,
    };
    let status: Option<String> = match sqlx::query_scalar("SELECT status FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_optional(&state.db)
        .await
    {
        Ok(status) => status,
        Err(e) => {
            tracing::error!(%e, %name, "start: could not read the agent's status");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    // An agent with no workload to scale up gets one, from the stored image and env, as a restart
    // does: one stopped or parked before parking scaled workloads to zero, and any agent whose row
    // was moved to this server without its workload — stopped, parked or failed where it came from.
    if status.as_deref() != Some("running")
        && state
            .runtime
            .status(&ContainerId::from_uuid(agent_id))
            .await
            .is_err()
    {
        return restart(
            State(state),
            claims,
            Path(name),
            Query(RestartQuery::default()),
        )
        .await
        .into_response();
    }
    let id = ContainerId::from_uuid(agent_id);
    match state.runtime.scale(&id, 1).await {
        Ok(()) => {
            record_lifecycle_status(&state, &name, "running").await;
            StatusCode::OK.into_response()
        }
        Err(e) => {
            tracing::error!(%e, %name, "start: runtime error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[derive(Deserialize, Default)]
struct RestartQuery {
    /// `?refresh=true` — force a fresh registry pull of this agent's image
    /// before recreating the container, bypassing Docker's local cache. For
    /// a mutable tag (e.g. `:latest`), this is what actually picks up a new
    /// push instead of silently reusing whatever was pulled last time.
    /// Defaults to `false` (existing behavior: reuse the cached image).
    #[serde(default)]
    refresh: bool,
}

async fn restart(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
    Query(query): Query<RestartQuery>,
) -> impl IntoResponse {
    // Look up agent record to get image and owner. `agents` has no `port` column
    // (that lives on `agent_deployments.spec_ports`, used by the catalog-aware
    // `deployments::restart_deployment`) — this ad-hoc router has no deployment
    // row to read from, so it falls back to the canonical default port, same as
    // `build_agent_spec` does for any other caller that omits ports.
    //
    // Unlike `deploy`'s ad-hoc-image branch, there is no "unclaimed, first-owner-
    // wins" case here — restart only ever acts on an *existing* container, and
    // every sibling op (stop/start/scale/logs, via `resolve_authorized_container`)
    // already 404s when `name` has no catalog row instead of falling back to a
    // raw, unchecked container ID. This used to fall back too, letting any
    // deployer restart any container on the host that merely wasn't tracked in
    // `agents` — with no owner to check an ACL against. Match the siblings: no
    // catalog row, no restart.
    // `image` must decode as Option: registry-pushed agents (`nasiko deploy`)
    // record their image on the deployment row (`spec_image`), leaving the
    // catalog column NULL — decoding it as String made this query error out
    // and masked every such agent as a 404 "agent not found".
    //
    // Accepts either a UUID or a display name, same as `resolve_agent_id_by_name`
    // (which the sibling stop/start/scale/logs ops use) — without this, copying
    // the UUID `nasiko ps` prints into `nasiko restart <uuid>` 404'd even though
    // the identical UUID worked for `nasiko rm`.
    #[derive(sqlx::FromRow)]
    struct RestartAgentRow {
        id: Uuid,
        owner_id: Uuid,
        image: Option<String>,
        writable: bool,
        writable_path: Option<String>,
    }

    //
    // Only a live agent: a superuser passes `can_manage_agent` for a deleted one too, and a
    // restart would bring its container back. A deleted agent's name may also belong to a new
    // agent by now, which is the one `nasiko restart <name>` means.
    let agent: Option<RestartAgentRow> = if let Ok(id) = name.parse::<Uuid>() {
        sqlx::query_as(
            "SELECT id, owner_id, image, writable, writable_path FROM agents \
             WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
    } else {
        sqlx::query_as(
            "SELECT id, owner_id, image, writable, writable_path FROM agents \
             WHERE name = $1 AND deleted_at IS NULL",
        )
        .bind(&name)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
    };

    let Some(RestartAgentRow {
        id: agent_id,
        owner_id,
        image,
        writable,
        writable_path,
    }) = agent
    else {
        return (StatusCode::NOT_FOUND, "agent not found").into_response();
    };

    if !crate::acl::can_manage_agent(&state, &claims, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
    }
    if let Some(refusal) = refuse_harness_agent(&state, agent_id).await {
        return refusal;
    }

    let image = match image {
        Some(image) => image,
        None => {
            let fallback: Option<String> = sqlx::query_scalar(
                "SELECT spec_image FROM agent_deployments \
                 WHERE agent_id = $1 AND spec_image IS NOT NULL \
                 ORDER BY created_at DESC LIMIT 1",
            )
            .bind(agent_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
            match fallback {
                Some(image) => image,
                None => {
                    return (
                        StatusCode::CONFLICT,
                        "agent has no recorded image to redeploy",
                    )
                        .into_response();
                }
            }
        }
    };
    // Both sources store what the deploy request carried — for OCI-push
    // deploys that's the registry-relative `nasiko/{name}:{tag}`, which pulls
    // from docker.io if applied as-is. Qualify exactly as the ad-hoc deploy
    // path does (no-op for refs outside the `nasiko/` convention).
    let image = crate::agents::qualify_deploy_image(&state.config.agent_image_registry, &image);

    let uuid_id = ContainerId::from_uuid(agent_id);
    // A running agent is restarted where it is, so it stays available and keeps its
    // autoscaling. Destroy and recreate stays for what it alone can do: starting an agent that
    // is not running (including a pre-UUID name-keyed one), pulling the image fresh
    // (`refresh`), and any runtime whose restart does not apply a changed env.
    if !query.refresh && can_restart_in_place(&state, &uuid_id).await {
        return restart_in_place(&state, &uuid_id, agent_id, owner_id, &name).await;
    }

    let scaling = state.agent_scaling.begin_deploy(agent_id).await;
    let plan = match scaling.plan().await {
        Ok(plan) => plan,
        Err(e) => {
            tracing::error!(%e, %name, "restart: could not read the deploy plan");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    // Resolve env: vault (base) + agent secrets (override)
    let mut env = resolve_full_env(&state, owner_id, agent_id).await;
    crate::agents::deploy_env::apply_platform_env(
        &state,
        &mut env,
        agent_id,
        owner_id,
        crate::agents::deploy_env::StateModeOnDeploy::Reset,
    )
    .await;

    // Destroy the UUID-keyed workload (post-fix); fall back to the name-keyed one
    // for pre-fix containers so we don't leave a stale duplicate running.
    if state.runtime.destroy(&uuid_id).await.is_err() {
        let _ = state.runtime.destroy(&ContainerId::new(&name)).await;
    }

    // Redeploy with fresh env, UUID-keyed (see agents::build_agent_spec), on the port its last
    // deployment recorded (the default when none did).
    let ports = crate::agents::recorded_ports(&state.db, agent_id, &mut env).await;
    let mut spec = crate::agents::build_agent_spec(
        agent_id,
        &name,
        image,
        ports,
        env,
        &state.config.agent_default_memory,
        plan,
        writable,
        writable_path,
        owner_id,
    );
    spec.force_pull = query.refresh;
    crate::agents::attach_pull_credential(
        &state.db,
        &state.config.agent_runtime,
        &state.config.agent_image_registry,
        &mut spec,
        agent_id,
    )
    .await;

    match scaling.deploy(&spec).await {
        Ok(status) => {
            // The container may come back on a different host port, and a
            // rebuilt image may advertise a different card (transport_path,
            // skills) — refresh both, same as the update/upload deploy paths.
            let agent_url =
                crate::agents::resolve_agent_url(&state.runtime, &status, &uuid_id).await;
            let _ = sqlx::query(
                "UPDATE agents SET status = 'running', url = $2, updated_at = now() WHERE id = $1",
            )
            .bind(agent_id)
            .bind(&agent_url)
            .execute(&state.db)
            .await;
            tokio::spawn(crate::agents::utils::fetch_agent_card_with_retry(
                state.db.clone(),
                state.http_client.clone(),
                state.runtime.clone(),
                state.llm_backend.clone(),
                agent_id,
                agent_url,
            ));
            Json(status).into_response()
        }
        Err(e) => {
            tracing::error!(%e, %name, "restart: redeploy failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Whether `restart` can replace the agent where it runs: the runtime rolls restarts in place
/// and the agent is serving. A status that cannot be read counts as not running, so the
/// restart falls back to a redeploy, which does not need it.
/// The answer to a redeploy or restart of a harness agent, `None` for any other agent: the
/// workload is built by the sandbox provider, from settings the agent row does not hold.
async fn refuse_harness_agent(
    state: &AppState,
    agent_id: Uuid,
) -> Option<axum::response::Response> {
    match crate::agents::is_harness_agent(&state.db, agent_id).await {
        Ok(false) => None,
        Ok(true) => {
            Some((StatusCode::CONFLICT, crate::agents::HARNESS_AGENT_MANAGED).into_response())
        }
        Err(e) => {
            tracing::error!(%e, %agent_id, "could not read the agent's kind");
            Some((StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response())
        }
    }
}

async fn can_restart_in_place(state: &AppState, id: &ContainerId) -> bool {
    if !state.runtime.restarts_in_place().await {
        return false;
    }
    match state.runtime.status(id).await {
        Ok(status) => status.state == RuntimeState::Running,
        Err(e) => {
            tracing::warn!(%e, container = %id, "restart: could not read the agent's status - redeploying it");
            false
        }
    }
}

/// Rolls a running agent's instances one at a time. The workload and its autoscaling are left
/// as they are, and so is `state_mode`: the same image comes back and registers its task
/// store as before. The env is rebuilt first, with the platform credentials rotated, and
/// applied to the runtime so the replacements start with it; if that fails nothing is rolled.
async fn restart_in_place(
    state: &AppState,
    id: &ContainerId,
    agent_id: Uuid,
    owner_id: Uuid,
    name: &str,
) -> axum::response::Response {
    if let Err(e) = refresh_runtime_env(state, id, agent_id, owner_id, name).await {
        tracing::error!(%e, %name, "restart: could not apply the agent's env");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
    }
    if let Err(e) = state.runtime.restart(id).await {
        tracing::error!(%e, %name, "restart: rolling restart failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
    }
    match state.runtime.status(id).await {
        Ok(status) => Json(status).into_response(),
        Err(e) => {
            tracing::error!(%e, %name, "restart: could not read the status after the restart");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Rebuilds the agent's env, with the platform credentials rotated, and hands it to the runtime,
/// so every instance it starts from now on runs with the agent's current secrets. Instances
/// already running keep the env they started with until they are rolled.
async fn refresh_runtime_env(
    state: &AppState,
    id: &ContainerId,
    agent_id: Uuid,
    owner_id: Uuid,
    name: &str,
) -> Result<(), nasiko_runtime::RuntimeError> {
    let mut env = resolve_full_env(state, owner_id, agent_id).await;
    crate::agents::deploy_env::apply_platform_env(
        state,
        &mut env,
        agent_id,
        owner_id,
        crate::agents::deploy_env::StateModeOnDeploy::Keep,
    )
    .await;
    // The runtime applies this as the agent's whole env, so it carries the port its last
    // deployment recorded, as a redeploy does.
    crate::agents::recorded_ports(&state.db, agent_id, &mut env).await;
    state.runtime.refresh_secrets(id, name, env).await
}

/// Scales an agent up on a runtime that restarts in place (Kubernetes), where new instances
/// read the env the runtime holds: that env is rebuilt first, so a secret changed since the last
/// deploy reaches the new instances. Rebuilding it rotates the platform credentials, which the
/// instances already running hold only until `ROTATION_GRACE_SECS` passes, so a running agent is
/// rolled afterwards and every instance ends up on the same, current env.
async fn scale_up_with_fresh_env(
    state: &AppState,
    id: &ContainerId,
    agent_id: Uuid,
    name: &str,
    replicas: u32,
) -> Result<(), nasiko_runtime::RuntimeError> {
    let owner_id: Uuid = sqlx::query_scalar("SELECT owner_id FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| nasiko_runtime::RuntimeError::Internal(format!("read owner: {e}")))?;
    let was_running = can_restart_in_place(state, id).await;
    refresh_runtime_env(state, id, agent_id, owner_id, name).await?;
    state.runtime.scale(id, replicas).await?;
    if was_running {
        state.runtime.restart(id).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct ScaleRequest {
    replicas: u32,
}

async fn scale(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
    Json(req): Json<ScaleRequest>,
) -> impl IntoResponse {
    let agent_id = match resolve_authorized_agent(&state, &claims, &name).await {
        Ok(agent_id) => agent_id,
        Err(resp) => return resp,
    };
    // A parkable agent's workload runs one replica (a Sandbox cannot run more): a larger count
    // would silently run one.
    if req.replicas > 1 {
        match crate::agent_scaling::runs_parkable(state.agent_scaling.as_ref(), &state.db, agent_id)
            .await
        {
            Ok(true) => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "a parkable agent runs one replica; set its parking_policy to resident to run more",
                        "code": "parkable",
                    })),
                )
                    .into_response();
            }
            Ok(false) => {}
            Err(e) => {
                tracing::error!(%e, %name, "scale: could not read whether the agent is parkable");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
            }
        }
    }
    // An autoscaled agent's count belongs to the autoscaler, which would undo a manual
    // one. Scaling to 0 still stops it: the runtime pauses the autoscaler first.
    if req.replicas > 0 {
        match state.agent_scaling.is_autoscaled(agent_id).await {
            Ok(true) => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "the replica count is managed by its autoscaler",
                        "code": "autoscaled",
                    })),
                )
                    .into_response();
            }
            Ok(false) => {}
            Err(e) => {
                tracing::error!(%e, %name, "scale: could not read the agent's scaling state");
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
            }
        }
    }
    let _lock = match lock_for_lifecycle(&state, agent_id, &name).await {
        Ok(lock) => lock,
        Err(resp) => return resp,
    };
    let id = ContainerId::from_uuid(agent_id);
    // Docker keeps the env a container was created with and runs one replica, so a scale there
    // starts no instance a fresh env could reach; its `restart` recreates the container instead.
    let scaled = if req.replicas > 0 && state.runtime.restarts_in_place().await {
        scale_up_with_fresh_env(&state, &id, agent_id, &name, req.replicas).await
    } else {
        state.runtime.scale(&id, req.replicas).await
    };
    match scaled {
        Ok(()) => {
            // Scaling to zero is what `stop` does, so it must leave the same
            // status behind — otherwise `scale 0` parks a container that the
            // catalog still advertises as running.
            let status = if req.replicas == 0 {
                "stopped"
            } else {
                "running"
            };
            record_lifecycle_status(&state, &name, status).await;
            StatusCode::OK.into_response()
        }
        Err(e) => {
            tracing::error!(%e, %name, "scale: runtime error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

#[derive(Deserialize)]
struct LogsQuery {
    #[serde(default = "default_tail")]
    tail: u32,
}
fn default_tail() -> u32 {
    100
}

async fn logs(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
    axum::extract::Query(q): axum::extract::Query<LogsQuery>,
) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    let id = match resolve_authorized_container(&state, &claims, &name).await {
        Ok(id) => id,
        Err(resp) if resp.status() == StatusCode::FORBIDDEN => return crate::unavailable(),
        Err(resp) => return resp,
    };
    match state.runtime.logs(&id, q.tail).await {
        Ok(lines) => Json(lines).into_response(),
        Err(e) => {
            tracing::error!(%e, %name, "logs: runtime error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

/// Resolve `name_or_id` to its catalog agent UUID — accepts either a UUID or a
/// display name, same as `catalog::routes::get_one`. Without the UUID branch,
/// copying the UUID `nasiko ps` itself prints into `stop`/`start`/`restart`/
/// `scale` (all of which route through this) 404'd, even though the identical
/// UUID works for `nasiko rm`.
///
/// Excludes soft-deleted agents in both branches — the `(owner_id, name)`
/// uniqueness constraint (`agents_owner_name_active_uniq`,
/// oss/migrations/0001_schema.sql) is scoped to `deleted_at IS NULL`, so a
/// deleted agent's name is meant to be free for a fresh row. Without this
/// filter, `deploy()`'s ad-hoc image path found the old deleted row, updated
/// it in place instead of treating the name as unclaimed, and left the
/// resulting running container permanently invisible to `nasiko ps`/`rm`.
/// Persist a lifecycle status change to the catalog row and its deployment row.
///
/// `GET /api/agents` reports `agents.status` straight from the column
/// (`catalog::routes`) with no runtime reconciliation, so a lifecycle op that
/// only talks to the runtime leaves the catalog lying: stopping a container left
/// it listed as running forever, and the UI's "Stopped" filter never matched it.
///
/// Awaited, not spawned like `destroy`'s deployment write: the UI refetches the
/// agent list as soon as the request returns, and a detached write would race
/// that refetch and hand back the pre-stop status.
async fn record_lifecycle_status(state: &AppState, name: &str, status: &str) {
    let Some(agent_id) = resolve_agent_id_by_name(state, name).await else {
        return;
    };
    if let Err(e) = sqlx::query("UPDATE agents SET status = $2, updated_at = now() WHERE id = $1")
        .bind(agent_id)
        .bind(status)
        .execute(&state.db)
        .await
    {
        tracing::error!(%e, %name, %status, "failed to record agent status");
    }
    // `agent_deployments` is append-only history: `restart_deployment` and every
    // update/upload path mark the current row `stopped` (by id) and INSERT a new
    // one, so an agent owns one live row and N historical ones.
    //
    // That makes the two directions asymmetric, and they must not share a query:
    //
    // - Bringing an agent UP may only touch the newest row. An agent-wide sweep
    //   would resurrect every historical row as `running`, which is not just a
    //   smudged history — EE's crash guardian polls *every* row in
    //   ('starting','running') (the EE crash guardian), so each stale
    //   row becomes a phantom deployment it probes and can mark crashed.
    // - Taking one DOWN sweeps the agent, matching `destroy` above. Nothing of
    //   this agent's is running afterwards, so any row still claiming otherwise
    //   is stale by definition and this converges it on reality — including rows
    //   orphaned `running` by an earlier crash.
    let recorded = if status == "stopped" {
        sqlx::query(
            "UPDATE agent_deployments SET status = $2::deployment_status, updated_at = now()
             WHERE agent_id = $1 AND status != $2::deployment_status",
        )
        .bind(agent_id)
        .bind(status)
        .execute(&state.db)
        .await
        .map(|_| ())
    } else {
        crate::agents::deployments::record_newest_deployment_status(&state.db, agent_id, status)
            .await
    };
    if let Err(e) = recorded {
        tracing::error!(%e, %name, %status, "failed to record deployment status");
    }
}

async fn resolve_agent_id_by_name(state: &AppState, name_or_id: &str) -> Option<Uuid> {
    if let Ok(id) = name_or_id.parse::<Uuid>() {
        return sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM agents WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    }
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM agents WHERE name = $1 AND deleted_at IS NULL")
        .bind(name_or_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
}

/// Resolve `name` to its catalog agent UUID, verify the caller may manage it
/// (owner ∪ superuser — the same predicate RUN-9 uses for catalog delete), and
/// return the UUID-keyed `ContainerId` that `build_agent_spec`/`deploy` used at
/// deploy time (RUN-2b).
///
/// Every admin lifecycle op (status/destroy/stop/start/restart/scale/logs) must
/// go through this, not just `resolve_agent_id_by_name` — this whole router is
/// gated only by `require_deployer` (a ROLE check), which is not scoped to the
/// caller's own agents. Without the ownership check here, any deployer-role
/// user could destroy, stop, or read the logs (which can contain prompts/
/// secrets) of any OTHER team's agent just by knowing its name. The RUN-2b
/// keying fix made this more directly reachable — these ops now resolve to the
/// *correct* container instead of a name-keyed one that likely didn't exist.
#[allow(clippy::result_large_err)]
async fn resolve_authorized_container(
    state: &AppState,
    claims: &Claims,
    name: &str,
) -> Result<ContainerId, axum::response::Response> {
    resolve_authorized_agent(state, claims, name)
        .await
        .map(ContainerId::from_uuid)
}

/// The agent `name` resolves to (see [`resolve_authorized_container`]), when the caller
/// may manage it.
async fn resolve_authorized_agent(
    state: &AppState,
    claims: &Claims,
    name: &str,
) -> Result<Uuid, axum::response::Response> {
    let Some(agent_id) = resolve_agent_id_by_name(state, name).await else {
        return Err((StatusCode::NOT_FOUND, "agent not found").into_response());
    };
    if !crate::acl::can_manage_agent(state, claims, agent_id).await {
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    Ok(agent_id)
}

/// Resolve the full env for an agent: platform defaults (base) + vault secrets
/// (override) + agent secrets (highest precedence).
async fn resolve_full_env(
    state: &AppState,
    owner_id: Uuid,
    agent_id: Uuid,
) -> std::collections::HashMap<String, String> {
    // 1-2. Vault secrets, then agent secrets on top.
    let mut env = agent_secrets::resolve_secret_env(&state.db, owner_id, agent_id).await;

    // 3. Platform-level LLM config — only fills gaps left by vault/agent
    // secrets. `AppState::agent_env` (used by the deployment-scoped restart
    // path) already does this; this ad-hoc container path had silently
    // dropped it, so `nasiko restart` never picked up OPENAI_API_KEY /
    // OPENAI_BASE_URL changes made to the platform's own .env.
    for (key, value) in state.platform_fallback_env() {
        env.entry(key).or_insert(value);
    }

    // 4. Same reasoning as the platform-LLM-config gap above, same fix shape —
    // this is a plain `agents` column (migration 0032), not a secret, so it's
    // not in `agent_secrets` at all. Unconditional insert, not `.or_insert`:
    // guards against a stale CODING_AGENT_MINIMAL_CODE secret a pre-migration
    // agent might still carry in `secrets_env` (see the matching comment in
    // `AppState::agent_env`, state.rs). The outer `deploy()` caller's own
    // `entry().or_insert()` merge still lets an explicit `-e
    // CODING_AGENT_MINIMAL_CODE=...` on this specific request win over it.
    let minimal_code_enabled: Option<bool> =
        sqlx::query_scalar("SELECT minimal_code_enabled FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let minimal_code_enabled = minimal_code_enabled.unwrap_or(false);
    tracing::info!(
        %agent_id,
        minimal_code_enabled,
        "resolve_full_env: injecting CODING_AGENT_MINIMAL_CODE"
    );
    env.insert(
        "CODING_AGENT_MINIMAL_CODE".into(),
        minimal_code_enabled.to_string(),
    );

    env
}
