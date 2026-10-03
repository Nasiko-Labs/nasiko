//! Local request classifier: two multinomial logistic regressions (request type, complexity)
//! over sparse binary text features. No network, no native dependencies, deterministic.
//!
//! Feature extraction, training and inference live together so the trainer
//! (`examples/classifier_train.rs`), the eval and the router share one featurizer — a model can
//! never be scored on features it was not trained with.
//!
//! # Features
//!
//! Lowercased word tokens of the query (`q:`), query bigrams (`q2:`), the first content word
//! after conversational filler (`v:` — "hey so could you **write**…"), context words (`c:`), and
//! a few shape features (`s:`): length buckets, code fences/backticks, question mark, sentence
//! and comma counts, negation cues. Only features seen in at least two training examples are
//! kept, which prunes one-off names and numbers.
//!
//! # Confidence
//!
//! `confidence` is the top request-type probability after temperature scaling. The temperature is
//! fitted on out-of-fold predictions over the training split (5-fold), never on the validation
//! split, so validation calibration is an honest estimate.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType,
};

/// Label order of the request-type head. Persisted model weights are indexed by this order.
pub const LABELS: [RequestType; 7] = [
    RequestType::CodeGeneration,
    RequestType::CodeUnderstanding,
    RequestType::TechnicalDesign,
    RequestType::AnalyticalReasoning,
    RequestType::Writing,
    RequestType::FactualLookup,
    RequestType::General,
];
const N_TYPES: usize = LABELS.len();
const N_CX: usize = 5;

/// Leading words that carry no task signal ("hey so could you please write…").
const FILLER: &[&str] = &[
    "hi",
    "hii",
    "hiii",
    "hey",
    "hello",
    "ok",
    "okay",
    "so",
    "um",
    "uh",
    "hmm",
    "please",
    "pls",
    "plz",
    "can",
    "could",
    "would",
    "will",
    "you",
    "u",
    "i",
    "need",
    "want",
    "quick",
    "q",
    "sorry",
    "thanks",
    "thx",
    "just",
    "help",
    "me",
    "basically",
    "anyway",
    "alright",
    "yo",
    "a",
    "the",
    "to",
    "some",
    "for",
];

fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .take(400)
        .map(str::to_lowercase)
        .collect()
}

fn len_bucket(n: usize) -> &'static str {
    match n {
        0 => "0",
        1..=3 => "1-3",
        4..=8 => "4-8",
        9..=16 => "9-16",
        17..=32 => "17-32",
        33..=64 => "33-64",
        _ => "65+",
    }
}

fn looks_like_code(s: &str) -> bool {
    s.contains("```")
        || s.contains('`')
        || (s.contains('(')
            && s.contains(')')
            && (s.contains('{') || s.contains(';') || s.contains('=')))
}

/// Sorted, de-duplicated binary features for one input.
pub fn features(query: &str, context: Option<&str>) -> Vec<String> {
    let mut f = BTreeSet::new();
    f.insert("bias".to_string());

    let q = tokens(query);
    for t in &q {
        f.insert(format!("q:{t}"));
    }
    for w in q.windows(2) {
        f.insert(format!("q2:{}_{}", w[0], w[1]));
    }
    if let Some(v) = q.iter().find(|t| !FILLER.contains(&t.as_str())) {
        f.insert(format!("v:{v}"));
    }

    let ctx = context.map(str::trim).filter(|c| !c.is_empty());
    let c = ctx.map(tokens).unwrap_or_default();
    for t in &c {
        f.insert(format!("c:{t}"));
    }

    let lower = query.to_lowercase();
    f.insert(format!("s:qlen:{}", len_bucket(q.len())));
    f.insert(format!("s:clen:{}", len_bucket(c.len())));
    if query.contains("```") {
        f.insert("s:q_fence".into());
    }
    if query.contains('`') {
        f.insert("s:q_tick".into());
    }
    if ctx.is_some_and(looks_like_code) {
        f.insert("s:c_code".into());
    }
    if query.trim_end().ends_with('?') {
        f.insert("s:qmark".into());
    }
    let sentences = query
        .split(['.', '!', '?'])
        .filter(|s| s.split_whitespace().count() >= 3)
        .count();
    f.insert(format!("s:sent:{}", sentences.min(6)));
    f.insert(format!("s:commas:{}", query.matches(',').count().min(6)));
    let negation = [
        "don't",
        "dont",
        "do not",
        "no need",
        "not asking",
        "nothing else",
        "only",
        "just",
    ];
    if negation.iter().any(|n| lower.contains(n)) {
        f.insert("s:neg".into());
    }
    f.into_iter().collect()
}

