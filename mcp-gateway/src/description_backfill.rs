//! LLM fallback for MCP connector/tool descriptions the native source didn't
//! provide (Composio toolkit metadata, a generic server's `initialize`
//! response, an uploaded server's own `tools/list`).
//!
//! Strict rules, per product decision:
//! - Only called when something is actually missing. A connector with both a
//!   server description and every tool description already present never
//!   reaches the LLM at all.
//! - Only fills gaps — never regenerates or overwrites a description that
//!   already came from the native source.
//! - One call per connector, not one per tool, and the request/response shape
//!   is built dynamically to contain **only** what's missing: if the server
//!   description is already known, the schema has no `server_description`
//!   field at all; if only 2 of 40 tools lack a description, the schema's
//!   `tools` object is `required`/`additionalProperties: false` for exactly
//!   those 2 names — the model is structurally unable to spend tokens
//!   re-describing tools we already have.
//! - Best-effort: a failed or timed-out call degrades to an empty
//!   [`Backfill`] (fields stay `NULL`) rather than failing the connector
//!   registration/build it's part of.

use std::collections::HashMap;
use std::time::Duration;

use nasiko_orchestrator::models::{ChatCompletionRequest, ChatMessage, JsonSchema, ResponseFormat};
use nasiko_orchestrator::providers::LLMProvider;
use serde_json::{Value, json};

const CALL_TIMEOUT: Duration = Duration::from_secs(15);

const SYSTEM_PROMPT: &str = "You are an expert at inferring MCP (Model Context Protocol) server \
and tool purposes from their names and input parameter schemas. Be concise and factual — infer \
only what the name/schema implies, never invent capabilities they don't suggest.";

/// Treat a blank/whitespace-only description the same as an absent one.
/// Confirmed live: FastMCP (the Python MCP SDK) returns `description: ""` for
/// a tool with no docstring rather than omitting the field entirely, so a
/// plain `.is_none()` check alone lets these silently skip the fallback.
pub fn is_missing(desc: &Option<String>) -> bool {
    desc.as_deref().map(str::trim).unwrap_or("").is_empty()
}

/// A tool that's missing a description, plus whatever input-schema signal is
/// available for it (may be `None` if the backend didn't provide one either).
pub struct ToolNeedingDescription {
    pub name: String,
    pub input_schema: Option<Value>,
}

/// What the fallback produced. Either field may be empty/absent if it wasn't
/// requested, or if the call failed/timed out.
#[derive(Debug, Clone, Default)]
pub struct Backfill {
    pub server_description: Option<String>,
    pub tool_descriptions: HashMap<String, String>,
}

/// Best-effort LLM backfill — see module doc for the exact contract.
///
/// `known_tool_names` are tools that already have a description; they're sent
/// as bare names only (no schema, no description text) purely so the model
/// understands the server's overall purpose when filling `server_description`.
/// They are never included in the output schema and can never receive a
/// (re-)generated description from this call.
pub async fn backfill(
    llm: &LLMProvider,
    model: &str,
    connector_name: &str,
    provider_type: &str,
    known_tool_names: &[String],
    need_server_description: bool,
    missing_tools: &[ToolNeedingDescription],
) -> Backfill {
    if !need_server_description && missing_tools.is_empty() {
        return Backfill::default();
    }

    let schema = build_schema(need_server_description, missing_tools);
    let prompt = build_prompt(
        connector_name,
        provider_type,
        known_tool_names,
        need_server_description,
        missing_tools,
    );

    let request = ChatCompletionRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: Some(SYSTEM_PROMPT.to_string()),
            },
            ChatMessage {
                role: "user".to_string(),
                content: Some(prompt),
            },
        ],
        stream: false,
        temperature: Some(0.2),
        max_tokens: Some(2000),
        response_format: Some(ResponseFormat::JsonSchema {
            json_schema: JsonSchema {
                name: "mcp_descriptions".to_string(),
                strict: Some(true),
                schema,
            },
        }),
        stream_options: None,
    };

    let result = match tokio::time::timeout(CALL_TIMEOUT, llm.chat_completion(&request)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!(connector_name, %e, "mcp description backfill: llm call failed");
            return Backfill::default();
        }
        Err(_) => {
            tracing::warn!(connector_name, "mcp description backfill: llm call timed out");
            return Backfill::default();
        }
    };

    parse_result(&result.content, need_server_description, missing_tools)
}

