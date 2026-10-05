use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use reqwest::Client;
use serde::Deserialize;
use sqlx::PgPool;

use crate::error::RouterError;
use crate::types::AgentCard;

pub struct EmbeddedAgent {
    pub agent: AgentCard,
    pub embedding: Vec<f32>,
}

/// Text embedded for an agent: `name + description + tags`. Kept in one place
/// so the hash computed here always matches what gets stored and re-checked.
fn agent_prompt(agent: &AgentCard) -> String {
    format!(
        "{} {} {}",
        agent.name,
        agent.description,
        agent.tags.join(" ")
    )
}

/// One cached embedding for a piece of free-form text (e.g. a PACMS candidate
/// message), keyed by content hash in `TextEmbeddingCache`. The key already
/// identifies the content, so there's no separate `content_hash` field to
/// compare.
pub struct CachedTextEmbedding {
    embedding: Vec<f32>,
    cached_at: Instant,
}

/// Cache of text embeddings keyed by a hash of the text itself, shared across
/// `route()` calls (held on `OssRoutingEngine`). PACMS's history pool overlaps
/// heavily turn-to-turn within a session, so without this every call to
/// `SessionHistory::fetch_pacms` would re-embed messages already embedded on a
/// previous turn.
pub type TextEmbeddingCache = Arc<DashMap<i64, CachedTextEmbedding>>;

const EMBEDDING_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

pub fn hash_prompt(prompt: &str) -> i64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    prompt.hash(&mut hasher);
    hasher.finish() as i64
}

/// Embeds `agent`'s current name/description/tags and persists the result
/// (embedding + content hash) to `agents.embedding` /
/// `agents.embedding_content_hash`. Called both proactively — right after an
/// agent's card is fetched/updated — and lazily from `VectorStore::build` when
/// a stored embedding is missing or stale. Returns the embedding on success.
pub async fn embed_and_store_agent(
    pool: &PgPool,
    agent: &AgentCard,
    api_key: &str,
    base_url: &str,
    model: &str,
) -> Result<Vec<f32>, RouterError> {
    let client = Client::new();
    let prompt = agent_prompt(agent);
    let content_hash = hash_prompt(&prompt);
    let embedding = embed_text(&client, api_key, base_url, model, &prompt).await?;

    let embedding_f64: Vec<f64> = embedding.iter().map(|f| *f as f64).collect();
    if let Err(e) = sqlx::query(
        r#"UPDATE agents SET embedding = $1, embedding_content_hash = $2, embedded_at = now()
           WHERE id = $3"#,
    )
    .bind(&embedding_f64)
    .bind(content_hash)
    .bind(agent.id)
    .execute(pool)
    .await
    {
        tracing::warn!(agent_id = %agent.id, error = %e, "failed to persist agent embedding (non-fatal)");
    }

    Ok(embedding)
}

