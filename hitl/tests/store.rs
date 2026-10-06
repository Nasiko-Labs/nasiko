//! Postgres-backed `HitlStore`/`authorize_hitl_action` integration tests.
//! Requires `DATABASE_URL` — run via `just infra` then
//! `cargo test -p nasiko-hitl --test store -- --ignored --test-threads=1` (serial: each test
//! truncates `hitl_requests` on entry, same `--test-threads=1` convention the repo's own
//! `test-server-oss`/`test-one` recipes use).

use nasiko_hitl::{HitlIdentity, HitlKind, HitlStatus, HitlStore, NewHitlRequest, PgHitlStore};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> PgPool {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPool::connect(&db_url).await.expect("connect");
    // Self-contained: this crate's tests don't depend on `nasiko-server`'s test harness having
    // already run migrations against the same DB. Idempotent — a no-op once applied.
    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .expect("run migrations");
    // These tests run serially (`--test-threads=1`) against a shared dev/CI Postgres that isn't
    // necessarily reset between invocations; a leftover row from an earlier run (e.g. an
    // unclaimed `resolved` row from a previous `stale_lease_is_reclaimable` run) would otherwise
    // outrank a fresh test's own fixture in `claim_for_resume`'s `ORDER BY resolved_at` and make
    // the test flaky. Both tables belong entirely to this crate (`mcp_session_tool_grants`
    // foreign-keys onto `hitl_requests`, so a bare `TRUNCATE hitl_requests` fails without it),
    // so truncating both here is safe.
    sqlx::query("TRUNCATE mcp_session_tool_grants, hitl_requests")
        .execute(&pool)
        .await
        .expect("truncate hitl_requests/mcp_session_tool_grants before the suite runs");
    pool
}

/// Every test gets its own user/agent fixture rows (unique random `username`/`email`/`name`), so
/// tests never collide with each other even though the DB isn't reset between them.
async fn fixture_user(pool: &PgPool) -> Uuid {
    let tag = Uuid::new_v4();
    sqlx::query_scalar("INSERT INTO users (username, email) VALUES ($1, $2) RETURNING id")
        .bind(format!("hitl-test-{tag}"))
        .bind(format!("hitl-test-{tag}@example.test"))
        .fetch_one(pool)
        .await
        .expect("insert fixture user")
}

async fn fixture_agent(pool: &PgPool, owner_id: Uuid) -> Uuid {
    let tag = Uuid::new_v4();
    sqlx::query_scalar("INSERT INTO agents (name, owner_id) VALUES ($1, $2) RETURNING id")
        .bind(format!("hitl-test-agent-{tag}"))
        .bind(owner_id)
        .fetch_one(pool)
        .await
        .expect("insert fixture agent")
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn duplicate_pending_create_on_same_task_is_idempotent() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;
    let task_id = format!("task-{}", Uuid::new_v4());
    let ctx = format!("ctx-{}", Uuid::new_v4());

    let first = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            task_id.clone(),
            ctx.clone(),
            json!({"message": "first"}),
        ))
        .await
        .expect("first create");

    let second = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            task_id,
            ctx,
            json!({"message": "second — should be ignored"}),
        ))
        .await
        .expect("second create is idempotent, not an error");

    assert_eq!(
        first.id, second.id,
        "second create must return the SAME pending row"
    );
    assert_eq!(
        second.question,
        json!({"message": "first"}),
        "the original question must survive"
    );
}

/// Security regression: `task_id` comes from the
/// CALLED AGENT's own A2A response, not a Nasiko-minted id, so it must never be trusted alone as
/// a database-wide uniqueness key — two different owners' pauses that happen to share an
/// agent-chosen `task_id` string must get their own rows, never collide onto one.
#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn duplicate_pending_create_with_same_task_id_different_owner_does_not_collide() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let victim = fixture_user(&pool).await;
    let attacker = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, victim).await;
    // Same task_id string, as an agent with predictable/non-random task ids might produce for
    // two unrelated callers.
    let shared_task_id = format!("task-{}", Uuid::new_v4());

    let victims_row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            victim,
            shared_task_id.clone(),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "victim's real question"}),
        ))
        .await
        .expect("victim's create");

    let attackers_row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            attacker,
            shared_task_id,
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "attacker's own question"}),
        ))
        .await
        .expect("attacker's create must succeed as its OWN row, not collide with the victim's");

    assert_ne!(
        victims_row.id, attackers_row.id,
        "two different owners must never be collapsed onto the same pending row just because \
         an agent-supplied task_id happens to match"
    );
    assert_eq!(attackers_row.owner_user_id, attacker);
    assert_eq!(victims_row.owner_user_id, victim);
    assert_eq!(
        victims_row.question,
        json!({"message": "victim's real question"}),
        "the victim's row must be unaffected by the attacker's later create"
    );
}

