//! How supplemental content is joined onto the text it qualifies.
//!
//! One function, in the lowest crate that needs it, because its callers across two crates have to
//! agree on the convention byte for byte: the routed planner's preamble (in `nasiko-server`) and
//! the MAF final-output synthesiser's system prompt (`nasiko-orchestrator`). Agent calls do not use
//! it: supplemental context rides on those as its own message part. A second implementation
//! anywhere would drift, and the drift would be invisible: both spellings produce text that looks
//! right in a log and reads right to a model.

/// Join `prefix` onto the front of `body`, separated by one blank line.
///
/// A prefix that is absent — or present but blank, which is what an enabled-but-empty
/// configuration looks like — returns `body` **unchanged** rather than `body` with whitespace
/// bolted on, so a deployment with nothing to add sends exactly what it sent before any of this
/// existed. Trailing newlines on the prefix are trimmed for the same reason: the separator is the
/// convention's, not the caller's, or the gap widens with every layer that contributes.
pub fn prefixed(prefix: Option<&str>, body: &str) -> String {
    match prefix {
        Some(text) if !text.trim().is_empty() => format!("{}\n\n{body}", text.trim_end()),
        _ => body.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The no-injection case must be byte-identical to no seam at all, or every deployment with
    /// nothing configured pays a formatting change it never asked for.
    #[test]
    fn an_absent_or_blank_prefix_leaves_the_body_untouched() {
        assert_eq!(prefixed(None, "hello"), "hello");
        assert_eq!(prefixed(Some(""), "hello"), "hello");
        assert_eq!(prefixed(Some("   \n  "), "hello"), "hello");
    }

    #[test]
    fn a_prefix_is_separated_from_the_body_by_exactly_one_blank_line() {
        assert_eq!(prefixed(Some("POLICY"), "hello"), "POLICY\n\nhello");
    }

    #[test]
    fn a_prefix_with_trailing_newlines_still_yields_one_blank_line() {
        assert_eq!(prefixed(Some("POLICY\n\n"), "hello"), "POLICY\n\nhello");
    }
}
