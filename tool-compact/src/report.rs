use crate::{CompactError, CompactTools, ScopeInput, ToolDef, encode_tools, select_tools};

/// Explicit request policy; every optimization defaults to caller opt-in.
pub struct OptimizationContext<'a> {
    /// Candidate tools in original order.
    pub tools: &'a [ToolDef],
    /// User text; absent means selection cannot infer relevance.
    pub query: Option<&'a str>,
    /// Explicit compaction enablement.
    pub compact_enabled: bool,
    /// Explicit ToolScope enablement, independent of compaction.
    pub scope_enabled: bool,
    /// Forced/unsupported choice semantics must use native tools in V1.
    pub forced_tool_choice: bool,
}

/// All-native or all-compact behavior, never an unsafe hybrid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizationPlan {
    /// Use original native tools.
    Native,
    /// Compile every supplied tool.
    CompactAll,
    /// Conservatively select, then compile the entire selected set.
    SelectAndCompact,
}

/// Why a whole request kept the native representation.
#[derive(Debug, Clone, PartialEq)]
pub enum BypassReason {
    /// Caller left compaction disabled.
    Disabled,
    /// No tool definitions were supplied.
    NoTools,
    /// Forced-choice semantics are outside the V1 contract.
    ForcedToolChoice,
    /// Guard rejected the schema or a documented resource bound.
    Schema(CompactError),
}

/// Structured optimization evidence, without tokenizer or I/O dependencies.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizationReport {
    /// Candidate count.
    pub input_tool_count: usize,
    /// Actual emitted count, including all original tools on native fallback.
    pub selected_tool_count: usize,
    /// Safely compiled count.
    pub schemas_compacted: usize,
    /// Native schema count.
    pub schemas_bypassed: usize,
    /// Actual behavior.
    pub plan: OptimizationPlan,
    /// Optional lexical selection heuristic.
    pub scope_confidence: Option<f32>,
    /// Typed native-fallback reasons.
    pub bypass_reasons: Vec<BypassReason>,
}

/// Policy result; `None` means pass through all original native tools.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizationOutcome {
    /// Model-visible schema grammar only when compaction is safe.
    pub compact: Option<CompactTools>,
    /// Actual emitted tool indices in original relative order.
    pub selected_indices: Vec<usize>,
    /// Evidence for the plan.
    pub report: OptimizationReport,
}

/// Apply SCOPE → ZIP → GUARD preflight without reading runtime configuration.
pub fn optimize_tools(context: OptimizationContext<'_>) -> OptimizationOutcome {
    let native = |reason, confidence| OptimizationOutcome {
        compact: None,
        selected_indices: (0..context.tools.len()).collect(),
        report: OptimizationReport {
            input_tool_count: context.tools.len(),
            selected_tool_count: context.tools.len(),
            schemas_compacted: 0,
            schemas_bypassed: context.tools.len(),
            plan: OptimizationPlan::Native,
            scope_confidence: confidence,
            bypass_reasons: vec![reason],
        },
    };
    if !context.compact_enabled {
        return native(BypassReason::Disabled, None);
    }
    if context.tools.is_empty() {
        return native(BypassReason::NoTools, None);
    }
    if context.forced_tool_choice {
        return native(BypassReason::ForcedToolChoice, None);
    }
    let (indices, confidence) = if context.scope_enabled {
        let decision = select_tools(ScopeInput {
            query: context.query.unwrap_or(""),
            tools: context.tools,
        });
        (decision.selected_indices, Some(decision.confidence))
    } else {
        ((0..context.tools.len()).collect(), None)
    };
    let selected: Vec<ToolDef> = indices
        .iter()
        .map(|&index| context.tools[index].clone())
        .collect();
    match encode_tools(&selected) {
        Err(error) => native(BypassReason::Schema(error), confidence),
        Ok(compact) => {
            let count = indices.len();
            let plan = if count == context.tools.len() {
                OptimizationPlan::CompactAll
            } else {
                OptimizationPlan::SelectAndCompact
            };
            OptimizationOutcome {
                compact: Some(compact),
                selected_indices: indices,
                report: OptimizationReport {
                    input_tool_count: context.tools.len(),
                    selected_tool_count: count,
                    schemas_compacted: count,
                    schemas_bypassed: 0,
                    plan,
                    scope_confidence: confidence,
                    bypass_reasons: vec![],
                },
            }
        }
    }
}
