//! Flow attribution — which user conversation an LLM call belongs to.
//!
//! The router never sees the caller's identity on the wire: an agent's only
//! credential is its agent-identity JWT, which names the agent/owner but never
//! the *user* whose message triggered the call. Per-user attribution requires
//! the agent to propagate the W3C `traceparent` the platform injected on the
//! inbound A2A call — the one header OTel auto-instrumentation forwards for
//! free (Python/Node/Java/.NET), and a small manual injection for Rust/Go.
//!
//! There is exactly one path — **`traceparent`, resolved, or rejected**
//! (`oss/docs/TOKEN_ATTRIBUTION.md`, "Strict enforcement"):
//!
//! 1. The header's trace id names a live `flows` row; billing goes to that
//!    flow's caller (`flows.user_id`), never the agent's owner.
//! 2. The JWT-authenticated agent must be a recorded `flow_participants`
//!    member of that flow — without this, agent A could name agent B's flow
//!    and drain another user's token budget or misbill them.
//! 3. Anything else — header absent, malformed, trace id with no live flow,
//!    a flow the agent is not part of, or the identity store being down — is
//!    a [`AttributionDenied`], surfaced as `403` by the handlers. No guessing,
//!    no fallback: unattributed usage would burn tokens outside every cascade
//!    limit, so the call is refused before any tokens are spent.
//!
//! The old active-flow fallback (attribute to the agent's sole running flow)
//! is removed, not disabled: a fuzzy fallback behind a strict gate defeats
//! the gate.
//!
//! The resolved flow drives both the usage row (per-user billing via
//! `flows.user_id`, session grouping) and the model-routing boundary signals.

use uuid::Uuid;

use super::Mode;
use crate::resolver::RegistryStore;

/// How the flow was found — recorded in the usage row's metadata. Under strict
/// enforcement `Traceparent` is the only way a served call can be attributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionSource {
    /// The agent forwarded `traceparent`; the trace id named the flow directly.
    Traceparent,
}

impl AttributionSource {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Traceparent => "traceparent",
        }
    }
}

/// The flow an LLM call was attributed to, plus the billing/grouping context
/// it carries.
#[derive(Debug, Clone)]
pub struct FlowAttribution {
    pub flow_id: String,
    /// The chatting user (`flows.user_id`) — the one who pays, NOT the agent
    /// owner the JWT names.
    pub user_id: Option<Uuid>,
    /// Stable conversation key (`flows.metadata->>'context_id'`); the
    /// decision-cache/session grouping key.
    pub context_id: Option<String>,
    pub mode: Mode,
    pub source: AttributionSource,
    /// The end user's raw message that opened the flow
    /// (`flows.metadata->>'user_message'`) — the salience gate's and classifier's input,
    /// independent of how the agent's framework wraps the prompt it sends to the LLM.
    pub user_message: Option<String>,
}

/// A live flow as fetched by [`RegistryStore::fetch_live_flow`] — the flow's
/// billing context plus whether the calling agent is a recorded participant.
#[derive(Debug, Clone)]
pub struct LiveFlow {
    pub user_id: Option<Uuid>,
    pub context_id: Option<String>,
    pub mode: Option<String>,
    pub user_message: Option<String>,
    /// Whether the calling agent has a `flow_participants` row for this flow.
    pub agent_is_participant: bool,
}

/// Why an LLM call could not be attributed — every variant is a hard `403`.
/// The `Display` text is the wire body (descriptive by design: the most common
/// cause is a silently-dropped malformed traceparent, and a bare 403 there is
/// miserable to debug).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributionDenied {
    /// No traceparent header, or one whose trace id is malformed/all-zero.
    MissingTraceparent,
    /// The trace id resolved to no live `flows` row.
    UnknownFlow { flow_id: String },
    /// The flow exists but the calling agent was never dispatched into it.
    NotParticipant { flow_id: String, agent_id: String },
    /// The JWT's agent id is not a UUID — cannot be checked against
    /// `flow_participants`, so it cannot be authorized.
    InvalidAgentId { agent_id: String },
    /// Identity store unreachable — fail closed, never guess.
    StoreUnavailable,
}

impl std::fmt::Display for AttributionDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingTraceparent => write!(
                f,
                "traceparent missing or malformed — every LLM call must carry the W3C trace \
                 context of the flow it serves (forwarded automatically by OTel \
                 auto-instrumentation; Rust/Go agents inject it manually)"
            ),
            Self::UnknownFlow { flow_id } => write!(
                f,
                "traceparent does not resolve to a live flow (trace_id {flow_id})"
            ),
            Self::NotParticipant { flow_id, agent_id } => {
                write!(f, "agent {agent_id} is not a participant of flow {flow_id}")
            }
            Self::InvalidAgentId { agent_id } => {
                write!(f, "agent id {agent_id:?} in the JWT is not a UUID")
            }
            Self::StoreUnavailable => write!(f, "identity store unavailable"),
        }
    }
}

