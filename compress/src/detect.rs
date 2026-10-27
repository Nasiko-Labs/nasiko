//! Deterministic content-type detection. First match wins, in the order
//! Json → Diff → TestOutput → Log → SearchResults → Code → Markup → Prose.

use crate::content_type::ContentType;

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
    // Before the log rule: a failing run carries tracebacks and `ERROR:` lines that would
    // otherwise make it a log, and the log compressor drops exactly the assertion messages a
    // failing test is about.
    if is_test_output(trimmed) {
        return ContentType::TestOutput;
    }
    // A stack trace is log-shaped: it is precisely the thing that went wrong, and it arrives
    // without any log-level line to give the level scan something to bite on. The signature is
    // what makes it recognisable.
    if is_log(trimmed) || is_traceback(trimmed) {
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
    crate::levels::has_level(line)
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
    starts_with_syslog_date(line)
}

/// RFC 3164 syslog date: `Jan  1 12:34:56 host tag[pid]: message`. The day is space-padded, so
/// the three parts are whitespace tokens: a month name, a bare day number, a clock.
fn starts_with_syslog_date(line: &str) -> bool {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let mut tokens = line.split_whitespace();
    let (Some(month), Some(day), Some(clock)) = (tokens.next(), tokens.next(), tokens.next())
    else {
        return false;
    };
    MONTHS.iter().any(|m| m.eq_ignore_ascii_case(month))
        && day.bytes().all(|c| c.is_ascii_digit())
        && clock.contains(':')
        && clock.bytes().filter(u8::is_ascii_digit).count() >= 3
}

/// A stack trace has no log-level line for the level scan to find, so it is recognised by shape:
/// a header that starts a line (`Traceback (most recent call last):` or Java's `Caused by:`) and
/// frames making up a real share of the text.
///
/// Both conditions matter. A source file quoting a traceback in a docstring (sympy and Django
/// doctests do this constantly) has the header indented and almost no frames; classifying it as
/// a log would hand a code view to the log compressor, which keeps only head, tail and severe
/// lines and drops the code in between.
pub(crate) fn is_traceback(trimmed: &str) -> bool {
    let (mut header, mut frames, mut non_blank) = (false, 0usize, 0usize);
    for line in trimmed.lines() {
        if line.trim().is_empty() {
            continue;
        }
        non_blank += 1;
        header |=
            line.starts_with("Traceback (most recent call last)") || line.starts_with("Caused by");
        let indented = line.len() != line.trim_start().len();
        if indented && is_frame(line.trim_start()) {
            frames += 1;
        }
    }
    header && frames * 5 >= non_blank
}

/// A Python (`File "…", line N`) or JVM (`at pkg.Class.method(…)`) stack frame, indent removed.
fn is_frame(line: &str) -> bool {
    line.starts_with("File \"") || line.starts_with("at ")
}

/// A test runner's report, recognised by the summary line every runner prints at column 0:
/// unittest/Django `Ran 12 tests in 0.4s`, `FAILED (errors=1)`, Django's `Found 614 test(s).`,
/// pytest's `==== 2 failed, 10 passed in 1.2s ====` and `==== test session starts ====`.
///
/// Column 0 matters: a source file that builds one of these strings (Django's own runner, a
/// doctest) has it indented or quoted, so a code view is never mistaken for a run.
pub(crate) fn is_test_output(trimmed: &str) -> bool {
    trimmed.lines().any(is_runner_summary)
}

fn is_runner_summary(line: &str) -> bool {
    let line = line.trim_end();
    if line.len() != line.trim_start().len() {
        return false;
    }
    is_unittest_ran(line)
        || is_unittest_failed(line)
        || is_django_found(line)
        || is_pytest_banner(line)
}

