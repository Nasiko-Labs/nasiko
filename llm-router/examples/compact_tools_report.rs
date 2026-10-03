//! Token report for the compact-tools eval (informational; the organizers' scorer recomputes it).
//!
//! Run the eval first (offline), then:
//!   EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
//!   cargo run --release -p nasiko-llm-router --example compact_tools_report
//!
//! Counts tokens with `o200k_base` over the COMPLETE request body: the native baseline is
//! `{messages, tools}` built from the eval set; the compact side is the `compact_request` the
//! eval wrote (instructions, signatures and notes included; bypassed cases carry their native
//! tools, so they count as 0% savings). Reduction = 1 - sum(compact) / sum(baseline).
use serde_json::{Value, json};

type AnyError = Box<dyn std::error::Error>;

fn main() -> Result<(), AnyError> {
    let eval_path = std::env::var("EVAL_SET").map_err(|_| "set EVAL_SET to the eval JSON path")?;
    let out_path = std::env::var("OUT").map_err(|_| "set OUT to the eval's JSONL output")?;
    let data: Value = serde_json::from_str(&std::fs::read_to_string(&eval_path)?)?;
    let lines: Vec<Value> = std::fs::read_to_string(&out_path)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let bpe = tiktoken_rs::o200k_base()?;
    let count = |v: &Value| bpe.encode_with_special_tokens(&v.to_string()).len();

    let all_tools = data["tools"].as_array().ok_or("eval set has no tools")?;
    println!(
        "{:<8} {:>8} {:>8} {:>9}  compacted",
        "case", "native", "compact", "reduction"
    );
    let (mut native_sum, mut compact_sum, mut bypassed) = (0usize, 0usize, 0usize);
    for case in data["cases"].as_array().ok_or("eval set has no cases")? {
        let id = case["id"].as_str().ok_or("case id")?;
        let names: Vec<&str> = case["tools"]
            .as_array()
            .ok_or("case tools")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let tools: Vec<&Value> = all_tools
            .iter()
            .filter(|t| names.contains(&t["function"]["name"].as_str().unwrap_or("")))
            .collect();
        let native = json!({"messages": case["messages"], "tools": tools});
        let line = lines
            .iter()
            .find(|l| l["id"] == id)
            .ok_or_else(|| format!("{id} missing from OUT"))?;
        let compacted = line["compacted"].as_bool().unwrap_or(false);
        let (n, c) = (count(&native), count(&line["compact_request"]));
        // A bypassed case is sent natively: count it as no savings.
        let c = if compacted { c } else { n };
        bypassed += usize::from(!compacted);
        native_sum += n;
        compact_sum += c;
        println!(
            "{id:<8} {n:>8} {c:>8} {:>8.1}%  {compacted}",
            100.0 * (1.0 - c as f64 / n as f64)
        );
    }
    println!(
        "TOTAL    {native_sum:>8} {compact_sum:>8} {:>8.1}%  (bypassed cases: {bypassed})",
        100.0 * (1.0 - compact_sum as f64 / native_sum as f64)
    );
    println!(
        "reduction = 1 - sum(compact)/sum(baseline) = {:.4}",
        1.0 - compact_sum as f64 / native_sum as f64
    );
    Ok(())
}
