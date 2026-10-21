//! The one concrete [`crate::dispatcher::ResumeNotifier`] this milestone
//! ships: pushes a resolved decision into the paused agent by resolving its
//! live address via `ContainerRuntime::endpoint()` (the same primitive
//! `oss/server/src/agent_proxy.rs` and `a2a_dispatch.rs::resolve_endpoint`
//! already use) and making a fresh outbound A2A `SendMessage` call carrying
//! the row's `context_id`.
//!
//! There is no reusable event bus and no A2A push-notification support
//! anywhere in this codebase (confirmed in the M6 handoff investigation) —
//! the original `tools/call` HTTP response is long gone by the time a human
//! resolves a request, so this cannot "wake up" anything already listening.
//! It has to be a brand-new connection into the agent, exactly like this.

use std::sync::Arc;
use std::time::Duration;

use nasiko_runtime::{ContainerId, ContainerRuntime};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::dispatcher::{NotifyError, ResumeNotifier};
use crate::types::{DECISION_APPROVE, HitlKind, HitlRequest};

/// `flows.title` for the row `traceparent_for_context` registers — named here instead of inline
/// in the SQL text (nit from review).
const FLOW_TITLE: &str = "HITL resume nudge";
/// `flows.status` value meaning "live, authorized to call back into the platform" — see
/// `mcp/handlers/gateway.rs`'s `flow_user` check. Named here instead of inline in the SQL text
/// (nit from review) — `flows.status` is a bare `TEXT` column with no CHECK constraint or shared
/// enum type anywhere in the codebase yet, so this is scoped to this file's own two call sites,
/// not a claim that every `'running'` literal elsewhere should reference it.
const FLOW_STATUS_RUNNING: &str = "running";

/// Per-attempt timeout on the outbound nudge. Public because the claim lease has to outlast a whole
/// delivery — `dispatcher::run` derives its floor from this — and a lease shorter than the work it
/// covers lets the recovery sweep quarantine a delivery that is still in flight.
pub const RESUME_REQUEST_TIMEOUT_SECS: u64 = 300;

pub struct RuntimeResumeNotifier {
    db: PgPool,
    runtime: Arc<dyn ContainerRuntime>,
    http_client: reqwest::Client,
    /// Mirrors the platform's `NASIKO_FLOW_TIMEOUT_SECS`. Received, not re-read from the env, so
    /// this agrees with the exact bound `gateway.rs`'s `flow_user` enforces on the agent's retry.
    flow_timeout_secs: i64,
}

impl RuntimeResumeNotifier {
    pub fn new(
        db: PgPool,
        runtime: Arc<dyn ContainerRuntime>,
        http_client: reqwest::Client,
        flow_timeout_secs: i64,
    ) -> Self {
        Self {
            db,
            runtime,
            http_client,
            flow_timeout_secs,
        }
    }

    /// Resolve the agent's currently-reachable A2A endpoint. Mirrors
    /// `a2a_dispatch.rs::resolve_endpoint`'s live-runtime-first, stored-URL-
    /// fallback shape, simplified: this notifier only ever needs to *reach*
    /// the agent, not also flip `agents.status` — that bookkeeping belongs to
    /// the request-serving code path, not a background dispatcher.
    async fn resolve_agent_endpoint(&self, agent_id: Uuid) -> Result<String, NotifyError> {
        let row: Option<(Option<String>, Option<String>)> =
            sqlx::query_as("SELECT transport_path, url FROM agents WHERE id = $1")
                .bind(agent_id)
                .fetch_optional(&self.db)
                .await
                .map_err(|e| NotifyError::EndpointResolution {
                    agent_id,
                    reason: format!("db lookup failed: {e}"),
                    permanent: false,
                })?;

        let Some((transport_path, stored_url)) = row else {
            return Err(NotifyError::EndpointResolution {
                agent_id,
                reason: "no such agent".to_string(),
                permanent: true,
            });
        };

        // The A2A spec fixes no path — it must come from the agent's card,
        // never be assumed. See resolve_endpoint's identical reasoning.
        let path = match transport_path.as_deref() {
            None | Some("/") | Some("") => "",
            Some(p) => p,
        };

        let container_id = ContainerId::from_uuid(agent_id);
        if let Ok(live) = self.runtime.endpoint(&container_id).await {
            return Ok(format!("{}{path}", live.trim_end_matches('/')));
        }

        if let Some(url) = stored_url.filter(|u| !u.is_empty()) {
            return Ok(format!("{}{path}", url.trim_end_matches('/')));
        }

        Err(NotifyError::EndpointResolution {
            agent_id,
            reason: "no live or stored endpoint".to_string(),
            permanent: false,
        })
    }

