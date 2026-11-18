//! Flat tool search — the query-aware discovery layer for `tools/list`.
//!
//! Two implementations behind a shared [`ToolSearchIndex`] trait:
//! - [`SemanticSearchIndex`]: embedding cosine similarity via the in-process LLM router
//!   (default; model from `EMBEDDING_MODEL`)
//! - [`Bm25SearchIndex`]: keyword BM25 scoring (fallback, zero external deps)
//!
//! Both search the full `mcp_connector_tools` table in a single pass (flat search).
//! Benchmark (`docs/MCP_TOOL_SEARCH_BENCHMARK.md`) showed flat beats two-level on every
//! metric because two-level approaches are capped by connector recall.

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use nasiko_orchestrator::llm_backend::{LlmCallContext, SharedLlmBackend};

use crate::error::Result;
use crate::permissions::EffectiveAccess;

// ── Public types ───────────────────────────────────────────────────────────────

/// A tool matched by search, with full schema for injection into `tools/list`.
pub struct ToolMatch {
    pub connector_id: Uuid,
    pub tool_name: String,
    pub description: Option<String>,
    pub input_schema: Option<Value>,
    pub score: f32,
}

/// Trait for the flat tool search index.
#[async_trait]
pub trait ToolSearchIndex: Send + Sync {
    /// Search all tools matching `query`, filtered by accessible connectors and
    /// agent permissions and the user's entitlement. Returns up to `limit` results ordered by relevance.
    async fn search_tools(
        &self,
        query: &str,
        accessible_connector_ids: &[Uuid],
        access: &EffectiveAccess<'_>,
        limit: usize,
    ) -> Vec<ToolMatch>;

    /// Every indexed tool of `connector_ids` the agent's permissions and the user's entitlement
    /// do not deny, unranked
    /// (`score` 0): what a listing without search is built from, measured against the search
    /// result by the savings ledger. Required, so an index cannot silently report no baseline.
    fn granted_tools(&self, connector_ids: &[Uuid], access: &EffectiveAccess<'_>)
    -> Vec<ToolMatch>;

    /// Rebuild the in-memory index from current DB state.
    async fn rebuild(&self, db: &PgPool) -> Result<()>;
}

// ── Shared internals ───────────────────────────────────────────────────────────

#[derive(Clone)]
struct ToolEntry {
    connector_id: Uuid,
    tool_name: String,
    description: Option<String>,
    input_schema: Option<Value>,
}

/// Load all tools from the DB for index building.
async fn load_all_tools(db: &PgPool) -> Result<Vec<ToolEntry>> {
    let rows = sqlx::query_as::<_, (Uuid, String, Option<String>, Option<Value>)>(
        "SELECT connector_id, tool_name, description, input_schema FROM mcp_connector_tools",
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(cid, name, desc, schema)| ToolEntry {
            connector_id: cid,
            tool_name: name,
            description: desc,
            input_schema: schema,
        })
        .collect())
}

/// [`ToolSearchIndex::granted_tools`] over one index's tool list, with the same connector and
/// permission filter `search_tools` applies.
fn granted(
    tools: &[ToolEntry],
    connector_ids: &[Uuid],
    access: &EffectiveAccess<'_>,
) -> Vec<ToolMatch> {
    tools
        .iter()
        .filter(|t| connector_ids.contains(&t.connector_id))
        .filter(|t| {
            access.decide(t.connector_id, &t.tool_name) != crate::permissions::ToolAccess::Denied
        })
        .map(|t| ToolMatch {
            connector_id: t.connector_id,
            tool_name: t.tool_name.clone(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
            score: 0.0,
        })
        .collect()
}

fn tool_search_text(name: &str, description: Option<&str>) -> String {
    format!("{} {}", name.replace('_', " "), description.unwrap_or(""))
}

// ── BM25 ───────────────────────────────────────────────────────────────────────

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty() && s.len() > 1)
        .map(|s| s.to_string())
        .collect()
}

struct Bm25Inner {
    tools: Vec<ToolEntry>,
    tokens: Vec<Vec<String>>,
    df: HashMap<String, usize>,
    avgdl: f64,
}

/// Flat BM25 keyword search over all `mcp_connector_tools`.
pub struct Bm25SearchIndex {
    inner: RwLock<Bm25Inner>,
    rebuild_lock: tokio::sync::Mutex<()>,
}

