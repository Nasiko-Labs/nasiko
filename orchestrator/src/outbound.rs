//! Where text that must reach **every** agent this crate calls comes from.
//!
//! The MAF executor talks to agents from a background worker spawned at startup, which is why
//! this is a trait rather than the plain `Option<String>` the chat-path orchestrator carries
//! (`nasiko_react_agent::OrchestratorConfig::outbound_prefix`): the worker outlives any one
//! request, so a value captured when it was spawned would be frozen for the life of the process
//! and could never reflect a later change.
//!
//! Deliberately says nothing about what the text *is*. A distribution with nothing to add wires
//! [`NoOutboundPrefix`] and every agent call goes out exactly as this crate composed it.

use std::fmt::Debug;

use async_trait::async_trait;

/// Supplies, and applies, the text prepended to every agent call this crate makes.
///
/// Two methods rather than one `compose(body)`, because they have different cadences: the prefix
/// is resolved **once per execution** (a MAF run is one unit of work, and a policy change landing
/// between step 2 and step 3 would otherwise leave one execution governed by two different
/// prefixes — worse to reason about and worse to audit than either alternative), while the join
/// happens once per step.
#[async_trait]
pub trait OutboundPrefix: Debug + Send + Sync {
    /// The prefix in force right now, or `None` to send agent calls unchanged.
    async fn resolve(&self) -> Option<String>;

    /// Join `prefix` onto the front of `body`.
    ///
    /// On the trait rather than a free function in this crate so the join keeps exactly **one**
    /// implementation in the tree. The canonical one is `nasiko_react_agent::prefixed`, which
    /// this crate cannot reach (no dependency edge, and adding one for a three-line join would be
    /// a poor trade); an implementor that can reach it delegates, and the convention cannot drift
    /// between the two orchestrators.
    fn compose(&self, prefix: Option<&str>, body: &str) -> String;
}

/// The default: nothing is added, and `compose` returns the body untouched.
#[derive(Debug)]
pub struct NoOutboundPrefix;

#[async_trait]
impl OutboundPrefix for NoOutboundPrefix {
    async fn resolve(&self) -> Option<String> {
        None
    }

    fn compose(&self, _prefix: Option<&str>, body: &str) -> String {
        body.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unconfigured path must be byte-identical to no seam at all — a MAF step's prompt is
    /// also persisted to `step_results`, so anything added here would show up in the record.
    #[tokio::test]
    async fn the_default_adds_nothing_and_composes_to_the_body() {
        let p = NoOutboundPrefix;
        assert_eq!(p.resolve().await, None);
        assert_eq!(p.compose(None, "step prompt"), "step prompt");
        // Even handed a prefix, the no-op implementation must not invent a join.
        assert_eq!(p.compose(Some("POLICY"), "step prompt"), "step prompt");
    }
}
