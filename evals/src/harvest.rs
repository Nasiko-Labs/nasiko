//! Harvest: one finished session plus the commits made during it → one candidate task.
//!
//! The steps are pure so the CLI (local git) and the server (the GitHub API) share them; each
//! caller fetches what a step asks for and passes it in:
//!
//! 1. [`precheck`]: does the session have typed messages and edits in the repo at all?
//! 2. [`link_tasks`]: given the commits in the session's window, which one (or which unbroken
//!    run) is its work, or, for a long session, which commits are each a task of their own, and
//!    which slice of the session produced each. Steps 3 to 5 then run once per task.
//! 3. [`split`] the linked commits' diff into fix and tests, then
//!    [`split_inline_rust_tests`] with the base and gold text of [`inline_test_paths`].
//! 4. [`has_tests`]: a commit that changes no test cannot be a task.
//! 5. [`draft_task`].
//!
//! A repository with no sessions is harvested from its own history instead, one commit at a
//! time. There is nothing to link: [`change_base`] stands in for steps 1 and 2,
//! [`commit_precheck`] is an early exit between the two halves of step 3, [`classify_commit`]
//! replaces step 4 (it also drops a commit whose message is too thin to state a problem), and
//! [`linked_from_commit`] gives step 5 its input.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::diff::{self, DiffError, FileDiff, SplitPatch, TestPathRules};
use crate::instance::{
    self, NasikoExtras, OriginalRun, SessionSource, TaskInstance, TaskStatus, TestSource,
};
use crate::link::{self, CommitFacts, LinkDecision, LinkScore};
use crate::session::{SessionRecord, UserMessage};
use crate::{patch, remote, rust_tests};

/// What happened to one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Task,
    NoTypedMessage,
    NoEditsInRepo,
    NoMatch,
    Ambiguous,
    Unreadable,
    /// The linked commits change no test, so nothing could prove the task was done.
    NoTests,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Outcome::Task => "task",
            Outcome::NoTypedMessage => "no_typed_message",
            Outcome::NoEditsInRepo => "no_edits_in_repo",
            Outcome::NoMatch => "no_match",
            Outcome::Ambiguous => "ambiguous",
            Outcome::Unreadable => "unreadable",
            Outcome::NoTests => "no_tests",
        }
    }
}

/// The repo-relative paths the session edited, or why it cannot be a task.
pub fn precheck(session: &SessionRecord, roots: &[String]) -> Result<Vec<String>, Outcome> {
    if session.user_messages.is_empty() {
        return Err(Outcome::NoTypedMessage);
    }
    let edited = session.edited_paths_in(roots);
    if edited.is_empty() {
        return Err(Outcome::NoEditsInRepo);
    }
    Ok(edited)
}

/// Whether a commit listed in the window is worth fetching in full: it touches a file the
/// session edited.
pub fn touches_edits(commit_files: &[String], edited: &[String]) -> bool {
    commit_files.iter().any(|f| edited.contains(f))
}

/// A session linked to its work.
#[derive(Debug, Clone)]
pub struct Linked {
    pub decision: LinkDecision,
    pub candidates: Vec<LinkScore>,
    /// The linked commits, oldest first (one, or an unbroken run).
    pub commits: Vec<CommitFacts>,
    /// The part of the session that produced the work.
    pub slice: SessionRecord,
    pub link_score: f64,
    /// One of several tasks the session holds ([`link_tasks`]); its id then names its commit.
    pub one_of_several: bool,
}

impl Linked {
    pub fn first(&self) -> &CommitFacts {
        &self.commits[0]
    }

    pub fn last(&self) -> &CommitFacts {
        &self.commits[self.commits.len() - 1]
    }
}

/// Picks the session's work among `candidates` (the commits in its window that touch its edits,
/// fetched in full): one task (a commit, or a short unbroken run of them), or, for a long
/// session whose several matching commits are not one short run, one task per commit
/// ([`link::separate_tasks`]), each with the share of the session that led to it.
pub fn link_tasks(
    session: &SessionRecord,
    roots: &[String],
    candidates: Vec<CommitFacts>,
) -> Result<Vec<Linked>, Outcome> {
    let mut commits: Vec<CommitFacts> = candidates
        .into_iter()
        .filter(|c| link::in_window(session, c) && link::is_eligible(c))
        .collect();
    commits.sort_by_key(|c| c.committed_at);
    let scores: Vec<LinkScore> = commits
        .iter()
        .map(|c| link::score(session, roots, c))
        .collect();
    let Some(separate) = link::separate_tasks(&scores) else {
        return one_task(session, roots, &commits, scores).map(|linked| vec![linked]);
    };
    let mut tasks = Vec::new();
    let mut previous = None;
    for i in separate {
        let commit = &commits[i];
        // The message that led to this commit's first edit since the previous linked commit;
        // a commit whose files were last edited before it starts where the session first
        // edited them.
        let start = session
            .task_start_after(roots, &commit.files, previous)
            .or_else(|| session.task_start(roots, &commit.files));
        tasks.push(Linked {
            slice: session.slice(start, commit.committed_at),
            link_score: scores[i].total,
            decision: LinkDecision::Single {
                link: scores[i].clone(),
            },
            candidates: scores.clone(),
            commits: vec![commit.clone()],
            one_of_several: true,
        });
        previous = Some(commit.committed_at);
    }
    Ok(tasks)
}

/// The session's one task among `commits` (oldest first, scored as `scores`).
fn one_task(
    session: &SessionRecord,
    roots: &[String],
    commits: &[CommitFacts],
    scores: Vec<LinkScore>,
) -> Result<Linked, Outcome> {
    let decision = link::decide(&scores);
    let (first, last) = match &decision {
        LinkDecision::Single { link } => (link.clone(), link.clone()),
        LinkDecision::Range { first, last } => (first.clone(), last.clone()),
        LinkDecision::Ambiguous { .. } => return Err(Outcome::Ambiguous),
        LinkDecision::NoMatch => return Err(Outcome::NoMatch),
    };
    let at = |sha: &str| {
        commits
            .iter()
            .find(|c| c.sha == sha)
            .map(|c| c.committed_at)
    };
    let (Some(from), Some(until)) = (at(&first.sha), at(&last.sha)) else {
        return Err(Outcome::NoMatch);
    };
    let mut linked: Vec<CommitFacts> = commits
        .iter()
        .filter(|c| c.committed_at >= from && c.committed_at <= until)
        .cloned()
        .collect();
    linked.sort_by_key(|c| c.committed_at);
    // A long session often holds several tasks: keep only the part from the message that led
    // to the first edit of the commits' files up to the last commit.
    let files: Vec<String> = linked.iter().flat_map(|c| c.files.clone()).collect();
    let start = session.task_start(roots, &files);
    Ok(Linked {
        slice: session.slice(start, until),
        link_score: first.total.min(last.total),
        decision,
        candidates: scores,
        commits: linked,
        one_of_several: false,
    })
}

