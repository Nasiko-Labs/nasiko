//! One line about an attempt that did not resolve its task: what stands out in how it worked
//! ([`crate::method`], [`crate::selfcheck`]).
//!
//! The line says what the session shows, never why the attempt failed: each finding is worded
//! as something the session does or does not show, because a session records only part of
//! what an agent did. A finding that could not be told either way is left out.

use crate::method::{BlindEdits, FirstRun, FixFiles, Method, Rework, SharedItems, named};
use crate::selfcheck::SelfCheck;

/// Most findings joined into the story of one failed attempt.
const MAX_STORY_FINDINGS: usize = 3;

/// What stands out in how a failed attempt worked, as one line: the first
/// [`MAX_STORY_FINDINGS`] of the findings below that apply, in the report's order. A likely
/// story for a person to check against the session, not the reason the attempt failed.
pub fn story(method: &Method, self_check: Option<&SelfCheck>) -> String {
    let findings: Vec<String> = [
        method.fix_files.as_ref().and_then(place_finding),
        method.shared_items.as_ref().and_then(names_finding),
        method.first_run.as_ref().and_then(run_finding),
        method.blind_edits.as_ref().and_then(blind_finding),
        method.rework.as_ref().and_then(rework_finding),
        self_check.and_then(check_finding),
    ]
    .into_iter()
    .flatten()
    .take(MAX_STORY_FINDINGS)
    .collect();
    if findings.is_empty() {
        "Failed; nothing stands out in what its session shows.".into()
    } else {
        format!("Failed. Its session shows {}.", findings.join("; "))
    }
}

fn it_or_them(names: &[String]) -> &'static str {
    if names.len() == 1 { "it" } else { "them" }
}

fn place_finding(files: &FixFiles) -> Option<String> {
    if !files.never_looked.is_empty() {
        Some(format!(
            "no look at {}, which the real fix changed",
            named(&files.never_looked)
        ))
    } else if !files.opened_not_changed().is_empty() {
        let opened = files.opened_not_changed();
        Some(format!(
            "a look at {} without a change to {}, which the real fix changed",
            named(&opened),
            it_or_them(&opened)
        ))
    } else if !files.never_opened.is_empty() {
        // A search result is not much of a look: say what it was.
        Some(format!(
            "{} only in a search result or named in a command, never opened, though the real fix changed {}",
            named(&files.never_opened),
            it_or_them(&files.never_opened)
        ))
    } else {
        None
    }
}

fn names_finding(shared: &SharedItems) -> Option<String> {
    let not_searched = shared.not_searched();
    (!not_searched.is_empty()).then(|| {
        format!(
            "no search for the name of {} shared item(s) it changed ({})",
            not_searched.len(),
            named(&not_searched)
        )
    })
}

fn run_finding(run: &FirstRun) -> Option<String> {
    (!run.ran_before_first_change)
        .then(|| "no build or test run before its first change to existing code".to_string())
}

fn blind_finding(edits: &BlindEdits) -> Option<String> {
    (!edits.without_looking.is_empty()).then(|| {
        format!(
            "no look at {} before editing {}",
            named(&edits.without_looking),
            it_or_them(&edits.without_looking)
        )
    })
}

fn rework_finding(rework: &Rework) -> Option<String> {
    rework.is_heavy().then(|| rework.in_words())
}

fn check_finding(check: &SelfCheck) -> Option<String> {
    (check.edits > 0 && !check.checked_after_last_edit)
        .then(|| "no build or test run after its last edit".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::method::tests::{
        a_hasty_attempt, a_patch_that_changes_shared_items, edit_lib, lib_only, measured, the_fix,
    };
    use crate::trace::fixtures::{edit, read, shell, shell_run};

    const PATH_RS: &str = "/testbed/src/uri/path.rs";
    const LIB_RS: &str = "/testbed/src/lib.rs";

    fn unchecked() -> SelfCheck {
        SelfCheck {
            edits: 2,
            ..SelfCheck::default()
        }
    }

    #[test]
    fn the_story_joins_the_first_findings_as_what_the_session_shows() {
        assert_eq!(
            story(&a_hasty_attempt(), Some(&unchecked())),
            "Failed. Its session shows no look at src/uri/path.rs, which the real fix changed; no build or test run before its first change to existing code; no look at src/lib.rs before editing it."
        );
        let looked = measured(
            vec![read(LIB_RS), read(PATH_RS), shell("cargo test"), edit_lib()],
            &a_patch_that_changes_shared_items(),
        );
        assert_eq!(
            story(&looked, Some(&unchecked())),
            "Failed. Its session shows a look at src/lib.rs, src/uri/path.rs without a change to them, which the real fix changed; no search for the name of 3 shared item(s) it changed (parse, port, host); no build or test run after its last edit."
        );
    }

    #[test]
    fn a_file_seen_only_in_a_search_result_is_not_called_a_look() {
        let broad_search = shell_run("grep -rn 'fn ' .", 0, "./src/uri/path.rs:1:fn b() {}");
        let listed = measured(
            vec![
                read(LIB_RS),
                broad_search,
                shell("cargo test"),
                edit_lib(),
                shell("cargo test"),
            ],
            &lib_only(),
        );
        let checked = SelfCheck {
            edits: 1,
            checks: 2,
            checked_after_last_edit: true,
            ..SelfCheck::default()
        };
        assert_eq!(
            story(&listed, Some(&checked)),
            "Failed. Its session shows src/uri/path.rs only in a search result or named in a command, never opened, though the real fix changed it."
        );
    }

    #[test]
    fn an_attempt_that_kept_nothing_of_what_it_wrote_says_so() {
        let reverted = measured(
            vec![
                read(LIB_RS),
                read(PATH_RS),
                shell("cargo test"),
                edit(LIB_RS, "fn a() {}", "fn a() {\n    1;\n}"),
                edit(LIB_RS, "fn a() {\n    1;\n}", "fn a() {}"),
                shell("cargo test"),
            ],
            &[],
        );
        let checked = SelfCheck {
            edits: 2,
            checks: 2,
            checked_after_last_edit: true,
            ..SelfCheck::default()
        };
        assert_eq!(
            story(&reverted, Some(&checked)),
            "Failed. Its session shows a look at src/lib.rs, src/uri/path.rs without a change to them, which the real fix changed; 4 line(s) written and none kept."
        );
    }

    #[test]
    fn what_cannot_be_told_and_what_went_well_are_not_findings() {
        let checked = SelfCheck {
            edits: 1,
            checks: 2,
            checked_after_last_edit: true,
            ..SelfCheck::default()
        };
        let careful = measured(
            vec![
                read(LIB_RS),
                read(PATH_RS),
                shell("cargo test"),
                edit_lib(),
                edit(PATH_RS, "fn b() {}", "fn b() { 2 }"),
                shell("cargo test"),
            ],
            &the_fix(),
        );
        assert_eq!(
            story(&careful, Some(&checked)),
            "Failed; nothing stands out in what its session shows."
        );
        // A glob may have shown the file: the story does not say it was never looked at.
        let unclear = measured(
            vec![
                shell("cat src/*.rs src/uri/*.rs"),
                shell("cargo test"),
                edit_lib(),
                shell("cargo test"),
            ],
            &lib_only(),
        );
        assert_eq!(
            story(&unclear, Some(&checked)),
            "Failed; nothing stands out in what its session shows."
        );
    }
}
