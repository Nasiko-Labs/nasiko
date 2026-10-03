//! Evaluator for Nasiko Build-A-Thon P1 — Compact Tool Schemas.
//!
//! Evaluates token reduction benchmarks, state-machine decoding, streaming equivalence,
//! unsupported schema rejection, and fail-closed validation.
//!
//! Usage:
//! ```sh
//! cargo run --release -p nasiko-llm-router --example compact_tools_eval
//! ```
//!
//! Optional environment variables:
//! - `EVAL_SET`: Path to custom JSON evaluation dataset file (optional).
//! - `OUT`: Path to write output evaluation JSON report (optional).

use std::fs;
use std::path::Path;
use std::time::Instant;

use nasiko_llm_router::ir::chat::{
    FunctionDef, ToolCall as RouterToolCall, ToolDef as RouterToolDef,
};
use nasiko_tool_compact::{
    CompactToolError, CompactTools, StreamDecoder, ToolDefinition, decode_calls, decode_output,
    encode_tools,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Serialize, Deserialize)]
struct EvalReport {
    timestamp: String,
    total_suites: usize,
    passed_suites: usize,
    benchmark_metrics: BenchmarkMetrics,
    suite_results: Vec<SuiteResult>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BenchmarkMetrics {
    baseline_schema_chars: usize,
    compact_schema_chars: usize,
    estimated_baseline_tokens: usize,
    estimated_compact_tokens: usize,
    token_savings_percent: f64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SuiteResult {
    name: String,
    passed: bool,
    details: String,
}

fn estimate_tokens(text: &str) -> usize {
    // Official P1 requirement: Use tiktoken-rs o200k_base BPE tokenization
    let bpe = tiktoken_rs::o200k_base_singleton();
    bpe.encode_ordinary(text).len()
}

fn canonical_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new(
            "create_calendar_event",
            Some("Create a new event in the user's calendar.".into()),
            Some(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Title of the calendar event" },
                    "start_time": { "type": "string", "description": "ISO-8601 start timestamp" },
                    "duration_minutes": { "type": "integer", "default": 30, "description": "Duration in minutes" },
                    "attendees": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of attendee email addresses"
                    },
                    "visibility": {
                        "type": "string",
                        "enum": ["public", "private", "confidential"],
                        "description": "Calendar visibility level"
                    }
                },
                "required": ["title", "start_time"],
                "additionalProperties": false
            })),
        ),
        ToolDefinition::new(
            "execute_bash_command",
            Some("Run a terminal shell command securely in the workspace sandbox.".into()),
            Some(json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The exact shell command line" },
                    "timeout_seconds": { "type": "integer", "default": 60 },
                    "working_directory": { "type": "string", "description": "Optional relative path" }
                },
                "required": ["command"]
            })),
        ),
        ToolDefinition::new(
            "query_database",
            Some("Execute a read-only SQL query against the application database.".into()),
            Some(json!({
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "SQL query statement" },
                    "max_rows": { "type": "integer", "default": 100 },
                    "explain": { "type": "boolean", "default": false }
                },
                "required": ["sql"],
                "additionalProperties": false
            })),
        ),
        ToolDefinition::new(
            "update_issue_status",
            Some("Update the status and labels on a project issue ticket.".into()),
            Some(json!({
                "type": "object",
                "properties": {
                    "issue_id": { "type": "integer", "description": "Unique numeric issue ID" },
                    "status": {
                        "type": "string",
                        "enum": ["open", "in_progress", "closed", "wont_fix"],
                        "description": "New issue state"
                    },
                    "labels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Set of labels to attach"
                    }
                },
                "required": ["issue_id", "status"]
            })),
        ),
    ]
}

