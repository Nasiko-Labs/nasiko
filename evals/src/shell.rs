//! A shell command line read as far as its words tell: the commands on it, what joins them,
//! and the words that may stand before a command without being it. Text in, values out: no
//! shell runs and nothing is expanded, so a variable, a glob or a `$(…)` stays as written.
//!
//! Shared by everything that reads the agent's shell commands ([`crate::selfcheck`] for build
//! and test runs, the `sight` module for what a command shows of a file).

/// What stands between two commands of one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// `|`: the next command reads this one's output.
    Pipe,
    /// `&&`.
    And,
    /// `||`.
    Or,
    /// `;` or a new line.
    Next,
    /// `&`: the command is left running in the background.
    Background,
    /// A parenthesis or a backtick.
    Group,
}

/// One command of a command line. Quoted text stays one word, so a command named inside a
/// message or a search pattern is not a command. A redirect (`2>&1`, `> out.txt`) stays among
/// the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub words: Vec<String>,
    /// What joins it to the command before it; `None` for the first.
    pub follows: Option<Link>,
    /// What comes after it; `None` when it ends the line.
    pub then: Option<Link>,
}

impl Command {
    /// Whether it reads the output of the command before it.
    pub fn is_fed_by_a_pipe(&self) -> bool {
        self.follows == Some(Link::Pipe)
    }
}

/// Words that may stand before a command without being it: wrappers and shell keywords.
const COMMAND_PREFIXES: [&str; 19] = [
    "timeout", "time", "nice", "nohup", "exec", "command", "env", "sudo", "if", "then", "else",
    "elif", "do", "while", "until", "!", "{", "npx", "bunx",
];

/// Two-word wrappers that run a project's command in its own environment (`uv run pytest`).
const RUNNERS: [(&str, &str); 7] = [
    ("uv", "run"),
    ("poetry", "run"),
    ("pdm", "run"),
    ("pipenv", "run"),
    ("hatch", "run"),
    ("pnpm", "exec"),
    ("bundle", "exec"),
];

/// The commands of a shell command line. Commands end at `;`, `&&`, `||`, `|`, `&`, a new
/// line, parentheses and backticks. Heredoc bodies and comments are dropped.
pub fn commands(command_line: &str) -> Vec<Command> {
    let script = without_heredoc_bodies(command_line);
    let mut reader = Reader::default();
    let mut quote: Option<char> = None;
    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(open), _) if c == open => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                // An escaped character is itself; an escaped line break joins two lines.
                if let Some(escaped) = chars.next().filter(|next| *next != '\n') {
                    reader.push(escaped);
                }
            }
            (Some(_), _) => reader.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                reader.word.get_or_insert_default();
            }
            (None, '#') if reader.word.is_none() => {
                // A comment runs to the end of its line.
                chars
                    .by_ref()
                    .take_while(|next| *next != '\n')
                    .for_each(drop);
                reader.end_command(Link::Next);
            }
            (None, '&') if reader.is_in_a_redirect(chars.peek()) => reader.push(c),
            (None, '&') => {
                let link = match chars.next_if_eq(&'&') {
                    Some(_) => Link::And,
                    None => Link::Background,
                };
                reader.end_command(link);
            }
            (None, '|') => {
                let link = match chars.next_if_eq(&'|') {
                    Some(_) => Link::Or,
                    None => Link::Pipe,
                };
                // `|&` pipes the error output too.
                chars.next_if_eq(&'&');
                reader.end_command(link);
            }
            (None, ';' | '\n') => reader.end_command(Link::Next),
            (None, '(' | ')' | '`') => reader.end_command(Link::Group),
            (None, _) if c.is_whitespace() => reader.end_word(),
            (None, _) => reader.push(c),
        }
    }
    reader.finish()
}

/// Whether a command line runs a command to put its output into another (`$(…)` or
/// backticks): what that inner command prints is not on the line.
pub fn has_substitution(command_line: &str) -> bool {
    command_line.contains("$(") || command_line.contains('`')
}

/// The program a command runs, without the folder it is called from.
pub fn program(words: &[String]) -> &str {
    words
        .first()
        .and_then(|word| word.rsplit('/').next())
        .unwrap_or_default()
}

