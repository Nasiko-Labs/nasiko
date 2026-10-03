//! In-process MiniLM semantic request-type classifier.
//!
//! [`sentence-transformers/all-MiniLM-L6-v2`](https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2)
//! (Apache-2.0) produces 384-d embeddings. Category labels are the seven existing
//! [`RequestType`]s; we embed representative descriptions once at load time and
//! classify each query by cosine similarity.
//!
//! The model is **not** a complexity classifier. Complexity lives in
//! [`super::complexity`]. This module only answers "what kind of request is this?".
//!
//! Load happens at router startup. Request handling never downloads weights: if
//! the model is missing or inference fails, callers fall back to the regex
//! classifier.

use std::sync::Arc;

use super::classifier::RequestType;

/// Apache-2.0 MiniLM checkpoint used for semantic request-type classification.
pub const MINILM_MODEL_ID: &str = "sentence-transformers/all-MiniLM-L6-v2";

/// Below this max cosine similarity we treat MiniLM as uncertain and let the
/// caller fall back to regex classification.
pub const DEFAULT_MIN_CONFIDENCE: f32 = 0.28;

/// Representative descriptions — one per [`RequestType`], declaration order
/// matching the regex classifier so ties can be broken the same way.
pub const CATEGORY_DESCRIPTIONS: &[(RequestType, &str)] = &[
    (
        RequestType::CodeGeneration,
        "Write or modify source code: implement functions, scripts, APIs, generate programs, add features, or produce working code.",
    ),
    (
        RequestType::CodeUnderstanding,
        "Explain, debug, or review existing code: what a function does, why a bug happens, how this code works.",
    ),
    (
        RequestType::TechnicalDesign,
        "Architecture and engineering design decisions: APIs, system design, tradeoffs, how to structure a service.",
    ),
    (
        RequestType::AnalyticalReasoning,
        "Calculations, inference, probability, and multi-step analytical reasoning about numbers or logic.",
    ),
    (
        RequestType::Writing,
        "Draft or edit prose: emails, documents, summaries, rewrites, and other writing tasks that are not code.",
    ),
    (
        RequestType::FactualLookup,
        "Retrieve a specific fact or definition: capitals, dates, who/what/when questions with a short factual answer.",
    ),
    (
        RequestType::General,
        "Casual conversation, greetings, chit-chat, and other requests that are not a specific technical task.",
    ),
];

/// Embedding backend. Production uses MiniLM ONNX via fastembed; tests inject a mock.
pub trait Embedder: Send + Sync {
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
}

/// Result of one semantic classification.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticHit {
    pub request_type: RequestType,
    /// Cosine similarity of the winning category in `[-1, 1]` (typically `0..1`).
    pub confidence: f32,
}

/// Cached MiniLM (or mock) classifier: category embeddings computed once.
pub struct SemanticClassifier {
    embedder: Arc<dyn Embedder>,
    category_embeddings: Vec<(RequestType, Vec<f32>)>,
    pub min_confidence: f32,
}

impl SemanticClassifier {
    /// Build from any embedder. Embeds category descriptions immediately so
    /// request handling only embeds the query.
    pub fn from_embedder(
        embedder: Arc<dyn Embedder>,
        min_confidence: f32,
    ) -> Result<Self, String> {
        let texts: Vec<String> = CATEGORY_DESCRIPTIONS
            .iter()
            .map(|(_, d)| (*d).to_string())
            .collect();
        let vectors = embedder.embed_passages(&texts)?;
        if vectors.len() != CATEGORY_DESCRIPTIONS.len() {
            return Err(format!(
                "embedder returned {} category vectors, expected {}",
                vectors.len(),
                CATEGORY_DESCRIPTIONS.len()
            ));
        }
        let category_embeddings = CATEGORY_DESCRIPTIONS
            .iter()
            .zip(vectors)
            .map(|((rt, _), vec)| (*rt, vec))
            .collect();
        Ok(Self {
            embedder,
            category_embeddings,
            min_confidence,
        })
    }

    /// Load `all-MiniLM-L6-v2` once via fastembed. May download into the cache
    /// directory on first process start; later loads are local files only.
    #[cfg(feature = "minilm")]
    pub fn load_minilm(cache_dir: &str, min_confidence: f32) -> Result<Self, String> {
        let embedder = FastembedMiniLm::load(cache_dir)?;
        Self::from_embedder(Arc::new(embedder), min_confidence)
    }

