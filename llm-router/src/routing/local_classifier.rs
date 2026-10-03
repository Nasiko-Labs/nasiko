use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use serde::Deserialize;

use super::{Classification, RequestClassifier, RequestType};

const CONFIDENCE_TEMPERATURE: f64 = 2.0;

const BUILTIN: &str = include_str!("../../tests/data/classifier-train.json");
const TYPES: [RequestType; 7] = [
    RequestType::CodeGeneration,
    RequestType::CodeUnderstanding,
    RequestType::TechnicalDesign,
    RequestType::AnalyticalReasoning,
    RequestType::Writing,
    RequestType::FactualLookup,
    RequestType::General,
];

#[derive(Deserialize)]
struct TrainingSet {
    examples: Vec<Example>,
}

#[derive(Deserialize)]
struct Example {
    query: String,
    #[serde(default)]
    context: String,
    request_type: String,
    complexity: u8,
}

#[derive(Default)]
struct Class {
    documents: usize,
    tokens: BTreeMap<String, f64>,
    total: f64,
}

impl Class {
    fn observe(&mut self, features: &BTreeMap<String, f64>) {
        self.documents += 1;
        for (token, count) in features {
            *self.tokens.entry(token.clone()).or_default() += count;
            self.total += count;
        }
    }
}

/// Multinomial naive Bayes fitted once from labelled training examples.
pub struct LocalClassifier {
    categories: [Class; 7],
    difficulties: [Class; 5],
    vocabulary: BTreeSet<String>,
    documents: usize,
    context_chars: usize,
}

impl LocalClassifier {
    /// `local-training-v1` uses embedded training data. Unsupported model IDs
    /// fail at startup; classification never performs file or environment access.
    pub fn load(model: &str, context_chars: usize) -> Result<Self, String> {
        if model != "local-training-v1" {
            return Err("unsupported local classifier model".into());
        }
        Self::from_training(BUILTIN, context_chars)
    }

    fn from_training(data: &str, context_chars: usize) -> Result<Self, String> {
        let training: TrainingSet =
            serde_json::from_str(data).map_err(|_| "invalid local training JSON")?;
        if training.examples.is_empty() {
            return Err("local training data is empty".into());
        }
        let mut model = Self {
            categories: std::array::from_fn(|_| Class::default()),
            difficulties: std::array::from_fn(|_| Class::default()),
            vocabulary: BTreeSet::new(),
            documents: training.examples.len(),
            context_chars,
        };
        for example in training.examples {
            let kind = RequestType::from_wire(&example.request_type)
                .ok_or("unknown local training category")?;
            if !(1..=5).contains(&example.complexity) {
                return Err("invalid local training difficulty".into());
            }
            let features = features(&example.query, &example.context, context_chars);
            model.vocabulary.extend(features.keys().cloned());
            let category = TYPES
                .iter()
                .position(|candidate| *candidate == kind)
                .unwrap();
            model.categories[category].observe(&features);
            model.difficulties[usize::from(example.complexity - 1)].observe(&features);
        }
        if model.vocabulary.is_empty() || model.categories.iter().any(|class| class.documents == 0)
        {
            return Err("local training requires text and every request category".into());
        }
        Ok(model)
    }

    fn predict(&self, classes: &[Class], features: &BTreeMap<String, f64>) -> (usize, f64) {
        let scores: Vec<f64> = classes
            .iter()
            .map(|class| {
                let prior =
                    ((class.documents + 1) as f64 / (self.documents + classes.len()) as f64).ln();
                features
                    .iter()
                    .filter(|(token, _)| self.vocabulary.contains(*token))
                    .fold(prior, |score, (token, count)| {
                        score
                            + count
                                * ((class.tokens.get(token).copied().unwrap_or(0.0) + 1.0)
                                    / (class.total + self.vocabulary.len() as f64))
                                    .ln()
                    })
            })
            .collect();
        let mut best = 0;
        for index in 1..scores.len() {
            if scores[index] > scores[best] {
                best = index;
            }
        }
        let denominator: f64 = scores
            .iter()
            .map(|score| ((score - scores[best]) / CONFIDENCE_TEMPERATURE).exp())
            .sum();
        (best, 1.0 / denominator)
    }
}

fn features(query: &str, context: &str, limit: usize) -> BTreeMap<String, f64> {
    let mut result = BTreeMap::new();
    for (text, weight) in [(query, 1.0), (context, 0.35)] {
        let bounded: String = text
            .chars()
            .take(limit)
            .flat_map(char::to_lowercase)
            .collect();
        for token in bounded.split(|ch: char| !ch.is_alphanumeric() && ch != '_') {
            if !token.is_empty() {
                *result.entry(token.to_owned()).or_default() += weight;
            }
        }
    }
    result
}

