//! Local request classifier — an in-process, offline, deterministic linear model.
//!
//! Two heads over the [`request_features`](super::request_features) vector:
//!
//! - **Request type:** a 7-way multinomial logistic regression, temperature-scaled so that
//!   `confidence = max softmax(z / T)` is a calibrated estimate of P(type is correct) (`T`
//!   is fitted on a held-out validation split by the trainer).
//! - **Complexity:** a Frank–Hall ordinal model — four binary heads for "complexity > k"
//!   (k = 1..4), made monotone at inference (`q_k = min(q_k, q_{k−1})`) and decoded as
//!   `1 + #{k : q_k ≥ 0.5}`.
//!
//! The weights ship embedded in the binary (`assets/request_classifier.json`, produced by
//! `training/request_classifier/rc.py export`); `CLASSIFIER_MODEL_PATH` overrides them. The
//! file is validated strictly on load — schema, feature-engine version, dimensions, class
//! order, finiteness — because a weights file is only meaningful for the exact feature engine
//! it was trained against. Scoring accumulates in `f64` in sorted bucket order, so a given
//! input always yields bit-identical output. Inference is a few microseconds of CPU and makes
//! no network call.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType, round_confidence,
};
use super::request_features::{RC_FEATURE_ENGINE, RC_NUM_BUCKETS, RC_NUM_DENSE, extract};

/// Schema tag of the weights file this loader understands.
pub const WEIGHTS_SCHEMA: &str = "nasiko-request-classifier-weights-v1";
/// Weights per hashed bucket: 7 request-type weights then 4 complexity-head weights.
const ROW_LEN: usize = 11;
const NUM_TYPES: usize = 7;
const NUM_CX: usize = 4;

const EMBEDDED_WEIGHTS_JSON: &str = include_str!("../../assets/request_classifier.json");

/// Size in bytes of the weights file embedded in this binary.
pub fn embedded_artifact_len() -> usize {
    EMBEDDED_WEIGHTS_JSON.len()
}

/// Untrusted on-disk form; validated into [`LocalClassifier`].
#[derive(Deserialize)]
struct WeightsFile {
    schema: String,
    feature_engine: String,
    num_buckets: usize,
    num_dense_features: usize,
    classes: Vec<String>,
    type_bias: Vec<f64>,
    type_dense: Vec<Vec<f64>>,
    temperature: f64,
    cx_bias: Vec<f64>,
    cx_dense: Vec<Vec<f64>>,
    hashed: BTreeMap<String, Vec<f64>>,
    #[serde(default)]
    provenance: serde_json::Value,
}

/// The validated, dense, ready-to-score local model.
pub struct LocalClassifier {
    /// One row per bucket (`RC_NUM_BUCKETS × 11`); absent buckets are zero rows.
    rows: Vec<[f32; ROW_LEN]>,
    type_bias: [f32; NUM_TYPES],
    type_dense: [[f32; RC_NUM_DENSE]; NUM_TYPES],
    temperature: f64,
    cx_bias: [f32; NUM_CX],
    cx_dense: [[f32; RC_NUM_DENSE]; NUM_CX],
    provenance: serde_json::Value,
}

fn model_err(msg: impl Into<String>) -> ClassifyError {
    ClassifyError::Model(msg.into())
}

fn finite_vec<const N: usize>(v: &[f64], what: &str) -> Result<[f32; N], ClassifyError> {
    if v.len() != N {
        return Err(model_err(format!("{what}: expected {N} values, got {}", v.len())));
    }
    let mut out = [0f32; N];
    for (o, x) in out.iter_mut().zip(v) {
        if !x.is_finite() {
            return Err(model_err(format!("{what}: non-finite value")));
        }
        *o = *x as f32;
    }
    Ok(out)
}

fn finite_matrix<const R: usize>(
    m: &[Vec<f64>],
    what: &str,
) -> Result<[[f32; RC_NUM_DENSE]; R], ClassifyError> {
    if m.len() != R {
        return Err(model_err(format!("{what}: expected {R} rows, got {}", m.len())));
    }
    let mut out = [[0f32; RC_NUM_DENSE]; R];
    for (o, row) in out.iter_mut().zip(m) {
        *o = finite_vec::<RC_NUM_DENSE>(row, what)?;
    }
    Ok(out)
}

