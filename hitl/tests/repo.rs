//! Integration tests for `nasiko_hitl::repo` against a real Postgres — needs
//! infra up (`just infra` from the repo root; override the admin connection
//! with `TEST_PG_URL` for CI), same convention `oss/server/tests` uses. Each
//! test creates and drops its own scratch database so tests can run
//! concurrently without colliding.

use nasiko_hitl::HitlStatus;
use nasiko_hitl::repo::{self, NewAuthRequired, NewSessionGrant, NewToolApproval, ResolveDecision};
use uuid::Uuid;

mod common;
use common::TestDb;

impl TestDb {
    fn new_auth_required(&self, connector_id: Uuid, context_id: &str) -> NewAuthRequired {
        NewAuthRequired {
            agent_id: self.agent_id,
            owner_user_id: self.owner_user_id,
            connector_id,
            context_id: context_id.to_string(),
            question: serde_json::json!({"connector": "github", "auth_url": "https://example.com/authorize"}),
        }
    }

    fn new_tool_approval(
        &self,
        connector_id: Uuid,
        tool_name: &str,
        context_id: &str,
    ) -> NewToolApproval {
        NewToolApproval {
            agent_id: self.agent_id,
            owner_user_id: self.owner_user_id,
            connector_id,
            tool_name: tool_name.to_string(),
            context_id: context_id.to_string(),
            question: serde_json::json!({"connector_id": connector_id, "tool_name": tool_name}),
        }
    }

    async fn seed_user(&self) -> Uuid {
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(format!("hitl-test-{}", user_id.simple()))
            .bind(format!("hitl-test-{}@example.com", user_id.simple()))
            .execute(&self.pool)
            .await
            .expect("seed second user");
        user_id
    }
}

#[tokio::test]
async fn create_pending_auth_required_persists_a_pending_row() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();

    let created =
        repo::create_pending_auth_required(&db.pool, db.new_auth_required(connector_id, "ctx-1"))
            .await
            .expect("create pending auth_required row");

    assert_eq!(created.agent_id, db.agent_id);
    assert_eq!(created.owner_user_id, db.owner_user_id);
    assert_eq!(created.connector_id, Some(connector_id));
    assert_eq!(created.context_id.as_deref(), Some("ctx-1"));
    assert_eq!(created.kind, nasiko_hitl::HitlKind::AuthRequired);
    assert_eq!(created.origin, nasiko_hitl::HitlOrigin::McpTool);
    assert_eq!(created.status, nasiko_hitl::HitlStatus::Pending);

    let fetched = repo::get_by_id(&db.pool, created.id)
        .await
        .expect("get_by_id")
        .expect("row exists");
    assert_eq!(fetched.id, created.id);
}

#[tokio::test]
async fn repeated_calls_for_the_same_identity_are_idempotent() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();

    let first =
        repo::create_pending_auth_required(&db.pool, db.new_auth_required(connector_id, "ctx-1"))
            .await
            .expect("first create");
    let second =
        repo::create_pending_auth_required(&db.pool, db.new_auth_required(connector_id, "ctx-1"))
            .await
            .expect("second create for the same identity");

    assert_eq!(
        first.id, second.id,
        "a second call for the same (agent, connector, context) must return the existing row, not a duplicate"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM hitl_requests WHERE agent_id = $1 AND connector_id = $2 AND context_id = $3",
    )
    .bind(db.agent_id)
    .bind(connector_id)
    .bind("ctx-1")
    .fetch_one(&db.pool)
    .await
    .expect("count rows");
    assert_eq!(count, 1, "exactly one row must exist for this identity");
}

#[tokio::test]
async fn different_context_id_creates_a_distinct_pending_row() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();

    let ctx1 =
        repo::create_pending_auth_required(&db.pool, db.new_auth_required(connector_id, "ctx-1"))
            .await
            .expect("create for ctx-1");
    let ctx2 =
        repo::create_pending_auth_required(&db.pool, db.new_auth_required(connector_id, "ctx-2"))
            .await
            .expect("create for ctx-2");

    assert_ne!(
        ctx1.id, ctx2.id,
        "different conversations must not share a pending auth_required row"
    );
}

#[tokio::test]
async fn different_connector_creates_a_distinct_pending_row() {
    let db = TestDb::new("hitl_test").await;
    let context_id = "ctx-shared";

    let a = repo::create_pending_auth_required(
        &db.pool,
        db.new_auth_required(Uuid::new_v4(), context_id),
    )
    .await
    .expect("create for connector a");
    let b = repo::create_pending_auth_required(
        &db.pool,
        db.new_auth_required(Uuid::new_v4(), context_id),
    )
    .await
    .expect("create for connector b");

    assert_ne!(
        a.id, b.id,
        "different connectors in the same conversation must not collide"
    );
}