    #[cfg(not(feature = "minilm"))]
    pub fn load_minilm(_cache_dir: &str, _min_confidence: f32) -> Result<Self, String> {
        Err("MiniLM support was not compiled (feature `minilm` disabled)".into())
    }

    /// Classify `query`. Empty input → `General` with confidence `0`.
    /// Inference errors are returned so the router can fall back to regex.
    pub fn classify(&self, query: &str) -> Result<SemanticHit, String> {
        if query.trim().is_empty() {
            return Ok(SemanticHit {
                request_type: RequestType::General,
                confidence: 0.0,
            });
        }
        let vectors = self.embedder.embed_passages(&[query.to_string()])?;
        let Some(query_vec) = vectors.into_iter().next() else {
            return Err("MiniLM inference returned no embedding".into());
        };
        let mut best_rt = RequestType::General;
        let mut best_sim = f32::NEG_INFINITY;
        for (rt, cat) in &self.category_embeddings {
            let sim = cosine(&query_vec, cat);
            if sim > best_sim {
                best_sim = sim;
                best_rt = *rt;
            }
        }
        if !best_sim.is_finite() {
            return Err("MiniLM cosine similarity was non-finite".into());
        }
        Ok(SemanticHit {
            request_type: best_rt,
            confidence: best_sim,
        })
    }

    /// Classify, falling back to `fallback` when confidence is below threshold
    /// or inference fails.
    pub fn classify_or_fallback(
        &self,
        query: &str,
        fallback: RequestType,
    ) -> (RequestType, Option<f32>) {
        match self.classify(query) {
            Ok(hit) if hit.confidence >= self.min_confidence => {
                (hit.request_type, Some(hit.confidence))
            }
            Ok(hit) => {
                tracing::debug!(
                    target: "nasiko::llm_router::minilm",
                    confidence = hit.confidence,
                    min_confidence = self.min_confidence,
                    minilm_type = %hit.request_type.as_str(),
                    fallback = %fallback.as_str(),
                    "minilm: low confidence, using regex request type"
                );
                (fallback, Some(hit.confidence))
            }
            Err(err) => {
                tracing::warn!(
                    target: "nasiko::llm_router::minilm",
                    error = %err,
                    fallback = %fallback.as_str(),
                    "minilm: inference failed, using regex request type"
                );
                (fallback, None)
            }
        }
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom < 1e-12 {
        0.0
    } else {
        (dot / denom).clamp(-1.0, 1.0)
    }
}

/// Production MiniLM backend (ONNX Runtime via fastembed). Apache-2.0.
#[cfg(feature = "minilm")]
struct FastembedMiniLm {
    model: std::sync::Mutex<fastembed::TextEmbedding>,
}

#[cfg(feature = "minilm")]
impl FastembedMiniLm {
    fn load(cache_dir: &str) -> Result<Self, String> {
        use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
        use std::path::PathBuf;

        let mut opts = TextInitOptions::new(EmbeddingModel::AllMiniLML6V2)
            .with_show_download_progress(true);
        if !cache_dir.is_empty() {
            opts = opts.with_cache_dir(PathBuf::from(cache_dir));
        }
        tracing::info!(
            target: "nasiko::llm_router::minilm",
            model = MINILM_MODEL_ID,
            cache_dir = %if cache_dir.is_empty() { "<fastembed-default>" } else { cache_dir },
            "minilm: loading ONNX embedding model (startup only; request path stays offline)"
        );
        let model = TextEmbedding::try_new(opts).map_err(|e| e.to_string())?;
        Ok(Self {
            model: std::sync::Mutex::new(model),
        })
    }
}

#[cfg(feature = "minilm")]
impl Embedder for FastembedMiniLm {
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let mut model = self
            .model
            .lock()
            .map_err(|_| "MiniLM embedder mutex poisoned".to_string())?;
        model.embed(texts.to_vec(), None).map_err(|e| e.to_string())
    }
}

/// Test embedder: 8-d keyword axes, no ONNX, no network.
#[cfg(test)]
pub(crate) struct KeywordEmbedder;

#[cfg(test)]
impl Embedder for KeywordEmbedder {
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Ok(texts.iter().map(|t| keyword_axes(t)).collect())
    }
}

#[cfg(test)]
struct FailingEmbedder;

#[cfg(test)]
impl Embedder for FailingEmbedder {
    fn embed_passages(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        Err("simulated inference failure".into())
    }
}

