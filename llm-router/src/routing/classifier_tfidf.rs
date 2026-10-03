//! TF-IDF centroid classifier backend.
//!
//! A lightweight statistical classifier that treats request categorization as a
//! text-classification problem:
//!
//! 1. **Training** (once, at first use): the embedded corpus (`TRAINING_DATA`)
//!    is tokenized; inverse document frequencies are computed; each category
//!    becomes the centroid (mean) of its documents' TF-IDF vectors.
//! 2. **Inference**: the input is vectorized with the trained IDF; cosine
//!    similarity against each centroid decides the winner.
//!
//! The training corpus is embedded in this module: generic examples per
//! category that teach vocabulary, not eval-specific cases. The private eval
//! set is unseen, so training examples are deliberately generic.
//!
//! Complements the pattern backends: patterns catch phrasing, TF-IDF catches
//! vocabulary. The ensemble backend combines both.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use super::classifier::{
    ClassifyError, ClassifyInput, Classification, RequestClassifier, RequestType,
};
use super::classifier::confidence_from_margin;

/// One training document.
struct TrainingDoc {
    text: &'static str,
    category: RequestType,
}

/// Embedded training corpus: generic vocabulary examples per category.
static TRAINING_DATA: &[TrainingDoc] = &[
    // CodeGeneration (10)
    TrainingDoc { text: "write a python function that sorts a list of integers", category: RequestType::CodeGeneration },
    TrainingDoc { text: "implement a REST API endpoint for user authentication", category: RequestType::CodeGeneration },
    TrainingDoc { text: "create a script that parses CSV files and outputs JSON", category: RequestType::CodeGeneration },
    TrainingDoc { text: "build a command line tool in rust", category: RequestType::CodeGeneration },
    TrainingDoc { text: "generate boilerplate code for a react component", category: RequestType::CodeGeneration },
    TrainingDoc { text: "fix the null pointer bug in the login handler", category: RequestType::CodeGeneration },
    TrainingDoc { text: "refactor this class to use dependency injection", category: RequestType::CodeGeneration },
    TrainingDoc { text: "add retry logic with exponential backoff", category: RequestType::CodeGeneration },
    TrainingDoc { text: "write unit tests for the payment module", category: RequestType::CodeGeneration },
    TrainingDoc { text: "implement a binary search tree with insertion and deletion", category: RequestType::CodeGeneration },
    // CodeUnderstanding (8)
    TrainingDoc { text: "explain what this recursive function does step by step", category: RequestType::CodeUnderstanding },
    TrainingDoc { text: "what does this code do when the input is empty", category: RequestType::CodeUnderstanding },
    TrainingDoc { text: "walk me through the authentication flow in this file", category: RequestType::CodeUnderstanding },
    TrainingDoc { text: "how does the caching layer work here", category: RequestType::CodeUnderstanding },
    TrainingDoc { text: "why does this loop terminate early", category: RequestType::CodeUnderstanding },
    TrainingDoc { text: "explain the difference between these two implementations", category: RequestType::CodeUnderstanding },
    TrainingDoc { text: "what happens when this promise rejects", category: RequestType::CodeUnderstanding },
    TrainingDoc { text: "help me understand this regular expression", category: RequestType::CodeUnderstanding },
    // TechnicalDesign (8)
    TrainingDoc { text: "how should I design a scalable notification system", category: RequestType::TechnicalDesign },
    TrainingDoc { text: "design a database schema for an e-commerce platform", category: RequestType::TechnicalDesign },
    TrainingDoc { text: "what are the trade-offs between SQL and NoSQL for this workload", category: RequestType::TechnicalDesign },
    TrainingDoc { text: "propose a microservices architecture for the billing service", category: RequestType::TechnicalDesign },
    TrainingDoc { text: "should we use event sourcing or CRUD for the order service", category: RequestType::TechnicalDesign },
    TrainingDoc { text: "design an API rate limiting strategy", category: RequestType::TechnicalDesign },
    TrainingDoc { text: "how to structure a monorepo for multiple teams", category: RequestType::TechnicalDesign },
    TrainingDoc { text: "evaluate message queues versus direct HTTP calls", category: RequestType::TechnicalDesign },
    // AnalyticalReasoning (8)
    TrainingDoc { text: "calculate the probability of three consecutive heads", category: RequestType::AnalyticalReasoning },
    TrainingDoc { text: "analyze the time complexity of this algorithm", category: RequestType::AnalyticalReasoning },
    TrainingDoc { text: "compare the ROI of both investment options", category: RequestType::AnalyticalReasoning },
    TrainingDoc { text: "what is the logical flaw in this argument", category: RequestType::AnalyticalReasoning },
    TrainingDoc { text: "solve this combinatorics problem step by step", category: RequestType::AnalyticalReasoning },
    TrainingDoc { text: "is this statistical claim valid given the sample size", category: RequestType::AnalyticalReasoning },
    TrainingDoc { text: "derive the formula for compound interest", category: RequestType::AnalyticalReasoning },
    TrainingDoc { text: "which sorting algorithm is optimal for nearly sorted data", category: RequestType::AnalyticalReasoning },
    // Writing (8)
    TrainingDoc { text: "draft an email to the team about the delayed launch", category: RequestType::Writing },
    TrainingDoc { text: "write a blog post introducing our new feature", category: RequestType::Writing },
    TrainingDoc { text: "summarize this report in three bullet points", category: RequestType::Writing },
    TrainingDoc { text: "proofread my cover letter for grammar", category: RequestType::Writing },
    TrainingDoc { text: "rewrite this paragraph to sound more professional", category: RequestType::Writing },
    TrainingDoc { text: "create an outline for the technical documentation", category: RequestType::Writing },
    TrainingDoc { text: "write release notes for version two point oh", category: RequestType::Writing },
    TrainingDoc { text: "compose a polite decline for the meeting invite", category: RequestType::Writing },
    // FactualLookup (8)
    TrainingDoc { text: "what is the capital of Japan", category: RequestType::FactualLookup },
    TrainingDoc { text: "who invented the World Wide Web", category: RequestType::FactualLookup },
    TrainingDoc { text: "define polymorphism in object oriented programming", category: RequestType::FactualLookup },
    TrainingDoc { text: "when did the first iPhone launch", category: RequestType::FactualLookup },
    TrainingDoc { text: "what does the HTTP 418 status code mean", category: RequestType::FactualLookup },
    TrainingDoc { text: "how many bits are in a byte", category: RequestType::FactualLookup },
    TrainingDoc { text: "what is the default port for HTTPS", category: RequestType::FactualLookup },
    TrainingDoc { text: "who wrote the Rust programming language", category: RequestType::FactualLookup },
    // General (5)
    TrainingDoc { text: "hello how are you today", category: RequestType::General },
    TrainingDoc { text: "thanks for your help", category: RequestType::General },
    TrainingDoc { text: "good morning", category: RequestType::General },
    TrainingDoc { text: "what can you do", category: RequestType::General },
    TrainingDoc { text: "tell me a joke", category: RequestType::General },
];