impl LocalClassifier {
    /// The model embedded in this binary.
    pub fn embedded() -> Result<Self, ClassifyError> {
        Self::from_json(EMBEDDED_WEIGHTS_JSON)
    }

    /// A model from a weights file on disk (`CLASSIFIER_MODEL_PATH`).
    pub fn from_path(path: &str) -> Result<Self, ClassifyError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| model_err(format!("cannot read weights file {path}: {e}")))?;
        Self::from_json(&raw)
    }

    /// Parse and validate a weights file.
    pub fn from_json(raw: &str) -> Result<Self, ClassifyError> {
        let f: WeightsFile = serde_json::from_str(raw)
            .map_err(|e| model_err(format!("weights file is not valid JSON: {e}")))?;
        if f.schema != WEIGHTS_SCHEMA {
            return Err(model_err(format!("unsupported weights schema {:?}", f.schema)));
        }
        if f.feature_engine != RC_FEATURE_ENGINE {
            return Err(model_err(format!(
                "weights trained for feature engine {:?}, this binary has {RC_FEATURE_ENGINE:?}",
                f.feature_engine
            )));
        }
        if f.num_buckets != RC_NUM_BUCKETS || f.num_dense_features != RC_NUM_DENSE {
            return Err(model_err(format!(
                "dimension mismatch: file {}x{}, engine {RC_NUM_BUCKETS}x{RC_NUM_DENSE}",
                f.num_buckets, f.num_dense_features
            )));
        }
        let expected: Vec<&str> = RequestType::ALL.iter().map(|rt| rt.as_str()).collect();
        if f.classes != expected {
            return Err(model_err(format!(
                "class list/order mismatch: {:?}",
                f.classes
            )));
        }
        if !f.temperature.is_finite() || f.temperature <= 0.0 {
            return Err(model_err("temperature must be finite and > 0"));
        }
        let mut rows = vec![[0f32; ROW_LEN]; RC_NUM_BUCKETS];
        for (key, row) in &f.hashed {
            let bucket: usize = key
                .parse()
                .map_err(|_| model_err(format!("hashed key {key:?} is not a bucket index")))?;
            if bucket >= RC_NUM_BUCKETS {
                return Err(model_err(format!("hashed bucket {bucket} out of range")));
            }
            rows[bucket] = finite_vec::<ROW_LEN>(row, "hashed row")?;
        }
        Ok(Self {
            rows,
            type_bias: finite_vec::<NUM_TYPES>(&f.type_bias, "type_bias")?,
            type_dense: finite_matrix::<NUM_TYPES>(&f.type_dense, "type_dense")?,
            temperature: f.temperature,
            cx_bias: finite_vec::<NUM_CX>(&f.cx_bias, "cx_bias")?,
            cx_dense: finite_matrix::<NUM_CX>(&f.cx_dense, "cx_dense")?,
            provenance: f.provenance,
        })
    }

    /// Training provenance recorded in the weights file (logged at startup).
    pub fn provenance(&self) -> &serde_json::Value {
        &self.provenance
    }

    /// Classify synchronously. Deterministic and total.
    pub fn predict(&self, query: &str, context: Option<&str>) -> Classification {
        let features = extract(query, context);
        let mut z_type = [0f64; NUM_TYPES];
        let mut z_cx = [0f64; NUM_CX];
        for k in 0..NUM_TYPES {
            z_type[k] = f64::from(self.type_bias[k])
                + dot(&self.type_dense[k], &features.dense);
        }
        for k in 0..NUM_CX {
            z_cx[k] = f64::from(self.cx_bias[k]) + dot(&self.cx_dense[k], &features.dense);
        }
        for (bucket, value) in &features.hashed {
            let row = &self.rows[*bucket as usize];
            let v = f64::from(*value);
            for k in 0..NUM_TYPES {
                z_type[k] += f64::from(row[k]) * v;
            }
            for k in 0..NUM_CX {
                z_cx[k] += f64::from(row[NUM_TYPES + k]) * v;
            }
        }

        // Temperature-scaled, numerically stable softmax; ties go to the lower index.
        let scaled: Vec<f64> = z_type.iter().map(|z| z / self.temperature).collect();
        let max = scaled.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let exps: Vec<f64> = scaled.iter().map(|z| (z - max).exp()).collect();
        let sum: f64 = exps.iter().sum();
        let mut best = 0usize;
        for k in 1..NUM_TYPES {
            if exps[k] > exps[best] {
                best = k;
            }
        }
        let confidence = round_confidence(exps[best] / sum);

        // Frank–Hall ordinal decode with a monotone (non-increasing) clamp.
        let mut prev = 1.0f64;
        let mut complexity = 1u8;
        for z in z_cx {
            let q = sigmoid(z).min(prev);
            prev = q;
            if q >= 0.5 {
                complexity += 1;
            }
        }
        Classification::new(RequestType::ALL[best], complexity, confidence)
    }
}

