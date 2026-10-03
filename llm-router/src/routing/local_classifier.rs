//! In-process request classifier: hashed n-gram features into two small linear models (request
//! type, complexity), embedded in the binary. Microseconds per call, no network, no sidecar.
//!
//! Features are computed here and **only** here — the training pipeline
//! (`examples/local_classifier_features.rs` → `eval/train_local.py`) dumps them through this
//! same function, so training and inference cannot drift apart. The weights file is the
//! trainer's output (`assets/local_classifier.json`); changing [`features`] requires
//! retraining.
//!
//! Output is a full belief, like Laya's: a probability per request type (temperature-
//! calibrated on cross-validated training predictions) and a distribution over the five
//! complexity levels, reduced to its expected level and a confidence of `1 − normalised
//! entropy`. The tier sampler consumes both exactly as it consumes Laya's answer.

use std::collections::HashMap;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;

use super::classifier::RequestType;
use super::request_classifier::{
    Classification, ClassifierInput, ClassifierSource, RequestClassifier,
};
use super::salience_classifier::fnv1a;

/// Hash space for features. Changing it requires retraining.
pub const NUM_BUCKETS: u32 = 1 << 18;
const COMPLEXITY_LEVELS: usize = 5;
const MAX_QUERY_CHARS: usize = 1_000;
const MAX_CONTEXT_CHARS: usize = 1_500;

const EMBEDDED_MODEL_JSON: &str = include_str!("../../assets/local_classifier.json");

fn bucket(feature: &str) -> u32 {
    (fnv1a(feature.as_bytes()) % u64::from(NUM_BUCKETS)) as u32
}

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Coarse log-scale bucket for a count: 0, 1, 2–3, 4–7, 8–15, ….
fn log_bucket(n: usize) -> u32 {
    usize::BITS - n.leading_zeros()
}

/// The hashed feature vector for a request and its (optional) context, as sorted
/// `(bucket, value)` pairs with sublinear term frequency and unit L2 norm. Sorted so the
/// model's dot products are summed in a fixed order — bit-identical across runs.
///
/// - query: word unigrams + bigrams, char 3–5-grams (typos, morphology, non-Latin scripts),
///   the first word (imperative verb), and shape signals — length, requirement separators,
///   question mark, code-ish characters;
/// - context: word unigrams in their own namespace, its length and whether it holds code.
pub fn features(query: &str, context: &str) -> Vec<(u32, f32)> {
    let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
    let context: String = context.chars().take(MAX_CONTEXT_CHARS).collect();
    let mut grams: Vec<String> = Vec::new();

    let qw = words(&query);
    grams.extend(qw.iter().map(|w| format!("w:{w}")));
    grams.extend(qw.windows(2).map(|p| format!("b:{} {}", p[0], p[1])));
    let lower: Vec<char> = query.to_lowercase().chars().collect();
    for n in 3..=5 {
        grams.extend(
            lower
                .windows(n)
                .map(|g| format!("c:{}", g.iter().collect::<String>())),
        );
    }
    if let Some(first) = qw.first() {
        grams.push(format!("first:{first}"));
    }
    grams.push(format!("qlen:{}", log_bucket(qw.len())));
    let separators = query.matches([',', ';', '\n']).count()
        + qw.iter()
            .filter(|w| matches!(w.as_str(), "and" | "then" | "also" | "plus"))
            .count();
    grams.push(format!("qsep:{}", log_bucket(separators)));
    if query.trim_end().ends_with('?') {
        grams.push("q:question".into());
    }
    let code_chars = query
        .matches(['`', '{', '}', '(', ')', ';', '=', '<', '>'])
        .count();
    grams.push(format!("qcode:{}", log_bucket(code_chars)));

    let cw = words(&context);
    grams.extend(cw.iter().map(|w| format!("x:{w}")));
    grams.push(format!("clen:{}", log_bucket(cw.len())));
    if context.contains("```") || context.matches('`').count() >= 2 {
        grams.push("x:has_code".into());
    }

    let mut counts: HashMap<u32, f32> = HashMap::new();
    for g in &grams {
        *counts.entry(bucket(g)).or_default() += 1.0;
    }
    let mut feats: Vec<(u32, f32)> = counts.into_iter().map(|(b, n)| (b, 1.0 + n.ln())).collect();
    feats.sort_unstable_by_key(|(b, _)| *b);
    let norm = feats.iter().map(|(_, v)| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for (_, v) in &mut feats {
            *v /= norm;
        }
    }
    feats
}

