//! Keeps the shape of source code and drops the inside of long function bodies.
//!
//! Everything outside a function body survives verbatim: imports, signatures, decorators and
//! attributes, class/struct/impl headers, fields, module constants. Inside a body, the first
//! [`crate::Level::code_body_keep`] non-blank lines stay and the rest becomes one counted marker.
//! The model keeps the file's map — what exists, where, with which signature — and can recover a
//! body it needs through the marker's handle.
//!
//! Two readers, no parser:
//!
//! * **indentation**, for Python: a `def` body is every following line indented deeper;
//! * **brace depth**, for Rust, JS/TS, Go, Java, C/C++: a function body runs from its `{` to the
//!   matching `}`, skipping braces inside strings, char literals and comments.
//!
//! Brace counting can still be fooled (a `{` inside a multi-character single-quoted JS string).
//! The guard is the file as a whole: if depth ever goes negative or does not end at zero, the
//! reader is confused and nothing is elided. A partial file view that starts or ends inside a
//! function fails that guard too, and is left alone rather than guessed at.
//!
//! Kept lines are byte-identical to the input, line-number prefixes included, so a model editing
//! the file still has exact line numbers for everything it can see.

use crate::marker;
use crate::policy::Policy;
use crate::text;

/// A body is trimmed only when at least this many lines would go — fewer, and the marker line
/// costs about what it saves.
const MIN_ELIDED: usize = 4;

/// How many lines a signature may span (`fn f(\n a,\n b,\n) -> T {`).
const MAX_SIGNATURE_LINES: usize = 12;

/// First words that open a block which is not a function body.
const NOT_FUNCTIONS: [&str; 28] = [
    "if",
    "else",
    "for",
    "while",
    "switch",
    "catch",
    "do",
    "try",
    "match",
    "loop",
    "return",
    "new",
    "case",
    "when",
    "synchronized",
    "using",
    "lock",
    "foreach",
    "with",
    "finally",
    "class",
    "struct",
    "enum",
    "interface",
    "impl",
    "trait",
    "namespace",
    "mod",
];

pub(super) fn compress(input: &str, policy: &Policy<'_>) -> Option<String> {
    let (prefixes, bodies): (Vec<&str>, Vec<&str>) =
        text::numbered_lines(input).into_iter().unzip();
    let keep = policy.level.code_body_keep();
    let regions = if is_indent_language(&bodies) {
        indent_regions(&bodies, keep)
    } else {
        brace_regions(&bodies, keep)?
    };
    if regions.is_empty() {
        return None;
    }
    Some(render(input, &prefixes, &bodies, &regions, policy))
}

// ── Indentation reader (Python) ─────────────────────────────────────────────────────────────────

/// Python when it has `def`/`class` headers ending in `:` and is not semicolon-terminated.
fn is_indent_language(bodies: &[&str]) -> bool {
    let (mut headers, mut semicolons, mut non_blank) = (0usize, 0usize, 0usize);
    for body in bodies {
        let code = strip_hash_comment(body).trim();
        if code.is_empty() {
            continue;
        }
        non_blank += 1;
        if is_py_header(code) && code.ends_with(':') {
            headers += 1;
        }
        if code.ends_with(';') {
            semicolons += 1;
        }
    }
    headers > 0 && semicolons * 4 < non_blank
}

fn is_py_header(code: &str) -> bool {
    code.starts_with("def ") || code.starts_with("async def ") || code.starts_with("class ")
}

fn is_py_def(code: &str) -> bool {
    code.starts_with("def ") || code.starts_with("async def ")
}

/// Text before a `#` comment. Only used to find a signature's closing `:`, so a `#` inside a
/// string default merely makes that one signature unrecognised.
fn strip_hash_comment(line: &str) -> &str {
    line.split_once('#').map_or(line, |(code, _)| code)
}

