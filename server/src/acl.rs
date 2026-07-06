use std::pin::Pin;
use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

use nasiko_react_agent::CallGuard;

use nasiko_flow::{FlowContext, FlowGuard};

/// Edition-aware per-agent access check — the single entry point handlers should use.
///
/// Delegates to `state.auth.can_access_agent`, so it honors exactly the rules of the
/// running edition: OSS = owner ∪ public ∪ user-grant (superuser sees all);
/// EE additionally honors team/department grants. This replaces direct calls to the
/// OSS-only [`user_can_access_agent`] free function on handler paths, which is why a
/// team/dept-granted user no longer 403s on catalog/deploy/update (AUTH-4).
///
/// The superuser short-circuit lives inside the trait impls, so callers do NOT need a
/// separate `claims.is_superuser` guard.
pub async fn can_access_agent(
    state: &crate::state::AppState,
    claims: &crate::auth::Claims,
    agent_id: Uuid,
) -> bool {
    let identity: nasiko_auth::Identity = claims.clone().into();
    state.auth.can_access_agent(&identity, &agent_id.to_string()).await
}

/// Owner-or-superuser check for **mutating / destructive** operations (update,
/// delete, rollback, secrets, invoke-ACL changes).
///
/// Deliberately stricter than [`can_access_agent`]: neither a public agent nor an
/// *invoke* grant (user/team/dept) may confer the right to modify or destroy the
/// agent. Using view-level access here would be an IDOR on public agents (RUN-9)
/// and would silently promote an invoke-grant into a manage-grant in EE. Mirrors
/// Python's `can_user_manage_agent` (owner ∨ admin) and EE `require_manage_agent`.
pub async fn can_manage_agent(
    state: &crate::state::AppState,
    claims: &crate::auth::Claims,
    agent_id: Uuid,
) -> bool {
    if claims.is_superuser {
        return true;
    }
    let Ok(user_id) = claims.user_uuid() else {
        return false;
    };
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND owner_id = $2 AND deleted_at IS NULL)",
    )
    .bind(agent_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false)
}

/// Check whether `caller_agent_id` is permitted to invoke `target_agent_id`.
///
/// Default-deny: both caller and target must have an explicit row in `agent_acl`.
/// To allow an agent to call another, insert a row:
///   `INSERT INTO agent_acl (caller_agent_id, target_agent_id) VALUES ($1, $2)`
pub async fn check_agent_acl(
    db: &PgPool,
    caller_agent_id: Uuid,
    target_agent_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let allowed: Option<bool> = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agent_acl
            WHERE caller_agent_id = $1 AND target_agent_id = $2
        )
        "#,
    )
    .bind(caller_agent_id)
    .bind(target_agent_id)
    .fetch_one(db)
    .await?;

    Ok(allowed.unwrap_or(false))
}

/// Fetch the set of agent IDs that `caller_agent_id` is explicitly allowed to invoke.
/// Returns an empty Vec when the caller has no grants (all calls denied by default).
pub async fn allowed_targets(
    db: &PgPool,
    caller_agent_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let targets: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT target_agent_id FROM agent_acl WHERE caller_agent_id = $1",
    )
    .bind(caller_agent_id)
    .fetch_all(db)
    .await?;

    Ok(targets.into_iter().map(|(id,)| id).collect())
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
                    .map_err(|e| {
                        warn!(%e, %caller_id, %target_id, "ACL check failed");
                        "ACL check failed".to_string()
                    })?;
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
            if tokens_used > 0
                && let Err(e) = self.flow_guard.record_tokens(&ctx, tokens_used).await {
                    warn!(%e, "flow token budget exceeded after call");
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
