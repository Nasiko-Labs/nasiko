//! Compact tool schema evaluation harness.
//!
//! Reads a test set of tool schemas and simulated calls, applies the compact
//! encoding, simulates model output, decodes back to ToolCalls, and reports
//! detailed metrics in JSONL format.
//!
//! # Usage
//!
//! ```sh
//! EVAL_SET=path/to/eval.json OUT=results.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! # Input Format (EVAL_SET)
//!
//! JSON array of evaluation cases:
//!
//! ```json
//! [
//!   {
//!     "name": "simple_search",
//!     "tools": [/* ToolDef objects */],
//!     "simulated_calls": [
//!       {
//!         "tool": "search_web",
//!         "args": {"query": "rust programming"}
//!       }
//!     ]
//!   }
//! ]
//! ```
//!
//! # Output Format (JSONL)
//!
//! One line per test case:
//!
//! ```json
//! {
//!   "name": "simple_search",
//!   "compact_request": "search_web(query:str) - Search.",
//!   "compacted": true,
//!   "rendered_calls": "<<call search_web {\"query\":\"rust programming\"}>>",
//!   "roundtrip_calls": [...],
//!   "decoded": true,
//!   "token_delta": -142,
//!   "original_tokens": 200,
//!   "compact_tokens": 58
//! }
//! ```

use nasiko_tool_compact::{decode_calls, decode_tools, encode_tools, ToolDef};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufWriter, Write};

#[derive(Debug, Deserialize)]
struct EvalCase {
    name: String,
    tools: Vec<ToolDef>,
    simulated_calls: Vec<SimulatedCall>,
}

#[derive(Debug, Deserialize)]
struct SimulatedCall {
    tool: String,
    args: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct EvalResult {
    name: String,
    compact_request: String,
    compacted: bool,
    rendered_calls: String,
    roundtrip_calls: Option<Vec<nasiko_tool_compact::ToolCall>>,
    decoded: bool,
    decode_error: Option<String>,
    token_delta: Option<i64>,
    original_tokens: Option<usize>,
    compact_tokens: Option<usize>,
    schemas_match: bool,
}

fn count_tokens_approx(text: &str) -> usize {
    // Approximate token count (word-based)
    // For production, integrate tiktoken-rs
    let words = text.split_whitespace().count();
    let punctuation = text.chars().filter(|c| c.is_ascii_punctuation()).count();
    words + (punctuation / 2)
}

fn render_model_output(calls: &[SimulatedCall]) -> String {
    let mut output = String::new();
    
    for (idx, call) in calls.iter().enumerate() {
        if idx > 0 {
            output.push_str(" and ");
        }
        
        // Render as compact call marker
        let args_json = serde_json::to_string(&call.args)
            .unwrap_or_else(|_| "{}".to_string());
        
        // Escape >> in JSON values
        let escaped_args = args_json.replace(">>", "\\u003e\\u003e");
        
        output.push_str(&format!("<<call {} {}>>", call.tool, escaped_args));
    }
    
    output
}

fn run_eval_case(case: EvalCase) -> EvalResult {
    let name = case.name.clone();
    
    // Step 1: Encode tools to compact format
    let compact = match encode_tools(&case.tools) {
        Ok(c) => c,
        Err(e) => {
            return EvalResult {
                name,
                compact_request: format!("ERROR: {}", e),
                compacted: false,
                rendered_calls: String::new(),
                roundtrip_calls: None,
                decoded: false,
                decode_error: Some(e.to_string()),
                token_delta: None,
                original_tokens: None,
                compact_tokens: None,
                schemas_match: false,
            };
        }
    };
    
    let compacted = !compact.schemas.iter().any(|s| s.bypassed);
    
    // Step 2: Calculate token counts
    let original_json = serde_json::to_string_pretty(&case.tools).unwrap();
    let original_tokens = count_tokens_approx(&original_json);
    let compact_tokens = count_tokens_approx(&compact.text);
    let token_delta = original_tokens as i64 - compact_tokens as i64;
    
    // Step 3: Simulate model output using compact call format
    let rendered_calls = render_model_output(&case.simulated_calls);
    
    // Step 4: Decode the simulated output back to ToolCalls
    let (roundtrip_calls, decoded, decode_error) = match decode_calls(&rendered_calls, &case.tools) {
        Ok(calls) => (Some(calls), true, None),
        Err(e) => (None, false, Some(e.to_string())),
    };
    
    // Step 5: Test schema round-trip (decode_tools)
    let schemas_match = match decode_tools(&compact.schemas) {
        Ok(reconstructed) => {
            // Check if names and parameter schemas match
            reconstructed.len() == case.tools.len() &&
            reconstructed.iter().zip(case.tools.iter()).all(|(r, orig)| {
                r.function.name == orig.function.name &&
                r.function.parameters == orig.function.parameters
            })
        }
        Err(_) => false,
    };
    
    EvalResult {
        name,
        compact_request: compact.text,
        compacted,
        rendered_calls,
        roundtrip_calls,
        decoded,
        decode_error,
        token_delta: Some(token_delta),
        original_tokens: Some(original_tokens),
        compact_tokens: Some(compact_tokens),
        schemas_match,
    }
}

fn main() {
    println!("🔬 Compact Tools Evaluation Harness");
    println!("{}", "=".repeat(70));
    
    // Read environment variables
    let eval_path = std::env::var("EVAL_SET")
        .unwrap_or_else(|_| {
            // Default to the sample eval set in tool-compact fixtures
            "tool-compact/fixtures/sample_eval.json".to_string()
        });
    
    let out_path = std::env::var("OUT")
        .unwrap_or_else(|_| "compact_tools_eval_results.jsonl".to_string());
    
    println!("📂 Input:  {}", eval_path);
    println!("📄 Output: {}", out_path);
    println!("{}", "=".repeat(70));
    
    // Read eval set
    let eval_file = match File::open(&eval_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("❌ Failed to open eval set: {}", e);
            eprintln!("\nExpected file: {}", eval_path);
            eprintln!("\nCreate an eval set JSON file with this structure:");
            eprintln!(r#"[
  {{
    "name": "test_case_1",
    "tools": [/* ToolDef objects */],
    "simulated_calls": [
      {{"tool": "tool_name", "args": {{"field": "value"}}}}
    ]
  }}
]"#);
            std::process::exit(1);
        }
    };
    
