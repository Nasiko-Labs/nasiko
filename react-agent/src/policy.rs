//! The delegation-policy seam.
//!
//! The ReAct loop decides *how* to run a turn. It does not decide what an
//! operator is allowed to make it do — a bar an agent match must clear before
//! it may be called, house rules every answer has to follow, a requirement that
//! the orchestrator only ever answers through an agent. All of that is operator
//! policy, it is supplied by an implementation of [`DelegationPolicy`], and it
//! is simply absent when no policy is configured.
//!
//! Hooks live here; behaviour does not. That is what keeps the loop one shape:
//! every policy question is asked at the same point whether or not anyone is
//! listening, so a policy can never be applied on one code path and skipped on
//! the sibling one — which is exactly how the two loops below used to drift.

use std::fmt::Debug;

use crate::registry::AgentInfo;

/// A JSON-schema fragment a policy adds to every agent tool.
///
/// Merged into the tool definition rather than replacing it: the loop owns
/// `message` and `context_id`, which it needs whatever the policy is, and a
/// policy that could rewrite the whole schema could break dispatch.
#[derive(Debug, Clone, Default)]
pub struct ToolSchemaExtra {
    /// Property name → its JSON Schema, merged into the tool's `properties`.
    pub properties: serde_json::Map<String, serde_json::Value>,
    /// Property names appended to the tool's `required` list.
    pub required: Vec<String>,
}

/// Operator policy applied to one orchestration.
///
/// Every method has a do-nothing default, so an implementation states only what
/// it actually constrains. A `None` policy on [`crate::OrchestratorConfig`] and
/// a policy that overrides nothing behave identically — the former is just
/// cheaper.
///
/// `Debug` is a supertrait because `OrchestratorConfig` derives `Debug` and is
/// logged on the error paths; a policy that hid itself from those logs would
/// make "why was this answer replaced?" unanswerable from a trace.
pub trait DelegationPolicy: Debug + Send + Sync {
    /// The policy's own section of the system prompt, rendered after the agent
    /// roster and before the protocol. Empty to add nothing.
    fn preamble_policy(&self) -> String {
        String::new()
    }

    /// Text rendered last in the system prompt, after every built-in section.
    ///
    /// Separate from [`Self::preamble_policy`] because position is load-bearing:
    /// an instruction about *how to answer* only survives if it is the last
    /// thing the model reads, while the mechanics of delegating belong next to
    /// the roster they refer to.
    fn preamble_footer(&self) -> String {
        String::new()
    }

    /// Appended to the user message on every turn, including the first.
    ///
    /// The turn that writes the answer is the one that has to obey a rule about
    /// how to answer, and the system prompt can be thousands of tokens behind by
    /// then. Returned as a whole string — leading separator included — so the
    /// policy controls how it joins onto the user's own words.
    fn turn_reminder(&self) -> Option<&str> {
        None
    }

    /// Extra arguments every agent tool must accept, and which of them are
    /// required. `None` leaves the tool schema untouched.
    fn tool_schema_extra(&self) -> Option<ToolSchemaExtra> {
        None
    }

    /// Judge one tool call from its raw arguments, before it is dispatched.
    ///
    /// `Err(reason)` blocks the call. The reason is shown to the operator *and*
    /// fed back into the model's context, so it should say what was wrong in
    /// terms the model can act on — a bare "blocked" teaches it nothing and it
    /// retries the identical call.
    ///
    /// Raw `arguments` rather than a typed struct: this runs before
    /// deserialization precisely so a policy violation surfaces as an explicit,
    /// model-readable rejection instead of an opaque JSON error.
    fn check_tool_call(&self, _arguments: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }

    /// What to tell the model after `check_tool_call` refused one of its calls to an agent,
    /// beyond the reason itself. `prior_refusals` is how many times this agent has already been
    /// refused or failed in this request (0 for the first).
    ///
    /// `None` keeps the loop's own wording, which tells the model not to retry. A policy whose
    /// refusal is a judgement the model may reasonably revisit (a confidence score) can invite
    /// one more attempt here, and return `None` for the next so the loop's limit takes over.
    fn refusal_guidance(&self, _prior_refusals: usize) -> Option<String> {
        None
    }

    /// A score to report alongside a call the policy allowed through.
    ///
    /// Carried on [`crate::OrchestratorEvent::ToolCall`] so a permitted call is
    /// as inspectable as a blocked one. `None` when the policy demanded no such
    /// judgement, in which case there is nothing to report.
    fn call_score(&self, _arguments: &serde_json::Value) -> Option<f64> {
        None
    }

    /// The last gate before a final answer reaches the user. Returns the text to
    /// actually send, so a caller cannot forget to use the result.
    ///
    /// `delegated` is true once any agent call in this run has returned
    /// successfully; `agents` is the live roster, which a policy may quote from
    /// because it is platform data rather than model-authored text.
    fn review_final_answer(
        &self,
        text: &str,
        _agents: &[AgentInfo],
        _delegated: bool,
        _turn_idx: usize,
    ) -> String {
        text.to_string()
    }

    /// Output cap for a turn that has not delegated yet, or `None` for no cap.
    ///
    /// A policy that discards long undelegated answers is paying to generate
    /// text nobody reads; this is how it stops.
    ///
    /// `user_query_chars` is the length of the request the turn is answering,
    /// and it is passed because no constant can be a safe cap on its own. An
    /// undelegated turn is precisely the turn that emits the agent tool call,
    /// and the Rules section above tells the model to pass the user's own
    /// wording through — so the one long thing such a turn legitimately writes
    /// scales with the request. A cap below it truncates the call mid-arguments
    /// and breaks the very turn it was meant to make cheaper.
    fn undelegated_max_tokens(&self, _user_query_chars: usize) -> Option<u64> {
        None
    }

    /// Must every turn be buffered rather than streamed?
    ///
    /// Streamed text reaches the client as it is generated, so there is no later
    /// point at which [`Self::review_final_answer`] could withhold it. A policy
    /// that can reject a final answer must return `true`, or turn 0 escapes it.
    ///
    /// The test is "can this policy ever change an answer's text", not "does it
    /// enforce something on this turn". The loop has no way to ask mid-stream,
    /// so a policy that rewrites only *some* turns still has to buffer all of
    /// them — returning a per-turn condition here silently disables
    /// [`Self::review_final_answer`] for whichever turns it excluded.
    fn buffer_every_turn(&self) -> bool {
        false
    }

    /// Is this text one of the refusals this policy produces?
    ///
    /// Callers persist refusals differently from real answers — they are kept
    /// out of the next turn's reasoning context, because a model that reads its
    /// own refusal back treats refusing as what this conversation does. Asking
    /// the policy beats comparing against a constant at the call site: the text
    /// is built here, so recognition cannot drift from construction.
    fn is_refusal(&self, _text: &str) -> bool {
        false
    }
}
