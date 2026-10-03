# nasiko-tool-compact

Compact tool schemas for LLM prompt-token reduction.

Native OpenAI `tools` arrays are large JSON Schema documents. This crate encodes them into a short, deterministic text form used **between the Nasiko LLM router and the model**, then decodes model text back into validated tool calls. Clients continue to see standard OpenAI-compatible `tool_calls`.

## Why

Tool definitions dominate prompt tokens for agentic workloads. Compacting schemas (while preserving names, types, required/optional, enums, nesting, and argument validity) cuts that overhead without changing the client wire format.

## Grammar

### Signatures

```text
create_calendar_event(attendees?:[str], duration_min?:int, start:datetime, title:str, visibility?:public|private) - Create an event in the user's calendar.
send_email(body:str, subject:str, to:[str]) - Send an email.
```

Property names are sorted for determinism. Optional fields use `?`.

### Calls

```text
<<call TOOL_NAME JSON_ARGUMENTS>>
```

Example:

```text
<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>
```

The closing `>>` is recognized only outside JSON strings, so argument values may contain `>>`.

## API

```rust
use nasiko_tool_compact::{encode_tools, decode_calls, decode_tools, StreamDecoder, ToolDef};

let compact = encode_tools(&tools)?;
let calls = decode_calls(model_text, &tools)?;
let mut stream = StreamDecoder::new(&tools);
stream.push(chunk)?;
let calls = stream.finish()?;
```

## Fail-closed validation

| Condition | Error label |
|-----------|-------------|
| Unknown tool name | `unknown_tool` |
| Missing required / bad type / bad enum / extra fields | `invalid_arguments` |
| Malformed marker / incomplete JSON | `malformed_call` / `invalid_json` |
| `anyOf` / `$ref` / recursive / similar | `unsupported_schema` |

Unsupported schemas must **bypass** compaction at the router — never silently approximate.

## Extra fields policy

Additional properties are **rejected** unless the original schema sets `"additionalProperties": true`.

## Configuration (router)

| Setting | Default | Env |
|---------|---------|-----|
| `GatewayConfig::compact_tools_enabled` | `false` | `TOKEN_COMPACT_TOOLS=true` |
| Reference date (optional) | unset | `TOKEN_COMPACT_TOOLS_REF_DATE` |
| Reference timezone (optional) | unset | `TOKEN_COMPACT_TOOLS_REF_TZ` |

When disabled, the chat request path is unchanged (regression-tested). When enabled, eligible `tools` are replaced with the compact prompt; decoded `<<call>>` markers become standard OpenAI `tool_calls` for the client (non-streaming path). Streaming requests bypass compaction until response rehydration is wired.

Eval harness reference defaults (overridable): `COMPACT_REF_DATE=2026-10-02`, `COMPACT_REF_TZ=Asia/Kolkata`.

## Tests

```bash
cargo test -p nasiko-tool-compact
cargo test -p nasiko-llm-router tool_compact
cargo clippy -p nasiko-tool-compact -p nasiko-llm-router --all-targets -- -D warnings
```

## Evaluation

```bash
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

Optional live mode (never required): set `PROVIDER_BASE_URL`, `MODEL`, and optionally `PROVIDER_API_KEY`.

## Measured token reduction (public eval, o200k_base)

| Case | Native → Compact | Reduction |
|------|------------------|-----------|
| ct-001 | 446 → 214 | **52.0%** |
| ct-002 | 455 → 223 | **51.0%** |
| ct-003 | 240 → 165 | **31.3%** |
| Aggregate | 1141 → 602 | **47.2%** |

Prior baseline (shorter instructions, no param hints): 66.6% / 65.3% / 52.9%. Current prompt trades some savings for clearer live call instructions + reference-time context (reference preamble excluded from the reduction metric).

## Limitations

- Only string enums are supported in compact form.
- `anyOf` / `oneOf` / `allOf` / `$ref` / constraint keywords (`pattern`, `minimum`, …) / `additionalProperties` schema objects are unsupported (bypass).
- Tool/param descriptions are clipped in the signature line; full structure is retained via `decode_tools`.
- Router streaming response → native `tool_calls` rehydration is not wired yet; streaming requests bypass compaction. The library `StreamDecoder` is streaming-safe. Compact mode defaults off.
