//! Whether the orchestrator may spend on a user's behalf.
//!
//! The chat-path orchestrator calls its model provider directly, not through
//! the `/v1` gateway that an edition can guard, so it asks this before every
//! model call it makes. The open-source gate is [`NoSpendGate`], which never
//! refuses. A distribution with spending limits replaces it on
//! [`crate::state::AppState`] at its composition root, the same way it
//! replaces `orchestrator_policy`.

use async_trait::async_trait;
use uuid::Uuid;

#[async_trait]
pub trait SpendGate: Send + Sync {
    /// `Some(reason)` refuses the call. The reason is the failure the turn
    /// ends with, saved and shown as sent.
    async fn refuse(&self, user_id: Uuid, chat_session_id: &str) -> Option<String>;
}

/// The open-source gate: never refuses.
pub struct NoSpendGate;

#[async_trait]
impl SpendGate for NoSpendGate {
    async fn refuse(&self, _user_id: Uuid, _chat_session_id: &str) -> Option<String> {
        None
    }
}
