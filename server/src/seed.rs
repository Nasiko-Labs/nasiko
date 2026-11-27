use std::collections::{BTreeMap, HashMap, HashSet};

use nasiko_config::SeedAgentSettings;
use tracing::{info, warn};
use uuid::Uuid;

use crate::agent_scaling::{
    AgentResources, ParkingPolicy, ReplicaBounds, save_agent_resources, save_parking_policy,
};
use crate::catalog::models::Agent;
use crate::seed_mirror::{self, MirrorSettings, RegistryCredential};
use crate::seed_registry_ready;
use crate::state::AppState;
use nasiko_mcp_gateway::catalog;
use nasiko_runtime::{ContainerId, RuntimeState};

const AGENT_PORT: u16 = 8000;

/// Ensure seed agents are deployed and running.
///
/// Reads `SEED_AGENTS` env var (space-separated image refs, e.g.
/// "nasiko/echo-agent ghcr.io/org/nutrition:v2"). For each image:
///
/// 1. Mirror it from its external registry into the embedded registry
///    (`seed_mirror`) — the agent is deployed from the internal copy, so every
///    later pull stays inside the cluster
/// 2. Upsert the DB record (insert if new, update image if changed)
/// 3. Check runtime status
/// 4. Deploy if not running or image changed — the first deploy first waits
///    for the registry to be pullable over TLS (`seed_registry_ready`)
/// 5. Fetch agent card once healthy
///
/// Designed to run as a background task — does not block server startup.
pub async fn seed_agents_if_configured(state: &AppState) {
    let images = match std::env::var("SEED_AGENTS") {
        Ok(val) if !val.trim().is_empty() => {
            info!(images = %val, "SEED_AGENTS configured, checking deployments");
            val
        }
        _ => {
            info!("SEED_AGENTS not set, skipping agent seeding");
            return;
        }
    };

    if registry_address_pending(
        &state.config.agent_image_registry,
        state.config.agent_image_registry_placeholder.as_deref(),
    ) {
        info!(
            registry = %state.config.agent_image_registry,
            "registry address not final yet; seeding deferred to the restart that sets it"
        );
        return;
    }

    let owner_id: Uuid = match sqlx::query_scalar(
        "SELECT id FROM users WHERE is_superuser = true AND deleted_at IS NULL ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(id)) => id,
        _ => {
            warn!("no admin user found, cannot seed agents (run bootstrap first)");
            return;
        }
    };

    let openai_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    let openai_base = std::env::var("OPENAI_BASE_URL").unwrap_or_default();
    // Without the model name, agents fall back to their compiled default
    // (e.g. gpt-4o-mini), which non-OpenAI providers reject with a 400.
    let openai_model = std::env::var("OPENAI_MODEL").unwrap_or_default();
    let otel_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").unwrap_or_default();
    let oci = nasiko_oci::OciState::new(
        state.db.clone(),
        state.oci_storage.clone(),
        state.config.agent_image_registry.clone(),
    );
    let mirror_settings = mirror_settings(&state.config);
    let mut registry_checked = false;
    warn_unmatched_settings(&state.config.seed_agent_settings, &images);

    for source_image in images.split_whitespace() {
        let agent_name = extract_name(source_image);

        // The seed's own row only: names are unique per owner, so a name-only
        // lookup could pick up (and redeploy, with platform credentials) another
        // user's agent that happens to share the name.
        let existing = sqlx::query_as::<_, Agent>(
            "SELECT * FROM agents WHERE name = $1 AND owner_id = $2 AND deleted_at IS NULL",
        )
        .bind(&agent_name)
        .bind(owner_id)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
        if let Some(agent) = &existing {
            keep_resident(&state.db, agent.id, &agent_name).await;
        }

        let Some((mirrored, repointed)) = resolve_seed_image(
            state,
            &oci,
            &mirror_settings,
            source_image,
            &agent_name,
            existing.as_ref(),
        )
        .await
        else {
            continue;
        };
        let image = mirrored.as_str();

        let settings = state.config.seed_agent_settings.get(&agent_name);
        let settings_changed = match &existing {
            Some(agent) => apply_seed_settings(state, agent.id, &agent_name, settings).await,
            None => false,
        };

        let force_pull = std::env::var("SEED_FORCE_PULL").is_ok();
        let needs_deploy = match &existing {
            None => true,
            Some(agent) => {
                // `repointed`: the external tag moved, so the internal tag now
                // holds new content under the same ref string — redeploy so the
                // running agent picks it up. Changed settings reach the workload
                // only through a deploy too.
                let image_changed = agent.image.as_deref() != Some(image) || repointed;
                if image_changed || force_pull || settings_changed {
                    true
                } else {
                    // UUID-keyed (see agents::build_agent_spec / RUN-2c) — the deploy a
                    // few lines below keys on the same UUID, so the liveness probe must
                    // too or it always reports "not found" and redeploys every run.
                    let container_id = ContainerId::from_uuid(agent.id);
                    match state.runtime.status(&container_id).await {
                        Ok(status) => status.state != RuntimeState::Running,
                        Err(_) => true,
                    }
                }
            }
        };

        if !needs_deploy {
            info!(agent = %agent_name, "seed agent already running, skipping");
            continue;
        }

        if !registry_checked {
            wait_for_registry(state).await;
            registry_checked = true;
        }
        info!(agent = %agent_name, image = %image, "seeding agent");

        let agent = match &existing {
            Some(a) => {
                // Update image if it changed
                if a.image.as_deref() != Some(image) {
                    let _ = sqlx::query(
                        "UPDATE agents SET image = $2, status = 'deploying', updated_at = now() WHERE id = $1",
                    )
                    .bind(a.id)
                    .bind(image)
                    .execute(&state.db)
                    .await;
                } else {
                    let _ = sqlx::query(
                        "UPDATE agents SET status = 'deploying', updated_at = now() WHERE id = $1",
                    )
                    .bind(a.id)
                    .execute(&state.db)
                    .await;
                }
                a.clone()
            }
            None => match register_agent(&state.db, &agent_name, image, owner_id).await {
                Ok(a) => {
                    // Before the first deploy's plan is read, so it deploys with them.
                    apply_seed_settings(state, a.id, &agent_name, settings).await;
                    a
                }
                Err(e) => {
                    warn!(agent = %agent_name, error = %e, "failed to register seed agent");
                    continue;
                }
            },
        };

        let scaling = state.agent_scaling.begin_deploy(agent.id).await;
        let plan = match scaling.plan().await {
            Ok(plan) => plan,
            Err(e) => {
                warn!(agent = %agent_name, error = %e, "failed to read the seed agent's deploy plan");
                continue;
            }
        };
        let mut env = HashMap::new();
        env.insert("PORT".into(), AGENT_PORT.to_string());
        if !openai_key.is_empty() {
            env.insert("OPENAI_API_KEY".into(), openai_key.clone());
        }
        if !openai_base.is_empty() {
            env.insert("OPENAI_BASE_URL".into(), openai_base.clone());
        }
        if !openai_model.is_empty() {
            env.insert("OPENAI_MODEL".into(), openai_model.clone());
        }
        if !otel_endpoint.is_empty() {
            env.insert("OTEL_EXPORTER_OTLP_ENDPOINT".into(), otel_endpoint.clone());
        }
        let discovery_url = std::env::var("A2A_DISCOVERY_URL")
            .unwrap_or_else(|_| "http://host.docker.internal:8080".into());
        env.insert("A2A_DISCOVERY_URL".into(), discovery_url);

        // The LLM router replaces the direct OPENAI_API_KEY/BASE_URL set above when
        // configured; the gateway and state credentials rotate on every re-seed.
        crate::agents::deploy_env::apply_platform_env(
            state,
            &mut env,
            agent.id,
            owner_id,
            crate::agents::deploy_env::StateModeOnDeploy::Reset,
        )
        .await;

        // UUID-keyed (see agents::build_agent_spec) so a re-seed re-targets the same
        // workload rather than leaving a name-keyed orphan.
        let mut spec = crate::agents::build_agent_spec(
            agent.id,
            &agent_name,
            image.to_string(),
            vec![AGENT_PORT],
            env,
            &state.config.agent_default_memory,
            plan,
            // SEED_AGENTS has no --writable equivalent yet.
            false,
            None,
            owner_id,
        );
        // Only takes effect when `deploy()` actually recreates the container
        // (no existing container, or its image/env changed) — the idempotent
        // "already running, same image, same env" branch never consults this
        // flag at all, so SEED_FORCE_PULL alone does not force a re-pull of an
        // unchanged, already-running seed agent. See DeploymentSpec::force_pull.
        // A re-pointed tag keeps its ref string, so a runtime with the old
        // content cached must be told to pull again.
        spec.force_pull = force_pull || repointed;
        crate::agents::attach_pull_credential(
            &state.db,
            &state.config.agent_runtime,
            &state.config.agent_image_registry,
            &mut spec,
            agent.id,
        )
        .await;

        match scaling.deploy(&spec).await {
            Ok(status) => {
                info!(agent = %agent_name, ?status, "seed agent deployed");
                let agent_url =
                    crate::agents::resolve_agent_url(&state.runtime, &status, &spec.container_id)
                        .await;
                let _ = sqlx::query(
                    "UPDATE agents SET status = 'running', url = $2, updated_at = now() WHERE id = $1",
                )
                .bind(agent.id)
                .bind(&agent_url)
                .execute(&state.db)
                .await;

                // SEED_AGENTS deploys straight via runtime.deploy() with no
                // agent_builds row behind it — without this, the crash-loop
                // guardian (EE) never sees this agent (docs/CRASH_GUARDIAN_REPORT.md §5.1).
                crate::agents::utils::ensure_deployment_tracked(
                    &state.db,
                    agent.id,
                    Some(owner_id),
                    image,
                )
                .await;

                // Fetch the agent card once the container is healthy. In the background: the
                // wait can be long (see `fetch_agent_card_with_retry`), and the next seed agent
                // must not queue behind it.
                tokio::spawn(crate::agents::utils::fetch_agent_card_with_retry(
                    state.db.clone(),
                    state.http_client.clone(),
                    state.runtime.clone(),
                    state.llm_backend.clone(),
                    agent.id,
                    agent_url.clone(),
                ));
            }
            Err(e) => {
                warn!(agent = %agent_name, error = %e, "failed to deploy seed agent");
                let _ = sqlx::query(
                    "UPDATE agents SET status = 'failed', updated_at = now() WHERE id = $1",
                )
                .bind(agent.id)
                .execute(&state.db)
                .await;
            }
        }
    }
}

