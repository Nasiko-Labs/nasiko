//! Integration tests for the resume dispatcher (M6): `repo::claim_for_resume`/
//! `finish_resume`/`recover_stuck_resumes` against a real Postgres, and the
//! full `dispatcher::run` loop + `RuntimeResumeNotifier` against a real HTTP
//! server standing in for an agent container. Needs infra up (`just infra`;
//! override with `TEST_PG_URL`), same convention as `tests/repo.rs`. Each
//! test creates and drops its own scratch database so tests can run
//! concurrently without colliding.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use nasiko_hitl::dispatcher::{self, DispatcherConfig};
use nasiko_hitl::notifier::RuntimeResumeNotifier;
use nasiko_hitl::repo::{self, NewAuthRequired, ResolveDecision};
use nasiko_hitl::{HitlKind, HitlStatus, NotifyError, ResumeNotifier, ResumeStatus};
use nasiko_runtime::{ContainerId, ContainerRuntime, DeploymentSpec, SimulatedRuntime};
use uuid::Uuid;

mod common;
use common::TestDb;

impl TestDb {
    /// Back a `context_id` with a real `chat_sessions` row owned by this fixture's own
    /// `owner_user_id` — what every HITL row persisted through the normal verified-flow path
    /// actually has. Without this, `RuntimeResumeNotifier::notify`'s `session_traces` insert (its
    /// `WHERE EXISTS (... user_id = $4)` ownership guard) affects 0 rows and the nudge now aborts
    /// with `NotifyError::ContextNotOwned` (mirroring the `flows` registration guard) rather than
    /// warn-and-continue, so every fixture that exercises a real `notify()` call needs one of
    /// these for its `context_id` unless it's deliberately testing that guard. `ON CONFLICT DO
    /// NOTHING` because a raw 32-hex trace_id `context_id` (the BL4 fixtures) never reaches this
    /// table anyway, but calling this for every `context_id` uniformly keeps the seed helpers
    /// simple.
    async fn seed_chat_session(&self, context_id: &str) {
        sqlx::query(
            "INSERT INTO chat_sessions (session_id, user_id, title) VALUES ($1, $2, $3) \
             ON CONFLICT (session_id) DO NOTHING",
        )
        .bind(context_id)
        .bind(self.owner_user_id)
        .bind("test session")
        .execute(&self.pool)
        .await
        .expect("seed chat session");
    }

    /// Create and immediately resolve (approve) a `tool_approval` row for
    /// this fixture's agent/owner — the dispatcher only ever acts on
    /// `status = 'resolved'` rows.
    async fn seed_resolved_tool_approval(&self, context_id: &str) -> Uuid {
        self.seed_chat_session(context_id).await;
        let created = repo::create_pending_tool_approval(
            &self.pool,
            repo::NewToolApproval {
                agent_id: self.agent_id,
                owner_user_id: self.owner_user_id,
                connector_id: Uuid::new_v4(),
                tool_name: "GITHUB_DELETE_REPO".to_string(),
                context_id: context_id.to_string(),
                question: serde_json::json!({"tool_name": "GITHUB_DELETE_REPO"}),
            },
        )
        .await
        .expect("create pending tool_approval");

        repo::resolve(
            &self.pool,
            created.id,
            ResolveDecision::Approve,
            self.owner_user_id,
            serde_json::json!({"decision": "approve"}),
        )
        .await
        .expect("resolve")
        .expect("row was pending");

        created.id
    }

    /// Create and immediately reject a `tool_approval` row — proves
    /// `claim_for_resume` also claims `status = 'rejected'` rows, not just
    /// `'resolved'` ones (found missing in code review: a human's reject
    /// decision never reached the paused agent at all before this fix).
    async fn seed_rejected_tool_approval(&self, context_id: &str) -> Uuid {
        self.seed_chat_session(context_id).await;
        let created = repo::create_pending_tool_approval(
            &self.pool,
            repo::NewToolApproval {
                agent_id: self.agent_id,
                owner_user_id: self.owner_user_id,
                connector_id: Uuid::new_v4(),
                tool_name: "GITHUB_DELETE_REPO".to_string(),
                context_id: context_id.to_string(),
                question: serde_json::json!({"tool_name": "GITHUB_DELETE_REPO"}),
            },
        )
        .await
        .expect("create pending tool_approval");

        repo::resolve(
            &self.pool,
            created.id,
            ResolveDecision::Reject,
            self.owner_user_id,
            serde_json::json!({"decision": "reject"}),
        )
        .await
        .expect("resolve")
        .expect("row was pending");

        created.id
    }

