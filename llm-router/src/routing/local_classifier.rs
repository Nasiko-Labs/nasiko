//! Small learned CPU classifier. Training is offline; runtime needs no Python or network.
use super::classifier::{Classification, ClassifyInput, RequestClassifier, RequestType};

#[derive(serde::Deserialize)]
struct Head {
    weights: Vec<Vec<f64>>,
    bias: Vec<f64>,
}

#[derive(serde::Deserialize)]
struct Model {
    version: u8,
    dimension: usize,
    labels: Vec<String>,
    type_head: Head,
    complexity_head: Head,
    confidence_cap: f64,
    temperature: f64,
}

pub struct LocalClassifier {
    model: std::sync::Arc<Model>,
}

impl LocalClassifier {
    pub fn embedded() -> Result<Self, String> {
        Self::from_json(include_str!("../../data/classifier/model.json"))
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        let model: Model = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let valid_head = |head: &Head, count: usize| {
            head.weights.len() == count
                && head.bias.len() == count
                && head.bias.iter().all(|v| v.is_finite())
                && head
                    .weights
                    .iter()
                    .all(|row| row.len() == model.dimension && row.iter().all(|v| v.is_finite()))
        };
        if model.version != 1
            || model.dimension != 2048
            || model.labels.len() != 7
            || model.labels.iter().enumerate().any(|(i, label)| {
                RequestType::from_wire(label).is_none() || model.labels[..i].contains(label)
            })
            || !valid_head(&model.type_head, 7)
            || !valid_head(&model.complexity_head, 5)
            || !model.temperature.is_finite()
            || model.temperature < 1.0
            || !model.confidence_cap.is_finite()
            || !(0.0..=0.8).contains(&model.confidence_cap)
        {
            return Err("invalid classifier model schema or weights".into());
        }
        Ok(Self {
            model: std::sync::Arc::new(model),
        })
    }
}

fn bucket(feature: &str, dimension: usize) -> usize {
    let mut value = 2166136261u32;
    for byte in feature.bytes() {
        value = (value ^ byte as u32).wrapping_mul(16777619);
    }
    value as usize % dimension
}

fn features(input: &ClassifyInput<'_>, dimension: usize) -> Vec<f64> {
    let mut result = vec![0.0; dimension];
    for (text, scale) in [(input.query, 1.0), (input.context.unwrap_or(""), 0.25)] {
        let lower = text.to_ascii_lowercase();
        let words: Vec<_> = lower
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .filter(|w| !w.is_empty())
            .collect();
        for word in &words {
            result[bucket(&format!("w:{word}"), dimension)] += scale;
        }
        for pair in words.windows(2) {
            result[bucket(&format!("b:{} {}", pair[0], pair[1]), dimension)] += scale;
        }
        let normalized = words.join(" ");
        for bytes in normalized.as_bytes().windows(4) {
            let gram = std::str::from_utf8(bytes).expect("normalized words are ASCII");
            result[bucket(&format!("c:{gram}"), dimension)] += 0.1 * scale;
        }
    }
    let norm = result.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm > 0.0 {
        for value in &mut result {
            *value /= norm;
        }
    }
    result
}

fn predict(head: &Head, x: &[f64], temperature: f64) -> (usize, f64) {
    let logits: Vec<_> = head
        .weights
        .iter()
        .zip(&head.bias)
        .map(|(row, bias)| {
            (row.iter().zip(x).map(|(w, v)| w * v).sum::<f64>() + bias) / temperature
        })
        .collect();
    // Stable ties go to the earliest class, with no random sampling.
    let mut best = 0;
    for i in 1..logits.len() {
        if logits[i] > logits[best] {
            best = i;
        }
    }
    let total = logits.iter().map(|v| (v - logits[best]).exp()).sum::<f64>();
    (best, 1.0 / total)
}

#[async_trait::async_trait]
impl RequestClassifier for LocalClassifier {
    fn name(&self) -> &str {
        "local"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, String> {
        // Bound allocation and inference work; the configured wrapper catches this error.
        if input.query.len() + input.context.map_or(0, str::len) > 64 * 1024 {
            return Err("classifier input exceeds 64 KiB".into());
        }
        let model = self.model.clone();
        let query = input.query.to_string();
        let context = input.context.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let input = ClassifyInput {
                query: &query,
                context: context.as_deref(),
            };
            let x = features(&input, model.dimension);
            if x.iter().all(|v| *v == 0.0) {
                return Err("input has no supported ASCII tokens".into());
            }
            let (category, type_confidence) = predict(&model.type_head, &x, model.temperature);
            let (complexity, complexity_confidence) =
                predict(&model.complexity_head, &x, model.temperature);
            Ok(Classification {
                request_type: RequestType::from_wire(&model.labels[category]).unwrap(),
                complexity: complexity as u8 + 1,
                // Conservative joint score, not a claim of empirically calibrated probability.
                confidence: (type_confidence
                    .min(complexity_confidence)
                    .min(model.confidence_cap)) as f32,
            })
        })
        .await
        .map_err(|error| error.to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rust_predictions_match_python_training_export() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../data/classifier/golden.json")).unwrap();
        let model = LocalClassifier::embedded().unwrap();
        for case in cases {
            let result = model
                .classify(&ClassifyInput {
                    query: case["query"].as_str().unwrap(),
                    context: case["context"].as_str(),
                })
                .await
                .unwrap();
            assert_eq!(
                result.request_type.as_str(),
                case["request_type"].as_str().unwrap()
            );
            assert_eq!(
                result.complexity as u64,
                case["complexity"].as_u64().unwrap()
            );
            assert!((result.confidence as f64 - case["confidence"].as_f64().unwrap()).abs() < 1e-6);
        }
    }

    #[tokio::test]
    async fn deterministic_and_context_sensitive() {
        let model = LocalClassifier::embedded().unwrap();
        let input = ClassifyInput {
            query: "Create a Python function to count words",
            context: Some("Return a dictionary"),
        };
        let a = model.classify(&input).await.unwrap();
        let b = model.classify(&input).await.unwrap();
        assert_eq!(a.request_type, b.request_type);
        assert_eq!(a.complexity, b.complexity);
        assert_eq!(a.confidence, b.confidence);
        assert!((1..=5).contains(&a.complexity));
        assert!((0.0..=0.8).contains(&a.confidence));
        assert_ne!(
            features(&input, 2048),
            features(
                &ClassifyInput {
                    query: input.query,
                    context: None
                },
                2048
            )
        );
    }

    #[test]
    fn invalid_model_is_rejected() {
        assert!(LocalClassifier::from_json("{}").is_err());
        let mut model: serde_json::Value =
            serde_json::from_str(include_str!("../../data/classifier/model.json")).unwrap();
        model["labels"][0] = serde_json::json!("unknown");
        assert!(LocalClassifier::from_json(&model.to_string()).is_err());
    }
}
