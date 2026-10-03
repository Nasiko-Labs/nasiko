//! Local report, separate from the organizer's output-only evaluator and private scorer.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use serde_json::{Value, json};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dataset_path = std::env::var_os("EVAL_SET")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/compact-tools-eval-v1.json")
        });
    let output_path = std::env::var_os("OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("compact-tools-out.jsonl"));
    let dataset: Value = serde_json::from_reader(File::open(dataset_path)?)?;
    let mut records = BTreeMap::new();
    for line in BufReader::new(File::open(output_path)?).lines() {
        let record: Value = serde_json::from_str(&line?)?;
        let id = record["id"].as_str().ok_or("missing output ID")?.to_owned();
        if records.insert(id, record).is_some() {
            return Err("duplicate output ID".into());
        }
    }
    let tokenizer = tiktoken_rs::o200k_base()?;
    let cases = dataset["cases"].as_array().ok_or("missing cases")?;
    let catalog = dataset["tools"].as_array().ok_or("missing tools")?;
    let mut baseline_tokens = 0usize;
    let mut compact_tokens = 0usize;
    let mut bypassed = 0usize;
    let mut roundtrip_pass = 0usize;
    let mut live_pass = 0usize;
    let mut live_total = 0usize;
    let mut native_pass = 0usize;
    let mut native_total = 0usize;
    let mut failures = Vec::new();
    for case in cases {
        let id = case["id"].as_str().ok_or("case missing ID")?;
        let record = records.get(id).ok_or("case missing output")?;
        let names = case["tools"].as_array().ok_or("missing case tools")?;
        let tools: Vec<_> = names
            .iter()
            .map(|name| {
                catalog
                    .iter()
                    .find(|tool| tool["function"]["name"] == *name)
                    .cloned()
                    .ok_or("unknown tool reference")
            })
            .collect::<Result<_, _>>()?;
        let mut baseline = json!({"messages":case["messages"],"tools":tools});
        if record.get("live_calls").is_some() {
            for key in [
                "model",
                "temperature",
                "max_tokens",
                "max_completion_tokens",
                "reasoning_effort",
            ] {
                baseline[key] = record["compact_request"][key].clone();
            }
            baseline["messages"].as_array_mut().ok_or("messages must be an array")?.insert(0,json!({"role":"system","content":"Today is 2026-10-03. Timezone: Asia/Kolkata."}));
        }
        let native_count = tokenizer.encode_ordinary(&baseline.to_string()).len();
        baseline_tokens += native_count;
        compact_tokens += if record["compacted"] == false {
            bypassed += 1;
            native_count
        } else {
            tokenizer
                .encode_ordinary(&record["compact_request"].to_string())
                .len()
        };
        if calls_match(&record["roundtrip_calls"], case) {
            roundtrip_pass += 1;
        } else {
            failures.push(format!("{id}: roundtrip"));
        }
        if let Some(live) = record.get("live_calls") {
            live_total += 1;
            if calls_match(&live["calls"], case) {
                live_pass += 1;
            } else {
                failures.push(format!("{id}: live"));
            }
        }
        if let Some(native) = record.get("baseline_calls") {
            native_total += 1;
            if calls_match(&native["calls"], case) {
                native_pass += 1;
            } else {
                failures.push(format!("{id}: native"));
            }
        }
    }
    let mut decoder_pass = 0usize;
    let decoder_cases = dataset["decoder_cases"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    for case in decoder_cases {
        let id = case["id"].as_str().ok_or("decoder case missing ID")?;
        let record = records.get(id).ok_or("decoder case missing output")?;
        if record["decoded"] == case["expected"] {
            decoder_pass += 1;
        } else {
            failures.push(format!("{id}: decoder"));
        }
    }
    if records.len() != cases.len() + decoder_cases.len() {
        return Err("unexpected output records".into());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "tokenizer":"tiktoken-rs=0.7.0/o200k_base", "case_count":cases.len(),
            "baseline_tokens":baseline_tokens,"compact_tokens":compact_tokens,
            "token_reduction": if baseline_tokens == 0 {0.0} else {1.0-compact_tokens as f64/baseline_tokens as f64},
            "bypassed":bypassed,"roundtrip_pass":roundtrip_pass,
            "decoder_pass":decoder_pass,"decoder_total":decoder_cases.len(),
            "live_pass":live_pass,"live_total":live_total,"native_pass":native_pass,"native_total":native_total,
            "failures":failures,"scope":"Local development report; organizer private scorer is authoritative."
        }))?
    );
    Ok(())
}

fn calls_match(actual: &Value, case: &Value) -> bool {
    let (Some(actual), Some(expected)) = (actual.as_array(), case["expected"].as_array()) else {
        return false;
    };
    if actual.len() != expected.len() {
        return false;
    }
    let free_text = case
        .pointer("/match/free_text_fields")
        .and_then(Value::as_array);
    actual.iter().zip(expected).all(|(actual, expected)| {
        if actual["name"] != expected["name"] {
            return false;
        }
        let (Some(actual), Some(expected)) = (
            actual["arguments"].as_object(),
            expected["arguments"].as_object(),
        ) else {
            return false;
        };
        if actual.len() != expected.len() {
            return false;
        }
        expected.iter().all(|(key, expected)| {
            let Some(actual) = actual.get(key) else {
                return false;
            };
            if free_text
                .is_some_and(|fields| fields.iter().any(|field| field.as_str() == Some(key)))
            {
                // The public rubric marks title/body/subject: check presence and string type.
                actual.is_string() && expected.is_string()
            } else {
                actual == expected
            }
        })
    })
}
