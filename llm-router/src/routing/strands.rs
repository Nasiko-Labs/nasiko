//! Strands request types with train-only probability calibration and local complexity.
//!
//! The remote asks only the request-type question. Its native `confidence` is a
//! normalized concentration score, not the probability of its chosen label. We instead
//! temperature-scale the complete returned distribution and expose the selected label's
//! probability. A positive scalar temperature preserves the ordering of labels.
//!
//! Complexity is supplied exclusively by the embedded, separately trained local model.
//! Its uncertainty weights the tier-prior shift; it is not a calibrated probability that
//! the rounded complexity level is correct. Transport and contract failures retain the
//! required regex fallback, and successful low-confidence answers remain available for
//! the router's existing safe-default policy.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use super::classifier::RequestType;
use super::laya::LayaClassifier;
use super::local_classifier::LocalClassifier;
use super::request_classifier::{
    Classification, ClassifierInput, ClassifierSource, RegexClassifier, RequestClassifier,
};

const EMBEDDED_CALIBRATION: &str = include_str!("../../assets/strands_calibration.json");
const TYPE_COUNT: usize = 7;
const SUM_TOLERANCE: f64 = 0.01;
// The upstream rounds each probability to four decimal places. A tied maximum can
// consequently look very slightly smaller than another option in its wire response.
const CHOICE_TOLERANCE: f64 = 0.0001;

#[derive(Deserialize)]
struct Calibration {
    version: u32,
    type_temperature: f64,
}

pub struct StrandsClassifier {
    remote: Arc<dyn RequestClassifier>,
    local: Arc<dyn RequestClassifier>,
    temperature: f64,
}

impl StrandsClassifier {
    /// Construct with a pinned scalar calibration. Invalid calibration prevents startup
    /// from enabling this backend, allowing the caller to select its regex fallback.
    pub fn new(
        remote: impl RequestClassifier + 'static,
        local: impl RequestClassifier + 'static,
        temperature: f64,
    ) -> Result<Self, String> {
        validate_temperature(temperature)?;
        Ok(Self {
            remote: Arc::new(remote),
            local: Arc::new(local),
            temperature,
        })
    }

    /// Load the checked-in calibration asset once, before the evaluation/request loop.
    /// Metadata records how it was fit; the identity placeholder does not claim a fit.
    pub fn embedded_temperature() -> Result<f64, String> {
        let calibration: Calibration = serde_json::from_str(EMBEDDED_CALIBRATION)
            .map_err(|e| format!("invalid Strands calibration: {e}"))?;
        if calibration.version != 1 {
            return Err(format!(
                "unsupported Strands calibration version {}",
                calibration.version
            ));
        }
        validate_temperature(calibration.type_temperature)?;
        Ok(calibration.type_temperature)
    }

    /// Loopback/no-auth convenience constructor. The factory can use `new` with a
    /// separately authenticated type-only remote when an endpoint requires a token.
    pub fn embedded(
        http: reqwest::Client,
        base_url: &str,
        timeout: Duration,
    ) -> Result<Self, String> {
        Self::new(
            LayaClassifier::for_strands_type_only(http, base_url, timeout),
            LocalClassifier::embedded()?,
            Self::embedded_temperature()?,
        )
    }
}

fn validate_temperature(temperature: f64) -> Result<(), String> {
    if temperature.is_finite() && temperature > 0.0 {
        Ok(())
    } else {
        Err("Strands type temperature must be finite and positive".into())
    }
}

