pub mod acl;
pub mod admission;
pub mod build_worker;
pub mod catalog_deploy;
pub mod deploy_env;
pub mod deployments;
pub mod grants;
pub mod hours_meter;
pub mod llm_config;
pub mod open_tasks;
pub mod reconcile;
pub mod rehost;
pub mod sandbox_smoke;
pub mod sharing;
pub mod update;
pub mod upload;
pub(crate) mod utils;
pub mod versions;

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use nasiko_runtime::{ContainerId, ContainerRuntime, DeploymentSpec, DeploymentStatus};
use nasiko_utils::image::strip_image_to_repo;
use uuid::Uuid;

use crate::agent_scaling::{DeployPlan, ReplicaBounds};
use crate::state::AppState;

pub use nasiko_utils::image::image_name_slug;
pub use upload::UploadAndDeployResponse;

/// Canonical container port for Nasiko agents. Every agent image serves on 8000
/// (each agent `Dockerfile` declares `EXPOSE 8000`; see also `seed::AGENT_PORT`).
pub(crate) const DEFAULT_AGENT_PORT: u16 = 8000;

/// Build a [`DeploymentSpec`] for an agent, **always** keying the container on the
/// agent UUID — never the display name.
///
/// This is the single constructor every deploy path (upload / update / rollback /
/// import / seed / admin) must go through. Keying on the UUID is a correctness and
/// security invariant:
/// - update/rollback re-target the SAME workload instead of spawning an orphaned,
///   name-keyed duplicate alongside the original (RUN-2);
/// - two teams' agents that share a display name can't collide on one container
///   object (the cross-team collision the UUID scheme closed);
/// - names that are valid agent names but invalid `ContainerId`s (`.`/uppercase)
///   no longer 500 on the update path.
///
/// `ports` defaults to `[DEFAULT_AGENT_PORT]` when empty so no path silently
/// diverges on the service target port (RUN-7: upload used 5000, update 8000).
/// Build a fully-qualified image tag, applying the configured registry prefix
/// when one is set.
///
/// Every path that builds/pushes/references an agent image (upload / update /
/// rollback / import) must go through this — `upload.rs` had it right; `update.rs`
/// and `catalog/import.rs` built an unprefixed `{name}:{tag}` unconditionally.
/// Latent with the default empty registry, but on K8s with `AGENT_IMAGE_REGISTRY`
/// set, an update/rollback push or deploy referenced an unqualified tag — wrong
/// push target, or an ImagePullBackOff since the cluster has no reason to resolve
/// a bare tag against the configured private registry.
///
/// Always includes the `nasiko/` owner segment — matching the same convention
/// `oss/cli`'s own push/deploy paths already use (`push.rs`/`deploy.rs`'s
/// `format!("nasiko/{agent_name}")`, `qualify_deploy_image`'s doc comment) —
/// because `oss/oci`'s registry routes are shaped `/v2/{owner}/{repo}/...`
/// (two path segments). A bare `{name}:{tag}` push target has only one segment
/// after the host and 404s at the Axum router level before any auth/handler
/// logic runs (found live: BuildKit push to `.../v2/translator/blobs/uploads/`
/// failed with a plain 404, not a 401/403).
///
/// The name segment goes through [`image_name_slug`]: an agent name is a
/// display string that may legally carry uppercase or spaces, neither of which
/// an OCI repository name admits.
///
/// Images are namespaced per owner: `u-{owner_uuid}/{slug}:{tag}`. The legacy
/// `nasiko/` namespace is no longer written except by seeds (BuildService identity).
/// What the generic lifecycle paths answer for an agent of kind `harness`: its workload is the
/// platform's to build (a harness agent's carries a command and settings no agent row records), so
/// a redeploy from the row, or an upload over it, would replace it with something that cannot run.
pub(crate) const HARNESS_AGENT_MANAGED: &str = "a harness agent is managed by the platform: it cannot be redeployed, restarted or replaced by an upload";

/// Whether the agent is a harness agent (`workload_kind = 'harness'`).
pub(crate) async fn is_harness_agent(db: &sqlx::PgPool, agent_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM agents WHERE id = $1 AND workload_kind = 'harness')",
    )
    .bind(agent_id)
    .fetch_one(db)
    .await
}

pub(crate) fn build_image_tag(registry: &str, owner_id: Uuid, name: &str, tag: &str) -> String {
    let slug = image_name_slug(name);
    let namespace = format!("u-{}", owner_id.as_simple());
    if registry.is_empty() {
        format!("{namespace}/{slug}:{tag}")
    } else {
        format!("{registry}/{namespace}/{slug}:{tag}")
    }
}

