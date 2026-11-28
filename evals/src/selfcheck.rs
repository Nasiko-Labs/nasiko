//! Did the agent check its own work: did it run the project's build or tests after its last
//! edit?
//!
//! Read from opencode's session export (`session-export.json`), which lists every tool call in
//! order: `{"type":"tool","tool":"bash","state":{"status":…,"input":{"command":…}}}` for a shell
//! command, and the tools in [`EDIT_TOOLS`] for file edits. This is the shape the CLI's opencode
//! session importer reads too (`cli/src/commands/integration/agents/opencode.rs`), including its
//! newer form with `content` for `parts` and `name` for `tool`. Limits that follow from that
//! source:
//!
//! * A file changed by a shell command (`sed -i`, a redirect, a heredoc) is not seen as an
//!   edit. An attempt with no edit-tool call that still handed in a change is counted apart
//!   ([`SelfCheckSummary::edited_outside_edit_tools`]): the order of its edits and checks cannot
//!   be told.
//! * A command is recognised by how it is written, not by what it did or whether it passed. It
//!   must be the command itself (`cargo test`, also after `cd x &&`, behind `timeout 60` or
//!   inside `bash -c "…"`); a command line that only mentions one (`echo "run cargo test"`, a
//!   commit message, `grep`) does not count. A script or a `make` target that runs the tests
//!   counts only when the repository's spec names it as its build or test command.
//! * The tool names and fields are opencode's as this reader knows them. A session whose shell
//!   calls carry no command is not read at all rather than read as "never checked".

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::report::{Rate, rate_of};
use crate::spec::EnvSpec;

/// opencode tools that change a file.
pub const EDIT_TOOLS: [&str; 5] = ["edit", "write", "multiedit", "patch", "apply_patch"];

/// opencode's shell tool, by the names its session formats use.
pub const SHELL_TOOLS: [&str; 2] = ["bash", "shell"];

/// `state.status` values of a tool call that did not go through.
const FAILED_STATUSES: [&str; 2] = ["error", "failed"];

/// Cargo subcommands that build or test a Rust project, whatever the repository's spec says
/// (`t`, `b` and `c` are cargo's own short names).
pub const CARGO_CHECKS: [&str; 8] = ["test", "t", "nextest", "build", "b", "check", "c", "clippy"];

/// Cargo flags that stand before the subcommand and take the next word as their value.
const CARGO_FLAGS_WITH_VALUE: [&str; 4] = ["--color", "--config", "-Z", "-C"];

/// Most leading words of a spec's command that name it (`make test` of `make test {scope}`).
const COMMAND_WORDS: usize = 2;

/// Words that may stand before a command without being it: wrappers and shell keywords.
const COMMAND_PREFIXES: [&str; 17] = [
    "timeout", "time", "nice", "nohup", "exec", "command", "env", "sudo", "if", "then", "else",
    "elif", "do", "while", "until", "!", "{",
];

/// Shells that run their `-c` argument as a command line.
const SHELLS: [&str; 3] = ["bash", "sh", "zsh"];

/// Commands that say nothing alone about building or testing: a spec command cut down to one
/// of these is not used.
const NOT_A_CHECK_ALONE: [&str; 9] = [
    "cd", "python", "python3", "node", "ruby", "perl", "bash", "sh", "zsh",
];

/// The repository's own build and test commands that are not cargo checks already, each cut
/// down to its leading words: after the last `&&`, without `VAR=value` words, up to the first
/// flag or placeholder (`make test` of `cd oss && make test {scope}`).
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
        if !says_nothing && !is_cargo_check(&words) && !commands.contains(&command) {
            commands.push(command);
        }
    }
    commands
}

fn leading_words(command: &str) -> Vec<String> {
    let last = command.rsplit("&&").next().unwrap_or_default();
    let words: Vec<String> = last.split_whitespace().map(str::to_string).collect();
    without_prefixes(&words)
        .iter()
        .take_while(|w| !w.starts_with('-') && !w.starts_with('{'))
        .take(COMMAND_WORDS)
        .cloned()
        .collect()
}

