use std::sync::Arc;

use uuid::Uuid;

use crate::agent_lifecycle::SwappableAgentDeletionHook;
use crate::build::BuildStatus;

/// How long [`answers_http`] waits for one answer.
const ANSWER_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Where an agent at `agent_url` serves its card: the current well-known name, then the older one.
fn agent_card_urls(agent_url: &str) -> [String; 2] {
    let base = agent_url.trim_end_matches('/');
    [
        format!("{base}/.well-known/agent-card.json"),
        format!("{base}/.well-known/agent.json"),
    ]
}

/// Whether the agent at `agent_url` is serving: it answers a card request with any HTTP status.
/// An error status still proves its server is listening, which is all a caller about to forward
/// to it needs to know; a refused or timed-out connection means it is not up yet. HTTP rather than
/// a bare TCP connect, because a port mapped by a container runtime's proxy accepts connections
/// before the agent inside listens.
pub(crate) async fn answers_http(http: &reqwest::Client, agent_url: &str) -> bool {
    let [card_url, _] = agent_card_urls(agent_url);
    http.get(card_url)
        .timeout(ANSWER_PROBE_TIMEOUT)
        .send()
        .await
        .is_ok()
}

/// Fetch the agent's card from its runtime endpoint and persist the fields
/// clients depend on: display_name, description, skills, tags, capabilities,
/// and `transport_path` (see [`nasiko_types::a2a::extract_transport_path`]).
///
/// Returns true if a card was fetched and applied. Every deploy path
/// (seed / upload / update / rollback) must go through this so `nasiko ps`
/// and the UI can surface a chat URL that actually works.
pub(crate) async fn fetch_and_apply_agent_card(
    db: &sqlx::PgPool,
    http: &reqwest::Client,
    backend: Option<&nasiko_orchestrator::llm_backend::SharedLlmBackend>,
    agent_id: Uuid,
    agent_url: &str,
) -> bool {
    if agent_url.is_empty() {
        return false;
    }

    let urls = agent_card_urls(agent_url);

    let mut card: Option<serde_json::Value> = None;
    for url in &urls {
        if let Ok(resp) = http.get(url).send().await
            && resp.status().is_success()
            && let Ok(v) = resp.json::<serde_json::Value>().await
        {
            card = Some(v);
            break;
        }
    }

    let card = match card {
        Some(c) => c,
        None => return false,
    };

    if !apply_agent_card(db, agent_id, &card).await {
        return false;
    }
    embed_updated_agent(db, backend, agent_id).await;

    true
}

/// Persist the card fields clients depend on (display_name, description, skills, tags,
/// capabilities, `transport_path`) and the agent's skill rows. Returns false when the
/// write failed. Shared by the deploy-time fetch above and the registration of agents the
/// CLI discovered, whose cards arrive in the request instead of being fetched.
pub(crate) async fn apply_agent_card(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    card: &serde_json::Value,
) -> bool {
    if let Err(e) = sqlx::query(
        r#"UPDATE agents SET
             display_name = COALESCE($2, display_name),
             description = COALESCE($3, description),
             skills = COALESCE($4, skills),
             tags = COALESCE($5, tags),
             capabilities = COALESCE($6, capabilities),
             transport_path = COALESCE($7, transport_path),
             updated_at = now()
           WHERE id = $1"#,
    )
    .bind(agent_id)
    .bind(card.get("name").and_then(|v| v.as_str()))
    .bind(card.get("description").and_then(|v| v.as_str()))
    .bind(card.get("skills"))
    .bind({
        let mut tags: Vec<String> = card
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(skills) = card.get("skills").and_then(|v| v.as_array()) {
            for skill in skills {
                if let Some(skill_tags) = skill.get("tags").and_then(|v| v.as_array()) {
                    for t in skill_tags.iter().filter_map(|v| v.as_str()) {
                        if !tags.contains(&t.to_string()) {
                            tags.push(t.to_string());
                        }
                    }
                }
            }
        }
        if tags.is_empty() { None } else { Some(tags) }
    })
    .bind(card.get("capabilities"))
    .bind(nasiko_types::a2a::extract_transport_path(card))
    .execute(db)
    .await
    {
        tracing::error!(%agent_id, %e, "failed to apply agent card fields to DB");
        return false;
    }

    if let Some(skills_json) = card.get("skills") {
        crate::catalog::skills::sync_agent_skills_json(db, agent_id, skills_json).await;
    }

    true
}