/// Holds the first seed deploy until nodes can pull from the embedded
/// registry — see [`seed_registry_ready`].
async fn wait_for_registry(state: &AppState) {
    let registry = &state.config.agent_image_registry;
    let insecure = seed_registry_ready::insecure_registries_from_env();
    let insecure: Vec<&str> = insecure.iter().map(String::as_str).collect();
    if seed_registry_ready::needs_tls_wait(registry, &insecure) {
        seed_registry_ready::wait_until_ready(
            &state.http_client,
            &format!("https://{registry}"),
            seed_registry_ready::READY_TIMEOUT,
            seed_registry_ready::POLL_INTERVAL,
        )
        .await;
    }
}

/// Whether the registry address is still the installer's "not known yet"
/// placeholder. Seeded agents bake that address into their image ref and pull
/// credential, so seeding against it would deploy agents that can never pull.
fn registry_address_pending(registry: &str, placeholder: Option<&str>) -> bool {
    placeholder.is_some_and(|placeholder| placeholder == registry)
}

fn mirror_settings(config: &nasiko_config::Config) -> MirrorSettings {
    let credential = match (
        &config.agent_registry_username,
        &config.agent_registry_password,
    ) {
        (Some(username), Some(password)) => Some(RegistryCredential {
            host: config.agent_registry_host.clone(),
            username: username.clone(),
            password: password.clone(),
        }),
        _ => None,
    };
    MirrorSettings {
        credential,
        platform: config.seed_image_platform.clone(),
        internal_registry: config.agent_image_registry.clone(),
    }
}

/// The internal reference to deploy `source_image` from, after mirroring it.
///
/// A mirror failure never falls back to pulling from the external registry.
/// An agent mirrored on an earlier boot keeps its internal copy (the external
/// registry being down must not take a running agent offline); one that was
/// never mirrored is marked `failed` and skipped.
async fn resolve_seed_image(
    state: &AppState,
    oci: &nasiko_oci::OciState,
    settings: &MirrorSettings,
    source_image: &str,
    agent_name: &str,
    existing: Option<&Agent>,
) -> Option<(String, bool)> {
    let error = match seed_mirror::mirror_image(oci, settings, source_image, agent_name).await {
        Ok(outcome) => return Some((outcome.internal_ref, outcome.repointed)),
        Err(e) => e,
    };
    let previous = existing.and_then(|a| a.image.clone()).filter(|i| {
        i.contains(&format!(
            "nasiko/{}:",
            crate::agents::image_name_slug(agent_name)
        ))
    });
    if let Some(previous) = previous {
        warn!(agent = %agent_name, image = %source_image, %error,
            "seed image mirror failed; keeping the previously mirrored copy");
        return Some((previous, false));
    }
    warn!(agent = %agent_name, image = %source_image, %error, "seed image mirror failed");
    if let Some(agent) = existing {
        let _ =
            sqlx::query("UPDATE agents SET status = 'failed', updated_at = now() WHERE id = $1")
                .bind(agent.id)
                .execute(&state.db)
                .await;
    }
    None
}

