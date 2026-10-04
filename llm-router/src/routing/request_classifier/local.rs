use async_trait::async_trait;

use super::features::{contains_any, count_any, normalize_text, token_count};
use super::{
    ClassifyError, ClassifyInput, Classification, ContextSignals, RequirementProfile,
    RequestClassifier, analyze_context_integrity, clamp_dim, complexity_from_profile,
    derive_context_signals,
};
use crate::routing::RequestType;

/// Deterministic local classifier used for P2 evaluation and console demo. This is a
/// compact, CPU-runnable implementation that estimates the request's requirement profile
/// from query + context rather than hard-coding a single regex category.
pub struct LocalRequestClassifier;

impl LocalRequestClassifier {
    pub fn embedded() -> Result<Self, ClassifyError> {
        Ok(Self)
    }

    pub fn load(path: &str) -> Result<Self, ClassifyError> {
        let _ = path;
        Ok(Self)
    }
}

#[async_trait]
impl RequestClassifier for LocalRequestClassifier {
    fn name(&self) -> &str {
        "local"
    }

    async fn classify(&self, input: &ClassifyInput<'_>) -> Result<Classification, ClassifyError> {
        let query = input.query.trim();
        let context = input.context.map(str::trim).unwrap_or("");
        let request_type = infer_request_type(query, context);

        let conversation_dependency = infer_conversation_dependency(query, context, input.cache_affinity);
        let profile = infer_profile(request_type, query, context, conversation_dependency);
        let context_signals = derive_context_signals(input, conversation_dependency);
        let complexity = complexity_from_profile(&profile);
        let confidence = infer_confidence(&request_type, query, context, &profile, &context_signals);

        let out = Classification {
            request_type,
            complexity,
            confidence,
            profile,
            context: context_signals,
            context_integrity: analyze_context_integrity(query, input.context),
            fallback: false,
        };
        out.validate()?;
        Ok(out)
    }
}

fn infer_request_type(query: &str, context: &str) -> RequestType {
    let text = format!("{} {}", query, context).to_ascii_lowercase();

    let code_generation = [
        "fix", "implement", "build", "create", "write", "patch", "refactor", "add support",
        "add validation", "bug", "error handling", "middleware", "function", "module",
        "script", "api", "endpoint", "class", "method", "service",
    ];
    let code_understanding = [
        "explain", "what does this", "how does this", "walk me through", "why does this",
        "what is this code doing", "why is this happening", "trace this",
    ];
    let technical_design = [
        "design this", "design the", "system design", "architecture", "trade-off", "tradeoff",
        "api design", "db schema", "schema design", "migration plan",
    ];
    let analytical_reasoning = [
        "calculate", "probability", "estimate", "derive", "solve", "proof", "compare",
        "why would", "how many", "average", "sum", "percent", "analysis",
    ];
    let writing = [
        "draft", "email", "blog", "article", "memo", "letter", "rewrite", "compose",
        "make this sound", "proposal",
    ];
    let factual = [
        "capital of", "who is", "what is", "where is", "when was", "define ", "what are",
    ];

    if count_any(&text, &technical_design) > 0 && !contains_any(&text, &["fix", "bug"]) {
        return RequestType::TechnicalDesign;
    }
    if count_any(&text, &code_understanding) > 0 {
        return RequestType::CodeUnderstanding;
    }
    if count_any(&text, &writing) > 0 {
        return RequestType::Writing;
    }
    if count_any(&text, &analytical_reasoning) > 0 {
        return RequestType::AnalyticalReasoning;
    }
    if count_any(&text, &factual) > 0 && !contains_any(&text, &["code", "function", "class"]) {
        return RequestType::FactualLookup;
    }
    if count_any(&text, &code_generation) > 0 || contains_any(&text, &["TODO", "refactor", "auth", "middleware"]) {
        return RequestType::CodeGeneration;
    }
    if query.starts_with("what ") || query.starts_with("who ") || query.starts_with("where ") || query.starts_with("when ") {
        return RequestType::FactualLookup;
    }
    RequestType::General
}