/// Same protection, scoped by agent instead of owner: one user pausing against two different
/// agents that coincidentally emit the same `task_id` must also get two independent rows.
#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn duplicate_pending_create_with_same_task_id_different_agent_does_not_collide() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent_a = fixture_agent(&pool, owner).await;
    let agent_b = fixture_agent(&pool, owner).await;
    let shared_task_id = format!("task-{}", Uuid::new_v4());

    let first = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent_a,
            owner,
            shared_task_id.clone(),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "from agent a"}),
        ))
        .await
        .expect("agent a's create");

    let second = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent_b,
            owner,
            shared_task_id,
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "from agent b"}),
        ))
        .await
        .expect("agent b's create must succeed as its own row");

    assert_ne!(first.id, second.id);
    assert_eq!(first.agent_id, agent_a);
    assert_eq!(second.agent_id, agent_b);
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn duplicate_pending_tool_approval_is_idempotent_and_distinct_tools_never_collide() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;
    let connector_id = Uuid::new_v4();
    let ctx = format!("ctx-{}", Uuid::new_v4());

    let first = store
        .create(NewHitlRequest::mcp_tool(
            agent,
            owner,
            ctx.clone(),
            connector_id,
            "github_create_issue",
            None,
            json!({"tool_name": "github_create_issue"}),
        ))
        .await
        .expect("first create");

    let duplicate = store
        .create(NewHitlRequest::mcp_tool(
            agent,
            owner,
            ctx.clone(),
            connector_id,
            "github_create_issue",
            None,
            json!({"tool_name": "github_create_issue", "arguments": {"different": true}}),
        ))
        .await
        .expect("duplicate create is idempotent");
    assert_eq!(
        first.id, duplicate.id,
        "same tool+conversation must collapse to one row"
    );

    // Regression test for the v4 index fix (§4): a DIFFERENT tool on the same
    // agent/connector/conversation must get its OWN row, never collide on the first tool's.
    let different_tool = store
        .create(NewHitlRequest::mcp_tool(
            agent,
            owner,
            ctx,
            connector_id,
            "github_close_issue",
            None,
            json!({"tool_name": "github_close_issue"}),
        ))
        .await
        .expect("a different tool must be its own row, not collide");
    assert_ne!(first.id, different_tool.id);
}

