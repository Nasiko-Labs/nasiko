use sqlx::PgPool;
use uuid::Uuid;

use crate::error::RouterError;
use crate::types::AgentCard;

/// `org_granted_agent_ids` carries the ids of agents reachable through an org-hierarchy grant
/// (`org_unit`/`organization`), as resolved by `AuthService::org_granted_agent_ids` — always
/// empty on OSS, populated by `EeRoutingEngine` on EE (see `RouteRequest`'s doc comment). It's a
/// separate, externally-resolved list rather than a join in this query because `org_unit`
/// visibility requires the EE-only `org_units`/`org_unit_members` tables (org membership,
/// leadership, subtree expansion): `oss/orchestrator` is synced verbatim to the public repo and
/// must stay runnable — and its SQL free of EE-only table names — on an OSS-only database that
/// never ran the EE org-hierarchy migrations. This mirrors the exact pattern
/// `oss/server/src/catalog/routes.rs::agent_access_predicate`/`listing_scope` already uses to
/// keep agent *listings* in sync with `can_access_agent` across the same OSS/EE split.
///
/// The `agent_grants` join below is intentionally narrowed to `grant_type = 'user'` only — the
/// one grant type this query CAN resolve entirely from `agent_grants` itself, since its
/// `grantee_id` is directly a user id. `org_unit` and `organization` grants also exist in
/// `agent_grants`, but their `grantee_id` holds an org-unit UUID or the `'*'` sentinel
/// respectively — comparing either against `$1::text` (a user id) can never match, so leaving
/// them in this join's allowlist would be a silent no-op dressed up as coverage. Real
/// `org_unit`/`organization` visibility comes entirely from `org_granted_agent_ids` above,
/// which is what actually reuses `can_access_agent`'s logic instead of re-deriving it.
pub async fn get_agents_for_user(
    user_id: Uuid,
    org_granted_agent_ids: &[String],
    pool: &PgPool,
) -> Result<Vec<AgentCard>, RouterError> {
    let rows = sqlx::query_as::<_, AgentRow>(
        r#"SELECT a.id, a.name, a.description, a.skills, a.tags, a.url,
                  a.embedding, a.embedding_content_hash
           FROM agents a
           LEFT JOIN agent_grants g
               ON g.agent_id = a.id
               AND g.grant_type::text = 'user'
           WHERE a.status = 'running'
             AND a.deleted_at IS NULL
             AND NOT a.is_internal
             AND (a.owner_id = $1
                  OR a.is_public = true
                  OR g.grantee_id = $1::text
                  OR a.id::text = ANY($2))
           GROUP BY a.id"#,
    )
    .bind(user_id)
    .bind(org_granted_agent_ids)
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
    //! Regression coverage for the P0 fix above, and its two follow-ups: a grant type that
    //! confers no agent-invoke access must not make its agent show up in `get_agents_for_user`'s
    //! result, since that result directly feeds `RoutingEngine::route` and MAF's auto-assign step
    //! — neither of which re-checks access on the routed result. History: originally fixed as a
    //! denylist excluding only `agent_admin` (an EE-only L1A-management grant); briefly an
    //! allowlist naming `user`/`org_unit`/`organization` directly in the `agent_grants` join
    //! (which was itself never actually reachable for `org_unit`/`organization`, since their
    //! `grantee_id` is never a user id — see the module doc comment above); now the join covers
    //! only `user` (the one type it can resolve unaided), and `org_unit`/`organization`
    //! visibility comes from the externally-resolved `org_granted_agent_ids` list, the same
    //! `AuthService::org_granted_agent_ids` method `catalog/routes.rs`'s listing endpoints already
    //! use to match `can_access_agent` — so `get_agents_for_user` and `can_access_agent` now share
    //! actual code for the org-hierarchy branches, not just parallel SQL that can drift.
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

        let visible = get_agents_for_user(grantee, &[], &pool)
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

        let visible = get_agents_for_user(grantee, &[], &pool)
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

        let visible = get_agents_for_user(grantee, &[], &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            visible.iter().any(|a| a.id == agent),
            "the real 'user' grant must still confer visibility even alongside an agent_admin grant"
        );

        cleanup(&pool, &[agent], &[owner, grantee]).await;
    }

    #[tokio::test]
    async fn grant_type_outside_the_allowlist_does_not_confer_routing_visibility() {
        // Stands in for "the next grant type added after agent_admin": `team` is a real,
        // already-existing enum value (added 2026-era pre-org_unit) that current application code
        // no longer writes anywhere, but the enum still accepts it, so it's the best available
        // proxy in this schema for "a grant type nobody remembered to exclude." Proves the
        // `agent_grants` join is a real allowlist (only `user` passes) and not just an
        // `agent_admin`-shaped denylist with one more exclusion bolted on. Deliberately does NOT
        // use `org_unit`/`organization` here — those never passed this join at all, even under
        // the (now-removed) allowlist that named them, since their `grantee_id` is never a user
        // id; see `org_id_list_confers_routing_visibility` below for how they're actually covered.
        let Some(pool) = pool().await else {
            return;
        };
        let suffix = Uuid::new_v4().simple().to_string();

        let owner = create_user(&pool, &format!("owner5_{suffix}")).await;
        let grantee = create_user(&pool, &format!("grantee5_{suffix}")).await;
        let agent = create_private_agent(&pool, owner, &format!("team-granted-{suffix}")).await;

        grant(&pool, agent, "team", grantee).await;

        let visible = get_agents_for_user(grantee, &[], &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            !visible.iter().any(|a| a.id == agent),
            "a grant type outside the 'user' allowlist must not confer routing visibility \
             through the agent_grants join"
        );

        cleanup(&pool, &[agent], &[owner, grantee]).await;
    }

    #[tokio::test]
    async fn org_id_list_confers_routing_visibility_matching_org_granted_agent_ids() {
        // Simulates what EeRoutingEngine actually does: resolve org_unit/organization visibility
        // externally (AuthService::org_granted_agent_ids, which does the real org_units join —
        // see that method's own tests for org membership/leadership/subtree correctness) and pass
        // the resulting agent ids straight through. This proves get_agents_for_user's half of the
        // contract: given that list, the agent becomes visible, with no agent_grants row at all —
        // matching an org_unit- or organization-only grant, neither of which this query's join
        // can see directly (their grantee_id is never a user id).
        let Some(pool) = pool().await else {
            return;
        };
        let suffix = Uuid::new_v4().simple().to_string();

        let owner = create_user(&pool, &format!("owner7_{suffix}")).await;
        let caller = create_user(&pool, &format!("caller7_{suffix}")).await;
        let agent = create_private_agent(&pool, owner, &format!("org-granted-{suffix}")).await;

        let org_granted_agent_ids = vec![agent.to_string()];
        let visible = get_agents_for_user(caller, &org_granted_agent_ids, &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            visible.iter().any(|a| a.id == agent),
            "an agent id present in org_granted_agent_ids must be routing-visible, with no \
             agent_grants row required — this is how org_unit/organization visibility reaches \
             routing"
        );

        cleanup(&pool, &[agent], &[owner, caller]).await;
    }

    #[tokio::test]
    async fn org_id_list_does_not_leak_visibility_to_unrelated_agents() {
        // The ANY($2) disjunct must scope to the exact ids in the list, not "any agent" once the
        // list is non-empty — a caller whose org_granted_agent_ids names one agent must not
        // thereby see a second, unrelated private agent they have no relationship to at all.
        let Some(pool) = pool().await else {
            return;
        };
        let suffix = Uuid::new_v4().simple().to_string();

        let owner = create_user(&pool, &format!("owner8_{suffix}")).await;
        let caller = create_user(&pool, &format!("caller8_{suffix}")).await;
        let granted_agent =
            create_private_agent(&pool, owner, &format!("org-granted-8-{suffix}")).await;
        let unrelated_agent =
            create_private_agent(&pool, owner, &format!("unrelated-8-{suffix}")).await;

        let org_granted_agent_ids = vec![granted_agent.to_string()];
        let visible = get_agents_for_user(caller, &org_granted_agent_ids, &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            visible.iter().any(|a| a.id == granted_agent),
            "sanity check: the listed agent must be visible"
        );
        assert!(
            !visible.iter().any(|a| a.id == unrelated_agent),
            "an unrelated private agent, not present in org_granted_agent_ids and with no grant \
             or ownership relationship to the caller, must not become visible"
        );

        cleanup(&pool, &[granted_agent, unrelated_agent], &[owner, caller]).await;
    }

    #[tokio::test]
    async fn public_agent_is_visible_regardless_of_grants() {
        // Owner/public visibility comes from the WHERE clause's `a.owner_id = $1 OR
        // a.is_public = true` branches, independent of both the grant LEFT JOIN's ON condition
        // and the org_granted_agent_ids list — this change must not affect a caller with no grant
        // row and an empty org list.
        let Some(pool) = pool().await else {
            return;
        };
        let suffix = Uuid::new_v4().simple().to_string();

        let owner = create_user(&pool, &format!("owner6_{suffix}")).await;
        let caller = create_user(&pool, &format!("caller6_{suffix}")).await;
        let agent_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO agents (id, name, owner_id, status, is_internal, is_public) \
             VALUES ($1, $2, $3, 'running', false, true)",
        )
        .bind(agent_id)
        .bind(format!("public-agent-{suffix}"))
        .bind(owner)
        .execute(&pool)
        .await
        .expect("create public agent");

        let visible = get_agents_for_user(caller, &[], &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            visible.iter().any(|a| a.id == agent_id),
            "a public agent must remain routing-visible to a caller with no grant row at all"
        );

        cleanup(&pool, &[agent_id], &[owner, caller]).await;
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

        let visible_before = get_agents_for_user(owner, &[], &pool)
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

        let visible_after = get_agents_for_user(owner, &[], &pool)
            .await
            .expect("get_agents_for_user must not error");
        assert!(
            !visible_after.iter().any(|a| a.id == agent),
            "a soft-deleted agent must not remain a routing candidate for its owner"
        );

        cleanup(&pool, &[agent], &[owner]).await;
    }
}
