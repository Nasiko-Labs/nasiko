//! How a model's change compares with the real fix in shape: which files each touched and how
//! many lines each changed.
//!
//! This says "differs from your fix", never "wrong": another correct fix can touch other files
//! and be a different size. It is for a reader who wants to know what a passing change looks
//! like before merging changes from that model, e.g. "passes the tests but is 3x larger".
//!
//! Lock files ([`BUILD_FILES`]) are left out of both sides. A model's patch is everything that
//! changed in its sandbox, and a build there can rewrite a tracked `Cargo.lock` without the
//! model touching it; counted, that churn would pass for the model's own work.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::diff::{self, FileDiff, LineKind};
use crate::report::{Rate, rate_of};
use crate::stats;

/// A model's change is called much larger than the real fix above this size ratio.
pub const OVERSIZE_RATIO: f64 = 3.0;

/// File names a build or a package manager rewrites by itself; never compared.
pub const BUILD_FILES: [&str; 8] = [
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.sum",
    "poetry.lock",
    "uv.lock",
    "Gemfile.lock",
];

/// Lines a patch adds and removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LineCount {
    pub added: u32,
    pub removed: u32,
}

impl LineCount {
    pub fn total(&self) -> u32 {
        self.added + self.removed
    }
}

/// One attempt's change next to the real fix, test files and build files left out of both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeDiff {
    /// Whether the model touched at least one file the real fix touched.
    pub found_place: bool,
    /// Files both touched, by the real fix's name for them. A file the model also renamed still
    /// counts: it is matched by its old and its new path.
    pub shared: Vec<String>,
    /// Files only the model touched.
    pub extra: Vec<String>,
    /// Files only the real fix touched.
    pub missed: Vec<String>,
    /// Build files in the model's patch ([`BUILD_FILES`]); listed, not compared.
    pub build_files: Vec<String>,
    pub model_lines: LineCount,
    pub fix_lines: LineCount,
    /// Lines the model changed ÷ lines the real fix changed; `None` when either side changes no
    /// counted line (a rename or a binary file, say), so there is nothing to divide.
    pub size_ratio: Option<f64>,
}

impl ScopeDiff {
    /// Whether the model's change is more than [`OVERSIZE_RATIO`] times the real fix.
    pub fn is_oversized(&self) -> bool {
        self.size_ratio.is_some_and(|r| r > OVERSIZE_RATIO)
    }

    /// Whether the model changed any file that is compared. `false` for a patch of build files
    /// only: there is no change of the model's to set against the real fix.
    pub fn has_model_change(&self) -> bool {
        !self.shared.is_empty() || !self.extra.is_empty()
    }
}

/// Compares the non-test part of a model's patch with the real fix (the task's gold patch).
pub fn compare(model: &[FileDiff], fix: &[FileDiff]) -> ScopeDiff {
    let (build_files, model): (Vec<&FileDiff>, Vec<&FileDiff>) =
        model.iter().partition(|file| is_build_file(file));
    let fix: Vec<&FileDiff> = fix.iter().filter(|file| !is_build_file(file)).collect();
    let (model_paths, fix_paths) = (paths(&model), paths(&fix));
    let touches = |file: &FileDiff, others: &BTreeSet<&str>| {
        both_paths(file).any(|path| others.contains(path))
    };
    let (shared, missed): (Vec<&FileDiff>, Vec<&FileDiff>) =
        fix.iter().partition(|file| touches(file, &model_paths));
    let extra: Vec<&FileDiff> = model
        .iter()
        .copied()
        .filter(|file| !touches(file, &fix_paths))
        .collect();
    let (model_lines, fix_lines) = (line_count(&model), line_count(&fix));
    ScopeDiff {
        found_place: !shared.is_empty(),
        shared: names(&shared),
        extra: names(&extra),
        missed: names(&missed),
        build_files: names(&build_files),
        model_lines,
        fix_lines,
        size_ratio: (model_lines.total() > 0 && fix_lines.total() > 0)
            .then(|| f64::from(model_lines.total()) / f64::from(fix_lines.total())),
    }
}

fn is_build_file(file: &FileDiff) -> bool {
    let name = file.path().rsplit('/').next().unwrap_or_default();
    BUILD_FILES.contains(&name)
}

/// A file's path before and after the change (the same path unless it was renamed).
fn both_paths(file: &FileDiff) -> impl Iterator<Item = &str> {
    [file.old_path.as_deref(), file.new_path.as_deref()]
        .into_iter()
        .flatten()
}

