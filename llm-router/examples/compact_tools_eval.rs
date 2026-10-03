use nasiko_tool_compact::{decode_calls, encode_tools, ToolDef};
use serde_json::{json, Value};
use std::env;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = env::var("EVAL_SET")
        .unwrap_or_else(|_| "/tmp/compact-tools-eval.json".to_string());

    let out_path = env::var("OUT")
        .unwrap_or_else(|_| "/tmp/out.jsonl".to_string());

    let input = fs::read_to_string(&eval_path)?;
let input = input.trim_start_matches('\u{feff}');
let data: Value = serde_json::from_str(input)?;


   let tools: Vec<ToolDef> = match data.get("tools").and_then(Value::as_array) {
    Some(items) => items
        .iter()
        .map(|tool| ToolDef {
            name: tool
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),

            description: tool
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string),

            parameters: tool.get("parameters").cloned(),
        })
        .collect(),

    None => Vec::new(),
};


    let compact = encode_tools(&tools)?;

    println!("=== Compact Tools ===");
    println!("{}", compact);
    println!("=====================");

    let mut output = String::new();

    if let Some(cases) = data.get("cases").and_then(Value::as_array) {
        for case in cases {
            let id = case
                .get("id")
                .cloned()
                .unwrap_or(Value::Null);

            let calls = case
                .get("calls")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();

            let mut generated = String::new();

            for call in calls {
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("");

                let arguments = call
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));

                generated.push_str("<<call ");
                generated.push_str(name);
                generated.push(' ');
                generated.push_str(&serde_json::to_string(&arguments)?);
                generated.push_str(">>");
            }

            let decoded = decode_calls(&generated, &tools);

            let result = match decoded {
                Ok(decoded_calls) => json!({
                    "id": id,
                    "ok": true,
                    "calls": decoded_calls,
                    "compact_tools": compact.as_str()
                }),

                Err(error) => json!({
                    "id": id,
                    "ok": false,
                    "error": error.to_string()
                }),
            };

            output.push_str(&serde_json::to_string(&result)?);
            output.push('\n');
        }
    }

    fs::write(&out_path, output)?;

    println!("Evaluation output written to: {}", out_path);

    Ok(())
}
