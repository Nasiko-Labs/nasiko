//! Did the agent check its own work: did it run the project's build or tests after its last
//! edit? This module also holds how a build or test command is recognised on a shell command
//! line and how such a run ended ([`check_run`]), which [`crate::method`] shares.
//!
//! Read from the agent's steps ([`crate::trace`]): its file edits and its shell commands, in
//! order. Limits that follow from that source:
//!
//! * A file changed by a shell command (`sed -i`, a redirect, a heredoc) is not seen as an
//!   edit. An attempt with no edit-tool call that still handed in a change is counted apart
//!   ([`SelfCheckSummary::edited_outside_edit_tools`]): the order of its edits and checks cannot
//!   be told.
//! * A command is recognised by how it is written. It must be the command itself (`cargo test`,
//!   also after `cd x &&`, behind `timeout 60` or `uv run`, or inside `bash -c "…"`); a command
//!   line that only mentions one (`echo "run cargo test"`, a commit message, `grep`) does not
//!   count, nor does one that only lists tests (`pytest --collect-only`, `tox -l`). Cargo's
//!   build and test subcommands and the usual build and test runners of other ecosystems
//!   ([`OTHER_CHECKS`]) are known; a script or a `make` target of another name counts only when
//!   the repository's spec names it as its build or test command.
//! * A run counts whether or not it passed, but not when its command did not start
//!   ([`RunEnd::DidNotStart`]).
//! * Every edit to a file inside the repository counts as an edit, also a note the agent wrote
//!   there after its last test run. An edit outside the repository does not.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::report::{Rate, rate_of};
use crate::shell::{self, Link};
use crate::spec::EnvSpec;
use crate::trace::{self, Step, Trace, TraceError};

/// Cargo subcommands that build or test a Rust project, whatever the repository's spec says
/// (`t`, `b` and `c` are cargo's own short names). `cargo test --no-run` and
/// `cargo test -- --list` compile the code, so they count as a build.
pub const CARGO_CHECKS: [&str; 8] = ["test", "t", "nextest", "build", "b", "check", "c", "clippy"];

/// Cargo flags that stand before the subcommand and take the next word as their value.
const CARGO_FLAGS_WITH_VALUE: [&str; 4] = ["--color", "--config", "-Z", "-C"];

/// Build and test commands of other ecosystems, known whatever the spec says: the program and
/// the first arguments that make it a check (none: the program alone is one).
pub const OTHER_CHECKS: [(&str, &[&str]); 19] = [
    ("pytest", &[]),
    ("py.test", &[]),
    ("tox", &[]),
    ("nox", &[]),
    ("go", &["test", "build", "vet"]),
    ("npm", &["test", "t"]),
    ("yarn", &["test"]),
    ("pnpm", &["test"]),
    ("bun", &["test"]),
    ("jest", &[]),
    ("vitest", &[]),
    ("mocha", &[]),
    ("make", &["test", "tests", "check"]),
    ("mvn", &["test", "verify"]),
    ("gradle", &["test", "build", "check"]),
    ("gradlew", &["test", "build", "check"]),
    ("dotnet", &["test", "build"]),
    ("ctest", &[]),
    ("rspec", &[]),
];

/// Programs that run a package script by name: `npm run test` is `npm test`.
const SCRIPT_RUNNERS: [&str; 4] = ["npm", "yarn", "pnpm", "bun"];

/// Python modules that run tests when started with `python -m`.
const PYTHON_TEST_MODULES: [&str; 2] = ["pytest", "unittest"];

/// Flags that make a command print what it knows instead of building or testing.
const ONLY_ASKS: [&str; 11] = [
    "--help",
    "-h",
    "--version",
    "-V",
    "--collect-only",
    "--co",
    "--fixtures",
    "--markers",
    "--setup-plan",
    "--listenvs",
    "--listTests",
];

/// tox's short flags that list its environments.
const TOX_LISTS: [&str; 2] = ["-l", "-a"];

/// Most leading words of a spec's command that name it (`make test` of `make test {scope}`).
const COMMAND_WORDS: usize = 2;

