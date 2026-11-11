//! The `tool_search` savings row: how much smaller a search-mode `tools/list` was than the listing
//! the same agent would have got with search off.
//!
//! Both sides are measured in bytes, as `tools/list` serializes them, over the generic connectors
//! only. Composio is left out of both: with search off it lists its own meta-tools, which this
//! gateway never sees without a backend round trip, so it has no baseline to measure against.
//!
//! The row is per listing, while the model reads that listing on every call of the flow, so the
//! figure undercounts. It also leaves out the extra `nasiko_search_tools` calls an agent may make.
//! It carries no model: the gateway never sees one, and the read path prices a model-less row at
//! the flow's first LLM call, as it does the orchestrator's layers.

use nasiko_savings::{Layer, Method, SavingsRow};
use uuid::Uuid;

/// No provider usage reaches the gateway to calibrate against, so the shared divisor
/// (`nasiko_llm_router::savings`, `ContextManager`, `PacmsSelector`) is used and the row flagged
/// estimated.
const CHARS_PER_TOKEN: f64 = 4.0;

/// Who and which flow a listing was served to.
pub(crate) struct ListingContext {
    pub user_id: Uuid,
    pub agent_id: Uuid,
    pub flow_id: String,
}

/// Build the row, or `None` when there is nothing to compare: no generic tool would have been
/// listed, or the two listings round to the same token count.
///
/// Signed, like `context_selection`: the search meta-tools are always listed, so an agent with
/// only a few small tools reads more with search on, and that is reported rather than clamped.
pub(crate) fn tool_search_row(
    baseline_bytes: usize,
    listed_bytes: usize,
    ctx: ListingContext,
) -> Option<SavingsRow> {
    if baseline_bytes == 0 {
        return None;
    }
    // Truncate toward zero in both directions, so neither a saving nor a cost is overstated.
    let saved_tokens = ((baseline_bytes as f64 - listed_bytes as f64) / CHARS_PER_TOKEN) as i64;
    if saved_tokens == 0 {
        return None;
    }
    Some(SavingsRow {
        user_id: ctx.user_id,
        agent_id: Some(ctx.agent_id),
        flow_id: Some(ctx.flow_id),
        session_id: None,
        provider: None,
        model: None,
        layer: Layer::ToolSearch,
        bytes_before: Some(baseline_bytes as i64),
        bytes_after: Some(listed_bytes as i64),
        saved_input_tokens: saved_tokens,
        saved_output_tokens: 0,
        saved_cost_usd: 0.0,
        method: Method::MeasuredBytes,
        token_estimated: true,
        context_tier: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ListingContext {
        ListingContext {
            user_id: Uuid::nil(),
            agent_id: Uuid::nil(),
            flow_id: "trace-1".into(),
        }
    }

    #[test]
    fn a_listing_smaller_than_the_full_catalog_is_a_saving() {
        let r = tool_search_row(40_000, 8_000, ctx()).expect("a smaller listing saved tokens");
        assert_eq!(r.layer, Layer::ToolSearch);
        assert_eq!(r.saved_input_tokens, 8_000);
        assert_eq!(r.bytes_before, Some(40_000));
        assert_eq!(r.bytes_after, Some(8_000));
        assert!(r.token_estimated);
        // Priced at read time from the flow's first call.
        assert_eq!(r.model, None);
        assert_eq!(r.saved_cost_usd, 0.0);
    }

    #[test]
    fn a_listing_larger_than_the_catalog_is_a_negative_row() {
        // Two small tools: the three meta-tools outweigh what search left out.
        let r = tool_search_row(1_000, 3_000, ctx()).expect("a cost is reported, not clamped");
        assert_eq!(r.saved_input_tokens, -500);
    }

    #[test]
    fn nothing_to_compare_writes_nothing() {
        // No generic tool would have been listed (a Composio-only agent).
        assert!(tool_search_row(0, 3_000, ctx()).is_none());
        // Under one token either way.
        assert!(tool_search_row(1_002, 1_000, ctx()).is_none());
        assert!(tool_search_row(1_000, 1_003, ctx()).is_none());
    }
}
