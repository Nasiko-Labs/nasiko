//! A seam for counting requests to agents.
//!
//! Code that sends a request straight to an agent (the ReAct orchestrator's tool calls, the MAF
//! executor's step calls) holds an [`AgentCallHandle`] for the whole request, including a
//! streamed reply, and drops it when the request is over. The platform implements the traits
//! and decides what to record; the callers never see how the load is counted.

use uuid::Uuid;

/// Held for the duration of one request to an agent. Dropping it ends the request.
///
/// A handle dropped without [`set_succeeded`](Self::set_succeeded) means the caller stopped
/// waiting before the request reached a result (for example, its own client disconnected).
pub trait AgentCallHandle: Send {
    /// Record whether the agent produced a result. A request for human input is a result.
    fn set_succeeded(&mut self, succeeded: bool);

    /// Where this call must be sent, when the platform resolved that at admission (it woke a
    /// parked agent, say) and the address the caller listed is only a placeholder. `None` keeps
    /// the address the caller already has.
    fn target(&self) -> Option<CallTarget> {
        None
    }
}

/// An address resolved for one call, with the headers its first hop requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallTarget {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// Opens one [`AgentCallHandle`] per request to an agent.
pub trait AgentCallMeter: Send + Sync {
    fn begin(&self, agent_id: Uuid) -> Box<dyn AgentCallHandle>;
}

/// A handle that records nothing: for a request whose target has no agent id to count against.
pub struct UnmeteredCall;

impl AgentCallHandle for UnmeteredCall {
    fn set_succeeded(&mut self, _succeeded: bool) {}
}