/// Shells that run their `-c` argument as a command line.
const SHELLS: [&str; 3] = ["bash", "sh", "zsh"];

/// Commands that say nothing alone about building or testing: a spec command cut down to one
/// of these is not used.
const NOT_A_CHECK_ALONE: [&str; 9] = [
    "cd", "python", "python3", "node", "ruby", "perl", "bash", "sh", "zsh",
];

/// Exit statuses a shell gives when a command did not start: not allowed to run, not found.
const DID_NOT_START_EXITS: [i64; 2] = [126, 127];

/// Exit statuses of a command stopped from outside: `timeout`'s own, then killed (128 + 9)
/// and terminated (128 + 15).
const CUT_OFF_EXITS: [i64; 3] = [124, 137, 143];

/// What a shell prints when the command it was asked to run does not exist.
const NOT_FOUND_MARK: &str = "command not found";

/// What opencode's shell tool adds to the output of a command it stopped at its time limit.
const TIMED_OUT_MARK: &str = "terminated command after exceeding timeout";

/// Text in a build or test run's output that shows something failed: a test runner's failure
/// line, a compiler error, a Python traceback, or a count of failures that is not zero. A
/// panic message alone is not one: a passing test may expect its panic.
static FAILURE_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)FAILED|^FAIL\b|--- FAIL|^error(?:\[E\d+\])?: |Traceback \(most recent call last\)|\b[1-9]\d* (?:failed|failures?|errors?)\b",
    )
    .expect("a valid regex")
});

/// Text in a build or test run's output that shows it got through.
static PASSING_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)test result: ok|\b\d+ passed\b|^(?:ok|PASS)\b|^\s*Finished ")
        .expect("a valid regex")
});

/// The repository's own build and test commands that are not known checks already, each cut
/// down to its leading words: after the last `&&`, without `VAR=value` words, up to the first
/// flag or placeholder (`just test` of `cd oss && just test {scope}`).
pub fn spec_commands(spec: Option<&EnvSpec>) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    let own = spec
        .iter()
        .flat_map(|s| [s.test_cmd.as_str(), s.build_cmd.as_str()]);
    for words in own.map(leading_words) {
        let command = words.join(" ");
        let says_nothing = match words.as_slice() {
            [] => true,
            [only] => NOT_A_CHECK_ALONE.contains(&only.as_str()),
            _ => false,
        };
        if !says_nothing && !is_known_check(&words) && !commands.contains(&command) {
            commands.push(command);
        }
    }
    commands
}

fn leading_words(command: &str) -> Vec<String> {
    let last = command.rsplit("&&").next().unwrap_or_default();
    let words: Vec<String> = last.split_whitespace().map(str::to_string).collect();
    shell::without_prefixes(&words)
        .iter()
        .take_while(|w| !w.starts_with('-') && !w.starts_with('{'))
        .take(COMMAND_WORDS)
        .cloned()
        .collect()
}

/// What one attempt's session shows about checking its work.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SelfCheck {
    /// Edits to files inside the repository made through the agent's edit tools (failed ones
    /// not counted).
    pub edits: u32,
    pub shell_commands: u32,
    /// Shell commands that ran a build or test command (one that did not start not counted).
    pub checks: u32,
    /// Whether a build or test command ran after the last edit. `false` without edits.
    pub checked_after_last_edit: bool,
    /// The tools the session called that this report does not read (not a look, a search, a
    /// shell command or an edit), by name. When an attempt changed files and shows no edit,
    /// the tool that made the change may be listed here.
    pub other_tools: Vec<String>,
}

impl SelfCheck {
    /// What happened, in one sentence a non-engineer can read.
    pub fn sentence(&self) -> String {
        let times = |n: u32| if n == 1 { "once" } else { "more than once" };
        match (self.edits, self.checks, self.checked_after_last_edit) {
            (0, _, _) => "Made no edit with the agent's edit tools, so whether it tested its \
                          final change cannot be told."
                .into(),
            (_, _, true) => "Ran the build or tests after its last edit.".into(),
            (_, 0, false) => "Never ran the build or tests.".into(),
            (_, checks, false) => format!(
                "Ran the build or tests {} but not after its last edit.",
                times(checks)
            ),
        }
    }
}

