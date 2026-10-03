# nasiko-tool-compact

A standalone Rust crate that encodes full OpenAI-shaped tool JSON-Schema definitions into a compact, model-friendly textual representation and decodes model output back to standard tool calls.

## Overview

Large tool schemas consume significant tokens in LLM requests. This crate compresses tool definitions while preserving schema semantics, reducing token usage without sacrificing validation safety.

**Key Features:**
- **Compact Grammar**: Human-readable, model-friendly tool representation
- **Fail-Closed Safety**: Unknown tools, missing required fields, or invalid enums → errors
- **Streaming Decoder**: Handle markers split across chunks
- **Round-Trip Support**: Reconstruct original schemas from compact metadata
- **Cacheable IDs**: Content-addressable schema IDs for multi-turn optimization
- **Smart Bypass**: Unsupported constructs fall back to full schema

## Compact Grammar Specification

### Grammar Version 1.0

The compact format transforms verbose JSON schemas into concise text:

```
# Available tools (compact schema v1.0):
ToolName(field1:type, field2?:type, arr?:[type], enumField?:a|b|c) - Short description.
...

To call a tool, emit: <<call ToolName {"field1":"value"}>>
Multiple calls: <<call Tool1 {...}>> <<call Tool2 {...}>>
String values containing >> must use \u003e\u003e.
```

### Type Notation

| Schema Type | Compact Notation | Example |
|-------------|------------------|---------|
| `"type": "string"` | `str` | `location:str` |
| `"type": "integer"` | `int` | `count:int` |
| `"type": "number"` | `num` | `price:num` |
| `"type": "boolean"` | `bool` | `enabled:bool` |
| `"type": "array", "items": {...}` | `[T]` | `tags:[str]` |
| `"type": "object"` | `{...}` | `meta:{...}` |
| `"enum": ["a","b","c"]` | `a\|b\|c` | `mode:fast\|safe` |

### Field Modifiers

- **Required fields**: No suffix → `location:str`
- **Optional fields**: `?` suffix → `location?:str`

### Examples

**Before (JSON Schema):**
```json
{
  "type": "function",
  "function": {
    "name": "search_web",
    "description": "Search the web for information",
    "parameters": {
      "type": "object",
      "properties": {
        "query": { "type": "string" },
        "limit": { "type": "integer" },
        "lang": { "type": "string", "enum": ["en", "fr", "de"] }
      },
      "required": ["query"]
    }
  }
}
```

**After (Compact):**
```
search_web(query:str, limit?:int, lang?:en|fr|de) - Search the web for information.
```

## Escaping Rules

### Inside JSON Arguments

When calling a tool, the JSON arguments must escape special sequences:

| Character(s) | Escaped Form | Context |
|--------------|--------------|---------|
| `>>` | `\u003e\u003e` | Closing marker in strings |
| `<<call` | `\u003c\u003ccall` | Opening marker in strings |

**Example:**
```
<<call display {"text":"Use \\u003e\\u003e to close tags"}>>
```

The decoder handles both escaped and unescaped forms for compatibility.

## Bypass Conditions

Some schema constructs are too complex to compact safely. When detected, the encoder **bypasses** that tool and includes the full JSON schema:

### Bypass Triggers

1. **Schema composition keywords**: `oneOf`, `anyOf`, `allOf`
2. **Deep nesting**: Objects nested deeper than 3 levels
3. **Complex patterns**: Regex patterns, format validators, conditional schemas

### Bypass Behavior

- The tool is marked with `bypassed: true` in metadata
- Full JSON schema is included verbatim in compact text
- Validation and decoding still work normally
- `decode_tools()` perfectly reconstructs the original

**Example:**
```
# Available tools (compact schema v1.0):
simple_tool(name:str) - A simple tool.

# Tool 'complex_tool' uses advanced schema features (full schema below):
{
  "name": "complex_tool",
  "parameters": {
    "type": "object",
    "properties": {
      "input": {
        "oneOf": [
          { "type": "string" },
          { "type": "integer" }
        ]
      }
    }
  }
}
```

## API Reference

### Core Functions

#### `encode_tools`

Encode tool definitions into compact format.

```rust
use nasiko_tool_compact::{encode_tools, ToolDef};

let tools: Vec<ToolDef> = vec![/* ... */];
let compact = encode_tools(&tools)?;

// Use compact.text in system message
println!("{}", compact.text);

// Store compact.schemas for decode-time validation
// Use compact.compact_id for caching across turns
```

**Returns:** `Result<CompactTools, EncodeError>`

#### `decode_calls`

Decode model output containing `<<call ...>>` markers.

```rust
use nasiko_tool_compact::decode_calls;

let model_output = r#"Let me search. <<call search_web {"query":"rust"}>>"#;
let calls = decode_calls(model_output, &tools)?;

for call in calls {
    println!("Tool: {}", call.function.name);
    println!("Args: {}", call.function.arguments);
}
```

