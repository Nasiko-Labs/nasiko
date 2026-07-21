use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::Claims;
use crate::catalog::agent_secrets;
use crate::state::AppState;
use nasiko_runtime::{ContainerId, DeploymentSpec};

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
}

async fn deploy(
    State(state): State<AppState>,
    claims: Claims,
    Json(req): Json<DeployRequest>,
) -> impl IntoResponse {
    // Start with env from request (inline -e flags)
    let mut env = req.env;

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

    // Resolve vault + agent secrets (vault = base, agent = override, request = highest)
    if let Some(agent_id) = resolved_agent_id {
        let owner_id = match claims.user_uuid() {
            Ok(id) => id,
            Err(e) => return e.into_response(),
        };
        let resolved = resolve_full_env(&state, owner_id, agent_id).await;
        // resolved secrets are base; request env overrides
        for (k, v) in resolved {
            env.entry(k).or_insert(v);
        }
    }

    // UUID-key when the name maps to a catalog agent; fall back to name-keying only
    // for ad-hoc images that have no catalog identity.
    let container_id = match resolved_agent_id {
        Some(agent_id) => ContainerId::from_uuid(agent_id),
        None => ContainerId::new(&req.name),
    };

    let ports = if req.ports.is_empty() {
        vec![crate::agents::DEFAULT_AGENT_PORT]
    } else {
        req.ports
    };
    // The Service/port-mapping targets ports[0], so the agent must listen
    // there — every in-tree agent honors PORT. Explicit env still wins.
    env.entry("PORT".into())
        .or_insert_with(|| ports[0].to_string());

    let mut spec = DeploymentSpec {
        container_id,
        name: req.name.clone(),
        image: crate::agents::qualify_deploy_image(&state.config.agent_image_registry, &req.image),
        ports,
        env_vars: env,
        min_replicas: req.replicas.unwrap_or(1),
        max_replicas: req.replicas.unwrap_or(1),
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
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

    match state.runtime.deploy(&spec).await {
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
                tokio::spawn(async move {
                    // Write the live endpoint URL + running status back to the catalog.
                    if !endpoint.is_empty() {
                        let url = endpoint;
                        let _ = sqlx::query(
                            "UPDATE agents SET url = $1, status = 'running', updated_at = now() WHERE id = $2",
                        )
                        .bind(&url)
                        .bind(agent_id)
                        .execute(&db)
                        .await;
                    }

                    let build_id: Option<Uuid> = sqlx::query_scalar(
                        "SELECT id FROM agent_builds WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
                    )
                    .bind(agent_id)
                    .fetch_optional(&db)
                    .await
                    .ok()
                    .flatten();

                    if let Some(build_id) = build_id {
                        let _ = sqlx::query(
                            "INSERT INTO agent_deployments (agent_id, build_id, status, k8s_deployment_name)
                             VALUES ($1, $2, 'running', $3)",
                        )
                        .bind(agent_id)
                        .bind(build_id)
                        .bind(agent_id.to_string())
                        .execute(&db)
                        .await;
                    }
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

async fn stop(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let id = match resolve_authorized_container(&state, &claims, &name).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.runtime.scale(&id, 0).await {
        Ok(()) => StatusCode::OK.into_response(),
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
    let id = match resolve_authorized_container(&state, &claims, &name).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.runtime.scale(&id, 1).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => {
            tracing::error!(%e, %name, "start: runtime error");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}

async fn restart(
    State(state): State<AppState>,
    claims: Claims,
    Path(name): Path<String>,
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
    let agent: Option<(Uuid, Uuid, Option<String>)> =
        sqlx::query_as("SELECT id, owner_id, image FROM agents WHERE name = $1")
            .bind(&name)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    let Some((agent_id, owner_id, image)) = agent else {
        return (StatusCode::NOT_FOUND, "agent not found").into_response();
    };

    if !crate::acl::can_manage_agent(&state, &claims, agent_id).await {
        return StatusCode::FORBIDDEN.into_response();
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

    // Resolve env: vault (base) + agent secrets (override)
    let env = resolve_full_env(&state, owner_id, agent_id).await;

    // Destroy the UUID-keyed workload (post-fix); fall back to the name-keyed one
    // for pre-fix containers so we don't leave a stale duplicate running.
    let uuid_id = ContainerId::from_uuid(agent_id);
    if state.runtime.destroy(&uuid_id).await.is_err() {
        let _ = state.runtime.destroy(&ContainerId::new(&name)).await;
    }

    // Redeploy with fresh env, UUID-keyed (see agents::build_agent_spec). Empty
    // ports → build_agent_spec defaults to DEFAULT_AGENT_PORT.
    let mut spec = crate::agents::build_agent_spec(agent_id, &name, image, vec![], env, None);
    crate::agents::attach_pull_credential(
        &state.db,
        &state.config.agent_runtime,
        &state.config.agent_image_registry,
        &mut spec,
        agent_id,
    )
    .await;

    match state.runtime.deploy(&spec).await {
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
    let id = match resolve_authorized_container(&state, &claims, &name).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };
    match state.runtime.scale(&id, req.replicas).await {
        Ok(()) => StatusCode::OK.into_response(),
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

async fn resolve_agent_id_by_name(state: &AppState, name: &str) -> Option<Uuid> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM agents WHERE name = $1")
        .bind(name)
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
async fn resolve_authorized_container(
    state: &AppState,
    claims: &Claims,
    name: &str,
) -> Result<ContainerId, axum::response::Response> {
    let Some(agent_id) = resolve_agent_id_by_name(state, name).await else {
        return Err((StatusCode::NOT_FOUND, "agent not found").into_response());
    };
    if !crate::acl::can_manage_agent(state, claims, agent_id).await {
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    Ok(ContainerId::from_uuid(agent_id))
}

/// Resolve the full env for an agent: platform defaults (base) + vault secrets
/// (override) + agent secrets (highest precedence).
async fn resolve_full_env(
    state: &AppState,
    owner_id: Uuid,
    agent_id: Uuid,
) -> std::collections::HashMap<String, String> {
    use crate::secrets::crypto::SecretsCrypto;

    let crypto = SecretsCrypto::for_user(owner_id);
    let mut env = std::collections::HashMap::new();

    // 1. Vault secrets (user-level, lower precedence)
    let vault_rows: Vec<(String, String)> =
        sqlx::query_as("SELECT name, encrypted_value FROM user_secrets WHERE user_id = $1")
            .bind(owner_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    for (name, encrypted) in vault_rows {
        if let Ok(value) = crypto.decrypt(&encrypted) {
            env.insert(name, value);
        }
    }

    // 2. Agent secrets (higher precedence — overrides vault)
    let agent_secrets = agent_secrets::resolve_agent_env(&state.db, agent_id).await;
    for (k, v) in agent_secrets {
        env.insert(k, v);
    }

    // 3. Platform-level LLM config — only fills gaps left by vault/agent
    // secrets. `AppState::agent_env` (used by the deployment-scoped restart
    // path) already does this; this ad-hoc container path had silently
    // dropped it, so `nasiko restart` never picked up OPENAI_API_KEY /
    // OPENAI_BASE_URL changes made to the platform's own .env.
    if let Some(ref key) = state.config.openai_api_key {
        env.entry("OPENAI_API_KEY".into())
            .or_insert_with(|| key.clone());
    }
    if let Some(ref url) = state.config.openai_base_url {
        env.entry("OPENAI_BASE_URL".into())
            .or_insert_with(|| url.clone());
    }
    env.entry("OPENAI_MODEL".into())
        .or_insert_with(|| state.config.openai_model.clone());

    env
}