/// Re-reads the agent's current name/description/tags and embeds+persists
/// them via `nasiko_orchestrator::embed_and_store_agent`, so routing's Stage 1
/// doesn't have to do it lazily on the next `route()` call. Best-effort —
/// failures are logged and never block the deploy/update flow.
///
/// Reads `OPENAI_BASE_URL`/`EMBEDDING_MODEL` straight from the environment
/// rather than threading `Config` through every deploy path (seed / upload /
/// update / rollback) that reaches this function — same env-driven approach
/// `nasiko_config::Config` itself uses for these fields. The key is the
/// owner's saved one, else `OPENAI_API_KEY`.
///
/// `backend` is the caller's `AppState::llm_backend` (the in-process router); it is
/// passed explicitly so each server instance (and each test) uses its own router.
async fn embed_updated_agent(
    db: &sqlx::PgPool,
    backend: Option<&nasiko_orchestrator::llm_backend::SharedLlmBackend>,
    agent_id: Uuid,
) {
    let base_url =
        std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com".into());
    let model =
        std::env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "text-embedding-3-small".into());

    let row: Option<(String, Option<String>, Vec<String>, Uuid)> =
        sqlx::query_as("SELECT name, description, tags, owner_id FROM agents WHERE id = $1")
            .bind(agent_id)
            .fetch_optional(db)
            .await
            .unwrap_or(None);

    let Some((name, description, tags, owner_id)) = row else {
        return;
    };
    let saved_key: Option<String> =
        nasiko_orchestrator::user_key::saved_openai_key(db, owner_id).await;

    let agent = nasiko_orchestrator::AgentCard {
        id: agent_id,
        name,
        description: description.unwrap_or_default(),
        skills: vec![],
        tags,
        url: None,
        embedding: None,
        embedding_content_hash: None,
    };

    let result = match (saved_key, backend) {
        (Some(key), _) => {
            let direct_model: String =
                nasiko_orchestrator::providers::direct_embedding_model(&model, &base_url);
            nasiko_orchestrator::embed_and_store_agent(db, &agent, &key, &base_url, &direct_model)
                .await
        }
        (None, Some(backend)) => {
            let routed = nasiko_orchestrator::vector_store::RoutedEmbedder {
                backend: std::sync::Arc::clone(backend),
                call: nasiko_orchestrator::llm_backend::LlmCallContext {
                    user_id: owner_id,
                    component: nasiko_orchestrator::llm_backend::PlatformComponent::AgentEmbedding
                        .to_string(),
                    session_id: None,
                },
            };
            nasiko_orchestrator::vector_store::embed_and_store_agent_routed(
                db, &agent, &routed, &model,
            )
            .await
        }
        (None, None) => match std::env::var("OPENAI_API_KEY") {
            Ok(key) => {
                nasiko_orchestrator::embed_and_store_agent(db, &agent, &key, &base_url, &model)
                    .await
            }
            Err(_) => return,
        },
    };
    if let Err(e) = result {
        tracing::warn!(%agent_id, error = %e, "proactive agent embedding failed (non-fatal, will be computed lazily on next route())");
    }
}

/// Ensure a successful `runtime.deploy()` is visible to the crash-loop
/// guardian (EE) by guaranteeing an `agent_deployments` row exists for this
/// agent. `agent_deployments.build_id` is a NOT NULL FK to `agent_builds`, so
/// a deploy path with no real build job (`seed.rs`, or a first-time
/// deploy-by-image with no prior `agent_builds` row) has nothing to
/// reference — this synthesizes a minimal `agent_builds` row (status
/// 'success', no real build artifact) rather than skipping the insert, which
/// previously left those agents with zero crash-loop protection and no
/// indication anywhere that this was the case (see
/// docs/CRASH_GUARDIAN_REPORT.md §5.1/§5.3).
pub(crate) async fn ensure_deployment_tracked(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    owner_id: Option<Uuid>,
    image: &str,
) {
    let existing_build_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM agent_builds WHERE agent_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();

    let build_id = match existing_build_id {
        Some(id) => id,
        None => {
            let version_tag = image
                .rsplit('/')
                .next()
                .unwrap_or(image)
                .rsplit_once(':')
                .map(|(_, tag)| tag.to_string())
                .unwrap_or_else(|| "latest".to_string());

            let synthesized: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
                "INSERT INTO agent_builds (agent_id, version_tag, image_reference, status)
                 VALUES ($1, $2, $3, 'success')
                 RETURNING id",
            )
            .bind(agent_id)
            .bind(version_tag)
            .bind(image)
            .fetch_one(db)
            .await;

            match synthesized {
                Ok(id) => id,
                Err(e) => {
                    tracing::error!(%e, %agent_id, "failed to synthesize agent_builds row for deployment tracking");
                    return;
                }
            }
        }
    };

    let _ = sqlx::query(
        "INSERT INTO agent_deployments (agent_id, build_id, owner_id, status, k8s_deployment_name)
         VALUES ($1, $2, $3, 'running', $4)",
    )
    .bind(agent_id)
    .bind(build_id)
    .bind(owner_id)
    .bind(agent_id.to_string())
    .execute(db)
    .await;
}

