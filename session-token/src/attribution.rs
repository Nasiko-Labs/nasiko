//! Per-turn flow attribution for session-token callers.
//!
//! A session token is minted once per sandbox, so its signed `flow_id` is the *first* turn's
//! flow, while every later turn runs under a new flow. Harness CLIs forward no `traceparent`,
//! so the flow is resolved on the server from the token's session. The signature still proves
//! the user and the agent, so billing is right; this decides which flow a call is *attributed*
//! to, in order:
//!
//! 1. the flow the request's `traceparent` names, when it belongs to the token's session, the
//!    token's user owns it and the token's agent is a participant of it;
//! 2. else the newest flow of the token's session owned by the same user with the same agent as
//!    a participant;
//! 3. else the claim.
//!
//! It never errors: a failed lookup or a mismatch costs attribution, not the call.

use sqlx::PgPool;
use uuid::Uuid;

use crate::SessionClaims;

/// Where an attributed flow id came from, recorded on the usage row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowSource {
    /// The request's `traceparent` named a flow the token's user owns.
    Traceparent,
    /// The newest flow of the token's session.
    Session,
    /// The token's signed `flow_id`.
    Claim,
}

impl FlowSource {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Traceparent => "traceparent",
            Self::Session => "session",
            Self::Claim => "claim",
        }
    }
}

/// The flow a session-token call is attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttributedFlow {
    pub flow_id: String,
    pub source: FlowSource,
}

/// Resolve the flow for one session-token call. `trace_flow` is the trace id the caller already
/// parsed from the `traceparent` header (each caller owns its parser).
pub async fn attributed_flow(
    db: &PgPool,
    trace_flow: Option<&str>,
    claims: &SessionClaims,
) -> AttributedFlow {
    let Some(identity) = SessionIdentity::parse(claims) else {
        return claim_flow(claims);
    };
    match lookup(db, trace_flow, &identity, &claims.session_id).await {
        Ok(found) => settle(trace_flow, found, &identity, claims),
        Err(error) => {
            tracing::warn!(%error, "session token: flow lookup failed; keeping the claim");
            claim_flow(claims)
        }
    }
}

struct SessionIdentity {
    user: Uuid,
    agent: Uuid,
}

impl SessionIdentity {
    fn parse(claims: &SessionClaims) -> Option<Self> {
        let user = Uuid::parse_str(&claims.user_id);
        let agent = Uuid::parse_str(&claims.agent_id);
        match (user, agent) {
            (Ok(user), Ok(agent)) => Some(Self { user, agent }),
            _ => {
                tracing::warn!(
                    user_id = %claims.user_id,
                    agent_id = %claims.agent_id,
                    "session token: user_id or agent_id is not a UUID; keeping the claim"
                );
                None
            }
        }
    }
}

/// What the database knows about the request's flow and the token's session.
#[derive(Debug)]
struct Lookup {
    /// `flows.user_id` of the traceparent's flow, if such a flow exists.
    trace_owner: Option<Uuid>,
    /// Whether the traceparent's flow is in the token's session with the token's agent a
    /// participant.
    trace_has_agent: bool,
    /// The session's newest flow for the token's user with the agent as participant.
    session_flow: Option<String>,
}

/// One round trip: the traceparent flow's owner and participant bit, and the session's newest
/// matching flow (`idx_flows_session`).
async fn lookup(
    db: &PgPool,
    trace_flow: Option<&str>,
    identity: &SessionIdentity,
    session_id: &str,
) -> Result<Lookup, sqlx::Error> {
    let (trace_owner, trace_has_agent, session_flow): (Option<Uuid>, bool, Option<String>) =
        sqlx::query_as(
            "SELECT
               (SELECT user_id FROM flows WHERE flow_id = $1::text),
               EXISTS(SELECT 1 FROM flows t
                        JOIN flow_participants tp ON tp.flow_id = t.flow_id
                       WHERE t.flow_id = $1::text AND t.session_id = $2 AND tp.agent_id = $4),
               (SELECT f.flow_id FROM flows f
                 WHERE f.session_id = $2 AND f.user_id = $3 AND f.status <> 'failed'
                   AND EXISTS(SELECT 1 FROM flow_participants fp
                               WHERE fp.flow_id = f.flow_id AND fp.agent_id = $4)
                 ORDER BY f.created_at DESC LIMIT 1)",
        )
        .bind(trace_flow)
        .bind(session_id)
        .bind(identity.user)
        .bind(identity.agent)
        .fetch_one(db)
        .await?;
    Ok(Lookup {
        trace_owner,
        trace_has_agent,
        session_flow,
    })
}