/// Security regression (`uq_hitl_pending_per_tool_call`, `0029_hitl_mcp_pending_owner_scope.sql`):
/// `context_id` for an `mcp_tool` row falls back to the raw, agent-controlled trace id whenever no
/// `session_traces` mapping exists, so two different users' calls can collide on the exact same
/// `(agent, connector, tool, context)` tuple without either of them doing anything wrong. Before
/// the index (and `find_existing_pending`'s matching lookup) included `owner_user_id`, the second
/// user's `create()` would silently `DO UPDATE` and return the FIRST user's pending row — this
/// user's own tool call would never get recorded at all, and they'd be handed someone else's
/// `hitl_request_id`.
#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn duplicate_pending_tool_approval_with_the_same_context_id_different_owner_does_not_collide()
{
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let agent = fixture_agent(&pool, fixture_user(&pool).await).await;
    let owner_a = fixture_user(&pool).await;
    let owner_b = fixture_user(&pool).await;
    let connector_id = Uuid::new_v4();
    // Same raw context_id for both users — exactly what an agent-controlled trace id colliding
    // (forced or coincidental) would look like from the store's point of view.
    let shared_ctx = format!("ctx-{}", Uuid::new_v4());

    let row_a = store
        .create(NewHitlRequest::mcp_tool(
            agent,
            owner_a,
            shared_ctx.clone(),
            connector_id,
            "github_create_issue",
            None,
            json!({"tool_name": "github_create_issue"}),
        ))
        .await
        .expect("owner A's create");

    let row_b = store
        .create(NewHitlRequest::mcp_tool(
            agent,
            owner_b,
            shared_ctx,
            connector_id,
            "github_create_issue",
            None,
            json!({"tool_name": "github_create_issue"}),
        ))
        .await
        .expect("owner B's create must succeed as its own row, not error out finding owner A's");

    assert_ne!(
        row_a.id, row_b.id,
        "two different owners' calls must never collapse onto the same pending row just because \
         they share an (agent, connector, tool, context) tuple"
    );
    assert_eq!(
        row_b.owner_user_id, owner_b,
        "owner B's row must be owned by owner B"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn resolve_once_then_twice_is_idempotent_not_an_error() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "?"}),
        ))
        .await
        .unwrap();

    let first = store
        .resolve(
            row.id,
            json!({"answer": "yes"}),
            owner,
            HitlStatus::Resolved,
        )
        .await
        .expect("first resolve");
    assert!(matches!(first, nasiko_hitl::ResolveOutcome::Applied(_)));

    let second = store
        .resolve(
            row.id,
            json!({"answer": "a different answer"}),
            owner,
            HitlStatus::Resolved,
        )
        .await
        .expect("second resolve must be a 200-shaped idempotent no-op, not an error");
    match second {
        nasiko_hitl::ResolveOutcome::AlreadyDecided(r) => {
            assert_eq!(
                r.human_response,
                Some(json!({"answer": "yes"})),
                "the FIRST answer must win"
            );
        }
        nasiko_hitl::ResolveOutcome::Applied(_) => panic!("second resolve must not re-apply"),
    }
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn concurrent_claim_for_resume_only_one_winner() {
    let pool = pool().await;
    let store = std::sync::Arc::new(PgHitlStore::new(pool.clone()));
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "?"}),
        ))
        .await
        .unwrap();
    store
        .resolve(row.id, json!({"answer": "go"}), owner, HitlStatus::Resolved)
        .await
        .unwrap();

    let (a, b) = tokio::join!(
        {
            let s = store.clone();
            async move { s.claim_for_resume(120).await.unwrap() }
        },
        {
            let s = store.clone();
            async move { s.claim_for_resume(120).await.unwrap() }
        },
    );
    let winners = [a, b].into_iter().flatten().count();
    assert_eq!(
        winners, 1,
        "exactly one concurrent claim must win the lease"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn stale_lease_is_reclaimable() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "?"}),
        ))
        .await
        .unwrap();
    store
        .resolve(row.id, json!({"answer": "go"}), owner, HitlStatus::Resolved)
        .await
        .unwrap();

    // Simulate a claim whose owning process died: lease held, well in the past.
    sqlx::query(
        "UPDATE hitl_requests SET resume_claimed_at = now() - interval '10 minutes' WHERE id = $1",
    )
    .bind(row.id)
    .execute(&pool)
    .await
    .unwrap();

    let reclaimed = store
        .claim_for_resume(120) // 2-minute lease — the 10-minute-old claim above is well past it
        .await
        .unwrap();
    assert_eq!(reclaimed.map(|r| r.id), Some(row.id));
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn list_pending_for_never_returns_another_users_row() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let other_user = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    // One conversational row and one tool_approval row, both owned by `owner`.
    store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "?"}),
        ))
        .await
        .unwrap();
    store
        .create(NewHitlRequest::mcp_tool(
            agent,
            owner,
            format!("ctx-{}", Uuid::new_v4()),
            Uuid::new_v4(),
            "some_tool",
            None,
            json!({"tool_name": "some_tool"}),
        ))
        .await
        .unwrap();

    let other_identity = HitlIdentity {
        user_id: other_user,
        is_superuser: false,
    };
    let visible_to_other = store.list_pending_for(&other_identity).await.unwrap();
    assert!(
        visible_to_other.iter().all(|r| r.owner_user_id != owner),
        "a non-superuser must never see another user's pending row, of any kind"
    );

    let owner_identity = HitlIdentity {
        user_id: owner,
        is_superuser: false,
    };
    let visible_to_owner = store.list_pending_for(&owner_identity).await.unwrap();
    assert!(visible_to_owner.iter().all(|r| r.owner_user_id == owner));
    assert!(visible_to_owner.len() >= 2);
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn create_sets_a_future_expiry() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "?"}),
        ))
        .await
        .unwrap();

    let expires_at = row.expires_at.expect("create must set expires_at");
    let days_out = (expires_at - chrono::Utc::now()).num_hours();
    assert!(
        (6 * 24..=8 * 24).contains(&days_out),
        "expected roughly a 7-day default, got {days_out}h out"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn with_ttl_days_overrides_the_default() {
    let pool = pool().await;
    let store = PgHitlStore::with_ttl_days(pool.clone(), 1);
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "?"}),
        ))
        .await
        .unwrap();

    let expires_at = row.expires_at.expect("create must set expires_at");
    let hours_out = (expires_at - chrono::Utc::now()).num_hours();
    assert!(
        (12..=36).contains(&hours_out),
        "expected roughly a 1-day TTL from with_ttl_days(1), got {hours_out}h out"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn expire_stale_only_flips_pending_rows_past_their_expiry() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let stale = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "stale"}),
        ))
        .await
        .unwrap();
    // Backdate directly — `create` always sets a 7-day-out expiry, so this is the only way to
    // get a row past it without waiting a week.
    sqlx::query("UPDATE hitl_requests SET expires_at = now() - interval '1 hour' WHERE id = $1")
        .bind(stale.id)
        .execute(&pool)
        .await
        .unwrap();

    let fresh = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "fresh"}),
        ))
        .await
        .unwrap();

    let already_resolved = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "resolved, also backdated"}),
        ))
        .await
        .unwrap();
    sqlx::query(
        "UPDATE hitl_requests SET status = 'resolved', expires_at = now() - interval '1 hour' WHERE id = $1",
    )
    .bind(already_resolved.id)
    .execute(&pool)
    .await
    .unwrap();

    let swept = store.expire_stale().await.unwrap();
    assert_eq!(swept, 1, "only the stale PENDING row should be swept");

    let stale_after = store.get(stale.id).await.unwrap().unwrap();
    assert_eq!(stale_after.status, HitlStatus::Expired);

    let fresh_after = store.get(fresh.id).await.unwrap().unwrap();
    assert_eq!(
        fresh_after.status,
        HitlStatus::Pending,
        "a row not yet past its expiry must be left alone"
    );

    let resolved_after = store.get(already_resolved.id).await.unwrap().unwrap();
    assert_eq!(
        resolved_after.status,
        HitlStatus::Resolved,
        "expiry only ever applies to still-pending rows"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn cancel_once_then_twice_is_idempotent() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "?"}),
        ))
        .await
        .unwrap();

    let first = store.cancel(row.id, owner).await.expect("first cancel");
    assert!(
        matches!(first, nasiko_hitl::ResolveOutcome::Applied(r) if r.status == HitlStatus::Canceled)
    );

    let second = store
        .cancel(row.id, owner)
        .await
        .expect("second cancel must be a no-op, not an error");
    assert!(matches!(
        second,
        nasiko_hitl::ResolveOutcome::AlreadyDecided(r) if r.status == HitlStatus::Canceled
    ));
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn cancel_does_not_apply_to_an_already_resolved_row() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "?"}),
        ))
        .await
        .unwrap();
    store
        .resolve(row.id, json!({"answer": "ok"}), owner, HitlStatus::Resolved)
        .await
        .unwrap();

    let outcome = store.cancel(row.id, owner).await.unwrap();
    assert!(
        matches!(outcome, nasiko_hitl::ResolveOutcome::AlreadyDecided(r) if r.status == HitlStatus::Resolved),
        "cancelling an already-resolved row must leave its resolved status alone"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn mark_resume_unknown_sets_the_terminal_state_and_releases_the_lease() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "?"}),
        ))
        .await
        .unwrap();
    store
        .resolve(row.id, json!({"answer": "ok"}), owner, HitlStatus::Resolved)
        .await
        .unwrap();
    // Simulate a claim in progress, same shape `claim_for_resume` itself would leave behind.
    sqlx::query(
        "UPDATE hitl_requests SET resume_claimed_at = now(), resume_dispatch_attempts = 5 WHERE id = $1",
    )
    .bind(row.id)
    .execute(&pool)
    .await
    .unwrap();

    store.mark_resume_unknown(row.id).await.unwrap();

    let after = store.get(row.id).await.unwrap().unwrap();
    assert_eq!(
        after.resume_status,
        nasiko_hitl::ResumeStatus::DeliveryOutcomeUnknown
    );
    assert!(
        after.resume_claimed_at.is_none(),
        "the lease must be released, not left dangling on a terminal row"
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn record_auth_start_annotates_without_resolving() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::AuthRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "authorize with GitHub", "auth_url": "https://example.test/oauth"}),
        ))
        .await
        .unwrap();

    let started = store
        .record_auth_start(row.id)
        .await
        .expect("record_auth_start should succeed")
        .expect("a pending auth_required row must match");
    assert_eq!(
        started.status,
        HitlStatus::Pending,
        "start must not resolve the row"
    );
    assert_eq!(
        started.human_response.unwrap()["auth_outcome"],
        json!("started")
    );

    // Idempotent: calling it again before confirm just re-writes the same marker.
    let started_again = store
        .record_auth_start(row.id)
        .await
        .unwrap()
        .expect("still pending, still matches");
    assert_eq!(started_again.status, HitlStatus::Pending);

    // A subsequent confirm still resolves normally — "start" never consumed the pending state.
    let outcome = store
        .resolve(
            row.id,
            json!({"auth_outcome": "confirmed"}),
            owner,
            HitlStatus::Resolved,
        )
        .await
        .unwrap();
    let resolved = match outcome {
        nasiko_hitl::ResolveOutcome::Applied(row) => row,
        nasiko_hitl::ResolveOutcome::AlreadyDecided(_) => {
            panic!("confirm must be the row's first real resolution")
        }
    };
    assert_eq!(resolved.status, HitlStatus::Resolved);
    assert_eq!(
        resolved.human_response.unwrap()["auth_outcome"],
        json!("confirmed")
    );
}

