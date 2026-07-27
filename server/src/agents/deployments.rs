use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::Serialize;
use uuid::Uuid;

use nasiko_runtime::{ContainerId, DeploymentSpec};

use crate::auth::Claims;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/deployment/{deployment_id}/restart",
        post(restart_deployment),
    )
}

/// Mounted separately from `router()`, under `require_auth` only — each
/// handler checks `can_deploy` (and, for the single-agent lookup, agent
/// access) itself and returns `crate::unavailable()` (200) instead
/// of a blanket 403.
pub fn degradable_router() -> Router<AppState> {
    Router::new()
        .route("/deployments", get(list_deployments))
        .route("/{id}/deployment", get(get_agent_deployment))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct DeploymentRow {
    id: Uuid,
    agent_id: Uuid,
    build_id: Uuid,
    namespace: String,
    replicas: i16,
    status: String,
    service_url: Option<String>,
    owner_id: Option<Uuid>,
    agent_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    crash_reason: Option<String>,
    crashed_at: Option<chrono::DateTime<chrono::Utc>>,
    last_logs: Option<String>,
    restart_count: i32,
    k8s_deployment_name: Option<String>,
}

#[derive(sqlx::FromRow)]
struct AgentDeployInfo {
    name: String,
    image: String,
    agent_id: Uuid,
    build_id: Option<Uuid>,
    owner_id: Option<Uuid>,
    /// DB-recorded deployment status — used to guard restart against already-live agents.
    status: String,
    /// Stored ports from migration 013 — None for agents deployed before the migration.
    spec_ports: Option<Vec<i32>>,
    /// Stored image from migration 013 — None falls back to agents.image.
    spec_image: Option<String>,
    /// K8s deployment name when running on Kubernetes. None for Docker agents.
    k8s_deployment_name: Option<String>,
}

// ─── GET /deployments ────────────────────────────────────────────────────────

async fn list_deployments(State(state): State<AppState>, claims: Claims) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let rows = if claims.is_superuser {
        sqlx::query_as::<_, DeploymentRow>(
            "SELECT d.id, d.agent_id, d.build_id, d.namespace, d.replicas,
                    d.status::text as status, d.service_url, d.owner_id,
                    a.name as agent_name, d.created_at,
                    d.crash_reason, d.crashed_at, d.last_logs, d.restart_count,
                    d.k8s_deployment_name
             FROM agent_deployments d
             LEFT JOIN agents a ON a.id = d.agent_id
             ORDER BY d.created_at DESC LIMIT 50",
        )
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as::<_, DeploymentRow>(
            "SELECT d.id, d.agent_id, d.build_id, d.namespace, d.replicas,
                    d.status::text as status, d.service_url, d.owner_id,
                    a.name as agent_name, d.created_at,
                    d.crash_reason, d.crashed_at, d.last_logs, d.restart_count,
                    d.k8s_deployment_name
             FROM agent_deployments d
             LEFT JOIN agents a ON a.id = d.agent_id
             WHERE d.owner_id = $1
             ORDER BY d.created_at DESC LIMIT 50",
        )
        .bind(user_id)
        .fetch_all(&state.db)
        .await
    };

    match rows {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => {
            tracing::error!(%e, "list_deployments db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ─── GET /{id}/deployment ────────────────────────────────────────────────────

async fn get_agent_deployment(
    State(state): State<AppState>,
    claims: Claims,
    Path(agent_id): Path<Uuid>,
) -> impl IntoResponse {
    let identity: nasiko_auth::Identity = claims.clone().into();
    if !state.auth.can_deploy(&identity).await {
        return crate::unavailable();
    }

    if !crate::acl::can_access_agent(&state, &claims, agent_id).await {
        return crate::unavailable();
    }

    match sqlx::query_as::<_, DeploymentRow>(
        "SELECT d.id, d.agent_id, d.build_id, d.namespace, d.replicas,
                d.status::text as status, d.service_url, d.owner_id,
                a.name as agent_name, d.created_at,
                d.crash_reason, d.crashed_at, d.last_logs, d.restart_count,
                d.k8s_deployment_name
         FROM agent_deployments d
         LEFT JOIN agents a ON a.id = d.agent_id
         WHERE d.agent_id = $1 AND d.status != 'stopped'
         ORDER BY d.created_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %agent_id, "get_agent_deployment db error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Best-effort revert of the `starting` mark applied before a runtime restart
/// attempt, used only when the runtime call itself fails (so the deployment
/// truly is back at its pre-attempt state, not just claimed to be) — never
/// call this once a runtime deploy/restart has actually succeeded.
async fn revert_starting_status(state: &AppState, deployment_id: Uuid, original_status: &str) {
    if let Err(e) =
        sqlx::query("UPDATE agent_deployments SET status = $2, updated_at = now() WHERE id = $1")
            .bind(deployment_id)
            .bind(original_status)
            .execute(&state.db)
            .await
    {
        tracing::error!(%e, %deployment_id, original_status, "restart_deployment: failed to revert starting status after runtime failure");
    }
}

// ─── POST /deployment/{id}/restart ──────────────────────────────────────────

async fn restart_deployment(
    State(state): State<AppState>,
    claims: Claims,
    Path(deployment_id): Path<Uuid>,
) -> impl IntoResponse {
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    // Fetch deployment and agent info together, including stored spec columns.
    let info = match sqlx::query_as::<_, AgentDeployInfo>(
        "SELECT a.name, a.image, a.id as agent_id,
                d.build_id, d.owner_id, d.status::text as status,
                d.spec_ports, d.spec_image, d.k8s_deployment_name
         FROM agent_deployments d
         JOIN agents a ON a.id = d.agent_id
         WHERE d.id = $1",
    )
    .bind(deployment_id)
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(%e, %deployment_id, "restart_deployment: fetch error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Owner or superuser only. Deliberately stricter than read access:
    // any deployer-role user can READ a public agent, but only the owner (or a
    // superuser/admin) may restart it — restarting causes destroy + recreate which
    // is a denial-of-service if granted too broadly.
    // Orphaned agents (owner_id = NULL, set by ON DELETE SET NULL) must be handled
    // by a superuser; non-superusers always receive 403 for them.
    if !claims.is_superuser {
        let is_owner = info.owner_id.map(|o| o == user_id).unwrap_or(false);
        if !is_owner {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    // Atomic mark-starting BEFORE touching the runtime: two concurrent restart
    // calls for the same deployment could both pass a plain read-then-check
    // (both see status='stopped' at fetch time) and then both race to
    // destroy/recreate the same container. This conditional UPDATE is the real
    // guard — the WHERE clause re-checks live DB state at write time, not the
    // possibly-stale value from the SELECT above, so only one caller can win
    // the transition; the other gets 409 immediately, before any runtime call.
    let original_status = info.status.clone();
    let transitioned = match sqlx::query(
        "UPDATE agent_deployments SET status = 'starting', updated_at = now()
         WHERE id = $1 AND status NOT IN ('running', 'starting')",
    )
    .bind(deployment_id)
    .execute(&state.db)
    .await
    {
        Ok(result) => result.rows_affected() > 0,
        Err(e) => {
            tracing::error!(%e, %deployment_id, "restart_deployment: failed to mark deployment starting");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    };
    if !transitioned {
        return (StatusCode::CONFLICT, "agent is already running or starting").into_response();
    }

    // Resolve stored ports; fall back to port 8000 for pre-migration agents.
    let ports: Vec<u16> = if let Some(stored) = info.spec_ports.filter(|v| !v.is_empty()) {
        tracing::info!(agent_id = %info.agent_id, ports = ?stored, "restart: using stored spec ports");
        stored.into_iter().map(|p| p as u16).collect()
    } else {
        tracing::warn!(agent_id = %info.agent_id, "restart: spec_ports not stored, falling back to port 8000");
        vec![8000]
    };

    // Resolve stored image; fall back to agents.image for pre-migration agents.
    let image = info.spec_image.unwrap_or_else(|| info.image.clone());

    // Resolve agent environment (platform vars + agent-specific secrets).
    let secrets = state.agent_env(info.agent_id).await;

    if let Some(k8s_name) = &info.k8s_deployment_name {
        // ── K8s path: scale-to-1 (avoids tearing down and recreating the Deployment) ──
        // k8s_deployment_name stores the ContainerId value persisted at deploy time.
        // Pre-fix agents have agent_name here; post-fix agents have the UUID.
        // Either way, the stored value is what K8s knows about — use it as-is.
        tracing::info!(agent_id = %info.agent_id, k8s_name, "restart: using K8s scale-to-1 path");
        let k8s_id = ContainerId::new(k8s_name);

        // Refresh the K8s Secret before scaling up so that secrets rotated while
        // the agent was stopped are picked up without requiring a full redeploy.
        // Non-fatal: proceed with scale-to-1 even if the Secret update fails —
        // the pod will start with the previously applied values.
        if let Err(e) = state.runtime.refresh_secrets(&k8s_id, secrets).await {
            tracing::warn!(%e, %deployment_id, k8s_name, "restart: failed to refresh K8s secret (using existing values)");
        }

        // Use restart() (not scale(1)) so the KEDA ScaledObject the crash-loop
        // guardian may have deleted before scaling to 0 gets re-applied on the way
        // back up (RUN-8) — calling scale() directly here bypassed that recovery.
        if let Err(e) = state.runtime.restart(&k8s_id).await {
            tracing::error!(%e, %deployment_id, k8s_name, "restart_deployment: restart failed");
            revert_starting_status(&state, deployment_id, &original_status).await;
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
        }
    } else {
        // ── Docker path: destroy + recreate (UUID-keyed) ─────────────────────────────
        // Post-fix containers use the agent UUID as ContainerId; pre-fix containers
        // used the agent name. Try UUID first; fall back to name to avoid leaving a
        // stale name-based container running alongside the new UUID-based one.
        let uuid_id = ContainerId::from_uuid(info.agent_id);
        let name_id = ContainerId::new(&info.name);
        if state.runtime.destroy(&uuid_id).await.is_err() {
            let _ = state.runtime.destroy(&name_id).await;
        }

        let spec = DeploymentSpec {
            container_id: uuid_id,
            name: info.name.clone(),
            image: image.clone(),
            ports: ports.clone(),
            env_vars: secrets,
            min_replicas: 1,
            max_replicas: 1,
            // TODO: persist spec_resources in agent_deployments and restore here.
            // Currently the upload path also uses None, so both deploy and restart
            // apply the runtime default (0.5 CPU / 512 MiB). No behavioral regression
            // until the API supports caller-specified resource limits.
            resources: None,
            // Docker-only path (see the `if`/`else` above) — these fields are
            // meaningless to DockerRuntime.
            image_pull_secret_name: None,
            image_pull_credential_seed: None,
        };

        match state.runtime.deploy(&spec).await {
            Ok(status) => {
                let agent_url = status.endpoint.unwrap_or_default();
                if let Err(e) = sqlx::query(
                    "UPDATE agents SET status = 'running', url = $2, updated_at = now() WHERE id = $1",
                )
                .bind(info.agent_id)
                .bind(&agent_url)
                .execute(&state.db)
                .await
                {
                    // Runtime redeploy already succeeded — this only means the catalog
                    // row's status/url is stale until the next reconcile. Surface it in
                    // logs (SRV-5) rather than silently swallowing it.
                    tracing::error!(%e, %deployment_id, agent_id = %info.agent_id, "restart_deployment: failed to update agent status/url after successful redeploy — catalog row stale until next reconcile");
                }
            }
            Err(e) => {
                tracing::error!(%e, %deployment_id, "restart_deployment: deploy failed");
                revert_starting_status(&state, deployment_id, &original_status).await;
                return (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response();
            }
        }
    }

    // Mark old deployment stopped and insert the new deployment row atomically.
    // The runtime-level restart/redeploy above already succeeded at this point,
    // so a failure here must not silently leave the agent running with a stale
    // 'starting'/old row and no live 'running' row for the guardian to find —
    // previously these were two independent writes, so the old row could end
    // up 'stopped' while the new-row insert failed, orphaning the agent
    // (untracked, but actually running). One transaction: either both land or
    // neither does, leaving the row at 'starting' (not silently 'stopped')
    // for the next reconcile to pick up. The new row's crash fields (crash_
    // reason/crashed_at/last_logs/pod_name = NULL, restart_count = 0) come from
    // the table's own column defaults — no separate clearing UPDATE needed for
    // a freshly inserted row.
    let mut warnings: Vec<&'static str> = Vec::new();
    let new_deploy_id: Option<Uuid> = 'bookkeeping: {
        let mut tx = match state.db.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                tracing::error!(%e, %deployment_id, "restart_deployment: failed to begin bookkeeping tx");
                warnings.push("failed to record deployment bookkeeping; agent is running but untracked — needs reconciliation");
                break 'bookkeeping None;
            }
        };

        if let Err(e) = sqlx::query(
            "UPDATE agent_deployments SET status = 'stopped', updated_at = now() WHERE id = $1",
        )
        .bind(deployment_id)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(%e, %deployment_id, "restart_deployment: failed to mark previous deployment stopped");
            warnings.push("failed to record deployment bookkeeping; agent is running but untracked — needs reconciliation");
            break 'bookkeeping None;
        }

        // Carry identity + spec forward so the guardian and a subsequent restart
        // keep working after this restart (RUN-3): k8s_deployment_name = agent
        // UUID string, plus the ports/image this deployment actually used.
        let inserted: Option<Uuid> = match sqlx::query_scalar(
            "INSERT INTO agent_deployments (agent_id, build_id, status, owner_id, k8s_deployment_name, spec_ports, spec_image)
             VALUES ($1, $2, 'running', $3, $4, $5, $6)
             RETURNING id",
        )
        .bind(info.agent_id)
        .bind(info.build_id)
        .bind(info.owner_id)
        .bind(info.agent_id.to_string())
        .bind(ports.iter().map(|&p| p as i32).collect::<Vec<i32>>())
        .bind(&image)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(%e, %deployment_id, agent_id = %info.agent_id, "restart_deployment: failed to record new deployment row — agent is running but untracked, needs reconciliation");
                warnings.push("failed to record deployment bookkeeping; agent is running but untracked — needs reconciliation");
                break 'bookkeeping None;
            }
        };

        if let Err(e) = tx.commit().await {
            tracing::error!(%e, %deployment_id, "restart_deployment: failed to commit bookkeeping tx");
            warnings.push("failed to record deployment bookkeeping; agent is running but untracked — needs reconciliation");
            break 'bookkeeping None;
        }

        inserted
    };

    let mut body = serde_json::json!({});
    if let Some(id) = new_deploy_id {
        body["deployment_id"] = serde_json::json!(id);
    }
    if !warnings.is_empty() {
        body["warnings"] = serde_json::json!(warnings);
    }
    (StatusCode::OK, Json(body)).into_response()
}