impl Default for Bm25SearchIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl Bm25SearchIndex {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Bm25Inner {
                tools: Vec::new(),
                tokens: Vec::new(),
                df: HashMap::new(),
                avgdl: 1.0,
            }),
            rebuild_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Replace the index contents with `tools`, so a caller that already holds
    /// the tool list (the semantic index's fallback) needs no second DB read.
    fn index(&self, tools: Vec<ToolEntry>) {
        let tokens: Vec<Vec<String>> = tools
            .iter()
            .map(|t| tokenize(&tool_search_text(&t.tool_name, t.description.as_deref())))
            .collect();

        let mut df: HashMap<String, usize> = HashMap::new();
        for doc_tokens in &tokens {
            let unique: HashSet<&str> = doc_tokens.iter().map(|s| s.as_str()).collect();
            for t in unique {
                *df.entry(t.to_string()).or_default() += 1;
            }
        }

        let total_len: usize = tokens.iter().map(|t| t.len()).sum();
        let avgdl = if tools.is_empty() {
            1.0
        } else {
            total_len as f64 / tools.len() as f64
        };

        let mut inner = self.inner.write().unwrap();
        inner.tools = tools;
        inner.tokens = tokens;
        inner.df = df;
        inner.avgdl = avgdl;
    }

    fn score_query(
        qtokens: &[String],
        doc_tokens: &[String],
        df: &HashMap<String, usize>,
        n: f64,
        avgdl: f64,
    ) -> f64 {
        let k1 = 1.5;
        let b = 0.75;
        let dl = doc_tokens.len() as f64;
        let mut score = 0.0f64;
        for qt in qtokens {
            let tf = doc_tokens.iter().filter(|t| t == &qt).count() as f64;
            let df_val = df.get(qt.as_str()).copied().unwrap_or(0) as f64;
            if df_val == 0.0 {
                continue;
            }
            let idf = ((n - df_val + 0.5) / (df_val + 0.5) + 1.0).ln();
            let tf_norm = (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / avgdl));
            score += idf * tf_norm;
        }
        score
    }
}

#[async_trait]
impl ToolSearchIndex for Bm25SearchIndex {
    async fn search_tools(
        &self,
        query: &str,
        accessible_connector_ids: &[Uuid],
        access: &EffectiveAccess<'_>,
        limit: usize,
    ) -> Vec<ToolMatch> {
        let qtokens = tokenize(query);
        if qtokens.is_empty() {
            return Vec::new();
        }

        let accessible: HashSet<Uuid> = accessible_connector_ids.iter().copied().collect();
        let inner = self.inner.read().unwrap();
        let n = inner.tools.len() as f64;

        let mut scored: Vec<(usize, f64)> = inner
            .tools
            .iter()
            .zip(inner.tokens.iter())
            .enumerate()
            .filter(|(_, (tool, _))| accessible.contains(&tool.connector_id))
            .filter(|(_, (tool, _))| {
                access.decide(tool.connector_id, &tool.tool_name)
                    != crate::permissions::ToolAccess::Denied
            })
            .map(|(i, (_, doc_tokens))| {
                let score = Self::score_query(&qtokens, doc_tokens, &inner.df, n, inner.avgdl);
                (i, score)
            })
            .filter(|(_, s)| *s > 0.0)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        scored
            .into_iter()
            .map(|(i, score)| {
                let tool = &inner.tools[i];
                ToolMatch {
                    connector_id: tool.connector_id,
                    tool_name: tool.tool_name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                    score: score as f32,
                }
            })
            .collect()
    }

    fn granted_tools(
        &self,
        connector_ids: &[Uuid],
        access: &EffectiveAccess<'_>,
    ) -> Vec<ToolMatch> {
        granted(&self.inner.read().unwrap().tools, connector_ids, access)
    }

    async fn rebuild(&self, db: &PgPool) -> Result<()> {
        let _serialized = self.rebuild_lock.lock().await;
        let tools = load_all_tools(db).await?;
        let tool_count = tools.len();
        self.index(tools);

        tracing::info!(tools = tool_count, "BM25 search index rebuilt");
        Ok(())
    }
}

// ── Semantic ───────────────────────────────────────────────────────────────────

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    // Vectors from different embedding models are not comparable.
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Persisted vectors whose recorded identity matches `identity`.
fn reusable_vectors(
    existing: HashMap<(Uuid, String), Vec<f32>>,
    models: &HashMap<(Uuid, String), String>,
    identity: &str,
) -> HashMap<(Uuid, String), Vec<f32>> {
    existing
        .into_iter()
        .filter(|(key, _)| models.get(key).is_some_and(|m| m == identity))
        .collect()
}

/// Whether any tool has a usable (non-empty) semantic vector.
fn has_semantic_vectors(embeddings: &[Vec<f32>]) -> bool {
    embeddings.iter().any(|e| !e.is_empty())
}

fn f32_vec_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn bytes_to_f32_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// `trace_usage.agent_name` for gateway embedding calls.
pub const EMBEDDING_COMPONENT: &str = "mcp_tool_embedding";

/// Texts per router call; keeps request bodies bounded for every provider.
const EMBED_CHUNK: usize = 256;