/// What one attempt's session shows about checking its work.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SelfCheck {
    /// File edits made through the agent's edit tools (failed ones not counted).
    pub edits: u32,
    pub shell_commands: u32,
    /// Shell commands that ran a build or test command.
    pub checks: u32,
    /// Whether a build or test command ran after the last edit. `false` without edits.
    pub checked_after_last_edit: bool,
    /// The other tools the session called, by name. When an attempt changed files and shows no
    /// edit, the tool that made the change may be listed here under a name this reader does
    /// not know.
    pub other_tools: Vec<String>,
}

/// Reads opencode's session export. `own_commands` are the repository's own build and test
/// commands ([`spec_commands`]). `Ok(None)` when the export has shell calls but none carries its
/// command: the format is not the one this reader knows, so nothing is claimed.
pub fn inspect(
    export: &str,
    own_commands: &[String],
) -> Result<Option<SelfCheck>, serde_json::Error> {
    let export: Value = serde_json::from_str(export)?;
    let mut check = SelfCheck::default();
    let mut other_tools = BTreeSet::new();
    let mut shell_calls_without_command = 0u32;
    for part in tool_parts(&export) {
        let tool = tool_name(part);
        let state = &part["state"];
        if EDIT_TOOLS.contains(&tool) {
            let failed = state["status"]
                .as_str()
                .is_some_and(|status| FAILED_STATUSES.contains(&status));
            if !failed {
                check.edits += 1;
                check.checked_after_last_edit = false;
            }
        } else if SHELL_TOOLS.contains(&tool) {
            let Some(command) = state["input"]["command"].as_str() else {
                shell_calls_without_command += 1;
                continue;
            };
            check.shell_commands += 1;
            if runs_check(command, own_commands) {
                check.checks += 1;
                check.checked_after_last_edit = check.edits > 0;
            }
        } else {
            other_tools.insert(tool.to_string());
        }
    }
    check.other_tools = other_tools.into_iter().collect();
    let unreadable = check.shell_commands == 0 && shell_calls_without_command > 0;
    Ok((!unreadable).then_some(check))
}

/// Every tool call of the session, in the order it happened. A message keeps its parts under
/// `parts`, or under `content` in opencode's newer session format.
fn tool_parts(export: &Value) -> impl Iterator<Item = &Value> {
    export["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(parts_of)
        .filter(|part| part["type"].as_str() == Some("tool"))
}

fn parts_of(message: &Value) -> impl Iterator<Item = &Value> {
    ["parts", "content"]
        .into_iter()
        .filter_map(|key| message[key].as_array())
        .flatten()
}

/// The tool a call names: `tool`, or `name` in opencode's newer session format.
fn tool_name(part: &Value) -> &str {
    part["tool"]
        .as_str()
        .or_else(|| part["name"].as_str())
        .unwrap_or_default()
}

/// Whether a shell command line runs a build or test command: one of its commands is a cargo
/// check ([`CARGO_CHECKS`]) or starts with one of `own_commands`.
pub fn runs_check(command_line: &str, own_commands: &[String]) -> bool {
    shell_commands(command_line)
        .iter()
        .any(|words| is_check(words, own_commands))
}

fn is_check(words: &[String], own_commands: &[String]) -> bool {
    let words = without_prefixes(words);
    if let Some(script) = shell_script_argument(words) {
        return runs_check(script, own_commands);
    }
    let asks_for_help = words
        .iter()
        .any(|w| ["--help", "-h", "--version", "-V"].contains(&w.as_str()));
    let is_own = |own: &String| {
        let wanted: Vec<&str> = own.split_whitespace().collect();
        !wanted.is_empty()
            && words.len() >= wanted.len()
            && words.iter().zip(&wanted).all(|(a, b)| a == b)
    };
    !asks_for_help && (is_cargo_check(words) || own_commands.iter().any(is_own))
}

/// The words of a command after what may stand before it: `VAR=value` settings, wrappers such
/// as `timeout 60` with their flags and numbers, and shell keywords.
fn without_prefixes(words: &[String]) -> &[String] {
    let mut rest = words;
    loop {
        let Some(first) = rest.first() else {
            return rest;
        };
        if is_assignment(first) {
            rest = &rest[1..];
        } else if COMMAND_PREFIXES.contains(&first.as_str()) {
            let own_arguments = rest[1..]
                .iter()
                .take_while(|w| w.starts_with('-') || w.starts_with(|c: char| c.is_ascii_digit()))
                .count();
            rest = &rest[1 + own_arguments..];
        } else {
            return rest;
        }
    }
}

/// `NAME=value`, as written before a command to set its environment.
fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let starts_like_a_name = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
    starts_like_a_name && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
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
    let Some((program, arguments)) = words.split_first() else {
        return false;
    };
    if program.rsplit('/').next() != Some("cargo") {
        return false;
    }
    let mut arguments = arguments.iter();
    while let Some(word) = arguments.next() {
        if CARGO_FLAGS_WITH_VALUE.contains(&word.as_str()) {
            arguments.next();
        } else if !word.starts_with('-') && !word.starts_with('+') {
            return CARGO_CHECKS.contains(&word.as_str());
        }
    }
    false
}

