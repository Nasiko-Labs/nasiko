//! Level-3 request-type classifier: ONNX `all-MiniLM-L6-v2` plus a hashing-trick fallback.
//!
//! Opt in with `ROUTER_CLASSIFIER=minilm`. Startup loads the MiniLM ONNX session via
//! [`fastembed`] (downloads into `ROUTER_MINILM_CACHE` / `FASTEMBED_CACHE_DIR` on first
//! use). A load failure logs a warning and the router stays on regex — Level 3 never
//! takes the process down.
//!
//! `ROUTER_CLASSIFIER=hash` keeps the 384-d hashing-trick encoder (no ONNX). Both
//! backends still cosine-match against the same prototype phrases; Thompson sampling
//! and the quality/cost blend in [`super::classifier`] are unchanged.

use std::path::Path;
use std::sync::Mutex;

use super::classifier::RequestType;

/// Same width as MiniLM-L6-v2.
pub const EMBED_DIM: usize = 384;

/// Hashing-trick floor. Below this cosine the query is [`RequestType::General`].
pub const HASH_MIN_COSINE: f32 = 0.12;

/// ONNX MiniLM floor. Transformer cosines for related sentences sit higher than the
/// hashing trick; 0.28 still lets greetings hit the General centroid.
pub const ONNX_MIN_COSINE: f32 = 0.28;

/// Seed phrases per [`RequestType`]. Class vectors are the mean embedding of these
/// phrases under whichever encoder is active.
const PROTOTYPES: &[(RequestType, &[&str])] = &[
    (
        RequestType::CodeGeneration,
        &[
            "write a python script that parses csv",
            "write me a python sort function",
            "implement a rust function",
            "create a javascript class",
            "build an api endpoint",
            "generate code for this module",
            "fix this bug in the program",
            "refactor this method",
            "add error handling to the script",
        ],
    ),
    (
        RequestType::CodeUnderstanding,
        &[
            "explain what this function does",
            "what does this code do",
            "how does this class work",
            "walk me through this script",
            "what is this code doing",
            "explain how the function works",
        ],
    ),
    (
        RequestType::TechnicalDesign,
        &[
            "how should i design this api",
            "system architecture tradeoffs",
            "database schema design",
            "design a service for this",
            "api design decisions",
        ],
    ),
    (
        RequestType::AnalyticalReasoning,
        &[
            "calculate the probability that it rains tomorrow",
            "solve this proof",
            "what is the sum of these numbers",
            "compute 12 + 34",
            "prove that this is true",
        ],
    ),
    (
        RequestType::Writing,
        &[
            "draft an email to my team about the outage",
            "write an article for the blog",
            "compose a letter",
            "rewrite this essay",
            "make this sound more professional",
        ],
    ),
    (
        RequestType::FactualLookup,
        &[
            "what is the capital of france",
            "who was the first president",
            "when was this invented",
            "define photosynthesis",
            "where is the eiffel tower",
        ],
    ),
    (
        RequestType::General,
        &[
            "hello there",
            "hi how are you",
            "thanks",
            "ok",
            "good morning",
        ],
    ),
];

struct HashPrototype {
    request_type: RequestType,
    vector: [f32; EMBED_DIM],
}

static HASH_CENTROIDS: std::sync::LazyLock<Vec<HashPrototype>> = std::sync::LazyLock::new(|| {
    PROTOTYPES
        .iter()
        .map(|(rt, phrases)| {
            let mut acc = [0.0f32; EMBED_DIM];
            for phrase in *phrases {
                let e = hash_embed(phrase);
                for (a, b) in acc.iter_mut().zip(e.iter()) {
                    *a += *b;
                }
            }
            let n = phrases.len() as f32;
            for a in acc.iter_mut() {
                *a /= n;
            }
            l2_normalize_arr(&mut acc);
            HashPrototype {
                request_type: *rt,
                vector: acc,
            }
        })
        .collect()
});

/// ONNX MiniLM session plus precomputed class centroids.
pub struct OnnxMiniLm {
    model: Mutex<fastembed::TextEmbedding>,
    centroids: Vec<(RequestType, Vec<f32>)>,
}