/// Security regression (`uq_hitl_pending_per_connector_auth`,
/// `0029_hitl_mcp_pending_owner_scope.sql`): `context_id` falls back to the raw, agent-controlled
/// trace id whenever no `session_traces` mapping exists, so two different users' calls can collide
/// on the same `(agent, connector, context)` tuple without either doing anything wrong. Before the
/// index (and this `ON CONFLICT` target) included `owner_user_id`, the second user's create would
/// silently `DO UPDATE` and return the FIRST user's row.
#[tokio::test]
async fn create_pending_auth_required_with_the_same_context_id_different_owner_does_not_collide() {
    let db = TestDb::new("hitl_test").await;
    let other_owner = db.seed_user().await;
    let connector_id = Uuid::new_v4();

    let a = repo::create_pending_auth_required(
        &db.pool,
        db.new_auth_required(connector_id, "ctx-shared-owner-test"),
    )
    .await
    .expect("owner A's create");

    let b = repo::create_pending_auth_required(
        &db.pool,
        NewAuthRequired {
            owner_user_id: other_owner,
            ..db.new_auth_required(connector_id, "ctx-shared-owner-test")
        },
    )
    .await
    .expect("owner B's create must succeed as its own row, not error out finding owner A's");

    assert_ne!(
        a.id, b.id,
        "two different owners must never collapse onto the same pending row just because they \
         share an (agent, connector, context) tuple"
    );
    assert_eq!(b.owner_user_id, other_owner);
}

/// Same regression as `create_pending_auth_required_with_the_same_context_id_different_owner_does_not_collide`,
/// for `uq_hitl_pending_per_tool_call`/`create_pending_tool_approval`.
#[tokio::test]
async fn create_pending_tool_approval_with_the_same_context_id_different_owner_does_not_collide() {
    let db = TestDb::new("hitl_test").await;
    let other_owner = db.seed_user().await;
    let connector_id = Uuid::new_v4();

    let a = repo::create_pending_tool_approval(
        &db.pool,
        db.new_tool_approval(connector_id, "GITHUB_CREATE_ISSUE", "ctx-shared-owner-test"),
    )
    .await
    .expect("owner A's create");

    let b = repo::create_pending_tool_approval(
        &db.pool,
        NewToolApproval {
            owner_user_id: other_owner,
            ..db.new_tool_approval(connector_id, "GITHUB_CREATE_ISSUE", "ctx-shared-owner-test")
        },
    )
    .await
    .expect("owner B's create must succeed as its own row, not error out finding owner A's");

    assert_ne!(
        a.id, b.id,
        "two different owners must never collapse onto the same pending row just because they \
         share an (agent, connector, tool, context) tuple"
    );
    assert_eq!(b.owner_user_id, other_owner);
}

// ─── M5: resolve ────────────────────────────────────────────────────────────
//
// `repo::list_pending_for`/`repo::authorize_hitl_action` (and their tests, formerly here) were
// removed — dead code with no callers outside their own tests. Production code exclusively uses
// `HitlStore::list_pending_for` (`store.rs`, mirror-filtered, superuser-aware, covered by the
// `/api/hitl/pending` integration tests in `oss/server/tests/`) and `authz::authorize_hitl_action`
// (covered by `oss/hitl/src/authz.rs`'s own unit tests).

#[tokio::test]
async fn resolve_approve_transitions_tool_approval_to_resolved_with_audit_fields() {
    let db = TestDb::new("hitl_test").await;
    let created = repo::create_pending_tool_approval(
        &db.pool,
        db.new_tool_approval(Uuid::new_v4(), "GITHUB_DELETE_REPO", "ctx-1"),
    )
    .await
    .expect("create pending tool_approval");

    let resolved = repo::resolve(
        &db.pool,
        created.id,
        ResolveDecision::Approve,
        db.owner_user_id,
        serde_json::json!({"decision": "approve", "scope": "once"}),
    )
    .await
    .expect("resolve")
    .expect("row was pending");

    assert_eq!(resolved.status, HitlStatus::Resolved);
    assert_eq!(resolved.resolved_by, Some(db.owner_user_id));
    assert!(resolved.resolved_at.is_some());
    assert_eq!(
        resolved.human_response,
        Some(serde_json::json!({"decision": "approve", "scope": "once"}))
    );
}

