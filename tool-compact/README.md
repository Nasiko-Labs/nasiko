# nasiko-tool-compact

`nasiko-tool-compact` is a lightweight, high-performance Rust crate for compact representation, encoding, decoding, streaming parsing, and schema validation of LLM tool calls.

---

## Features & Supported Schema Features

### Supported Schema Features
- **Primitive Types**: `string`, `integer`, `number` (float), `boolean`, `null`.
- **Composite Types**: `array` (with element type validation), `object` (with nested property schemas).
- **Constraints & Modifiers**:
  - `required` field flags (e.g. `location:string!`)
  - `default` values (automatically populated if omitted in call)
  - `enum` value validation (e.g. `units: [celsius|fahrenheit]`)
  - Full Unicode and string escape sequence support (`\n`, `\t`, `\"`, `\uXXXX`)
- **JSON Schema Interop**: Conversion from standard OpenAI / Anthropic tool schema JSON objects.
- **Fail-Closed Streaming**: Immediate failure notification on unknown tool identifiers or malformed syntax during stream chunk processing.

### Unsupported Schema Features
- **Union Types**: `anyOf`, `oneOf`, `allOf` (schema variants).
- **Recursive / Cyclic Schemas**: Self-referencing schema structures.
- **Complex Regex / Constraint Specifications**: `pattern`, `multipleOf`, `exclusiveMinimum` (use custom application validation if required).

---

## Token Efficiency

By replacing verbose JSON tool definitions with compact signature prompts:
- **System Prompt Tool Definitions**: ~40% - 60% reduction in token consumption compared to JSON Schema.
- **Tool Invocations**: ~30% - 50% token savings compared to JSON tool call payloads.

---

## Quickstart Usage

### 1. Registering Tools & Encoding System Prompts
```rust
use nasiko_tool_compact::{ToolRegistry, ToolSchema, ParameterSchema, ValueType, encode_schemas};

let mut registry = ToolRegistry::new();

let weather_tool = ToolSchema::new("get_weather", "Get current weather for location")
    .with_parameter(ParameterSchema::new("location", ValueType::String, true))
    .with_parameter(
        ParameterSchema::new("units", ValueType::String, false)
            .with_default("celsius".into())
            .with_enum(vec!["celsius".into(), "fahrenheit".into()])
    );

registry.register(weather_tool);

let system_prompt = encode_schemas(&registry);
println!("{}", system_prompt);
```

### 2. Decoding and Validating Tool Calls
```rust
use nasiko_tool_compact::{decode_call, validate_call, ToolRegistry};

let input = r#"get_weather(location="San Francisco", units="fahrenheit")"#;

let (tool_name, args) = decode_call(input).unwrap();
let validated_args = validate_call(&tool_name, &args, &registry).unwrap();

println!("Tool: {}", tool_name);
println!("Args: {}", validated_args);
```

### 3. Streaming Chunk Processing with Fail-Closed Behavior
```rust
use nasiko_tool_compact::{CompactStreamDecoder, StreamEvent, ToolRegistry};

let registry = ToolRegistry::new(); // populate with schemas
let mut decoder = CompactStreamDecoder::with_registry(registry);

let chunks = vec!["get_wea", "ther(loca", "tion=\"London\")"];

for chunk in chunks {
    let events = decoder.feed_chunk(chunk);
    for event in events {
        match event {
            StreamEvent::ToolStarted { name } => println!("Started tool: {}", name),
            StreamEvent::CallComplete { name, args } => println!("Call complete: {} {:?}", name, args),
            StreamEvent::Error(err) => eprintln!("Error: {:?}", err),
        }
    }
}
```
