//! Scan `<<call name {json}>>` out of model text.
//!
//! The closer is the first `>>` for now. A later task treats `>>` inside a
//! JSON string as argument text.

use crate::types::{ArgumentFault, CompactError};

pub(crate) struct RawCall {
    pub name: String,
    pub arguments: String,
}

pub(crate) fn scan_one(text: &str) -> Result<Option<RawCall>, CompactError> {
    let Some((_, after_marker)) = text.split_once("<<call ") else {
        return Ok(None);
    };
    let Some((name, after_name)) = after_marker.split_once(' ') else {
        return Err(malformed(&String::new()));
    };
    if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(malformed(name));
    }
    let Some((arguments, _)) = after_name.split_once(">>") else {
        return Err(malformed(name));
    };
    let arguments = arguments.trim().to_string();
    if !arguments.starts_with('{') {
        return Err(malformed(name));
    }
    Ok(Some(RawCall {
        name: name.to_string(),
        arguments,
    }))
}

pub(crate) fn malformed(name: &str) -> CompactError {
    CompactError::InvalidArguments {
        name: name.to_string(),
        reason: ArgumentFault::Malformed,
    }
}
