//! Shared Postgres + `McpState` fixture for `nasiko-mcp-gateway`'s integration tests
//! (`tool_approval.rs`, `auth_required.rs`) — needs infra up (`just infra` from the repo root;
//! override the admin connection with `TEST_PG_URL`), same convention `oss/hitl/tests/common` and
//! `oss/server/tests/common` use. Each call creates and migrates its own scratch database so tests
//! can run concurrently without colliding.
//!
//! Each test file wraps this with its own `impl TestDb { ... }` block for file-specific helpers —
//! inherent impls aren't module-scoped, so this works from any module in the same test binary
//! without needing a wrapper type.

use std::sync::Arc;

use async_trait::async_trait;
use nasiko_mcp_gateway::config::{McpConfig, ToolSearchMode};
use nasiko_mcp_gateway::permissions::{PermissionContext, PermissionRule};
use nasiko_mcp_gateway::provider::{GenericMcpProvider, Providers};
use nasiko_mcp_gateway::repo::McpConnector;
use nasiko_mcp_gateway::types::{AccessReason, OrgGrantConsumer};
use nasiko_mcp_gateway::{ConnectorAuthorizer, McpState};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

pub fn pg_admin_url() -> String {
    std::env::var("TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://nasiko:nasiko@localhost:5432/nasiko_dev".into())
}

/// Layer-1 stub that always allows — mirrors `protocol.rs`'s own `AllowAllAuthorizer` test
/// fixture, duplicated here because that one is private to the crate's `#[cfg(test)]` module and
/// unreachable from an external integration test. Tests exercising `Stance::Ask`/`Block` never
/// reach this check at all; tests that do (broken-credential `auth_required`) pass the real
/// `OssConnectorAuthorizer` instead.
#[allow(dead_code)] // only `tool_approval.rs` constructs this; `auth_required.rs` uses the real one
pub struct AllowAllAuthorizer;
#[async_trait]
impl ConnectorAuthorizer for AllowAllAuthorizer {
    async fn can_access_connector(
        &self,
        _db: &PgPool,
        _user_id: Uuid,
        _connector_id: Uuid,
    ) -> nasiko_mcp_gateway::Result<bool> {
        Ok(true)
    }
    async fn list_accessible_connectors(
        &self,
        _db: &PgPool,
        _user_id: Uuid,
    ) -> nasiko_mcp_gateway::Result<Vec<McpConnector>> {
        Ok(vec![])
    }
    async fn list_accessible_mcp_connectors(
        &self,
        _db: &PgPool,
        _user_id: Uuid,
    ) -> nasiko_mcp_gateway::Result<Vec<McpConnector>> {
        Ok(vec![])
    }
    async fn list_access_reasons(
        &self,
        _db: &PgPool,
        _connector: &McpConnector,
    ) -> nasiko_mcp_gateway::Result<Vec<AccessReason>> {
        Ok(vec![])
    }
    async fn list_org_grant_consumers(
        &self,
        _db: &PgPool,
        _connector_id: Uuid,
    ) -> nasiko_mcp_gateway::Result<(Vec<OrgGrantConsumer>, Vec<OrgGrantConsumer>)> {
        Ok((vec![], vec![]))
    }
}

pub struct TestDb {
    pub state: McpState,
    pub agent_id: Uuid,
    pub owner_user_id: Uuid,
    /// Retained so `Drop` can remove the scratch database — see its own comment.
    db_name: String,
}

impl TestDb {
    /// `prefix` names the scratch database and seed rows (e.g. `"mcp_tool_approval_test"`);
    /// `authorizer` is the connector-access-layer stub/impl this file's tests need
    /// (`AllowAllAuthorizer` for Stance tests, `OssConnectorAuthorizer` for auth_required tests).
    pub async fn new(prefix: &str, authorizer: Arc<dyn ConnectorAuthorizer>) -> Self {
        let pg_admin = pg_admin_url();
        let db_name = format!("nasiko_{prefix}_{}", Uuid::new_v4().simple());

        let admin = PgPoolOptions::new()
            .max_connections(2)
            .connect(&pg_admin)
            .await
            .expect("connect to postgres — is infra up? (set TEST_PG_URL to override; `just infra` starts it)");
        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin)
            .await
            .expect("create scratch test database");

        let base = pg_admin
            .rsplit_once('/')
            .map_or(pg_admin.as_str(), |(b, _)| b);
        let db_url = format!("{base}/{db_name}");
        let db: PgPool = PgPoolOptions::new()
            .max_connections(4)
            .connect(&db_url)
            .await
            .expect("connect to scratch test database");

        sqlx::migrate!("../migrations")
            .run(&db)
            .await
            .expect("run oss/migrations against scratch database");