/// The words of a command after what may stand before it: `VAR=value` settings, wrappers such
/// as `timeout 60` or `uv run` with their flags and numbers, and shell keywords.
pub fn without_prefixes(words: &[String]) -> &[String] {
    let mut rest = words;
    loop {
        let Some(first) = rest.first() else {
            return rest;
        };
        let second = rest.get(1).map(String::as_str).unwrap_or_default();
        let wrapper_words = if is_assignment(first) {
            Some(1)
        } else if RUNNERS.contains(&(first.as_str(), second)) {
            Some(2)
        } else if COMMAND_PREFIXES.contains(&first.as_str()) {
            Some(1)
        } else {
            None
        };
        let Some(wrapper_words) = wrapper_words else {
            return rest;
        };
        let after = &rest[wrapper_words..];
        let own_arguments = if is_assignment(first) {
            0
        } else {
            after
                .iter()
                .take_while(|w| w.starts_with('-') || w.starts_with(|c: char| c.is_ascii_digit()))
                .count()
        };
        rest = &after[own_arguments..];
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

/// Collects words into commands, remembering what stood between them.
#[derive(Default)]
struct Reader {
    commands: Vec<Command>,
    words: Vec<String>,
    word: Option<String>,
    /// What has stood between commands since the last one ended.
    between: Option<Link>,
}

impl Reader {
    fn push(&mut self, c: char) {
        self.word.get_or_insert_default().push(c);
    }

    fn end_word(&mut self) {
        if let Some(done) = self.word.take() {
            self.words.push(done);
        }
    }

    /// Whether an `&` belongs to a redirect (`2>&1`, `&> file`) rather than ending a command.
    fn is_in_a_redirect(&self, next: Option<&char>) -> bool {
        let after_an_arrow = self
            .word
            .as_deref()
            .is_some_and(|word| word.ends_with(['>', '<']));
        after_an_arrow || (self.word.is_none() && next == Some(&'>'))
    }

    fn end_command(&mut self, link: Link) {
        self.end_word();
        self.flush();
        if self.commands.is_empty() {
            return;
        }
        // A parenthesis next to `|` or `&&` says less than they do.
        self.between = match self.between {
            None | Some(Link::Group) => Some(link),
            said => said,
        };
    }

    fn flush(&mut self) {
        if self.words.is_empty() {
            return;
        }
        let follows = self.between.take();
        if let Some(previous) = self.commands.last_mut() {
            previous.then = follows;
        }
        self.commands.push(Command {
            words: std::mem::take(&mut self.words),
            follows,
            then: None,
        });
    }

    fn finish(mut self) -> Vec<Command> {
        self.end_word();
        self.flush();
        let trailing = self.between.filter(|link| *link != Link::Group);
        if let Some(last) = self.commands.last_mut() {
            last.then = trailing;
        }
        self.commands
    }
}

/// The text a command line feeds to its commands through heredocs (`python - <<EOF … EOF`).
pub fn heredoc_bodies(command_line: &str) -> String {
    split_heredocs(command_line).1
}

/// The script without the text it feeds to commands through heredocs (`cat > f <<EOF … EOF`):
/// that text is data, even when it reads like a command.
fn without_heredoc_bodies(script: &str) -> String {
    split_heredocs(script).0
}

/// A script's own lines, and the lines of its heredoc bodies.
fn split_heredocs(script: &str) -> (String, String) {
    let mut own = Vec::new();
    let mut bodies = Vec::new();
    let mut body_ends_at: Option<String> = None;
    for line in script.lines() {
        match &body_ends_at {
            Some(end) if line.trim() == end => body_ends_at = None,
            Some(_) => bodies.push(line),
            None => {
                own.push(line);
                body_ends_at = heredoc_end(line);
            }
        }
    }
    (own.join("\n"), bodies.join("\n"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn words_of(command_line: &str) -> Vec<Vec<String>> {
        commands(command_line)
            .into_iter()
            .map(|command| command.words)
            .collect()
    }

    fn links_of(command_line: &str) -> Vec<(String, Option<Link>, Option<Link>)> {
        commands(command_line)
            .into_iter()
            .map(|command| (command.words[0].clone(), command.follows, command.then))
            .collect()
    }

    #[test]
    fn a_line_is_split_into_commands_and_quoted_text_stays_one_word() {
        assert_eq!(
            words_of("cd /testbed && cargo test -p app 2>&1 | tail -20"),
            vec![
                vec!["cd", "/testbed"],
                vec!["cargo", "test", "-p", "app", "2>&1"],
                vec!["tail", "-20"],
            ]
        );
        assert_eq!(
            words_of("git commit -am \"fix: make cargo test pass\"; echo 'a; b'"),
            vec![
                vec!["git", "commit", "-am", "fix: make cargo test pass"],
                vec!["echo", "a; b"],
            ]
        );
        assert_eq!(
            words_of("echo start\ncargo test \\\n  -p app"),
            vec![vec!["echo", "start"], vec!["cargo", "test", "-p", "app"]]
        );
        assert_eq!(words_of("ls # cargo test"), vec![vec!["ls"]]);
        assert_eq!(
            heredoc_bodies("python - <<EOF\nimport os\nprint(1)\nEOF\nls"),
            "import os\nprint(1)"
        );
        assert_eq!(
            words_of("cat <<'EOF' > run.sh\ncargo test\nEOF\nls"),
            vec![vec!["cat", "<<EOF", ">", "run.sh"], vec!["ls"]]
        );
        assert!(commands("").is_empty());
    }

    #[test]
    fn each_command_knows_what_joins_it_to_its_neighbours() {
        assert_eq!(
            links_of("cd x && cargo test | tail; ls || true &"),
            vec![
                ("cd".to_string(), None, Some(Link::And)),
                ("cargo".to_string(), Some(Link::And), Some(Link::Pipe)),
                ("tail".to_string(), Some(Link::Pipe), Some(Link::Next)),
                ("ls".to_string(), Some(Link::Next), Some(Link::Or)),
                ("true".to_string(), Some(Link::Or), Some(Link::Background)),
            ]
        );
        // A closing parenthesis ends the line as nothing does; next to a pipe the pipe counts.
        assert_eq!(
            links_of("(cargo test)"),
            vec![("cargo".to_string(), None, None)]
        );
        assert_eq!(
            links_of("(cargo test) | tail"),
            vec![
                ("cargo".to_string(), None, Some(Link::Pipe)),
                ("tail".to_string(), Some(Link::Pipe), None),
            ]
        );
        assert_eq!(
            links_of("echo `cargo test`"),
            vec![
                ("echo".to_string(), None, Some(Link::Group)),
                ("cargo".to_string(), Some(Link::Group), None),
            ]
        );
        assert!(commands("cargo test | tail")[1].is_fed_by_a_pipe());
    }

    #[test]
    fn an_ampersand_inside_a_redirect_does_not_end_the_command() {
        for line in [
            "cargo test 2>&1",
            "cargo test >&2",
            "cargo test &> out.txt",
            "cargo test &>> out.txt",
        ] {
            let commands = commands(line);
            assert_eq!(commands.len(), 1, "{line}");
            assert_eq!(commands[0].then, None, "{line}");
        }
        assert_eq!(commands("cargo test|&tail").len(), 2);
    }

    #[test]
    fn wrappers_and_settings_before_a_command_are_not_the_command() {
        for (line, expected) in [
            ("RUSTFLAGS=-Dwarnings cargo b", "cargo b"),
            (
                "timeout --signal=TERM 600 env RUST_BACKTRACE=1 cargo test",
                "cargo test",
            ),
            ("uv run --frozen pytest -q", "pytest -q"),
            ("poetry run pytest", "pytest"),
            ("npx jest", "jest"),
            ("if cargo build", "cargo build"),
            ("uv pip install x", "uv pip install x"),
        ] {
            let words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
            assert_eq!(without_prefixes(&words).join(" "), expected, "{line}");
        }
        let cargo = ["/usr/local/cargo/bin/cargo".to_string()];
        assert_eq!(program(&cargo), "cargo");
        assert_eq!(program(&[]), "");
    }

    #[test]
    fn a_substitution_is_seen_in_either_spelling() {
        assert!(has_substitution("cat $(find . -name a.rs)"));
        assert!(has_substitution("cat `find . -name a.rs`"));
        assert!(!has_substitution("cat a.rs"));
    }
}