/// Build a legacy image tag in the `nasiko/` namespace for platform-owned
/// seeds and system agents. New user agents should use `build_image_tag`
/// with an owner_id instead.
pub(crate) fn build_legacy_image_tag(registry: &str, name: &str, tag: &str) -> String {
    let slug = image_name_slug(name);
    if registry.is_empty() {
        format!("nasiko/{slug}:{tag}")
    } else {
        format!("{registry}/nasiko/{slug}:{tag}")
    }
}

/// Mints (or reuses) a per-agent OCI pull credential and attaches it to
/// `spec` — deterministic secret name always set so the Kubernetes runtime can
/// wire `imagePullSecrets` on every deploy, with the one-time plaintext seed
/// set only when a NEW credential was just minted (see `nasiko-oci`'s
/// `pull_credentials::get_or_create`). No-op outside the K8s runtime — these
/// fields are meaningless to `DockerRuntime`, and minting a DB row + credential
/// for a deploy that will never reference it is pointless.
///
/// Takes primitives rather than `&AppState` so the build-worker deploy path
/// (`upload::execute_upload_and_deploy`, which runs detached from a request's
/// `AppState` and already threads individual config values the same way —
/// see its `openai_api_key`/`openai_base_url` params) can call it too.
pub(crate) async fn attach_pull_credential(
    db: &sqlx::PgPool,
    agent_runtime: &str,
    agent_image_registry: &str,
    spec: &mut DeploymentSpec,
    agent_id: Uuid,
) {
    if agent_runtime != "kubernetes" {
        return;
    }
    spec.image_pull_secret_name = Some(format!("pull-{agent_id}"));
    match nasiko_oci::pull_credentials::get_or_create(db, agent_id).await {
        Ok(Some(cred)) => {
            spec.image_pull_credential_seed =
                Some((cred.username, cred.token, agent_image_registry.to_string()));
        }
        Ok(None) => {}
        Err(e) => {
            tracing::error!(%e, %agent_id, "failed to mint OCI pull credential; image pulls may fail")
        }
    }
}

/// Resolves the URL to persist as `agents.url` right after a `deploy()` call.
///
/// `DeploymentStatus::endpoint` (as returned by both `deploy()` and `status()`)
/// is only populated once the workload is observed actually `Running` at that
/// exact instant — see the Kubernetes runtime's `status()`. For Kubernetes, a fresh
/// Deployment/Service apply is essentially never Ready yet by the time
/// `deploy()` returns (scheduling, image pull, and readiness probes all take
/// real time), so every caller that persisted `deploy_status.endpoint`
/// directly was writing an empty `agents.url` on every single deploy, and
/// nothing ever went back to fill it in once the pod *did* become ready —
/// found live: `agent_proxy` 502'd with `NoEndpoint` on an agent that had
/// been happily `Running` for many minutes.
///
/// The Service address itself is deterministic and exists the moment the
/// Service object is created, independent of pod readiness, so fall back to
/// the readiness-independent `endpoint()` call instead of defaulting to an
/// empty string.
pub(crate) async fn resolve_agent_url(
    runtime: &Arc<dyn ContainerRuntime>,
    deploy_status: &DeploymentStatus,
    container_id: &ContainerId,
) -> String {
    if let Some(ep) = &deploy_status.endpoint {
        return ep.clone();
    }
    runtime.endpoint(container_id).await.unwrap_or_default()
}

/// Parse an OCI repository path into namespace and slug components.
///
/// Returns `Some((owner_uuid, slug))` for `u-{32 lowercase hex}/{slug}`,
/// `None` for `nasiko/{slug}` (legacy), and errors for malformed `u-` paths.
/// Parse an owner-namespaced repository - delegates to the shared implementation
/// in `nasiko_utils::image`.
fn parse_owner_namespace(repo: &str) -> Result<Option<(Uuid, &str)>, String> {
    nasiko_utils::image::parse_owner_namespace(repo)
}

