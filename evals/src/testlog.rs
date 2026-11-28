//! Turning test-runner output into one status per test.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Which test runner wrote the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogParserKind {
    Cargo,
    Vitest,
    Pytest,
}

impl LogParserKind {
    /// The matching parser's name in SWE-bench (`log_parser` field), so the public grader can
    /// re-check a dataset.
    pub fn swebench_name(self) -> &'static str {
        match self {
            LogParserKind::Cargo => "parse_log_cargo",
            LogParserKind::Vitest => "parse_log_vitest",
            LogParserKind::Pytest => "parse_log_pytest",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    Passed,
    Failed,
    Ignored,
}

/// Test name → status. Names are prefixed with their test binary (`crate::path::test`), since
/// two binaries can hold tests of the same name.
pub type TestResults = BTreeMap<String, TestStatus>;

pub fn parse(kind: LogParserKind, output: &str) -> TestResults {
    match kind {
        LogParserKind::Cargo => parse_cargo(output),
        LogParserKind::Vitest => parse_vitest(output),
        LogParserKind::Pytest => parse_pytest(output),
    }
}

/// Reads `cargo test` output: `Running … (target/debug/deps/<bin>-<hash>)` or
/// `Doc-tests <crate>` start a binary; `test <name> ... ok|FAILED|ignored` is one result.
fn parse_cargo(output: &str) -> TestResults {
    let mut results = TestResults::new();
    let mut binary = String::new();
    for line in output.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("Running ") {
            binary = binary_name(rest);
        } else if let Some(krate) = line.strip_prefix("Doc-tests ") {
            binary = format!("doc:{}", krate.trim());
        } else if let Some(rest) = line.strip_prefix("test ")
            && let Some((name, verdict)) = rest.rsplit_once(" ... ")
        {
            let status = match verdict.split([',', ' ']).next() {
                Some("ok") => TestStatus::Passed,
                Some("FAILED") => TestStatus::Failed,
                Some("ignored") => TestStatus::Ignored,
                _ => continue,
            };
            results.insert(format!("{binary}::{}", name.trim()), status);
        }
    }
    results
}

/// Reads `vitest run --reporter=verbose` output: strips ANSI escapes, matches `✓` / `×` / `↓`
/// test lines, and drops trailing durations (`4ms`, `1.25s`) and `[skipped]` tags.
fn parse_vitest(output: &str) -> TestResults {
    let mut results = TestResults::new();
    for line in output.lines() {
        let clean = strip_ansi(line);
        let trimmed = clean.trim();
        let (status, rest) = if let Some(rest) = trimmed.strip_prefix("✓ ") {
            (TestStatus::Passed, rest)
        } else if let Some(rest) = trimmed.strip_prefix("× ") {
            (TestStatus::Failed, rest)
        } else if let Some(rest) = trimmed.strip_prefix("↓ ") {
            (TestStatus::Ignored, rest)
        } else {
            continue;
        };
        let name = strip_vitest_suffixes(rest);
        if !name.is_empty() {
            results.insert(name.to_string(), status);
        }
    }
    results
}

