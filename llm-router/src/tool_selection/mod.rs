//! Optional catalog selection before the pure Phase 1 tool encoder.
//!
//! Scores affect visibility only. Native definitions remain authoritative, and
//! neither selectors nor policy construct arguments or execute tools.

mod jev;
mod policy;
#[cfg(test)]
mod tests;

pub use jev::JevSelector;
pub use policy::{SelectedTools, apply_policy, dependency_closure};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use nasiko_tool_compact::{ToolDef as CompactDef, encode_tools};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ir::{ChatRequest, ToolDef};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionMode {
    #[default]
    Off,
    Deterministic,
    Jev,
    Hybrid,
}

impl std::str::FromStr for SelectionMode {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" => Ok(Self::Off),
            "deterministic" => Ok(Self::Deterministic),
            "jev" => Ok(Self::Jev),
            "hybrid" => Ok(Self::Hybrid),
            _ => Err("expected off, deterministic, jev or hybrid"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FallbackMode {
    Native,
    #[default]
    CompactAll,
    DeterministicSubset,
}

impl std::str::FromStr for FallbackMode {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "native" => Ok(Self::Native),
            "compact-all" => Ok(Self::CompactAll),
            "deterministic-subset" => Ok(Self::DeterministicSubset),
            _ => Err("expected native, compact-all or deterministic-subset"),
        }
    }
}

/// Credential with deliberately redacted Debug output, including GatewayConfig.
#[derive(Clone, Default)]
pub struct SelectionApiKey(String);

