# nasiko-tool-compact (`[compact-tools]`)

Compress OpenAI-shaped JSON Schema tool definitions into compact prompt signatures, instruct models to invoke tools using deterministic stream markers (`<<call name {json}>>`), and reliably decode streaming LLM output back to standard `ToolCall` records without breaking tool calls.

---

## 1. Problem and Goal

Standard OpenAI-style JSON Schema tool definitions carry severe token overhead—verbose metadata, repetitive keywords (`"type"`, `"properties"`), redundant descriptions, and boilerplate object syntax consume hundreds of prompt tokens per request before a conversation even begins. `nasiko-tool-compact` eliminates this tax while adhering strictly to the core invariant: **compress representation, never semantics**. The compact representation preserves exact types, nullability, required constraints, nested objects, arrays, string formats, and enums, and fails closed onto native tool schemas if any unsupported schema keyword is encountered.

---

## 2. Architecture & Pipeline

```text
 ┌──────────────────────┐
 │  Vec<ToolDef>        │ (OpenAI-style tool definitions with JSON Schema)
 └──────────┬───────────┘
            │
            ▼
 ┌──────────────────────┐
 │  encode_tools()      │─── [Unsupported Schema] ──► Bypass Compaction (Native Tools)
 └──────────┬───────────┘
            │
            ▼
 ┌──────────────────────┐
 │  CompactTools        │ (Definitions in system message + Instruction text)
 └──────────┬───────────┘
            │
            ▼
 ┌──────────────────────┐
 │  Model Stream        │ (Emits prose and/or: <<call name {json args}>>)
 └──────────┬───────────┘
            │
            ▼
 ┌──────────────────────┐
 │  StreamDecoder       │ (Deterministic state machine; chunk-invariant)
 └──────────┬───────────┘
            │
            ▼
 ┌──────────────────────┐
 │  Strict Validation   │ (Canonical schema checks: types, enums, formats, required)
 └──────────┬───────────┘
            │
            ▼
 ┌──────────────────────┐
 │  Vec<ToolCall>       │ (Standard tool call records ready for tool execution)
 └──────────────────────┘
```

---

## 3. Compact Definition Grammar

Each tool definition is encoded as a concise function signature on its own line:

```text
tool_line   = name "(" [ field_list ] ")" [ " - " description ]
field_list  = field ( ", " field )*
field       = name [ "?" ] ":" type [ " " '"' description '"' ]
type        = "str" | "int" | "num" | "bool" | "null"
            | "datetime" | "date" | "time" | "email" | "uri" | "uuid"
            | enum_type | array_type | object_type
enum_type   = ident ( "|" ident )*
array_type  = "[" type "]"
object_type = "{" [ field_list ] "}"
```

### Full Example
**Original OpenAI JSON Schema (282 tokens):**
```json
{
  "type": "function",
  "function": {
    "name": "create_calendar_event",
    "description": "Create an event in the user's calendar.",
    "parameters": {
      "type": "object",
      "properties": {
        "title": { "type": "string" },
        "start": { "type": "string", "format": "date-time", "description": "Start time, ISO 8601" },
        "duration_min": { "type": "integer", "description": "Duration in minutes" },
        "attendees": { "type": "array", "items": { "type": "string" }, "description": "Attendee emails" },
        "visibility": { "type": "string", "enum": ["public", "private"] }
      },
      "required": ["title", "start"],
      "additionalProperties": false
    }
  }
}
```

**Compact Representation (168 tokens, 40.4% reduction):**
```text
create_calendar_event(attendees?:[str] "Attendee emails", duration_min?:int "Duration in minutes", start:datetime "Start time, ISO 8601", title:str, visibility?:public|private) - Create an event in the user's calendar.
To call a tool, emit: <<call name {json args}>>
```

---

## 4. Call Grammar & Decoding Behavior

```text
output      = { text | call }
call        = "<<call" WS+ name WS* json_object WS* ">>"
json_object = strict RFC 8259 JSON object
```

- **Marker Detection**: Matches exact ASCII `<<call` preceded by arbitrary text. Text before and after calls is ignored.
- **`>>` in Strings**: String scanner tracks quotes and escapes; a literal `>>` inside a JSON string value (`{"subject": "a >> b"}`) **never** closes the call prematurely.
- **Multiple Calls**: Sequential or spaced calls are each parsed and emitted in order.
- **Conversational Prose**: Conversational answers wrapping tool calls are permitted; if no tool call marker appears, zero calls are returned without error.

---

## 5. Strict Validation Rules

Completed calls are parsed with strict JSON rules and validated against original schemas:

| Check | Behavior |
| :--- | :--- |
| **Tool Name** | Must match known tool exactly (`unknown_tool` error). |
| **Duplicate Keys** | Explicitly rejected at all object nesting levels (`{"a":1,"a":2}`). |
| **Extra Fields** | Enforces `additionalProperties: false` (unknown arguments rejected). |
| **Integer Strictness** | Strict lexical integers (`30` accepted; `30.0` and `3e1` rejected). |
| **String Formats** | RFC 3339 validation for `date-time`, `date`, `time`; format checks for `email`, `uri`, `uuid`. |
| **Enums** | Exact case-sensitive match against permitted variants. |
| **Limits** | Max call size 256 KiB; max nesting depth 64; max 128 calls per message. |

---

