//! Turning test-runner output into one status per test.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Which test runner wrote the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogParserKind {
    Cargo,
    Pytest,
}

impl LogParserKind {
    /// The matching parser's name in SWE-bench (`log_parser` field), so the public grader can
    /// re-check a dataset.
    pub fn swebench_name(self) -> &'static str {
        match self {
            LogParserKind::Cargo => "parse_log_cargo",
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
        LogParserKind::Pytest => parse_pytest(output),
    }
}

/// Reads the `short test summary info` section that `pytest -rA` ends with: one
/// `STATUS path::test[ - message]` line per test. Only that section is read, so a test that
/// prints `PASSED` itself cannot fake a result. As in SWE-bench, an expected failure (`XFAIL`)
/// counts as passing and an error as failing; a skip names no test, and an unexpected pass
/// (`XPASS`) proves nothing either way, so both are ignored. A file that failed to import
/// (`ERROR path - …`, no `::`) names no test either: its tests are simply missing, which
/// reads as not passing.
fn parse_pytest(output: &str) -> TestResults {
    let mut results = TestResults::new();
    let summary = output
        .lines()
        .skip_while(|l| !l.contains("short test summary info"))
        .skip(1);
    for line in summary.map(str::trim) {
        let Some((verdict, rest)) = line.split_once(' ') else {
            continue;
        };
        let status = match verdict {
            "PASSED" | "XFAIL" => TestStatus::Passed,
            "FAILED" | "ERROR" => TestStatus::Failed,
            "XPASS" => TestStatus::Ignored,
            _ => continue,
        };
        let name = pytest_node_id(verdict, rest);
        if name.contains("::") {
            results.insert(name.to_string(), status);
        }
    }
    results
}

/// The test id of a summary line, without the ` - message` pytest appends to every status but
/// `PASSED`. A parametrized id (`test_units[1h - one hour]`) may hold ` - ` itself, so its
/// brackets are kept whole.
fn pytest_node_id<'a>(verdict: &str, rest: &'a str) -> &'a str {
    let rest = rest.trim();
    if verdict == "PASSED" {
        return rest;
    }
    if let Some(open) = rest.find('[')
        && let Some(close) = rest[open..].find("] - ").map(|i| open + i + 1)
    {
        return &rest[..close];
    }
    rest.split_once(" - ").map_or(rest, |(name, _)| name).trim()
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

    const PYTEST_OUT: &str =
        "============================= test session starts ==============================
collected 6 items

tests/test_dur.py .F.sx                                                  [ 83%]
tests/test_parse.py E                                                    [100%]

==================================== PASSES ====================================
____________________________ test_prints_a_verdict _____________________________
----------------------------- Captured stdout call -----------------------------
PASSED tests/test_dur.py::test_fake_from_stdout
=========================== short test summary info ============================
PASSED tests/test_dur.py::test_empty_is_error
PASSED tests/test_dur.py::test_units[1h - one hour]
FAILED tests/test_dur.py::test_negative - AssertionError: assert -1 == 1
FAILED tests/test_dur.py::test_units[2h - two hours] - AssertionError: 7200
SKIPPED [1] tests/test_dur.py:30: needs network
XFAIL tests/test_dur.py::test_known_bug - reason: issue 12
XPASS tests/test_dur.py::test_lucky
ERROR tests/test_parse.py - ImportError: cannot import name 'parse' from 'dur'
ERROR tests/test_dur.py::test_fixture_broke - RuntimeError: boom
============= 1 failed, 2 passed, 1 skipped, 1 xfailed, 2 errors in 0.12s =============
";

    #[test]
    fn pytest_reads_only_the_summary_with_swebench_statuses() {
        let r = parse(LogParserKind::Pytest, PYTEST_OUT);
        let status = |name: &str| r.get(&format!("tests/test_dur.py::{name}")).copied();
        assert_eq!(status("test_empty_is_error"), Some(TestStatus::Passed));
        // A parametrized id may hold ` - ` itself.
        assert_eq!(
            status("test_units[1h - one hour]"),
            Some(TestStatus::Passed)
        );
        assert_eq!(
            status("test_units[2h - two hours]"),
            Some(TestStatus::Failed)
        );
        assert_eq!(status("test_negative"), Some(TestStatus::Failed));
        assert_eq!(status("test_known_bug"), Some(TestStatus::Passed));
        assert_eq!(status("test_lucky"), Some(TestStatus::Ignored));
        assert_eq!(status("test_fixture_broke"), Some(TestStatus::Failed));
        assert_eq!(
            status("test_fake_from_stdout"),
            None,
            "captured output is not a result"
        );
        assert!(!r.keys().any(|k| k.starts_with("tests/test_parse.py")));
        assert_eq!(r.len(), 7);
    }

    #[test]
    fn pytest_output_without_a_summary_has_no_results() {
        let out = "ERROR: file or directory not found: tests/test_gone.py\n";
        assert!(parse(LogParserKind::Pytest, out).is_empty());
    }
}