/// `skip_resume_for_mirrored_row` marks an `mcp_tool` row's resume as terminal without ever
/// attempting delivery, for the case where the real resume happens on its linked
/// `direct_chat`/`agent_proxy` mirror instead — see that function's own doc comment. It must
/// record `ResumeStatus::Skipped`, not `Completed`: no delivery attempt is ever made for this
/// row, so `Completed` would misreport a successful delivery that never happened (found in
/// review — previously the only test touching this function asserted on unrelated reply text and
/// never checked `resume_status` at all, so the wrong value went uncaught for a full review
/// round).
#[tokio::test]
async fn skip_resume_for_mirrored_row_marks_the_row_skipped_not_completed() {
    let db = TestDb::new("hitl_test").await;
    let created = repo::create_pending_tool_approval(
        &db.pool,
        db.new_tool_approval(Uuid::new_v4(), "GITHUB_CREATE_ISSUE", "ctx-1"),
    )
    .await
    .expect("create pending tool_approval");

    repo::skip_resume_for_mirrored_row(&db.pool, created.id)
        .await
        .expect("skip_resume_for_mirrored_row");

    let row = repo::get_by_id(&db.pool, created.id)
        .await
        .expect("get_by_id")
        .expect("row still exists");
    assert_eq!(row.resume_status, nasiko_hitl::ResumeStatus::Skipped);
    assert!(row.resume_claimed_at.is_some());

    // Once skipped, `claim_for_resume` (the mcp_tool dispatcher's own claim, scoped to
    // `resume_status = 'not_started'`) must never pick this row up — that's the entire point of
    // marking it terminal instead of leaving it `not_started`.
    repo::resolve(
        &db.pool,
        created.id,
        ResolveDecision::Approve,
        db.owner_user_id,
        serde_json::json!({"decision": "approve"}),
    )
    .await
    .expect("resolve")
    .expect("row was pending");
    let claimed = repo::claim_for_resume(&db.pool)
        .await
        .expect("claim_for_resume");
    assert!(
        claimed.is_none(),
        "a skipped row must never be claimed for real delivery"
    );
}

#[tokio::test]
async fn resolve_reject_transitions_auth_required_to_rejected() {
    let db = TestDb::new("hitl_test").await;
    let created =
        repo::create_pending_auth_required(&db.pool, db.new_auth_required(Uuid::new_v4(), "ctx-1"))
            .await
            .expect("create pending auth_required");

    let resolved = repo::resolve(
        &db.pool,
        created.id,
        ResolveDecision::Reject,
        db.owner_user_id,
        serde_json::json!({"decision": "reject"}),
    )
    .await
    .expect("resolve")
    .expect("row was pending");

    assert_eq!(resolved.status, HitlStatus::Rejected);
}

#[tokio::test]
async fn resolve_a_non_pending_row_returns_none_not_an_error() {
    let db = TestDb::new("hitl_test").await;
    let created = repo::create_pending_tool_approval(
        &db.pool,
        db.new_tool_approval(Uuid::new_v4(), "GITHUB_DELETE_REPO", "ctx-1"),
    )
    .await
    .expect("create pending tool_approval");

    repo::resolve(
        &db.pool,
        created.id,
        ResolveDecision::Approve,
        db.owner_user_id,
        serde_json::json!({"decision": "approve"}),
    )
    .await
    .expect("first resolve")
    .expect("row was pending");

    let second = repo::resolve(
        &db.pool,
        created.id,
        ResolveDecision::Reject,
        db.owner_user_id,
        serde_json::json!({"decision": "reject"}),
    )
    .await
    .expect("second resolve must not error");

    assert!(
        second.is_none(),
        "a second resolve of an already-resolved row must be a no-op, not silently re-apply"
    );
}

#[tokio::test]
async fn resolve_unknown_id_returns_none() {
    let db = TestDb::new("hitl_test").await;
    let result = repo::resolve(
        &db.pool,
        Uuid::new_v4(),
        ResolveDecision::Approve,
        db.owner_user_id,
        serde_json::json!({}),
    )
    .await
    .expect("resolve on an unknown id must not error");
    assert!(result.is_none());
}

#[tokio::test]
async fn concurrent_resolve_attempts_exactly_one_wins() {
    let db = TestDb::new("hitl_test").await;
    let created = repo::create_pending_tool_approval(
        &db.pool,
        db.new_tool_approval(Uuid::new_v4(), "GITHUB_DELETE_REPO", "ctx-1"),
    )
    .await
    .expect("create pending tool_approval");

    let (a, b) = tokio::join!(
        repo::resolve(
            &db.pool,
            created.id,
            ResolveDecision::Approve,
            db.owner_user_id,
            serde_json::json!({"decision": "approve"}),
        ),
        repo::resolve(
            &db.pool,
            created.id,
            ResolveDecision::Reject,
            db.owner_user_id,
            serde_json::json!({"decision": "reject"}),
        ),
    );

    let winners = [
        a.expect("first call must not error"),
        b.expect("second call must not error"),
    ]
    .into_iter()
    .filter(|r| r.is_some())
    .count();
    assert_eq!(
        winners, 1,
        "exactly one of two concurrent resolve attempts on the same row must win"
    );
}

// ─── M7: claim_resolved_tool_approval / session grants ─────────────────────

impl TestDb {
    /// Create + resolve a `tool_approval` row in one step, for tests that
    /// only care about the post-resolve retry-matching behavior.
    async fn resolved_tool_approval(
        &self,
        connector_id: Uuid,
        tool_name: &str,
        context_id: &str,
        decision: ResolveDecision,
        scope: Option<&str>,
    ) -> nasiko_hitl::HitlRequest {
        let created = repo::create_pending_tool_approval(
            &self.pool,
            self.new_tool_approval(connector_id, tool_name, context_id),
        )
        .await
        .expect("create pending tool_approval");

        let decision_label = match decision {
            ResolveDecision::Approve => "approve",
            ResolveDecision::Reject => "reject",
        };
        repo::resolve(
            &self.pool,
            created.id,
            decision,
            self.owner_user_id,
            serde_json::json!({"decision": decision_label, "scope": scope}),
        )
        .await
        .expect("resolve")
        .expect("row was pending")
    }
}

