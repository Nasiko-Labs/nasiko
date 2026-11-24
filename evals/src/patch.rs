//! Writing and applying one file's unified diff in process, with the `a/<path>` / `b/<path>`
//! headers `git apply` expects. The server has no git CLI, and the CLI no longer needs temp
//! files for this.

/// Why a diff could not be applied.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PatchError {
    #[error("not a unified diff: {0}")]
    Parse(String),
    #[error("the diff does not apply: {0}")]
    Apply(String),
}

/// A unified diff of one file from `old` (`None` = the file is new) to `new`, as `git diff`
/// writes it. Empty when the two are the same.
pub fn diff_texts(path: &str, old: Option<&str>, new: &str) -> String {
    let before = old.unwrap_or("");
    if old.is_some() && before == new {
        return String::new();
    }
    let body = diffy::create_patch(before, new).to_string();
    // diffy names the sides `original` / `modified`; keep only its hunks.
    let hunks: String = body
        .lines()
        .skip_while(|l| !l.starts_with("@@"))
        .map(|l| format!("{l}\n"))
        .collect();
    let old_side = if old.is_some() {
        format!("a/{path}")
    } else {
        "/dev/null".to_string()
    };
    let mode = if old.is_some() {
        String::new()
    } else {
        "new file mode 100644\n".to_string()
    };
    format!("diff --git a/{path} b/{path}\n{mode}--- {old_side}\n+++ b/{path}\n{hunks}")
}

/// `base` with one file's unified diff applied: the file as the diff leaves it.
pub fn apply_to_text(base: &str, file_diff: &str) -> Result<String, PatchError> {
    // diffy reads from the `---` line on; git's `diff --git` / `index` / mode lines come before.
    let from_header: String = file_diff
        .lines()
        .skip_while(|l| !l.starts_with("--- "))
        .map(|l| format!("{l}\n"))
        .collect();
    let patch =
        diffy::Patch::from_str(&from_header).map_err(|e| PatchError::Parse(e.to_string()))?;
    diffy::apply(base, &patch).map_err(|e| PatchError::Apply(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_repo_relative_headers() {
        let diff = diff_texts("src/a.rs", Some("fn a() {}\n"), "fn a() { 1; }\n");
        assert!(
            diff.starts_with(
                "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@"
            )
        );
        assert!(diff.contains("-fn a() {}\n+fn a() { 1; }\n"), "{diff}");
    }

    #[test]
    fn a_new_file_comes_from_dev_null() {
        let diff = diff_texts("src/new.rs", None, "fn n() {}\n");
        assert!(diff.contains("new file mode 100644\n--- /dev/null\n+++ b/src/new.rs\n"));
        assert!(diff.contains("+fn n() {}"));
    }

    #[test]
    fn identical_texts_give_no_diff() {
        assert_eq!(diff_texts("a.rs", Some("x\n"), "x\n"), "");
    }

    #[test]
    fn apply_returns_the_changed_file() {
        let diff = diff_texts("src/lib.rs", Some("fn a() {}\n"), "fn b() {}\n");
        assert_eq!(
            apply_to_text("fn a() {}\n", &diff).expect("applies"),
            "fn b() {}\n"
        );
        assert!(apply_to_text("fn other() {}\n", &diff).is_err());
    }

    #[test]
    fn applies_what_git_writes() {
        // `git diff` output: an index line and function context after the hunk header.
        let git = "diff --git a/src/lib.rs b/src/lib.rs\nindex 1111111..2222222 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@ mod m {\n fn a() {}\n-fn b() {}\n+fn c() {}\n fn d() {}\n";
        let base = "fn a() {}\nfn b() {}\nfn d() {}\n";
        assert_eq!(
            apply_to_text(base, git).expect("applies"),
            "fn a() {}\nfn c() {}\nfn d() {}\n"
        );
    }

    #[test]
    fn round_trips_a_file_without_a_final_newline() {
        let old = "a\nb";
        let new = "a\nc";
        let diff = diff_texts("f.txt", Some(old), new);
        assert_eq!(apply_to_text(old, &diff).expect("applies"), new);
    }
}