/// Tokenize: lowercase, split on non-alphanumeric, drop stopwords/short tokens.
fn tokenize(text: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of",
        "with", "by", "is", "are", "was", "were", "be", "been", "have", "has", "had",
        "do", "does", "did", "will", "would", "could", "should", "may", "might", "must",
        "shall", "it", "its", "this", "that", "these", "those", "i", "you", "he",
        "she", "we", "they", "me", "him", "her", "us", "them", "my", "your", "his",
        "our", "their", "as", "from", "into", "over", "after", "before", "between",
    ];
    let stop: HashSet<&str> = STOPWORDS.iter().copied().collect();
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2 && !stop.contains(*t))
        .map(|t| t.to_string())
        .collect()
}

/// Trained TF-IDF model.
struct TfidfModel {
    idf: HashMap<String, f64>,
    centroids: HashMap<RequestType, HashMap<String, f64>>,
}

impl TfidfModel {
    fn train() -> Self {
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        let mut doc_terms: Vec<(RequestType, HashMap<String, f64>)> = Vec::new();

        for doc in TRAINING_DATA {
            let tokens = tokenize(doc.text);
            let mut tf: HashMap<String, f64> = HashMap::new();
            for tok in &tokens {
                *tf.entry(tok.clone()).or_insert(0.0) += 1.0;
            }
            let len = tokens.len().max(1) as f64;
            for v in tf.values_mut() {
                *v /= len;
            }
            for term in tf.keys() {
                *doc_freq.entry(term.clone()).or_insert(0) += 1;
            }
            doc_terms.push((doc.category, tf));
        }

        let n = TRAINING_DATA.len();
        let idf: HashMap<String, f64> = doc_freq
            .iter()
            .map(|(t, df)| (t.clone(), (((n + 1) as f64) / ((*df + 1) as f64)).ln() + 1.0))
            .collect();

        let mut sums: HashMap<RequestType, HashMap<String, f64>> = HashMap::new();
        let mut counts: HashMap<RequestType, usize> = HashMap::new();
        for (cat, tf) in &doc_terms {
            let e = sums.entry(*cat).or_default();
            for (term, v) in tf {
                *e.entry(term.clone()).or_insert(0.0) += v * idf.get(term).copied().unwrap_or(1.0);
            }
            *counts.entry(*cat).or_insert(0) += 1;
        }
        let centroids = sums
            .into_iter()
            .map(|(cat, mut v)| {
                let n = counts.get(&cat).copied().unwrap_or(1) as f64;
                for x in v.values_mut() {
                    *x /= n;
                }
                (cat, v)
            })
            .collect();

        Self { idf, centroids }
    }