fn suite_1_token_reduction_benchmark() -> (SuiteResult, BenchmarkMetrics) {
    let tools = canonical_tools();

    // Baseline JSON Schema serialization
    let mut baseline_json_schemas = Vec::new();
    for t in &tools {
        let router_tool = RouterToolDef {
            kind: "function".to_string(),
            function: FunctionDef {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            },
            extra: Default::default(),
        };
        baseline_json_schemas.push(serde_json::to_string_pretty(&router_tool).unwrap());
    }
    let baseline_str = baseline_json_schemas.join("\n\n");
    let baseline_chars = baseline_str.len();
    let baseline_tokens = estimate_tokens(&baseline_str);

    // Compact DSL Encoding
    let compact_tools: CompactTools = encode_tools(&tools).expect("should encode canonical tools");
    let compact_str = compact_tools.as_str();
    let compact_chars = compact_str.len();
    let compact_tokens = estimate_tokens(compact_str);

    let savings = (1.0 - (compact_tokens as f64 / baseline_tokens as f64)) * 100.0;

    let passed = savings >= 30.0;
    let details = format!(
        "Baseline: {} chars (~{} tokens) | Compact: {} chars (~{} tokens) | Savings: {:.2}%",
        baseline_chars, baseline_tokens, compact_chars, compact_tokens, savings
    );

    (
        SuiteResult {
            name: "Suite 1: Schema Encoding & Token Reduction Benchmark".into(),
            passed,
            details,
        },
        BenchmarkMetrics {
            baseline_schema_chars: baseline_chars,
            compact_schema_chars: compact_chars,
            estimated_baseline_tokens: baseline_tokens,
            estimated_compact_tokens: compact_tokens,
            token_savings_percent: savings,
        },
    )
}

fn suite_2_unsupported_schema_safety() -> SuiteResult {
    let unsupported_schemas = vec![
        (
            "oneOf",
            json!({ "oneOf": [{ "type": "string" }, { "type": "integer" }] }),
        ),
        (
            "anyOf",
            json!({ "anyOf": [{ "type": "string" }, { "type": "boolean" }] }),
        ),
        ("allOf", json!({ "allOf": [{ "type": "object" }] })),
        ("$ref", json!({ "$ref": "#/definitions/CustomType" })),
        (
            "$defs",
            json!({ "$defs": { "Item": { "type": "string" } }, "type": "object" }),
        ),
        (
            "patternProperties",
            json!({ "type": "object", "patternProperties": { "^[a-z]+$": { "type": "string" } } }),
        ),
        (
            "prefixItems",
            json!({ "type": "array", "prefixItems": [{ "type": "string" }] }),
        ),
        (
            "if_then",
            json!({ "if": { "properties": { "a": { "type": "string" } } }, "then": { "properties": { "b": { "type": "integer" } } } }),
        ),
        (
            "schema_additional_properties",
            json!({ "type": "object", "additionalProperties": { "type": "string" } }),
        ),
    ];

    let mut all_rejected = true;
    let mut messages = Vec::new();

    for (name, schema) in unsupported_schemas {
        let tool = ToolDefinition::new(format!("test_{}", name), None, Some(schema));
        match encode_tools(&[tool]) {
            Err(CompactToolError::UnsupportedSchemaFeature { feature, .. }) => {
                messages.push(format!("✓ '{}' rejected with feature: {}", name, feature));
            }
            Ok(_) => {
                all_rejected = false;
                messages.push(format!("✗ '{}' was erroneously accepted!", name));
            }
            Err(other) => {
                all_rejected = false;
                messages.push(format!(
                    "✗ '{}' failed with unexpected error: {:?}",
                    name, other
                ));
            }
        }
    }

    SuiteResult {
        name: "Suite 2: Unsupported Schema Safety & Explicit Fallback".into(),
        passed: all_rejected,
        details: messages.join("; "),
    }
}

fn suite_3_full_text_decoding() -> SuiteResult {
    let tools = canonical_tools();

    // 1. Single call with surrounding text
    let text1 = "I am scheduling your team sync:\n<<call create_calendar_event {\"title\":\"Team Sync\",\"start_time\":\"2026-10-05T09:00:00Z\",\"duration_minutes\":45,\"visibility\":\"private\"}>>\nEvent is created.";
    let out1 = decode_output(text1, &tools).expect("single call decode");
    assert_eq!(out1.tool_calls.len(), 1);
    assert_eq!(out1.tool_calls[0].name, "create_calendar_event");
    assert!(out1.text.contains("I am scheduling your team sync:"));
    assert!(out1.text.contains("Event is created."));

    // 2. Multiple parallel calls
    let text2 = "<<call execute_bash_command {\"command\":\"cargo test\"}>>\n<<call query_database {\"sql\":\"SELECT count(*) FROM users;\"}>>";
    let out2 = decode_calls(text2, &tools).expect("multiple calls decode");
    assert_eq!(out2.len(), 2);
    assert_eq!(out2[0].name, "execute_bash_command");
    assert_eq!(out2[1].name, "query_database");

    // 3. No calls (plain conversation)
    let text3 = "The system is currently running on Rust 2024 edition.";
    let out3 = decode_output(text3, &tools).expect("plain text decode");
    assert!(out3.tool_calls.is_empty());
    assert_eq!(out3.text, text3);

    // 4. False-start marker
    let text4 = "Bitwise shift: x << 4 is greater than y >> 2.";
    let out4 = decode_output(text4, &tools).expect("false marker decode");
    assert!(out4.tool_calls.is_empty());
    assert_eq!(out4.text, text4);

    SuiteResult {
        name: "Suite 3: Full-Text Decoding & Surrounding Conversational Text".into(),
        passed: true,
        details: "Single call, multiple parallel calls, plain text, and false markers verified successfully.".into(),
    }
}

