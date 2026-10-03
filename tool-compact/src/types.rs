use thiserror::Error;

/// A function tool as the client sent it. `parameters` is a JSON Schema.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<serde_json::Value>,
}

/// One decoded call. `arguments` is a JSON object string, OpenAI's shape.
/// The router assigns the call id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

/// Compact signatures plus the call-format instruction.
///
/// `tools` stays inside the crate so a caller cannot swap the schema the
/// decoder validates against without going through `decode_tools`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactTools {
    pub text: String,
    pub(crate) tools: Vec<ToolDef>,
}

impl CompactTools {
    pub fn tools(&self) -> &[ToolDef] {
        &self.tools
    }
}

/// Why encoding or decoding refused the input. Callers match the variant.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompactError {
    #[error("unknown tool '{name}'")]
    UnknownTool { name: String },

    #[error("invalid arguments for '{name}': {reason}")]
    InvalidArguments { name: String, reason: ArgumentFault },

    #[error("unsupported schema for '{name}': {feature}")]
    UnsupportedSchema { name: String, feature: String },
}

/// What was wrong with a call's arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentFault {
    MissingField(String),
    WrongType { field: String },
    BadEnum { field: String },
    Malformed,
}

impl std::fmt::Display for ArgumentFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgumentFault::MissingField(field) => write!(f, "missing field '{field}'"),
            ArgumentFault::WrongType { field } => write!(f, "wrong type for '{field}'"),
            ArgumentFault::BadEnum { field } => write!(f, "value not in enum '{field}'"),
            ArgumentFault::Malformed => write!(f, "malformed call"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{calendar, json};

    #[test]
    fn calendar_tool_keeps_name_description_and_required_fields() {
        let tool = calendar();
        assert_eq!(tool.name, "create_calendar_event");
        assert_eq!(
            tool.description.as_deref(),
            Some("Create an event in the user's calendar.")
        );
        let required = tool.parameters.as_ref().unwrap()["required"]
            .as_array()
            .unwrap();
        assert_eq!(required, &vec![json!("title"), json!("start")]);
    }

    #[test]
    fn error_variants_match_without_reading_the_message() {
        let unknown = CompactError::UnknownTool {
            name: "weather".into(),
        };
        assert!(matches!(unknown, CompactError::UnknownTool { name } if name == "weather"));

        let missing = CompactError::InvalidArguments {
            name: "create_calendar_event".into(),
            reason: ArgumentFault::MissingField("title".into()),
        };
        assert!(matches!(
            missing,
            CompactError::InvalidArguments { reason: ArgumentFault::MissingField(field), .. } if field == "title"
        ));

        let unsupported = CompactError::UnsupportedSchema {
            name: "t".into(),
            feature: "$ref".into(),
        };
        assert!(
            matches!(unsupported, CompactError::UnsupportedSchema { feature, .. } if feature == "$ref")
        );
    }
}
