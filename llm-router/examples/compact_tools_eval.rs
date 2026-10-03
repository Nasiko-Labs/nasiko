// use nasiko_tool_compact::{
//     decode::decode_calls,
//     encode::encode_tools,
//     validate::validate_call,
//     Schema, SchemaType, ToolDef,
   
// };
//  use tiktoken_rs::o200k_base;

// fn count_tokens(text: &str) -> usize {
//     let bpe = o200k_base().expect("failed to load o200k_base tokenizer");
//     bpe.encode_with_special_tokens(text).len()
// }
// fn main() {
//     let tools = vec![
//         ToolDef {
//             name: "create_calendar_event".to_string(),
//             description: Some("Create an event".to_string()),
//             parameters: calendar_event_schema(),
//         },
//         ToolDef {
//             name: "send_email".to_string(),
//             description: Some("Send an email".to_string()),
//             parameters: email_schema(),
//         },
//     ];

//     println!("=== P1 Compact Tools Evaluation ===\n");

//     // ------------------------------------------------------------
//     // 1. Encode tool definitions
//     // ------------------------------------------------------------
//     let compact = encode_tools(&tools);


//     let verbose = verbose_tools_json();

// let original_tokens = count_tokens(&verbose);
// let compact_tokens = count_tokens(&compact);

// let reduction = if original_tokens == 0 {
//     0.0
// } else {
//     (1.0 - compact_tokens as f64 / original_tokens as f64) * 100.0
// };

// println!("=== Token Reduction ===");
// println!("Original tokens: {original_tokens}");
// println!("Compact tokens:  {compact_tokens}");
// println!("Reduction:       {reduction:.2}%\n");

//     // let original = serde_json::to_string_pretty(&tools)
//     // .expect("failed to serialize tool definitions");

// // let original_tokens = count_tokens(&original);
// // let compact_tokens = count_tokens(&compact);

// // let reduction = if original_tokens == 0 {
// //     0.0
// // } else {
// //     (1.0 - compact_tokens as f64 / original_tokens as f64) * 100.0
// // };

// // println!("=== Token Reduction ===");
// // println!("Original tokens: {original_tokens}");
// // println!("Compact tokens:  {compact_tokens}");
// // println!("Reduction:       {reduction:.2}%\n");

//     println!("Compact tool definitions:");
//     println!("{compact}\n");

//     // ------------------------------------------------------------
//     // 2. Decode a simulated LLM tool call
//     // ------------------------------------------------------------
//     let model_output = r#"
// I will create the event.

// <<call create_calendar_event {"title":"Design review","start":"2026-10-03T14:00:00","duration_min":60,"attendees":["alice@example.com"],"visibility":"private"}>>
// "#;

//     println!("Model output:");
//     println!("{model_output}");

//     let calls = decode_calls(model_output)
//         .expect("model output should contain a valid compact tool call");

//     assert_eq!(calls.len(), 1);

//     // ------------------------------------------------------------
//     // 3. Validate decoded call
//     // ------------------------------------------------------------
//     for call in &calls {
//         validate_call(call, &tools)
//             .expect("decoded tool call should pass schema validation");

//         println!("Validated call:");
//         println!("  tool: {}", call.name);
//         println!("  args: {}", call.arguments);
//     }

//     // ------------------------------------------------------------
//     // 4. Test failure case: unknown tool
//     // ------------------------------------------------------------
//     let bad_output =
//         r#"<<call unknown_tool {"title":"Should fail"}>>"#;

//     let bad_calls = decode_calls(bad_output)
//         .expect("syntax should still be valid");

//     let result = validate_call(&bad_calls[0], &tools);

//     assert!(
//         result.is_err(),
//         "unknown tool must be rejected"
//     );

//     println!("\nFailure case:");
//     println!("  unknown tool correctly rejected");