/// Weights for one feature: request-type logits and complexity logits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureWeights {
    pub t: [f32; N_TYPES],
    pub c: [f32; N_CX],
}

/// A trained model, as persisted in `assets/classifier/linear-v1.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearModel {
    pub version: u32,
    pub labels: Vec<String>,
    pub temperature: f32,
    pub complexity_temperature: f32,
    pub features: BTreeMap<String, FeatureWeights>,
}

/// One prediction from [`LinearModel::predict`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prediction {
    pub request_type: RequestType,
    /// 1-5: the probability-weighted mean of the complexity head, rounded.
    pub complexity: u8,
    /// Calibrated top request-type probability.
    pub confidence: f32,
}

fn softmax<const N: usize>(logits: &[f64; N], temperature: f64) -> [f64; N] {
    let m = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mut out = [0.0; N];
    let mut sum = 0.0;
    for (o, l) in out.iter_mut().zip(logits) {
        *o = ((l - m) / temperature).exp();
        sum += *o;
    }
    out.iter_mut().for_each(|o| *o /= sum);
    out
}

impl LinearModel {
    pub fn from_json(s: &str) -> Result<Self, String> {
        let m: LinearModel = serde_json::from_str(s).map_err(|e| e.to_string())?;
        let want: Vec<&str> = LABELS.iter().map(|l| l.as_str()).collect();
        if m.version != 1 || m.labels != want {
            return Err(format!(
                "unsupported model: version {} labels {:?}",
                m.version, m.labels
            ));
        }
        if !(m.temperature > 0.0 && m.complexity_temperature > 0.0) {
            return Err("temperatures must be positive".into());
        }
        Ok(m)
    }

    fn logits(&self, feats: &[String]) -> ([f64; N_TYPES], [f64; N_CX]) {
        let (mut t, mut c) = ([0.0; N_TYPES], [0.0; N_CX]);
        for f in feats {
            if let Some(w) = self.features.get(f) {
                t.iter_mut().zip(w.t).for_each(|(a, b)| *a += b as f64);
                c.iter_mut().zip(w.c).for_each(|(a, b)| *a += b as f64);
            }
        }
        (t, c)
    }

    /// Request-type probabilities (label order = [`LABELS`]) and complexity probabilities.
    pub fn probabilities(
        &self,
        query: &str,
        context: Option<&str>,
    ) -> ([f64; N_TYPES], [f64; N_CX]) {
        let (t, c) = self.logits(&features(query, context));
        (
            softmax(&t, self.temperature as f64),
            softmax(&c, self.complexity_temperature as f64),
        )
    }

    pub fn predict(&self, query: &str, context: Option<&str>) -> Prediction {
        let (pt, pc) = self.probabilities(query, context);
        let (best, p) = argmax(&pt);
        let expected: f64 = pc.iter().enumerate().map(|(k, p)| (k + 1) as f64 * p).sum();
        Prediction {
            request_type: LABELS[best],
            complexity: (expected.round() as u8).clamp(1, 5),
            confidence: p as f32,
        }
    }
}

fn argmax(p: &[f64]) -> (usize, f64) {
    p.iter().cloned().enumerate().fold(
        (0, f64::NEG_INFINITY),
        |b, (i, v)| if v > b.1 { (i, v) } else { b },
    )
}

// ─── the local backend ───────────────────────────────────────────────────────────────────────

/// The shipped model, trained by `examples/classifier_train.rs` from `labelled.jsonl`.
pub const EMBEDDED_MODEL: &str = include_str!("../../assets/classifier/linear-v1.json");

/// [`LinearModel`] behind the [`RequestClassifier`] trait (`CLASSIFIER_BACKEND=local`).
pub struct LocalClassifier {
    model: LinearModel,
}