fn dot(w: &[f32; RC_NUM_DENSE], d: &[f32; RC_NUM_DENSE]) -> f64 {
    w.iter()
        .zip(d)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum()
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

#[async_trait::async_trait]
impl RequestClassifier for LocalClassifier {
    fn name(&self) -> &str {
        "local"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        Ok(self.predict(input.query, input.context))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::salience_classifier::hash_to_bucket_n;
    use super::*;

    /// Signed weight row so a gram contributes `strength` (per unit of its feature value) to
    /// type `class` and `cx` to every complexity head.
    fn row_for(gram: &str, class: usize, strength: f64, cx: f64) -> (String, Vec<f64>) {
        let (bucket, sign) = hash_to_bucket_n(gram, RC_NUM_BUCKETS);
        let mut row = vec![0.0; ROW_LEN];
        row[class] = sign * strength;
        for k in 0..NUM_CX {
            row[NUM_TYPES + k] = sign * cx;
        }
        (bucket.to_string(), row)
    }

    /// A tiny, valid, hand-built weights file: a few cue words push their class (and
    /// complexity), everything else leans `general` at complexity 2. Used by the tests and
    /// (via `write_fixture_artifact`) as the embedded placeholder before a trained model
    /// exists.
    pub(crate) fn fixture_json() -> String {
        let cues = [
            ("q:w:fix", 0, 25.0, -6.0),
            ("q:w:typo", 0, 25.0, -12.0),
            ("q:w:implement", 0, 25.0, 0.0),
            ("q:w:explain", 1, 25.0, 0.0),
            ("q:w:design", 2, 25.0, 12.0),
            ("q:w:architecture", 2, 25.0, 12.0),
            ("q:w:diagnose", 3, 25.0, 12.0),
            ("q:w:summarize", 4, 25.0, 0.0),
            ("q:w:rewrite", 4, 25.0, 0.0),
            ("q:w:what does", 5, 25.0, -6.0),
        ];
        let hashed: serde_json::Map<String, serde_json::Value> = cues
            .iter()
            .map(|(g, c, s, cx)| {
                let (k, row) = row_for(g, *c, *s, *cx);
                (k, serde_json::json!(row))
            })
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": WEIGHTS_SCHEMA,
            "feature_engine": RC_FEATURE_ENGINE,
            "num_buckets": RC_NUM_BUCKETS,
            "num_dense_features": RC_NUM_DENSE,
            "classes": RequestType::ALL.iter().map(|rt| rt.as_str()).collect::<Vec<_>>(),
            "type_bias": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.3],
            "type_dense": vec![vec![0.0; RC_NUM_DENSE]; NUM_TYPES],
            "temperature": 1.0,
            "cx_bias": [1.0, -1.0, -2.0, -3.0],
            "cx_dense": vec![vec![0.0; RC_NUM_DENSE]; NUM_CX],
            "hashed": hashed,
            "provenance": { "note": "FIXTURE — hand-built placeholder, not a trained model" },
        }))
        .unwrap()
    }

    #[test]
    #[ignore = "regenerates the placeholder artifact; only for bootstrapping"]
    fn write_fixture_artifact() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/request_classifier.json");
        std::fs::write(path, fixture_json() + "\n").unwrap();
    }

    fn mutate(f: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut v: serde_json::Value = serde_json::from_str(&fixture_json()).unwrap();
        f(&mut v);
        v.to_string()
    }

    #[test]
    fn embedded_artifact_loads_with_provenance_and_is_under_2mb() {
        let m = LocalClassifier::embedded().expect("embedded weights load");
        assert!(m.provenance().is_object());
        assert!(embedded_artifact_len() <= 2 * 1024 * 1024);
    }

    #[test]
    fn loader_rejects_bad_schema_dims_classes_rows_temperature_nonfinite() {
        let bad: Vec<(&str, String)> = vec![
            ("schema", mutate(|v| v["schema"] = "other".into())),
            ("engine", mutate(|v| v["feature_engine"] = "rc-v0".into())),
            ("buckets", mutate(|v| v["num_buckets"] = 65536.into())),
            ("dense", mutate(|v| v["num_dense_features"] = 8.into())),
            ("classes", mutate(|v| v["classes"][0] = "writing".into())),
            ("type_bias", mutate(|v| v["type_bias"] = serde_json::json!([0.0, 1.0]))),
            ("type_dense", mutate(|v| v["type_dense"][0] = serde_json::json!([1.0]))),
            ("cx_bias", mutate(|v| v["cx_bias"] = serde_json::json!([0.0]))),
            ("temperature0", mutate(|v| v["temperature"] = 0.0.into())),
            ("temperature-", mutate(|v| v["temperature"] = (-1.0).into())),
            ("row len", mutate(|v| v["hashed"]["5"] = serde_json::json!([1.0, 2.0]))),
            ("row key", mutate(|v| v["hashed"]["abc"] = serde_json::json!(vec![0.0; 11]))),
            ("row range", mutate(|v| v["hashed"]["40000"] = serde_json::json!(vec![0.0; 11]))),
            ("not json", "{".to_string()),
        ];
        for (what, raw) in bad {
            assert!(
                matches!(LocalClassifier::from_json(&raw), Err(ClassifyError::Model(_))),
                "{what} should be rejected"
            );
        }
        // Non-finite numbers cannot be expressed in JSON; out-of-f64-range literals parse to
        // infinity in some parsers — serde_json rejects them, which is also a rejection.
        assert!(LocalClassifier::from_json(&fixture_json().replace("1.0,", "1e999,")).is_err());
    }

    #[test]
    fn fixture_artifact_predicts_known_class_and_complexity() {
        let m = LocalClassifier::from_json(&fixture_json()).unwrap();
        let c = m.predict("fix typo in comment", None);
        assert_eq!(c.request_type, RequestType::CodeGeneration);
        assert_eq!(c.complexity, 1);
        let d = m.predict("design the architecture for a queue", None);
        assert_eq!(d.request_type, RequestType::TechnicalDesign);
        assert!(d.complexity >= 4);
        let g = m.predict("hello there", None);
        assert_eq!(g.request_type, RequestType::General);
        assert_eq!(g.complexity, 2);
    }

    #[test]
    fn outputs_always_in_range_and_deterministic() {
        let m = LocalClassifier::from_json(&fixture_json()).unwrap();
        let mut probes: Vec<String> = (0..200)
            .map(|i| format!("probe {i} fix design summarize {}", "x ".repeat(i % 17)))
            .collect();
        probes.extend(["", "\u{1F600}", "\0", "مرحبا"].map(String::from));
        probes.push("long ".repeat(50_000));
        for p in &probes {
            let a = m.predict(p, Some(p));
            let b = m.predict(p, Some(p));
            assert_eq!(a, b);
            assert_eq!(a.confidence.to_bits(), b.confidence.to_bits());
            assert!((0.0..=1.0).contains(&a.confidence));
            assert!((1..=5).contains(&a.complexity));
        }
    }

    #[test]
    fn complexity_head_is_monotone() {
        // Head biases rising with k would decode to 5 without the clamp (all four ≥ 0.5 only
        // because later heads are more confident); the monotone clamp caps it at the first
        // head that falls below 0.5.
        let raw = mutate(|v| v["cx_bias"] = serde_json::json!([-1.0, 3.0, 3.0, 3.0]));
        let m = LocalClassifier::from_json(&raw).unwrap();
        assert_eq!(m.predict("hello there", None).complexity, 1);
    }
}