#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn record_auth_start_is_a_noop_once_already_resolved() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::AuthRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({"message": "authorize"}),
        ))
        .await
        .unwrap();
    store
        .resolve(
            row.id,
            json!({"auth_outcome": "confirmed"}),
            owner,
            HitlStatus::Resolved,
        )
        .await
        .unwrap();

    let result = store.record_auth_start(row.id).await.unwrap();
    assert!(
        result.is_none(),
        "a late 'start' after the row already resolved must not resurrect or mutate it"
    );
    let after = store.get(row.id).await.unwrap().unwrap();
    assert_eq!(
        after.human_response.unwrap()["auth_outcome"],
        json!("confirmed"),
        "the earlier confirm's human_response must be untouched"
    );
}

/// A `direct_chat` row that only mirrors a still-pending `mcp_tool` row (the dual-origin
/// scenario: an agent maps MCP's `ask_required` onto the A2A `AUTH_REQUIRED` task state) must
/// never appear in `list_pending_for` — resolving it directly triggers a real but premature
/// resume without granting the actual MCP permission, so it must not be offered as its own
/// actionable item. The linked `mcp_tool` row is the one that does real work and must stay
/// visible.
#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn list_pending_hides_a_direct_chat_mirror_of_a_still_pending_mcp_tool_row() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let mcp_row = store
        .create(NewHitlRequest::mcp_tool(
            agent,
            owner,
            format!("ctx-{}", Uuid::new_v4()),
            Uuid::new_v4(),
            "some_tool",
            None,
            json!({"tool_name": "some_tool"}),
        ))
        .await
        .unwrap();
    let mirror_row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::AuthRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({
                "message": "Tool(s) require user approval for this agent.",
                "metadata": {"hitl_request_id": mcp_row.id.to_string()},
            }),
        ))
        .await
        .unwrap();

    let identity = HitlIdentity {
        user_id: owner,
        is_superuser: false,
    };
    let visible = store.list_pending_for(&identity).await.unwrap();
    assert!(
        visible.iter().any(|r| r.id == mcp_row.id),
        "the real mcp_tool row must still be listed"
    );
    assert!(
        visible.iter().all(|r| r.id != mirror_row.id),
        "the direct_chat mirror must not be listed while its linked mcp_tool row is pending"
    );

    // Once the real row stops being pending (resolved here directly, bypassing
    // `auto_resolve_linked_direct_chat_row`, to isolate the list filter itself), the mirror
    // is no longer hidden — it's a genuinely orphaned row a human must be able to see and act
    // on, e.g. if the linkage resolve step ever failed.
    store
        .resolve(mcp_row.id, json!({}), owner, HitlStatus::Resolved)
        .await
        .unwrap();
    let visible_after = store.list_pending_for(&identity).await.unwrap();
    assert!(
        visible_after.iter().any(|r| r.id == mirror_row.id),
        "an orphaned mirror (linked row no longer pending) must become visible again"
    );
}