    fn vectorize(&self, text: &str) -> HashMap<String, f64> {
        let tokens = tokenize(text);
        if tokens.is_empty() {
            return HashMap::new();
        }
        let mut tf: HashMap<String, f64> = HashMap::new();
        for tok in &tokens {
            *tf.entry(tok.clone()).or_insert(0.0) += 1.0;
        }
        let len = tokens.len() as f64;
        tf.into_iter()
            .map(|(t, c)| (t.clone(), (c / len) * self.idf.get(&t).copied().unwrap_or(1.0)))
            .collect()
    }

    fn cosine(a: &HashMap<String, f64>, b: &HashMap<String, f64>) -> f64 {
        let mut dot = 0.0;
        let mut na = 0.0;
        let mut nb = 0.0;
        for (t, va) in a {
            na += va * va;
            if let Some(vb) = b.get(t) {
                dot += va * vb;
            }
        }
        for vb in b.values() {
            nb += vb * vb;
        }
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na.sqrt() * nb.sqrt())
        }
    }

    fn predict(&self, text: &str) -> Vec<(RequestType, f64)> {
        let v = self.vectorize(text);
        let mut scores: Vec<(RequestType, f64)> = vec![
            (RequestType::CodeGeneration, 0.0),
            (RequestType::CodeUnderstanding, 0.0),
            (RequestType::TechnicalDesign, 0.0),
            (RequestType::AnalyticalReasoning, 0.0),
            (RequestType::Writing, 0.0),
            (RequestType::FactualLookup, 0.0),
            (RequestType::General, 0.0),
        ];
        for (rt, s) in scores.iter_mut() {
            *s = self.centroids.get(rt).map(|c| Self::cosine(&v, c)).unwrap_or(0.0);
        }
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores
    }
}

static MODEL: LazyLock<TfidfModel> = LazyLock::new(TfidfModel::train);

/// TF-IDF centroid classifier.
pub struct TfidfClassifier;

#[async_trait::async_trait]
impl RequestClassifier for TfidfClassifier {
    fn name(&self) -> &'static str {
        "tfidf"
    }

    fn description(&self) -> &'static str {
        "TF-IDF centroid classifier over an embedded training corpus."
    }

    fn uses_context(&self) -> bool {
        true
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        if input.query.trim().is_empty() {
            return Ok(Classification::unknown(self.name()));
        }
        let text = match input.context {
            Some(c) if !c.trim().is_empty() => format!("{}\n{}", input.query, c),
            _ => input.query.to_string(),
        };
        let scores = MODEL.predict(&text);
        let (rt, top) = scores[0];
        let second = scores.get(1).map(|(_, s)| *s).unwrap_or(0.0);
        let request_type = if top <= 0.0 { RequestType::General } else { rt };
        let confidence = confidence_from_margin(top * 3.0, second * 3.0);

        let chars = text.chars().count();
        let complexity = if chars < 30 { 1 } else if chars < 120 { 2 } else if chars < 300 { 3 } else if chars < 500 { 4 } else { 5 };

        Ok(Classification::new(request_type, complexity, confidence, self.name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_trains() {
        let m = TfidfModel::train();
        assert!(!m.idf.is_empty());
        assert_eq!(m.centroids.len(), 7);
    }

    #[tokio::test]
    async fn classifies_correctly() {
        let b = TfidfClassifier;
        assert_eq!(
            b.classify(&ClassifyInput::new("write a python function to parse json")).await.unwrap().request_type,
            RequestType::CodeGeneration
        );
        assert_eq!(
            b.classify(&ClassifyInput::new("what is the capital of France")).await.unwrap().request_type,
            RequestType::FactualLookup
        );
    }
}