/// One internal agent to deploy.
///
/// An internal agent is a platform-owned agent excluded from every agent
/// list, routing candidate and A2A discovery query (`is_internal = true`, see
/// `migrations/0014_agent_internal_flag.sql`), reachable only through whatever
/// dedicated route its owner mounts rather than the generic
/// explicit-`agent_id` A2A dispatch path.
///
/// The caller owns the policy — which image, under what name, and what the
/// container needs in its environment. This module owns the mechanics, which
/// are the ones `seed_agents_if_configured` already uses for ordinary agents.
/// The split is deliberate: an edition that ships such an agent describes it
/// in its own tree, and this one carries no particular agent's vocabulary.
pub struct InternalAgentSeed {
    /// Agent row name, and the name the deployment is tracked under.
    pub name: String,
    /// Container image reference.
    pub image: String,
    /// Redeploy even when the image reference has not changed — for reusing
    /// one tag across builds instead of bumping it, the single-agent
    /// equivalent of `SEED_FORCE_PULL`.
    pub force_pull: bool,
    /// The agent's own settings, stored as its `secrets_env` so every later
    /// redeploy (restart, reconcile) gives the container the same environment.
    pub env: HashMap<String, String>,
    pub public: bool,
    /// Where the agent's persistent writable volume is mounted — the internal
    /// agent's `--writable-path`. `None` deploys without one, which leaves no
    /// writable path at all where the runtime makes the root filesystem
    /// read-only (`KubeRuntime`).
    pub writable_path: Option<String>,
}

/// Deploys one internal agent the same way `seed_agents_if_configured` deploys
/// any other, except the row carries `is_internal = true`.
///
/// Designed to run as a background task — does not block server startup, and
/// returns quietly when the agent is already running on the same image.
pub async fn seed_internal_agent(state: &AppState, seed: InternalAgentSeed) {
    let InternalAgentSeed {
        name: agent_name,
        image,
        force_pull,
        env: extra_env,
        public,
        writable_path,
    } = seed;
    if let Some(path) = writable_path.as_deref()
        && let Err(e) = nasiko_runtime::validate_writable_path(path)
    {
        warn!(agent = %agent_name, error = %e, "invalid internal agent writable path, not seeding");
        return;
    }

    let owner_id: Uuid = match sqlx::query_scalar(
        "SELECT id FROM users WHERE is_superuser = true AND deleted_at IS NULL ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(id)) => id,
        _ => {
            warn!(agent = %agent_name, "no admin user found, cannot seed internal agent (run bootstrap first)");
            return;
        }
    };

    // Only a live internal row is ours. Agent names are unique per owner, not
    // globally, so a name-only lookup adopted any user's agent with this name
    // (and a soft-deleted one): the seed then rewrote its image and settings and
    // redeployed it under that user's ownership. `is_internal` is set only here.
    let existing = sqlx::query_as::<_, Agent>(
        "SELECT * FROM agents WHERE name = $1 AND is_internal AND deleted_at IS NULL",
    )
    .bind(&agent_name)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    if let Some(agent) = &existing {
        keep_resident(&state.db, agent.id, &agent_name).await;
    }

    let needs_deploy = match &existing {
        None => true,
        Some(agent) => {
            let image_changed = agent.image.as_deref() != Some(image.as_str());
            let settings_changed =
                crate::catalog::agent_secrets::resolve_agent_env(&state.db, agent.id).await
                    != extra_env
                    || stored_writable(&state.db, agent.id).await.ok()
                        != Some((writable_path.is_some(), writable_path.clone()));
            let _ = sqlx::query(
                "UPDATE agents SET is_public = $2 WHERE id = $1 AND is_public IS DISTINCT FROM $2",
            )
            .bind(agent.id)
            .bind(public)
            .execute(&state.db)
            .await;
            if image_changed || settings_changed || force_pull {
                true
            } else {
                let container_id = ContainerId::from_uuid(agent.id);
                match state.runtime.status(&container_id).await {
                    Ok(status) => status.state != RuntimeState::Running,
                    Err(_) => true,
                }
            }
        }
    };
    if !needs_deploy {
        info!(agent = %agent_name, "internal agent already running, skipping");
        return;
    }

    info!(agent = %agent_name, %image, "seeding internal agent");

    let agent = match &existing {
        Some(a) => {
            let _ = sqlx::query(
                "UPDATE agents SET image = $2, is_public = $3, writable = $4, writable_path = $5, \
                 status = 'deploying', updated_at = now() WHERE id = $1",
            )
            .bind(a.id)
            .bind(&image)
            .bind(public)
            .bind(writable_path.is_some())
            .bind(&writable_path)
            .execute(&state.db)
            .await;
            a.clone()
        }
        None => {
            let inserted = sqlx::query_as::<_, Agent>(
                r#"INSERT INTO agents (name, owner_id, image, status, is_public, is_internal, writable, writable_path, parking_policy, metadata)
                   VALUES ($1, $2, $3, 'deploying', $4, true, $5, $6, $7, '{"seed": true}')
                   RETURNING *"#,
            )
            .bind(&agent_name)
            .bind(owner_id)
            .bind(&image)
            .bind(public)
            .bind(writable_path.is_some())
            .bind(&writable_path)
            .bind(ParkingPolicy::Resident.as_str())
            .fetch_one(&state.db)
            .await;
            match inserted {
                Ok(a) => a,
                Err(e) => {
                    warn!(agent = %agent_name, error = %e, "failed to register internal agent");
                    return;
                }
            }
        }
    };

    if let Err(e) =
        crate::catalog::agent_secrets::replace_agent_env(&state.db, agent.id, &extra_env).await
    {
        warn!(agent = %agent_name, error = %e, "failed to store internal agent settings");
        return;
    }
    if let Err(e) =
        deploy_internal_agent(state, &agent_name, agent.id, owner_id, &image, force_pull).await
    {
        warn!(agent = %agent_name, error = %e, "failed to deploy internal agent");
        let _ =
            sqlx::query("UPDATE agents SET status = 'failed', updated_at = now() WHERE id = $1")
                .bind(agent.id)
                .execute(&state.db)
                .await;
    }
}

