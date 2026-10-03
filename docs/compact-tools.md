# Compact tool schemas (`nasiko-tool-compact`)

## Motivation

Native OpenAI-style tool definitions embed full JSON Schema objects in the prompt.
That costs a large share of input tokens on tool-heavy turns.

`nasiko-tool-compact` encodes those schemas into a short, deterministic text form and
decodes model replies back into validated tool calls. **Tool names and argument
semantics must remain unchanged** for clients. Compaction is **opt-in** — existing
router behaviour is the default.

## Compact format

```text
create_calendar_event(title:str!, start:datetime!, duration_min?:int, attendees?:[str], visibility?:public|private)
send_email(to:[str]! "recipient emails", subject:str!, body:str!, cc?:[str])
```

Tool-level descriptions are omitted when they only restate the tool name.
Property descriptions appear as short JSON-quoted hints **only when sibling fields
share the same type and the name alone is ambiguous**. Clear names (`subject`,
`body`, `title`) omit hints; ambiguous same-type pairs keep discriminative words
(`to` → `"recipient emails"`).

## Grammar

### Tool list (encoder → model)

```text
TOOL_LINE  := NAME '(' FIELDS ')' (' - ' DESCRIPTION)?
FIELDS     := FIELD (', ' FIELD)*
FIELD      := NAME ':' TYPE '!' (WS QUOTED_DESC)?   # required
             | NAME '?:' TYPE (WS QUOTED_DESC)?      # optional
QUOTED_DESC:= JSON string literal (short discriminative hint)
TYPE       := 'str' | 'int' | 'float' | 'bool' | 'datetime' | 'null'
             | 'object' | '[' TYPE ']' | '{' FIELDS '}'
             | ENUM_LIT ('|' ENUM_LIT)*
```

### Calls (model → decoder)

```text
CALL        := '<<' TOOL_NAME WS+ JSON_OBJECT '>>'
TOOL_NAME   := [A-Za-z_][A-Za-z0-9_]*
JSON_OBJECT := a single JSON object value
```

Instruction injected with tool lines:

```text
CALL <<name {json}>>
```

Rules:

- Multiple calls: one `<<name {json}>>` each.
- Text before/after markers is ignored.
- No markers ⇒ plain answer ⇒ empty call list (not an error).
- `>>` **inside a JSON string** is literal. The closer is accepted only after the
  JSON object closes (no naive `find(">>")`).
- Escaped quotes (`\"`) and backslashes are handled by a string-aware scanner.

## Required vs optional

- Required: `title:str!`
- Optional: `duration_min?:int`

## Supported types

`str`, `int`, `float`, `bool`, `datetime` (`string` + `format=date-time`), `null`,
arrays (`[str]`), enums (`public|private`), nested objects (`{room?:str}`).

Tool-level descriptions that restate the tool name are dropped. Property hints
are shortened to discriminative content and only kept for ambiguous same-type
fields.

## Safety model

| Situation | Behaviour |
|---|---|
| Unsupported schema (`$ref`, `anyOf`, …) | Encode error → **bypass** compaction; native tools kept |
| Unknown tool name | Reject (`unknown_tool`) — never invent a call |
| Missing required field | Reject (`invalid_arguments`) |
| Invalid type / enum / malformed JSON | Reject (`invalid_arguments` / `malformed_call`) |
| Incomplete stream at `finish()` | Reject (`malformed_call`) — never a partial/guessed call |

The decoder never invents, coerces, or silently drops arguments.

## Gateway name aliases

Nasiko MCP tools are often named `{uuid_hex16}__{suffix}` (digit-leading prefixes
fail compact-identifier rules). The encoder:

1. Recognizes only the exact gateway pattern (16 ASCII hex chars + `__` + non-empty suffix).
2. Exposes a valid compact **alias** (usually the suffix) to the model.
3. Keeps an exact `alias → original` reverse map for decode.

On decode, every call name is looked up in that map. Unknown aliases and missing
reverse entries **fail closed** — no UUID prefix is guessed or reconstructed.
Suffix collisions disambiguate deterministically as `{suffix}_{prefix}`.
Malformed gateway-like names (`__` near-misses, short/non-hex prefixes, digit-leading
suffixes) fail closed at encode time (router bypasses to native tools).

## Native ToolCall passthrough

If the provider already returns OpenAI-shaped `message.tool_calls` / stream
`delta.tool_calls`, the router leaves them unchanged and does not reinterpret
assistant content as compact markers.

## Streaming

`StreamDecoder::push` accepts arbitrary chunk boundaries (marker, name, JSON, and
closing `>>` may all be split). `push` returns newly completed calls; `finish`
returns the full accumulated list or a deterministic error if a call is still open.

## Unsupported schemas

Encoding returns `UnsupportedSchema` for:

`$ref`, `$defs`, `definitions`, `anyOf` / `oneOf` / `allOf` / `not`, conditionals,
`patternProperties`, tuple `items`, `additionalProperties` as a nested schema,
nesting deeper than 8.

The router then leaves the native `tools` array unchanged.

## Configuration

| Env | Default | Effect |
|---|---|---|
| `TOKEN_TOOL_COMPACT=false` | **default** | No compaction; request bytes unchanged |
| `TOKEN_TOOL_COMPACT=true` | opt-in | Compact tools + inject call-format instructions when safe |

`tool_choice` behaviour when compaction is enabled:

| Client `tool_choice` | Behaviour |
|---|---|
| absent / `"auto"` | Compact when schemas supported |
| `"required"` (non-streaming) | Compact + `MUST emit <<name {json}>>.` instruction; router decodes markers → native `tool_calls` |
| `"required"` + `stream: true` | Compact + streaming decode (`StreamDecoder` → native `ToolCallDelta`) |
| specific function force | **Bypass** (native tools kept) |
| unsupported schema | **Bypass** (native tools kept) |

Response-side decode of `<<…>>` → OpenAI `tool_calls` / `ToolCallDelta` is wired for
both non-streaming and streaming chat completions.

## Evaluation

```sh
curl -fsSL https://registry.nasiko.dev/r/nasiko/compact-tools-eval -o /tmp/compact-tools-eval.json
EVAL_SET=/tmp/compact-tools-eval.json OUT=/tmp/out.jsonl \
  cargo run --release -p nasiko-llm-router --example compact_tools_eval
```

| Env | Role |
|---|---|
| `EVAL_SET` | Path to eval JSON (`tools`, `cases`, `decoder_cases`) |
| `OUT` | JSONL output path |
| `PROVIDER_BASE_URL` + `MODEL` | Optional live OpenAI-compatible chat at temperature 0 |
| `PROVIDER_API_KEY` | Optional Bearer token for live mode |

- **Offline** (default): no network, no API key, deterministic — run twice and `diff` `OUT`.
- **Live**: sends each `compact_request` when both URL and model are set.

## Limitations

- Live evaluation requires `PROVIDER_BASE_URL` + `MODEL`.
- Unsupported schemas bypass compaction (native tools kept).
- `"required"` after compaction is prompt-enforced (not provider-enforced).
- Specific function `tool_choice` remains native (cannot name-force via markers).
- Anthropic-specific compact grammar is **not** implemented.
- Field descriptions are shortened (≤40 chars); tool descriptions ≤80 chars.
- Model adherence to `<<name {json}>>` varies; non-grammar surfaces fail closed.
