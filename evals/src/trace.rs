//! What an agent did, as one ordered list of steps: looked at a file, searched, ran a shell
//! command, edited a file. The measures of how an agent worked ([`crate::method`],
//! [`crate::selfcheck`]) read only this list, so a session of another kind (the developer's own
//! session, say) can be measured the same way once a reader turns it into a [`Trace`].
//!
//! The one reader here is for `opencode export`, checked against the opencode release Nasiko
//! pins (1.19.5: the types of its SDK package and the source bundled in its binary):
//!
//! * The export is `{"info": session, "messages": [{"info": message, "parts": [part]}]}`;
//!   `info.directory` is the folder the session ran in.
//! * A tool call is a part `{"type": "tool", "tool": name, "state": {"status", "input", …}}`.
//!   `status` is `pending` (the call never ran; its input may be incomplete), `running`,
//!   `completed` (with `output`, `title`, `metadata`, `time.start`, `time.end` in milliseconds)
//!   or `error` (with `error`).
//! * Inputs by tool: `read` `filePath`; `grep` `pattern`, `path`, `include`; `glob` `pattern`,
//!   `path`; `bash` `command`, `workdir`, `timeout`; `edit` `filePath`, `oldString`,
//!   `newString`, `replaceAll`; `write` `filePath`, `content`; `apply_patch` `patchText` (the
//!   tool a `gpt-` model gets in place of `edit` and `write`).
//! * Results used: `bash` keeps the command's exit status in `metadata.exit`; `write` says in
//!   `metadata.exists` whether the file was there before; `read` of a directory answers with
//!   `<type>directory</type>` in its output.
//!
//! No export written by a real run has been through this reader yet, so it claims nothing from
//! a shape it does not know: an export without a `messages` list, one where no tool call could
//! be read, or one whose shell calls or edit calls all lack the field they need, is an error
//! ([`TraceError::UnknownShape`]), never an empty trace. The reader also accepts what Nasiko's
//! own session importer accepts (`content` for `parts`, `name` for `tool`) and the snake_case
//! and older tool names in the tables below.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// Where Nasiko's sandbox keeps the repository; used when the export does not say where the
/// session ran.
pub const DEFAULT_ROOT: &str = "/testbed";

/// Tools that open a file.
const READ_TOOLS: [&str; 2] = ["read", "view"];

/// Tools that list a directory (opencode releases before the pinned one).
const LIST_TOOLS: [&str; 2] = ["list", "ls"];

/// Tools that search file contents.
const CONTENT_SEARCH_TOOLS: [&str; 1] = ["grep"];

/// Tools that search file names.
const NAME_SEARCH_TOOLS: [&str; 1] = ["glob"];

/// opencode's shell tool, by the names its session formats use.
pub const SHELL_TOOLS: [&str; 2] = ["bash", "shell"];

/// Tools that replace text in one file.
const REPLACE_TOOLS: [&str; 2] = ["edit", "multiedit"];

/// Tools that write a whole file.
const WRITE_TOOLS: [&str; 1] = ["write"];

/// Tools that apply a patch in opencode's `*** Begin Patch` format.
const PATCH_TOOLS: [&str; 2] = ["apply_patch", "patch"];

/// `state.status` of a call that never ran.
const PENDING_STATUS: &str = "pending";

/// `state.status` of a call that was still running when the session ended.
const RUNNING_STATUS: &str = "running";

/// `state.status` values of a call that did not go through.
const FAILED_STATUSES: [&str; 2] = ["error", "failed"];

/// What a `read` of a directory answers with.
const DIRECTORY_MARK: &str = "<type>directory</type>";

/// How a tool call that does not say which tool it is gets counted.
const UNNAMED_TOOL: &str = "unnamed tool";

/// Every tool that changes a file, for messages that name them.
pub fn edit_tools() -> Vec<&'static str> {
    [&REPLACE_TOOLS[..], &WRITE_TOOLS[..], &PATCH_TOOLS[..]].concat()
}