/// What the agent's steps show about checking its work. `own_commands` are the repository's
/// own build and test commands ([`spec_commands`]).
pub fn of_trace(trace: &Trace, own_commands: &[String]) -> SelfCheck {
    let mut check = SelfCheck {
        other_tools: trace.other_tools.clone(),
        ..SelfCheck::default()
    };
    for step in &trace.steps {
        match step {
            Step::Edit { path, .. } if trace::is_under_root(path) => {
                check.edits += 1;
                check.checked_after_last_edit = false;
            }
            Step::Shell {
                command,
                exit,
                output,
                ..
            } => {
                check.shell_commands += 1;
                let end = check_run(command, *exit, output.as_deref(), own_commands);
                if end.is_some_and(|end| end != RunEnd::DidNotStart) {
                    check.checks += 1;
                    check.checked_after_last_edit = check.edits > 0;
                }
            }
            // A file written outside the repository is not part of what is handed in.
            Step::Edit { .. } | Step::Look { .. } | Step::List { .. } | Step::Search { .. } => {}
        }
    }
    check
}

/// Reads opencode's session export ([`trace::read_opencode_export`]) and checks it.
/// `Ok(None)` when the export is not laid out the way the reader knows: nothing is claimed.
pub fn inspect(
    export: &str,
    own_commands: &[String],
) -> Result<Option<SelfCheck>, serde_json::Error> {
    match trace::read_opencode_export(export) {
        Ok(trace) => Ok(Some(of_trace(&trace, own_commands))),
        Err(TraceError::UnknownShape(_)) => Ok(None),
        Err(TraceError::NotJson(e)) => Err(e),
    }
}

/// How a build or test run ended, as far as the session shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEnd {
    /// Its own exit status says it failed, or its output holds failure text.
    ShowedFailure,
    /// Its own exit status says it passed, or its output holds a passing result and no
    /// failure text.
    NoFailureSeen,
    /// The command did not start: not found, or not allowed to run.
    DidNotStart,
    /// It was stopped before it finished (a time limit, a kill).
    CutOff,
    /// The session kept nothing that says how it ended.
    NotShown,
}

/// Whether a shell command line runs a build or test command: one of its commands is a cargo
/// check ([`CARGO_CHECKS`]), another known one ([`OTHER_CHECKS`]) or starts with one of
/// `own_commands`.
pub fn runs_check(command_line: &str, own_commands: &[String]) -> bool {
    exit_is_the_checks_own(command_line, own_commands).is_some()
}

/// How the build or test command on a shell command line ended; `None` when the line runs
/// none. `exit` is the exit status of the whole line and `output` what it printed, where the
/// session kept them.
///
/// The exit status is used only when it is the check's own: the check is the last command of
/// the line. Behind a pipe (`cargo test | tail`), before `;` or `||`, in the background or
/// inside `$(…)`, the status is another command's, and only the output is read.
pub fn check_run(
    command_line: &str,
    exit: Option<i64>,
    output: Option<&str>,
    own_commands: &[String],
) -> Option<RunEnd> {
    let exit_is_its_own = exit_is_the_checks_own(command_line, own_commands)?;
    let exit = exit.filter(|_| exit_is_its_own);
    let text = output.unwrap_or_default();
    let did_not_start = exit.is_some_and(|code| DID_NOT_START_EXITS.contains(&code));
    let cut_off = exit.is_some_and(|code| CUT_OFF_EXITS.contains(&code));
    Some(if did_not_start || text.contains(NOT_FOUND_MARK) {
        RunEnd::DidNotStart
    } else if cut_off || text.contains(TIMED_OUT_MARK) {
        RunEnd::CutOff
    } else if let Some(code) = exit {
        if code == 0 {
            RunEnd::NoFailureSeen
        } else {
            RunEnd::ShowedFailure
        }
    } else if FAILURE_TEXT.is_match(text) {
        RunEnd::ShowedFailure
    } else if PASSING_TEXT.is_match(text) {
        RunEnd::NoFailureSeen
    } else {
        RunEnd::NotShown
    })
}

