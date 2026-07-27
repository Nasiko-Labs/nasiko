//! Build orchestration for uploaded MCP servers — the direct analog of
//! `crate::agents::upload::execute_upload_and_deploy`, but for MCP connectors
//! instead of agents. Lives in `oss/server` (not `oss/mcp-gateway`) because it
//! needs `ContainerRuntime`, which the gateway crate deliberately never
//! depends on (see `oss/mcp-gateway/src/lib.rs`'s crate-boundary doc).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rand::RngCore;
use sqlx::PgPool;
use uuid::Uuid;

use nasiko_mcp_gateway::McpError;
use nasiko_mcp_gateway::provider::GenericMcpProvider;
use nasiko_mcp_gateway::repo::SourceKind;
use nasiko_mcp_gateway::types::{MCPServerConfig, ServerType};
use nasiko_mcp_gateway::validation::validate_mcp_server_zip;
use nasiko_runtime::{ContainerId, ContainerRuntime, DeploymentSpec};

use crate::agents::upload::{BuildJobPayload, McpBuildSourcePayload};
use crate::secrets::crypto::SecretsCrypto;
use crate::state::AppState;

/// Where an uploaded MCP server's source came from.
pub enum BuildSource {
    Zip(PathBuf),
    Github { url: String },
}

/// Streams-to-disk zip already landed at `zip_path` (by the handler, via
/// `multipart_util::stream_field_to_fresh_temp_file`) — inserts the
/// connector+build+job rows in one transaction and wakes the build worker.
/// Mirrors `agents::upload::upload_and_deploy`'s transaction shape exactly
/// (SRV-5: all three writes commit or fail together, so a job-insert failure
/// never leaves an orphaned pending connector with nothing to move it out of
/// that state).
pub async fn queue_zip_upload(
    state: &AppState,
    owner_id: Uuid,
    name: String,
    version_tag: String,
    zip_path: PathBuf,
    env: HashMap<String, String>,
) -> Result<(Uuid, Uuid), McpError> {
    let source_key = zip_path.to_string_lossy().into_owned();
    queue_upload(
        state,
        owner_id,
        name,
        version_tag,
        None,
        Some(source_key.clone()),
        McpBuildSourcePayload::Zip { zip_path: source_key },
        env,
    )
    .await
}

/// Same as [`queue_zip_upload`] but for a GitHub source — no local file to
/// stream, `github_url` is stored on the build row instead of `source_key`.
pub async fn queue_github_upload(
    state: &AppState,
    owner_id: Uuid,
    name: String,
    version_tag: String,
    github_url: String,
    env: HashMap<String, String>,
) -> Result<(Uuid, Uuid), McpError> {
    queue_upload(
        state,
        owner_id,
        name,
        version_tag,
        Some(github_url.clone()),
        None,
        McpBuildSourcePayload::Github { url: github_url },
        env,
    )
    .await
}

/// Shared transaction body for both queue functions above.
#[allow(clippy::too_many_arguments)]
async fn queue_upload(
    state: &AppState,
    owner_id: Uuid,
    name: String,
    version_tag: String,
    github_url: Option<String>,
    source_key: Option<String>,
    source: McpBuildSourcePayload,
    env: HashMap<String, String>,
) -> Result<(Uuid, Uuid), McpError> {
    let image_tag = crate::agents::build_image_tag(
        &state.config.agent_image_registry, &format!("mcp-{name}"), &version_tag,
    );

    let mut tx = state.db.begin().await?;

    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connectors (provider_type, owner_id, name, source_kind, build_status, is_active) \
         VALUES ('mcp_server', $1, $2, $3, 'pending', false) RETURNING id",
    )
    .bind(owner_id)
    .bind(&name)
    .bind(SourceKind::UploadedBuild)
    .fetch_one(&mut *tx)
    .await?;

    let build_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mcp_connector_builds (connector_id, owner_id, version_tag, github_url, source_key) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(connector_id)
    .bind(owner_id)
    .bind(&version_tag)
    .bind(&github_url)
    .bind(&source_key)
    .fetch_one(&mut *tx)
    .await?;

    // Encrypted at rest in build_jobs.payload — decrypted only inside the
    // worker immediately before use (Step 9's decrypt_build_secrets), never
    // persisted in plaintext. Owner-scoped, same key agent LLM secrets never
    // use (that's the platform's own key, injected separately) — this is the
    // user's own build-time secret.
    let crypto = SecretsCrypto::for_user(owner_id);
    let encrypted_env: HashMap<String, String> =
        env.into_iter().map(|(k, v)| (k, crypto.encrypt(&v))).collect();

    let payload = BuildJobPayload::McpServerUpload {
        build_id,
        connector_id,
        owner_id,
        name,
        source,
        image_tag,
        env: encrypted_env,
    };
    let payload_value = serde_json::to_value(&payload).map_err(|e| McpError::Internal(e.to_string()))?;

    sqlx::query("INSERT INTO build_jobs (connector_id, owner_id, payload) VALUES ($1, $2, $3)")
        .bind(connector_id)
        .bind(owner_id)
        .bind(&payload_value)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    let _ = state.build_tx.send(()).await;

    Ok((connector_id, build_id))
}

