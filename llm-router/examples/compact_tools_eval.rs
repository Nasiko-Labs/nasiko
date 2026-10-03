use nasiko_llm_router::compact_tools::prepare_request;
use nasiko_llm_router::ir::{ChatRequest, FunctionDef, Message, ToolDef};
use serde_json::{Map, Value, json};

fn make_tool(index: usize) -> ToolDef {
    ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name: format!("operation_{index}"),
            description: Some(format!(
                "Perform business operation number {index} using the supplied parameters"
            )),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "resource_id": {
                        "type": "string",
                        "description": "Unique identifier of the target resource"
                    },
                    "action": {
                        "type": "string",
                        "enum": ["create", "read", "update", "delete"],
                        "description": "Action to perform"
                    },
                    "priority": {
                        "type": "integer",
                        "enum": [1, 2, 3, 4, 5],
                        "description": "Priority of the operation"
                    },
                    "options": {
                        "type": "object",
                        "properties": {
                            "dry_run": {
                                "type": "boolean",
                                "description": "Validate without applying changes"
                            },
                            "region": {
                                "type": "string",
                                "enum": ["us", "eu", "apac"]
                            }
                        },
                        "required": ["dry_run"]
                    }
                },
                "required": ["resource_id", "action"],
                "additionalProperties": false
            })),
        },
        extra: Map::new(),
    }
}

fn make_tools(count: usize) -> Vec<ToolDef> {
    (1..=count).map(make_tool).collect()
}

fn request_with_tools(tools: Vec<ToolDef>) -> ChatRequest {
    ChatRequest {
        model: Some("eval-model".to_string()),
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(Value::String("Perform the requested task.".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        }],
        tools: Some(tools),
        tool_choice: None,
        temperature: None,
        max_tokens: None,
        stream: None,
        extra: Map::new(),
    }
}

struct ResultRow {
    tools: usize,
    native_bytes: usize,
    compact_bytes: usize,
    schema_bytes: usize,
    overhead_bytes: usize,
    saved_bytes: isize,
    reduction: f64,
}

fn evaluate(tool_count: usize) -> ResultRow {
    let tools = make_tools(tool_count);

    let native_json = serde_json::to_string(&tools).expect("serialize native tools");

    let mut request = request_with_tools(tools);

    let context = prepare_request(&mut request)
        .expect("compact preparation should succeed")
        .expect("compact context should exist");

    let protocol = request
        .messages
        .first()
        .and_then(|message| message.content.as_ref())
        .and_then(Value::as_str)
        .expect("compact protocol system message");

    let native_bytes = native_json.len();
    let compact_bytes = protocol.len();
    let schema_bytes = context.compact_schema.as_str().len();

    let overhead_bytes = compact_bytes.saturating_sub(schema_bytes);
    let saved_bytes = native_bytes as isize - compact_bytes as isize;

    let reduction = if native_bytes == 0 {
        0.0
    } else {
        saved_bytes as f64 / native_bytes as f64 * 100.0
    };

    ResultRow {
        tools: tool_count,
        native_bytes,
        compact_bytes,
        schema_bytes,
        overhead_bytes,
        saved_bytes,
        reduction,
    }
}

fn main() {
    let counts = [1usize, 3, 5, 10, 20, 50];

    // Machine-readable mode for external tokenization/evaluation.
    // Each tool count is emitted as exactly one JSON object per line.
    if std::env::args().any(|arg| arg == "--json") {
        for count in counts {
            let tools = make_tools(count);

            let native = serde_json::to_string(&tools).expect("serialize native tools");

            let mut request = request_with_tools(tools);

            let context = prepare_request(&mut request)
                .expect("compact preparation should succeed")
                .expect("compact context should exist");

            let compact = request
                .messages
                .first()
                .and_then(|message| message.content.as_ref())
                .and_then(Value::as_str)
                .expect("compact protocol system message");

            println!(
                "{}",
                serde_json::json!({
                    "tools": count,
                    "native": native,
                    "compact": compact,
                    "compact_schema": context.compact_schema.as_str()
                })
            );
        }

        return;
    }

    println!();
    println!("Compact Tools Scaling Evaluation");
    println!("================================");
    println!();

    println!(
        "{:>5} | {:>12} | {:>13} | {:>12} | {:>10} | {:>11} | {:>9}",
        "Tools",
        "Native bytes",
        "Compact bytes",
        "Schema bytes",
        "Overhead",
        "Bytes saved",
        "Saving %"
    );

    println!("{}", "-".repeat(96));

    for count in counts {
        let result = evaluate(count);

        println!(
            "{:>5} | {:>12} | {:>13} | {:>12} | {:>10} | {:>11} | {:>8.2}%",
            result.tools,
            result.native_bytes,
            result.compact_bytes,
            result.schema_bytes,
            result.overhead_bytes,
            result.saved_bytes,
            result.reduction
        );
    }

    println!();
    println!("Notes:");
    println!("  Native bytes  = serialized native provider tool definitions");
    println!("  Compact bytes = complete provider-facing compact protocol");
    println!("  Schema bytes  = compact tool definitions only");
    println!("  Overhead      = fixed protocol/instruction cost");
    println!("  Saving %      = (native - compact) / native * 100");
}