async fn deploy_internal_agent(
    state: &AppState,
    agent_name: &str,
    agent_id: Uuid,
    owner_id: Uuid,
    image: &str,
    force_pull: bool,
) -> Result<(), String> {
    let scaling = state.agent_scaling.begin_deploy(agent_id).await;
    let plan = scaling
        .plan()
        .await
        .map_err(|e| format!("failed to read the deploy plan: {e}"))?;
    let mut env = state.agent_env(agent_id).await;
    env.insert("PORT".into(), AGENT_PORT.to_string());
    // Without the gateway credential, McpInjector still sets MCP_GATEWAY_URL but has
    // no MCP_GATEWAY_TOKEN to complete MCP_GATEWAY_CONNECT_URL with, leaving the
    // gateway unreachable from inside the container. Credentials only: an internal agent's
    // model access comes from its own `extra_env`, never the LLM router.
    crate::agents::deploy_env::apply_credentials(
        state,
        &mut env,
        agent_id,
        crate::agents::deploy_env::StateModeOnDeploy::Reset,
    )
    .await;

    // Read from the row, not passed in, so the watch's redeploy below mounts
    // the same volume the seed asked for.
    let (writable, writable_path) = stored_writable(&state.db, agent_id)
        .await
        .map_err(|e| format!("failed to read the writable storage settings: {e}"))?;
    let mut spec = crate::agents::build_agent_spec(
        agent_id,
        agent_name,
        image.to_string(),
        vec![AGENT_PORT],
        env,
        &state.config.agent_default_memory,
        plan,
        writable,
        writable_path,
        owner_id,
    );
    spec.force_pull = force_pull;
    crate::agents::attach_pull_credential(
        &state.db,
        &state.config.agent_runtime,
        &state.config.agent_image_registry,
        &mut spec,
        agent_id,
    )
    .await;

    let status = scaling.deploy(&spec).await.map_err(|e| e.to_string())?;
    info!(agent = %agent_name, ?status, "internal agent deployed");
    let agent_url =
        crate::agents::resolve_agent_url(&state.runtime, &status, &spec.container_id).await;
    let _ = sqlx::query(
        "UPDATE agents SET status = 'running', url = $2, transport_path = '/jsonrpc', updated_at = now() WHERE id = $1",
    )
    .bind(agent_id)
    .bind(&agent_url)
    .execute(&state.db)
    .await;
    crate::agents::utils::ensure_deployment_tracked(&state.db, agent_id, Some(owner_id), image)
        .await;
    tokio::spawn(crate::agents::utils::fetch_agent_card_with_retry(
        state.db.clone(),
        state.http_client.clone(),
        state.runtime.clone(),
        state.llm_backend.clone(),
        agent_id,
        agent_url,
    ));
    Ok(())
}

/// Keeps a platform agent (a seed or an internal agent) `resident`: it always runs. Written on
/// every seed, so a row created while new agents defaulted to `auto` is resident too. A failed
/// write is logged and the seed goes on; the agent's next seed tries again.
async fn keep_resident(db: &sqlx::PgPool, agent_id: Uuid, agent_name: &str) {
    if let Err(e) = save_parking_policy(db, agent_id, ParkingPolicy::Resident).await {
        warn!(agent = %agent_name, error = %e, "could not keep the platform agent resident");
    }
}

/// Saves the agent's `SEED_AGENT_SETTINGS` entry, the config being the source of truth for what
/// it names: its CPU and memory (a value it leaves out returns to the platform's), and, when it
/// names replicas, its replica bounds as a stateless agent. An agent with no entry keeps what it
/// has. `true` when anything changed, which only a deploy brings to its workload. A failed save is
/// logged and the agent deploys with what it had.
async fn apply_seed_settings(
    state: &AppState,
    agent_id: Uuid,
    agent_name: &str,
    settings: Option<&SeedAgentSettings>,
) -> bool {
    let Some(settings) = settings else {
        return false;
    };
    let resources = AgentResources {
        cpu_limit_milli: settings.cpu_limit_milli,
        cpu_request_milli: settings.cpu_request_milli,
        memory_limit_mib: settings.memory_limit_mib,
    };
    let resources_changed = save_agent_resources(&state.db, agent_id, resources)
        .await
        .unwrap_or_else(|e| {
            warn!(agent = %agent_name, error = %e, "could not save the seed agent's CPU and memory");
            false
        });
    let bounds_changed = match settings.replicas {
        None => false,
        Some((min, max)) => {
            let bounds = ReplicaBounds {
                min,
                max,
                target: settings.replica_target,
            };
            state
                .agent_scaling
                .declare_stateless_bounds(agent_id, bounds)
                .await
                .unwrap_or_else(|e| {
                    warn!(agent = %agent_name, error = %e, "could not save the seed agent's replica bounds");
                    false
                })
        }
    };
    if resources_changed || bounds_changed {
        info!(agent = %agent_name, ?settings, "seed agent settings changed");
    }
    resources_changed || bounds_changed
}

/// Warns for each `SEED_AGENT_SETTINGS` entry that names no agent in `images` (`SEED_AGENTS`):
/// most likely a typo, which would leave the agent it meant on the platform's defaults.
fn warn_unmatched_settings(settings: &BTreeMap<String, SeedAgentSettings>, images: &str) {
    for name in unmatched_settings(settings, images) {
        warn!(agent = %name, "SEED_AGENT_SETTINGS names an agent SEED_AGENTS does not seed; ignored");
    }
}

fn unmatched_settings<'a>(
    settings: &'a BTreeMap<String, SeedAgentSettings>,
    images: &str,
) -> Vec<&'a str> {
    let seeded: HashSet<String> = images.split_whitespace().map(extract_name).collect();
    settings
        .keys()
        .filter(|name| !seeded.contains(name.as_str()))
        .map(String::as_str)
        .collect()
}

/// The `(writable, writable_path)` an agent's row records, as `build_agent_spec` takes them.
async fn stored_writable(
    db: &sqlx::PgPool,
    agent_id: Uuid,
) -> Result<(bool, Option<String>), sqlx::Error> {
    sqlx::query_as("SELECT writable, writable_path FROM agents WHERE id = $1")
        .bind(agent_id)
        .fetch_one(db)
        .await
}

const INTERNAL_AGENT_WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

pub fn spawn_internal_agent_watch(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval_at(
            tokio::time::Instant::now() + INTERNAL_AGENT_WATCH_INTERVAL,
            INTERNAL_AGENT_WATCH_INTERVAL,
        );
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            redeploy_dead_internal_agents(&state).await;
        }
    });
}