/// `None` when the line runs no build or test command; otherwise whether the line's exit
/// status is that command's own.
fn exit_is_the_checks_own(command_line: &str, own_commands: &[String]) -> Option<bool> {
    let substitutes = shell::has_substitution(command_line);
    let mut found: Option<bool> = None;
    for command in shell::commands(command_line) {
        let words = shell::without_prefixes(&command.words);
        let ends_the_line =
            command.then.is_none() && command.follows != Some(Link::Or) && !substitutes;
        let own_exit = match shell_script_argument(words) {
            Some(script) => {
                exit_is_the_checks_own(script, own_commands).map(|inside| inside && ends_the_line)
            }
            None => is_check(words, own_commands).then_some(ends_the_line),
        };
        found = match (found, own_exit) {
            (Some(earlier), Some(this)) => Some(earlier || this),
            (earlier, this) => earlier.or(this),
        };
    }
    found
}

/// Whether a command's words, wrappers already taken off, are a build or test command.
fn is_check(words: &[String], own_commands: &[String]) -> bool {
    let is_own = |own: &String| {
        let wanted: Vec<&str> = own.split_whitespace().collect();
        !wanted.is_empty()
            && words.len() >= wanted.len()
            && words.iter().zip(&wanted).all(|(a, b)| a == b)
    };
    !only_asks(words) && (is_known_check(words) || own_commands.iter().any(is_own))
}

/// Whether a command only prints what it knows (its help, its version, the tests it would
/// run) instead of building or testing.
fn only_asks(words: &[String]) -> bool {
    let lists_tox_environments =
        shell::program(words) == "tox" && words.iter().any(|w| TOX_LISTS.contains(&w.as_str()));
    lists_tox_environments || words.iter().any(|w| ONLY_ASKS.contains(&w.as_str()))
}

/// A build or test command known whatever the spec says.
fn is_known_check(words: &[String]) -> bool {
    is_cargo_check(words) || is_other_check(words)
}

