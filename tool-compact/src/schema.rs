use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrimitiveType {
    String,
    Integer,
    Number,
    Boolean,
    Null,
}

impl PrimitiveType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Null => "null",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeExpr {
    Primitive(PrimitiveType),
    StringDateTime,
    StringEnum(Vec<String>),
    Array(Box<CompactSchema>),
    Object(Vec<CompactField>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Metadata {
    pub title: Option<String>,
    pub additional_properties: Option<bool>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub unique_items: Option<bool>,
    pub min_properties: Option<u64>,
    pub max_properties: Option<u64>,
    pub empty_required: bool,
    pub schema_uri: Option<String>,
}

impl Metadata {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.additional_properties.is_none()
            && self.min_length.is_none()
            && self.max_length.is_none()
            && self.min_items.is_none()
            && self.max_items.is_none()
            && self.unique_items.is_none()
            && self.min_properties.is_none()
            && self.max_properties.is_none()
            && !self.empty_required
            && self.schema_uri.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactField {
    pub name: String,
    pub optional: bool,
    pub schema: CompactSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactSchema {
    pub type_expr: TypeExpr,
    pub metadata: Option<Metadata>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactToolDef {
    pub name: String,
    pub fields: Vec<CompactField>,
    pub metadata: Option<Metadata>,
    pub schema_description: Option<String>,
    pub tool_description: Option<String>,
}