/// A malformed or unrelated `hitl_request_id` in a `direct_chat` row's metadata (any agent may
/// put arbitrary metadata there) must not break the `::uuid` cast the filter relies on — the
/// row simply isn't treated as a mirror, and the listing must not error.
#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn list_pending_is_unaffected_by_a_malformed_hitl_request_id() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;

    let row = store
        .create(NewHitlRequest::direct_chat(
            HitlKind::AuthRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            format!("ctx-{}", Uuid::new_v4()),
            json!({
                "message": "some other agent's pause, unrelated to MCP",
                "metadata": {"hitl_request_id": "not-a-uuid"},
            }),
        ))
        .await
        .unwrap();

    let identity = HitlIdentity {
        user_id: owner,
        is_superuser: false,
    };
    let visible = store.list_pending_for(&identity).await.unwrap();
    assert!(
        visible.iter().any(|r| r.id == row.id),
        "a row with a malformed hitl_request_id must still list normally, not error out"
    );
}

// ─── list_for_chat_session ──────────────────────────────────────────────────────────────────

/// Regression test: `AgentProxy`/`DirectChat`-origin rows never set `chat_session_id` at all —
/// `context_id` already IS their stable, caller-facing session id (no separate orchestrator-level
/// session to distinguish it from). Querying by that same id (what `chat/routes.rs`'s
/// session-load HITL discovery passes as `chat_session_id`) previously matched nothing for these
/// origins — confirmed live: a real direct-chat session with two resolved `input_required` rows
/// came back empty. The web UI's own session-history endpoint has always claimed to include
/// "every HITL request tied to this session, pending or already resolved" — this is what makes
/// that true for the two origins that actually produce most real HITL traffic.
#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn list_for_chat_session_finds_agent_proxy_rows_by_their_context_id() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;
    let session_id = format!("ses_{}", Uuid::new_v4().simple());

    let pending = store
        .create(NewHitlRequest::agent_proxy(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            &session_id,
            json!({"message": "which repo?"}),
        ))
        .await
        .expect("create agent_proxy row");
    store
        .resolve(
            pending.id,
            json!({"answer": "nasiko-bishnu/test"}),
            owner,
            HitlStatus::Resolved,
        )
        .await
        .expect("resolve");

    let rows = store
        .list_for_chat_session(&session_id, owner)
        .await
        .expect("list_for_chat_session");
    assert!(
        rows.iter().any(|r| r.id == pending.id),
        "a resolved AgentProxy-origin row must be found by its context_id: {rows:?}"
    );
}