/// Reads `pytest -v` output: strips ANSI escapes, drops trailing `[ 50%]` progress percentages,
/// and matches `<node_id> PASSED|FAILED|SKIPPED|XFAIL|ERROR` lines where `<node_id>` contains `::`.
fn parse_pytest(output: &str) -> TestResults {
    let mut results = TestResults::new();
    for line in output.lines() {
        let clean = strip_ansi(line);
        let trimmed = clean.trim();
        let without_progress = match trimmed.strip_suffix(']') {
            Some(before_bracket) => match before_bracket.rsplit_once('[') {
                Some((before, pct)) if pct.trim().ends_with('%') => before.trim_end(),
                _ => trimmed,
            },
            None => trimmed,
        };
        let without_reason = if let Some((before, reason)) = without_progress.rsplit_once(" (")
            && reason.ends_with(')')
        {
            before.trim_end()
        } else {
            without_progress
        };
        let Some((node_id, verdict)) = without_reason.rsplit_once(char::is_whitespace) else {
            continue;
        };
        let node_id = node_id.trim();
        if node_id.is_empty() || !node_id.contains("::") {
            continue;
        }
        let status = match verdict {
            "PASSED" => TestStatus::Passed,
            "FAILED" | "ERROR" => TestStatus::Failed,
            "SKIPPED" | "XFAIL" => TestStatus::Ignored,
            _ => continue,
        };
        results.insert(node_id.to_string(), status);
    }
    results
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for seq in chars.by_ref() {
                if seq.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn strip_vitest_suffixes(mut s: &str) -> &str {
    loop {
        s = s.trim();
        if let Some(prefix) = s.strip_suffix("[skipped]") {
            s = prefix;
            continue;
        }
        if let Some((prefix, last)) = s.rsplit_once(' ')
            && is_duration_token(last)
        {
            s = prefix;
            continue;
        }
        return s;
    }
}

fn is_duration_token(token: &str) -> bool {
    let Some(num) = token.strip_suffix("ms").or_else(|| token.strip_suffix('s')) else {
        return false;
    };
    !num.is_empty() && num.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// `unittests src/lib.rs (target/debug/deps/nasiko_compress-1a2b)` → `nasiko_compress`;
/// `tests/it.rs (target/debug/deps/it-9f)` → `it`.
fn binary_name(running: &str) -> String {
    let inside = running
        .rsplit_once('(')
        .map(|(_, r)| r.trim_end_matches(')'))
        .unwrap_or(running);
    let file = inside.rsplit('/').next().unwrap_or(inside);
    match file.rsplit_once('-') {
        Some((name, hash)) if hash.chars().all(|c| c.is_ascii_hexdigit()) => name.to_string(),
        _ => file.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = "   Compiling x v0.1.0
     Running unittests src/lib.rs (target/debug/deps/nasiko_compress-0f1e2d3c)

running 3 tests
test detect::tests::finds_json ... ok
test policy::tests::slow ... ignored, needs network
test text::tests::wraps ... FAILED

     Running tests/it.rs (target/debug/deps/it-abc123)
test detect::tests::finds_json ... ok
   Doc-tests nasiko_compress
test src/lib.rs - compress (line 10) ... ok
";

    #[test]
    fn reads_every_binary_and_status() {
        let r = parse(LogParserKind::Cargo, OUT);
        assert_eq!(
            r.get("nasiko_compress::detect::tests::finds_json"),
            Some(&TestStatus::Passed)
        );
        assert_eq!(
            r.get("nasiko_compress::policy::tests::slow"),
            Some(&TestStatus::Ignored)
        );
        assert_eq!(
            r.get("nasiko_compress::text::tests::wraps"),
            Some(&TestStatus::Failed)
        );
        assert_eq!(
            r.get("it::detect::tests::finds_json"),
            Some(&TestStatus::Passed)
        );
        assert_eq!(
            r.get("doc:nasiko_compress::src/lib.rs - compress (line 10)"),
            Some(&TestStatus::Passed)
        );
        assert_eq!(r.len(), 5);
    }

    const RANGOT_VITEST_OUT: &str = "\
\x1b[1m\x1b[30m\x1b[46m RUN \x1b[49m\x1b[39m\x1b[22m \x1b[36mv5.1.2 \x1b[39m\x1b[90m/home/p/proj/rangot\x1b[39m

 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mfuture dates\x1b[2m > \x1b[22mcounts one day for the next day\x1b[32m 4\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mfuture dates\x1b[2m > \x1b[22mcounts days within the same month\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mfuture dates\x1b[2m > \x1b[22mcounts across a month boundary\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mfuture dates\x1b[2m > \x1b[22mcounts across a year boundary\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mfuture dates\x1b[2m > \x1b[22mcounts a full leap year correctly\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mfuture dates\x1b[2m > \x1b[22mcounts over a non-leap February\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mfuture dates\x1b[2m > \x1b[22mcounts over a leap February\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mfuture dates\x1b[2m > \x1b[22mcounts a long span in the future\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[31m×\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22msame day\x1b[2m > \x1b[22mreturns 0 when both dates are identical\x1b[32m 12\x1b[2mms\x1b[22m\x1b[39m
\x1b[31m   → expected +0 to be 999 // Object.is equality\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22msame day\x1b[2m > \x1b[22mreturns 0 for the same leap day\x1b[32m 1\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mpast dates\x1b[2m > \x1b[22mreturns 0 when the festival was yesterday\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mpast dates\x1b[2m > \x1b[22mreturns 0 for a date many months in the past\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mpast dates\x1b[2m > \x1b[22mreturns 0 across a year boundary in the past\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mcalendar handling\x1b[2m > \x1b[22mhandles year 0000 without remapping to 1900\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22mcalendar handling\x1b[2m > \x1b[22mhandles years 0–99 without remapping to the 1900s\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows when targetDateISO is an empty string\x1b[32m 2\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows when nowDateISO is an empty string\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for a non-date string\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for a wrong separator\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for a day-first format\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for a missing zero-padding\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for a date-time string instead of a date\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for a string with surrounding whitespace\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for a month out of range\x1b[32m 3\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for a month of zero\x1b[32m 1\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for a day out of range for the month\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for Feb 29 in a non-leap year\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for a day of zero\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows when nowDateISO is invalid even if target is valid\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows when both dates are invalid, naming the target first\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows for non-string input at runtime\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m
 \x1b[32m✓\x1b[39m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22minvalid input\x1b[2m > \x1b[22mthrows an Error instance\x1b[32m 0\x1b[2mms\x1b[22m\x1b[39m

\x1b[41m\x1b[1m FAIL \x1b[22m\x1b[49m src/utils/festivalTimer.test.ts\x1b[2m > \x1b[22mgetDaysUntilFestival\x1b[2m > \x1b[22msame day\x1b[2m > \x1b[22mreturns 0 when both dates are identical
\x1b[31m\x1b[1mAssertionError\x1b[22m: expected +0 to be 999 // Object.is equality\x1b[39m

\x1b[2m Test Files \x1b[22m \x1b[1m\x1b[31m1 failed\x1b[22m\x1b[90m (1)\x1b[39m
\x1b[2m      Tests \x1b[22m \x1b[1m\x1b[31m1 failed\x1b[22m\x1b[2m | \x1b[22m\x1b[1m\x1b[32m31 passed\x1b[22m\x1b[90m (32)\x1b[39m
\x1b[2m   Duration \x1b[22m 374ms\x1b[2m (transform 58%, tests 20%, import 17%, worker 5%)\x1b[22m
";

    #[test]
    fn parses_rangot_vitest_verbose_output() {
        assert_eq!(LogParserKind::Vitest.swebench_name(), "parse_log_vitest");
        let r = parse(LogParserKind::Vitest, RANGOT_VITEST_OUT);
        assert_eq!(r.len(), 32);
        assert_eq!(r.values().filter(|s| **s == TestStatus::Passed).count(), 31);
        assert_eq!(
            r.get("src/utils/festivalTimer.test.ts > getDaysUntilFestival > same day > returns 0 when both dates are identical"),
            Some(&TestStatus::Failed)
        );
        assert_eq!(
            r.get("src/utils/festivalTimer.test.ts > getDaysUntilFestival > future dates > counts one day for the next day"),
            Some(&TestStatus::Passed)
        );
        assert!(r.keys().all(|k| !k.contains('\x1b') && !k.ends_with("ms")));
    }

    #[test]
    fn parses_vitest_skipped_and_seconds_durations() {
        let out = "\
 \x1b[32m✓\x1b[39m src/a.test.ts > suite > slow test 1.25s
 \x1b[33m↓\x1b[39m src/a.test.ts > suite > skipped test [skipped] 0ms
";
        let r = parse(LogParserKind::Vitest, out);
        assert_eq!(
            r.get("src/a.test.ts > suite > slow test"),
            Some(&TestStatus::Passed)
        );
        assert_eq!(
            r.get("src/a.test.ts > suite > skipped test"),
            Some(&TestStatus::Ignored)
        );
        assert_eq!(r.len(), 2);
    }

    const PYTEST_VERBOSE_OUT: &str = "\
\x1b[1m============================= test session starts ==============================\x1b[0m
platform linux -- Python 3.12.0, pytest-8.3.2, pluggy-1.5.0 -- /usr/local/bin/python
cachedir: .pytest_cache
rootdir: /testbed
collecting ... collected 6 items

tests/test_math.py::TestCalc::test_add \x1b[32mPASSED\x1b[0m                            \x1b[36m[ 16%]\x1b[0m
tests/test_math.py::TestCalc::test_sub \x1b[31mFAILED\x1b[0m                            \x1b[36m[ 33%]\x1b[0m
tests/test_math.py::test_setup_err \x1b[31mERROR\x1b[0m                                 \x1b[36m[ 50%]\x1b[0m
tests/test_math.py::test_skip \x1b[33mSKIPPED\x1b[0m (not on linux)                   \x1b[36m[ 66%]\x1b[0m
tests/test_math.py::test_skip_clean \x1b[33mSKIPPED\x1b[0m                              \x1b[36m[ 83%]\x1b[0m
tests/test_math.py::test_expected_fail \x1b[33mXFAIL\x1b[0m                             \x1b[36m[100%]\x1b[0m

=================================== FAILURES ===================================
______________________________ TestCalc.test_sub _______________________________

    def test_sub(self):
>       assert 5 - 3 == 1
E       assert 2 == 1

tests/test_math.py:12: AssertionError
=========================== short test summary info ============================
FAILED tests/test_math.py::TestCalc::test_sub - assert 2 == 1
ERROR tests/test_math.py::test_setup_err - RuntimeError: boom
\x1b[31m============== 1 failed, 1 passed, 1 skipped, 1 xfailed, 1 error in 0.05s ==============\x1b[0m
";

    #[test]
    fn parses_pytest_verbose_output() {
        assert_eq!(LogParserKind::Pytest.swebench_name(), "parse_log_pytest");
        let r = parse(LogParserKind::Pytest, PYTEST_VERBOSE_OUT);
        assert_eq!(
            r.get("tests/test_math.py::TestCalc::test_add"),
            Some(&TestStatus::Passed)
        );
        assert_eq!(
            r.get("tests/test_math.py::TestCalc::test_sub"),
            Some(&TestStatus::Failed)
        );
        assert_eq!(
            r.get("tests/test_math.py::test_setup_err"),
            Some(&TestStatus::Failed)
        );
        assert_eq!(
            r.get("tests/test_math.py::test_skip"),
            Some(&TestStatus::Ignored)
        );
        assert_eq!(
            r.get("tests/test_math.py::test_skip_clean"),
            Some(&TestStatus::Ignored)
        );
        assert_eq!(
            r.get("tests/test_math.py::test_expected_fail"),
            Some(&TestStatus::Ignored)
        );
        assert_eq!(r.len(), 6);
        assert!(r.keys().all(|k| !k.contains('\x1b')));
    }
}