/// Canonicalize an image reference (add registry prefix if needed), then validate
/// that it is owned by the caller. This is the single authorization function for
/// user-controlled image references.
///
/// Qualifying first closes the R-123 bypass: an unqualified reference like
/// `u-abc/agent:v1` has no host, so its comparison with the configured registry
/// would incorrectly treat it as external. Qualifying it first produces
/// `registry.example.com/u-abc/agent:v1`, which has a host and can be compared
/// correctly.
///
/// Returns the canonicalized (qualified) image reference on success.
pub(crate) async fn canonicalize_and_validate_image_ref(
    pool: &sqlx::PgPool,
    registry: &str,
    aliases: &[String],
    image: &str,
    caller_id: Uuid,
    is_superuser: bool,
) -> Result<String, (axum::http::StatusCode, String)> {
    let qualified_image = qualify_deploy_image(registry, image);
    validate_image_ref(
        pool,
        registry,
        aliases,
        &qualified_image,
        caller_id,
        is_superuser,
    )
    .await?;
    Ok(qualified_image)
}

/// Validate that an image reference in THIS registry is owned by the caller.
///
/// Images in the caller's own namespace (`u-{caller}/...`) are always allowed.
/// Legacy `nasiko/R` images are allowed iff `owners(R)` is non-empty and contains
/// only the caller (superusers may use any image). External registries (different
/// host) are unaffected.
///
/// Most callers should use `canonicalize_and_validate_image_ref` instead, which
/// qualifies the image first to close the R-123 unqualified-reference bypass.
pub(crate) async fn validate_image_ref(
    pool: &sqlx::PgPool,
    registry: &str,
    aliases: &[String],
    image: &str,
    caller_id: Uuid,
    is_superuser: bool,
) -> Result<(), (axum::http::StatusCode, String)> {
    use axum::http::StatusCode;

    if is_superuser {
        return Ok(());
    }

    let repo = strip_image_to_repo(image);

    // Check if this is an external registry by comparing hosts
    let image_host = if let Some(slash) = image.find('/') {
        let before = &image[..slash];
        if before.contains('.') || before.contains(':') {
            Some(before)
        } else {
            None
        }
    } else {
        None
    };

    let our_registry = if registry.is_empty() {
        None
    } else {
        Some(registry)
    };

    // A host this registry also answers on (its name before a rename) is this
    // registry: an image naming it is held to the same namespace rules, or any
    // caller could deploy someone else's image by spelling the old host.
    let ours = image_host == our_registry
        || image_host.is_some_and(|host| aliases.iter().any(|alias| alias == host));

    // If hosts differ, it's external - allow it. With no registry configured, a
    // hostless reference outside the platform namespaces names a Docker Hub image.
    if !ours || (our_registry.is_none() && !repo.starts_with("u-") && !repo.starts_with("nasiko/"))
    {
        return Ok(());
    }

    // It's our registry - enforce namespace rules
    match parse_owner_namespace(repo) {
        Ok(Some((owner_uuid, _slug))) => {
            if owner_uuid == caller_id {
                Ok(())
            } else {
                Err((
                    StatusCode::FORBIDDEN,
                    format!(
                        "image reference '{image}' is not in your namespace (expected u-{}/...)",
                        caller_id.as_simple()
                    ),
                ))
            }
        }
        Ok(None) => {
            // Legacy nasiko/ path - check owners(R), counting platform images only.
            let repo_without_prefix = repo.strip_prefix("nasiko/").unwrap();

            // Narrow in SQL via substring match, then confirm exact match in Rust
            let rows: Vec<(Uuid, String)> = sqlx::query_as(
                "SELECT owner_id, image FROM agents WHERE deleted_at IS NULL AND strpos(image, $1) > 0",
            )
            .bind(repo)
            .fetch_all(pool)
            .await
            .map_err(|e| {
                tracing::error!(%e, "validate_image_ref: db query failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            })?;

            let mut owner_ids = Vec::new();
            for (owner_id, img) in rows {
                if let Some(platform_repo) =
                    nasiko_utils::image::platform_repository_from_image(&img, registry)
                    && platform_repo == repo
                    && !owner_ids.contains(&owner_id)
                {
                    owner_ids.push(owner_id);
                }
            }

            if owner_ids.is_empty() {
                Err((
                    StatusCode::FORBIDDEN,
                    format!(
                        "legacy repository 'nasiko/{}' is unclaimed - use your own namespace u-{}/... instead",
                        repo_without_prefix,
                        caller_id.as_simple()
                    ),
                ))
            } else if owner_ids.len() == 1 && owner_ids[0] == caller_id {
                Ok(())
            } else if owner_ids.contains(&caller_id) {
                Err((
                    StatusCode::FORBIDDEN,
                    format!(
                        "legacy repository 'nasiko/{}' is shared with other owners - use your own namespace u-{}/... instead",
                        repo_without_prefix,
                        caller_id.as_simple()
                    ),
                ))
            } else {
                Err((
                    StatusCode::FORBIDDEN,
                    format!(
                        "legacy repository 'nasiko/{}' is owned by another user - use your own namespace u-{}/... instead",
                        repo_without_prefix,
                        caller_id.as_simple()
                    ),
                ))
            }
        }
        Err(e) => Err((StatusCode::BAD_REQUEST, e)),
    }
}

/// Qualifies an already-composed `name:tag` image reference with
/// `AGENT_IMAGE_REGISTRY`, for the ad-hoc `POST /containers` deploy path
/// (`admin::routes::deploy`) — unlike upload/update/rollback/import, that
/// path receives a pre-built ref string from the caller rather than
/// composing one from separate name+tag args, so it can't go through
/// `build_image_tag` directly.
///
/// Qualifies images in the platform namespaces: `nasiko/` (legacy) and
/// `u-{32 lowercase hex}/` (per-owner). An arbitrary third-party reference
/// (e.g. `nginx:latest`) is left untouched.
pub fn qualify_deploy_image(registry: &str, image: &str) -> String {
    if registry.is_empty() {
        return image.to_string();
    }

    let needs_qualification = image.starts_with("nasiko/") || {
        // Check for u-{32 hex}/ without regex dependency
        if let Some(rest) = image.strip_prefix("u-") {
            if let Some(slash_pos) = rest.find('/') {
                let hex = &rest[..slash_pos];
                hex.len() == 32
                    && hex
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            } else {
                false
            }
        } else {
            false
        }
    };

    if needs_qualification {
        format!("{registry}/{image}")
    } else {
        image.to_string()
    }
}

/// Tells the agent which port to listen on (the Service targets the first port, and every
/// in-tree agent honors `PORT`). An explicit `PORT` in `env` wins.
pub(crate) fn set_port_env(env: &mut HashMap<String, String>, port: u16) {
    env.entry("PORT".to_string())
        .or_insert_with(|| port.to_string());
}

/// The ports the agent listens on, as its last deployment recorded them (what a restart uses),
/// else its `PORT` env, else the platform default. The first also becomes `PORT`, so the env and
/// the exposed port cannot disagree.
pub(crate) async fn recorded_ports(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    env: &mut HashMap<String, String>,
) -> Vec<u16> {
    let recorded: Option<Vec<i32>> = sqlx::query_scalar(
        "SELECT spec_ports FROM agent_deployments \
         WHERE agent_id = $1 AND spec_ports IS NOT NULL AND cardinality(spec_ports) > 0 \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    let ports = choose_ports(recorded, env.get("PORT").map(String::as_str));
    env.insert("PORT".to_owned(), ports[0].to_string());
    ports
}

fn choose_ports(recorded: Option<Vec<i32>>, env_port: Option<&str>) -> Vec<u16> {
    let recorded: Vec<u16> = recorded
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .collect();
    if !recorded.is_empty() {
        return recorded;
    }
    let from_env = env_port
        .and_then(|p| p.parse::<u16>().ok())
        .filter(|p| *p != 0);
    vec![from_env.unwrap_or(DEFAULT_AGENT_PORT)]
}

/// The spec every framework-agent deploy path sends, `plan` (from
/// [`AgentDeploy::plan`](crate::agent_scaling::AgentDeploy::plan)) deciding its replica bounds
/// and whether it is parkable.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_agent_spec(
    agent_id: Uuid,
    name: &str,
    image: impl Into<String>,
    ports: Vec<u16>,
    mut env: HashMap<String, String>,
    default_memory: &str,
    plan: DeployPlan,
    writable: bool,
    writable_path: Option<String>,
    owner_id: Uuid,
) -> DeploymentSpec {
    let ports = if ports.is_empty() {
        vec![DEFAULT_AGENT_PORT]
    } else {
        ports
    };
    set_port_env(&mut env, ports[0]);
    // A writable agent's files sit on one volume, whatever bounds the caller computed.
    let bounds = if writable {
        ReplicaBounds::SINGLE
    } else {
        plan.bounds
    };
    DeploymentSpec {
        container_id: ContainerId::from_uuid(agent_id),
        name: name.to_string(),
        image: image.into(),
        ports,
        env_vars: env,
        min_replicas: bounds.min,
        max_replicas: bounds.max,
        replica_target: bounds.target,
        // The agent's own CPU and memory where it has any (OSS migration 0101), else the
        // platform's: AGENT_DEFAULT_MEMORY (see Config::agent_default_memory's doc comment)
        // and a 500m CPU limit.
        resources: Some(plan.resources.limits(default_memory)),
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: Default::default(),
        writable,
        writable_path,
        owner_id,
        force_pull: false,
        parkable: plan.parkable,
        overrides: Default::default(),
    }
}

pub fn router() -> Router<AppState> {
    upload::router()
        .merge(deployments::router())
        .merge(update::router())
    // `grants::router()` is deliberately NOT merged here: EE's `build_ee_app`
    // builds on top of this router and mounts its own richer grants router
    // (team/department grants + the live, CLI-consumed request shapes in
    // the enterprise CLI) at the same paths. Merging both panics on route
    // registration conflicts. The OSS-tier grants module IS live — the agent
    // card's "Access & security" tab consumes it — but it is mounted only in
    // the OSS-only composition root (`crate::build_app`), which EE never
    // calls, so each edition serves exactly one grants API.
}

/// GET routes pulled out from under `router()`'s `require_deployer` gate —
/// mounted separately under `require_auth` only, each handler checks
/// `can_deploy` itself. See `upload::degradable_router`/
/// `deployments::degradable_router`.
pub fn degradable_router() -> Router<AppState> {
    upload::degradable_router().merge(deployments::degradable_router())
}

pub fn user_routes() -> Router<AppState> {
    upload::user_routes().merge(llm_config::router())
}

/// Capability route for CLI feature detection.
pub fn capability_routes() -> Router<AppState> {
    use axum::routing::get;
    Router::new().route("/image-namespace", get(get_image_namespace))
}

/// GET /api/agents/image-namespace - returns the caller's image namespace
/// for CLI push/deploy operations.
#[utoipa::path(
    get,
    path = "/api/agents/image-namespace",
    tag = "agents",
    responses(
        (status = 200, description = "Image namespace for this user"),
    ),
)]
async fn get_image_namespace(claims: crate::auth::Claims) -> impl axum::response::IntoResponse {
    use axum::response::IntoResponse;
    let user_id = match claims.user_uuid() {
        Ok(id) => id,
        Err((status, msg)) => return (status, msg).into_response(),
    };
    axum::Json(serde_json::json!({
        "namespace": format!("u-{}", user_id.as_simple())
    }))
    .into_response()
}

#[cfg(test)]
mod spec_tests {
    use super::*;
    use crate::agent_scaling::AgentResources;

    #[test]
    fn a_recorded_port_wins_then_the_env_port_then_the_default() {
        assert_eq!(
            choose_ports(Some(vec![3000, 4000]), Some("9000")),
            [3000, 4000]
        );
        assert_eq!(choose_ports(None, Some("9000")), [9000]);
        assert_eq!(choose_ports(Some(vec![]), None), [DEFAULT_AGENT_PORT]);
        assert_eq!(choose_ports(None, Some("not-a-port")), [DEFAULT_AGENT_PORT]);
        assert_eq!(
            choose_ports(Some(vec![0, -1]), Some("0")),
            [DEFAULT_AGENT_PORT]
        );
    }

    const RESIDENT: DeployPlan = DeployPlan {
        bounds: ReplicaBounds::SINGLE,
        parkable: false,
        resources: AgentResources::PLATFORM,
    };

    #[test]
    fn always_keys_on_uuid_and_defaults_port() {
        let id = Uuid::new_v4();
        // Same agent_id → same ContainerId regardless of the display name, so every
        // deploy path converges on one workload.
        let a = build_agent_spec(
            id,
            "My.Agent",
            "img:1",
            vec![],
            HashMap::new(),
            "512Mi",
            RESIDENT,
            false,
            None,
            Uuid::nil(),
        );
        let b = build_agent_spec(
            id,
            "totally-different-name",
            "img:2",
            vec![],
            HashMap::new(),
            "512Mi",
            RESIDENT,
            false,
            None,
            Uuid::nil(),
        );
        assert_eq!(a.container_id, ContainerId::from_uuid(id));
        assert_eq!(a.container_id, b.container_id);
        // Empty ports → canonical 8000 (not 5000).
        assert_eq!(a.ports, vec![DEFAULT_AGENT_PORT]);
    }

    #[test]
    fn sets_the_port_env_var_to_match_the_default_exposed_port() {
        // Regression: this used to be every CALLER's job to remember. Only
        // seed_agents_if_configured did; restart/upload/update/reconcile/
        // import didn't — confirmed live, a seed-deployed agent restarted
        // via the admin restart endpoint lost $PORT, fell back to its own
        // image's internal default, and the platform's Docker port mapping
        // (still pointed at DEFAULT_AGENT_PORT) reached nothing.
        let id = Uuid::new_v4();
        let s = build_agent_spec(
            id,
            "a",
            "img:1",
            vec![],
            HashMap::new(),
            "512Mi",
            RESIDENT,
            false,
            None,
            Uuid::nil(),
        );
        assert_eq!(
            s.env_vars.get("PORT"),
            Some(&DEFAULT_AGENT_PORT.to_string())
        );
    }

    #[test]
    fn sets_the_port_env_var_to_match_an_explicit_port() {
        let id = Uuid::new_v4();
        let s = build_agent_spec(
            id,
            "a",
            "img:1",
            vec![9091],
            HashMap::new(),
            "512Mi",
            RESIDENT,
            false,
            None,
            Uuid::nil(),
        );
        assert_eq!(s.env_vars.get("PORT"), Some(&"9091".to_string()));
    }

    #[test]
    fn a_caller_supplied_port_env_var_is_never_overwritten() {
        let id = Uuid::new_v4();
        let mut env = HashMap::new();
        env.insert("PORT".to_string(), "1234".to_string());
        let s = build_agent_spec(
            id,
            "a",
            "img:1",
            vec![],
            env,
            "512Mi",
            RESIDENT,
            false,
            None,
            Uuid::nil(),
        );
        assert_eq!(s.env_vars.get("PORT"), Some(&"1234".to_string()));
    }

    #[test]
    fn preserves_explicit_ports() {
        let id = Uuid::new_v4();
        let s = build_agent_spec(
            id,
            "a",
            "img:1",
            vec![9091],
            HashMap::new(),
            "512Mi",
            RESIDENT,
            false,
            None,
            Uuid::nil(),
        );
        assert_eq!(s.ports, vec![9091]);
    }

    #[test]
    fn port_env_tracks_the_first_of_several_ports() {
        let id = Uuid::new_v4();
        let s = build_agent_spec(
            id,
            "a",
            "img:1",
            vec![9091, 9092],
            HashMap::new(),
            "512Mi",
            RESIDENT,
            false,
            None,
            Uuid::nil(),
        );
        assert_eq!(s.ports, vec![9091, 9092]);
        assert_eq!(s.env_vars.get("PORT"), Some(&"9091".to_string()));
    }

    #[test]
    fn spec_replicas_come_from_the_bounds() {
        let bounds = ReplicaBounds {
            min: 2,
            max: 5,
            target: Some(6),
        };
        let s = build_agent_spec(
            Uuid::new_v4(),
            "a",
            "img:1",
            vec![],
            HashMap::new(),
            "512Mi",
            DeployPlan {
                bounds,
                parkable: false,
                resources: AgentResources::PLATFORM,
            },
            false,
            None,
            Uuid::nil(),
        );
        assert_eq!((s.min_replicas, s.max_replicas), (2, 5));
        assert_eq!(s.replica_target, Some(6));
    }

    #[test]
    fn a_writable_spec_runs_one_replica_whatever_the_bounds() {
        let bounds = ReplicaBounds {
            min: 2,
            max: 5,
            target: None,
        };
        let s = build_agent_spec(
            Uuid::new_v4(),
            "a",
            "img:1",
            vec![],
            HashMap::new(),
            "512Mi",
            DeployPlan {
                bounds,
                parkable: false,
                resources: AgentResources::PLATFORM,
            },
            true,
            None,
            Uuid::nil(),
        );
        assert_eq!((s.min_replicas, s.max_replicas), (1, 1));
    }

    #[test]
    fn the_plan_decides_whether_the_spec_is_parkable() {
        for parkable in [false, true] {
            for writable in [false, true] {
                let s = build_agent_spec(
                    Uuid::new_v4(),
                    "a",
                    "img:1",
                    vec![],
                    HashMap::new(),
                    "512Mi",
                    DeployPlan {
                        bounds: ReplicaBounds::SINGLE,
                        parkable,
                        resources: AgentResources::PLATFORM,
                    },
                    writable,
                    None,
                    Uuid::nil(),
                );
                assert_eq!(s.parkable, parkable, "writable={writable}");
                assert_eq!(s.workload_kind, nasiko_runtime::WorkloadKind::Agent);
            }
        }
    }

    fn spec_with(resources: AgentResources) -> DeploymentSpec {
        build_agent_spec(
            Uuid::new_v4(),
            "a",
            "img:1",
            vec![],
            HashMap::new(),
            "1Gi",
            DeployPlan {
                resources,
                ..RESIDENT
            },
            false,
            None,
            Uuid::nil(),
        )
    }

    #[test]
    fn an_agent_without_resources_of_its_own_gets_the_platform_defaults() {
        let limits = spec_with(AgentResources::PLATFORM).resources.unwrap();
        assert_eq!(limits.memory, "1Gi");
        assert_eq!(limits.cpu_milli, AgentResources::DEFAULT_CPU_LIMIT_MILLI);
        assert_eq!(limits.cpu_request_milli, None);
    }

    #[test]
    fn an_agents_own_resources_reach_its_spec_and_fill_in_from_the_defaults() {
        let limits = spec_with(AgentResources {
            cpu_limit_milli: Some(2000),
            cpu_request_milli: Some(250),
            memory_limit_mib: Some(1536),
        })
        .resources
        .unwrap();
        assert_eq!(limits.memory, "1536Mi");
        assert_eq!(limits.cpu_milli, 2000);
        assert_eq!(limits.cpu_request_milli, Some(250));

        let memory_only = spec_with(AgentResources {
            memory_limit_mib: Some(2048),
            ..AgentResources::PLATFORM
        })
        .resources
        .unwrap();
        assert_eq!(memory_only.memory, "2048Mi");
        assert_eq!(
            memory_only.cpu_milli,
            AgentResources::DEFAULT_CPU_LIMIT_MILLI
        );
    }

    #[test]
    fn spec_memory_comes_from_default_memory_parameter() {
        let id = Uuid::new_v4();
        let s = build_agent_spec(
            id,
            "a",
            "img:1",
            vec![],
            HashMap::new(),
            "2Gi",
            RESIDENT,
            false,
            None,
            Uuid::nil(),
        );
        assert_eq!(
            s.resources.expect("resources must always be set").memory,
            "2Gi",
            "memory must match the default_memory parameter (AGENT_DEFAULT_MEMORY), not a \
             hardcoded value"
        );
    }

    #[test]
    fn qualify_deploy_image_passthrough_when_registry_empty() {
        assert_eq!(
            qualify_deploy_image("", "nasiko/my-agent:1.0.0"),
            "nasiko/my-agent:1.0.0"
        );
    }

    #[test]
    fn qualify_deploy_image_prefixes_nasiko_convention() {
        assert_eq!(
            qualify_deploy_image("registry.example.com", "nasiko/my-agent:1.0.0"),
            "registry.example.com/nasiko/my-agent:1.0.0"
        );
    }

    #[test]
    fn qualify_deploy_image_leaves_third_party_image_untouched() {
        assert_eq!(
            qualify_deploy_image("registry.example.com", "nginx:latest"),
            "nginx:latest"
        );
    }

    #[test]
    fn image_name_slug_yields_oci_safe_repository_components() {
        assert_eq!(
            image_name_slug("Infrastructure Manager"),
            "infrastructure-manager"
        );
        assert_eq!(
            image_name_slug("infrastructure_manager"),
            "infrastructure_manager"
        );
        assert_eq!(image_name_slug("Code Reviewer 2.0"), "code-reviewer-20");
    }

    #[test]
    fn image_name_slug_never_yields_an_empty_name() {
        // An empty repo component is as invalid a reference as a spaced one.
        assert_eq!(image_name_slug("---"), "agent");
        assert_eq!(image_name_slug(""), "agent");
    }

    #[test]
    fn image_name_slug_is_idempotent() {
        // Publish slugifies before pushing, and update/rollback re-derive the
        // tag from the stored name — every re-application must land on the same
        // repository, or a second build pushes a second image.
        let published = "infrastructure-manager";
        assert_eq!(image_name_slug("Infrastructure Manager"), published);
        assert_eq!(image_name_slug(published), published);
    }
}

#[test]
fn build_image_tag_with_owner_namespace() {
    let owner_id = Uuid::parse_str("a1b2c3d4-e5f6-a1b2-c3d4-e5f6a1b2c3d4").unwrap();
    let result = build_image_tag("", owner_id, "My Agent", "1.0.0");
    assert_eq!(result, "u-a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4/my-agent:1.0.0");
}

#[test]
fn build_image_tag_with_registry_and_owner() {
    let owner_id = Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap();
    let result = build_image_tag("registry.example.com", owner_id, "test", "2.0.0");
    assert_eq!(
        result,
        "registry.example.com/u-12345678123412341234123456789abc/test:2.0.0"
    );
}

#[test]
fn qualify_deploy_image_recognizes_owner_namespace() {
    let image = "u-a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4/agent:1.0";
    assert_eq!(
        qualify_deploy_image("registry.io", image),
        "registry.io/u-a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4/agent:1.0"
    );
}

#[test]
fn qualify_deploy_image_rejects_malformed_owner_namespace() {
    // Uppercase should not be qualified
    let image = "u-A1B2C3D4E5F6A1B2C3D4E5F6A1B2C3D4/agent:1.0";
    assert_eq!(qualify_deploy_image("registry.io", image), image);
}

#[test]
fn parse_owner_namespace_valid() {
    let repo = "u-a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4/agent";
    let result = parse_owner_namespace(repo).unwrap();
    assert!(result.is_some());
    let (uuid, slug) = result.unwrap();
    assert_eq!(
        uuid.as_simple().to_string(),
        "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4"
    );
    assert_eq!(slug, "agent");
}

#[test]
fn parse_owner_namespace_legacy() {
    let repo = "nasiko/code-reviewer";
    let result = parse_owner_namespace(repo).unwrap();
    assert!(result.is_none());
}

#[test]
fn parse_owner_namespace_malformed() {
    assert!(parse_owner_namespace("u-short/agent").is_err());
    assert!(parse_owner_namespace("u-UPPERCASE32CHARSUPPERCASECHARS/agent").is_err());
    assert!(parse_owner_namespace("unknown/agent").is_err());
}

#[test]
fn strip_image_to_repo_strips_tag() {
    assert_eq!(strip_image_to_repo("u-abc/agent:1.0"), "u-abc/agent");
    assert_eq!(strip_image_to_repo("nasiko/agent:latest"), "nasiko/agent");
}

#[test]
fn strip_image_to_repo_strips_registry() {
    assert_eq!(
        strip_image_to_repo("registry.io/u-a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4/agent:1.0"),
        "u-a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4/agent"
    );
    assert_eq!(
        strip_image_to_repo("registry.io:5000/nasiko/agent"),
        "nasiko/agent"
    );
}

#[test]
fn strip_image_to_repo_bare_repo() {
    assert_eq!(strip_image_to_repo("u-abc/agent"), "u-abc/agent");
    assert_eq!(strip_image_to_repo("nasiko/agent"), "nasiko/agent");
}

#[test]
fn slug_vs_name_mismatch() {
    // The slug for "Code Reviewer" is "code-reviewer"
    let owner_id = Uuid::new_v4();
    let tag = build_image_tag("", owner_id, "Code Reviewer", "1.0");
    assert!(tag.contains("/code-reviewer:"));
    assert!(!tag.contains("/Code Reviewer:"));
}

#[cfg(test)]
mod registry_alias_tests {
    use super::validate_image_ref;
    use uuid::Uuid;

    /// An image naming the registry's former host is this registry's: someone else's namespace
    /// under it is refused, as under the current host, instead of passing as an outside image.
    #[tokio::test]
    async fn an_image_on_a_former_registry_host_is_held_to_the_namespace_rules() {
        let db = sqlx::PgPool::connect_lazy("postgres://hermetic.invalid/none").unwrap();
        let (caller, other) = (Uuid::new_v4(), Uuid::new_v4());
        let image = format!("free.old.example/u-{}/agent:1", other.as_simple());
        let aliases = vec!["free.old.example".to_string()];

        let refused =
            validate_image_ref(&db, "free.app.nasiko.com", &aliases, &image, caller, false).await;
        assert_eq!(refused.unwrap_err().0, axum::http::StatusCode::FORBIDDEN);

        let own = format!("free.old.example/u-{}/agent:1", caller.as_simple());
        assert!(
            validate_image_ref(&db, "free.app.nasiko.com", &aliases, &own, caller, false)
                .await
                .is_ok()
        );
        // Without the alias it is an outside image, which is what the alias closes.
        assert!(
            validate_image_ref(&db, "free.app.nasiko.com", &[], &image, caller, false)
                .await
                .is_ok()
        );
    }
}