/// `python`, `python3`, `python3.12`.
fn is_python(program: &str) -> bool {
    program
        .strip_prefix("python")
        .is_some_and(|version| version.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// One of [`OTHER_CHECKS`], or `python -m pytest` and the like.
fn is_other_check(words: &[String]) -> bool {
    let program = shell::program(words);
    let arguments = words.get(1..).unwrap_or_default();
    if is_python(program) {
        let module = arguments.get(1).map(String::as_str).unwrap_or_default();
        let starts_a_module = arguments.first().is_some_and(|flag| flag == "-m");
        return starts_a_module && PYTHON_TEST_MODULES.contains(&module);
    }
    let arguments = match arguments.split_first() {
        Some((run, script)) if run == "run" && SCRIPT_RUNNERS.contains(&program) => script,
        _ => arguments,
    };
    // `test:unit` is a test script as `test` is.
    let first = arguments
        .first()
        .and_then(|word| word.split(':').next())
        .unwrap_or_default();
    OTHER_CHECKS.iter().any(|(name, subcommands)| {
        *name == program && (subcommands.is_empty() || subcommands.contains(&first))
    })
}

/// The command line a shell is asked to run: the word after `-c` in `bash -c "…"` (or `-lc`).
fn shell_script_argument(words: &[String]) -> Option<&str> {
    let (shell, arguments) = words.split_first()?;
    if !SHELLS.contains(&shell.as_str()) {
        return None;
    }
    let flag = arguments
        .iter()
        .position(|w| w.starts_with('-') && !w.starts_with("--") && w.ends_with('c'))?;
    arguments.get(flag + 1).map(String::as_str)
}

/// `cargo [+toolchain] [flags] <subcommand>` with a subcommand that builds or tests.
fn is_cargo_check(words: &[String]) -> bool {
    if shell::program(words) != "cargo" {
        return false;
    }
    let mut arguments = words.iter().skip(1);
    while let Some(word) = arguments.next() {
        if CARGO_FLAGS_WITH_VALUE.contains(&word.as_str()) {
            arguments.next();
        } else if !word.starts_with('-') && !word.starts_with('+') {
            return CARGO_CHECKS.contains(&word.as_str());
        }
    }
    false
}

/// How often a group of attempts checked its work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelfCheckSummary {
    /// Of the attempts with an edit-tool call: the share that ran a build or test command
    /// after their last edit.
    pub checked_after_last_edit: Rate,
    /// Attempts with an edit-tool call that ran no build or test command at all.
    pub never_checked: u32,
    /// Attempts with no edit-tool call that still handed in a change: they changed files some
    /// other way (a shell command, a tool this reader does not know), so whether they checked
    /// afterwards cannot be told. Left out of the share.
    pub edited_outside_edit_tools: u32,
    /// Attempts with no edit-tool call and no change handed in. Left out of the share.
    pub without_edits: u32,
}

/// Summarises one model's attempts; each item is (what the session shows, whether the attempt
/// handed in a change).
pub fn summarise(attempts: &[(&SelfCheck, bool)]) -> SelfCheckSummary {
    let used_edit_tools: Vec<&SelfCheck> = attempts
        .iter()
        .filter(|(check, _)| check.edits > 0)
        .map(|(check, _)| *check)
        .collect();
    let checked = used_edit_tools
        .iter()
        .filter(|c| c.checked_after_last_edit)
        .count();
    let no_edit_tool = |handed_in: bool| {
        let matches =
            |(check, changed): &&(&SelfCheck, bool)| check.edits == 0 && *changed == handed_in;
        attempts.iter().filter(matches).count() as u32
    };
    SelfCheckSummary {
        checked_after_last_edit: rate_of(checked as u32, used_edit_tools.len() as u32),
        never_checked: used_edit_tools.iter().filter(|c| c.checks == 0).count() as u32,
        edited_outside_edit_tools: no_edit_tool(true),
        without_edits: no_edit_tool(false),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::trace::fixtures::{export, failed, read, shell, shell_run, trace, write};

    const SOURCE: &str = "/testbed/src/lib.rs";

    fn edit() -> Value {
        crate::trace::fixtures::edit(SOURCE, "fn a() {}", "fn a() { b() }")
    }

    fn checked(calls: Vec<Value>) -> SelfCheck {
        of_trace(&trace(calls), &[])
    }

    fn is_a_check(command: &str) -> bool {
        runs_check(command, &[])
    }

    #[test]
    fn a_test_run_after_the_last_edit_counts() {
        let check = checked(vec![
            shell("ls src"),
            read(SOURCE),
            json!({"type": "tool", "tool": "todowrite", "state": {"status": "completed"}}),
            edit(),
            shell("cd /testbed && cargo test -p app 2>&1 | tail -20"),
        ]);
        let expected = SelfCheck {
            edits: 1,
            shell_commands: 2,
            checks: 1,
            checked_after_last_edit: true,
            other_tools: vec!["todowrite".to_string()],
        };
        assert_eq!(check, expected);
        assert_eq!(
            check.sentence(),
            "Ran the build or tests after its last edit."
        );
    }

    #[test]
    fn an_edit_after_the_last_test_run_does_not() {
        let check = checked(vec![
            edit(),
            shell("cargo +nightly build"),
            edit(),
            shell("git diff"),
        ]);
        assert_eq!((check.edits, check.checks), (2, 1));
        assert!(!check.checked_after_last_edit);
        assert_eq!(
            check.sentence(),
            "Ran the build or tests once but not after its last edit."
        );
    }

    #[test]
    fn a_failed_edit_changes_nothing_so_the_earlier_check_still_stands() {
        let check = checked(vec![edit(), shell("cargo check"), failed(edit())]);
        assert_eq!(check.edits, 1);
        assert!(check.checked_after_last_edit);
        assert!(check.other_tools.is_empty());
    }

    #[test]
    fn a_check_without_an_edit_tool_call_is_not_a_check_after_an_edit() {
        let without_edits = checked(vec![shell("sed -i s/1/2/ src/lib.rs"), shell("cargo test")]);
        assert_eq!((without_edits.edits, without_edits.checks), (0, 1));
        assert!(!without_edits.checked_after_last_edit);
        assert!(without_edits.sentence().contains("cannot be told"));
        let never = checked(vec![edit(), shell("git diff")]);
        assert_eq!(never.sentence(), "Never ran the build or tests.");
    }

    #[test]
    fn an_export_is_checked_through_the_one_reader() {
        let read = inspect(&export(vec![edit(), shell("cargo test")]), &[]);
        let check = read.expect("valid JSON").expect("readable export");
        assert!(check.checked_after_last_edit);
        let bare = json!({"type": "tool", "tool": "bash"});
        let unknown = inspect(&export(vec![bare.clone(), bare]), &[]);
        assert_eq!(unknown.expect("valid JSON"), None);
        assert!(inspect("not json", &[]).is_err());
        assert_eq!(inspect("{}", &[]).expect("valid JSON"), None);
    }

    #[test]
    fn the_test_runners_of_other_ecosystems_are_known() {
        for command in [
            "pytest -x tests/test_api.py",
            "python -m pytest -q",
            "python3 -m unittest discover",
            ".venv/bin/pytest",
            "tox -e py312",
            "go test ./...",
            "go vet ./...",
            "npm test",
            "CI=1 yarn test --watch=false",
            "uv run pytest -q",
            "poetry run pytest tests/",
            "python3.11 -m pytest",
            "npm run test",
            "pnpm run test:unit",
            "npx jest src/",
            "bun test",
            "make test",
            "make check",
            "./gradlew test",
        ] {
            assert!(is_a_check(command), "{command:?} is a check");
        }
        for command in [
            "python fix.py",
            "python -m pip install pytest",
            "python -c 'import pytest'",
            "go run ./cmd/app",
            "go env",
            "npm install",
            "npm run lint",
            "pip install pytest",
            "pytest --version",
            "uv pip install pytest",
            "make",
            "make lint",
        ] {
            assert!(!is_a_check(command), "{command:?} is not a check");
        }
    }

    #[test]
    fn listing_tests_is_not_running_them_but_compiling_them_is_a_build() {
        for command in [
            "pytest --co -q",
            "python -m pytest --collect-only",
            "pytest --fixtures",
            "tox -l",
            "tox --listenvs",
            "npx jest --listTests",
        ] {
            assert!(!is_a_check(command), "{command:?} runs nothing");
        }
        // Both compile the crate and its tests: a build, and Nasiko's own default build command.
        for command in ["cargo test --no-run", "cargo test -- --list"] {
            assert!(is_a_check(command), "{command:?} builds");
        }
    }

    fn end_of(command: &str, exit: i64, output: &str) -> Option<RunEnd> {
        check_run(command, Some(exit), Some(output), &[])
    }

    #[test]
    fn the_exit_status_counts_only_when_it_is_the_checks_own() {
        for (command, exit, output, expected) in [
            ("cargo test", 101, "", RunEnd::ShowedFailure),
            ("cargo test", 0, "", RunEnd::NoFailureSeen),
            (
                "cd /testbed && cargo test 2>&1",
                101,
                "",
                RunEnd::ShowedFailure,
            ),
            ("(cargo build)", 101, "", RunEnd::ShowedFailure),
            (
                "bash -lc 'cd x && cargo test'",
                101,
                "",
                RunEnd::ShowedFailure,
            ),
            ("pytest -x", 1, "", RunEnd::ShowedFailure),
            // Another command's status: only the output is read.
            ("cargo test 2>&1 | grep FAILED", 1, "", RunEnd::NotShown),
            (
                "cargo test 2>&1 | tail -3",
                0,
                "test result: ok. 3 passed",
                RunEnd::NoFailureSeen,
            ),
            (
                "cargo test 2>&1 | tail -3",
                0,
                "test result: FAILED. 2 passed; 1 failed",
                RunEnd::ShowedFailure,
            ),
            (
                "pytest -q | tail -1",
                0,
                "1 failed, 2 passed in 0.1s",
                RunEnd::ShowedFailure,
            ),
            (
                "pytest -q | tail -1",
                0,
                "3 passed in 0.1s",
                RunEnd::NoFailureSeen,
            ),
            (
                "cargo build 2>&1 | head",
                0,
                "error[E0425]: cannot find value",
                RunEnd::ShowedFailure,
            ),
            ("cargo test; echo done", 0, "", RunEnd::NotShown),
            ("cargo test || true", 0, "", RunEnd::NotShown),
            ("cargo test &", 0, "", RunEnd::NotShown),
            ("echo $(cargo test)", 0, "", RunEnd::NotShown),
            ("bash -c 'cargo test | tail'", 0, "", RunEnd::NotShown),
            (
                "go test ./... | tee out.txt",
                0,
                "--- FAIL: TestA (0.00s)\nFAIL",
                RunEnd::ShowedFailure,
            ),
        ] {
            assert_eq!(end_of(command, exit, output), Some(expected), "{command:?}");
        }
        assert_eq!(end_of("ls", 1, "FAILED"), None, "not a build or test run");
        assert_eq!(
            check_run("cargo test", None, None, &[]),
            Some(RunEnd::NotShown)
        );
    }

    #[test]
    fn a_run_that_did_not_start_or_was_cut_off_is_neither_a_pass_nor_a_failure() {
        for (command, exit, output, expected) in [
            (
                "pytest -x",
                127,
                "bash: pytest: command not found",
                RunEnd::DidNotStart,
            ),
            ("./run.sh", 126, "", RunEnd::DidNotStart),
            (
                "pytest 2>&1 | tail -5",
                0,
                "bash: pytest: command not found",
                RunEnd::DidNotStart,
            ),
            ("timeout 5 cargo test", 124, "", RunEnd::CutOff),
            ("cargo test", 137, "", RunEnd::CutOff),
            (
                "cargo test | tail",
                0,
                "shell tool terminated command after exceeding timeout 120000 ms.",
                RunEnd::CutOff,
            ),
        ] {
            let own = ["./run.sh".to_string()];
            let end = check_run(command, Some(exit), Some(output), &own);
            assert_eq!(end, Some(expected), "{command:?}");
        }
        // A passing test that expects a panic prints one; that is not a failure.
        let expected_panic =
            "thread 't' panicked at src/lib.rs:1:1\ntest result: ok. 1 passed; 0 failed";
        assert_eq!(
            end_of("cargo test -- --nocapture", 0, expected_panic),
            Some(RunEnd::NoFailureSeen)
        );
        assert_eq!(
            end_of("cargo test -- --nocapture | tail", 0, expected_panic),
            Some(RunEnd::NoFailureSeen)
        );
    }

    #[test]
    fn a_note_outside_the_repository_and_a_run_that_did_not_start_change_nothing() {
        let note = write("/tmp/notes.md", "done\n", false);
        let check = checked(vec![edit(), shell("cargo test"), note]);
        assert_eq!((check.edits, check.checks), (1, 1));
        assert!(check.checked_after_last_edit);
        let not_found = shell_run("cargo test", 127, "bash: cargo: command not found");
        let check = checked(vec![edit(), not_found]);
        assert_eq!(check.checks, 0);
        assert_eq!(check.sentence(), "Never ran the build or tests.");
        // A note inside the repository is an edit like any other.
        let inside = write("/testbed/NOTES.md", "done\n", false);
        let check = checked(vec![edit(), shell("cargo test"), inside]);
        assert!(!check.checked_after_last_edit);
    }

    #[test]
    fn mentioning_a_command_is_not_running_it() {
        for command in [
            "grep -rn \"cargo test\" docs",
            "echo \"now run cargo test to verify\"",
            "git commit -am \"fix: make cargo test pass\"",
            "echo cargo test",
            "cargo test --help",
            "cargo help test",
            "cat > NOTES.md <<EOF\nRun cargo build before merging\ncargo test\nEOF",
            "cat <<'EOF' > run.sh\ncargo test\nEOF\nls",
            "ls # cargo test",
            "echo 'a; cargo test'",
            "cargo fmt --all",
            "cargo run --bin app",
            "cargo --version",
            "cargotest",
            "",
        ] {
            assert!(!is_a_check(command), "{command:?} is not a check");
        }
    }

    #[test]
    fn a_command_is_found_however_the_line_is_put_together() {
        for command in [
            "cargo test",
            "cd /testbed; cargo test; echo done",
            "cd /testbed && cargo test -p app 2>&1 | tail -20",
            "(cargo test)",
            "cargo test|tail -5",
            "cargo -q test",
            "cargo t",
            "cargo +nightly build",
            "cargo --color never --locked check --workspace",
            "cargo nextest run",
            "cargo clippy --all-targets -- -D warnings",
            "RUSTFLAGS=-Dwarnings CARGO_TERM_COLOR=never cargo b",
            "timeout 600 cargo test",
            "timeout --signal=TERM 600 env RUST_BACKTRACE=1 cargo test",
            "time cargo c",
            "bash -lc \"cd /testbed && cargo test\"",
            "sh -c 'cargo build'",
            "/usr/local/cargo/bin/cargo test",
            "echo start\ncargo test \\\n  -p app",
            "if cargo build; then echo ok; fi",
            "ls || cargo test",
            "echo `cargo test`",
            "cat <<EOF > notes\nhello\nEOF\ncargo test",
        ] {
            assert!(is_a_check(command), "{command:?} is a check");
        }
    }

    fn spec_with(test_cmd: &str, build_cmd: &str) -> EnvSpec {
        let mut spec = crate::spec::propose("o/r", &["Cargo.toml".to_string()], None)
            .expect("a Cargo repository");
        spec.test_cmd = test_cmd.into();
        spec.build_cmd = build_cmd.into();
        spec
    }

    #[test]
    fn the_repository_commands_join_the_cargo_ones() {
        let spec = spec_with("just test {scope}", "cargo test {scope} --no-run");
        let own = spec_commands(Some(&spec));
        assert_eq!(own, vec!["just test"], "cargo checks are known already");
        assert!(runs_check("cd oss && just test", &own));
        assert!(!runs_check("just", &own) && !runs_check("just lint", &own));
        assert!(!runs_check("just test", &[]));
        assert!(spec_commands(None).is_empty());
    }

    #[test]
    fn a_spec_command_is_cut_to_the_words_that_name_it() {
        let spec = spec_with(
            "cd oss && RUSTFLAGS=-Dwarnings cargo test {scope}",
            "python -m pytest {scope}",
        );
        let own = spec_commands(Some(&spec));
        assert!(own.is_empty(), "{own:?}");
        // Neither `cd oss` nor a bare interpreter became a check.
        let calls = vec![edit(), shell("cd oss && ls"), shell("python fix.py")];
        assert_eq!(of_trace(&trace(calls), &own).checks, 0);
        let spec = spec_with("cd oss && just test {scope}", "./scripts/build.sh --all");
        assert_eq!(
            spec_commands(Some(&spec)),
            vec!["just test", "./scripts/build.sh"]
        );
    }

    #[test]
    fn the_summary_keeps_shell_only_edits_apart() {
        let checked = SelfCheck {
            edits: 2,
            shell_commands: 3,
            checks: 1,
            checked_after_last_edit: true,
            other_tools: Vec::new(),
        };
        let stale = SelfCheck {
            checked_after_last_edit: false,
            ..checked.clone()
        };
        let never = SelfCheck {
            edits: 1,
            ..SelfCheck::default()
        };
        let through_the_shell = SelfCheck {
            shell_commands: 2,
            checks: 1,
            ..SelfCheck::default()
        };
        let idle = SelfCheck::default();
        let summary = summarise(&[
            (&checked, true),
            (&stale, true),
            (&never, false),
            (&through_the_shell, true),
            (&idle, false),
        ]);
        assert_eq!(summary.checked_after_last_edit, rate_of(1, 3));
        assert_eq!(summary.never_checked, 1);
        assert_eq!(summary.edited_outside_edit_tools, 1);
        assert_eq!(summary.without_edits, 1);
    }
}