fn paths<'a>(files: &[&'a FileDiff]) -> BTreeSet<&'a str> {
    files.iter().flat_map(|file| both_paths(file)).collect()
}

/// The files' names, sorted, each once.
fn names(files: &[&FileDiff]) -> Vec<String> {
    let sorted: BTreeSet<&str> = files.iter().map(|file| file.path()).collect();
    sorted.into_iter().map(str::to_string).collect()
}

fn line_count(files: &[&FileDiff]) -> LineCount {
    let mut count = LineCount::default();
    for (kind, _) in files
        .iter()
        .flat_map(|file| diff::hunks(file))
        .flat_map(|hunk| hunk.lines)
    {
        match kind {
            LineKind::Added => count.added += 1,
            LineKind::Removed => count.removed += 1,
            LineKind::Context => {}
        }
    }
    count
}

/// The scope figures of a group of attempts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeGroup {
    /// Share of attempts that touched at least one file the real fix touched.
    pub found_place: Rate,
    /// Share of attempts that touched a file the real fix did not.
    pub extra_files: Rate,
    /// Share of attempts that left out a file the real fix touched.
    pub missed_files: Rate,
    /// Middle size ratio; `None` when no attempt in the group has one.
    pub median_size_ratio: Option<f64>,
    /// Share of attempts more than [`OVERSIZE_RATIO`] times the size of the real fix.
    pub oversized: Rate,
}

/// One model's scope figures: every attempt that handed in a code change, then the same split
/// by whether the hidden tests passed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeSummary {
    pub all: ScopeGroup,
    pub resolved: ScopeGroup,
    pub not_resolved: ScopeGroup,
}

/// Summarises one model's attempts; each item is (whether the attempt resolved, its scope).
pub fn summarise(attempts: &[(bool, &ScopeDiff)]) -> ScopeSummary {
    let only = |wanted: bool| -> Vec<&ScopeDiff> {
        attempts
            .iter()
            .filter(|(resolved, _)| *resolved == wanted)
            .map(|(_, scope)| *scope)
            .collect()
    };
    let all: Vec<&ScopeDiff> = attempts.iter().map(|(_, scope)| *scope).collect();
    ScopeSummary {
        all: group(&all),
        resolved: group(&only(true)),
        not_resolved: group(&only(false)),
    }
}