//     // ------------------------------------------------------------
//     // 5. Test failure case: invalid enum
//     // ------------------------------------------------------------
//     let invalid_enum_output =
//         r#"<<call create_calendar_event {"title":"Review","start":"2026-10-03T14:00:00","visibility":"everyone"}>>"#;

//     let invalid_calls = decode_calls(invalid_enum_output)
//         .expect("syntax should be valid");

//     let result = validate_call(&invalid_calls[0], &tools);

//     assert!(
//         result.is_err(),
//         "invalid enum must be rejected"
//     );

//     println!("  invalid enum correctly rejected");

//     println!("\nP1 evaluation passed.");
// }
// fn calendar_event_schema() -> Schema {
//     let mut schema = Schema::new(SchemaType::Object);

//     schema.properties.push((
//         "title".to_string(),
//         Schema::new(SchemaType::String),
//     ));

//     let mut start = Schema::new(SchemaType::String);
//     start.format = Some("date-time".to_string());

//     schema.properties.push((
//         "start".to_string(),
//         start,
//     ));

//     schema.properties.push((
//         "duration_min".to_string(),
//         Schema::new(SchemaType::Integer),
//     ));

//     let mut attendees = Schema::new(SchemaType::Array);
//     attendees.items = Some(Box::new(Schema::new(SchemaType::String)));

//     schema.properties.push((
//         "attendees".to_string(),
//         attendees,
//     ));

//     let mut visibility = Schema::new(SchemaType::String);
//     visibility.enum_values = vec![
//         "public".to_string(),
//         "private".to_string(),
//     ];

//     schema.properties.push((
//         "visibility".to_string(),
//         visibility,
//     ));

//     schema.required = vec![
//         "title".to_string(),
//         "start".to_string(),
//     ];

//     schema
// }

// fn email_schema() -> Schema {
//     let mut schema = Schema::new(SchemaType::Object);

//     schema.properties.push((
//         "to".to_string(),
//         Schema::new(SchemaType::String),
//     ));

//     schema.properties.push((
//         "subject".to_string(),
//         Schema::new(SchemaType::String),
//     ));

//     schema.properties.push((
//         "body".to_string(),
//         Schema::new(SchemaType::String),
//     ));

//     schema.required = vec![
//         "to".to_string(),
//         "subject".to_string(),
//         "body".to_string(),
//     ];

//     schema
// }


// fn verbose_tools_json() -> String {
//     serde_json::json!([
//         {
//             "type": "function",
//             "function": {
//                 "name": "create_calendar_event",
//                 "description": "Create an event",
//                 "parameters": {
//                     "type": "object",
//                     "properties": {
//                         "title": {
//                             "type": "string"
//                         },
//                         "start": {
//                             "type": "string",
//                             "format": "date-time"
//                         },
//                         "duration_min": {
//                             "type": "integer"
//                         },
//                         "attendees": {
//                             "type": "array",
//                             "items": {
//                                 "type": "string"
//                             }
//                         },
//                         "visibility": {
//                             "type": "string",
//                             "enum": [
//                                 "public",
//                                 "private"
//                             ]
//                         }
//                     },
//                     "required": [
//                         "title",
//                         "start"
//                     ]
//                 }
//             }
//         },
//         {
//             "type": "function",
//             "function": {
//                 "name": "send_email",
//                 "description": "Send an email",
//                 "parameters": {
//                     "type": "object",
//                     "properties": {
//                         "to": {
//                             "type": "string"
//                         },
//                         "subject": {
//                             "type": "string"
//                         },
//                         "body": {
//                             "type": "string"
//                         }
//                     },
//                     "required": [
//                         "to",
//                         "subject",
//                         "body"
//                     ]
//                 }
//             }
//         }
//     ])
//     .to_string()
// }



use nasiko_tool_compact::{
    decode::decode_calls,
    encode::encode_tools,
    validate::validate_call,
    stream::StreamDecoder,
    parse_schema,
    ToolCall,
    ToolDef,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    env,
    fs::File,
   io::{BufReader, Write},
};
use tiktoken_rs::o200k_base;