/// Embeds through the in-process LLM router, so whichever provider serves
/// `EMBEDDING_MODEL` (OpenAI, Gemini, Vertex, …) is used and usage is billed
/// to the platform (`Uuid::nil()`), labelled [`EMBEDDING_COMPONENT`].
struct EmbeddingClient {
    backend: SharedLlmBackend,
    model: String,
}

impl EmbeddingClient {
    fn call() -> LlmCallContext {
        LlmCallContext {
            user_id: Uuid::nil(),
            component: EMBEDDING_COMPONENT.to_string(),
            session_id: None,
        }
    }

    async fn embed_batch(&self, texts: &[String]) -> std::result::Result<Vec<Vec<f32>>, String> {
        let call: LlmCallContext = Self::call();
        let mut all: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(EMBED_CHUNK) {
            let vectors: Vec<Vec<f32>> = self.backend.embed(&call, &self.model, chunk).await?;
            if vectors.len() != chunk.len() {
                return Err(format!(
                    "embedding backend returned {} vectors for {} inputs",
                    vectors.len(),
                    chunk.len()
                ));
            }
            all.extend(vectors);
        }
        Ok(all)
    }

    async fn embed_one(&self, text: &str) -> std::result::Result<Vec<f32>, String> {
        let mut results: Vec<Vec<f32>> = self.embed_batch(&[text.to_string()]).await?;
        results
            .pop()
            .ok_or_else(|| "embedding backend returned empty result".to_string())
    }
}

struct SemanticInner {
    tools: Vec<ToolEntry>,
    embeddings: Vec<Vec<f32>>,
}

/// Flat semantic search using embedding cosine similarity. Falls back to BM25
/// over the same tools when the query cannot be embedded.
pub struct SemanticSearchIndex {
    inner: RwLock<SemanticInner>,
    rebuild_lock: tokio::sync::Mutex<()>,
    bm25: Bm25SearchIndex,
    client: EmbeddingClient,
    redis: nasiko_redis::SharedRedis,
}

impl SemanticSearchIndex {
    /// `backend` embeds both tool texts and queries with `model`.
    pub fn new(redis: nasiko_redis::SharedRedis, backend: SharedLlmBackend, model: String) -> Self {
        Self {
            inner: RwLock::new(SemanticInner {
                tools: Vec::new(),
                embeddings: Vec::new(),
            }),
            rebuild_lock: tokio::sync::Mutex::new(()),
            bm25: Bm25SearchIndex::new(),
            client: EmbeddingClient { backend, model },
            redis,
        }
    }

    /// Embed a query, using Redis cache to avoid redundant API calls during
    /// multi-turn ReAct loops.
    async fn embed_query(&self, query: &str) -> std::result::Result<Vec<f32>, String> {
        use sha2::{Digest, Sha256};

        let hash = format!(
            "{:x}",
            Sha256::new().chain_update(query.as_bytes()).finalize()
        );
        // Key by the provider/model that actually serves the embedding, so a
        // provider switch never reuses vectors from another embedding space.
        let identity: String = self.client.backend.embedding_identity(&self.client.model);
        let cache_key = format!("mcp:qemb:{}:{}", identity, &hash[..16]);

        // Check Redis cache.
        if let Ok(mut conn) = self.redis.connection().await
            && let Ok(bytes) = redis::AsyncCommands::get::<_, Vec<u8>>(&mut conn, &cache_key).await
            && !bytes.is_empty()
        {
            return Ok(bytes_to_f32_vec(&bytes));
        }

        let embedding = self.client.embed_one(query).await?;

        // Cache for 10 minutes (covers a full multi-turn conversation).
        if let Ok(mut conn) = self.redis.connection().await {
            let _: std::result::Result<(), _> = redis::AsyncCommands::set_ex(
                &mut conn,
                &cache_key,
                f32_vec_to_bytes(&embedding),
                600,
            )
            .await;
        }

        Ok(embedding)
    }
}

#[async_trait]
impl ToolSearchIndex for SemanticSearchIndex {
    async fn search_tools(
        &self,
        query: &str,
        accessible_connector_ids: &[Uuid],
        access: &EffectiveAccess<'_>,
        limit: usize,
    ) -> Vec<ToolMatch> {
        // No vectors for the current embedding identity: semantic ranking is meaningless.
        let has_vectors: bool = has_semantic_vectors(&self.inner.read().unwrap().embeddings);
        if !has_vectors {
            return self
                .bm25
                .search_tools(query, accessible_connector_ids, access, limit)
                .await;
        }
        let query_emb = match self.embed_query(query).await {
            Ok(emb) => emb,
            Err(e) => {
                tracing::warn!(error = %e, "semantic search: query embedding failed, falling back to BM25");
                return self
                    .bm25
                    .search_tools(query, accessible_connector_ids, access, limit)
                    .await;
            }
        };

        let accessible: HashSet<Uuid> = accessible_connector_ids.iter().copied().collect();
        let inner = self.inner.read().unwrap();

        let mut scored: Vec<(usize, f64)> = inner
            .tools
            .iter()
            .zip(inner.embeddings.iter())
            .enumerate()
            .filter(|(_, (tool, _))| accessible.contains(&tool.connector_id))
            .filter(|(_, (tool, _))| {
                access.decide(tool.connector_id, &tool.tool_name)
                    != crate::permissions::ToolAccess::Denied
            })
            .map(|(i, (_, emb))| (i, cosine_similarity(&query_emb, emb) as f64))
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        scored
            .into_iter()
            .map(|(i, score)| {
                let tool = &inner.tools[i];
                ToolMatch {
                    connector_id: tool.connector_id,
                    tool_name: tool.tool_name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.input_schema.clone(),
                    score: score as f32,
                }
            })
            .collect()
    }

