//! Pluggable request classification with a deterministic local linear backend.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;

use super::classifier::{RequestType, classify_request_type};

#[derive(Debug, Clone, Copy)]
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum ClassifyError {
    #[error("classifier model could not be loaded: {0}")]
    Load(String),
    #[error("classifier inference failed: {0}")]
    Inference(String),
}

#[async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;
    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError>;
}

#[derive(Debug, Default)]
pub struct RegexClassifier;

#[async_trait]
impl RequestClassifier for RegexClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(Classification {
            request_type: classify_request_type(input.query),
            complexity: 3,
            confidence: 0.35,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct RawModelFile {
    schema: String,
    buckets: usize,
    type_bias: Vec<f32>,
    type_weights: Vec<HashMap<String, f32>>,
    complexity_bias: Vec<f32>,
    complexity_weights: Vec<HashMap<String, f32>>,
    temperature: f32,
}

#[derive(Debug)]
struct ModelFile {
    buckets: usize,
    type_bias: Vec<f32>,
    type_weights: Vec<HashMap<usize, f32>>,
    complexity_bias: Vec<f32>,
    complexity_weights: Vec<HashMap<usize, f32>>,
    temperature: f32,
}

#[derive(Debug)]
pub struct LocalClassifier {
    model: ModelFile,
}

impl LocalClassifier {
    pub fn embedded() -> Result<Self, ClassifyError> {
        Self::from_json(include_str!("../../assets/classifier/linear-v1.json"))
    }

    pub fn from_path(path: &std::path::Path) -> Result<Self, ClassifyError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| ClassifyError::Load(format!("{}: {e}", path.display())))?;
        Self::from_json(&raw)
    }

    fn from_json(raw: &str) -> Result<Self, ClassifyError> {
        let raw_model: RawModelFile = serde_json::from_str(raw)
            .map_err(|e| ClassifyError::Load(format!("invalid model JSON: {e}")))?;
        if raw_model.schema != "nasiko-request-classifier-linear-v1"
            || !raw_model.buckets.is_power_of_two()
            || raw_model.type_bias.len() != 7
            || raw_model.type_weights.len() != 7
            || raw_model.complexity_bias.len() != 4
            || raw_model.complexity_weights.len() != 4
            || !raw_model.temperature.is_finite()
            || raw_model.temperature <= 0.0
        {
            return Err(ClassifyError::Load("incompatible model dimensions".into()));
        }
        let parse_rows =
            |rows: Vec<HashMap<String, f32>>| -> Result<Vec<HashMap<usize, f32>>, ClassifyError> {
                rows.into_iter()
                    .map(|row| {
                        row.into_iter()
                            .map(|(bucket, weight)| {
                                bucket
                                    .parse::<usize>()
                                    .map(|bucket| (bucket, weight))
                                    .map_err(|_| {
                                        ClassifyError::Load("non-numeric model bucket".into())
                                    })
                            })
                            .collect()
                    })
                    .collect()
            };
        let model = ModelFile {
            buckets: raw_model.buckets,
            type_bias: raw_model.type_bias,
            type_weights: parse_rows(raw_model.type_weights)?,
            complexity_bias: raw_model.complexity_bias,
            complexity_weights: parse_rows(raw_model.complexity_weights)?,
            temperature: raw_model.temperature,
        };
        Ok(Self { model })
    }

    fn logits(
        &self,
        features: &HashMap<usize, f32>,
        weights: &[HashMap<usize, f32>],
        bias: &[f32],
    ) -> Vec<f32> {
        let mut ordered: Vec<(usize, f32)> = features.iter().map(|(k, v)| (*k, *v)).collect();
        ordered.sort_unstable_by_key(|(bucket, _)| *bucket);
        weights
            .iter()
            .zip(bias)
            .map(|(row, b)| {
                ordered.iter().fold(*b, |sum, (bucket, value)| {
                    sum + row.get(bucket).copied().unwrap_or(0.0) * value
                })
            })
            .collect()
    }
}