/// Validate the Strands choice contract and calibrate its seven-category belief.
/// Zero wire probabilities remain zero. Stable relative log weights prevent overflow
/// even for a very small temperature, and the maximum always has weight one.
fn calibrate(classification: &mut Classification, temperature: f64) -> Result<(), &'static str> {
    if validate_temperature(temperature).is_err() {
        return Err("bad_calibration");
    }
    let probabilities = classification
        .type_probabilities
        .as_ref()
        .ok_or("bad_response")?;
    if probabilities.len() != TYPE_COUNT {
        return Err("bad_response");
    }
    let mut seen = std::collections::HashSet::with_capacity(TYPE_COUNT);
    let mut total = 0.0;
    let mut maximum: f64 = 0.0;
    let mut selected = None;
    for &(label, probability) in probabilities {
        if !seen.insert(label) {
            return Err("bad_response");
        }
        if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
            return Err("bad_score");
        }
        total += probability;
        maximum = maximum.max(probability);
        if label == classification.request_type {
            selected = Some(probability);
        }
    }
    if (total - 1.0).abs() > SUM_TOLERANCE || maximum <= 0.0 {
        return Err("bad_score");
    }
    if selected.ok_or("bad_response")? + CHOICE_TOLERANCE < maximum {
        return Err("bad_response");
    }
    let mut calibrated: Vec<(RequestType, f64)> = probabilities
        .iter()
        .map(|&(label, probability)| {
            let weight = if probability == 0.0 {
                0.0
            } else {
                ((probability / maximum).ln() / temperature).exp()
            };
            (label, weight)
        })
        .collect();
    let normalizer: f64 = calibrated.iter().map(|(_, probability)| probability).sum();
    if !normalizer.is_finite() || normalizer <= 0.0 {
        return Err("bad_calibration");
    }
    for (_, probability) in &mut calibrated {
        *probability /= normalizer;
    }
    classification.confidence = calibrated
        .iter()
        .find(|(label, _)| *label == classification.request_type)
        .ok_or("bad_response")?
        .1;
    classification.type_probabilities = Some(calibrated);
    Ok(())
}

