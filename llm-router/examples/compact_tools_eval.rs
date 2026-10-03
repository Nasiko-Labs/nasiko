use nasiko_tool_compact::{
    decode_calls, encode_tools, StreamDecoder, ToolDef as CompactToolDef,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, env, fs};

use tiktoken_rs::cl100k_base;

#[derive(Debug, Deserialize)]
struct EvalSet {
    schema_version: String,
    tools: Vec<OpenAITool>,
    cases: Vec<EvalCase>,
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenAITool {
    #[serde(rename = "type")]
    kind: String,
    function: OpenAIFunction,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct OpenAIFunction {
    name: String,
    description: Option<String>,
    parameters: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Message>,
    expected: Vec<ExpectedCall>,
    #[allow(dead_code)]
    #[serde(default)]
    r#match: Option<MatchRules>,
}

#[derive(Debug, Deserialize)]
struct MatchRules {
    #[serde(default)]
    free_text_fields: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ExpectedCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    #[allow(dead_code)]
    note: String,
    tools: Vec<String>,
    chunks: Vec<String>,
    expected: DecoderExpected,
}

#[derive(Debug, Deserialize)]
struct DecoderExpected {
    calls: Option<Vec<ExpectedCall>>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct EvalOutput {
    schema_version: String,
    original_tokens: usize,
    compact_tokens: usize,
    reduction_percent: f64,
    cases_total: usize,
    cases_passed: usize,
    decoder_cases_total: usize,
    decoder_cases_passed: usize,
    adherence_pass: bool,
}

fn compact_tool_from_openai(tool: &OpenAITool) -> CompactToolDef {
    CompactToolDef {
        name: tool.function.name.clone(),
        description: tool.function.description.clone(),
        parameters: tool.function.parameters.clone(),
    }
}

fn selected_tools(
    all_tools: &HashMap<String, OpenAITool>,
    names: &[String],
) -> Vec<CompactToolDef> {
    names
        .iter()
        .filter_map(|name| all_tools.get(name))
        .map(compact_tool_from_openai)
        .collect()
}

fn values_equal(actual: &Value, expected: &Value, free_text: bool) -> bool {
    if free_text {
        return actual.is_string() && expected.is_string();
    }

    actual == expected
}

fn arguments_match(actual: &Value, expected: &Value, free_text_fields: &[String]) -> bool {
    let Some(actual_obj) = actual.as_object() else {
        return false;
    };

    let Some(expected_obj) = expected.as_object() else {
        return actual == expected;
    };

    for (key, expected_value) in expected_obj {
        let free_text = free_text_fields.iter().any(|f| f == key);

        let Some(actual_value) = actual_obj.get(key) else {
            return false;
        };

        if !values_equal(actual_value, expected_value, free_text) {
            return false;
        }
    }

    true
}

fn calls_match(
    actual: &[nasiko_tool_compact::ToolCall],
    expected: &[ExpectedCall],
    free_text_fields: &[String],
) -> bool {
    if actual.len() != expected.len() {
        return false;
    }

    actual.iter().zip(expected).all(|(actual, expected)| {
        actual.name == expected.name
            && arguments_match(&actual.arguments, &expected.arguments, free_text_fields)
    })
}

fn run_case(eval: &EvalSet, case: &EvalCase) -> bool {
    let all_tools: HashMap<String, OpenAITool> = eval
        .tools
        .iter()
        .cloned()
        .map(|tool| (tool.function.name.clone(), tool))
        .collect();

    let tools = selected_tools(&all_tools, &case.tools);

    if case.expected.is_empty() {
        return true;
    }

    let simulated = case
        .expected
        .iter()
        .map(|call| {
            format!(
                "<<call {} {}>>",
                call.name,
                serde_json::to_string(&call.arguments).unwrap()
            )
        })
        .collect::<Vec<_>>()
        .join("");

    let calls = match decode_calls(&simulated, &tools) {
        Ok(calls) => calls,
        Err(_) => return false,
    };

    let free_text_fields = case
        .r#match
        .as_ref()
        .map(|m| m.free_text_fields.clone())
        .unwrap_or_default();

    calls_match(&calls, &case.expected, &free_text_fields)
}

fn run_decoder_case(eval: &EvalSet, case: &DecoderCase) -> bool {
    let all_tools: HashMap<String, OpenAITool> = eval
        .tools
        .iter()
        .cloned()
        .map(|tool| (tool.function.name.clone(), tool))
        .collect();

    let tools = selected_tools(&all_tools, &case.tools);

    let mut decoder = StreamDecoder::new(&tools);

    for chunk in &case.chunks {
        if decoder.push(chunk).is_err() {
            return case.expected.error.as_deref() == Some("invalid_arguments")
                || case.expected.error.as_deref() == Some("unknown_tool");
        }
    }

    let result = decoder.finish();

    match (&case.expected.calls, &case.expected.error, result) {
        (Some(expected), None, Ok(actual)) => {
            calls_match(&actual, expected, &[])
        }

        (None, Some(expected_error), Err(error)) => {
            match expected_error.as_str() {
                "unknown_tool" => error.to_string().contains("unknown tool"),
                "invalid_arguments" => {
                    error.to_string().contains("invalid arguments")
                        || error.to_string().contains("invalid")
                }
                _ => false,
            }
        }

        _ => false,
    }
}

fn run_eval(path: &str) -> Result<EvalOutput, Box<dyn std::error::Error>> {
    let contents = fs::read_to_string(path)?;
    let eval: EvalSet = serde_json::from_str(&contents)?;

    if eval.schema_version != "compact-tools-eval-v1" {
        return Err(format!(
            "unsupported schema version: {}",
            eval.schema_version
        )
        .into());
    }

    let all_tools: Vec<CompactToolDef> =
        eval.tools.iter().map(compact_tool_from_openai).collect();

    let original = serde_json::to_string(&eval.tools)?;
    let compact = encode_tools(&all_tools)?.text;

    let tokenizer = cl100k_base()?;

    let original_tokens = tokenizer.encode_with_special_tokens(&original).len();
    let compact_tokens = tokenizer.encode_with_special_tokens(&compact).len();

    let reduction_percent = if original_tokens == 0 {
        0.0
    } else {
        (1.0 - compact_tokens as f64 / original_tokens as f64) * 100.0
    };

    let cases_passed = eval
        .cases
        .iter()
        .filter(|case| run_case(&eval, case))
        .count();

    let decoder_cases_passed = eval
        .decoder_cases
        .iter()
        .filter(|case| run_decoder_case(&eval, case))
        .count();

    Ok(EvalOutput {
        schema_version: eval.schema_version,
        original_tokens,
        compact_tokens,
        reduction_percent,
        cases_total: eval.cases.len(),
        cases_passed,
        decoder_cases_total: eval.decoder_cases.len(),
        decoder_cases_passed,
        adherence_pass: cases_passed == eval.cases.len()
            && decoder_cases_passed == eval.decoder_cases.len(),
    })
}

fn main() {
    let eval_path = env::var("EVAL_SET");

    match eval_path {
        Ok(path) => {
            let output_path =
                env::var("OUT").unwrap_or_else(|_| "compact-tools-out.jsonl".to_string());

            match run_eval(&path) {
                Ok(output) => {
                    println!("=== Compact Tools Evaluation ===");
                    println!("Original tokens : {}", output.original_tokens);
                    println!("Compact tokens  : {}", output.compact_tokens);
                    println!(
                        "Reduction       : {:.2}%",
                        output.reduction_percent
                    );
                    println!();
                    println!(
                        "Cases            : {}/{}",
                        output.cases_passed, output.cases_total
                    );
                    println!(
                        "Decoder cases    : {}/{}",
                        output.decoder_cases_passed, output.decoder_cases_total
                    );
                    println!(
                        "Adherence        : {}",
                        if output.adherence_pass {
                            "PASS"
                        } else {
                            "FAIL"
                        }
                    );

                    let json_line = serde_json::to_string(&output)
                        .expect("evaluation output should serialize");

                    if let Err(error) = fs::write(&output_path, format!("{json_line}\n")) {
                        eprintln!("failed to write OUT={output_path}: {error}");
                        std::process::exit(1);
                    }

                    if !output.adherence_pass {
                        std::process::exit(1);
                    }
                }

                Err(error) => {
                    eprintln!("evaluation failed: {error}");
                    std::process::exit(1);
                }
            }
        }

        Err(_) => {
            println!("EVAL_SET not set.");
            println!("Run with:");
            println!(
                "EVAL_SET=compact-tools-eval.json OUT=out.jsonl cargo run -p nasiko-llm-router --example compact_tools_eval"
            );
        }
    }
}