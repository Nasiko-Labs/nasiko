use nasiko_orchestrator::SessionHistory;
use nasiko_orchestrator::session_history::ChatMessage;

// ── Helpers ───────────────────────────────────────────────────────────────────

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

// ── is_empty ──────────────────────────────────────────────────────────────────

#[test]
fn empty_history_is_empty() {
    let h = SessionHistory::default();
    assert!(h.is_empty());
}

#[test]
fn non_empty_history_is_not_empty() {
    let h = make_history(&[("user", "hello")]);
    assert!(!h.is_empty());
}

// ── with_current_query ────────────────────────────────────────────────────────

#[test]
fn empty_history_returns_query_unchanged() {
    let h = SessionHistory::default();
    assert_eq!(h.with_current_query("hello"), "hello");
}

#[test]
fn with_current_query_prepends_history() {
    let h = make_history(&[("user", "hi"), ("assistant", "hello")]);
    let result = h.with_current_query("what now?");
    assert!(result.contains("user: hi"));
    assert!(result.contains("Current message: what now?"));
}

#[test]
fn with_current_query_includes_all_messages() {
    let h = make_history(&[
        ("user", "first"),
        ("assistant", "response"),
        ("user", "second"),
    ]);
    let result = h.with_current_query("third");
    assert!(result.contains("first"));
    assert!(result.contains("response"));
    assert!(result.contains("second"));
    assert!(result.contains("Current message: third"));
}

// ── summary_text ──────────────────────────────────────────────────────────────

#[test]
fn summary_text_joins_messages() {
    let h = make_history(&[("user", "hi"), ("assistant", "hello")]);
    assert_eq!(h.summary_text(), "user: hi\nassistant: hello");
}

#[test]
fn summary_text_single_message() {
    let h = make_history(&[("user", "solo")]);
    assert_eq!(h.summary_text(), "user: solo");
}

#[test]
fn summary_text_empty_is_empty_string() {
    let h = SessionHistory::default();
    assert_eq!(h.summary_text(), "");
}

// ── to_llm_messages ───────────────────────────────────────────────────────────

#[test]
fn to_llm_messages_maps_correctly() {
    let h = make_history(&[("user", "test message")]);
    let msgs = h.to_llm_messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[0].content, "test message");
}

#[test]
fn to_llm_messages_preserves_order() {
    let h = make_history(&[("user", "a"), ("assistant", "b"), ("user", "c")]);
    let msgs = h.to_llm_messages();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].role, "user");
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[2].role, "user");
}

#[test]
fn to_llm_messages_empty_returns_empty() {
    let h = SessionHistory::default();
    let msgs = h.to_llm_messages();
    assert!(msgs.is_empty());
}

// ── ConversationMessage serialization ─────────────────────────────────────────

#[test]
fn chat_message_serializes_correctly() {
    let msg = ChatMessage {
        role: "user".to_string(),
        content: "hello world".to_string(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["role"], "user");
    assert_eq!(v["content"], "hello world");
}

#[test]
fn chat_message_round_trips_through_json() {
    let original = ChatMessage {
        role: "assistant".to_string(),
        content: "I can help with that".to_string(),
    };
    let json = serde_json::to_string(&original).unwrap();
    let restored: ChatMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.role, original.role);
    assert_eq!(restored.content, original.content);
}

// ── DB-dependent tests (require running Postgres) ─────────────────────────────

#[tokio::test]
#[ignore = "requires live Postgres database"]
async fn fetch_returns_empty_for_unknown_session() {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost/nasiko".to_string());
    let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
    let h = SessionHistory::fetch("nonexistent-session-id-xyz", &pool, 20).await;
    assert!(h.is_empty());
}
