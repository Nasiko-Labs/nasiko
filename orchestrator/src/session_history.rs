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
        // Take the LATEST `limit` messages, then restore chronological order —
        // `ORDER BY timestamp ASC LIMIT n` would pin the window to the oldest
        // messages and never advance in long sessions.
        let mut messages: Vec<ChatMessage> = sqlx::query_as::<_, (String, String)>(
            "SELECT role, content FROM chat_messages \
             WHERE session_id = $1 ORDER BY timestamp DESC LIMIT $2",
        )
        .bind(session_id)
        .bind(limit as i64)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(role, content)| ChatMessage { role, content })
        .collect();
        messages.reverse();

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