impl SelectionApiKey {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub(crate) fn value(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SelectionApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

pub type ToolDependencyGraph = BTreeMap<String, Vec<String>>;
pub const SELECTION_PROMPT_VERSION: &str = "nasiko-noul-v1";

#[derive(Debug, Clone)]
pub struct SelectionConfig {
    /// Invalid environment configuration cannot be revived by evaluator mode overrides.
    pub configuration_error: Option<SelectionError>,
    pub mode: SelectionMode,
    pub fallback: FallbackMode,
    pub include_threshold: f64,
    /// Conservatively retain scores between this floor and the inclusion threshold.
    pub uncertainty_floor: Option<f64>,
    pub max_tools: Option<usize>,
    /// Budget uses a documented bytes/4 estimate, not provider billing tokens.
    pub max_tool_tokens: Option<u32>,
    pub min_selected_tools: usize,
    /// Optional economic gate: skip Jev for catalogs below this local cost estimate.
    /// Explicit budgets take precedence, so a skipped decision never evades a cap.
    pub min_jev_catalog_tokens: u32,
    pub timeout_ms: u64,
    pub jev_model: String,
    pub jev_base_url: String,
    pub api_key: SelectionApiKey,
    pub mandatory_tools: Vec<String>,
    pub dependencies: ToolDependencyGraph,
}

impl Default for SelectionConfig {
    fn default() -> Self {
        Self {
            configuration_error: None,
            mode: SelectionMode::Off,
            fallback: FallbackMode::CompactAll,
            include_threshold: 0.5,
            uncertainty_floor: Some(0.35),
            max_tools: None,
            max_tool_tokens: None,
            min_selected_tools: 1,
            min_jev_catalog_tokens: 0,
            timeout_ms: 1500,
            jev_model: "jev-latest".into(),
            jev_base_url: "https://api.typesafe.ai".into(),
            api_key: SelectionApiKey::default(),
            mandatory_tools: Vec::new(),
            dependencies: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCandidate {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub input_summary: String,
    pub compactable: bool,
    pub estimated_tokens: u32,
    pub mandatory: bool,
}

#[derive(Debug, Clone)]
pub struct SelectionContext {
    pub request_text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SelectorUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Default)]
pub struct SelectionResult {
    pub probabilities: BTreeMap<String, f64>,
    pub resolved_model: Option<String>,
    pub usage: Option<SelectorUsage>,
}

/// Errors contain categories only: upstream bodies, URLs and secrets are excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum SelectionError {
    #[error("missing selector configuration")]
    Configuration,
    #[error("selector timeout")]
    Timeout,
    #[error("selector authentication failed")]
    Auth,
    #[error("selector rate limited")]
    RateLimit,
    #[error("selector service unavailable")]
    Service,
    #[error("selector transport failed")]
    Transport,
    #[error("malformed selector response")]
    Malformed,
    #[error("unknown selector candidate ID")]
    UnknownId,
    #[error("incomplete selector answers")]
    Incomplete,
    #[error("invalid probability")]
    Probability,
    #[error("invalid selection registry or policy")]
    InvalidPolicy,
    #[error("selection produced too few tools")]
    Empty,
    #[error("selector input exceeds bounded metadata limits")]
    InputLimit,
}

#[async_trait]
pub trait ToolSelector: Send + Sync {
    async fn select(
        &self,
        context: &SelectionContext,
        candidates: &[ToolCandidate],
    ) -> Result<SelectionResult, SelectionError>;
}

pub fn compact_definition(tool: &ToolDef) -> CompactDef {
    CompactDef {
        name: tool.function.name.clone(),
        description: tool.function.description.clone(),
        parameters: tool.function.parameters.clone(),
    }
}

pub fn token_estimate(text: &str) -> u32 {
    u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX)
}

fn bounded(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

fn normalized_description(text: &str) -> String {
    let mut normalized = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
        if !normalized.is_empty() {
            if count == 384 {
                return normalized;
            }
            normalized.push(' ');
            count += 1;
        }
        for ch in word.chars() {
            if count == 384 {
                return normalized;
            }
            normalized.push(ch);
            count += 1;
        }
    }
    normalized
}

/// Stable ordinal IDs, unique native names, bounded metadata, no raw schemas.
pub fn build_candidates(tools: &[ToolDef]) -> Result<Vec<ToolCandidate>, SelectionError> {
    let mut names = BTreeSet::new();
    tools
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            if tool.function.name.is_empty() || !names.insert(&tool.function.name) {
                return Err(SelectionError::InvalidPolicy);
            }
            let compact = if tool.kind == "function" && tool.extra.is_empty() {
                encode_tools(&[compact_definition(tool)]).ok()
            } else {
                None
            };
            let schema = tool.function.parameters.as_ref();
            let properties = schema
                .and_then(|s| s.get("properties"))
                .and_then(Value::as_object);
            let required = schema
                .and_then(|s| s.get("required"))
                .and_then(Value::as_array);
            let summary = properties
                .map(|properties| {
                    properties
                        .iter()
                        .take(16)
                        .map(|(name, value)| {
                            let marker = if required
                                .is_some_and(|r| r.iter().any(|v| v.as_str() == Some(name)))
                            {
                                "!"
                            } else {
                                "?"
                            };
                            let kind = value
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or("complex");
                            format!("{}{marker}:{}", bounded(name, 48), bounded(kind, 16))
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let native = serde_json::to_string(tool).map_err(|_| SelectionError::InvalidPolicy)?;
            Ok(ToolCandidate {
                id: format!("tool_{index:04}"),
                name: tool.function.name.clone(),
                description: tool
                    .function
                    .description
                    .as_ref()
                    .map(|d| normalized_description(d)),
                input_summary: bounded(&summary, 512),
                compactable: compact.is_some(),
                estimated_tokens: token_estimate(compact.as_ref().map_or(&native, |c| &c.text)),
                mandatory: false,
            })
        })
        .collect()
}

pub fn selection_context(req: &ChatRequest) -> SelectionContext {
    // Only the latest user text is sent. Tool outputs and system messages are private.
    let text = crate::routing::latest_user_query(&req.messages).unwrap_or_default();
    SelectionContext {
        request_text: bounded(&text, 4096),
    }
}

fn terms(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.len() >= 3)
        .map(str::to_lowercase)
        .filter(|word| {
            !matches!(
                word.as_str(),
                "the"
                    | "and"
                    | "for"
                    | "with"
                    | "from"
                    | "this"
                    | "that"
                    | "user"
                    | "tool"
                    | "tools"
                    | "please"
                    | "can"
                    | "create"
                    | "get"
                    | "set"
                    | "add"
                    | "send"
            )
        })
        .collect()
}

/// Optional lexical baseline. It has no ground-truth input and makes no recall promise.
pub fn deterministic_result(
    context: &SelectionContext,
    candidates: &[ToolCandidate],
) -> SelectionResult {
    let request = terms(&context.request_text);
    SelectionResult {
        probabilities: candidates
            .iter()
            .map(|c| {
                let metadata = terms(&format!(
                    "{} {}",
                    c.name,
                    c.description.as_deref().unwrap_or("")
                ));
                let relevant = c.mandatory || !request.is_disjoint(&metadata);
                (c.id.clone(), if relevant { 1.0 } else { 0.0 })
            })
            .collect(),
        ..Default::default()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectionTelemetry {
    pub mode: SelectionMode,
    pub prompt_version: &'static str,
    pub selector_invoked: bool,
    pub selector_skip_reason: Option<&'static str>,
    pub original_tools: usize,
    pub selected_tools: usize,
    pub mandatory_tools: usize,
    pub dependency_tools: usize,
    pub jev_selected_tools: usize,
    pub uncertain_tools: usize,
    pub bypassed_tools: usize,
    pub budget_overflow: bool,
    pub fallback: Option<SelectionError>,
    pub selection_latency_ms: u64,
    pub selection_usage: Option<SelectorUsage>,
    pub resolved_model: Option<String>,
    pub native_tool_token_estimate: u32,
    pub selected_tool_token_estimate: u32,
    pub compact_tool_token_estimate: u32,
}

#[derive(Debug, Clone)]
pub struct SelectionOutcome {
    pub indices: Vec<usize>,
    /// Optional evaluator/replay diagnostics; never included in default tracing.
    pub probabilities: Option<BTreeMap<String, f64>>,
    /// A native fallback skips Phase 1 compaction as well as selection.
    pub native_fallback: bool,
    pub telemetry: SelectionTelemetry,
}

fn mark_mandatory(
    req: &ChatRequest,
    cfg: &SelectionConfig,
    candidates: &mut [ToolCandidate],
) -> Result<(), SelectionError> {
    let mut mandatory = cfg.mandatory_tools.iter().cloned().collect::<BTreeSet<_>>();
    if let Some(name) = req
        .tool_choice
        .as_ref()
        .and_then(|c| c.pointer("/function/name"))
        .and_then(Value::as_str)
    {
        mandatory.insert(name.to_string());
    }
    if mandatory
        .iter()
        .any(|name| !candidates.iter().any(|c| &c.name == name))
    {
        return Err(SelectionError::InvalidPolicy);
    }
    for c in candidates {
        c.mandatory = mandatory.contains(&c.name);
    }
    Ok(())
}

/// Shared by the router and Phase 2 evaluator; override injects a fake selector for tests.
pub async fn select_request(
    req: &ChatRequest,
    cfg: &SelectionConfig,
    http: &reqwest::Client,
    selector_override: Option<&dyn ToolSelector>,
) -> Option<SelectionOutcome> {
    if cfg.mode == SelectionMode::Off
        || req.tools.as_ref().is_none_or(Vec::is_empty)
        || req.tool_choice.as_ref().is_some_and(|c| c == "none")
        || req
            .messages
            .iter()
            .any(|m| m.role == "tool" || m.tool_calls.is_some())
    {
        return None;
    }
    let tools = req.tools.as_ref()?;
    let started = Instant::now();
    let mut candidates = build_candidates(tools).unwrap_or_default();
    let context = selection_context(req);
    let prepared = if let Some(error) = cfg.configuration_error {
        Err(error)
    } else if candidates.len() != tools.len() {
        Err(SelectionError::InvalidPolicy)
    } else if crate::routing::latest_user_query(&req.messages)
        .is_none_or(|text| text.chars().count() > 4096)
    {
        // Preserve tools when the bounded selector cannot see the complete request.
        Err(SelectionError::InputLimit)
    } else {
        mark_mandatory(req, cfg, &mut candidates)
            .and_then(|()| policy::validate_config(&candidates, cfg))
    };
    let mut jev_ids = BTreeSet::new();
    let mut selector_invoked = false;
    let mut selector_skip_reason = None;
    let result = match prepared {
        Err(error) => Err(error),
        Ok(()) if cfg.mode == SelectionMode::Deterministic => {
            Ok(deterministic_result(&context, &candidates))
        }
        Ok(()) => {
            // Hybrid only treats an explicit native-name mention as conclusive. Other
            // candidates, including negative lexical matches, still go to Jev.
            let text = context.request_text.to_lowercase();
            let optional = candidates
                .iter()
                .filter(|c| {
                    !c.mandatory
                        && !(cfg.mode == SelectionMode::Hybrid
                            && text
                                .split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-'))
                                .any(|s| s == c.name.to_lowercase()))
                })
                .cloned()
                .collect::<Vec<_>>();
            // With one tool and the default non-empty safeguard, preselection
            // cannot save any tokens. Avoid paying for a pointless external call.
            let single_catalog = candidates.len() == 1 && cfg.min_selected_tools >= 1;
            let below_cost_floor = cfg.max_tools.is_none()
                && cfg.max_tool_tokens.is_none()
                && candidates
                    .iter()
                    .fold(0u64, |sum, c| sum + u64::from(c.estimated_tokens))
                    < u64::from(cfg.min_jev_catalog_tokens);
            let result = if optional.is_empty() || single_catalog || below_cost_floor {
                selector_skip_reason = Some(if optional.is_empty() {
                    "no_optional_candidates"
                } else if single_catalog {
                    "single_catalog"
                } else {
                    "below_cost_floor"
                });
                Ok(SelectionResult {
                    probabilities: optional.iter().map(|c| (c.id.clone(), 1.0)).collect(),
                    usage: Some(SelectorUsage::default()),
                    ..Default::default()
                })
            } else {
                selector_invoked = true;
                let adapter = JevSelector::new(http.clone(), cfg);
                let selector = selector_override.unwrap_or(&adapter);
                match tokio::time::timeout(
                    Duration::from_millis(cfg.timeout_ms),
                    selector.select(&context, &optional),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(SelectionError::Timeout),
                }
            };
            result.and_then(|mut result| {
                policy::validate_result(&optional, &result)?;
                jev_ids = optional
                    .iter()
                    .filter(|c| {
                        selector_invoked && result.probabilities[&c.id] >= cfg.include_threshold
                    })
                    .map(|c| c.id.clone())
                    .collect();
                for candidate in &candidates {
                    result
                        .probabilities
                        .entry(candidate.id.clone())
                        .or_insert(1.0);
                }
                Ok(result)
            })
        }
    };
    let mut fallback = None;
    let mut resolved_model = None;
    let mut usage = None;
    let mut probabilities = None;
    let applied = result.and_then(|result| {
        // Keep actual selector usage even if local policy subsequently rejects it.
        resolved_model = result.resolved_model.clone();
        usage = result.usage.clone();
        // Only expose actual scores from an invoked selector or lexical baseline.
        if selector_invoked || cfg.mode == SelectionMode::Deterministic {
            probabilities = Some(result.probabilities.clone());
        }
        apply_policy(&candidates, &result, cfg)
    });
    let (mut selected, native_fallback) = match applied {
        Ok(selected) => (selected, false),
        Err(error) => {
            fallback = Some(error);
            jev_ids.clear();
            let subset = (cfg.fallback == FallbackMode::DeterministicSubset
                && error != SelectionError::InvalidPolicy)
                .then(|| {
                    apply_policy(
                        &candidates,
                        &deterministic_result(&context, &candidates),
                        cfg,
                    )
                })
                .and_then(Result::ok)
                .filter(|selected| !selected.indices.is_empty());
            let selected = subset.unwrap_or_else(|| SelectedTools {
                indices: (0..tools.len()).collect(),
                dependency_count: 0,
                uncertain_count: 0,
                budget_overflow: false,
            });
            (selected, cfg.fallback == FallbackMode::Native)
        }
    };
    let selected_estimate = selected.indices.iter().fold(0u64, |sum, &i| {
        sum + u64::from(candidates.get(i).map_or_else(
            || token_estimate(&serde_json::to_string(&tools[i]).unwrap_or_default()),
            |c| c.estimated_tokens,
        ))
    });
    selected.budget_overflow |= cfg
        .max_tools
        .is_some_and(|cap| selected.indices.len() > cap)
        || cfg
            .max_tool_tokens
            .is_some_and(|cap| selected_estimate > u64::from(cap));
    let sum = |values: Vec<u32>| values.into_iter().fold(0u32, u32::saturating_add);
    let telemetry = SelectionTelemetry {
        mode: cfg.mode,
        prompt_version: SELECTION_PROMPT_VERSION,
        selector_invoked,
        selector_skip_reason,
        original_tools: tools.len(),
        selected_tools: selected.indices.len(),
        mandatory_tools: candidates.iter().filter(|c| c.mandatory).count(),
        dependency_tools: selected.dependency_count,
        jev_selected_tools: jev_ids.len(),
        uncertain_tools: selected.uncertain_count,
        bypassed_tools: selected
            .indices
            .iter()
            .filter(|&&i| candidates.get(i).is_none_or(|c| !c.compactable))
            .count(),
        budget_overflow: selected.budget_overflow,
        fallback,
        selection_latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        selection_usage: usage,
        resolved_model,
        native_tool_token_estimate: token_estimate(
            &serde_json::to_string(tools).unwrap_or_default(),
        ),
        selected_tool_token_estimate: token_estimate(
            &serde_json::to_string(
                &selected
                    .indices
                    .iter()
                    .map(|&i| &tools[i])
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_default(),
        ),
        compact_tool_token_estimate: sum(selected
            .indices
            .iter()
            .filter_map(|&i| candidates.get(i).map(|c| c.estimated_tokens))
            .collect()),
    };
    // Structured measurements only; no request, catalog, key or response body.
    tracing::info!(target: "nasiko::llm_router::tool_selection", mode = ?telemetry.mode,
        selector_invoked = telemetry.selector_invoked,
        selector_skip_reason = telemetry.selector_skip_reason,
        original_tools = telemetry.original_tools, selected_tools = telemetry.selected_tools,
        mandatory_tools = telemetry.mandatory_tools, dependency_tools = telemetry.dependency_tools,
        jev_selected_tools = telemetry.jev_selected_tools, uncertain_tools = telemetry.uncertain_tools,
        bypassed_tools = telemetry.bypassed_tools, budget_overflow = telemetry.budget_overflow,
        fallback = ?telemetry.fallback, tool_selection_fallback_total = usize::from(telemetry.fallback.is_some()),
        latency_ms = telemetry.selection_latency_ms, usage = ?telemetry.selection_usage,
        resolved_model = ?telemetry.resolved_model,
        native_tokens_estimate = telemetry.native_tool_token_estimate,
        selected_tokens_estimate = telemetry.selected_tool_token_estimate,
        compact_tokens_estimate = telemetry.compact_tool_token_estimate,
        "tool selection completed");
    Some(SelectionOutcome {
        indices: selected.indices,
        probabilities,
        native_fallback,
        telemetry,
    })
}
