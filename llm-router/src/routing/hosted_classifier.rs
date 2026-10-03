//! Optional OpenAI-compatible request classifier. Any unavailable or untrusted hosted
//! result is discarded and the local regex classifier remains authoritative.

use serde::Deserialize;

use super::classifier::{RequestType, Tier, classify_request_type};

pub const MAX_CONTEXT_MESSAGES: usize = 6;
pub const MAX_CONTEXT_CHARS: usize = 6_000;
pub const MAX_QUERY_CHARS: usize = 2_000;
pub const MIN_CONFIDENCE: f64 = 0.65;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Classification {
    pub request_type: RequestType,
    /// One is simplest, three is most complex.
    pub complexity: u8,
    pub confidence: f64,
    pub hosted: bool,
}

impl Classification {
    pub fn tier(self) -> Tier {
        match self.complexity {
            1 => Tier::Tier3,
            2 => Tier::Tier2,
            _ => Tier::Tier1,
        }
    }
}

#[derive(Deserialize)]
struct Completion {
    choices: Vec<Choice>,
}
#[derive(Deserialize)]
struct Choice {
    message: AssistantMessage,
}
#[derive(Deserialize)]
struct AssistantMessage {
    content: String,
}
#[derive(Deserialize)]
struct HostedResult {
    request_type: String,
    complexity: u8,
    confidence: f64,
}

fn parse_result(raw: &str) -> Option<Classification> {
    let value: HostedResult = serde_json::from_str(raw.trim()).ok()?;
    let request_type = RequestType::from_wire(&value.request_type)?;
    if !(1..=3).contains(&value.complexity)
        || !value.confidence.is_finite()
        || !(MIN_CONFIDENCE..=1.0).contains(&value.confidence)
    {
        return None;
    }
    Some(Classification {
        request_type,
        complexity: value.complexity,
        confidence: value.confidence,
        hosted: true,
    })
}

fn bounded_context(user_context: &[String]) -> Vec<String> {
    let mut remaining = MAX_CONTEXT_CHARS;
    user_context.iter().rev().take(MAX_CONTEXT_MESSAGES)
        .filter_map(|item| {
            let bounded: String = item.chars().take(remaining).collect();
            remaining = remaining.saturating_sub(bounded.chars().count());
            (!bounded.is_empty()).then_some(bounded)
        })
        .collect::<Vec<_>>().into_iter().rev().collect()
}

pub fn regex_fallback(query: &str) -> Classification {
    let request_type = classify_request_type(query);
    let complexity = match request_type {
        RequestType::CodeGeneration | RequestType::TechnicalDesign | RequestType::AnalyticalReasoning => 3,
        RequestType::CodeUnderstanding | RequestType::Writing => 2,
        RequestType::FactualLookup | RequestType::General => 1,
    };
    Classification { request_type, complexity, confidence: 0.0, hosted: false }
}

/// The hosted classifier receives user text only. Context is newest-first bounded by both
/// message count and characters; callers must filter roles before passing it here.
pub async fn classify(query: &str, user_context: &[String]) -> Classification {
    let fallback = regex_fallback(query);
    let Some(api_key) = std::env::var("NASIKO_CLASSIFIER_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
    else {
        return fallback;
    };

    let base = std::env::var("NASIKO_CLASSIFIER_BASE_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
    let model = std::env::var("NASIKO_CLASSIFIER_MODEL")
        .unwrap_or_else(|_| "gpt-4o-mini".to_string());
    let query: String = query.chars().take(MAX_QUERY_CHARS).collect();
    let context = bounded_context(user_context);
    let prompt = format!(
        "Classify the user's current request. Use prior user messages only as context. Return only JSON with request_type (one of code_generation, code_understanding, technical_design, analytical_reasoning, writing, factual_lookup, general), complexity (integer 1 simple to 3 complex), and confidence (number 0 to 1).\nContext: {}\nCurrent request: {}",
        context.join("\n"), query
    );
    let endpoint = format!("{}/chat/completions", base.trim_end_matches('/'));
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build() else { return fallback };
    let response = client
        .post(endpoint)
        .bearer_auth(api_key)
        .json(&serde_json::json!({
            "model": model,
            "temperature": 0,
            "messages": [{"role":"user", "content": prompt}]
        }))
        .send()
        .await;
    let Ok(response) = response else { return fallback };
    if !response.status().is_success() { return fallback; }
    let Ok(completion) = response.json::<Completion>().await else { return fallback };
    completion.choices.first()
        .and_then(|choice| parse_result(&choice.message.content))
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_malformed_unknown_and_low_confidence_results() {
        assert!(parse_result("not json").is_none());
        assert!(parse_result(r#"{"request_type":"unknown","complexity":2,"confidence":0.9}"#).is_none());
        assert!(parse_result(r#"{"request_type":"general","complexity":2,"confidence":0.2}"#).is_none());
        assert!(parse_result(r#"{"request_type":"general","complexity":4,"confidence":0.9}"#).is_none());
    }

    #[test]
    fn complexity_maps_to_model_strength() {
        assert_eq!(Classification { request_type: RequestType::General, complexity: 1, confidence: 1.0, hosted: true }.tier(), Tier::Tier3);
        assert_eq!(Classification { request_type: RequestType::General, complexity: 2, confidence: 1.0, hosted: true }.tier(), Tier::Tier2);
        assert_eq!(Classification { request_type: RequestType::General, complexity: 3, confidence: 1.0, hosted: true }.tier(), Tier::Tier1);
    }

    #[test]
    fn regex_fallback_derives_complexity_and_tier() {
        let simple = regex_fallback("What is the capital of France?");
        assert_eq!(simple.request_type, RequestType::FactualLookup);
        assert_eq!(simple.tier(), Tier::Tier3);
        let complex = regex_fallback("Build a Python script to parse a CSV file");
        assert_eq!(complex.request_type, RequestType::CodeGeneration);
        assert_eq!(complex.tier(), Tier::Tier1);
        assert!(!complex.hosted);
    }

    #[test]
    fn context_is_limited_by_message_count_and_total_characters() {
        let context: Vec<String> = (0..10).map(|n| format!("{n}:{}", "x".repeat(1_500))).collect();
        let bounded = bounded_context(&context);
        assert!(bounded.len() <= MAX_CONTEXT_MESSAGES);
        assert!(bounded.iter().map(|s| s.chars().count()).sum::<usize>() <= MAX_CONTEXT_CHARS);
        assert!(bounded.last().unwrap().starts_with("9:"));
    }
}