/// The commands of a shell command line, each as its words. Commands end at `;`, `&&`, `||`,
/// `|`, `&`, a new line, parentheses and backticks; quoted text stays one word, so a command
/// named inside a message or a search pattern is not a command. Heredoc bodies and comments
/// are dropped.
fn shell_commands(command_line: &str) -> Vec<Vec<String>> {
    let script = without_heredoc_bodies(command_line);
    let mut commands: Vec<Vec<String>> = vec![Vec::new()];
    let mut word: Option<String> = None;
    let mut quote: Option<char> = None;
    let mut chars = script.chars();
    while let Some(c) = chars.next() {
        let (ends_word, ends_command) = match (quote, c) {
            (Some(open), _) if c == open => {
                quote = None;
                (false, false)
            }
            (Some('"'), '\\') | (None, '\\') => {
                // An escaped character is itself; an escaped line break joins two lines.
                if let Some(escaped) = chars.next().filter(|next| *next != '\n') {
                    word.get_or_insert_default().push(escaped);
                }
                (false, false)
            }
            (Some(_), _) => {
                word.get_or_insert_default().push(c);
                (false, false)
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                word.get_or_insert_default();
                (false, false)
            }
            (None, '#') if word.is_none() => {
                // A comment runs to the end of its line.
                chars
                    .by_ref()
                    .take_while(|next| *next != '\n')
                    .for_each(drop);
                (true, true)
            }
            (None, ';' | '\n' | '|' | '&' | '(' | ')' | '`') => (true, true),
            (None, _) if c.is_whitespace() => (true, false),
            (None, _) => {
                word.get_or_insert_default().push(c);
                (false, false)
            }
        };
        if ends_word
            && let Some(done) = word.take()
            && let Some(command) = commands.last_mut()
        {
            command.push(done);
        }
        if ends_command {
            commands.push(Vec::new());
        }
    }
    if let Some(done) = word.take()
        && let Some(command) = commands.last_mut()
    {
        command.push(done);
    }
    commands.retain(|command| !command.is_empty());
    commands
}

/// The script without the text it feeds to commands through heredocs (`cat > f <<EOF … EOF`):
/// that text is data, even when it reads like a command.
fn without_heredoc_bodies(script: &str) -> String {
    let mut kept = Vec::new();
    let mut body_ends_at: Option<String> = None;
    for line in script.lines() {
        match &body_ends_at {
            Some(end) if line.trim() == end => body_ends_at = None,
            Some(_) => {}
            None => {
                kept.push(line);
                body_ends_at = heredoc_end(line);
            }
        }
    }
    kept.join("\n")
}

