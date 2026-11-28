//! What the agent's steps ([`crate::trace`]) show of a file: did it open it, did a search list
//! it, did a command only name it, or can that not be told ([`Sight`]). Also which contents
//! searches it ran ([`Seen::searched_for`]).
//!
//! Only what the session records is seen, and a shell command is read by its words, not run:
//!
//! * A file is opened when the read tool, a shell viewer ([`VIEWERS`]) or a search is pointed
//!   at it by name, or the agent edited it.
//! * A viewer or search pointed at a glob (`cat src/*.rs`), fed by `$(…)` or `xargs`, or a
//!   script handed to an interpreter through a heredoc, may have shown a file without naming
//!   it: that is [`Sight::MayHave`], never "did not look".
//! * A command that changes, moves or removes a file ([`NOT_LOOKING`], `sed -i`,
//!   `git checkout`) or writes into it (`> file`) names it without showing it.
//! * The output of a search or of `git diff`, `show`, `log` counts for the files it lists. The
//!   output of anything else (a compiler error naming a file) does not, nor does a `grep` that
//!   only filters another command's output.

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;

use crate::shell;
use crate::trace::{SearchKind, Step, Trace};

/// Shell programs that print what is in a file.
const VIEWERS: [&str; 14] = [
    "cat", "head", "tail", "less", "more", "nl", "bat", "sed", "awk", "wc", "cut", "sort", "diff",
    "xxd",
];

/// Shell programs that search file contents.
const SEARCHERS: [&str; 6] = ["grep", "egrep", "fgrep", "rg", "ag", "ack"];

/// Shell programs that list files.
const LISTERS: [&str; 4] = ["ls", "find", "tree", "fd"];

/// git subcommands that print what is in files or what changed in them.
const GIT_VIEWERS: [&str; 4] = ["diff", "show", "blame", "log"];

/// Programs that name a file without showing it: they change, move or remove it, open an
/// editor nobody reads, or only print their own arguments.
const NOT_LOOKING: [&str; 18] = [
    "rm", "mv", "cp", "touch", "chmod", "chown", "mkdir", "ln", "tee", "truncate", "vim", "vi",
    "nano", "emacs", "rustfmt", "patch", "echo", "printf",
];

/// git subcommands that change files or the index without showing them.
const GIT_NOT_LOOKING: [&str; 10] = [
    "checkout", "restore", "rm", "mv", "add", "stash", "reset", "clean", "apply", "commit",
];

/// Programs that run a script given on their input, so a heredoc body is their program.
const INTERPRETERS: [&str; 8] = [
    "python", "python3", "node", "ruby", "perl", "bash", "sh", "zsh",
];

/// Flags of a searcher that take the next word as their value.
const SEARCH_FLAGS_WITH_VALUE: [&str; 20] = [
    "-A",
    "-B",
    "-C",
    "-m",
    "-g",
    "-t",
    "-T",
    "-d",
    "-j",
    "--include",
    "--exclude",
    "--exclude-dir",
    "--glob",
    "--type",
    "--max-count",
    "--context",
    "--after-context",
    "--before-context",
    "--max-depth",
    "--threads",
];

/// Flags of a searcher that give its pattern, or the file its patterns are in.
const SEARCH_PATTERN_FLAGS: [&str; 4] = ["-e", "--regexp", "-f", "--file"];

/// Characters that make a word a glob.
const GLOB_MARKS: [char; 3] = ['*', '?', '['];

/// Regex escapes that end a word (`\b`, `\s`, `\<`): not part of the name next to them.
static REGEX_ESCAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\[bBsSwWdD<>]").expect("a valid regex"));

/// How much of a file the agent got to see, least first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Sight {
    /// Nothing in the session shows the file.
    None,
    /// A command may have shown it without naming it: it cannot be told.
    MayHave,
    /// A command named it, without a program known to show files.
    Named,
    /// A contents search, or `git diff` and the like, listed it.
    Listed,
    /// It opened the file, or edited it.
    Opened,
}

/// The steps of a session, each shell command already read for what it shows.
pub(crate) struct Seen<'a> {
    trace: &'a Trace,
    /// One entry per step; `Some` for a shell command.
    views: Vec<Option<ShellView>>,
}