/// `GET /api/mcp/connectors/{id}/build-status` — plain polling read, no SSE in
/// v1. Ownership-checked the same way `service::connectors::delete` already
/// does (fetch by id, compare `owner_id`, admins bypass).
///
/// Takes `db`/`runtime` directly (not the whole `AppState`) so it's callable
/// from a test harness that only has a real Postgres pool + a real
/// `ContainerRuntime`, without needing a full `McpState`/`AppState`.
pub async fn get_build_status(db: &PgPool, caller: Uuid, is_admin: bool, connector_id: Uuid) -> Result<serde_json::Value, McpError> {
    let connector = nasiko_mcp_gateway::repo::get_connector_by_id(db, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{connector_id}' not found")))?;
    if !is_admin && connector.owner_id != Some(caller) {
        return Err(McpError::Forbidden("this connector does not belong to you".into()));
    }
    let row: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT status, error_msg FROM mcp_connector_builds WHERE connector_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(connector_id)
    .fetch_optional(db)
    .await?
    .unwrap_or((None, None));
    Ok(serde_json::json!({
        "build_status": connector.build_status,
        "error_msg": row.1,
        "image_tag": connector.container_image_tag,
    }))
}

/// Best-effort container teardown for an `uploaded_build` connector being
/// deleted. Called from `service::connectors::delete` *before* the DB row is
/// removed — an already-dead/missing container must never block cleanup of
/// the record, so a `destroy` failure is logged, not propagated (mirrors how
/// the failed-build cleanup path above already treats `destroy` failures as
/// best-effort). No-op for `external_url`/composio connectors, which never
/// had a container to begin with.
pub async fn destroy_uploaded_connector_container(runtime: &Arc<dyn ContainerRuntime>, connector_id: Uuid) {
    if let Err(e) = runtime.destroy(&ContainerId::from_uuid(connector_id)).await {
        tracing::warn!(connector_id = %connector_id, %e, "failed to destroy container while deleting mcp connector");
    }
}

/// `GET /api/mcp/connectors/{id}/build-logs` — same ownership check as
/// `get_build_status`, then the exact same `ContainerRuntime::logs` call the
/// existing agent logs route (`admin/routes.rs::logs`) already exposes.
pub async fn get_build_logs(
    db: &PgPool,
    runtime: &Arc<dyn ContainerRuntime>,
    caller: Uuid,
    is_admin: bool,
    connector_id: Uuid,
    tail: u32,
) -> Result<Vec<String>, McpError> {
    let connector = nasiko_mcp_gateway::repo::get_connector_by_id(db, connector_id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("connector '{connector_id}' not found")))?;
    if !is_admin && connector.owner_id != Some(caller) {
        return Err(McpError::Forbidden("this connector does not belong to you".into()));
    }
    runtime.logs(&ContainerId::from_uuid(connector_id), tail).await.map_err(|e| McpError::Internal(e.to_string()))
}

