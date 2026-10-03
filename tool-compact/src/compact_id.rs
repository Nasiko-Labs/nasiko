//! Cacheable compact IDs for multi-turn token savings.
//!
//! A [`CompactId`] is a content-addressable hash of a tool set's canonical JSON.
//! After an initial handshake turn where the full compact schema is sent, subsequent
//! turns can reference the short ID (e.g. `CT-a3f2b1c8`) instead of repeating the
//! schema, saving tokens across multi-turn flows.
//!
//! # Invalidation
//!
//! If the tool set changes (tools added, removed, or parameters modified), the
//! [`CompactId`] changes and the full schema must be re-sent.

use sha2::{Digest, Sha256};

use crate::types::ToolDef;

/// A content-addressable ID for a set of compacted tool schemas.
///
/// Format: `CT-` followed by 8 hex characters (32-bit prefix of SHA-256).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompactId(pub String);

impl CompactId {
    /// Compute from the canonical JSON of the tool definitions.
    ///
    /// Deterministic: the same tool set always produces the same ID.
    pub fn from_tools(tools: &[ToolDef]) -> Self {
        let canonical = serde_json::to_string(tools).unwrap_or_default();
        let hash = Sha256::digest(canonical.as_bytes());
        let short = hex::encode(&hash[..4]); // 8 hex chars
        CompactId(format!("CT-{short}"))
    }

    /// Check if tool definitions still match this ID (invalidation check).
    pub fn matches(&self, tools: &[ToolDef]) -> bool {
        Self::from_tools(tools) == *self
    }
}

impl std::fmt::Display for CompactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionDef, ToolDef};
    use serde_json::json;

    fn tool(name: &str) -> ToolDef {
        ToolDef {
            kind: "function".into(),
            function: FunctionDef {
                name: name.into(),
                description: None,
                parameters: Some(json!({
                    "type": "object",
                    "properties": {
                        "x": { "type": "string" }
                    }
                })),
            },
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn deterministic_id() {
        let tools = vec![tool("a"), tool("b")];
        let id1 = CompactId::from_tools(&tools);
        let id2 = CompactId::from_tools(&tools);
        assert_eq!(id1, id2);
    }

    #[test]
    fn different_tools_different_id() {
        let tools_a = vec![tool("a")];
        let tools_b = vec![tool("b")];
        assert_ne!(
            CompactId::from_tools(&tools_a),
            CompactId::from_tools(&tools_b)
        );
    }

    #[test]
    fn matches_same_tools() {
        let tools = vec![tool("test")];
        let id = CompactId::from_tools(&tools);
        assert!(id.matches(&tools));
    }

    #[test]
    fn does_not_match_different_tools() {
        let tools_a = vec![tool("a")];
        let tools_b = vec![tool("b")];
        let id = CompactId::from_tools(&tools_a);
        assert!(!id.matches(&tools_b));
    }

    #[test]
    fn format_starts_with_ct_prefix() {
        let tools = vec![tool("x")];
        let id = CompactId::from_tools(&tools);
        assert!(id.0.starts_with("CT-"));
        // CT- + 8 hex chars = 11 total
        assert_eq!(id.0.len(), 11);
    }

    #[test]
    fn display_matches_inner() {
        let tools = vec![tool("test")];
        let id = CompactId::from_tools(&tools);
        assert_eq!(format!("{id}"), id.0);
    }
}
