/// compact_eval — offline evaluation of the tool-compact library.
///
/// Measures the byte reduction achieved by replacing verbose OpenAI-style
/// tool-definition JSON with the compact text representation, using the
/// exact tool schemas from the DevOps agent.
///
/// Run with:
///   cargo run --example compact_eval -p tool-compact
///
/// No API keys or network access required.
///
/// NOTE ON MEASUREMENT UNITS
/// ─────────────────────────
/// All sizes are reported in **UTF-8 bytes**, NOT in model tokens.
/// Byte counts are the most honest proxy we can produce without a tokeniser
/// library dependency.  The actual token reduction will typically be similar
/// in proportion but not identical to the byte reduction.
use serde_json::json;
use tool_compact::{CompactToolSet, StreamDecoder, decode_calls, decode_tools, encode_tools};

// ─── Evaluation dataset ───────────────────────────────────────────────────────
//
// These are the real OpenAI-style tool definitions from the DevOps agent
// (agents/devops-agent/src/tools.rs), embedded here verbatim so the
// evaluation does not require the agent crate to compile or run.
//
// Any schema change in the agent should be reflected here to keep the
// evaluation accurate.

fn devops_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "github_repo_info",
                "description": "Get info about a GitHub repository including stars, forks, open issues, language, and description.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": {
                            "type": "string",
                            "description": "Repository owner (user or organization)"
                        },
                        "repo": {
                            "type": "string",
                            "description": "Repository name"
                        }
                    },
                    "required": ["owner", "repo"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "github_actions_runs",
                "description": "List recent CI/CD workflow runs for a GitHub repository. Shows workflow name, status, conclusion, branch, and trigger event.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "owner": {
                            "type": "string",
                            "description": "Repository owner (user or organization)"
                        },
                        "repo": {
                            "type": "string",
                            "description": "Repository name"
                        }
                    },
                    "required": ["owner", "repo"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "docker_hub_search",
                "description": "Search Docker Hub for container images. Returns image name, description, star count, pull count, and whether it's an official image.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query for Docker Hub images"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "check_endpoint",
                "description": "Check if an HTTP endpoint is responding and measure its latency. Reports status code, response time, content-type, and health status.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to check (must include scheme, e.g. https://example.com)"
                        }
                    },
                    "required": ["url"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web for DevOps documentation, tutorials, best practices, or troubleshooting guides. Use when existing tools don't cover the question.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query for web results"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
    ]
}