async fn redeploy_dead_internal_agents(state: &AppState) {
    let agents: Vec<(Uuid, String, Uuid, String)> = match sqlx::query_as(
        "SELECT id, name, owner_id, image FROM agents
         WHERE is_internal AND status = 'running' AND image IS NOT NULL AND deleted_at IS NULL",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "internal agent watch: lookup failed");
            return;
        }
    };
    for (agent_id, name, owner_id, image) in agents {
        let alive = matches!(
            state.runtime.status(&ContainerId::from_uuid(agent_id)).await,
            Ok(status) if matches!(status.state, RuntimeState::Running | RuntimeState::Pending)
        );
        if alive {
            continue;
        }
        warn!(agent = %name, "internal agent is not running, redeploying");
        if let Err(e) = deploy_internal_agent(state, &name, agent_id, owner_id, &image, false).await
        {
            warn!(agent = %name, error = %e, "internal agent redeploy failed, retrying next round");
        }
    }
}

/// Extract agent name from image ref: "nasiko/echo-agent:v1" -> "echo-agent"
/// Extract agent name from image ref: "nasiko/echo-agent:v1" -> "echo-agent",
/// "localhost:5050/echo-agent:v1" -> "echo-agent".
///
/// Splits on `/` FIRST, not `:` first — a registry host commonly carries its
/// own port (`localhost:5050/...`, `myregistry.example.com:5000/...`), and
/// splitting on the first `:` would wrongly treat that port as the tag
/// separator, extracting the registry host itself as the "name" (confirmed
/// live: `localhost:5050/weave-build-orchestrator:v1` produced an agent
/// literally named "localhost"). The tag, if any, only ever appears after
/// the LAST `/`, so isolating that final path segment before splitting on
/// `:` is unambiguous regardless of how many colons a registry host has.
fn extract_name(image: &str) -> String {
    // A digest pin (`name@sha256:…`) is not part of the name.
    let image = image.split('@').next().unwrap_or(image);
    let last_segment = image.rsplit('/').next().unwrap_or(image);
    last_segment
        .split(':')
        .next()
        .unwrap_or(last_segment)
        .to_string()
}

async fn register_agent(
    db: &sqlx::PgPool,
    name: &str,
    image: &str,
    owner_id: Uuid,
) -> Result<Agent, sqlx::Error> {
    sqlx::query_as::<_, Agent>(
        r#"INSERT INTO agents (name, owner_id, image, status, is_public, parking_policy, metadata)
           VALUES ($1, $2, $3, 'deploying', true, $4, '{"seed": true}')
           RETURNING *"#,
    )
    .bind(name)
    .bind(owner_id)
    .bind(image)
    .bind(ParkingPolicy::Resident.as_str())
    .fetch_one(db)
    .await
}

pub(crate) fn toolkit_description(toolkit: &str) -> Option<&'static str> {
    Some(match toolkit {
        "slack" => "Send messages, read channels, and manage threads in Slack.",
        "gmail" => "Read, send, and manage emails in Gmail.",
        "github" => "Manage repositories, issues, pull requests, and code search on GitHub.",
        "notion" => "Create, read, and update pages and databases in Notion.",
        "jira" => "Create, update, and search Jira issues and sprints.",
        "linear" => "Manage issues, projects, and cycles in Linear.",
        "asana" => "Create and manage tasks, projects, and workspaces in Asana.",
        "trello" => "Manage boards, lists, and cards in Trello.",
        "discord" => "Send messages, manage channels, and interact with Discord servers.",
        "dropbox" => "Upload, download, and manage files in Dropbox.",
        "salesforce" => "Query, create, and update records in Salesforce CRM.",
        "hubspot" => "Manage contacts, deals, and marketing in HubSpot.",
        "zendesk" => "Create and manage support tickets in Zendesk.",
        "intercom" => "Manage conversations, contacts, and messages in Intercom.",
        "stripe" => "Manage payments, customers, and subscriptions in Stripe.",
        "airtable" => "Read and write records in Airtable bases.",
        "clickup" => "Manage tasks, spaces, and goals in ClickUp.",
        "monday" => "Manage boards, items, and workflows in monday.com.",
        "gitlab" => "Manage repositories, merge requests, and CI/CD pipelines on GitLab.",
        "pagerduty" => "Manage incidents, schedules, and escalation policies in PagerDuty.",
        "sentry" => "Monitor errors, manage issues, and track releases in Sentry.",
        "confluence" => "Create and manage pages, spaces, and content in Confluence.",
        "bitbucket" => "Manage repositories, pull requests, and pipelines on Bitbucket.",
        "googlecalendar" => "List, create, and manage events in Google Calendar.",
        "googledrive" => "Upload, download, and manage files in Google Drive.",
        "googlesheets" => "Read, write, and manage spreadsheets in Google Sheets.",
        _ => return None,
    })
}

pub(crate) fn toolkit_logo_url(toolkit: &str) -> Option<&'static str> {
    // simple-icons on jsdelivr CDN — monochrome SVGs, always available.
    Some(match toolkit {
        "slack" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/slack.svg",
        "gmail" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/gmail.svg",
        "github" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/github.svg",
        "notion" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/notion.svg",
        "jira" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/jira.svg",
        "linear" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/linear.svg",
        "asana" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/asana.svg",
        "trello" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/trello.svg",
        "discord" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/discord.svg",
        "dropbox" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/dropbox.svg",
        "salesforce" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/salesforce.svg",
        "hubspot" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/hubspot.svg",
        "zendesk" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/zendesk.svg",
        "intercom" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/intercom.svg",
        "stripe" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/stripe.svg",
        "airtable" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/airtable.svg",
        "clickup" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/clickup.svg",
        "monday" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/monday.svg",
        "gitlab" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/gitlab.svg",
        "pagerduty" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/pagerduty.svg",
        "sentry" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/sentry.svg",
        "confluence" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/confluence.svg",
        "bitbucket" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/bitbucket.svg",
        "googlecalendar" => {
            "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/googlecalendar.svg"
        }
        "googledrive" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/googledrive.svg",
        "googlesheets" => "https://cdn.jsdelivr.net/npm/simple-icons@latest/icons/googlesheets.svg",
        _ => return None,
    })
}

/// The Composio toolkits an admin can switch on from Settings — the ones with a
/// description and logo above.
pub(crate) const KNOWN_TOOLKITS: &[&str] = &[
    "slack",
    "gmail",
    "github",
    "notion",
    "jira",
    "linear",
    "asana",
    "trello",
    "discord",
    "dropbox",
    "salesforce",
    "hubspot",
    "zendesk",
    "intercom",
    "stripe",
    "airtable",
    "clickup",
    "monday",
    "gitlab",
    "pagerduty",
    "sentry",
    "confluence",
    "bitbucket",
    "googlecalendar",
    "googledrive",
    "googlesheets",
];