#[async_trait]
impl RequestClassifier for StrandsClassifier {
    async fn classify(&self, input: &ClassifierInput<'_>) -> Classification {
        let mut remote = self.remote.classify(input).await;
        if remote.source.fallback_reason().is_some() {
            return remote;
        }
        if let Err(reason) = calibrate(&mut remote, self.temperature) {
            return Classification {
                source: ClassifierSource::Fallback(reason),
                ..RegexClassifier::classify_query(input.query)
            };
        }
        let local = self.local.classify(input).await;
        if local.source.fallback_reason().is_some()
            || !(1..=5).contains(&local.complexity)
            || !local.complexity_level.is_finite()
            || !(0.0..=4.0).contains(&local.complexity_level)
            || !local.complexity_confidence.is_finite()
            || !(0.0..=1.0).contains(&local.complexity_confidence)
        {
            return Classification {
                source: ClassifierSource::Fallback("bad_complexity"),
                ..RegexClassifier::classify_query(input.query)
            };
        }
        remote.complexity = local.complexity;
        remote.complexity_level = local.complexity_level;
        remote.complexity_confidence = local.complexity_confidence;
        remote.source = ClassifierSource::Strands;
        remote
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub(Classification);
    #[async_trait]
    impl RequestClassifier for Stub {
        async fn classify(&self, _: &ClassifierInput<'_>) -> Classification {
            self.0.clone()
        }
    }

    fn remote(probabilities: [f64; TYPE_COUNT]) -> Classification {
        let mut classification = RegexClassifier::classify_query("write a function");
        classification.request_type = RequestType::Writing;
        classification.type_probabilities = Some(
            [
                RequestType::CodeGeneration,
                RequestType::CodeUnderstanding,
                RequestType::TechnicalDesign,
                RequestType::AnalyticalReasoning,
                RequestType::Writing,
                RequestType::FactualLookup,
                RequestType::General,
            ]
            .into_iter()
            .zip(probabilities)
            .collect(),
        );
        // The seven-option upstream concentration is not the selected probability.
        classification.confidence = (7.0 * probabilities[4] - 1.0) / 6.0;
        classification.source = ClassifierSource::Strands;
        classification
    }

    fn local() -> Classification {
        let mut classification = RegexClassifier::classify_query("hello");
        classification.source = ClassifierSource::Local;
        classification.request_type = RequestType::General;
        classification.complexity = 5;
        classification.complexity_level = 3.7;
        classification.complexity_confidence = 0.83;
        classification
    }

    async fn classify(classifier: &StrandsClassifier) -> Classification {
        classifier
            .classify(&ClassifierInput {
                query: "write a function",
                state: "Latest request:\nwrite a function",
            })
            .await
    }

    #[tokio::test]
    async fn corrects_confidence_and_keeps_calibrated_belief_with_local_complexity() {
        let original = remote([0.2, 0.05, 0.05, 0.05, 0.55, 0.05, 0.05]);
        let classifier =
            StrandsClassifier::new(Stub(original.clone()), Stub(local()), 1.0).unwrap();
        let identity = classify(&classifier).await;
        assert_eq!(identity.request_type, RequestType::Writing);
        assert!((identity.confidence - 0.55).abs() < 1e-12);
        assert_ne!(identity.confidence, original.confidence);
        assert!(!identity.is_low_confidence());
        let classifier = StrandsClassifier::new(Stub(original), Stub(local()), 2.0).unwrap();
        let softened = classify(&classifier).await;
        assert_eq!(softened.request_type, RequestType::Writing);
        assert_eq!(softened.source, ClassifierSource::Strands);
        assert_eq!(softened.complexity, 5);
        assert_eq!(softened.complexity_level, 3.7);
        assert_eq!(softened.complexity_confidence, 0.83);
        let probabilities = softened.type_probabilities.as_ref().unwrap();
        assert_eq!(probabilities.len(), 7);
        assert!((probabilities.iter().map(|(_, p)| p).sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(probabilities.iter().all(|(_, p)| *p <= softened.confidence));
        assert!(softened.confidence < identity.confidence);
        assert!(
            softened.is_low_confidence(),
            "existing safe-default policy applies"
        );
    }

    #[test]
    fn validates_complete_unique_distribution_and_selected_maximum() {
        let valid = remote([0.1, 0.1, 0.1, 0.1, 0.4, 0.1, 0.1]);
        let mut incomplete = valid.clone();
        incomplete.type_probabilities.as_mut().unwrap().pop();
        let mut duplicate = valid.clone();
        duplicate.type_probabilities.as_mut().unwrap()[0].0 = RequestType::General;
        let mut inconsistent = valid.clone();
        inconsistent.request_type = RequestType::CodeGeneration;
        let mut nonfinite = valid.clone();
        nonfinite.type_probabilities.as_mut().unwrap()[0].1 = f64::NAN;
        let mut wrong_sum = valid.clone();
        wrong_sum.type_probabilities.as_mut().unwrap()[0].1 = 0.3;
        let mut absent = valid.clone();
        absent.type_probabilities = None;
        for (mut classification, reason) in [
            (incomplete, "bad_response"),
            (duplicate, "bad_response"),
            (inconsistent, "bad_response"),
            (nonfinite, "bad_score"),
            (wrong_sum, "bad_score"),
            (absent, "bad_response"),
        ] {
            assert_eq!(calibrate(&mut classification, 1.0), Err(reason));
        }
        let mut tied = remote([0.4, 0.04, 0.04, 0.04, 0.4, 0.04, 0.04]);
        calibrate(&mut tied, 1.0).unwrap();
        assert_eq!(tied.request_type, RequestType::Writing);
    }

    #[test]
    fn rejects_invalid_temperatures_and_handles_zeros_and_small_temperature() {
        for temperature in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(StrandsClassifier::new(Stub(local()), Stub(local()), temperature).is_err());
        }
        let mut one_hot = remote([0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        calibrate(&mut one_hot, 1e-300).unwrap();
        assert_eq!(one_hot.confidence, 1.0);
        assert_eq!(one_hot.type_probabilities.as_ref().unwrap()[0].1, 0.0);
        let mut uncertain = remote([1.0 / 7.0; 7]);
        calibrate(&mut uncertain, 2.0).unwrap();
        assert!(uncertain.is_low_confidence());
    }

    #[tokio::test]
    async fn preserves_outage_fallback_and_rejects_malformed_probability_contract() {
        let fallback = Classification {
            source: ClassifierSource::Fallback("timeout"),
            ..RegexClassifier::classify_query("write a function")
        };
        let classifier =
            StrandsClassifier::new(Stub(fallback.clone()), Stub(local()), 1.0).unwrap();
        assert_eq!(classify(&classifier).await, fallback);
        let mut malformed = remote([0.1, 0.1, 0.1, 0.1, 0.4, 0.1, 0.1]);
        malformed.request_type = RequestType::General;
        let classifier = StrandsClassifier::new(Stub(malformed), Stub(local()), 1.0).unwrap();
        let result = classify(&classifier).await;
        assert_eq!(result.source, ClassifierSource::Fallback("bad_response"));
        assert_eq!(result.request_type, fallback.request_type);
        assert_eq!(result.type_probabilities, None);
        assert_eq!(result.complexity_confidence, 0.0);
    }
}
