use serde::{Deserialize, Serialize};
use sqlx::PgPool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct SessionHistory {
    pub messages: Vec<ChatMessage>,
}

impl SessionHistory {
    pub async fn fetch(session_id: &str, pool: &PgPool, limit: usize) -> Self {
        let messages = sqlx::query_as::<_, (String, String)>(
            "SELECT role, content FROM chat_messages \
             WHERE session_id = $1 ORDER BY timestamp ASC LIMIT $2",
        )
        .bind(session_id)
        .bind(limit as i64)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(role, content)| ChatMessage { role, content })
        .collect();

        Self { messages }
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Map to LLM-format messages (role + content pairs).
    pub fn to_llm_messages(&self) -> Vec<LlmMessage> {
        self.messages
            .iter()
            .map(|m| LlmMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect()
    }

    /// Flat text of all messages — used as Stage 2 embedding input for re-ranking.
    pub fn summary_text(&self) -> String {
        self.messages
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Build the full query string: history context + current message.
    pub fn with_current_query(&self, query: &str) -> String {
        if self.is_empty() {
            query.to_string()
        } else {
            format!("{}\n\nCurrent message: {}", self.summary_text(), query)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

#[cfg(test)]
mod tests {
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
}