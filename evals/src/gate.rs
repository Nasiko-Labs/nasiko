//! The sanity check that decides whether a task is reproducible, and fills its
//! FAIL_TO_PASS and PASS_TO_PASS lists.
//!
//! Three set-ups are run in the task image: `pristine` (old code, old tests) once, `before`
//! (old code + the new tests) and `after` (the real fix + the new tests) several times each.
//! In Rust the new tests often call new functions, so `before` may not compile at all; then
//! `pristine` tells which tests already passed before the change.

use serde::{Deserialize, Serialize};

use crate::testlog::{TestResults, TestStatus};

/// One test run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    pub built: bool,
    pub results: TestResults,
}

/// All runs of one task.
#[derive(Debug, Clone, PartialEq)]
pub struct GateRuns {
    pub pristine: RunOutcome,
    pub before: Vec<RunOutcome>,
    pub after: Vec<RunOutcome>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestLists {
    pub fail_to_pass: Vec<String>,
    pub pass_to_pass: Vec<String>,
    /// Tests whose result changed between identical runs; left out of both lists.
    pub flaky: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum GateVerdict {
    Reproducible(TestLists),
    Rejected { reason: GateRejection },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateRejection {
    /// The real fix with its tests does not compile.
    GoldDoesNotBuild,
    /// No test goes from failing to passing, so nothing proves the task was done.
    NoFailToPass,
    /// Every candidate test was flaky.
    AllFlaky,
    /// Neither the old code nor the old code with the new tests compiled, so nothing tells
    /// which tests passed before the fix; counting them all as fixed would be a false task.
    BaseDoesNotBuild,
}

pub fn judge(runs: &GateRuns) -> GateVerdict {
    if runs.after.is_empty() || runs.after.iter().any(|r| !r.built) {
        return GateVerdict::Rejected {
            reason: GateRejection::GoldDoesNotBuild,
        };
    }
    let flaky = flaky_tests(&runs.after, &runs.before);
    let passes_after: Vec<&String> = runs.after[0]
        .results
        .iter()
        .filter(|(name, s)| **s == TestStatus::Passed && !flaky.contains(name))
        .map(|(name, _)| name)
        .collect();
    let before_built = !runs.before.is_empty() && runs.before.iter().all(|r| r.built);
    if !before_built && !runs.pristine.built {
        return GateVerdict::Rejected {
            reason: GateRejection::BaseDoesNotBuild,
        };
    }
    let mut lists = TestLists {
        fail_to_pass: Vec::new(),
        pass_to_pass: Vec::new(),
        flaky: flaky.clone(),
    };
    for name in passes_after {
        let passed_before = if before_built {
            runs.before[0].results.get(name) == Some(&TestStatus::Passed)
        } else {
            runs.pristine.results.get(name) == Some(&TestStatus::Passed)
        };
        if passed_before {
            lists.pass_to_pass.push(name.clone());
        } else {
            lists.fail_to_pass.push(name.clone());
        }
    }
    if lists.fail_to_pass.is_empty() {
        let reason = if flaky.is_empty() {
            GateRejection::NoFailToPass
        } else {
            GateRejection::AllFlaky
        };
        return GateVerdict::Rejected { reason };
    }
    GateVerdict::Reproducible(lists)
}

/// Tests whose status differs between runs of the same set-up.
fn flaky_tests(after: &[RunOutcome], before: &[RunOutcome]) -> Vec<String> {
    let mut flaky = Vec::new();
    for runs in [after, before] {
        let built: Vec<&RunOutcome> = runs.iter().filter(|r| r.built).collect();
        let Some(first) = built.first() else { continue };
        for (name, status) in &first.results {
            if built.iter().any(|r| r.results.get(name) != Some(status)) && !flaky.contains(name) {
                flaky.push(name.clone());
            }
        }
    }
    flaky.sort();
    flaky
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(built: bool, results: &[(&str, TestStatus)]) -> RunOutcome {
        RunOutcome {
            built,
            results: results.iter().map(|(n, s)| (n.to_string(), *s)).collect(),
        }
    }
    use TestStatus::{Failed, Passed};

    #[test]
    fn new_test_that_fails_before_is_fail_to_pass() {
        let runs = GateRuns {
            pristine: run(true, &[("old", Passed)]),
            before: vec![run(true, &[("old", Passed), ("new", Failed)]); 3],
            after: vec![run(true, &[("old", Passed), ("new", Passed)]); 3],
        };
        let GateVerdict::Reproducible(l) = judge(&runs) else {
            panic!("reproducible")
        };
        assert_eq!(l.fail_to_pass, vec!["new"]);
        assert_eq!(l.pass_to_pass, vec!["old"]);
    }

    #[test]
    fn when_before_does_not_compile_pristine_decides() {
        let runs = GateRuns {
            pristine: run(true, &[("old", Passed)]),
            before: vec![run(false, &[]); 3],
            after: vec![run(true, &[("old", Passed), ("new", Passed)]); 3],
        };
        let GateVerdict::Reproducible(l) = judge(&runs) else {
            panic!("reproducible")
        };
        assert_eq!(l.fail_to_pass, vec!["new"]);
        assert_eq!(l.pass_to_pass, vec!["old"]);
    }

    #[test]
    fn when_the_old_code_never_compiles_no_test_counts_as_fixed() {
        let runs = GateRuns {
            pristine: run(false, &[]),
            before: vec![run(false, &[]); 3],
            after: vec![run(true, &[("old", Passed), ("new", Passed)]); 3],
        };
        assert_eq!(
            judge(&runs),
            GateVerdict::Rejected {
                reason: GateRejection::BaseDoesNotBuild
            }
        );
    }

    #[test]
    fn flaky_tests_are_dropped_and_gold_must_build() {
        let runs = GateRuns {
            pristine: run(true, &[]),
            before: vec![run(true, &[("new", Failed)]); 3],
            after: vec![
                run(true, &[("new", Passed)]),
                run(true, &[("new", Failed)]),
                run(true, &[("new", Passed)]),
            ],
        };
        assert_eq!(
            judge(&runs),
            GateVerdict::Rejected {
                reason: GateRejection::AllFlaky
            }
        );
        let broken = GateRuns {
            after: vec![run(false, &[])],
            ..runs
        };
        assert_eq!(
            judge(&broken),
            GateVerdict::Rejected {
                reason: GateRejection::GoldDoesNotBuild
            }
        );
    }

    const RANGOT_VITEST_FAILING: &str = "\
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
";

    fn rangot_vitest_outcome(log: &str) -> RunOutcome {
        RunOutcome {
            built: true,
            results: crate::testlog::parse(crate::testlog::LogParserKind::Vitest, log),
        }
    }

    #[test]
    fn rangot_vitest_fail_to_pass_is_reproducible() {
        let passing_log = RANGOT_VITEST_FAILING.replace("\x1b[31m×\x1b[39m", "\x1b[32m✓\x1b[39m");
        let runs = GateRuns {
            pristine: run(true, &[]),
            before: vec![rangot_vitest_outcome(RANGOT_VITEST_FAILING); 3],
            after: vec![rangot_vitest_outcome(&passing_log); 3],
        };
        let GateVerdict::Reproducible(lists) = judge(&runs) else {
            panic!("expected reproducible verdict");
        };
        assert_eq!(
            lists.fail_to_pass,
            vec![
                "src/utils/festivalTimer.test.ts > getDaysUntilFestival > same day > returns 0 when both dates are identical"
            ]
        );
        assert_eq!(lists.pass_to_pass.len(), 31);
        assert!(lists.flaky.is_empty());
    }

    #[test]
    fn rangot_vitest_flaky_fail_to_pass_rejects_all_flaky() {
        let passing_log = RANGOT_VITEST_FAILING.replace("\x1b[31m×\x1b[39m", "\x1b[32m✓\x1b[39m");
        let runs = GateRuns {
            pristine: run(true, &[]),
            before: vec![rangot_vitest_outcome(RANGOT_VITEST_FAILING); 3],
            after: vec![
                rangot_vitest_outcome(&passing_log),
                rangot_vitest_outcome(RANGOT_VITEST_FAILING),
                rangot_vitest_outcome(&passing_log),
            ],
        };
        assert_eq!(
            judge(&runs),
            GateVerdict::Rejected {
                reason: GateRejection::AllFlaky,
            }
        );
    }
}
