//! Ensemble backend: weighted voting over multiple backends.
//!
//! Different backends make different errors: patterns catch phrasing, TF-IDF
//! catches vocabulary. The ensemble lets them vote, weighted by confidence,
//! and takes the winner. This is the highest-accuracy local backend.
//!
//! ## Voting algorithm
//!
//! 1. Each backend classifies the input independently.
//! 2. Each vote contributes `weight × confidence` to its predicted category.
//! 3. The category with the highest total wins.
//! 4. Complexity is the confidence-weighted mean of backends' estimates.
//! 5. Confidence is the winner's total divided by the sum of all totals.
//!
//! Backends that fail simply don't vote. If all fail, returns a low-confidence
//! `General` (which never happens in practice — the regex never fails).

use std::collections::HashMap;
use std::sync::Arc;

use super::classifier::{
    ClassifyError, ClassifyInput, Classification, RequestClassifier, RequestType,
};

/// A backend with its voting weight.
pub struct WeightedBackend {
    /// The backend.
    pub backend: Arc<dyn RequestClassifier>,
    /// Voting weight (default 1.0). Higher = more influence.
    pub weight: f64,
}

impl WeightedBackend {
    /// Create with default weight 1.0.
    pub fn new(backend: Arc<dyn RequestClassifier>) -> Self {
        Self { backend, weight: 1.0 }
    }

    /// Create with a custom weight (clamped to >= 0).
    pub fn weighted(backend: Arc<dyn RequestClassifier>, weight: f64) -> Self {
        Self { backend, weight: weight.max(0.0) }
    }
}

/// Ensemble classifier over multiple backends.
pub struct EnsembleClassifier {
    backends: Vec<WeightedBackend>,
}

impl EnsembleClassifier {
    /// Create from weighted backends. Panics if empty; use `try_new` for checked.
    pub fn new(backends: Vec<WeightedBackend>) -> Self {
        Self::try_new(backends).expect("ensemble needs at least one backend")
    }

    /// Checked constructor.
    pub fn try_new(backends: Vec<WeightedBackend>) -> Result<Self, ClassifyError> {
        if backends.is_empty() {
            return Err(ClassifyError::Backend("ensemble needs at least one backend".to_string()));
        }
        Ok(Self { backends })
    }

    /// Default ensemble: regex + heuristic + tfidf, equal weights.
    pub fn default_ensemble() -> Self {
        use super::classifier::{HeuristicClassifier, RegexClassifier};
        use super::classifier_tfidf::TfidfClassifier;
        Self::new(vec![
            WeightedBackend::new(Arc::new(RegexClassifier)),
            WeightedBackend::new(Arc::new(HeuristicClassifier)),
            WeightedBackend::new(Arc::new(TfidfClassifier)),
        ])
    }

    /// Number of backends.
    pub fn len(&self) -> usize {
        self.backends.len()
    }

    /// Always false for `new()`; true only if constructed empty (impossible).
    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }
}

#[async_trait::async_trait]
impl RequestClassifier for EnsembleClassifier {
    fn name(&self) -> &'static str {
        "ensemble"
    }

    fn description(&self) -> &'static str {
        "Weighted voting over regex, heuristic, and TF-IDF backends."
    }

    fn uses_context(&self) -> bool {
        self.backends.iter().any(|b| b.backend.uses_context())
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let mut votes: Vec<(RequestType, f64, u8)> = Vec::new();
        for wb in &self.backends {
            if let Ok(c) = wb.backend.classify(input).await {
                votes.push((c.request_type, wb.weight * c.confidence as f64, c.complexity));
            }
        }

        if votes.is_empty() {
            return Ok(Classification::unknown(self.name()));
        }

        let mut totals: HashMap<RequestType, f64> = HashMap::new();
        let mut cx_num = 0.0;
        let mut cx_den = 0.0;
        for (rt, w, cx) in &votes {
            *totals.entry(*rt).or_insert(0.0) += w;
            cx_num += *cx as f64 * w;
            cx_den += w;
        }

        let total_all: f64 = totals.values().sum();
        let mut ranked: Vec<(RequestType, f64)> = totals.into_iter().collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let (winner, winner_total) = ranked[0];
        let confidence = if total_all > 0.0 {
            (winner_total / total_all) as f32
        } else {
            0.25
        };
        let complexity = if cx_den > 0.0 {
            (cx_num / cx_den).round() as u8
        } else {
            2
        };

        Ok(Classification::new(winner, complexity, confidence, self.name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::classifier::{HeuristicClassifier, RegexClassifier};
    use super::super::classifier_tfidf::TfidfClassifier;

    #[tokio::test]
    async fn ensemble_votes() {
        let e = EnsembleClassifier::default_ensemble();
        assert_eq!(e.len(), 3);
        let c = e.classify(&ClassifyInput::new("write a python sort function")).await.unwrap();
        assert_eq!(c.request_type, RequestType::CodeGeneration);
        assert_eq!(c.backend, "ensemble");
    }

    #[tokio::test]
    async fn rejects_empty() {
        assert!(EnsembleClassifier::try_new(vec![]).is_err());
    }
}
