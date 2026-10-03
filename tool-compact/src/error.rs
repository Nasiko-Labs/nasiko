use thiserror::Error;

/// Every error this crate surfaces.
///
/// [`CompactError::Unsupported`] is the bypass signal: the schema is valid but the compact text
/// cannot carry it exactly, so the caller should send the native definition. Every other
/// variant is a decoding failure, and decoding fails closed — a reply that produces one of them
/// yields no calls at all.
///
/// [`CompactError::code`] maps each variant onto the two stable codes OpenAI-shaped callers and
/// the evaluation contract use: `unknown_tool` and `invalid_arguments`.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum CompactError {
    #[error("tool `{tool}` cannot be compacted: {feature} at `{path}`")]
    Unsupported {
        tool: String,
        path: String,
        feature: UnsupportedFeature,
    },

    #[error("unknown tool `{name}`")]
    UnknownTool { name: String },

    #[error("invalid arguments for `{tool}` at `{path}`: {fault}")]
    InvalidArguments {
        tool: String,
        path: String,
        fault: ArgumentFault,
    },

    #[error("malformed call: {fault}")]
    MalformedCall { fault: CallFault },

    #[error("invalid compact definitions at line {line}: {reason}")]
    InvalidDefinitions { line: usize, reason: String },
}

impl CompactError {
    /// The stable error code for this failure.
    ///
    /// A malformed call maps to `invalid_arguments`: once `<<call` has been written, a call was
    /// attempted, and its arguments could not be read. Callers that need the precise reason
    /// match on the variant instead.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownTool { .. } => "unknown_tool",
            Self::InvalidArguments { .. } | Self::MalformedCall { .. } => "invalid_arguments",
            Self::Unsupported { .. } => "unsupported_schema",
            Self::InvalidDefinitions { .. } => "invalid_definitions",
        }
    }
}

/// Why a schema cannot be compacted (and must be sent natively).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UnsupportedFeature {
    /// A JSON Schema keyword, or a keyword in a position, the compact text does not carry.
    #[error("keyword `{0}`")]
    Keyword(String),
    #[error("a name outside the compact grammar's character set")]
    UnsafeName,
    #[error("an enum the grammar cannot express")]
    Enum,
    #[error("`required` names a property that is not declared")]
    InconsistentRequired,
    #[error("two tools share a name")]
    DuplicateTool,
    #[error("`parameters` is not an object schema")]
    NonObjectParameters,
    #[error("a `pattern` the regex engine cannot compile")]
    Pattern,
    #[error("a recursive `$ref`")]
    RecursiveRef,
}

impl UnsupportedFeature {
    /// Stable label for diagnostics and metadata. Spelled out so a rename cannot silently change
    /// a queryable value.
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Keyword(_) => "unsupported_keyword",
            Self::UnsafeName => "unsafe_name",
            Self::Enum => "unsupported_enum",
            Self::InconsistentRequired => "inconsistent_required",
            Self::DuplicateTool => "duplicate_tool",
            Self::NonObjectParameters => "non_object_parameters",
            Self::Pattern => "unsupported_pattern",
            Self::RecursiveRef => "recursive_ref",
        }
    }
}

/// Why a call's arguments were rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ArgumentFault {
    #[error("not valid JSON ({0})")]
    Json(String),
    #[error("the same key appears twice")]
    DuplicateKey,
    #[error("required argument is missing")]
    MissingRequired,
    #[error("argument is not declared by the schema")]
    Undeclared,
    #[error("expected {expected}")]
    WrongType { expected: String },
    #[error("value is not one of the allowed values")]
    NotInEnum,
    #[error("value does not equal the required constant")]
    ConstMismatch,
    #[error("violates `{keyword}`")]
    Constraint { keyword: &'static str },
    #[error("matches none of the allowed alternatives")]
    NoAlternative,
    #[error("matches more than one `oneOf` alternative")]
    AmbiguousAlternative,
    /// The tool's schema uses an assertion this crate does not implement, so no call to it can
    /// be proven valid. Fail closed rather than pass it unchecked.
    #[error("the schema uses `{keyword}`, which cannot be validated")]
    Unvalidatable { keyword: String },
}

impl ArgumentFault {
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::Json(_) => "json",
            Self::DuplicateKey => "duplicate_key",
            Self::MissingRequired => "missing_required",
            Self::Undeclared => "undeclared",
            Self::WrongType { .. } => "wrong_type",
            Self::NotInEnum => "not_in_enum",
            Self::ConstMismatch => "const_mismatch",
            Self::Constraint { .. } => "constraint",
            Self::NoAlternative => "no_alternative",
            Self::AmbiguousAlternative => "ambiguous_alternative",
            Self::Unvalidatable { .. } => "unvalidatable",
        }
    }
}

/// Why the call marker itself could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CallFault {
    #[error("`<<call` is not followed by a tool name")]
    EmptyName,
    #[error("unexpected character `{0}` inside the call marker")]
    UnexpectedCharacter(char),
    #[error("the call is not closed with `>>`")]
    MissingClose,
    #[error("the reply ended inside a call")]
    Unterminated,
    #[error("the call exceeds the size limit")]
    TooLarge,
}

impl CallFault {
    pub fn as_label(&self) -> &'static str {
        match self {
            Self::EmptyName => "empty_name",
            Self::UnexpectedCharacter(_) => "unexpected_character",
            Self::MissingClose => "missing_close",
            Self::Unterminated => "unterminated",
            Self::TooLarge => "too_large",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_map_onto_the_two_contract_strings() {
        let unknown = CompactError::UnknownTool { name: "x".into() };
        let invalid = CompactError::InvalidArguments {
            tool: "t".into(),
            path: "/a".into(),
            fault: ArgumentFault::NotInEnum,
        };
        let malformed = CompactError::MalformedCall {
            fault: CallFault::Unterminated,
        };

        assert_eq!(unknown.code(), "unknown_tool");
        assert_eq!(invalid.code(), "invalid_arguments");
        assert_eq!(
            malformed.code(),
            "invalid_arguments",
            "a broken marker is reported as unreadable arguments"
        );
    }
}
