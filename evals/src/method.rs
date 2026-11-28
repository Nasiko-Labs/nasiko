//! How an agent worked, read from its steps ([`crate::trace`]) next to its patch and the real
//! fix: did it look where the real fix is, did it search for the names of the shared items it
//! changed, did it run anything before changing code, how much did it write to keep how
//! little, and did it look at a file before editing it.
//!
//! These are signals about method, never verdicts about the change: a correct fix can come out
//! of a session that scores badly on all of them, and the other way round. Limits they share:
//!
//! * Only what the session records is seen (the `sight` module says what counts as looking). Where
//!   a command may have shown a file without naming it, the answer is "cannot tell", and the
//!   attempt is left out of the share rather than counted either way.
//! * Edits are the calls of the agent's edit tools. A file changed by a shell command
//!   (`sed -i`, a redirect) is not seen as an edit, so the measures that need the order of
//!   edits are left out for an attempt that only edited that way.
//! * Whether a file was there before the session is taken from the patches and from what the
//!   edit tool says; where neither says (a whole-file write the session does not mark, to a
//!   file that is in no patch), the file is taken as a scratch file and not as existing code.
//! * Shared items are found by Rust's `pub` on the declaration line, in `.rs` files only.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::diff::{self, FileDiff, LineKind, TestPathRules};
use crate::report::{Rate, rate_of};
use crate::scope::BUILD_FILES;
use crate::selfcheck::{self, RunEnd, SelfCheck};
use crate::sight::{Seen, Sight};
use crate::stats;
use crate::trace::{self, Step, Trace};

/// Attributes that mark the Rust item after them as a test, or as test-only code.
const TEST_ATTRIBUTE_MARKS: [&str; 5] = ["test]", "test(", "cfg(test", "rstest", "test_case"];

/// An attempt wrote much more than it kept above this many lines written per line kept.
pub const REWORK_RATIO: f64 = 2.0;

/// Most file or item names spelled out in one sentence; the rest are counted.
const MAX_NAMES: usize = 3;

/// A `pub` item's declaration line: its kind and its name.
static DECLARATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^\s*pub(?:\([^)]*\))?\s+(?:(?:async|const|unsafe|default|extern(?:\s+"[^"]*")?)\s+)*(?:fn|struct|enum|trait|const|static\s+mut|static|type|union)\s+([A-Za-z_][A-Za-z0-9_]*)"#,
    )
    .expect("a valid regex")
});

/// A `pub` field's line: its name.
static FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*pub(?:\([^)]*\))?\s+([a-z_][A-Za-z0-9_]*)\s*:[^:]").expect("a valid regex")
});

/// What the measures read besides the agent's steps. A patch that is not on disk is `None`,
/// and the measures that need it are left out rather than guessed.
pub struct Evidence<'a> {
    /// The real fix (the task's gold patch).
    pub fix: Option<&'a [FileDiff]>,
    /// The attempt's patch as it was graded (no test files, edits to tests inside source
    /// files undone where the run could).
    pub graded: Option<&'a [FileDiff]>,
    /// The attempt's patch as the agent handed it in.
    pub handed_in: Option<&'a [FileDiff]>,
    pub rules: &'a TestPathRules,
    /// The repository's own build and test commands ([`selfcheck::spec_commands`]).
    pub own_commands: &'a [String],
}

/// How one attempt worked. A measure is `None` when what it needs is missing: a patch, the
/// real fix, a Rust file to read declarations from, or any edit made with the agent's edit
/// tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Method {
    pub fix_files: Option<FixFiles>,
    pub shared_items: Option<SharedItems>,
    pub breadth: Breadth,
    pub first_run: Option<FirstRun>,
    pub rework: Option<Rework>,
    pub blind_edits: Option<BlindEdits>,
}

impl Method {
    /// One plain sentence per measure that could be read, in the report's order; the sentence
    /// about testing its own fix is taken from `self_check`.
    pub fn sentences(&self, self_check: Option<&SelfCheck>) -> Vec<String> {
        [
            self.fix_files.as_ref().map(FixFiles::sentence),
            self.shared_items.as_ref().map(SharedItems::sentence),
            Some(self.breadth.sentence()),
            self.first_run.as_ref().map(FirstRun::sentence),
            self.rework.as_ref().map(Rework::sentence),
            self_check.map(SelfCheck::sentence),
            self.blind_edits.as_ref().map(BlindEdits::sentence),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// Measures one attempt.
pub fn measure(trace: &Trace, evidence: &Evidence) -> Method {
    let seen = Seen::of(trace);
    let origins = origins_of_edits(trace, &Patches::of(evidence));
    Method {
        fix_files: evidence
            .fix
            .zip(evidence.graded)
            .and_then(|(fix, model)| fix_files(&seen, trace, fix, model, evidence.rules)),
        shared_items: evidence
            .graded
            .and_then(|model| shared_items(&seen, model, evidence.rules)),
        breadth: breadth(&seen, trace),
        first_run: first_run(trace, &origins, evidence),
        rework: rework(trace, &origins, evidence),
        blind_edits: blind_edits(&seen, trace, &origins),
    }
}

fn is_edit(step: &Step) -> bool {
    matches!(step, Step::Edit { .. })
}

fn is_build_file(path: &str) -> bool {
    BUILD_FILES.contains(&path.rsplit('/').next().unwrap_or_default())
}

/// Up to [`MAX_NAMES`] names spelled out, the rest counted: `a, b, c and 2 more`.
pub(crate) fn named(names: &[String]) -> String {
    let shown = names
        .iter()
        .take(MAX_NAMES)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match names.len().saturating_sub(MAX_NAMES) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

/// Every path a patch names, before and after a rename.
fn paths_of(files: &[FileDiff]) -> BTreeSet<&str> {
    files
        .iter()
        .flat_map(|file| [file.old_path.as_deref(), file.new_path.as_deref()])
        .flatten()
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Which files were there before the session
// ---------------------------------------------------------------------------------------------

/// Whether a file an edit touches was there before the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    WasThere,
    MadeByTheAgent,
    /// Outside the repository: a scratch file that cannot end up in a patch.
    Outside,
    /// Neither the patches nor the session say.
    Unknown,
}

/// What the attempt's patches and the real fix say about which files were there before.
struct Patches<'a> {
    /// Files the attempt's patch adds.
    added: BTreeSet<&'a str>,
    /// Files a patch changes, removes or renames: they were there.
    there_before: BTreeSet<&'a str>,
}

impl<'a> Patches<'a> {
    fn of(evidence: &Evidence<'a>) -> Self {
        let own = || {
            [evidence.graded, evidence.handed_in]
                .into_iter()
                .flatten()
                .flatten()
        };
        Self {
            added: own()
                .filter(|file| file.old_path.is_none())
                .map(FileDiff::path)
                .collect(),
            there_before: own()
                .chain(evidence.fix.into_iter().flatten())
                .filter_map(|file| file.old_path.as_deref())
                .collect(),
        }
    }
}

/// For each step that is an edit, where the file it edits comes from; `None` for the other
/// steps. A file keeps the origin its first edit shows: once the agent made a file, its later
/// edits to it are edits to its own file.
fn origins_of_edits(trace: &Trace, patches: &Patches) -> Vec<Option<Origin>> {
    let mut origins: BTreeMap<&str, Origin> = BTreeMap::new();
    let mut opened: BTreeSet<&str> = BTreeSet::new();
    let mut of_steps = Vec::with_capacity(trace.steps.len());
    for step in &trace.steps {
        of_steps.push(match step {
            Step::Edit { path, creates, .. } => {
                let origin = origins
                    .entry(path)
                    .or_insert_with(|| origin_at_first_edit(path, *creates, patches, &opened));
                Some(*origin)
            }
            Step::Look { path } => {
                opened.insert(path);
                None
            }
            _ => None,
        });
    }
    of_steps
}

/// The patches are believed first, then the edit tool (`creates`); a file the agent's read
/// tool `opened` before it was edited was there.
fn origin_at_first_edit(
    path: &str,
    creates: Option<bool>,
    patches: &Patches,
    opened: &BTreeSet<&str>,
) -> Origin {
    if !trace::is_under_root(path) {
        Origin::Outside
    } else if patches.there_before.contains(path) {
        Origin::WasThere
    } else if patches.added.contains(path) || creates == Some(true) {
        Origin::MadeByTheAgent
    } else if creates == Some(false) || opened.contains(path) {
        Origin::WasThere
    } else {
        Origin::Unknown
    }
}

/// An edit to a file that was there before the session.
fn edits_an_existing_file(origin: &Option<Origin>) -> bool {
    *origin == Some(Origin::WasThere)
}

// ---------------------------------------------------------------------------------------------
// A1. The places the real fix touched
// ---------------------------------------------------------------------------------------------

/// The files the real fix changed, by what the attempt did with each. Files the real fix
/// creates are left out (there was nothing to look at), as are test files and lock files.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FixFiles {
    /// Files the attempt's final patch changes too.
    pub changed: Vec<String>,
    /// Files the agent looked at and left unchanged: it opened them, a search it ran listed
    /// them, or a command named them.
    pub looked_not_changed: Vec<String>,
    /// Those of `looked_not_changed` it never opened: a search listed them or a command named
    /// them, no more. One broad search lists many files.
    pub never_opened: Vec<String>,
    /// Files a command may have shown without naming them (a glob, `$(…)`, `xargs`, a script):
    /// whether the agent saw them cannot be told.
    pub cannot_tell: Vec<String>,
    /// Files the session shows no look at.
    pub never_looked: Vec<String>,
}

impl FixFiles {
    pub fn total(&self) -> usize {
        self.changed.len()
            + self.looked_not_changed.len()
            + self.cannot_tell.len()
            + self.never_looked.len()
    }

    /// The files the agent opened and left unchanged.
    pub fn opened_not_changed(&self) -> Vec<String> {
        self.looked_not_changed
            .iter()
            .filter(|file| !self.never_opened.contains(file))
            .cloned()
            .collect()
    }

    /// Whether the agent looked at every file the real fix changed; `None` when that cannot
    /// be told.
    pub fn looked_at_all(&self) -> Option<bool> {
        if !self.never_looked.is_empty() {
            Some(false)
        } else if !self.cannot_tell.is_empty() {
            None
        } else {
            Some(true)
        }
    }

    pub fn sentence(&self) -> String {
        let mut sentence = format!(
            "The real fix changed {} file(s); it changed {} of them",
            self.total(),
            self.changed.len()
        );
        if !self.looked_not_changed.is_empty() {
            sentence.push_str(&format!(
                ", looked at {} more without changing {}",
                self.looked_not_changed.len(),
                named(&self.looked_not_changed)
            ));
        }
        if !self.never_opened.is_empty() {
            sentence.push_str(&format!(
                " ({} of them never opened: only listed by a search or named in a command)",
                self.never_opened.len()
            ));
        }
        if !self.cannot_tell.is_empty() {
            sentence.push_str(&format!(
                "; whether it saw {} cannot be told",
                named(&self.cannot_tell)
            ));
        }
        if !self.never_looked.is_empty() {
            sentence.push_str(&format!(
                "; its session shows no look at {}",
                named(&self.never_looked)
            ));
        }
        sentence.push('.');
        sentence
    }
}

/// Sorts the files of the real fix by what the attempt did with them. `model` is the
/// attempt's final patch. `None` when the real fix changes no file that was already there.
fn fix_files(
    seen: &Seen,
    trace: &Trace,
    fix: &[FileDiff],
    model: &[FileDiff],
    rules: &TestPathRules,
) -> Option<FixFiles> {
    let model_paths = paths_of(model);
    let places: BTreeSet<&str> = fix
        .iter()
        .filter_map(|file| file.old_path.as_deref())
        .filter(|path| !rules.is_test(path) && !is_build_file(path))
        .collect();
    if places.is_empty() {
        return None;
    }
    let mut files = FixFiles::default();
    for place in places {
        let file = place.to_string();
        if model_paths.contains(place) {
            files.changed.push(file);
            continue;
        }
        match seen.sight(0..trace.steps.len(), place) {
            Sight::Opened => files.looked_not_changed.push(file),
            Sight::Listed | Sight::Named => {
                files.looked_not_changed.push(file.clone());
                files.never_opened.push(file);
            }
            Sight::MayHave => files.cannot_tell.push(file),
            Sight::None => files.never_looked.push(file),
        }
    }
    Some(files)
}

// ---------------------------------------------------------------------------------------------
// A2. Searched for a shared item's name when changing it
// ---------------------------------------------------------------------------------------------

/// One `pub` item whose declaration the attempt's patch removes or changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedItem {
    pub name: String,
    pub file: String,
    /// Whether the agent ran a contents search for the name, beyond the file that declares
    /// it, at any point of its session. It does not show that it read what the search found.
    pub searched: bool,
}

/// The shared items an attempt changed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SharedItems {
    pub items: Vec<SharedItem>,
}

