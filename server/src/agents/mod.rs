pub(crate) mod utils;
pub mod acl;
pub mod build_worker;
pub mod deployments;
pub mod grants;
pub mod hours_meter;
pub mod update;
pub mod upload;

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use nasiko_runtime::{ContainerId, ContainerRuntime, DeploymentSpec, DeploymentStatus, ResourceLimits};
use uuid::Uuid;

use crate::state::AppState;

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
pub(crate) fn build_image_tag(registry: &str, name: &str, tag: &str) -> String {
    if registry.is_empty() {
        format!("nasiko/{name}:{tag}")
    } else {
        format!("{registry}/nasiko/{name}:{tag}")
    }
}

/// Mints (or reuses) a per-agent OCI pull credential and attaches it to
/// `spec` — deterministic secret name always set so `ee/k8s-runtime` can
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
pub(crate) async fn attach_pull_credential(db: &sqlx::PgPool, agent_runtime: &str, agent_image_registry: &str, spec: &mut DeploymentSpec, agent_id: Uuid) {
    if agent_runtime != "kubernetes" {
        return;
    }
    spec.image_pull_secret_name = Some(format!("pull-{agent_id}"));
    match nasiko_oci::pull_credentials::get_or_create(db, agent_id).await {
        Ok(Some(cred)) => {
            spec.image_pull_credential_seed = Some((cred.username, cred.token, agent_image_registry.to_string()));
        }
        Ok(None) => {}
        Err(e) => tracing::error!(%e, %agent_id, "failed to mint OCI pull credential; image pulls may fail"),
    }
}

/// Resolves the URL to persist as `agents.url` right after a `deploy()` call.
///
/// `DeploymentStatus::endpoint` (as returned by both `deploy()` and `status()`)
/// is only populated once the workload is observed actually `Running` at that
/// exact instant — see `ee/k8s-runtime`'s `status()`. For Kubernetes, a fresh
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

/// Qualifies an already-composed `name:tag` image reference with
/// `AGENT_IMAGE_REGISTRY`, for the ad-hoc `POST /containers` deploy path
/// (`admin::routes::deploy`) — unlike upload/update/rollback/import, that
/// path receives a pre-built ref string from the caller rather than
/// composing one from separate name+tag args, so it can't go through
/// `build_image_tag` directly.
///
/// Only qualifies images starting with the literal `"nasiko/"` prefix — the
/// CLI's own internal convention for images it just pushed via `nasiko
/// deploy`/`nasiko push` (see `oss/cli/src/commands/deploy.rs`/`push.rs`).
/// An arbitrary third-party reference the caller deploys directly (e.g.
/// `nginx:latest`) is left untouched, since there's no way to distinguish
/// "this needs our private registry" from "this is already resolvable as-is"
/// beyond that one known convention.
pub(crate) fn qualify_deploy_image(registry: &str, image: &str) -> String {
    if registry.is_empty() || !image.starts_with("nasiko/") {
        image.to_string()
    } else {
        format!("{registry}/{image}")
    }
}

pub(crate) fn build_agent_spec(
    agent_id: Uuid,
    name: &str,
    image: impl Into<String>,
    ports: Vec<u16>,
    env: HashMap<String, String>,
    resources: Option<ResourceLimits>,
) -> DeploymentSpec {
    DeploymentSpec {
        container_id: ContainerId::from_uuid(agent_id),
        name: name.to_string(),
        image: image.into(),
        ports: if ports.is_empty() { vec![DEFAULT_AGENT_PORT] } else { ports },
        env_vars: env,
        min_replicas: 1,
        max_replicas: 1,
        resources,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
    }
}

pub fn router() -> Router<AppState> {
    upload::router()
        .merge(deployments::router())
        .merge(update::router())
    // `grants::router()` is deliberately NOT merged here: EE's `build_ee_app`
    // builds on top of this router and mounts its own richer grants router
    // (team/department grants + the live, CLI-consumed request shapes in
    // ee/cli/src/access.rs) at the same paths. Merging both panics on route
    // registration conflicts. This OSS-tier grants module has no CLI/UI/test
    // consumer yet — wire it up (and reconcile it with EE's implementation)
    // in a dedicated pass rather than mounting two incompatible APIs.
}

/// GET routes pulled out from under `router()`'s `require_deployer` gate —
/// mounted separately under `require_auth` only, each handler checks
/// `can_deploy` itself. See `upload::degradable_router`/
/// `deployments::degradable_router`.
pub fn degradable_router() -> Router<AppState> {
    upload::degradable_router().merge(deployments::degradable_router())
}

pub fn user_routes() -> Router<AppState> {
    upload::user_routes()
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    #[test]
    fn always_keys_on_uuid_and_defaults_port() {
        let id = Uuid::new_v4();
        // Same agent_id → same ContainerId regardless of the display name, so every
        // deploy path converges on one workload.
        let a = build_agent_spec(id, "My.Agent", "img:1", vec![], HashMap::new(), None);
        let b = build_agent_spec(id, "totally-different-name", "img:2", vec![], HashMap::new(), None);
        assert_eq!(a.container_id, ContainerId::from_uuid(id));
        assert_eq!(a.container_id, b.container_id);
        // Empty ports → canonical 8000 (not 5000).
        assert_eq!(a.ports, vec![DEFAULT_AGENT_PORT]);
    }

    #[test]
    fn preserves_explicit_ports() {
        let id = Uuid::new_v4();
        let s = build_agent_spec(id, "a", "img:1", vec![9091], HashMap::new(), None);
        assert_eq!(s.ports, vec![9091]);
    }

    #[test]
    fn qualify_deploy_image_passthrough_when_registry_empty() {
        assert_eq!(qualify_deploy_image("", "nasiko/my-agent:1.0.0"), "nasiko/my-agent:1.0.0");
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
        assert_eq!(qualify_deploy_image("registry.example.com", "nginx:latest"), "nginx:latest");
    }

    #[test]
    fn build_image_tag_includes_nasiko_owner_segment_with_registry() {
        // oss/oci's registry routes are shaped /v2/{owner}/{repo}/... (two
        // path segments) — a single-segment repo 404s at the router level.
        assert_eq!(
            build_image_tag("registry.example.com", "my-agent", "1.0.0"),
            "registry.example.com/nasiko/my-agent:1.0.0"
        );
    }

    #[test]
    fn build_image_tag_includes_nasiko_owner_segment_without_registry() {
        assert_eq!(build_image_tag("", "my-agent", "1.0.0"), "nasiko/my-agent:1.0.0");
    }
}
