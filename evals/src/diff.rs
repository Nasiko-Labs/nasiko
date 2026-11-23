//! Reading unified diffs (as `git diff` prints them) and splitting test files from code.

use regex::Regex;

/// One file's part of a unified diff.
#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    /// Path before the change; `None` when the file was added.
    pub old_path: Option<String>,
    /// Path after the change; `None` when the file was deleted.
    pub new_path: Option<String>,
    /// The full text of this file's section, starting with `diff --git`.
    pub text: String,
    /// Added lines, without the leading `+`.
    pub added: Vec<String>,
}

impl FileDiff {
    /// The path that best names this file (new path, else old path).
    pub fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or_default()
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DiffError {
    #[error("text before the first `diff --git` line is not a diff: {0:?}")]
    NotADiff(String),
}

const DIFF_HEADER: &str = "diff --git ";

/// Splits `git diff` output into one [`FileDiff`] per file.
pub fn parse_unified(diff: &str) -> Result<Vec<FileDiff>, DiffError> {
    let mut files = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in diff.lines() {
        if line.starts_with(DIFF_HEADER) {
            if let Some(lines) = current.take() {
                files.push(file_from_lines(&lines));
            }
            current = Some(vec![line]);
        } else if let Some(lines) = current.as_mut() {
            lines.push(line);
        } else if !line.trim().is_empty() {
            return Err(DiffError::NotADiff(line.to_string()));
        }
    }
    if let Some(lines) = current {
        files.push(file_from_lines(&lines));
    }
    Ok(files)
}

fn file_from_lines(lines: &[&str]) -> FileDiff {
    let (mut old_path, mut new_path) = header_paths(lines[0]);
    let mut added = Vec::new();
    let mut in_hunk = false;
    for line in &lines[1..] {
        if line.starts_with("@@") {
            in_hunk = true;
        } else if !in_hunk {
            if let Some(p) = line.strip_prefix("--- ") {
                old_path = strip_side(p, "a/");
            } else if let Some(p) = line.strip_prefix("+++ ") {
                new_path = strip_side(p, "b/");
            }
        } else if let Some(text) = line.strip_prefix('+') {
            added.push(text.to_string());
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    FileDiff {
        old_path,
        new_path,
        text,
        added,
    }
}

/// Paths from `diff --git a/x b/y`; only reliable when neither path has spaces, so the
/// `---`/`+++` lines override them when present.
fn header_paths(header: &str) -> (Option<String>, Option<String>) {
    let rest = header.trim_start_matches(DIFF_HEADER);
    match rest.split_once(" b/") {
        Some((a, b)) => (
            Some(a.trim_start_matches("a/").to_string()),
            Some(b.to_string()),
        ),
        None => (None, None),
    }
}

fn strip_side(path: &str, side: &str) -> Option<String> {
    let path = path.trim();
    if path == "/dev/null" {
        return None;
    }
    Some(path.strip_prefix(side).unwrap_or(path).to_string())
}

/// Joins file diffs back into one patch that `git apply` accepts.
pub fn join(files: &[FileDiff]) -> String {
    files.iter().map(|f| f.text.as_str()).collect()
}

/// Which paths count as test files, written as globs (`**/tests/**`, `**/*_test.rs`).
#[derive(Debug, Clone)]
pub struct TestPathRules {
    patterns: Vec<Regex>,
}

impl TestPathRules {
    /// Builds the rules from globs. `**` matches any number of folders, `*` anything inside
    /// one path part.
    pub fn from_globs(globs: &[String]) -> Self {
        let patterns = globs.iter().map(|g| glob_to_regex(g)).collect();
        Self { patterns }
    }

    /// Default test-path rules for a Rust workspace.
    pub fn rust_default() -> Self {
        Self::from_globs(&[
            "**/tests/**".to_string(),
            "**/*_test.rs".to_string(),
            "**/*_tests.rs".to_string(),
            "**/tests.rs".to_string(),
        ])
    }

    pub fn is_test(&self, path: &str) -> bool {
        self.patterns.iter().any(|p| p.is_match(path))
    }
}

fn glob_to_regex(glob: &str) -> Regex {
    let mut re = String::from("^");
    let mut chars = glob.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    re.push_str("(?:.*/)?");
                } else {
                    re.push_str(".*");
                }
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    Regex::new(&re).expect("escaped glob is a valid regex")
}

/// A commit's diff split into the real fix and its tests.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SplitPatch {
    pub gold: Vec<FileDiff>,
    pub test: Vec<FileDiff>,
    /// Non-test files whose added lines contain test code, so they cannot be split by file.
    pub mixed: Vec<String>,
}

/// Lines that mark inline test code inside a normal source file.
const INLINE_TEST_MARKERS: [&str; 2] = ["#[cfg(test)]", "#[test]"];

/// Splits files into test files and the rest, and flags inline test code.
pub fn split_tests(files: Vec<FileDiff>, rules: &TestPathRules) -> SplitPatch {
    let mut split = SplitPatch::default();
    for file in files {
        if rules.is_test(file.path()) {
            split.test.push(file);
            continue;
        }
        if file.added.iter().any(|l| {
            INLINE_TEST_MARKERS
                .iter()
                .any(|m| l.trim_start().starts_with(m))
        }) {
            split.mixed.push(file.path().to_string());
        }
        split.gold.push(file);
    }
    split
}

/// Drops test files, e.g. to clean a model's patch before grading.
pub fn without_tests(files: Vec<FileDiff>, rules: &TestPathRules) -> Vec<FileDiff> {
    files
        .into_iter()
        .filter(|f| !rules.is_test(f.path()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 111..222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 fn a() {}
+fn b() {}
+#[cfg(test)]
diff --git a/tests/it.rs b/tests/it.rs
new file mode 100644
--- /dev/null
+++ b/tests/it.rs
@@ -0,0 +1 @@
+#[test] fn t() {}
";

    #[test]
    fn parses_files_paths_and_added_lines() {
        let files = parse_unified(DIFF).expect("valid diff");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path(), "src/lib.rs");
        assert_eq!(files[0].added, vec!["fn b() {}", "#[cfg(test)]"]);
        assert_eq!(files[1].old_path, None);
        assert_eq!(files[1].path(), "tests/it.rs");
        assert_eq!(join(&files), DIFF);
    }

    #[test]
    fn splits_tests_and_flags_inline_test_code() {
        let files = parse_unified(DIFF).expect("valid diff");
        let split = split_tests(files, &TestPathRules::rust_default());
        assert_eq!(split.gold.len(), 1);
        assert_eq!(split.test.len(), 1);
        assert_eq!(split.mixed, vec!["src/lib.rs"]);
    }

    #[test]
    fn globs_match_folders() {
        let rules = TestPathRules::rust_default();
        assert!(rules.is_test("oss/cli/tests/a.rs"));
        assert!(rules.is_test("tests/a.rs"));
        assert!(rules.is_test("oss/x/src/foo_test.rs"));
        assert!(!rules.is_test("oss/x/src/testing.rs"));
    }

    #[test]
    fn rejects_non_diff_text() {
        assert!(parse_unified("hello\n").is_err());
        assert_eq!(parse_unified("").expect("empty is fine"), vec![]);
    }
}
