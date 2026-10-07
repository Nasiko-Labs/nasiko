//! Relative-path validation for every workspace boundary (tool arguments, the
//! upload route, the download route). These paths live only in the catalog —
//! object-store keys are derived from identifiers, never from a path, so a
//! hostile name can never reach the object store — but they are echoed back to
//! users and used as download filenames, so the rules are strict.
//!
//! One Windows save quirk is knowingly accepted: a segment ending in `.`
//! (`a.`), which Explorer strips when saving. It is a legal name on every
//! platform this server runs on, and rejecting it would surprise more users
//! than the quirk does. Trailing whitespace, the other such quirk, *is*
//! rejected — not for Windows' sake but because it is invisible in every UI
//! that renders the name, so two distinct rows would look like one file.

use unicode_normalization::UnicodeNormalization;

/// Longest relative path the catalog accepts, measured in bytes after normalisation.
pub const MAX_REL_PATH_BYTES: usize = 900;

/// Characters that are invisible or change how the characters around them are
/// displayed, none of which `char::is_control` catches: that predicate covers
/// only Unicode general category `Cc`, and these are the `Cf` format characters
/// (soft hyphen, zero-width space/non-joiner/joiner, the bidirectional marks,
/// embeddings, overrides and isolates, word joiner and the invisible operators,
/// BOM) plus the line and paragraph separators (`Zl`/`Zp`). Each lets a name
/// display as something it is not — `report\u{202E}gnp.exe` renders as
/// `reportexe.png`, and `rep\u{200B}ort.md` is indistinguishable from
/// `report.md` while being a different catalog row. The list is hand-maintained
/// because the crate graph carries no general-category table. A non-breaking
/// space (`U+00A0`) is absent on purpose: it is `White_Space`, so the
/// whitespace rule on segments already rejects it where it would matter.
const INVISIBLE_OR_BIDI: [std::ops::RangeInclusive<char>; 8] = [
    '\u{00AD}'..='\u{00AD}', // soft hyphen
    '\u{061C}'..='\u{061C}', // Arabic letter mark
    '\u{200B}'..='\u{200F}', // zero-width space, ZWNJ, ZWJ, LRM, RLM
    '\u{2028}'..='\u{2029}', // line separator, paragraph separator
    '\u{202A}'..='\u{202E}', // bidi embeddings and overrides
    '\u{2060}'..='\u{2064}', // word joiner, invisible operators
    '\u{2066}'..='\u{2069}', // bidi isolates
    '\u{FEFF}'..='\u{FEFF}', // byte-order mark / zero-width no-break space
];

fn is_invisible_or_bidi(c: char) -> bool {
    INVISIBLE_OR_BIDI.iter().any(|range| range.contains(&c))
}

/// Validates a user-supplied relative path and returns its canonical form.
///
/// Every returned path holds all of these, and the error names the first rule broken:
/// - Unicode NFC, so a decomposed name and its composed twin are one path
/// - a single leading `./` is stripped, exactly once; any further `.` segment
///   (`././a`, `a/./b`) is rejected rather than stripped, because stripping to a
///   fixpoint would silently canonicalise what should surface as the caller's
///   own malformed input
/// - non-empty and at most [`MAX_REL_PATH_BYTES`] bytes
/// - relative — no leading `/` or `\`
/// - no `:`, which rejects drive letters and scheme-like names
/// - no `"`, which would end a quoted `Content-Disposition` parameter early
/// - no control characters, which corrupt logs and headers, and none of the
///   invisible or bidirectional characters in [`INVISIBLE_OR_BIDI`], which make a
///   name display as a different one
/// - no `.`, `..` or empty segments, so a path can neither climb nor repeat separators
/// - no segment that begins or ends with whitespace: it is invisible when the
///   name is rendered, and Windows strips a trailing space on save, so `a.txt `
///   and `a.txt` would be two rows that download as one file
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
    if path.contains('"') {
        return Err("path may not contain '\"'".into());
    }
    if path
        .chars()
        .any(|c| c.is_control() || is_invisible_or_bidi(c))
    {
        return Err("path may not contain control, invisible or bidirectional characters".into());
    }
    for segment in path.split(['/', '\\']) {
        validate_segment(segment)?;
    }
    Ok(path.replace('\\', "/"))
}

/// The per-segment rules: nothing that climbs or repeats a separator, and no
/// whitespace at either edge.
fn validate_segment(segment: &str) -> Result<(), String> {
    if segment == ".." || segment == "." || segment.is_empty() {
        return Err("path may not contain '..', '.' or empty segments".into());
    }
    if segment != segment.trim() {
        return Err("path segments may not begin or end with whitespace".into());
    }
    Ok(())
}

/// The last `/`-separated segment of a validated path, which is the whole path
/// when that path is a bare file name. Total over every input: when the trailing
/// segment is empty (`a/b/`, which [`validate_rel_path`] never produces) the whole
/// input is returned rather than an empty name.
///
/// The returned segment is untrusted text. When it becomes a download filename
/// it must be emitted as an RFC 6266 `filename*=UTF-8''<percent-encoded>`
/// parameter, never interpolated into a quoted-string `filename="…"` — the
/// validator rejects `"`, but that is defence in depth, not a licence to quote.
pub fn file_name(rel_path: &str) -> &str {
    let last = rel_path
        .rfind('/')
        .map_or(rel_path, |slash| &rel_path[slash + 1..]);
    if last.is_empty() { rel_path } else { last }
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
    fn accepts_a_trailing_dot_and_the_full_length() {
        // A knowingly accepted Windows quirk — see the module docs.
        assert_eq!(validate_rel_path("a.").unwrap(), "a.");
        assert!(validate_rel_path(&"x".repeat(900)).is_ok());
    }

    #[test]
    fn rejects_traversal_absolute_colon_quote_invisible_whitespace_and_long() {
        for bad in [
            "../x",
            "a/../b",
            "a/./b",
            "././a",
            "a//b",
            "a/b/",
            "/etc/passwd",
            "C:\\x",
            "a:b",
            "a\".txt",
            "a\u{0}b",
            "a\nb",
            "report\u{202E}gnp.exe",
            "rep\u{200B}ort.md",
            " a.txt ",
            "a.txt ",
            "a.txt\u{a0}",
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

    #[test]
    fn file_name_is_total_over_inputs_the_validator_never_produces() {
        assert_eq!(file_name("a/b/"), "a/b/");
        assert_eq!(file_name(""), "");
    }
}