/// Self-heal (Step 13): re-resolves an `uploaded_build` connector's live
/// container address and updates `mcp_connectors.url` if it drifted since the
/// last time it was recorded (container restart/redeploy/host reboot all
/// change the underlying address). Called on-demand, from
/// `RuntimeEndpointRefresher` — never on every request, only when a live
/// tool call has already failed at the connection level. Returns the fresh
/// URL (with the platform's `/mcp` path convention appended) whether or not
/// it changed from what was stored.
pub async fn refresh_uploaded_connector_endpoint(
    runtime: &Arc<dyn ContainerRuntime>,
    db: &PgPool,
    connector_id: Uuid,
) -> Result<String, McpError> {
    let endpoint =
        runtime.endpoint(&ContainerId::from_uuid(connector_id)).await.map_err(|e| McpError::Internal(e.to_string()))?;
    let fresh_url = format!("{endpoint}/mcp");

    if let Err(e) = sqlx::query("UPDATE mcp_connectors SET url = $2, updated_at = now() WHERE id = $1 AND url IS DISTINCT FROM $2")
        .bind(connector_id)
        .bind(&fresh_url)
        .execute(db)
        .await
    {
        tracing::warn!(connector_id = %connector_id, %e, "failed to persist refreshed connector endpoint");
    }

    Ok(fresh_url)
}

/// `ContainerRuntime`-backed [`EndpointRefresher`](nasiko_mcp_gateway::endpoint_refresh::EndpointRefresher)
/// — wired into `McpState` once at `AppState` construction (`state.rs`),
/// swapping out the gateway crate's `NoopEndpointRefresher` default. Kept in
/// `oss/server` (not the gateway crate) for the same reason every other
/// `ContainerRuntime`-touching MCP operation lives here.
pub struct RuntimeEndpointRefresher {
    runtime: Arc<dyn ContainerRuntime>,
    db: PgPool,
}

impl RuntimeEndpointRefresher {
    pub fn new(runtime: Arc<dyn ContainerRuntime>, db: PgPool) -> Self {
        Self { runtime, db }
    }
}

#[async_trait::async_trait]
impl nasiko_mcp_gateway::endpoint_refresh::EndpointRefresher for RuntimeEndpointRefresher {
    async fn refresh(&self, connector_id: Uuid) -> Option<String> {
        match refresh_uploaded_connector_endpoint(&self.runtime, &self.db, connector_id).await {
            Ok(url) => Some(url),
            Err(e) => {
                tracing::warn!(connector_id = %connector_id, %e, "endpoint refresh failed — container may be genuinely gone");
                None
            }
        }
    }
}

