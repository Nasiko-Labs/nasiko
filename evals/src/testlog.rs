//! Turning test-runner output into one status per test.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::junit;

pub use crate::junit::{JUNIT_BEGIN, JUNIT_END, JUNIT_FILE};

/// Which test runner wrote the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogParserKind {
    Cargo,
    /// JUnit XML reports between [`JUNIT_BEGIN`] and [`JUNIT_END`] ([`crate::junit`]): what
    /// node:test, vitest, jest-junit and pytest's `--junitxml` all write.
    Junit,
}

impl LogParserKind {
    /// The matching parser's name in SWE-bench (`log_parser` field), so the public grader can
    /// re-check a dataset. SWE-bench has no JUnit parser; `parse_log_junit` names this one.
    pub fn swebench_name(self) -> &'static str {
        match self {
            LogParserKind::Cargo => "parse_log_cargo",
            LogParserKind::Junit => "parse_log_junit",
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

/// Test name → status. Cargo names are prefixed with their test binary (`crate::path::test`),
/// since two binaries can hold tests of the same name; JUnit names with their test file and
/// suites (`test/finch.test.ts::outer::inner::falls back`).
pub type TestResults = BTreeMap<String, TestStatus>;

pub fn parse(kind: LogParserKind, output: &str) -> TestResults {
    match kind {
        LogParserKind::Cargo => parse_cargo(output),
        LogParserKind::Junit => junit::parse(output),
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
}
