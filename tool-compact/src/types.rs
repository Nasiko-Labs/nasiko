use thiserror::Error;

/// Why a schema or a call was refused. Expanded by later tasks.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompactError {
    #[error("unsupported schema for '{name}': {feature}")]
    UnsupportedSchema { name: String, feature: String },
}

pub(crate) fn not_built() -> CompactError {
    CompactError::UnsupportedSchema {
        name: "uninitialized".into(),
        feature: "not built".into(),
    }
}