#[cfg(test)]
fn keyword_axes(text: &str) -> Vec<f32> {
    let t = text.to_ascii_lowercase();
    let mut v = vec![0.05f32; 8];
    let sets: [(&[&str], usize); 7] = [
        (
            &[
                "source code",
                "implement",
                "python",
                "script",
                "function",
                "generate programs",
                "working code",
                "csv",
            ],
            0,
        ),
        (
            &[
                "explain",
                "debug",
                "existing code",
                "what a function",
                "how this code",
            ],
            1,
        ),
        (
            &[
                "architecture",
                "engineering design",
                "system design",
                "design this api",
                "how should i design",
            ],
            2,
        ),
        (
            &[
                "calculations",
                "probability",
                "analytical",
                "calculate",
                "rains tomorrow",
            ],
            3,
        ),
        (
            &[
                "draft",
                "email",
                "prose",
                "writing tasks",
                "documents",
                "outage",
            ],
            4,
        ),
        (
            &[
                "specific fact",
                "capitals",
                "capital of",
                "definition",
                "who/what/when",
                "france",
            ],
            5,
        ),
        (
            &[
                "casual conversation",
                "greetings",
                "chit-chat",
                "hello",
                "hello there",
            ],
            6,
        ),
    ];
    for (cues, idx) in sets {
        if cues.iter().any(|c| t.contains(c)) {
            v[idx] = 1.0;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classifier() -> SemanticClassifier {
        SemanticClassifier::from_embedder(Arc::new(KeywordEmbedder), DEFAULT_MIN_CONFIDENCE)
            .expect("keyword embedder must embed categories")
    }

    #[test]
    fn empty_query_is_general() {
        let hit = classifier().classify("  ").unwrap();
        assert_eq!(hit.request_type, RequestType::General);
        assert_eq!(hit.confidence, 0.0);
    }

    #[test]
    fn categories_match_reference_examples() {
        let c = classifier();
        assert_eq!(
            c.classify("build me a python script that parses CSV")
                .unwrap()
                .request_type,
            RequestType::CodeGeneration
        );
        assert_eq!(
            c.classify("explain what this function does")
                .unwrap()
                .request_type,
            RequestType::CodeUnderstanding
        );
        assert_eq!(
            c.classify("how should I design this API?")
                .unwrap()
                .request_type,
            RequestType::TechnicalDesign
        );
        assert_eq!(
            c.classify("calculate the probability that it rains tomorrow")
                .unwrap()
                .request_type,
            RequestType::AnalyticalReasoning
        );
        assert_eq!(
            c.classify("draft an email to my team about the outage")
                .unwrap()
                .request_type,
            RequestType::Writing
        );
        assert_eq!(
            c.classify("what is the capital of France?")
                .unwrap()
                .request_type,
            RequestType::FactualLookup
        );
        assert_eq!(
            c.classify("hello there").unwrap().request_type,
            RequestType::General
        );
    }

    #[test]
    fn inference_error_falls_back() {
        let c = SemanticClassifier {
            embedder: Arc::new(FailingEmbedder),
            category_embeddings: vec![(RequestType::General, vec![1.0])],
            min_confidence: DEFAULT_MIN_CONFIDENCE,
        };
        let (rt, conf) = c.classify_or_fallback("anything", RequestType::Writing);
        assert_eq!(rt, RequestType::Writing);
        assert_eq!(conf, None);
    }

    #[test]
    fn low_confidence_falls_back_to_regex_type() {
        let c = SemanticClassifier::from_embedder(Arc::new(KeywordEmbedder), 0.99).unwrap();
        let (rt, conf) = c.classify_or_fallback(
            "a completely unrelated zzz query with no axes",
            RequestType::General,
        );
        assert_eq!(rt, RequestType::General);
        assert!(conf.is_some());
    }

    #[test]
    fn keyword_minilm_agrees_with_regex_on_reference_prompts() {
        use crate::routing::classifier::classify_request_type;
        let c = classifier();
        let prompts = [
            "build me a python script that parses CSV",
            "explain what this function does",
            "how should I design this API?",
            "calculate the probability that it rains tomorrow",
            "draft an email to my team about the outage",
            "what is the capital of France?",
            "hello there",
        ];
        for p in prompts {
            let regex = classify_request_type(p);
            let mini = c.classify(p).unwrap().request_type;
            assert_eq!(mini, regex, "disagreement on {p:?}");
        }
    }
}