#[tokio::test]
async fn claim_resolved_tool_approval_claims_an_approved_once_row() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();
    db.resolved_tool_approval(
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
        ResolveDecision::Approve,
        Some("once"),
    )
    .await;

    let claimed = repo::claim_resolved_tool_approval(
        &db.pool,
        db.owner_user_id,
        db.agent_id,
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await
    .expect("claim must not error")
    .expect("an approved, unconsumed row must be claimable");

    assert!(claimed.consumed_at.is_some(), "claim must set consumed_at");
    assert_eq!(
        claimed.human_response.unwrap()["decision"],
        serde_json::json!("approve")
    );
}

#[tokio::test]
async fn claim_resolved_tool_approval_is_single_use() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();
    db.resolved_tool_approval(
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
        ResolveDecision::Approve,
        Some("once"),
    )
    .await;

    let first = repo::claim_resolved_tool_approval(
        &db.pool,
        db.owner_user_id,
        db.agent_id,
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await
    .expect("first claim must not error");
    assert!(first.is_some(), "first retry must find the approved row");

    let second = repo::claim_resolved_tool_approval(
        &db.pool,
        db.owner_user_id,
        db.agent_id,
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await
    .expect("second claim must not error");
    assert!(
        second.is_none(),
        "a second retry of the same once-scope approval must find nothing (already consumed) — must re-ask"
    );
}

#[tokio::test]
async fn claim_resolved_tool_approval_claims_a_rejected_row() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();
    db.resolved_tool_approval(
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
        ResolveDecision::Reject,
        None,
    )
    .await;

    let claimed = repo::claim_resolved_tool_approval(
        &db.pool,
        db.owner_user_id,
        db.agent_id,
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await
    .expect("claim must not error")
    .expect("a rejected row must also be claimable, for the deny-on-retry path");

    assert_eq!(
        claimed.human_response.unwrap()["decision"],
        serde_json::json!("reject")
    );
}

#[tokio::test]
async fn claim_resolved_tool_approval_never_claims_a_session_scoped_row() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();
    db.resolved_tool_approval(
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
        ResolveDecision::Approve,
        Some("session"),
    )
    .await;

    let claimed = repo::claim_resolved_tool_approval(
        &db.pool,
        db.owner_user_id,
        db.agent_id,
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await
    .expect("claim must not error");

    assert!(
        claimed.is_none(),
        "a session-scoped approval's reusability lives in mcp_session_tool_grants — \
         claim_resolved_tool_approval must never consume it"
    );
}

#[tokio::test]
async fn claim_resolved_tool_approval_ignores_a_still_pending_row() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();
    repo::create_pending_tool_approval(
        &db.pool,
        db.new_tool_approval(connector_id, "GITHUB_DELETE_REPO", "ctx-1"),
    )
    .await
    .expect("create pending tool_approval");

    let claimed = repo::claim_resolved_tool_approval(
        &db.pool,
        db.owner_user_id,
        db.agent_id,
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
    )
    .await
    .expect("claim must not error");

    assert!(
        claimed.is_none(),
        "a still-pending (not yet resolved) row must never be claimed"
    );
}

#[tokio::test]
async fn concurrent_claims_of_the_same_approval_exactly_one_wins() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();
    db.resolved_tool_approval(
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-1",
        ResolveDecision::Approve,
        Some("once"),
    )
    .await;

    let (a, b) = tokio::join!(
        repo::claim_resolved_tool_approval(
            &db.pool,
            db.owner_user_id,
            db.agent_id,
            connector_id,
            "GITHUB_DELETE_REPO",
            "ctx-1",
        ),
        repo::claim_resolved_tool_approval(
            &db.pool,
            db.owner_user_id,
            db.agent_id,
            connector_id,
            "GITHUB_DELETE_REPO",
            "ctx-1",
        ),
    );

    let winners = [
        a.expect("first claim must not error"),
        b.expect("second claim must not error"),
    ]
    .into_iter()
    .filter(|r| r.is_some())
    .count();
    assert_eq!(
        winners, 1,
        "exactly one of two concurrent retries of the same approved tool call must execute"
    );
}