fn build_schema(need_server_description: bool, missing_tools: &[ToolNeedingDescription]) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    if need_server_description {
        properties.insert(
            "server_description".to_string(),
            json!({
                "type": "string",
                "description": "1-2 sentence description of what this MCP server/toolkit provides overall"
            }),
        );
        required.push(Value::String("server_description".to_string()));
    }

    if !missing_tools.is_empty() {
        let mut tool_props = serde_json::Map::new();
        let mut tool_required = Vec::new();
        for t in missing_tools {
            tool_props.insert(
                t.name.clone(),
                json!({
                    "type": "string",
                    "description": "one sentence describing what this specific tool does"
                }),
            );
            tool_required.push(Value::String(t.name.clone()));
        }
        properties.insert(
            "tools".to_string(),
            json!({
                "type": "object",
                "properties": Value::Object(tool_props),
                "required": tool_required,
                "additionalProperties": false
            }),
        );
        required.push(Value::String("tools".to_string()));
    }

    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": required,
        "additionalProperties": false
    })
}

fn build_prompt(
    connector_name: &str,
    provider_type: &str,
    known_tool_names: &[String],
    need_server_description: bool,
    missing_tools: &[ToolNeedingDescription],
) -> String {
    let mut prompt = format!("Connector name: {connector_name}\nProvider type: {provider_type}\n\n");

    if !known_tool_names.is_empty() {
        prompt.push_str(&format!(
            "This server also exposes these other tools (already described elsewhere — \
             context only, do not describe these): {}\n\n",
            known_tool_names.join(", ")
        ));
    }

    if need_server_description {
        prompt.push_str(
            "Produce `server_description`: a 1-2 sentence summary of what this MCP server/\
             toolkit provides overall.\n\n",
        );
    }

    if !missing_tools.is_empty() {
        prompt.push_str(
            "Produce a one-sentence description for each of these tools, using its name and \
             input schema (when present) as signal:\n",
        );
        for t in missing_tools {
            match &t.input_schema {
                Some(schema) => prompt.push_str(&format!("- {}: input schema = {schema}\n", t.name)),
                None => prompt.push_str(&format!("- {}\n", t.name)),
            }
        }
    }

    prompt
}

/// Strip a ` ```json ... ``` ` (or bare ` ``` ... ``` `) fence if the model
/// wrapped its output in one. Defense in depth only — `strict: true` json_schema
/// mode (used above) already guarantees fence-free output on real OpenAI, but
/// some OpenAI-compatible providers (e.g. DeepSeek, confirmed elsewhere in
/// this codebase — see `capabilities/generator.rs`'s `is_response_format_rejection`)
/// don't honor `strict` the same way, so this costs nothing and covers that gap.
fn strip_markdown_fence(content: &str) -> &str {
    let trimmed = content.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    rest.strip_suffix("```").unwrap_or(rest).trim()
}