fn group(scopes: &[&ScopeDiff]) -> ScopeGroup {
    let n = scopes.len() as u32;
    let share = |holds: fn(&ScopeDiff) -> bool| {
        rate_of(scopes.iter().filter(|s| holds(s)).count() as u32, n)
    };
    let ratios: Vec<f64> = scopes.iter().filter_map(|s| s.size_ratio).collect();
    ScopeGroup {
        found_place: share(|s| s.found_place),
        extra_files: share(|s| !s.extra.is_empty()),
        missed_files: share(|s| !s.missed.is_empty()),
        median_size_ratio: stats::median(&ratios),
        oversized: share(ScopeDiff::is_oversized),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified;
    use crate::patch::diff_texts;

    /// A patch that replaces `old_lines` lines of `path` with `new_lines` lines.
    fn edit(path: &str, old_lines: usize, new_lines: usize) -> String {
        let text = |n: usize, word: &str| -> String {
            (0..n).map(|i| format!("{word} {i}\n")).collect::<String>() + "end\n"
        };
        diff_texts(path, Some(&text(old_lines, "old")), &text(new_lines, "new"))
    }

    fn files(patch: &str) -> Vec<FileDiff> {
        parse_unified(patch).expect("valid diff")
    }

    #[test]
    fn files_are_sorted_into_shared_extra_and_missed() {
        let model = files(&(edit("src/a.rs", 1, 1) + &edit("src/extra.rs", 1, 1)));
        let fix = files(&(edit("src/a.rs", 1, 1) + &edit("src/missed.rs", 1, 1)));
        let scope = compare(&model, &fix);
        assert!(scope.found_place);
        assert_eq!(scope.shared, vec!["src/a.rs"]);
        assert_eq!(scope.extra, vec!["src/extra.rs"]);
        assert_eq!(scope.missed, vec!["src/missed.rs"]);
        assert!(scope.build_files.is_empty());
    }

    #[test]
    fn the_size_ratio_is_changed_lines_over_the_real_fix() {
        // The real fix swaps 1 line for 1 (2 changed); the model swaps 2 for 6 (8 changed).
        let scope = compare(
            &files(&edit("src/a.rs", 2, 6)),
            &files(&edit("src/a.rs", 1, 1)),
        );
        let two_for_six = LineCount {
            added: 6,
            removed: 2,
        };
        assert_eq!(scope.model_lines, two_for_six);
        assert_eq!(scope.fix_lines.total(), 2);
        assert_eq!(scope.size_ratio, Some(4.0));
        assert!(scope.is_oversized());
        let same = compare(
            &files(&edit("src/a.rs", 1, 1)),
            &files(&edit("src/a.rs", 1, 1)),
        );
        assert_eq!(same.size_ratio, Some(1.0));
        assert!(!same.is_oversized());
    }

    #[test]
    fn a_change_elsewhere_did_not_find_the_place() {
        let scope = compare(
            &files(&edit("src/b.rs", 1, 1)),
            &files(&edit("src/a.rs", 1, 1)),
        );
        assert!(!scope.found_place);
        assert!(scope.shared.is_empty());
        assert_eq!(compare(&[], &[]).size_ratio, None);
    }

    #[test]
    fn lock_file_churn_is_listed_and_not_compared() {
        let fix = files(&edit("src/a.rs", 1, 1));
        let with_lock = files(&(edit("src/a.rs", 1, 1) + &edit("Cargo.lock", 100, 100)));
        let scope = compare(&with_lock, &fix);
        assert!(scope.extra.is_empty(), "{scope:?}");
        assert_eq!(scope.build_files, vec!["Cargo.lock"]);
        assert_eq!(scope.size_ratio, Some(1.0));
        assert!(!scope.is_oversized() && scope.has_model_change());
        // A real fix that updates its lock file is not missed by a model that does not.
        let fix_with_lock = files(&(edit("src/a.rs", 1, 1) + &edit("oss/Cargo.lock", 3, 3)));
        let scope = compare(&files(&edit("src/a.rs", 1, 1)), &fix_with_lock);
        assert!(scope.missed.is_empty(), "{scope:?}");
        assert_eq!(scope.fix_lines.total(), 2);
        // Nothing but lock churn is no change of the model's.
        let only_lock = compare(&files(&edit("Cargo.lock", 9, 9)), &fix);
        assert!(!only_lock.has_model_change());
        assert_eq!(only_lock.size_ratio, None);
    }

    #[test]
    fn a_file_the_model_also_renamed_is_still_the_right_place() {
        let renamed = "diff --git a/src/a.rs b/src/b.rs\nsimilarity index 90%\nrename from src/a.rs\nrename to src/b.rs\n--- a/src/a.rs\n+++ b/src/b.rs\n@@ -1,2 +1,2 @@\n-old 0\n+new 0\n end\n";
        let scope = compare(&files(renamed), &files(&edit("src/a.rs", 1, 1)));
        assert!(scope.found_place, "{scope:?}");
        assert_eq!(scope.shared, vec!["src/a.rs"]);
        assert!(scope.extra.is_empty() && scope.missed.is_empty());
        assert_eq!(scope.size_ratio, Some(1.0));
    }

    #[test]
    fn a_change_without_counted_lines_has_no_ratio() {
        let binary = "diff --git a/assets/logo.png b/assets/logo.png\nindex 111..222 100644\nGIT binary patch\nliteral 4\nLcmZQzWMT#Y01f~L\n\nliteral 0\nHcmV?d00001\n\n";
        let scope = compare(&files(binary), &files(&edit("src/a.rs", 1, 1)));
        assert_eq!(scope.size_ratio, None);
        assert_eq!(scope.extra, vec!["assets/logo.png"]);
        assert!(scope.has_model_change() && !scope.is_oversized());
    }

    #[test]
    fn the_summary_splits_by_resolved() {
        let fix = files(&edit("src/a.rs", 1, 1));
        let same = compare(&files(&edit("src/a.rs", 1, 1)), &fix);
        let big = compare(&files(&edit("src/a.rs", 2, 6)), &fix);
        let elsewhere = compare(&files(&edit("src/b.rs", 1, 1)), &fix);
        let summary = summarise(&[(true, &same), (true, &big), (false, &elsewhere)]);
        assert_eq!(summary.all.found_place, rate_of(2, 3));
        assert_eq!(summary.resolved.found_place, rate_of(2, 2));
        assert_eq!(summary.resolved.median_size_ratio, Some(2.5));
        assert_eq!(summary.resolved.oversized, rate_of(1, 2));
        assert_eq!(summary.not_resolved.found_place, rate_of(0, 1));
        assert_eq!(summary.not_resolved.extra_files, rate_of(1, 1));
        assert_eq!(summary.not_resolved.missed_files, rate_of(1, 1));
        assert_eq!(summarise(&[]).all.median_size_ratio, None);
    }
}