#[tokio::test]
async fn session_grant_is_visible_to_has_active_session_grant() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = db.seed_connector("session-grant-visible").await;
    let resolved = db
        .resolved_tool_approval(
            connector_id,
            "GITHUB_DELETE_REPO",
            "ctx-1",
            ResolveDecision::Approve,
            Some("session"),
        )
        .await;

    assert!(
        !repo::has_active_session_grant(
            &db.pool,
            db.owner_user_id,
            db.agent_id,
            connector_id,
            "GITHUB_DELETE_REPO",
            "ctx-1",
        )
        .await
        .expect("lookup must not error"),
        "resolving with scope=session does not itself create a grant — \
         the caller (the resolve API) must call create_session_grant separately"
    );

    repo::create_session_grant(
        &db.pool,
        NewSessionGrant {
            agent_id: db.agent_id,
            connector_id,
            tool_name: "GITHUB_DELETE_REPO".to_string(),
            context_id: "ctx-1".to_string(),
            granted_by: db.owner_user_id,
            hitl_request_id: Some(resolved.id),
        },
    )
    .await
    .expect("create_session_grant must not error");

    assert!(
        repo::has_active_session_grant(
            &db.pool,
            db.owner_user_id,
            db.agent_id,
            connector_id,
            "GITHUB_DELETE_REPO",
            "ctx-1",
        )
        .await
        .expect("lookup must not error"),
        "an unexpired grant for the exact tuple must be found"
    );
}

#[tokio::test]
async fn session_grant_does_not_match_a_different_tool_or_conversation() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = db.seed_connector("session-grant-mismatch").await;
    repo::create_session_grant(
        &db.pool,
        NewSessionGrant {
            agent_id: db.agent_id,
            connector_id,
            tool_name: "GITHUB_DELETE_REPO".to_string(),
            context_id: "ctx-1".to_string(),
            granted_by: db.owner_user_id,
            hitl_request_id: None,
        },
    )
    .await
    .expect("create_session_grant must not error");

    assert!(
        !repo::has_active_session_grant(
            &db.pool,
            db.owner_user_id,
            db.agent_id,
            connector_id,
            "GITHUB_CREATE_ISSUE", // different tool
            "ctx-1",
        )
        .await
        .expect("lookup must not error")
    );
    assert!(
        !repo::has_active_session_grant(
            &db.pool,
            db.owner_user_id,
            db.agent_id,
            connector_id,
            "GITHUB_DELETE_REPO",
            "ctx-2", // different conversation
        )
        .await
        .expect("lookup must not error")
    );
}

#[tokio::test]
async fn expired_session_grant_is_not_active() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = db.seed_connector("session-grant-expired").await;

    // Insert an already-expired grant directly — create_session_grant always
    // computes a future expiry, so an expired row can only be exercised by
    // writing it in by hand, exactly like the dispatcher's own lease tests do.
    sqlx::query(
        r#"
        INSERT INTO mcp_session_tool_grants
            (agent_id, connector_id, tool_name, context_id, granted_by, expires_at)
        VALUES ($1, $2, $3, $4, $5, now() - interval '1 hour')
        "#,
    )
    .bind(db.agent_id)
    .bind(connector_id)
    .bind("GITHUB_DELETE_REPO")
    .bind("ctx-1")
    .bind(db.owner_user_id)
    .execute(&db.pool)
    .await
    .expect("seed expired grant");

    assert!(
        !repo::has_active_session_grant(
            &db.pool,
            db.owner_user_id,
            db.agent_id,
            connector_id,
            "GITHUB_DELETE_REPO",
            "ctx-1",
        )
        .await
        .expect("lookup must not error"),
        "an expired grant must not be treated as active"
    );
}

/// Security regression: `context_id` alone is derived from the unauthenticated,
/// agent-controlled `traceparent` header (`session::resolve_context_id`) — it is a
/// trace-correlation id, not an identity boundary. On a shared agent, User A's approval
/// decision must never be claimable by User B just because an agent replayed the same
/// `context_id` while acting on User B's behalf; `owner_user_id` is a required predicate
/// specifically to close that hole.
#[tokio::test]
async fn claim_resolved_tool_approval_never_claims_a_different_users_approval() {
    let db = TestDb::new("hitl_test").await;
    let other_user = db.seed_user().await;
    let connector_id = Uuid::new_v4();

    // User A (db.owner_user_id) approves — same (agent_id, connector_id, tool_name,
    // context_id) tuple an adversarial or buggy shared agent could replay for anyone.
    db.resolved_tool_approval(
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-shared",
        ResolveDecision::Approve,
        Some("once"),
    )
    .await;

    let claimed_as_other_user = repo::claim_resolved_tool_approval(
        &db.pool,
        other_user,
        db.agent_id,
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-shared",
    )
    .await
    .expect("claim must not error");
    assert!(
        claimed_as_other_user.is_none(),
        "a different user must never be able to claim another user's approval decision, \
         even with an identical (agent_id, connector_id, tool_name, context_id) tuple"
    );

    // The real owner can still claim it — the row wasn't consumed by the failed attempt.
    let claimed_as_owner = repo::claim_resolved_tool_approval(
        &db.pool,
        db.owner_user_id,
        db.agent_id,
        connector_id,
        "GITHUB_DELETE_REPO",
        "ctx-shared",
    )
    .await
    .expect("claim must not error");
    assert!(
        claimed_as_owner.is_some(),
        "the real owner's own claim must still succeed"
    );
}