/// Auto-register Composio toolkits from `SEED_TOOLKITS` config.
/// Runs once at boot — skips toolkits that already exist in the DB.
/// Needs a Composio key (`COMPOSIO_API_KEY` or one saved in Settings); skips otherwise.
pub async fn seed_toolkits_if_configured(state: &AppState) {
    if state.config.seed_toolkits.is_empty() {
        info!("SEED_TOOLKITS not set, skipping toolkit seeding");
        return;
    }
    if state.mcp.providers.composio.get().is_none() {
        warn!(
            "SEED_TOOLKITS is set but no Composio API key is configured — \
             cannot register Composio toolkits without one"
        );
        return;
    }
    info!(
        count = state.config.seed_toolkits.len(),
        "SEED_TOOLKITS configured, registering"
    );
    register_toolkits(state, &state.config.seed_toolkits).await;
}

/// Register `toolkits` as platform Composio connectors and sync their tools; ones
/// already registered are skipped. Returns `(toolkit, error)` for each that failed.
pub(crate) async fn register_toolkits(
    state: &AppState,
    toolkits: &[String],
) -> Vec<(String, String)> {
    let mut failed: Vec<(String, String)> = Vec::new();

    // Phase 1: register each toolkit connector (fast, one API call each).
    let mut newly_seeded: HashMap<String, Uuid> = HashMap::new();
    for toolkit in toolkits {
        let toolkit = toolkit.to_lowercase();
        if nasiko_mcp_gateway::repo::get_composio_connector_by_name(&state.db, &toolkit)
            .await
            .ok()
            .flatten()
            .is_some()
        {
            info!(toolkit = %toolkit, "toolkit already registered, skipping");
            continue;
        }
        let logo = toolkit_logo_url(&toolkit);
        let desc = toolkit_description(&toolkit);
        match catalog::create_composio_connector(
            &state.mcp,
            catalog::CreateComposioInput {
                toolkit: &toolkit,
                use_composio_managed: true,
                client_id: None,
                client_secret: None,
                scopes: None,
                display_name: None,
                description: desc,
                logo_url: logo,
            },
        )
        .await
        {
            Ok(view) => {
                info!(toolkit = %toolkit, "seeded composio toolkit");
                if let Some(id) = view.get("connector_id").and_then(|v| v.as_str())
                    && let Ok(cid) = uuid::Uuid::parse_str(id)
                {
                    newly_seeded.insert(toolkit, cid);
                }
            }
            Err(e) => {
                warn!(toolkit = %toolkit, %e, "failed to seed composio toolkit");
                failed.push((toolkit, e.to_string()));
            }
        }
    }

    // Phase 2: sync tools per-toolkit using the v3.1 API (server-side filtered).
    // The previous bulk scan (v3 `/api/v3/tools`) hit a 50-page safety cap and
    // missed tools — e.g. gmail has 63 tools but only 23 were synced.  The v3.1
    // per-toolkit endpoint returns only that toolkit's tools, no cap risk.
    if newly_seeded.is_empty() {
        return failed;
    }
    let Some(provider) = state.mcp.providers.composio.get() else {
        return failed;
    };
    info!(
        count = newly_seeded.len(),
        "syncing tools for newly seeded toolkits (per-toolkit v3.1 API)"
    );
    for (toolkit, cid) in &newly_seeded {
        let tools = match provider.list_toolkit_tools(toolkit).await {
            Ok(t) => t,
            Err(e) => {
                warn!(toolkit = %toolkit, %e, "failed to fetch tools for toolkit");
                continue;
            }
        };
        if tools.is_empty() {
            continue;
        }
        let parsed: Vec<(String, Option<String>, Option<serde_json::Value>)> = tools
            .iter()
            .map(|t| {
                (
                    t.name.clone(),
                    t.description.clone(),
                    t.input_schema.clone(),
                )
            })
            .collect();
        match nasiko_mcp_gateway::repo::upsert_connector_tools(&state.db, *cid, &parsed).await {
            Ok(()) => {
                info!(toolkit = %toolkit, count = parsed.len(), "synced toolkit tools to DB");
                nasiko_mcp_gateway::cache::delete(
                    &state.mcp.redis,
                    &nasiko_mcp_gateway::catalog::toolcount_cache_key(*cid),
                )
                .await;
            }
            Err(e) => warn!(toolkit = %toolkit, %e, "failed to sync tools"),
        }
    }

    // Phase 3: backfill tools for any existing Composio connector whose
    // mcp_connector_tools is empty — handles restarts after a failed sync or
    // cleared table.  Newly seeded connectors were already handled above.
    match nasiko_mcp_gateway::repo::list_composio_connectors(&state.db).await {
        Ok(all_composio) => {
            for conn in all_composio {
                if newly_seeded.values().any(|id| *id == conn.id) {
                    continue; // already synced above
                }
                let tools =
                    nasiko_mcp_gateway::repo::list_connector_tools(&state.db, conn.id).await;
                if matches!(&tools, Ok(t) if t.is_empty()) {
                    info!(connector = %conn.name, "backfilling empty tool catalog");
                    if let Some(owner) = conn.owner_id
                        && let Err(e) = nasiko_mcp_gateway::permissions::sync_connector_tools_by_id(
                            &state.mcp, owner, conn.id,
                        )
                        .await
                    {
                        warn!(connector = %conn.name, %e, "backfill sync failed");
                    }
                }
            }
        }
        Err(e) => warn!(%e, "failed to list composio connectors for backfill"),
    }

    // Rebuild the search index once after all tool syncs are done.
    if let Err(e) = state.mcp.search_index.rebuild(&state.db).await {
        warn!(%e, "search index rebuild after seed failed");
    }
    failed
}

/// What a seed agent may never do through a toolkit, whoever calls it: delete, and send mail (a
/// reply is saved as a draft). Lowercase, as the gateway compares (`permissions::get_stance`).
const SEED_AGENT_TOOL_RULES: &[&str] = &[
    "*delete*",
    "*remove*",
    "*trash*",
    "gmail_send*",
    "gmail_reply*",
    "gmail_forward*",
];

