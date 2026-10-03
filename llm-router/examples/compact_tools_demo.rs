//! P1 Compact Tool Protocol Token and Decodability Demo.
//!
//! Measures full-request token counts under o200k_base tokenizer, demonstrates
//! one accepted call, and demonstrates one rejected call.

use nasiko_tool_compact::types::ToolDef as CompactToolDef;
use nasiko_tool_compact::{decode_calls, encode_tools};
use serde_json::json;
use tiktoken_rs::o200k_base;

fn make_calendar_tool(name: &str) -> CompactToolDef {
    CompactToolDef::new(
        name,
        Some("Create an event in the user's calendar.".to_string()),
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "Event title" },
                "start": { "type": "string", "format": "date-time", "description": "Start time, ISO 8601" },
                "attendees": { "type": "array", "items": { "type": "string" }, "description": "Attendee emails" },
                "visibility": { "type": "string", "enum": ["public", "private"] }
            },
            "required": ["title", "start"]
        }),
    )
}

fn count_tokens(bpe: &tiktoken_rs::CoreBPE, text: &str) -> usize {
    bpe.encode_with_special_tokens(text).len()
}

fn build_native_request_json(tools: &[CompactToolDef], user_text: &str) -> String {
    let tools_val: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters
                }
            })
        })
        .collect();
    let body = json!({
        "model": "gpt-4o-mini",
        "messages": [
            { "role": "system", "content": "You are a helpful assistant." },
            { "role": "user", "content": user_text }
        ],
        "tools": tools_val,
        "tool_choice": "auto"
    });
    serde_json::to_string(&body).unwrap()
}

fn build_compact_request_json(compact_doc: &str, user_text: &str) -> String {
    let body = json!({
        "model": "gpt-4o-mini",
        "messages": [
            { "role": "system", "content": "You are a helpful assistant." },
            { "role": "system", "content": compact_doc },
            { "role": "user", "content": user_text }
        ]
    });
    serde_json::to_string(&body).unwrap()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("═══════════════════════════════════════════════════════════════════════");
    println!("  P1 Compact Tool Protocol (`ctp/1`) Token & Decoding Demo");
    println!("═══════════════════════════════════════════════════════════════════════\n");

    let bpe = o200k_base().expect("failed to load o200k_base tokenizer");
    let user_prompt = "Schedule a design review meeting for October 5, 2026 at 3:00 PM IST with alice@example.com.";

    // 1. Single tool comparison
    let single_tool = vec![make_calendar_tool("create_calendar_event")];
    let single_native_json = build_native_request_json(&single_tool, user_prompt);
    let single_compact = encode_tools(&single_tool)?;
    let single_compact_json = build_compact_request_json(&single_compact.text, user_prompt);

    let single_native_tokens = count_tokens(&bpe, &single_native_json);
    let single_compact_tokens = count_tokens(&bpe, &single_compact_json);
    let single_reduction = (1.0 - (single_compact_tokens as f64 / single_native_tokens as f64)) * 100.0;

    println!("▶ Scenario 1: Single Calendar Tool");
    println!("  Native request tokens:  {}", single_native_tokens);
    println!("  Compact request tokens: {}", single_compact_tokens);
    println!("  Full-request reduction: {:.2}%\n", single_reduction);

    // 2. Ten synthetic tools comparison
    let mut ten_tools = Vec::new();
    for i in 1..=10 {
        ten_tools.push(make_calendar_tool(&format!("create_calendar_event_{i}")));
    }
    let ten_native_json = build_native_request_json(&ten_tools, user_prompt);
    let ten_compact = encode_tools(&ten_tools)?;
    let ten_compact_json = build_compact_request_json(&ten_compact.text, user_prompt);

    let ten_native_tokens = count_tokens(&bpe, &ten_native_json);
    let ten_compact_tokens = count_tokens(&bpe, &ten_compact_json);
    let ten_reduction = (1.0 - (ten_compact_tokens as f64 / ten_native_tokens as f64)) * 100.0;

    println!("▶ Scenario 2: Ten Synthetic Calendar Tools");
    println!("  Native request tokens:  {}", ten_native_tokens);
    println!("  Compact request tokens: {}", ten_compact_tokens);
    println!("  Full-request reduction: {:.2}%\n", ten_reduction);

    // 3. Accepted Call Demonstration
    println!("▶ Scenario 3: Accepted Call Demonstration");
    let valid_model_response = "I have scheduled the design review for you:\n<<call create_calendar_event {\"title\":\"Design review\",\"start\":\"2026-10-05T15:00:00+05:30\",\"visibility\":\"private\"}>>\nLet me know if you need any adjustments.";
    println!("  Raw model output:\n  ---\n  {}\n  ---", valid_model_response.replace('\n', "\n  "));
    
    let decoded_valid = decode_calls(valid_model_response, &single_tool)?;
    println!("  Decoded calls ({} call accepted):", decoded_valid.len());
    for (idx, call) in decoded_valid.iter().enumerate() {
        println!("    [{}] name: {}", idx + 1, call.name);
        println!("        args: {}", serde_json::to_string(&call.arguments)?);
    }
    println!("  Status: ACCEPTED (RFC 8259 + schema valid)\n");

    // 4. Rejected Call Demonstration (Duplicate Key)
    println!("▶ Scenario 4: Rejected Call Demonstration (Adversarial Duplicate Key)");
    let invalid_model_response = "<<call create_calendar_event {\"title\":\"Review\",\"title\":\"Hacked Title\",\"start\":\"2026-10-05T15:00:00+05:30\"}>>";
    println!("  Raw model output:\n  ---\n  {}\n  ---", invalid_model_response);
    
    match decode_calls(invalid_model_response, &single_tool) {
        Ok(_) => panic!("duplicate keys must be rejected!"),
        Err(err) => {
            println!("  Rejection error: {}", err);
            println!("  Evaluation label: {}", err.eval_label());
            println!("  Status: REJECTED (strict zero-tolerance for ambiguous syntax)\n");
        }
    }

    println!("═══════════════════════════════════════════════════════════════════════");
    println!("  Demo completed successfully. All protocol invariants verified.");
    println!("═══════════════════════════════════════════════════════════════════════");
    Ok(())
}