impl SharedItems {
    fn names_where(&self, wanted: impl Fn(&SharedItem) -> bool) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| wanted(item))
            .map(|item| item.name.clone())
            .collect()
    }

    /// The names the agent searched for.
    pub fn searched(&self) -> Vec<String> {
        self.names_where(|item| item.searched)
    }

    /// The names the session shows no search for.
    pub fn not_searched(&self) -> Vec<String> {
        self.names_where(|item| !item.searched)
    }

    /// Whether the agent searched for the name of every shared item it changed.
    pub fn searched_for_all(&self) -> bool {
        self.items.iter().all(|item| item.searched)
    }

    pub fn sentence(&self) -> String {
        let (searched, not_searched) = (self.searched(), self.not_searched());
        match (self.items.len(), searched.len()) {
            (0, _) => {
                "Changed no shared item (no `pub` declaration changed in its Rust files).".into()
            }
            (all, 0) => format!(
                "Changed {all} shared item(s) ({}); its session shows no search for the name of any.",
                named(&not_searched)
            ),
            (all, found) if found == all => format!(
                "Changed {all} shared item(s) ({}) and searched for the name of each.",
                named(&searched)
            ),
            (all, found) => format!(
                "Changed {all} shared item(s) and searched for the name of {found}; not of {}.",
                named(&not_searched)
            ),
        }
    }
}

/// The `pub` items whose declaration `model` removes or changes in Rust files that are not
/// tests, each with whether the agent searched for its name ([`Seen::searched_for`]). An item
/// is counted per file, so the same name changed in two files is two items. `None` when the
/// patch changes no such file: there is nothing this reader could find a declaration in.
///
/// A declaration is its line up to the body (`{`) or the end (`;`), white space apart: a line
/// that only moved, was reformatted, or changed in its body is not a change. A signature
/// spread over several lines is seen only when its first line changes.
fn shared_items(seen: &Seen, model: &[FileDiff], rules: &TestPathRules) -> Option<SharedItems> {
    let sources: Vec<&FileDiff> = model
        .iter()
        .filter(|file| file.path().ends_with(".rs") && !rules.is_test(file.path()))
        .collect();
    if sources.is_empty() {
        return None;
    }
    let mut items: Vec<SharedItem> = Vec::new();
    for file in sources {
        for name in changed_declarations(file) {
            items.push(SharedItem {
                searched: seen.searched_for(&name, file.path()),
                name,
                file: file.path().to_string(),
            });
        }
    }
    Some(SharedItems { items })
}

/// The names of the `pub` items whose declaration a file's diff removes or changes, each
/// once, in the order the diff has them.
fn changed_declarations(file: &FileDiff) -> Vec<String> {
    let lines: Vec<(LineKind, String)> = diff::hunks(file)
        .into_iter()
        .flat_map(|hunk| hunk.lines)
        .collect();
    let declared = |wanted: LineKind| {
        lines
            .iter()
            .filter(move |(kind, _)| *kind == wanted)
            .filter_map(|(_, line)| Some((declared_name(line)?, signature(line))))
    };
    let added: BTreeSet<(&str, String)> = declared(LineKind::Added).collect();
    let mut names: Vec<String> = Vec::new();
    for declaration in declared(LineKind::Removed) {
        let name = declaration.0;
        if !added.contains(&declaration) && names.iter().all(|known| known != name) {
            names.push(name.to_string());
        }
    }
    names
}

/// The name a `pub` declaration or field line declares.
fn declared_name(line: &str) -> Option<&str> {
    let captures = DECLARATION
        .captures(line)
        .or_else(|| FIELD.captures(line))?;
    captures.get(1).map(|name| name.as_str())
}

/// A declaration line up to its body or its end, white space and a closing comma apart: the
/// part its users depend on.
fn signature(line: &str) -> String {
    let head = line.split(['{', ';']).next().unwrap_or_default();
    head.trim()
        .trim_end_matches(',')
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// A3. Breadth
// ---------------------------------------------------------------------------------------------

/// How widely the agent looked, and when.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Breadth {
    /// Distinct files it opened by name (the read tool, or a shell viewer or search pointed
    /// at them). Files a glob stands for are not counted.
    pub files_looked_at: u32,
    /// Distinct files it edited with its edit tools.
    pub files_edited: u32,
    /// Steps spent looking (opening, listing, searching) before its first edit, and after.
    pub looks_before_first_edit: u32,
    pub looks_after_first_edit: u32,
    /// The share of its looking that came before its first edit; `None` when it never looked
    /// or made no edit with its edit tools.
    pub share_before_first_edit: Option<f64>,
}

impl Breadth {
    pub fn sentence(&self) -> String {
        let counts = format!(
            "Opened {} file(s) and edited {}",
            self.files_looked_at, self.files_edited
        );
        match self.share_before_first_edit {
            Some(share) => format!(
                "{counts}; {:.0}% of its looking came before its first edit.",
                share * 100.0
            ),
            None => format!("{counts}."),
        }
    }
}

fn breadth(seen: &Seen, trace: &Trace) -> Breadth {
    let steps = trace.steps.len();
    let first_edit = trace.steps.iter().position(is_edit);
    let edited: BTreeSet<&str> = trace
        .steps
        .iter()
        .filter_map(|step| match step {
            Step::Edit { path, .. } => Some(path.as_str()),
            _ => None,
        })
        .collect();
    let split = first_edit.unwrap_or(steps);
    let before = (0..split).filter(|at| seen.is_looking(*at)).count() as u32;
    let after = (split..steps).filter(|at| seen.is_looking(*at)).count() as u32;
    Breadth {
        files_looked_at: seen.files_opened().len() as u32,
        files_edited: edited.len() as u32,
        looks_before_first_edit: before,
        looks_after_first_edit: after,
        share_before_first_edit: (first_edit.is_some() && before + after > 0)
            .then(|| f64::from(before) / f64::from(before + after)),
    }
}

// ---------------------------------------------------------------------------------------------
// B. Ran the build or tests before changing code
// ---------------------------------------------------------------------------------------------