pub struct VectorStore {
    agents: Vec<EmbeddedAgent>,
    api_key: String,
    base_url: String,
    model: String,
    enabled: bool,
    /// Only set by `for_embedding` — the agent-catalog constructors reuse the
    /// embeddings persisted on the `agents` table instead.
    text_cache: Option<TextEmbeddingCache>,
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
    /// Each agent's `embedding`/`embedding_content_hash` (loaded from the
    /// `agents` table by `agent_registry::get_agents_for_user`) is checked
    /// against a freshly computed hash of its current name/description/tags:
    /// a match skips the embeddings API entirely for that agent. A miss (never
    /// embedded, or the agent's card changed) re-embeds and persists the
    /// result via `embed_and_store_agent` so it's not recomputed next time.
    pub async fn build(
        agents: Vec<AgentCard>,
        api_key: String,
        base_url: String,
        model: String,
        pool: &PgPool,
    ) -> Self {
        if api_key.is_empty() {
            tracing::debug!("No OpenAI API key configured — Stage 1 (vector store) disabled");
            return Self::disabled_from(agents);
        }

        let mut embedded = Vec::with_capacity(agents.len());
        let mut stored_hits = 0usize;
        let mut stored_misses = 0usize;

        for agent in &agents {
            let content_hash = hash_prompt(&agent_prompt(agent));

            if let (Some(emb), Some(stored_hash)) = (&agent.embedding, agent.embedding_content_hash)
                && stored_hash == content_hash
            {
                stored_hits += 1;
                embedded.push(EmbeddedAgent {
                    agent: agent.clone(),
                    embedding: emb.clone(),
                });
                continue;
            }

            stored_misses += 1;
            let embed_start = Instant::now();
            match embed_and_store_agent(pool, agent, &api_key, &base_url, &model).await {
                Ok(emb) => {
                    tracing::info!(
                        agent_name = %agent.name,
                        elapsed_ms = embed_start.elapsed().as_millis() as u64,
                        "vector_store: embedded agent (stored embedding missing/stale)"
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

        tracing::info!(stored_hits, stored_misses, "vector_store: build() done");

        Self {
            agents: embedded,
            api_key,
            base_url,
            model,
            enabled: true,
            text_cache: None,
        }
    }

    /// A store with no agent catalog, usable only for `embed()`/`embed_batch()`
    /// — for callers (e.g. `SessionHistory::fetch_pacms`) that need text
    /// embeddings but have no agent shortlist to build. Disabled (falls back
    /// cleanly) when `api_key` is empty.
    ///
    /// `cache` is consulted per-text before making a network call, keyed by a
    /// hash of the text — see `TextEmbeddingCache` docs.
    pub fn for_embedding(
        api_key: String,
        base_url: String,
        model: String,
        cache: TextEmbeddingCache,
    ) -> Self {
        let enabled = !api_key.is_empty();
        Self {
            agents: vec![],
            api_key,
            base_url,
            model,
            enabled,
            text_cache: Some(cache),
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
            text_cache: None,
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
                .map(|a| EmbeddedAgent {
                    agent: a,
                    embedding: vec![],
                })
                .collect(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            enabled: false,
            text_cache: None,
        }
    }

    /// Look up a text embedding in `text_cache`, if this store has one and the
    /// entry hasn't expired. Logs the outcome under `pacms_embedding_cache`
    /// (hit/miss + text hash) so external probes (e.g.
    /// `scripts/pacms_longmemeval_test.py`) can verify caching behavior from
    /// the server log without instrumenting the call sites themselves.
    fn cached_embedding(&self, text: &str) -> Option<Vec<f32>> {
        let cache = self.text_cache.as_ref()?;
        let key = hash_prompt(text);
        let hit = cache
            .get(&key)
            .filter(|entry| entry.cached_at.elapsed() < EMBEDDING_CACHE_TTL)
            .map(|entry| entry.embedding.clone());
        tracing::debug!(
            target: "pacms_embedding_cache",
            hit = hit.is_some(),
            text_hash = key,
            "PACMS embedding cache {}",
            if hit.is_some() { "hit" } else { "miss" }
        );
        hit
    }

    fn store_embedding(&self, text: &str, embedding: &[f32]) {
        if let Some(cache) = &self.text_cache {
            cache.insert(
                hash_prompt(text),
                CachedTextEmbedding {
                    embedding: embedding.to_vec(),
                    cached_at: Instant::now(),
                },
            );
        }
    }

    /// Embed a single text string — reused by Reranker for history embedding.
    /// Consults `text_cache` first when this store has one.
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, RouterError> {
        if !self.enabled {
            return Err(RouterError::Embedding("vector store is disabled".into()));
        }
        if let Some(cached) = self.cached_embedding(text) {
            return Ok(cached);
        }
        let client = Client::new();
        let embedding =
            embed_text(&client, &self.api_key, &self.base_url, &self.model, text).await?;
        self.store_embedding(text, &embedding);
        Ok(embedding)
    }

    /// Embed multiple texts, fetching only the ones missing from `text_cache`
    /// in a single request — lets callers that need one embedding per
    /// candidate (e.g. PACMS's coverage/diversity scoring) pay for at most one
    /// HTTP round-trip per call, and none at all once the pool is warm.
    pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, RouterError> {
        if !self.enabled {
            return Err(RouterError::Embedding("vector store is disabled".into()));
        }
        if texts.is_empty() {
            return Ok(vec![]);
        }

        let mut result: Vec<Option<Vec<f32>>> = Vec::with_capacity(texts.len());
        let mut misses: Vec<(usize, String)> = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            match self.cached_embedding(text) {
                Some(emb) => result.push(Some(emb)),
                None => {
                    result.push(None);
                    misses.push((i, text.clone()));
                }
            }
        }

        if !misses.is_empty() {
            let client = Client::new();
            let miss_texts: Vec<String> = misses.iter().map(|(_, t)| t.clone()).collect();
            let fetched = embed_texts(
                &client,
                &self.api_key,
                &self.base_url,
                &self.model,
                &miss_texts,
            )
            .await?;
            for ((i, text), emb) in misses.into_iter().zip(fetched) {
                self.store_embedding(&text, &emb);
                result[i] = Some(emb);
            }
        }

        Ok(result
            .into_iter()
            .map(|e| e.expect("filled above"))
            .collect())
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

pub(crate) fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
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
    // A `base_url` already ending in `/v1` (as `OPENAI_BASE_URL` is commonly
    // configured in the deployment's env) must not double up into
    // `.../v1/v1/embeddings`.
    let url = format!(
        "{}/v1/embeddings",
        nasiko_config::openai_base_url_without_v1(base_url)
    );
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
        return Err(RouterError::Embedding(format!(
            "OpenAI embeddings returned {status}: {body}"
        )));
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

/// Batch variant of `embed_text` — sends all `texts` as a single `input` array
/// and returns their embeddings in the same order. The OpenAI embeddings API
/// preserves input order in `data` (each item carries an `index`, and results
/// are returned sorted by it), so a positional zip is safe here.
async fn embed_texts(
    client: &Client,
    api_key: &str,
    base_url: &str,
    model: &str,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, RouterError> {
    let url = format!(
        "{}/v1/embeddings",
        nasiko_config::openai_base_url_without_v1(base_url)
    );
    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .json(&serde_json::json!({
            "model": model,
            "input": texts,
        }))
        .send()
        .await
        .map_err(|e| RouterError::Embedding(format!("OpenAI embeddings request failed: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(RouterError::Embedding(format!(
            "OpenAI embeddings returned {status}: {body}"
        )));
    }

    let parsed: OpenAiEmbeddingResponse = resp
        .json()
        .await
        .map_err(|e| RouterError::Embedding(format!("failed to parse embedding response: {e}")))?;

    if parsed.data.len() != texts.len() {
        return Err(RouterError::Embedding(format!(
            "expected {} embeddings, got {}",
            texts.len(),
            parsed.data.len()
        )));
    }

    Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
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