/// What a search looks through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    /// The text of files (`grep`).
    Contents,
    /// The names of files (`glob`).
    FileNames,
}

/// One thing the agent did. Paths are relative to [`Trace::root`] where they lie under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Opened a file.
    Look { path: String },
    /// Listed a directory.
    List { path: String },
    Search {
        kind: SearchKind,
        pattern: String,
        /// The file or directory searched; `None` for the whole repository.
        path: Option<String>,
        /// The file-name filter of a contents search (`*.rs`).
        include: Option<String>,
        /// What the search answered, when the session kept it.
        output: Option<String>,
    },
    Shell {
        command: String,
        workdir: Option<String>,
        /// The command line's exit status, when the session kept it.
        exit: Option<i64>,
        output: Option<String>,
    },
    Edit {
        path: String,
        added: Vec<String>,
        /// Lines the edit took out. Empty for a whole-file write: what was there is not known.
        removed: Vec<String>,
        /// Whether the edit made a file that was not there before; `None` when the session
        /// does not say.
        creates: Option<bool>,
        /// Whether the whole file was written in one go, so `added` is all of its lines.
        whole_file: bool,
    },
}

/// The steps of one session, in the order they happened.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Trace {
    /// The folder the session ran in.
    pub root: String,
    pub steps: Vec<Step>,
    /// Tools the session called that are not turned into steps, by name.
    pub other_tools: Vec<String>,
    /// Calls of known tools that were left out because the field they need was missing.
    pub unread_calls: u32,
}