impl<'a> Seen<'a> {
    pub(crate) fn of(trace: &'a Trace) -> Self {
        let views = trace
            .steps
            .iter()
            .map(|step| match step {
                Step::Shell {
                    command, workdir, ..
                } => Some(ShellView::of(&trace.root, command, workdir.as_deref())),
                _ => None,
            })
            .collect();
        Self { trace, views }
    }

    /// The most the steps in `steps` show of `file` (a path relative to the session's folder).
    pub(crate) fn sight(&self, steps: Range<usize>, file: &str) -> Sight {
        steps
            .map(|at| self.sight_of_step(at, file))
            .max()
            .unwrap_or(Sight::None)
    }

    fn sight_of_step(&self, at: usize, file: &str) -> Sight {
        let root = self.trace.root.as_str();
        match (&self.trace.steps[at], &self.views[at]) {
            (Step::Look { path } | Step::Edit { path, .. }, _) if path == file => Sight::Opened,
            (
                Step::Search {
                    kind,
                    pattern,
                    path,
                    include,
                    output,
                },
                _,
            ) => {
                let names = |text: &Option<String>| {
                    text.as_deref()
                        .is_some_and(|text| mentions(root, text, file))
                };
                if path.as_deref() == Some(file) {
                    Sight::Opened
                } else if *kind == SearchKind::Contents && names(output) {
                    Sight::Listed
                } else if names(include)
                    || (*kind == SearchKind::FileNames && mentions(root, pattern, file))
                {
                    Sight::Named
                } else {
                    Sight::None
                }
            }
            (Step::Shell { output, .. }, Some(view)) => view.sight(root, output.as_deref(), file),
            _ => Sight::None,
        }
    }

    /// Every file a step opened by name: the read tool, or a shell viewer or search pointed
    /// at a path that reads as a file.
    pub(crate) fn files_opened(&self) -> BTreeSet<&str> {
        let looked = self.trace.steps.iter().filter_map(|step| match step {
            Step::Look { path } => Some(path.as_str()),
            _ => None,
        });
        let viewed = self
            .views
            .iter()
            .flatten()
            .flat_map(|view| &view.pointed_at)
            .map(String::as_str)
            .filter(|path| looks_like_a_file(path));
        looked.chain(viewed).collect()
    }

    /// Whether the step at `at` is spent looking: opening, listing or searching.
    pub(crate) fn is_looking(&self, at: usize) -> bool {
        match (&self.trace.steps[at], &self.views[at]) {
            (Step::Look { .. } | Step::List { .. } | Step::Search { .. }, _) => true,
            (_, Some(view)) => view.looks,
            _ => false,
        }
    }

    /// Whether the agent ran a contents search for `name` as a whole word that reached
    /// beyond `declared_in`, the file that declares it: its search tool, or a shell searcher
    /// ([`SEARCHERS`]) run over files. A name that only stands in a path, a search of the
    /// declaring file alone and a `grep` filtering another command's output do not count.
    pub(crate) fn searched_for(&self, name: &str, declared_in: &str) -> bool {
        let reaches_beyond =
            |paths: &[String]| paths.is_empty() || paths.iter().any(|path| path != declared_in);
        let by_tool = self.trace.steps.iter().any(|step| match step {
            Step::Search {
                kind: SearchKind::Contents,
                pattern,
                path,
                ..
            } => holds_word(pattern, name) && path.as_deref() != Some(declared_in),
            _ => false,
        });
        let by_shell = self
            .views
            .iter()
            .flatten()
            .flat_map(|view| &view.searches)
            .any(|search| {
                reaches_beyond(&search.paths)
                    && search
                        .patterns
                        .iter()
                        .any(|pattern| holds_word(pattern, name))
            });
        by_tool || by_shell
    }
}

/// One contents search run from the shell: what it looked for and where.
#[derive(Debug, Default, PartialEq, Eq)]
struct CodeSearch {
    patterns: Vec<String>,
    /// The files and folders it was pointed at; empty for the folder it ran in.
    paths: Vec<String>,
}