/// Same coverage for `Orchestrator`-origin, which uses `chat_session_id` (not `context_id`) as
/// its stable id — the pre-existing, already-working half of this query, kept passing after
/// widening the `WHERE` clause to also match `context_id`.
#[tokio::test]
#[ignore = "requires PostgreSQL (DATABASE_URL)"]
async fn list_for_chat_session_still_finds_orchestrator_rows_by_chat_session_id() {
    let pool = pool().await;
    let store = PgHitlStore::new(pool.clone());
    let owner = fixture_user(&pool).await;
    let agent = fixture_agent(&pool, owner).await;
    let chat_session_id = format!("ses_{}", Uuid::new_v4().simple());
    let sub_agent_context_id = format!("sub-ctx-{}", Uuid::new_v4());
    // `chat_session_id` FK-references a real `chat_sessions` row (`ensure_orchestrator_chat_session`
    // creates one in production before any HITL row can name it).
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title) \
         VALUES ($1, $2, $3, '/api/orchestrator/a2a', 'test session')",
    )
    .bind(&chat_session_id)
    .bind(owner)
    .bind(agent)
    .execute(&pool)
    .await
    .expect("seed chat_sessions row");

    let row = store
        .create(NewHitlRequest::orchestrator(
            HitlKind::InputRequired,
            agent,
            owner,
            format!("task-{}", Uuid::new_v4()),
            &sub_agent_context_id,
            &chat_session_id,
            json!({"message": "which repo?"}),
        ))
        .await
        .expect("create orchestrator row");

    let rows = store
        .list_for_chat_session(&chat_session_id, owner)
        .await
        .expect("list_for_chat_session");
    assert!(
        rows.iter().any(|r| r.id == row.id),
        "an Orchestrator-origin row must still be found by its chat_session_id: {rows:?}"
    );

    // The sub-agent's own unstable per-dispatch context must never be treated as if it were a
    // real, independent session — matching it here would risk cross-session leakage the day two
    // different sessions' sub-dispatches happen to share a context value.
    let rows_by_subcontext = store
        .list_for_chat_session(&sub_agent_context_id, owner)
        .await
        .expect("list_for_chat_session");
    assert!(
        rows_by_subcontext.is_empty(),
        "the sub-agent's own per-dispatch context_id must not double as a session lookup key: {rows_by_subcontext:?}"
    );
}