/// How long a card fetch waits for a freshly deployed agent to become reachable. A bulk
/// deploy queues an agent's pod behind thousands of others on Kubernetes: at 15,000 agents
/// the last pods started over an hour after their deploy was accepted.
const CARD_ROUTABLE_WAIT: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

/// Retry wrapper around [`fetch_and_apply_agent_card`] for freshly deployed
/// containers that need a few seconds to become healthy. With no `agent_url` (an agent
/// with no address until it runs, such as a Kubernetes agent reached at its pod's IP), it
/// first waits for the runtime to report one.
pub(crate) async fn fetch_agent_card_with_retry(
    db: sqlx::PgPool,
    http: reqwest::Client,
    runtime: Arc<dyn nasiko_runtime::ContainerRuntime>,
    backend: nasiko_orchestrator::llm_backend::SharedLlmBackend,
    agent_id: Uuid,
    agent_url: String,
) {
    let agent_url = if agent_url.is_empty() {
        let container_id = nasiko_runtime::ContainerId::from_uuid(agent_id);
        match runtime
            .wait_routable(&container_id, CARD_ROUTABLE_WAIT)
            .await
        {
            Ok(url) => url,
            Err(e) => {
                tracing::warn!(%agent_id, %e, "agent card fetch: the agent never became reachable");
                return;
            }
        }
    } else {
        agent_url
    };
    for attempt in 1..=30u32 {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        if fetch_and_apply_agent_card(&db, &http, Some(&backend), agent_id, &agent_url).await {
            return;
        }
        tracing::debug!(%agent_id, attempt, "agent card fetch attempt failed");
    }
    tracing::warn!(%agent_id, url = %agent_url, "agent card fetch: giving up after retries");
}

/// On failure of a first-time deploy (no prior successful `agent_builds`), delete
/// the agents row so no orphaned `status='failed'` record is left in the catalog.
///
/// For re-uploads of an existing, previously-working agent, the row is kept and
/// set to `status='failed'` so the agent's history and grants are preserved.
///
/// Cascade effects of the DELETE path (all intentional):
/// - `agent_builds`      ON DELETE CASCADE  → failed build records removed
/// - `build_jobs`        ON DELETE CASCADE  → orphaned job rows removed
/// - `agent_deployments` ON DELETE CASCADE  → deployment rows removed
/// - `agent_versions`    ON DELETE CASCADE  → version history removed
/// - `upload_status`     ON DELETE SET NULL → row survives; agent_id becomes NULL
///
/// This is a real `DELETE`, not the soft-delete `oss/server/src/catalog/routes.rs::delete()`
/// uses — so it never goes through that handler's own `agent_deletion_hook` call. It must fire
/// the hook itself here, or enterprise-only, agent-keyed state with no FK to `agents` (e.g. an
/// L1A domain set on this brand-new agent before its first build ever finished) would be left
/// permanently orphaned with nothing left to clean it up.
pub(crate) async fn delete_agent_or_mark_failed(
    db: &sqlx::PgPool,
    agent_id: Uuid,
    deletion_hook: &Arc<SwappableAgentDeletionHook>,
) {
    let has_prior_success: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agent_builds WHERE agent_id = $1 AND status = 'success')",
    )
    .bind(agent_id)
    .fetch_one(db)
    .await
    .unwrap_or(false);

    if has_prior_success {
        let _ =
            sqlx::query("UPDATE agents SET status = 'failed', updated_at = now() WHERE id = $1")
                .bind(agent_id)
                .execute(db)
                .await;
    } else {
        // Deleting the agent nulls `agent_id` on its sessions, which hides them from everything
        // that would later clean up after them. Their sandbox credentials are revoked now; a
        // sandbox itself is released only where a provider is at hand (the user and session
        // delete paths), and an agent that failed its build has none.
        crate::sessions::teardown::revoke_tokens_of_agent(db, agent_id).await;
        let deleted = sqlx::query("DELETE FROM agents WHERE id = $1")
            .bind(agent_id)
            .execute(db)
            .await
            .map(|r| r.rows_affected() > 0)
            .unwrap_or(false);
        // Only fire the hook if the row was actually removed — a transient DB error on the
        // DELETE above must not free enterprise-only, agent-keyed state (e.g. an L1A domain
        // name) for an agent that still exists. Mirrors `catalog/routes.rs::delete()`'s
        // `RETURNING` + existence check for the same reason.
        if deleted {
            deletion_hook.on_agent_deleted(agent_id).await;
            tracing::info!(%agent_id, "deleted new-agent row after build failure (no prior successful builds)");
        }
    }
}

