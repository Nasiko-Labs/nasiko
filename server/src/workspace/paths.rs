//! Relative-path validation for every workspace boundary (tool arguments, the
//! upload route, the download route). These paths live only in the catalog —
//! object-store keys are derived from identifiers, never from a path, so a
//! hostile name can never reach the object store — but they are echoed back to
//! users and used as download filenames, so the rules are strict.

use unicode_normalization::UnicodeNormalization;

/// Longest relative path the catalog accepts, measured in bytes after normalisation.
pub const MAX_REL_PATH_BYTES: usize = 900;

/// Validates a user-supplied relative path and returns its canonical form.
///
/// Every returned path holds all of these, and the error names the first rule broken:
/// - Unicode NFC, so a decomposed name and its composed twin are one path
/// - no leading `./`
/// - non-empty and at most [`MAX_REL_PATH_BYTES`] bytes
/// - relative — no leading `/` or `\`
/// - no `:`, which rejects drive letters and scheme-like names
/// - no control characters, which corrupt logs and `Content-Disposition` headers
/// - no `.`, `..` or empty segments, so a path can neither climb nor repeat separators
/// - `\` separators rewritten to `/`, the single separator the catalog stores
///
/// The error is a plain message; callers wrap it in their own error shape.
pub fn validate_rel_path(raw: &str) -> Result<String, String> {
    let composed: String = raw.nfc().collect();
    let path = composed.strip_prefix("./").unwrap_or(&composed);
    if path.is_empty() {
        return Err("path is empty".into());
    }
    if path.len() > MAX_REL_PATH_BYTES {
        return Err(format!("path longer than {MAX_REL_PATH_BYTES} bytes"));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err("path must be relative".into());
    }
    if path.contains(':') {
        return Err("path may not contain ':'".into());
    }
    if path.chars().any(char::is_control) {
        return Err("path may not contain control characters".into());
    }
    if path
        .split(['/', '\\'])
        .any(|segment| segment == ".." || segment == "." || segment.is_empty())
    {
        return Err("path may not contain '..', '.' or empty segments".into());
    }
    Ok(path.replace('\\', "/"))
}

/// The last `/`-separated segment of a validated path, which is the whole path
/// when that path is a bare file name.
pub fn file_name(rel_path: &str) -> &str {
    rel_path.rsplit('/').next().unwrap_or(rel_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_nested_paths() {
        assert_eq!(validate_rel_path("src/main.rs").unwrap(), "src/main.rs");
    }

    #[test]
    fn normalizes_nfc_and_strips_leading_dot_slash() {
        assert_eq!(validate_rel_path("./a/b.txt").unwrap(), "a/b.txt");
    }

    #[test]
    fn composes_decomposed_characters() {
        assert_eq!(validate_rel_path("e\u{301}.txt").unwrap(), "\u{e9}.txt");
    }

    #[test]
    fn normalizes_backslash_separators() {
        assert_eq!(validate_rel_path("a\\b.txt").unwrap(), "a/b.txt");
    }

    #[test]
    fn rejects_traversal_absolute_colon_control_and_long() {
        for bad in [
            "../x",
            "a/../b",
            "/etc/passwd",
            "C:\\x",
            "a:b",
            "a\u{0}b",
            "a\nb",
            "",
        ] {
            assert!(validate_rel_path(bad).is_err(), "{bad:?}");
        }
        assert!(validate_rel_path(&"x".repeat(901)).is_err());
    }

    #[test]
    fn file_name_returns_the_last_segment() {
        assert_eq!(file_name("a/b/c.txt"), "c.txt");
        assert_eq!(file_name("c.txt"), "c.txt");
    }
}