/// The linked commits' full diff (`first.parent..last`), split into fix and test files.
pub fn split(full_diff: &str) -> Result<SplitPatch, DiffError> {
    Ok(diff::split_tests(
        diff::parse_unified(full_diff)?,
        &TestPathRules::rust_default(),
    ))
}

/// Gold Rust files whose base and gold text [`split_inline_rust_tests`] needs.
pub fn inline_test_paths(split: &SplitPatch) -> Vec<String> {
    split
        .gold
        .iter()
        .filter(|f| f.path().ends_with(".rs") && f.new_path.is_some())
        .map(|f| f.path().to_string())
        .collect()
}

/// One file at the base commit (`None` = the fix creates it) and at the gold commit.
#[derive(Debug, Clone, Default)]
pub struct FileVersions {
    pub base: Option<String>,
    pub gold: String,
}

/// Moves each changed Rust file's inline `#[cfg(test)]` module change into the test patch.
/// `versions` holds every path of [`inline_test_paths`]. Files that cannot be split (several
/// test modules) stay in `mixed`.
pub fn split_inline_rust_tests(
    split: &mut SplitPatch,
    versions: &BTreeMap<String, FileVersions>,
) -> Result<(), DiffError> {
    let mut gold_files = Vec::new();
    let mut unsplit = Vec::new();
    for file in std::mem::take(&mut split.gold) {
        let path = file.path().to_string();
        if !path.ends_with(".rs") || file.new_path.is_none() {
            gold_files.push(file);
            continue;
        }
        // A file the fix creates keeps its tests: hidden tests in it could only ever apply to a
        // fix that creates the very same file, which would punish a correct fix laid out
        // differently. Hidden tests come from files that already existed, as in SWE-bench.
        let Some(FileVersions {
            base: Some(old),
            gold: new,
        }) = versions.get(&path)
        else {
            gold_files.push(file);
            continue;
        };
        let Some(middle) = rust_tests::code_with_old_tests(old, new) else {
            if split.mixed.contains(&path) {
                unsplit.push(path);
            }
            gold_files.push(file);
            continue;
        };
        if &middle == new {
            gold_files.push(file);
            continue;
        }
        gold_files.extend(diff::parse_unified(&patch::diff_texts(
            &path,
            Some(old),
            &middle,
        ))?);
        split.test.extend(diff::parse_unified(&patch::diff_texts(
            &path,
            Some(&middle),
            new,
        ))?);
    }
    split.gold = gold_files;
    split.mixed = unsplit;
    Ok(())
}

/// Whether the test patch changes a test, not just test helpers: a commit whose only test
/// changes are in shared helpers (`tests/common/mod.rs`) or that has none cannot yield a test
/// that fails before the fix and passes after it. A test file counts when it is an integration
/// test target (`tests/<name>.rs`, `tests/<name>/main.rs`), lies outside a `tests/` folder (an
/// inline test module moved here, a `*_test.rs`), or adds a `#[…test…]` attribute.
pub fn has_tests(split: &SplitPatch) -> bool {
    split.test.iter().any(changes_a_test)
}

/// One file of a test patch that is a test, not a shared helper ([`has_tests`]).
fn changes_a_test(file: &FileDiff) -> bool {
    !in_tests_folder(file.path()) || is_test_target(file.path()) || adds_test_attr(file)
}

fn in_tests_folder(path: &str) -> bool {
    path.starts_with("tests/") || path.contains("/tests/")
}

/// `…/tests/<name>.rs` or `…/tests/<name>/main.rs`.
fn is_test_target(path: &str) -> bool {
    let rest = match path.rsplit_once("tests/") {
        Some((before, rest)) if before.is_empty() || before.ends_with('/') => rest,
        _ => return false,
    };
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        [file] => file.ends_with(".rs"),
        [_, "main.rs"] => true,
        _ => false,
    }
}

/// Adds an attribute whose last path segment names a test: `#[test]`, `#[tokio::test]`,
/// `#[sqlx::test(...)]`, `#[rstest]`, `#[test_case(...)]`.
fn adds_test_attr(file: &FileDiff) -> bool {
    file.added.iter().any(|line| {
        let Some(attr) = line.trim_start().strip_prefix("#[") else {
            return false;
        };
        let path = attr.split(['(', ']']).next().unwrap_or_default().trim();
        let last = path.rsplit("::").next().unwrap_or_default();
        matches!(last, "test" | "rstest" | "test_case")
    })
}

/// Until prompt P1 rewrites it: the first typed message is the statement, later ones are hints.
pub fn statement_and_hints(session: &SessionRecord) -> (String, String) {
    let mut messages = session.user_messages.iter().map(|m| m.text.as_str());
    let statement = messages.next().unwrap_or_default().to_string();
    let hints = messages.collect::<Vec<_>>().join("\n\n---\n\n");
    (statement, hints)
}

/// The candidate task. `repo_full_name` is `owner/name`; `slice` is [`Linked::slice`].
pub fn draft_task(repo_full_name: &str, linked: &Linked, split: SplitPatch) -> TaskInstance {
    let (first, last, session) = (linked.first(), linked.last(), &linked.slice);
    let base = first.parent.clone();
    let slug = repo_full_name.rsplit('/').next().unwrap_or(repo_full_name);
    let (statement, hints) = statement_and_hints(session);
    TaskInstance {
        instance_id: if linked.one_of_several {
            instance::commit_instance_id(slug, &session.session_id, &last.sha)
        } else {
            instance::instance_id(slug, &session.session_id)
        },
        repo: repo_full_name.to_string(),
        base_commit: base.clone(),
        environment_setup_commit: base,
        version: last.committed_at.format("%Y.%m").to_string(),
        created_at: last.committed_at,
        problem_statement: statement,
        hints_text: hints,
        patch: diff::join(&split.gold),
        test_patch: diff::join(&split.test),
        fail_to_pass: Vec::new(),
        pass_to_pass: Vec::new(),
        image: String::new(),
        eval_script: String::new(),
        log_parser: String::new(),
        eval_type: String::new(),
        difficulty: None,
        x_nasiko: NasikoExtras {
            status: TaskStatus::Candidate,
            source: session.source,
            source_session_id: session.session_id.clone(),
            gold_commit: last.sha.clone(),
            link_score: linked.link_score,
            test_source: if split.test.is_empty() {
                TestSource::None
            } else {
                TestSource::Commit
            },
            orig: OriginalRun {
                model: session.main_model().unwrap_or_default(),
                usage: session.total_usage(),
                user_turns: session.user_messages.len() as u32,
            },
            mixed_files: split.mixed,
            reject_reason: None,
            statement_rewritten: false,
            test_scope: Vec::new(),
        },
    }
}

