use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool definition understood by the compact-tools format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

/// A tool call decoded from compact model output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_definition_preserves_schema() {
        let tool = ToolDef {
            name: "get_weather".to_string(),
            description: Some("Get the current weather".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string"
                    }
                },
                "required": ["city"]
            })),
        };

        assert_eq!(tool.name, "get_weather");
        assert!(tool.description.is_some());
        assert!(tool.parameters.is_some());
    }

    #[test]
    fn tool_call_preserves_arguments() {
        let call = ToolCall {
            name: "get_weather".to_string(),
            arguments: json!({
                "city": "Hyderabad"
            }),
        };

        assert_eq!(call.name, "get_weather");
        assert_eq!(call.arguments["city"], "Hyderabad");
    }
}