    fn granted_tools(
        &self,
        connector_ids: &[Uuid],
        access: &EffectiveAccess<'_>,
    ) -> Vec<ToolMatch> {
        granted(&self.inner.read().unwrap().tools, connector_ids, access)
    }

    async fn rebuild(&self, db: &PgPool) -> Result<()> {
        // Every catalog write now rebuilds, so rebuilds can overlap. Unserialized,
        // one that read the table before another's write could finish last and
        // install the older tool set, dropping the newer connector's tools.
        let _serialized = self.rebuild_lock.lock().await;
        let tools = load_all_tools(db).await?;
        self.bm25.index(tools.clone());
        if tools.is_empty() {
            let mut inner = self.inner.write().unwrap();
            inner.tools = Vec::new();
            inner.embeddings = Vec::new();
            tracing::info!("semantic search index rebuilt (empty)");
            return Ok(());
        }

        // Stored rows record the provider/model that served them; a switch re-embeds.
        let identity: String = self.client.backend.embedding_identity(&self.client.model);

        // Load existing embeddings from DB, keeping only vectors from the current identity
        // so a failed re-embed never ranks new-model queries against old-model vectors.
        let existing_model: HashMap<(Uuid, String), String> = load_embedding_models(db).await?;
        let existing: HashMap<(Uuid, String), Vec<f32>> =
            reusable_vectors(load_embeddings(db).await?, &existing_model, &identity);
        let mut texts_to_embed: Vec<(usize, String)> = Vec::new();
        for (i, tool) in tools.iter().enumerate() {
            let key = (tool.connector_id, tool.tool_name.clone());
            let needs_embed = !matches!((existing.get(&key), existing_model.get(&key)), (Some(_), Some(model)) if model == &identity);
            if needs_embed {
                texts_to_embed.push((
                    i,
                    tool_search_text(&tool.tool_name, tool.description.as_deref()),
                ));
            }
        }

        // Embed missing tools.
        let mut new_embeddings: HashMap<usize, Vec<f32>> = HashMap::new();
        if !texts_to_embed.is_empty() {
            tracing::info!(count = texts_to_embed.len(), "embedding new/changed tools");
            let texts: Vec<String> = texts_to_embed.iter().map(|(_, t)| t.clone()).collect();
            match self.client.embed_batch(&texts).await {
                Ok(embeddings) => {
                    let mut db_rows: Vec<(Uuid, String, Vec<u8>, String)> = Vec::new();
                    for ((idx, _), emb) in texts_to_embed.iter().zip(embeddings) {
                        let tool = &tools[*idx];
                        db_rows.push((
                            tool.connector_id,
                            tool.tool_name.clone(),
                            f32_vec_to_bytes(&emb),
                            identity.clone(),
                        ));
                        new_embeddings.insert(*idx, emb);
                    }
                    if let Err(e) = upsert_embeddings(db, &db_rows).await {
                        tracing::warn!(error = %e, "failed to persist tool embeddings");
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "embedding API call failed during index rebuild");
                    // Continue with whatever embeddings we have from DB.
                }
            }
        }

        // Build the parallel embeddings vec.
        let embeddings: Vec<Vec<f32>> = tools
            .iter()
            .enumerate()
            .map(|(i, tool)| {
                if let Some(emb) = new_embeddings.remove(&i) {
                    emb
                } else {
                    let key = (tool.connector_id, tool.tool_name.clone());
                    existing.get(&key).cloned().unwrap_or_default()
                }
            })
            .collect();

        let tool_count = tools.len();
        let embedded_count = embeddings.iter().filter(|e| !e.is_empty()).count();

        let mut inner = self.inner.write().unwrap();
        inner.tools = tools;
        inner.embeddings = embeddings;

        tracing::info!(
            tools = tool_count,
            embedded = embedded_count,
            "semantic search index rebuilt"
        );
        Ok(())
    }
}

// ── Noop (rollback) ────────────────────────────────────────────────────────────

