//! Token scaling experiment: native vs compact request size as the number of tools grows.
//!
//!   cargo run --release -p nasiko-llm-router --example compact_tools_scaling
//!
//! Tools come from `tool-compact/tests/fixtures/tool_pool.json` (12 SYNTHETIC public-style
//! tools). For n > 12 the pool repeats with numeric name suffixes, so large n repeats templates;
//! treat those rows as a model of per-tool overhead, not of a real catalogue. Tokens are
//! `o200k_base` over the complete request body (`{messages, tools}` native; system message
//! with the compact text for compact). Loop rows resend the definitions every step, so the
//! cumulative figure is steps x per-request tokens. These are projections of request size only;
//! cost and latency are not measured here.
use nasiko_tool_compact::{ToolDef, encode_tools};
use serde_json::{Value, json};

type AnyError = Box<dyn std::error::Error>;

fn main() -> Result<(), AnyError> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tool-compact/tests/fixtures/tool_pool.json"
    );
    let pool: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let pool = pool["tools"].as_array().ok_or("pool has no tools")?;
    let bpe = tiktoken_rs::o200k_base()?;
    let count = |v: &Value| bpe.encode_with_special_tokens(&v.to_string()).len();
    let user = json!([{"role": "user", "content": "What is the weather in Pune?"}]);

    println!("| tools | native tokens | compact tokens | saved | reduction | bypassed |");
    println!("|---:|---:|---:|---:|---:|---:|");
    let mut rows = Vec::new();
    for n in [1usize, 2, 5, 10, 25, 50] {
        let mut natives: Vec<Value> = Vec::new();
        let mut defs: Vec<ToolDef> = Vec::new();
        for i in 0..n {
            let mut tool = pool[i % pool.len()].clone();
            if i >= pool.len() {
                let name = format!(
                    "{}_{}",
                    tool["function"]["name"].as_str().unwrap_or("t"),
                    i / pool.len() + 1
                );
                tool["function"]["name"] = json!(name);
            }
            let f = &tool["function"];
            defs.push(ToolDef {
                name: f["name"].as_str().unwrap_or_default().to_string(),
                description: f["description"].as_str().map(String::from),
                parameters: f.get("parameters").cloned(),
            });
            natives.push(tool);
        }
        let encoded = encode_tools(&defs)?;
        let native = count(&json!({"messages": user, "tools": natives}));
        let mut messages = vec![json!({"role": "system", "content": encoded.text})];
        messages.extend(user.as_array().cloned().unwrap_or_default());
        let compact = count(&json!({"messages": messages}));
        let saved = native as i64 - compact as i64;
        println!(
            "| {n} | {native} | {compact} | {saved} | {:.1}% | {} |",
            100.0 * saved as f64 / native as f64,
            encoded.bypassed.len()
        );
        rows.push((n, native, compact));
    }
    println!("\nLoop (definitions resent every step), cumulative request tokens:\n");
    println!("| tools | steps | native | compact | saved |");
    println!("|---:|---:|---:|---:|---:|");
    for &(n, native, compact) in rows.iter().filter(|r| [5, 25].contains(&r.0)) {
        for steps in [1usize, 5, 20] {
            println!(
                "| {n} | {steps} | {} | {} | {} |",
                native * steps,
                compact * steps,
                (native - compact.min(native)) * steps
            );
        }
    }
    Ok(())
}