        let owner_user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, username, email) VALUES ($1, $2, $3)")
            .bind(owner_user_id)
            .bind(format!("{prefix}-{}", owner_user_id.simple()))
            .bind(format!("{prefix}-{}@example.com", owner_user_id.simple()))
            .execute(&db)
            .await
            .expect("seed user");

        let agent_id = Uuid::new_v4();
        sqlx::query("INSERT INTO agents (id, name, owner_id) VALUES ($1, $2, $3)")
            .bind(agent_id)
            .bind(format!("{prefix}-agent-{}", agent_id.simple()))
            .bind(owner_user_id)
            .execute(&db)
            .await
            .expect("seed agent");

        let state = McpState {
            db,
            redis: nasiko_redis::SharedRedis::new(
                redis::Client::open("redis://127.0.0.1:1/").expect("lazy redis client"),
            ),
            http_client: reqwest::Client::new(),
            guarded_http_client: reqwest::Client::new(),
            config: McpConfig {
                composio_api_key: None,
                composio_base_url: "http://localhost".to_string(),
                composio_webhook_secret: None,
                gateway_public_url: None,
                oauth_redirect_base_url: None,
                composio_callback_base_url: None,
                session_ttl_seconds: 60,
                perm_cache_ttl_seconds: 60,
                manifest_ttl_seconds: 60,
                toolcount_ttl_seconds: 3600,
                oauth_state_signing_key: "test".to_string(),
                description_model: "gpt-4o-mini".to_string(),
                hitl_request_ttl_days: 7,
                tool_search_mode: ToolSearchMode::Semantic,
                tool_search_tool_limit: 0,
                tool_search_meta_limit: 0,
                embedding_model: "".to_string(),
                gateway_instructions: String::new(),
                identity_signing_key: b"test-identity-signing-key".to_vec(),
                capture_content: false,
            },
            providers: Providers {
                composio: Default::default(),
                mcp: GenericMcpProvider::new(reqwest::Client::new(), reqwest::Client::new()),
            },
            authorizer,
            entitlements: std::sync::Arc::new(nasiko_mcp_gateway::OssToolEntitlements),
            endpoint_refresher: std::sync::Arc::new(
                nasiko_mcp_gateway::endpoint_refresh::NoopEndpointRefresher,
            ),
            llm: nasiko_orchestrator::providers::LLMProvider::from_env(reqwest::Client::new()),
            search_index: Arc::new(nasiko_mcp_gateway::search::NoopSearchIndex),
        };

        Self {
            state,
            agent_id,
            owner_user_id,
            db_name,
        }
    }

    /// Seed a `chat_sessions` + `flows` row (`flow_id = trace_id`) so `trace_id` resolves to
    /// `session_id` (the A2A contextId) via `session::resolve_context_id`.
    #[allow(dead_code)] // `hidden_toolkit.rs` refuses before any HITL row, so has no trace to seed
    pub async fn seed_session_trace(&self, session_id: &str, trace_id: &str) {
        sqlx::query(
            "INSERT INTO chat_sessions (session_id, user_id, title) VALUES ($1, $2, 'test session')",
        )
        .bind(session_id)
        .bind(self.owner_user_id)
        .execute(&self.state.db)
        .await
        .expect("seed chat_sessions row");

        sqlx::query("INSERT INTO flows (flow_id, user_id, session_id) VALUES ($1, $2, $3)")
            .bind(trace_id)
            .bind(self.owner_user_id)
            .bind(session_id)
            .execute(&self.state.db)
            .await
            .expect("seed flows row");
    }

    /// Seeds a minimal, real `mcp_connectors` row and returns its id — needed by any test that
    /// resolves a `tool_approval` with `scope=session` (`create_session_grant`'s `connector_id`
    /// gained a real FK to this table, `0026_mcp_session_tool_grants_fk.sql`); a synthetic
    /// `Uuid::new_v4()` connector id (fine for the in-memory `MCPServerConfig`/`ResolvedSession`
    /// these tests build, but not for a real grant row) now violates that constraint.
    ///
    /// `url` is not optional padding: `source_kind` defaults to `external_url`, and
    /// `chk_connectors_provider_fields` (`0003_mcp.sql`) requires `url IS NOT NULL` for that
    /// combination, so a `(provider_type, name)`-only insert fails the CHECK. Never dialed — the
    /// real backend these tests talk to comes from the in-memory `MCPServerConfig` above; this row
    /// exists only to satisfy the FK.
    #[allow(dead_code)] // only tool_approval.rs's session-scope tests construct this
    pub async fn seed_connector(&self, prefix: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO mcp_connectors (provider_type, name, url) \
             VALUES ('mcp_server', $1, 'http://127.0.0.1:1/mcp') \
             RETURNING id",
        )
        .bind(format!("{prefix}-connector-{}", Uuid::new_v4().simple()))
        .fetch_one(&self.state.db)
        .await
        .expect("seed connector")
    }

    pub fn perms(&self, enabled: &[Uuid], rules: Vec<PermissionRule>) -> PermissionContext {
        PermissionContext {
            agent_id: self.agent_id,
            enabled_connectors: enabled.iter().copied().collect(),
            rules,
            hash: "h".into(),
        }
    }
}

/// Drop the scratch database when the fixture goes out of scope.
///
/// Without this every test leaked its database — a local Postgres had accumulated 1062 of them,
/// and `CREATE DATABASE` degrades as `pg_database` grows, which is what turned a 5s test binary
/// into a 27s one and started timing out poll loops. `oss/server/tests/common` has always dropped
/// its own via an explicit `cleanup()`; these fixtures had no equivalent.
///
/// Done on a detached thread with its own runtime because `Drop` cannot await and the test's
/// runtime may already be shutting down, and `join()`ed so the drop actually completes before the
/// process exits. `WITH (FORCE)` terminates the pool's remaining backends — otherwise the open
/// connections this fixture still holds would block the drop.
impl Drop for TestDb {
    fn drop(&mut self) {
        let url = pg_admin_url();
        let name = std::mem::take(&mut self.db_name);
        if name.is_empty() {
            return;
        }
        let _ = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async {
                if let Ok(admin) = PgPoolOptions::new().max_connections(1).connect(&url).await {
                    let _ =
                        sqlx::query(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                            .execute(&admin)
                            .await;
                }
            });
        })
        .join();
    }
}
