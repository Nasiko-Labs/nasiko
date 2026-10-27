//! Tool calls that ask for an exact slice of a file.
//!
//! When an agent asks for lines 1900–1960, those lines are the point of the call. Compressing the
//! result cuts the very body the agent asked to read, and the agent then re-reads it in ever
//! smaller pieces until a view slips under the size floor — each extra step re-sending the whole
//! history (the re-read loop in the SWE-bench compression eval, where 90% of compressed code
//! bytes were requested ranges). A requested range is therefore sent whole.
//!
//! The decision reads only the tool call that produced a result. That call never changes once
//! sent, so a result renders to the same bytes on every turn and the provider prefix cache holds.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::ir::chat::{Message, ToolCall};

/// Shell commands whose output is bounded to a line range the caller chose:
/// `sed -n 'A,Bp'`, `head -n N` / `head -N`, `tail -n N` / `tail -N`, `awk 'NR>=A && NR<=B'`.
/// `tail -n +N` is deliberately absent: it prints everything from line N to the end of the file.
static BOUNDED_VIEW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?x)
          \bsed \s+ -n \s+ ['"]? \s* \d+ \s* , \s* \d+ \s* p
        | \bhead \s+ (?: -n \s* \d+ | -\d+ | --lines[=\s]\d+ )
        | \btail \s+ (?: -n \s* \d+ | -\d+ | --lines[=\s]\d+ )
        | \bawk \s+ ['"] \s* NR \s* (?: >= | > | == ) \s* \d+
        "#,
    )
    .expect("BOUNDED_VIEW is a valid regex")
});

/// The ids of every tool call in `messages` that asked for an exact line range.
pub(crate) fn requested_range_call_ids(messages: &[Message]) -> HashSet<String> {
    messages
        .iter()
        .filter_map(|m| m.tool_calls.as_deref())
        .flatten()
        .filter(|call| is_range_view(call))
        .map(|call| call.id.clone())
        .collect()
}

/// A shell call whose last command is a bounded view, or an editor call with a `view_range`.
/// Unparseable arguments count as "not a range": the result is then compressed as before.
fn is_range_view(call: &ToolCall) -> bool {
    let Ok(args) = serde_json::from_str::<Value>(&call.function.arguments) else {
        return false;
    };
    if args.get("view_range").is_some_and(|r| !r.is_null()) {
        return true;
    }
    args.get("command")
        .and_then(Value::as_str)
        .is_some_and(is_bounded_shell_view)
}

/// Only the last command of a `&&`/`;` chain prints the output the agent reads, so earlier
/// commands (`cd repo && …`) are ignored; any stage of that command's pipeline may bound it.
fn is_bounded_shell_view(command: &str) -> bool {
    BOUNDED_VIEW.is_match(last_command(command))
}

/// The last non-empty command of a `&&` / `;` / newline chain. Separators inside quotes are
/// arguments, not separators (`awk 'NR>=10 && NR<=40'`, `sed -n '1,5p;9,12p'`).
fn last_command(command: &str) -> &str {
    let bytes = command.as_bytes();
    let mut quote: Option<u8> = None;
    let (mut start, mut last) = (0, "");
    let mut i = 0;
    while i < bytes.len() {
        let separator_len = match (quote, bytes[i]) {
            (Some(q), b) if b == q => {
                quote = None;
                0
            }
            (Some(_), _) => 0,
            (None, b @ (b'\'' | b'"')) => {
                quote = Some(b);
                0
            }
            (None, b';' | b'\n') => 1,
            (None, b'&') if bytes.get(i + 1) == Some(&b'&') => 2,
            _ => 0,
        };
        if separator_len > 0 {
            let segment = command.get(start..i).unwrap_or_default().trim();
            if !segment.is_empty() {
                last = segment;
            }
            start = i + separator_len;
        }
        i += separator_len.max(1);
    }
    let tail = command.get(start..).unwrap_or_default().trim();
    if tail.is_empty() { last } else { tail }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::chat::FunctionCall;
    use serde_json::json;

    fn call(id: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "bash".into(),
                arguments: arguments.to_string(),
            },
            extra: Default::default(),
        }
    }

    fn shell(command: &str) -> bool {
        is_range_view(&call("c", json!({ "command": command })))
    }

    #[test]
    fn sed_line_ranges_are_requested_ranges() {
        assert!(shell(
            "cd /repo && sed -n '1,140p' django/db/models/enums.py"
        ));
        assert!(shell("nl -ba src/app.py | sed -n '45,100p'"));
        assert!(shell("sed -n 1900,1960p file.py"));
    }

    #[test]
    fn head_tail_and_awk_ranges_are_requested_ranges() {
        assert!(shell("head -n 80 setup.py"));
        assert!(shell("head -40 setup.py"));
        assert!(shell("tail -n 50 build.log"));
        assert!(shell("awk 'NR>=10 && NR<=40' a.py"));
    }

    #[test]
    fn whole_file_reads_and_open_ended_tails_are_not() {
        assert!(!shell("cat django/db/models/enums.py"));
        assert!(!shell("tail -n +1900 file.py"));
        assert!(!shell("python -c 'print(open(\"a.py\").read())'"));
    }

    #[test]
    fn only_the_last_command_of_a_chain_counts() {
        assert!(!shell("sed -n '1,5p' a.py && cat b.py"));
        assert!(shell("cat b.py > /dev/null; sed -n '1,5p' a.py"));
    }

    #[test]
    fn separators_inside_quotes_do_not_split_the_command() {
        assert!(shell("sed -n '1,5p;9,12p' a.py"));
        assert_eq!(last_command("cd /r && echo 'a && b'"), "echo 'a && b'");
        assert_eq!(last_command("python - <<'PY'\nprint(1)\nPY"), "PY");
    }

    #[test]
    fn editor_view_range_is_a_requested_range() {
        let c = call(
            "c",
            json!({ "command": "view", "path": "a.py", "view_range": [10, 40] }),
        );
        assert!(is_range_view(&c));
        let whole = call("c", json!({ "command": "view", "path": "a.py" }));
        assert!(!is_range_view(&whole));
    }

    #[test]
    fn malformed_arguments_are_not_a_range() {
        let mut c = call("c", json!({}));
        c.function.arguments = "{not json".into();
        assert!(!is_range_view(&c));
    }

    #[test]
    fn collects_ids_across_assistant_turns() {
        let assistant = Message {
            role: "assistant".into(),
            content: None,
            name: None,
            tool_calls: Some(vec![
                call("a", json!({ "command": "sed -n '1,9p' x.py" })),
                call("b", json!({ "command": "cat x.py" })),
            ]),
            tool_call_id: None,
            extra: Default::default(),
        };

        let ids = requested_range_call_ids(&[assistant]);

        assert_eq!(ids, HashSet::from(["a".to_string()]));
    }
}
