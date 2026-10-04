use async_trait::async_trait;

use super::{
    ClassifyError, ClassifyInput, Classification, ContextSignals, RequirementProfile,
    RequestClassifier,
};
use crate::routing::classifier::classify_request_type as legacy_type;
use crate::routing::classifier::RequestType as LegacyRequestType;

/// Regex-backed request classifier. Keeps the old behaviour and acts as the default
/// fallback when the experimental local/hosted backend is off or fails.
pub struct RegexRequestClassifier;

#[async_trait]
impl RequestClassifier for RegexRequestClassifier {
    fn name(&self) -> &str {
        "regex"
    }

    async fn classify(
        &self,
        input: &ClassifyInput<'_>,
    ) -> Result<Classification, ClassifyError> {
        let request_type = legacy_type(input.query);
        let context = input.context.unwrap_or("");
        let conversation_dependency = match input.context {
            Some(ctx) if !ctx.trim().is_empty() && input.query.trim().split_whitespace().count() < 10 => 4,
            Some(_) => 3,
            None => 2,
        };
        let profile = match request_type {
            LegacyRequestType::CodeGeneration => RequirementProfile {
                reasoning_depth: 3,
                coding_complexity: 3,
                debugging_difficulty: 2,
                tool_orchestration: 3,
                conversation_dependency,
                change_impact: 3,
            },
            LegacyRequestType::CodeUnderstanding => RequirementProfile {
                reasoning_depth: 3,
                coding_complexity: 2,
                debugging_difficulty: 2,
                tool_orchestration: 2,
                conversation_dependency: conversation_dependency.max(2),
                change_impact: 2,
            },
            LegacyRequestType::TechnicalDesign => RequirementProfile {
                reasoning_depth: 4,
                coding_complexity: 3,
                debugging_difficulty: 2,
                tool_orchestration: 3,
                conversation_dependency: 3,
                change_impact: 4,
            },
            LegacyRequestType::AnalyticalReasoning => RequirementProfile {
                reasoning_depth: 4,
                coding_complexity: 2,
                debugging_difficulty: 2,
                tool_orchestration: 1,
                conversation_dependency: 2,
                change_impact: 2,
            },
            LegacyRequestType::Writing => RequirementProfile {
                reasoning_depth: 2,
                coding_complexity: 1,
                debugging_difficulty: 1,
                tool_orchestration: 1,
                conversation_dependency: 2,
                change_impact: 2,
            },
            LegacyRequestType::FactualLookup => RequirementProfile {
                reasoning_depth: 2,
                coding_complexity: 1,
                debugging_difficulty: 1,
                tool_orchestration: 1,
                conversation_dependency: 2,
                change_impact: 1,
            },
            LegacyRequestType::General => RequirementProfile {
                reasoning_depth: 1,
                coding_complexity: 1,
                debugging_difficulty: 1,
                tool_orchestration: 1,
                conversation_dependency: 1,
                change_impact: 1,
            },
        };
        let context_signals = ContextSignals {
            codebase_size: input.codebase_size.or_else(|| super::codebase_size_from_text(input.context)),
            context_size: super::context_size_from_text(input.context),
            cache_affinity: input.cache_affinity.or_else(|| {
                let ctx = context.trim();
                if !ctx.is_empty() && input.query.trim().split_whitespace().count() < 12 {
                    Some(true)
                } else {
                    None
                }
            }),
        };
        let complexity = super::complexity_from_profile(&profile);
        let mut confidence = match request_type {
            LegacyRequestType::General => 0.58f32,
            LegacyRequestType::FactualLookup => 0.8f32,
            _ => 0.76f32,
        };
        if !context.is_empty() && input.query.trim().split_whitespace().count() < 12 {
            confidence += 0.06;
        }
        confidence = confidence.clamp(0.0, 0.99);

        let out = Classification {
            request_type,
            complexity,
            confidence,
            profile,
            context: context_signals,
            context_integrity: super::analyze_context_integrity(input.query, input.context),
            fallback: false,
        };
        out.validate()?;
        Ok(out)
    }
}