/// What a shell command line shows of files, as far as its words tell. Paths are relative to
/// the session's folder.
#[derive(Debug, Default)]
struct ShellView {
    /// Paths a viewer or a search was pointed at by name.
    pointed_at: Vec<String>,
    /// Globs a viewer or a search was pointed at.
    globs: Vec<String>,
    /// Whether a viewer got its file names from somewhere the line does not spell out.
    reads_unnamed_files: bool,
    /// Whether its output lists the files it shows (a search over files, `git diff`).
    lists_files: bool,
    /// The arguments of commands that neither show files by name nor only change them.
    named: Vec<String>,
    /// Heredoc text handed to an interpreter: a script that may open files.
    script: String,
    searches: Vec<CodeSearch>,
    /// Whether it views, searches or lists anything.
    looks: bool,
}

/// What a command does with the files it names.
enum Role {
    Viewer,
    Searcher,
    GitViewer,
    Lister,
    NotLooking,
    Other,
}

impl Role {
    fn of(program: &str, arguments: &[String]) -> Self {
        let first = arguments.first().map(String::as_str).unwrap_or_default();
        let edits_in_place = arguments.iter().any(|a| a.starts_with("-i"));
        match program {
            "git" if GIT_VIEWERS.contains(&first) => Role::GitViewer,
            "git" if GIT_NOT_LOOKING.contains(&first) => Role::NotLooking,
            "sed" | "perl" if edits_in_place => Role::NotLooking,
            _ if SEARCHERS.contains(&program) => Role::Searcher,
            _ if VIEWERS.contains(&program) => Role::Viewer,
            _ if LISTERS.contains(&program) => Role::Lister,
            _ if NOT_LOOKING.contains(&program) => Role::NotLooking,
            _ => Role::Other,
        }
    }
}

impl ShellView {
    fn of(root: &str, command_line: &str, workdir: Option<&str>) -> Self {
        let mut view = ShellView::default();
        let mut dir = workdir.unwrap_or(".").to_string();
        let substitutes = shell::has_substitution(command_line);
        let mut feeds_an_interpreter = false;
        for command in shell::commands(command_line) {
            let words = shell::without_prefixes(&command.words);
            let (words, through_xargs) = without_xargs(words);
            let (program, arguments) = program_and_arguments(words);
            if program == "cd" {
                if let Some(target) = arguments.first().filter(|target| !target.starts_with('-')) {
                    dir = resolve(root, &dir, target);
                }
                continue;
            }
            feeds_an_interpreter |= INTERPRETERS.contains(&program);
            let here = |path: &str| resolve(root, &dir, path);
            match Role::of(program, &arguments) {
                Role::Viewer => {
                    let paths: Vec<String> = plain(&arguments).map(here).collect();
                    // `… | head` shows the output of a command, not a file.
                    view.looks |= !paths.is_empty() || !command.is_fed_by_a_pipe();
                    view.reads_unnamed_files |= through_xargs || substitutes;
                    view.point_at(paths.into_iter());
                }
                Role::Searcher => {
                    let (patterns, places) = search_arguments(&arguments);
                    let filters_a_pipe = command.is_fed_by_a_pipe() && !through_xargs;
                    if patterns.is_empty() || filters_a_pipe {
                        continue;
                    }
                    let paths: Vec<String> = places.into_iter().map(here).collect();
                    view.looks = true;
                    view.lists_files = true;
                    view.point_at(paths.iter().cloned());
                    view.searches.push(CodeSearch { patterns, paths });
                }
                Role::GitViewer => {
                    view.looks = true;
                    view.lists_files = true;
                    view.point_at(plain(&arguments).skip(1).map(here));
                }
                Role::Lister => {
                    view.looks = true;
                    view.named.extend(arguments);
                }
                Role::NotLooking => {}
                Role::Other => view.named.extend(arguments),
            }
        }
        if feeds_an_interpreter {
            view.script = shell::heredoc_bodies(command_line);
        }
        view
    }

    fn point_at(&mut self, paths: impl Iterator<Item = String>) {
        for path in paths {
            if path.contains(GLOB_MARKS) {
                self.globs.push(path);
            } else {
                self.pointed_at.push(path);
            }
        }
    }

    fn sight(&self, root: &str, output: Option<&str>, file: &str) -> Sight {
        let listed = self.lists_files
            && output.is_some_and(|text| mentions(root, text, file) || diff_names(text, file));
        if self.pointed_at.iter().any(|path| path == file) {
            Sight::Opened
        } else if listed {
            Sight::Listed
        } else if self.named.iter().any(|word| mentions(root, word, file)) {
            Sight::Named
        } else if self.reads_unnamed_files
            || self.globs.iter().any(|glob| glob_covers(glob, file))
            || mentions(root, &self.script, file)
        {
            Sight::MayHave
        } else {
            Sight::None
        }
    }
}

