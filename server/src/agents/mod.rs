pub(crate) mod utils;
pub mod acl;
pub mod build_worker;
pub mod deployments;
pub mod grants;
pub mod update;
pub mod upload;

use std::collections::HashMap;

use axum::Router;
use nasiko_runtime::{ContainerId, DeploymentSpec, ResourceLimits};
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
pub(crate) fn build_image_tag(registry: &str, name: &str, tag: &str) -> String {
    if registry.is_empty() {
        format!("{name}:{tag}")
    } else {
        format!("{registry}/{name}:{tag}")
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
}
