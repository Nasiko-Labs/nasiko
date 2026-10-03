use serde_json::Value;
use std::collections::BTreeMap;

/// Canonical supported function semantics.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalTool {
    /// Exact tool name.
    pub name: String,
    /// Exact function description.
    pub description: Option<String>,
    /// `None` means strictly zero arguments.
    pub parameters: Option<SchemaNode>,
}

/// Canonical schema with metadata shared by every supported kind.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaNode {
    /// Validation semantics.
    pub kind: SchemaKind,
    /// Description, including an explicitly empty string.
    pub description: Option<String>,
    /// Preserved string format metadata; not a format validator.
    pub format: Option<String>,
    /// Scalar enum, retaining supplied order and primitive type.
    pub enum_values: Option<Vec<Value>>,
}

/// Supported JSON Schema types.
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaKind {
    /// JSON string.
    String,
    /// Mathematically integral JSON number.
    Integer,
    /// JSON number.
    Number,
    /// JSON boolean.
    Boolean,
    /// Homogeneous array of supported schemas.
    Array(Box<SchemaNode>),
    /// Object with deterministic properties and explicit extra-key policy.
    Object {
        /// Canonical key order.
        properties: BTreeMap<String, Property>,
        /// Absent/true allows extra keys; false forbids them.
        additional_properties: bool,
    },
}

/// Object property with independent presence requirement.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    /// Whether this key must be present.
    pub required: bool,
    /// Value semantics.
    pub schema: SchemaNode,
}