/// No-op search index for `MCP_TOOL_SEARCH_MODE=none` — signals the caller
/// to use the existing eager fan-out path.
pub struct NoopSearchIndex;

#[async_trait]
impl ToolSearchIndex for NoopSearchIndex {
    async fn search_tools(
        &self,
        _query: &str,
        _accessible_connector_ids: &[Uuid],
        _access: &EffectiveAccess<'_>,
        _limit: usize,
    ) -> Vec<ToolMatch> {
        Vec::new()
    }

    /// Search is off, so no listing is measured against a baseline.
    fn granted_tools(
        &self,
        _connector_ids: &[Uuid],
        _access: &EffectiveAccess<'_>,
    ) -> Vec<ToolMatch> {
        Vec::new()
    }

    async fn rebuild(&self, _db: &PgPool) -> Result<()> {
        Ok(())
    }
}

// ── DB helpers ─────────────────────────────────────────────────────────────────

async fn load_embeddings(db: &PgPool) -> Result<HashMap<(Uuid, String), Vec<f32>>> {
    let rows = sqlx::query_as::<_, (Uuid, String, Vec<u8>)>(
        "SELECT connector_id, tool_name, embedding FROM mcp_tool_embeddings",
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(cid, name, bytes)| ((cid, name), bytes_to_f32_vec(&bytes)))
        .collect())
}

async fn load_embedding_models(db: &PgPool) -> Result<HashMap<(Uuid, String), String>> {
    let rows = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT connector_id, tool_name, model FROM mcp_tool_embeddings",
    )
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(cid, name, model)| ((cid, name), model))
        .collect())
}