/// Turn the platform's Composio toolkits on for the seed agents, so they can read and write the
/// apps a user names. Each call still runs on the CALLER's own connection: the gateway resolves
/// tools per user, and a user who has not connected the app gets a connect card instead.
///
/// A connector without an access row is off for an agent (`PermissionContext::decide`), and only
/// the agent's owner can add one, so without this no user could reach a toolkit through a seed
/// agent. Existing rows are left alone, so a toolkit an admin switched off for a seed agent stays
/// off.
/// Runs at boot after the toolkits are registered; a toolkit added later is turned on at the next
/// start.
pub async fn grant_toolkits_to_seed_agents(state: &AppState) {
    let names: Vec<String> = std::env::var("SEED_AGENTS")
        .unwrap_or_default()
        .split_whitespace()
        .map(extract_name)
        .collect();
    if names.is_empty() {
        return;
    }
    let rules: Vec<serde_json::Value> = SEED_AGENT_TOOL_RULES
        .iter()
        .map(|pattern| serde_json::json!({ "pattern": pattern, "stance": "block" }))
        .collect();
    // The seed's own rows only (as in `seed_agents_if_configured`): another user's agent may
    // share a seed agent's name.
    let granted = sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO mcp_agent_connector_access (agent_id, connector_id, enabled, tool_rules)
           SELECT a.id, c.id, true, $2
           FROM agents a
           CROSS JOIN mcp_connectors c
           WHERE a.name = ANY($1)
             AND a.deleted_at IS NULL
             AND a.owner_id = (SELECT id FROM users WHERE is_superuser = true AND deleted_at IS NULL
                               ORDER BY created_at LIMIT 1)
             AND c.provider_type = 'composio'
             AND COALESCE(c.is_active, true)
           ON CONFLICT (agent_id, connector_id) DO NOTHING
           RETURNING agent_id"#,
    )
    .bind(&names)
    .bind(serde_json::Value::Array(rules))
    .fetch_all(&state.db)
    .await;
    match granted {
        Ok(rows) => {
            let agents: HashSet<Uuid> = rows.into_iter().collect();
            for agent_id in &agents {
                nasiko_mcp_gateway::permissions::invalidate_permission_cache(&state.mcp, *agent_id)
                    .await;
            }
            if !agents.is_empty() {
                info!(
                    agents = agents.len(),
                    "seed agents: Composio toolkits turned on"
                );
            }
        }
        Err(e) => warn!(%e, "seed agents: turning on Composio toolkits failed"),
    }
}

#[cfg(test)]
mod registry_address_pending_tests {
    use super::registry_address_pending;

    #[test]
    fn seeding_waits_while_the_registry_is_the_placeholder() {
        assert!(registry_address_pending(
            "nasiko.pending.nip.io",
            Some("nasiko.pending.nip.io")
        ));
    }

    #[test]
    fn seeding_runs_once_the_real_address_is_set() {
        assert!(!registry_address_pending(
            "nasiko.34.1.2.3.nip.io",
            Some("nasiko.pending.nip.io")
        ));
    }

    #[test]
    fn seeding_runs_when_no_placeholder_is_configured() {
        assert!(!registry_address_pending("", None));
        assert!(!registry_address_pending("registry.example.com", None));
    }
}

#[cfg(test)]
mod unmatched_settings_tests {
    use super::*;

    #[test]
    fn only_settings_for_an_agent_seed_agents_does_not_seed_are_unmatched() {
        let settings: BTreeMap<String, SeedAgentSettings> = ["summarizer", "summariser"]
            .into_iter()
            .map(|name| (name.to_owned(), SeedAgentSettings::default()))
            .collect();
        let images = "registry.nasiko.com/nasiko/summarizer:1.0.0 nasiko/translator:1.0.0";
        assert_eq!(unmatched_settings(&settings, images), ["summariser"]);
    }
}

#[cfg(test)]
mod extract_name_tests {
    use super::extract_name;

    #[test]
    fn digest_pin_is_not_part_of_the_name() {
        assert_eq!(
            extract_name("us-central1-docker.pkg.dev/p/agents/hr@sha256:abc123"),
            "hr"
        );
        assert_eq!(extract_name("ghcr.io/org/hr:v1@sha256:abc123"), "hr");
    }

    #[test]
    fn plain_dockerhub_style_ref() {
        assert_eq!(extract_name("nasiko/echo-agent:v1"), "echo-agent");
    }

    #[test]
    fn no_tag_at_all() {
        assert_eq!(extract_name("nasiko/echo-agent"), "echo-agent");
    }

    #[test]
    fn bare_name_no_namespace_no_tag() {
        assert_eq!(extract_name("echo-agent"), "echo-agent");
    }

    #[test]
    fn registry_host_with_a_port_is_not_mistaken_for_the_tag_separator() {
        // Regression: a naive "split on first colon" treats the registry
        // port as the tag delimiter, extracting "localhost" as the name
        // instead of the actual repository — confirmed live before this fix.
        assert_eq!(
            extract_name("localhost:5050/weave-build-orchestrator:v1"),
            "weave-build-orchestrator"
        );
    }

    #[test]
    fn registry_host_with_a_port_and_no_tag() {
        assert_eq!(
            extract_name("localhost:5050/weave-build-orchestrator"),
            "weave-build-orchestrator"
        );
    }

    #[test]
    fn multi_level_registry_path() {
        assert_eq!(
            extract_name("myregistry.example.com:5000/team/project/agent:v3"),
            "agent"
        );
    }
}

/// Why harness agents must not be seeded, if they must not: sessions could not run anyway.
///
/// A harness sandbox's credentials are signed with `AGENT_JWT_SECRET`, so without it every turn
/// fails; and a server whose sandbox provider is not available (a backend that is not built in or
/// not reachable) cannot start one. Seeding the agents would only advertise harnesses that cannot
/// work. The reason is `(level_is_error, message)`.
fn harness_seeding_blocked(
    jwt_secret_set: bool,
    provider_available: bool,
    provider_kind: &str,
) -> Option<(bool, String)> {
    if !jwt_secret_set {
        return Some((
            true,
            "harness sessions disabled: AGENT_JWT_SECRET unset".into(),
        ));
    }
    if !provider_available {
        return Some((
            false,
            format!("harness sessions disabled: sandbox provider {provider_kind} unavailable"),
        ));
    }
    None
}