fn infer_conversation_dependency(
    query: &str,
    context: &str,
    cache_affinity: Option<bool>,
) -> u8 {
    if let Some(v) = cache_affinity {
        return if v { 5 } else { 2 };
    }
    let short = token_count(query) < 10;
    let has_context = !context.trim().is_empty();
    let mentions_prior = contains_any(context, &["previous", "earlier", "before", "last turn", "we discussed", "as discussed"])
        || contains_any(query, &["this", "that", "it", "now", "continue", "fix the retry logic"]);

    match (short, has_context, mentions_prior) {
        (true, true, true) => 5,
        (true, true, false) => 4,
        (true, false, _) => 2,
        (false, true, true) => 4,
        _ => 3,
    }
}

fn infer_profile(
    request_type: RequestType,
    query: &str,
    context: &str,
    conversation_dependency: u8,
) -> RequirementProfile {
    let lower = normalize_text(query);
    let context_lower = normalize_text(context);
    let is_code = matches!(request_type, RequestType::CodeGeneration | RequestType::CodeUnderstanding);
    let has_multi_file = contains_any(&format!("{} {}", lower, context_lower), &["several files", "multiple files", "across", "src/", ".rs", ".ts", ".py", "module", "package"]);
    let has_debug = contains_any(&format!("{} {}", lower, context_lower), &["bug", "diagnose", "failure", "error", "401", "timeout", "race", "stale", "intermittent", "exception", "token refresh"]);
    let has_design = contains_any(&format!("{} {}", lower, context_lower), &["design", "architecture", "trade-off", "tradeoff", "schema", "migration", "plan"]);
    let has_high_impact = contains_any(&format!("{} {}", lower, context_lower), &["auth", "authentication", "payment", "security", "database", "middleware", "critical", "oauth", "token"]) || has_multi_file;

    let reasoning = match request_type {
        RequestType::TechnicalDesign | RequestType::AnalyticalReasoning => 4,
        RequestType::CodeUnderstanding => 3,
        RequestType::CodeGeneration if has_design => 3,
        RequestType::Writing => 2,
        RequestType::FactualLookup => 1,
        _ => 2,
    };

    let coding = match request_type {
        RequestType::CodeGeneration => {
            if has_multi_file || has_debug || has_high_impact { 4 } else { 2 }
        }
        RequestType::CodeUnderstanding => 2,
        RequestType::TechnicalDesign => 3,
        RequestType::AnalyticalReasoning => 2,
        _ => 1,
    };

    let debugging = if has_debug {
        if has_multi_file || has_high_impact { 4 } else { 3 }
    } else {
        if request_type == RequestType::CodeGeneration && has_multi_file { 3 } else { 1 }
    };

    let tool = if has_multi_file || has_debug || contains_any(&format!("{} {}", lower, context_lower), &["read", "inspect", "check", "repo", "files", "across"]) {
        4
    } else if is_code {
        2
    } else {
        1
    };

    let change = if has_high_impact {
        5
    } else if has_multi_file {
        4
    } else if request_type == RequestType::TechnicalDesign {
        4
    } else {
        2
    };

    let conversation = conversation_dependency.max(1).min(5);

    RequirementProfile {
        reasoning_depth: clamp_dim(reasoning as f32),
        coding_complexity: clamp_dim(coding as f32),
        debugging_difficulty: clamp_dim(debugging as f32),
        tool_orchestration: clamp_dim(tool as f32),
        conversation_dependency: conversation,
        change_impact: clamp_dim(change as f32),
    }
}

fn infer_confidence(
    request_type: &RequestType,
    query: &str,
    context: &str,
    profile: &RequirementProfile,
    context_signals: &ContextSignals,
) -> f32 {
    let words = token_count(query) as f32;
    let mut conf = 0.72f32;

    if matches!(request_type, RequestType::FactualLookup | RequestType::Writing) {
        conf += 0.12;
    } else if matches!(request_type, RequestType::CodeGeneration | RequestType::TechnicalDesign | RequestType::CodeUnderstanding) {
        conf += 0.08;
    }
    if words >= 12.0 && words <= 48.0 {
        conf += 0.08;
    } else if words < 6.0 {
        conf -= 0.12;
    }
    if !context.trim().is_empty() {
        conf += 0.04;
    }
    if profile.conversation_dependency >= 4 {
        conf -= 0.06;
    }
    if context_signals.cache_affinity == Some(true) {
        conf -= 0.04;
    }
    if query.contains("or") && query.contains("and") {
        conf -= 0.08;
    }
    if request_type == &RequestType::General {
        conf -= 0.14;
    }
    conf.clamp(0.35, 0.96)
}
