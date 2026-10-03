use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use serde_json::{Value, json};
use tool_compact::{ToolDef, encode_tools, decode_calls, StreamDecoder};
use std::env;

fn main() -> anyhow::Result<()> {
    let eval_set = env::var("EVAL_SET").expect("EVAL_SET must be set");
    let out_path = env::var("OUT").expect("OUT must be set");
    let f = File::open(&eval_set)?;
    let reader = BufReader::new(f);
    let v: Value = serde_json::from_reader(reader)?;
    let cases = v.get("cases").and_then(|c| c.as_array()).cloned().unwrap_or_default();
    let tools_arr = v.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default();

    // build ToolDef list
    let mut tools = Vec::new();
    for t in tools_arr.iter() {
        let name = t.get("name").and_then(|n| n.as_str()).unwrap_or("unknown").to_string();
        let params = t.get("function").and_then(|f| f.get("parameters")).cloned();
        let desc = t.get("function").and_then(|f| f.get("description")).and_then(|d| d.as_str().map(|s| s.to_string()));
        tools.push(ToolDef { name, parameters: params, description: desc });
    }

    let compact = encode_tools(&tools)?;

    let mut out = BufWriter::new(File::create(&out_path)?);

    for case in cases.iter() {
        let id = case.get("id").and_then(|s| s.as_str()).unwrap_or("unknown");
        // build a baseline request (just echo messages)
        let compact_request = json!({"messages": case.get("messages").cloned().unwrap_or(json!([])), "tools": compact.rendered});
        // rendered_calls: for deterministic offline eval, render expected calls if present
        let rendered_calls = if let Some(expected) = case.get("expected") {
            // join expected calls into our compact syntax
            let mut parts = Vec::new();
            if let Some(arr) = expected.as_array() {
                for e in arr {
                    if let Some(name) = e.get("name").and_then(|n| n.as_str()) {
                        let args = e.get("arguments").cloned().unwrap_or(json!({}));
                        parts.push(format!("<<call {} {}>>", name, args.to_string()));
                    }
                }
            }
            parts.join("\n")
        } else { String::new() };

        // roundtrip: decode the rendered_calls back
        let roundtrip_calls = match decode_calls(&rendered_calls, &tools) {
            Ok(calls) => serde_json::to_value(calls).unwrap_or(json!([])),
            Err(e) => json!({"error": e.to_string()}),
        };

        let line = json!({"id": id, "compact_request": compact_request, "compacted": true, "rendered_calls": rendered_calls, "roundtrip_calls": roundtrip_calls});
        writeln!(out, "{}", line.to_string())?;
    }

    Ok(())
}