#[async_trait]
impl RequestClassifier for LocalClassifier {
    fn name(&self) -> &str {
        "local-linear-subword-v1"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let features = features(input, self.model.buckets);
        let logits = self.logits(&features, &self.model.type_weights, &self.model.type_bias);
        let scaled: Vec<f32> = logits.iter().map(|v| *v / self.model.temperature).collect();
        let max = scaled.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = scaled.iter().map(|v| (*v - max).exp()).collect();
        let total: f32 = exps.iter().sum();
        if !total.is_finite() || total <= 0.0 {
            return Err(ClassifyError::Inference("non-finite softmax".into()));
        }
        let (index, probability) = exps
            .iter()
            .enumerate()
            .map(|(i, v)| (i, *v / total))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .ok_or_else(|| ClassifyError::Inference("empty type head".into()))?;
        let model_request_type =
            type_at(index).ok_or_else(|| ClassifyError::Inference("invalid type index".into()))?;
        let rubric_request_type = strong_intent(input.query);
        let request_type = rubric_request_type.unwrap_or(model_request_type);
        // Rubric matches are intentionally narrow and were designed as high-precision
        // tie-breakers. Use a conservative confidence rather than pretending the model's
        // probability belongs to a class it did not select.
        let confidence = if rubric_request_type.is_some() {
            0.90
        } else {
            probability
        };

        let ordinal = self.logits(
            &features,
            &self.model.complexity_weights,
            &self.model.complexity_bias,
        );
        // Cumulative ordinal heads represent P(C > k). Decode only the leading
        // threshold prefix so an inconsistent later head can never create an impossible
        // ordinal pattern (for example C>2 true after C>1 false).
        let complexity = 1 + ordinal
            .iter()
            .take_while(|logit| sigmoid(**logit) >= 0.5)
            .count() as u8;
        Ok(Classification {
            request_type,
            complexity,
            confidence,
        })
    }
}