/// Retry count/backoff for the post-deploy readiness check — mirrors
/// `agents::utils::fetch_agent_card_with_retry`'s convention exactly, applied
/// to an MCP `initialize`+`tools/list` handshake instead of an A2A card fetch.
const READINESS_RETRIES: u32 = 10;
const READINESS_BACKOFF: Duration = Duration::from_secs(2);
const READINESS_CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Executes one MCP-server build+deploy job end to end. Mirrors
/// `agents::upload::execute_upload_and_deploy` structurally — read that
/// function first if anything here is ambiguous, this is its sibling.
///
/// `build_secrets_env` must already be **decrypted plaintext** by the time it
/// reaches this function — per Step 9's design, `build_worker.rs` decrypts the
/// job payload's encrypted blob immediately before calling this, mirroring the
/// existing "inject server secrets at execution — not persisted in the
/// payload" precedent. This function never persists it in plaintext anywhere.
#[allow(clippy::too_many_arguments)]
pub async fn execute_mcp_server_build(
    runtime: Arc<dyn ContainerRuntime>,
    db: PgPool,
    http_client: reqwest::Client,
    build_id: Uuid,
    connector_id: Uuid,
    owner_id: Uuid,
    name: String,
    source: BuildSource,
    image_tag: String,
    mut build_secrets_env: HashMap<String, String>,
    mcp_servers_network: String,
    upload_default_port: u16,
    git_clone_allowed_hosts: Vec<String>,
    agent_runtime: String,
    agent_image_registry: String,
    max_replicas: u32,
) {
    let _ = owner_id; // kept for signature symmetry with execute_upload_and_deploy; not needed by any query here
    mark_building(&db, build_id, connector_id).await;

    let tmp_dir = std::env::temp_dir().join(format!("nasiko-mcp-build-{build_id}"));

    // A per-connector secret the uploaded server MAY optionally read and echo
    // back on inbound requests — the platform never requires or verifies it
    // comes back (enforcement is explicitly deferred, see the plan's Deferred
    // section). Generated once here; persisted encrypted alongside the user's
    // own build secrets so it survives for the container's lifetime.
    let mut token_bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut token_bytes);
    let internal_token = hex::encode(token_bytes);
    build_secrets_env.insert("NASIKO_INTERNAL_TOKEN".to_string(), internal_token.clone());
    persist_internal_token(&db, connector_id, &internal_token).await;

    let build_and_deploy: Result<String, String> = async {
        // 2. Acquire source into tmp_dir.
        match &source {
            BuildSource::Zip(zip_path) => {
                let zp = zip_path.clone();
                let td = tmp_dir.clone();
                tokio::task::spawn_blocking(move || nasiko_utils::zip::extract_zip_from_file(&zp, &td))
                    .await
                    .map_err(|e| format!("spawn_blocking extract: {e}"))??;
            }
            BuildSource::Github { url } => {
                crate::build::routes::clone_repo(url, &git_clone_allowed_hosts, &tmp_dir).await?;
            }
        }

        // 3. Validate — pattern-based, fail-open (see validation.rs's doc comment).
        let detected = validate_mcp_server_zip(&tmp_dir).map_err(|e| e.to_string())?;
        set_detected_runtime(&db, build_id, detected.as_str()).await;

        // 4. Tar the directory.
        let tar_bytes = crate::build::tar_directory(&tmp_dir).map_err(|e| format!("tar source: {e}"))?;

        // 5. Build the image.
        runtime.build(&tar_bytes, &image_tag).await.map_err(|e| format!("docker build: {e}"))?;

        // 6-7. Deploy.
        let mut spec = build_mcp_server_spec(
            connector_id,
            image_tag.clone(),
            build_secrets_env.clone(),
            upload_default_port,
            mcp_servers_network.clone(),
            max_replicas,
        );
        // Mint the K8s image-pull Secret exactly as the agent upload path does
        // (upload.rs:596). No-ops under Docker (short-circuits when
        // agent_runtime != "kubernetes").
        crate::agents::attach_pull_credential(
            &db, &agent_runtime, &agent_image_registry, &mut spec, connector_id,
        ).await;
        runtime.deploy(&spec).await.map_err(|e| format!("deploy: {e}"))?;

        // 8. Resolve the internal endpoint. The platform's own path convention
        // for uploaded servers: the Streamable HTTP endpoint must be at `/mcp`
        // (documented to users at upload time).
        let endpoint = runtime
            .endpoint(&ContainerId::from_uuid(connector_id))
            .await
            .map_err(|e| format!("resolve endpoint: {e}"))?;
        Ok(format!("{endpoint}/mcp"))
    }
    .await;

    // Clean up both the extracted dir and the original zip directory (mirrors
    // execute_upload_and_deploy's cleanup — always runs, success or failure).
    let _ = tokio::fs::remove_dir_all(&tmp_dir).await;
    if let BuildSource::Zip(zip_path) = &source
        && let Some(zip_dir) = zip_path.parent()
    {
        let _ = tokio::fs::remove_dir_all(zip_dir).await;
    }

    let mcp_url = match build_and_deploy {
        Ok(url) => url,
        Err(e) => {
            tracing::error!(build_id = %build_id, connector_id = %connector_id, %e, "mcp server build failed");
            fail_mcp_connector(&db, build_id, connector_id, &e).await;
            return;
        }
    };

    // 9. Live readiness check — not fetch_agent_card_with_retry (A2A-specific).
    // trusted: true is exactly the case Step 7 built: this URL was resolved by
    // ContainerRuntime::endpoint() above, never typed by a user.
    let server_cfg = MCPServerConfig {
        connector_id,
        kind: ServerType::Mcp,
        name: name.clone(),
        url: mcp_url.clone(),
        headers: HashMap::new(),
        transport: "streamable_http".to_string(),
        trusted: true,
    };
    // The guarded client is never selected here (trusted is always true for
    // this one-off config), so a plain unconfigured client is fine as its slot.
    let provider = GenericMcpProvider::new(reqwest::Client::new(), http_client);

    let tools = wait_for_readiness(&provider, &server_cfg, READINESS_RETRIES, READINESS_BACKOFF).await;

    if let Some(tools) = tools {
        mark_running(&db, build_id, connector_id, &mcp_url, &image_tag).await;
        sync_tools(&db, connector_id, &tools).await;
        tracing::info!(build_id = %build_id, connector_id = %connector_id, tool_count = tools.len(), "mcp server build succeeded");
    } else {
        // A failed build must never leave an orphaned container running —
        // mirrors the RUN-4 cleanup guarantee already established for agents.
        if let Err(e) = runtime.destroy(&ContainerId::from_uuid(connector_id)).await {
            tracing::warn!(connector_id = %connector_id, %e, "failed to destroy container after readiness check exhausted retries");
        }
        tracing::error!(build_id = %build_id, connector_id = %connector_id, "mcp server readiness check failed after {READINESS_RETRIES} retries");
        fail_mcp_connector(&db, build_id, connector_id, "readiness check failed after exhausting retries").await;
    }
}