/// The request and its context, recovered from a [`ClassifierInput`]: the state is the
/// latest request (`Latest request:\n<query>`) followed, after a blank line, by one header
/// line and the context body — the shape both `classify_input` (live conversations) and
/// [`eval_state`] (eval cases) build.
fn context_of<'a>(input: &ClassifierInput<'a>) -> &'a str {
    let head: String = input.query.chars().take(MAX_QUERY_CHARS).collect();
    let prefix = format!("Latest request:\n{head}");
    let Some(rest) = input.state.strip_prefix(prefix.as_str()) else {
        return "";
    };
    let rest = rest.trim_start_matches('\n');
    rest.split_once('\n').map_or("", |(_header, body)| body)
}

/// The classifier state for an eval/training case with a free-text `context`: the request
/// first, then the context under a `Context:` header, bounded like `classify_input`.
pub fn eval_state(query: &str, context: Option<&str>) -> String {
    let mut s = format!(
        "Latest request:\n{}",
        query.chars().take(MAX_QUERY_CHARS).collect::<String>()
    );
    if let Some(ctx) = context.filter(|c| !c.trim().is_empty()) {
        s.push_str("\n\nContext:\n");
        s.push_str(ctx);
    }
    s.chars()
        .take(super::request_classifier::STATE_CHARS)
        .collect()
}

/// The features a trained model sees for `input` — what the training dump must record.
pub fn features_for(input: &ClassifierInput<'_>) -> Vec<(u32, f32)> {
    features(input.query, context_of(input))
}

/// On-disk model, as written by `eval/train_local.py`.
#[derive(Deserialize)]
struct ModelFile {
    version: u32,
    num_buckets: u32,
    types: Vec<String>,
    type_bias: Vec<f32>,
    complexity_bias: Vec<f32>,
    type_temperature: f32,
    complexity_temperature: f32,
    /// Buckets with non-zero weights, and their weights flattened bucket-major: per bucket,
    /// one weight per type then one per complexity level.
    buckets: Vec<u32>,
    weights: Vec<f32>,
}

pub struct LocalModel {
    types: Vec<RequestType>,
    type_bias: Vec<f32>,
    complexity_bias: Vec<f32>,
    type_temperature: f32,
    complexity_temperature: f32,
    weights: HashMap<u32, Vec<f32>>,
}

impl LocalModel {
    pub fn from_json(json: &str) -> Result<Self, String> {
        let m: ModelFile = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if m.version != 1 || m.num_buckets != NUM_BUCKETS {
            return Err(format!(
                "model version {} / {} buckets; expected 1 / {NUM_BUCKETS}",
                m.version, m.num_buckets
            ));
        }
        let types = m
            .types
            .iter()
            .map(|t| RequestType::from_wire(t).ok_or_else(|| format!("unknown type {t}")))
            .collect::<Result<Vec<_>, _>>()?;
        let width = types.len() + COMPLEXITY_LEVELS;
        if types.is_empty()
            || m.type_bias.len() != types.len()
            || m.complexity_bias.len() != COMPLEXITY_LEVELS
            || m.weights.len() != m.buckets.len() * width
            || !(m.type_temperature > 0.0 && m.complexity_temperature > 0.0)
        {
            return Err("model shape mismatch".into());
        }
        let weights = m
            .buckets
            .iter()
            .zip(m.weights.chunks_exact(width))
            .map(|(b, w)| (*b, w.to_vec()))
            .collect();
        Ok(Self {
            types,
            type_bias: m.type_bias,
            complexity_bias: m.complexity_bias,
            type_temperature: m.type_temperature,
            complexity_temperature: m.complexity_temperature,
            weights,
        })
    }

