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
    /// The real fix with its tests does not compile, although the old code alone does.
    GoldDoesNotBuild,
    /// No test goes from failing to passing, so nothing proves the task was done.
    NoFailToPass,
    /// Every candidate test was flaky.
    AllFlaky,
    /// The build command fails on the old code in the task image with no patch applied. Either
    /// the fix did not compile on top of it (then the fix was not judged at all), or it did
    /// and the old code with the new tests did not either, so nothing tells which tests
    /// passed before the fix; counting them all as fixed would be a false task.
    ///
    /// A run stopped at a limit also counts as not built, so this reason covers a pristine
    /// run that ran out of time or disk; the caller knows which and should say so.
    BaseDoesNotBuild,
}

impl GateRejection {
    /// One plain sentence a user can act on: what the gate saw, and what to try. Written for
    /// a build that failed; a run stopped at a limit needs other words from the caller.
    pub fn explain(self) -> &'static str {
        match self {
            GateRejection::GoldDoesNotBuild => {
                "the build command works on the old code but fails on the real fix with its \
                 tests: read the first error; if the build was killed, raise memory_mib in \
                 the spec"
            }
            GateRejection::NoFailToPass => {
                "no test went from failing to passing, so nothing would show the task was done"
            }
            GateRejection::AllFlaky => {
                "every test that could prove the fix changed its result between identical runs"
            }
            GateRejection::BaseDoesNotBuild => {
                "the build command fails on the old code in this image with no patch applied, \
                 so the fix was not judged. Read the first error. Often it is an old commit \
                 on a newer compiler: pin an older toolchain_image in the spec. If the error \
                 is a lint, the lint cap is not reaching the compiler: check that the spec \
                 has cap_lints on, that its commands do not set RUSTFLAGS, and that the \
                 repository's .cargo/config.toml sets no target rustflags (those switch the \
                 cap off). If the build was killed, raise memory_mib"
            }
        }
    }
}

