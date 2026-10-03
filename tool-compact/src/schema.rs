// use serde::{Deserialize, Serialize};

// #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
// pub struct ToolDef {
//     pub name: String,
//     pub description: Option<String>,
//     pub parameters: Schema,
// }

// #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
// pub struct ToolCall {
//     pub name: String,
//     pub arguments: serde_json::Value,
// }

// #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
// pub struct Schema {
//     pub schema_type: SchemaType,

//     // Vec preserves the order from the original JSON Schema.
//     pub properties: Vec<(String, Schema)>,

//     pub required: Vec<String>,
//     pub enum_values: Vec<String>,
//     pub items: Option<Box<Schema>>,
//     pub description: Option<String>,
// }

// #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
// pub enum SchemaType {
//     String,
//     Integer,
//     Number,
//     Boolean,
//     Object,
//     Array,
//     Unknown(String),
// }

// impl Schema {
//     pub fn new(schema_type: SchemaType) -> Self {
//         Self {
//             schema_type,
//             properties: Vec::new(),
//             required: Vec::new(),
//             enum_values: Vec::new(),
//             items: None,
//             description: None,
//         }
//     }
// }




use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Schema,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Schema {
    pub schema_type: SchemaType,
    pub properties: Vec<(String, Schema)>,
    pub required: Vec<String>,
    pub enum_values: Vec<String>,
    pub items: Option<Box<Schema>>,
    pub description: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SchemaType {
    String,
    Integer,
    Number,
    Boolean,
    Object,
    Array,
    Unknown(String),
}

impl Schema {
pub fn new(schema_type: SchemaType) -> Self {
    Self {
        schema_type,
        properties: Vec::new(),
        required: Vec::new(),
        enum_values: Vec::new(),
        items: None,
        description: None,
        format: None,
    }
}
}