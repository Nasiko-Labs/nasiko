//! Separating a Rust file's inline test module (`#[cfg(test)] mod tests { … }`) from its code.
//!
//! Rust keeps unit tests in the same file as the code, so a commit that changes both cannot be
//! split into "fix" and "tests" by file. Instead, for a file changed from `base` to `gold`, we
//! build a middle version: `gold` with its test module swapped back to `base`'s. Then
//! `base → middle` is the fix and `middle → gold` is the test change.

use std::ops::Range;

/// The middle version of a file: `gold`'s code with `base`'s test module (or none, when `base`
/// had none). `None` when the file cannot be split: `gold` has no inline test module, more
/// than one, or `base` has more than one.
pub fn code_with_old_tests(base: &str, gold: &str) -> Option<String> {
    let gold_spans = test_module_spans(gold);
    let [gold_span] = gold_spans.as_slice() else {
        return None;
    };
    let base_tests = match test_module_spans(base).as_slice() {
        [] => "",
        [span] => &base[span.clone()],
        _ => return None,
    };
    let mut middle = String::with_capacity(gold.len());
    middle.push_str(&gold[..gold_span.start]);
    middle.push_str(base_tests);
    let rest = &gold[gold_span.end..];
    if base_tests.is_empty() {
        // Drop the blank line that separated the code from the removed module.
        middle.truncate(middle.trim_end().len());
        middle.push('\n');
        middle.push_str(rest.trim_start_matches('\n'));
    } else {
        middle.push_str(rest);
    }
    Some(middle)
}

/// Byte ranges of every `#[cfg(test)] mod name { … }`, from the attribute line to the closing
/// brace's line end. `#[cfg(test)] mod tests;` (tests in another file) is not counted.
pub fn test_module_spans(src: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut search_from = 0;
    while let Some(found) = src[search_from..].find("#[cfg(test)]") {
        let attr = search_from + found;
        search_from = attr + 1;
        let line_start = src[..attr].rfind('\n').map_or(0, |i| i + 1);
        if !src[line_start..attr].trim().is_empty() || in_comment_or_string(src, attr) {
            continue;
        }
        let Some(open) = module_open_brace(src, attr + "#[cfg(test)]".len()) else {
            continue;
        };
        let Some(close) = matching_brace(src, open) else {
            continue;
        };
        let end = src[close..].find('\n').map_or(src.len(), |i| close + i + 1);
        spans.push(line_start..end);
        search_from = end;
    }
    spans
}

/// After the attribute, skips whitespace and further attributes, expects `mod <name>` (maybe
/// `pub` / `pub(crate)`), and returns the index of its `{`.
fn module_open_brace(src: &str, mut at: usize) -> Option<usize> {
    loop {
        let rest = &src[at..];
        let trimmed = rest.trim_start();
        at += rest.len() - trimmed.len();
        if trimmed.starts_with("#[") {
            at += trimmed.find(']')? + 1;
        } else {
            break;
        }
    }
    let rest = &src[at..];
    let rest = rest
        .strip_prefix("pub(crate) ")
        .or_else(|| rest.strip_prefix("pub(super) "))
        .or_else(|| rest.strip_prefix("pub "))
        .unwrap_or(rest);
    let after_mod = rest.strip_prefix("mod ")?;
    let name_len = after_mod
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(after_mod.len());
    let after_name = after_mod[name_len..].trim_start();
    if !after_name.starts_with('{') || name_len == 0 {
        return None;
    }
    Some(src.len() - after_name.len())
}

/// Index of the `}` closing the `{` at `open`, skipping braces inside comments and literals.
fn matching_brace(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {
                if let Some(skip) = skip_non_code(src, i) {
                    i = skip;
                    continue;
                }
            }
        }
        i += 1;
    }
    None
}

/// When a comment or literal starts at `i`, the index just after it.
fn skip_non_code(src: &str, i: usize) -> Option<usize> {
    let rest = &src[i..];
    if rest.starts_with("//") {
        return Some(rest.find('\n').map_or(src.len(), |n| i + n));
    }
    if rest.starts_with("/*") {
        return Some(block_comment_end(src, i));
    }
    if let Some(end) = raw_string_end(src, i) {
        return Some(end);
    }
    let b = rest.as_bytes();
    if b[0] == b'"' {
        return Some(quoted_end(src, i, b'"'));
    }
    if b[0] == b'\'' && is_char_literal(rest) {
        return Some(quoted_end(src, i, b'\''));
    }
    None
}

