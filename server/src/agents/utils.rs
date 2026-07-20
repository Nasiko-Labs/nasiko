use uuid::Uuid;

use crate::build::BuildStatus;

/// Fetch the agent's card from its runtime endpoint and persist the fields
/// clients depend on: description, skills, tags, capabilities, and
/// `transport_path` (see [`nasiko_types::a2a::extract_transport_path`]).
///
/// Returns true if a card was fetched and applied. Every deploy path
/// (seed / upload / update / rollback) must go through this so `nasiko ps`
/// and the UI can surface a chat URL that actually works.
pub(crate) async fn fetch_and_apply_agent_card(
    db: &sqlx::PgPool,
    http: &reqwest::Client,
    agent_id: Uuid,
    agent_url: &str,
) -> bool {
    if agent_url.is_empty() {
        return false;
    }

    let base = agent_url.trim_end_matches('/');

    let urls = [
        format!("{base}/.well-known/agent-card.json"),
        format!("{base}/.well-known/agent.json"),
    ];

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

    let _ = sqlx::query(
        r#"UPDATE agents SET
             description = COALESCE($2, description),
             skills = COALESCE($3, skills),
             tags = COALESCE($4, tags),
             capabilities = COALESCE($5, capabilities),
             transport_path = COALESCE($6, transport_path),
             updated_at = now()
           WHERE id = $1"#,
    )
    .bind(agent_id)
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
    .bind(nasiko_types::a2a::extract_transport_path(&card))
    .execute(db)
    .await;

    if let Some(skills_json) = card.get("skills") {
        crate::catalog::skills::sync_agent_skills_json(db, agent_id, skills_json).await;
    }

    true
}

/// Retry wrapper around [`fetch_and_apply_agent_card`] for freshly deployed
/// containers that need a few seconds to become healthy.
pub(crate) async fn fetch_agent_card_with_retry(
    db: sqlx::PgPool,
    http: reqwest::Client,
    agent_id: Uuid,
    agent_url: String,
) {
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        if fetch_and_apply_agent_card(&db, &http, agent_id, &agent_url).await {
            return;
        }
    }
    tracing::warn!(%agent_id, url = %agent_url, "agent card fetch: giving up after retries");
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

pub(super) async fn set_upload_status(
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
