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
    let mut lists = TestLists {
        fail_to_pass: Vec::new(),
        pass_to_pass: Vec::new(),
        flaky: flaky.clone(),
    };
    for name in passes_after {
        let passed_before = if before_built {
            runs.before[0].results.get(name) == Some(&TestStatus::Passed)
        } else {
            runs.pristine.built && runs.pristine.results.get(name) == Some(&TestStatus::Passed)
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
}