/// Retries `tools/list` against `server` up to `retries` times with `backoff`
/// between attempts, returning `true` on the first success. `retries`/`backoff`
/// are parameters (not the module consts directly) so tests can exercise the
/// same logic with a millisecond-scale backoff instead of the real 2s.
async fn wait_for_readiness(
    provider: &GenericMcpProvider,
    server: &MCPServerConfig,
    retries: u32,
    backoff: Duration,
) -> Option<Vec<serde_json::Value>> {
    for _ in 0..retries {
        tokio::time::sleep(backoff).await;
        if let Ok(tools) = provider.list_tools(server, READINESS_CALL_TIMEOUT, None).await {
            return Some(tools);
        }
    }
    None
}

/// Persist the discovered tool list into `mcp_connector_tools` so the detail
/// view can show tool count + names immediately after deploy.
async fn sync_tools(db: &sqlx::PgPool, connector_id: Uuid, tools: &[serde_json::Value]) {
    let parsed: Vec<(String, Option<String>)> = tools
        .iter()
        .filter_map(|t| {
            t.get("name").and_then(|n| n.as_str()).map(|name| {
                (
                    name.to_string(),
                    t.get("description")
                        .and_then(|d| d.as_str())
                        .map(str::to_string),
                )
            })
        })
        .collect();
    if parsed.is_empty() {
        return;
    }
    if let Err(e) = nasiko_mcp_gateway::repo::upsert_connector_tools(db, connector_id, &parsed).await {
        tracing::warn!(connector_id = %connector_id, %e, "failed to sync tools after build");
    }
}

/// Mirrors `agents::mod::build_agent_spec`'s shape, MCP-specific values.
fn build_mcp_server_spec(
    connector_id: Uuid,
    image_tag: String,
    mut env: HashMap<String, String>,
    port: u16,
    network: String,
    max_replicas: u32,
) -> DeploymentSpec {
    // The platform's chosen convention — documented to users: "your server
    // must read $PORT and bind 0.0.0.0:$PORT" (the same convention
    // Smithery.ai uses, confirmed in the industry research).
    env.insert("PORT".to_string(), port.to_string());
    DeploymentSpec {
        container_id: ContainerId::from_uuid(connector_id),
        name: format!("mcp-connector-{connector_id}"),
        image: image_tag,
        ports: vec![port],
        env_vars: env,
        resources: None, // DockerRuntime defaults None to ResourceLimits::default()
        min_replicas: 1,
        max_replicas,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: true,
        network_override: Some(network),
        workload_kind: nasiko_runtime::WorkloadKind::McpConnector,
    }
}

async fn mark_building(db: &PgPool, build_id: Uuid, connector_id: Uuid) {
    if let Err(e) = sqlx::query("UPDATE mcp_connectors SET build_status = 'building' WHERE id = $1")
        .bind(connector_id)
        .execute(db)
        .await
    {
        tracing::error!(connector_id = %connector_id, %e, "failed to mark connector building");
    }
    if let Err(e) = sqlx::query(
        "UPDATE mcp_connector_builds SET status = 'building', updated_at = now() WHERE id = $1",
    )
    .bind(build_id)
    .execute(db)
    .await
    {
        tracing::error!(build_id = %build_id, %e, "failed to mark build building");
    }
}

