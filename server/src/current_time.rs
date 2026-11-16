//! Appends the current date and time to a message the server forwards to an agent
//! (`nasiko_types::agent_clock`), on the paths that forward a caller's own JSON-RPC body: the
//! agent proxy and direct dispatch. The orchestrator's agent tool and the MAF executor build their
//! own messages and add it as they do.

use nasiko_agent_proxy::context_part::{self, PartTagging};
use nasiko_types::agent_clock::{self, AgentClock};
use serde_json::Value;

/// Appends the current-time part to `message` (an A2A `params.message`) as its own part, after
/// any already there. Returns whether it did.
///
/// Nothing is added when the clock is off, when the message continues a task (a `taskId`: a
/// human's literal answer to a question the agent paused on, whose task was opened by a message
/// that carried the time), when its parts already hold one, or when it has no `parts` array.
pub fn append_current_time(message: &mut Value, clock: AgentClock, tagging: PartTagging) -> bool {
    if continues_a_task(message) || already_told(message) {
        return false;
    }
    let Some(note) = clock.note() else {
        return false;
    };
    context_part::append_context_part(message, &note, tagging).is_some()
}

fn continues_a_task(message: &Value) -> bool {
    message
        .get("taskId")
        .and_then(Value::as_str)
        .is_some_and(|task| !task.is_empty())
}

fn already_told(message: &Value) -> bool {
    message
        .get("parts")
        .and_then(Value::as_array)
        .is_some_and(|parts| agent_clock::carries_note(parts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn texts(message: &Value) -> Vec<String> {
        message["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    #[test]
    fn the_time_goes_last_and_the_users_part_is_untouched() {
        let mut message =
            json!({ "parts": [{ "text": "hi" }, { "text": "## Organization Policy" }] });

        assert!(append_current_time(
            &mut message,
            AgentClock::ON,
            PartTagging::Untagged
        ));

        let texts = texts(&message);
        assert_eq!(texts.len(), 3);
        assert_eq!(texts[..2], ["hi", "## Organization Policy"]);
        assert!(texts[2].starts_with(agent_clock::NOTE_LEAD));
        assert!(message["parts"][2].get("kind").is_none());
    }

    #[test]
    fn a_tagged_message_gets_a_tagged_part() {
        let mut message = json!({ "parts": [{ "kind": "text", "text": "hi" }] });

        assert!(append_current_time(
            &mut message,
            AgentClock::ON,
            PartTagging::Tagged
        ));

        assert_eq!(message["parts"][1]["kind"], "text");
    }

    #[test]
    fn a_continued_task_gets_nothing() {
        let original = json!({ "taskId": "t1", "parts": [{ "text": "authorized" }] });
        let mut message = original.clone();

        assert!(!append_current_time(
            &mut message,
            AgentClock::ON,
            PartTagging::Untagged
        ));
        assert_eq!(message, original);
    }

    #[test]
    fn a_message_that_already_tells_the_time_is_not_told_twice() {
        let mut message = json!({ "parts": [{ "text": "hi" }] });
        append_current_time(&mut message, AgentClock::ON, PartTagging::Untagged);
        let once = message.clone();

        assert!(!append_current_time(
            &mut message,
            AgentClock::ON,
            PartTagging::Untagged
        ));
        assert_eq!(message, once);
    }

    #[test]
    fn switched_off_or_without_parts_nothing_changes() {
        let original = json!({ "parts": [{ "text": "hi" }] });
        let mut message = original.clone();
        assert!(!append_current_time(
            &mut message,
            AgentClock::OFF,
            PartTagging::Untagged
        ));
        assert_eq!(message, original);

        let mut partless = json!({ "contextId": "c" });
        assert!(!append_current_time(
            &mut partless,
            AgentClock::ON,
            PartTagging::Untagged
        ));
        assert_eq!(partless, json!({ "contextId": "c" }));
    }
}
