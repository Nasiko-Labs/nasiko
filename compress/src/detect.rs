//! Deterministic content-type detection. First match wins, in the order
//! Json → Diff → Log → SearchResults → Code → Markup → Prose.

use crate::content_type::ContentType;

const LOG_LEVELS: [&str; 7] = [
    "ERROR", "WARN", "WARNING", "INFO", "DEBUG", "TRACE", "FATAL",
];

/// Classify `text`. Never fails; unrecognised input is [`ContentType::Prose`].
pub fn detect(text: &str) -> ContentType {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return ContentType::Prose;
    }

    if is_json(trimmed) {
        return ContentType::Json;
    }
    if is_diff(trimmed) {
        return ContentType::Diff;
    }
    if is_log(trimmed) {
        return ContentType::Log;
    }
    if is_search_results(trimmed) {
        return ContentType::SearchResults;
    }
    if is_code(trimmed) {
        return ContentType::Code;
    }
    if is_markup(trimmed) {
        return ContentType::Markup;
    }
    ContentType::Prose
}

/// A **full** parse, not a prefix guess — a truncated payload must not be handed to the JSON
/// compressor, which would then fail and waste the parse.
fn is_json(trimmed: &str) -> bool {
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
}

fn is_diff(trimmed: &str) -> bool {
    if trimmed.contains("diff --git") {
        return true;
    }
    let mut hunks = 0usize;
    let mut changes = 0usize;
    for line in trimmed.lines() {
        if line.starts_with("@@") && line.matches("@@").count() >= 2 {
            hunks += 1;
        } else if (line.starts_with('+') || line.starts_with('-'))
            && !line.starts_with("+++")
            && !line.starts_with("---")
        {
            changes += 1;
        }
    }
    hunks >= 2 && changes > 0
}

fn is_log(trimmed: &str) -> bool {
    let mut total = 0usize;
    let mut matched = 0usize;
    for line in trimmed.lines() {
        if line.trim().is_empty() {
            continue;
        }
        total += 1;
        if has_log_level(line) || starts_with_timestamp(line) {
            matched += 1;
        }
    }
    total >= 4 && matched * 2 >= total
}

pub(crate) fn has_log_level(line: &str) -> bool {
    // Bounded scan: a level token appears in the prefix of a log line, never deep in a payload.
    let prefix = crate::text::take_chars(line, 120);
    LOG_LEVELS.iter().any(|lvl| prefix.contains(lvl))
}

fn starts_with_timestamp(line: &str) -> bool {
    let b = line.trim_start().as_bytes();
    // ISO-8601 date: 4 digits then '-'.
    if b.len() >= 5 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' {
        return true;
    }
    // Bare clock time: HH:MM.
    if b.len() >= 5
        && b[..2].iter().all(u8::is_ascii_digit)
        && b[2] == b':'
        && b[3].is_ascii_digit()
    {
        return true;
    }
    // Bracketed timestamp: `[...]` containing a digit and a colon.
    if b.first() == Some(&b'[')
        && let Some(end) = line.find(']')
        && end <= 40
        && let Some(inner) = line.get(1..end)
    {
        return inner.contains(':') && inner.chars().any(|c| c.is_ascii_digit());
    }
    false
}

/// `path:line:content` or `path:line:col:content`, as emitted by grep/ripgrep.
pub(crate) fn is_search_hit(line: &str) -> bool {
    let mut parts = line.splitn(3, ':');
    let Some(path) = parts.next() else {
        return false;
    };
    let Some(number) = parts.next() else {
        return false;
    };
    if parts.next().is_none() {
        return false;
    }
    !path.is_empty()
        && !path.contains(char::is_whitespace)
        && !number.is_empty()
        && number.bytes().all(|c| c.is_ascii_digit())
}

fn is_search_results(trimmed: &str) -> bool {
    trimmed.lines().filter(|l| is_search_hit(l)).count() >= 5
}