/// The program a command runs and its arguments without redirects; `git grep` is read as
/// the `grep` it is.
fn program_and_arguments(words: &[String]) -> (&str, Vec<String>) {
    let arguments = without_redirects(words.get(1..).unwrap_or_default());
    match (shell::program(words), arguments.split_first()) {
        ("git", Some((subcommand, rest))) if subcommand == "grep" => ("grep", rest.to_vec()),
        (program, _) => (program, arguments),
    }
}

/// The command `xargs` runs, and whether there was an `xargs` before it: its file names then
/// come from its input.
fn without_xargs(words: &[String]) -> (&[String], bool) {
    if shell::program(words) != "xargs" {
        return (words, false);
    }
    let own_flags = words[1..]
        .iter()
        .take_while(|word| word.starts_with('-'))
        .count();
    (&words[1 + own_flags..], true)
}

/// A command's arguments without its redirects (`2>&1`, `> out.txt`).
fn without_redirects(arguments: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut is_a_target = false;
    for word in arguments {
        if std::mem::take(&mut is_a_target) {
            continue;
        }
        match redirect_wants_a_target(word) {
            Some(wants_a_target) => is_a_target = wants_a_target,
            None => kept.push(word.clone()),
        }
    }
    kept
}

/// `None` when `word` is not a redirect; otherwise whether its target is the next word
/// (`>` of `> out.txt`, against `>out.txt` and `2>&1`).
fn redirect_wants_a_target(word: &str) -> Option<bool> {
    let arrow = word.trim_start_matches(|c: char| c.is_ascii_digit());
    let arrow = arrow.strip_prefix('&').unwrap_or(arrow);
    if !arrow.starts_with(['>', '<']) {
        return None;
    }
    Some(arrow.trim_start_matches(['>', '<', '&']).is_empty())
}

/// The arguments that are not flags and read as a path or a glob.
fn plain(arguments: &[String]) -> impl Iterator<Item = &str> {
    arguments
        .iter()
        .map(String::as_str)
        .filter(|word| !word.starts_with('-') && reads_as_a_path(word))
}

/// A searcher's arguments: the patterns it looks for, and the places it is pointed at.
fn search_arguments(arguments: &[String]) -> (Vec<String>, Vec<&str>) {
    let pattern_is_flagged = arguments
        .iter()
        .any(|word| SEARCH_PATTERN_FLAGS.contains(&word.as_str()));
    let mut patterns = Vec::new();
    let mut places = Vec::new();
    let mut words = arguments.iter().map(String::as_str);
    while let Some(word) = words.next() {
        if word == "-e" || word == "--regexp" {
            patterns.extend(words.next().map(str::to_string));
        } else if SEARCH_FLAGS_WITH_VALUE.contains(&word) || SEARCH_PATTERN_FLAGS.contains(&word) {
            words.next();
        } else if word.starts_with('-') && word.len() > 1 {
            // Another flag (`-rn`), or `--` before the places.
        } else if patterns.is_empty() && !pattern_is_flagged {
            patterns.push(word.to_string());
        } else if reads_as_a_path(word) {
            places.push(word);
        }
    }
    (patterns, places)
}

/// A word made of path and glob characters only.
fn reads_as_a_path(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./+@~*?[]{},!".contains(c))
}

/// A path that reads as a file: no glob, and an extension at its end.
fn looks_like_a_file(path: &str) -> bool {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    !path.contains(GLOB_MARKS)
        && extension.is_some_and(|e| {
            (1..=6).contains(&e.len())
                && e.chars().all(|c| c.is_ascii_alphanumeric())
                && e.chars().any(|c| c.is_ascii_alphabetic())
        })
}

/// A path a shell command names, as the steps name paths: an absolute one by the session's
/// folder, any other as seen from `dir`.
fn resolve(root: &str, dir: &str, path: &str) -> String {
    if path.starts_with('/') {
        crate::trace::relative_to(root, path)
    } else {
        join(dir, path)
    }
}

