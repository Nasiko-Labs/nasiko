use super::*;

fn make_history(pairs: &[(&str, &str)]) -> SessionHistory {
    SessionHistory {
        messages: pairs
            .iter()
            .map(|(role, content)| ChatMessage {
                role: role.to_string(),
                content: content.to_string(),
            })
            .collect(),
    }
}

#[test]
fn empty_history_returns_query_unchanged() {
    let h = SessionHistory::default();
    assert_eq!(h.with_current_query("hello"), "hello");
    assert!(h.is_empty());
}

#[test]
fn summary_text_joins_messages() {
    let h = make_history(&[("user", "hi"), ("assistant", "hello")]);
    assert_eq!(h.summary_text(), "user: hi\nassistant: hello");
}

#[test]
fn with_current_query_prepends_history() {
    let h = make_history(&[("user", "hi"), ("assistant", "hello")]);
    let result = h.with_current_query("what now?");
    assert!(result.contains("user: hi"));
    assert!(result.contains("Current message: what now?"));
}

#[test]
fn to_llm_messages_maps_correctly() {
    let h = make_history(&[("user", "test")]);
    let msgs = h.to_llm_messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "test");
}
