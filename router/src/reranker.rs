use std::sync::Arc;

use crate::session_history::SessionHistory;
use crate::types::AgentCard;
use crate::vector_store::VectorStore;

/// Stage 2: conversation-aware re-ranking of the Stage 1 shortlist.
///
/// Stateless — reuses embeddings already held by `VectorStore`.
/// When history is absent, returns the shortlist unchanged (no embed call made).
pub struct Reranker {
    vector_store: Arc<VectorStore>,
}

impl Reranker {
    pub fn new(vector_store: Arc<VectorStore>) -> Self {
        Self { vector_store }
    }

    /// Re-rank `agents` using conversation context + current query.
    ///
    /// - Empty history → return `agents[..k]` (no embed call, no change in order)
    /// - Non-empty history → embed `history + query`, score against stored agent
    ///   embeddings via VectorStore, return top-k by score
    /// - If embedding fails → return `agents[..k]` unchanged (graceful degradation)
    pub async fn rerank(
        &self,
        agents: Vec<AgentCard>,
        history: &SessionHistory,
        query: &str,
        k: usize,
    ) -> Vec<AgentCard> {
        if history.is_empty() {
            return agents.into_iter().take(k).collect();
        }

        let text = format!("{} {}", history.summary_text(), query);

        let embedding = match self.vector_store.embed(&text).await {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(%e, "reranker embed failed, returning shortlist unchanged");
                return agents.into_iter().take(k).collect();
            }
        };

        self.vector_store
            .score_agents(&embedding, &agents)
            .into_iter()
            .take(k)
            .map(|(_, a)| a)
            .collect()
    }
}

#[cfg(test)]
#[path = "tests/reranker.rs"]
mod tests;