async fn set_detected_runtime(db: &PgPool, build_id: Uuid, detected_runtime: &str) {
    if let Err(e) = sqlx::query("UPDATE mcp_connector_builds SET detected_runtime = $2 WHERE id = $1")
        .bind(build_id)
        .bind(detected_runtime)
        .execute(db)
        .await
    {
        tracing::error!(build_id = %build_id, %e, "failed to record detected_runtime");
    }
}

async fn persist_internal_token(db: &PgPool, connector_id: Uuid, token: &str) {
    let crypto = SecretsCrypto::for_connector(connector_id);
    let encrypted = crypto.encrypt(token);
    let patch = serde_json::json!({ "NASIKO_INTERNAL_TOKEN": encrypted });
    if let Err(e) = sqlx::query(
        "UPDATE mcp_connectors SET build_secrets_env = COALESCE(build_secrets_env, '{}'::jsonb) || $2::jsonb WHERE id = $1",
    )
    .bind(connector_id)
    .bind(patch)
    .execute(db)
    .await
    {
        tracing::error!(connector_id = %connector_id, %e, "failed to persist NASIKO_INTERNAL_TOKEN");
    }
}

async fn mark_running(db: &PgPool, build_id: Uuid, connector_id: Uuid, url: &str, image_tag: &str) {
    if let Err(e) = sqlx::query(
        "UPDATE mcp_connectors SET build_status = 'success', url = $2, is_active = true, \
         container_image_tag = $3, updated_at = now() WHERE id = $1",
    )
    .bind(connector_id)
    .bind(url)
    .bind(image_tag)
    .execute(db)
    .await
    {
        tracing::error!(connector_id = %connector_id, %e, "failed to mark connector running");
    }
    if let Err(e) = sqlx::query(
        "UPDATE mcp_connector_builds SET status = 'success', image_tag = $2, completed_at = now() WHERE id = $1",
    )
    .bind(build_id)
    .bind(image_tag)
    .execute(db)
    .await
    {
        tracing::error!(build_id = %build_id, %e, "failed to mark build success");
    }
}

/// Marks both the connector and its build row as terminally failed. Public so
/// `build_worker.rs`'s stuck-job/panic-recovery paths (Step 9) can call it for
/// an MCP job the same way `fail_agent_terminal` covers an agent job.
pub async fn fail_mcp_connector_terminal(db: &PgPool, connector_id: Uuid) {
    if let Err(e) = sqlx::query(
        "UPDATE mcp_connectors SET build_status = 'failed' WHERE id = $1 AND build_status IN ('pending', 'building')",
    )
    .bind(connector_id)
    .execute(db)
    .await
    {
        tracing::error!(connector_id = %connector_id, %e, "failed to mark connector terminally failed");
    }
    if let Err(e) = sqlx::query(
        "UPDATE mcp_connector_builds SET status = 'failed', completed_at = now() \
         WHERE connector_id = $1 AND status IN ('pending', 'building')",
    )
    .bind(connector_id)
    .execute(db)
    .await
    {
        tracing::error!(connector_id = %connector_id, %e, "failed to mark build terminally failed");
    }
}