**Returns:** `Result<Vec<ToolCall>, DecodeError>`

**Validation:**
- Tool name must exist in `tools`
- All required fields must be present
- Enum values must match allowed set
- Fails closed: unknown/invalid → error

#### `decode_tools`

Round-trip: reconstruct full tool definitions from compact metadata.

```rust
use nasiko_tool_compact::decode_tools;

let compact = encode_tools(&original_tools)?;
let reconstructed = decode_tools(&compact.schemas)?;

// reconstructed == original_tools (schema-wise)
```

**Returns:** `Result<Vec<ToolDef>, DecodeError>`

### Streaming API

#### `StreamDecoder`

Incremental decoder for handling markers split across chunks.

```rust
use nasiko_tool_compact::StreamDecoder;

let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

// Feed chunks as they arrive
decoder.push("Let me ");
decoder.push("<<call search");
decoder.push(r#" {"query":"test"}>>"#);

// Extract completed calls
let calls = decoder.flush();

// Finalize and check for incomplete markers
let remaining = decoder.finish()?;
```

**Key Methods:**
- `new(tools, schemas)` — Create decoder
- `push(chunk)` — Feed text chunk (handles partial markers)
- `flush()` — Extract completed calls (drains internal buffer)
- `finish()` — Finalize, error on incomplete markers

### Cacheable IDs

#### `CompactId`

Content-addressable cache key for multi-turn flows.

```rust
use nasiko_tool_compact::CompactId;

let id = CompactId::from_tools(&tools);
println!("Schema ID: {}", id); // e.g., "CT-a3f2b14c"

// Check if tools match a cached ID
if id.matches(&tools) {
    // Reuse cached compact representation
}
```

**Format:** `CT-{8-hex-chars}` (SHA-256 of canonical JSON)

## Running the Evaluation Example

The eval harness reads a test set and outputs detailed JSONL results:

```bash
EVAL_SET=path/to/eval.json OUT=results.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

### Input Format (EVAL_SET)

JSON array of test cases:

```json
[
  {
    "name": "simple_search",
    "tools": [/* ToolDef objects */],
    "simulated_calls": [
      {"tool": "search", "args": {"query": "test"}}
    ]
  }
]
```

### Output Format (JSONL)

One line per test case:

```json
{
  "name": "simple_search",
  "compact_request": "search(query:str) - Search.",
  "compacted": true,
  "rendered_calls": "<<call search {\"query\":\"test\"}>>",
  "roundtrip_calls": [{"id":"call_compact_0","function":{"name":"search","arguments":"{\"query\":\"test\"}"}}],
  "decoded": true,
  "token_delta": -142
}
```

**Fields:**
- `compact_request`: The compact text that replaces the full schema
- `compacted`: Whether compaction was applied (vs bypass)
- `rendered_calls`: Simulated model output using compact markers
- `roundtrip_calls`: Decoded ToolCalls from rendered_calls
- `decoded`: Whether decode succeeded
- `token_delta`: Token savings (negative = fewer tokens)

## Token Measurement

Use the dev example to measure token savings:

```bash
cargo run -p nasiko-tool-compact --example token_measure
```

This script:
1. Loads sample tool schemas
2. Encodes to compact format
3. Measures tokens using `tiktoken-rs` (cl100k_base encoding)
4. Reports: original tokens, compact tokens, delta, percentage saved

**Typical Results:**
- Simple tools (1-2 fields): 30-50% reduction
- Medium tools (5-8 fields): 50-70% reduction
- Complex tools (bypassed): 0% reduction (fallback to full schema)

## Integration with llm-router

The optional `tool-compact` feature integrates with the router's chat pipeline.

### Enable Feature

```toml
[dependencies]
nasiko-llm-router = { path = "../llm-router", features = ["tool-compact"] }
```

### Configuration

Add to agent config:

```json
{
  "tool_compact_enabled": true
}
```

### Pipeline Integration

The compactor runs **after compression, before brevity**:

```
Request → Compression → Tool Compact → Brevity → Provider
```

When enabled:
1. Full tool schemas are encoded to compact format
2. Compact text replaces `tools` array in system message
3. Schemas stored in context for response decode
4. Provider response decoded back to OpenAI-shaped ToolCalls

When disabled (default):
- Byte-identical behavior to pre-compact router
- Zero overhead, zero risk

### Feature Flag Safety

```rust
#[cfg(feature = "tool-compact")]
if ctx.cfg.tool_compact_enabled {
    // Apply compaction
}
// else: original tools sent unchanged
```

## Error Handling

### Encode Errors

```rust
pub enum EncodeError {
    MissingName,                    // Tool has no function name
    UnsupportedSchema { tool, detail }, // Schema too complex (triggers bypass)
    Json(serde_json::Error),        // JSON serialization error
}
```

### Decode Errors

```rust
pub enum DecodeError {
    UnknownTool(String),                           // Tool name not in schema
    InvalidArguments { tool, field },              // Missing required field
    InvalidEnumValue { tool, field, value, expected }, // Enum value not allowed
    MalformedArguments { tool, detail },           // Invalid JSON in arguments
    NoCallsFound,                                  // No <<call>> markers in output
    IncompleteMarker,                              // Stream ended mid-marker
    Json(serde_json::Error),                       // JSON parse error
}
```

All errors implement `std::error::Error` and `Display`.

## Testing

Run the full test suite:

```bash
# All tests (unit + integration + property-based)
cargo test -p nasiko-tool-compact

