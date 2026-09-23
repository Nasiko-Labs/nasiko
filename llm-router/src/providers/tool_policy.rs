//! Shared parsing and validation for request-side tool restrictions.

use serde_json::Value;

use crate::ir::{ChatRequest, ToolDef};

use super::ProviderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AllowedToolsMode {
    Auto,
    Required,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolChoicePolicy {
    Auto,
    Required,
    None,
    Function(String),
    AllowedTools {
        mode: AllowedToolsMode,
        names: Vec<String>,
    },
}

pub(crate) fn tool_choice_policy(
    req: &ChatRequest,
) -> Result<Option<ToolChoicePolicy>, ProviderError> {
    let Some(choice) = &req.tool_choice else {
        return Ok(None);
    };

    let policy = match choice {
        Value::String(value) => match value.as_str() {
            "auto" => ToolChoicePolicy::Auto,
            "required" => ToolChoicePolicy::Required,
            "none" => ToolChoicePolicy::None,
            _ => return Err(invalid("tool_choice must be auto, required, or none")),
        },
        Value::Object(object) => match object.get("type").and_then(Value::as_str) {
            Some("allowed_tools") => {
                let allowed_tools = object
                    .get("allowed_tools")
                    .and_then(Value::as_object)
                    .ok_or_else(|| {
                        invalid("allowed_tools requires a nested allowed_tools object")
                    })?;
                let mode = match allowed_tools.get("mode").and_then(Value::as_str) {
                    Some("auto") => AllowedToolsMode::Auto,
                    Some("required") => AllowedToolsMode::Required,
                    _ => return Err(invalid("allowed_tools mode must be auto or required")),
                };
                let Some(tools) = allowed_tools.get("tools").and_then(Value::as_array) else {
                    return Err(invalid("allowed_tools requires a tools array"));
                };
                let mut names = Vec::with_capacity(tools.len());
                for tool in tools {
                    if tool.get("type").and_then(Value::as_str) != Some("function") {
                        return Err(invalid("allowed_tools only supports function tools"));
                    }
                    let name = tool
                        .get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| invalid("allowed_tools entries require a function name"))?;
                    names.push(name.to_string());
                }
                if mode == AllowedToolsMode::Required && names.is_empty() {
                    return Err(invalid("required allowed_tools must include a function"));
                }
                ToolChoicePolicy::AllowedTools { mode, names }
            }
            Some("function") | Some("tool") => {
                let name = object
                    .get("name")
                    .or_else(|| object.get("function").and_then(|f| f.get("name")))
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| invalid("named tool_choice requires a function name"))?;
                ToolChoicePolicy::Function(name.to_string())
            }
            Some("auto") => ToolChoicePolicy::Auto,
            Some("required") | Some("any") => ToolChoicePolicy::Required,
            Some("none") => ToolChoicePolicy::None,
            _ => return Err(invalid("unsupported tool_choice object")),
        },
        _ => return Err(invalid("tool_choice must be a string or object")),
    };

    validate_choice_names(req.tools.as_deref(), &policy)?;
    Ok(Some(policy))
}