/// One commit of a repository's history, as `git log` lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryCommit {
    pub sha: String,
    /// Every parent: none for a root commit, several for a merge.
    pub parents: Vec<String>,
    pub committed_at: DateTime<Utc>,
    /// Subject and body, as written.
    pub message: String,
}

impl HistoryCommit {
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or_default().trim()
    }
}

/// What happened to one commit of a repository's history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitOutcome {
    Task,
    /// A merge (its diff is not one change: several parents, or one parent under the subject
    /// git writes for a branch merge) or a root commit (no code to start from).
    MergeOrRoot,
    /// More than [`link::MAX_FILES`] files: too big to be one task.
    TooLarge,
    /// Tests aside, the commit changes no Rust source file (only tests, documentation, data or
    /// nothing at all): there is no fix to ask a model for.
    NoCodeChange,
    /// The commit adds nothing to a test (it has none, changes only shared helpers, or only
    /// removes tests), so nothing could prove the task was done.
    NoTests,
    /// A fix with tests whose message says too little to state a problem
    /// ([`is_thin_message`]).
    ThinMessage,
    /// Its task id, which carries only the first sha characters, is another commit's already.
    IdTaken,
    /// git or the diff could not be read.
    Unreadable,
}

impl CommitOutcome {
    pub fn label(self) -> &'static str {
        match self {
            CommitOutcome::Task => "task",
            CommitOutcome::MergeOrRoot => "merge_or_root",
            CommitOutcome::TooLarge => "too_large",
            CommitOutcome::NoCodeChange => "no_code_change",
            CommitOutcome::NoTests => "no_tests",
            CommitOutcome::ThinMessage => "thin_message",
            CommitOutcome::IdTaken => "id_taken",
            CommitOutcome::Unreadable => "unreadable",
        }
    }
}

/// The commit one commit's diff is taken against (its only parent), or why the commit cannot be
/// a task: a merge has several parents, a root commit has none, and a history that was
/// flattened keeps a branch merge as a commit with one parent whose diff is still a whole
/// branch.
pub fn change_base(commit: &HistoryCommit) -> Result<&str, CommitOutcome> {
    match commit.parents.as_slice() {
        [parent] if !is_branch_merge_subject(commit.subject()) => Ok(parent),
        _ => Err(CommitOutcome::MergeOrRoot),
    }
}

/// What [`split`] of a commit's diff already rules out, before the caller reads any file for
/// [`split_inline_rust_tests`] (which moves changes from the fix to the tests, so it can empty
/// the fix but never changes the file count).
pub fn commit_precheck(split: &SplitPatch) -> Result<(), CommitOutcome> {
    if changed_paths(split).len() > link::MAX_FILES {
        return Err(CommitOutcome::TooLarge);
    }
    if !split.gold.iter().any(is_rust_source) {
        return Err(CommitOutcome::NoCodeChange);
    }
    Ok(())
}

/// The fix must change code, and the test rules ([`TestPathRules::rust_default`]) know Rust
/// only: a fix that is documentation, a manifest or a binary file is nothing a model could be
/// asked to write and the hidden tests could judge.
fn is_rust_source(file: &FileDiff) -> bool {
    file.path().ends_with(".rs")
}

/// What one commit is, given its diff fully split into fix and tests. The message is judged
/// last, so [`CommitOutcome::ThinMessage`] counts only commits that would otherwise be tasks:
/// the ones a better message (or a person writing the statement) would recover.
pub fn classify_commit(commit: &HistoryCommit, split: &SplitPatch) -> CommitOutcome {
    if change_base(commit).is_err() {
        return CommitOutcome::MergeOrRoot;
    }
    if let Err(outcome) = commit_precheck(split) {
        return outcome;
    }
    if !adds_to_a_test(split) {
        return CommitOutcome::NoTests;
    }
    if is_thin_message(&commit.message) {
        return CommitOutcome::ThinMessage;
    }
    CommitOutcome::Task
}

/// [`has_tests`], and at least one of those test files gains a line. A session's commit is
/// known to answer a request; a commit of the history may as well be a revert or a clean-up
/// that only removes tests, and a test that fails before the fix and passes after it has to be
/// written or changed by it.
fn adds_to_a_test(split: &SplitPatch) -> bool {
    split
        .test
        .iter()
        .any(|f| changes_a_test(f) && f.added.iter().any(|line| !line.trim().is_empty()))
}

/// The distinct paths a split diff changes, sorted. A file whose inline tests were split off is
/// in both halves and counts once.
fn changed_paths(split: &SplitPatch) -> Vec<String> {
    let mut paths: Vec<String> = split
        .gold
        .iter()
        .chain(&split.test)
        .map(|f| f.path().to_string())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Fewest words a commit message needs to be worth rewriting as an issue. Shorter ones ("fix
/// bug", "address review comments", "Fix #712") name no behaviour, so prompt P1 could only
/// reject them or invent a problem.
pub const MIN_MESSAGE_WORDS: usize = 5;

/// Fewest characters that do the same in a script written without spaces between words
/// (Chinese, Japanese, Thai), where a whole sentence is one whitespace-separated token.
pub const MIN_UNSPACED_CHARS: usize = 10;

/// A message this long usually says what was wrong, not only what changed.
pub const DETAILED_MESSAGE_CHARS: usize = 150;

/// How the subject a forge writes for a merged pull request begins. Its diff is one pull
/// request, so the commit can be a task, but the subject names a branch, not a problem.
const PULL_REQUEST_MERGE_SUBJECT: &str = "Merge pull request ";

/// How the subjects git itself writes for a branch merge begin (the quote included, which a
/// hand-written subject such as "Merge tag parsing into the lexer" does not have).
const BRANCH_MERGE_SUBJECTS: [&str; 6] = [
    "Merge branch '",
    "Merge branches '",
    "Merge remote-tracking branch '",
    "Merge remote-tracking branches '",
    "Merge tag '",
    "Merge commit '",
];

/// Whether a commit message is too short to state a problem: fewer than
/// [`MIN_MESSAGE_WORDS`] words (and fewer than [`MIN_UNSPACED_CHARS`] characters of a script
/// written without spaces), not counting its trailers, a generated merge subject, or tokens
/// without a letter (issue and pull-request numbers). An issue number does not rescue a short
/// message: prompt P1 is given the message only and cannot read the issue.
pub fn is_thin_message(message: &str) -> bool {
    let written = written_prose(message);
    let words = written
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphabetic))
        .count();
    let unspaced_chars = written.chars().filter(|c| is_unspaced_script(*c)).count();
    words < MIN_MESSAGE_WORDS && unspaced_chars < MIN_UNSPACED_CHARS
}

/// Whether a message is long enough ([`DETAILED_MESSAGE_CHARS`], counting what
/// [`is_thin_message`] counts) that prompt P1 is likely to get a problem statement out of it.
pub fn is_detailed_message(message: &str) -> bool {
    written_prose(message).chars().count() >= DETAILED_MESSAGE_CHARS
}