    /// The model compiled into this binary.
    pub fn embedded() -> Result<&'static LocalModel, String> {
        static MODEL: OnceLock<Result<LocalModel, String>> = OnceLock::new();
        MODEL
            .get_or_init(|| LocalModel::from_json(EMBEDDED_MODEL_JSON))
            .as_ref()
            .map_err(Clone::clone)
    }

    pub fn classify_features(&self, feats: &[(u32, f32)]) -> Classification {
        let nt = self.types.len();
        let mut type_logits = self.type_bias.clone();
        let mut cx_logits = self.complexity_bias.clone();
        for (b, v) in feats {
            if let Some(w) = self.weights.get(b) {
                for (k, l) in type_logits.iter_mut().enumerate() {
                    *l += v * w[k];
                }
                for (j, l) in cx_logits.iter_mut().enumerate() {
                    *l += v * w[nt + j];
                }
            }
        }
        let type_p = softmax(&type_logits, self.type_temperature);
        let cx_p = softmax(&cx_logits, self.complexity_temperature);
        let (best, confidence) =
            type_p.iter().enumerate().fold(
                (0, f64::MIN),
                |acc, (i, p)| if *p > acc.1 { (i, *p) } else { acc },
            );
        let level: f64 = cx_p.iter().enumerate().map(|(j, p)| j as f64 * p).sum();
        let entropy: f64 = -cx_p
            .iter()
            .filter(|p| **p > 0.0)
            .map(|p| p * p.ln())
            .sum::<f64>();
        Classification {
            request_type: self.types[best],
            type_probabilities: Some(self.types.iter().copied().zip(type_p).collect()),
            complexity: level.round() as u8 + 1,
            complexity_level: level,
            complexity_confidence: (1.0 - entropy / (COMPLEXITY_LEVELS as f64).ln())
                .clamp(0.0, 1.0),
            confidence,
            source: ClassifierSource::Local,
        }
    }
}

fn softmax(logits: &[f32], temperature: f32) -> Vec<f64> {
    let scaled: Vec<f64> = logits
        .iter()
        .map(|l| f64::from(*l) / f64::from(temperature))
        .collect();
    let max = scaled.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let exp: Vec<f64> = scaled.iter().map(|l| (l - max).exp()).collect();
    let sum: f64 = exp.iter().sum();
    exp.into_iter().map(|e| e / sum).collect()
}

/// The embedded model as a [`RequestClassifier`]. Pure CPU, microseconds, cannot fail.
pub struct LocalClassifier {
    model: &'static LocalModel,
}

impl LocalClassifier {
    pub fn embedded() -> Result<Self, String> {
        LocalModel::embedded().map(|model| Self { model })
    }

    pub fn classify_sync(&self, input: &ClassifierInput<'_>) -> Classification {
        self.model.classify_features(&features_for(input))
    }
}

