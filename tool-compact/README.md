# nasiko-tool-compact

`nasiko-tool-compact` provides compact, token-efficient tool schema representations and streaming decoders for LLM tool calling in Nasiko.

## The Problem

Traditional OpenAI/JSON Schema function definitions can consume hundreds to thousands of tokens before an LLM even begins generating output. In agentic workflows with dozens of tools, tool schemas often represent 50–70% of the prompt token budget.

## The Solution

`nasiko-tool-compact` compresses verbose JSON Schema definitions into a concise, human-and-LLM-readable specification injected directly into the system prompt. It instructs the model to call tools using explicit delimiter markers:

```text
<<call tool_name {"param1":"value1","param2":123}>>
```

### Key Highlights

- **30%–50% Token Reduction**: Eliminates verbose JSON Schema metadata boilerplate (`$schema`, nested `properties`, verbose type descriptors) while retaining full type fidelity, field descriptions, and enum constraints.
- **Robust Streaming Parser**: `StreamDecoder` incrementally consumes raw token deltas as they stream from the LLM, emitting regular text immediately and capturing complete tool calls without buffering whole responses.
- **Escape- & String-Safe Lexing**: Correctly parses `>>` when inside string arguments, preserving JSON integrity and preventing truncation or desynchronization.
- **Schema Validation**: Validates required parameters, type conformity (string, integer, float, boolean, array, object), and enum constraints before returning typed `ToolCall` records.

## Usage

### 1. Generating Compact Tool Schemas

```rust
use nasiko_tool_compact::{ToolDef, ParamDef, ParamSchema, encode_system_prompt_block};

let tools = vec![
    ToolDef {
        name: "get_weather".to_string(),
        description: "Fetch current weather for a city".to_string(),
        parameters: vec![
            ParamDef {
                name: "city".to_string(),
                description: Some("Target city name".to_string()),
                schema: ParamSchema::Str { enum_values: None },
                required: true,
            },
        ],
    },
];

let system_instruction = encode_system_prompt_block(&tools).unwrap();
```

### 2. Decoding Complete LLM Responses

```rust
use nasiko_tool_compact::decode_calls;

let response = r#"Checking the forecast now: <<call get_weather {"city":"Tokyo"}>>"#;
let calls = decode_calls(response, &tools).unwrap();

assert_eq!(calls.len(), 1);
assert_eq!(calls[0].name, "get_weather");
assert_eq!(calls[0].arguments["city"], "Tokyo");
```

### 3. Streaming Mode

```rust
use nasiko_tool_compact::{StreamDecoder, StreamOutput};

let mut decoder = StreamDecoder::new(&tools);

let chunk = "Here is the result: <<call get_weather {\"city\":\"Tokyo\"}>> Done!";
let output = decoder.feed(chunk).unwrap();

for item in output {
    match item {
        StreamOutput::Text(t) => print!("{t}"),
        StreamOutput::Call(call) => println!("[Executing tool: {}]", call.name),
    }
}
```

## Running the Evaluation Benchmark

Run the evaluation harness:

```bash
cargo run --example compact_tools_eval -p nasiko-llm-router
```