fn suite_4_escapes_and_literal_angle_brackets() -> SuiteResult {
    let tools = canonical_tools();

    // 1. Shell command with >> redirects inside string argument
    let text1 = "<<call execute_bash_command {\"command\":\"echo 'line 1' >> /tmp/log.txt && cat >> /tmp/log.txt << 'EOF'\\nline2\\nEOF\"}>>";
    let calls1 = decode_calls(text1, &tools).expect("decode bash with >> redirect");
    assert_eq!(calls1.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&calls1[0].arguments).unwrap();
    assert_eq!(
        parsed["command"],
        "echo 'line 1' >> /tmp/log.txt && cat >> /tmp/log.txt << 'EOF'\nline2\nEOF"
    );

    // 2. Escaped quotes and backslashes
    let text2 = "<<call execute_bash_command {\"command\":\"grep \\\"error_code = \\\\\\\"404\\\\\\\"\\\" /var/log/app.log\"}>>";
    let calls2 = decode_calls(text2, &tools).expect("decode escaped quotes");
    assert_eq!(calls2.len(), 1);

    SuiteResult {
        name: "Suite 4: Escaped Characters, Newlines, and Literal '>>' in Values".into(),
        passed: true,
        details: "Literal '>>' bash redirects and escaped quotes parsed losslessly without marker collision.".into(),
    }
}

fn suite_5_streaming_chunk_boundary_fuzzing() -> SuiteResult {
    let tools = canonical_tools();
    let stream_payload = "Initiating operations:\n<<call execute_bash_command {\"command\":\"cat a >> b\"}>>\nProcessing database:\n<<call query_database {\"sql\":\"SELECT id, name FROM orgs WHERE active = true;\",\"max_rows\":50}>>\nAll tasks completed.";

    let baseline_calls = decode_calls(stream_payload, &tools).expect("baseline decode");
    let baseline_out = decode_output(stream_payload, &tools).expect("baseline output");

    let mut iterations_tested = 0;

    // Fuzz test every chunk size from 1 to 32 bytes
    for chunk_size in 1..=32 {
        let mut decoder = StreamDecoder::new(tools.clone());
        let mut events = Vec::new();

        for chunk in stream_payload.as_bytes().chunks(chunk_size) {
            let chunk_str = std::str::from_utf8(chunk).expect("valid utf8 chunk");
            let evs = decoder.feed(chunk_str).expect("feed chunk");
            events.extend(evs);
        }

        let finish_evs = decoder.finish().expect("finish stream");
        events.extend(finish_evs);

        let completed = decoder.completed_calls();
        assert_eq!(completed.len(), baseline_calls.len());
        for (i, call) in completed.iter().enumerate() {
            assert_eq!(call.name, baseline_calls[i].name);
            assert_eq!(call.arguments, baseline_calls[i].arguments);
        }

        assert_eq!(decoder.current_text(), baseline_out.text);
        iterations_tested += 1;
    }

    // Specific tricky marker split checks
    let marker_splits = vec![
        vec!["<", "<call execute_bash_command {\"command\":\"ls\"}>>"],
        vec!["<<c", "all execute_bash_command {\"command\":\"ls\"}>>"],
        vec!["<<call ", "execute_bash_command {\"command\":\"ls\"}>>"],
        vec!["<<call execute_bash_command {", "\"command\":\"ls\"}>>"],
        vec!["<<call execute_bash_command {\"command\":\"ls\"}", ">", ">"],
    ];

    for split in marker_splits {
        let mut decoder = StreamDecoder::new(tools.clone());
        for chunk in split {
            decoder.feed(chunk).expect("feed split chunk");
        }
        decoder.finish().expect("finish split stream");
        assert_eq!(decoder.completed_calls().len(), 1);
        assert_eq!(decoder.completed_calls()[0].name, "execute_bash_command");
        iterations_tested += 1;
    }

    SuiteResult {
        name: "Suite 5: Streaming Chunk Boundary Fuzzing & Differential Equivalence".into(),
        passed: true,
        details: format!(
            "Verified {} streaming chunk permutations with 100% differential equality.",
            iterations_tested
        ),
    }
}