/// The verdict on one task's runs.
///
/// The pristine run is read before the fix is blamed: when the old code does not build with no
/// patch applied, a fix that does not build on top of it shows nothing about the fix.
///
/// The caller decides what `built` means for the pristine run. A pristine run that stopped
/// only because the task's scope names a test target the old code does not have yet
/// ([`scope_absent_on_base`]) must be passed as built with no results, or a fix that really
/// does not build would be read as a base that does not.
pub fn judge(runs: &GateRuns) -> GateVerdict {
    if runs.after.is_empty() || runs.after.iter().any(|r| !r.built) {
        let reason = if runs.pristine.built {
            GateRejection::GoldDoesNotBuild
        } else {
            GateRejection::BaseDoesNotBuild
        };
        return GateVerdict::Rejected { reason };
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

/// The line of a failed build's log that names what went wrong: the first `error…:` line of
/// the compiler or cargo, or cargo's closing `could not compile` line when it is the only one
/// (a compiler killed for memory prints nothing else). `None` when the log has no such line,
/// as when the run was stopped for time.
pub fn first_build_error(log: &str) -> Option<&str> {
    let mut errors = log.lines().map(str::trim).filter(|line| {
        line.strip_prefix("error")
            .is_some_and(|rest| rest.starts_with(':') || rest.starts_with('['))
    });
    let first = errors.next()?;
    if !first.starts_with(CARGO_GAVE_UP) {
        return Some(first);
    }
    Some(
        errors
            .find(|line| !line.starts_with(CARGO_GAVE_UP))
            .unwrap_or(first),
    )
}

/// How cargo's summary of a failed crate starts; the compiler's own errors say more.
const CARGO_GAVE_UP: &str = "error: could not compile";

/// [`first_build_error`], and, when that is only cargo's `could not compile` line, the signal
/// that killed the compiler if the log names one: `… (signal: 9, SIGKILL: kill)`. A build
/// killed for memory prints nothing else, and without the signal it reads like a compile error.
pub fn build_error_summary(log: &str) -> Option<String> {
    let line = first_build_error(log)?;
    match killing_signal(log).filter(|_| line.starts_with(CARGO_GAVE_UP)) {
        Some(signal) => Some(format!("{line} ({signal})")),
        None => Some(line.to_string()),
    }
}

/// The `signal: N, NAME: …` text of cargo's "process didn't exit successfully" line, when the
/// log has one.
fn killing_signal(log: &str) -> Option<&str> {
    log.lines().find_map(|line| {
        let text = line[line.find("(signal: ")? + 1..].trim_end();
        Some(text.strip_suffix(')').unwrap_or(text))
    })
}

/// Whether a failed build's log says cargo stopped before compiling anything because the
/// command's scope names something the checked-out code does not have: a `--test` target or a
/// `-p` package. A task's scope is read from the commit of the fix, so on the untouched old
/// code this means "the task's tests did not exist yet", not "the old code does not build".
///
/// A text check on cargo's wording (seen with cargo 1.100; older versions were not tried).
pub fn scope_absent_on_base(log: &str) -> bool {
    first_build_error(log).is_some_and(|line| {
        line.starts_with("error: no test target named")
            || (line.starts_with("error: package ID specification")
                && line.contains("did not match any packages"))
    })
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

    /// Nine of twenty hyperium/http tasks on the newest toolchain: the crate's
    /// `deny(warnings)` met warnings newer than the code, so nothing built, patched or not.
    #[test]
    fn when_nothing_builds_the_base_is_named_not_the_fix() {
        let runs = GateRuns {
            pristine: run(false, &[]),
            before: vec![run(false, &[]); 2],
            after: vec![run(false, &[]); 2],
        };
        assert_eq!(
            judge(&runs),
            GateVerdict::Rejected {
                reason: GateRejection::BaseDoesNotBuild
            }
        );
    }

    #[test]
    fn the_fix_is_named_only_when_the_old_code_builds_without_it() {
        let runs = GateRuns {
            pristine: run(true, &[("old", Passed)]),
            before: vec![run(false, &[]); 2],
            after: vec![run(false, &[]); 2],
        };
        assert_eq!(
            judge(&runs),
            GateVerdict::Rejected {
                reason: GateRejection::GoldDoesNotBuild
            }
        );
        // One failed build of the fix out of several is still the fix's.
        let once = GateRuns {
            after: vec![run(true, &[("new", Passed)]), run(false, &[])],
            ..runs
        };
        assert_eq!(
            judge(&once),
            GateVerdict::Rejected {
                reason: GateRejection::GoldDoesNotBuild
            }
        );
    }

    #[test]
    fn the_saved_names_of_the_reasons_do_not_change() {
        let saved = |reason| {
            serde_json::to_string(&GateVerdict::Rejected { reason }).expect("a verdict is JSON")
        };
        assert_eq!(
            saved(GateRejection::BaseDoesNotBuild),
            r#"{"verdict":"rejected","reason":"base_does_not_build"}"#
        );
        assert_eq!(
            saved(GateRejection::GoldDoesNotBuild),
            r#"{"verdict":"rejected","reason":"gold_does_not_build"}"#
        );
    }

    #[test]
    fn each_reason_says_what_to_try_and_blames_the_right_side() {
        let base = GateRejection::BaseDoesNotBuild.explain();
        assert!(base.contains("the fix was not judged"));
        assert!(base.contains("toolchain_image") && base.contains("cap_lints"));
        // The cap is silently off when the repository sets target rustflags: say where to look.
        assert!(base.contains(".cargo/config.toml") && base.contains("memory_mib"));
        let gold = GateRejection::GoldDoesNotBuild.explain();
        assert!(gold.contains("works on the old code") && gold.contains("memory_mib"));
    }

    #[test]
    fn the_first_compiler_error_is_picked_out_of_a_build_log() {
        let lint = "   Compiling http v0.2.0 (/testbed)\n\
                    warning: unused import\n\
                    error: use of deprecated constant `std::u16::MAX`: replaced by the `MAX` associated constant on this type\n\
                    \x20 --> src/header/map.rs:9:5\n\
                    error[E0308]: mismatched types\n\
                    error: could not compile `http` (lib) due to 2 previous errors\n";
        assert_eq!(
            first_build_error(lint),
            Some(
                "error: use of deprecated constant `std::u16::MAX`: replaced by the `MAX` associated constant on this type"
            )
        );
        let typed = "error[E0308]: mismatched types\n";
        assert_eq!(
            first_build_error(typed),
            Some("error[E0308]: mismatched types")
        );
    }

    #[test]
    fn cargo_giving_up_is_shown_only_when_nothing_says_more() {
        let killed = "error: could not compile `big` (lib)\n\nCaused by:\n  process didn't exit successfully (signal: 9, SIGKILL: kill)\n";
        assert_eq!(
            first_build_error(killed),
            Some("error: could not compile `big` (lib)")
        );
        // Parallel jobs: cargo's summary of one crate can come before another crate's error.
        let interleaved = "error: could not compile `a` (lib) due to 1 previous error\n\
                           error[E0425]: cannot find value `x` in this scope\n";
        assert_eq!(
            first_build_error(interleaved),
            Some("error[E0425]: cannot find value `x` in this scope")
        );
        assert_eq!(first_build_error("test old ... ok\nerrors: none\n"), None);
        assert_eq!(first_build_error(""), None);
    }

    #[test]
    fn a_killed_compiler_is_named_by_its_signal() {
        // The shape cargo 1.100 prints when rustc is killed with SIGKILL.
        let killed = "   Compiling c1 v0.1.0 (/testbed)\n\
                      error: could not compile `c1` (lib)\n\n\
                      Caused by:\n\
                      \x20 process didn't exit successfully: `rustc --crate-name c1` (signal: 9, SIGKILL: kill)\n";
        assert_eq!(
            build_error_summary(killed).as_deref(),
            Some("error: could not compile `c1` (lib) (signal: 9, SIGKILL: kill)")
        );
        // A compiler error says more than a signal elsewhere in the log would.
        let typed = format!("error[E0308]: mismatched types\n{killed}");
        assert_eq!(
            build_error_summary(&typed).as_deref(),
            Some("error[E0308]: mismatched types")
        );
        let plain = "error: could not compile `c1` (lib) due to 1 previous error";
        assert_eq!(build_error_summary(plain).as_deref(), Some(plain));
        assert_eq!(build_error_summary("test old ... ok\n"), None);
    }

    #[test]
    fn a_scope_the_old_code_lacks_is_told_from_code_that_does_not_compile() {
        assert!(scope_absent_on_base(
            "error: no test target named `header_map` in `http` package\n"
        ));
        assert!(scope_absent_on_base(
            "error: package ID specification `newcrate` did not match any packages\n"
        ));
        assert!(!scope_absent_on_base(
            "error: use of deprecated constant `std::u16::MAX`\n\
             error: could not compile `http` (lib) due to 1 previous error\n"
        ));
        // A compiler error first: the code was compiled, so the scope existed.
        assert!(!scope_absent_on_base(
            "error[E0425]: cannot find value `x`\nerror: no test target named `t`\n"
        ));
        assert!(!scope_absent_on_base(""));
    }

    /// The scope names a test target the test patch adds. The caller passes such a pristine
    /// run as built with no results; then a fix that does not build is the fix's failure, and
    /// a fix that builds makes every passing test one that did not pass before.
    #[test]
    fn a_pristine_run_without_the_new_test_target_does_not_hide_a_broken_fix() {
        let broken = GateRuns {
            pristine: run(true, &[]),
            before: vec![run(false, &[]); 2],
            after: vec![run(false, &[]); 2],
        };
        assert_eq!(
            judge(&broken),
            GateVerdict::Rejected {
                reason: GateRejection::GoldDoesNotBuild
            }
        );
        let sound = GateRuns {
            after: vec![run(true, &[("new", Passed)]); 2],
            ..broken
        };
        let GateVerdict::Reproducible(lists) = judge(&sound) else {
            panic!("reproducible")
        };
        assert_eq!(lists.fail_to_pass, vec!["new"]);
        assert!(lists.pass_to_pass.is_empty());
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