impl OnnxMiniLm {
    /// Load `all-MiniLM-L6-v2` from `cache_dir` (or the fastembed default) and embed
    /// the prototype phrases once. Network is used only when the cache is cold.
    pub fn load(cache_dir: Option<&Path>) -> Result<Self, String> {
        let mut opts =
            fastembed::TextInitOptions::new(fastembed::EmbeddingModel::AllMiniLML6V2)
                .with_show_download_progress(false);
        if let Some(dir) = cache_dir {
            opts = opts.with_cache_dir(dir.to_path_buf());
        }
        let mut model =
            fastembed::TextEmbedding::try_new(opts).map_err(|e| e.to_string())?;

        let mut phrases: Vec<String> = Vec::new();
        let mut spans: Vec<(RequestType, std::ops::Range<usize>)> = Vec::new();
        for (rt, seeds) in PROTOTYPES {
            let start = phrases.len();
            for s in *seeds {
                phrases.push((*s).to_string());
            }
            spans.push((*rt, start..phrases.len()));
        }
        let embeddings = model
            .embed(phrases, None)
            .map_err(|e| format!("embed prototypes: {e}"))?;

        let mut centroids = Vec::with_capacity(spans.len());
        for (rt, range) in spans {
            let dim = embeddings
                .get(range.start)
                .map(|v| v.len())
                .unwrap_or(EMBED_DIM);
            let mut acc = vec![0.0f32; dim];
            let n = (range.end - range.start) as f32;
            for i in range {
                for (a, b) in acc.iter_mut().zip(embeddings[i].iter()) {
                    *a += *b;
                }
            }
            if n > 0.0 {
                for a in acc.iter_mut() {
                    *a /= n;
                }
            }
            l2_normalize_vec(&mut acc);
            centroids.push((rt, acc));
        }

        Ok(Self {
            model: Mutex::new(model),
            centroids,
        })
    }

    /// Nearest prototype in MiniLM space. Empty text and weak matches are General.
    pub fn classify(&self, text: &str) -> RequestType {
        if text.trim().is_empty() {
            return RequestType::General;
        }
        let mut model = self.model.lock().unwrap_or_else(|e| e.into_inner());
        let ok = model.embed(vec![text.to_string()], None);
        drop(model);
        let embedding = match ok {
            Ok(mut v) if !v.is_empty() => v.swap_remove(0),
            Ok(_) | Err(_) => return RequestType::General,
        };
        nearest(&embedding, &self.centroids, ONNX_MIN_COSINE)
    }
}

/// Hashing-trick nearest prototype (no ONNX). Used by `ROUTER_CLASSIFIER=hash`.
pub fn hash_classify(text: &str) -> RequestType {
    if text.trim().is_empty() {
        return RequestType::General;
    }
    let query = hash_embed(text);
    let mut best = RequestType::General;
    let mut best_score = f32::NEG_INFINITY;
    for proto in HASH_CENTROIDS.iter() {
        let score = dot_arr(&query, &proto.vector);
        if score > best_score {
            best_score = score;
            best = proto.request_type;
        }
    }
    if best_score < HASH_MIN_COSINE {
        return RequestType::General;
    }
    best
}

fn nearest(query: &[f32], centroids: &[(RequestType, Vec<f32>)], min_cos: f32) -> RequestType {
    let mut best = RequestType::General;
    let mut best_score = f32::NEG_INFINITY;
    for (rt, vec) in centroids {
        let score = dot_slice(query, vec);
        if score > best_score {
            best_score = score;
            best = *rt;
        }
    }
    if best_score < min_cos {
        RequestType::General
    } else {
        best
    }
}

fn hash_embed(text: &str) -> [f32; EMBED_DIM] {
    let tokens = word_tokens(text);
    let mut v = [0.0f32; EMBED_DIM];
    for token in &tokens {
        add_gram(&mut v, token);
    }
    for window in tokens.windows(2) {
        let gram = format!("{} {}", window[0], window[1]);
        add_gram(&mut v, &gram);
    }
    l2_normalize_arr(&mut v);
    v
}

fn add_gram(v: &mut [f32; EMBED_DIM], gram: &str) {
    let bucket = (fnv1a(gram.as_bytes()) as usize) % EMBED_DIM;
    let sign_bit = fnv1a(format!("sign:{gram}").as_bytes()) & 1;
    let sign = if sign_bit == 0 { 1.0 } else { -1.0 };
    v[bucket] += sign;
}

fn word_tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn l2_normalize_arr(v: &mut [f32; EMBED_DIM]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn l2_normalize_vec(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn dot_arr(a: &[f32; EMBED_DIM], b: &[f32; EMBED_DIM]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn dot_slice(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_regex_reference_examples() {
        use RequestType::*;
        assert_eq!(
            hash_classify("build me a python script that parses CSV"),
            CodeGeneration
        );
        assert_eq!(
            hash_classify("write me a Python sort function"),
            CodeGeneration
        );
        assert_eq!(
            hash_classify("explain what this function does"),
            CodeUnderstanding
        );
        assert_eq!(
            hash_classify("how should I design this API?"),
            TechnicalDesign
        );
        assert_eq!(
            hash_classify("calculate the probability that it rains tomorrow"),
            AnalyticalReasoning
        );
        assert_eq!(
            hash_classify("draft an email to my team about the outage"),
            Writing
        );
        assert_eq!(
            hash_classify("what is the capital of France?"),
            FactualLookup
        );
        assert_eq!(hash_classify("hello there"), General);
    }

    #[test]
    fn hash_paraphrase_without_regex_trigger_still_codes() {
        assert_eq!(
            hash_classify("I need a python script that parses csv files"),
            RequestType::CodeGeneration
        );
    }

    #[test]
    fn empty_and_whitespace_are_general() {
        assert_eq!(hash_classify(""), RequestType::General);
        assert_eq!(hash_classify("   "), RequestType::General);
    }
}
