use thiserror::Error;

/// Every error this crate surfaces.
///
/// There is no "best effort" outcome: a tool that cannot be written compactly, or a call that
/// does not check out against its schema, is an error for the caller to act on — bypass
/// compaction for the first, refuse the call for the rest.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompactError {
    /// The tool uses a schema feature the compact form cannot carry without losing meaning.
    /// The caller sends the native definitions instead.
    #[error("tool '{tool}' cannot be compacted: {reason}")]
    Unsupported { tool: String, reason: String },

    #[error("unknown tool '{0}'")]
    UnknownTool(String),

    #[error("invalid arguments for tool '{tool}': {reason}")]
    InvalidArguments { tool: String, reason: String },

    /// The text does not follow the grammar: a call that never closes, a missing name, a
    /// definition line that does not parse.
    #[error("malformed compact text: {0}")]
    Malformed(String),
}

impl CompactError {
    /// Stable string for eval output and telemetry — spelled out rather than derived from the
    /// variant name, so renaming a variant cannot change a reported value.
    ///
    /// The decode vocabulary has two labels. A call whose text cannot be parsed has no valid
    /// arguments, so [`CompactError::Malformed`] reports as `invalid_arguments`.
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "unsupported_schema",
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidArguments { .. } | Self::Malformed(_) => "invalid_arguments",
        }
    }
}

pub type Result<T> = std::result::Result<T, CompactError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_stable() {
        let cases = [
            (
                CompactError::Unsupported {
                    tool: "t".into(),
                    reason: "r".into(),
                },
                "unsupported_schema",
            ),
            (CompactError::UnknownTool("t".into()), "unknown_tool"),
            (
                CompactError::InvalidArguments {
                    tool: "t".into(),
                    reason: "r".into(),
                },
                "invalid_arguments",
            ),
            (CompactError::Malformed("m".into()), "invalid_arguments"),
        ];
        for (error, label) in cases {
            assert_eq!(error.as_label(), label, "label drifted for {error:?}");
        }
    }
}