pub(crate) fn validate_provider_tool_policy(
    provider: &str,
    req: &ChatRequest,
) -> Result<Option<ToolChoicePolicy>, ProviderError> {
    let policy = tool_choice_policy(req)?;

    if provider == "gemini" && req.parallel_tool_calls == Some(false) {
        return Err(invalid("Gemini does not support parallel_tool_calls=false"));
    }

    if provider == "gemini"
        && !matches!(policy.as_ref(), Some(ToolChoicePolicy::None))
        && req.tools.as_deref().is_some_and(|tools| {
            filtered_tools(tools, policy.as_ref()).any(|tool| tool.function.strict == Some(true))
        })
    {
        return Err(invalid(
            "Gemini strict tool calls require thoughtSignature round-tripping, which this router does not preserve",
        ));
    }

    if provider == "openrouter" {
        if matches!(policy.as_ref(), Some(ToolChoicePolicy::AllowedTools { .. })) {
            return Err(invalid(
                "OpenRouter does not guarantee allowed_tools across routed models",
            ));
        }
        if req
            .tools
            .as_deref()
            .is_some_and(|tools| tools.iter().any(|tool| tool.function.strict == Some(true)))
        {
            return Err(invalid(
                "OpenRouter does not guarantee strict tool validation across routed models",
            ));
        }
    }

    for tool in req.tools.iter().flatten() {
        if tool.extra.contains_key("allowed_callers") && provider != "anthropic" {
            return Err(invalid(format!(
                "{provider} cannot enforce Anthropic allowed_callers restrictions"
            )));
        }
    }

    Ok(policy)
}

pub(crate) fn filtered_tools<'a>(
    tools: &'a [ToolDef],
    policy: Option<&ToolChoicePolicy>,
) -> impl Iterator<Item = &'a ToolDef> {
    let names = match policy {
        Some(ToolChoicePolicy::AllowedTools { names, .. }) => Some(names),
        _ => None,
    };

    tools.iter().filter(move |tool| {
        names.is_none_or(|names| names.iter().any(|name| name == &tool.function.name))
    })
}

pub(crate) fn invalid(message: impl Into<String>) -> ProviderError {
    ProviderError::InvalidRequest(message.into())
}

fn validate_choice_names(
    tools: Option<&[ToolDef]>,
    policy: &ToolChoicePolicy,
) -> Result<(), ProviderError> {
    let names: Vec<&str> = match policy {
        ToolChoicePolicy::Function(name) => vec![name],
        ToolChoicePolicy::AllowedTools { names, .. } => names.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    };
    if names.is_empty() {
        return Ok(());
    }

    let Some(tools) = tools else {
        return Err(invalid(
            "tool_choice references tools but no tools were declared",
        ));
    };
    for name in names {
        if !tools.iter().any(|tool| tool.function.name == name) {
            return Err(invalid(format!(
                "tool_choice references undeclared function '{name}'"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(body: Value) -> ChatRequest {
        serde_json::from_value(body).unwrap()
    }

    #[test]
    fn parses_chat_completions_allowed_tools_shape() {
        let req = request(json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [
                { "type": "function", "function": { "name": "safe" } },
                { "type": "function", "function": { "name": "admin" } }
            ],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "required",
                    "tools": [{ "type": "function", "function": { "name": "safe" } }]
                }
            }
        }));

        assert_eq!(
            tool_choice_policy(&req).unwrap(),
            Some(ToolChoicePolicy::AllowedTools {
                mode: AllowedToolsMode::Required,
                names: vec!["safe".to_string()]
            })
        );
    }

    #[test]
    fn rejects_flat_or_unenforceable_allowed_tools_requests() {
        let flat = request(json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "tool_choice": {
                "type": "allowed_tools",
                "mode": "auto",
                "tools": [{ "type": "function", "name": "safe" }]
            }
        }));
        assert!(tool_choice_policy(&flat).is_err());

        let allowed_tools = request(json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [{ "type": "function", "function": { "name": "safe" } }],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": {
                    "mode": "auto",
                    "tools": [{ "type": "function", "function": { "name": "safe" } }]
                }
            }
        }));
        assert!(validate_provider_tool_policy("openrouter", &allowed_tools).is_err());
    }

    #[test]
    fn rejects_strict_tools_when_provider_cannot_preserve_the_contract() {
        let strict = request(json!({
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [{ "type": "function", "function": { "name": "safe", "strict": true } }]
        }));

        assert!(validate_provider_tool_policy("gemini", &strict).is_err());
        assert!(validate_provider_tool_policy("openrouter", &strict).is_err());
        assert!(validate_provider_tool_policy("openai", &strict).is_ok());
    }
}