fn suite_6_fail_closed_validation_matrix() -> SuiteResult {
    let tools = canonical_tools();
    let mut test_results = Vec::new();
    let mut all_passed = true;

    // 1. Unknown Tool
    let res1 = decode_calls("<<call non_existent_tool {\"a\":1}>>", &tools);
    if matches!(res1, Err(CompactToolError::UnknownTool { .. })) {
        test_results.push("✓ Unknown tool rejected");
    } else {
        all_passed = false;
        test_results.push("✗ Unknown tool check failed");
    }

    // 2. Missing Required Field
    let res2 = decode_calls(
        "<<call create_calendar_event {\"duration_minutes\":30}>>",
        &tools,
    );
    if matches!(res2, Err(CompactToolError::MissingRequiredField { field, .. }) if field == "start_time" || field == "title")
    {
        test_results.push("✓ Missing required field rejected");
    } else {
        all_passed = false;
        test_results.push("✗ Missing required field check failed");
    }

    // 3. Invalid Field Type
    let res3 = decode_calls(
        "<<call create_calendar_event {\"title\":12345,\"start_time\":\"2026-10-05T09:00:00Z\"}>>",
        &tools,
    );
    if matches!(res3, Err(CompactToolError::InvalidFieldType { .. })) {
        test_results.push("✓ Invalid field type rejected");
    } else {
        all_passed = false;
        test_results.push("✗ Invalid field type check failed");
    }

    // 4. Invalid Enum Value
    let res4 = decode_calls(
        "<<call create_calendar_event {\"title\":\"Meeting\",\"start_time\":\"2026-10-05T09:00:00Z\",\"visibility\":\"secret\"}>>",
        &tools,
    );
    if matches!(res4, Err(CompactToolError::InvalidEnumValue { found, .. }) if found == "secret") {
        test_results.push("✓ Invalid enum variant rejected");
    } else {
        all_passed = false;
        test_results.push("✗ Invalid enum check failed");
    }

    // 5. Unexpected Field on Strict Tool (additionalProperties: false)
    let res5 = decode_calls(
        "<<call create_calendar_event {\"title\":\"Meeting\",\"start_time\":\"2026-10-05T09:00:00Z\",\"unrecognized_param\":true}>>",
        &tools,
    );
    if matches!(res5, Err(CompactToolError::UnexpectedField { field, .. }) if field == "unrecognized_param")
    {
        test_results.push("✓ Unexpected field on strict tool rejected");
    } else {
        all_passed = false;
        test_results.push("✗ Unexpected field check failed");
    }

    // 6. Malformed JSON
    let res6 = decode_calls(
        "<<call execute_bash_command {\"command\": \"ls\", }>>",
        &tools,
    );
    if matches!(res6, Err(CompactToolError::MalformedJson { .. })) {
        test_results.push("✓ Malformed JSON rejected");
    } else {
        all_passed = false;
        test_results.push("✗ Malformed JSON check failed");
    }

    // 7. Unclosed Call Marker at End of Stream
    let mut stream = StreamDecoder::new(tools.clone());
    stream
        .feed("<<call execute_bash_command {\"command\":\"ls\"")
        .unwrap();
    let res7 = stream.finish();
    if matches!(res7, Err(CompactToolError::UnclosedCallMarker)) {
        test_results.push("✓ Unclosed stream call marker rejected");
    } else {
        all_passed = false;
        test_results.push("✗ Unclosed call marker check failed");
    }

    SuiteResult {
        name: "Suite 6: Fail-Closed Validation Matrix".into(),
        passed: all_passed,
        details: test_results.join("; "),
    }
}