/// Same guarantee as the test above, for the reusable "approve for this session" grant path.
#[tokio::test]
async fn has_active_session_grant_never_matches_a_different_users_grant() {
    let db = TestDb::new("hitl_test").await;
    let other_user = db.seed_user().await;
    let connector_id = db.seed_connector("session-grant-cross-user").await;

    repo::create_session_grant(
        &db.pool,
        NewSessionGrant {
            agent_id: db.agent_id,
            connector_id,
            tool_name: "GITHUB_DELETE_REPO".to_string(),
            context_id: "ctx-shared".to_string(),
            granted_by: db.owner_user_id,
            hitl_request_id: None,
        },
    )
    .await
    .expect("create_session_grant must not error");

    assert!(
        !repo::has_active_session_grant(
            &db.pool,
            other_user,
            db.agent_id,
            connector_id,
            "GITHUB_DELETE_REPO",
            "ctx-shared",
        )
        .await
        .expect("lookup must not error"),
        "a different user must never see another user's session grant as active, \
         even for an identical (agent_id, connector_id, tool_name, context_id) tuple"
    );
    assert!(
        repo::has_active_session_grant(
            &db.pool,
            db.owner_user_id,
            db.agent_id,
            connector_id,
            "GITHUB_DELETE_REPO",
            "ctx-shared",
        )
        .await
        .expect("lookup must not error"),
        "the real grantee's own lookup must still succeed"
    );
}

/// A real broken-connector-credential pause (not a permission gate) can also be mirrored by a
/// `direct_chat`/`agent_proxy` row, the same way a `tool_approval` pause is — an agent that maps
/// MCP's connector-level `auth_required` onto its own A2A `AUTH_REQUIRED` task state produces
/// exactly this. `resolve_pending_auth_required_for_connector` is the OAuth/Composio/credential
/// callbacks' bulk-resolve path, called directly from `oss/mcp-gateway` (never through
/// `router/hitl.rs`'s single-row resolve API), so it must carry its own auto-resolve-linkage
/// rather than relying on the one wired into the manual resolve handler.
#[tokio::test]
async fn resolve_pending_auth_required_for_connector_auto_resolves_a_linked_direct_chat_row() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();

    let mcp_row = repo::create_pending_auth_required(
        &db.pool,
        NewAuthRequired {
            agent_id: db.agent_id,
            owner_user_id: db.owner_user_id,
            connector_id,
            context_id: "ctx-reconnect".into(),
            question: serde_json::json!({"provider": "github"}),
        },
    )
    .await
    .expect("create pending auth_required");

    let mirror_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, task_id, context_id, question, status, expires_at)
        VALUES
            ('auth_required', 'direct_chat', $1, $2, 'task-1', 'ctx-mirror', $3, 'pending', now() + interval '7 days')
        RETURNING id
        "#,
    )
    .bind(db.agent_id)
    .bind(db.owner_user_id)
    .bind(serde_json::json!({
        "message": "Tool(s) require user approval for this agent.",
        "metadata": {"hitl_request_id": mcp_row.id.to_string()},
    }))
    .fetch_one(&db.pool)
    .await
    .expect("seed linked direct_chat mirror row");

    let resolved =
        repo::resolve_pending_auth_required_for_connector(&db.pool, db.owner_user_id, connector_id)
            .await
            .expect("resolve pending auth_required rows");
    assert_eq!(resolved.len(), 1, "must resolve the one real mcp_tool row");
    assert_eq!(resolved[0].id, mcp_row.id);

    let (mirror_status, human_response): (String, Option<serde_json::Value>) =
        sqlx::query_as("SELECT status, human_response FROM hitl_requests WHERE id = $1")
            .bind(mirror_id)
            .fetch_one(&db.pool)
            .await
            .expect("fetch mirror row");
    assert_eq!(
        mirror_status, "resolved",
        "the linked direct_chat mirror must be auto-resolved alongside the real mcp_tool row"
    );
    assert_eq!(
        human_response
            .as_ref()
            .and_then(|v| v["auth_outcome"].as_str()),
        Some("confirmed")
    );

    // The mirror is the real, task_id-bearing resume now — the `mcp_tool` row's own resume must
    // be skipped, exactly like the single-row `router/hitl.rs::auto_resolve_linked_direct_chat_row`
    // path already does, or `repo::claim_for_resume` picks it up and fires a redundant,
    // context-free nudge racing the mirror's resume (found in review).
    let mcp_row_resume_status: String =
        sqlx::query_scalar("SELECT resume_status FROM hitl_requests WHERE id = $1")
            .bind(mcp_row.id)
            .fetch_one(&db.pool)
            .await
            .expect("fetch mcp_tool row");
    assert_eq!(
        mcp_row_resume_status, "skipped",
        "the mcp_tool row's own resume must be skipped once its mirror is resolved instead"
    );
}

