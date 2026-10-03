use sqlx::PgPool;
use uuid::Uuid;

use crate::error::RouterError;
use crate::types::AgentCard;

pub async fn get_agents_for_user(
    user_id: Uuid,
    pool: &PgPool,
) -> Result<Vec<AgentCard>, RouterError> {
    let rows = sqlx::query_as::<_, AgentRow>(
        r#"SELECT a.id, a.name, a.description, a.skills, a.tags, a.url,
                  a.embedding, a.embedding_content_hash
           FROM agents a
           LEFT JOIN agent_grants g
               ON g.agent_id = a.id
               AND g.grant_type::text <> 'agent_admin'
           WHERE a.status = 'running'
             AND a.deleted_at IS NULL
             AND NOT a.is_internal
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
            embedding: r
                .embedding
                .map(|v| v.into_iter().map(|f| f as f32).collect()),
            embedding_content_hash: r.embedding_content_hash,
        })
        .collect())
}

#[derive(sqlx::FromRow)]
struct AgentRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    skills: sqlx::types::Json<serde_json::Value>,
    tags: Vec<String>,
    url: Option<String>,
    embedding: Option<Vec<f64>>,
    embedding_content_hash: Option<i64>,
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
    //! Regression coverage for the P0 fix above: a grant type that confers no agent-invoke
    //! access (`agent_admin`, an EE-only L1A-management grant) must not make its agent show up
    //! in `get_agents_for_user`'s result, since that result directly feeds `RoutingEngine::route`
    //! and MAF's auto-assign step — neither of which re-checks access on the routed result.
    //!
    //! Shares the real dev DB (this crate has no per-test throwaway-database harness, same as
    //! `tests/engine.rs`'s DB-backed test); every fixture uses a fresh UUID and is deleted at the
    //! end, so it's safe to run alongside other suites.
    //!
    //! Requires infra: DATABASE_URL=postgres://nasiko:nasiko@localhost:5432/nasiko_dev
    //!   cargo test -p nasiko-orchestrator --lib agent_registry
    //!
    //! Skips (does not fail) when `DATABASE_URL` is unset — same convention as
    //! `oss/server/src/catalog/import.rs::find_owned_agent_never_matches_a_different_owner` —
    //! so `cargo test --workspace --lib --bins` (the CI "workspace unit tests" step, which runs
    //! with `DATABASE_URL` deliberately unset to keep hermetic tests hermetic) doesn't hard-fail
    //! trying to connect with a guessed fallback URL/credentials.

    use super::get_agents_for_user;
    use sqlx::PgPool;
    use uuid::Uuid;

    async fn pool() -> Option<PgPool> {
        let Ok(db_url) = std::env::var("DATABASE_URL") else {
            eprintln!("skipping: DATABASE_URL not set");
            return None;
        };
        Some(PgPool::connect(&db_url).await.expect("connect to test DB"))
    }

    async fn create_user(pool: &PgPool, suffix: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(format!("agent_registry_test_{suffix}"))
            .bind(format!("agent_registry_test_{suffix}@example.test"))
            .execute(pool)
            .await
            .expect("create user");
        id
    }

    /// A private (`is_public = false`), running, non-internal agent — the shape that only shows
    /// up in `get_agents_for_user`'s result via ownership or an explicit grant.
    async fn create_private_agent(pool: &PgPool, owner_id: Uuid, name: &str) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO agents (id, name, owner_id, status, is_internal, is_public) \
             VALUES ($1, $2, $3, 'running', false, false)",
        )
        .bind(id)
        .bind(name)
        .bind(owner_id)
        .execute(pool)
        .await
        .expect("create agent");
        id
    }

    async fn grant(pool: &PgPool, agent_id: Uuid, grant_type: &str, grantee_id: Uuid) {
        sqlx::query(
            "INSERT INTO agent_grants (agent_id, grant_type, grantee_id) \
             VALUES ($1, $2::grant_type, $3)",
        )
        .bind(agent_id)
        .bind(grant_type)
        .bind(grantee_id.to_string())
        .execute(pool)
        .await
        .expect("insert grant");
    }

    async fn cleanup(pool: &PgPool, agent_ids: &[Uuid], user_ids: &[Uuid]) {
        // agent_grants cascades on agent delete.
        sqlx::query("DELETE FROM agents WHERE id = ANY($1)")
            .bind(agent_ids)
            .execute(pool)
            .await
            .expect("cleanup agents");
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(user_ids)
            .execute(pool)
            .await
            .expect("cleanup users");
    }

    #[tokio::test]
    async fn agent_admin_grant_does_not_confer_routing_visibility() {
        let Some(pool) = pool().await else {
            return;
        };
        let suffix = Uuid::new_v4().simple().to_string();

        let owner = create_user(&pool, &format!("owner_{suffix}")).await;
        let grantee = create_user(&pool, &format!("grantee_{suffix}")).await;
        let agent = create_private_agent(&pool, owner, &format!("agent-admin-only-{suffix}")).await;

        grant(&pool, agent, "agent_admin", grantee).await;

        let visible = get_agents_for_user(grantee, &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            !visible.iter().any(|a| a.id == agent),
            "an agent_admin-only grantee must not see the agent through routing — that grant is \
             for L1A knowledge management, not agent access"
        );

        cleanup(&pool, &[agent], &[owner, grantee]).await;
    }

    #[tokio::test]
    async fn ordinary_user_grant_still_confers_routing_visibility() {
        let Some(pool) = pool().await else {
            return;
        };
        let suffix = Uuid::new_v4().simple().to_string();

        let owner = create_user(&pool, &format!("owner2_{suffix}")).await;
        let grantee = create_user(&pool, &format!("grantee2_{suffix}")).await;
        let agent = create_private_agent(&pool, owner, &format!("user-granted-{suffix}")).await;

        grant(&pool, agent, "user", grantee).await;

        let visible = get_agents_for_user(grantee, &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            visible.iter().any(|a| a.id == agent),
            "an ordinary 'user' grant must still confer routing visibility — the agent_admin fix \
             must not overcorrect into denying real grants"
        );

        cleanup(&pool, &[agent], &[owner, grantee]).await;
    }

    #[tokio::test]
    async fn both_grants_present_still_confers_visibility_via_the_real_one() {
        // A grantee holding BOTH an agent_admin grant and an ordinary access grant on the same
        // agent must still see it — the fix filters the agent_admin row out of the join, not the
        // whole grantee/agent pairing.
        let Some(pool) = pool().await else {
            return;
        };
        let suffix = Uuid::new_v4().simple().to_string();

        let owner = create_user(&pool, &format!("owner3_{suffix}")).await;
        let grantee = create_user(&pool, &format!("grantee3_{suffix}")).await;
        let agent = create_private_agent(&pool, owner, &format!("dual-grant-{suffix}")).await;

        grant(&pool, agent, "agent_admin", grantee).await;
        grant(&pool, agent, "user", grantee).await;

        let visible = get_agents_for_user(grantee, &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            visible.iter().any(|a| a.id == agent),
            "the real 'user' grant must still confer visibility even alongside an agent_admin grant"
        );

        cleanup(&pool, &[agent], &[owner, grantee]).await;
    }

    /// A soft-deleted agent must never be a routing candidate again, regardless of how the
    /// caller would otherwise be authorized (ownership, here) — `agents.deleted_at` is set by
    /// `catalog/routes.rs::delete()`, which never touches `status`, so without this filter a
    /// deleted agent stays permanently "running" and routable forever.
    #[tokio::test]
    async fn soft_deleted_agent_is_never_a_routing_candidate() {
        let Some(pool) = pool().await else {
            return;
        };
        let suffix = Uuid::new_v4().simple().to_string();

        let owner = create_user(&pool, &format!("owner4_{suffix}")).await;
        let agent = create_private_agent(&pool, owner, &format!("soft-deleted-{suffix}")).await;

        let visible_before = get_agents_for_user(owner, &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            visible_before.iter().any(|a| a.id == agent),
            "sanity check: the owner must see their own live agent before it's deleted"
        );

        sqlx::query("UPDATE agents SET deleted_at = now() WHERE id = $1")
            .bind(agent)
            .execute(&pool)
            .await
            .expect("soft-delete fixture agent");

        let visible_after = get_agents_for_user(owner, &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            !visible_after.iter().any(|a| a.id == agent),
            "a soft-deleted agent must not remain a routing candidate for its owner"
        );

        cleanup(&pool, &[agent], &[owner]).await;
    }
}