fn indent_width(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

fn indent_regions(bodies: &[&str], keep: usize) -> Vec<(usize, usize)> {
    let mut regions = Vec::new();
    let n = bodies.len();
    let mut i = 0;
    while i < n {
        if !is_py_def(bodies[i].trim_start()) {
            i += 1;
            continue;
        }
        let head_indent = indent_width(bodies[i]);
        let Some(sig_end) = (i..n.min(i + MAX_SIGNATURE_LINES))
            .find(|&j| strip_hash_comment(bodies[j]).trim_end().ends_with(':'))
        else {
            i += 1;
            continue;
        };
        let mut last = sig_end;
        for (j, body) in bodies.iter().enumerate().skip(sig_end + 1) {
            if body.trim().is_empty() {
                continue;
            }
            if indent_width(body) <= head_indent {
                break;
            }
            last = j;
        }
        push_trim(&mut regions, bodies, sig_end + 1, last, keep);
        i = last + 1;
    }
    regions
}

// ── Brace reader (Rust, JS/TS, Go, Java, C/C++) ─────────────────────────────────────────────────

/// Per line: brace depth before and after it, and its code with strings and comments removed.
struct Scan {
    before: Vec<i64>,
    after: Vec<i64>,
    code: Vec<String>,
}

/// `None` when the braces do not balance — the reader is confused, or the view is partial.
fn scan(bodies: &[&str]) -> Option<Scan> {
    let mut out = Scan {
        before: Vec::with_capacity(bodies.len()),
        after: Vec::with_capacity(bodies.len()),
        code: Vec::with_capacity(bodies.len()),
    };
    let (mut depth, mut in_block_comment, mut in_template) = (0i64, false, false);
    for body in bodies {
        let before = depth;
        let chars: Vec<char> = body.chars().collect();
        let mut code = String::new();
        let mut k = 0;
        while k < chars.len() {
            let c = chars[k];
            let next = chars.get(k + 1).copied();
            if in_block_comment {
                if c == '*' && next == Some('/') {
                    in_block_comment = false;
                    k += 2;
                } else {
                    k += 1;
                }
                continue;
            }
            if in_template {
                match c {
                    '\\' => k += 2,
                    '`' => {
                        in_template = false;
                        k += 1;
                    }
                    _ => k += 1,
                }
                continue;
            }
            match c {
                '/' if next == Some('/') => break,
                '/' if next == Some('*') => {
                    in_block_comment = true;
                    k += 2;
                    continue;
                }
                '`' => {
                    in_template = true;
                    k += 1;
                    continue;
                }
                '"' => {
                    k = string_end(&chars, k);
                    continue;
                }
                '\'' => {
                    if let Some(end) = char_literal_end(&chars, k) {
                        k = end;
                        continue;
                    }
                }
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth < 0 {
                        return None;
                    }
                }
                _ => {}
            }
            code.push(c);
            k += 1;
        }
        out.before.push(before);
        out.after.push(depth);
        out.code.push(code);
    }
    (depth == 0 && !in_block_comment && !in_template).then_some(out)
}

/// Index just past the closing quote of a double-quoted string starting at `start`, or the end
/// of the line for an unterminated one.
fn string_end(chars: &[char], start: usize) -> usize {
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '"' => return i + 1,
            _ => i += 1,
        }
    }
    chars.len()
}

/// `'x'` or `'\n'`/`'\u{1F600}'` — but not a Rust lifetime (`'a`) or an apostrophe, which have
/// no closing quote and must not swallow the rest of the line.
fn char_literal_end(chars: &[char], start: usize) -> Option<usize> {
    match chars.get(start + 1) {
        Some('\\') => (start + 3..chars.len().min(start + 12))
            .find(|&i| chars[i] == '\'')
            .map(|i| i + 1),
        Some(_) if chars.get(start + 2) == Some(&'\'') => Some(start + 3),
        _ => None,
    }
}

