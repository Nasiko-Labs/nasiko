use crate::schema::{Schema, SchemaType, ToolDef};

/// Encode tool definitions into the compact P1 representation.
///
/// Example:
///
/// create_calendar_event(title:str, start:datetime, duration_min?:int,
/// attendees?:[str], visibility?:public|private) - Create an event...
pub fn encode_tools(tools: &[ToolDef]) -> String {
    tools
        .iter()
        .map(encode_tool)
        .collect::<Vec<_>>()
        .join("\n")
}

fn encode_tool(tool: &ToolDef) -> String {
    let Schema {
        properties,
        required,
        ..
    } = &tool.parameters;

    let args = properties
        .iter()
        .map(|(name, schema)| {
            let optional = if required.contains(name) { "" } else { "?" };

            format!(
                "{}{}:{}",
                name,
                optional,
                encode_schema_type(schema)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    let mut output = format!("{}({})", tool.name, args);

    if let Some(description) = &tool.description {
        if !description.is_empty() {
            output.push_str(" - ");
            output.push_str(description);
        }
    }

    output
}

fn encode_schema_type(schema: &Schema) -> String {
    match &schema.schema_type {
SchemaType::String => {
    if !schema.enum_values.is_empty() {
        schema.enum_values.join("|")
    } else if schema.format.as_deref() == Some("date-time") {
        "datetime".into()
    } else {
        "str".into()
    }
}

        SchemaType::Integer => "int".into(),

        SchemaType::Number => "number".into(),

        SchemaType::Boolean => "bool".into(),

        SchemaType::Object => {
            let fields = schema
                .properties
                .iter()
                .map(|(name, child)| {
                    let optional = if schema.required.contains(name) {
                        ""
                    } else {
                        "?"
                    };

                    format!(
                        "{}{}:{}",
                        name,
                        optional,
                        encode_schema_type(child)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");

            format!("{{{}}}", fields)
        }

        SchemaType::Array => {
            match &schema.items {
                Some(items) => {
                    format!("[{}]", encode_schema_type(items))
                }
                None => "[unknown]".into(),
            }
        }

        SchemaType::Unknown(value) => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_schema, ToolDef};
    use serde_json::json;

    fn tool(schema: serde_json::Value) -> ToolDef {
        ToolDef {
            name: "create_calendar_event".into(),
            description: Some("Create an event".into()),
            parameters: parse_schema(&schema).unwrap(),
        }
    }

    #[test]
    fn encodes_required_and_optional_arguments() {
        let tool = tool(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "start": {"type": "string"},
                "duration_min": {"type": "integer"},
                "attendees": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            },
            "required": ["title", "start"]
        }));

        let result = encode_tools(&[tool]);

        assert_eq!(
            result,
            "create_calendar_event(title:str, start:str, duration_min?:int, attendees?:[str]) - Create an event"
        );
    }

    #[test]
    fn encodes_string_enum() {
        let tool = tool(json!({
            "type": "object",
            "properties": {
                "visibility": {
                    "type": "string",
                    "enum": ["public", "private"]
                }
            },
            "required": ["visibility"]
        }));

        assert_eq!(
            encode_tools(&[tool]),
            "create_calendar_event(visibility:public|private) - Create an event"
        );
    }

    #[test]
    fn encodes_nested_objects() {
        let tool = tool(json!({
            "type": "object",
            "properties": {
                "recipient": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "email": {"type": "string"}
                    },
                    "required": ["email"]
                }
            },
            "required": ["recipient"]
        }));

        assert_eq!(
            encode_tools(&[tool]),
            "create_calendar_event(recipient:{name?:str, email:str}) - Create an event"
        );
    }

    #[test]
    fn encodes_multiple_tools() {
        let first = tool(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"}
            },
            "required": ["title"]
        }));

        let second = ToolDef {
            name: "send_email".into(),
            description: Some("Send an email".into()),
            parameters: parse_schema(&json!({
                "type": "object",
                "properties": {
                    "to": {
                        "type": "array",
                        "items": {"type": "string"}
                    }
                },
                "required": ["to"]
            }))
            .unwrap(),
        };

        let result = encode_tools(&[first, second]);

        assert_eq!(
            result,
            "create_calendar_event(title:str) - Create an event\nsend_email(to:[str]) - Send an email"
        );
    }

    #[test]
    fn encoding_is_deterministic() {
        let tool = tool(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "count": {"type": "integer"}
            },
            "required": ["title"]
        }));

        let first = encode_tools(std::slice::from_ref(&tool));
        let second = encode_tools(std::slice::from_ref(&tool));

        assert_eq!(first, second);
    }
}