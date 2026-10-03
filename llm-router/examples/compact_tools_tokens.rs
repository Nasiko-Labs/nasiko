//! Dev-only full-request token measurement using pinned o200k_base.
//! EVAL_SET=input.json cargo run -p nasiko-llm-router --example compact_tools_tokens
#[path = "compact_tools/dataset.rs"]
mod dataset;
#[path = "compact_tools/request.rs"]
mod request;

use anyhow::{Context, Result};
use dataset::{Case, Dataset};
use nasiko_tool_compact::{analyze_tools, decode_tools, encode_tools};
use serde_json::json;
use std::fs::File;

fn main() -> Result<()> {
    let input = std::env::var("EVAL_SET").context("EVAL_SET must name the dataset")?;
    let dataset: Dataset = serde_json::from_reader(File::open(input)?)?;
    let lookup = dataset.lookup()?;
    let tokenizer = tiktoken_rs::o200k_base()?;
    let mut native_total = 0;
    let mut compact_total = 0;
    let mut cases = Vec::new();
    for case in &dataset.cases {
        let tools = dataset::resolve(&lookup, &case.tools)?;
        let native = request::native(case, &tools);
        let (compact, compacted) = request::build(case, &tools)?;
        let schema_roundtrip_equal = if compacted {
            Some(analyze_tools(&tools)? == analyze_tools(&decode_tools(&encode_tools(&tools)?)?)?)
        } else {
            None
        };
        let native_tokens = tokenizer
            .encode_with_special_tokens(&serde_json::to_string(&native)?)
            .len();
        let compact_tokens = tokenizer
            .encode_with_special_tokens(&serde_json::to_string(&compact)?)
            .len();
        native_total += native_tokens;
        compact_total += compact_tokens;
        cases.push(json!({"id":case.id,"compacted":compacted,"native_tokens":native_tokens,"compact_tokens":compact_tokens,"expected_call_count":case.expected.len(),"schema_roundtrip_equal":schema_roundtrip_equal}));
    }
    let reduction = if native_total == 0 {
        0.0
    } else {
        1.0 - compact_total as f64 / native_total as f64
    };
    let decoder_cases: Vec<_> = dataset.decoder_cases.iter().map(|case| json!({"id":case.id,"tool_count":case.tools.len(),"chunk_count":case.chunks.len()})).collect();
    println!(
        "{}",
        json!({"tokenizer":"tiktoken-rs=0.12.1/o200k_base","native_tokens":native_total,"compact_tokens":compact_total,"reduction":reduction,"cases":cases,"decoder_cases":decoder_cases})
    );
    Ok(())
}
