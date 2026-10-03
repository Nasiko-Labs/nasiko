//! Hosted Jev request classifier.
//!
//! This module owns only the TypeSafe System One HTTP dialect.  Environment handling and
//! backend selection stay in `config`/`classifier`, and any failure returned here is converted
//! to the offline regex result by `FallbackClassifier`.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::classifier::{
    Classification, ClassifyError, ClassifyInput, RequestClassifier, RequestType,
};

const MAX_STATE_CHARS: usize = 8_000;

pub struct JevClassifier {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    timeout: Duration,
}

impl JevClassifier {
    pub fn from_config(cfg: &crate::config::GatewayConfig) -> Result<Self, ClassifyError> {
        if cfg.classifier_endpoint.trim().is_empty() {
            return Err(ClassifyError::Backend("empty CLASSIFIER_ENDPOINT".into()));
        }
        if cfg.classifier_api_key.trim().is_empty() {
            return Err(ClassifyError::Backend("empty classifier API key".into()));
        }
        let timeout = Duration::from_millis(cfg.classifier_timeout_ms.max(1));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| ClassifyError::Backend(format!("cannot create HTTP client: {e}")))?;
        Ok(Self {
            client,
            endpoint: cfg.classifier_endpoint.clone(),
            api_key: cfg.classifier_api_key.clone(),
            model: cfg.classifier_model.clone(),
            timeout,
        })
    }

    fn request_body(&self, input: &ClassifyInput<'_>) -> Value {
        let query: String = input.query.chars().take(MAX_STATE_CHARS).collect();
        let context = input.context.map(|value| {
            value
                .chars()
                .take(MAX_STATE_CHARS.saturating_sub(query.chars().count()))
                .collect::<String>()
        });
        let mut body = json!({
            "state": { "query": query, "context": context },
            "questions": {
                "request_type": {
                    "type": "choice",
                    "instructions": "Classify the user's primary requested work. Choose exactly one category.",
                    "criteria": {
                        "code_generation": "write, modify, debug, or implement code",
                        "code_understanding": "explain, review, or understand existing code",
                        "technical_design": "design an architecture, system, API, or technical approach",
                        "analytical_reasoning": "solve multi-step math, logic, analysis, or reasoning",
                        "writing": "draft, rewrite, summarize, or edit prose",
                        "factual_lookup": "answer a factual or informational lookup question",
                        "general": "general conversation or another kind of request"
                    }
                },
                "complexity": {
                    "type": "score",
                    "instructions": "Rate the work complexity from 1 (trivial one-shot request) to 5 (hard multi-step expert work).",
                    "criteria": ["1 trivial", "2 simple", "3 moderate", "4 complex", "5 expert"]
                }
            }
        });
        if !self.model.trim().is_empty() {
            body["model"] = Value::String(self.model.clone());
        }
        body
    }

    fn parse_response(value: Value) -> Result<Classification, ClassifyError> {
        let answers = value
            .get("answers")
            .and_then(Value::as_object)
            .ok_or_else(|| ClassifyError::Invalid("missing answers object".into()))?;
        let request_type = answers
            .get("request_type")
            .and_then(Value::as_object)
            .ok_or_else(|| ClassifyError::Invalid("missing request_type answer".into()))?;
        let type_name = request_type.get("type").and_then(Value::as_str);
        if type_name != Some("choice") {
            return Err(ClassifyError::Invalid(
                "request_type answer is not a choice".into(),
            ));
        }
        let choice = request_type
            .get("choice")
            .and_then(Value::as_str)
            .and_then(RequestType::from_wire)
            .ok_or_else(|| {
                ClassifyError::Invalid("unknown or missing request_type choice".into())
            })?;
        let type_confidence =
            number_in_unit_interval(request_type.get("confidence"), "request_type confidence")?;

        let complexity = answers
            .get("complexity")
            .and_then(Value::as_object)
            .ok_or_else(|| ClassifyError::Invalid("missing complexity answer".into()))?;
        if complexity.get("type").and_then(Value::as_str) != Some("score") {
            return Err(ClassifyError::Invalid(
                "complexity answer is not a score".into(),
            ));
        }
        let score = complexity
            .get("score")
            .and_then(Value::as_f64)
            .ok_or_else(|| ClassifyError::Invalid("missing complexity score".into()))?;
        // Jev score indices are 0–4 for our five ordered criteria. Reject anything else
        // rather than guessing a complexity level.
        let complexity_value = if score.fract() == 0.0 && (0.0..=4.0).contains(&score) {
            score as u8 + 1
        } else {
            return Err(ClassifyError::Invalid(
                "complexity score must be an integer in 0..=4".into(),
            ));
        };
        let complexity_confidence =
            number_in_unit_interval(complexity.get("confidence"), "complexity confidence")?;

        Ok(Classification {
            request_type: choice,
            complexity: complexity_value,
            confidence: type_confidence.min(complexity_confidence),
            fallback: false,
        })
    }
}

fn number_in_unit_interval(value: Option<&Value>, name: &str) -> Result<f32, ClassifyError> {
    let value = value
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
        .ok_or_else(|| ClassifyError::Invalid(format!("missing or invalid {name}")))?;
    Ok(value as f32)
}

#[async_trait]
impl RequestClassifier for JevClassifier {
    fn name(&self) -> &str {
        "jev"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&self.request_body(input))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ClassifyError::Timeout(self.timeout)
                } else {
                    ClassifyError::Backend(e.to_string())
                }
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let detail: String = body.chars().take(240).collect();
            return Err(ClassifyError::Backend(format!(
                "HTTP {}: {}",
                status, detail
            )));
        }
        let value = response
            .json::<Value>()
            .await
            .map_err(|e| ClassifyError::Invalid(format!("invalid JSON response: {e}")))?;
        Self::parse_response(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_typed_answers() {
        let c = JevClassifier::parse_response(json!({"answers": {
            "request_type": {"type":"choice", "choice":"technical_design", "confidence":0.9},
            "complexity": {"type":"score", "score":4.0, "confidence":0.8}
        }}))
        .unwrap();
        assert_eq!(c.request_type, RequestType::TechnicalDesign);
        assert_eq!(c.complexity, 5);
        assert_eq!(c.confidence, 0.8);
    }

    #[test]
    fn rejects_unknown_choice_and_bad_score() {
        let bad = json!({"answers": {
            "request_type": {"type":"choice", "choice":"invented", "confidence":1.0},
            "complexity": {"type":"score", "score":2.5, "confidence":1.0}
        }});
        assert!(JevClassifier::parse_response(bad).is_err());
    }
}
