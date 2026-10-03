use nasiko_tool_compact::{ToolDef, decode_calls, encode_tools};
use serde_json::Value;
use std::env;
use std::fs::File;
use std::io::{BufReader, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_set_path = env::var("EVAL_SET").expect("EVAL_SET is required");
    let out_path = env::var("OUT").expect("OUT is required");

    let file = File::open(eval_set_path)?;
    let reader = BufReader::new(file);
    let dataset: Value = serde_json::from_reader(reader)?;

    let mut out_file = File::create(out_path)?;

    if let Some(tools_array) = dataset.get("tools").and_then(|t| t.as_array()) {
        let mut parsed_tools = Vec::new();
        for t in tools_array {
            if let Ok(tool_def) = serde_json::from_value::<ToolDef>(t.clone()) {
                parsed_tools.push(tool_def);
            }
        }

        // 1. Test Encoding
        match encode_tools(&parsed_tools) {
            Ok(compact_str) => println!("--- COMPRESSED TOOLS ---\n{}", compact_str),
            Err(e) => println!("Error compressing: {}", e),
        }

        // 2. Test Decoding (Simulating an LLM response)
        let simulated_llm_response = "Sure, I can help you with that! <<call create_calendar_event {\"title\": \"Hackathon Sync\", \"duration_min\": 60, \"start\": \"tomorrow\"}>> <<call send_email {\"to\": [\"team@example.com\"], \"subject\": \"Meeting scheduled\", \"body\": \"See you there!\"}>>";

        println!(
            "\n--- SIMULATED LLM RESPONSE ---\n{}",
            simulated_llm_response
        );

        match decode_calls(simulated_llm_response, &parsed_tools) {
            Ok(decoded_calls) => {
                println!("\n--- DECODED OPENAI TOOL CALLS ---");
                println!("{}", serde_json::to_string_pretty(&decoded_calls).unwrap());
            }
            Err(e) => println!("Error decoding: {}", e),
        }
    }

    writeln!(out_file, "{{\"status\": \"eval_script_running\"}}")?;
    Ok(())
}
