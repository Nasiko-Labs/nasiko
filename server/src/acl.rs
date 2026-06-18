use std::pin::Pin;
use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

use nasiko_react_agent::CallGuard;

use crate::flow::{FlowContext, FlowGuard};

/// Check whether a user owns or is a team member of the agent's owning team.
/// Used for ownership gates on update/delete/proxy.
pub async fn user_can_access_agent(db: &PgPool, user_id: Uuid, agent_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS(
            SELECT 1 FROM agents a
            LEFT JOIN team_members tm ON tm.team_id = a.owner_team_id AND tm.user_id = $1
            WHERE a.id = $2 AND (a.owner_id = $1 OR tm.user_id IS NOT NULL)
        )"#,
    )
    .bind(user_id)
    .bind(agent_id)
    .fetch_one(db)
    .await
    .unwrap_or(false)
}

/// Check whether `caller_agent_id` is permitted to invoke `target_agent_id`.
///
/// Allowlist semantics:
/// - If agent_acl has NO rows for the caller → unrestricted (can call anything).
/// - If agent_acl has ANY rows for the caller → only listed targets are allowed.
pub async fn check_agent_acl(
    db: &PgPool,
    caller_agent_id: Uuid,
    target_agent_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let allowed: Option<bool> = sqlx::query_scalar(
        r#"
        SELECT CASE
            WHEN NOT EXISTS (SELECT 1 FROM agent_acl WHERE caller_agent_id = $1)
                THEN true
            WHEN EXISTS (SELECT 1 FROM agent_acl WHERE caller_agent_id = $1 AND target_agent_id = $2)
                THEN true
            ELSE false
        END
        "#,
    )
    .bind(caller_agent_id)
    .bind(target_agent_id)
    .fetch_one(db)
    .await?;

    Ok(allowed.unwrap_or(true))
}

/// Fetch the set of agent IDs that `caller_agent_id` is allowed to invoke.
/// Returns None if the caller has no ACL rows (unrestricted).
/// Returns Some(set) if the caller has an explicit allowlist.
pub async fn allowed_targets(
    db: &PgPool,
    caller_agent_id: Uuid,
) -> Result<Option<Vec<Uuid>>, sqlx::Error> {
    let targets: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT target_agent_id FROM agent_acl WHERE caller_agent_id = $1",
    )
    .bind(caller_agent_id)
    .fetch_all(db)
    .await?;

    if targets.is_empty() {
        Ok(None)
    } else {
        Ok(Some(targets.into_iter().map(|(id,)| id).collect()))
    }
}

/// CallGuard implementation for the CP orchestrator.
/// Enforces ACL, flow depth/fan-out/cycle/token limits before each agent call.
pub struct CpCallGuard {
    pub db: PgPool,
    pub flow_guard: FlowGuard,
    pub flow_ctx: Mutex<FlowContext>,
    pub caller_agent_id: Option<Uuid>,
}

impl CpCallGuard {
    pub fn new(
        db: PgPool,
        flow_guard: FlowGuard,
        flow_ctx: FlowContext,
        caller_agent_id: Option<Uuid>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            flow_guard,
            flow_ctx: Mutex::new(flow_ctx),
            caller_agent_id,
        })
    }
}

impl CallGuard for CpCallGuard {
    fn before_call(
        &self,
        target_agent: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> {
        let target = target_agent.to_string();
        Box::pin(async move {
            let ctx = self.flow_ctx.lock().await;

            // ACL check: if the caller has an allowlist, verify target is in it
            if let Some(caller_id) = self.caller_agent_id {
                let target_id = resolve_agent_id(&self.db, &target).await?;
                let allowed = check_agent_acl(&self.db, caller_id, target_id)
                    .await
                    .map_err(|e| format!("ACL check failed: {e}"))?;
                if !allowed {
                    return Err(format!("agent ACL denied: caller cannot invoke '{}'", target));
                }
            }

            // Flow guard: depth, cycle, fan-out, token budget, timeout (all in Redis)
            if let Err(rejection) = self.flow_guard.check(&ctx, &target).await {
                return Err(rejection.to_string());
            }

            if let Err(rejection) = self.flow_guard.record_invocation(&ctx, &target).await {
                return Err(rejection.to_string());
            }

            Ok(())
        })
    }

    fn after_call(
        &self,
        _target_agent: &str,
        tokens_used: u64,
    ) -> Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let ctx = self.flow_ctx.lock().await;
            // Pop from call stack so cycle detection allows repeated calls
            self.flow_guard.record_return(&ctx).await;
            if tokens_used > 0 {
                if let Err(e) = self.flow_guard.record_tokens(&ctx, tokens_used).await {
                    warn!(%e, "flow token budget exceeded after call");
                }
            }
        })
    }
}

/// Resolve an agent name to its UUID.
async fn resolve_agent_id(db: &PgPool, agent_name: &str) -> Result<Uuid, String> {
    let id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM agents WHERE name = $1 AND status = 'running'",
    )
    .bind(agent_name)
    .fetch_optional(db)
    .await
    .map_err(|e| format!("db error resolving agent '{}': {}", agent_name, e))?;

    id.ok_or_else(|| format!("agent '{}' not found or not running", agent_name))
}
