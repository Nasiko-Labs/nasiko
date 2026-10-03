//! Scan `<<call name {json}>>` out of model text.
//!
//! `>>` ends a call only outside a JSON string. Inside a string it is argument
//! text, including when the quote before it is escaped.

use crate::types::{ArgumentFault, CompactError};

#[derive(Clone, Copy)]
enum Scan {
    Outside,
    InString,
    Escaped,
}

pub(crate) struct RawCall {
    pub name: String,
    pub arguments: String,
}

pub(crate) enum Taken {
    Ready(RawCall, usize),
    /// The marker has started and the closer has not arrived yet.
    Incomplete,
}

/// One call, and how many chars of `after_marker` it consumed, including the closer.
pub(crate) fn take_call(after_marker: &str) -> Result<Taken, CompactError> {
    let mut chars = after_marker.chars().peekable();
    let mut consumed = 0usize;
    let mut name = String::new();
    while let Some(ch) = chars.next() {
        consumed += 1;
        if ch == ' ' {
            break;
        }
        name.push(ch);
    }
    if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(malformed(&name));
    }

    let mut arguments = String::new();
    let mut state = Scan::Outside;
    while let Some(ch) = chars.next() {
        consumed += 1;
        match state {
            Scan::Outside if ch == '>' && chars.peek() == Some(&'>') => {
                chars.next();
                consumed += 1;
                let arguments = arguments.trim().to_string();
                if !arguments.starts_with('{') {
                    return Err(malformed(&name));
                }
                return Ok(Taken::Ready(RawCall { name, arguments }, consumed));
            }
            Scan::Outside if ch == '"' => {
                arguments.push(ch);
                state = Scan::InString;
            }
            Scan::InString if ch == '\\' => {
                arguments.push(ch);
                state = Scan::Escaped;
            }
            Scan::InString if ch == '"' => {
                arguments.push(ch);
                state = Scan::Outside;
            }
            Scan::Escaped => {
                arguments.push(ch);
                state = Scan::InString;
            }
            _ => arguments.push(ch),
        }
    }
    Ok(Taken::Incomplete)
}

pub(crate) fn malformed(name: &str) -> CompactError {
    CompactError::InvalidArguments {
        name: name.to_string(),
        reason: ArgumentFault::Malformed,
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::calendar;
    use serde_json::Value;

    #[test]
    fn greater_than_inside_a_string_stays_in_the_title() {
        let text = r#"<<call create_calendar_event {"title":"meet >> review","start":"2026-10-05T15:00:00+05:30"}>> trailing"#;
        let calls = crate::decode_calls(text, &[calendar()]).unwrap();
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["title"], "meet >> review");
        assert!(!calls[0].arguments.contains("trailing"));
    }

    #[test]
    fn escaped_quote_does_not_end_the_json_string() {
        let text = r#"<<call create_calendar_event {"title":"say \"hi\"","start":"2026-10-05T15:00:00+05:30"}>>"#;
        let calls = crate::decode_calls(text, &[calendar()]).unwrap();
        let args: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        assert_eq!(args["title"], "say \"hi\"");
    }
}
