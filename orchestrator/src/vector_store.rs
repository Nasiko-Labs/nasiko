use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use reqwest::Client;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::RouterError;
use crate::types::AgentCard;

pub struct EmbeddedAgent {
    pub agent: AgentCard,
    pub embedding: Vec<f32>,
}

/// One cached embedding, keyed by agent id in `EmbeddingCache`. Fields are
/// private — this type is only nameable because it appears inside the public
/// `EmbeddingCache` alias, not meant to be constructed or inspected directly.
pub struct CachedEmbedding {
    /// Hash of the text that was embedded (name + description + tags). If the
    /// agent's catalog entry changes, the hash no longer matches and the entry
    /// is treated as stale even before the TTL expires.
    content_hash: u64,
    embedding: Vec<f32>,
    cached_at: Instant,
}

/// Shared cache of agent embeddings, keyed by agent id. This is held on
/// `OssRoutingEngine` (not on `VectorStore`, which is rebuilt per request) so
/// embeddings survive across requests instead of re-embedding the whole
/// catalog against Ollama/OpenAI on every single `route()` call.
///
/// Invalidation strategy: there's no cheap signal available here for "the
/// catalog changed", so entries are considered stale after
/// `EMBEDDING_CACHE_TTL` elapses, in addition to being invalidated immediately
/// if the embedded fields change (via `content_hash`). A TTL is simpler to
/// reason about than wiring up exact change-tracking (e.g. a DB trigger or a
/// version column) and is good enough since catalog edits are infrequent
/// relative to the window.
pub type EmbeddingCache = Arc<DashMap<Uuid, CachedEmbedding>>;

const EMBEDDING_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

fn hash_prompt(prompt: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    prompt.hash(&mut hasher);
    hasher.finish()
}

pub struct VectorStore {
    agents: Vec<EmbeddedAgent>,
    api_key: String,
    base_url: String,
    model: String,
    enabled: bool,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingResponse {
    data: Vec<OpenAiEmbeddingData>,
}

#[derive(Deserialize)]
struct OpenAiEmbeddingData {
    embedding: Vec<f32>,
}

impl VectorStore {
    /// Build an embedded store from a list of agents using the OpenAI embeddings API.
    /// If the API key is empty or the call fails, falls back to disabled mode —
    /// shortlist() returns all agents unchanged.
    ///
    /// `cache` is consulted per-agent before making a network call: a cache hit
    /// (matching content hash, not yet past `EMBEDDING_CACHE_TTL`) skips the
    /// embeddings API entirely for that agent. See `EmbeddingCache` docs.
    pub async fn build(
        agents: Vec<AgentCard>,
        api_key: String,
        base_url: String,
        model: String,
        cache: &EmbeddingCache,
    ) -> Self {
        if api_key.is_empty() {
            tracing::debug!("No OpenAI API key configured — Stage 1 (vector store) disabled");
            return Self::disabled_from(agents);
        }

        let client = Client::new();
        let mut embedded = Vec::with_capacity(agents.len());

        for agent in &agents {
            let prompt = format!("{} {} {}", agent.name, agent.description, agent.tags.join(" "));
            let content_hash = hash_prompt(&prompt);

            if let Some(cached) = cache.get(&agent.id)
                && cached.content_hash == content_hash
                && cached.cached_at.elapsed() < EMBEDDING_CACHE_TTL
            {
                embedded.push(EmbeddedAgent {
                    agent: agent.clone(),
                    embedding: cached.embedding.clone(),
                });
                continue;
            }

            match embed_text(&client, &api_key, &base_url, &model, &prompt).await {
                Ok(emb) => {
                    cache.insert(
                        agent.id,
                        CachedEmbedding {
                            content_hash,
                            embedding: emb.clone(),
                            cached_at: Instant::now(),
                        },
                    );
                    embedded.push(EmbeddedAgent {
                        agent: agent.clone(),
                        embedding: emb,
                    });
                }
                Err(e) => {
                    tracing::warn!(%e, "OpenAI embeddings failed — disabling vector store, Stage 1 will be skipped");
                    return Self::disabled_from(agents);
                }
            }
        }

        Self {
            agents: embedded,
            api_key,
            base_url,
            model,
            enabled: true,
        }
    }

    /// Empty disabled store — used when agents list is empty or as a placeholder.
    pub fn disabled() -> Self {
        Self {
            agents: vec![],
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            enabled: false,
        }
    }