/// Regression: `find_linked_direct_chat_row` was scoped to `origin IN ('direct_chat',
/// 'agent_proxy')` only — `'maf'` was never added when MAF's own HITL support was merged in, so
/// resolving the real `mcp_tool` row never found (and therefore never auto-resolved) a MAF-step
/// mirror, even though `list_pending_for` and the resume dispatcher have the exact same
/// three-origin scoping elsewhere. A human approving the real row was left having to separately,
/// manually resolve the mirror too.
#[tokio::test]
async fn find_linked_direct_chat_row_finds_a_maf_origin_mirror() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();

    let mcp_row = repo::create_pending_tool_approval(
        &db.pool,
        db.new_tool_approval(connector_id, "GITHUB_CREATE_AN_ISSUE", "ctx-maf-link"),
    )
    .await
    .expect("create pending tool_approval");

    let maf_execution_id = Uuid::new_v4();
    sqlx::query("INSERT INTO maf_executions (id, user_id) VALUES ($1, $2)")
        .bind(maf_execution_id)
        .bind(db.owner_user_id)
        .execute(&db.pool)
        .await
        .expect("seed maf_executions row");

    let mirror_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, task_id, context_id, maf_execution_id,
             maf_step_index, question, status, expires_at)
        VALUES
            ('auth_required', 'maf', $1, $2, 'maf-task-1', 'maf-ctx-1', $3, 0, $4,
             'pending', now() + interval '7 days')
        RETURNING id
        "#,
    )
    .bind(db.agent_id)
    .bind(db.owner_user_id)
    .bind(maf_execution_id)
    .bind(serde_json::json!({
        "message": "Tool(s) require user approval for this agent.",
        "metadata": {"hitl_request_id": mcp_row.id.to_string()},
    }))
    .fetch_one(&db.pool)
    .await
    .expect("seed maf-origin mirror row");

    let linked =
        repo::find_linked_direct_chat_row(&db.pool, mcp_row.id, db.owner_user_id, db.agent_id)
            .await
            .expect("find_linked_direct_chat_row must not error")
            .expect(
                "must find the maf-origin mirror — this is the regression this test guards against",
            );
    assert_eq!(linked.id, mirror_id);
    assert_eq!(linked.origin, nasiko_hitl::HitlOrigin::Maf);
}

/// Same class of regression as `find_linked_direct_chat_row_finds_a_maf_origin_mirror`, for the
/// `orchestrator` origin merged in later from `feature/orchestrator-hitl`: a sub-agent the
/// orchestrator delegates to can map an MCP tool block onto its own pause exactly like a
/// direct-chat agent can, so an `orchestrator`-origin row must be found as a mirror too.
#[tokio::test]
async fn find_linked_direct_chat_row_finds_an_orchestrator_origin_mirror() {
    let db = TestDb::new("hitl_test").await;
    let connector_id = Uuid::new_v4();

    let mcp_row = repo::create_pending_tool_approval(
        &db.pool,
        db.new_tool_approval(
            connector_id,
            "GITHUB_CREATE_AN_ISSUE",
            "ctx-orchestrator-link",
        ),
    )
    .await
    .expect("create pending tool_approval");

    let chat_session_id = format!("orchestrator-chat-session-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO chat_sessions (session_id, user_id, agent_id, title) VALUES ($1, $2, $3, $4)",
    )
    .bind(&chat_session_id)
    .bind(db.owner_user_id)
    .bind(db.agent_id)
    .bind("repo-test-session")
    .execute(&db.pool)
    .await
    .expect("seed chat_sessions row");

    let mirror_id: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, task_id, context_id, chat_session_id,
             question, status, expires_at)
        VALUES
            ('auth_required', 'orchestrator', $1, $2, 'orchestrator-task-1', 'orchestrator-ctx-1',
             $3, $4, 'pending', now() + interval '7 days')
        RETURNING id
        "#,
    )
    .bind(db.agent_id)
    .bind(db.owner_user_id)
    .bind(&chat_session_id)
    .bind(serde_json::json!({
        "message": "Tool(s) require user approval for this agent.",
        "metadata": {"hitl_request_id": mcp_row.id.to_string()},
    }))
    .fetch_one(&db.pool)
    .await
    .expect("seed orchestrator-origin mirror row");

    let linked =
        repo::find_linked_direct_chat_row(&db.pool, mcp_row.id, db.owner_user_id, db.agent_id)
            .await
            .expect("find_linked_direct_chat_row must not error")
            .expect(
                "must find the orchestrator-origin mirror — this is the regression this test guards against",
            );
    assert_eq!(linked.id, mirror_id);
    assert_eq!(linked.origin, nasiko_hitl::HitlOrigin::Orchestrator);
}

