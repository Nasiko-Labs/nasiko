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

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::diff::{self, DiffError, FileDiff, SplitPatch, TestPathRules};
use crate::instance::{self, NasikoExtras, OriginalRun, TaskInstance, TaskStatus, TestSource};
use crate::link::{self, CommitFacts, LinkDecision, LinkScore};
use crate::session::SessionRecord;
use crate::spec::{EnvSpec, Language};
use crate::{node, patch, rust_tests};

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
    split_by(full_diff, &TestPathRules::rust_default())
}

/// [`split`] by the repository's own test-path rules ([`split_rules`]).
pub fn split_by(full_diff: &str, rules: &TestPathRules) -> Result<SplitPatch, DiffError> {
    Ok(diff::split_tests(diff::parse_unified(full_diff)?, rules))
}

/// The test-path rules and language a repository's commits are split by: its spec's, or
/// Rust's when it has none (its tasks are then rejected as unsupported, and kept so the person
/// sees why).
pub fn split_rules(spec: Option<&EnvSpec>) -> (TestPathRules, Language) {
    match spec {
        Some(spec) => (TestPathRules::from_globs(&spec.test_paths), spec.language),
        None => (TestPathRules::rust_default(), Language::Rust),
    }
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
    split
        .test
        .iter()
        .any(|f| !in_tests_folder(f.path()) || is_test_target(f.path()) || adds_test_attr(f))
}

/// [`has_tests`] for a repository's language. A Node test file counts when a test runner runs
/// it ([`node::is_test_module`]) and the patch leaves it in the tree: a helper under `test/`
/// or a deleted test proves nothing.
pub fn has_tests_in(split: &SplitPatch, language: Language) -> bool {
    match language {
        Language::Rust => has_tests(split),
        Language::Node => split
            .test
            .iter()
            .any(|f| f.new_path.is_some() && node::is_test_module(f.path())),
    }
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

    #[test]
    fn a_node_commit_needs_a_test_file_not_only_helpers() {
        let rules = TestPathRules::from_globs(&node::test_paths());
        let only = |path: &str| {
            let parts = split_by(&patch::diff_texts(path, Some("a\n"), "b\n"), &rules).expect("ok");
            has_tests_in(&parts, Language::Node)
        };
        assert!(only("server/test/finch.test.ts"));
        assert!(only("web/src/__tests__/a.tsx"));
        assert!(!only("server/test/helpers.ts"));
        assert!(!only("server/src/finch.ts"));
    }

    #[test]
    fn node_fixes_are_split_by_file_and_never_by_inline_rust_tests() {
        let rules = TestPathRules::from_globs(&node::test_paths());
        let diff = format!(
            "{}{}",
            patch::diff_texts("server/src/llm.ts", Some("a\n"), "#[test]\nb\n"),
            patch::diff_texts("server/test/llm.test.ts", Some("a\n"), "b\n")
        );
        let parts = split_by(&diff, &rules).expect("splits");
        assert_eq!(parts.gold.len(), 1);
        assert_eq!(parts.test[0].path(), "server/test/llm.test.ts");
        assert!(inline_test_paths(&parts).is_empty());
        assert!(graded_paths(&parts.gold).is_empty());
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
