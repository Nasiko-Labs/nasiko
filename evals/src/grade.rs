//! Scoring one attempt with SWE-bench's rule: resolved only when every FAIL_TO_PASS and every
//! PASS_TO_PASS test passes.

use serde::{Deserialize, Serialize};

use crate::spec::{EXIT_BUILD, EXIT_MODEL_PATCH, EXIT_TEST_PATCH};
use crate::testlog::{TestResults, TestStatus};

/// How the agent's run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExit {
    Finished,
    StepLimit,
    TimeLimit,
    DiskLimit,
    Crashed,
    InfraError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Full,
    Partial,
    No,
}

/// Why an attempt was not resolved, first that applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    InfraError,
    /// The agent's container died (e.g. out of memory) before its changes were collected.
    AgentCrashed,
    NoPatch,
    Budget,
    PatchFailed,
    BuildFailed,
    Regression,
    TestsFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grade {
    pub resolution: Resolution,
    pub resolved: bool,
    pub f2p_passed: u32,
    pub f2p_total: u32,
    pub p2p_passed: u32,
    pub p2p_total: u32,
    pub failure: Option<FailureClass>,
}

/// Everything grading needs about one attempt.
pub struct GradeInput<'a> {
    pub fail_to_pass: &'a [String],
    pub pass_to_pass: &'a [String],
    pub agent_exit: AgentExit,
    pub patch_is_empty: bool,
    /// Exit code of the grading script; `None` when it never ran.
    pub script_exit: Option<i32>,
    pub results: &'a TestResults,
}

pub fn grade(input: &GradeInput) -> Grade {
    let count = |names: &[String]| {
        names
            .iter()
            .filter(|n| input.results.get(*n) == Some(&TestStatus::Passed))
            .count() as u32
    };
    let (f2p_total, p2p_total) = (
        input.fail_to_pass.len() as u32,
        input.pass_to_pass.len() as u32,
    );
    let (f2p_passed, p2p_passed) = (count(input.fail_to_pass), count(input.pass_to_pass));
    let resolution = if f2p_passed == f2p_total && p2p_passed == p2p_total {
        Resolution::Full
    } else if f2p_passed > 0 && p2p_passed == p2p_total {
        Resolution::Partial
    } else {
        Resolution::No
    };
    let resolved = resolution == Resolution::Full && !input.patch_is_empty;
    let failure = (!resolved).then(|| failure_class(input, p2p_passed < p2p_total));
    Grade {
        resolution,
        resolved,
        f2p_passed,
        f2p_total,
        p2p_passed,
        p2p_total,
        failure,
    }
}

fn failure_class(input: &GradeInput, broke_something: bool) -> FailureClass {
    match (input.agent_exit, input.script_exit) {
        (AgentExit::InfraError, _) | (_, None) => FailureClass::InfraError,
        _ if input.patch_is_empty => match input.agent_exit {
            AgentExit::StepLimit | AgentExit::TimeLimit | AgentExit::DiskLimit => {
                FailureClass::Budget
            }
            AgentExit::Crashed => FailureClass::AgentCrashed,
            _ => FailureClass::NoPatch,
        },
        (_, Some(EXIT_MODEL_PATCH)) => FailureClass::PatchFailed,
        (_, Some(EXIT_TEST_PATCH)) => FailureClass::PatchFailed,
        (_, Some(EXIT_BUILD)) => FailureClass::BuildFailed,
        _ if broke_something => FailureClass::Regression,
        _ => FailureClass::TestsFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn results(passed: &[&str]) -> TestResults {
        passed
            .iter()
            .map(|n| (n.to_string(), TestStatus::Passed))
            .collect()
    }

    fn input<'a>(r: &'a TestResults, f2p: &'a [String], p2p: &'a [String]) -> GradeInput<'a> {
        GradeInput {
            fail_to_pass: f2p,
            pass_to_pass: p2p,
            agent_exit: AgentExit::Finished,
            patch_is_empty: false,
            script_exit: Some(0),
            results: r,
        }
    }

    #[test]
    fn all_tests_passing_is_resolved() {
        let (f2p, p2p) = (vec!["new".to_string()], vec!["old".to_string()]);
        let r = results(&["new", "old"]);
        let g = grade(&input(&r, &f2p, &p2p));
        assert!(g.resolved);
        assert_eq!(g.failure, None);
    }

    #[test]
    fn breaking_an_old_test_is_a_regression() {
        let (f2p, p2p) = (vec!["new".to_string()], vec!["old".to_string()]);
        let r = results(&["new"]);
        let g = grade(&input(&r, &f2p, &p2p));
        assert_eq!(
            (g.resolution, g.failure),
            (Resolution::No, Some(FailureClass::Regression))
        );
    }

    #[test]
    fn exit_codes_and_empty_patches_are_classified() {
        let (f2p, p2p) = (vec!["new".to_string()], vec![]);
        let r = TestResults::new();
        let mut i = input(&r, &f2p, &p2p);
        i.script_exit = Some(EXIT_BUILD);
        assert_eq!(grade(&i).failure, Some(FailureClass::BuildFailed));
        i.patch_is_empty = true;
        i.agent_exit = AgentExit::StepLimit;
        assert_eq!(grade(&i).failure, Some(FailureClass::Budget));
        i.agent_exit = AgentExit::Crashed;
        assert_eq!(grade(&i).failure, Some(FailureClass::AgentCrashed));
        i.agent_exit = AgentExit::Finished;
        assert_eq!(grade(&i).failure, Some(FailureClass::NoPatch));
        i.script_exit = None;
        assert_eq!(grade(&i).failure, Some(FailureClass::InfraError));
    }

    #[test]
    fn some_new_tests_passing_is_partial() {
        let (f2p, p2p) = (vec!["a".to_string(), "b".to_string()], vec![]);
        let r = results(&["a"]);
        let g = grade(&input(&r, &f2p, &p2p));
        assert_eq!((g.resolution, g.resolved), (Resolution::Partial, false));
        assert_eq!(g.failure, Some(FailureClass::TestsFailed));
    }
}
