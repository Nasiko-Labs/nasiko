//! Query classifier — maps an incoming query to a model [`Tier`] for the destination
//! provider.
//!
//! The classifier answers "how much model does this query need?" as a coarse tier; the
//! [tier registry](super::registry) then maps `(provider, tier)` to a concrete model.
//! Provider selection and request translation happen elsewhere (the resolver / inbound
//! spokes) — the classifier only chooses the *strength* of the model, never the provider.
//!
//! **The classification logic is deliberately not implemented yet.** This is the agreed
//! signature and a placeholder body so the precedence machinery around it (boundary
//! gating, registry lookup, decision cache, fallbacks) can be built and tested end-to-end
//! before the real query analysis lands as a separate effort.

/// Coarse model strength tier. Tier 1 = most capable (complex queries), Tier 3 = smallest
/// (very simple queries), Tier 2 = in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Complex queries — the strongest model in the provider's registry.
    Tier1,
    /// Mid-complexity queries.
    Tier2,
    /// Very simple queries — the smallest/cheapest model.
    Tier3,
}

/// Classify a `query` into a model [`Tier`] for the destination `provider`.
///
/// `provider` is the **destination** provider the request will be routed to (already
/// resolved), not the agent's client SDK — the tier is later looked up in *that*
/// provider's registry.
///
/// TODO: implement the real classification. The business logic that analyses `query`
/// (and may use `provider`) to decide the tier is a separate, later effort. For now this
/// returns a fixed mid tier so the surrounding precedence chain is exercisable end-to-end.
pub fn classify(_query: &str, _provider: &str) -> Tier {
    // Placeholder — real query analysis to be added later.
    Tier::Tier2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_returns_mid_tier() {
        assert_eq!(classify("anything", "anthropic"), Tier::Tier2);
    }
}
