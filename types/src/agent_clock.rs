//! The current date and time, sent with every new message the platform hands an agent.
//!
//! An agent's model has no clock of its own: asked about "today" or "the last 24 hours", it
//! guesses a date from its training data and searches the wrong range. The platform knows the
//! time, so it says it, as one more text part after the message's own text (the same place
//! organization policy goes, never inside the user's part). A continuation of a task the agent
//! paused (a human's literal answer) gets nothing: the message that opened the task carried it.
//!
//! The time is UTC and says so; no user time zone is known yet.

use chrono::{DateTime, Utc};
use serde_json::Value;

/// How the part starts. A message whose parts already hold one is not given a second (an agent
/// forwarding its own input to a peer).
pub const NOTE_LEAD: &str = "Current date and time: ";

/// Whether agent messages carry the current time (`AGENT_CONTEXT_CURRENT_TIME`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentClock {
    enabled: bool,
}

impl AgentClock {
    pub const OFF: Self = Self { enabled: false };
    pub const ON: Self = Self { enabled: true };

    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// The part for a message sent now, or `None` when switched off.
    pub fn note(self) -> Option<String> {
        self.enabled.then(|| note_at(Utc::now()))
    }
}

/// The part for a message sent at `now`. To the minute, so a cached prompt that ends with it
/// changes at most once a minute.
pub fn note_at(now: DateTime<Utc>) -> String {
    format!(
        "{NOTE_LEAD}{} UTC ({}). Use it to work out relative dates and times such as \
         \"today\", \"yesterday\" or \"the last 24 hours\".",
        now.format("%A, %-d %B %Y, %H:%M"),
        now.format("%Y-%m-%dT%H:%MZ"),
    )
}

/// Whether `parts` (an A2A message's `parts`) already hold a current-time part.
pub fn carries_note(parts: &[Value]) -> bool {
    parts.iter().any(|part| {
        part.get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.starts_with(NOTE_LEAD))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 16, 13, 45, 31).unwrap()
    }

    #[test]
    fn the_note_names_the_day_the_date_and_the_minute_in_utc() {
        assert_eq!(
            note_at(at()),
            "Current date and time: Monday, 16 November 2026, 13:45 UTC (2026-11-16T13:45Z). \
             Use it to work out relative dates and times such as \"today\", \"yesterday\" or \
             \"the last 24 hours\"."
        );
    }

    #[test]
    fn a_clock_switched_off_says_nothing() {
        assert_eq!(AgentClock::OFF.note(), None);
        assert_eq!(AgentClock::default(), AgentClock::OFF);
        assert!(AgentClock::ON.note().unwrap().starts_with(NOTE_LEAD));
    }

    #[test]
    fn a_note_already_in_the_parts_is_recognised_in_either_wire_shape() {
        let note = note_at(at());
        assert!(carries_note(&[
            json!({ "text": "hi" }),
            json!({ "text": note })
        ]));
        assert!(carries_note(&[json!({ "kind": "text", "text": note })]));
        assert!(!carries_note(&[
            json!({ "text": "hi" }),
            json!({ "file": {} })
        ]));
        assert!(!carries_note(&[]));
    }
}
