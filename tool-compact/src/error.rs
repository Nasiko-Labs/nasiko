use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidFormat(String),
    UnknownTool(String),
    InvalidArguments(String),
    MissingRequiredArgument(String),
    InvalidArgumentType(String),
    InvalidEnumValue(String),
    UnsupportedSchema(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(msg) => write!(f, "invalid compact tool format: {msg}"),
            Self::UnknownTool(name) => write!(f, "unknown tool: {name}"),
            Self::InvalidArguments(msg) => write!(f, "invalid arguments: {msg}"),
            Self::MissingRequiredArgument(name) => {
                write!(f, "missing required argument: {name}")
            }
            Self::InvalidArgumentType(msg) => write!(f, "invalid argument type: {msg}"),
            Self::InvalidEnumValue(msg) => write!(f, "invalid enum value: {msg}"),
            Self::UnsupportedSchema(msg) => write!(f, "unsupported schema: {msg}"),
        }
    }
}

impl std::error::Error for Error {}
