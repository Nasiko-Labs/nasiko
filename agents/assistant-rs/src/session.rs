//! Per-context conversation memory so a paused task resumes in place.
//!
//! On pause the agent saves the chat transcript plus the tool call it is waiting on. The
//! next user message in the same `context_id` answers that call (input) or signals the
//! user connected the account (auth), and the loop continues.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// Why the task paused.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    /// Waiting for the answer to an `ask_user` tool call.
    Input { tool_call_id: String },
    /// Waiting for the user to connect an account; the tool call is retried on resume.
    Auth {
        tool_call_id: String,
        connector: String,
    },
}

/// Saved state of one conversation.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    /// OpenAI chat messages, system prompt excluded (rebuilt per request).
    pub messages: Vec<Value>,
    pub pending: Option<Pending>,
}

impl Conversation {
    /// Appends the new user text: as the pending tool's result when paused, else as a turn.
    pub fn resume_with(&mut self, user_text: &str) {
        match self.pending.take() {
            Some(Pending::Input { tool_call_id }) => self.messages.push(json!({
                "role": "tool", "tool_call_id": tool_call_id,
                "content": format!("User answered: {user_text}"),
            })),
            Some(Pending::Auth {
                tool_call_id,
                connector,
            }) => self.messages.push(json!({
                "role": "tool", "tool_call_id": tool_call_id,
                "content": format!(
                    "The user was asked to connect {connector} and replied: {user_text}. \
                     If they connected it, call the same tool again now."
                ),
            })),
            None => self
                .messages
                .push(json!({"role": "user", "content": user_text})),
        }
    }
}

/// In-memory store keyed by A2A `context_id`, capped to bound memory.
pub struct ConversationStore {
    inner: Mutex<HashMap<String, Conversation>>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    max: usize,
}

impl ConversationStore {
    pub fn new(max: usize) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            max,
        }
    }

    /// Per-context mutex so overlapping turns on the same `context_id` serialize.
    pub fn context_lock(&self, context_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.locks.lock().unwrap();
        locks
            .entry(context_id.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    pub fn take(&self, context_id: &str) -> Conversation {
        self.inner
            .lock()
            .unwrap()
            .remove(context_id)
            .unwrap_or_default()
    }

    pub fn put(&self, context_id: &str, conv: Conversation) {
        let mut map = self.inner.lock().unwrap();
        if map.len() >= self.max
            && !map.contains_key(context_id)
            && let Some(k) = map.keys().next().cloned()
        {
            map.remove(&k);
        }
        map.insert(context_id.to_owned(), conv);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_answer_becomes_tool_result() {
        let mut c = Conversation {
            messages: vec![],
            pending: Some(Pending::Input {
                tool_call_id: "1".into(),
            }),
        };
        c.resume_with("Work");
        assert_eq!(c.messages[0]["role"], "tool");
        assert_eq!(c.messages[0]["tool_call_id"], "1");
        assert!(c.pending.is_none());
    }

    #[test]
    fn auth_resume_asks_for_retry() {
        let mut c = Conversation {
            messages: vec![],
            pending: Some(Pending::Auth {
                tool_call_id: "2".into(),
                connector: "gmail".into(),
            }),
        };
        c.resume_with("done");
        assert!(
            c.messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("call the same tool again")
        );
    }

    #[test]
    fn store_round_trips_and_caps() {
        let s = ConversationStore::new(1);
        s.put("a", Conversation::default());
        s.put("b", Conversation::default());
        assert!(s.inner.lock().unwrap().len() == 1);
        let _ = s.take("b");
        assert!(s.inner.lock().unwrap().is_empty());
    }
}