#[async_trait]
impl RequestClassifier for LocalClassifier {
    fn name(&self) -> &str {
        "local"
    }

    async fn classify(&self, query: &str, context: &str) -> Result<Classification, String> {
        let features = features(query, context, self.context_chars);
        let known: f64 = features
            .iter()
            .filter(|(token, _)| self.vocabulary.contains(*token))
            .map(|(_, count)| count)
            .sum();
        let total: f64 = features.values().sum();
        let (category, confidence) = self.predict(&self.categories, &features);
        let (difficulty, _) = self.predict(&self.difficulties, &features);
        Ok(Classification {
            request_type: if known == 0.0 {
                RequestType::General
            } else {
                TYPES[category]
            },
            complexity: (difficulty + 1) as u8,
            confidence: (confidence * known / total.max(1.0)) as f32,
            fallback_reason: None,
            decision_cost_usd: Some(0.0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn novel_queries_have_repeatable_predictions() {
        let classifier = LocalClassifier::load("local-training-v1", 4000).unwrap();
        let independently_loaded = LocalClassifier::load("local-training-v1", 4000).unwrap();
        for (query, expected) in [
            (
                "Draft a friendly reminder about tomorrow",
                RequestType::Writing,
            ),
            ("Explain how the loop runs", RequestType::CodeUnderstanding),
            (
                "Calculate the balance after subtracting 17",
                RequestType::AnalyticalReasoning,
            ),
        ] {
            let first = classifier.classify(query, "").await.unwrap();
            assert_eq!(first.request_type, expected);
            independently_loaded.classify("Hello", "").await.unwrap();
            let separate = independently_loaded.classify(query, "").await.unwrap();
            assert_eq!(separate.request_type, first.request_type);
            assert_eq!(separate.complexity, first.complexity);
            assert_eq!(separate.confidence.to_bits(), first.confidence.to_bits());
            for _ in 0..100 {
                let next = classifier.classify(query, "").await.unwrap();
                assert_eq!(next.request_type, expected);
                assert_eq!(next.complexity, first.complexity);
                assert_eq!(next.confidence.to_bits(), first.confidence.to_bits());
            }
        }
    }

    #[tokio::test]
    async fn context_resolves_an_ambiguous_followup() {
        let classifier = LocalClassifier::load("local-training-v1", 4000).unwrap();
        let result = classifier
            .classify(
                "Please continue",
                "Draft a reminder for the team. Keep a friendly tone.",
            )
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::Writing);
        let unknown = classifier.classify("zyxwvu", "").await.unwrap();
        assert_eq!(unknown.request_type, RequestType::General);
        assert_eq!(unknown.confidence, 0.0);
    }

    #[test]
    fn malformed_models_fail_at_initialization() {
        assert!(LocalClassifier::from_training("{\"examples\":[]}", 4000).is_err());
        assert!(LocalClassifier::from_training("{}", 4000).is_err());
        assert!(LocalClassifier::load("unknown-model", 4000).is_err());
    }

    #[tokio::test]
    async fn builder_selects_local_and_falls_back_for_unknown_backends() {
        let mut cfg = crate::config::GatewayConfig::default();
        cfg.classifier_backend = "local".into();
        cfg.classifier_model = "local-training-v1".into();
        cfg.classifier_min_confidence = 0.0;
        let local = super::super::build_classifier(&cfg, reqwest::Client::new());
        assert_eq!(local.name(), "local");
        let result = local
            .classify("Draft a friendly reminder about tomorrow", "")
            .await
            .unwrap();
        assert_eq!(result.request_type, RequestType::Writing);
        assert_eq!(result.decision_cost_usd, Some(0.0));
        assert_eq!(result.fallback_reason, None);

        cfg.classifier_backend = "unknown".into();
        let fallback = super::super::build_classifier(&cfg, reqwest::Client::new());
        assert_eq!(fallback.name(), "unavailable");
        assert_eq!(
            fallback
                .classify("Hello", "")
                .await
                .unwrap()
                .fallback_reason,
            Some("backend_error")
        );
        cfg.classifier_backend = "local".into();
        cfg.classifier_model = "missing".into();
        let missing = super::super::build_classifier(&cfg, reqwest::Client::new());
        assert_eq!(
            missing.classify("Hello", "").await.unwrap().fallback_reason,
            Some("backend_error")
        );
        assert_eq!(fallback.classify("Hello", "").await.unwrap().complexity, 3);
    }
}
