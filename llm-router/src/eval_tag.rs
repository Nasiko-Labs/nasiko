//! The replay-eval attempt tag a call's usage row is filed under.
//!
//! A replay eval drives a coding agent through the router once per attempt and then asks the
//! server what that attempt cost (`GET /api/evals/usage`). Its calls carry no flow, so nothing
//! else on the row says which attempt they belong to: the eval runner names the attempt in the
//! [`EVAL_ATTEMPT_HEADER`] request header, and the router copies it onto `trace_usage.eval_attempt`.
//!
//! Only the dedicated `eval` coding-agent identity may tag its calls. The integration id is
//! server-managed (`agents.coding_agent_integration_id`), so an ordinary agent cannot file its
//! spend under someone's eval run by sending the header.

use axum::http::HeaderMap;

/// The request header naming the attempt, e.g. `run-42:task-7:1`.
pub const EVAL_ATTEMPT_HEADER: &str = "x-nasiko-eval-attempt";

/// The coding-agent integration id whose calls may carry an attempt tag.
const EVAL_INTEGRATION: &str = "eval";

/// The longest attempt tag kept, in characters. Long enough for `<run>:<task>:<attempt>` built from
/// UUIDs, short enough that a tag is never a payload.
const MAX_ATTEMPT_LEN: usize = 128;

/// The attempt this call belongs to, when the calling agent is the `eval` integration: the
/// attempt its token was minted for (`token_attempt`, set by the server's own eval runs, which
/// the agent in the container cannot change), else the [`EVAL_ATTEMPT_HEADER`] value (the local
/// CLI's gateway sets it), else `None`.
///
/// A well-formed tag is 1 to [`MAX_ATTEMPT_LEN`] characters of `A-Z a-z 0-9 : . _ / -`; the
/// server matches attempts of one run by the `<run>:` prefix. A malformed tag is dropped with a
/// warning rather than refusing the call: the call still runs and is still billed, it is only
/// missing from its run's totals, which the eval report shows as a gap.
pub fn eval_attempt(
    headers: &HeaderMap,
    integration: Option<&str>,
    token_attempt: Option<&str>,
) -> Option<String> {
    if integration != Some(EVAL_INTEGRATION) {
        return None;
    }
    if let Some(tag) = token_attempt {
        return is_well_formed(tag).then(|| tag.to_string());
    }
    let raw = headers.get(EVAL_ATTEMPT_HEADER)?;
    match raw.to_str().ok().filter(|tag| is_well_formed(tag)) {
        Some(tag) => Some(tag.to_string()),
        None => {
            tracing::warn!(
                target: "nasiko::llm_router::eval_tag",
                header = EVAL_ATTEMPT_HEADER,
                len = raw.len(),
                "eval attempt tag is malformed; the call is recorded without it"
            );
            None
        }
    }
}

fn is_well_formed(tag: &str) -> bool {
    (1..=MAX_ATTEMPT_LEN).contains(&tag.len())
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'.' | b'_' | b'/' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn tagged(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            EVAL_ATTEMPT_HEADER,
            HeaderValue::from_str(value).expect("test header is valid"),
        );
        headers
    }

    #[test]
    fn the_eval_identity_keeps_a_well_formed_tag() {
        let tag = "run-1:task.7/a_b:2";
        assert_eq!(
            eval_attempt(&tagged(tag), Some("eval"), None).as_deref(),
            Some(tag)
        );
    }

    #[test]
    fn another_integration_cannot_tag_its_calls() {
        for integration in [Some("opencode"), Some("claude"), None] {
            assert_eq!(eval_attempt(&tagged("run:1"), integration, None), None);
        }
    }

    #[test]
    fn the_tokens_attempt_wins_over_the_header() {
        assert_eq!(
            eval_attempt(&tagged("forged:1"), Some("eval"), Some("run:task:1")).as_deref(),
            Some("run:task:1")
        );
        assert_eq!(
            eval_attempt(&HeaderMap::new(), Some("eval"), Some("bad tag")),
            None
        );
        assert_eq!(
            eval_attempt(&HeaderMap::new(), Some("claude"), Some("run:1")),
            None
        );
    }

    #[test]
    fn no_header_means_no_tag() {
        assert_eq!(eval_attempt(&HeaderMap::new(), Some("eval"), None), None);
    }

    #[test]
    fn a_tag_at_the_length_limit_is_kept_and_one_past_it_is_dropped() {
        let at_limit = "a".repeat(MAX_ATTEMPT_LEN);
        assert_eq!(
            eval_attempt(&tagged(&at_limit), Some("eval"), None),
            Some(at_limit.clone())
        );
        let past_limit = "a".repeat(MAX_ATTEMPT_LEN + 1);
        assert_eq!(eval_attempt(&tagged(&past_limit), Some("eval"), None), None);
    }

    #[test]
    fn an_empty_tag_is_dropped() {
        assert_eq!(eval_attempt(&tagged(""), Some("eval"), None), None);
    }

    #[test]
    fn a_tag_with_a_character_outside_the_set_is_dropped() {
        for bad in ["run 1", "run%1", "run:1;drop", "run*", "run'1"] {
            assert_eq!(
                eval_attempt(&tagged(bad), Some("eval"), None),
                None,
                "{bad}"
            );
        }
    }

    #[test]
    fn a_tag_that_is_not_visible_ascii_is_dropped() {
        let mut headers = HeaderMap::new();
        headers.insert(
            EVAL_ATTEMPT_HEADER,
            HeaderValue::from_bytes("run:é".as_bytes()).expect("opaque bytes are a valid value"),
        );
        assert_eq!(eval_attempt(&headers, Some("eval"), None), None);
    }
}
