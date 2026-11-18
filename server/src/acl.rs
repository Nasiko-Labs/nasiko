use std::pin::Pin;
use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::warn;
use uuid::Uuid;

use nasiko_hitl::{AgentTargetResolver, ResolveCaller};
use nasiko_react_agent::{BeforeCallFuture, CallGuard};
use nasiko_types::agent_call::{AgentCallHandle, AgentCallMeter, CallTarget};

use nasiko_flow::{FlowContext, FlowGuard};

use crate::agent_wake::AgentWake;
use crate::spend_gate::SpendGate;

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
    state
        .auth
        .can_access_agent(&identity, &agent_id.to_string())
        .await
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

/// True if the caller can manage `connector_id`'s per-agent state on
/// `agent_id` — either full agent management ([`can_manage_agent`]), or the
/// caller can reach `connector_id` at all (Layer 1: owner, composio,
/// user/public grant — EE additionally: team/department) AND either they
/// **own** the connector and it is attached to this live agent
/// (`grant_type = 'agent'`, see `connector grant-agent`) OR they have at
/// least ordinary, non-managing access to the agent itself
/// ([`can_access_agent`] — owner, public, or a plain `agent_grants` share).
///
/// The first branch lets a connector's owner manage their own connector on
/// an agent they don't otherwise have access to, once it is attached there
/// (the owner may attach it to any agent). It is owner-only: someone the
/// connector was merely shared with — every user, for a public connector —
/// gets nothing from the attachment and must pass [`can_access_agent`].
/// The second branch lets a caller given both the connector and the agent
/// attach the connector for the very first time, before any attachment
/// exists. A caller with no relationship to the agent, who doesn't own the
/// connector, is refused, as is anyone acting on a deleted agent. It grants
/// no control over the agent's other connectors or its agent-wide settings
/// (reset, full connector/rule listing stay [`can_manage_agent`]-only).
pub async fn can_manage_agent_connector(
    state: &crate::state::AppState,
    claims: &crate::auth::Claims,
    agent_id: Uuid,
    connector_id: Uuid,
) -> bool {
    if can_manage_agent(state, claims, agent_id).await {
        return true;
    }
    let Ok(user_id) = claims.user_uuid() else {
        return false;
    };
    let can_reach_connector = state
        .mcp
        .authorizer
        .can_access_connector(&state.mcp.db, user_id, connector_id)
        .await
        .unwrap_or(false);
    if !can_reach_connector {
        return false;
    }
    if owns_connector_attached_to_live_agent(state, user_id, agent_id, connector_id).await {
        return true;
    }
    can_access_agent(state, claims, agent_id).await
}

/// True if `user_id` owns `connector_id`, it is attached to `agent_id`
/// (`grant_type = 'agent'`), and the agent is not deleted. A database error
/// reads as `false`.
async fn owns_connector_attached_to_live_agent(
    state: &crate::state::AppState,
    user_id: Uuid,
    agent_id: Uuid,
    connector_id: Uuid,
) -> bool {
    let owns_connector = nasiko_mcp_gateway::repo::get_connector_by_id(&state.mcp.db, connector_id)
        .await
        .ok()
        .flatten()
        .is_some_and(|connector| connector.owner_id == Some(user_id));
    if !owns_connector {
        return false;
    }
    let attached =
        nasiko_mcp_gateway::repo::agent_has_connector_grant(&state.db, agent_id, connector_id)
            .await
            .unwrap_or(false);
    if !attached {
        return false;
    }
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM agents WHERE id = $1 AND deleted_at IS NULL)",
    )
    .bind(agent_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(false)
}