/// What the agent ran before it first changed code that was already there. Writing a file of
/// its own (a reproduction script, say), editing that file again, writing outside the
/// repository, editing under a test path and adding a Rust test to a source file are not that
/// first change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirstRun {
    /// Whether a build or test command ran before that first change. A command that did not
    /// start (not found) is not a run.
    pub ran_before_first_change: bool,
    /// How the most telling of those runs ended: a failure shown before anything else.
    /// `None` when there was no run.
    pub outcome: Option<RunEnd>,
    /// The first build or test command it ran before that change.
    pub command: Option<String>,
    /// Build or test commands before that change that did not start.
    pub did_not_start: u32,
}

impl FirstRun {
    /// Whether a run before the first change showed a failure.
    pub fn showed_failure(&self) -> bool {
        self.outcome == Some(RunEnd::ShowedFailure)
    }

    pub fn sentence(&self) -> String {
        let ran = "Ran the build or tests before changing existing code";
        match (self.ran_before_first_change, self.outcome) {
            (false, _) if self.did_not_start > 0 => "Tried to run the build or tests before changing existing code, but the command did not start.".into(),
            (false, _) => "Changed existing code without running the build or tests first.".into(),
            (true, Some(RunEnd::ShowedFailure)) => format!("{ran}, and that run showed a failure."),
            (true, Some(RunEnd::NoFailureSeen)) => format!("{ran}; that run showed no failure."),
            (true, Some(RunEnd::CutOff)) => format!("{ran}; that run was cut off before it finished."),
            (true, _) => format!("{ran}; the session does not show how that run ended."),
        }
    }
}

/// `None` when the agent made no change to existing code with its edit tools: there is no
/// "before" to look at.
fn first_run(trace: &Trace, origins: &[Option<Origin>], evidence: &Evidence) -> Option<FirstRun> {
    let first_change = trace
        .steps
        .iter()
        .zip(origins)
        .position(|(step, origin)| changes_existing_code(step, origin, evidence.rules))?;
    let ends: Vec<(&str, RunEnd)> = trace.steps[..first_change]
        .iter()
        .filter_map(|step| match step {
            Step::Shell {
                command,
                exit,
                output,
                ..
            } => selfcheck::check_run(command, *exit, output.as_deref(), evidence.own_commands)
                .map(|end| (command.as_str(), end)),
            _ => None,
        })
        .collect();
    let runs: Vec<&(&str, RunEnd)> = ends
        .iter()
        .filter(|(_, end)| *end != RunEnd::DidNotStart)
        .collect();
    Some(FirstRun {
        ran_before_first_change: !runs.is_empty(),
        outcome: runs.iter().map(|(_, end)| *end).max_by_key(how_telling),
        command: runs.first().map(|(command, _)| command.to_string()),
        did_not_start: (ends.len() - runs.len()) as u32,
    })
}

/// Which of several runs to report: one that showed a failure before one that showed none,
/// before one whose end is not known.
fn how_telling(end: &RunEnd) -> u8 {
    match end {
        RunEnd::ShowedFailure => 4,
        RunEnd::NoFailureSeen => 3,
        RunEnd::NotShown => 2,
        RunEnd::CutOff => 1,
        RunEnd::DidNotStart => 0,
    }
}

/// Whether a step changes code that was already there: an edit to a file that was there
/// before the session, not under a test path, that does more than add Rust tests.
fn changes_existing_code(step: &Step, origin: &Option<Origin>, rules: &TestPathRules) -> bool {
    let Step::Edit {
        path,
        added,
        removed,
        whole_file,
        ..
    } = step
    else {
        return false;
    };
    // A whole-file write replaces the code that was there, whatever tests it holds.
    let only_adds_tests = !whole_file && removed.is_empty() && adds_only_tests(added);
    edits_an_existing_file(origin) && !rules.is_test(path) && !only_adds_tests
}

/// Whether added lines are Rust tests and nothing else: every item at their own top level
/// stands behind a test attribute (`#[test]`, `#[tokio::test]`, `#[cfg(test)]`). Braces are
/// counted as written, so a brace inside a string can mislead it.
fn adds_only_tests(added: &[String]) -> bool {
    let mut depth = 0i32;
    let mut behind_a_test_attribute = false;
    let mut tests = 0;
    let code = added
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with("//"));
    for line in code {
        if depth == 0 {
            let is_attribute = line.starts_with("#[");
            if is_attribute && TEST_ATTRIBUTE_MARKS.iter().any(|mark| line.contains(mark)) {
                behind_a_test_attribute = true;
            } else if !is_attribute && std::mem::take(&mut behind_a_test_attribute) {
                tests += 1;
            } else if !is_attribute && !line.starts_with('}') {
                return false;
            }
        }
        let opened = line.matches('{').count() as i32;
        let closed = line.matches('}').count() as i32;
        depth = (depth + opened - closed).max(0);
    }
    tests > 0
}

// ---------------------------------------------------------------------------------------------
// C. Guessed and redid
// ---------------------------------------------------------------------------------------------

/// How much the agent wrote against how much it kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rework {
    /// Lines added over all its edits to files in the repository that are not test files.
    pub lines_written: u32,
    /// Lines its patch as handed in adds outside test files; a test it wrote inside a source
    /// file is in both counts. See `kept_read_from_graded_patch`.
    pub lines_kept: u32,
    /// Lines written per line kept; 1.0 means it wrote its change once. `None` when it kept
    /// no line. Under 1.0 when part of the change was made some other way (a shell command).
    pub ratio: Option<f64>,
    /// True when the patch as handed in was not on disk and the graded patch stood in. Grading
    /// undoes edits to tests inside source files, so such a test then counts as written and
    /// not as kept, and the ratio can read high.
    pub kept_read_from_graded_patch: bool,
    /// Whole-file writes over a file that was already there. Every line of such a write
    /// counts as written, so a model that rewrites files in full reads high.
    pub whole_file_rewrites: u32,
    /// Files it edited that are not in the patch it handed in.
    pub abandoned_files: Vec<String>,
}

impl Rework {
    /// Whether it wrote lines and kept none of them.
    pub fn kept_nothing(&self) -> bool {
        self.lines_written > 0 && self.lines_kept == 0
    }

    /// Whether it wrote more than [`REWORK_RATIO`] times what it kept, or kept nothing of
    /// what it wrote.
    pub fn is_heavy(&self) -> bool {
        self.kept_nothing() || self.ratio.is_some_and(|ratio| ratio > REWORK_RATIO)
    }

    /// The lines written and kept, as the middle of a sentence.
    pub fn in_words(&self) -> String {
        if self.kept_nothing() {
            format!("{} line(s) written and none kept", self.lines_written)
        } else {
            format!(
                "{} line(s) written to end with a {}-line change",
                self.lines_written, self.lines_kept
            )
        }
    }

    pub fn sentence(&self) -> String {
        let mut sentence = if self.kept_nothing() {
            format!("Wrote {} line(s) and kept none of them", self.lines_written)
        } else {
            format!(
                "Wrote {} line(s) to end with a {}-line change",
                self.lines_written, self.lines_kept
            )
        };
        if self.whole_file_rewrites > 0 {
            sentence.push_str(&format!(
                " ({} whole-file rewrite(s) counted in full)",
                self.whole_file_rewrites
            ));
        }
        if !self.abandoned_files.is_empty() {
            sentence.push_str(&format!(
                "; edited {} that is not in its final patch",
                named(&self.abandoned_files)
            ));
        }
        sentence.push('.');
        sentence
    }
}

/// One edit to a file inside the repository that is not a lock file.
struct RepoEdit<'a> {
    path: &'a str,
    lines_added: usize,
    /// Whether it wrote the whole of a file that was already there.
    rewrites_whole_file: bool,
}

/// The edits that can end up in a patch. A scratch file outside the repository cannot, and a
/// lock file is not the agent's own writing.
fn repo_edits<'a>(trace: &'a Trace, origins: &[Option<Origin>]) -> Vec<RepoEdit<'a>> {
    trace
        .steps
        .iter()
        .zip(origins)
        .filter_map(|(step, origin)| match step {
            Step::Edit {
                path,
                added,
                whole_file,
                ..
            } if trace::is_under_root(path) && !is_build_file(path) => Some(RepoEdit {
                path,
                lines_added: added.len(),
                rewrites_whole_file: *whole_file && edits_an_existing_file(origin),
            }),
            _ => None,
        })
        .collect()
}

/// `None` without the graded patch, or when the agent made no edit inside the repository with
/// its edit tools.
fn rework(trace: &Trace, origins: &[Option<Origin>], evidence: &Evidence) -> Option<Rework> {
    let graded = evidence.graded?;
    let rules = evidence.rules;
    let edits = repo_edits(trace, origins);
    if edits.is_empty() {
        return None;
    }
    let in_code: Vec<&RepoEdit> = edits.iter().filter(|e| !rules.is_test(e.path)).collect();
    let lines_written: usize = in_code.iter().map(|edit| edit.lines_added).sum();
    // The patch as handed in still holds tests written inside source files; the graded one
    // has them undone.
    let lines_kept: usize = evidence
        .handed_in
        .unwrap_or(graded)
        .iter()
        .filter(|file| !rules.is_test(file.path()) && !is_build_file(file.path()))
        .map(|file| file.added.len())
        .sum();
    let rewrites = in_code.iter().filter(|edit| edit.rewrites_whole_file);
    Some(Rework {
        lines_written: lines_written as u32,
        lines_kept: lines_kept as u32,
        ratio: (lines_kept > 0).then(|| lines_written as f64 / lines_kept as f64),
        kept_read_from_graded_patch: evidence.handed_in.is_none(),
        whole_file_rewrites: rewrites.count() as u32,
        abandoned_files: abandoned_files(&edits, graded, evidence.handed_in, rules),
    })
}