/// The decision, before logging.
#[derive(Debug, PartialEq, Eq)]
struct Decision {
    pick: Pick,
    /// A traceparent flow that exists but is not this user's (or not this agent's).
    rejected_trace: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum Pick {
    Traceparent(String),
    Session(String),
    Claim,
}

/// The pure decision table over what [`lookup`] found.
fn decide(trace_flow: Option<&str>, found: Lookup, token_user: Uuid) -> Decision {
    let mut rejected_trace = None;
    if let (Some(flow_id), Some(owner)) = (trace_flow, found.trace_owner) {
        if owner == token_user && found.trace_has_agent {
            return Decision {
                pick: Pick::Traceparent(flow_id.to_owned()),
                rejected_trace,
            };
        }
        rejected_trace = Some(flow_id.to_owned());
    }
    let pick = match found.session_flow {
        Some(flow_id) => Pick::Session(flow_id),
        None => Pick::Claim,
    };
    Decision {
        pick,
        rejected_trace,
    }
}

fn settle(
    trace_flow: Option<&str>,
    found: Lookup,
    identity: &SessionIdentity,
    claims: &SessionClaims,
) -> AttributedFlow {
    let decision = decide(trace_flow, found, identity.user);
    if let Some(trace_id) = &decision.rejected_trace {
        tracing::warn!(
            %trace_id, user_id = %claims.user_id, agent_id = %claims.agent_id,
            "session token: traceparent names a flow this user/agent does not own"
        );
    }
    let attributed = match decision.pick {
        Pick::Traceparent(flow_id) => AttributedFlow {
            flow_id,
            source: FlowSource::Traceparent,
        },
        Pick::Session(flow_id) => AttributedFlow {
            flow_id,
            source: FlowSource::Session,
        },
        Pick::Claim => claim_flow(claims),
    };
    if attributed.flow_id != claims.flow_id {
        tracing::debug!(
            flow_id = %attributed.flow_id, claim_flow_id = %claims.flow_id,
            source = ?attributed.source,
            "session token: attributing to a flow other than the claim"
        );
    }
    attributed
}

fn claim_flow(claims: &SessionClaims) -> AttributedFlow {
    AttributedFlow {
        flow_id: claims.flow_id.clone(),
        source: FlowSource::Claim,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

    fn found(owner: Option<Uuid>, has_agent: bool, session: Option<&str>) -> Lookup {
        Lookup {
            trace_owner: owner,
            trace_has_agent: has_agent,
            session_flow: session.map(str::to_owned),
        }
    }

    #[test]
    fn owned_flow_with_agent_participant_wins_over_the_session_flow() {
        let user = Uuid::new_v4();
        let d = decide(Some(TRACE), found(Some(user), true, Some("newest")), user);
        assert_eq!(d.pick, Pick::Traceparent(TRACE.into()));
        assert_eq!(d.rejected_trace, None);
    }

    #[test]
    fn foreign_owner_is_rejected_and_falls_to_the_session_flow() {
        let d = decide(
            Some(TRACE),
            found(Some(Uuid::new_v4()), true, Some("newest")),
            Uuid::new_v4(),
        );
        assert_eq!(d.pick, Pick::Session("newest".into()));
        assert_eq!(d.rejected_trace.as_deref(), Some(TRACE));
    }

    #[test]
    fn own_flow_without_the_agent_is_rejected() {
        let user = Uuid::new_v4();
        let d = decide(Some(TRACE), found(Some(user), false, None), user);
        assert_eq!(d.pick, Pick::Claim);
        assert_eq!(d.rejected_trace.as_deref(), Some(TRACE));
    }

    #[test]
    fn unknown_trace_or_none_uses_session_then_claim() {
        let user = Uuid::new_v4();
        let d = decide(Some(TRACE), found(None, false, Some("newest")), user);
        assert_eq!(d.pick, Pick::Session("newest".into()));
        assert_eq!(d.rejected_trace, None);
        assert_eq!(
            decide(None, found(None, false, None), user).pick,
            Pick::Claim
        );
    }

    #[test]
    fn labels_are_the_recorded_values() {
        assert_eq!(FlowSource::Traceparent.as_label(), "traceparent");
        assert_eq!(FlowSource::Session.as_label(), "session");
        assert_eq!(FlowSource::Claim.as_label(), "claim");
    }
}
