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
            match runs.before[0].results.get(name) {
                Some(status) => *status == TestStatus::Passed,
                // Missing before the fix: its whole file failed to load with the new tests (in
                // Python, an import of a function the fix adds). If it passed on the untouched
                // code it is an old test the fix must keep passing, not one the fix makes pass;
                // counting it as fixed would hand a model credit for a stub that only imports.
                None => runs.pristine.results.get(name) == Some(&TestStatus::Passed),
            }
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

    /// `pytest -rA` output with only its summary, as `testlog` reads it.
    fn pytest(summary: &str) -> RunOutcome {
        let log = format!(
            "== short test summary info ==
{summary}"
        );
        RunOutcome {
            built: true,
            results: crate::testlog::parse(crate::testlog::LogParserKind::Pytest, &log),
        }
    }

    #[test]
    fn a_python_test_file_that_cannot_import_before_the_fix_keeps_its_old_tests_as_pass_to_pass() {
        // The test patch adds `test_empty_is_error` to a file whose old test passed on the
        // untouched code. Before the fix the file imports a function that does not exist yet,
        // so pytest names none of its tests.
        let before = "ERROR tests/test_dur.py - ImportError: cannot import name 'parse'
                      PASSED tests/test_other.py::test_kept
";
        let after = "PASSED tests/test_dur.py::test_empty_is_error
                     PASSED tests/test_dur.py::test_old
                     PASSED tests/test_other.py::test_kept
";
        let pristine = "PASSED tests/test_dur.py::test_old
";
        let verdict = judge(&GateRuns {
            pristine: pytest(pristine),
            before: vec![pytest(before), pytest(before)],
            after: vec![pytest(after), pytest(after)],
        });
        let GateVerdict::Reproducible(lists) = verdict else {
            panic!("{verdict:?}");
        };
        assert_eq!(
            lists.fail_to_pass,
            ["tests/test_dur.py::test_empty_is_error"]
        );
        assert_eq!(
            lists.pass_to_pass,
            [
                "tests/test_dur.py::test_old",
                "tests/test_other.py::test_kept"
            ]
        );
    }

    #[test]
    fn a_python_test_that_flips_between_identical_runs_is_dropped_as_flaky() {
        let after_a = "PASSED tests/t.py::test_fix
PASSED tests/t.py::test_clock
";
        let after_b = "PASSED tests/t.py::test_fix
FAILED tests/t.py::test_clock - late
";
        let before = "FAILED tests/t.py::test_fix - no
FAILED tests/t.py::test_clock - late
";
        let verdict = judge(&GateRuns {
            pristine: pytest(""),
            before: vec![pytest(before), pytest(before)],
            after: vec![pytest(after_a), pytest(after_b)],
        });
        let GateVerdict::Reproducible(lists) = verdict else {
            panic!("{verdict:?}");
        };
        assert_eq!(lists.fail_to_pass, ["tests/t.py::test_fix"]);
        assert_eq!(lists.flaky, ["tests/t.py::test_clock"]);
    }

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
}