impl LocalClassifier {
    /// The model compiled into the binary.
    pub fn embedded() -> Result<Self, ClassifyError> {
        Self::from_json(EMBEDDED_MODEL)
    }

    /// A model file (`CLASSIFIER_MODEL_PATH`); the path is read by the caller's config.
    pub fn from_path(path: &str) -> Result<Self, ClassifyError> {
        let s = std::fs::read_to_string(path)
            .map_err(|e| ClassifyError::Load(format!("{path}: {e}")))?;
        Self::from_json(&s)
    }

    pub fn from_json(s: &str) -> Result<Self, ClassifyError> {
        LinearModel::from_json(s)
            .map(|model| Self { model })
            .map_err(ClassifyError::Load)
    }

    pub fn model(&self) -> &LinearModel {
        &self.model
    }
}

#[async_trait::async_trait]
impl RequestClassifier for LocalClassifier {
    fn name(&self) -> &str {
        "local"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let p = self.model.predict(input.query, input.context);
        Ok(Classification {
            request_type: p.request_type,
            complexity: p.complexity,
            confidence: p.confidence,
        })
    }
}

// ─── training ────────────────────────────────────────────────────────────────────────────────

/// One labelled example.
#[derive(Debug, Clone, Deserialize)]
pub struct Example {
    pub id: String,
    pub query: String,
    #[serde(default)]
    pub context: String,
    pub request_type: String,
    pub complexity: u8,
}

/// Training hyperparameters. Defaults are what shipped `linear-v1.json` was trained with.
#[derive(Debug, Clone, Copy)]
pub struct TrainConfig {
    pub epochs: usize,
    pub learning_rate: f64,
    pub l2: f64,
    pub min_df: usize,
    pub seed: u64,
}

impl Default for TrainConfig {
    fn default() -> Self {
        Self {
            epochs: 40,
            learning_rate: 0.2,
            l2: 1e-4,
            min_df: 2,
            seed: 7,
        }
    }
}

struct Encoded {
    feats: Vec<usize>,
    t: usize,
    c: usize,
}

/// xorshift64* — a fixed, dependency-free RNG so training is reproducible bit for bit.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
    }
}

fn type_index(label: &str) -> Result<usize, String> {
    LABELS
        .iter()
        .position(|l| l.as_str() == label)
        .ok_or_else(|| format!("unknown request_type '{label}'"))
}

/// Vocabulary plus raw request-type and complexity weights, row per vocabulary entry.
type Fitted = (Vec<String>, Vec<[f64; N_TYPES]>, Vec<[f64; N_CX]>);

/// Raw (untempered) weights trained by SGD on `examples`.
fn fit(examples: &[&Example], cfg: &TrainConfig) -> Result<Fitted, String> {
    let mut df: HashMap<String, usize> = HashMap::new();
    let raw: Vec<Vec<String>> = examples
        .iter()
        .map(|e| features(&e.query, Some(&e.context)))
        .collect();
    for fs in &raw {
        for f in fs {
            *df.entry(f.clone()).or_default() += 1;
        }
    }
    let mut vocab: Vec<String> = df
        .into_iter()
        .filter(|(f, n)| *n >= cfg.min_df || f.starts_with("s:") || f == "bias")
        .map(|(f, _)| f)
        .collect();
    vocab.sort();
    let index: HashMap<&str, usize> = vocab
        .iter()
        .enumerate()
        .map(|(i, f)| (f.as_str(), i))
        .collect();

    let data: Vec<Encoded> = examples
        .iter()
        .zip(&raw)
        .map(|(e, fs)| {
            if !(1..=5).contains(&e.complexity) {
                return Err(format!("{}: complexity must be 1-5", e.id));
            }
            Ok(Encoded {
                feats: fs
                    .iter()
                    .filter_map(|f| index.get(f.as_str()).copied())
                    .collect(),
                t: type_index(&e.request_type)?,
                c: e.complexity as usize - 1,
            })
        })
        .collect::<Result<_, String>>()?;

    let mut wt = vec![[0.0; N_TYPES]; vocab.len()];
    let mut wc = vec![[0.0; N_CX]; vocab.len()];
    let mut order: Vec<usize> = (0..data.len()).collect();
    let mut rng = Rng(cfg.seed.max(1));
    for epoch in 0..cfg.epochs {
        rng.shuffle(&mut order);
        let lr = cfg.learning_rate / (1.0 + epoch as f64 * 0.1);
        for &i in &order {
            let ex = &data[i];
            step(&mut wt, &ex.feats, ex.t, lr, cfg.l2);
            step(&mut wc, &ex.feats, ex.c, lr, cfg.l2);
        }
    }
    Ok((vocab, wt, wc))
}