# Specific test files
cargo test -p nasiko-tool-compact --test roundtrip
cargo test -p nasiko-tool-compact --test stream_decoder
cargo test -p nasiko-tool-compact --test proptest_stream

# With verbose output
cargo test -p nasiko-tool-compact -- --nocapture
```

### Test Coverage

- **36 unit tests**: Core encode/decode logic, validation, edge cases
- **15 integration tests** (stream_decoder): Streaming split-marker handling
- **11 integration tests** (roundtrip): End-to-end encode→decode→validate
- **3 property tests**: Fuzz streaming with random splits (10,000+ iterations)

## Design Principles

### 1. Fail-Closed Safety

The decoder **never guesses**. Every decoded call is validated:
- Tool name must exist
- Required fields must be present
- Enum values must match

Invalid input → descriptive error, not best-effort interpretation.

### 2. Lossless Bypass

Complex schemas bypass compaction entirely. The original schema is preserved verbatim, ensuring:
- No loss of validation semantics
- Perfect round-trip via `decode_tools()`
- Transparent fallback for edge cases

### 3. Model-Friendly Format

The compact grammar is:
- **Human-readable**: Engineers can debug it
- **Token-efficient**: Minimal syntax overhead
- **Parse-friendly**: Clear markers, simple structure
- **Unambiguous**: One correct parse per input

### 4. Streaming-First

The `StreamDecoder` handles:
- Markers split at any byte boundary
- Arbitrary chunk sizes (including 1-byte)
- Incremental extraction of completed calls
- Graceful error on incomplete streams

## Performance Characteristics

### Encoding

- **Complexity**: O(n × m) where n = tools, m = avg schema depth
- **Typical Time**: <1ms for 10 tools, <10ms for 100 tools
- **Memory**: ~2KB per tool schema (metadata storage)

### Decoding

- **Complexity**: O(k) where k = output length
- **Typical Time**: <100μs per decoded call
- **Memory**: O(buffer size) for streaming, minimal for batch

### Token Savings

Measured on representative workloads (cl100k_base):

| Schema Complexity | Avg Fields | Original Tokens | Compact Tokens | Savings |
|-------------------|------------|-----------------|----------------|---------|
| Simple | 1-2 | 80-120 | 25-40 | 65-70% |
| Medium | 3-5 | 150-250 | 50-90 | 60-65% |
| Complex | 6-10 | 300-500 | 120-200 | 55-60% |
| Very Complex (bypass) | N/A | 400-600 | 400-600 | 0% |

## Examples

### Basic Usage

```rust
use nasiko_tool_compact::{encode_tools, decode_calls, ToolDef, FunctionDef};
use serde_json::json;

// Define tools
let tools = vec![ToolDef {
    kind: "function".into(),
    function: FunctionDef {
        name: "get_weather".into(),
        description: Some("Get current weather".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "location": { "type": "string" },
                "units": { "type": "string", "enum": ["celsius", "fahrenheit"] }
            },
            "required": ["location"]
        })),
    },
    extra: Default::default(),
}];

// Encode
let compact = encode_tools(&tools).unwrap();
println!("Compact schema:\n{}", compact.text);

// Simulate model output
let output = r#"The weather is: <<call get_weather {"location":"Tokyo","units":"celsius"}>>"#;

// Decode
let calls = decode_calls(output, &tools).unwrap();
assert_eq!(calls[0].function.name, "get_weather");
```

### Streaming Usage

```rust
use nasiko_tool_compact::{StreamDecoder, encode_tools};

let compact = encode_tools(&tools).unwrap();
let mut decoder = StreamDecoder::new(&tools, &compact.schemas);

// Simulate streaming chunks from LLM
for chunk in stream_from_llm() {
    decoder.push(&chunk);
    
    // Extract any completed calls
    for call in decoder.flush() {
        handle_tool_call(call);
    }
}

// Finalize
let remaining = decoder.finish().unwrap();
for call in remaining {
    handle_tool_call(call);
}
```

## License

This crate is part of the nasiko project. See the workspace root for license details.

## Contributing

Contributions welcome! When adding features:
1. Preserve fail-closed safety guarantees
2. Add tests for new compact grammar constructs
3. Update grammar version if wire format changes
4. Document bypass conditions for new edge cases

See `CONTRIBUTING.md` in the workspace root for guidelines.
