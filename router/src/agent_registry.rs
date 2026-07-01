use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::RouterError;
use crate::types::AgentCard;

struct CachedAgents {
    agents: Vec<AgentCard>,
    built_at: Instant,
}

pub struct AgentRegistry {
    cache: Arc<RwLock<Option<CachedAgents>>>,
    ttl: Duration,
}

impl AgentRegistry {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            cache: Arc::new(RwLock::new(None)),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    pub async fn get_agents_for_user(
        &self,
        user_id: Uuid,
        pool: &PgPool,
    ) -> Result<Vec<AgentCard>, RouterError> {
        {
            let guard = self.cache.read().await;
            if let Some(ref cached) = *guard {
                if cached.built_at.elapsed() < self.ttl {
                    return Ok(cached.agents.clone());
                }
            }
        }

        let agents = self.fetch_from_db(user_id, pool).await?;

        {
            let mut guard = self.cache.write().await;
            *guard = Some(CachedAgents {
                agents: agents.clone(),
                built_at: Instant::now(),
            });
        }

        Ok(agents)
    }

    pub async fn invalidate(&self) {
        let mut guard = self.cache.write().await;
        *guard = None;
    }

    async fn fetch_from_db(
        &self,
        user_id: Uuid,
        pool: &PgPool,
    ) -> Result<Vec<AgentCard>, RouterError> {
        let rows = sqlx::query_as::<_, AgentRow>(
            r#"SELECT a.id, a.name, a.description, a.skills, a.tags, a.url
               FROM agents a
               LEFT JOIN agent_grants g ON g.agent_id = a.id
               WHERE a.status = 'running'
                 AND (a.owner_id = $1 OR a.is_public = true OR g.grantee_id = $1::text)
               GROUP BY a.id"#,
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| AgentCard {
                id: r.id,
                name: r.name,
                description: r.description.unwrap_or_default(),
                skills: extract_skill_names(r.skills.0),
                tags: r.tags,
                url: r.url,
            })
            .collect())
    }
}

#[derive(sqlx::FromRow)]
struct AgentRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    skills: sqlx::types::Json<serde_json::Value>,
    tags: Vec<String>,
    url: Option<String>,
}

fn extract_skill_names(skills_json: serde_json::Value) -> Vec<String> {
    if let Some(arr) = skills_json.as_array() {
        arr.iter()
            .filter_map(|s| s.get("name").and_then(|n| n.as_str()).map(String::from))
            .collect()
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_registry_has_empty_cache() {
        let registry = AgentRegistry::new(3600);
        // Cache starts empty — no way to get a hit without populating it
        let guard = registry.cache.try_read().unwrap();
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn invalidate_clears_cache() {
        let registry = AgentRegistry::new(3600);
        // Manually populate the cache
        {
            let mut guard = registry.cache.write().await;
            *guard = Some(CachedAgents {
                agents: vec![],
                built_at: Instant::now(),
            });
        }
        // Verify it's populated
        {
            let guard = registry.cache.read().await;
            assert!(guard.is_some());
        }
        // Invalidate and verify it's gone
        registry.invalidate().await;
        let guard = registry.cache.read().await;
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn cache_hit_within_ttl() {
        let registry = AgentRegistry::new(3600);
        {
            let mut guard = registry.cache.write().await;
            *guard = Some(CachedAgents {
                agents: vec![],
                built_at: Instant::now(),
            });
        }
        // Within TTL — should be a cache hit (elapsed << 3600s)
        let guard = registry.cache.read().await;
        let cached = guard.as_ref().unwrap();
        assert!(cached.built_at.elapsed() < Duration::from_secs(3600));
    }
}
