//! Elision breadcrumbs.
//!
//! Every removal this crate makes leaves a counted marker. That is not decoration: a model shown
//! a silently-truncated payload answers confidently from incomplete data, whereas one shown
//! `[⋯ 412 lines elided ⋯]` knows to caveat or to ask for the rest.
//!
//! The marker is also the idempotency mechanism — [`contains_marker`] is what lets
//! [`crate::compress`] recognise its own prior output and decline to compress it again.

/// Opening sigil of every breadcrumb.
///
/// U+22EF (`⋯`), deliberately *not* U+2026 (`…`): `ContextManager::compact_simple` in
/// `nasiko-react-agent` already uses `…` for its own 300-char truncation. Distinct sigils keep
/// the two kinds of elision distinguishable when they compose, and leave that crate's existing
/// assertions on `…` valid.
pub(crate) const MARKER_OPEN: &str = "[⋯ ";
pub(crate) const MARKER_CLOSE: &str = " ⋯]";

/// Whether `s` already carries a breadcrumb from this crate.
///
/// Public because callers wiring two compression seams in series (the router and the ReAct loop)
/// may want to assert the second pass is a no-op.
pub fn contains_marker(s: &str) -> bool {
    s.contains(MARKER_OPEN)
}

fn wrap(body: &str, recovery_ref: Option<&str>) -> String {
    match recovery_ref {
        Some(handle) => format!("{MARKER_OPEN}{body} · recover: {handle}{MARKER_CLOSE}"),
        None => format!("{MARKER_OPEN}{body}{MARKER_CLOSE}"),
    }
}

pub(crate) fn elided_lines(n: usize, recovery_ref: Option<&str>) -> String {
    wrap(&format!("{n} lines elided"), recovery_ref)
}

/// Names the cut lines by the view's own numbers, so an agent can fetch exactly that range back
/// with one read instead of narrowing towards it step by step.
pub(crate) fn elided_line_range(first: u64, last: u64, recovery_ref: Option<&str>) -> String {
    wrap(&format!("lines {first}–{last} elided"), recovery_ref)
}

/// First line of a lossless table ([`crate::table`]). It opens with the marker sigil, so a
/// second pass recognises the output as already compressed and leaves it alone.
pub(crate) fn table_header(rows: usize) -> String {
    format!("{}{rows} rows{MARKER_CLOSE}", table_header_prefix())
}

pub(crate) fn table_header_prefix() -> String {
    format!("{MARKER_OPEN}table: ")
}

pub(crate) fn elided_items(n: usize, of: usize, recovery_ref: Option<&str>) -> String {
    wrap(&format!("{n} of {of} items elided"), recovery_ref)
}

pub(crate) fn elided_chars(n: usize, recovery_ref: Option<&str>) -> String {
    wrap(&format!("{n} chars elided"), recovery_ref)
}

pub(crate) fn elided_matches(n: usize, files: usize, recovery_ref: Option<&str>) -> String {
    wrap(
        &format!("{n} more matches in {files} files elided"),
        recovery_ref,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builder_is_detectable_as_a_marker() {
        for marker in [
            elided_lines(412, None),
            elided_items(8, 10, None),
            elided_chars(900, None),
            elided_matches(30, 4, None),
        ] {
            assert!(contains_marker(&marker), "not detectable: {marker}");
        }
    }

    #[test]
    fn markers_carry_a_count() {
        assert!(elided_lines(412, None).contains("412"));
        assert!(elided_items(8, 10, None).contains("8 of 10"));
    }

    #[test]
    fn recovery_handle_is_interpolated_when_present() {
        let m = elided_lines(5, Some("nasiko://c/9f3a"));
        assert!(m.contains("recover: nasiko://c/9f3a"));
        assert!(contains_marker(&m));
    }

    #[test]
    fn marker_sigil_is_not_the_one_react_agent_uses() {
        assert!(!MARKER_OPEN.contains('…'));
        assert!(!elided_lines(1, None).contains('…'));
    }
}