async fn fail_mcp_connector(db: &PgPool, build_id: Uuid, connector_id: Uuid, error_msg: &str) {
    if let Err(e) = sqlx::query("UPDATE mcp_connectors SET build_status = 'failed' WHERE id = $1")
        .bind(connector_id)
        .execute(db)
        .await
    {
        tracing::error!(connector_id = %connector_id, %e, "failed to mark connector failed");
    }
    if let Err(e) = sqlx::query(
        "UPDATE mcp_connector_builds SET status = 'failed', error_msg = $2, completed_at = now() WHERE id = $1",
    )
    .bind(build_id)
    .bind(error_msg)
    .execute(db)
    .await
    {
        tracing::error!(build_id = %build_id, %e, "failed to mark build failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_injects_port_hardens_and_overrides_network() {
        let connector_id = Uuid::new_v4();
        let env = HashMap::from([("STRIPE_KEY".to_string(), "sk_test".to_string())]);
        let spec = build_mcp_server_spec(
            connector_id,
            "my-image:v1".to_string(),
            env,
            8080,
            "nasiko-mcp-servers-net".to_string(),
            3,
        );
        assert_eq!(spec.env_vars.get("PORT").map(String::as_str), Some("8080"));
        assert_eq!(spec.env_vars.get("STRIPE_KEY").map(String::as_str), Some("sk_test"));
        assert_eq!(spec.ports, vec![8080]);
        assert!(spec.harden);
        assert_eq!(spec.network_override.as_deref(), Some("nasiko-mcp-servers-net"));
        assert_eq!(spec.min_replicas, 1);
        assert_eq!(spec.container_id, ContainerId::from_uuid(connector_id));
    }

    #[test]
    fn spec_sets_workload_kind_to_mcp_connector() {
        let spec = build_mcp_server_spec(
            Uuid::new_v4(),
            "img:v1".to_string(),
            HashMap::new(),
            8080,
            "net".to_string(),
            1,
        );
        assert_eq!(
            spec.workload_kind,
            nasiko_runtime::WorkloadKind::McpConnector,
            "MCP connector specs must set workload_kind to McpConnector"
        );
    }

    #[test]
    fn spec_max_replicas_comes_from_parameter() {
        let spec = build_mcp_server_spec(
            Uuid::new_v4(),
            "img:v1".to_string(),
            HashMap::new(),
            8080,
            "net".to_string(),
            5,
        );
        assert_eq!(spec.max_replicas, 5, "max_replicas must match the parameter, not be hardcoded");
    }

    fn test_server_cfg(url: String) -> MCPServerConfig {
        MCPServerConfig {
            connector_id: Uuid::new_v4(),
            kind: ServerType::Mcp,
            name: "readiness-test".to_string(),
            url,
            headers: HashMap::new(),
            transport: "streamable_http".to_string(),
            trusted: true,
        }
    }

    /// mockito's mock-matching has no "exhausted, fall through" semantics —
    /// the most-recently-created matching mock always wins regardless of
    /// `.expect()` hit counts, so it can't sequence "fail twice, then
    /// succeed." A tiny axum handler with a shared call counter can.
    async fn spawn_flaky_backend(fail_times: u32) -> (String, std::sync::Arc<std::sync::atomic::AtomicU32>) {
        use axum::{Json, routing::post};
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let calls_for_handler = calls.clone();
        let app = axum::Router::new().route(
            "/mcp",
            post(move || {
                let calls = calls_for_handler.clone();
                async move {
                    let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if n < fail_times {
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    } else {
                        Json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}})).into_response()
                    }
                }
            }),
        );
        use axum::response::IntoResponse;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://127.0.0.1:{port}/mcp"), calls)
    }

    #[tokio::test]
    async fn wait_for_readiness_succeeds_after_transient_failures() {
        let (url, calls) = spawn_flaky_backend(2).await;
        let provider = GenericMcpProvider::new(reqwest::Client::new(), reqwest::Client::new());
        let server = test_server_cfg(url);

        let ready = wait_for_readiness(&provider, &server, 5, Duration::from_millis(10)).await;

        assert!(ready.is_some(), "must succeed once the backend starts responding");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "must have actually retried: 2 failures then 1 success"
        );
    }

    #[tokio::test]
    async fn wait_for_readiness_gives_up_after_exhausting_retries() {
        let mut srv = mockito::Server::new_async().await;
        let failure = srv.mock("POST", "/mcp").with_status(500).expect(3).create_async().await;

        let provider = GenericMcpProvider::new(reqwest::Client::new(), reqwest::Client::new());
        let server = test_server_cfg(format!("{}/mcp", srv.url()));

        let ready = wait_for_readiness(&provider, &server, 3, Duration::from_millis(10)).await;

        assert!(ready.is_none(), "must give up, not loop forever, once retries are exhausted");
        failure.assert_async().await;
    }
}
