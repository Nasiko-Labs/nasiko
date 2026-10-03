//! Domain error types for tool schema compaction and call decoding.

use thiserror::Error;

/// Errors that can occur during tool encoding, call decoding, or schema validation.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum ToolCompactError {
    /// A tool call named a function not present in the toolset.
    #[error("unknown tool: {0}")]
    UnknownTool(String),

    /// A required argument defined in the tool's JSON Schema was omitted.
    #[error("missing required field '{field}' for tool '{tool}'")]
    MissingRequiredField { tool: String, field: String },

    /// An argument value did not match any of the allowed enum variants.
    #[error("invalid enum value '{value}' for field '{field}' in tool '{tool}'; allowed: {allowed:?}")]
    InvalidEnumValue {
        tool: String,
        field: String,
        value: String,
        allowed: Vec<String>,
    },

    /// Arguments failed validation against the tool's JSON Schema.
    #[error("invalid arguments for tool '{tool}': {details}")]
    InvalidArguments { tool: String, details: String },

    /// The model output contained invalid or unparseable compact call syntax.
    #[error("malformed compact call syntax: {0}")]
    MalformedSyntax(String),

    /// A tool definition contained an unsupported or invalid JSON Schema.
    #[error("schema error: {0}")]
    SchemaError(String),
}

impl ToolCompactError {
    /// Normalized short error code matching the evaluation harness contract:
    /// - `"unknown_tool"`
    /// - `"invalid_arguments"`
    /// - `"malformed_syntax"`
    /// - `"schema_error"`
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::UnknownTool(_) => "unknown_tool",
            Self::MissingRequiredField { .. }
            | Self::InvalidEnumValue { .. }
            | Self::InvalidArguments { .. }
            | Self::MalformedSyntax(_) => "invalid_arguments",
            Self::SchemaError(_) => "schema_error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_mapping() {
        let err1 = ToolCompactError::UnknownTool("fake_tool".to_string());
        assert_eq!(err1.as_code(), "unknown_tool");

        let err2 = ToolCompactError::MissingRequiredField {
            tool: "calc".into(),
            field: "x".into(),
        };
        assert_eq!(err2.as_code(), "invalid_arguments");

        let err3 = ToolCompactError::InvalidEnumValue {
            tool: "format".into(),
            field: "mode".into(),
            value: "yaml".into(),
            allowed: vec!["json".into(), "csv".into()],
        };
        assert_eq!(err3.as_code(), "invalid_arguments");

        let err4 = ToolCompactError::InvalidArguments {
            tool: "calc".into(),
            details: "expected integer".into(),
        };
        assert_eq!(err4.as_code(), "invalid_arguments");

        let err5 = ToolCompactError::MalformedSyntax("unclosed brace".into());
        assert_eq!(err5.as_code(), "invalid_arguments");

        let err6 = ToolCompactError::SchemaError("missing type".into());
        assert_eq!(err6.as_code(), "schema_error");
    }

    #[test]
    fn test_error_display() {
        let err = ToolCompactError::MissingRequiredField {
            tool: "search".into(),
            field: "query".into(),
        };
        assert_eq!(
            err.to_string(),
            "missing required field 'query' for tool 'search'"
        );
    }
}