fn step<const N: usize>(w: &mut [[f64; N]], feats: &[usize], y: usize, lr: f64, l2: f64) {
    let mut logits = [0.0; N];
    for &f in feats {
        logits.iter_mut().zip(w[f]).for_each(|(a, b)| *a += b);
    }
    let p = softmax(&logits, 1.0);
    for &f in feats {
        for k in 0..N {
            let g = p[k] - if k == y { 1.0 } else { 0.0 };
            w[f][k] -= lr * (g + l2 * w[f][k]);
        }
    }
}

fn raw_logits<const N: usize>(
    vocab: &HashMap<&str, usize>,
    w: &[[f64; N]],
    e: &Example,
) -> [f64; N] {
    let mut l = [0.0; N];
    for f in features(&e.query, Some(&e.context)) {
        if let Some(&i) = vocab.get(f.as_str()) {
            l.iter_mut().zip(w[i]).for_each(|(a, b)| *a += b);
        }
    }
    l
}

/// Temperature minimizing negative log-likelihood of `(logits, label)` pairs (grid 0.25-5.0).
fn fit_temperature<const N: usize>(pairs: &[([f64; N], usize)]) -> f64 {
    let nll = |t: f64| -> f64 {
        pairs
            .iter()
            .map(|(l, y)| -softmax(l, t)[*y].max(1e-12).ln())
            .sum()
    };
    (5..=100)
        .map(|i| i as f64 * 0.05)
        .fold((1.0, f64::INFINITY), |best, t| {
            let v = nll(t);
            if v < best.1 { (t, v) } else { best }
        })
        .0
}

/// Train on `examples`: temperatures from 5-fold out-of-fold predictions, then final weights on
/// all of `examples`. Deterministic for a given input order and config.
pub fn train(examples: &[Example], cfg: &TrainConfig) -> Result<LinearModel, String> {
    let all: Vec<&Example> = examples.iter().collect();
    let (mut oof_t, mut oof_c) = (Vec::new(), Vec::new());
    for fold in 0..5 {
        let tr: Vec<&Example> = (0..all.len())
            .filter(|i| i % 5 != fold)
            .map(|i| all[i])
            .collect();
        let te: Vec<&Example> = (0..all.len())
            .filter(|i| i % 5 == fold)
            .map(|i| all[i])
            .collect();
        let (vocab, wt, wc) = fit(&tr, cfg)?;
        let index: HashMap<&str, usize> = vocab
            .iter()
            .enumerate()
            .map(|(i, f)| (f.as_str(), i))
            .collect();
        for e in te {
            oof_t.push((raw_logits(&index, &wt, e), type_index(&e.request_type)?));
            oof_c.push((raw_logits(&index, &wc, e), e.complexity as usize - 1));
        }
    }
    let temperature = fit_temperature(&oof_t);
    let complexity_temperature = fit_temperature(&oof_c);

    let (vocab, wt, wc) = fit(&all, cfg)?;
    let round = |x: f64| ((x * 1e4).round() / 1e4) as f32;
    let features = vocab
        .into_iter()
        .zip(wt.iter().zip(&wc))
        .map(|(f, (t, c))| {
            (
                f,
                FeatureWeights {
                    t: t.map(round),
                    c: c.map(round),
                },
            )
        })
        .filter(|(_, w)| w.t.iter().chain(&w.c).any(|v| *v != 0.0))
        .collect();
    Ok(LinearModel {
        version: 1,
        labels: LABELS.iter().map(|l| l.as_str().to_string()).collect(),
        temperature: round(temperature),
        complexity_temperature: round(complexity_temperature),
        features,
    })
}

