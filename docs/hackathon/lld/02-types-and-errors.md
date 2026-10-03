# Task 2 — Types and errors

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

`types.rs` holds the crate's own types. They do not import `llm-router`.

```rust
pub struct ToolDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<serde_json::Value>,
}

pub struct ToolCall {
    pub name: String,
    pub arguments: String, // JSON object text, OpenAI shape
}

pub struct CompactTools {
    pub text: String,
    tools: Vec<ToolDef>, // private; decode_calls and decode_tools borrow it
}

pub enum CompactError {
    UnknownTool { name: String },
    InvalidArguments { name: String, reason: ArgumentFault },
    UnsupportedSchema { name: String, feature: String },
}

pub enum ArgumentFault {
    MissingField(String),
    WrongType { field: String },
    BadEnum { field: String },
    Malformed,
}
```

`CompactTools::tools()` returns `&[ToolDef]`. Derive `Debug`, `Clone`, `PartialEq`.
Derive `thiserror::Error` on `CompactError` only. Callers match the enum.

### Tests

```rust
#[test]
fn calendar_tool_keeps_name_description_and_required_fields() {
    let tool = calendar();
    assert_eq!(tool.name, "create_calendar_event");
    assert_eq!(tool.description.as_deref(), Some("Create an event in the user's calendar."));
    let required = tool.parameters.as_ref().unwrap()["required"].as_array().unwrap();
    assert_eq!(required, &vec![json!("title"), json!("start")]);
}

#[test]
fn error_variants_match_without_reading_the_message() {
    let unknown = CompactError::UnknownTool { name: "weather".into() };
    assert!(matches!(unknown, CompactError::UnknownTool { name } if name == "weather"));

    let missing = CompactError::InvalidArguments {
        name: "create_calendar_event".into(),
        reason: ArgumentFault::MissingField("title".into()),
    };
    assert!(matches!(
        missing,
        CompactError::InvalidArguments { reason: ArgumentFault::MissingField(field), .. } if field == "title"
    ));

    let unsupported = CompactError::UnsupportedSchema { name: "t".into(), feature: "$ref".into() };
    assert!(matches!(unsupported, CompactError::UnsupportedSchema { feature, .. } if feature == "$ref"));
}
```