/// `Ran 1 test in 0.003s`, `Ran 12 tests in 1.204s`.
fn is_unittest_ran(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("Ran ") else {
        return false;
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let Some(rest) = rest.get(digits..) else {
        return false;
    };
    digits > 0
        && (rest.starts_with(" test in ") || rest.starts_with(" tests in "))
        && line.ends_with('s')
}

/// `FAILED (errors=1)`, `FAILED (failures=2, errors=1)`.
fn is_unittest_failed(line: &str) -> bool {
    line.strip_prefix("FAILED (").is_some_and(|rest| {
        (rest.starts_with("errors=") || rest.starts_with("failures=")) && rest.ends_with(')')
    })
}

/// Django's `Found 614 test(s).`
fn is_django_found(line: &str) -> bool {
    line.strip_prefix("Found ")
        .and_then(|rest| rest.strip_suffix(" test(s)."))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

/// pytest's `=== test session starts ===` and `=== 2 failed, 10 passed in 1.23s ===`.
fn is_pytest_banner(line: &str) -> bool {
    if !line.starts_with("===") || !line.ends_with("===") {
        return false;
    }
    let inner = line.trim_matches('=').trim();
    inner == "test session starts"
        || (inner.contains(" in ")
            && ["passed", "failed", "error", "errors", "skipped"]
                .iter()
                .any(|w| inner.split([' ', ',']).any(|t| t == *w)))
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
    if hits >= 3 && structural {
        return true;
    }
    starts_like_code(trimmed)
}

/// The shape of a file view rather than a keyword count: a real source file can use only one
/// or two of [`is_code`]'s keywords (a Python module of imports and one `def`), and an agent's
/// file view prefixes every line with a number that hides the indentation.
fn starts_like_code(trimmed: &str) -> bool {
    const LEADS: [&str; 30] = [
        "def ",
        "async def ",
        "class ",
        "import ",
        "from ",
        "return",
        "if ",
        "elif ",
        "else",
        "for ",
        "while ",
        "try",
        "except",
        "raise ",
        "with ",
        "fn ",
        "pub ",
        "let ",
        "use ",
        "impl ",
        "struct ",
        "func ",
        "function ",
        "const ",
        "var ",
        "public ",
        "private ",
        "#include",
        "package ",
        "@",
    ];
    let lines = crate::text::numbered_lines(trimmed);
    let mut non_blank = 0usize;
    let mut leads = 0usize;
    let mut indented = 0usize;
    let mut shaped = 0usize;
    for (_, body) in &lines {
        let stripped = body.trim_start();
        if stripped.is_empty() {
            continue;
        }
        non_blank += 1;
        if stripped.len() < body.len() {
            indented += 1;
        }
        if LEADS.iter().any(|kw| stripped.starts_with(kw)) {
            leads += 1;
        }
        if is_code_shaped(stripped.trim_end()) {
            shaped += 1;
        }
    }
    // Keywords open only a minority of real lines (`x = f(y)` and `log.info(...)` open with an
    // identifier), so the bulk test is line *shape*; one keyword-led line is still required.
    non_blank >= 5
        && leads >= 1
        && shaped * 5 >= non_blank * 3
        && (indented >= 3 || trimmed.contains('{'))
}

/// A line that reads as a statement, not a sentence: it closes a block or argument list, opens
/// one, ends a statement, assigns, or is a comment/decorator. Prose lines end in `.`/`!`/`?` or
/// a bare word, and essentially never assign with ` = `.
fn is_code_shaped(line: &str) -> bool {
    const ENDS: [char; 9] = [':', '{', '}', ';', '(', ')', ',', '[', ']'];
    const STARTS: [&str; 8] = ["}", ")", "]", "#", "//", "/*", "*", "@"];
    line.ends_with(ENDS)
        || STARTS.iter().any(|s| line.starts_with(s))
        || line.contains(" = ")
        || line.contains(" => ")
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
    fn detects_log_by_bracketed_lower_case_levels() {
        let s = "[error] a failed\n[notice] b\n[warn] c\n[info] d";
        assert_eq!(detect(s), ContentType::Log);
    }

    #[test]
    fn detects_python_traceback_as_a_log() {
        // Tracebacks arrive with no log-level line to give the level scan anything to bite on;
        // the traceback signature is what makes them recognisable.
        let s = "Traceback (most recent call last):\n  File \"/app/src/worker.py\", line 12, in run\n    boom\n";
        assert_eq!(detect(s), ContentType::Log);
    }

    #[test]
    fn a_source_file_quoting_a_traceback_in_a_doctest_stays_code() {
        let mut s = String::from(
            "class Point(Entity):\n    \"\"\"A point.\n\n    >>> Point([1])\n    \
             Traceback (most recent call last):\n      ...\n    ValueError: needs 2 coords.\n    \"\"\"\n",
        );
        for i in 0..12 {
            s.push_str(&format!(
                "\n    def op_{i}(self, other):\n        a = self.x + other\n        return a\n"
            ));
        }
        assert_eq!(detect(&s), ContentType::Code);
    }

    #[test]
    fn a_long_output_with_one_short_traceback_is_not_reclassified() {
        // Mostly ordinary output: the frames are too small a share to call the whole thing a log.
        let mut s: String = (0..60)
            .map(|i| format!("collected item {i} ok\n"))
            .collect();
        s.push_str("Traceback (most recent call last):\n  File \"a.py\", line 1, in <module>\n");
        s.push_str("ValueError: boom\n");
        assert_ne!(detect(&s), ContentType::Log);
    }

    #[test]
    fn detects_django_and_unittest_runs_as_test_output() {
        let django = "Testing against Django installed in '/repo/django'\n\
                      Found 614 test(s).\n\
                      Creating test database for alias 'default'...\n\
                      ........\n\
                      Ran 614 tests in 12.004s\n\nOK";
        assert_eq!(detect(django), ContentType::TestOutput);
        let failed = "E\n====\nERROR: test_x (a.B)\n----\nTraceback (most recent call last):\n  \
                      File \"a.py\", line 1, in test_x\nValueError: boom\n\nRan 1 test in 0.001s\n\n\
                      FAILED (errors=1)";
        assert_eq!(detect(failed), ContentType::TestOutput);
    }

    #[test]
    fn detects_pytest_runs_as_test_output() {
        let s = "============ test session starts ============\ncollected 3 items\n\n\
                 tests/test_a.py ...                    [100%]\n\n\
                 ============ 3 passed in 0.12s ============";
        assert_eq!(detect(s), ContentType::TestOutput);
    }

    #[test]
    fn source_that_builds_a_summary_string_is_not_test_output() {
        // Django's own runner prints the summary from an indented, quoted format string.
        let s = "class TextTestResult:\n    def printSummary(self, run, elapsed):\n        \
                 self.stream.writeln(\"Ran %d test%s in %.3fs\" % (run, run != 1 and \"s\" or \"\", elapsed))\n        \
                 return None\n\n    def stopTest(self, test):\n        super().stopTest(test)\n";
        assert_ne!(detect(s), ContentType::TestOutput);
    }

    #[test]
    fn detects_syslog_rfc3164_timestamps_as_a_log() {
        // RFC 3164 syslog dates: `Mon DD HH:MM:SS host tag[pid]: msg`.
        let s = "Jan  1 12:00:01 host agent[42]: ERROR failure number 7\n\
                 Jan  1 12:00:02 host agent[42]: INFO handled request 8\n\
                 Jan  1 12:00:03 host agent[42]: INFO ok\n\
                 Jan  1 12:00:04 host agent[42]: INFO done";
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

    fn few_keyword_python() -> &'static str {
        "import logging\nfrom typing import Any\n\n\nasync def main() -> None:\n    \
         logging.basicConfig(level=logging.INFO)\n    base = \"http://localhost\"\n    \
         for i in range(3):\n        print(base, i)\n    return None\n"
    }

    #[test]
    fn detects_code_that_uses_only_two_keywords() {
        assert_eq!(detect(few_keyword_python()), ContentType::Code);
    }

    #[test]
    fn detects_line_numbered_file_views_as_code() {
        for format in ["{:05}| {}", "{:>6}\t{}", "{:>6}→{}"] {
            let numbered: String = few_keyword_python()
                .lines()
                .enumerate()
                .map(|(i, l)| match format {
                    "{:05}| {}" => format!("{:05}| {l}\n", i + 1),
                    "{:>6}\t{}" => format!("{:>6}\t{l}\n", i + 1),
                    _ => format!("{:>6}→{l}\n", i + 1),
                })
                .collect();
            assert_eq!(detect(&numbered), ContentType::Code, "{format}");
        }
    }

    #[test]
    fn prose_with_some_indentation_is_not_code() {
        let s = "Release notes\n\nWe fixed several bugs this week.\n    Login is faster now.\n    \
                 Search handles typos.\n    Exports no longer time out.\nThanks to everyone who \
                 reported issues.\nMore next week.";
        assert_eq!(detect(s), ContentType::Prose);
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