fn block_comment_end(src: &str, start: usize) -> usize {
    let bytes = src.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i + 1 < bytes.len() {
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            depth += 1;
            i += 2;
        } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    src.len()
}

/// `r"…"`, `r#"…"#`, `br"…"` and so on.
fn raw_string_end(src: &str, i: usize) -> Option<usize> {
    let rest = &src[i..];
    let after_prefix = rest.strip_prefix("br").or_else(|| rest.strip_prefix('r'))?;
    if i > 0 && src.as_bytes()[i - 1].is_ascii_alphanumeric() {
        return None;
    }
    let hashes = after_prefix.len() - after_prefix.trim_start_matches('#').len();
    let body = after_prefix[hashes..].strip_prefix('"')?;
    let closing = format!("\"{}", "#".repeat(hashes));
    let end_in_body = body.find(&closing)?;
    Some(src.len() - body.len() + end_in_body + closing.len())
}

fn quoted_end(src: &str, start: usize, quote: u8) -> usize {
    let bytes = src.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b if b == quote => return i + 1,
            _ => i += 1,
        }
    }
    src.len()
}

/// `'a'`, `'\n'`, `'\u{1F600}'` are char literals; `'a` in `&'a str` is a lifetime.
fn is_char_literal(rest: &str) -> bool {
    let mut chars = rest.chars().skip(1);
    match chars.next() {
        Some('\\') => true,
        Some(_) => chars.next() == Some('\''),
        None => false,
    }
}

/// Whether byte `at` sits inside a comment or string literal (scanning from the file start).
fn in_comment_or_string(src: &str, at: usize) -> bool {
    let mut i = 0;
    while i < at {
        match skip_non_code(src, i) {
            Some(end) if end > at => return true,
            Some(end) => i = end,
            None => i += src[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "fn a() -> u8 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn one() { assert_eq!(super::a(), 1); }\n}\n";
    const GOLD: &str = "fn a() -> u8 {\n    2\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn one() { assert_eq!(super::a(), 2); }\n    // a brace in a comment: }\n    #[test]\n    fn two() { let s = \"}\"; let c = '}'; let _l: &'static str = s; assert!(!s.is_empty() && c == '}'); }\n}\n";

    #[test]
    fn finds_one_module_and_skips_braces_in_literals() {
        let spans = test_module_spans(GOLD);
        assert_eq!(spans.len(), 1);
        assert!(GOLD[spans[0].clone()].starts_with("#[cfg(test)]"));
        assert!(GOLD[spans[0].clone()].ends_with("}\n"));
        assert_eq!(spans[0].end, GOLD.len());
    }

    #[test]
    fn middle_has_new_code_and_old_tests() {
        let middle = code_with_old_tests(BASE, GOLD).expect("splittable");
        assert!(middle.contains("    2\n"), "new code kept");
        assert!(
            middle.contains("assert_eq!(super::a(), 1)"),
            "old test kept"
        );
        assert!(!middle.contains("fn two"), "new test removed");
    }

    #[test]
    fn module_added_by_the_commit_is_removed_from_the_middle() {
        let base = "fn a() {}\n";
        let gold = "fn a() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        assert_eq!(
            code_with_old_tests(base, gold).as_deref(),
            Some("fn a() {}\n")
        );
    }

    #[test]
    fn external_test_files_and_commented_attributes_are_ignored() {
        assert!(test_module_spans("#[cfg(test)]\nmod tests;\n").is_empty());
        assert!(test_module_spans("// #[cfg(test)]\nfn x() {}\n").is_empty());
        assert!(
            test_module_spans("const S: &str = r#\"\n#[cfg(test)]\nmod t {}\n\"#;\n").is_empty()
        );
        assert_eq!(code_with_old_tests("", "fn a() {}\n"), None);
    }
}