/// Security regression: `question.metadata.hitl_request_id` is fully agent-controlled
/// (`build_pause_question` forwards the agent's own status-message metadata verbatim) — a
/// malicious or buggy agent could plant another user's real `hitl_request_id` into a victim's
/// pause. Without the `owner_user_id` filter, resolving the ATTACKER's own unrelated row would
/// find and auto-resolve the VICTIM's mirror, then alias the victim's continuation buffer onto an
/// id the attacker is authorized to reconnect with — a forged approval plus a way to read the
/// victim's resumed execution under their own identity. This must return `None`, not the victim's
/// row, even though the metadata link matches exactly.
#[tokio::test]
async fn find_linked_direct_chat_row_never_crosses_owners() {
    let db = TestDb::new("hitl_test").await;
    let attacker_connector_id = Uuid::new_v4();
    let victim_user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, username, email) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(victim_user_id)
    .bind(format!("victim-{}", victim_user_id.simple()))
    .bind(format!("victim-{}@test.example", victim_user_id.simple()))
    .execute(&db.pool)
    .await
    .expect("seed victim user");

    // The attacker's own, entirely unrelated pending tool_approval row.
    let attacker_row = repo::create_pending_tool_approval(
        &db.pool,
        db.new_tool_approval(
            attacker_connector_id,
            "GITHUB_CREATE_AN_ISSUE",
            "ctx-attacker",
        ),
    )
    .await
    .expect("create attacker's own pending tool_approval");

    // The victim's mirror, planted by a malicious agent with the ATTACKER's row id hardcoded into
    // its own pause metadata — nothing about this event has anything to do with the attacker.
    sqlx::query(
        r#"
        INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, task_id, context_id, question, status, expires_at)
        VALUES
            ('auth_required', 'direct_chat', $1, $2, 'victim-task-1', 'victim-ctx-1', $3,
             'pending', now() + interval '7 days')
        "#,
    )
    .bind(db.agent_id)
    .bind(victim_user_id)
    .bind(serde_json::json!({
        "message": "Tool(s) require user approval for this agent.",
        "metadata": {"hitl_request_id": attacker_row.id.to_string()},
    }))
    .execute(&db.pool)
    .await
    .expect("seed victim's mirror row, planted with the attacker's row id");

    // The attacker resolves their OWN row — authorized, since they own it — but the lookup must
    // not hand back the victim's mirror just because the (agent-controlled) link matches.
    let linked =
        repo::find_linked_direct_chat_row(&db.pool, attacker_row.id, db.owner_user_id, db.agent_id)
            .await
            .expect("find_linked_direct_chat_row must not error");
    assert!(
        linked.is_none(),
        "must never return another user's row, even with a matching metadata.hitl_request_id link"
    );
}

/// Same class of forged-link attack as `find_linked_direct_chat_row_never_crosses_owners`, but
/// same owner, different agent: the two halves of one real pause always belong to the same agent
/// (`resolve_display_row`'s own invariant, `store.rs`), so a link naming a different agent's row
/// can only be forged or stale. Before the `agent_id` filter was added, this same-owner,
/// cross-agent link matched and would have let one agent's pause be resolved and its output-stream
/// access aliased via a completely different agent's mcp_tool row.
#[tokio::test]
async fn find_linked_direct_chat_row_never_crosses_agents() {
    let db = TestDb::new("hitl_test").await;
    let other_agent_id = Uuid::new_v4();
    sqlx::query("INSERT INTO agents (id, name, owner_id) VALUES ($1, $2, $3)")
        .bind(other_agent_id)
        .bind(format!("other-agent-{}", other_agent_id.simple()))
        .bind(db.owner_user_id)
        .execute(&db.pool)
        .await
        .expect("seed a second agent");

    let this_agent_row = repo::create_pending_tool_approval(
        &db.pool,
        db.new_tool_approval(Uuid::new_v4(), "GITHUB_CREATE_AN_ISSUE", "ctx-this-agent"),
    )
    .await
    .expect("create pending tool_approval for this agent");

    // Same owner, but the mirror belongs to a DIFFERENT agent — only the metadata link ties it to
    // `this_agent_row`, which a malicious or buggy agent fully controls.
    sqlx::query(
        r#"
        INSERT INTO hitl_requests
            (kind, origin, agent_id, owner_user_id, task_id, context_id, question, status, expires_at)
        VALUES
            ('auth_required', 'direct_chat', $1, $2, 'other-agent-task-1', 'other-agent-ctx-1', $3,
             'pending', now() + interval '7 days')
        "#,
    )
    .bind(other_agent_id)
    .bind(db.owner_user_id)
    .bind(serde_json::json!({
        "message": "Tool(s) require user approval for this agent.",
        "metadata": {"hitl_request_id": this_agent_row.id.to_string()},
    }))
    .execute(&db.pool)
    .await
    .expect("seed the other agent's mirror row, planted with this agent's row id");

    let linked = repo::find_linked_direct_chat_row(
        &db.pool,
        this_agent_row.id,
        db.owner_user_id,
        db.agent_id,
    )
    .await
    .expect("find_linked_direct_chat_row must not error");
    assert!(
        linked.is_none(),
        "must never return another agent's row, even with a matching metadata.hitl_request_id link"
    );
}