fn is_code(trimmed: &str) -> bool {
    const KEYWORDS: [&str; 10] = [
        "fn ",
        "def ",
        "class ",
        "import ",
        "function ",
        "func ",
        "package ",
        "struct ",
        "impl ",
        "const ",
    ];
    let hits = KEYWORDS.iter().filter(|kw| trimmed.contains(**kw)).count();
    let structural =
        trimmed.contains('{') || trimmed.lines().filter(|l| l.starts_with("    ")).count() >= 3;
    hits >= 3 && structural
}

fn is_markup(trimmed: &str) -> bool {
    if !trimmed.starts_with('<') {
        return false;
    }
    let mut tags = Vec::new();
    for (i, _) in trimmed.match_indices('<') {
        let Some(rest) = trimmed.get(i + 1..) else {
            continue;
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if !name.is_empty() && !tags.contains(&name) {
            tags.push(name);
        }
        if tags.len() >= 3 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_json_object_and_array() {
        assert_eq!(detect(r#"{"a": 1, "b": [1,2]}"#), ContentType::Json);
        assert_eq!(detect("[1, 2, 3]"), ContentType::Json);
    }

    #[test]
    fn truncated_json_is_not_json() {
        // The whole point of requiring a full parse: this must not reach the JSON compressor.
        let truncated = r#"{"items": [{"id": 1}, {"id": 2"#;
        assert_ne!(detect(truncated), ContentType::Json);
    }

    #[test]
    fn json_looking_log_is_not_json() {
        let s = "2026-01-01T00:00:00Z INFO {request started}\n\
                 2026-01-01T00:00:01Z INFO {request done}\n\
                 2026-01-01T00:00:02Z WARN {retrying}\n\
                 2026-01-01T00:00:03Z INFO {ok}";
        assert_eq!(detect(s), ContentType::Log);
    }

    #[test]
    fn detects_git_diff() {
        let s = "diff --git a/x.rs b/x.rs\n@@ -1,3 +1,3 @@\n-old\n+new\n ctx";
        assert_eq!(detect(s), ContentType::Diff);
    }

    #[test]
    fn detects_bare_hunk_diff_without_git_header() {
        let s = "@@ -1,2 +1,2 @@\n-a\n+b\n@@ -9,2 +9,2 @@\n-c\n+d";
        assert_eq!(detect(s), ContentType::Diff);
    }

    #[test]
    fn detects_log_by_level_tokens() {
        let s = "INFO starting\nDEBUG loading config\nINFO ready\nERROR boom";
        assert_eq!(detect(s), ContentType::Log);
    }

    #[test]
    fn detects_log_by_bracketed_timestamp() {
        let s = "[12:00:01] a\n[12:00:02] b\n[12:00:03] c\n[12:00:04] d";
        assert_eq!(detect(s), ContentType::Log);
    }

    #[test]
    fn prose_mentioning_a_level_word_is_not_a_log() {
        let s = "We should INFO the user about this.\nIt is only one line of prose.";
        assert_ne!(detect(s), ContentType::Log);
    }

    #[test]
    fn detects_search_results() {
        let s = "src/a.rs:10:let x = 1;\nsrc/b.rs:22:let y = 2;\nsrc/c.rs:3:fn z() {}\n\
                 src/d.rs:44:use std;\nsrc/e.rs:5:mod m;";
        assert_eq!(detect(s), ContentType::SearchResults);
    }

    #[test]
    fn detects_code() {
        let s = "import os\nclass A:\n    def run(self):\n        return 1\nconst X = 2\n{";
        assert_eq!(detect(s), ContentType::Code);
    }

    #[test]
    fn detects_markup() {
        assert_eq!(
            detect("<html><body><div>hi</div></body></html>"),
            ContentType::Markup
        );
    }

    #[test]
    fn falls_back_to_prose() {
        assert_eq!(
            detect("The quick brown fox jumps over the lazy dog."),
            ContentType::Prose
        );
    }

    #[test]
    fn empty_and_whitespace_are_prose() {
        assert_eq!(detect(""), ContentType::Prose);
        assert_eq!(detect("   \n  "), ContentType::Prose);
    }
}