/// Boot-time harness seeding: [`seed_harness_agents`], unless coding sessions are switched off
/// (`HARNESS_SESSIONS_ENABLED`, then [`hide_harness_agents`]) or [`harness_seeding_blocked`].
/// When blocked, logs the reason once (error for a missing secret, warn for an unavailable
/// provider) with the harness agents already seeded, which stay listed but cannot run.
pub async fn seed_harness_agents_if_enabled(state: &AppState) {
    if !state.config.harness_sessions_enabled {
        hide_harness_agents(&state.db).await;
        return;
    }
    let session_key = nasiko_runtime::SandboxKey::Session {
        session_id: uuid::Uuid::nil(),
    };
    let blocked = harness_seeding_blocked(
        !state.config.agent_jwt_secret.is_empty(),
        state.sandbox.is_available_for(&session_key),
        &state.sandbox.kind().to_string(),
    );
    if let Some((is_error, reason)) = blocked {
        let advertised: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT name FROM agents WHERE {} AND deleted_at IS NULL",
            crate::sessions::PLATFORM_HARNESS_AGENT
        ))
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
        if is_error {
            tracing::error!(
                ?advertised,
                "{reason} (agents already seeded stay listed but cannot run)"
            );
        } else {
            tracing::warn!(
                ?advertised,
                "{reason} (agents already seeded stay listed but cannot run)"
            );
        }
        return;
    }
    seed_harness_agents(state).await;
}

/// With coding sessions switched off, soft-delete the platform harness agents an earlier boot
/// created, so no agent list, chat picker or registry offers them and no turn reaches them.
/// [`seed_harness_agents`] restores them when the switch is turned back on. A user's own
/// harness agent is left alone: none is created while sessions are refused.
pub async fn hide_harness_agents(db: &sqlx::PgPool) {
    let hidden = sqlx::query(&format!(
        "UPDATE agents SET deleted_at = now() WHERE {} AND deleted_at IS NULL",
        crate::sessions::PLATFORM_HARNESS_AGENT
    ))
    .execute(db)
    .await;
    match hidden {
        Ok(done) => info!(
            hidden = done.rows_affected(),
            "coding sessions are off (HARNESS_SESSIONS_ENABLED); no harness agents are offered"
        ),
        Err(e) => {
            warn!(error = %e, "coding sessions are off, but the harness agents could not be hidden")
        }
    }
}

/// Bring back the platform harness agents [`hide_harness_agents`] hid, keeping their ids so the
/// sessions opened on them still resolve: per harness, the most recently hidden row of `owner_id`,
/// unless that owner already has a live one.
async fn restore_hidden_harness_agents(
    db: &sqlx::PgPool,
    owner_id: Uuid,
    harness_ids: &[String],
) -> sqlx::Result<u64> {
    sqlx::query(&format!(
        "UPDATE agents SET deleted_at = NULL \
          WHERE id IN (SELECT DISTINCT ON (name) id FROM agents \
                        WHERE {platform} AND owner_id = $1 AND name = ANY($2) \
                          AND deleted_at IS NOT NULL \
                        ORDER BY name, deleted_at DESC) \
            AND NOT EXISTS (SELECT 1 FROM agents live \
                             WHERE live.owner_id = $1 AND live.name = agents.name \
                               AND live.deleted_at IS NULL)",
        platform = crate::sessions::PLATFORM_HARNESS_AGENT
    ))
    .bind(owner_id)
    .bind(harness_ids)
    .execute(db)
    .await
    .map(|done| done.rows_affected())
}

/// Ensure every enabled harness has the `agents` row sessions attach to, restoring a row
/// [`hide_harness_agents`] hid before creating one.
///
/// A harness agent is a routable identity, not a deployment: `status = 'running'` with no
/// container, because each session brings up its own sandbox. Idempotent. The upsert only
/// ever updates a row that is already a harness, so an unrelated agent that happens to share
/// the name is left alone and reported instead of being repurposed. It re-asserts `is_public`
/// every time: a platform row is the catalog entry everyone starts sessions from, and the only
/// public kind of harness agent (`nasiko_auth::sharing`).
pub async fn seed_harness_agents(state: &AppState) {
    let owner_id: Uuid = match sqlx::query_scalar(
        "SELECT id FROM users WHERE is_superuser = true AND deleted_at IS NULL ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await
    {
        Ok(Some(id)) => id,
        _ => {
            warn!("no admin user found, cannot seed harness agents (run bootstrap first)");
            return;
        }
    };
    let harnesses = match crate::sessions::HarnessCatalog::new(state.db.clone())
        .list_enabled()
        .await
    {
        Ok(h) => h,
        Err(e) => {
            warn!(error = %e, "harness catalog unreadable, skipping harness agent seeding");
            return;
        }
    };
    let ids: Vec<String> = harnesses.iter().map(|h| h.id.clone()).collect();
    match restore_hidden_harness_agents(&state.db, owner_id, &ids).await {
        Ok(0) => {}
        Ok(n) => info!(
            restored = n,
            "restored the harness agents hidden while coding sessions were off"
        ),
        Err(e) => warn!(error = %e, "could not restore hidden harness agents; creating new ones"),
    }
    for harness in harnesses {
        match upsert_harness_agent(&state.db, owner_id, &harness).await {
            Ok(1) => info!(harness = %harness.id, "harness agent ready"),
            Ok(_) => warn!(
                harness = %harness.id,
                "an agent named '{}' already exists and is not a harness; not seeding it", harness.id
            ),
            Err(e) => warn!(harness = %harness.id, error = %e, "failed to seed harness agent"),
        }
    }
}

async fn upsert_harness_agent(
    db: &sqlx::PgPool,
    owner_id: Uuid,
    harness: &crate::sessions::Harness,
) -> sqlx::Result<u64> {
    sqlx::query(
        r#"INSERT INTO agents (name, display_name, owner_id, image, status, is_public, is_internal,
                               workload_kind, harness_id)
           VALUES ($1, $2, $3, $4, 'running', true, false, 'harness', $1)
           ON CONFLICT (owner_id, name) WHERE deleted_at IS NULL DO UPDATE
               SET display_name = EXCLUDED.display_name, image = EXCLUDED.image,
                   is_public = true, updated_at = now()
               WHERE agents.workload_kind = 'harness'"#,
    )
    .bind(&harness.id)
    .bind(&harness.name)
    .bind(owner_id)
    .bind(&harness.image)
    .execute(db)
    .await
    .map(|done| done.rows_affected())
}

#[cfg(test)]
mod harness_seeding_tests {
    use super::harness_seeding_blocked;

    #[test]
    fn seeding_needs_a_secret_and_a_live_provider() {
        assert_eq!(harness_seeding_blocked(true, true, "docker"), None);
        let (error, msg) = harness_seeding_blocked(false, true, "docker").unwrap();
        assert!(error && msg.contains("AGENT_JWT_SECRET unset"), "{msg}");
        let (error, msg) = harness_seeding_blocked(true, false, "pod").unwrap();
        assert!(!error, "an unavailable provider is a warning");
        assert_eq!(
            msg,
            "harness sessions disabled: sandbox provider pod unavailable"
        );
        // The secret is reported first when both are missing.
        assert!(harness_seeding_blocked(false, false, "pod").unwrap().0);
    }
}
