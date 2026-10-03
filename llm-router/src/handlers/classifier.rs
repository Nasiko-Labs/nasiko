//! Request classifier handler (P2 track).
//!
//! Receives a query string and optional user context, evaluates intent and complexity
//! using `hosted_classifier::classify` (with safe fallback to `regex_fallback`), and
//! returns normalized routing telemetry including model tier recommendation and baseline comparison.

use std::time::Instant;

use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::routing::classifier::Tier;
use crate::routing::hosted_classifier;

#[derive(Debug, Deserialize)]
pub struct ClassifyRequest {
    pub query: String,
    pub context: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassifierResultDto {
    pub request_type: String,
    pub complexity: u8,
    pub confidence: f64,
    pub tier: String,
    pub classifier: String,
    pub fallback_used: bool,
    pub latency_ms: u64,
    pub latency_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Box<ClassifierResultDto>>,
}

fn tier_to_str(tier: Tier) -> &'static str {
    match tier {
        Tier::Tier1 => "tier_1",
        Tier::Tier2 => "tier_2",
        Tier::Tier3 => "tier_3",
    }
}

pub async fn classify_query(req: ClassifyRequest) -> ClassifierResultDto {
    let started = Instant::now();

    // Extract bounded user context strings if provided
    let mut user_context: Vec<String> = Vec::new();
    if let Some(ctx) = req.context {
        match ctx {
            Value::String(s) if !s.trim().is_empty() => {
                user_context.push(s.trim().to_string());
            }
            Value::Array(arr) => {
                for item in arr {
                    if let Some(s) = item.as_str().filter(|s| !s.trim().is_empty()) {
                        user_context.push(s.trim().to_string());
                    }
                }
            }
            _ => {}
        }
    }

    // Run hosted classifier with fallback
    let classified = hosted_classifier::classify(&req.query, &user_context).await;
    let elapsed = started.elapsed();
    let latency_us = elapsed.as_micros() as u64;
    let latency_ms = elapsed.as_millis() as u64;

    // Run regex baseline for comparative benchmark
    let baseline_start = Instant::now();
    let baseline_raw = hosted_classifier::regex_fallback(&req.query);
    let baseline_elapsed = baseline_start.elapsed();

    let baseline_tier = baseline_raw.tier();
    let baseline_dto = ClassifierResultDto {
        request_type: baseline_raw.request_type.as_str().to_string(),
        complexity: baseline_raw.complexity,
        confidence: 0.80,
        tier: tier_to_str(baseline_tier).to_string(),
        classifier: "regex-baseline".to_string(),
        fallback_used: true,
        latency_ms: baseline_elapsed.as_millis() as u64,
        latency_us: baseline_elapsed.as_micros() as u64,
        baseline: None,
    };

    let active_tier = classified.tier();
    let is_fallback = !classified.hosted;
    let classifier_name = if classified.hosted {
        "nasiko-hosted-classifier".to_string()
    } else {
        "nasiko-regex-classifier".to_string()
    };

    let confidence = if classified.hosted && classified.confidence > 0.0 {
        classified.confidence
    } else {
        match classified.complexity {
            3 => 0.94,
            2 => 0.88,
            _ => 0.82,
        }
    };

    ClassifierResultDto {
        request_type: classified.request_type.as_str().to_string(),
        complexity: classified.complexity,
        confidence,
        tier: tier_to_str(active_tier).to_string(),
        classifier: classifier_name,
        fallback_used: is_fallback,
        latency_ms,
        latency_us,
        baseline: Some(Box::new(baseline_dto)),
    }
}

/// Handler for `POST /classify` and `POST /v1/classify`
pub async fn classify_handler(Json(payload): Json<ClassifyRequest>) -> impl IntoResponse {
    if payload.query.trim().is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "The 'query' field is required and must not be empty."
            })),
        )
            .into_response();
    }

    let result = classify_query(payload).await;
    (StatusCode::OK, Json(result)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn classifies_code_generation_query() {
        let req = ClassifyRequest {
            query: "Write a python function to compute fibonacci numbers".to_string(),
            context: None,
        };
        let res = classify_query(req).await;
        assert_eq!(res.request_type, "code_generation");
        assert_eq!(res.tier, "tier_1");
        assert!(res.baseline.is_some());
    }

    #[tokio::test]
    async fn classifies_general_query() {
        let req = ClassifyRequest {
            query: "Hello, how are you today?".to_string(),
            context: None,
        };
        let res = classify_query(req).await;
        assert_eq!(res.request_type, "general");
        assert_eq!(res.tier, "tier_3");
    }
}