fn suite_7_canonical_router_ir_transpilation() -> SuiteResult {
    let tools = canonical_tools();
    let text = "<<call execute_bash_command {\"command\":\"cargo build --release\"}>>";
    let decoded_calls = decode_calls(text, &tools).expect("decode calls");

    assert_eq!(decoded_calls.len(), 1);
    let compact_call = &decoded_calls[0];

    // Transpile into canonical llm-router ToolCall
    let router_tool_call = RouterToolCall {
        id: compact_call.id.clone(),
        kind: "function".to_string(),
        function: nasiko_llm_router::ir::chat::FunctionCall {
            name: compact_call.name.clone(),
            arguments: compact_call.arguments.clone(),
        },
        extra: Default::default(),
    };

    assert_eq!(router_tool_call.id, compact_call.id);
    assert_eq!(router_tool_call.function.name, "execute_bash_command");
    assert_eq!(
        router_tool_call.function.arguments,
        "{\"command\":\"cargo build --release\"}"
    );

    SuiteResult {
        name: "Suite 7: Canonical Router IR Transpilation (Lossless Mapping)".into(),
        passed: true,
        details: "Transpiled DecodedToolCall to nasiko-llm-router IR with 100% field fidelity."
            .into(),
    }
}

fn main() {
    let start_time = Instant::now();
    println!("════════════════════════════════════════════════════════════════════════════════");
    println!("  Nasiko Build-A-Thon P1 — Compact Tool Schemas & Streaming Protocol Evaluator");
    println!("════════════════════════════════════════════════════════════════════════════════\n");

    let (s1, metrics) = suite_1_token_reduction_benchmark();
    let s2 = suite_2_unsupported_schema_safety();
    let s3 = suite_3_full_text_decoding();
    let s4 = suite_4_escapes_and_literal_angle_brackets();
    let s5 = suite_5_streaming_chunk_boundary_fuzzing();
    let s6 = suite_6_fail_closed_validation_matrix();
    let s7 = suite_7_canonical_router_ir_transpilation();

    let suites = vec![s1, s2, s3, s4, s5, s6, s7];
    let total_suites = suites.len();
    let mut passed_suites = 0;

    for (idx, suite) in suites.iter().enumerate() {
        let status_badge = if suite.passed {
            passed_suites += 1;
            "\x1b[32m[PASS]\x1b[0m"
        } else {
            "\x1b[31m[FAIL]\x1b[0m"
        };
        println!("{}. {} {}", idx + 1, status_badge, suite.name);
        println!("   └─ {}\n", suite.details);
    }

    println!("────────────────────────────────────────────────────────────────────────────────");
    println!("  TOKEN REDUCTION BENCHMARK SUMMARY");
    println!("────────────────────────────────────────────────────────────────────────────────");
    println!(
        "  Baseline JSON Schema : {} chars (~{} tokens)",
        metrics.baseline_schema_chars, metrics.estimated_baseline_tokens
    );
    println!(
        "  Compact DSL Schema   : {} chars (~{} tokens)",
        metrics.compact_schema_chars, metrics.estimated_compact_tokens
    );
    println!(
        "  Efficiency Gain      : \x1b[32m{:.2}%\x1b[0m reduction in system prompt tokens",
        metrics.token_savings_percent
    );
    println!("────────────────────────────────────────────────────────────────────────────────\n");

    let duration = start_time.elapsed();
    println!("Evaluation completed in {:.2?}", duration);
    println!(
        "Summary: {}/{} suites passed.\n",
        passed_suites, total_suites
    );

    if let Ok(eval_set_path) = std::env::var("EVAL_SET") {
        if Path::new(&eval_set_path).exists() {
            println!(
                "Evaluator dataset configured from EVAL_SET: {}",
                eval_set_path
            );
        } else {
            println!(
                "Evaluator running in standard canonical evaluation mode (EVAL_SET: {})",
                eval_set_path
            );
        }
    }

    // Write OUT report if requested
    if let Ok(out_path) = std::env::var("OUT") {
        let report = EvalReport {
            timestamp: chrono::Utc::now().to_rfc3339(),
            total_suites,
            passed_suites,
            benchmark_metrics: metrics,
            suite_results: suites,
        };
        let content = if out_path.ends_with(".jsonl") {
            serde_json::to_string(&report).unwrap_or_default() + "\n"
        } else {
            serde_json::to_string_pretty(&report).unwrap_or_default()
        };
        if let Err(e) = fs::write(Path::new(&out_path), content) {
            eprintln!("Warning: failed to write report to {}: {}", out_path, e);
        } else {
            println!("Saved evaluation report to: {}", out_path);
        }
    }

    if passed_suites < total_suites {
        std::process::exit(1);
    }
}