/// The word that ends the heredoc a line opens (`EOF` of `<<EOF`, `<<-'EOF'`), if it opens one.
fn heredoc_end(line: &str) -> Option<String> {
    let (_, after) = line.split_once("<<")?;
    if after.starts_with('<') {
        // `<<<` feeds one word, not a body.
        return None;
    }
    let word: String = after
        .trim_start_matches('-')
        .trim_start()
        .chars()
        .take_while(|c| !c.is_whitespace() && !";|&)".contains(*c))
        .filter(|c| !"'\"\\".contains(*c))
        .collect();
    (!word.is_empty()).then_some(word)
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
    use serde_json::json;

    use super::*;

    fn shell(command: &str) -> Value {
        json!({"type": "tool", "tool": "bash",
               "state": {"status": "completed", "input": {"command": command}}})
    }

    fn edit(status: &str) -> Value {
        json!({"type": "tool", "tool": "edit",
               "state": {"status": status, "input": {"filePath": "/testbed/src/lib.rs"}}})
    }

    /// A session export with these tool calls, split over two assistant messages.
    fn export(parts: Vec<Value>) -> String {
        let (first, second) = parts.split_at(parts.len() / 2);
        json!({"info": {}, "messages": [
            {"info": {"role": "user"}, "parts": [{"type": "text", "text": "fix it"}]},
            {"info": {"role": "assistant"}, "parts": first},
            {"info": {"role": "assistant"}, "parts": second},
        ]})
        .to_string()
    }

    fn inspected(parts: Vec<Value>) -> SelfCheck {
        inspect(&export(parts), &[])
            .expect("valid JSON")
            .expect("readable export")
    }

    fn is_a_check(command: &str) -> bool {
        runs_check(command, &[])
    }

    #[test]
    fn a_test_run_after_the_last_edit_counts() {
        let check = inspected(vec![
            shell("ls src"),
            json!({"type": "tool", "tool": "read"}),
            edit("completed"),
            shell("cd /testbed && cargo test -p app 2>&1 | tail -20"),
        ]);
        let expected = SelfCheck {
            edits: 1,
            shell_commands: 2,
            checks: 1,
            checked_after_last_edit: true,
            other_tools: vec!["read".to_string()],
        };
        assert_eq!(check, expected);
    }

    #[test]
    fn an_edit_after_the_last_test_run_does_not() {
        let check = inspected(vec![
            edit("completed"),
            shell("cargo +nightly build"),
            edit("completed"),
            shell("git diff"),
        ]);
        assert_eq!((check.edits, check.checks), (2, 1));
        assert!(!check.checked_after_last_edit);
    }

    #[test]
    fn a_failed_edit_changes_nothing_so_the_earlier_check_still_stands() {
        let check = inspected(vec![edit("completed"), shell("cargo check"), edit("error")]);
        assert_eq!(check.edits, 1);
        assert!(check.checked_after_last_edit);
        assert!(check.other_tools.is_empty());
    }

    #[test]
    fn the_newer_session_format_is_read_the_same_way() {
        let session = json!({"session_id": "s", "messages": [
            {"id": "m", "type": "assistant", "content": [
                {"type": "tool", "name": "write", "state": {"status": "failed"}},
                {"type": "tool", "name": "edit", "state": {"status": "completed"}},
                {"type": "tool", "name": "shell",
                 "state": {"status": "completed", "input": {"command": "cargo test"}}},
                {"type": "text", "text": "Done"},
            ]},
        ]});
        let check = inspect(&session.to_string(), &[])
            .expect("valid JSON")
            .expect("readable export");
        assert_eq!((check.edits, check.shell_commands, check.checks), (1, 1, 1));
        assert!(check.checked_after_last_edit);
    }

    #[test]
    fn a_check_without_an_edit_tool_call_is_not_a_check_after_an_edit() {
        let without_edits = inspected(vec![shell("sed -i s/1/2/ src/lib.rs"), shell("cargo test")]);
        assert_eq!((without_edits.edits, without_edits.checks), (0, 1));
        assert!(!without_edits.checked_after_last_edit);
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

    #[test]
    fn an_export_without_commands_claims_nothing() {
        let bare = json!({"type": "tool", "tool": "bash"});
        let read = inspect(&export(vec![bare.clone(), bare]), &[]);
        assert_eq!(read.expect("valid JSON"), None);
        assert!(inspect("not json", &[]).is_err());
        let empty = inspect("{}", &[]).expect("valid JSON");
        assert_eq!(empty, Some(SelfCheck::default()));
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
        let spec = spec_with("make test {scope}", "cargo test {scope} --no-run");
        let own = spec_commands(Some(&spec));
        assert_eq!(own, vec!["make test"], "cargo checks are known already");
        assert!(runs_check("cd oss && make test", &own));
        assert!(!runs_check("make", &own) && !runs_check("make lint", &own));
        assert!(!runs_check("make test", &[]));
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
        let check = inspect(
            &export(vec![
                edit("completed"),
                shell("cd oss && ls"),
                shell("python fix.py"),
            ]),
            &own,
        );
        assert_eq!(check.expect("valid JSON").expect("readable").checks, 0);
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