/// What a person wrote about the change: the message without its trailers
/// ([`commit_prose`]) and without a generated merge subject.
fn written_prose(message: &str) -> String {
    let prose = commit_prose(message);
    match prose.split_once('\n') {
        Some((subject, body)) if is_generated_merge_subject(subject) => body.trim().to_string(),
        None if is_generated_merge_subject(&prose) => String::new(),
        _ => prose,
    }
}

fn is_generated_merge_subject(subject: &str) -> bool {
    subject.starts_with(PULL_REQUEST_MERGE_SUBJECT) || is_branch_merge_subject(subject)
}

fn is_branch_merge_subject(subject: &str) -> bool {
    BRANCH_MERGE_SUBJECTS.iter().any(|m| subject.starts_with(m))
}

/// Han, kana, Thai, Lao, Khmer and Myanmar letters: scripts that put no space between words.
fn is_unspaced_script(c: char) -> bool {
    matches!(
        c,
        '\u{0E00}'..='\u{0EFF}'
            | '\u{1000}'..='\u{109F}'
            | '\u{1780}'..='\u{17FF}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{20000}'..='\u{2A6DF}'
    )
}

/// Whether a message names an issue or pull request by number (`#712`, `owner/repo#712`), where
/// the problem a thin message leaves out may be written down.
pub fn references_issue(message: &str) -> bool {
    message
        .split('#')
        .skip(1)
        .any(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// A commit message without its trailer block (`Signed-off-by: …`, `Fixes: #712`): the last
/// paragraph, when every line of it is a trailer ([`is_trailer`]). Trailers carry names, emails
/// and links and say nothing about the change.
pub fn commit_prose(message: &str) -> String {
    let lines: Vec<&str> = message.trim().lines().collect();
    let Some(last_break) = lines.iter().rposition(|l| l.trim().is_empty()) else {
        return lines.join("\n");
    };
    if lines[last_break + 1..].iter().all(|line| is_trailer(line)) {
        lines[..last_break].join("\n").trim_end().to_string()
    } else {
        lines.join("\n")
    }
}

/// Trailer keys whose value is a person.
const PERSON_TRAILER_KEYS: [&str; 10] = [
    "signed-off-by",
    "co-authored-by",
    "co-developed-by",
    "reviewed-by",
    "acked-by",
    "tested-by",
    "reported-by",
    "suggested-by",
    "helped-by",
    "cc",
];

/// Trailer keys whose value is an id when it is a single token (`Change-Id: I8f3a…`,
/// `Refs: NAS-1701`).
const REFERENCE_TRAILER_KEYS: [&str; 10] = [
    "change-id",
    "fixes",
    "closes",
    "resolves",
    "refs",
    "ref",
    "issue",
    "bug",
    "link",
    "see-also",
];

/// Whether a `Key: value` line is a trailer and not a sentence of the message. Any such line
/// would be a trailer to git ([`remote::trailers`]), but people also write `Before: total(0.5)
/// returned 0` or `Problem: split_off(len) panics`, which is exactly the text a statement
/// needs. So a line is dropped only when its key names a person, or its value is nothing but a
/// person, links, issue numbers or an id.
fn is_trailer(line: &str) -> bool {
    let Some((key, value)) = line.trim().split_once(": ") else {
        return false;
    };
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return false;
    }
    let (key, value) = (key.to_ascii_lowercase(), value.trim());
    let one_token = !value.contains(char::is_whitespace);
    PERSON_TRAILER_KEYS.contains(&key.as_str())
        || is_person(value)
        || is_only_references(value)
        || (one_token && REFERENCE_TRAILER_KEYS.contains(&key.as_str()))
}

/// `Dev One <dev@example.com>`.
fn is_person(value: &str) -> bool {
    value
        .rsplit_once('<')
        .is_some_and(|(_, email)| email.ends_with('>') && email.contains('@'))
}

/// Links and issue numbers only: `#712`, `owner/repo#712, #713`, `https://…`.
fn is_only_references(value: &str) -> bool {
    let mut tokens = value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|token| !token.is_empty())
        .peekable();
    tokens.peek().is_some()
        && tokens.all(|token| {
            token.starts_with("http://") || token.starts_with("https://") || is_issue_number(token)
        })
}