## 6. Supported Schemas & Fail-Closed Bypass Policy

### Supported Constructs
- Primitive types: `string`, `integer`, `number`, `boolean`, `null`.
- Supported string formats: `date-time`, `date`, `time`, `email`, `uri`, `uuid`.
- String enum arrays: `["val1", "val2"]`.
- Arbitrary object nesting: `{ type: "object", properties: { ... } }`.
- Arbitrary array items: `{ type: "array", items: { ... } }`.
- Required arrays and optional field omission.
- Description normalization: rule R1 drops repetitive type/name descriptions while preserving informative context.

### Unsupported Schemas (Bypass Compaction)
Any schema containing:
- Conditionals / composition: `anyOf`, `oneOf`, `allOf`, `not`, `if`, `then`, `else`, `dependentRequired`, `dependentSchemas`
- References: `$ref`, `$defs`, `definitions`
- Constraints: `pattern`, `minimum`, `maximum`, `minLength`, `maxLength`, `minItems`, `maxItems`, `uniqueItems`, `default`, `patternProperties`
- Permissive objects: `additionalProperties: true` or custom schema object

**Bypass Rule**: If ANY tool in a request uses an unsupported keyword, `encode_tools()` returns `Err(Error::UnsupportedSchema)`. The caller bypasses compaction completely, passing native OpenAI `tools` with 0% token reduction. **Never silently approximate or lose schema constraints.**

---

## 7. Streaming Guarantees

1. **Single Implementation**: Whole-text `decode_calls(text, tools)` is implemented strictly as `StreamDecoder::new(tools)` $\to$ `push(text)` $\to$ `finish()`.
2. **Chunk Invariance**: Output is identical whether input is fed as one large chunk, line by line, or 1-byte splits across marker and JSON boundaries.
3. **State Poisoning**: If an error occurs, the decoder enters a terminal `Poisoned` state; all subsequent operations fail with the identical error.
4. **Fail-Closed Truncation**: A stream ending inside an unclosed call returns `Err(Error::UnterminatedCall)` (maps to `invalid_arguments`).

---

## 8. Error Codes & Mapping

Per the buildathon evaluation contract, errors map to exactly two stable error codes:

| Internal Error | Evaluator Code |
| :--- | :--- |
| `Error::UnknownTool` | `"unknown_tool"` |
| `Error::InvalidArguments` | `"invalid_arguments"` |
| `Error::DuplicateKey` | `"invalid_arguments"` |
| `Error::MalformedCall` | `"invalid_arguments"` |
| `Error::UnterminatedCall` | `"invalid_arguments"` |
| `Error::LimitExceeded` | `"invalid_arguments"` |

---

## 9. Determinism

- **Zero HashMap Iteration**: All internal maps use `IndexMap` or `BTreeMap` and vector traversals.
- **Byte-for-Byte Reproducibility**: Running `compact_tools_eval` repeatedly produces byte-identical JSONL output (`cmp -s` verified).
- **Offline Integrity**: Default mode operates with zero network access and deterministic timestamps.

---

## 10. Measured Results

Measured with `tiktoken-rs = "=0.5.9"` using `o200k_base` on full JSON-serialized requests:

### Token Reduction
- **Official Sample Suite** (`compact-tools-eval.json`, 3 cases):
  - Baseline: 743 tokens $\to$ Compact: 461 tokens $\implies$ **38.0% reduction** (0/3 bypassed)
- **Anti-Overfitting Corpus** (`corpus-benchmark.json`, 15 cases):
  - Baseline: 2,394 tokens $\to$ Compact: 1,657 tokens $\implies$ **30.8% reduction** (2/15 bypassed fail-closed)
  - Compacted cases only: **33.7% reduction**
  - Multi-tool prompt (`bench-13`): **49.2% reduction** (329 $\to$ 167 tokens)
- **Combined Aggregate (18 cases)**:
  - Baseline: 3,137 tokens $\to$ Compact: 2,118 tokens $\implies$ **32.5% net token reduction**

### Live Model Adherence
Evaluated live via hackathon Bedrock endpoint (`https://bedrock-mantle.us-east-1.api.aws/v1`, temperature 0):
- **Models Verified**: `deepseek.v3.2` and `mistral.mistral-large-3-675b-instruct`.
- **Single-Tool Adherence (`ct-001`)**: **100%** correct call with valid ISO 8601 timestamps and parameters.
- **Multi-Tool Adherence (`ct-002`)**: **100%** sequential call execution (`send_email` + `create_calendar_event`).
- **Plain-Answer Adherence (`ct-003`)**: **100%** conversational response with 0 hallucinated calls.
- **Fail-Closed Validation**: Invalid datetime formatting without timezone strictly rejected with `invalid_arguments`.

---

## 11. Running the Evaluator & Tests

### Offline Evaluator
```bash
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

### With Token Measurement Summary
```bash
REPORT=1 EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

### Live Model Adherence Mode
```bash
PROVIDER_BASE_URL="https://bedrock-mantle.us-east-1.api.aws/v1" \
PROVIDER_API_KEY="<YOUR_KEY>" \
MODEL="deepseek.v3.2" \
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/live_out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

### Running Test Suite
```bash
cargo test -p nasiko-tool-compact
```
*(57 tests: 32 unit tests, 20 decoder streaming edge cases, 5 property & round-trip suites).*