impl Trace {
    /// A path as the steps name it: relative to the session's folder when it lies under it.
    pub fn relative(&self, path: &str) -> String {
        relative_to(&self.root, path)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TraceError {
    #[error("not JSON: {0}")]
    NotJson(#[from] serde_json::Error),
    /// Valid JSON that is not laid out the way this reader knows.
    #[error("{0}")]
    UnknownShape(String),
}

/// Reads `opencode export <session>` output into a [`Trace`].
pub fn read_opencode_export(text: &str) -> Result<Trace, TraceError> {
    let export: Value = serde_json::from_str(text)?;
    let Some(messages) = export["messages"].as_array() else {
        return Err(TraceError::UnknownShape(
            "the session export has no `messages` list".into(),
        ));
    };
    let root = export["info"]["directory"]
        .as_str()
        .filter(|directory| !directory.is_empty())
        .unwrap_or(DEFAULT_ROOT)
        .trim_end_matches('/')
        .to_string();
    let mut reader = Reader::new(root);
    for part in messages.iter().flat_map(parts_of).filter(is_tool_call) {
        reader.read_call(part);
    }
    reader.finish()
}

/// A message's parts: under `parts`, or under `content` in opencode's newer session format.
fn parts_of(message: &Value) -> impl Iterator<Item = &Value> {
    ["parts", "content"]
        .into_iter()
        .filter_map(|key| message[key].as_array())
        .flatten()
}

fn is_tool_call(part: &&Value) -> bool {
    part["type"].as_str() == Some("tool")
}

/// The tool a call names: `tool`, or `name` in opencode's newer session format.
fn tool_name(part: &Value) -> &str {
    part["tool"]
        .as_str()
        .or_else(|| part["name"].as_str())
        .unwrap_or_default()
}

/// How many calls of one tool were seen, and how many of them could be read.
#[derive(Default)]
struct Tally {
    calls: u32,
    read: u32,
}

/// Turns tool calls into steps, keeping count of what could not be read.
struct Reader {
    trace: Trace,
    other_tools: BTreeSet<String>,
    tallies: BTreeMap<String, Tally>,
}

impl Reader {
    fn new(root: String) -> Self {
        Self {
            trace: Trace {
                root,
                ..Trace::default()
            },
            other_tools: BTreeSet::new(),
            tallies: BTreeMap::new(),
        }
    }

    fn read_call(&mut self, part: &Value) {
        let tool = tool_name(part);
        let state = &part["state"];
        let status = state["status"].as_str().unwrap_or_default();
        if tool.is_empty() {
            // Not a tool this reader skips on purpose: a call it cannot tell apart at all.
            self.tallies.entry(UNNAMED_TOOL.into()).or_default().calls += 1;
            self.trace.unread_calls += 1;
            return;
        }
        if !is_known(tool) {
            self.other_tools.insert(tool.to_string());
            return;
        }
        // A pending call never ran. A failed look, search or edit showed and changed nothing;
        // a failed shell call is kept, because the command may have run in part.
        let failed = FAILED_STATUSES.contains(&status) && !SHELL_TOOLS.contains(&tool);
        if status == PENDING_STATUS || failed {
            return;
        }
        let steps = self.steps_of(tool, state);
        if steps.is_empty() {
            self.trace.unread_calls += 1;
        }
        // A call cut off while it ran may lack its input; that says nothing about the format.
        if !(steps.is_empty() && status == RUNNING_STATUS) {
            let tally = self.tallies.entry(tool.to_string()).or_default();
            tally.calls += 1;
            tally.read += u32::from(!steps.is_empty());
        }
        self.trace.steps.extend(steps);
    }

    /// The steps one call stands for; empty when the field the tool needs is missing.
    fn steps_of(&self, tool: &str, state: &Value) -> Vec<Step> {
        let input = &state["input"];
        let path = text_field(input, &["filePath", "file_path", "path"])
            .map(|path| self.trace.relative(path));
        let one = |step: Option<Step>| step.into_iter().collect::<Vec<Step>>();
        if READ_TOOLS.contains(&tool) {
            one(path.map(|path| look_or_list(path, state)))
        } else if LIST_TOOLS.contains(&tool) {
            vec![Step::List {
                path: path.unwrap_or_else(|| ".".into()),
            }]
        } else if CONTENT_SEARCH_TOOLS.contains(&tool) {
            one(search(SearchKind::Contents, input, path, state))
        } else if NAME_SEARCH_TOOLS.contains(&tool) {
            one(search(SearchKind::FileNames, input, path, state))
        } else if SHELL_TOOLS.contains(&tool) {
            one(self.shell(input, state))
        } else if REPLACE_TOOLS.contains(&tool) {
            one(path.and_then(|path| replacement(path, input)))
        } else if WRITE_TOOLS.contains(&tool) {
            one(path.and_then(|path| whole_file_write(path, input, state)))
        } else {
            self.patch_edits(input)
        }
    }

    fn shell(&self, input: &Value, state: &Value) -> Option<Step> {
        let command = text_field(input, &["command"])?;
        Some(Step::Shell {
            command: command.to_string(),
            workdir: text_field(input, &["workdir"]).map(|dir| self.trace.relative(dir)),
            exit: state["metadata"]["exit"].as_i64(),
            output: output_text(state),
        })
    }

    fn patch_edits(&self, input: &Value) -> Vec<Step> {
        let Some(text) = text_field(input, &["patchText", "patch_text", "patch"]) else {
            return Vec::new();
        };
        let mut edits = patch_text_edits(text);
        for edit in &mut edits {
            if let Step::Edit { path, .. } = edit {
                *path = self.trace.relative(path);
            }
        }
        edits
    }

    /// The trace, unless the export is not laid out the way this reader knows: no call of any
    /// tool could be read, or not one call of a tool that runs commands or changes files.
    /// Then nothing is claimed from it. A look or search tool whose calls could not be read,
    /// next to others that could, only adds to [`Trace::unread_calls`].
    fn finish(mut self) -> Result<Trace, TraceError> {
        let nothing_read = self.tallies.values().all(|tally| tally.read == 0);
        let unknown = self
            .tallies
            .iter()
            .filter(|(_, tally)| tally.calls > 0 && tally.read == 0)
            .find(|(tool, _)| nothing_read || runs_or_changes(tool));
        if let Some((tool, tally)) = unknown {
            return Err(TraceError::UnknownShape(format!(
                "none of the session's {} `{tool}` call(s) carries the input this report reads",
                tally.calls
            )));
        }
        self.trace.other_tools = self.other_tools.into_iter().collect();
        Ok(self.trace)
    }
}

/// Whether a tool runs shell commands or changes files: the calls every measure of method
/// rests on.
fn runs_or_changes(tool: &str) -> bool {
    SHELL_TOOLS.contains(&tool) || edit_tools().contains(&tool)
}

fn is_known(tool: &str) -> bool {
    [
        &READ_TOOLS[..],
        &LIST_TOOLS[..],
        &CONTENT_SEARCH_TOOLS[..],
        &NAME_SEARCH_TOOLS[..],
        &SHELL_TOOLS[..],
        &REPLACE_TOOLS[..],
        &WRITE_TOOLS[..],
        &PATCH_TOOLS[..],
    ]
    .iter()
    .any(|names| names.contains(&tool))
}

/// The first of `keys` that holds text.
fn text_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| value[*key].as_str())
}

/// What a call answered: `output`, the text of `content` in opencode's newer session format,
/// or the error of a call that failed.
fn output_text(state: &Value) -> Option<String> {
    if let Some(output) = state["output"].as_str() {
        return Some(output.to_string());
    }
    let texts: Vec<&str> = state["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["text"].as_str())
        .collect();
    if !texts.is_empty() {
        return Some(texts.join("\n"));
    }
    state["error"].as_str().map(str::to_string)
}

fn look_or_list(path: String, state: &Value) -> Step {
    let lists_a_directory = state["output"]
        .as_str()
        .is_some_and(|output| output.contains(DIRECTORY_MARK));
    if lists_a_directory {
        Step::List { path }
    } else {
        Step::Look { path }
    }
}

fn search(kind: SearchKind, input: &Value, path: Option<String>, state: &Value) -> Option<Step> {
    Some(Step::Search {
        kind,
        pattern: text_field(input, &["pattern"])?.to_string(),
        path,
        include: text_field(input, &["include"]).map(str::to_string),
        output: output_text(state),
    })
}

/// An `edit` call, or a `multiedit` call with its list of replacements.
fn replacement(path: String, input: &Value) -> Option<Step> {
    let pairs: Vec<&Value> = match input["edits"].as_array() {
        Some(edits) => edits.iter().collect(),
        None => vec![input],
    };
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut old_texts = Vec::new();
    for pair in pairs {
        let old = text_field(pair, &["oldString", "old_string"]);
        let new = text_field(pair, &["newString", "new_string"]);
        if old.is_none() && new.is_none() {
            return None;
        }
        let (more_added, more_removed) =
            changed_lines(old.unwrap_or_default(), new.unwrap_or_default());
        added.extend(more_added);
        removed.extend(more_removed);
        old_texts.push(old.unwrap_or_default());
    }
    // opencode's `edit` makes a new file when the text to replace is empty.
    let creates = old_texts.first().is_some_and(|old| old.is_empty());
    Some(Step::Edit {
        path,
        added,
        removed,
        creates: Some(creates),
        whole_file: false,
    })
}

fn whole_file_write(path: String, input: &Value, state: &Value) -> Option<Step> {
    let content = text_field(input, &["content"])?;
    Some(Step::Edit {
        path,
        added: content.lines().map(str::to_string).collect(),
        removed: Vec::new(),
        creates: state["metadata"]["exists"].as_bool().map(|exists| !exists),
        whole_file: true,
    })
}

/// The lines that differ between two texts, as (added, removed). The texts of an edit often
/// stop without a line end; one is put there, so a last line that did not change is not read
/// as removed and added again.
fn changed_lines(old: &str, new: &str) -> (Vec<String>, Vec<String>) {
    let with_line_end = |text: &str| match text.is_empty() || text.ends_with('\n') {
        true => text.to_string(),
        false => format!("{text}\n"),
    };
    let (old, new) = (with_line_end(old), with_line_end(new));
    let patch = diffy::create_patch(&old, &new);
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for line in patch.hunks().iter().flat_map(|hunk| hunk.lines()) {
        let whole_line = |text: &str| text.trim_end_matches(['\n', '\r']).to_string();
        match line {
            diffy::Line::Insert(text) => added.push(whole_line(text)),
            diffy::Line::Delete(text) => removed.push(whole_line(text)),
            diffy::Line::Context(_) => {}
        }
    }
    (added, removed)
}

const PATCH_ADD: &str = "*** Add File: ";
const PATCH_UPDATE: &str = "*** Update File: ";
const PATCH_DELETE: &str = "*** Delete File: ";

/// One edit per file of a patch in opencode's format:
///
/// ```text
/// *** Begin Patch
/// *** Update File: src/lib.rs
/// @@
/// -old line
/// +new line
/// *** Add File: src/new.rs
/// +first line
/// *** End Patch
/// ```
fn patch_text_edits(text: &str) -> Vec<Step> {
    let mut edits: Vec<Step> = Vec::new();
    for line in text.lines() {
        let header = [
            (PATCH_ADD, true),
            (PATCH_UPDATE, false),
            (PATCH_DELETE, false),
        ]
        .into_iter()
        .find_map(|(mark, creates)| Some((line.strip_prefix(mark)?, creates)));
        if let Some((path, creates)) = header {
            edits.push(Step::Edit {
                path: path.trim().to_string(),
                added: Vec::new(),
                removed: Vec::new(),
                creates: Some(creates),
                whole_file: false,
            });
        } else if line.starts_with("***") {
            // `*** Begin Patch`, `*** End Patch`, `*** Move to:`, `*** End of File`.
        } else if let Some(Step::Edit { added, removed, .. }) = edits.last_mut() {
            if let Some(text) = line.strip_prefix('+') {
                added.push(text.to_string());
            } else if let Some(text) = line.strip_prefix('-') {
                removed.push(text.to_string());
            }
        }
    }
    edits
}

/// Whether a path, as the steps name it, lies in the folder the session ran in. A path
/// outside it stays absolute (a scratch file under `/tmp`) or climbs out with `..`.
pub fn is_under_root(path: &str) -> bool {
    !(path.starts_with('/') || path.starts_with("..") || path.starts_with('~'))
}

/// `path` relative to `root` when it lies under it, without a leading `./`.
pub(crate) fn relative_to(root: &str, path: &str) -> String {
    let path = path.trim();
    let inside = path
        .strip_prefix(root)
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
        .map(|rest| rest.trim_start_matches('/'));
    let relative = match inside {
        Some("") => ".",
        Some(rest) => rest,
        None => path,
    };
    let trimmed = relative.trim_start_matches("./").trim_end_matches('/');
    if trimmed.is_empty() {
        ".".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Tool calls shaped as opencode 1.19.5 writes them, for this crate's tests.

    use serde_json::{Value, json};

    fn call(tool: &str, input: Value, output: &str, metadata: Value) -> Value {
        json!({"id": "prt_1", "sessionID": "ses_1", "messageID": "msg_1", "type": "tool",
               "callID": "call_1", "tool": tool,
               "state": {"status": "completed", "input": input, "output": output,
                         "title": "", "metadata": metadata,
                         "time": {"start": 1_700_000_000_000u64, "end": 1_700_000_000_500u64}}})
    }

    pub fn read(path: &str) -> Value {
        let output = format!("<path>{path}</path>\n<type>file</type>\n<content>\n1: fn a() {{}}");
        call(
            "read",
            json!({"filePath": path}),
            &output,
            json!({"truncated": false}),
        )
    }

    pub fn read_directory(path: &str) -> Value {
        let output = format!("<path>{path}</path>\n<type>directory</type>\n<entries>\nlib.rs");
        call("read", json!({"filePath": path}), &output, json!({}))
    }

    pub fn grep(pattern: &str, output: &str) -> Value {
        call(
            "grep",
            json!({"pattern": pattern}),
            output,
            json!({"matches": 1}),
        )
    }

    pub fn glob(pattern: &str) -> Value {
        call(
            "glob",
            json!({"pattern": pattern}),
            "No files found",
            json!({"count": 0}),
        )
    }

    pub fn shell(command: &str) -> Value {
        shell_run(command, 0, "")
    }

    pub fn shell_run(command: &str, exit: i64, output: &str) -> Value {
        let metadata = json!({"output": output, "exit": exit, "truncated": false});
        call("bash", json!({"command": command}), output, metadata)
    }

    pub fn edit(path: &str, old: &str, new: &str) -> Value {
        let input = json!({"filePath": path, "oldString": old, "newString": new});
        call(
            "edit",
            input,
            "Edit applied successfully.",
            json!({"diff": ""}),
        )
    }

    /// A `write` call; `existed` is whether the file was there before.
    pub fn write(path: &str, content: &str, existed: bool) -> Value {
        let input = json!({"filePath": path, "content": content});
        let metadata = json!({"filepath": path, "exists": existed});
        call("write", input, "Wrote file successfully.", metadata)
    }

    pub fn failed(mut call: Value) -> Value {
        call["state"]["status"] = json!("error");
        call["state"]["error"] = json!("File not found");
        call
    }

    /// A session export with these tool calls, split over two assistant messages.
    pub fn export(calls: Vec<Value>) -> String {
        let (first, second) = calls.split_at(calls.len() / 2);
        json!({"info": {"id": "ses_1", "directory": "/testbed"}, "messages": [
            {"info": {"role": "user"}, "parts": [{"type": "text", "text": "fix it"}]},
            {"info": {"role": "assistant"}, "parts": first},
            {"info": {"role": "assistant"}, "parts": second},
        ]})
        .to_string()
    }

    pub fn trace(calls: Vec<Value>) -> super::Trace {
        super::read_opencode_export(&export(calls)).expect("a readable export")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::fixtures::*;
    use super::*;

    fn edit_step(path: &str, added: &[&str], removed: &[&str], creates: Option<bool>) -> Step {
        Step::Edit {
            path: path.into(),
            added: added.iter().map(|line| line.to_string()).collect(),
            removed: removed.iter().map(|line| line.to_string()).collect(),
            creates,
            whole_file: false,
        }
    }

    #[test]
    fn every_kind_of_call_becomes_a_step_in_order_with_paths_under_the_root() {
        let trace = trace(vec![
            read_directory("/testbed/src"),
            read("/testbed/src/lib.rs"),
            grep(
                "fn a",
                "Found 1 matches\n/testbed/src/lib.rs:\n  Line 1: fn a() {}",
            ),
            glob("**/*.toml"),
            shell_run("cargo test", 101, "test result: FAILED"),
            edit(
                "/testbed/src/lib.rs",
                "fn a() {}\nfn b() {}\n",
                "fn a() { b() }\nfn b() {}\n",
            ),
            json!({"type": "tool", "tool": "todowrite", "state": {"status": "completed"}}),
        ]);
        assert_eq!(trace.root, "/testbed");
        assert_eq!(trace.other_tools, vec!["todowrite"]);
        assert_eq!(trace.unread_calls, 0);
        let expected = vec![
            Step::List { path: "src".into() },
            Step::Look {
                path: "src/lib.rs".into(),
            },
            Step::Search {
                kind: SearchKind::Contents,
                pattern: "fn a".into(),
                path: None,
                include: None,
                output: Some("Found 1 matches\n/testbed/src/lib.rs:\n  Line 1: fn a() {}".into()),
            },
            Step::Search {
                kind: SearchKind::FileNames,
                pattern: "**/*.toml".into(),
                path: None,
                include: None,
                output: Some("No files found".into()),
            },
            Step::Shell {
                command: "cargo test".into(),
                workdir: None,
                exit: Some(101),
                output: Some("test result: FAILED".into()),
            },
            edit_step(
                "src/lib.rs",
                &["fn a() { b() }"],
                &["fn a() {}"],
                Some(false),
            ),
        ];
        assert_eq!(trace.steps, expected);
    }

    #[test]
    fn a_write_says_whether_the_file_was_there_and_an_empty_old_text_makes_one() {
        let trace = trace(vec![
            write("/testbed/tests/new.rs", "#[test]\nfn new() {}\n", false),
            write("/testbed/src/lib.rs", "fn a() {}\n", true),
            edit("/testbed/src/made.rs", "", "fn made() {}\n"),
        ]);
        let facts: Vec<(&str, usize, Option<bool>, bool)> = trace
            .steps
            .iter()
            .map(|step| match step {
                Step::Edit {
                    path,
                    added,
                    creates,
                    whole_file,
                    ..
                } => (path.as_str(), added.len(), *creates, *whole_file),
                other => panic!("not an edit: {other:?}"),
            })
            .collect();
        assert_eq!(
            facts,
            vec![
                ("tests/new.rs", 2, Some(true), true),
                ("src/lib.rs", 1, Some(false), true),
                ("src/made.rs", 1, Some(true), false),
            ]
        );
    }

    #[test]
    fn a_patch_call_is_one_edit_per_file() {
        let patch = "*** Begin Patch\n*** Update File: src/lib.rs\n@@ fn a\n-old\n+new\n+more\n context\n*** Add File: /testbed/src/new.rs\n+fn new() {}\n*** Delete File: src/gone.rs\n*** End Patch";
        let call = json!({"type": "tool", "tool": "apply_patch",
            "state": {"status": "completed", "input": {"patchText": patch}}});
        let expected = vec![
            edit_step("src/lib.rs", &["new", "more"], &["old"], Some(false)),
            edit_step("src/new.rs", &["fn new() {}"], &[], Some(true)),
            edit_step("src/gone.rs", &[], &[], Some(false)),
        ];
        assert_eq!(trace(vec![call]).steps, expected);
    }

    #[test]
    fn calls_that_never_ran_or_failed_are_not_steps_but_a_failed_shell_call_is() {
        let pending = json!({"type": "tool", "tool": "edit",
            "state": {"status": "pending", "input": {}, "raw": ""}});
        let trace = trace(vec![
            pending,
            failed(read("/testbed/src/missing.rs")),
            failed(edit("/testbed/src/lib.rs", "x", "y")),
            failed(shell("cargo test")),
        ]);
        assert_eq!(trace.steps.len(), 1, "{:?}", trace.steps);
        assert!(matches!(&trace.steps[0], Step::Shell { command, .. } if command == "cargo test"));
    }

    #[test]
    fn the_newer_session_format_is_read_the_same_way() {
        let session = json!({"session_id": "s", "messages": [
            {"id": "m", "type": "assistant", "content": [
                {"type": "tool", "name": "shell", "state": {"status": "completed",
                    "input": {"command": "cargo test"},
                    "content": [{"type": "text", "text": "ok"}]}},
                {"type": "tool", "name": "edit", "state": {"status": "completed",
                    "input": {"file_path": "/testbed/a.rs", "old_string": "a", "new_string": "b"}}},
                {"type": "text", "text": "Done"},
            ]},
        ]});
        let trace = read_opencode_export(&session.to_string()).expect("readable");
        let expected = vec![
            Step::Shell {
                command: "cargo test".into(),
                workdir: None,
                exit: None,
                output: Some("ok".into()),
            },
            edit_step("a.rs", &["b"], &["a"], Some(false)),
        ];
        assert_eq!(trace.steps, expected);
    }

    #[test]
    fn a_shape_this_reader_does_not_know_claims_nothing() {
        let bare_shell = json!({"type": "tool", "tool": "bash"});
        let unknown = read_opencode_export(&export(vec![bare_shell.clone(), bare_shell]));
        let message = unknown.expect_err("unknown shape").to_string();
        assert_eq!(
            message,
            "none of the session's 2 `bash` call(s) carries the input this report reads"
        );
        let renamed_field = json!({"type": "tool", "tool": "edit",
            "state": {"status": "completed", "input": {"target": "a.rs", "text": "x"}}});
        assert!(matches!(
            read_opencode_export(&export(vec![renamed_field])),
            Err(TraceError::UnknownShape(_))
        ));
        assert!(matches!(
            read_opencode_export("{}"),
            Err(TraceError::UnknownShape(_))
        ));
        let unnamed = json!({"type": "tool", "function": "bash", "arguments": {"cmd": "ls"}});
        let message = read_opencode_export(&export(vec![unnamed]))
            .expect_err("unknown shape")
            .to_string();
        assert!(message.contains("1 `unnamed tool` call(s)"), "{message}");
        assert!(matches!(
            read_opencode_export("not json"),
            Err(TraceError::NotJson(_))
        ));
    }

    #[test]
    fn one_unreadable_call_among_readable_ones_is_counted_not_fatal() {
        let without_command = json!({"type": "tool", "tool": "bash",
            "state": {"status": "running", "input": {}}});
        let trace = trace(vec![shell("ls"), without_command]);
        assert_eq!((trace.steps.len(), trace.unread_calls), (1, 1));
    }

    #[test]
    fn an_unread_search_does_not_cost_the_session_but_an_unread_shell_or_edit_tool_does() {
        let odd_search = json!({"type": "tool", "tool": "grep",
            "state": {"status": "completed", "input": {"query": "fn a"}}});
        let trace = trace(vec![
            read("/testbed/src/lib.rs"),
            edit("/testbed/src/lib.rs", "a", "b"),
            odd_search,
        ]);
        assert_eq!((trace.steps.len(), trace.unread_calls), (2, 1));
        // Every measure rests on shell commands and edits: one of those tools unread is a
        // format this reader does not know, however much else it read.
        let odd_shell = json!({"type": "tool", "tool": "bash",
            "state": {"status": "completed", "input": {"cmd": "cargo test"}}});
        let unknown = read_opencode_export(&export(vec![read("/testbed/a.rs"), odd_shell]));
        assert_eq!(
            unknown.expect_err("unknown shape").to_string(),
            "none of the session's 1 `bash` call(s) carries the input this report reads"
        );
        // A call cut off while it ran never got its input: that is not a format.
        let cut_off = json!({"type": "tool", "tool": "bash",
            "state": {"status": "running", "input": {}}});
        let trace = super::fixtures::trace(vec![read("/testbed/a.rs"), cut_off]);
        assert_eq!((trace.steps.len(), trace.unread_calls), (1, 1));
    }

    #[test]
    fn an_edit_whose_texts_stop_without_a_line_end_counts_only_the_lines_that_differ() {
        let trace = trace(vec![
            edit("/testbed/a.rs", "fn a() {}\nfn b() {}", "fn a() {}"),
            edit("/testbed/a.rs", "fn a() {}", "fn a() {}\nfn c() {}"),
        ]);
        let expected = vec![
            edit_step("a.rs", &[], &["fn b() {}"], Some(false)),
            edit_step("a.rs", &["fn c() {}"], &[], Some(false)),
        ];
        assert_eq!(trace.steps, expected);
    }

    #[test]
    fn a_session_without_tool_calls_is_an_empty_trace() {
        let trace = trace(Vec::new());
        assert!(trace.steps.is_empty() && trace.other_tools.is_empty());
    }

    #[test]
    fn paths_are_made_relative_only_under_the_root() {
        for (path, expected) in [
            ("/testbed/src/lib.rs", "src/lib.rs"),
            ("/testbed", "."),
            ("/testbed/", "."),
            ("./src/lib.rs", "src/lib.rs"),
            ("src/", "src"),
            ("/testbed2/src/lib.rs", "/testbed2/src/lib.rs"),
            ("/tmp/notes.txt", "/tmp/notes.txt"),
        ] {
            assert_eq!(relative_to("/testbed", path), expected, "{path}");
        }
        let elsewhere = json!({"info": {"directory": "/work/repo/"}, "messages": [
            {"info": {}, "parts": [read("/work/repo/src/lib.rs")]}]});
        let trace = read_opencode_export(&elsewhere.to_string()).expect("readable");
        assert_eq!(trace.root, "/work/repo");
        assert_eq!(
            trace.steps,
            vec![Step::Look {
                path: "src/lib.rs".into()
            }]
        );
    }
}
