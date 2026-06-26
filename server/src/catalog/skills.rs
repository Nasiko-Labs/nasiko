use std::collections::HashSet;

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::models::Skill;

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

    // Deduplicate by skill id (first occurrence wins, deterministic). Postgres
    // raises an error if the same key appears twice in one INSERT batch.
    let mut seen: HashSet<&str> = HashSet::with_capacity(skills.len());
    let unique: Vec<&Skill> = skills.iter().filter(|s| seen.insert(s.id.as_str())).collect();
    if unique.len() < skills.len() {
        tracing::warn!(%agent_id, dupes = skills.len() - unique.len(), "sync_agent_skills: duplicate skill ids dropped");
    }

    let payload = Value::Array(
        unique
            .into_iter()
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

    // One round-trip regardless of skill count.
    // No ON CONFLICT needed — the DELETE above removed all rows for this agent.
    sqlx::query(
        r#"INSERT INTO agent_skills (agent_id, skill_key, name, description, tags, examples)
           SELECT $1, x.skill_key, x.name, x.description,
                  COALESCE(x.tags, '{}'::text[]), COALESCE(x.examples, '[]'::jsonb)
           FROM jsonb_to_recordset($2::jsonb) AS x(
               skill_key text, name text, description text, tags text[], examples jsonb
           )"#,
    )
    .bind(agent_id)
    .bind(&payload)
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Best-effort sync for background/auto paths. Wraps DELETE+INSERT in its own
/// transaction so a crash between the two statements cannot leave an empty
/// projection. Never propagates errors — `agents.skills` stays authoritative.
pub async fn sync_agent_skills_json(pool: &PgPool, agent_id: Uuid, skills_json: &Value) {
    let skills: Vec<Skill> = match serde_json::from_value(skills_json.clone()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(%agent_id, %e, "sync_agent_skills_json: invalid skills json");
            return;
        }
    };
    match pool.begin().await {
        Ok(mut tx) => {
            if let Err(e) = sync_agent_skills(&mut tx, agent_id, &skills).await {
                tracing::warn!(%agent_id, %e, "sync_agent_skills_json: db error");
                return;
            }
            if let Err(e) = tx.commit().await {
                tracing::warn!(%agent_id, %e, "sync_agent_skills_json: commit error");
            }
        }
        Err(e) => tracing::warn!(%agent_id, %e, "sync_agent_skills_json: begin tx"),
    }
}
