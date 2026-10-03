use nasiko_tool_compact::{
    decode_tool_call, encode_tool, encode_tool_schema, validate_tool_call,
    CompactParameter,CompactType,
};

use serde_json::Map;
use nasiko_llm_router::ir::{FunctionCall, ToolCall ,ToolDef, FunctionDef};

fn main() {
    let nasiko_tool = ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: "create_calendar_event".to_string(),
            description: Some(
                "Create a calendar event".to_string(),
            ),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string"
                    },
                    "start": {
                        "type": "string"
                    },
                    "duration_min": {
                        "type": "integer"
                    }
                },
                "required": ["title", "start"]
            })),
        },
        extra: Map::new(),
    };
    let schema = nasiko_tool
        .function
        .parameters
        .as_ref()
        .expect("tool must have parameters");

    let compact_from_nasiko = encode_tool_schema(
        &nasiko_tool.function.name,
        schema,
    )
    .expect("failed to encode Nasiko ToolDef");

    println!("\nNasiko ToolDef -> Compact Tool:");
    println!(
        "{}",
        serde_json::to_string(&compact_from_nasiko)
            .expect("failed to serialize compact tool")
    );
    // Original, larger tool definition.
    let original = r#"{
        "name": "create_calendar_event",
        "description": "Create a calendar event with a title, start time, optional duration, optional attendees, and visibility.",
        "parameters": {
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "The title of the calendar event."
                },
                "start": {
                    "type": "string",
                    "description": "The start date and time of the event."
                },
                "duration_min": {
                    "type": "integer",
                    "description": "Optional duration of the event in minutes."
                },
                "attendees": {
                    "type": "array",
                    "description": "Optional list of attendees."
                },
                "visibility": {
                    "type": "string",
                    "enum": ["public", "private"],
                    "description": "Visibility of the event."
                }
            },
            "required": ["title", "start"]
        }
    }"#;

    // Compact tool definition.
    let tool = encode_tool(
        "create_calendar_event",
        vec![
            CompactParameter {
                name: "title".to_string(),
                kind: CompactType::String,
                required: true,
            },
            CompactParameter {
                name: "start".to_string(),
                kind: CompactType::String,
                required: true,
            },
            CompactParameter {
                name: "duration_min".to_string(),
                kind: CompactType::Integer,
                required: false,
            },
            CompactParameter {
                name: "attendees".to_string(),
                kind: CompactType::Array,
                required: false,
            },
            CompactParameter {
                name: "visibility".to_string(),
                kind: CompactType::Enum(vec![
                    "public".to_string(),
                    "private".to_string(),
                ]),
                required: false,
            },
        ],
    );

    let compact = serde_json::to_string(&tool)
        .expect("failed to serialize compact tool");

    let original_size = original.len();
    let compact_size = compact.len();

    let reduction = if original_size > 0 {
        ((original_size - compact_size) as f64 / original_size as f64) * 100.0
    } else {
        0.0
    };

    println!("Tool name: {}", tool.name);
    println!("Parameters: {}", tool.parameters.len());

    println!("\nOriginal representation:");
    println!("{original}");
    println!("Original characters: {original_size}");

    println!("\nCompact representation:");
    println!("{compact}");
    println!("Compact characters: {compact_size}");

    println!("\nSize reduction: {:.2}%", reduction);
println!("\nSize reduction: {:.2}%", reduction);

    // Decode a compact AI tool call.
    let input =
        r#"<<call create_calendar_event {"title":"Meeting","start":"2026-10-05T15:00:00+05:30"}>>"#;

    let call = decode_tool_call(input)
        .expect("failed to decode compact tool call");

    // Validate the decoded call against our compact tool definition.
    validate_tool_call(&call, &tool)
        .expect("decoded tool call failed validation");

    println!("\nDecoded tool call:");
    println!("Tool name: {}", call.name);
    println!("Arguments: {:?}", call.arguments);
// Invalid call: missing required "start" field.
    let invalid_input =
        r#"<<call create_calendar_event {"title":"Meeting"}>>"#;

    let invalid_call = decode_tool_call(invalid_input)
        .expect("decoder should parse the call");

    let result = validate_tool_call(&invalid_call, &tool);

    println!("\nInvalid call validation:");
    println!("Rejected: {}", result.is_err());

    assert!(result.is_err());
// Convert the validated compact call into a normal Nasiko ToolCall.
    let normal_call = ToolCall {
        id: "compact-1".to_string(),
        kind: "function".to_string(),
        function: FunctionCall {
            name: call.name.clone(),
            arguments: serde_json::to_string(&call.arguments)
                .expect("failed to serialize arguments"),
        },
        extra: Map::new(),
    };

    println!("\nNormal Nasiko ToolCall:");
    println!("id: {}", normal_call.id);
    println!("type: {}", normal_call.kind);
    println!("name: {}", normal_call.function.name);
    println!("arguments: {}", normal_call.function.arguments);
let schema = serde_json::json!({
    "type": "object",
    "properties": {
        "title": {
            "type": "string"
        },
        "start": {
            "type": "string"
        },
        "duration_min": {
            "type": "integer"
        }
    },
    "required": ["title", "start"]
});

let schema_tool = encode_tool_schema(
    "create_calendar_event",
    &schema,
)
.expect("failed to encode tool schema");

println!("\nSchema encoder:");
println!("{}", serde_json::to_string(&schema_tool).unwrap());
}