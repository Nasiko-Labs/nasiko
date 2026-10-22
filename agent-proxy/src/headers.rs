//! The single source of truth for which HTTP headers reach an agent container, and under what
//! names — shared by `oss/server/src/agent_proxy.rs` (which builds the outbound request to an
//! agent) and `ee/server/src/injection_proxy.rs` (the EE hop that relays it onward when
//! `MEMORY_ENABLED=true`). Before this module existed, the two kept independent allowlists;
//! `injection_proxy.rs`'s was copied from `agent_proxy.rs`'s *inbound* copy-from-caller allowlist
//! rather than its *outbound* header set, so the EE hop silently dropped `x-user-id`/`x-username`/
//! `x-is-superuser` the moment memory was enabled — found in review, not by a test. Both call
//! sites now reference these constants by name, so a header added to one side is automatically
//! visible to the other; there is no longer a second list to forget.

/// Copied verbatim from the caller's inbound request onto the outbound one — deliberately an
/// allowlist, not a denylist: the agent container is unvetted, so anything not named here is
/// dropped rather than forwarded by default. In particular this must never include
/// `authorization`/`cookie` (the caller's platform credentials — a hostile agent could otherwise
/// replay them against `/api/*`) or any of [`IDENTITY_HEADERS`] (spoofed identity — a caller could
/// otherwise set its own `x-user-id`).
pub const CALLER_SAFE_HEADERS: &[&str] = &[
    "content-type",
    "accept",
    "accept-encoding",
    "accept-language",
    "a2a-version",
];

pub const HEADER_TRACEPARENT: &str = "traceparent";
pub const HEADER_X_USER_ID: &str = "x-user-id";
pub const HEADER_X_USERNAME: &str = "x-username";
pub const HEADER_X_IS_SUPERUSER: &str = "x-is-superuser";

/// Trace context and caller identity, computed server-side from the caller's already-verified
/// claims — never copied from the caller's own inbound headers (see [`CALLER_SAFE_HEADERS`]'s doc
/// comment for why). `agent_proxy.rs` sets these explicitly, by name, using these same constants;
/// `injection_proxy.rs` relays them onward alongside [`CALLER_SAFE_HEADERS`] because from its
/// point of view — a second HTTP hop — they're indistinguishable from any other inbound header
/// without this shared list naming them.
pub const IDENTITY_HEADERS: &[&str] = &[
    HEADER_TRACEPARENT,
    HEADER_X_USER_ID,
    HEADER_X_USERNAME,
    HEADER_X_IS_SUPERUSER,
];

/// True if `name` (a lowercase header name) should reach the agent container — either copied
/// from the caller's inbound request, or one of the server-computed identity headers.
pub fn is_agent_forwarded_header(name: &str) -> bool {
    CALLER_SAFE_HEADERS.contains(&name) || IDENTITY_HEADERS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_safe_and_identity_headers_are_disjoint() {
        for h in CALLER_SAFE_HEADERS {
            assert!(
                !IDENTITY_HEADERS.contains(h),
                "{h} must not be in both lists — it's either copied from the caller's inbound \
                 request or server-computed, never both"
            );
        }
    }

    #[test]
    fn is_agent_forwarded_header_covers_both_lists() {
        for h in CALLER_SAFE_HEADERS.iter().chain(IDENTITY_HEADERS.iter()) {
            assert!(is_agent_forwarded_header(h), "{h} must be recognized");
        }
        assert!(!is_agent_forwarded_header("authorization"));
        assert!(!is_agent_forwarded_header("cookie"));
    }
}