    let eval_cases: Vec<EvalCase> = match serde_json::from_reader(eval_file) {
        Ok(cases) => cases,
        Err(e) => {
            eprintln!("❌ Failed to parse eval set: {}", e);
            std::process::exit(1);
        }
    };
    
    let total_cases = eval_cases.len();
    println!("\n📊 Loaded {} eval case(s)", total_cases);
    println!();
    
    // Open output file
    let out_file = match File::create(&out_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("❌ Failed to create output file: {}", e);
            std::process::exit(1);
        }
    };
    let mut writer = BufWriter::new(out_file);
    
    // Run each eval case
    let mut total_delta: i64 = 0;
    let mut success_count = 0;
    let mut bypass_count = 0;
    
    for (idx, case) in eval_cases.into_iter().enumerate() {
        let case_name = case.name.clone();
        print!("[{}/{}] Running: {} ... ", idx + 1, total_cases, case_name);
        std::io::stdout().flush().unwrap();
        
        let result = run_eval_case(case);
        
        // Track statistics
        if result.decoded {
            success_count += 1;
        }
        if !result.compacted {
            bypass_count += 1;
        }
        if let Some(delta) = result.token_delta {
            total_delta += delta;
        }
        
        // Write JSONL output
        let json_line = serde_json::to_string(&result).unwrap();
        writeln!(writer, "{}", json_line).unwrap();
        
        // Print status
        if result.decoded {
            if result.compacted {
                if let Some(delta) = result.token_delta {
                    println!("✅ OK (Δ {} tokens)", delta);
                } else {
                    println!("✅ OK");
                }
            } else {
                println!("⚠️  BYPASSED");
            }
        } else {
            println!("❌ DECODE FAILED");
            if let Some(err) = &result.decode_error {
                println!("   Error: {}", err);
            }
        }
    }
    
    writer.flush().unwrap();
    
    // Print summary
    println!();
    println!("{}", "=".repeat(70));
    println!("📈 Evaluation Summary");
    println!("{}", "=".repeat(70));
    println!("Total cases:          {}", total_cases);
    println!("Successful decodes:   {}", success_count);
    println!("Bypassed tools:       {}", bypass_count);
    println!("Total token delta:    {} tokens", total_delta);
    
    if success_count > 0 {
        let avg_delta = total_delta / success_count as i64;
        println!("Average token delta:  {} tokens/case", avg_delta);
    }
    
    println!();
    println!("✅ Results written to: {}", out_path);
    println!();
    println!("To analyze results:");
    println!("  cat {} | jq '.token_delta' | awk '{{sum+=$1}} END {{print \"Total savings:\", sum}}'", out_path);
    println!("  cat {} | jq 'select(.decoded == false)'", out_path);
    println!();
}