    async fn seed_resolved_auth_required(&self, context_id: &str) -> Uuid {
        self.seed_chat_session(context_id).await;
        let created = repo::create_pending_auth_required(
            &self.pool,
            NewAuthRequired {
                agent_id: self.agent_id,
                owner_user_id: self.owner_user_id,
                connector_id: Uuid::new_v4(),
                context_id: context_id.to_string(),
                question: serde_json::json!({"connector": "github"}),
            },
        )
        .await
        .expect("create pending auth_required");

        repo::resolve(
            &self.pool,
            created.id,
            ResolveDecision::Approve,
            self.owner_user_id,
            serde_json::json!({"decision": "approve"}),
        )
        .await
        .expect("resolve")
        .expect("row was pending");

        created.id
    }

    async fn resume_status_of(&self, id: Uuid) -> String {
        sqlx::query_scalar("SELECT resume_status FROM hitl_requests WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .expect("fetch resume_status")
    }
}

fn agent_spec(container_id: ContainerId) -> DeploymentSpec {
    DeploymentSpec {
        container_id,
        name: "hitl-dispatch-test-agent".to_string(),
        image: "example/agent:latest".to_string(),
        min_replicas: 1,
        max_replicas: 1,
        env_vars: HashMap::new(),
        ports: vec![8080],
        resources: None,
        image_pull_secret_name: None,
        image_pull_credential_seed: None,
        harden: false,
        network_override: None,
        workload_kind: Default::default(),
        writable: false,
        writable_path: None,
        owner_id: Uuid::nil(),
        force_pull: false,
    }
}

// ─── claim_for_resume / finish_resume / recover_stuck_resumes ──────────────

#[tokio::test]
async fn claim_for_resume_only_claims_resolved_not_started_rows() {
    let db = TestDb::new("hitl_dispatch_test").await;
    let pending_id = repo::create_pending_tool_approval(
        &db.pool,
        repo::NewToolApproval {
            agent_id: db.agent_id,
            owner_user_id: db.owner_user_id,
            connector_id: Uuid::new_v4(),
            tool_name: "GITHUB_DELETE_REPO".to_string(),
            context_id: "ctx-pending".to_string(),
            question: serde_json::json!({}),
        },
    )
    .await
    .expect("create pending row")
    .id;
    let resolved_id = db.seed_resolved_tool_approval("ctx-resolved").await;

    let claimed = repo::claim_for_resume(&db.pool)
        .await
        .expect("claim query")
        .expect("exactly one resolved row is claimable");

    assert_eq!(
        claimed.id, resolved_id,
        "must claim the resolved row, not the still-pending one"
    );
    assert_ne!(claimed.id, pending_id);

    // The queue is now empty — the only resolved row is already claimed.
    assert!(
        repo::claim_for_resume(&db.pool)
            .await
            .expect("claim query")
            .is_none()
    );
}

#[tokio::test]
async fn concurrent_claims_exactly_one_wins() {
    let db = TestDb::new("hitl_dispatch_test").await;
    db.seed_resolved_tool_approval("ctx-1").await;

    let (a, b) = tokio::join!(
        repo::claim_for_resume(&db.pool),
        repo::claim_for_resume(&db.pool),
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
        "exactly one of two concurrent claims on the same row must win"
    );
}

#[tokio::test]
async fn finish_resume_records_completed_outcome() {
    let db = TestDb::new("hitl_dispatch_test").await;
    let id = db.seed_resolved_tool_approval("ctx-1").await;
    repo::claim_for_resume(&db.pool)
        .await
        .expect("claim")
        .expect("row claimable");

    let updated = repo::finish_resume(&db.pool, id, ResumeStatus::Completed, 1, None)
        .await
        .expect("finish_resume")
        .expect("claimed row can be finished");
    assert_eq!(updated.resume_status, ResumeStatus::Completed);
    assert_eq!(updated.resume_dispatch_attempts, 1);
}

#[tokio::test]
async fn finish_resume_does_not_clobber_a_quarantined_row() {
    let db = TestDb::new("hitl_dispatch_test").await;
    let id = db.seed_resolved_tool_approval("ctx-1").await;
    repo::claim_for_resume(&db.pool)
        .await
        .expect("claim")
        .expect("row claimable");

    // Simulate the dispatcher process dying mid-attempt: the lease is set,
    // resume_status is still not_started. A 0-minute lease makes it
    // immediately eligible for quarantine.
    let quarantined = repo::recover_stuck_resumes(&db.pool, 0)
        .await
        .expect("recovery sweep");
    assert_eq!(quarantined, 1);
    assert_eq!(db.resume_status_of(id).await, "delivery_outcome_unknown");

    // A late-finishing zombie attempt must not be able to overwrite that.
    let result = repo::finish_resume(&db.pool, id, ResumeStatus::Completed, 1, None)
        .await
        .expect("finish_resume must not error");
    assert!(
        result.is_none(),
        "finish_resume must refuse to overwrite a row the recovery sweep already quarantined"
    );
    assert_eq!(db.resume_status_of(id).await, "delivery_outcome_unknown");
}

#[tokio::test]
async fn a_quarantined_row_is_never_reclaimed() {
    let db = TestDb::new("hitl_dispatch_test").await;
    let id = db.seed_resolved_tool_approval("ctx-1").await;
    repo::claim_for_resume(&db.pool)
        .await
        .expect("claim")
        .expect("row claimable");
    repo::recover_stuck_resumes(&db.pool, 0)
        .await
        .expect("recovery sweep");

    assert_eq!(db.resume_status_of(id).await, "delivery_outcome_unknown");
    assert!(
        repo::claim_for_resume(&db.pool)
            .await
            .expect("claim query")
            .is_none(),
        "claim_for_resume must never select a quarantined row"
    );
}

#[tokio::test]
async fn recover_stuck_resumes_ignores_unclaimed_rows() {
    let db = TestDb::new("hitl_dispatch_test").await;
    db.seed_resolved_tool_approval("ctx-1").await;

    // Nothing has been claimed yet, so a 0-minute lease must still find
    // nothing to quarantine — recover_stuck_resumes only ever touches rows
    // with resume_claimed_at already set.
    let quarantined = repo::recover_stuck_resumes(&db.pool, 0)
        .await
        .expect("recovery sweep");
    assert_eq!(quarantined, 0);
}

/// Generous flow-timeout bound for these tests: the notifier refuses to nudge when the flow its
/// context names is older than this (`NotifyError::FlowNotLive`), and fixtures here seed rows with
/// `now()` timestamps, so any value comfortably above the test's own runtime keeps that guard out
/// of the way of what each test is actually asserting.
const TEST_FLOW_TIMEOUT_SECS: i64 = 3600;

// ─── End-to-end: dispatcher::run + RuntimeResumeNotifier ───────────────────

#[tokio::test]
async fn resolved_row_is_delivered_exactly_once_end_to_end() {
    let db = TestDb::new("hitl_dispatch_test").await;
    let request_id = db.seed_resolved_auth_required("ctx-e2e").await;

    let mut mock_server = mockito::Server::new_async().await;
    let mock = mock_server
        .mock("POST", "/")
        .match_header("a2a-version", "1.0")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":"1","result":{"kind":"message"}}"#)
        .expect(1)
        .create_async()
        .await;

    let runtime = Arc::new(SimulatedRuntime::new(mock_server.url()));
    let container_id = ContainerId::from_uuid(db.agent_id);
    runtime
        .deploy(&agent_spec(container_id))
        .await
        .expect("seed the simulated runtime's endpoint for this agent");

    let notifier: Arc<dyn nasiko_hitl::ResumeNotifier> = Arc::new(RuntimeResumeNotifier::new(
        db.pool.clone(),
        runtime.clone(),
        reqwest::Client::new(),
        TEST_FLOW_TIMEOUT_SECS,
    ));

    let config = DispatcherConfig {
        poll_interval: Duration::from_millis(20),
        recovery_interval: Duration::from_secs(3600),
        ..Default::default()
    };
    let handle = tokio::spawn(dispatcher::run(db.pool.clone(), notifier, config));

    // Bounded poll for the async loop to pick up and finish the row —
    // generous but not infinite, so a real regression fails the test instead
    // of hanging the suite.
    let mut delivered = false;
    for _ in 0..200 {
        if db.resume_status_of(request_id).await == "completed" {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    handle.abort();

    assert!(
        delivered,
        "dispatcher must mark the row completed once delivered"
    );
    mock.assert_async().await;

    let row = repo::get_by_id(&db.pool, request_id)
        .await
        .expect("get_by_id")
        .expect("row exists");
    assert_eq!(row.kind, HitlKind::AuthRequired);
    assert_eq!(row.status, HitlStatus::Resolved);
    assert_eq!(row.resume_status, ResumeStatus::Completed);
    assert_eq!(row.resume_dispatch_attempts, 1);
}

/// The exploit this guard closes: a coding-agent row's owner-fallback `tools/call` can carry a
/// well-formed traceparent naming some OTHER user's (the "victim's") live or recently-live flow.
/// Without `traceparent_for_context`'s `flows.user_id = $2` scope on the `ON CONFLICT DO UPDATE`,
/// `oss/mcp-gateway/src/session.rs::resolve_context_id` handing that flow's own trace id back as
/// a HITL row's `context_id` (no `session_traces` mapping yet) would let this notifier's resume
/// nudge upsert `flows` keyed on that trace id — flipping the victim's flow back to `running` and
/// adding the coding-agent row to `flow_participants`, after which `gateway.rs::flow_user` would
/// resolve the coding row's calls as acting for the VICTIM, not its real owner.
/// `create_tool_approval_id` never seeds a `context_id` from anything but a verified flow (see
/// its own doc comment) — this test instead proves the second, independent guard: even if a
/// `context_id` naming another user's flow ever reaches this notifier (a persisted row is trusted
/// input to `notify`, not re-validated against how it was created), `traceparent_for_context`'s
/// `flows.user_id = $2` scope on the `ON CONFLICT DO UPDATE` must make the adoption attempt a
/// no-op and the notify abort, rather than silently succeeding.
#[tokio::test]
async fn resume_nudge_never_adopts_another_users_flow_via_context_id() {
    let db = TestDb::new("hitl_notifier_guard_test").await;

    // The victim: a different user, with their own flow — deliberately
    // seeded as `completed`, so a successful (buggy) adoption would be
    // observable as this flipping back to `running`.
    let victim_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
        .bind(victim_id)
        .bind(format!("victim-{}", victim_id.simple()))
        .bind(format!("victim-{}@example.com", victim_id.simple()))
        .execute(&db.pool)
        .await
        .expect("seed victim user");

    let victim_flow_id = Uuid::new_v4().simple().to_string();
    sqlx::query("INSERT INTO flows (flow_id, user_id, status) VALUES ($1, $2, 'completed')")
        .bind(&victim_flow_id)
        .bind(victim_id)
        .execute(&db.pool)
        .await
        .expect("seed victim flow");

    // This fixture's own agent/owner stand in for the coding-agent row and
    // its real owner — the HITL row is persisted exactly as
    // `create_tool_approval_id` would (owned by the coding row's owner), but
    // with `context_id` set to the VICTIM's flow id, simulating a row that
    // reached this notifier however it got here.
    let created = repo::create_pending_tool_approval(
        &db.pool,
        repo::NewToolApproval {
            agent_id: db.agent_id,
            owner_user_id: db.owner_user_id,
            connector_id: Uuid::new_v4(),
            tool_name: "GITHUB_DELETE_REPO".to_string(),
            context_id: victim_flow_id.clone(),
            question: serde_json::json!({"tool_name": "GITHUB_DELETE_REPO"}),
        },
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
    .expect("resolve")
    .expect("row was pending");

    let claimed = repo::claim_for_resume(&db.pool)
        .await
        .expect("claim query")
        .expect("the resolved row is claimable");

    let runtime = Arc::new(SimulatedRuntime::new("http://127.0.0.1:1".to_string()));
    let container_id = ContainerId::from_uuid(db.agent_id);
    runtime
        .deploy(&agent_spec(container_id))
        .await
        .expect("seed the simulated runtime's endpoint for this agent");

    let notifier = RuntimeResumeNotifier::new(
        db.pool.clone(),
        runtime.clone(),
        reqwest::Client::new(),
        TEST_FLOW_TIMEOUT_SECS,
    );

    let result = notifier.notify(&claimed).await;
    assert!(
        matches!(result, Err(NotifyError::FlowNotLive { .. })),
        "notify must abort rather than adopt the victim's flow: {result:?}"
    );

    let victim_flow_after: (String, Uuid) =
        sqlx::query_as("SELECT status, user_id FROM flows WHERE flow_id = $1")
            .bind(&victim_flow_id)
            .fetch_one(&db.pool)
            .await
            .expect("victim flow row must still exist");
    assert_eq!(
        victim_flow_after,
        ("completed".to_string(), victim_id),
        "the victim's flow must be untouched — not resurrected, not reassigned"
    );

    let participant_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flow_participants WHERE flow_id = $1 AND agent_id = $2",
    )
    .bind(&victim_flow_id)
    .bind(db.agent_id)
    .fetch_one(&db.pool)
    .await
    .expect("count query");
    assert_eq!(
        participant_count, 0,
        "the coding-agent row must never become a participant of the victim's flow"
    );
}

/// The other half of the same guard: a `context_id` that is NOT a raw trace id (so it takes the
/// `session_traces`-mapping branch, not the `is_raw_trace_id` one the test above exercises) but
/// names a chat session owned by someone other than this HITL row's own owner. The `WHERE EXISTS`
/// guard on the `session_traces` insert makes that insert affect 0 rows, and `notify` must abort
/// right there rather than warn-and-continue: the flow registration below runs unconditionally,
/// so continuing would still open a live `flows`/`flow_participants` row for this fresh
/// `trace_id`, attributed to the row's own owner, over a context (the victim's session) that
/// owner was never granted. And since no `session_traces` mapping now links `trace_id` back to
/// the real `context_id`, the agent's retry resolves to `trace_id` itself
/// (`resolve_context_id`'s no-mapping fallback) — which never matches this HITL row's actual
/// `context_id`, so the human would be asked to approve the same action again.
#[tokio::test]
async fn resume_nudge_aborts_when_context_id_names_another_users_chat_session() {
    let db = TestDb::new("hitl_context_not_owned_test").await;

    // The victim: a different user, with their own chat session.
    let victim_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
        .bind(victim_id)
        .bind(format!("victim-{}", victim_id.simple()))
        .bind(format!("victim-{}@example.com", victim_id.simple()))
        .execute(&db.pool)
        .await
        .expect("seed victim user");

    let victim_session_id = format!("ses_victim_{}", Uuid::new_v4().simple());
    sqlx::query("INSERT INTO chat_sessions (session_id, user_id, title) VALUES ($1, $2, $3)")
        .bind(&victim_session_id)
        .bind(victim_id)
        .bind("victim session")
        .execute(&db.pool)
        .await
        .expect("seed victim chat session");

    // This fixture's own agent/owner stand in for the coding-agent row and its real owner — the
    // HITL row is owned by `db.owner_user_id` but its `context_id` names the VICTIM's session,
    // simulating a row that reached this notifier however it got here.
    let created = repo::create_pending_tool_approval(
        &db.pool,
        repo::NewToolApproval {
            agent_id: db.agent_id,
            owner_user_id: db.owner_user_id,
            connector_id: Uuid::new_v4(),
            tool_name: "GITHUB_DELETE_REPO".to_string(),
            context_id: victim_session_id.clone(),
            question: serde_json::json!({"tool_name": "GITHUB_DELETE_REPO"}),
        },
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
    .expect("resolve")
    .expect("row was pending");

    let claimed = repo::claim_for_resume(&db.pool)
        .await
        .expect("claim query")
        .expect("the resolved row is claimable");

    let runtime = Arc::new(SimulatedRuntime::new("http://127.0.0.1:1".to_string()));
    runtime
        .deploy(&agent_spec(ContainerId::from_uuid(db.agent_id)))
        .await
        .expect("seed the simulated runtime's endpoint for this agent");

    let notifier = RuntimeResumeNotifier::new(
        db.pool.clone(),
        runtime.clone(),
        reqwest::Client::new(),
        TEST_FLOW_TIMEOUT_SECS,
    );

    let err = notifier
        .notify(&claimed)
        .await
        .expect_err("notify must abort rather than map onto another user's chat session");
    assert!(
        matches!(err, NotifyError::ContextNotOwned { .. }),
        "expected ContextNotOwned, got {err:?}"
    );
    assert!(
        err.is_permanent(),
        "ownership of a chat session never changes on retry"
    );

    let session_traces_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_traces WHERE session_id = $1")
            .bind(&victim_session_id)
            .fetch_one(&db.pool)
            .await
            .expect("count query");
    assert_eq!(
        session_traces_count, 0,
        "no session_traces mapping must be written for the victim's session"
    );

    let flows_count: i64 = sqlx::query_scalar("SELECT count(*) FROM flows")
        .fetch_one(&db.pool)
        .await
        .expect("count query");
    assert_eq!(flows_count, 0, "no flow must be registered");

    let participants_count: i64 = sqlx::query_scalar("SELECT count(*) FROM flow_participants")
        .fetch_one(&db.pool)
        .await
        .expect("count query");
    assert_eq!(
        participants_count, 0,
        "no flow participant must be registered"
    );
}

#[tokio::test]
async fn peer_error_response_is_retried_then_marked_failed() {
    let db = TestDb::new("hitl_dispatch_test").await;
    let request_id = db.seed_resolved_tool_approval("ctx-peer-error").await;

    let mut mock_server = mockito::Server::new_async().await;
    let mock = mock_server
        .mock("POST", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":"1","error":{"code":-32000,"message":"boom"}}"#)
        .expect(3)
        .create_async()
        .await;

    let runtime = Arc::new(SimulatedRuntime::new(mock_server.url()));
    let container_id = ContainerId::from_uuid(db.agent_id);
    runtime
        .deploy(&agent_spec(container_id))
        .await
        .expect("seed the simulated runtime's endpoint for this agent");

    let notifier: Arc<dyn nasiko_hitl::ResumeNotifier> = Arc::new(RuntimeResumeNotifier::new(
        db.pool.clone(),
        runtime.clone(),
        reqwest::Client::new(),
        TEST_FLOW_TIMEOUT_SECS,
    ));

    let config = DispatcherConfig {
        poll_interval: Duration::from_millis(20),
        recovery_interval: Duration::from_secs(3600),
        max_attempts: 3,
        retry_delay: Duration::from_millis(10),
        ..Default::default()
    };
    let handle = tokio::spawn(dispatcher::run(db.pool.clone(), notifier, config));

    let mut finished = false;
    for _ in 0..200 {
        let status = db.resume_status_of(request_id).await;
        if status == "failed" {
            finished = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    handle.abort();

    assert!(
        finished,
        "dispatcher must give up and mark the row failed after exhausting retries"
    );
    mock.assert_async().await;

    let row = repo::get_by_id(&db.pool, request_id)
        .await
        .expect("get_by_id")
        .expect("row exists");
    assert_eq!(row.resume_dispatch_attempts, 3);
    assert!(row.resume_last_error.is_some());
}

/// A rejected `tool_approval` row must reach the paused agent too, not just an approved one —
/// `claim_for_resume`'s `WHERE status IN ('resolved', 'rejected')` is what makes this possible;
/// before the fix the row simply sat un-dispatched forever (`resume_status` never left
/// `not_started`), even though `build_resume_message` already had a "denied, do not retry"
/// message ready for it.
#[tokio::test]
async fn rejected_tool_approval_row_is_claimed_and_delivered() {
    let db = TestDb::new("hitl_dispatch_test").await;
    let request_id = db.seed_rejected_tool_approval("ctx-rejected").await;

    let mut mock_server = mockito::Server::new_async().await;
    let mock = mock_server
        .mock("POST", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":"1","result":{"kind":"message"}}"#)
        .expect(1)
        .create_async()
        .await;

    let runtime = Arc::new(SimulatedRuntime::new(mock_server.url()));
    let container_id = ContainerId::from_uuid(db.agent_id);
    runtime
        .deploy(&agent_spec(container_id))
        .await
        .expect("seed the simulated runtime's endpoint for this agent");

    let notifier: Arc<dyn nasiko_hitl::ResumeNotifier> = Arc::new(RuntimeResumeNotifier::new(
        db.pool.clone(),
        runtime.clone(),
        reqwest::Client::new(),
        TEST_FLOW_TIMEOUT_SECS,
    ));

    let config = DispatcherConfig {
        poll_interval: Duration::from_millis(20),
        recovery_interval: Duration::from_secs(3600),
        ..Default::default()
    };
    let handle = tokio::spawn(dispatcher::run(db.pool.clone(), notifier, config));

    let mut delivered = false;
    for _ in 0..200 {
        if db.resume_status_of(request_id).await == "completed" {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    handle.abort();

    assert!(
        delivered,
        "a rejected row must be claimed and delivered too, not left stuck at not_started"
    );
    mock.assert_async().await;

    let row = repo::get_by_id(&db.pool, request_id)
        .await
        .expect("get_by_id")
        .expect("row exists");
    assert_eq!(row.status, HitlStatus::Rejected);
    assert_eq!(row.resume_status, ResumeStatus::Completed);
}

/// The resume nudge must carry a `traceparent` whose trace_id resolves back to the row's own
/// `context_id` — without it, the agent's retried tool call gets an unrelated context_id and an
/// already-approved action gets asked for again. Covers both the fallback and mapped cases:
/// `ctx-raw-e2e` is deliberately a valid-looking raw 32-hex trace_id, so this asserts the
/// traceparent's trace_id segment equals it exactly (no `session_traces` row needed at all —
/// `resolve_context_id` would return it unchanged either way).
#[tokio::test]
async fn resolved_row_delivery_carries_a_traceparent_matching_its_context_id() {
    let db = TestDb::new("hitl_dispatch_test").await;
    let raw_trace_id = Uuid::new_v4().simple().to_string();
    let request_id = db.seed_resolved_tool_approval(&raw_trace_id).await;

    let mut mock_server = mockito::Server::new_async().await;
    let mock = mock_server
        .mock("POST", "/")
        .match_header(
            "traceparent",
            mockito::Matcher::Regex(format!("^00-{raw_trace_id}-[0-9a-f]{{16}}-01$")),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":"1","result":{"kind":"message"}}"#)
        .expect(1)
        .create_async()
        .await;

    let runtime = Arc::new(SimulatedRuntime::new(mock_server.url()));
    let container_id = ContainerId::from_uuid(db.agent_id);
    runtime
        .deploy(&agent_spec(container_id))
        .await
        .expect("seed the simulated runtime's endpoint for this agent");

    let notifier: Arc<dyn nasiko_hitl::ResumeNotifier> = Arc::new(RuntimeResumeNotifier::new(
        db.pool.clone(),
        runtime.clone(),
        reqwest::Client::new(),
        TEST_FLOW_TIMEOUT_SECS,
    ));

    let config = DispatcherConfig {
        poll_interval: Duration::from_millis(20),
        recovery_interval: Duration::from_secs(3600),
        ..Default::default()
    };
    let handle = tokio::spawn(dispatcher::run(db.pool.clone(), notifier, config));

    let mut delivered = false;
    for _ in 0..200 {
        if db.resume_status_of(request_id).await == "completed" {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    handle.abort();

    assert!(delivered, "dispatcher must mark the row completed");
    // The real assertion is `mock`'s header match — this just confirms the mock actually got hit
    // (a header mismatch in mockito is a silent non-match, not a request failure, so without
    // this the test would pass even if the traceparent were missing entirely).
    mock.assert_async().await;
}

// ─── BL4: the nudge must never register a window the gateway will reject ────

/// The platform default, so these two tests exercise the real bound rather than an invented one.
const GATEWAY_FLOW_TIMEOUT_SECS: i64 = 120;

/// Seed a `flows` row for `flow_id` with an explicit age and status — stands in for the original
/// tool call's own flow, which is what `is_raw_trace_id` collides with.
async fn seed_flow(db: &TestDb, flow_id: &str, status: &str, age_secs: i64) {
    sqlx::query(
        "INSERT INTO flows (flow_id, user_id, root_agent_id, title, status, created_at)
         VALUES ($1, $2, $3, 'original call', $4, now() - make_interval(secs => $5))",
    )
    .bind(flow_id)
    .bind(db.owner_user_id)
    .bind(db.agent_id)
    .bind(status)
    .bind(age_secs as f64)
    .execute(&db.pool)
    .await
    .expect("seed flows row");
}

async fn flow_status(db: &TestDb, flow_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM flows WHERE flow_id = $1")
        .bind(flow_id)
        .fetch_one(&db.pool)
        .await
        .expect("read flows.status")
}

/// A `context_id` that is itself a raw trace id collides with the original call's own `flows` row,
/// whose `created_at` never moves. Once that row is older than the platform's flow timeout,
/// `gateway.rs`'s `flow_user` will reject the agent's retry no matter what — so the nudge must
/// fail loudly instead of being delivered as if healthy, and must NOT flip the dead flow back to
/// `running` (which would re-open a closed `/api/mcp` window for every agent in it).
#[tokio::test]
async fn an_expired_flow_is_not_resurrected_and_the_nudge_fails_loudly() {
    let db = TestDb::new("hitl_bl4_expired").await;
    let trace_id = "0af7651916cd43dd8448eb211c80319c";
    seed_flow(&db, trace_id, "completed", 10 * 60).await;
    let request_id = db.seed_resolved_auth_required(trace_id).await;

    let mut mock_server = mockito::Server::new_async().await;
    // The agent must never be contacted: a nudge it cannot act on is worse than none.
    let mock = mock_server.mock("POST", "/").expect(0).create_async().await;

    let runtime = Arc::new(SimulatedRuntime::new(mock_server.url()));
    runtime
        .deploy(&agent_spec(ContainerId::from_uuid(db.agent_id)))
        .await
        .expect("seed the simulated runtime's endpoint");

    let notifier = RuntimeResumeNotifier::new(
        db.pool.clone(),
        runtime.clone(),
        reqwest::Client::new(),
        GATEWAY_FLOW_TIMEOUT_SECS,
    );
    let request = repo::get_by_id(&db.pool, request_id)
        .await
        .expect("get_by_id")
        .expect("row exists");

    let err = nasiko_hitl::ResumeNotifier::notify(&notifier, &request)
        .await
        .expect_err("an expired flow must not be nudged");
    assert!(
        matches!(
            err,
            nasiko_hitl::dispatcher::NotifyError::FlowNotLive { .. }
        ),
        "expected FlowNotLive, got {err:?}"
    );
    assert!(
        err.is_permanent(),
        "a flow only gets older — retrying can never make this succeed"
    );

    mock.assert_async().await;
    assert_eq!(
        flow_status(&db, trace_id).await,
        "completed",
        "the dead flow must stay closed — flipping it back to running re-opens the /api/mcp \
         window for every agent in its flow_participants"
    );
}

/// The other half: a flow still inside the timeout is exactly the case this registration exists
/// for, so it IS reopened and the nudge goes out.
#[tokio::test]
async fn a_still_live_flow_is_reopened_and_the_nudge_is_delivered() {
    let db = TestDb::new("hitl_bl4_live").await;
    let trace_id = "1bf7651916cd43dd8448eb211c80319d";
    seed_flow(&db, trace_id, "completed", 5).await;
    let request_id = db.seed_resolved_auth_required(trace_id).await;

    let mut mock_server = mockito::Server::new_async().await;
    let mock = mock_server
        .mock("POST", "/")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"jsonrpc":"2.0","id":"1","result":{"kind":"message"}}"#)
        .expect(1)
        .create_async()
        .await;

    let runtime = Arc::new(SimulatedRuntime::new(mock_server.url()));
    runtime
        .deploy(&agent_spec(ContainerId::from_uuid(db.agent_id)))
        .await
        .expect("seed the simulated runtime's endpoint");

    let notifier = RuntimeResumeNotifier::new(
        db.pool.clone(),
        runtime.clone(),
        reqwest::Client::new(),
        GATEWAY_FLOW_TIMEOUT_SECS,
    );
    let request = repo::get_by_id(&db.pool, request_id)
        .await
        .expect("get_by_id")
        .expect("row exists");

    nasiko_hitl::ResumeNotifier::notify(&notifier, &request)
        .await
        .expect("a live flow must be nudged");

    mock.assert_async().await;
    assert_eq!(
        flow_status(&db, trace_id).await,
        "running",
        "a flow still inside the timeout is reopened so the retry can authenticate"
    );
    let participant: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM flow_participants WHERE flow_id = $1 AND agent_id = $2",
    )
    .bind(trace_id)
    .bind(db.agent_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(
        participant, 1,
        "the nudged agent must be a flow participant"
    );
}