#[derive(Debug, Deserialize)]
struct EvalSet {
    schema_version: String,
    tools: Vec<RawTool>,
    cases: Vec<EvalCase>,
    decoder_cases: Vec<DecoderCase>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RawTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: RawFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct RawFunction {
    name: String,
    description: Option<String>,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct EvalCase {
    id: String,
    tools: Vec<String>,
    messages: Vec<Value>,
    expected: Vec<ToolCall>,
    #[allow(dead_code)]
    #[serde(default)]
    r#match: Option<MatchConfig>,
}

#[derive(Debug, Deserialize)]
struct MatchConfig {
    #[allow(dead_code)]
    #[serde(default)]
    free_text_fields: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DecoderCase {
    id: String,
    tools: Vec<String>,
    chunks: Vec<String>,
    expected: ExpectedDecoderResult,
}
#[derive(Debug, Deserialize)]
struct ExpectedDecoderResult {
    #[allow(dead_code)]
    calls: Option<Vec<ToolCall>>,
    error: Option<String>,
}   

#[derive(Debug, Serialize)]
struct CaseOutput {
    id: String,
    compact_request: Value,
    compacted: bool,
    rendered_calls: String,
    roundtrip_calls: Vec<ToolCall>,
    raw_output: Option<String>,
    live_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Serialize)]
struct DecoderOutput {
    id: String,
    decoded: Value,
}

fn build_tool(raw: &RawTool) -> Result<ToolDef, Box<dyn std::error::Error>> {
    if raw.tool_type != "function" {
        return Err(format!("unsupported tool type: {}", raw.tool_type).into());
    }

    Ok(ToolDef {
        name: raw.function.name.clone(),
        description: raw.function.description.clone(),
        parameters: parse_schema(&raw.function.parameters)?,
    })
}

fn build_tool_map(
    raw_tools: &[RawTool],
) -> Result<HashMap<String, ToolDef>, Box<dyn std::error::Error>> {
    let mut map = HashMap::new();

    for raw in raw_tools {
        let tool = build_tool(raw)?;
        map.insert(tool.name.clone(), tool);
    }

    Ok(map)
}

fn render_call(call: &ToolCall) -> String {
    format!(
        "<<call {} {}>>",
        call.name,
        serde_json::to_string(&call.arguments).unwrap()
    )
}

fn render_calls(calls: &[ToolCall]) -> String {
    calls
        .iter()
        .map(render_call)
        .collect::<Vec<_>>()
        .join("\n")
}

fn selected_tools(
    names: &[String],
    all_tools: &HashMap<String, ToolDef>,
) -> Result<Vec<ToolDef>, Box<dyn std::error::Error>> {
    names
        .iter()
        .map(|name| {
            all_tools
                .get(name)
                .cloned()
                .ok_or_else(|| format!("unknown tool in eval set: {}", name).into())
        })
        .collect()
}

fn compact_request(messages: &[Value], tools: &[ToolDef]) -> Value {
    let compact_tools = encode_tools(tools);

    json!({
        "messages": messages,
        "tools": compact_tools,
        "tool_call_format":
            "<<call TOOL_NAME JSON_ARGUMENTS>>"
    })
}
fn baseline_request(messages: &[Value], tools: &[Value]) -> Value {
    json!({
        "messages": messages,
        "tools": tools
    })
}

fn count_tokens(text: &str) -> usize {
    let bpe = o200k_base().expect("failed to load o200k_base tokenizer");
    bpe.encode_with_special_tokens(text).len()
}

fn validate_roundtrip(
    calls: &[ToolCall],
    tools: &[ToolDef],
) -> Result<(), Box<dyn std::error::Error>> {
    for call in calls {
        validate_call(call, tools)?;
    }

    Ok(())
}

fn assert_calls_match(
    case_id: &str,
    expected: &[ToolCall],
    actual: &[ToolCall],
    match_config: Option<&MatchConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    if expected.len() != actual.len() {
        return Err(format!(
            "{} mismatch: expected {} calls, got {}",
            case_id,
            expected.len(),
            actual.len()
        )
        .into());
    }

    let free_text_fields = match_config
        .map(|m| m.free_text_fields.as_slice())
        .unwrap_or(&[]);

    for (expected_call, actual_call) in expected.iter().zip(actual.iter()) {
        if expected_call.name != actual_call.name {
            return Err(format!(
                "{} mismatch: expected tool {}, got {}",
                case_id,
                expected_call.name,
                actual_call.name
            )
            .into());
        }

        let expected_args = expected_call
            .arguments
            .as_object()
            .ok_or_else(|| format!("{} expected arguments must be an object", case_id))?;

        let actual_args = actual_call
            .arguments
            .as_object()
            .ok_or_else(|| format!("{} actual arguments must be an object", case_id))?;

        // Same argument keys are required.
        if expected_args.len() != actual_args.len()
            || expected_args.keys().any(|key| !actual_args.contains_key(key))
        {
            return Err(format!(
                "{} argument keys mismatch:\nexpected: {}\nactual: {}",
                case_id,
                serde_json::to_string_pretty(expected_args)?,
                serde_json::to_string_pretty(actual_args)?
            )
            .into());
        }

        for (key, expected_value) in expected_args {
            let actual_value = &actual_args[key];

            // Free-text fields are intentionally not compared for exact wording.
            if free_text_fields.iter().any(|field| field == key) {  
                continue;
            }

            if expected_value != actual_value {
                return Err(format!(
                    "{} mismatch in argument '{}':\nexpected: {}\nactual: {}",
                    case_id,
                    key,
                    serde_json::to_string_pretty(expected_value)?,
                    serde_json::to_string_pretty(actual_value)?
                )
                .into());
            }
        }
    }

    Ok(())
}

fn decoder_error_code(
    error: &nasiko_tool_compact::CompactError,
) -> &'static str {
    match error {
        nasiko_tool_compact::CompactError::UnknownTool(_) => "unknown_tool",
        nasiko_tool_compact::CompactError::InvalidArguments(_) => {
            "invalid_arguments"
        }
        _ => "invalid_arguments",
    }
}

// fn run_decoder_case(
//     case: &DecoderCase,
//     all_tools: &HashMap<String, ToolDef>,
// ) -> Result<DecoderOutput, Box<dyn std::error::Error>> {
//     let tools = selected_tools(&case.tools, all_tools)?;

//     let mut decoder = StreamDecoder::new();
//     let mut decoded_calls = Vec::new();

//     for chunk in &case.chunks {
//         let calls = decoder.push(chunk)?;

//         for call in calls {
//             validate_call(&call, &tools)?;
//             decoded_calls.push(call);
//         }
//     }

//     let final_calls = decoder.finish()?;

//     for call in final_calls {
//         validate_call(&call, &tools)?;
//         decoded_calls.push(call);
//     }

//     let decoded = if let Some(expected_error) = &case.expected.error {
//         let mut fresh_decoder = StreamDecoder::new();

//         let result = (|| -> Result<Vec<ToolCall>, nasiko_tool_compact::CompactError> {
//             let mut calls = Vec::new();

//             for chunk in &case.chunks {
//                 calls.extend(fresh_decoder.push(chunk)?);
//             }

//             calls.extend(fresh_decoder.finish()?);

//             for call in &calls {
//                 validate_call(call, &tools)?;
//             }

//             Ok(calls)
//         })();

//         match result {
//             Ok(_) => {
//                 return Err(format!(
//                     "{} expected error {}, but decoding succeeded",
//                     case.id, expected_error
//                 )
//                 .into());
//             }
//             Err(error) => json!({
//                 "error": decoder_error_code(&error)
//             }),
//         }
//     } else {
//         json!({
//             "calls": decoded_calls
//         })
//     };

//     Ok(DecoderOutput {
//         id: case.id.clone(),
//         decoded,
//     })
// }



fn run_decoder_case(
    case: &DecoderCase,
    all_tools: &HashMap<String, ToolDef>,
) -> Result<DecoderOutput, Box<dyn std::error::Error>> {
    let tools = selected_tools(&case.tools, all_tools)?;

    let mut decoder = StreamDecoder::new();
    let mut decoded_calls = Vec::new();

    let result = (|| -> Result<Vec<ToolCall>, nasiko_tool_compact::CompactError> {
        for chunk in &case.chunks {
            let calls = decoder.push(chunk)?;

            for call in calls {
                validate_call(&call, &tools)?;
                decoded_calls.push(call);
            }
        }

decoder.finish()?;

        Ok(decoded_calls)
    })();

    let decoded = if let Some(expected_error) = &case.expected.error {
        match result {
            Ok(_) => {
                return Err(format!(
                    "{} expected error {}, but decoding succeeded",
                    case.id, expected_error
                )
                .into());
            }
            Err(error) => {
                let actual_error = decoder_error_code(&error);

                if actual_error != expected_error {
                    return Err(format!(
                        "{} expected error {}, got {}",
                        case.id, expected_error, actual_error
                    )
                    .into());
                }

                json!({
                    "error": actual_error
                })
            }
        }
} else {
    let calls = result?;

    let expected_calls = case
        .expected
        .calls
        .as_ref()
        .ok_or_else(|| format!("{} has no expected calls", case.id))?;

assert_calls_match(
    &case.id,
    expected_calls,
    &calls,
    None,
)?;

    json!({
        "calls": calls
    })
};

    Ok(DecoderOutput {
        id: case.id.clone(),
        decoded,
    })
}


fn call_bedrock(
    base_url: &str,
    model: &str,
    api_key: &str,
    messages: &[Value],
    tools: &[ToolDef],
) -> Result<String, Box<dyn std::error::Error>> {
    let compact_tools = encode_tools(tools);

    let mut input = Vec::new();

   input.push(json!({
    "role": "system",
    "content": format!(
        "You have access to these tools:\n{}\n\n\
         Identify every action requested by the user. \
         Generate one tool call for each required action. \
         If multiple actions are requested, output all corresponding tool calls. \
         Do not omit an action and do not add conversational text. \
         Output only tool calls using exactly this format:\n\
         <<call TOOL_NAME JSON_ARGUMENTS>>",
        compact_tools
    )
}));

    input.extend(messages.iter().cloned());

    let body = json!({
        "model": model,
        "input": input
    });

    let url = format!("{}/responses", base_url.trim_end_matches('/'));

    let client = reqwest::blocking::Client::new();

    let response = client
        .post(url)
        .bearer_auth(api_key)
        .json(&body)
        .send()?;

    let status = response.status();
    let response_json: Value = response.json()?;

    if !status.is_success() {
        return Err(format!(
            "Bedrock request failed: {} {}",
            status,
            serde_json::to_string_pretty(&response_json)?
        )
        .into());
    }


    // Extract the assistant's final text from the Responses API.
    let output = response_json["output"]
        .as_array()
        .ok_or("Bedrock response did not contain output")?;

    for item in output {
        if item["type"] == "message" && item["role"] == "assistant" {
            if let Some(content) = item["content"].as_array() {
                for part in content {
                    if part["type"] == "output_text" {
                        if let Some(text) = part["text"].as_str() {
                            return Ok(text.to_string());
                        }
                    }
                }
            }
        }
    }

    Ok(String::new())
}



fn main() -> Result<(), Box<dyn std::error::Error>> {
    let eval_path = env::var("EVAL_SET")
        .map_err(|_| "EVAL_SET environment variable is required")?;

    let out_path =
        env::var("OUT").map_err(|_| "OUT environment variable is required")?;

    let file = File::open(&eval_path)?;
    let reader = BufReader::new(file);

    let eval: EvalSet = serde_json::from_reader(reader)?;

    println!("Schema version: {}", eval.schema_version);
    println!("Tools: {}", eval.tools.len());
    println!("Cases: {}", eval.cases.len());
    println!("Decoder cases: {}", eval.decoder_cases.len());
let all_tools = build_tool_map(&eval.tools)?;

let live_base_url = env::var("PROVIDER_BASE_URL").ok();
let live_model = env::var("MODEL").ok();
let live_api_key = env::var("API_KEY").ok();

let live_mode =
    live_base_url.is_some() &&
    live_model.is_some() &&
    live_api_key.is_some();

println!("Live mode: {}", live_mode);

let mut output = File::create(&out_path)?;
    // Normal evaluation cases.
    for case in &eval.cases {
        let tools = selected_tools(&case.tools, &all_tools)?;
        let raw_tools = raw_tools_as_values(&case.tools, &eval.tools)?;

let baseline = baseline_request(&case.messages, &raw_tools);

let compact = compact_request(&case.messages, &tools);

let baseline_text = serde_json::to_string(&baseline)?;
let compact_text = serde_json::to_string(&compact)?;

let baseline_tokens = count_tokens(&baseline_text);
let compact_tokens = count_tokens(&compact_text);

let reduction =
    1.0 - (compact_tokens as f64 / baseline_tokens as f64);

println!(
    "{}: baseline={} compact={} reduction={:.2}%",
    case.id,
    baseline_tokens,
    compact_tokens,
    reduction * 100.0
);

        let rendered_calls = render_calls(&case.expected);

let decoded = decode_calls(&rendered_calls)?;

validate_roundtrip(&decoded, &tools)?;

assert_calls_match(
    &case.id,
    &case.expected,
    &decoded,
    case.r#match.as_ref(),
)?;

let mut raw_output = None;
let mut live_calls = None;

if live_mode {
    let model_output = call_bedrock(
        live_base_url.as_ref().unwrap(),
        live_model.as_ref().unwrap(),
        live_api_key.as_ref().unwrap(),
        &case.messages,
        &tools,
    )?;

    println!("--- {} MODEL OUTPUT ---", case.id);
    println!("{}", model_output);

    let calls = decode_calls(&model_output)?;

    validate_roundtrip(&calls, &tools)?;

assert_calls_match(
    &case.id,
    &case.expected,
    &calls,
    case.r#match.as_ref(),
)?;

    println!("LIVE PASS {}", case.id);

    raw_output = Some(model_output);
    live_calls = Some(calls);
}

let output_case = CaseOutput {
    id: case.id.clone(),
    compact_request: compact_request(&case.messages, &tools),
    compacted: true,
    rendered_calls,
    roundtrip_calls: decoded,
    raw_output,
    live_calls,
};
        serde_json::to_writer(&mut output, &output_case)?;
        writeln!(output)?;

      println!("PASS {}", case.id);

    }

    // Streaming decoder cases.
    for case in &eval.decoder_cases {
        let output_case = run_decoder_case(case, &all_tools)?;

        serde_json::to_writer(&mut output, &output_case)?;
        writeln!(output)?;

        println!("PASS {}", case.id);
    }

    println!();
    println!("Evaluation completed successfully.");
    println!("Output: {}", out_path);

    Ok(())
}


fn raw_tools_as_values(
    names: &[String],
    all_raw_tools: &[RawTool],
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    names
        .iter()
        .map(|name| {
            all_raw_tools
                .iter()
                .find(|tool| tool.function.name == *name)
                .map(|tool| serde_json::to_value(tool).unwrap())
                .ok_or_else(|| format!("unknown tool: {}", name).into())
        })
        .collect()
}