pub(crate) async fn set_build_status(db: &sqlx::PgPool, build_id: Uuid, status: BuildStatus) {
    if let Err(e) =
        sqlx::query("UPDATE agent_builds SET status = $2, updated_at = now() WHERE id = $1")
            .bind(build_id)
            .bind(status)
            .execute(db)
            .await
    {
        tracing::error!(build_id = %build_id, ?status, %e, "failed to update build status");
    }
}

/// Persist a build's captured output on its `agent_builds` row, so the Builds tab and
/// `GET /api/builds/{id}/logs` can show what the build did (success) or why it failed. Best effort:
/// a write failure is logged, never fatal — the build result stands on its own. Empty logs are
/// skipped so a runtime that keeps none leaves the column untouched rather than blanking it.
pub(crate) async fn set_build_logs(db: &sqlx::PgPool, build_id: Uuid, logs: &str) {
    if logs.trim().is_empty() {
        return;
    }
    if let Err(e) =
        sqlx::query("UPDATE agent_builds SET logs = $2, updated_at = now() WHERE id = $1")
            .bind(build_id)
            .bind(logs)
            .execute(db)
            .await
    {
        tracing::error!(build_id = %build_id, %e, "failed to persist build logs");
    }
}

/// The object-storage key a build's logs live under. One object per build attempt,
/// keyed by the build id (so a retry of the same build overwrites, never piles up).
pub(crate) fn build_logs_key(build_id: Uuid) -> String {
    format!("build-logs/{build_id}")
}

/// Persists build logs to object storage and records only the pointer in
/// `agent_builds.logs_url` (`blob:<key>`), keeping the log bytes out of Postgres.
///
/// The same path serves Docker (S3) and Kubernetes (S3/Azure) builds — `storage`
/// is the shared [`BlobStore`](nasiko_runtime::BlobStore) seam the OCI registry
/// already uses. The object is uploaded **first**; the row is pointed at it only
/// after the upload succeeds, so the database never references an object that was
/// not written. If the upload fails, it falls back to the `logs` column, so a
/// storage hiccup never loses the only record of why a build failed.
/// `get_build_logs` reads either shape back.
pub(crate) async fn store_build_logs(
    db: &sqlx::PgPool,
    storage: &dyn nasiko_runtime::BlobStore,
    build_id: Uuid,
    logs: &str,
) {
    if logs.trim().is_empty() {
        return;
    }
    let key = build_logs_key(build_id);
    match storage
        .put_blob(&key, bytes::Bytes::copy_from_slice(logs.as_bytes()))
        .await
    {
        Ok(_) => {
            // Upload succeeded: point the row at the object and clear any text a
            // fallback left behind, so the reader never serves a stale DB copy.
            if let Err(e) = sqlx::query(
                "UPDATE agent_builds SET logs_url = $2, logs = NULL, updated_at = now() WHERE id = $1",
            )
            .bind(build_id)
            .bind(format!("blob:{key}"))
            .execute(db)
            .await
            {
                tracing::error!(build_id = %build_id, %e, "failed to record build log pointer");
            }
        }
        Err(e) => {
            tracing::warn!(
                build_id = %build_id,
                %e,
                "build log upload to object storage failed; falling back to the logs column"
            );
            set_build_logs(db, build_id, logs).await;
        }
    }
}

pub(crate) async fn set_upload_status(
    db: &sqlx::PgPool,
    upload_id: &str,
    agent_name: &str,
    owner_id: Uuid,
    status: &str,
    agent_id: Option<Uuid>,
    error: Option<&str>,
) {
    if let Err(e) = sqlx::query(
        "INSERT INTO upload_status (upload_id, agent_name, owner_id, status, agent_id, error_message)
         VALUES ($1, $2, $3, $4::upload_pipeline_status, $5, $6)
         ON CONFLICT (upload_id) DO UPDATE
           SET status = EXCLUDED.status,
               agent_id = COALESCE(EXCLUDED.agent_id, upload_status.agent_id),
               error_message = EXCLUDED.error_message",
    )
    .bind(upload_id)
    .bind(agent_name)
    .bind(owner_id)
    .bind(status)
    .bind(agent_id)
    .bind(error)
    .execute(db)
    .await
    {
        tracing::warn!(%e, upload_id, "failed to update upload_status");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_logs_key_is_stable_and_namespaced() {
        let id = Uuid::parse_str("279f0b99-c789-405b-bd4f-a4ae0bc249a8").unwrap();
        // The key must stay `build-logs/<id>`: it is the object-storage path a build's
        // logs live under, and `get_build_logs` resolves `blob:<key>` back to it, so a
        // change here would orphan every already-stored log.
        assert_eq!(
            build_logs_key(id),
            "build-logs/279f0b99-c789-405b-bd4f-a4ae0bc249a8"
        );
        // Namespaced away from image blobs, and one object per build id.
        assert!(build_logs_key(id).starts_with("build-logs/"));
    }
}