fn brace_regions(bodies: &[&str], keep: usize) -> Option<Vec<(usize, usize)>> {
    let scan = scan(bodies)?;
    // `Ok(v) => {` is a Rust match arm, not a JS arrow function.
    let arrows_are_functions = !bodies.iter().any(|b| {
        let t = b.trim_start();
        t.starts_with("fn ") || t.starts_with("pub fn ") || t.starts_with("pub(crate) fn ")
    });

    let mut regions = Vec::new();
    let n = bodies.len();
    let mut i = 0;
    while i < n {
        let opens_block = scan.code[i].trim_end().ends_with('{') && scan.after[i] > scan.before[i];
        if !opens_block || !is_function_signature(&signature(&scan.code, i), arrows_are_functions) {
            i += 1;
            continue;
        }
        let outside = scan.after[i] - 1;
        let Some(close) = (i + 1..n).find(|&k| scan.after[k] <= outside) else {
            i += 1;
            continue;
        };
        if close > i + 1 {
            push_trim(&mut regions, bodies, i + 1, close - 1, keep);
        }
        i = close + 1;
    }
    Some(regions)
}

/// The statement ending at line `end`, joined across the lines a multi-line signature spans.
fn signature(code: &[String], end: usize) -> String {
    let mut start = end;
    while start > 0 && end - start < MAX_SIGNATURE_LINES {
        let prev = code[start - 1].trim();
        if prev.is_empty() || prev.ends_with(';') || prev.ends_with('{') || prev.ends_with('}') {
            break;
        }
        start -= 1;
    }
    code[start..=end]
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_function_signature(sig: &str, arrows_are_functions: bool) -> bool {
    let sig = sig.trim();
    if sig.starts_with('}') {
        return false;
    }
    let tokens: Vec<&str> = sig
        .split(|c: char| c.is_whitespace() || c == '(')
        .filter(|t| !t.is_empty())
        .collect();
    if tokens
        .iter()
        .any(|t| *t == "fn" || *t == "func" || t.starts_with("function"))
    {
        return true;
    }
    if arrows_are_functions && sig.ends_with("=> {") {
        return true;
    }
    let Some(first) = tokens.first() else {
        return false;
    };
    if NOT_FUNCTIONS.contains(first) {
        return false;
    }
    // C-family method or function: `name(args) [qualifiers] {`. A trailing call with an object
    // literal (`foo(a, {`) has no `)` after its last `(`, and an assignment has `=` outside it.
    let Some((head, tail)) = sig.rsplit_once(')') else {
        return false;
    };
    let tail = tail.trim_end_matches('{');
    head.contains('(') && !tail.contains('=') && !tail.contains('(') && !tail.contains(';')
}

// ── Shared ──────────────────────────────────────────────────────────────────────────────────────

/// Keep the first `keep` non-blank lines of `start..=end`; record the rest as one elision when
/// it is long enough to be worth a marker.
fn push_trim(
    regions: &mut Vec<(usize, usize)>,
    bodies: &[&str],
    start: usize,
    end: usize,
    keep: usize,
) {
    if end < start {
        return;
    }
    let mut kept = 0;
    let mut cut = None;
    for (k, body) in bodies.iter().enumerate().take(end + 1).skip(start) {
        if kept == keep {
            cut = Some(k);
            break;
        }
        if !body.trim().is_empty() {
            kept += 1;
        }
    }
    if let Some(cut) = cut
        && end + 1 - cut >= MIN_ELIDED
    {
        regions.push((cut, end));
    }
}

fn render(
    input: &str,
    prefixes: &[&str],
    bodies: &[&str],
    regions: &[(usize, usize)],
    policy: &Policy<'_>,
) -> String {
    let raw: Vec<&str> = input.lines().collect();
    let mut out = String::with_capacity(input.len());
    let mut pending = regions.iter().peekable();
    let mut i = 0;
    while i < raw.len() {
        if let Some(&&(start, end)) = pending.peek()
            && i == start
        {
            let indent: String = bodies[start]
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();
            out.push_str(&indent);
            out.push_str(&elision_marker(prefixes, start, end, policy));
            out.push('\n');
            pending.next();
            i = end + 1;
            continue;
        }
        out.push_str(raw[i]);
        out.push('\n');
        i += 1;
    }
    if !input.ends_with('\n') {
        out.pop();
    }
    out
}

/// A numbered view names the cut lines by the view's own numbers; anything else gets a count.
fn elision_marker(prefixes: &[&str], start: usize, end: usize, policy: &Policy<'_>) -> String {
    match (
        text::line_number(prefixes[start]),
        text::line_number(prefixes[end]),
    ) {
        (Some(first), Some(last)) => marker::elided_line_range(first, last, policy.recovery_ref),
        _ => marker::elided_lines(end + 1 - start, policy.recovery_ref),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_type::TypeMask;
    use crate::policy::Level;

    fn policy(level: Level) -> Policy<'static> {
        Policy {
            enabled: true,
            min_bytes: 0,
            types: TypeMask::ALL,
            level,
            ..Default::default()
        }
    }

    fn python() -> String {
        let mut s = String::from(
            "import os\nfrom typing import Any\n\n\n@cache\ndef charge(order: Any) -> int:\n    \"\"\"Charge the card for an order.\"\"\"\n    if not order:\n        raise ValueError(\"empty\")\n",
        );
        for i in 0..20 {
            s.push_str(&format!("    total_{i} = order.line({i}) * 2\n"));
        }
        s.push_str(
            "    return total_0\n\n\nclass Ledger:\n    rate = 3\n\n    def post(self, entry):\n",
        );
        for i in 0..12 {
            s.push_str(&format!("        self.rows.append(entry + {i})\n"));
        }
        s.push_str("\n    def size(self):\n        return len(self.rows)\n");
        s
    }

    #[test]
    fn python_keeps_the_map_and_trims_long_bodies() {
        let input = python();
        let out = compress(&input, &policy(Level::Balanced)).unwrap();

        assert!(out.len() < input.len());
        for kept in [
            "import os",
            "from typing import Any",
            "@cache",
            "def charge(order: Any) -> int:",
            "\"\"\"Charge the card for an order.\"\"\"",
            "class Ledger:",
            "    rate = 3",
            "    def post(self, entry):",
            "    def size(self):",
            "        return len(self.rows)",
        ] {
            assert!(out.contains(kept), "lost {kept:?}:\n{out}");
        }
        assert!(!out.contains("total_19"), "long body not trimmed:\n{out}");
        assert!(
            !out.contains("entry + 11"),
            "method body not trimmed:\n{out}"
        );
        // 24 body lines, 3 kept at Balanced.
        assert!(
            out.contains("    [⋯ 21 lines elided ⋯]"),
            "marker missing or mis-indented:\n{out}"
        );
    }

    #[test]
    fn short_bodies_are_left_alone() {
        let input = "def a():\n    return 1\n\ndef b(x):\n    y = x\n    return y\n";
        assert_eq!(compress(input, &policy(Level::Aggressive)), None);
    }

    #[test]
    fn levels_keep_different_amounts() {
        let input = python();
        let conservative = compress(&input, &policy(Level::Conservative)).unwrap();
        let aggressive = compress(&input, &policy(Level::Aggressive)).unwrap();
        assert!(aggressive.len() < conservative.len());
        assert!(conservative.contains("total_2 ="));
        assert!(!aggressive.contains("total_2 ="));
    }

    fn rust() -> String {
        let mut s = String::from(
            "use std::fmt;\n\n/// Docs.\n#[derive(Debug)]\npub struct Order<'a> {\n    id: &'a str,\n}\n\nimpl<'a> Order<'a> {\n    pub fn total(&self, rates: &[u32]) -> u32 {\n        let label = \"{not a brace}\";\n        let c = '{';\n",
        );
        for i in 0..15 {
            s.push_str(&format!(
                "        let v{i} = rates[{i}] * 2; // }} in a comment\n"
            ));
        }
        s.push_str("        v0\n    }\n}\n\nfn helper<'b>(\n    a: &'b str,\n    b: u32,\n) -> &'b str {\n");
        for i in 0..8 {
            s.push_str(&format!("    let _x{i} = b + {i};\n"));
        }
        s.push_str("    if b > 0 {\n        return a;\n    }\n    a\n}\n");
        s
    }

    #[test]
    fn brace_languages_keep_signatures_and_ignore_braces_in_strings_and_comments() {
        let input = rust();
        let out = compress(&input, &policy(Level::Balanced)).unwrap();

        for kept in [
            "use std::fmt;",
            "#[derive(Debug)]",
            "pub struct Order<'a> {",
            "    id: &'a str,",
            "impl<'a> Order<'a> {",
            "    pub fn total(&self, rates: &[u32]) -> u32 {",
            "fn helper<'b>(",
            ") -> &'b str {",
        ] {
            assert!(out.contains(kept), "lost {kept:?}:\n{out}");
        }
        assert!(!out.contains("let v14"), "method body not trimmed:\n{out}");
        assert!(
            !out.contains("let _x7"),
            "multi-line-signature body not trimmed:\n{out}"
        );
        // The closing braces survive, so the kept outline still balances.
        assert_eq!(out.matches("\n}").count(), input.matches("\n}").count());
    }

    #[test]
    fn javascript_methods_and_arrow_functions() {
        let mut input =
            String::from("import x from 'y';\n\nexport class Api {\n  async fetch(url) {\n");
        for i in 0..10 {
            input.push_str(&format!("    const r{i} = await get(url + {i});\n"));
        }
        input.push_str("    return r0;\n  }\n}\n\nconst handler = (req) => {\n");
        for i in 0..10 {
            input.push_str(&format!("  log(`req ${{req.id}} step {i}`);\n"));
        }
        input.push_str("};\n");
        let out = compress(&input, &policy(Level::Balanced)).unwrap();
        assert!(out.contains("  async fetch(url) {"));
        assert!(out.contains("const handler = (req) => {"));
        assert!(!out.contains("const r9"));
        assert!(!out.contains("step 9"));
    }

    #[test]
    fn control_blocks_are_not_mistaken_for_functions() {
        let mut input = String::from("int main(void) {\n");
        for i in 0..3 {
            input.push_str(&format!("    step({i});\n"));
        }
        input.push_str("}\n\nif (ready) {\n");
        for i in 0..10 {
            input.push_str(&format!("    go({i});\n"));
        }
        input.push_str("}\n");
        // main's body is too short to trim; the `if` block is not a function at all.
        assert_eq!(compress(&input, &policy(Level::Aggressive)), None);
    }

    #[test]
    fn unbalanced_or_partial_views_are_left_alone() {
        let partial = "    pub fn total(&self) -> u32 {\n        let a = 1;\n        let b = 2;\n        let c = 3;\n        let d = 4;\n        let e = 5;\n        a\n    }\n}\n";
        assert_eq!(compress(partial, &policy(Level::Aggressive)), None);
    }

    #[test]
    fn numbered_file_views_keep_their_line_numbers() {
        let numbered: String = python()
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{:05}| {l}\n", i + 1))
            .collect();
        let out = compress(&numbered, &policy(Level::Balanced)).unwrap();
        assert!(out.contains("00006| def charge(order: Any) -> int:"));
        assert!(out.contains("00001| import os"));
        assert!(!out.contains("total_19"));
        // Every kept line is verbatim from the input.
        for line in out.lines().filter(|l| !l.contains('⋯')) {
            assert!(numbered.contains(line), "rewrote {line:?}");
        }
    }

    #[test]
    fn numbered_views_name_the_cut_lines_by_their_own_numbers() {
        let numbered: String = python()
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{:>6}\t{l}\n", i + 1900))
            .collect();
        let out = compress(&numbered, &policy(Level::Balanced)).unwrap();
        let marker = out.lines().find(|l| l.contains('⋯')).unwrap();
        assert!(
            marker.contains("lines 19") && marker.contains('–'),
            "marker must name the cut range: {marker}"
        );
        assert!(!marker.contains("lines elided"), "{marker}");
    }

    #[test]
    fn markers_carry_the_recovery_handle() {
        let p = Policy {
            recovery_ref: Some("h-42"),
            ..policy(Level::Balanced)
        };
        let out = compress(&python(), &p).unwrap();
        assert!(out.contains("recover: h-42"));
    }

    #[test]
    fn is_deterministic() {
        let input = rust();
        assert_eq!(
            compress(&input, &policy(Level::Balanced)),
            compress(&input, &policy(Level::Balanced))
        );
    }
}
