use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionDef,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Recursive helper to dig into nested objects and arrays
fn format_type(val: &Value) -> String {
    let type_str = val.get("type").and_then(|v| v.as_str()).unwrap_or("any");

    match type_str {
        "object" => {
            if let Some(Value::Object(props)) = val.get("properties") {
                let mut nested = Vec::new();
                for (k, v) in props {
                    nested.push(format!("{}:{}", k, format_type(v)));
                }
                format!("object{{{}}}", nested.join(", "))
            } else {
                "object".to_string()
            }
        }
        "array" => {
            if let Some(items) = val.get("items") {
                format!("array[{}]", format_type(items))
            } else {
                "array".to_string()
            }
        }
        _ => type_str.to_string(),
    }
}

pub fn encode_tools(tools: &[ToolDef]) -> Result<String, String> {
    let mut output = String::new();

    for tool in tools {
        let func = &tool.function;
        let name = &func.name;
        let desc = func.description.as_deref().unwrap_or("");

        let mut params_formatted = Vec::new();

        if let Some(Value::Object(params)) = &func.parameters {
            if let Some(Value::Object(props)) = params.get("properties") {
                for (key, val) in props {
                    // Use the recursive helper instead of just grabbing the top-level type
                    params_formatted.push(format!("{}:{}", key, format_type(val)));
                }
            }
        }

        let params_str = params_formatted.join(", ");
        output.push_str(&format!("{}({}) - {}\n", name, params_str, desc));
    }

    output.push_str("To call a tool, emit: <<call name {json args}>>\n");

    Ok(output)
}

pub fn decode_calls(text: &str, _tools: &[ToolDef]) -> Result<Vec<ToolCall>, String> {
    let mut calls = Vec::new();
    let mut current_idx = 0;

    while let Some(start_idx) = text[current_idx..].find("<<call ") {
        let absolute_start = current_idx + start_idx;

        if let Some(end_idx) = text[absolute_start..].find(">>") {
            let tag_content = &text[absolute_start + 7..absolute_start + end_idx];

            if let Some(space_idx) = tag_content.find(' ') {
                let name = &tag_content[..space_idx];
                let args_str = &tag_content[space_idx + 1..];

                // Refinement: Catch malformed JSON and return a strict error back to the router
                match serde_json::from_str::<Value>(args_str) {
                    Ok(_) => {
                        let call = ToolCall {
                            id: format!("call_{}", calls.len() + 1),
                            kind: "function".to_string(),
                            function: FunctionCall {
                                name: name.to_string(),
                                arguments: args_str.to_string(),
                            },
                            extra: Value::Object(serde_json::Map::new()),
                        };
                        calls.push(call);
                    }
                    Err(e) => {
                        return Err(format!(
                            "LLM returned malformed JSON for tool '{}': {}",
                            name, e
                        ));
                    }
                }
            }
            current_idx = absolute_start + end_idx + 2;
        } else {
            break;
        }
    }

    Ok(calls)
}
