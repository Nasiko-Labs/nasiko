//! Normalized `agent_skills` projection.
//!
//! `agents.skills` (JSONB) is the source of truth (it serializes into the agent
//! card). `agent_skills` is a derived, queryable projection (GIN index on `tags`)
//! kept in sync here so agents can be discovered by skill tag.

use std::collections::HashMap;

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::models::Skill;

/// Replace the `agent_skills` rows for `agent_id` from `skills`.
///
/// Runs on a caller-supplied connection so it can join the caller's transaction
/// (atomic with the `agents` write). Delete-then-insert drops removed skills.
/// Input is deduplicated by `skill_key` (last wins) so the set-based insert can't
/// affect the same row twice; `ON CONFLICT` guards the rare concurrent-write case.
pub async fn sync_agent_skills(
    conn: &mut sqlx::PgConnection,
    agent_id: Uuid,
    skills: &[Skill],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM agent_skills WHERE agent_id = $1")
        .bind(agent_id)
        .execute(&mut *conn)
        .await?;

    if skills.is_empty() {
        return Ok(());
    }

    let mut by_key: HashMap<&str, &Skill> = HashMap::with_capacity(skills.len());
    for s in skills {
        by_key.insert(s.id.as_str(), s);
    }
    let payload = Value::Array(
        by_key
            .values()
            .map(|s| {
                json!({
                    "skill_key": s.id,
                    "name": s.name,
                    "description": s.description,
                    "tags": s.tags,
                    "examples": s.examples,
                })
            })
            .collect(),
    );

    // One round-trip regardless of skill count. jsonb_to_recordset expands the
    // payload server-side, including per-row `tags text[]` and `examples jsonb`.
    sqlx::query(
        r#"INSERT INTO agent_skills (agent_id, skill_key, name, description, tags, examples)
           SELECT $1, x.skill_key, x.name, x.description,
                  COALESCE(x.tags, '{}'::text[]), COALESCE(x.examples, '[]'::jsonb)
           FROM jsonb_to_recordset($2::jsonb) AS x(
               skill_key text, name text, description text, tags text[], examples jsonb
           )
           ON CONFLICT (agent_id, skill_key) DO UPDATE
             SET name = EXCLUDED.name,
                 description = EXCLUDED.description,
                 tags = EXCLUDED.tags,
                 examples = EXCLUDED.examples"#,
    )
    .bind(agent_id)
    .bind(&payload)
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Best-effort sync for background/auto paths that only hold `agents.skills` as
/// JSON (capability generation, seeding, import). Never propagates errors —
/// those call sites are fire-and-forget and `agents.skills` stays authoritative.
pub async fn sync_agent_skills_json(pool: &PgPool, agent_id: Uuid, skills_json: &Value) {
    let skills: Vec<Skill> = match serde_json::from_value(skills_json.clone()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%agent_id, %e, "sync_agent_skills_json: invalid skills json");
            return;
        }
    };
    match pool.acquire().await {
        Ok(mut conn) => {
            if let Err(e) = sync_agent_skills(&mut conn, agent_id, &skills).await {
                tracing::warn!(%agent_id, %e, "sync_agent_skills_json: db error");
            }
        }
        Err(e) => tracing::warn!(%agent_id, %e, "sync_agent_skills_json: acquire connection"),
    }
}