/// Files the agent edited that are not in the patch it handed in. Without that patch the
/// graded one stands in and test files are left out: grading drops them, so they cannot be
/// told from abandoned ones.
fn abandoned_files(
    edits: &[RepoEdit],
    graded: &[FileDiff],
    handed_in: Option<&[FileDiff]>,
    rules: &TestPathRules,
) -> Vec<String> {
    let final_paths = paths_of(handed_in.unwrap_or(graded));
    let abandoned: BTreeSet<&str> = edits
        .iter()
        .map(|edit| edit.path)
        .filter(|path| !final_paths.contains(path))
        .filter(|path| handed_in.is_some() || !rules.is_test(path))
        .collect();
    abandoned.into_iter().map(str::to_string).collect()
}

// ---------------------------------------------------------------------------------------------
// E. Read before changing
// ---------------------------------------------------------------------------------------------

/// Whether the agent looked at the files it edited before editing them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BlindEdits {
    /// Files that were there before the session and that the agent edited. Files it made, and
    /// files whose origin is not known, do not count.
    pub existing_files_edited: u32,
    /// Those the session shows no look at before their first edit.
    pub without_looking: Vec<String>,
    /// Those a command named or may have shown before their first edit, without a program
    /// known to show files: whether the agent saw them cannot be told.
    pub cannot_tell: Vec<String>,
}

impl BlindEdits {
    /// Whether the agent looked at every existing file before its first edit to it; `None`
    /// when that cannot be told.
    pub fn looked_at_each_first(&self) -> Option<bool> {
        if !self.without_looking.is_empty() {
            Some(false)
        } else if !self.cannot_tell.is_empty() {
            None
        } else {
            Some(true)
        }
    }

    pub fn sentence(&self) -> String {
        let all = self.existing_files_edited;
        match (all, self.without_looking.len(), self.cannot_tell.len()) {
            (0, _, _) => "Edited no file known to have been there before its session.".into(),
            (_, 0, 0) => format!("Looked at each of the {all} existing file(s) it edited first."),
            (_, 0, _) => format!(
                "Edited {all} existing file(s); whether it looked at {} first cannot be told.",
                named(&self.cannot_tell)
            ),
            (_, blind, _) => format!(
                "Edited {all} existing file(s), {blind} of them with no look shown first ({}).",
                named(&self.without_looking)
            ),
        }
    }
}

/// `None` when the agent made no edit with its edit tools. Looking here is opening the file
/// or a search listing it; a command that only names the file does not show it.
fn blind_edits(seen: &Seen, trace: &Trace, origins: &[Option<Origin>]) -> Option<BlindEdits> {
    let mut edited: BTreeSet<&str> = BTreeSet::new();
    let mut edits = BlindEdits::default();
    for (at, (step, origin)) in trace.steps.iter().zip(origins).enumerate() {
        let Step::Edit { path, .. } = step else {
            continue;
        };
        let is_first_edit_of_it = edited.insert(path.as_str());
        if !is_first_edit_of_it || !edits_an_existing_file(origin) {
            continue;
        }
        edits.existing_files_edited += 1;
        match seen.sight(0..at, path) {
            Sight::Opened | Sight::Listed => {}
            Sight::Named | Sight::MayHave => edits.cannot_tell.push(path.clone()),
            Sight::None => edits.without_looking.push(path.clone()),
        }
    }
    (!edited.is_empty()).then_some(edits)
}

// ---------------------------------------------------------------------------------------------
// Per model
// ---------------------------------------------------------------------------------------------

/// The method figures of a group of attempts. Each rate's `n` is the attempts the measure
/// could be read for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodGroup {
    /// Attempts whose session was read.
    pub sessions: u32,
    /// Attempts set against a real fix that changes a file that was already there.
    pub compared_with_fix: u32,
    /// Share of those attempts that looked at every such file, of the ones it can be told for.
    pub looked_at_every_fix_file: Rate,
    /// Files of the real fix over the group's attempts, by what was done with them.
    pub fix_files_changed: u32,
    pub fix_files_looked_not_changed: u32,
    /// Those of `fix_files_looked_not_changed` that were never opened.
    pub fix_files_never_opened: u32,
    pub fix_files_cannot_tell: u32,
    pub fix_files_never_looked: u32,
    /// Attempts whose patch changes a Rust file that is not a test: those shared items could
    /// be read for.
    pub shared_items_read: u32,
    /// Of the attempts that changed a shared item: the share that searched for the name of
    /// every one.
    pub searched_for_every_shared_item: Rate,
    pub shared_items_changed: u32,
    pub shared_items_searched: u32,
    pub median_files_looked_at: Option<f64>,
    pub median_files_edited: Option<f64>,
    pub median_share_of_looking_before_first_edit: Option<f64>,
    /// Of the attempts that changed existing code with edit tools: the share that ran the
    /// build or tests before that first change.
    pub ran_before_first_change: Rate,
    /// Attempts whose run before the first change showed a failure.
    pub first_run_showed_failure: u32,
    /// Middle number of lines written per line kept, over the attempts that kept a line.
    pub median_rework_ratio: Option<f64>,
    /// Of the attempts that edited inside the repository with edit tools: the share that
    /// wrote more than [`REWORK_RATIO`] times what they kept, or kept none of it.
    pub reworked_heavily: Rate,
    /// Attempts, of the same ones, that wrote lines and kept none.
    pub kept_nothing: u32,
    /// Attempts, of the same ones, that edited a file that is not in their final patch.
    pub with_abandoned_files: u32,
    /// Attempts that edited a file that was there before the session, with edit tools.
    pub edited_existing_files: u32,
    /// Share of those attempts that looked at each such file first, of the ones it can be
    /// told for.
    pub looked_before_every_edit: Rate,
    pub files_edited_without_looking: u32,
    pub files_edited_cannot_tell: u32,
}

/// One model's method figures: every attempt whose session was read, then the same split by
/// whether the hidden tests passed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodSummary {
    pub all: MethodGroup,
    pub resolved: MethodGroup,
    pub not_resolved: MethodGroup,
}

/// Summarises one model's attempts; each item is (whether the attempt resolved, its method).
pub fn summarise(attempts: &[(bool, &Method)]) -> MethodSummary {
    let only = |wanted: bool| -> Vec<&Method> {
        attempts
            .iter()
            .filter(|(resolved, _)| *resolved == wanted)
            .map(|(_, method)| *method)
            .collect()
    };
    let all: Vec<&Method> = attempts.iter().map(|(_, method)| *method).collect();
    MethodSummary {
        all: group(&all),
        resolved: group(&only(true)),
        not_resolved: group(&only(false)),
    }
}

/// The share of `items` for which `holds`, over those it can be told for.
fn share<T>(items: &[&T], holds: impl Fn(&T) -> Option<bool>) -> Rate {
    let told: Vec<bool> = items.iter().filter_map(|item| holds(item)).collect();
    let hits = told.iter().filter(|held| **held).count();
    rate_of(hits as u32, told.len() as u32)
}

/// How many of `items` `counts`.
fn count<T>(items: &[&T], counts: impl Fn(&T) -> bool) -> u32 {
    items.iter().filter(|item| counts(item)).count() as u32
}

/// The sum of `size` over `items`.
fn total<T>(items: &[&T], size: impl Fn(&T) -> usize) -> u32 {
    items.iter().map(|item| size(item)).sum::<usize>() as u32
}