async fn upsert_embeddings(db: &PgPool, rows: &[(Uuid, String, Vec<u8>, String)]) -> Result<()> {
    let mut tx = db.begin().await?;
    for (cid, name, embedding, model) in rows {
        sqlx::query(
            r#"INSERT INTO mcp_tool_embeddings (connector_id, tool_name, embedding, model)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (connector_id, tool_name) DO UPDATE SET
                 embedding = EXCLUDED.embedding,
                 model = EXCLUDED.model,
                 created_at = now()"#,
        )
        .bind(cid)
        .bind(name)
        .bind(embedding)
        .bind(model)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::PermissionContext;
    use std::collections::HashSet;

    #[test]
    fn cosine_similarity_is_zero_for_mismatched_or_empty_vectors() {
        // 3072-dim Gemini vs 1536-dim OpenAI vectors must not be compared.
        assert_eq!(cosine_similarity(&[1.0, 0.0, 0.0], &[1.0, 0.0]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert!((cosine_similarity(&[1.0, 2.0], &[1.0, 2.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn reusable_vectors_drops_other_identity_and_falls_back_when_none_left() {
        let a: (Uuid, String) = (connector_id("a"), "t1".to_string());
        let b: (Uuid, String) = (connector_id("b"), "t2".to_string());
        let existing: HashMap<(Uuid, String), Vec<f32>> =
            HashMap::from([(a.clone(), vec![1.0]), (b.clone(), vec![2.0])]);
        let models: HashMap<(Uuid, String), String> = HashMap::from([
            (a.clone(), "openai/text-embedding-3-small".to_string()),
            (b.clone(), "gemini/gemini-embedding-001".to_string()),
        ]);
        let kept: HashMap<(Uuid, String), Vec<f32>> =
            reusable_vectors(existing.clone(), &models, "gemini/gemini-embedding-001");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept.get(&b), Some(&vec![2.0]));

        let none: HashMap<(Uuid, String), Vec<f32>> =
            reusable_vectors(existing, &models, "vertex/text-embedding-005");
        assert!(none.is_empty());
        let rebuilt: Vec<Vec<f32>> = vec![Vec::new(), Vec::new()];
        assert!(!has_semantic_vectors(&rebuilt));
        assert!(has_semantic_vectors(&[Vec::new(), vec![1.0]]));
    }

    fn tool(connector: &str, name: &str, desc: &str) -> ToolEntry {
        ToolEntry {
            connector_id: Uuid::new_v5(&Uuid::NAMESPACE_DNS, connector.as_bytes()),
            tool_name: name.to_string(),
            description: Some(desc.to_string()),
            input_schema: None,
        }
    }

    fn connector_id(name: &str) -> Uuid {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, name.as_bytes())
    }

    fn all_connector_ids() -> Vec<Uuid> {
        vec![
            connector_id("gmail"),
            connector_id("slack"),
            connector_id("github"),
        ]
    }

    fn permissive_perms() -> PermissionContext {
        PermissionContext {
            agent_id: Uuid::nil(),
            enabled_connectors: all_connector_ids().into_iter().collect(),
            rules: vec![],
            hash: "test".to_string(),
        }
    }

    fn sample_tools() -> Vec<ToolEntry> {
        vec![
            tool(
                "gmail",
                "GMAIL_SEND_EMAIL",
                "Creates and sends an email from a gmail account",
            ),
            tool(
                "gmail",
                "GMAIL_FETCH_EMAILS",
                "Fetches emails from a gmail account inbox",
            ),
            tool(
                "gmail",
                "GMAIL_CREATE_EMAIL_DRAFT",
                "Creates a draft email in gmail",
            ),
            tool(
                "slack",
                "SLACK_CHAT_POST_MESSAGE",
                "Sends a message to a slack channel or DM",
            ),
            tool(
                "slack",
                "SLACK_LIST_CHANNELS",
                "Lists all channels in a slack workspace",
            ),
            tool(
                "github",
                "GITHUB_CREATE_ISSUE",
                "Creates a new issue in a github repository",
            ),
            tool(
                "github",
                "GITHUB_LIST_REPOS",
                "Lists repositories for the authenticated user",
            ),
        ]
    }

    // ─── BM25 tests ────────────────────────────────────────────────────────

    #[test]
    fn bm25_tokenize_splits_on_nonalpha_and_lowercases() {
        let tokens = tokenize("GMAIL_SEND_EMAIL is great");
        assert_eq!(tokens, vec!["gmail", "send", "email", "is", "great"]);
    }

    #[test]
    fn bm25_tokenize_filters_single_char() {
        let tokens = tokenize("a b cc dd");
        assert_eq!(tokens, vec!["cc", "dd"]);
    }

    #[tokio::test]
    async fn bm25_exact_keyword_match() {
        let index = Bm25SearchIndex::new();
        {
            let tools = sample_tools();
            let tokens: Vec<Vec<String>> = tools
                .iter()
                .map(|t| tokenize(&tool_search_text(&t.tool_name, t.description.as_deref())))
                .collect();
            let mut df: HashMap<String, usize> = HashMap::new();
            for doc_tokens in &tokens {
                let unique: HashSet<&str> = doc_tokens.iter().map(|s| s.as_str()).collect();
                for t in unique {
                    *df.entry(t.to_string()).or_default() += 1;
                }
            }
            let total_len: usize = tokens.iter().map(|t| t.len()).sum();
            let avgdl = total_len as f64 / tools.len() as f64;
            let mut inner = index.inner.write().unwrap();
            inner.tools = tools;
            inner.tokens = tokens;
            inner.df = df;
            inner.avgdl = avgdl;
        }

        let results = index
            .search_tools(
                "send an email",
                &all_connector_ids(),
                &EffectiveAccess::unrestricted(&permissive_perms()),
                5,
            )
            .await;

        assert!(
            !results.is_empty(),
            "should return results for 'send an email'"
        );
        assert_eq!(
            results[0].tool_name, "GMAIL_SEND_EMAIL",
            "top result should be GMAIL_SEND_EMAIL"
        );
    }

    #[tokio::test]
    async fn bm25_filters_by_accessible_connectors() {
        let index = Bm25SearchIndex::new();
        {
            let tools = sample_tools();
            let tokens: Vec<Vec<String>> = tools
                .iter()
                .map(|t| tokenize(&tool_search_text(&t.tool_name, t.description.as_deref())))
                .collect();
            let mut df: HashMap<String, usize> = HashMap::new();
            for doc_tokens in &tokens {
                let unique: HashSet<&str> = doc_tokens.iter().map(|s| s.as_str()).collect();
                for t in unique {
                    *df.entry(t.to_string()).or_default() += 1;
                }
            }
            let total_len: usize = tokens.iter().map(|t| t.len()).sum();
            let avgdl = total_len as f64 / tools.len() as f64;
            let mut inner = index.inner.write().unwrap();
            inner.tools = tools;
            inner.tokens = tokens;
            inner.df = df;
            inner.avgdl = avgdl;
        }

        // Only allow slack — gmail and github tools should be excluded.
        let results = index
            .search_tools(
                "send an email",
                &[connector_id("slack")],
                &EffectiveAccess::unrestricted(&permissive_perms()),
                5,
            )
            .await;

        for r in &results {
            assert_eq!(
                r.connector_id,
                connector_id("slack"),
                "all results should be from slack, got {} ({})",
                r.tool_name,
                r.connector_id
            );
        }
    }

    #[tokio::test]
    async fn bm25_filters_by_agent_permissions() {
        let index = Bm25SearchIndex::new();
        {
            let tools = sample_tools();
            let tokens: Vec<Vec<String>> = tools
                .iter()
                .map(|t| tokenize(&tool_search_text(&t.tool_name, t.description.as_deref())))
                .collect();
            let mut df: HashMap<String, usize> = HashMap::new();
            for doc_tokens in &tokens {
                let unique: HashSet<&str> = doc_tokens.iter().map(|s| s.as_str()).collect();
                for t in unique {
                    *df.entry(t.to_string()).or_default() += 1;
                }
            }
            let total_len: usize = tokens.iter().map(|t| t.len()).sum();
            let avgdl = total_len as f64 / tools.len() as f64;
            let mut inner = index.inner.write().unwrap();
            inner.tools = tools;
            inner.tokens = tokens;
            inner.df = df;
            inner.avgdl = avgdl;
        }

        // Gmail connector NOT enabled for agent → all gmail tools denied.
        let perms = PermissionContext {
            agent_id: Uuid::nil(),
            enabled_connectors: vec![connector_id("slack"), connector_id("github")]
                .into_iter()
                .collect(),
            rules: vec![],
            hash: "test".to_string(),
        };

        let results = index
            .search_tools(
                "send an email",
                &all_connector_ids(),
                &EffectiveAccess::unrestricted(&perms),
                5,
            )
            .await;

        for r in &results {
            assert_ne!(
                r.connector_id,
                connector_id("gmail"),
                "gmail tools should be filtered by agent perms"
            );
        }
    }

    #[tokio::test]
    async fn bm25_hides_tools_outside_the_user_entitlement() {
        use crate::entitlements::{ToolSet, UserEntitlement};
        let index = Bm25SearchIndex::new();
        {
            let tools = sample_tools();
            let tokens: Vec<Vec<String>> = tools
                .iter()
                .map(|t| tokenize(&tool_search_text(&t.tool_name, t.description.as_deref())))
                .collect();
            let mut df: HashMap<String, usize> = HashMap::new();
            for doc_tokens in &tokens {
                let unique: HashSet<&str> = doc_tokens.iter().map(|s| s.as_str()).collect();
                for t in unique {
                    *df.entry(t.to_string()).or_default() += 1;
                }
            }
            let total_len: usize = tokens.iter().map(|t| t.len()).sum();
            let avgdl = total_len as f64 / tools.len() as f64;
            let mut inner = index.inner.write().unwrap();
            inner.tools = tools;
            inner.tokens = tokens;
            inner.df = df;
            inner.avgdl = avgdl;
        }
        let perms = permissive_perms();
        // Entitled to nothing on gmail (no entry) and to Slack tools only.
        let ent =
            UserEntitlement::Restricted(HashMap::from([(connector_id("slack"), ToolSet::All)]));
        let results = index
            .search_tools(
                "send an email message",
                &all_connector_ids(),
                &EffectiveAccess::new(&perms, &ent),
                10,
            )
            .await;
        assert!(!results.is_empty());
        assert!(
            results
                .iter()
                .all(|r| r.connector_id == connector_id("slack")),
            "only entitled connectors may surface"
        );
    }

    #[tokio::test]
    async fn bm25_respects_limit() {
        let index = Bm25SearchIndex::new();
        {
            let tools = sample_tools();
            let tokens: Vec<Vec<String>> = tools
                .iter()
                .map(|t| tokenize(&tool_search_text(&t.tool_name, t.description.as_deref())))
                .collect();
            let mut df: HashMap<String, usize> = HashMap::new();
            for doc_tokens in &tokens {
                let unique: HashSet<&str> = doc_tokens.iter().map(|s| s.as_str()).collect();
                for t in unique {
                    *df.entry(t.to_string()).or_default() += 1;
                }
            }
            let total_len: usize = tokens.iter().map(|t| t.len()).sum();
            let avgdl = total_len as f64 / tools.len() as f64;
            let mut inner = index.inner.write().unwrap();
            inner.tools = tools;
            inner.tokens = tokens;
            inner.df = df;
            inner.avgdl = avgdl;
        }

        let results = index
            .search_tools(
                "email",
                &all_connector_ids(),
                &EffectiveAccess::unrestricted(&permissive_perms()),
                2,
            )
            .await;

        assert!(results.len() <= 2, "should respect limit of 2");
    }

    #[tokio::test]
    async fn bm25_empty_query_returns_empty() {
        let index = Bm25SearchIndex::new();
        let results = index
            .search_tools(
                "",
                &all_connector_ids(),
                &EffectiveAccess::unrestricted(&permissive_perms()),
                5,
            )
            .await;
        assert!(results.is_empty(), "empty query should return empty");
    }

    /// The savings baseline: every tool of the asked-for connectors the agent may see, whatever
    /// the query, and nothing from a connector it was not asked about or may not use.
    #[test]
    fn granted_tools_lists_the_asked_connectors_permitted_tools_unranked() {
        let index = Bm25SearchIndex::new();
        index.index(sample_tools());
        let perms = PermissionContext {
            enabled_connectors: [connector_id("slack"), connector_id("github")]
                .into_iter()
                .collect(),
            ..permissive_perms()
        };

        let granted = index.granted_tools(
            &[connector_id("gmail"), connector_id("slack")],
            &EffectiveAccess::unrestricted(&perms),
        );

        let names: HashSet<&str> = granted.iter().map(|m| m.tool_name.as_str()).collect();
        // gmail is asked for but not enabled; github is enabled but not asked for.
        assert_eq!(
            names,
            HashSet::from(["SLACK_CHAT_POST_MESSAGE", "SLACK_LIST_CHANNELS"])
        );
        assert!(granted.iter().all(|m| m.score == 0.0));
        assert!(
            NoopSearchIndex
                .granted_tools(
                    &all_connector_ids(),
                    &EffectiveAccess::unrestricted(&permissive_perms()),
                )
                .is_empty()
        );
    }

    #[tokio::test]
    async fn noop_always_returns_empty() {
        let index = NoopSearchIndex;
        let results = index
            .search_tools(
                "anything",
                &all_connector_ids(),
                &EffectiveAccess::unrestricted(&permissive_perms()),
                10,
            )
            .await;
        assert!(results.is_empty());
    }

    // ─── Semantic fallback tests ───────────────────────────────────────────

    /// Router stand-in whose every call fails.
    struct FailingBackend;

    #[async_trait]
    impl nasiko_orchestrator::llm_backend::LlmBackend for FailingBackend {
        async fn chat(
            &self,
            _call: &LlmCallContext,
            _body: serde_json::Value,
        ) -> std::result::Result<serde_json::Value, nasiko_orchestrator::llm_backend::LlmError>
        {
            Err(nasiko_orchestrator::llm_backend::LlmError {
                status: None,
                message: "unavailable".into(),
            })
        }
        async fn embed(
            &self,
            _call: &LlmCallContext,
            _model: &str,
            _texts: &[String],
        ) -> std::result::Result<Vec<Vec<f32>>, String> {
            Err("unavailable".into())
        }
        fn provider(&self) -> String {
            "test".into()
        }
    }

    #[tokio::test]
    async fn embed_batch_rejects_vector_count_mismatch() {
        struct ShortBackend;
        #[async_trait]
        impl nasiko_orchestrator::llm_backend::LlmBackend for ShortBackend {
            async fn chat(
                &self,
                _call: &LlmCallContext,
                _body: serde_json::Value,
            ) -> std::result::Result<serde_json::Value, nasiko_orchestrator::llm_backend::LlmError>
            {
                Err(nasiko_orchestrator::llm_backend::LlmError {
                    status: None,
                    message: "unused".into(),
                })
            }
            async fn embed(
                &self,
                call: &LlmCallContext,
                _model: &str,
                _texts: &[String],
            ) -> std::result::Result<Vec<Vec<f32>>, String> {
                assert_eq!(call.component, EMBEDDING_COMPONENT);
                assert_eq!(call.user_id, Uuid::nil());
                Ok(vec![vec![1.0]])
            }
            fn provider(&self) -> String {
                "test".into()
            }
        }
        let client = EmbeddingClient {
            backend: std::sync::Arc::new(ShortBackend),
            model: "m".into(),
        };
        let err: String = client
            .embed_batch(&["a".to_string(), "b".to_string()])
            .await
            .unwrap_err();
        assert!(err.contains("1 vectors for 2 inputs"), "{err}");
    }

    #[tokio::test]
    async fn semantic_falls_back_to_bm25_when_query_embedding_fails() {
        // Nothing listens on port 1, so the Redis cache and the embedding call
        // both fail at once without leaving the machine.
        let index = SemanticSearchIndex::new(
            nasiko_redis::SharedRedis::new(redis::Client::open("redis://127.0.0.1:1").unwrap()),
            std::sync::Arc::new(FailingBackend),
            "test-model".to_string(),
        );
        index.bm25.index(sample_tools());

        let results = index
            .search_tools(
                "send an email",
                &all_connector_ids(),
                &EffectiveAccess::unrestricted(&permissive_perms()),
                5,
            )
            .await;

        assert!(
            !results.is_empty(),
            "should fall back to BM25 instead of returning empty"
        );
        assert_eq!(
            results[0].tool_name, "GMAIL_SEND_EMAIL",
            "top result should match the BM25 ranking"
        );
    }

    // ─── Cosine similarity tests ───────────────────────────────────────────

    #[test]
    fn cosine_identical_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_zero_vector_returns_zero() {
        let a = vec![1.0, 2.0];
        let b = vec![0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    // ─── Serialization round-trip tests ────────────────────────────────────

    #[test]
    fn f32_bytes_roundtrip() {
        let original = vec![1.5f32, -2.3, 0.0, f32::MAX, f32::MIN];
        let bytes = f32_vec_to_bytes(&original);
        let recovered = bytes_to_f32_vec(&bytes);
        assert_eq!(original, recovered);
    }
}
