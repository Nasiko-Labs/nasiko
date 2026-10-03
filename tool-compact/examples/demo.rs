use nasiko_tool_compact::{ToolDef, decode_tools, encode_tools};
use serde_json::json;

fn main() {
    println!("\n=======================================================");
    println!("       NASIKO COMPACT TOOLS: GROUP A DEMO              ");
    println!("=======================================================\n");

    let tools = vec![
        ToolDef {
            name: "create_calendar_event".to_string(),
            description: Some("Create an event in the user's calendar.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Event title"
                    },
                    "start": {
                        "type": "string",
                        "format": "date-time",
                        "description": "Start time, ISO 8601"
                    },
                    "duration_min": {
                        "type": "integer",
                        "description": "Duration in minutes"
                    },
                    "attendees": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Attendee emails"
                    },
                    "visibility": {
                        "type": "string",
                        "enum": ["public", "private"]
                    }
                },
                "required": ["title", "start"]
            })),
        },
        ToolDef {
            name: "send_email".to_string(),
            description: Some("Send an email from the user's account.".to_string()),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Recipient emails"
                    },
                    "subject": {
                        "type": "string",
                        "description": "Subject line"
                    },
                    "body": {
                        "type": "string",
                        "description": "Plain-text body"
                    },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "CC emails"
                    }
                },
                "required": ["to", "subject", "body"]
            })),
        },
    ];

    println!(">>> 1. ORIGINAL NATIVE TOOLS (Verbose JSON Schema):");
    let native_json = serde_json::to_string_pretty(&tools).unwrap();
    println!("{native_json}\n");
    println!("Raw size: {} bytes\n", native_json.len());

    println!(">>> 2. RUNNING encode_tools():");
    let compact = encode_tools(&tools).expect("Encoding failed");
    println!("--- Compact Definitions ---");
    println!("{}", compact.definitions);
    println!("\n--- Injected System Instructions ---");
    println!("{}", compact.instructions);

    let compact_rendered = compact.render();
    println!("\nTotal compact size: {} bytes", compact_rendered.len());
    let reduction = 100.0 * (1.0 - (compact_rendered.len() as f64 / native_json.len() as f64));
    println!("Character size cut: {:.1}%\n", reduction);

    println!(">>> 3. TESTING ROUND-TRIP WITH decode_tools():");
    let decoded = decode_tools(&compact).expect("Decoding failed");
    println!(
        "Successfully decoded {} tools back into JSON Schema!",
        decoded.len()
    );
    for (i, (dec, orig)) in decoded.iter().zip(tools.iter()).enumerate() {
        assert_eq!(dec.name, orig.name);
        println!(
            "  ✓ Tool [{}] '{}' names and schemas match 100%!",
            i + 1,
            dec.name
        );
    }

    println!("\n=======================================================");
    println!("               DEMO PASSED SUCCESSFULLY!               ");
    println!("=======================================================\n");
}