/// Deterministic train/validation split that keeps near-duplicates together: examples whose
/// query token sets have Jaccard ≥ `threshold` are unioned into one group, and each group goes
/// to validation when an FNV-1a hash of its first id is ≡ 0 (mod 5) — about 20%.
pub fn split(examples: &[Example], threshold: f64) -> (Vec<Example>, Vec<Example>) {
    let sets: Vec<BTreeSet<String>> = examples
        .iter()
        .map(|e| tokens(&e.query).into_iter().collect())
        .collect();
    let mut parent: Vec<usize> = (0..examples.len()).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        p[i] = r;
        r
    }
    for i in 0..examples.len() {
        for j in i + 1..examples.len() {
            if jaccard(&sets[i], &sets[j]) >= threshold {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let fnv = |s: &str| {
        s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
        })
    };
    let (mut train, mut val) = (Vec::new(), Vec::new());
    for i in 0..examples.len() {
        let root = find(&mut parent, i);
        if fnv(&examples[root].id) % 5 == 0 {
            val.push(examples[i].clone());
        } else {
            train.push(examples[i].clone());
        }
    }
    (train, val)
}

/// Jaccard similarity of two token sets.
pub fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let inter = a.intersection(b).count();
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

/// Query token set, as used by [`split`] for the near-duplicate check.
pub fn token_set(s: &str) -> BTreeSet<String> {
    tokens(s).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_are_sorted_deduped_and_skip_filler() {
        let f = features("hey so could you write write a parser?", None);
        let mut sorted = f.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(f, sorted);
        assert!(f.contains(&"v:write".to_string()));
        assert!(f.contains(&"s:qmark".to_string()));
        assert!(f.contains(&"s:clen:0".to_string()));
    }

    #[test]
    fn training_is_deterministic_and_learns_a_toy_task() {
        let ex = |id: &str, q: &str, t: &str, c: u8| Example {
            id: id.into(),
            query: q.into(),
            context: String::new(),
            request_type: t.into(),
            complexity: c,
        };
        let mut data = Vec::new();
        for i in 0..20 {
            data.push(ex(
                &format!("a{i}"),
                &format!("write code function {i}"),
                "code_generation",
                2,
            ));
            data.push(ex(
                &format!("b{i}"),
                &format!("hello there friend {i}"),
                "general",
                1,
            ));
        }
        let a = train(&data, &TrainConfig::default()).unwrap();
        let b = train(&data, &TrainConfig::default()).unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        let p = a.predict("write a function", None);
        assert_eq!(p.request_type, RequestType::CodeGeneration);
        assert!(p.confidence > 0.5 && p.confidence <= 1.0);
        assert_eq!(
            a.predict("hello friend", None).request_type,
            RequestType::General
        );
    }

    #[test]
    fn split_keeps_near_duplicates_together() {
        let ex = |id: &str, q: &str| Example {
            id: id.into(),
            query: q.into(),
            context: String::new(),
            request_type: "general".into(),
            complexity: 1,
        };
        let data: Vec<Example> = (0..50)
            .flat_map(|i| {
                [
                    ex(&format!("x{i}"), &format!("w{i}a w{i}b w{i}c w{i}d")),
                    ex(&format!("y{i}"), &format!("w{i}a w{i}b w{i}c w{i}d extra")),
                ]
            })
            .collect();
        let (train, val) = split(&data, 0.6);
        assert_eq!(train.len() + val.len(), 100);
        assert!(!val.is_empty() && !train.is_empty());
        for v in &val {
            for t in &train {
                assert!(jaccard(&token_set(&v.query), &token_set(&t.query)) < 0.6);
            }
        }
    }

    /// The shipped model must be exactly what the trainer produces from the shipped data, so
    /// anyone can audit or reproduce it (`examples/classifier_train.rs`).
    #[test]
    fn embedded_model_is_reproducible_from_the_labelled_data() {
        let data: Vec<Example> = include_str!("../../assets/classifier/labelled.jsonl")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let (train_split, _) = split(&data, 0.6);
        let model = train(&train_split, &TrainConfig::default()).unwrap();
        assert_eq!(
            serde_json::to_string(&model).unwrap() + "\n",
            EMBEDDED_MODEL,
            "re-run `cargo run --release -p nasiko-llm-router --example classifier_train`"
        );
    }
}