/// Resolve the flow this LLM call belongs to, or deny it. `trace_flow` is the
/// trace id parsed from the agent-forwarded `traceparent`, when present and
/// well-formed. `window_secs` bounds flow liveness (a `running` row whose
/// completion marking never ran ages out of attribution).
pub async fn resolve(
    store: &dyn RegistryStore,
    agent_id: &str,
    trace_flow: Option<String>,
    window_secs: i64,
) -> Result<FlowAttribution, AttributionDenied> {
    let Some(flow_id) = trace_flow else {
        return Err(AttributionDenied::MissingTraceparent);
    };
    let Ok(agent_uuid) = Uuid::parse_str(agent_id) else {
        return Err(AttributionDenied::InvalidAgentId {
            agent_id: agent_id.to_string(),
        });
    };

    let flow = store
        .fetch_live_flow(&flow_id, agent_uuid, window_secs)
        .await
        .map_err(|e| {
            tracing::warn!(
                target: "nasiko::llm_router::attribution",
                error = %e, %flow_id, "attribution: flow lookup failed — failing closed"
            );
            AttributionDenied::StoreUnavailable
        })?;

    let Some(flow) = flow else {
        return Err(AttributionDenied::UnknownFlow { flow_id });
    };
    if !flow.agent_is_participant {
        return Err(AttributionDenied::NotParticipant {
            flow_id,
            agent_id: agent_id.to_string(),
        });
    }

    Ok(FlowAttribution {
        flow_id,
        user_id: flow.user_id,
        context_id: flow.context_id,
        mode: flow
            .mode
            .as_deref()
            .map(Mode::from_label)
            .unwrap_or(Mode::FreeFlowing),
        source: AttributionSource::Traceparent,
        user_message: flow.user_message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::{AgentConfigResult, RegistryStore};
    use async_trait::async_trait;

    struct Store {
        flow: Option<LiveFlow>,
        fail: bool,
    }

    #[async_trait]
    impl RegistryStore for Store {
        async fn fetch_llm_config(
            &self,
            _: Uuid,
        ) -> Result<Option<AgentConfigResult>, sqlx::Error> {
            unreachable!("attribution never touches llm_config")
        }
        async fn fetch_user_secret(&self, _: Uuid, _: &str) -> Result<Option<String>, sqlx::Error> {
            unreachable!("attribution never touches secrets")
        }
        async fn fetch_live_flow(
            &self,
            _: &str,
            _: Uuid,
            _: i64,
        ) -> Result<Option<LiveFlow>, sqlx::Error> {
            if self.fail {
                return Err(sqlx::Error::PoolTimedOut);
            }
            Ok(self.flow.clone())
        }
        async fn fetch_custom_provider(
            &self,
            _: &str,
        ) -> Result<Option<crate::resolver::CustomProvider>, sqlx::Error> {
            Ok(None)
        }
    }

    const AGENT: &str = "11111111-1111-1111-1111-111111111111";

    fn live_flow(participant: bool) -> LiveFlow {
        LiveFlow {
            user_id: Some(Uuid::new_v4()),
            context_id: Some("ses_1".into()),
            mode: Some("free_flowing".into()),
            user_message: Some("hi".into()),
            agent_is_participant: participant,
        }
    }

    #[tokio::test]
    async fn missing_traceparent_is_denied() {
        let store = Store {
            flow: Some(live_flow(true)),
            fail: false,
        };
        let err = resolve(&store, AGENT, None, 300).await.unwrap_err();
        assert_eq!(err, AttributionDenied::MissingTraceparent);
    }

    #[tokio::test]
    async fn unknown_flow_is_denied() {
        let store = Store {
            flow: None,
            fail: false,
        };
        let err = resolve(&store, AGENT, Some("f1".into()), 300)
            .await
            .unwrap_err();
        assert_eq!(
            err,
            AttributionDenied::UnknownFlow {
                flow_id: "f1".into()
            }
        );
    }

    #[tokio::test]
    async fn non_participant_is_denied() {
        let store = Store {
            flow: Some(live_flow(false)),
            fail: false,
        };
        let err = resolve(&store, AGENT, Some("f1".into()), 300)
            .await
            .unwrap_err();
        assert!(matches!(err, AttributionDenied::NotParticipant { .. }));
    }

    #[tokio::test]
    async fn store_failure_fails_closed() {
        let store = Store {
            flow: Some(live_flow(true)),
            fail: true,
        };
        let err = resolve(&store, AGENT, Some("f1".into()), 300)
            .await
            .unwrap_err();
        assert_eq!(err, AttributionDenied::StoreUnavailable);
    }

    #[tokio::test]
    async fn participant_of_live_flow_is_attributed() {
        let store = Store {
            flow: Some(live_flow(true)),
            fail: false,
        };
        let a = resolve(&store, AGENT, Some("f1".into()), 300)
            .await
            .unwrap();
        assert_eq!(a.flow_id, "f1");
        assert!(a.user_id.is_some());
        assert_eq!(a.context_id.as_deref(), Some("ses_1"));
        assert_eq!(a.mode, Mode::FreeFlowing);
        assert_eq!(a.source, AttributionSource::Traceparent);
        assert_eq!(a.user_message.as_deref(), Some("hi"));
    }

    #[tokio::test]
    async fn unknown_mode_label_defaults_to_free_flowing() {
        let store = Store {
            flow: Some(LiveFlow {
                mode: Some("bogus".into()),
                ..live_flow(true)
            }),
            fail: false,
        };
        let a = resolve(&store, AGENT, Some("f1".into()), 300)
            .await
            .unwrap();
        assert_eq!(a.mode, Mode::FreeFlowing);
    }

    #[tokio::test]
    async fn non_uuid_agent_id_is_denied() {
        let store = Store {
            flow: Some(live_flow(true)),
            fail: false,
        };
        let err = resolve(&store, "not-a-uuid", Some("f1".into()), 300)
            .await
            .unwrap_err();
        assert!(matches!(err, AttributionDenied::InvalidAgentId { .. }));
    }
}