/// Check whether `caller_agent_id` is permitted to invoke `target_agent_id`.
///
/// An allowlist, as the platform documents it: a caller with no `agent_acl` rows is
/// unrestricted; a caller with any rows may invoke only the targets listed. To restrict an
/// agent, insert a row per target it may call:
///   `INSERT INTO agent_acl (caller_agent_id, target_agent_id) VALUES ($1, $2)`
pub async fn check_agent_acl(
    db: &PgPool,
    caller_agent_id: Uuid,
    target_agent_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let allowed: Option<bool> = sqlx::query_scalar(
        r#"
        SELECT NOT EXISTS (SELECT 1 FROM agent_acl WHERE caller_agent_id = $1)
            OR EXISTS (
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

/// The agent a request is made on behalf of, when an authenticated agent (not a user) is the
/// one calling. Set as a request extension by whatever authenticates the agent; the proxy
/// enforces `agent_acl` against it before waking or forwarding to the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallingAgent(pub Uuid);

/// [`check_agent_acl`] as a proxy gate: `403` when the allowlist excludes the target, `500`
/// when it cannot be read (fail closed).
pub async fn require_agent_acl(
    db: &PgPool,
    caller_agent_id: Uuid,
    target_agent_id: Uuid,
) -> Result<(), axum::http::StatusCode> {
    match check_agent_acl(db, caller_agent_id, target_agent_id).await {
        Ok(true) => Ok(()),
        Ok(false) => {
            warn!(%caller_agent_id, %target_agent_id, "agent ACL denied the call");
            Err(axum::http::StatusCode::FORBIDDEN)
        }
        Err(e) => {
            warn!(%e, %caller_agent_id, %target_agent_id, "agent ACL check failed");
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Fetch the set of agent IDs that `caller_agent_id` is explicitly allowed to invoke.
/// An empty Vec means the caller has no allowlist, so it is unrestricted (see [`check_agent_acl`]).
pub async fn allowed_targets(db: &PgPool, caller_agent_id: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    let targets: Vec<(Uuid,)> =
        sqlx::query_as("SELECT target_agent_id FROM agent_acl WHERE caller_agent_id = $1")
            .bind(caller_agent_id)
            .fetch_all(db)
            .await?;

    Ok(targets.into_iter().map(|(id,)| id).collect())
}

/// CallGuard implementation for the CP orchestrator.
/// Enforces ACL, flow depth/fan-out/cycle/token limits before each agent call, and counts each
/// call that goes ahead as in flight to its target until the orchestrator drops the handle.
pub struct CpCallGuard {
    pub db: PgPool,
    pub flow_guard: FlowGuard,
    pub flow_ctx: Mutex<FlowContext>,
    pub caller_agent_id: Option<Uuid>,
    /// Asked before each of the orchestrator's own model calls, for this user and chat.
    pub spend_gate: Arc<dyn SpendGate>,
    pub user_id: Uuid,
    pub chat_session_id: String,
    pub agent_calls: Arc<dyn AgentCallMeter>,
    /// The agent wake seam; asked just before each call that passes the flow checks. The leg's
    /// `targets` resolver must not ask it again (`ServerTargetResolver::after_caller_wake`).
    pub agent_wake: Arc<dyn AgentWake>,
    /// Finds (and, for a parked agent, wakes) where each admitted call is sent.
    pub targets: Arc<dyn AgentTargetResolver>,
    /// The agents this turn may call, by the exact name the loop hands the guard.
    pub candidates: TurnCandidates,
}

/// What a routed turn needs to resolve each leg it admits.
pub struct TurnLegs {
    /// Finds (and, for a parked agent, wakes) where each admitted call is sent.
    pub targets: Arc<dyn AgentTargetResolver>,
    /// The user the turn is for.
    pub user_id: Uuid,
    /// The agents the turn offered the model.
    pub candidates: TurnCandidates,
}

/// The access-filtered agents a routed turn offered the model, keyed by the display name the ReAct
/// loop passes to [`CallGuard::before_call`] (`A2aTool::agent_display_name`). A leg is resolved
/// from this table by id; there is no lookup by name in the database, where a name is only unique
/// per owner. Two candidates that fold to the same display name cannot be told apart by a call, so
/// neither is in the table: a call to that name is refused as ambiguous, and the turn should not
/// offer them at all ([`is_ambiguous`](Self::is_ambiguous)).
#[derive(Clone, Debug, Default)]
pub struct TurnCandidates {
    by_name: std::collections::HashMap<String, Uuid>,
    ambiguous: std::collections::HashSet<String>,
}

impl TurnCandidates {
    pub fn new<'a>(agents: impl IntoIterator<Item = (&'a str, Uuid)>) -> Self {
        let mut candidates = Self::default();
        for (name, id) in agents {
            let key = nasiko_react_agent::A2aTool::agent_display_name(name);
            if let Some(first) = candidates.by_name.insert(key.clone(), id) {
                warn!(display = %key, %first, second = %id, "routed turn: two candidates share a display name; neither is callable");
                candidates.ambiguous.insert(key);
            }
        }
        for key in &candidates.ambiguous {
            candidates.by_name.remove(key);
        }
        candidates
    }

    /// The candidate a call to `display_name` (as the loop spells it) reaches.
    pub fn get(&self, display_name: &str) -> Option<Uuid> {
        self.by_name.get(display_name).copied()
    }

    /// Whether `agent_name` shares its display name with another candidate.
    pub fn is_ambiguous(&self, agent_name: &str) -> bool {
        self.ambiguous
            .contains(&nasiko_react_agent::A2aTool::agent_display_name(agent_name))
    }
}

impl CpCallGuard {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: PgPool,
        flow_guard: FlowGuard,
        flow_ctx: FlowContext,
        caller_agent_id: Option<Uuid>,
        spend_gate: Arc<dyn SpendGate>,
        user_id: Uuid,
        chat_session_id: String,
        agent_calls: Arc<dyn AgentCallMeter>,
        agent_wake: Arc<dyn AgentWake>,
        legs: TurnLegs,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            flow_guard,
            flow_ctx: Mutex::new(flow_ctx),
            caller_agent_id,
            spend_gate,
            user_id,
            chat_session_id,
            agent_calls,
            agent_wake,
            targets: legs.targets,
            candidates: legs.candidates,
        })
    }
}

impl CallGuard for CpCallGuard {
    fn before_call(&self, target_agent: &str) -> BeforeCallFuture<'_> {
        let target = target_agent.to_string();
        Box::pin(async move {
            // Looked up by the exact id of the candidate the turn was offered: a name alone is
            // unique only per owner, so a name lookup could reach another owner's agent.
            let Some(target_id) = self.candidates.get(&target) else {
                return Err(if self.candidates.ambiguous.contains(&target) {
                    format!("agent '{target}' is ambiguous: another candidate has the same name")
                } else {
                    format!("agent '{target}' is not one of this turn's candidates")
                });
            };

            // ACL check: if the caller has an allowlist, verify target is in it
            if let Some(caller_id) = self.caller_agent_id {
                let allowed = check_agent_acl(&self.db, caller_id, target_id)
                    .await
                    .map_err(|e| {
                        warn!(%e, %caller_id, %target_id, "ACL check failed");
                        "ACL check failed".to_string()
                    })?;
                if !allowed {
                    return Err(format!(
                        "agent ACL denied: caller cannot invoke '{}'",
                        target
                    ));
                }
            }

            // Keyed on the id, as the proxy keys it, so cycle detection agrees across both.
            let target_key = target_id.to_string();
            // The lock is held only to copy the context: nothing below waits on it.
            let ctx = self.flow_ctx.lock().await.clone();

            // Flow guard: depth, cycle, fan-out, token budget, timeout (all in Redis)
            if let Err(rejection) = self.flow_guard.check(&ctx, &target_key).await {
                return Err(rejection.to_string());
            }

            // After the checks and before `record_invocation`: a refused call spends no fan-out
            // or depth budget. The candidate lookup above already resolved the target UUID.
            self.agent_wake
                .before_forward(target_id, Some(&ctx.flow_id))
                .await?;

            if let Err(rejection) = self.flow_guard.record_invocation(&ctx, &target_key).await {
                return Err(rejection.to_string());
            }

            // The call is going ahead — record the target as a flow participant
            // (orchestrator cascade legs have no `flows`-insert of their own).
            // Load-bearing for MCP gateway / LLM router authorization.
            crate::flows::record_participant(&self.db, &ctx.flow_id, target_id).await;
            // Past every gate: resolve where the call goes, waking a parked agent. Only the
            // agent the model chose is woken, and every leg stamps its request time.
            let caller = ResolveCaller {
                user_id: Some(self.user_id),
                flow_id: Some(ctx.flow_id.clone()),
            };
            let resolved = match self.targets.resolve(target_id, &caller, None).await {
                Ok(resolved) => resolved,
                Err(failure) => {
                    // The invocation was recorded above and no handle will ever be dropped to
                    // balance it, so unwind it here.
                    self.flow_guard.record_return(&ctx).await;
                    return Err(format!("agent '{target}' unavailable: {failure}"));
                }
            };
            Ok(Box::new(ResolvedCall {
                inner: self.agent_calls.begin(target_id),
                target: CallTarget {
                    url: resolved.url,
                    headers: resolved.headers,
                },
            }) as Box<dyn AgentCallHandle>)
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
                && let Err(e) = self.flow_guard.record_tokens(&ctx, tokens_used).await
            {
                warn!(%e, "flow token budget exceeded after call");
            }
        })
    }

    fn before_completion(
        &self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async move {
            match self
                .spend_gate
                .refuse(self.user_id, &self.chat_session_id)
                .await
            {
                Some(reason) => Err(reason),
                None => Ok(()),
            }
        })
    }
}

/// An admitted call's in-flight handle, which also carries the address the call must be sent to.
struct ResolvedCall {
    inner: Box<dyn AgentCallHandle>,
    target: CallTarget,
}

impl AgentCallHandle for ResolvedCall {
    fn set_succeeded(&mut self, succeeded: bool) {
        self.inner.set_succeeded(succeeded);
    }

    fn target(&self) -> Option<CallTarget> {
        Some(self.target.clone())
    }
}