#[async_trait]
impl RequestClassifier for LocalClassifier {
    async fn classify(&self, input: &ClassifierInput<'_>) -> Classification {
        self.classify_sync(input)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn features_are_sorted_unit_norm_and_deterministic() {
        let f = features("Fix the typo in this comment", "```rust\n// retrun x\n```");
        assert!(
            f.windows(2).all(|w| w[0].0 < w[1].0),
            "sorted, no duplicates"
        );
        let norm: f32 = f.iter().map(|(_, v)| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
        assert_eq!(
            f,
            features("Fix the typo in this comment", "```rust\n// retrun x\n```")
        );
        assert!(f.iter().all(|(b, _)| *b < NUM_BUCKETS));
        assert_ne!(
            f,
            features("Fix the typo in this comment", ""),
            "context matters"
        );
        assert!(
            !features("नमस्ते, आप कैसे हैं?", "").is_empty(),
            "non-Latin scripts"
        );
    }

    #[test]
    fn context_is_recovered_from_both_state_shapes() {
        let state = eval_state("explain this", Some("fn f() {}"));
        let input = ClassifierInput {
            query: "explain this",
            state: &state,
        };
        assert_eq!(context_of(&input), "fn f() {}");
        let live = "Latest request:\nmake it shorter\n\nEarlier conversation (newest first):\n\
                    [assistant] Here's a draft toast\n[user] draft a toast";
        let input = ClassifierInput {
            query: "make it shorter",
            state: live,
        };
        assert_eq!(
            context_of(&input),
            "[assistant] Here's a draft toast\n[user] draft a toast"
        );
        let bare = ClassifierInput {
            query: "hi",
            state: "hi",
        };
        assert_eq!(context_of(&bare), "");
    }

    /// A hand-built model: one feature pushes towards `writing` / complexity 5.
    fn toy_model() -> LocalModel {
        let types = ["code_generation", "writing"];
        let b = bucket("w:poem");
        let mut weights = vec![0.0, 8.0];
        weights.extend([0.0, 0.0, 0.0, 0.0, 8.0]);
        LocalModel::from_json(
            &json!({
                "version": 1, "num_buckets": NUM_BUCKETS, "types": types,
                "type_bias": [0.0, 0.0], "complexity_bias": [0.0, 0.0, 0.0, 0.0, 0.0],
                "type_temperature": 1.0, "complexity_temperature": 1.0,
                "buckets": [b], "weights": weights,
            })
            .to_string(),
        )
        .unwrap()
    }

    #[test]
    fn classification_is_a_calibrated_belief() {
        let m = toy_model();
        let c = m.classify_features(&[(bucket("w:poem"), 1.0)]);
        assert_eq!(c.request_type, RequestType::Writing);
        assert_eq!(c.source, ClassifierSource::Local);
        let probs = c.type_probabilities.unwrap();
        assert!((probs.iter().map(|(_, p)| p).sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(c.confidence > 0.99);
        assert!(c.complexity_level > 3.9 && c.complexity == 5);
        assert!(c.complexity_confidence > 0.9);

        let unknown = m.classify_features(&[(12345, 1.0)]);
        assert!(
            (unknown.confidence - 0.5).abs() < 1e-9,
            "no evidence = uniform"
        );
        assert!(unknown.complexity_confidence < 1e-9);
        assert_eq!(unknown.complexity, 3);
    }

    #[test]
    fn malformed_models_are_rejected() {
        for bad in [
            json!({"version": 2}),
            json!({
                "version": 1, "num_buckets": 7, "types": ["writing"], "type_bias": [0.0],
                "complexity_bias": [0.0, 0.0, 0.0, 0.0, 0.0], "type_temperature": 1.0,
                "complexity_temperature": 1.0, "buckets": [], "weights": []
            }),
            json!({
                "version": 1, "num_buckets": NUM_BUCKETS, "types": ["poetry"],
                "type_bias": [0.0], "complexity_bias": [0.0, 0.0, 0.0, 0.0, 0.0], "type_temperature": 1.0,
                "complexity_temperature": 1.0, "buckets": [], "weights": []
            }),
            json!({
                "version": 1, "num_buckets": NUM_BUCKETS, "types": ["writing"],
                "type_bias": [0.0], "complexity_bias": [0.0, 0.0, 0.0, 0.0, 0.0], "type_temperature": 1.0,
                "complexity_temperature": 1.0, "buckets": [1], "weights": [0.0]
            }),
        ] {
            assert!(LocalModel::from_json(&bad.to_string()).is_err(), "{bad}");
        }
    }

    #[test]
    fn embedded_model_loads() {
        let model = LocalModel::embedded().expect("embedded model");
        assert_eq!(model.types.len(), 7, "all seven request types");
    }
}