    /// Public alias used by tests and the Reranker.
    pub fn disabled_from_public(agents: Vec<AgentCard>) -> Self {
        Self::disabled_from(agents)
    }

    pub(crate) fn disabled_from(agents: Vec<AgentCard>) -> Self {
        Self {
            agents: agents
                .into_iter()
                .map(|a| EmbeddedAgent { agent: a, embedding: vec![] })
                .collect(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            enabled: false,
        }
    }

    /// Embed a single text string — reused by Reranker for history embedding.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, RouterError> {
        if !self.enabled {
            return Err(RouterError::Embedding("vector store is disabled".into()));
        }
        let client = Client::new();
        embed_text(&client, &self.api_key, &self.base_url, &self.model, text).await
    }

    /// Score a pre-computed embedding against a subset of agents using stored embeddings.
    /// Used by Reranker so it doesn't need to re-embed the agents.
    /// Falls back to equal weight (1.0) when store is disabled.
    pub fn score_agents(&self, query_emb: &[f32], agents: &[AgentCard]) -> Vec<(f32, AgentCard)> {
        if !self.enabled {
            return agents.iter().map(|a| (1.0, a.clone())).collect();
        }

        let mut scored: Vec<(f32, AgentCard)> = agents
            .iter()
            .filter_map(|a| {
                self.agents
                    .iter()
                    .find(|ea| ea.agent.id == a.id)
                    .map(|ea| (cosine_similarity(query_emb, &ea.embedding), a.clone()))
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }

    /// Return top-k agents by cosine similarity to query.
    ///
    /// Falls back to returning all agents when:
    ///   - Store is disabled (no API key or embeddings failed)
    ///   - Agent count < threshold (skip semantic search for small catalogs)
    ///   - Top similarity score < 0.2 (no meaningful match found)
    pub async fn shortlist(&self, query: &str, k: usize, threshold: usize) -> Vec<AgentCard> {
        let all: Vec<AgentCard> = self.agents.iter().map(|a| a.agent.clone()).collect();

        if !self.enabled || self.agents.len() < threshold {
            return all;
        }

        let query_emb = match self.embed(query).await {
            Ok(e) => e,
            Err(_) => return all,
        };

        let mut scored: Vec<(f32, &AgentCard)> = self
            .agents
            .iter()
            .map(|a| (cosine_similarity(&query_emb, &a.embedding), &a.agent))
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        if scored.first().map(|(s, _)| *s).unwrap_or(0.0) < 0.2 {
            tracing::debug!("top cosine score < 0.2, returning all agents as fallback");
            return all;
        }

        scored.into_iter().take(k).map(|(_, a)| a.clone()).collect()
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

async fn embed_text(
    client: &Client,
    api_key: &str,
    base_url: &str,
    model: &str,
    text: &str,
) -> Result<Vec<f32>, RouterError> {
    // Same normalization as `providers::normalize_base_url` — a `base_url`
    // already ending in `/v1` (as `OPENAI_BASE_URL` is commonly configured,
    // e.g. `ee/server/.env`) must not double up into `.../v1/v1/embeddings`.
    let trimmed = base_url.trim_end_matches('/');
    let trimmed = trimmed.strip_suffix("/v1").unwrap_or(trimmed);
    let url = format!("{trimmed}/v1/embeddings");
    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .json(&serde_json::json!({
            "model": model,
            "input": text,
        }))
        .send()
        .await
        .map_err(|e| RouterError::Embedding(format!("OpenAI embeddings request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(RouterError::Embedding(format!("OpenAI embeddings returned {status}: {body}")));
    }

    let parsed: OpenAiEmbeddingResponse = resp
        .json()
        .await
        .map_err(|e| RouterError::Embedding(format!("failed to parse embedding response: {e}")))?;

    parsed
        .data
        .into_iter()
        .next()
        .map(|d| d.embedding)
        .ok_or_else(|| RouterError::Embedding("empty embedding response".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_prompt_is_deterministic() {
        let h1 = hash_prompt("agent-1 does engineering things");
        let h2 = hash_prompt("agent-1 does engineering things");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_prompt_differs_for_different_content() {
        let h1 = hash_prompt("agent-1 does engineering things");
        let h2 = hash_prompt("agent-1 does completely different things");
        assert_ne!(h1, h2);
    }
}