fn group(methods: &[&Method]) -> MethodGroup {
    let fix_files: Vec<&FixFiles> = methods
        .iter()
        .filter_map(|m| m.fix_files.as_ref())
        .collect();
    let read_for_items: Vec<&SharedItems> = methods
        .iter()
        .filter_map(|m| m.shared_items.as_ref())
        .collect();
    let shared: Vec<&SharedItems> = read_for_items
        .iter()
        .copied()
        .filter(|shared| !shared.items.is_empty())
        .collect();
    let first_runs: Vec<&FirstRun> = methods
        .iter()
        .filter_map(|m| m.first_run.as_ref())
        .collect();
    let reworks: Vec<&Rework> = methods.iter().filter_map(|m| m.rework.as_ref()).collect();
    let blind: Vec<&BlindEdits> = methods
        .iter()
        .filter_map(|m| m.blind_edits.as_ref())
        .filter(|edits| edits.existing_files_edited > 0)
        .collect();
    let median = |values: Vec<f64>| stats::median(&values);
    let breadths = || methods.iter().map(|m| &m.breadth);
    MethodGroup {
        sessions: methods.len() as u32,
        compared_with_fix: fix_files.len() as u32,
        looked_at_every_fix_file: share(&fix_files, FixFiles::looked_at_all),
        fix_files_changed: total(&fix_files, |f| f.changed.len()),
        fix_files_looked_not_changed: total(&fix_files, |f| f.looked_not_changed.len()),
        fix_files_never_opened: total(&fix_files, |f| f.never_opened.len()),
        fix_files_cannot_tell: total(&fix_files, |f| f.cannot_tell.len()),
        fix_files_never_looked: total(&fix_files, |f| f.never_looked.len()),
        shared_items_read: read_for_items.len() as u32,
        searched_for_every_shared_item: share(&shared, |s| Some(s.searched_for_all())),
        shared_items_changed: total(&shared, |s| s.items.len()),
        shared_items_searched: total(&shared, |s| s.searched().len()),
        median_files_looked_at: median(breadths().map(|b| f64::from(b.files_looked_at)).collect()),
        median_files_edited: median(breadths().map(|b| f64::from(b.files_edited)).collect()),
        median_share_of_looking_before_first_edit: median(
            breadths()
                .filter_map(|b| b.share_before_first_edit)
                .collect(),
        ),
        ran_before_first_change: share(&first_runs, |run| Some(run.ran_before_first_change)),
        first_run_showed_failure: count(&first_runs, FirstRun::showed_failure),
        median_rework_ratio: median(reworks.iter().filter_map(|r| r.ratio).collect()),
        reworked_heavily: share(&reworks, |rework| Some(rework.is_heavy())),
        kept_nothing: count(&reworks, Rework::kept_nothing),
        with_abandoned_files: count(&reworks, |r| !r.abandoned_files.is_empty()),
        edited_existing_files: blind.len() as u32,
        looked_before_every_edit: share(&blind, BlindEdits::looked_at_each_first),
        files_edited_without_looking: total(&blind, |e| e.without_looking.len()),
        files_edited_cannot_tell: total(&blind, |e| e.cannot_tell.len()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::patch::diff_texts;
    use crate::trace::fixtures::{edit, glob, grep, read, read_directory, shell, shell_run, write};

    const PATH_RS: &str = "/testbed/src/uri/path.rs";
    const LIB_RS: &str = "/testbed/src/lib.rs";

    fn trace(calls: Vec<Value>) -> Trace {
        crate::trace::fixtures::trace(calls)
    }

    pub(crate) fn patch(files: &[(&str, Option<&str>, &str)]) -> Vec<FileDiff> {
        let text: String = files
            .iter()
            .map(|(path, old, new)| diff_texts(path, *old, new))
            .collect();
        diff::parse_unified(&text).expect("a diff")
    }

    /// The real fix of these tests: it changes `src/lib.rs` and `src/uri/path.rs`.
    pub(crate) fn the_fix() -> Vec<FileDiff> {
        patch(&[
            ("src/lib.rs", Some("fn a() {}\n"), "fn a() { b() }\n"),
            ("src/uri/path.rs", Some("fn b() {}\n"), "fn b() { c() }\n"),
        ])
    }

    pub(crate) fn lib_only() -> Vec<FileDiff> {
        patch(&[("src/lib.rs", Some("fn a() {}\n"), "fn a() { 1; }\n")])
    }

    /// Measures a session against the real fix of these tests; `model` is the attempt's patch,
    /// graded and handed in.
    pub(crate) fn measured(calls: Vec<Value>, model: &[FileDiff]) -> Method {
        let fix = the_fix();
        let rules = TestPathRules::rust_default();
        let evidence = Evidence {
            fix: Some(&fix),
            graded: Some(model),
            handed_in: Some(model),
            rules: &rules,
            own_commands: &[],
        };
        measure(&trace(calls), &evidence)
    }

    fn places(calls: Vec<Value>) -> FixFiles {
        measured(calls, &lib_only()).fix_files.expect("fix files")
    }

    pub(crate) fn edit_lib() -> Value {
        edit(LIB_RS, "fn a() {}", "fn a() { 1; }")
    }

    #[test]
    fn a_fix_file_is_changed_looked_at_or_shows_no_look() {
        let never = places(vec![read(LIB_RS), edit_lib()]);
        assert_eq!(never.changed, vec!["src/lib.rs"]);
        assert_eq!(never.never_looked, vec!["src/uri/path.rs"]);
        assert_eq!(never.looked_at_all(), Some(false));
        assert_eq!(
            never.sentence(),
            "The real fix changed 2 file(s); it changed 1 of them; its session shows no look at src/uri/path.rs."
        );
        let looked = places(vec![read(LIB_RS), read(PATH_RS), edit_lib()]);
        assert_eq!(looked.looked_not_changed, vec!["src/uri/path.rs"]);
        assert!(looked.never_opened.is_empty());
        assert_eq!(looked.looked_at_all(), Some(true));
        assert_eq!(
            looked.sentence(),
            "The real fix changed 2 file(s); it changed 1 of them, looked at 1 more without changing src/uri/path.rs."
        );
        let undone = places(vec![edit(PATH_RS, "fn b() {}", "fn b() { 2 }")]);
        assert_eq!(undone.looked_not_changed, vec!["src/uri/path.rs"]);
        let both = measured(vec![], &the_fix()).fix_files.expect("files");
        assert_eq!(both.changed.len(), 2);
        assert_eq!(
            both.sentence(),
            "The real fix changed 2 file(s); it changed 2 of them."
        );
    }

    #[test]
    fn a_file_only_listed_by_a_search_counts_as_looked_at_and_is_said_to_be_unopened() {
        let broad_search = shell_run("grep -rn 'fn ' .", 0, "./src/uri/path.rs:1:fn b() {}");
        let listed = places(vec![broad_search]);
        assert_eq!(listed.looked_not_changed, vec!["src/uri/path.rs"]);
        assert_eq!(listed.never_opened, vec!["src/uri/path.rs"]);
        assert_eq!(listed.looked_at_all(), Some(true));
        assert_eq!(listed.total(), 2);
        assert_eq!(
            listed.sentence(),
            "The real fix changed 2 file(s); it changed 1 of them, looked at 1 more without changing src/uri/path.rs (1 of them never opened: only listed by a search or named in a command)."
        );
    }

    #[test]
    fn a_file_a_command_may_have_shown_is_neither_looked_at_nor_never_looked_at() {
        for command in [
            "cat src/uri/*.rs",
            "cd src/uri && cat *.rs",
            "cat $(find . -name path.rs)",
        ] {
            let files = places(vec![shell(command)]);
            assert_eq!(files.cannot_tell, vec!["src/uri/path.rs"], "{command}");
            assert!(files.never_looked.is_empty(), "{command}");
            assert_eq!(files.looked_at_all(), None, "{command}");
        }
        let files = places(vec![shell("cat src/uri/*.rs")]);
        assert_eq!(
            files.sentence(),
            "The real fix changed 2 file(s); it changed 1 of them; whether it saw src/uri/path.rs cannot be told."
        );
        // Commands that name the file only to change or remove it are not a look.
        for command in [
            "rm src/uri/path.rs",
            "git checkout src/uri/path.rs",
            "sed -i 's/a/b/' src/uri/path.rs",
        ] {
            let files = places(vec![shell(command)]);
            assert_eq!(files.never_looked, vec!["src/uri/path.rs"], "{command}");
        }
    }

    #[test]
    fn files_the_fix_creates_and_its_tests_are_not_places_to_look() {
        let fix = patch(&[
            ("src/new.rs", None, "fn new() {}\n"),
            ("tests/it.rs", Some("fn t() {}\n"), "fn t() { a(); }\n"),
            ("Cargo.lock", Some("a\n"), "b\n"),
        ]);
        let steps = trace(vec![]);
        let seen = Seen::of(&steps);
        let rules = TestPathRules::rust_default();
        assert_eq!(fix_files(&seen, &steps, &fix, &lib_only(), &rules), None);
    }

    /// A patch that changes the signatures of `parse` and `Uri::host`, drops the field `port`,
    /// moves `moved` unchanged and rewrites the body of `kept`.
    pub(crate) fn a_patch_that_changes_shared_items() -> Vec<FileDiff> {
        let old = "pub fn parse(s: &str) -> Uri {\n    todo!()\n}\npub struct Uri {\n    pub port: u16,\n    host: String,\n}\nimpl Uri {\n    pub(crate) fn host(&self) -> &str {\n        &self.host\n    }\n}\npub fn moved() {}\npub fn kept() -> u8 {\n    1\n}\nfn private(a: u8) {}\n";
        let new = "pub fn moved() {}\npub fn parse(s: &str, strict: bool) -> Uri {\n    todo!()\n}\npub struct Uri {\n    host: String,\n}\nimpl Uri {\n    pub(crate) fn host(&self) -> Option<&str> {\n        Some(&self.host)\n    }\n}\npub fn kept() -> u8 {\n    2\n}\nfn private(a: u16) {}\n";
        patch(&[
            ("src/uri.rs", Some(old), new),
            (
                "tests/api.rs",
                Some("pub fn helper() {}\n"),
                "pub fn helper2() {}\n",
            ),
        ])
    }

    fn shared(calls: Vec<Value>, model: &[FileDiff]) -> SharedItems {
        measured(calls, model).shared_items.expect("a Rust patch")
    }

    #[test]
    fn shared_items_are_the_pub_declarations_a_patch_changes() {
        let model = a_patch_that_changes_shared_items();
        let searched_one = shared(vec![shell("grep -rn 'parse(' src/")], &model);
        let facts: Vec<(&str, bool)> = searched_one
            .items
            .iter()
            .map(|item| (item.name.as_str(), item.searched))
            .collect();
        assert_eq!(
            facts,
            vec![("parse", true), ("port", false), ("host", false)]
        );
        assert!(
            searched_one
                .items
                .iter()
                .all(|item| item.file == "src/uri.rs")
        );
        assert_eq!(
            searched_one.sentence(),
            "Changed 3 shared item(s) and searched for the name of 1; not of port, host."
        );
        let none = shared(vec![read(LIB_RS)], &model);
        assert_eq!(
            none.sentence(),
            "Changed 3 shared item(s) (parse, port, host); its session shows no search for the name of any."
        );
        let all = shared(
            vec![
                grep("parse\\(", "No files found"),
                shell("cd src && rg -n \"\\.port\" | head"),
                shell("git grep -n host"),
            ],
            &model,
        );
        assert!(all.searched_for_all());
        assert_eq!(
            all.sentence(),
            "Changed 3 shared item(s) (parse, port, host) and searched for the name of each."
        );
        let untouched = shared(vec![], &lib_only());
        assert_eq!(
            untouched.sentence(),
            "Changed no shared item (no `pub` declaration changed in its Rust files)."
        );
    }

    #[test]
    fn a_search_counts_only_for_the_name_as_a_word_beyond_its_own_file() {
        let model = a_patch_that_changes_shared_items();
        for command in [
            "grep -n 'pub fn parse' src/uri.rs",
            "grep -rn error src/parse.rs",
            "rg --files | grep parser",
            "grep -rn 'parser' src/",
            "cargo test 2>&1 | grep -i 'parse'",
            "echo parse",
            "cat src/parse.rs",
            "cargo test parse",
        ] {
            let items = shared(vec![shell(command)], &model);
            assert!(!items.items[0].searched, "{command}");
        }
        let by_name = shared(vec![glob("**/parse*")], &model);
        assert!(!by_name.items[0].searched, "a file-name search");
    }

    #[test]
    fn a_declaration_that_only_moved_or_was_reformatted_is_not_a_changed_item() {
        let model = patch(&[(
            "src/a.rs",
            Some(
                "pub fn body() -> u8 { 1 }\npub fn fmt_it() -> u8 {\n    1\n}\npub const MAX: usize = 4;\n",
            ),
            "pub fn body() -> u8 { 2 }\npub fn fmt_it()  ->  u8 {\n    1\n}\npub const MAX: usize = 8;\n",
        )]);
        let items = shared(vec![], &model);
        let names: Vec<&str> = items.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["MAX"], "a constant's value is part of it");
    }

    #[test]
    fn the_same_name_changed_in_two_files_is_two_items_and_other_languages_are_not_read() {
        let model = patch(&[
            (
                "src/a.rs",
                Some("pub fn new() -> A {}\n"),
                "pub fn new(x: u8) -> A {}\n",
            ),
            (
                "src/b.rs",
                Some("pub fn new() -> B {}\n"),
                "pub fn new(y: u8) -> B {}\n",
            ),
        ]);
        let items = shared(vec![shell("grep -rn 'new(' src/")], &model);
        let facts: Vec<(&str, &str, bool)> = items
            .items
            .iter()
            .map(|item| (item.name.as_str(), item.file.as_str(), item.searched))
            .collect();
        assert_eq!(
            facts,
            vec![("new", "src/a.rs", true), ("new", "src/b.rs", true)]
        );
        let python = patch(&[(
            "app/api.py",
            Some("def a():\n    pass\n"),
            "def a(x):\n    pass\n",
        )]);
        assert_eq!(measured(vec![], &python).shared_items, None);
    }

    #[test]
    fn declarations_are_recognised_by_their_first_line() {
        for (line, name) in [
            ("pub fn parse(s: &str) -> Uri {", "parse"),
            ("    pub(crate) async fn fetch(&self) {", "fetch"),
            ("pub const fn new() -> Self {", "new"),
            ("pub const MAX: usize = 4;", "MAX"),
            ("pub static mut COUNT: u8 = 0;", "COUNT"),
            ("pub unsafe extern \"C\" fn call() {", "call"),
            ("pub struct Uri<'a> {", "Uri"),
            ("pub enum Kind {", "Kind"),
            ("pub trait Parse: Sized {", "Parse"),
            (
                "pub type Result<T> = std::result::Result<T, Error>;",
                "Result",
            ),
            ("    pub host: String,", "host"),
            ("    pub(super) port: u16,", "port"),
        ] {
            assert_eq!(declared_name(line), Some(name), "{line}");
        }
        for line in [
            "fn private() {}",
            "    host: String,",
            "pub use crate::uri::Uri;",
            "pub mod uri;",
            "// pub fn old() {}",
            "    pub(crate) use self::path::Path;",
            "let public = 1;",
        ] {
            assert_eq!(declared_name(line), None, "{line}");
        }
    }

    #[test]
    fn breadth_counts_files_opened_and_when_the_looking_happened() {
        let wide = measured(
            vec![
                read_directory("/testbed/src"),
                read(LIB_RS),
                read(LIB_RS),
                shell("cat src/uri/path.rs Cargo.toml"),
                grep("fn a", "No files found"),
                edit_lib(),
                shell("cargo test 2>&1 | tail -5"),
                read(PATH_RS),
                edit(PATH_RS, "fn b() {}", "fn b() { 2 }"),
            ],
            &lib_only(),
        )
        .breadth;
        assert_eq!(
            wide,
            Breadth {
                files_looked_at: 3,
                files_edited: 2,
                looks_before_first_edit: 5,
                looks_after_first_edit: 1,
                share_before_first_edit: Some(5.0 / 6.0),
            }
        );
        assert_eq!(
            wide.sentence(),
            "Opened 3 file(s) and edited 2; 83% of its looking came before its first edit."
        );
        let no_edit = measured(
            vec![read(LIB_RS), shell("sed -i s/1/2/ src/lib.rs")],
            &lib_only(),
        )
        .breadth;
        assert_eq!(no_edit.looks_before_first_edit, 1, "sed -i is not looking");
        assert_eq!(no_edit.share_before_first_edit, None);
        assert_eq!(no_edit.sentence(), "Opened 1 file(s) and edited 0.");
    }

    fn first_run_of(calls: Vec<Value>) -> Option<FirstRun> {
        measured(calls, &lib_only()).first_run
    }

    /// The run before the first change, when the session is one command and then the fix.
    fn run_then_fix(command: &str, exit: i64, output: &str) -> FirstRun {
        first_run_of(vec![shell_run(command, exit, output), edit_lib()]).expect("a change")
    }

    #[test]
    fn a_run_before_the_first_change_to_existing_code_is_found_with_how_it_ended() {
        let failing = first_run_of(vec![
            read(LIB_RS),
            shell_run(
                "cargo test 2>&1 | tail -3",
                0,
                "test result: FAILED. 0 passed; 1 failed",
            ),
            edit_lib(),
        ])
        .expect("a change");
        assert!(failing.ran_before_first_change);
        assert_eq!(failing.outcome, Some(RunEnd::ShowedFailure));
        assert_eq!(
            failing.command.as_deref(),
            Some("cargo test 2>&1 | tail -3")
        );
        assert_eq!(
            failing.sentence(),
            "Ran the build or tests before changing existing code, and that run showed a failure."
        );
        let by_exit = run_then_fix("cargo build", 101, "");
        assert!(by_exit.showed_failure());
        let passing = run_then_fix("cargo test", 0, "test result: ok.");
        assert_eq!(passing.outcome, Some(RunEnd::NoFailureSeen));
        assert!(passing.sentence().ends_with("that run showed no failure."));
        let too_late = first_run_of(vec![edit_lib(), shell("cargo test")]).expect("a change");
        assert!(!too_late.ran_before_first_change);
        assert_eq!((too_late.outcome, &too_late.command), (None, &None));
        assert_eq!(
            too_late.sentence(),
            "Changed existing code without running the build or tests first."
        );
    }

    #[test]
    fn an_exit_status_that_is_not_the_runs_own_is_not_read_as_its_result() {
        // grep found nothing: its exit status 1 says nothing about the tests.
        let filtered = run_then_fix("cargo test 2>&1 | grep FAILED", 1, "");
        assert!(filtered.ran_before_first_change);
        assert_eq!(filtered.outcome, Some(RunEnd::NotShown));
        assert_eq!(
            filtered.sentence(),
            "Ran the build or tests before changing existing code; the session does not show how that run ended."
        );
        let not_found = run_then_fix("pytest -x", 127, "bash: pytest: command not found");
        assert!(!not_found.ran_before_first_change);
        assert_eq!((not_found.outcome, not_found.did_not_start), (None, 1));
        assert_eq!(
            not_found.sentence(),
            "Tried to run the build or tests before changing existing code, but the command did not start."
        );
        let timed_out = run_then_fix("timeout 5 cargo test", 124, "");
        assert_eq!(timed_out.outcome, Some(RunEnd::CutOff));
        assert!(
            timed_out
                .sentence()
                .ends_with("that run was cut off before it finished.")
        );
        let expected_panic = run_then_fix(
            "cargo test -- --nocapture",
            0,
            "thread 't' panicked at src/lib.rs:1:1\ntest result: ok. 1 passed; 0 failed",
        );
        assert_eq!(expected_panic.outcome, Some(RunEnd::NoFailureSeen));
        // One run that showed a failure is the one reported, whatever else ran.
        let several = first_run_of(vec![
            shell_run("cargo build", 0, ""),
            shell_run("cargo test", 101, ""),
            edit_lib(),
        ])
        .expect("a change");
        assert!(several.showed_failure());
        assert_eq!(several.command.as_deref(), Some("cargo build"));
    }

    #[test]
    fn writing_a_reproduction_first_is_not_the_first_change() {
        let reproduced = [
            vec![write(
                "/testbed/tests/repro.rs",
                "#[test]\nfn repro() {}\n",
                false,
            )],
            vec![edit("/testbed/tests/it.rs", "fn t() {}", "fn t() { a(); }")],
            vec![edit(
                LIB_RS,
                "mod tests {\n}",
                "mod tests {\n    #[test]\n    fn repro() {}\n}",
            )],
            vec![write("/testbed/repro.py", "print(1)\n", false)],
            // A scratch file of its own, adjusted before the run.
            vec![
                write("/testbed/repro.rs", "fn main() {}\n", false),
                edit("/testbed/repro.rs", "fn main() {}", "fn main() { a() }"),
            ],
            // A scratch file outside the repository, written twice.
            vec![
                write("/tmp/repro.sh", "cargo test\n", false),
                write("/tmp/repro.sh", "cargo test -q\n", true),
            ],
        ];
        for calls in reproduced {
            let mut session = calls.clone();
            session.push(shell_run("cargo test repro", 101, "FAILED"));
            session.push(edit_lib());
            session.push(shell("rm -f repro.rs"));
            let run = first_run_of(session).expect("a change");
            assert!(run.showed_failure(), "{calls:?}");
        }
        assert_eq!(first_run_of(vec![shell("cargo test")]), None);
        let only_shell = first_run_of(vec![shell("sed -i s/1/2/ src/lib.rs")]);
        assert_eq!(
            only_shell, None,
            "no edit-tool change to place a run before"
        );
    }

    #[test]
    fn rewriting_an_existing_file_is_a_change_whatever_tests_it_holds() {
        let with_tests =
            "fn a() { 1; }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        let rewrite = write(LIB_RS, with_tests, true);
        let alone = first_run_of(vec![rewrite.clone()]).expect("a change to existing code");
        assert!(!alone.ran_before_first_change);
        let then_tested = first_run_of(vec![rewrite, shell("cargo test"), edit_lib()]);
        assert!(!then_tested.expect("a change").ran_before_first_change);
        // An edit that adds a function next to a test is a change too.
        let mixed = edit(
            LIB_RS,
            "fn a() {}",
            "fn a() {}\nfn helper() {}\n#[test]\nfn t() {}",
        );
        let run = first_run_of(vec![mixed, shell("cargo test")]).expect("a change");
        assert!(!run.ran_before_first_change);
    }

    #[test]
    fn a_file_whose_origin_nothing_shows_is_not_taken_as_existing_code() {
        // A write the session does not mark as new or old, to a file that is in no patch.
        let mut unmarked = write("/testbed/scratch.rs", "fn s() {}\n", true);
        unmarked["state"]["metadata"] = json!({});
        let method = measured(
            vec![
                unmarked.clone(),
                shell_run("cargo test", 101, ""),
                edit_lib(),
            ],
            &lib_only(),
        );
        assert!(method.first_run.expect("a change").showed_failure());
        let blind = method.blind_edits.expect("edits");
        assert_eq!(blind.existing_files_edited, 1, "only src/lib.rs");
        // The same write is a rewrite of existing code once the agent has opened the file.
        let known = measured(
            vec![read("/testbed/scratch.rs"), unmarked, edit_lib()],
            &lib_only(),
        );
        assert_eq!(known.blind_edits.expect("edits").existing_files_edited, 2);
        assert!(!known.first_run.expect("a change").ran_before_first_change);
    }

    #[test]
    fn only_rust_tests_at_the_top_of_an_edit_make_it_a_test_edit() {
        let lines = |text: &str| -> Vec<String> { text.lines().map(str::to_string).collect() };
        for added in [
            "#[test]\nfn repro() {}",
            "    #[tokio::test]\n    async fn repro() {\n        assert!(a());\n    }",
            "#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn t() {}\n}",
            "    }\n\n    #[test]\n    #[should_panic]\n    fn repro() {\n        a();",
            "/// Reproduces the bug.\n#[rstest]\nfn repro() {}",
        ] {
            assert!(adds_only_tests(&lines(added)), "{added}");
        }
        for added in [
            "fn helper() {}",
            "#[test]\nfn t() {}\nfn helper() {}",
            "#[derive(Debug)]\nstruct S;",
            "let x = 1;",
            "",
        ] {
            assert!(!adds_only_tests(&lines(added)), "{added}");
        }
    }

    fn rework_of(calls: Vec<Value>, graded: &[FileDiff], handed_in: Option<&[FileDiff]>) -> Rework {
        let rules = TestPathRules::rust_default();
        let evidence = Evidence {
            fix: None,
            graded: Some(graded),
            handed_in,
            rules: &rules,
            own_commands: &[],
        };
        measure(&trace(calls), &evidence).rework.expect("edits")
    }

    #[test]
    fn rework_sets_lines_written_against_lines_kept() {
        let graded = patch(&[("src/lib.rs", Some("fn a() {}\n"), "fn a() {\n    1;\n}\n")]);
        let handed_in = patch(&[
            ("src/lib.rs", Some("fn a() {}\n"), "fn a() {\n    1;\n}\n"),
            ("tests/mine.rs", None, "#[test]\nfn mine() {}\n"),
        ]);
        let steps = vec![
            edit(LIB_RS, "fn a() {}", "fn a() {\n    0;\n    0;\n}"),
            edit(LIB_RS, "    0;\n    0;", "    1;"),
            write("/testbed/src/scratch.rs", "fn s() {}\nfn t() {}\n", false),
            write("/testbed/tests/mine.rs", "#[test]\nfn mine() {}\n", false),
            write("/tmp/notes.txt", "a\nb\nc\n", false),
            write("/testbed/Cargo.lock", "x\n", true),
        ];
        let redone = rework_of(steps.clone(), &graded, Some(&handed_in));
        // 4 + 1 lines in src/lib.rs and 2 in the scratch file; the test, the note outside the
        // repository and the lock file are not counted.
        assert_eq!((redone.lines_written, redone.lines_kept), (7, 3));
        assert_eq!(redone.ratio, Some(7.0 / 3.0));
        assert!(redone.is_heavy() && !redone.kept_nothing());
        assert!(!redone.kept_read_from_graded_patch);
        assert_eq!(redone.abandoned_files, vec!["src/scratch.rs"]);
        assert_eq!(redone.whole_file_rewrites, 0);
        assert_eq!(
            redone.sentence(),
            "Wrote 7 line(s) to end with a 3-line change; edited src/scratch.rs that is not in its final patch."
        );
        let without_raw = rework_of(steps, &graded, None);
        assert_eq!(without_raw.abandoned_files, vec!["src/scratch.rs"]);
        assert!(without_raw.kept_read_from_graded_patch);
    }

    #[test]
    fn a_test_written_inside_a_source_file_is_not_rework() {
        // Grading undoes the test module; the patch as handed in still holds it.
        let fixed = "fn a() { 1; }\n";
        let with_test = "fn a() { 1; }\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn a_is_one() {\n        a();\n    }\n}\n";
        let graded = patch(&[("src/lib.rs", Some("fn a() {}\n"), fixed)]);
        let handed_in = patch(&[("src/lib.rs", Some("fn a() {}\n"), with_test)]);
        let steps = vec![edit_lib(), edit(LIB_RS, "fn a() { 1; }\n", with_test)];
        let once = rework_of(steps.clone(), &graded, Some(&handed_in));
        assert_eq!((once.lines_written, once.lines_kept), (9, 9));
        assert_eq!(once.ratio, Some(1.0));
        assert!(!once.is_heavy());
        // Without the handed-in patch the graded one stands in, and the record says so.
        let fallback = rework_of(steps, &graded, None);
        assert_eq!((fallback.lines_written, fallback.lines_kept), (9, 1));
        assert!(fallback.kept_read_from_graded_patch);
    }

    #[test]
    fn writing_lines_and_keeping_none_is_the_heaviest_rework_not_a_gap() {
        let many: String = (0..300).map(|n| format!("    line{n}();\n")).collect();
        let steps = vec![
            edit(LIB_RS, "fn a() {}", &format!("fn a() {{\n{many}}}")),
            edit(LIB_RS, &format!("fn a() {{\n{many}}}"), "fn a() {}"),
        ];
        let nothing: Vec<FileDiff> = Vec::new();
        let reverted = rework_of(steps, &nothing, Some(&nothing));
        assert_eq!((reverted.lines_written, reverted.lines_kept), (303, 0));
        assert_eq!(reverted.ratio, None);
        assert!(reverted.kept_nothing() && reverted.is_heavy());
        assert_eq!(
            reverted.sentence(),
            "Wrote 303 line(s) and kept none of them; edited src/lib.rs that is not in its final patch."
        );
        let method = Method {
            rework: Some(reverted),
            ..measured(vec![], &lib_only())
        };
        let group = summarise(&[(false, &method)]).not_resolved;
        assert_eq!(group.reworked_heavily, rate_of(1, 1));
        assert_eq!((group.kept_nothing, group.with_abandoned_files), (1, 1));
        assert_eq!(group.median_rework_ratio, None);
    }

    #[test]
    fn a_change_written_once_has_ratio_one_and_a_whole_file_rewrite_is_said() {
        let graded = lib_only();
        let once = rework_of(vec![edit_lib()], &graded, Some(&graded));
        assert_eq!(once.ratio, Some(1.0));
        assert!(!once.is_heavy());
        assert_eq!(
            once.sentence(),
            "Wrote 1 line(s) to end with a 1-line change."
        );
        let rewrite = vec![write(LIB_RS, "fn a() { 1; }\nfn z() {}\n", true)];
        let rewritten = rework_of(rewrite, &graded, Some(&graded));
        assert_eq!(
            (rewritten.lines_written, rewritten.whole_file_rewrites),
            (2, 1)
        );
        assert!(
            rewritten
                .sentence()
                .contains("1 whole-file rewrite(s) counted in full")
        );
        let by_shell = measured(vec![shell("sed -i s/1/2/ src/lib.rs")], &graded);
        assert_eq!(by_shell.rework, None);
        let outside = measured(vec![write("/tmp/notes.md", "a\n", false)], &graded);
        assert_eq!(outside.rework, None, "no edit inside the repository");
        let deletion = patch(&[("src/lib.rs", Some("fn a() {}\nfn b() {}\n"), "fn a() {}\n")]);
        let removal = vec![edit(LIB_RS, "fn a() {}\nfn b() {}", "fn a() {}")];
        let nothing_written = rework_of(removal, &deletion, Some(&deletion));
        assert_eq!(nothing_written.ratio, None);
        assert!(!nothing_written.is_heavy(), "it wrote nothing to keep");
    }

    #[test]
    fn an_edit_is_blind_when_nothing_showed_the_file_first() {
        let model = patch(&[
            ("src/lib.rs", Some("fn a() {}\n"), "fn a() { 1; }\n"),
            ("src/uri/path.rs", Some("fn b() {}\n"), "fn b() { 3 }\n"),
            ("src/made.rs", None, "fn made() {}\n"),
        ]);
        let blind = measured(
            vec![
                read(LIB_RS),
                edit_lib(),
                edit(PATH_RS, "fn b() {}", "fn b() { 2 }"),
                read(PATH_RS),
                edit(PATH_RS, "fn b() { 2 }", "fn b() { 3 }"),
                write("/testbed/src/made.rs", "fn made() {}\n", true),
                write("/testbed/src/new.rs", "fn new() {}\n", false),
            ],
            &model,
        )
        .blind_edits
        .expect("edits");
        assert_eq!(blind.existing_files_edited, 2);
        assert_eq!(blind.without_looking, vec!["src/uri/path.rs"]);
        assert_eq!(blind.looked_at_each_first(), Some(false));
        assert_eq!(
            blind.sentence(),
            "Edited 2 existing file(s), 1 of them with no look shown first (src/uri/path.rs)."
        );
        let own_files = vec![write("/testbed/src/new.rs", "fn new() {}\n", false)];
        let own_files = measured(own_files, &model).blind_edits.expect("edits");
        assert_eq!(
            own_files.sentence(),
            "Edited no file known to have been there before its session."
        );
        assert_eq!(measured(vec![read(LIB_RS)], &model).blind_edits, None);
    }

    #[test]
    fn only_a_real_view_counts_as_looking_before_an_edit() {
        let edit_path = || edit(PATH_RS, "fn b() {}", "fn b() { 2 }");
        let before_edit = |call: Value| {
            measured(vec![call, edit_path()], &the_fix())
                .blind_edits
                .expect("edits")
        };
        let listed = before_edit(grep(
            "fn b",
            "Found 1 matches\n/testbed/src/uri/path.rs:\n  Line 1: fn b() {}",
        ));
        assert_eq!(listed.looked_at_each_first(), Some(true));
        assert_eq!(
            listed.sentence(),
            "Looked at each of the 1 existing file(s) it edited first."
        );
        let restored = before_edit(shell("git checkout src/uri/path.rs"));
        assert_eq!(restored.without_looking, vec!["src/uri/path.rs"]);
        for command in ["python analyse.py src/uri/path.rs", "cat src/uri/*.rs"] {
            let unclear = before_edit(shell(command));
            assert_eq!(unclear.cannot_tell, vec!["src/uri/path.rs"], "{command}");
            assert!(unclear.without_looking.is_empty(), "{command}");
            assert_eq!(unclear.looked_at_each_first(), None, "{command}");
            assert_eq!(
                unclear.sentence(),
                "Edited 1 existing file(s); whether it looked at src/uri/path.rs first cannot be told."
            );
        }
    }

    /// Looked at both files of the fix, ran the tests first and last, wrote its change once.
    pub(crate) fn a_careful_attempt() -> Method {
        measured(
            vec![
                read(LIB_RS),
                read(PATH_RS),
                shell_run("cargo test", 101, "test result: FAILED"),
                edit_lib(),
                shell("cargo test"),
            ],
            &lib_only(),
        )
    }

    /// Edited one file blind, twice, and never looked at the other file of the fix.
    pub(crate) fn a_hasty_attempt() -> Method {
        measured(
            vec![
                edit(LIB_RS, "fn a() {}", "fn a() {\n    0;\n    0;\n}"),
                edit(LIB_RS, "fn a() {\n    0;\n    0;\n}", "fn a() { 1; }"),
            ],
            &lib_only(),
        )
    }

    #[test]
    fn every_measure_has_its_sentence_in_the_reports_order() {
        let check = SelfCheck {
            edits: 1,
            checks: 2,
            checked_after_last_edit: true,
            ..SelfCheck::default()
        };
        assert_eq!(
            a_careful_attempt().sentences(Some(&check)),
            vec![
                "The real fix changed 2 file(s); it changed 1 of them, looked at 1 more without changing src/uri/path.rs.",
                "Changed no shared item (no `pub` declaration changed in its Rust files).",
                "Opened 2 file(s) and edited 1; 100% of its looking came before its first edit.",
                "Ran the build or tests before changing existing code, and that run showed a failure.",
                "Wrote 1 line(s) to end with a 1-line change.",
                "Ran the build or tests after its last edit.",
                "Looked at each of the 1 existing file(s) it edited first.",
            ]
        );
    }

    #[test]
    fn missing_evidence_leaves_a_measure_out_instead_of_guessing() {
        let rules = TestPathRules::rust_default();
        let nothing = Evidence {
            fix: None,
            graded: None,
            handed_in: None,
            rules: &rules,
            own_commands: &[],
        };
        let method = measure(&trace(vec![shell("sed -i s/1/2/ src/lib.rs")]), &nothing);
        assert_eq!(method.fix_files, None);
        assert_eq!(method.shared_items, None);
        assert_eq!(method.first_run, None);
        assert_eq!(method.rework, None);
        assert_eq!(method.blind_edits, None);
        assert_eq!(
            method.sentences(None),
            vec!["Opened 0 file(s) and edited 0."]
        );
    }

    #[test]
    fn the_summary_splits_by_outcome_and_counts_only_what_was_read() {
        let (careful, hasty) = (a_careful_attempt(), a_hasty_attempt());
        let summary = summarise(&[(true, &careful), (false, &hasty), (false, &hasty)]);
        assert_eq!(summary.all.sessions, 3);
        assert_eq!(summary.resolved.looked_at_every_fix_file, rate_of(1, 1));
        let failed = &summary.not_resolved;
        assert_eq!(failed.looked_at_every_fix_file, rate_of(0, 2));
        assert_eq!(
            (
                failed.fix_files_changed,
                failed.fix_files_looked_not_changed,
                failed.fix_files_never_opened,
                failed.fix_files_cannot_tell,
                failed.fix_files_never_looked
            ),
            (2, 0, 0, 0, 2)
        );
        assert_eq!(failed.ran_before_first_change, rate_of(0, 2));
        assert_eq!(summary.resolved.ran_before_first_change, rate_of(1, 1));
        assert_eq!(summary.resolved.first_run_showed_failure, 1);
        assert_eq!(failed.median_rework_ratio, Some(5.0));
        assert_eq!(failed.reworked_heavily, rate_of(2, 2));
        assert_eq!(failed.kept_nothing, 0);
        assert_eq!(summary.resolved.median_rework_ratio, Some(1.0));
        assert_eq!(failed.looked_before_every_edit, rate_of(0, 2));
        assert_eq!(failed.files_edited_without_looking, 2);
        assert_eq!(summary.all.looked_before_every_edit, rate_of(1, 3));
        // Every patch here is Rust, and none changed a shared item: nothing to take a share of.
        assert_eq!(summary.all.shared_items_read, 3);
        assert_eq!(summary.all.searched_for_every_shared_item.n, 0);
        assert_eq!(summary.all.median_files_looked_at, Some(0.0));
        assert_eq!(
            summary.resolved.median_share_of_looking_before_first_edit,
            Some(1.0)
        );
        assert_eq!(failed.median_share_of_looking_before_first_edit, None);
    }

    #[test]
    fn an_attempt_that_cannot_be_told_is_left_out_of_the_share_not_counted_against() {
        let globbed = shell("cat src/*.rs src/uri/*.rs");
        let unclear = measured(vec![globbed, edit_lib()], &lib_only());
        let careful = a_careful_attempt();
        let group = summarise(&[(true, &unclear), (true, &careful)]).resolved;
        assert_eq!(group.compared_with_fix, 2);
        assert_eq!(group.looked_at_every_fix_file, rate_of(1, 1));
        assert_eq!(group.fix_files_cannot_tell, 1);
        // The glob may have shown src/lib.rs too, before it was edited.
        assert_eq!(group.edited_existing_files, 2);
        assert_eq!(group.looked_before_every_edit, rate_of(1, 1));
        assert_eq!(group.files_edited_cannot_tell, 1);
    }

    #[test]
    fn long_lists_of_names_are_cut_short() {
        let names: Vec<String> = ["a", "b", "c", "d", "e"].map(String::from).to_vec();
        assert_eq!(named(&names), "a, b, c and 2 more");
        assert_eq!(named(&names[..2]), "a, b");
    }
}