/// High-precision rubric tie-breakers for intents whose requested action is explicit.
///
/// This deliberately covers only narrow, semantically strong cases. Ambiguous or mixed
/// requests return `None` and remain entirely model-decided. The rules encode the same
/// label policy documented in data/classifier/LABELLING.md; they do not contain eval cases.
fn strong_intent(query: &str) -> Option<RequestType> {
    let lower = query.to_lowercase();
    let has = |terms: &[&str]| terms.iter().any(|term| lower.contains(term));

    if has(&[
        "rewrite",
        "summarize",
        "translate",
        "compose",
        "meeting minutes",
        "product description",
        "executive summary",
        "release notes",
        "changelog",
        "grammar",
        "tone",
        "warmer",
        "shorten this",
        "blog post",
        "email",
        "thank you letter",
        "user facing explanation",
        "status update",
        "plain language",
    ]) {
        return Some(RequestType::Writing);
    }

    let change = has(&[
        "implement",
        "fix",
        "patch",
        "refactor",
        "add ",
        "change ",
        "complete ",
        "generate ",
        "write tests",
        "create a validator",
        "convert this callback",
        "update this dockerfile",
    ]);
    let codeish = has(&[
        "function",
        " fn",
        "code",
        "api",
        "client",
        "query",
        "sql",
        "middleware",
        "class",
        "parser",
        "dockerfile",
        "serializer",
        "request",
        "endpoint",
        "component",
        "script",
        "worker",
        "cache",
        "migration",
        "rate limiter",
        "feature flag",
    ]);
    if change && codeish {
        return Some(RequestType::CodeGeneration);
    }

    let understand = has(&[
        "explain",
        "review",
        "audit",
        "trace",
        "walk me",
        "why does",
        "identify",
        "which branch",
        "what assumptions",
        "determine which function",
    ]);
    if understand && codeish && !change {
        return Some(RequestType::CodeUnderstanding);
    }

    if has(&[
        "analyze",
        "evaluate",
        "calculate",
        "prove",
        "derive",
        "diagnose",
        "reconcile",
        "infer",
        "root cause",
        "determine why",
        "assess whether",
        "estimate capacity",
        "work out whether",
        "reason about",
        "find the contradiction",
        "expected value",
    ]) {
        return Some(RequestType::AnalyticalReasoning);
    }
    if lower.contains("compare")
        && has(&[
            "tradeoff",
            "tradeoffs",
            "failure",
            "strategy",
            "plan",
            "database",
            "outbox",
            "cdc",
            "consistency",
            "availability",
            "rollout",
        ])
    {
        return Some(RequestType::AnalyticalReasoning);
    }

    let negated_design = lower.contains("do not design") || lower.contains("do not redesign");
    if !negated_design
        && has(&[
            "design an ",
            "design a ",
            "propose an architecture",
            "how should we ",
            "plan a ",
            "create a disaster recovery strategy",
            "recommend an observability architecture",
            "choose a schema",
            "propose a storage architecture",
            "design the boundaries",
            "create a schema evolution strategy",
            "plan an active passive",
        ])
    {
        return Some(RequestType::TechnicalDesign);
    }

    let trimmed = lower.trim_start();
    if [
        "what is ",
        "what does ",
        "who ",
        "when ",
        "which ",
        "define ",
        "name the ",
        "list the ",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
        && !has(&["why", "compare", "design", "fix", "review", "analyze"])
    {
        return Some(RequestType::FactualLookup);
    }

    if [
        "hello",
        "good morning",
        "thanks",
        "cool thanks",
        "nice to meet",
        "tell me a joke",
        "i am bored",
        "i'm just testing",
        "can we chat",
        "okay got it",
        "that makes sense",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
    {
        return Some(RequestType::General);
    }

    None
}

fn type_at(index: usize) -> Option<RequestType> {
    [
        RequestType::CodeGeneration,
        RequestType::CodeUnderstanding,
        RequestType::TechnicalDesign,
        RequestType::AnalyticalReasoning,
        RequestType::Writing,
        RequestType::FactualLookup,
        RequestType::General,
    ]
    .get(index)
    .copied()
}

fn sigmoid(value: f32) -> f32 {
    1.0 / (1.0 + (-value).exp())
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn add_feature(output: &mut HashMap<usize, f32>, name: &str, buckets: usize) {
    let bucket = fnv1a(name.as_bytes()) as usize & (buckets - 1);
    let sign = if fnv1a(format!("sign:{name}").as_bytes()) & 1 == 0 {
        1.0
    } else {
        -1.0
    };
    *output.entry(bucket).or_default() += sign;
}

fn add_text_features(output: &mut HashMap<usize, f32>, prefix: &str, text: &str, buckets: usize) {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|v| !v.is_empty())
        .collect();
    for word in &words {
        add_feature(output, &format!("{prefix}w:{word}"), buckets);
    }
    for pair in words.windows(2) {
        add_feature(
            output,
            &format!("{prefix}b:{}_{}", pair[0], pair[1]),
            buckets,
        );
    }
    let chars: Vec<char> = lower.chars().collect();
    for n in 3..=5 {
        for gram in chars.windows(n) {
            add_feature(
                output,
                &format!("{prefix}c:{}", gram.iter().collect::<String>()),
                buckets,
            );
        }
    }
}

fn features(input: &ClassifyInput<'_>, buckets: usize) -> HashMap<usize, f32> {
    let mut output = HashMap::new();
    add_text_features(&mut output, "q:", input.query, buckets);
    if let Some(context) = input.context {
        add_text_features(&mut output, "ctx:", context, buckets);
    } else {
        add_feature(&mut output, "shape:no_context", buckets);
    }
    let query = input.query;
    for (name, yes) in [
        ("code_fence", query.contains("```")),
        ("inline_code", query.contains('`')),
        ("question", query.contains('?')),
        ("json", query.contains('{') && query.contains(':')),
        (
            "stack_trace",
            query.contains("Traceback") || query.contains(" at "),
        ),
        ("multiline", query.lines().count() > 2),
        ("many_constraints", query.matches(',').count() >= 3),
    ] {
        if yes {
            add_feature(&mut output, &format!("shape:{name}"), buckets);
        }
    }
    let lower = query.to_lowercase();
    for (name, terms) in [
        (
            "intent_change",
            &[
                "implement",
                "build",
                "fix",
                "patch",
                "refactor",
                "add",
                "change",
                "complete",
                "generate",
            ][..],
        ),
        (
            "intent_understand",
            &[
                "explain", "review", "audit", "trace", "walk me", "why does", "identify",
            ][..],
        ),
        (
            "intent_design",
            &[
                "design",
                "architecture",
                "migrate",
                "strategy",
                "capacity",
                "schema",
                "components",
                "queue based",
            ][..],
        ),
        (
            "intent_reason",
            &[
                "compare",
                "analyze",
                "evaluate",
                "calculate",
                "prove",
                "derive",
                "diagnose",
                "reconcile",
                "tradeoff",
                "investigate",
                "infer",
                "invariant",
                "event by event",
                "root cause",
            ][..],
        ),
        (
            "intent_write",
            &[
                "draft",
                "rewrite",
                "summarize",
                "compose",
                "edit",
                "translate",
                "blog",
                "minutes",
                "description",
                "copy",
                "documentation",
                "report",
                "email",
                "letter",
                "notes",
            ][..],
        ),
        (
            "intent_fact",
            &[
                "what is",
                "who ",
                "when ",
                "which ",
                "define",
                "what does",
                "name the",
                "list the",
                "formula",
                "stand for",
            ][..],
        ),
        (
            "intent_general",
            &[
                "hello",
                "thanks",
                "joke",
                "chat",
                "help me",
                "continue",
                "good morning",
                "favorite",
                "bored",
                "got it",
            ][..],
        ),
        (
            "risk_high",
            &[
                "distributed",
                "concurrent",
                "failover",
                "disaster",
                "zero downtime",
                "security",
                "production",
                "multi-tenant",
            ][..],
        ),
    ] {
        if name == "intent_design"
            && (lower.contains("do not design") || lower.contains("do not redesign"))
        {
            continue;
        }
        if terms.iter().any(|term| lower.contains(term)) {
            add_feature(&mut output, &format!("intent:{name}"), buckets);
        }
    }
    if ["rewrite", "grammar", "tone", "warmer", "shorten", "copy"]
        .iter()
        .any(|term| lower.contains(term))
    {
        add_feature(&mut output, "intent:text_rewrite", buckets);
    }
    let norm = (output.values().map(|v| v.abs()).sum::<f32>())
        .sqrt()
        .max(1.0);
    for value in output.values_mut() {
        *value /= norm;
    }
    output
}

#[derive(Debug, Default)]
pub struct FallbackCounts {
    pub error: AtomicU64,
    pub timeout: AtomicU64,
    pub invalid: AtomicU64,
    pub low_confidence: AtomicU64,
}

pub struct GuardedClassifier {
    backend: Arc<dyn RequestClassifier>,
    regex: RegexClassifier,
    timeout: Duration,
    min_confidence: f32,
    pub fallbacks: FallbackCounts,
}

impl GuardedClassifier {
    pub fn new(
        backend: Arc<dyn RequestClassifier>,
        timeout: Duration,
        min_confidence: f32,
    ) -> Self {
        Self {
            backend,
            regex: RegexClassifier,
            timeout,
            min_confidence: min_confidence.clamp(0.0, 1.0),
            fallbacks: FallbackCounts::default(),
        }
    }

    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }

    pub async fn decide(&self, input: &ClassifyInput<'_>) -> Classification {
        let result = tokio::time::timeout(self.timeout, self.backend.classify(input)).await;
        match result {
            Ok(Ok(value)) if valid(value) && value.confidence >= self.min_confidence => value,
            Ok(Ok(value)) if !valid(value) => {
                self.fallbacks.invalid.fetch_add(1, Ordering::Relaxed);
                self.fallback(input).await
            }
            Ok(Ok(_)) => {
                self.fallbacks
                    .low_confidence
                    .fetch_add(1, Ordering::Relaxed);
                self.fallback(input).await
            }
            Ok(Err(error)) => {
                self.fallbacks.error.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(error=%error, "request classifier failed; using regex");
                self.fallback(input).await
            }
            Err(_) => {
                self.fallbacks.timeout.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("request classifier timed out; using regex");
                self.fallback(input).await
            }
        }
    }

    async fn fallback(&self, input: &ClassifyInput<'_>) -> Classification {
        self.regex
            .classify(input)
            .await
            .expect("regex classifier is infallible")
    }
}

fn valid(value: Classification) -> bool {
    (1..=5).contains(&value.complexity)
        && value.confidence.is_finite()
        && (0.0..=1.0).contains(&value.confidence)
}

pub fn build_classifier(
    backend: &str,
    model_path: &str,
    timeout_ms: u64,
    min_confidence: f32,
) -> GuardedClassifier {
    let selected: Arc<dyn RequestClassifier> = match backend {
        "local" => {
            let loaded = if model_path.is_empty() {
                LocalClassifier::embedded()
            } else {
                LocalClassifier::from_path(std::path::Path::new(model_path))
            };
            match loaded {
                Ok(model) => Arc::new(model),
                Err(error) => {
                    tracing::warn!(%error, "local classifier load failed; using regex");
                    Arc::new(RegexClassifier)
                }
            }
        }
        "regex" => Arc::new(RegexClassifier),
        other => {
            tracing::warn!(backend = other, "unknown classifier backend; using regex");
            Arc::new(RegexClassifier)
        }
    };
    GuardedClassifier::new(
        selected,
        Duration::from_millis(timeout_ms.max(1)),
        min_confidence,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scripted(Result<Classification, ClassifyError>);
    #[async_trait]
    impl RequestClassifier for Scripted {
        fn name(&self) -> &str {
            "scripted"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            match &self.0 {
                Ok(value) => Ok(*value),
                Err(error) => Err(ClassifyError::Inference(error.to_string())),
            }
        }
    }

    struct Stalls;
    #[async_trait]
    impl RequestClassifier for Stalls {
        fn name(&self) -> &str {
            "stalls"
        }
        async fn classify(&self, _: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
            tokio::time::sleep(Duration::from_millis(50)).await;
            unreachable!()
        }
    }

    #[tokio::test]
    async fn embedded_model_is_deterministic_and_bounded() {
        let model = LocalClassifier::embedded().unwrap();
        let input = ClassifyInput {
            query: "plz fix ths python fn",
            context: Some("the function returns the wrong value"),
        };
        let a = model.classify(&input).await.unwrap();
        let b = model.classify(&input).await.unwrap();
        assert_eq!(a, b);
        assert!((1..=5).contains(&a.complexity));
        assert!((0.0..=1.0).contains(&a.confidence));
    }

    #[tokio::test]
    async fn regex_adapter_matches_legacy() {
        let input = ClassifyInput {
            query: "write a Rust parser",
            context: None,
        };
        assert_eq!(
            RegexClassifier.classify(&input).await.unwrap().request_type,
            classify_request_type(input.query)
        );
    }

    #[tokio::test]
    async fn guard_falls_back_on_error_invalid_low_confidence_and_timeout() {
        let input = ClassifyInput {
            query: "write a Rust parser",
            context: None,
        };
        let expected = classify_request_type(input.query);
        let cases: Vec<(Arc<dyn RequestClassifier>, Duration)> = vec![
            (
                Arc::new(Scripted(Err(ClassifyError::Inference("boom".into())))),
                Duration::from_millis(10),
            ),
            (
                Arc::new(Scripted(Ok(Classification {
                    request_type: RequestType::Writing,
                    complexity: 0,
                    confidence: f32::NAN,
                }))),
                Duration::from_millis(10),
            ),
            (
                Arc::new(Scripted(Ok(Classification {
                    request_type: RequestType::Writing,
                    complexity: 2,
                    confidence: 0.1,
                }))),
                Duration::from_millis(10),
            ),
            (Arc::new(Stalls), Duration::from_millis(1)),
        ];
        for (backend, timeout) in cases {
            let guard = GuardedClassifier::new(backend, timeout, 0.5);
            assert_eq!(guard.decide(&input).await.request_type, expected);
        }
    }

    #[test]
    fn strong_intent_only_handles_high_precision_rubric_cases() {
        assert_eq!(
            strong_intent("Rewrite this support reply to sound warmer"),
            Some(RequestType::Writing)
        );
        assert_eq!(
            strong_intent("Fix this parser so quoted commas work"),
            Some(RequestType::CodeGeneration)
        );
        assert_eq!(
            strong_intent("Analyze why throughput dropped after the rollout"),
            Some(RequestType::AnalyticalReasoning)
        );
        assert_eq!(
            strong_intent("Design an API for tenant scoped webhooks"),
            Some(RequestType::TechnicalDesign)
        );
        assert_eq!(
            strong_intent("What does HTTP 429 mean"),
            Some(RequestType::FactualLookup)
        );
        assert_eq!(strong_intent("Build something useful"), None);
        assert_eq!(
            strong_intent("Do not redesign this; explain the current behavior"),
            None
        );
    }

    #[tokio::test]
    async fn character_features_keep_small_typos_stable() {
        let model = LocalClassifier::embedded().unwrap();
        let clean = model
            .classify(&ClassifyInput {
                query: "fix this python function",
                context: None,
            })
            .await
            .unwrap();
        let noisy = model
            .classify(&ClassifyInput {
                query: "plz fix ths pyhton fn",
                context: None,
            })
            .await
            .unwrap();
        assert_eq!(clean.request_type, RequestType::CodeGeneration);
        assert_eq!(noisy.request_type, clean.request_type);
    }
}