fn parse_result(
    content: &str,
    need_server_description: bool,
    missing_tools: &[ToolNeedingDescription],
) -> Backfill {
    let value: Value = match serde_json::from_str(strip_markdown_fence(content)) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(%e, "mcp description backfill: failed to parse llm response as json");
            return Backfill::default();
        }
    };

    let server_description = need_server_description
        .then(|| value.get("server_description").and_then(|v| v.as_str()))
        .flatten()
        .map(str::to_string);

    let mut tool_descriptions = HashMap::new();
    if !missing_tools.is_empty()
        && let Some(tools) = value.get("tools").and_then(|v| v.as_object())
    {
        for t in missing_tools {
            if let Some(desc) = tools.get(&t.name).and_then(|v| v.as_str()) {
                tool_descriptions.insert(t.name.clone(), desc.to_string());
            }
        }
    }

    Backfill {
        server_description,
        tool_descriptions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_missing_treats_none_and_blank_strings_as_missing() {
        assert!(is_missing(&None));
        assert!(is_missing(&Some(String::new())));
        assert!(is_missing(&Some("   ".to_string())));
        assert!(!is_missing(&Some("a real description".to_string())));
    }

    #[test]
    fn schema_omits_server_description_when_not_needed() {
        let schema = build_schema(false, &[]);
        assert!(!schema["required"]
            .as_array()
            .unwrap()
            .contains(&Value::String("server_description".to_string())));
        assert!(schema["properties"].get("server_description").is_none());
    }

    #[test]
    fn schema_requires_exactly_the_missing_tool_names() {
        let missing = vec![
            ToolNeedingDescription { name: "tool_a".into(), input_schema: None },
            ToolNeedingDescription { name: "tool_b".into(), input_schema: None },
        ];
        let schema = build_schema(true, &missing);
        let tools_required = schema["properties"]["tools"]["required"].as_array().unwrap();
        assert_eq!(tools_required.len(), 2);
        assert!(tools_required.contains(&Value::String("tool_a".to_string())));
        assert!(tools_required.contains(&Value::String("tool_b".to_string())));
        assert_eq!(
            schema["properties"]["tools"]["additionalProperties"],
            Value::Bool(false)
        );
    }

    #[test]
    fn parse_result_only_applies_returned_descriptions_for_missing_tools() {
        let missing = vec![ToolNeedingDescription { name: "tool_a".into(), input_schema: None }];
        let content = r#"{"server_description":"does stuff","tools":{"tool_a":"describes tool a"}}"#;
        let result = parse_result(content, true, &missing);
        assert_eq!(result.server_description.as_deref(), Some("does stuff"));
        assert_eq!(result.tool_descriptions.get("tool_a").map(String::as_str), Some("describes tool a"));
        assert_eq!(result.tool_descriptions.len(), 1);
    }

    #[test]
    fn parse_result_ignores_extra_tools_the_model_hallucinated() {
        // Even if the model (incorrectly) returns a description for a tool
        // that wasn't in `missing_tools`, we must never apply it — only
        // entries matching a name we actually asked for are kept.
        let missing = vec![ToolNeedingDescription { name: "tool_a".into(), input_schema: None }];
        let content = r#"{"tools":{"tool_a":"desc a","tool_never_asked_for":"desc x"}}"#;
        let result = parse_result(content, false, &missing);
        assert_eq!(result.tool_descriptions.len(), 1);
        assert!(result.tool_descriptions.contains_key("tool_a"));
    }

    #[test]
    fn parse_result_ignores_server_description_when_not_needed() {
        let content = r#"{"server_description":"should be ignored"}"#;
        let result = parse_result(content, false, &[]);
        assert_eq!(result.server_description, None);
    }

    #[test]
    fn parse_result_degrades_gracefully_on_malformed_json() {
        let result = parse_result("not json", true, &[]);
        assert_eq!(result.server_description, None);
        assert!(result.tool_descriptions.is_empty());
    }

    #[test]
    fn parse_result_strips_a_json_markdown_fence() {
        let content = "```json\n{\"server_description\":\"does stuff\"}\n```";
        let result = parse_result(content, true, &[]);
        assert_eq!(result.server_description.as_deref(), Some("does stuff"));
    }

    #[test]
    fn parse_result_strips_a_bare_markdown_fence() {
        let content = "```\n{\"server_description\":\"does stuff\"}\n```";
        let result = parse_result(content, true, &[]);
        assert_eq!(result.server_description.as_deref(), Some("does stuff"));
    }
}
