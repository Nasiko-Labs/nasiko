//! The only module in this crate permitted to slice a string.
//!
//! Everything here indexes on char boundaries via `char_indices` + `str::get`. Compressor input
//! is attacker-influenceable tool output, and a byte-offset slice landing mid-character panics.

use crate::marker;

/// First `n` characters. Returns the whole input when it is shorter.
pub(crate) fn take_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((byte_idx, _)) => s.get(..byte_idx).unwrap_or(s),
        None => s,
    }
}

/// Last `n` characters. Returns the whole input when it is shorter.
pub(crate) fn tail_chars(s: &str, n: usize) -> &str {
    let total = s.chars().count();
    if total <= n {
        return s;
    }
    match s.char_indices().nth(total - n) {
        Some((byte_idx, _)) => s.get(byte_idx..).unwrap_or(s),
        None => s,
    }
}

/// `head` chars, a counted marker, then `tail` chars.
///
/// Returns `None` when the input is short enough that this would not shrink it — the caller then
/// leaves it alone rather than paying marker bytes for nothing.
pub(crate) fn head_tail(
    s: &str,
    head: usize,
    tail: usize,
    recovery_ref: Option<&str>,
) -> Option<String> {
    let total = s.chars().count();
    if total <= head + tail {
        return None;
    }
    let elided = total - head - tail;
    Some(format!(
        "{}{}{}",
        take_chars(s, head),
        marker::elided_chars(elided, recovery_ref),
        tail_chars(s, tail)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_chars_never_splits_a_multibyte_character() {
        // 300 ASCII then a 3-byte char: a byte-offset slice at 300 would land mid-character.
        let s = format!("{}€tail", "a".repeat(300));
        assert_eq!(take_chars(&s, 301).chars().count(), 301);
        assert!(take_chars(&s, 301).ends_with('€'));
    }

    #[test]
    fn take_chars_returns_whole_input_when_shorter() {
        assert_eq!(take_chars("abc", 99), "abc");
    }

    #[test]
    fn tail_chars_never_splits_a_multibyte_character() {
        let s = format!("head€{}", "z".repeat(300));
        assert_eq!(tail_chars(&s, 301).chars().count(), 301);
        assert!(tail_chars(&s, 301).starts_with('€'));
    }

    #[test]
    fn tail_chars_returns_whole_input_when_shorter() {
        assert_eq!(tail_chars("abc", 99), "abc");
    }

    #[test]
    fn head_tail_declines_when_it_would_not_shrink() {
        assert!(head_tail("short", 10, 10, None).is_none());
    }

    #[test]
    fn head_tail_keeps_both_ends_and_counts_the_middle() {
        let s = "a".repeat(100);
        let out = head_tail(&s, 5, 5, None).unwrap();
        assert!(out.starts_with("aaaaa"));
        assert!(out.ends_with("aaaaa"));
        assert!(out.contains("90 chars elided"));
    }

    #[test]
    fn head_tail_is_multibyte_safe_at_both_boundaries() {
        let s = format!("{}€{}€{}", "a".repeat(50), "b".repeat(50), "c".repeat(50));
        let out = head_tail(&s, 51, 51, None).unwrap();
        assert!(out.contains('€'));
    }
}