/// `#712` or `owner/repo#712`.
fn is_issue_number(token: &str) -> bool {
    token
        .split_once('#')
        .is_some_and(|(_, number)| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
}

/// Why a statement that prompt P1 wrote from a commit message must not be used, if it must
/// not: it repeats the message or its body (the rewriter echoed its input, so whatever the
/// message says about the fix is still there), or it names a file the commit changed
/// (`changed_files`, which the rewriter was never shown, so the message named it). `None` is
/// not a proof that nothing leaks: a statement can still describe the fix in other words,
/// which is what the screen (prompt P3) and a person are asked to look for.
pub fn commit_statement_leak(
    statement: &str,
    commit_message: &str,
    changed_files: &[String],
) -> Option<String> {
    let message = commit_prose(commit_message);
    let body = message.split_once('\n').map_or("", |(_, body)| body);
    let statement_words = squashed(statement);
    let repeats = |text: &str| {
        let text = squashed(text);
        !text.is_empty() && statement_words.contains(&text)
    };
    if repeats(&message) || (!is_thin_message(body) && repeats(body)) {
        return Some("the statement repeats the commit message".to_string());
    }
    changed_files
        .iter()
        .find(|path| names_file(statement, path))
        .map(|path| format!("the statement names {path}, a file the commit changed"))
}

/// Whether `statement` has the file name of `path` in it. A name without an extension
/// (`Makefile`, `justfile`) is also an ordinary word for the thing that misbehaves, so only
/// names with one count.
fn names_file(statement: &str, path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    file_name.contains('.') && statement.contains(file_name)
}

/// Lower case, with every run of whitespace as one space: how two texts are compared when only
/// their words matter.
fn squashed(text: &str) -> String {
    text.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The score of a link that needs no matching: a commit of the history is its own source.
const CERTAIN_LINK: f64 = 1.0;

/// A commit of the history as the [`Linked`] that [`draft_task`] takes; `split` is its diff.
/// There is no session, so the commit stands in for one: its full sha is the session id (the
/// task id is then `<repo>__<first 8 sha chars>`, stable and one per commit) and its message is
/// the one typed message, timed at the commit so that `TaskSource::new`, which drops messages
/// later than the commit, keeps it. The task's statement is therefore the raw commit message
/// until prompt P1 rewrites it (`NasikoExtras::needs_rewrite`). A commit without a single
/// parent to start from ([`change_base`]) has no task to draft.
pub fn linked_from_commit(
    commit: &HistoryCommit,
    split: &SplitPatch,
) -> Result<Linked, CommitOutcome> {
    let facts = CommitFacts {
        sha: commit.sha.clone(),
        parent: change_base(commit)?.to_string(),
        committed_at: commit.committed_at,
        subject: commit.subject().to_string(),
        files: changed_paths(split),
        added_lines: split
            .gold
            .iter()
            .chain(&split.test)
            .flat_map(|f| f.added.clone())
            .collect(),
        trailers: remote::trailers(&commit.message),
        branches: Vec::new(),
        is_merge: false,
    };
    // Nothing was matched, so the parts a session's link is scored on do not apply: the
    // overlaps are whole by definition and the trailer and branch were never looked at.
    let link = LinkScore {
        sha: commit.sha.clone(),
        total: CERTAIN_LINK,
        file_overlap: CERTAIN_LINK,
        line_overlap: CERTAIN_LINK,
        has_ai_trailer: false,
        same_branch: false,
    };
    Ok(Linked {
        decision: LinkDecision::Single { link: link.clone() },
        candidates: vec![link],
        commits: vec![facts],
        slice: SessionRecord {
            session_id: commit.sha.clone(),
            source: SessionSource::CommitHistory,
            cwd: String::new(),
            git_branch: None,
            git_branches: Vec::new(),
            started_at: commit.committed_at,
            ended_at: commit.committed_at,
            user_messages: vec![UserMessage {
                at: commit.committed_at,
                text: commit.message.clone(),
            }],
            edits: Vec::new(),
            usage: Vec::new(),
        },
        link_score: CERTAIN_LINK,
        one_of_several: false,
    })
}

/// Rust files of a model's patch that [`graded_files`] needs the base text of.
pub fn graded_paths(files: &[FileDiff]) -> Vec<String> {
    files
        .iter()
        .filter(|f| f.path().ends_with(".rs") && f.old_path.is_some() && f.old_path == f.new_path)
        .map(|f| f.path().to_string())
        .collect()
}

/// A model's patch with its edits to inline Rust test modules undone. Hidden tests often sit
/// as unit tests inside the code's own file, so without this a test the model added under a
/// hidden test's name breaks the build, and a model that rewrote a file's tests makes the
/// hidden test patch unappliable, both failing a fix whose code may be right. Files the model
/// created, renamed or deleted, files missing from `base_texts`, and files whose test modules
/// cannot be told apart, are kept as they are.
pub fn graded_files(
    files: Vec<FileDiff>,
    base_texts: &BTreeMap<String, String>,
) -> Result<Vec<FileDiff>, DiffError> {
    let mut kept = Vec::with_capacity(files.len());
    for file in files {
        let path = file.path().to_string();
        let same_file = file.old_path.is_some() && file.old_path == file.new_path;
        let base = base_texts.get(&path);
        let (true, true, Some(base)) = (path.ends_with(".rs"), same_file, base) else {
            kept.push(file);
            continue;
        };
        let Ok(changed) = patch::apply_to_text(base, &file.text) else {
            kept.push(file);
            continue;
        };
        let Some(graded) = rust_tests::restore_old_tests(base, &changed) else {
            kept.push(file);
            continue;
        };
        if graded == changed {
            kept.push(file);
        } else if &graded == base {
            // Only test edits in this file: none of it is graded.
        } else {
            kept.extend(diff::parse_unified(&patch::diff_texts(
                &path,
                Some(base),
                &graded,
            ))?);
        }
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::cost::TokenUsage;
    use crate::instance::SessionSource;
    use crate::session::{FileEdit, ModelUsage, UserMessage};

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().expect("timestamp")
    }

    fn session() -> SessionRecord {
        SessionRecord {
            session_id: "abcdef1234".into(),
            source: SessionSource::ClaudeCode,
            cwd: "/r".into(),
            git_branch: Some("main".into()),
            git_branches: vec!["main".into()],
            started_at: at("2026-11-01T10:00:00Z"),
            ended_at: at("2026-11-01T11:00:00Z"),
            user_messages: vec![
                UserMessage {
                    at: at("2026-11-01T10:00:00Z"),
                    text: "make b return 2".into(),
                },
                UserMessage {
                    at: at("2026-11-01T10:20:00Z"),
                    text: "and keep a".into(),
                },
            ],
            edits: vec![FileEdit {
                at: at("2026-11-01T10:10:00Z"),
                path: "/r/src/lib.rs".into(),
                added_lines: vec!["pub fn b() -> u8 { 2 }".into()],
            }],
            usage: vec![ModelUsage {
                at: at("2026-11-01T10:05:00Z"),
                message_id: "m1".into(),
                model: "claude-opus".into(),
                usage: TokenUsage {
                    input: 10,
                    output: 20,
                    ..TokenUsage::default()
                },
            }],
        }
    }

    fn commit(sha: &str, when: &str) -> CommitFacts {
        CommitFacts {
            sha: sha.into(),
            parent: "base".into(),
            committed_at: at(when),
            subject: "b returns 2".into(),
            files: vec!["src/lib.rs".into()],
            added_lines: vec!["pub fn b() -> u8 { 2 }".into()],
            trailers: vec!["Co-Authored-By: Claude".into()],
            branches: vec!["main".into()],
            is_merge: false,
        }
    }

    #[test]
    fn a_session_without_edits_in_the_repo_is_not_a_task() {
        let mut s = session();
        s.edits[0].path = "/elsewhere/x.rs".into();
        assert_eq!(precheck(&s, &["/r".into()]), Err(Outcome::NoEditsInRepo));
        s.user_messages.clear();
        assert_eq!(precheck(&s, &["/r".into()]), Err(Outcome::NoTypedMessage));
    }

    #[test]
    fn links_the_commit_and_drafts_a_task() {
        let s = session();
        let roots = vec!["/r".to_string()];
        assert_eq!(precheck(&s, &roots), Ok(vec!["src/lib.rs".to_string()]));
        let tasks =
            link_tasks(&s, &roots, vec![commit("c1", "2026-11-01T10:30:00Z")]).expect("links");
        let [linked] = tasks.as_slice() else {
            panic!("one task")
        };
        assert_eq!(linked.first().sha, "c1");
        let diff = patch::diff_texts(
            "src/lib.rs",
            Some("pub fn b() -> u8 { 1 }\n"),
            "pub fn b() -> u8 { 2 }\n",
        );
        let task = draft_task("o/r", linked, split(&diff).expect("splits"));
        assert_eq!(task.instance_id, "r__abcdef12");
        assert_eq!(task.base_commit, "base");
        assert_eq!(task.problem_statement, "make b return 2");
        assert_eq!(task.hints_text, "and keep a");
        assert_eq!(task.x_nasiko.orig.model, "claude-opus");
        assert_eq!(task.x_nasiko.test_source, TestSource::None);
    }

    #[test]
    fn commits_outside_the_window_do_not_link() {
        let s = session();
        let late = commit("c1", "2026-11-02T10:30:00Z");
        assert_eq!(
            link_tasks(&s, &["/r".into()], vec![late]).err(),
            Some(Outcome::NoMatch)
        );
    }

    #[test]
    fn a_long_session_gives_a_task_per_commit_each_from_its_own_request() {
        let mut s = session();
        s.ended_at = at("2026-11-01T14:00:00Z");
        s.user_messages = vec![
            UserMessage {
                at: at("2026-11-01T10:00:00Z"),
                text: "make b return 2".into(),
            },
            UserMessage {
                at: at("2026-11-01T12:00:00Z"),
                text: "now add c".into(),
            },
        ];
        s.edits.push(FileEdit {
            at: at("2026-11-01T12:10:00Z"),
            path: "/r/src/c.rs".into(),
            added_lines: vec!["pub fn c() -> u8 { 3 }".into()],
        });
        let first = commit("c1aaaaaaaa", "2026-11-01T10:30:00Z");
        let unrelated = CommitFacts {
            files: vec!["README.md".into()],
            added_lines: vec!["someone else's words".into()],
            trailers: vec![],
            ..commit("u1", "2026-11-01T11:00:00Z")
        };
        let second = CommitFacts {
            files: vec!["src/c.rs".into()],
            added_lines: vec!["pub fn c() -> u8 { 3 }".into()],
            ..commit("c2bbbbbbbb", "2026-11-01T12:30:00Z")
        };
        let roots = vec!["/r".to_string()];
        let tasks = link_tasks(&s, &roots, vec![second, unrelated, first]).expect("links");
        let shas: Vec<&str> = tasks.iter().map(|t| t.first().sha.as_str()).collect();
        assert_eq!(shas, ["c1aaaaaaaa", "c2bbbbbbbb"]);
        assert!(tasks.iter().all(|t| t.one_of_several));
        let requests: Vec<&str> = tasks
            .iter()
            .map(|t| t.slice.user_messages[0].text.as_str())
            .collect();
        assert_eq!(requests, ["make b return 2", "now add c"]);
        let ids: Vec<String> = tasks
            .iter()
            .map(|t| draft_task("o/r", t, SplitPatch::default()).instance_id)
            .collect();
        assert_eq!(ids, ["r__abcdef12-c1aaaaa", "r__abcdef12-c2bbbbb"]);
    }

    #[test]
    fn inline_tests_move_to_the_test_patch() {
        let base = "pub fn b() -> u8 { 1 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn old() {}\n}\n";
        let gold = "pub fn b() -> u8 { 2 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn old() {}\n\n    #[test]\n    fn new() { assert_eq!(super::b(), 2) }\n}\n";
        let mut parts = split(&patch::diff_texts("src/lib.rs", Some(base), gold)).expect("splits");
        assert_eq!(inline_test_paths(&parts), vec!["src/lib.rs"]);
        let versions = BTreeMap::from([(
            "src/lib.rs".to_string(),
            FileVersions {
                base: Some(base.into()),
                gold: gold.into(),
            },
        )]);
        split_inline_rust_tests(&mut parts, &versions).expect("splits inline");
        assert_eq!(parts.gold.len(), 1);
        assert_eq!(parts.test.len(), 1);
        assert!(parts.gold[0].text.contains("+pub fn b() -> u8 { 2 }"));
        assert!(parts.test[0].text.contains("fn new()"));
        assert!(parts.mixed.is_empty());
        assert!(has_tests(&parts));
    }

    #[test]
    fn a_commit_changing_only_test_helpers_or_no_tests_has_none() {
        let only = |path: &str, old: &str, new: &str| {
            split(&patch::diff_texts(path, Some(old), new)).expect("splits")
        };
        assert!(!has_tests(&only("justfile", "run:\n", "run: build\n")));
        assert!(!has_tests(&only(
            "oss/server/tests/common/mod.rs",
            "pub fn cfg() {}\n",
            "pub fn cfg() { let _ = 1; }\n"
        )));
        assert!(has_tests(&only(
            "oss/server/tests/common/mod.rs",
            "\n",
            "#[tokio::test(flavor = \"multi_thread\")]\nasync fn t() {}\n"
        )));
        assert!(has_tests(&only(
            "oss/server/tests/pricing.rs",
            "fn a() {}\n",
            "fn a() { assert!(true) }\n"
        )));
        assert!(has_tests(&only("tests/budgets/main.rs", "\n", "mod x;\n")));
        assert!(!has_tests(&only(
            "tests/budgets/helpers.rs",
            "\n",
            "fn h() {}\n"
        )));
    }

    const SHA: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";
    const CLEAR_MESSAGE: &str = "Fix b returning the wrong value\n\nCallers of b() got 1 where the documented result is 2.\n\nSigned-off-by: Dev One <dev@example.com>\n";

    fn history_commit(parents: &[&str], message: &str) -> HistoryCommit {
        HistoryCommit {
            sha: SHA.into(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
            committed_at: at("2026-11-01T10:30:00Z"),
            message: message.into(),
        }
    }

    fn fix_diff() -> String {
        patch::diff_texts(
            "src/lib.rs",
            Some("pub fn b() -> u8 { 1 }\n"),
            "pub fn b() -> u8 { 2 }\n",
        )
    }

    fn test_diff() -> String {
        patch::diff_texts("tests/it.rs", None, "#[test]\nfn b_is_two() {}\n")
    }

    #[test]
    fn a_merge_or_a_root_commit_is_never_a_task() {
        let fix_and_test = split(&(fix_diff() + &test_diff())).expect("splits");
        for parents in [&[][..], &["p1", "p2"]] {
            let commit = history_commit(parents, CLEAR_MESSAGE);
            assert_eq!(change_base(&commit), Err(CommitOutcome::MergeOrRoot));
            assert_eq!(
                classify_commit(&commit, &fix_and_test),
                CommitOutcome::MergeOrRoot
            );
            assert_eq!(
                linked_from_commit(&commit, &fix_and_test).err(),
                Some(CommitOutcome::MergeOrRoot),
                "no task is drafted on an empty base commit"
            );
        }
        assert_eq!(
            change_base(&history_commit(&["p1"], CLEAR_MESSAGE)),
            Ok("p1")
        );
    }

    #[test]
    fn a_flattened_branch_merge_is_a_merge_but_a_merged_pull_request_is_one_change() {
        let fix_and_test = split(&(fix_diff() + &test_diff())).expect("splits");
        for subject in [
            "Merge branch 'development' into feat/seed-the-search-connector",
            "Merge remote-tracking branch 'origin/development' into feat/x [synced]",
            "Merge tag 'v1.2.0'",
        ] {
            let message = format!("{subject}\n\nReading an empty buffer no longer panics");
            let commit = history_commit(&["p1"], &message);
            assert_eq!(
                classify_commit(&commit, &fix_and_test),
                CommitOutcome::MergeOrRoot,
                "{subject}"
            );
        }
        for subject in [
            "Merge pull request #12 from o/fix",
            "Merge tag parsing into the lexer so that both report one error",
        ] {
            let message = format!("{subject}\n\nReading an empty buffer no longer panics");
            let commit = history_commit(&["p1"], &message);
            assert_eq!(
                classify_commit(&commit, &fix_and_test),
                CommitOutcome::Task,
                "{subject}"
            );
        }
    }

    #[test]
    fn a_commit_is_a_task_only_with_a_fix_its_tests_and_a_message_that_states_a_problem() {
        let commit = history_commit(&["base"], CLEAR_MESSAGE);
        let classify = |diff: &str, commit: &HistoryCommit| {
            classify_commit(commit, &split(diff).expect("splits"))
        };
        let fix_and_test = fix_diff() + &test_diff();
        assert_eq!(classify(&fix_and_test, &commit), CommitOutcome::Task);
        assert_eq!(classify(&fix_diff(), &commit), CommitOutcome::NoTests);
        assert_eq!(classify(&test_diff(), &commit), CommitOutcome::NoCodeChange);
        assert_eq!(classify("", &commit), CommitOutcome::NoCodeChange);
        let thin = history_commit(&["base"], "Fix #712");
        assert_eq!(classify(&fix_and_test, &thin), CommitOutcome::ThinMessage);
        assert_eq!(
            classify(&fix_diff(), &thin),
            CommitOutcome::NoTests,
            "the message is judged last"
        );
    }

    #[test]
    fn a_fix_without_rust_source_is_no_code_change_even_with_tests() {
        let commit = history_commit(&["base"], CLEAR_MESSAGE);
        let with_tests =
            |fix: String| classify_commit(&commit, &split(&(fix + &test_diff())).expect("splits"));
        let docs = patch::diff_texts("README.md", Some("b is 1\n"), "b is 2\n");
        let manifest = patch::diff_texts("Cargo.toml", Some("[package]\n"), "[package]\n\n");
        let binary = "diff --git a/assets/logo.png b/assets/logo.png\nindex 1a2b3c4..5d6e7f8 100644\nGIT binary patch\nliteral 3\nKcmZQzU|?\n\nliteral 3\nKcmZQzU|?\n\n";
        assert_eq!(with_tests(docs.clone()), CommitOutcome::NoCodeChange);
        assert_eq!(with_tests(manifest), CommitOutcome::NoCodeChange);
        assert_eq!(with_tests(binary.to_string()), CommitOutcome::NoCodeChange);
        assert_eq!(with_tests(docs + &fix_diff()), CommitOutcome::Task);
        assert_eq!(
            classify_commit(
                &commit,
                &split(&patch::diff_texts("README.md", None, "b\n")).expect("splits")
            ),
            CommitOutcome::NoCodeChange,
            "a documentation commit is not short of tests, it has no code"
        );
    }

    #[test]
    fn a_commit_that_only_removes_tests_has_no_tests() {
        let commit = history_commit(&["base"], CLEAR_MESSAGE);
        let removed_file = "diff --git a/tests/it.rs b/tests/it.rs\ndeleted file mode 100644\nindex 1a2b3c4..0000000\n--- a/tests/it.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-#[test]\n-fn b_is_two() {}\n";
        let revert = split(&(fix_diff() + removed_file)).expect("splits");
        assert!(has_tests(&revert), "the session rule is left as it was");
        assert_eq!(classify_commit(&commit, &revert), CommitOutcome::NoTests);

        let removed_test = patch::diff_texts(
            "tests/it.rs",
            Some("#[test]\nfn b_is_two() {}\n\n#[test]\nfn b_is_even() {}\n"),
            "#[test]\nfn b_is_two() {}\n",
        );
        let cleanup = split(&(fix_diff() + &removed_test)).expect("splits");
        assert_eq!(classify_commit(&commit, &cleanup), CommitOutcome::NoTests);
    }

    #[test]
    fn a_commit_touching_too_many_files_is_too_large() {
        let many: String = (0..=link::MAX_FILES)
            .map(|i| patch::diff_texts(&format!("src/f{i}.rs"), None, "pub fn f() {}\n"))
            .collect();
        let parts = split(&(many + &test_diff())).expect("splits");
        assert_eq!(commit_precheck(&parts), Err(CommitOutcome::TooLarge));
        assert_eq!(
            classify_commit(&history_commit(&["base"], CLEAR_MESSAGE), &parts),
            CommitOutcome::TooLarge
        );
    }

    #[test]
    fn a_commit_changing_only_an_inline_test_module_has_no_code_change() {
        let base = "pub fn b() -> u8 { 2 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn old() {}\n}\n";
        let gold = "pub fn b() -> u8 { 2 }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn old() {}\n\n    #[test]\n    fn new() { assert_eq!(super::b(), 2) }\n}\n";
        let mut parts = split(&patch::diff_texts("src/lib.rs", Some(base), gold)).expect("splits");
        assert_eq!(
            commit_precheck(&parts),
            Ok(()),
            "not known before the split"
        );
        let versions = BTreeMap::from([(
            "src/lib.rs".to_string(),
            FileVersions {
                base: Some(base.into()),
                gold: gold.into(),
            },
        )]);
        split_inline_rust_tests(&mut parts, &versions).expect("splits inline");
        assert_eq!(
            classify_commit(&history_commit(&["base"], CLEAR_MESSAGE), &parts),
            CommitOutcome::NoCodeChange
        );
    }

    #[test]
    fn a_message_is_thin_below_five_words_trailers_and_numbers_aside() {
        for thin in [
            "",
            "wip",
            "Fix #712",
            "fix bug (#31)",
            "Address review comments",
            "Fix the parser\n\nSigned-off-by: Dev One <dev@example.com>\nReviewed-by: Dev Two <two@example.com>",
            "Merge pull request #1232 from some-org/feat/agent-months-edge-fixes",
            "Merge branch 'development' into feat/seed-the-search-connector\n\nFix conflicts",
            "Fix the parser\n\nFixes: #712\nLink: https://example.com/issues/712\nChange-Id: I8f3a2b1c",
            "修复错误",
        ] {
            assert!(is_thin_message(thin), "{thin:?}");
        }
        for clear in [
            "Fix panic on empty buffer",
            "Merge pull request #12 from o/fix\n\nReading an empty buffer no longer panics",
            "Fix reserve\n\nReserving past isize::MAX wrapped around instead of failing.",
            "Fix rounding of totals\n\nBefore: total(0.5) returned 0\nAfter: total(0.5) returns 1",
            "Fix panic in split_off\n\nProblem: split_off(len) panics although the docs allow it.",
            "读取空缓冲区时程序会崩溃而不是返回零字节，调用方无法恢复",
            CLEAR_MESSAGE,
        ] {
            assert!(!is_thin_message(clear), "{clear:?}");
        }
    }

    #[test]
    fn prose_drops_only_a_closing_trailer_block() {
        assert_eq!(
            commit_prose(CLEAR_MESSAGE),
            "Fix b returning the wrong value\n\nCallers of b() got 1 where the documented result is 2."
        );
        let colon_in_body = "Fix b\n\nNote: callers relied on the old value.\nIt was wrong.";
        assert_eq!(commit_prose(colon_in_body), colon_in_body);
        assert_eq!(commit_prose("  Fix b\n"), "Fix b");
        assert_eq!(
            commit_prose(
                "Fix b\n\nb() returned 1.\n\nFixes: o/r#712, #713\nRefs: NAS-1701\nSee-also: https://example.com/a\nCc: the storage team\nReviewed-by: Dev Two <two@example.com>"
            ),
            "Fix b\n\nb() returned 1."
        );
    }

    #[test]
    fn key_value_lines_that_state_the_problem_are_not_trailers() {
        for stated in [
            "Fix rounding of totals\n\nBefore: total(0.5) returned 0\nAfter: total(0.5) returns 1",
            "Fix panic in split_off\n\nProblem: split_off(len) panics although the docs allow it.",
            "Fix b\n\nFixes: a wrong value that callers of b() relied on",
            "Fix b\n\nResult: 1\nExpected: 2",
        ] {
            assert_eq!(commit_prose(stated), stated);
        }
    }

    #[test]
    fn issue_references_and_detailed_messages_are_told_apart() {
        assert!(references_issue("Fix #712"));
        assert!(references_issue("Fix b (tokio-rs/bytes#712)"));
        assert!(!references_issue("Derive #[non_exhaustive] for errors"));
        assert!(!is_detailed_message(CLEAR_MESSAGE));
        let body = "The old value broke callers. ".repeat(6);
        assert!(is_detailed_message(&format!("Fix b\n\n{body}")));
    }

    #[test]
    fn a_generated_merge_subject_does_not_make_a_message_detailed() {
        let subject = "Merge pull request #1232 from some-org/feat/agent-months-edge-fixes";
        let body = "The old value broke callers of b. ".repeat(3);
        let padded_by_subject = format!("{subject}\n\n{body}");
        assert!(padded_by_subject.chars().count() >= DETAILED_MESSAGE_CHARS);
        assert!(!is_detailed_message(&padded_by_subject));
        assert!(is_detailed_message(&format!("{subject}\n\n{body}{body}")));
    }

    #[test]
    fn a_rewritten_statement_that_repeats_the_message_or_names_a_changed_file_leaks() {
        let files = vec!["src/bytes_mut.rs".to_string(), "Makefile".to_string()];
        let message = "Fix panic in split_off\n\nClamp the index in split_off so that at == len returns an empty buffer.\n\nSigned-off-by: Dev One <dev@example.com>";
        let leak = |statement: &str| commit_statement_leak(statement, message, &files);

        let echoed = "Fix panic in split_off\n\nClamp the index in split_off so that\nat == len returns an empty buffer.";
        assert_eq!(
            leak(echoed).as_deref(),
            Some("the statement repeats the commit message")
        );
        let body_kept = "split_off panics\n\nclamp the index in split_off so that at == len returns an empty buffer. It should not panic.";
        assert!(leak(body_kept).is_some());
        assert_eq!(
            leak("split_off panics at the end\n\nSee bytes_mut.rs.").as_deref(),
            Some("the statement names src/bytes_mut.rs, a file the commit changed")
        );
        assert_eq!(
            leak(
                "split_off panics at the end\n\nCalling split_off(len) panics; the Makefile target shows it. It should return an empty buffer."
            ),
            None
        );
    }

    #[test]
    fn a_commit_drafts_a_task_named_by_its_sha_whose_statement_still_needs_rewriting() {
        let commit = history_commit(&["base"], CLEAR_MESSAGE);
        let parts = split(&(fix_diff() + &test_diff())).expect("splits");
        let linked = linked_from_commit(&commit, &parts).expect("one parent");
        assert_eq!(linked.first().files, ["src/lib.rs", "tests/it.rs"]);
        assert_eq!(linked.first().subject, "Fix b returning the wrong value");
        assert!(!linked.one_of_several);

        let source = crate::TaskSource::new(&linked.slice, &linked.slice, &[linked.first()]);
        assert_eq!(source.source, SessionSource::CommitHistory);
        assert_eq!(source.context_messages.len(), 1, "the message is kept");
        assert_eq!(source.context_messages[0].text, CLEAR_MESSAGE);
        assert!(source.usage_by_model.is_empty());

        let task = draft_task("o/r", &linked, parts);
        assert_eq!(task.instance_id, "r__1a2b3c4d");
        assert_eq!(task.base_commit, "base");
        assert_eq!(task.created_at, commit.committed_at);
        assert_eq!(task.problem_statement, CLEAR_MESSAGE);
        assert_eq!(task.hints_text, "");
        assert_eq!(task.x_nasiko.source, SessionSource::CommitHistory);
        assert_eq!(task.x_nasiko.source_session_id, SHA);
        assert_eq!(task.x_nasiko.gold_commit, SHA);
        assert_eq!(task.x_nasiko.test_source, TestSource::Commit);
        assert_eq!(task.x_nasiko.orig.model, "");
        assert!(task.x_nasiko.needs_rewrite());
        assert!(task.patch.contains("src/lib.rs") && !task.patch.contains("tests/it.rs"));
        assert!(task.test_patch.contains("tests/it.rs"));
    }

    #[test]
    fn a_models_edits_to_inline_tests_are_not_graded() {
        let base = "pub fn f() -> u8 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn old() {}\n}\n";
        let model = "pub fn f() -> u8 {\n    2\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn old() {}\n\n    #[test]\n    fn hidden() {}\n}\n";
        let files = diff::parse_unified(&patch::diff_texts("src/lib.rs", Some(base), model))
            .expect("parses");
        assert_eq!(graded_paths(&files), vec!["src/lib.rs"]);
        let texts = BTreeMap::from([("src/lib.rs".to_string(), base.to_string())]);
        let graded = graded_files(files, &texts).expect("grades");
        assert_eq!(graded.len(), 1);
        assert!(graded[0].text.contains("+    2"));
        assert!(!graded[0].text.contains("hidden"));

        let tests_only = base.replace("fn old() {}", "fn old() { assert!(true) }");
        let files = diff::parse_unified(&patch::diff_texts("src/lib.rs", Some(base), &tests_only))
            .expect("parses");
        assert!(graded_files(files, &texts).expect("grades").is_empty());
    }
}