fn main() {
    let tools = devops_tool_definitions();
    let cts = CompactToolSet::new(tools.clone());

    println!("════════════════════════════════════════════════════════════");
    println!("  tool-compact — Evaluation Report");
    println!("  Measurement unit: UTF-8 bytes (NOT tokens)");
    println!("════════════════════════════════════════════════════════════");
    println!();

    // ── Per-tool breakdown ────────────────────────────────────────────────────

    let result = cts.analyze();

    println!("Per-tool breakdown");
    println!("──────────────────");

    for t in &result.tools {
        if t.compactable {
            let saved = t.baseline_bytes.saturating_sub(t.compact_bytes);
            let pct = (saved as f64 / t.baseline_bytes as f64) * 100.0;
            println!("  Tool:     {}", t.name);
            println!("  Status:   COMPACTED");
            println!("  Baseline: {} bytes", t.baseline_bytes);
            println!(
                "  Compact:  {} bytes  → {}",
                t.compact_bytes, t.compact_line
            );
            println!("  Saved:    {} bytes  ({:.1}%)", saved, pct);
        } else {
            println!("  Tool:     {}", t.name);
            println!("  Status:   BYPASSED (schema contains unsupported constraints)");
            println!(
                "  Baseline: {} bytes  (sent verbatim — 0 savings)",
                t.baseline_bytes
            );
        }
        println!();
    }

    // ── Aggregate summary ─────────────────────────────────────────────────────

    println!("════════════════════════════════════════════════════════════");
    println!("  Summary");
    println!("════════════════════════════════════════════════════════════");
    println!("  Total tools:        {}", result.tools.len());
    println!("  Compacted tools:    {}", result.compacted_count());
    println!("  Bypassed tools:     {}", result.bypassed_count());
    println!();
    println!(
        "  Baseline total:     {} bytes",
        result.total_baseline_bytes
    );
    println!(
        "  Compact total:      {} bytes  (compactable tools only)",
        result.total_compact_bytes
    );
    println!(
        "  Bypass overhead:    {} bytes  (bypassed tools, unchanged)",
        result.total_bypass_bytes
    );
    println!();
    println!("  Bytes saved:        {}", result.bytes_saved());
    println!(
        "  Reduction:          {:.2}%  (of total baseline)",
        result.percent_saved()
    );
    println!();

    // ── Compact system block ──────────────────────────────────────────────────

    println!("════════════════════════════════════════════════════════════");
    println!("  Compact system block (what the model receives)");
    println!("════════════════════════════════════════════════════════════");
    let block = cts.compact_system_block();
    println!("{block}");

    // ── Round-trip validation: encode → decode ────────────────────────────────

    section("Round-trip: encode_tools → decode_tools");
    let compact_repr = encode_tools(&tools);
    match decode_tools(&compact_repr) {
        Ok(reconstructed) => {
            let original_names: Vec<&str> = tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str())
                .collect();
            let rebuilt_names: Vec<&str> = reconstructed
                .iter()
                .filter_map(|t| t["function"]["name"].as_str())
                .collect();

            if original_names == rebuilt_names {
                println!("  Tool names preserved:  PASS {original_names:?}");
            } else {
                println!("  Tool names MISMATCH:");
                println!("    Original:    {original_names:?}");
                println!("    Rebuilt:     {rebuilt_names:?}");
                std::process::exit(1);
            }

            // Check parameter names and types for each tool.
            let mut all_ok = true;
            for (orig, rebuilt) in tools.iter().zip(reconstructed.iter()) {
                let name = orig["function"]["name"].as_str().unwrap_or("");
                let orig_props = orig["function"]["parameters"]["properties"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                let rebuilt_props = rebuilt["function"]["parameters"]["properties"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();

                for (param, schema) in &orig_props {
                    let orig_type = schema["type"].as_str().unwrap_or("");
                    let rebuilt_type = rebuilt_props
                        .get(param)
                        .and_then(|s| s["type"].as_str())
                        .unwrap_or("<missing>");
                    if orig_type == rebuilt_type {
                        println!("  {name}.{param}: type={orig_type}  PASS");
                    } else {
                        println!("  {name}.{param}: orig={orig_type} rebuilt={rebuilt_type}  FAIL");
                        all_ok = false;
                    }
                }
            }
            if !all_ok {
                std::process::exit(1);
            }
        }
        Err(e) => {
            println!("  decode_tools FAILED: {e}");
            std::process::exit(1);
        }
    }

    // ── Tool-call round-trip: StreamDecoder → decode_calls → execute path ─────

    section("Tool-call round-trip: StreamDecoder → decode_calls");

    // Representative compact calls for each tool (deterministic, no API call).
    let test_calls: &[(&str, &str)] = &[
        (
            "github_repo_info",
            r#"{"owner":"octocat","repo":"hello-world"}"#,
        ),
        (
            "github_actions_runs",
            r#"{"owner":"tokio-rs","repo":"tokio"}"#,
        ),
        ("docker_hub_search", r#"{"query":"postgres"}"#),
        ("check_endpoint", r#"{"url":"https://example.com"}"#),
        ("web_search", r#"{"query":"GitHub Actions Rust"}"#),
    ];

    let mut all_ok = true;
    for (tool_name, args_json) in test_calls {
        let compact_call = format!("{tool_name}({args_json})");
        let mut dec = StreamDecoder::new(tools.clone());

        match dec.push(&compact_call) {
            Ok(decoded) if decoded.len() == 1 => {
                let call = &decoded[0];
                let got_name = call["function"]["name"].as_str().unwrap_or("");
                let got_args = &call["function"]["arguments"];
                if got_name == *tool_name {
                    println!("  {tool_name}: decoded OK  args={got_args}");
                } else {
                    println!("  {tool_name}: name mismatch (got {got_name})  FAIL");
                    all_ok = false;
                }
            }
            Ok(decoded) => {
                println!(
                    "  {tool_name}: expected 1 decoded call, got {}  FAIL",
                    decoded.len()
                );
                all_ok = false;
            }
            Err(e) => {
                println!("  {tool_name}: decode error: {e}  FAIL");
                all_ok = false;
            }
        }

        // Verify finish() sees no leftover content.
        if let Err(e) = dec.finish() {
            println!("  {tool_name}: finish() error: {e}  FAIL");
            all_ok = false;
        }
    }

    if !all_ok {
        std::process::exit(1);
    }

    // ── decode_calls compatibility with existing execute() path ───────────────

    section("decode_calls compatibility check");

    // Verify that the decoded call structure is compatible with the agent's
    // existing tool execution path, which reads:
    //   tc["function"]["name"].as_str()
    //   tc["function"]["arguments"].as_str()  -- NOTE: agent uses raw string
    //
    // decode_calls returns a structured Value, not a raw arguments string.
    // The integration layer must serialise arguments back to a string for
    // tools::execute().  We verify this is possible here.

    let decoded = decode_calls(
        &tools,
        "github_repo_info",
        r#"{"owner":"octocat","repo":"hello-world"}"#,
    )
    .expect("decode_calls must succeed for a valid call");

    let name_field = decoded["function"]["name"].as_str().unwrap_or("");
    let args_value = &decoded["function"]["arguments"];

    // The agent currently receives raw JSON strings from the model.
    // When using the compact path, the integration must serialise the
    // validated args Value back to a string for tools::execute().
    let args_for_execute = serde_json::to_string(args_value).expect("args must serialise");

    assert_eq!(name_field, "github_repo_info");
    assert!(args_for_execute.contains("octocat"));
    println!("  name_field:        {name_field}  OK");
    println!("  args_for_execute:  {args_for_execute}  OK");
    println!("  NOTE: The integration layer must call serde_json::to_string(&args)");
    println!("        before passing to tools::execute(name, &args_str).");

    // ── Optional-parameter check ──────────────────────────────────────────────

    section("Optional-parameter check");
    println!("  All 5 DevOps tools have all parameters listed in `required`.");
    println!("  decode_tools() marks ALL params required (Phase 4 limitation).");
    println!("  This limitation does NOT affect the current evaluation because");
    println!("  no optional parameters exist in the DevOps tool set.");

    // ── Final status ──────────────────────────────────────────────────────────

    println!();
    println!("════════════════════════════════════════════════════════════");
    println!("  All checks PASSED.  Evaluation complete.");
    println!("════════════════════════════════════════════════════════════");
}

fn section(title: &str) {
    println!();
    println!("════════════════════════════════════════════════════════════");
    println!("  {title}");
    println!("════════════════════════════════════════════════════════════");
}