/// A relative `path` as seen from `dir`, with `.` and `..` worked out. A `dir` outside the
/// session's folder (an absolute one) gives a path outside it.
fn join(dir: &str, path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in dir.split('/').chain(path.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let joined = parts.join("/");
    match (dir.starts_with('/'), joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// Whether a glob, as the shell would expand it, can stand for `file`. A glob this reader
/// cannot follow is taken to cover it: the answer then is "cannot tell", not "no".
fn glob_covers(glob: &str, file: &str) -> bool {
    let mut pattern = String::from("^");
    let mut chars = glob.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.next_if_eq(&'*').is_some() => pattern.push_str(".*"),
            '*' => pattern.push_str("[^/]*"),
            '?' => pattern.push_str("[^/]"),
            '[' | ']' => pattern.push(c),
            '{' => pattern.push_str("(?:"),
            '}' => pattern.push(')'),
            ',' => pattern.push('|'),
            other => pattern.push_str(&regex::escape(&other.to_string())),
        }
    }
    pattern.push('$');
    Regex::new(&pattern).map_or(true, |regex| regex.is_match(file))
}

/// Whether `text` names `file` (a path relative to the session's folder): written out in
/// full, on its own or under the session's folder, not as part of a longer path or name.
fn mentions(root: &str, text: &str, file: &str) -> bool {
    if file.is_empty() {
        return false;
    }
    let in_a_name = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    text.match_indices(file).any(|(at, _)| {
        let before = &text[..at];
        let starts_clean = match before.chars().next_back() {
            None => true,
            Some('/') => before.ends_with("./") || before.ends_with(&format!("{root}/")),
            Some(c) => !(in_a_name(c) || c == '.'),
        };
        let mut after = text[at + file.len()..].chars();
        let ends_clean = match (after.next(), after.next()) {
            (None, _) => true,
            // `path.rs.bak` is another file; `path.rs.` ends a sentence.
            (Some('.'), next) => !next.is_some_and(|c| c.is_alphanumeric()),
            (Some(c), _) => !(in_a_name(c) || c == '/'),
        };
        starts_clean && ends_clean
    })
}

/// Whether a diff in `text` is of `file`: git writes its paths as `a/path` and `b/path`.
fn diff_names(text: &str, file: &str) -> bool {
    ["a", "b"].iter().any(|side| {
        let path = format!(" {side}/{file}");
        text.match_indices(&path).any(|(at, _)| {
            let after = text[at + path.len()..].chars().next();
            after.is_none_or(char::is_whitespace)
        })
    })
}

/// Whether a search pattern holds `name` as a whole word.
fn holds_word(pattern: &str, name: &str) -> bool {
    let plain = REGEX_ESCAPE.replace_all(pattern, " ");
    let in_a_name = |c: char| c.is_alphanumeric() || c == '_';
    !name.is_empty()
        && plain.match_indices(name).any(|(at, _)| {
            let before = plain[..at].chars().next_back();
            let after = plain[at + name.len()..].chars().next();
            !before.is_some_and(in_a_name) && !after.is_some_and(in_a_name)
        })
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::trace::fixtures::{edit, glob, grep, read, read_directory, shell, shell_run, trace};

    const FILE: &str = "src/uri/path.rs";

    /// The most one call shows of `src/uri/path.rs`.
    fn sight_of(call: Value) -> Sight {
        let trace = trace(vec![call]);
        Seen::of(&trace).sight(0..1, FILE)
    }

    #[test]
    fn a_file_is_opened_when_a_step_is_pointed_at_it_by_name() {
        for call in [
            read("/testbed/src/uri/path.rs"),
            edit("/testbed/src/uri/path.rs", "fn b() {}", "fn b() { 2 }"),
            shell("cat src/uri/path.rs"),
            shell("cd /testbed/src/uri && sed -n '1,40p' path.rs"),
            shell("cd src && head -20 ./uri/path.rs | tail -5"),
            shell("grep -n 'fn b' src/uri/path.rs"),
            shell("git diff HEAD~1 -- src/uri/path.rs"),
            shell("wc -l /testbed/src/uri/path.rs"),
        ] {
            assert_eq!(sight_of(call.clone()), Sight::Opened, "{call}");
        }
    }

    #[test]
    fn a_search_or_a_diff_lists_the_files_in_its_output() {
        for call in [
            shell_run("rg 'fn b'", 0, "src/uri/path.rs:1:fn b() {}"),
            shell_run("grep -rn 'fn ' .", 0, "./src/uri/path.rs:1:fn b() {}"),
            grep(
                "fn b",
                "Found 1 matches\n/testbed/src/uri/path.rs:\n  Line 1: fn b() {}",
            ),
            shell_run(
                "git diff",
                0,
                "diff --git a/src/uri/path.rs b/src/uri/path.rs\n--- a/src/uri/path.rs\n",
            ),
            shell_run(
                "find . -name '*.rs' | xargs grep -l parse",
                0,
                "./src/uri/path.rs\n",
            ),
        ] {
            assert_eq!(sight_of(call.clone()), Sight::Listed, "{call}");
        }
    }

    #[test]
    fn a_command_that_names_a_file_without_showing_it_is_kept_apart() {
        for call in [
            shell("python3 -c \"print(open('/testbed/src/uri/path.rs').read())\""),
            shell("python analyse.py src/uri/path.rs"),
            shell("ls -la src/uri/path.rs"),
            glob("src/uri/path.rs"),
        ] {
            assert_eq!(sight_of(call.clone()), Sight::Named, "{call}");
        }
    }

    #[test]
    fn what_may_have_shown_a_file_without_naming_it_cannot_be_told() {
        for call in [
            shell("cat src/uri/*.rs"),
            shell("cd src/uri && cat *.rs"),
            shell("head -50 src/**/*.rs"),
            shell("cat $(find . -name path.rs)"),
            shell("cat `find . -name path.rs`"),
            shell("find . -name '*.rs' | xargs cat"),
            shell("grep -n parse src/uri/p*.rs"),
            shell("python3 - <<'EOF'\nprint(open('src/uri/path.rs').read())\nEOF"),
        ] {
            assert_eq!(sight_of(call.clone()), Sight::MayHave, "{call}");
        }
    }

    #[test]
    fn what_does_not_show_a_file_is_not_a_look() {
        for call in [
            read("/testbed/src/uri/mod.rs"),
            read_directory("/testbed/src/uri"),
            shell("cat src/other/uri/path.rs"),
            shell("cat vendor/src/uri/path.rs.orig > /dev/null"),
            shell("cat /testbed/src/uri/path.rs.bak"),
            shell("cat src/other/*.rs"),
            shell("ls src/uri"),
            shell("rm src/uri/path.rs"),
            shell("git checkout src/uri/path.rs"),
            shell("sed -i 's/a/b/' src/uri/path.rs"),
            shell("echo x >> src/uri/path.rs"),
            shell("echo 'now fix src/uri/path.rs'"),
            shell("vim src/uri/path.rs"),
            shell("cat > notes.md <<EOF\nlook at src/uri/path.rs\nEOF"),
            shell_run("cargo build", 101, "error[E0425]: --> src/uri/path.rs:1:1"),
            shell_run(
                "cargo build 2>&1 | grep -A5 error",
                0,
                "error[E0425]: --> src/uri/path.rs:1:1",
            ),
            shell_run("rg --files", 0, "src/uri/path.rs\n"),
            shell_run("find . -name '*.rs'", 0, "./src/uri/path.rs\n"),
            glob("**/*.rs"),
            grep("fn b", "No files found"),
        ] {
            assert_eq!(sight_of(call.clone()), Sight::None, "{call}");
        }
    }

    #[test]
    fn the_most_any_step_shows_is_what_counts_and_only_within_the_steps_asked_for() {
        let trace = trace(vec![
            shell("cat src/uri/*.rs"),
            shell_run("rg 'fn b'", 0, "src/uri/path.rs:1:fn b() {}"),
            read("/testbed/src/uri/path.rs"),
        ]);
        let seen = Seen::of(&trace);
        assert_eq!(seen.sight(0..1, FILE), Sight::MayHave);
        assert_eq!(seen.sight(0..2, FILE), Sight::Listed);
        assert_eq!(seen.sight(0..3, FILE), Sight::Opened);
        assert_eq!(seen.sight(0..0, FILE), Sight::None);
    }

    #[test]
    fn a_shell_command_points_at_files_from_where_it_runs() {
        let pointed_at = |command: &str, workdir: Option<&str>| {
            ShellView::of("/testbed", command, workdir).pointed_at
        };
        for (command, workdir, expected) in [
            ("cat src/lib.rs", None, vec!["src/lib.rs"]),
            ("cat lib.rs", Some("src"), vec!["src/lib.rs"]),
            ("cat Makefile", None, vec!["Makefile"]),
            (
                "cd src/uri && cat path.rs ../lib.rs",
                None,
                vec!["src/uri/path.rs", "src/lib.rs"],
            ),
            (
                "cd src && cat /testbed/Cargo.toml; cd /testbed; nl README.md",
                None,
                vec!["Cargo.toml", "README.md"],
            ),
            ("cd - && cat a.rs", Some("src"), vec!["src/a.rs"]),
            (
                "grep -n 'Vec<u8>' src/lib.rs > out.txt",
                None,
                vec!["src/lib.rs"],
            ),
            ("grep -A 5 parse src/lib.rs", None, vec!["src/lib.rs"]),
            ("git grep -n parse -- src/uri.rs", None, vec!["src/uri.rs"]),
            ("find . -name '*.rs' | xargs grep -l parse", None, vec![]),
            ("cat /tmp/notes.txt", None, vec!["/tmp/notes.txt"]),
            ("cat notes.txt", Some("/tmp"), vec!["/tmp/notes.txt"]),
            ("sed -i 's/a/b/' src/lib.rs", None, vec![]),
            ("cargo test --test it 2>&1 | tail -20", None, vec![]),
            ("echo done > src/lib.rs", None, vec![]),
        ] {
            assert_eq!(pointed_at(command, workdir), expected, "{command}");
        }
    }

    #[test]
    fn files_opened_are_those_named_with_an_extension() {
        let trace = trace(vec![
            read("/testbed/src/lib.rs"),
            read("/testbed/src/lib.rs"),
            shell("sed -n 1,20p src/uri/path.rs"),
            shell("head -n 20 Cargo.toml | tail -5"),
            shell("ls src"),
        ]);
        let seen = Seen::of(&trace);
        let opened: Vec<&str> = seen.files_opened().into_iter().collect();
        assert_eq!(opened, vec!["Cargo.toml", "src/lib.rs", "src/uri/path.rs"]);
        assert!((0..5).all(|at| seen.is_looking(at)));
    }

    #[test]
    fn a_search_for_a_name_is_a_whole_word_in_the_pattern_beyond_the_declaring_file() {
        let searched = |call: Value, name: &str| {
            let trace = trace(vec![call]);
            Seen::of(&trace).searched_for(name, "src/uri.rs")
        };
        for (call, name) in [
            (shell("grep -rn 'parse(' src/"), "parse"),
            (shell("cd src && rg -n \"\\.port\" | head"), "port"),
            (shell("git grep -n host"), "host"),
            (shell("rg -e 'fn\\s+parse' -e other"), "parse"),
            (shell("grep -rn '\\bnew\\b' ."), "new"),
            (shell("grep -n parse src/uri.rs src/lib.rs"), "parse"),
            (shell("find . -name '*.rs' | xargs grep -l parse"), "parse"),
            (grep("parse\\(", "No files found"), "parse"),
        ] {
            assert!(searched(call.clone(), name), "{call}");
        }
        for (call, name) in [
            (shell("grep -n 'pub fn parse' src/uri.rs"), "parse"),
            (shell("grep -rn error src/parse.rs"), "parse"),
            (shell("rg --files | grep parser"), "parse"),
            (shell("grep -rn 'renewal' src/"), "new"),
            (shell("cargo test 2>&1 | grep -i 'new'"), "new"),
            (shell("echo parse"), "parse"),
            (shell("cat src/parse.rs"), "parse"),
            (shell("cargo test parse"), "parse"),
            (glob("**/parse*"), "parse"),
        ] {
            assert!(!searched(call.clone(), name), "{call}");
        }
    }

    #[test]
    fn a_glob_covers_the_files_the_shell_would_expand_it_to() {
        for (glob, covered) in [
            ("src/uri/*.rs", true),
            ("src/*.rs", false),
            ("src/**/*.rs", true),
            ("src/uri/pa?h.rs", true),
            ("src/uri/[a-p]*.rs", true),
            ("src/{uri,other}/path.rs", true),
            ("tests/*.rs", false),
            ("src/uri/[", true),
        ] {
            assert_eq!(glob_covers(glob, FILE), covered, "{glob}");
        }
    }
}