    /// Build a `traceparent` for the resume nudge whose trace_id resolves back to `context_id`
    /// via `session::resolve_context_id` (`oss/mcp-gateway/src/session.rs`) on the agent's
    /// retried tool call — without this, the retry mints/forwards an unrelated trace_id, gets a
    /// different resolved context_id, and `claim_resolved_tool_approval`'s exact-match lookup
    /// fails, asking the user to approve the same already-approved action again (found in
    /// review; confirmed against the real matching logic).
    ///
    /// `resolve_context_id` returns `context_id` unchanged for a traceparent whose trace_id has
    /// no `session_traces` row — a valid (32 lowercase hex) trace_id equal to `context_id`
    /// itself needs no DB write at all. `context_id` is otherwise already a resolved
    /// `chat_sessions.session_id` (not a raw trace_id — `traceparent` requires exactly 32 hex
    /// chars, which a `session_id` never is), so a fresh trace_id is minted and mapped to it via
    /// `session_traces`, the same table `agent_proxy`'s normal request path populates.
    ///
    /// Also registers `trace_id` as a live flow (`flows` + `flow_participants`) — a completely
    /// separate lookup from `session_traces` above, and the one that actually gates whether the
    /// resumed agent can call back into the platform at all. The MCP gateway's `flow_user` check
    /// (`oss/server/src/mcp/handlers/gateway.rs`) resolves the caller's identity from exactly
    /// those two tables for the trace_id named in an inbound `traceparent`, requiring
    /// `flows.status = 'running'` plus a matching `flow_participants` row — neither of which
    /// `session_traces` satisfies. Every other place that mints a traceparent for an outbound
    /// call on a human's behalf registers both (`hitl/mod.rs::deliver`, `agent_proxy.rs`,
    /// `a2a_dispatch.rs`); this notifier was the one exception. Without it, the very retry this
    /// nudge exists to prompt — the agent re-attempting the tool call a human just approved —
    /// 403s with "traceparent does not resolve to a live flow", so the approval never actually
    /// takes effect.
    async fn traceparent_for_context(
        &self,
        context_id: &str,
        agent_id: Uuid,
        owner_user_id: Uuid,
    ) -> Result<String, NotifyError> {
        // Lowercase only, matching this function's own doc comment ("32 lowercase hex") and W3C
        // traceparent's own requirement — `is_ascii_hexdigit()` alone also accepts `A-F`, which
        // would embed an uppercase-hex `context_id` verbatim into the outbound `traceparent`
        // header and into `flows.flow_id`, where the receiving side's parse/lookup won't match
        // (found in review).
        let is_raw_trace_id = context_id.len() == 32
            && context_id
                .chars()
                .all(|c| matches!(c, '0'..='9' | 'a'..='f'));
        let trace_id = if is_raw_trace_id {
            context_id.to_string()
        } else {
            let trace_id = Uuid::new_v4().simple().to_string();
            // Defense in depth (review finding): only map `trace_id` to
            // `context_id` when `context_id` actually names a chat session
            // owned by THIS HITL row's owner (`owner_user_id`) — the
            // `WHERE EXISTS` makes the insert a no-op otherwise. On the
            // normal path (a row persisted from a verified flow —
            // `oss/mcp-gateway/src/session.rs::resolve_context_id` only ever
            // hands back a session belonging to the caller it resolved a
            // context for) this always matches; it only ever fires if some
            // other path ever persisted a `context_id` naming a different
            // user's session. A zero-row affect is not a no-op to shrug at
            // (mirrors the `flows` registration guard below, same reasoning):
            // continuing to deliver the nudge anyway does NOT merely leave a
            // `traceparent` with no live flow behind it — the registration
            // below runs unconditionally and would still open one, keyed on
            // this fresh `trace_id` and attributed to `owner_user_id`, over a
            // context that user was never granted access to. And since no
            // `session_traces` row now maps `trace_id` back to the real
            // `context_id`, the agent's retry resolves its own traceparent to
            // `trace_id` itself (`resolve_context_id`'s no-mapping fallback),
            // which never matches the HITL row's actual `context_id` — so
            // `claim_resolved_tool_approval`'s exact-match lookup fails and
            // the human is asked to approve the same action again. Fail
            // loudly instead: never open that flow when the ownership check
            // itself failed.
            match sqlx::query(
                "INSERT INTO session_traces (session_id, trace_id, agent_id)
                 SELECT $1, $2, $3
                 WHERE EXISTS (
                     SELECT 1 FROM chat_sessions WHERE session_id = $1 AND user_id = $4
                 )",
            )
            .bind(context_id)
            .bind(&trace_id)
            .bind(agent_id)
            .bind(owner_user_id)
            .execute(&self.db)
            .await
            {
                Ok(result) if result.rows_affected() == 0 => {
                    tracing::warn!(
                        %context_id, %owner_user_id,
                        "resume nudge aborted: context_id does not name a chat session owned by \
                         this HITL row's owner (should never happen for a row persisted through \
                         the normal verified-flow path)"
                    );
                    return Err(NotifyError::ContextNotOwned {
                        context_id: context_id.to_string(),
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        %context_id, error = %e,
                        "failed to record session_traces mapping for resume nudge traceparent"
                    );
                }
            }
            trace_id
        };

        // On the fresh-trace-id path this is a plain INSERT, so `created_at` is now and the window
        // is live. On the `is_raw_trace_id` path it collides with the ORIGINAL call's own flow row,
        // whose `created_at` is fixed at that call — and `gateway.rs`'s `flow_user` requires
        // `created_at > now() - flow_timeout_secs` as well as `status = 'running'`. Three things
        // follow, all found in review:
        //
        //   * The `DO UPDATE` is scoped by that same age bound. Without it, flipping `status` back
        //     to `running` re-opened a closed `/api/mcp` window for an unrelated, already-finished
        //     flow — and for every agent in its `flow_participants`, not just the one being nudged.
        //   * It is ALSO scoped to `flows.user_id = $2` (this HITL row's own `owner_user_id`) —
        //     without this, a `context_id` that happens to equal some OTHER live flow's trace id
        //     (reachable if a HITL row's `context_id` were ever seeded from an unverified trace id
        //     — see `oss/mcp-gateway/src/session.rs::resolve_context_id`'s own doc comment) would
        //     have this UPDATE flip that flow back to `running` and the `INSERT INTO
        //     flow_participants` below add THIS agent as one of its participants — after which
        //     `gateway.rs`'s `flow_user` would resolve this agent's calls as acting for that
        //     flow's actual (different) user. Scoping to the row's own owner means a mismatched
        //     conflict updates nothing, same as the age bound.
        //   * When either bound excludes the row, `ON CONFLICT DO UPDATE ... WHERE` updates nothing
        //     and returns no row. That is not a no-op to shrug at: it means the retry this nudge
        //     exists to prompt cannot be authorized, so the nudge must fail loudly rather than be
        //     delivered as if healthy (the agent would 403 with "traceparent does not resolve to a
        //     live flow" and the human's approval would silently do nothing).
        let registered: Option<(String,)> = match sqlx::query_as(
            r#"INSERT INTO flows (flow_id, user_id, root_agent_id, title, status)
               VALUES ($1, $2, $3, $4, $5)
               ON CONFLICT (flow_id) DO UPDATE SET status = $5
                 WHERE flows.created_at > now() - make_interval(secs => $6)
                   AND flows.user_id = $2
               RETURNING flow_id"#,
        )
        .bind(&trace_id)
        .bind(owner_user_id)
        .bind(agent_id)
        .bind(FLOW_TITLE)
        .bind(FLOW_STATUS_RUNNING)
        .bind(self.flow_timeout_secs as f64)
        .fetch_optional(&self.db)
        .await
        {
            Ok(row) => row,
            Err(e) => {
                tracing::warn!(
                    error = %e, %trace_id,
                    "failed to register resume-nudge flow — the agent's retry may 403 with \
                     'traceparent does not resolve to a live flow'"
                );
                None
            }
        };
        if registered.is_none() {
            tracing::warn!(
                %trace_id, %context_id, %owner_user_id, timeout_secs = self.flow_timeout_secs,
                "resume nudge aborted: either the flow this context names is older than the \
                 platform's flow timeout, or it belongs to a different user than this HITL \
                 row's owner — either way the agent's retried tool call could not be authorized"
            );
            return Err(NotifyError::FlowNotLive {
                context_id: context_id.to_string(),
            });
        }
        if let Err(e) = sqlx::query(
            "INSERT INTO flow_participants (flow_id, agent_id) VALUES ($1, $2)
             ON CONFLICT (flow_id, agent_id) DO NOTHING",
        )
        .bind(&trace_id)
        .bind(agent_id)
        .execute(&self.db)
        .await
        {
            tracing::warn!(
                error = %e, %trace_id,
                "failed to register resume-nudge flow participant — the agent's retry may 403 \
                 with 'traceparent does not resolve to a live flow'"
            );
        }

        let span_id: String = Uuid::new_v4().as_bytes()[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(format!("00-{trace_id}-{span_id}-01"))
    }
}

#[async_trait::async_trait]
impl ResumeNotifier for RuntimeResumeNotifier {
    async fn notify(&self, request: &HitlRequest) -> Result<(), NotifyError> {
        let context_id = request
            .context_id
            .as_deref()
            .ok_or(NotifyError::MissingContextId(request.id))?;

        let endpoint = self.resolve_agent_endpoint(request.agent_id).await?;
        let message = build_resume_message(request);
        let body = nasiko_types::a2a::build_send_request(&message, Some(context_id));
        let traceparent = self
            .traceparent_for_context(context_id, request.agent_id, request.owner_user_id)
            .await?;

        // 300s, not `http_client`'s shared 60s default — matches `hitl/mod.rs::deliver`'s own
        // override for the equivalent "make an outbound A2A call on a human's behalf" work.
        // Without it, a legitimately slow-but-healthy agent turn (well under the 300s the OTHER
        // dispatcher tolerates for the same kind of call) reads as a transport failure here,
        // retries the nudge up to `max_attempts` times, and since `claim_resolved_tool_approval`
        // already consumed the row on the first (successful) delivery, the human ends up
        // re-prompted for an action that already executed.
        let response = self
            .http_client
            .post(&endpoint)
            .timeout(Duration::from_secs(RESUME_REQUEST_TIMEOUT_SECS))
            .header("A2A-Version", nasiko_types::a2a::A2A_VERSION_HEADER_VALUE)
            .header("traceparent", traceparent)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let payload: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() || payload.get("error").is_some() {
            return Err(NotifyError::PeerError(format!("http {status}: {payload}")));
        }

        Ok(())
    }
}

/// Build a human-legible, kind-agnostic nudge from a resolved row's own
/// `question`/`human_response` — deliberately free-form (both columns are
/// untyped JSONB, per their own doc comments) rather than assuming an
/// MCP-specific schema, so this stays reusable for a future non-MCP origin.
fn build_resume_message(request: &HitlRequest) -> String {
    let label = request
        .tool_name
        .as_deref()
        .or_else(|| request.question.get("tool_name").and_then(Value::as_str))
        .unwrap_or("the previously blocked action");

    match request.kind {
        // `auth_required`/`mcp_tool` rows have no `tool_name` (the schema
        // only carries it for `tool_approval` — see `chk_hitl_mcp_auth_required_identity`
        // vs `chk_hitl_tool_approval_identity`), so `label` above is always
        // its generic fallback here — confirmed live: a real deployed agent,
        // forced to call some tool by this vague a nudge, called an
        // unrelated tool of its own instead of retrying the right one. Name
        // the connector instead — the one piece of real identity every
        // `auth_required` row's `question` does carry
        // (`handle_auth_required`'s own construction) — so the receiving
        // agent has an actual anchor instead of a placeholder that reads
        // like a real tool name but never is one.
        HitlKind::AuthRequired => {
            let connector = request
                .question
                .get("connector")
                .and_then(Value::as_str)
                .unwrap_or("the connector");
            format!(
                "Authentication for the `{connector}` connector has been completed. \
                 You may retry whichever tool call needed it now."
            )
        }
        HitlKind::ToolApproval => {
            let approved = request
                .human_response
                .as_ref()
                .and_then(|r| r.get("decision"))
                .and_then(Value::as_str)
                == Some(DECISION_APPROVE);
            if approved {
                format!(
                    "The user approved your request to use `{label}`. You may retry the tool call now."
                )
            } else {
                format!(
                    "The user denied your request to use `{label}`. Do not retry this tool call — \
                     inform the user or choose a different approach."
                )
            }
        }
        HitlKind::InputRequired => {
            "The user has responded to your request for input. Continue the task.".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{HitlOrigin, HitlStatus, ResumeStatus};
    use chrono::Utc;

    fn request(
        kind: HitlKind,
        human_response: Option<Value>,
        tool_name: Option<&str>,
    ) -> HitlRequest {
        request_with_question(kind, human_response, tool_name, serde_json::json!({}))
    }

    fn request_with_question(
        kind: HitlKind,
        human_response: Option<Value>,
        tool_name: Option<&str>,
        question: Value,
    ) -> HitlRequest {
        let now = Utc::now();
        HitlRequest {
            id: Uuid::new_v4(),
            kind,
            origin: HitlOrigin::McpTool,
            status: HitlStatus::Resolved,
            resume_status: ResumeStatus::NotStarted,
            agent_id: Uuid::new_v4(),
            owner_user_id: Uuid::new_v4(),
            resolved_by: Some(Uuid::new_v4()),
            task_id: None,
            context_id: Some("ctx-1".to_string()),
            chat_session_id: None,
            maf_execution_id: None,
            maf_step_index: None,
            connector_id: Some(Uuid::new_v4()),
            tool_name: tool_name.map(str::to_string),
            arguments_hash: None,
            consumed_at: None,
            question,
            human_response,
            resume_state: serde_json::json!({}),
            resume_claimed_at: Some(now),
            resume_dispatch_attempts: 1,
            resume_last_error: None,
            created_at: now,
            updated_at: now,
            expires_at: None,
            resolved_at: Some(now),
        }
    }

    #[test]
    fn auth_required_message_names_the_connector_not_a_nonexistent_tool_name() {
        // `auth_required`/`mcp_tool` rows never have `tool_name` set (only
        // `tool_approval` rows do) — this must not fall back to the generic
        // placeholder when `question.connector` is available, the way a
        // real `handle_auth_required`-created row always has it.
        let req = request_with_question(
            HitlKind::AuthRequired,
            None,
            None,
            serde_json::json!({"connector": "github", "connector_id": Uuid::new_v4()}),
        );
        let msg = build_resume_message(&req);
        assert!(msg.contains("Authentication"));
        assert!(
            msg.contains("`github`"),
            "must name the actual connector, not a placeholder: {msg}"
        );
    }

    #[test]
    fn auth_required_message_falls_back_gracefully_with_no_connector_in_question() {
        let req = request(HitlKind::AuthRequired, None, None);
        let msg = build_resume_message(&req);
        assert!(msg.contains("Authentication"));
        assert!(msg.contains("the connector"));
    }

    #[test]
    fn tool_approval_approved_message_says_retry() {
        let req = request(
            HitlKind::ToolApproval,
            Some(serde_json::json!({"decision": "approve"})),
            Some("GITHUB_DELETE_REPO"),
        );
        let msg = build_resume_message(&req);
        assert!(msg.contains("approved"));
        assert!(msg.contains("retry"));
    }

    #[test]
    fn tool_approval_denied_message_says_do_not_retry() {
        let req = request(
            HitlKind::ToolApproval,
            Some(serde_json::json!({"decision": "reject"})),
            Some("GITHUB_DELETE_REPO"),
        );
        let msg = build_resume_message(&req);
        assert!(msg.contains("denied"));
        assert!(msg.contains("Do not retry"));
    }
}
